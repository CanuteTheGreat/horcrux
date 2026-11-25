//! iSCSI initiator side of the SAN: portal parsing, SendTargets discovery
//! (every advertised portal), node-record settings, login/logout on every
//! path and session inspection (`iscsiadm -m session -P 3`).

use super::IscsiTargetSpec;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Duration;

pub const DEFAULT_PORT: u16 = 3260;

/// `host:port` of an iSCSI portal; IPv6 hosts are bracketed.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct Portal {
    pub host: String,
    pub port: u16,
}

impl Portal {
    /// `10.0.0.1`, `10.0.0.1:3261`, `san.example`, `[fd00::1]`, `[fd00::1]:3261`,
    /// bare `fd00::1`; a `,tpgt` suffix (as iscsiadm prints) is ignored.
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        let s = s.split(',').next().unwrap_or("");
        let bad = || format!("portal {s:?}: use host, host:port or [ipv6]:port");
        if s.is_empty() || s.len() > 270 || s.chars().any(|c| c.is_whitespace()) {
            return Err(bad());
        }
        let (host, port) = if let Some(rest) = s.strip_prefix('[') {
            let (h, after) = rest.split_once(']').ok_or_else(bad)?;
            let port = match after.strip_prefix(':') {
                Some(p) => p.parse().map_err(|_| bad())?,
                None if after.is_empty() => DEFAULT_PORT,
                None => return Err(bad()),
            };
            h.parse::<std::net::Ipv6Addr>().map_err(|_| bad())?;
            (h.to_string(), port)
        } else if s.matches(':').count() > 1 {
            // bare IPv6
            s.parse::<std::net::Ipv6Addr>().map_err(|_| bad())?;
            (s.to_string(), DEFAULT_PORT)
        } else if let Some((h, p)) = s.split_once(':') {
            (h.to_string(), p.parse().map_err(|_| bad())?)
        } else {
            (s.to_string(), DEFAULT_PORT)
        };
        if port == 0
            || host.is_empty()
            || !host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_'))
        {
            return Err(bad());
        }
        Ok(Self { host, port })
    }
}

impl fmt::Display for Portal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.host.contains(':') {
            write!(f, "[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{}:{}", self.host, self.port)
        }
    }
}

/// One target as SendTargets discovery reported it, with every portal it
/// advertised (possibly from several discovery portals).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredTarget {
    pub iqn: String,
    pub portals: Vec<String>,
}

/// Parse `iscsiadm -m discovery -t st` output
/// (`10.0.0.1:3260,1 iqn....` / `[fd00::1]:3260,1 iqn....`), grouping portals
/// per IQN in first-seen order and dropping duplicates. Lines that are not
/// records (warnings) are skipped.
pub fn parse_discovery(text: &str) -> Vec<DiscoveredTarget> {
    let mut out: Vec<DiscoveredTarget> = Vec::new();
    merge_discovery(&mut out, text);
    out
}

/// [`parse_discovery`] into an existing list (discovery through several
/// portals of the same array).
pub fn merge_discovery(out: &mut Vec<DiscoveredTarget>, text: &str) {
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(p), Some(iqn), None) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        if !super::valid_iqn(iqn) {
            continue;
        }
        let Ok(portal) = Portal::parse(p) else {
            continue;
        };
        let portal = portal.to_string();
        match out.iter_mut().find(|t| t.iqn == iqn) {
            Some(t) => {
                if !t.portals.contains(&portal) {
                    t.portals.push(portal);
                }
            }
            None => out.push(DiscoveredTarget {
                iqn: iqn.to_string(),
                portals: vec![portal],
            }),
        }
    }
}

/// The portals a target should be logged in on, given what discovery
/// found: every advertised portal (plus the configured ones), or only the
/// configured ones when `use_advertised_portals` is off.
pub fn login_portals(t: &IscsiTargetSpec, discovered: &[DiscoveredTarget]) -> Vec<String> {
    let mut out: Vec<String> = t.portals.clone();
    if t.use_advertised_portals {
        if let Some(d) = discovered.iter().find(|d| d.iqn == t.iqn) {
            for p in &d.portals {
                if !out.contains(p) {
                    out.push(p.clone());
                }
            }
        }
    }
    out
}

/// A disk attached through a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionDisk {
    pub lun: u32,
    /// Kernel name (`sdb`).
    pub dev: String,
    /// SCSI device state (`running`, `blocked`, `transport-offline`).
    pub state: String,
}

/// One iSCSI session (one path).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Session {
    pub iqn: String,
    /// Portal in use (`10.0.0.1:3260`).
    pub portal: String,
    pub sid: Option<u32>,
    /// Local address of the connection.
    pub local_address: Option<String>,
    pub transport: Option<String>,
    /// `LOGGED IN`, `FAILED`, ...
    pub connection_state: Option<String>,
    /// `LOGGED_IN`, `FAILED`, `FREE`.
    pub session_state: Option<String>,
    pub scsi_host: Option<u32>,
    pub disks: Vec<SessionDisk>,
}

impl Session {
    pub fn logged_in(&self) -> bool {
        self.session_state.as_deref() == Some("LOGGED_IN")
    }
}

fn after<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.trim().strip_prefix(key).map(str::trim)
}

/// Parse `iscsiadm -m session -P 3`.
pub fn parse_sessions(text: &str) -> Vec<Session> {
    let mut out: Vec<Session> = Vec::new();
    let mut iqn = String::new();
    let mut lun: Option<u32> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(v) = after(line, "Target:") {
            iqn = v.split_whitespace().next().unwrap_or("").to_string();
            continue;
        }
        if let Some(v) = after(line, "Current Portal:") {
            out.push(Session {
                iqn: iqn.clone(),
                portal: Portal::parse(v)
                    .map(|p| p.to_string())
                    .unwrap_or_else(|_| v.to_string()),
                ..Default::default()
            });
            lun = None;
            continue;
        }
        let Some(s) = out.last_mut() else { continue };
        if let Some(v) = after(line, "SID:") {
            s.sid = v.parse().ok();
        } else if let Some(v) = after(line, "Iface IPaddress:") {
            if v != "<empty>" && v != "default" {
                s.local_address = Some(v.to_string());
            }
        } else if let Some(v) = after(line, "Iface Transport:") {
            s.transport = Some(v.to_string());
        } else if let Some(v) = after(line, "iSCSI Connection State:") {
            s.connection_state = Some(v.to_string());
        } else if let Some(v) = after(line, "iSCSI Session State:") {
            s.session_state = Some(v.to_string());
        } else if let Some(v) = after(line, "Host Number:") {
            s.scsi_host = v.split_whitespace().next().and_then(|n| n.parse().ok());
        } else if line.starts_with("scsi") && line.contains(" Lun: ") {
            lun = line
                .rsplit("Lun:")
                .next()
                .and_then(|n| n.trim().parse().ok());
        } else if let Some(v) = after(line, "Attached scsi disk") {
            let mut it = v.split_whitespace();
            let dev = it.next().unwrap_or("").to_string();
            let state = v
                .split("State:")
                .nth(1)
                .map(|x| x.trim().to_string())
                .unwrap_or_default();
            if !dev.is_empty() {
                s.disks.push(SessionDisk {
                    lun: lun.unwrap_or(0),
                    dev,
                    state,
                });
            }
        }
    }
    out
}

/// `iscsiadm -m node ... -o update` settings for one target's node records.
/// The CHAP secret is never part of this list (see [`chap_updates`]).
pub fn node_updates(t: &IscsiTargetSpec) -> Vec<(&'static str, String)> {
    vec![
        (
            "node.startup",
            if t.startup_automatic {
                "automatic"
            } else {
                "manual"
            }
            .into(),
        ),
        (
            "node.session.timeo.replacement_timeout",
            t.replacement_timeout_secs.to_string(),
        ),
        (
            "node.conn[0].timeo.login_timeout",
            t.login_timeout_secs.to_string(),
        ),
    ]
}

/// CHAP settings for the node records (empty list = authentication off).
pub fn chap_updates(user: Option<&str>, secret: Option<&str>) -> Vec<(&'static str, String)> {
    match (user, secret) {
        (Some(u), Some(s)) if !s.is_empty() => vec![
            ("node.session.auth.authmethod", "CHAP".into()),
            ("node.session.auth.username", u.to_string()),
            ("node.session.auth.password", s.to_string()),
        ],
        _ => vec![("node.session.auth.authmethod", "None".into())],
    }
}

/// Output of a host tool run.
#[derive(Debug, Clone)]
pub struct Run {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Run a host tool with a deadline; `Err` when it is missing or timed out.
pub async fn run(program: &str, args: &[&str], timeout: Duration) -> Result<Run, String> {
    let fut = tokio::process::Command::new(program)
        .args(args)
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(timeout, fut).await {
        Err(_) => Err(format!(
            "{program} did not finish within {}s",
            timeout.as_secs()
        )),
        Ok(Err(e)) => Err(format!("{program}: {e}")),
        Ok(Ok(o)) => Ok(Run {
            ok: o.status.success(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }),
    }
}

/// Discovery through every listed portal; a portal that does not answer
/// is reported in the error list but does not stop the others.
pub async fn discover(
    portals: &[String],
    timeout: Duration,
) -> (Vec<DiscoveredTarget>, Vec<String>) {
    let mut found = Vec::new();
    let mut errors = Vec::new();
    for p in portals {
        match run(
            "iscsiadm",
            &["-m", "discovery", "-t", "sendtargets", "-p", p],
            timeout,
        )
        .await
        {
            Ok(r) if r.ok => merge_discovery(&mut found, &r.stdout),
            Ok(r) => errors.push(format!("{p}: {}", r.stderr.trim())),
            Err(e) => errors.push(format!("{p}: {e}")),
        }
    }
    (found, errors)
}

/// Bring one path up: make sure the node record exists, apply the settings
/// and log in (an existing session is fine).
pub async fn ensure_path(
    t: &IscsiTargetSpec,
    portal: &str,
    secret: Option<&str>,
    timeout: Duration,
) -> Result<(), String> {
    let base = ["-m", "node", "-T", t.iqn.as_str(), "-p", portal];
    let exists = run("iscsiadm", &base, timeout).await?;
    if !exists.ok {
        let mut a = base.to_vec();
        a.extend(["-o", "new"]);
        let r = run("iscsiadm", &a, timeout).await?;
        if !r.ok {
            return Err(format!("{portal}: node record: {}", r.stderr.trim()));
        }
    }
    let mut updates = node_updates(t);
    updates.extend(chap_updates(t.chap_user.as_deref(), secret));
    for (k, v) in &updates {
        let mut a = base.to_vec();
        a.extend(["-o", "update", "-n", k, "-v", v.as_str()]);
        let r = run("iscsiadm", &a, timeout).await?;
        if !r.ok {
            // Never echo the value: it may be the CHAP secret.
            return Err(format!("{portal}: setting {k}: {}", r.stderr.trim()));
        }
    }
    let mut a = base.to_vec();
    a.push("--login");
    let r = run("iscsiadm", &a, timeout).await?;
    if r.ok || r.stderr.contains("already present") || r.stderr.contains("session exists") {
        Ok(())
    } else {
        Err(format!("{portal}: login: {}", r.stderr.trim()))
    }
}

/// Log out of a path and drop its node record.
pub async fn remove_path(iqn: &str, portal: &str, timeout: Duration) -> Result<(), String> {
    let base = ["-m", "node", "-T", iqn, "-p", portal];
    let mut a = base.to_vec();
    a.push("--logout");
    let _ = run("iscsiadm", &a, timeout).await?;
    let mut a = base.to_vec();
    a.extend(["-o", "delete"]);
    let r = run("iscsiadm", &a, timeout).await?;
    if r.ok || r.stderr.contains("No records found") {
        Ok(())
    } else {
        Err(format!("{portal}: {}", r.stderr.trim()))
    }
}

/// Current sessions (empty when there are none: iscsiadm exits 21 then).
pub async fn sessions(timeout: Duration) -> Result<Vec<Session>, String> {
    let r = run("iscsiadm", &["-m", "session", "-P", "3"], timeout).await?;
    Ok(parse_sessions(&r.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portals() {
        assert_eq!(
            Portal::parse("10.0.0.1").unwrap().to_string(),
            "10.0.0.1:3260"
        );
        assert_eq!(
            Portal::parse("10.0.0.1:3261,1").unwrap().to_string(),
            "10.0.0.1:3261"
        );
        assert_eq!(
            Portal::parse("fd00::1").unwrap().to_string(),
            "[fd00::1]:3260"
        );
        assert_eq!(
            Portal::parse("[fd00::1]:3262,1").unwrap().to_string(),
            "[fd00::1]:3262"
        );
        assert_eq!(
            Portal::parse("san-a.example").unwrap().to_string(),
            "san-a.example:3260"
        );
        for bad in [
            "",
            "1.2.3.4:x",
            "[::1",
            "a b",
            "1.2.3.4:0",
            "[zz]:1",
            "x;rm",
        ] {
            assert!(Portal::parse(bad).is_err(), "{bad}");
        }
    }

    const DISCOVERY: &str = "\
10.10.1.1:3260,1 iqn.2003-01.org.linux-iscsi.san:sn.a1
10.10.1.2:3260,1 iqn.2003-01.org.linux-iscsi.san:sn.a1
10.10.2.1:3260,1 iqn.2003-01.org.linux-iscsi.san:sn.a1
[fd10:2::2]:3260,1 iqn.2003-01.org.linux-iscsi.san:sn.a1
10.10.1.1:3260,1 iqn.2003-01.org.linux-iscsi.san:sn.other
iscsiadm: some warning text
";

    #[test]
    fn discovery_collects_every_advertised_portal() {
        let mut d = parse_discovery(DISCOVERY);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].iqn, "iqn.2003-01.org.linux-iscsi.san:sn.a1");
        assert_eq!(
            d[0].portals,
            vec![
                "10.10.1.1:3260",
                "10.10.1.2:3260",
                "10.10.2.1:3260",
                "[fd10:2::2]:3260"
            ]
        );
        // asking a second portal of the same array adds nothing twice
        merge_discovery(&mut d, "10.10.2.1:3260,1 iqn.2003-01.org.linux-iscsi.san:sn.a1\n10.10.2.2:3260,1 iqn.2003-01.org.linux-iscsi.san:sn.a1\n");
        assert_eq!(d[0].portals.len(), 5);
    }

    fn target() -> IscsiTargetSpec {
        serde_json::from_value(serde_json::json!({
            "id": "a1",
            "iqn": "iqn.2003-01.org.linux-iscsi.san:sn.a1",
            "portals": ["10.10.1.1:3260"],
        }))
        .unwrap()
    }

    #[test]
    fn login_portals_follow_the_setting() {
        let d = parse_discovery(DISCOVERY);
        let mut t = target();
        assert_eq!(login_portals(&t, &d).len(), 4);
        t.use_advertised_portals = false;
        assert_eq!(login_portals(&t, &d), vec!["10.10.1.1:3260"]);
    }

    #[test]
    fn chap_settings() {
        assert_eq!(
            chap_updates(None, None),
            vec![("node.session.auth.authmethod", "None".to_string())]
        );
        let c = chap_updates(Some("u"), Some("012345678901"));
        assert_eq!(c[0].1, "CHAP");
        assert_eq!(c[1], ("node.session.auth.username", "u".to_string()));
        // node_updates never carries the secret
        assert!(node_updates(&target())
            .iter()
            .all(|(k, _)| !k.contains("password")));
    }

    const SESSIONS: &str = "\
iSCSI Transport Class version 2.0-870
version 2.1.9
Target: iqn.2003-01.org.linux-iscsi.san:sn.a1 (non-flash)
	Current Portal: 10.10.1.1:3260,1
	Persistent Portal: 10.10.1.1:3260,1
		**********
		Interface:
		**********
		Iface Name: default
		Iface Transport: tcp
		Iface Initiatorname: iqn.1993-08.org.debian:01:1
		Iface IPaddress: 10.10.1.20
		Iface HWaddress: default
		Iface Netdev: default
		SID: 1
		iSCSI Connection State: LOGGED IN
		iSCSI Session State: LOGGED_IN
		Internal iscsid Session State: NO CHANGE
		************************
		Attached SCSI devices:
		************************
		Host Number: 2	State: running
		scsi2 Channel 00 Id 0 Lun: 0
			Attached scsi disk sdb		State: running
		scsi2 Channel 00 Id 0 Lun: 1
			Attached scsi disk sdc		State: running
	Current Portal: [fd10:2::2]:3260,1
	Persistent Portal: [fd10:2::2]:3260,1
		Iface Transport: tcp
		Iface IPaddress: fd10:2::20
		SID: 2
		iSCSI Connection State: TRANSPORT WAIT
		iSCSI Session State: FAILED
		Host Number: 3	State: running
		scsi3 Channel 00 Id 0 Lun: 0
			Attached scsi disk sdd		State: transport-offline
";

    #[test]
    fn sessions_parse() {
        let s = parse_sessions(SESSIONS);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].iqn, "iqn.2003-01.org.linux-iscsi.san:sn.a1");
        assert_eq!(s[0].portal, "10.10.1.1:3260");
        assert_eq!(s[0].sid, Some(1));
        assert_eq!(s[0].local_address.as_deref(), Some("10.10.1.20"));
        assert!(s[0].logged_in());
        assert_eq!(s[0].scsi_host, Some(2));
        assert_eq!(
            s[0].disks,
            vec![
                SessionDisk {
                    lun: 0,
                    dev: "sdb".into(),
                    state: "running".into()
                },
                SessionDisk {
                    lun: 1,
                    dev: "sdc".into(),
                    state: "running".into()
                }
            ]
        );
        assert_eq!(s[1].portal, "[fd10:2::2]:3260");
        assert!(!s[1].logged_in());
        assert_eq!(s[1].disks[0].state, "transport-offline");
        assert!(parse_sessions("iscsiadm: No active sessions.\n").is_empty());
    }
}
