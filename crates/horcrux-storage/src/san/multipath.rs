//! dm-multipath: the Horcrux-owned drop-in rendered from
//! [`MultipathSettings`], applying it (`multipathd reconfigure`), WWID
//! registration for `find_multipaths strict`, and the map/path state from
//! `multipathd show maps json`.

use super::{DeviceMatch, MultipathSettings};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

/// First line of the drop-in: what owns it.
pub const HEADER: &str =
    "# Managed by Horcrux (SAN settings: /api/san/settings). Do not edit: changes are overwritten.";

fn rule(out: &mut String, r: &DeviceMatch) {
    match r {
        DeviceMatch::Wwid { wwid } => out.push_str(&format!("\twwid \"{wwid}\"\n")),
        DeviceMatch::Devnode { devnode } => out.push_str(&format!("\tdevnode \"{devnode}\"\n")),
        DeviceMatch::Device { vendor, product } => out.push_str(&format!(
            "\tdevice {{\n\t\tvendor \"{vendor}\"\n\t\tproduct \"{product}\"\n\t}}\n"
        )),
    }
}

/// The multipath.conf drop-in for these settings (deterministic, so an
/// unchanged config is never rewritten or reloaded).
pub fn render(s: &MultipathSettings) -> String {
    let mut o = String::new();
    o.push_str(HEADER);
    o.push('\n');
    o.push_str("defaults {\n");
    let kv = |o: &mut String, k: &str, v: &str| o.push_str(&format!("\t{k} {v}\n"));
    kv(
        &mut o,
        "user_friendly_names",
        if s.user_friendly_names { "yes" } else { "no" },
    );
    kv(&mut o, "find_multipaths", s.find_multipaths.as_conf());
    kv(
        &mut o,
        "path_grouping_policy",
        s.path_grouping_policy.as_conf(),
    );
    kv(
        &mut o,
        "path_selector",
        &format!("\"{}\"", s.path_selector.as_conf()),
    );
    kv(&mut o, "no_path_retry", &s.no_path_retry.to_string());
    kv(&mut o, "failback", &s.failback.to_string());
    kv(
        &mut o,
        "polling_interval",
        &s.polling_interval_secs.to_string(),
    );
    kv(
        &mut o,
        "fast_io_fail_tmo",
        &s.fast_io_fail_tmo_secs.to_string(),
    );
    kv(&mut o, "dev_loss_tmo", &s.dev_loss_tmo_secs.to_string());
    o.push_str("}\n");
    if !s.blacklist.is_empty() {
        o.push_str("blacklist {\n");
        for r in &s.blacklist {
            rule(&mut o, r);
        }
        o.push_str("}\n");
    }
    if !s.blacklist_exceptions.is_empty() {
        o.push_str("blacklist_exceptions {\n");
        for r in &s.blacklist_exceptions {
            rule(&mut o, r);
        }
        o.push_str("}\n");
    }
    o
}

/// What writing the drop-in did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    Unchanged,
    Written,
}

/// Write `content` to `path` atomically when it differs. Refuses to
/// replace a file that is not ours (no [`HEADER`]): a hand-written file at
/// that path is the admin's, so pick another `config_path`.
pub fn write_dropin(path: &Path, content: &str) -> Result<WriteOutcome, String> {
    match std::fs::read_to_string(path) {
        Ok(old) if old == content => return Ok(WriteOutcome::Unchanged),
        Ok(old) if !old.starts_with(HEADER) => {
            return Err(format!(
                "{} exists and is not managed by Horcrux; move it away or set another config_path",
                path.display()
            ))
        }
        _ => {}
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("conf.horcrux-tmp");
    std::fs::write(&tmp, content).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(WriteOutcome::Written)
}

/// Remove our drop-in (multipath turned off).
pub fn remove_dropin(path: &Path) -> Result<bool, String> {
    match std::fs::read_to_string(path) {
        Ok(old) if old.starts_with(HEADER) => std::fs::remove_file(path)
            .map(|_| true)
            .map_err(|e| format!("{}: {e}", path.display())),
        _ => Ok(false),
    }
}

/// One path of a multipath map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MapPath {
    /// Kernel name (`sdb`, `nvme0n1`).
    pub dev: String,
    pub dev_t: Option<String>,
    /// Device-mapper view: `active` / `failed`.
    pub dm_state: String,
    /// Path checker: `ready`, `faulty`, `ghost`, `shaky`.
    pub checker_state: String,
    /// Kernel device state: `running`, `offline`, ...
    pub device_state: String,
    pub priority: Option<i64>,
    pub group: Option<i64>,
    /// Target name (IQN, NQN or WWNN) as multipathd sees it.
    pub target: Option<String>,
    /// Local adapter (initiator address for iSCSI, host WWPN for FC).
    pub host_adapter: Option<String>,
}

impl MapPath {
    pub fn healthy(&self) -> bool {
        self.dm_state == "active" && self.checker_state == "ready"
    }
}

/// One multipath map (one LUN).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Map {
    pub name: String,
    pub wwid: String,
    /// `dm-N`.
    pub sysfs: String,
    pub dm_state: String,
    pub vendor: String,
    pub product: String,
    pub queueing: Option<String>,
    pub path_faults: Option<i64>,
    pub paths: Vec<MapPath>,
}

fn s(v: &Value, k: &str) -> String {
    v.get(k)
        .and_then(Value::as_str)
        .map(|x| x.trim().to_string())
        .unwrap_or_default()
}

fn opt(v: &Value, k: &str) -> Option<String> {
    let x = s(v, k);
    if x.is_empty() || x == "[undef]" || x == "undef" || x == "-" {
        None
    } else {
        Some(x)
    }
}

/// Parse `multipathd show maps json` (multipath-tools 0.8+ layout:
/// `maps[].path_groups[].paths[]`). Unknown fields are ignored so newer
/// releases keep working.
pub fn parse_maps_json(text: &str) -> Result<Vec<Map>, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("multipathd json: {e}"))?;
    let mut out = Vec::new();
    for m in v
        .get("maps")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let mut map = Map {
            name: s(m, "name"),
            wwid: s(m, "uuid"),
            sysfs: s(m, "sysfs"),
            dm_state: s(m, "dm_st"),
            vendor: s(m, "vend"),
            product: s(m, "prod"),
            queueing: opt(m, "queueing"),
            path_faults: m.get("path_faults").and_then(Value::as_i64),
            paths: Vec::new(),
        };
        for g in m
            .get("path_groups")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let group = g.get("group").and_then(Value::as_i64);
            for p in g
                .get("paths")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                map.paths.push(MapPath {
                    dev: s(p, "dev"),
                    dev_t: opt(p, "dev_t"),
                    dm_state: s(p, "dm_st"),
                    checker_state: s(p, "chk_st"),
                    device_state: s(p, "dev_st"),
                    priority: p.get("pri").and_then(Value::as_i64),
                    group,
                    target: opt(p, "target_wwnn"),
                    host_adapter: opt(p, "host_adapter"),
                });
            }
        }
        out.push(map);
    }
    Ok(out)
}

/// The WWID multipath uses for a SCSI disk: udev's `ID_SERIAL` (multipath's
/// default `uid_attribute`), with udev's `scsi_id` helper as the fallback.
pub fn wwid_from_udev_props(text: &str) -> Option<String> {
    text.lines()
        .find_map(|l| l.strip_prefix("ID_SERIAL="))
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

pub async fn wwid_of(dev: &str, timeout: Duration) -> Option<String> {
    let path = format!("/dev/{dev}");
    if let Ok(r) = super::iscsi::run(
        "udevadm",
        &["info", "--query=property", "--name", &path],
        timeout,
    )
    .await
    {
        if let Some(w) = wwid_from_udev_props(&r.stdout).filter(|_| r.ok) {
            return Some(w);
        }
    }
    for prog in ["/usr/lib/udev/scsi_id", "/lib/udev/scsi_id"] {
        if Path::new(prog).exists() {
            if let Ok(r) = super::iscsi::run(prog, &["-g", "-u", "-d", &path], timeout).await {
                let w = r.stdout.trim().to_string();
                if r.ok && !w.is_empty() {
                    return Some(w);
                }
            }
        }
    }
    None
}

/// Tell multipathd about a WWID (needed with `find_multipaths strict`).
pub async fn add_wwid(wwid: &str, timeout: Duration) -> Result<(), String> {
    if wwid.is_empty()
        || !wwid
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'))
    {
        return Err(format!("bad wwid {wwid:?}"));
    }
    // `multipath -a` records it in /etc/multipath/wwids; `multipathd add map`
    // then builds the map without a rescan.
    let r = super::iscsi::run("multipath", &["-a", wwid], timeout).await?;
    if !r.ok && !r.stdout.contains("already") && !r.stderr.contains("already") {
        return Err(format!("multipath -a {wwid}: {}", r.stderr.trim()));
    }
    let _ = super::iscsi::run("multipathd", &["add", "map", wwid], timeout).await;
    Ok(())
}

/// Make multipathd read the config again.
pub async fn reconfigure(timeout: Duration) -> Result<(), String> {
    let r = super::iscsi::run("multipathd", &["reconfigure"], timeout).await?;
    if r.ok && !r.stdout.contains("fail") {
        Ok(())
    } else {
        Err(format!(
            "multipathd reconfigure: {} {}",
            r.stdout.trim(),
            r.stderr.trim()
        ))
    }
}

/// The maps multipathd knows.
pub async fn maps(timeout: Duration) -> Result<Vec<Map>, String> {
    let r = super::iscsi::run("multipathd", &["show", "maps", "json"], timeout).await?;
    if !r.ok {
        return Err(format!("multipathd show maps json: {}", r.stderr.trim()));
    }
    parse_maps_json(&r.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::san::{FindMultipaths, NoPathRetry, PathGrouping};

    #[test]
    fn default_render() {
        let c = render(&MultipathSettings::default());
        assert!(c.starts_with(HEADER));
        let want = "defaults {\n\tuser_friendly_names no\n\tfind_multipaths strict\n\tpath_grouping_policy multibus\n\tpath_selector \"service-time 0\"\n\tno_path_retry 12\n\tfailback immediate\n\tpolling_interval 5\n\tfast_io_fail_tmo 5\n\tdev_loss_tmo 600\n}\n";
        assert!(c.ends_with(want), "{c}");
        assert!(!c.contains("blacklist"));
    }

    #[test]
    fn render_with_rules() {
        let s = MultipathSettings {
            path_grouping_policy: PathGrouping::GroupByPrio,
            no_path_retry: NoPathRetry::Queue,
            find_multipaths: FindMultipaths::Yes,
            blacklist: vec![
                DeviceMatch::Devnode {
                    devnode: "^(ram|zram|loop|fd|md|dm-|sr|scd|st)[0-9]*".into(),
                },
                DeviceMatch::Wwid {
                    wwid: "36001405local".into(),
                },
            ],
            blacklist_exceptions: vec![DeviceMatch::Device {
                vendor: "LIO-ORG".into(),
                product: ".*".into(),
            }],
            ..Default::default()
        };
        let c = render(&s);
        assert!(c.contains("\tpath_grouping_policy group_by_prio\n"));
        assert!(c.contains("\tno_path_retry queue\n"));
        assert!(c.contains("\tfind_multipaths yes\n"));
        assert!(c.contains(
            "blacklist {\n\tdevnode \"^(ram|zram|loop|fd|md|dm-|sr|scd|st)[0-9]*\"\n\twwid \"36001405local\"\n}\n"
        ));
        assert!(c.contains(
            "blacklist_exceptions {\n\tdevice {\n\t\tvendor \"LIO-ORG\"\n\t\tproduct \".*\"\n\t}\n}\n"
        ));
        // deterministic
        assert_eq!(c, render(&s));
    }

    #[test]
    fn dropin_write_is_idempotent_and_never_takes_foreign_files() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("conf.d/horcrux.conf");
        let c = render(&MultipathSettings::default());
        assert_eq!(write_dropin(&p, &c).unwrap(), WriteOutcome::Written);
        assert_eq!(write_dropin(&p, &c).unwrap(), WriteOutcome::Unchanged);
        let c2 = render(&MultipathSettings {
            polling_interval_secs: 10,
            ..Default::default()
        });
        assert_eq!(write_dropin(&p, &c2).unwrap(), WriteOutcome::Written);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), c2);
        let foreign = d.path().join("admin.conf");
        std::fs::write(&foreign, "defaults { }\n").unwrap();
        assert!(write_dropin(&foreign, &c).is_err());
        assert!(!remove_dropin(&foreign).unwrap());
        assert!(remove_dropin(&p).unwrap());
        assert!(!p.exists());
    }

    pub const MAPS_JSON: &str = r#"{
   "major_version": 0,
   "minor_version": 1,
   "maps": [{
      "name" : "36001405d27e5d898829468b90ce4ef8c",
      "uuid" : "36001405d27e5d898829468b90ce4ef8c",
      "sysfs" : "dm-2",
      "failback" : "immediate",
      "queueing" : "12 chk",
      "paths" : 4,
      "write_prot" : "rw",
      "dm_st" : "active",
      "features" : "1 queue_if_no_path",
      "hwhandler" : "1 alua",
      "action" : "",
      "path_faults" : 1,
      "vend" : "LIO-ORG ",
      "prod" : "lun0            ",
      "rev" : "4.0 ",
      "switch_grp" : 0,
      "map_loads" : 1,
      "total_q_time" : 0,
      "q_timeouts" : 0,
      "path_groups": [{
         "selector" : "service-time 0",
         "pri" : 50,
         "dm_st" : "active",
         "marginal_st" : "normal",
         "group" : 1,
         "paths": [{
            "dev" : "sdb",
            "dev_t" : "8:16",
            "dm_st" : "active",
            "dev_st" : "running",
            "chk_st" : "ready",
            "checker" : "tur",
            "pri" : 50,
            "host_wwnn" : "[undef]",
            "target_wwnn" : "iqn.2003-01.org.linux-iscsi.san:sn.a1",
            "host_wwpn" : "[undef]",
            "target_wwpn" : "[undef]",
            "host_adapter" : "10.10.1.20",
            "marginal_st" : "normal"
         },{
            "dev" : "sdd",
            "dev_t" : "8:48",
            "dm_st" : "failed",
            "dev_st" : "running",
            "chk_st" : "faulty",
            "checker" : "tur",
            "pri" : 50,
            "host_wwnn" : "[undef]",
            "target_wwnn" : "iqn.2003-01.org.linux-iscsi.san:sn.a1",
            "host_wwpn" : "[undef]",
            "target_wwpn" : "[undef]",
            "host_adapter" : "10.10.2.20",
            "marginal_st" : "normal"
         }]
      }]
   }]
}
"#;

    #[test]
    fn maps_json_parse() {
        let m = parse_maps_json(MAPS_JSON).unwrap();
        assert_eq!(m.len(), 1);
        let m = &m[0];
        assert_eq!(m.wwid, "36001405d27e5d898829468b90ce4ef8c");
        assert_eq!(m.sysfs, "dm-2");
        assert_eq!(m.vendor, "LIO-ORG");
        assert_eq!(m.product, "lun0");
        assert_eq!(m.path_faults, Some(1));
        assert_eq!(m.paths.len(), 2);
        assert!(m.paths[0].healthy());
        assert_eq!(m.paths[0].host_adapter.as_deref(), Some("10.10.1.20"));
        assert_eq!(
            m.paths[0].target.as_deref(),
            Some("iqn.2003-01.org.linux-iscsi.san:sn.a1")
        );
        assert!(!m.paths[1].healthy());
        assert_eq!(m.paths[1].checker_state, "faulty");
        assert_eq!(m.paths[1].group, Some(1));
        assert!(parse_maps_json("{\"maps\": []}").unwrap().is_empty());
        assert!(parse_maps_json("multipathd not running").is_err());
    }

    #[test]
    fn udev_wwid() {
        let t = "DEVNAME=/dev/sdb\nID_SCSI=1\nID_SERIAL=36001405d27e5d898829468b90ce4ef8c\nID_SERIAL_SHORT=6001405d27e5d898829468b90ce4ef8c\n";
        assert_eq!(
            wwid_from_udev_props(t).as_deref(),
            Some("36001405d27e5d898829468b90ce4ef8c")
        );
        assert_eq!(wwid_from_udev_props("DEVNAME=/dev/sda\n"), None);
    }
}
