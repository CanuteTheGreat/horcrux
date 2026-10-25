//! Node roles and per-node placement limits for mixed-hardware clusters.
//!
//! Pure functions, no I/O. A node is `Storage` (Ceph OSD host, also runs
//! guests), `Compute` (guests only), or `Witness` (quorum/infra only, never
//! guests or OSDs). Weak nodes (e.g. a 32 GB PowerEdge 1950) get explicit
//! limits so heavy guests are never placed there.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum NodeRole {
    Storage,
    #[default]
    Compute,
    Witness,
}

impl NodeRole {
    pub fn runs_guests(self) -> bool {
        !matches!(self, NodeRole::Witness)
    }
    pub fn hosts_osds(self) -> bool {
        matches!(self, NodeRole::Storage)
    }
}

/// Operator-set capabilities of one node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct NodeCaps {
    pub role: NodeRole,
    /// Free-form labels (e.g. "lab", "nvme", "old-hw").
    #[serde(default)]
    pub labels: Vec<String>,
    /// Max vCPUs of a single guest (None = unlimited).
    #[serde(default)]
    pub max_guest_vcpus: Option<u32>,
    /// Max memory in bytes of a single guest (None = unlimited).
    #[serde(default)]
    pub max_guest_memory: Option<u64>,
}

/// What a guest needs from its host.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GuestNeeds {
    pub vcpus: u32,
    pub memory: u64,
    /// Node must carry all of these labels.
    pub required_labels: Vec<String>,
}

/// `Ok(())` if the node may host the guest, else a human-readable reason
/// (shown in the UI as "why ineligible").
pub fn eligible(caps: &NodeCaps, need: &GuestNeeds) -> Result<(), String> {
    if !caps.role.runs_guests() {
        return Err("node is witness-only and does not run guests".to_string());
    }
    if let Some(max) = caps.max_guest_vcpus {
        if need.vcpus > max {
            return Err(format!("guest needs {} vCPUs, node limit is {}", need.vcpus, max));
        }
    }
    if let Some(max) = caps.max_guest_memory {
        if need.memory > max {
            return Err(format!("guest needs {} B RAM, limit {}", need.memory, max));
        }
    }
    for l in &need.required_labels {
        if !caps.labels.iter().any(|x| x == l) {
            return Err(format!("node lacks required label '{}'", l));
        }
    }
    Ok(())
}

/// Check whether a node may host Ceph OSDs: role must be Storage and the
/// uplink must be at least `min_mbps` (OSDs never belong on 1 GbE nodes).
pub fn osd_allowed(caps: &NodeCaps, uplink_mbps: u64, min_mbps: u64) -> Result<(), String> {
    if !caps.role.hosts_osds() {
        return Err("node role is not storage".to_string());
    }
    if uplink_mbps < min_mbps {
        return Err(format!("uplink {} < min {} Mbps", uplink_mbps, min_mbps));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    fn weak() -> NodeCaps {
        NodeCaps {
            role: NodeRole::Compute,
            labels: vec!["lab".into(), "old-hw".into()],
            max_guest_vcpus: Some(4),
            max_guest_memory: Some(8 * GIB),
        }
    }

    #[test]
    fn default_role_is_compute() {
        assert_eq!(NodeRole::default(), NodeRole::Compute);
        assert!(NodeRole::Compute.runs_guests());
        assert!(!NodeRole::Compute.hosts_osds());
        assert!(NodeRole::Storage.hosts_osds());
    }

    #[test]
    fn witness_never_hosts_guests() {
        let c = NodeCaps {
            role: NodeRole::Witness,
            ..Default::default()
        };
        let n = GuestNeeds {
            vcpus: 1,
            memory: GIB,
            ..Default::default()
        };
        assert!(eligible(&c, &n).unwrap_err().contains("witness"));
    }

    #[test]
    fn weak_node_rejects_heavy_guest_accepts_light() {
        let heavy = GuestNeeds {
            vcpus: 16,
            memory: 64 * GIB,
            ..Default::default()
        };
        assert!(eligible(&weak(), &heavy).unwrap_err().contains("vCPUs"));
        let mem = GuestNeeds {
            vcpus: 2,
            memory: 64 * GIB,
            ..Default::default()
        };
        assert!(eligible(&weak(), &mem).unwrap_err().contains("RAM"));
        let light = GuestNeeds {
            vcpus: 2,
            memory: 4 * GIB,
            ..Default::default()
        };
        assert!(eligible(&weak(), &light).is_ok());
    }

    #[test]
    fn required_labels_enforced() {
        let n = GuestNeeds {
            vcpus: 1,
            memory: GIB,
            required_labels: vec!["nvme".into()],
        };
        assert!(eligible(&weak(), &n).unwrap_err().contains("nvme"));
        let n2 = GuestNeeds {
            required_labels: vec!["lab".into()],
            ..n
        };
        assert!(eligible(&weak(), &n2).is_ok());
    }

    #[test]
    fn osd_gating_by_role_and_speed() {
        let storage = NodeCaps {
            role: NodeRole::Storage,
            ..Default::default()
        };
        assert!(osd_allowed(&storage, 25_000, 10_000).is_ok());
        let slow = osd_allowed(&storage, 1_000, 10_000);
        assert!(slow.unwrap_err().contains("min"));
        let wrong = osd_allowed(&weak(), 25_000, 10_000);
        assert!(wrong.unwrap_err().contains("storage"));
    }

    #[test]
    fn role_serde_lowercase() {
        assert_eq!(serde_json::to_string(&NodeRole::Witness).unwrap(), "\"witness\"");
        let c: NodeCaps = serde_json::from_str(r#"{"role":"storage"}"#).unwrap();
        assert_eq!(c.role, NodeRole::Storage);
        assert!(c.labels.is_empty());
    }
}
