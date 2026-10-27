//! Idle power-down / wake-on-demand planning for lab nodes.
//!
//! Pure functions, no I/O. Only nodes carrying the opt-in label
//! [`POWER_LABEL`] are ever powered down. A node is never shut down while it
//! runs guests, hosts OSDs, is HA-pinned, or when doing so would break
//! quorum or leave fewer than `min_online` nodes running.

/// Opt-in label: only nodes with this label may be powered down/woken.
pub const POWER_LABEL: &str = "power-managed";

/// How a powered-off node is woken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeMethod {
    /// Wake-on-LAN magic packet to this MAC.
    Wol([u8; 6]),
    /// BMC power-on (IPMI/Redfish) through the node's fencing config.
    Bmc,
}

#[derive(Debug, Clone)]
pub struct PowerNode {
    pub name: String,
    pub labels: Vec<String>,
    pub powered_on: bool,
    pub running_guests: u32,
    pub cpu_percent: f32,
    pub hosts_osds: bool,
    /// Pinned by HA / passthrough devices; never powered down.
    pub pinned: bool,
    /// Votes this node contributes to quorum.
    pub votes: u32,
    pub wake: Option<WakeMethod>,
}

impl PowerNode {
    fn managed(&self) -> bool {
        self.labels.iter().any(|l| l == POWER_LABEL)
    }
}

#[derive(Debug, Clone)]
pub struct PowerPolicy {
    /// CPU % below which a guest-free node counts as idle.
    pub idle_cpu_percent: f32,
    /// Never power down below this many online nodes.
    pub min_online: usize,
}

impl Default for PowerPolicy {
    fn default() -> Self {
        Self {
            idle_cpu_percent: 5.0,
            min_online: 3,
        }
    }
}

/// Nodes that may be shut down now, with the reason each one is safe.
/// Greedy and quorum-safe: after each pick the remaining online votes must
/// still be a strict majority of all votes.
pub fn plan_power_down(nodes: &[PowerNode], policy: &PowerPolicy) -> Vec<String> {
    let total_votes: u32 = nodes.iter().map(|n| n.votes).sum();
    let mut online_votes: u32 = nodes.iter().filter(|n| n.powered_on).map(|n| n.votes).sum();
    let mut online = nodes.iter().filter(|n| n.powered_on).count();
    let mut out = Vec::new();
    let mut cands: Vec<&PowerNode> = nodes
        .iter()
        .filter(|n| {
            n.powered_on
                && n.managed()
                && !n.pinned
                && !n.hosts_osds
                && n.running_guests == 0
                && n.cpu_percent < policy.idle_cpu_percent
                && n.wake.is_some()
        })
        .collect();
    // Fewest votes first so quorum is preserved longest.
    cands.sort_by(|a, b| a.votes.cmp(&b.votes).then_with(|| a.name.cmp(&b.name)));
    for n in cands {
        if online <= policy.min_online {
            break;
        }
        let after = online_votes - n.votes;
        if after * 2 <= total_votes {
            continue;
        }
        online_votes = after;
        online -= 1;
        out.push(n.name.clone());
    }
    out
}

/// Nodes to wake so `needed` more guest-capable nodes are online. Only
/// powered-off managed nodes with a wake method qualify.
pub fn plan_wake(nodes: &[PowerNode], needed: usize) -> Vec<String> {
    let mut v: Vec<&PowerNode> = nodes
        .iter()
        .filter(|n| !n.powered_on && n.managed() && n.wake.is_some())
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v.into_iter().take(needed).map(|n| n.name.clone()).collect()
}

/// Build a 102-byte Wake-on-LAN magic packet (6x 0xFF, then MAC x16).
pub fn wol_magic_packet(mac: [u8; 6]) -> Vec<u8> {
    let mut p = vec![0xFFu8; 6];
    for _ in 0..16 {
        p.extend_from_slice(&mac);
    }
    p
}

/// Parse "aa:bb:cc:dd:ee:ff" (or '-' separated).
pub fn parse_mac(s: &str) -> Option<[u8; 6]> {
    let parts: Vec<&str> = s.split(|c| c == ':' || c == '-').collect();
    if parts.len() != 6 {
        return None;
    }
    let mut m = [0u8; 6];
    for (i, p) in parts.iter().enumerate() {
        if p.len() != 2 {
            return None;
        }
        m[i] = u8::from_str_radix(p, 16).ok()?;
    }
    Some(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, on: bool) -> PowerNode {
        PowerNode {
            name: name.to_string(),
            labels: vec![POWER_LABEL.to_string()],
            powered_on: on,
            running_guests: 0,
            cpu_percent: 1.0,
            hosts_osds: false,
            pinned: false,
            votes: 1,
            wake: Some(WakeMethod::Bmc),
        }
    }

    fn cluster(n: usize) -> Vec<PowerNode> {
        (0..n).map(|i| node(&format!("n{i}"), true)).collect()
    }

    #[test]
    fn idle_managed_node_powered_down_keeping_min_online() {
        let nodes = cluster(5);
        let p = PowerPolicy {
            idle_cpu_percent: 5.0,
            min_online: 3,
        };
        // quorum 5 votes: 3 online is the majority floor.
        assert_eq!(plan_power_down(&nodes, &p), vec!["n0", "n1"]);
    }

    #[test]
    fn busy_osd_pinned_or_unmanaged_nodes_kept() {
        let mut nodes = cluster(6);
        nodes[0].running_guests = 1;
        nodes[1].hosts_osds = true;
        nodes[2].pinned = true;
        nodes[3].labels.clear();
        nodes[4].cpu_percent = 50.0;
        let p = PowerPolicy {
            idle_cpu_percent: 5.0,
            min_online: 1,
        };
        assert_eq!(plan_power_down(&nodes, &p), vec!["n5"]);
    }

    #[test]
    fn never_breaks_quorum() {
        let nodes = cluster(4);
        let p = PowerPolicy {
            idle_cpu_percent: 5.0,
            min_online: 1,
        };
        // 4 votes: keeping 3 is a majority, 2 is not.
        assert_eq!(plan_power_down(&nodes, &p).len(), 1);
    }

    #[test]
    fn no_wake_method_never_powered_down() {
        let mut nodes = cluster(5);
        for n in nodes.iter_mut() {
            n.wake = None;
        }
        assert!(plan_power_down(&nodes, &PowerPolicy::default()).is_empty());
    }

    #[test]
    fn wake_picks_off_managed_nodes() {
        let mut nodes = cluster(4);
        nodes[1].powered_on = false;
        nodes[2].powered_on = false;
        nodes[3].powered_on = false;
        nodes[3].labels.clear();
        assert_eq!(plan_wake(&nodes, 5), vec!["n1", "n2"]);
        assert_eq!(plan_wake(&nodes, 1), vec!["n1"]);
    }

    #[test]
    fn magic_packet_layout_and_mac_parse() {
        let mac = parse_mac("aa:bb:cc:dd:ee:01").unwrap();
        assert_eq!(parse_mac("AA-BB-CC-DD-EE-01"), Some(mac));
        assert_eq!(parse_mac("aa:bb:cc"), None);
        assert_eq!(parse_mac("zz:bb:cc:dd:ee:01"), None);
        let p = wol_magic_packet(mac);
        assert_eq!(p.len(), 102);
        assert!(p[..6].iter().all(|b| *b == 0xFF));
        assert_eq!(&p[96..], &mac);
    }
}
