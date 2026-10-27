//! Ceph daemon placement for mixed-hardware clusters.
//!
//! Pure functions, no I/O. Rules: OSDs only on `Storage`-role nodes with an
//! uplink >= `min_osd_mbps` and at least one data disk; MONs on an odd number
//! (3 by default) of *distinct* nodes, preferring non-OSD-heavy and
//! non-weak nodes but never a node whose uplink is below `min_mon_mbps`.

use super::roles::{osd_allowed, NodeCaps};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CephCandidate {
    pub name: String,
    pub caps: NodeCaps,
    pub uplink_mbps: u64,
    /// Unused data disks suitable for OSDs (e.g. NVMe).
    pub data_disks: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CephPlan {
    pub osd_nodes: Vec<String>,
    pub mon_nodes: Vec<String>,
    /// (node, reason) for every node refused as OSD host.
    pub osd_refused: Vec<(String, String)>,
    pub warnings: Vec<String>,
}

/// Plan OSD and MON placement. `mon_count` is forced odd (rounded down, min 1).
pub fn plan(nodes: &[CephCandidate], min_osd_mbps: u64, min_mon_mbps: u64, mon_count: usize) -> CephPlan {
    let mut out = CephPlan::default();
    for n in nodes {
        match osd_allowed(&n.caps, n.uplink_mbps, min_osd_mbps) {
            Err(e) => out.osd_refused.push((n.name.clone(), e)),
            Ok(()) if n.data_disks == 0 => out
                .osd_refused
                .push((n.name.clone(), "no unused data disks".to_string())),
            Ok(()) => out.osd_nodes.push(n.name.clone()),
        }
    }
    let want = if mon_count % 2 == 0 { mon_count.saturating_sub(1) } else { mon_count }.max(1);
    // Candidates: any node with enough uplink. Prefer non-OSD nodes first? No:
    // spread across distinct nodes, preferring faster uplinks, then name.
    let mut cand: Vec<&CephCandidate> =
        nodes.iter().filter(|n| n.uplink_mbps >= min_mon_mbps).collect();
    cand.sort_by(|a, b| b.uplink_mbps.cmp(&a.uplink_mbps).then(a.name.cmp(&b.name)));
    out.mon_nodes = cand.iter().take(want).map(|n| n.name.clone()).collect();
    if out.mon_nodes.len() < want {
        out.warnings.push(format!(
            "only {} node(s) eligible for {} MONs",
            out.mon_nodes.len(),
            want
        ));
    }
    if out.mon_nodes.len() % 2 == 0 && !out.mon_nodes.is_empty() {
        out.mon_nodes.pop();
        out.warnings.push("MON count made odd".to_string());
    }
    if out.osd_nodes.len() < 3 {
        out.warnings.push(format!(
            "{} OSD host(s): 3x replication with host failure domain needs >= 3",
            out.osd_nodes.len()
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::roles::NodeRole;

    fn n(name: &str, role: NodeRole, mbps: u64, disks: u32) -> CephCandidate {
        CephCandidate {
            name: name.into(),
            caps: NodeCaps { role, ..Default::default() },
            uplink_mbps: mbps,
            data_disks: disks,
        }
    }

    #[test]
    fn owner_cluster_shape() {
        let mut v: Vec<_> = (1..=4).map(|i| n(&format!("r6525-{i}"), NodeRole::Storage, 25_000, 8)).collect();
        v.push(n("r430-1", NodeRole::Compute, 10_000, 0));
        v.push(n("old-1", NodeRole::Witness, 1_000, 0));
        let p = plan(&v, 10_000, 1_000, 3);
        assert_eq!(p.osd_nodes.len(), 4);
        assert_eq!(p.mon_nodes.len(), 3);
        assert!(p.osd_refused.iter().any(|(n, _)| n == "old-1"));
        assert!(p.warnings.is_empty());
    }

    #[test]
    fn slow_or_diskless_storage_node_refused() {
        let v = vec![
            n("a", NodeRole::Storage, 1_000, 4),
            n("b", NodeRole::Storage, 25_000, 0),
        ];
        let p = plan(&v, 10_000, 1_000, 3);
        assert!(p.osd_nodes.is_empty());
        assert_eq!(p.osd_refused.len(), 2);
        assert!(p.warnings.iter().any(|w| w.contains("OSD host")));
    }

    #[test]
    fn even_mon_count_rounds_down_and_mons_distinct() {
        let v: Vec<_> = (1..=6).map(|i| n(&format!("n{i}"), NodeRole::Compute, 10_000, 0)).collect();
        let p = plan(&v, 10_000, 1_000, 4);
        assert_eq!(p.mon_nodes.len(), 3);
        let mut s = p.mon_nodes.clone();
        s.sort();
        s.dedup();
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn too_few_mon_candidates_warns() {
        let v = vec![n("a", NodeRole::Compute, 100, 0), n("b", NodeRole::Compute, 10_000, 0)];
        let p = plan(&v, 10_000, 1_000, 3);
        assert_eq!(p.mon_nodes, vec!["b".to_string()]);
        assert!(p.warnings.iter().any(|w| w.contains("MON")));
    }
}
