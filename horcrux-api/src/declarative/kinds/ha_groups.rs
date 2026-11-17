//! HA groups. Needs a node built with USE=cluster. HA *resources* (which
//! guests are protected) are not part of this section yet.
//!
//! ```yaml
//! ha_groups:
//!   - {name: prefer-a, nodes: [node-a, node-b], restricted: false, no_failback: false}
//! ```
//! A group that HA resources still use is never deleted (`blocked`).
use super::{ch, items, to_json, Kind, KindPlan};
use crate::declarative::{Action, Change, Live, Spec};
use crate::AppState;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::Arc;

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HaGroupSpec {
    pub name: String,
    pub nodes: Vec<String>,
    /// Only ever use the nodes listed here.
    #[serde(default, skip_serializing_if = "is_false")]
    pub restricted: bool,
    /// Don't migrate back to the preferred node after recovery.
    #[serde(default, skip_serializing_if = "is_false")]
    pub no_failback: bool,
}

#[derive(Debug, Clone)]
pub struct LiveHaGroup {
    pub group: HaGroupSpec,
    /// HA resources that still reference the group.
    pub resources: usize,
}

fn nodes(g: &HaGroupSpec) -> BTreeSet<&str> {
    g.nodes.iter().map(String::as_str).collect()
}

pub fn validate(v: &[HaGroupSpec]) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for g in v {
        if g.name.trim().is_empty() {
            return Err("HA group name is required".into());
        }
        if !seen.insert(&g.name) {
            return Err(format!("HA group {:?} is listed twice", g.name));
        }
        if g.nodes.is_empty() || g.nodes.iter().any(|n| n.trim().is_empty()) {
            return Err(format!("HA group {}: list at least one node", g.name));
        }
    }
    Ok(())
}

pub fn plan(want: &[HaGroupSpec], live: &[LiveHaGroup], prune: bool) -> KindPlan {
    let mut p = KindPlan::default();
    for w in want {
        match live.iter().find(|l| l.group.name == w.name) {
            None => p.changes.push(ch(
                "ha_group",
                &w.name,
                Action::Create,
                vec![format!("nodes {:?}", w.nodes)],
            )),
            Some(l) => {
                let mut d = Vec::new();
                if nodes(&l.group) != nodes(w) {
                    d.push(format!("nodes: {:?} -> {:?}", l.group.nodes, w.nodes));
                }
                if l.group.restricted != w.restricted {
                    d.push(format!(
                        "restricted: {} -> {}",
                        l.group.restricted, w.restricted
                    ));
                }
                if l.group.no_failback != w.no_failback {
                    d.push(format!(
                        "no_failback: {} -> {}",
                        l.group.no_failback, w.no_failback
                    ));
                }
                if !d.is_empty() {
                    p.changes.push(ch("ha_group", &w.name, Action::Update, d));
                }
            }
        }
    }
    if prune {
        for l in live
            .iter()
            .filter(|l| !want.iter().any(|w| w.name == l.group.name))
        {
            if l.resources > 0 {
                p.deletes.push(ch(
                    "ha_group",
                    &l.group.name,
                    Action::Blocked,
                    vec![format!(
                        "prune: {} HA resource(s) still use the group",
                        l.resources
                    )],
                ));
            } else {
                p.deletes
                    .push(ch("ha_group", &l.group.name, Action::Delete, vec![]));
            }
        }
    }
    p
}

pub fn export(live: &[LiveHaGroup]) -> Vec<HaGroupSpec> {
    let mut v: Vec<HaGroupSpec> = live.iter().map(|l| l.group.clone()).collect();
    for g in &mut v {
        g.nodes.sort();
    }
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

// ------------------------------------------------------------------ kind

pub struct HaGroups;

#[async_trait]
impl Kind for HaGroups {
    fn key(&self) -> &'static str {
        "ha_groups"
    }
    fn change_kinds(&self) -> &'static [&'static str] {
        &["ha_group"]
    }
    fn rank(&self) -> u32 {
        90
    }
    fn validate(&self, section: &Value) -> Result<(), String> {
        validate(&items::<HaGroupSpec>(section)?)
    }
    async fn gather(&self, state: &Arc<AppState>) -> Result<Value, String> {
        to_json(&export(&gather_live(state).await?))
    }
    fn plan(
        &self,
        section: &Value,
        live: &Value,
        prune: bool,
        whole: (&Spec, &Live),
        out: &mut KindPlan,
    ) {
        match (items::<HaGroupSpec>(section), items::<HaGroupSpec>(live)) {
            (Ok(w), Ok(l)) => {
                let l: Vec<LiveHaGroup> = l
                    .into_iter()
                    .map(|g| LiveHaGroup {
                        resources: whole.1.kinds.ha_usage.get(&g.name).copied().unwrap_or(0),
                        group: g,
                    })
                    .collect();
                out.merge(plan(&w, &l, prune));
            }
            (Err(e), _) | (_, Err(e)) => out.errors.push(format!("ha_groups: {e}")),
        }
    }
    async fn apply(
        &self,
        state: &Arc<AppState>,
        section: &Value,
        _live: &Value,
        c: &Change,
    ) -> Result<Option<String>, String> {
        apply(state, section, c).await
    }
}

#[cfg(feature = "cluster")]
async fn gather_live(state: &Arc<AppState>) -> Result<Vec<LiveHaGroup>, String> {
    Ok(state
        .ha_manager
        .list_groups()
        .await
        .into_iter()
        .map(|g| LiveHaGroup {
            resources: 0,
            group: HaGroupSpec {
                name: g.name,
                nodes: g.nodes,
                restricted: g.restricted,
                no_failback: g.no_failback,
            },
        })
        .collect())
}

#[cfg(not(feature = "cluster"))]
async fn gather_live(_: &Arc<AppState>) -> Result<Vec<LiveHaGroup>, String> {
    Err("this node was built without clustering (USE=cluster)".into())
}

#[cfg(feature = "cluster")]
async fn apply(
    state: &Arc<AppState>,
    section: &Value,
    c: &Change,
) -> Result<Option<String>, String> {
    use crate::api::ha_loop::{broadcast_group, GroupChange};
    let e = |x: horcrux_common::Error| format!("{x}");
    match c.action {
        Action::Create | Action::Update => {
            let want: Vec<HaGroupSpec> = items(section)?;
            let w = want
                .iter()
                .find(|g| g.name == c.name)
                .ok_or("not in spec")?;
            let group = crate::ha::HaGroup {
                name: w.name.clone(),
                nodes: w.nodes.clone(),
                restricted: w.restricted,
                no_failback: w.no_failback,
            };
            if c.action == Action::Create {
                state.ha_manager.add_group(group.clone()).await.map_err(e)?;
            } else {
                state
                    .ha_manager
                    .update_group(group.clone())
                    .await
                    .map_err(e)?;
            }
            broadcast_group(state, GroupChange::Put { group }).await;
            Ok(None)
        }
        Action::Delete => {
            state.ha_manager.remove_group(&c.name).await.map_err(e)?;
            broadcast_group(
                state,
                GroupChange::Delete {
                    name: c.name.clone(),
                },
            )
            .await;
            Ok(None)
        }
        a => Err(format!("no executor for ha_group {a:?}")),
    }
}

#[cfg(not(feature = "cluster"))]
async fn apply(_: &Arc<AppState>, _: &Value, _: &Change) -> Result<Option<String>, String> {
    Err("this server was built without clustering (USE=cluster)".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(name: &str, nodes: &[&str]) -> HaGroupSpec {
        HaGroupSpec {
            name: name.into(),
            nodes: nodes.iter().map(|s| s.to_string()).collect(),
            restricted: false,
            no_failback: false,
        }
    }
    fn l(name: &str, nodes: &[&str], resources: usize) -> LiveHaGroup {
        LiveHaGroup {
            group: g(name, nodes),
            resources,
        }
    }
    fn acts(p: &KindPlan) -> Vec<String> {
        p.changes
            .iter()
            .chain(&p.deletes)
            .map(|c| format!("{:?} {}", c.action, c.name))
            .collect()
    }

    #[test]
    fn create_update_and_node_order_is_not_drift() {
        let live = vec![l("a", &["n1", "n2"], 0), l("b", &["n1"], 0)];
        let mut b = g("b", &["n1"]);
        b.restricted = true;
        let p = plan(&[g("a", &["n2", "n1"]), b, g("c", &["n1"])], &live, false);
        assert_eq!(acts(&p), ["Update b", "Create c"]);
    }

    #[test]
    fn prune_deletes_unused_groups_and_blocks_used_ones() {
        let live = vec![
            l("keep", &["n1"], 0),
            l("gone", &["n1"], 0),
            l("used", &["n1"], 2),
        ];
        let p = plan(&[g("keep", &["n1"])], &live, true);
        assert_eq!(acts(&p), ["Delete gone", "Blocked used"]);
        assert!(acts(&plan(&[g("keep", &["n1"])], &live, false)).is_empty());
    }

    #[test]
    fn export_is_a_fixed_point() {
        let live = vec![l("b", &["n2", "n1"], 1), l("a", &["n1"], 0)];
        let spec = export(&live);
        assert_eq!(spec[0].name, "a");
        assert_eq!(spec[1].nodes, ["n1", "n2"]);
        validate(&spec).unwrap();
        assert!(acts(&plan(&spec, &live, true)).is_empty());
        let yaml = serde_yaml::to_string(&spec).unwrap();
        assert_eq!(
            serde_yaml::from_str::<Vec<HaGroupSpec>>(&yaml).unwrap(),
            spec
        );
    }

    #[test]
    fn validation() {
        assert!(validate(&[g("a", &[])]).is_err());
        assert!(validate(&[g("a", &["n"]), g("a", &["n"])])
            .unwrap_err()
            .contains("twice"));
        assert!(validate(&[g("", &["n"])]).is_err());
    }
}
