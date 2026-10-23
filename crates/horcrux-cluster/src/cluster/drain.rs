//! Placement planning for node drain (maintenance) and rebalance. Pure
//! functions over a snapshot of nodes and guests, so the rules - capacity
//! with headroom, architecture, pin, affinity, anti-affinity - are unit
//! tested without a cluster. The API layer (`horcrux-api` `api/drain.rs`)
//! gathers the snapshot from the nodes, executes the moves as live migrations
//! and records them.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Memory kept free on a target (a node is never filled to the brim).
pub const HEADROOM: f64 = 0.10;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlanNode {
    pub name: String,
    pub online: bool,
    /// In maintenance: takes no new guests.
    pub maintenance: bool,
    pub arch: String,
    pub mem_total_mb: u64,
    pub cpu_cores: u32,
}

/// Per-guest placement rules (set on the guest).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Rules {
    /// Only ever on this node (a drain of that node cannot move it).
    pub pin: Option<String>,
    /// Guests of one group run on the same node when possible.
    pub affinity_group: Option<String>,
    /// Guests of one group never share a node.
    pub anti_affinity_group: Option<String>,
    /// Never moved by drain or rebalance (falls back to Save state).
    pub no_migrate: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlanGuest {
    pub id: String,
    pub name: String,
    pub node: String,
    pub running: bool,
    pub memory_mb: u64,
    pub cpus: u32,
    pub arch: String,
    /// Why this guest cannot be live-migrated at all (not a QEMU VM, ...).
    pub immovable: Option<String>,
    pub rules: Rules,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Move {
    pub guest_id: String,
    pub name: String,
    pub from: String,
    pub to: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Unplaced {
    pub guest_id: String,
    pub name: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Plan {
    pub moves: Vec<Move>,
    pub unplaced: Vec<Unplaced>,
}

struct World<'a> {
    nodes: &'a [PlanNode],
    guests: &'a [PlanGuest],
    /// guest id -> node it will be on once the plan so far ran.
    at: HashMap<String, String>,
}

impl<'a> World<'a> {
    fn new(nodes: &'a [PlanNode], guests: &'a [PlanGuest]) -> Self {
        Self {
            nodes,
            guests,
            at: guests
                .iter()
                .map(|g| (g.id.clone(), g.node.clone()))
                .collect(),
        }
    }

    /// Memory of the running guests that will be on `node`.
    fn committed(&self, node: &str) -> u64 {
        self.guests
            .iter()
            .filter(|g| g.running && self.at.get(&g.id).map(String::as_str) == Some(node))
            .map(|g| g.memory_mb)
            .sum()
    }

    fn load(&self, n: &PlanNode) -> f64 {
        if n.mem_total_mb == 0 {
            return 1.0;
        }
        self.committed(&n.name) as f64 / n.mem_total_mb as f64
    }

    fn members_on(
        &self,
        group_of: impl Fn(&PlanGuest) -> Option<&String>,
        g: &PlanGuest,
        node: &str,
    ) -> usize {
        let Some(grp) = group_of(g) else { return 0 };
        self.guests
            .iter()
            .filter(|o| o.id != g.id && group_of(o) == Some(grp))
            .filter(|o| self.at.get(&o.id).map(String::as_str) == Some(node))
            .count()
    }

    /// Why `g` cannot go to `t` (None = it can).
    fn refuse(&self, g: &PlanGuest, t: &PlanNode) -> Option<String> {
        if t.name == self.at[&g.id] {
            return Some("already there".into());
        }
        if !t.online {
            return Some(format!("{} is offline", t.name));
        }
        if t.maintenance {
            return Some(format!("{} is in maintenance", t.name));
        }
        if t.arch != g.arch {
            return Some(format!(
                "{} is {} but the guest needs {}",
                t.name, t.arch, g.arch
            ));
        }
        if let Some(p) = &g.rules.pin {
            if *p != t.name {
                return Some(format!("pinned to {p}"));
            }
        }
        if self.members_on(|x| x.rules.anti_affinity_group.as_ref(), g, &t.name) > 0 {
            return Some(format!(
                "anti-affinity: a guest of its group already runs on {}",
                t.name
            ));
        }
        let cap = (t.mem_total_mb as f64 * (1.0 - HEADROOM)) as u64;
        if self.committed(&t.name) + g.memory_mb > cap {
            return Some(format!(
                "{} has no room ({} MB committed of {} MB, guest needs {} MB)",
                t.name,
                self.committed(&t.name),
                t.mem_total_mb,
                g.memory_mb
            ));
        }
        None
    }
}

/// Move every running guest off `source`: biggest first, each to the allowed
/// node with the lowest load afterwards (preferring the node that already
/// hosts members of its affinity group). Guests nobody can take are listed
/// with the reason; the caller saves their state instead.
pub fn plan_drain(source: &str, nodes: &[PlanNode], guests: &[PlanGuest]) -> Plan {
    let mut w = World::new(nodes, guests);
    let mut plan = Plan::default();
    let mut todo: Vec<&PlanGuest> = guests
        .iter()
        .filter(|g| g.node == source && g.running)
        .collect();
    todo.sort_by(|a, b| b.memory_mb.cmp(&a.memory_mb).then(a.id.cmp(&b.id)));
    for g in todo {
        let unplaced = |reason: String| Unplaced {
            guest_id: g.id.clone(),
            name: g.name.clone(),
            reason,
        };
        if let Some(why) = &g.immovable {
            plan.unplaced.push(unplaced(why.clone()));
            continue;
        }
        if g.rules.no_migrate {
            plan.unplaced
                .push(unplaced("marked never to migrate".into()));
            continue;
        }
        if let Some(p) = &g.rules.pin {
            if p == source {
                plan.unplaced.push(unplaced(format!("pinned to {source}")));
                continue;
            }
        }
        let mut reasons = Vec::new();
        let mut best: Option<(&PlanNode, (bool, f64))> = None;
        for n in nodes.iter().filter(|n| n.name != source) {
            match w.refuse(g, n) {
                Some(r) => reasons.push(r),
                None => {
                    let together =
                        w.members_on(|x| x.rules.affinity_group.as_ref(), g, &n.name) > 0;
                    let after =
                        (w.committed(&n.name) + g.memory_mb) as f64 / n.mem_total_mb.max(1) as f64;
                    // affinity first (true sorts before false via negation), then load
                    let key = (!together, after);
                    if best.as_ref().is_none_or(|(_, k)| key < *k) {
                        best = Some((n, key));
                    }
                }
            }
        }
        match best {
            Some((n, _)) => {
                w.at.insert(g.id.clone(), n.name.clone());
                plan.moves.push(Move {
                    guest_id: g.id.clone(),
                    name: g.name.clone(),
                    from: source.to_string(),
                    to: n.name.clone(),
                    reason: "drain".into(),
                });
            }
            None => plan.unplaced.push(unplaced(if reasons.is_empty() {
                "there is no other node in the cluster".into()
            } else {
                reasons.join("; ")
            })),
        }
    }
    plan
}

/// Even out memory load: repeatedly move a guest from the most to the least
/// loaded eligible node, while the spread exceeds `threshold_pct` points,
/// at most `max_moves` times, only movable running guests whose rules allow it.
pub fn plan_rebalance(
    nodes: &[PlanNode],
    guests: &[PlanGuest],
    max_moves: usize,
    threshold_pct: f64,
) -> Plan {
    let mut w = World::new(nodes, guests);
    let mut plan = Plan::default();
    let mut moved: Vec<String> = Vec::new();
    let usable: Vec<&PlanNode> = nodes
        .iter()
        .filter(|n| n.online && !n.maintenance)
        .collect();
    if usable.len() < 2 {
        return plan;
    }
    while plan.moves.len() < max_moves {
        let mut by_load: Vec<(&PlanNode, f64)> = usable.iter().map(|n| (*n, w.load(n))).collect();
        by_load.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        let (lo, lo_l) = by_load[0];
        let (hi, hi_l) = by_load[by_load.len() - 1];
        if (hi_l - lo_l) * 100.0 < threshold_pct {
            break;
        }
        let gap_mb = ((hi_l * hi.mem_total_mb as f64) - (lo_l * lo.mem_total_mb as f64)) / 2.0;
        // Largest guest that does not overshoot (the move must narrow the gap).
        let mut cands: Vec<&PlanGuest> = guests
            .iter()
            .filter(|g| g.running && w.at[&g.id] == hi.name && !moved.contains(&g.id))
            .filter(|g| g.immovable.is_none() && !g.rules.no_migrate && g.rules.pin.is_none())
            .filter(|g| (g.memory_mb as f64) <= gap_mb)
            .collect();
        cands.sort_by(|a, b| b.memory_mb.cmp(&a.memory_mb).then(a.id.cmp(&b.id)));
        let pick = cands.into_iter().find(|g| {
            // Keep affinity groups together: don't split a group apart.
            let split = g.rules.affinity_group.as_ref().is_some_and(|_| {
                w.members_on(|x| x.rules.affinity_group.as_ref(), g, &hi.name) > 0
            });
            !split && w.refuse(g, lo).is_none()
        });
        let Some(g) = pick else { break };
        w.at.insert(g.id.clone(), lo.name.clone());
        moved.push(g.id.clone());
        plan.moves.push(Move {
            guest_id: g.id.clone(),
            name: g.name.clone(),
            from: hi.name.clone(),
            to: lo.name.clone(),
            reason: "rebalance".into(),
        });
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(n: &str, mem: u64) -> PlanNode {
        PlanNode {
            name: n.into(),
            online: true,
            maintenance: false,
            arch: "x86_64".into(),
            mem_total_mb: mem,
            cpu_cores: 4,
        }
    }

    fn guest(id: &str, on: &str, mem: u64) -> PlanGuest {
        PlanGuest {
            id: id.into(),
            name: id.into(),
            node: on.into(),
            running: true,
            memory_mb: mem,
            cpus: 1,
            arch: "x86_64".into(),
            immovable: None,
            rules: Rules::default(),
        }
    }

    #[test]
    fn drain_goes_to_the_least_loaded_node_and_spreads() {
        let nodes = [node("a", 8000), node("b", 8000), node("c", 8000)];
        let guests = [
            guest("g1", "a", 2000),
            guest("g2", "a", 2000),
            guest("x", "b", 3000),
        ];
        let p = plan_drain("a", &nodes, &guests);
        assert!(p.unplaced.is_empty());
        let to: Vec<&str> = p.moves.iter().map(|m| m.to.as_str()).collect();
        // c is empty, then b (3000) vs c (2000): second guest also fits c
        // better than b.
        assert_eq!(to, ["c", "c"]);
        assert!(p.moves.iter().all(|m| m.from == "a" && m.reason == "drain"));
    }

    #[test]
    fn drain_respects_maintenance_arch_pin_headroom_and_never_overfills() {
        let mut m = node("m", 8000);
        m.maintenance = true;
        let mut arm = node("arm", 8000);
        arm.arch = "aarch64".into();
        let small = node("small", 2000);
        let nodes = [node("a", 8000), m, arm, small];
        let mut pinned = guest("pinned", "a", 100);
        pinned.rules.pin = Some("a".into());
        let big = guest("big", "a", 1900); // 2000 MB node, 10% headroom -> 1800 max
        let ok = guest("ok", "a", 500);
        let p = plan_drain("a", &nodes, &[pinned, big, ok]);
        assert_eq!(p.moves.len(), 1);
        assert_eq!(
            (p.moves[0].guest_id.as_str(), p.moves[0].to.as_str()),
            ("ok", "small")
        );
        let why: HashMap<_, _> = p
            .unplaced
            .iter()
            .map(|u| (u.guest_id.as_str(), u.reason.as_str()))
            .collect();
        assert!(why["pinned"].contains("pinned"));
        assert!(why["big"].contains("no room"), "{}", why["big"]);
        assert!(why["big"].contains("maintenance") && why["big"].contains("aarch64"));
    }

    #[test]
    fn anti_affinity_keeps_group_members_on_different_nodes() {
        let nodes = [node("a", 8000), node("b", 8000), node("c", 8000)];
        let mut g1 = guest("g1", "a", 1000);
        let mut g2 = guest("g2", "a", 900);
        g1.rules.anti_affinity_group = Some("db".into());
        g2.rules.anti_affinity_group = Some("db".into());
        let p = plan_drain("a", &nodes, &[g1, g2]);
        assert_eq!(p.moves.len(), 2);
        assert_ne!(p.moves[0].to, p.moves[1].to);
        // Only one other node: the second cannot be placed.
        let p = plan_drain("a", &[node("a", 8000), node("b", 8000)], &{
            let mut x = guest("x", "a", 100);
            let mut y = guest("y", "a", 50);
            x.rules.anti_affinity_group = Some("g".into());
            y.rules.anti_affinity_group = Some("g".into());
            [x, y]
        });
        assert_eq!(p.moves.len(), 1);
        assert!(p.unplaced[0].reason.contains("anti-affinity"));
    }

    #[test]
    fn affinity_prefers_the_node_hosting_the_group() {
        let nodes = [node("a", 8000), node("b", 8000), node("c", 8000)];
        let mut mate = guest("mate", "b", 3000);
        mate.rules.affinity_group = Some("app".into());
        let mut g = guest("g", "a", 500);
        g.rules.affinity_group = Some("app".into());
        let p = plan_drain("a", &nodes, &[mate, g]);
        assert_eq!(
            p.moves[0].to, "b",
            "b already hosts the group although c is emptier"
        );
    }

    #[test]
    fn immovable_and_no_migrate_guests_are_reported() {
        let nodes = [node("a", 8000), node("b", 8000)];
        let mut ct = guest("ct", "a", 100);
        ct.immovable = Some("containers cannot be moved between nodes".into());
        let mut nm = guest("nm", "a", 100);
        nm.rules.no_migrate = true;
        let mut stopped = guest("off", "a", 100);
        stopped.running = false;
        let p = plan_drain("a", &nodes, &[ct, nm, stopped]);
        assert!(p.moves.is_empty());
        assert_eq!(p.unplaced.len(), 2, "a stopped guest is not drained");
        assert!(
            plan_drain("a", &[node("a", 8000)], &[guest("g", "a", 1)]).unplaced[0]
                .reason
                .contains("no other node")
        );
    }

    #[test]
    fn rebalance_evens_out_within_limits() {
        let nodes = [node("a", 10000), node("b", 10000), node("c", 10000)];
        let guests: Vec<PlanGuest> = (0..6).map(|i| guest(&format!("g{i}"), "a", 1000)).collect();
        let p = plan_rebalance(&nodes, &guests, 10, 15.0);
        // 6000/0/0 -> 2000 each is 4 moves; stops once the spread is < 15 points
        assert!(p.moves.len() >= 3 && p.moves.len() <= 4, "{:?}", p.moves);
        assert!(p
            .moves
            .iter()
            .all(|m| m.from == "a" && m.reason == "rebalance"));
        let capped = plan_rebalance(&nodes, &guests, 2, 15.0);
        assert_eq!(capped.moves.len(), 2, "max_moves is honoured");
        assert!(plan_rebalance(&nodes, &guests, 0, 15.0).moves.is_empty());
        // a balanced cluster stays as it is
        let even = [
            guest("x", "a", 1000),
            guest("y", "b", 1000),
            guest("z", "c", 1000),
        ];
        assert!(plan_rebalance(&nodes, &even, 5, 15.0).moves.is_empty());
    }

    #[test]
    fn rebalance_skips_pinned_and_unmovable_and_needs_two_nodes() {
        let nodes = [node("a", 10000), node("b", 10000)];
        let mut pinned = guest("p", "a", 1000);
        pinned.rules.pin = Some("a".into());
        let mut nm = guest("n", "a", 1000);
        nm.rules.no_migrate = true;
        assert!(plan_rebalance(&nodes, &[pinned, nm], 5, 10.0)
            .moves
            .is_empty());
        assert!(
            plan_rebalance(&[node("a", 100)], &[guest("g", "a", 50)], 5, 1.0)
                .moves
                .is_empty()
        );
        let mut off = node("b", 10000);
        off.maintenance = true;
        assert!(
            plan_rebalance(&[node("a", 10000), off], &[guest("g", "a", 5000)], 5, 1.0)
                .moves
                .is_empty()
        );
    }
}
