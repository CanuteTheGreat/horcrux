//! NIC / bond link-speed awareness (1 GbE .. 100 GbE and bonds of them).
//!
//! Everything reads a sysfs-style tree (`<root>/class/net/<if>/...`) so it is
//! unit-testable with fixtures. No hardcoded 1/10 GbE assumptions: limits scale
//! with the detected speed.

use serde::Serialize;
use std::fs;
use std::path::Path;

/// Speeds Horcrux recognises (Mbit/s): 1/2.5/5/10/25/40/50/100 GbE.
pub const KNOWN_SPEEDS_MBPS: [u64; 9] = [
    100, 1_000, 2_500, 5_000, 10_000, 25_000, 40_000, 50_000, 100_000,
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NicInfo {
    pub name: String,
    /// Link speed in Mbit/s; None when down/unknown (sysfs reports -1).
    pub speed_mbps: Option<u64>,
    pub mtu: u32,
    pub up: bool,
    pub driver: Option<String>,
    pub is_bond: bool,
    pub bond_mode: Option<String>,
    pub slaves: Vec<String>,
    pub sriov_vfs: u32,
}

impl NicInfo {
    /// Aggregate bandwidth in Mbit/s. For bonds this is computed from the
    /// slaves (see [`scan_nics`]); a single NIC is just its own speed.
    pub fn speed_gbps(&self) -> Option<f64> {
        self.speed_mbps.map(|m| m as f64 / 1000.0)
    }
}

/// Human label, e.g. 25000 -> "25 GbE", 2500 -> "2.5 GbE", 20000 -> "20 GbE".
pub fn speed_label(mbps: u64) -> String {
    if mbps.is_multiple_of(1000) {
        format!("{} GbE", mbps / 1000)
    } else if mbps >= 1000 {
        format!("{:.1} GbE", mbps as f64 / 1000.0)
    } else {
        format!("{} MbE", mbps)
    }
}

fn read_trim(p: &Path) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

fn parse_speed(raw: &str) -> Option<u64> {
    match raw.trim().parse::<i64>() {
        Ok(v) if v > 0 => Some(v as u64),
        _ => None,
    }
}

/// Whether a bond mode aggregates bandwidth across slaves.
fn bond_aggregates(mode: &str) -> bool {
    let m = mode.split_whitespace().next().unwrap_or("");
    matches!(
        m,
        "802.3ad" | "balance-rr" | "balance-xor" | "balance-tlb" | "balance-alb"
    )
}

/// Scan `<root>/class/net` and return physical NICs and bonds (lo, veth,
/// bridges, tap and other virtual devices are skipped). Sorted by name.
pub fn scan_nics(root: &Path) -> Vec<NicInfo> {
    let base = root.join("class/net");
    let mut out: Vec<NicInfo> = Vec::new();
    let entries = match fs::read_dir(&base) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let dir = base.join(&name);
        let is_bond = dir.join("bonding").is_dir();
        let has_device = dir.join("device").exists();
        if !is_bond && !has_device {
            continue; // lo, bridges, veth, tap, vlan...
        }
        let slaves: Vec<String> = if is_bond {
            read_trim(&dir.join("bonding/slaves"))
                .map(|s| s.split_whitespace().map(String::from).collect())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let up = read_trim(&dir.join("operstate"))
            .map(|s| s == "up")
            .unwrap_or(false);
        out.push(NicInfo {
            name: name.clone(),
            speed_mbps: read_trim(&dir.join("speed")).and_then(|s| parse_speed(&s)),
            mtu: read_trim(&dir.join("mtu"))
                .and_then(|s| s.parse().ok())
                .unwrap_or(1500),
            up,
            driver: fs::read_link(dir.join("device/driver"))
                .ok()
                .and_then(|p| p.file_name().map(|f| f.to_string_lossy().to_string())),
            is_bond,
            bond_mode: if is_bond {
                read_trim(&dir.join("bonding/mode"))
            } else {
                None
            },
            slaves,
            sriov_vfs: read_trim(&dir.join("device/sriov_numvfs"))
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    // Bond speed: bonds usually report their own speed, but fall back to / correct
    // from the slaves (sum for aggregating modes, max for active-backup).
    let snapshot = out.clone();
    for nic in out.iter_mut().filter(|n| n.is_bond) {
        let speeds: Vec<u64> = nic
            .slaves
            .iter()
            .filter_map(|s| {
                snapshot
                    .iter()
                    .find(|n| &n.name == s)
                    .and_then(|n| n.speed_mbps)
            })
            .collect();
        if speeds.is_empty() {
            continue;
        }
        let aggregate = nic
            .bond_mode
            .as_deref()
            .map(bond_aggregates)
            .unwrap_or(false);
        nic.speed_mbps = Some(if aggregate {
            speeds.iter().sum()
        } else {
            *speeds.iter().max().unwrap()
        });
    }
    // Slaves of a bond are not independent endpoints.
    let enslaved: Vec<String> = out.iter().flat_map(|n| n.slaves.clone()).collect();
    out.retain(|n| !enslaved.contains(&n.name));
    out
}

/// Fastest usable link on the node, in Mbit/s.
pub fn best_speed_mbps(nics: &[NicInfo]) -> Option<u64> {
    nics.iter()
        .filter(|n| n.up)
        .filter_map(|n| n.speed_mbps)
        .max()
}

/// Bandwidth cap (Mbit/s) for background traffic (live-migration, Ceph
/// recovery) so it uses at most `percent` of the link. Never below 100 Mbit/s.
pub fn background_cap_mbps(link_mbps: u64, percent: u64) -> u64 {
    (link_mbps.saturating_mul(percent.min(100)) / 100).max(100)
}

/// Warnings for a NIC that carries storage/cluster traffic.
/// `min_mbps` is the policy minimum (e.g. 10_000).
pub fn check_storage_nic(nic: &NicInfo, min_mbps: u64) -> Vec<String> {
    let mut w = Vec::new();
    if !nic.up {
        w.push(format!("{}: link is down", nic.name));
    }
    match nic.speed_mbps {
        None => w.push(format!("{}: link speed unknown", nic.name)),
        Some(s) if s < min_mbps => w.push(format!(
            "{}: {} is below the required {} (degraded link or 1 GbE fallback?)",
            nic.name,
            speed_label(s),
            speed_label(min_mbps)
        )),
        _ => {}
    }
    if nic.mtu < 9000 {
        w.push(format!(
            "{}: MTU {} (jumbo frames, MTU 9000, recommended for Ceph/migration)",
            nic.name, nic.mtu
        ));
    }
    if nic.is_bond && nic.slaves.len() < 2 {
        w.push(format!(
            "{}: bond has {} slave(s), no redundancy",
            nic.name,
            nic.slaves.len()
        ));
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn mk(root: &Path, name: &str, speed: &str, mtu: &str, state: &str, driver: &str) {
        let d = root.join("class/net").join(name);
        fs::create_dir_all(d.join("device")).unwrap();
        fs::write(d.join("speed"), format!("{}\n", speed)).unwrap();
        fs::write(d.join("mtu"), format!("{}\n", mtu)).unwrap();
        fs::write(d.join("operstate"), format!("{}\n", state)).unwrap();
        let drv = root.join("drivers").join(driver);
        fs::create_dir_all(&drv).unwrap();
        symlink(&drv, d.join("device/driver")).unwrap();
    }

    fn mk_bond(root: &Path, name: &str, mode: &str, slaves: &str) {
        let d = root.join("class/net").join(name);
        fs::create_dir_all(d.join("bonding")).unwrap();
        fs::write(d.join("bonding/mode"), format!("{} 4\n", mode)).unwrap();
        fs::write(d.join("bonding/slaves"), format!("{}\n", slaves)).unwrap();
        fs::write(d.join("mtu"), "9000\n").unwrap();
        fs::write(d.join("operstate"), "up\n").unwrap();
        fs::write(d.join("speed"), "-1\n").unwrap();
    }

    #[test]
    fn labels_cover_all_speeds() {
        let l: Vec<String> = KNOWN_SPEEDS_MBPS.iter().map(|s| speed_label(*s)).collect();
        assert_eq!(
            l,
            vec![
                "100 MbE", "1 GbE", "2.5 GbE", "5 GbE", "10 GbE", "25 GbE", "40 GbE", "50 GbE",
                "100 GbE"
            ]
        );
        assert_eq!(speed_label(20_000), "20 GbE");
    }

    #[test]
    fn detects_each_speed_and_driver() {
        let t = tempfile::tempdir().unwrap();
        let cases = [
            ("eth1g", "1000", "igb"),
            ("eth25", "25000", "mlx5_core"),
            ("eth100", "100000", "ice"),
            ("eth10", "10000", "ixgbe"),
        ];
        for (n, s, d) in cases {
            mk(t.path(), n, s, "1500", "up", d);
        }
        let nics = scan_nics(t.path());
        assert_eq!(nics.len(), 4);
        let by = |n: &str| nics.iter().find(|x| x.name == n).unwrap().clone();
        assert_eq!(by("eth100").speed_mbps, Some(100_000));
        assert_eq!(by("eth25").driver.as_deref(), Some("mlx5_core"));
        assert_eq!(best_speed_mbps(&nics), Some(100_000));
    }

    #[test]
    fn down_link_reports_unknown_speed() {
        let t = tempfile::tempdir().unwrap();
        mk(t.path(), "eth0", "-1", "1500", "down", "ixgbe");
        let nics = scan_nics(t.path());
        assert_eq!(nics[0].speed_mbps, None);
        assert!(!nics[0].up);
        assert_eq!(best_speed_mbps(&nics), None);
    }

    #[test]
    fn lacp_bond_sums_slaves_and_hides_them() {
        let t = tempfile::tempdir().unwrap();
        mk(t.path(), "ens1", "10000", "9000", "up", "ixgbe");
        mk(t.path(), "ens2", "10000", "9000", "up", "ixgbe");
        mk_bond(t.path(), "bond0", "802.3ad", "ens1 ens2");
        let nics = scan_nics(t.path());
        assert_eq!(nics.len(), 1);
        assert_eq!(nics[0].name, "bond0");
        assert_eq!(nics[0].speed_mbps, Some(20_000));
        assert!(check_storage_nic(&nics[0], 10_000).is_empty());
    }

    #[test]
    fn active_backup_bond_uses_max_not_sum() {
        let t = tempfile::tempdir().unwrap();
        mk(t.path(), "ens1", "25000", "9000", "up", "mlx5_core");
        mk(t.path(), "ens2", "25000", "9000", "up", "mlx5_core");
        mk_bond(t.path(), "bond0", "active-backup", "ens1 ens2");
        assert_eq!(scan_nics(t.path())[0].speed_mbps, Some(25_000));
    }

    #[test]
    fn virtual_devices_skipped() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join("class/net/lo")).unwrap();
        fs::create_dir_all(t.path().join("class/net/vmbr0")).unwrap();
        assert!(scan_nics(t.path()).is_empty());
    }

    #[test]
    fn warns_on_1g_fallback_and_small_mtu() {
        let t = tempfile::tempdir().unwrap();
        mk(t.path(), "eth0", "1000", "1500", "up", "ixgbe");
        let w = check_storage_nic(&scan_nics(t.path())[0], 10_000);
        assert_eq!(w.len(), 2);
        assert!(w[0].contains("1 GbE") && w[0].contains("10 GbE"));
        assert!(w[1].contains("MTU 1500"));
    }

    #[test]
    fn background_cap_scales_with_link() {
        assert_eq!(background_cap_mbps(1_000, 50), 500);
        assert_eq!(background_cap_mbps(10_000, 50), 5_000);
        assert_eq!(background_cap_mbps(100_000, 25), 25_000);
        assert_eq!(background_cap_mbps(100, 10), 100);
        assert_eq!(background_cap_mbps(10_000, 500), 10_000);
    }

    #[test]
    fn sriov_vfs_read() {
        let t = tempfile::tempdir().unwrap();
        mk(t.path(), "eth0", "25000", "9000", "up", "mlx5_core");
        fs::write(t.path().join("class/net/eth0/device/sriov_numvfs"), "8\n").unwrap();
        assert_eq!(scan_nics(t.path())[0].sriov_vfs, 8);
    }
}
