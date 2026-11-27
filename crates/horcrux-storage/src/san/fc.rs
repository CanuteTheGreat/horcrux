//! Fibre Channel: the node's HBAs (`/sys/class/fc_host`), the remote ports
//! they see (`/sys/class/fc_remote_ports`) and the LUNs behind each target
//! port. FC needs no login: the HBA and the fabric zoning decide what a node
//! sees, so Horcrux only reads sysfs, rescans on request and feeds every
//! FC path into the same dm-multipath maps and path view as iSCSI.
//!
//! Everything here reads a sysfs tree under a root directory, so the unit
//! tests run against a fixture tree. QEMU has no emulated FC HBA, so FC is
//! covered by these fixture tests only (docs/SAN.md).

use super::iscsi::{Session, SessionDisk};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// One local FC HBA port (`hostN`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FcHost {
    /// `host5`.
    pub name: String,
    /// SCSI host number (`5`).
    pub host: u32,
    /// WWPN in `0x...` form as sysfs prints it, normalised to
    /// `21:00:00:24:ff:12:34:56`.
    pub wwpn: Option<String>,
    pub wwnn: Option<String>,
    /// `Online`, `Linkdown`, `Offline`, ...
    pub port_state: Option<String>,
    /// `NPort (fabric via point-to-point)`, `LPort (private loop)`, ...
    pub port_type: Option<String>,
    /// `16 Gbit`.
    pub speed: Option<String>,
    pub supported_speeds: Option<String>,
    pub fabric_name: Option<String>,
    pub symbolic_name: Option<String>,
}

impl FcHost {
    pub fn online(&self) -> bool {
        self.port_state.as_deref() == Some("Online")
    }
}

/// One remote FC port seen by a local HBA (`rport-H:B-R`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FcRemotePort {
    /// `rport-5:0-2`.
    pub name: String,
    /// Local SCSI host number.
    pub host: u32,
    pub wwpn: Option<String>,
    pub wwnn: Option<String>,
    /// FC address (`0x010200`).
    pub port_id: Option<String>,
    /// `Online`, `Blocked`, `Not Present`, ...
    pub port_state: Option<String>,
    /// `FCP Target`, `FCP Initiator`, `NVMe Target`, ...
    pub roles: Option<String>,
    /// LUNs behind this port (only for target ports).
    pub luns: Vec<FcLun>,
}

impl FcRemotePort {
    pub fn online(&self) -> bool {
        self.port_state.as_deref() == Some("Online")
    }
    pub fn is_target(&self) -> bool {
        self.roles
            .as_deref()
            .is_some_and(|r| r.split(',').any(|x| x.trim().ends_with("Target")))
    }
}

/// A SCSI LUN behind a remote port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FcLun {
    /// `5:0:0:1`.
    pub hctl: String,
    pub lun: u32,
    /// Block device (`sdc`), if the disk driver bound.
    pub dev: Option<String>,
    /// SCSI device state (`running`, `blocked`, `transport-offline`).
    pub state: Option<String>,
    pub vendor: Option<String>,
    pub model: Option<String>,
    pub wwid: Option<String>,
}

/// The node's FC view: HBAs and their remote ports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FcView {
    pub hosts: Vec<FcHost>,
    pub remote_ports: Vec<FcRemotePort>,
}

fn read(p: &Path) -> Option<String> {
    std::fs::read_to_string(p)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != "unknown")
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

/// `0x21000024ff123456` -> `21:00:00:24:ff:12:34:56`. Anything else
/// (already colon form, odd length) is lower-cased and kept.
pub fn normalize_wwn(s: &str) -> String {
    let t = s.trim().to_ascii_lowercase();
    let hex = t.strip_prefix("0x").unwrap_or(&t);
    if hex.len() == 16 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        hex.as_bytes()
            .chunks(2)
            .map(|c| std::str::from_utf8(c).unwrap_or("00"))
            .collect::<Vec<_>>()
            .join(":")
    } else {
        t
    }
}

/// Is `s` a WWPN/WWNN (`0x` + 16 hex digits or 8 colon-separated bytes)?
pub fn valid_wwn(s: &str) -> bool {
    let n = normalize_wwn(s);
    let parts: Vec<&str> = n.split(':').collect();
    parts.len() == 8
        && parts
            .iter()
            .all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// `host5` -> 5.
fn host_no(name: &str) -> Option<u32> {
    name.strip_prefix("host")?.parse().ok()
}

/// `rport-5:0-2` -> host 5.
pub fn rport_host(name: &str) -> Option<u32> {
    name.strip_prefix("rport-")?.split(':').next()?.parse().ok()
}

/// `5:0:0:1` -> LUN 1.
fn hctl_lun(hctl: &str) -> Option<u32> {
    let p: Vec<&str> = hctl.split(':').collect();
    if p.len() == 4 && p.iter().all(|x| x.parse::<u64>().is_ok()) {
        p[3].parse().ok()
    } else {
        None
    }
}

/// Every FC HBA port under `<root>/sys/class/fc_host`.
pub fn scan_hosts(root: &Path) -> Vec<FcHost> {
    let base = root.join("sys/class/fc_host");
    names(&base)
        .into_iter()
        .filter_map(|n| {
            let host = host_no(&n)?;
            let d = base.join(&n);
            Some(FcHost {
                host,
                wwpn: read(&d.join("port_name")).map(|s| normalize_wwn(&s)),
                wwnn: read(&d.join("node_name")).map(|s| normalize_wwn(&s)),
                port_state: read(&d.join("port_state")),
                port_type: read(&d.join("port_type")),
                speed: read(&d.join("speed")),
                supported_speeds: read(&d.join("supported_speeds")),
                fabric_name: read(&d.join("fabric_name")).map(|s| normalize_wwn(&s)),
                symbolic_name: read(&d.join("symbolic_name")),
                name: n,
            })
        })
        .collect()
}

/// The LUNs under a remote port's device directory:
/// `<rport>/device/target5:0:0/5:0:0:1/block/sdc`.
fn scan_luns(dev_dir: &Path) -> Vec<FcLun> {
    let mut out = Vec::new();
    for t in names(dev_dir)
        .into_iter()
        .filter(|t| t.starts_with("target"))
    {
        let td = dev_dir.join(&t);
        for h in names(&td) {
            let Some(lun) = hctl_lun(&h) else { continue };
            let sd = td.join(&h);
            out.push(FcLun {
                dev: names(&sd.join("block")).into_iter().next(),
                state: read(&sd.join("state")),
                vendor: read(&sd.join("vendor")),
                model: read(&sd.join("model")),
                wwid: read(&sd.join("wwid")),
                lun,
                hctl: h,
            });
        }
    }
    out.sort_by(|a, b| a.hctl.cmp(&b.hctl));
    out
}

/// Every remote port under `<root>/sys/class/fc_remote_ports`, with the
/// LUNs behind the target ports.
pub fn scan_remote_ports(root: &Path) -> Vec<FcRemotePort> {
    let base = root.join("sys/class/fc_remote_ports");
    names(&base)
        .into_iter()
        .filter_map(|n| {
            let host = rport_host(&n)?;
            let d = base.join(&n);
            Some(FcRemotePort {
                host,
                wwpn: read(&d.join("port_name")).map(|s| normalize_wwn(&s)),
                wwnn: read(&d.join("node_name")).map(|s| normalize_wwn(&s)),
                port_id: read(&d.join("port_id")),
                port_state: read(&d.join("port_state")),
                roles: read(&d.join("roles")),
                luns: scan_luns(&d.join("device")),
                name: n,
            })
        })
        .collect()
}

pub fn scan(root: &Path) -> FcView {
    FcView {
        hosts: scan_hosts(root),
        remote_ports: scan_remote_ports(root),
    }
}

/// FC paths as path-view sessions: one per (local HBA, remote target port),
/// so the iSCSI/multipath assembler groups them by WWID into the same LUN
/// rows. `target` is the remote WWPN, `portal` the remote FC address,
/// `local_address` the HBA's WWPN.
pub fn as_sessions(v: &FcView) -> Vec<Session> {
    v.remote_ports
        .iter()
        .filter(|r| r.is_target())
        .map(|r| {
            let h = v.hosts.iter().find(|h| h.host == r.host);
            let up = r.online() && h.is_none_or(FcHost::online);
            Session {
                iqn: r.wwpn.clone().unwrap_or_else(|| r.name.clone()),
                portal: r
                    .port_id
                    .clone()
                    .map(|p| format!("fc:{p}"))
                    .unwrap_or_else(|| r.name.clone()),
                sid: None,
                local_address: h.and_then(|h| h.wwpn.clone()),
                transport: Some("fc".into()),
                connection_state: r.port_state.clone(),
                session_state: Some(if up { "LOGGED_IN" } else { "FAILED" }.into()),
                scsi_host: Some(r.host),
                disks: r
                    .luns
                    .iter()
                    .filter_map(|l| {
                        Some(SessionDisk {
                            lun: l.lun,
                            dev: l.dev.clone()?,
                            state: l.state.clone().unwrap_or_else(|| "running".into()),
                        })
                    })
                    .collect(),
            }
        })
        .collect()
}

/// How a rescan reaches the fabric.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FcRescanMode {
    /// Scan the SCSI host for new targets/LUNs (`scsi_host/hostN/scan`).
    /// Non-disruptive.
    #[default]
    Scan,
    /// Also issue a LIP (`fc_host/hostN/issue_lip`): the HBA logs in to the
    /// fabric again. Briefly interrupts I/O on that HBA; multipath carries
    /// it on the other HBAs.
    Lip,
}

/// Rescan one HBA (or every HBA when `host` is None). Returns the hosts
/// touched. Writes under `<root>/sys`.
pub fn rescan(root: &Path, host: Option<u32>, mode: FcRescanMode) -> Result<Vec<String>, String> {
    let hosts: Vec<FcHost> = scan_hosts(root)
        .into_iter()
        .filter(|h| host.is_none_or(|n| n == h.host))
        .collect();
    if hosts.is_empty() {
        return Err(match host {
            Some(n) => format!("no FC HBA host{n}"),
            None => "no FC HBA on this node".into(),
        });
    }
    let mut done = Vec::new();
    for h in hosts {
        if mode == FcRescanMode::Lip {
            let f = root
                .join("sys/class/fc_host")
                .join(&h.name)
                .join("issue_lip");
            std::fs::write(&f, "1").map_err(|e| format!("{}: {e}", f.display()))?;
        }
        let f = root.join("sys/class/scsi_host").join(&h.name).join("scan");
        std::fs::write(&f, "- - -").map_err(|e| format!("{}: {e}", f.display()))?;
        done.push(h.name);
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(root: &Path, rel: &str, v: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, format!("{v}\n")).unwrap();
    }

    /// Two HBAs (host5, host6), each seeing one target port of the same
    /// array plus an initiator port; LUN 1 behind both targets.
    fn tree(host6_down: bool) -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        for (h, wwpn, state) in [
            ("host5", "0x21000024ff000005", "Online"),
            (
                "host6",
                "0x21000024ff000006",
                if host6_down { "Linkdown" } else { "Online" },
            ),
        ] {
            let d = format!("sys/class/fc_host/{h}");
            w(r, &format!("{d}/port_name"), wwpn);
            w(r, &format!("{d}/node_name"), "0x20000024ff000001");
            w(r, &format!("{d}/port_state"), state);
            w(
                r,
                &format!("{d}/port_type"),
                "NPort (fabric via point-to-point)",
            );
            w(r, &format!("{d}/speed"), "16 Gbit");
            w(r, &format!("{d}/fabric_name"), "0x100000051e000001");
            w(r, &format!("sys/class/scsi_host/{h}/scan"), "");
        }
        for (rp, wwpn, roles, hctl, dev) in [
            (
                "rport-5:0-0",
                "0x500a098100000001",
                "FCP Target",
                "5:0:0:1",
                "sdc",
            ),
            (
                "rport-6:0-0",
                "0x500a098200000001",
                "FCP Target",
                "6:0:0:1",
                "sdd",
            ),
            ("rport-5:0-1", "0x21000024ff0000aa", "FCP Initiator", "", ""),
        ] {
            let d = format!("sys/class/fc_remote_ports/{rp}");
            w(r, &format!("{d}/port_name"), wwpn);
            w(r, &format!("{d}/node_name"), "0x500a098000000001");
            w(r, &format!("{d}/port_id"), "0x010200");
            w(r, &format!("{d}/roles"), roles);
            let st = if rp.starts_with("rport-6") && host6_down {
                "Blocked"
            } else {
                "Online"
            };
            w(r, &format!("{d}/port_state"), st);
            if !hctl.is_empty() {
                let t = format!("{d}/device/target{}", &hctl[..hctl.rfind(':').unwrap()]);
                w(
                    r,
                    &format!("{t}/{hctl}/state"),
                    if st == "Online" { "running" } else { "blocked" },
                );
                w(r, &format!("{t}/{hctl}/vendor"), "NETAPP");
                w(r, &format!("{t}/{hctl}/model"), "LUN C-Mode");
                w(r, &format!("{t}/{hctl}/wwid"), "naa.600a098038303053");
                w(r, &format!("{t}/{hctl}/block/{dev}/size"), "2097152");
            }
        }
        t
    }

    #[test]
    fn wwn_forms() {
        assert_eq!(
            normalize_wwn("0x21000024FF123456"),
            "21:00:00:24:ff:12:34:56"
        );
        assert_eq!(
            normalize_wwn("21:00:00:24:ff:12:34:56"),
            "21:00:00:24:ff:12:34:56"
        );
        assert!(valid_wwn("0x21000024ff123456"));
        assert!(valid_wwn("21:00:00:24:FF:12:34:56"));
        assert!(!valid_wwn("0x2100"));
        assert!(!valid_wwn("zz:00:00:24:ff:12:34:56"));
        assert_eq!(rport_host("rport-12:0-7"), Some(12));
        assert_eq!(hctl_lun("5:0:0:17"), Some(17));
        assert_eq!(hctl_lun("target5:0:0"), None);
    }

    #[test]
    fn sysfs_hosts_and_remote_ports() {
        let t = tree(false);
        let v = scan(t.path());
        assert_eq!(v.hosts.len(), 2);
        let h5 = &v.hosts[0];
        assert_eq!(h5.name, "host5");
        assert_eq!(h5.host, 5);
        assert_eq!(h5.wwpn.as_deref(), Some("21:00:00:24:ff:00:00:05"));
        assert_eq!(h5.speed.as_deref(), Some("16 Gbit"));
        assert!(h5.online());
        assert_eq!(v.remote_ports.len(), 3);
        let tgt: Vec<&FcRemotePort> = v.remote_ports.iter().filter(|r| r.is_target()).collect();
        assert_eq!(tgt.len(), 2);
        assert_eq!(tgt[0].luns.len(), 1);
        assert_eq!(tgt[0].luns[0].dev.as_deref(), Some("sdc"));
        assert_eq!(tgt[0].luns[0].lun, 1);
        assert_eq!(tgt[0].luns[0].wwid.as_deref(), Some("naa.600a098038303053"));
        let ini = v.remote_ports.iter().find(|r| !r.is_target()).unwrap();
        assert!(ini.luns.is_empty());
    }

    #[test]
    fn fc_paths_feed_the_path_view() {
        let t = tree(true);
        let v = scan(t.path());
        let s = as_sessions(&v);
        assert_eq!(s.len(), 2, "initiator ports are not paths");
        assert!(s.iter().all(|s| s.transport.as_deref() == Some("fc")));
        assert!(s[0].logged_in());
        assert!(
            !s[1].logged_in(),
            "a blocked rport behind a down HBA is a failed path"
        );
        assert_eq!(
            s[0].local_address.as_deref(),
            Some("21:00:00:24:ff:00:00:05")
        );
        let wwids: std::collections::HashMap<String, String> = [
            ("sdc".to_string(), "naa.600a098038303053".to_string()),
            ("sdd".to_string(), "naa.600a098038303053".to_string()),
        ]
        .into();
        let luns = super::super::paths::assemble(&s, &[], &wwids, |_| Some(1 << 30));
        assert_eq!(luns.len(), 1, "both FC paths are one LUN");
        let l = &luns[0];
        assert_eq!(l.paths.len(), 2);
        assert!(l.paths.iter().all(|p| p.transport == "fc"));
        assert_eq!(l.failed_paths, 1);
        assert_eq!(l.active_paths, 1);
    }

    #[test]
    fn rescan_writes_scan_and_lip() {
        let t = tree(false);
        let r = t.path();
        assert_eq!(
            rescan(r, Some(6), FcRescanMode::Scan).unwrap(),
            vec!["host6"]
        );
        assert_eq!(
            std::fs::read_to_string(r.join("sys/class/scsi_host/host6/scan")).unwrap(),
            "- - -"
        );
        assert!(!r.join("sys/class/fc_host/host6/issue_lip").exists());
        assert_eq!(rescan(r, None, FcRescanMode::Lip).unwrap().len(), 2);
        assert_eq!(
            std::fs::read_to_string(r.join("sys/class/fc_host/host5/issue_lip")).unwrap(),
            "1"
        );
        assert!(rescan(r, Some(9), FcRescanMode::Scan).is_err());
        let empty = tempfile::tempdir().unwrap();
        assert!(rescan(empty.path(), None, FcRescanMode::Scan).is_err());
    }
}
