//! The per-node path view: for every SAN LUN, its multipath map, its
//! block devices and each path's address and state. Built from the iSCSI
//! sessions and multipathd's maps (pure, fixture-tested), so it shows
//! paths even when multipath is off or multipathd is down.

use super::iscsi::Session;
use super::multipath::Map;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// A path's overall state.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PathState {
    /// Carrying I/O.
    Active,
    /// Up, in a standby/ghost group (ALUA non-optimised).
    Standby,
    /// The checker or device-mapper marks it failed.
    Failed,
    /// Not in a multipath map (multipath off or not yet built).
    Unmapped,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PathView {
    /// Kernel device (`sdb`).
    pub dev: String,
    /// `iscsi`, `nvme` or `fc`.
    pub transport: String,
    pub target: Option<String>,
    /// Remote address (`10.0.0.1:3260`).
    pub portal: Option<String>,
    /// Local address of the connection.
    pub local_address: Option<String>,
    pub lun: Option<u32>,
    pub state: PathState,
    pub dm_state: Option<String>,
    pub checker_state: Option<String>,
    pub device_state: Option<String>,
    pub session_state: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LunView {
    pub wwid: String,
    /// Multipath map name (`None` = no map: a single-path device).
    pub map: Option<String>,
    /// The device to use (`/dev/mapper/<map>` or `/dev/<sdX>`).
    pub device: String,
    pub size_bytes: Option<u64>,
    pub vendor: Option<String>,
    pub product: Option<String>,
    pub target: Option<String>,
    pub lun: Option<u32>,
    pub paths: Vec<PathView>,
    pub active_paths: usize,
    pub failed_paths: usize,
}

struct DiskRef<'a> {
    s: &'a Session,
    lun: u32,
    state: &'a str,
}

fn path_state(dm: &str, chk: &str) -> PathState {
    match (dm, chk) {
        (_, "faulty" | "shaky" | "delayed") | ("failed", _) => PathState::Failed,
        (_, "ghost") => PathState::Standby,
        _ => PathState::Active,
    }
}

/// Assemble the LUN list. `wwids` maps kernel device names to their WWID
/// (for disks outside a map); `size_of` reads a block device's size.
pub fn assemble(
    sessions: &[Session],
    maps: &[Map],
    wwids: &HashMap<String, String>,
    size_of: impl Fn(&str) -> Option<u64>,
) -> Vec<LunView> {
    let mut disks: HashMap<&str, DiskRef> = HashMap::new();
    for s in sessions {
        for d in &s.disks {
            disks.insert(
                d.dev.as_str(),
                DiskRef {
                    s,
                    lun: d.lun,
                    state: d.state.as_str(),
                },
            );
        }
    }
    let view_of = |dev: &str, state: PathState, mp: Option<&super::multipath::MapPath>| {
        let d = disks.get(dev);
        PathView {
            dev: dev.to_string(),
            transport: if let Some(t) = d
                .and_then(|d| d.s.transport.as_deref())
                .filter(|t| *t == "fc")
            {
                t.into()
            } else if d.is_some() || dev.starts_with("sd") {
                "iscsi".into()
            } else if dev.starts_with("nvme") {
                "nvme".into()
            } else {
                "scsi".into()
            },
            target: d
                .map(|d| d.s.iqn.clone())
                .or_else(|| mp.and_then(|p| p.target.clone())),
            portal: d.map(|d| d.s.portal.clone()),
            local_address: d
                .and_then(|d| d.s.local_address.clone())
                .or_else(|| mp.and_then(|p| p.host_adapter.clone())),
            lun: d.map(|d| d.lun),
            state: match d {
                // A failed session beats a stale "ready" from the checker.
                Some(d) if !d.s.logged_in() || d.state != "running" => PathState::Failed,
                _ => state,
            },
            dm_state: mp.map(|p| p.dm_state.clone()),
            checker_state: mp.map(|p| p.checker_state.clone()),
            device_state: mp
                .map(|p| p.device_state.clone())
                .or_else(|| d.map(|d| d.state.to_string())),
            session_state: d.and_then(|d| d.s.session_state.clone()),
        }
    };

    let mut out = Vec::new();
    let mut mapped: std::collections::HashSet<&str> = Default::default();
    for m in maps {
        let mut paths = Vec::new();
        for p in &m.paths {
            mapped.insert(p.dev.as_str());
            paths.push(view_of(
                &p.dev,
                path_state(&p.dm_state, &p.checker_state),
                Some(p),
            ));
        }
        paths.sort_by(|a, b| a.dev.cmp(&b.dev));
        let first = m.paths.first().and_then(|p| disks.get(p.dev.as_str()));
        out.push(finish(LunView {
            wwid: m.wwid.clone(),
            map: Some(m.name.clone()),
            device: format!("/dev/mapper/{}", m.name),
            size_bytes: size_of(&m.sysfs),
            vendor: Some(m.vendor.clone()).filter(|v| !v.is_empty()),
            product: Some(m.product.clone()).filter(|v| !v.is_empty()),
            target: first
                .map(|d| d.s.iqn.clone())
                .or_else(|| m.paths.first().and_then(|p| p.target.clone())),
            lun: first.map(|d| d.lun),
            paths,
            active_paths: 0,
            failed_paths: 0,
        }));
    }

    // iSCSI disks outside any map, grouped by WWID (or target+LUN).
    let mut rest: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    let mut devs: Vec<&&str> = disks.keys().collect();
    devs.sort();
    for dev in devs {
        if mapped.contains(*dev) {
            continue;
        }
        let d = &disks[*dev];
        let key = wwids
            .get(*dev)
            .cloned()
            .unwrap_or_else(|| format!("{}:lun{}", d.s.iqn, d.lun));
        rest.entry(key).or_default().push(dev);
    }
    for (wwid, devs) in rest {
        // A disk whose WWID already has a map but which multipathd has not
        // taken into it (it appeared before the WWID was known): it belongs
        // to that LUN, shown unmapped and not counted as carrying I/O.
        if let Some(l) = out.iter_mut().find(|l| l.map.is_some() && l.wwid == wwid) {
            l.paths.extend(
                devs.iter()
                    .map(|dev| view_of(dev, PathState::Unmapped, None)),
            );
            l.paths.sort_by(|a, b| a.dev.cmp(&b.dev));
            recount(l);
            continue;
        }
        let d = &disks[devs[0]];
        out.push(finish(LunView {
            device: format!("/dev/{}", devs[0]),
            size_bytes: size_of(devs[0]),
            wwid,
            map: None,
            vendor: None,
            product: None,
            target: Some(d.s.iqn.clone()),
            lun: Some(d.lun),
            paths: devs
                .iter()
                .map(|dev| view_of(dev, PathState::Unmapped, None))
                .collect(),
            active_paths: 0,
            failed_paths: 0,
        }));
    }
    out
}

fn finish(mut l: LunView) -> LunView {
    recount(&mut l);
    l
}

pub fn recount(l: &mut LunView) {
    // Without a map every disk is a usable path; with one, only the map's
    // active paths carry I/O.
    let mapped = l.map.is_some();
    l.active_paths = l
        .paths
        .iter()
        .filter(|p| p.state == PathState::Active || (!mapped && p.state == PathState::Unmapped))
        .count();
    l.failed_paths = l
        .paths
        .iter()
        .filter(|p| p.state == PathState::Failed)
        .count();
}

/// Disks that belong to a mapped LUN but are not in its map (the
/// reconcile loop hands them to `multipathd add path`).
pub fn unmapped_in_maps(luns: &[LunView]) -> Vec<String> {
    luns.iter()
        .filter(|l| l.map.is_some())
        .flat_map(|l| l.paths.iter())
        .filter(|p| p.state == PathState::Unmapped)
        .map(|p| p.dev.clone())
        .collect()
}

/// Size of `/sys/block/<dev>` in bytes.
pub fn sysfs_size(dev: &str) -> Option<u64> {
    if dev.is_empty() || dev.contains('/') {
        return None;
    }
    std::fs::read_to_string(format!("/sys/block/{dev}/size"))
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(|s| s * 512)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::san::iscsi::{parse_sessions, SessionDisk};
    use crate::san::multipath::parse_maps_json;

    fn sess(
        iqn: &str,
        portal: &str,
        local: &str,
        ok: bool,
        disks: &[(&str, u32, &str)],
    ) -> Session {
        Session {
            iqn: iqn.into(),
            portal: portal.into(),
            local_address: Some(local.into()),
            session_state: Some(if ok { "LOGGED_IN" } else { "FAILED" }.into()),
            disks: disks
                .iter()
                .map(|(d, l, s)| SessionDisk {
                    dev: (*d).into(),
                    lun: *l,
                    state: (*s).into(),
                })
                .collect(),
            ..Default::default()
        }
    }

    const IQN: &str = "iqn.2003-01.org.linux-iscsi.san:sn.a1";

    fn four_paths(failed: bool) -> (Vec<Session>, Vec<Map>) {
        let s = vec![
            sess(
                IQN,
                "10.10.1.1:3260",
                "10.10.1.20",
                true,
                &[("sdb", 0, "running")],
            ),
            sess(
                IQN,
                "10.10.1.2:3260",
                "10.10.1.20",
                true,
                &[("sdc", 0, "running")],
            ),
            sess(
                IQN,
                "10.10.2.1:3260",
                "10.10.2.20",
                !failed,
                &[(
                    "sdd",
                    0,
                    if failed {
                        "transport-offline"
                    } else {
                        "running"
                    },
                )],
            ),
            sess(
                IQN,
                "10.10.2.2:3260",
                "10.10.2.20",
                !failed,
                &[(
                    "sde",
                    0,
                    if failed {
                        "transport-offline"
                    } else {
                        "running"
                    },
                )],
            ),
        ];
        let chk = |d: &str| {
            if failed && (d == "sdd" || d == "sde") {
                ("failed", "faulty")
            } else {
                ("active", "ready")
            }
        };
        let paths: Vec<String> = ["sdb", "sdc", "sdd", "sde"]
            .iter()
            .map(|d| {
                let (dm, ck) = chk(d);
                format!(r#"{{"dev":"{d}","dm_st":"{dm}","chk_st":"{ck}","dev_st":"running","target_wwnn":"{IQN}","host_adapter":"10.10.1.20"}}"#)
            })
            .collect();
        let json = format!(
            r#"{{"maps":[{{"name":"3600140501","uuid":"3600140501","sysfs":"dm-3","dm_st":"active","vend":"LIO-ORG","prod":"lun0","path_groups":[{{"group":1,"paths":[{}]}}]}}]}}"#,
            paths.join(",")
        );
        (s, parse_maps_json(&json).unwrap())
    }

    #[test]
    fn four_iscsi_paths_in_one_map() {
        let (s, m) = four_paths(false);
        let v = assemble(&s, &m, &HashMap::new(), |d| {
            (d == "dm-3").then_some(1 << 30)
        });
        assert_eq!(v.len(), 1);
        let l = &v[0];
        assert_eq!(l.device, "/dev/mapper/3600140501");
        assert_eq!(l.size_bytes, Some(1 << 30));
        assert_eq!(l.target.as_deref(), Some(IQN));
        assert_eq!(l.lun, Some(0));
        assert_eq!((l.active_paths, l.failed_paths), (4, 0));
        let portals: Vec<_> = l.paths.iter().map(|p| p.portal.clone().unwrap()).collect();
        assert_eq!(
            portals,
            vec![
                "10.10.1.1:3260",
                "10.10.1.2:3260",
                "10.10.2.1:3260",
                "10.10.2.2:3260"
            ]
        );
        assert_eq!(l.paths[2].local_address.as_deref(), Some("10.10.2.20"));
    }

    #[test]
    fn a_dead_nic_shows_two_failed_paths() {
        let (s, m) = four_paths(true);
        let v = assemble(&s, &m, &HashMap::new(), |_| None);
        assert_eq!((v[0].active_paths, v[0].failed_paths), (2, 2));
        assert_eq!(v[0].paths[2].state, PathState::Failed);
        assert_eq!(v[0].paths[2].session_state.as_deref(), Some("FAILED"));
    }

    #[test]
    fn unmapped_disks_group_by_wwid() {
        let s = vec![
            sess(
                IQN,
                "10.10.1.1:3260",
                "10.10.1.20",
                true,
                &[("sdb", 0, "running"), ("sdc", 1, "running")],
            ),
            sess(
                IQN,
                "10.10.2.1:3260",
                "10.10.2.20",
                true,
                &[("sdd", 0, "running"), ("sde", 1, "running")],
            ),
        ];
        let w: HashMap<String, String> =
            [("sdb", "w0"), ("sdd", "w0"), ("sdc", "w1"), ("sde", "w1")]
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect();
        let v = assemble(&s, &[], &w, |_| None);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].wwid, "w0");
        assert_eq!(v[0].map, None);
        assert_eq!(v[0].device, "/dev/sdb");
        assert_eq!(v[0].paths.len(), 2);
        assert!(v[0].paths.iter().all(|p| p.state == PathState::Unmapped));
        assert_eq!(v[1].lun, Some(1));
        // without WWIDs: target + LUN
        let v = assemble(&s, &[], &HashMap::new(), |_| None);
        assert_eq!(v[0].wwid, format!("{IQN}:lun0"));
    }

    #[test]
    fn a_disk_left_out_of_its_map_joins_that_lun_unmapped() {
        let (s, mut m) = four_paths(false);
        let left_out = m[0].paths.pop().unwrap().dev;
        let w: HashMap<String, String> = [(left_out.clone(), m[0].wwid.clone())]
            .into_iter()
            .collect();
        let v = assemble(&s, &m, &w, |_| None);
        assert_eq!(v.len(), 1, "one LUN, never a second phantom one: {v:?}");
        assert_eq!(v[0].paths.len(), 4);
        assert_eq!(v[0].active_paths, 3);
        let p = v[0].paths.iter().find(|p| p.dev == left_out).unwrap();
        assert_eq!(p.state, PathState::Unmapped);
        assert_eq!(unmapped_in_maps(&v), vec![left_out]);
    }

    #[test]
    fn session_fixture_round_trip() {
        let text = "Target: iqn.2003-01.org.linux-iscsi.san:sn.a1 (non-flash)\n\tCurrent Portal: 10.10.1.1:3260,1\n\t\tiSCSI Session State: LOGGED_IN\n\t\tscsi2 Channel 00 Id 0 Lun: 0\n\t\t\tAttached scsi disk sdb\t\tState: running\n";
        let v = assemble(&parse_sessions(text), &[], &HashMap::new(), |_| None);
        assert_eq!(v[0].paths[0].portal.as_deref(), Some("10.10.1.1:3260"));
        assert_eq!(v[0].active_paths, 1);
    }
}
