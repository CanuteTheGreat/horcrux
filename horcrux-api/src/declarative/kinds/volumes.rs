//! Container volumes (Podman and Docker named volumes).
//!
//! ```yaml
//! volumes:
//!   - {name: data, runtime: podman, driver: local}   # runtime: podman (default) | docker
//! ```
//! Volumes hold data, so plan never deletes one: a volume the spec does not
//! list is `blocked` under `prune` (remove it by hand when you are sure).
//! Volumes are created, never edited: a different driver is `blocked`.
use super::containers::{resolve_runtime, RuntimeInfo};
use super::{ch, items, to_json, Kind, KindPlan};
use crate::declarative::{Action, Change, Live, Spec};
use crate::AppState;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct VolumeSpec {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveVolume {
    pub name: String,
    pub runtime: String,
    pub driver: String,
}

pub fn valid_name(n: &str) -> bool {
    let mut c = n.chars();
    matches!(c.next(), Some(x) if x.is_ascii_alphanumeric())
        && c.all(|x| x.is_ascii_alphanumeric() || matches!(x, '_' | '.' | '-'))
}

pub fn validate(v: &[VolumeSpec]) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for x in v {
        if !valid_name(&x.name) {
            return Err(format!(
                "volume {:?}: use letters, digits, '_', '.' or '-' (starting with a letter or digit)",
                x.name
            ));
        }
        if let Some(r) = x.runtime.as_deref() {
            if r != "podman" && r != "docker" {
                return Err(format!(
                    "volume {}: runtime must be podman or docker",
                    x.name
                ));
            }
        }
        if let Some(d) = &x.driver {
            if d.is_empty() || d.starts_with('-') {
                return Err(format!("volume {}: invalid driver {d:?}", x.name));
            }
        }
        if !seen.insert((x.name.clone(), x.runtime.clone())) {
            return Err(format!("volume {:?} is listed twice", x.name));
        }
    }
    Ok(())
}

pub fn plan(
    want: &[VolumeSpec],
    live: &[LiveVolume],
    prune: bool,
    rts: &[RuntimeInfo],
) -> KindPlan {
    let mut p = KindPlan::default();
    let rt_of = |w: &VolumeSpec| resolve_runtime(w.runtime.as_deref(), rts);
    for w in want {
        let rt = rt_of(w);
        let name = format!("{rt}/{}", w.name);
        match live.iter().find(|l| l.name == w.name && l.runtime == rt) {
            None => {
                if let Some(i) = rts.iter().find(|r| r.runtime == rt && !r.available) {
                    p.changes.push(ch(
                        "volume",
                        &name,
                        Action::Blocked,
                        vec![format!(
                            "node lacks {}: {} {}",
                            i.reason.as_deref().unwrap_or("runtime"),
                            i.message.as_deref().unwrap_or("runtime not available"),
                            i.hint.as_deref().unwrap_or("")
                        )
                        .trim()
                        .to_string()],
                    ));
                } else {
                    p.changes.push(ch("volume", &name, Action::Create, vec![]));
                }
            }
            Some(l) => {
                if let Some(d) = &w.driver {
                    if &l.driver != d {
                        p.changes.push(ch(
                            "volume",
                            &name,
                            Action::Blocked,
                            vec![
                                format!("driver: {} -> {d}", l.driver),
                                "volumes can't change driver: re-create it by hand".into(),
                            ],
                        ));
                    }
                }
            }
        }
    }
    if prune {
        for l in live.iter().filter(|l| {
            !want
                .iter()
                .any(|w| w.name == l.name && rt_of(w) == l.runtime)
        }) {
            p.deletes.push(ch(
                "volume",
                &format!("{}/{}", l.runtime, l.name),
                Action::Blocked,
                vec!["prune: volumes hold data and are never deleted by apply".into()],
            ));
        }
    }
    p
}

pub fn export(live: &[LiveVolume]) -> Vec<VolumeSpec> {
    let mut v: Vec<VolumeSpec> = live
        .iter()
        .map(|l| VolumeSpec {
            name: l.name.clone(),
            runtime: Some(l.runtime.clone()),
            driver: Some(l.driver.clone()).filter(|d| !d.is_empty()),
        })
        .collect();
    v.sort_by(|a, b| (&a.runtime, &a.name).cmp(&(&b.runtime, &b.name)));
    v
}

// ------------------------------------------------------------------ kind

pub struct Volumes;

#[async_trait]
impl Kind for Volumes {
    fn key(&self) -> &'static str {
        "volumes"
    }
    fn change_kinds(&self) -> &'static [&'static str] {
        &["volume"]
    }
    fn rank(&self) -> u32 {
        55
    }
    fn validate(&self, section: &Value) -> Result<(), String> {
        validate(&items::<VolumeSpec>(section)?)
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
        match (items::<VolumeSpec>(section), items::<VolumeSpec>(live)) {
            (Ok(w), Ok(l)) => {
                let l: Vec<LiveVolume> = l
                    .into_iter()
                    .map(|v| LiveVolume {
                        name: v.name,
                        runtime: v.runtime.unwrap_or_else(|| "podman".into()),
                        driver: v.driver.unwrap_or_default(),
                    })
                    .collect();
                out.merge(plan(&w, &l, prune, &whole.1.kinds.runtimes));
            }
            (Err(e), _) | (_, Err(e)) => out.errors.push(format!("volumes: {e}")),
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

#[cfg(any(feature = "docker", feature = "podman"))]
async fn gather_live(state: &Arc<AppState>) -> Result<Vec<LiveVolume>, String> {
    let rts = crate::container::probe::probe_all_cached(false).await;
    let up = |n: &str| rts.iter().any(|r| r.runtime == n && r.available);
    let mut out = Vec::new();
    let mut push = |rt: &str, v: Vec<crate::container::volume::VolumeInfo>| {
        out.extend(v.into_iter().map(|v| LiveVolume {
            name: v.name,
            runtime: rt.to_string(),
            driver: v.driver,
        }));
    };
    #[cfg(feature = "podman")]
    if up("podman") {
        if let Ok(v) = state.volume_manager.podman.list_volumes().await {
            push("podman", v);
        }
    }
    #[cfg(feature = "docker")]
    if up("docker") {
        if let Ok(v) = state.volume_manager.docker.list_volumes().await {
            push("docker", v);
        }
    }
    let _ = (&up, state);
    Ok(out)
}

#[cfg(not(any(feature = "docker", feature = "podman")))]
async fn gather_live(_: &Arc<AppState>) -> Result<Vec<LiveVolume>, String> {
    Err("this node has no Docker or Podman support (USE=docker or USE=podman)".into())
}

#[cfg(any(feature = "docker", feature = "podman"))]
async fn apply(
    state: &Arc<AppState>,
    section: &Value,
    c: &Change,
) -> Result<Option<String>, String> {
    let (rt, name) = c
        .name
        .split_once('/')
        .ok_or_else(|| "bad volume change name".to_string())?;
    if c.action != Action::Create {
        return Err(format!("no executor for volume {:?}", c.action));
    }
    let want: Vec<VolumeSpec> = items(section)?;
    let driver = want
        .iter()
        .find(|v| v.name == name && v.runtime.as_deref().map(|r| r == rt).unwrap_or(true))
        .and_then(|v| v.driver.clone());
    crate::api::unavailable::container_engine(rt).map_err(crate::declarative::exec::api_msg)?;
    let lacks = |e: horcrux_common::Error| match e {
        horcrux_common::Error::NodeLacks {
            reason,
            message,
            hint,
        } => format!("node lacks {reason}: {message} {hint}"),
        other => format!("{other:?}"),
    };
    match rt {
        #[cfg(feature = "podman")]
        "podman" => {
            state
                .volume_manager
                .podman
                .create_volume(name, driver.as_deref())
                .await
                .map(|_| ())
                .map_err(lacks)?;
        }
        #[cfg(feature = "docker")]
        "docker" => {
            state
                .volume_manager
                .docker
                .create_volume(name, driver.as_deref())
                .await
                .map(|_| ())
                .map_err(lacks)?;
        }
        other => {
            return Err(format!(
                "the {other} runtime is not compiled into this server"
            ))
        }
    }
    Ok(None)
}

#[cfg(not(any(feature = "docker", feature = "podman")))]
async fn apply(_: &Arc<AppState>, _: &Value, _: &Change) -> Result<Option<String>, String> {
    Err("this server was built without Docker/Podman support".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(name: &str, rt: Option<&str>) -> VolumeSpec {
        VolumeSpec {
            name: name.into(),
            runtime: rt.map(Into::into),
            driver: None,
        }
    }
    fn l(name: &str, rt: &str) -> LiveVolume {
        LiveVolume {
            name: name.into(),
            runtime: rt.into(),
            driver: "local".into(),
        }
    }
    fn acts(p: &KindPlan) -> Vec<String> {
        p.changes
            .iter()
            .chain(&p.deletes)
            .map(|c| format!("{:?} {}", c.action, c.name))
            .collect()
    }
    fn rt(name: &str, ok: bool) -> RuntimeInfo {
        RuntimeInfo {
            runtime: name.into(),
            available: ok,
            reason: (!ok).then(|| "not-installed".into()),
            message: (!ok).then(|| format!("{name} is not installed.")),
            hint: None,
        }
    }

    #[test]
    fn podman_by_default_and_per_runtime_namespaces() {
        let live = vec![l("data", "docker")];
        let p = plan(
            &[v("data", None), v("data", Some("docker"))],
            &live,
            false,
            &[],
        );
        assert_eq!(acts(&p), ["Create podman/data"]);
    }

    #[test]
    fn prune_never_deletes_data_and_says_so() {
        let live = vec![l("keep", "podman"), l("other", "podman")];
        let p = plan(&[v("keep", None)], &live, true, &[]);
        assert_eq!(acts(&p), ["Blocked podman/other"]);
        assert!(acts(&plan(&[v("keep", None)], &live, false, &[])).is_empty());
    }

    #[test]
    fn driver_change_is_blocked_and_a_missing_runtime_blocks_creation() {
        let mut w = v("a", Some("podman"));
        w.driver = Some("nfs".into());
        let p = plan(&[w], &[l("a", "podman")], false, &[]);
        assert_eq!(acts(&p), ["Blocked podman/a"]);
        let rts = vec![rt("podman", false), rt("docker", false)];
        let p = plan(&[v("n", None)], &[], false, &rts);
        assert_eq!(acts(&p), ["Blocked podman/n"]);
        assert!(p.changes[0].details[0].contains("node lacks not-installed"));
    }

    #[test]
    fn export_is_a_fixed_point_and_validation_works() {
        let live = vec![l("b", "podman"), l("a", "docker")];
        let spec = export(&live);
        validate(&spec).unwrap();
        assert!(acts(&plan(&spec, &live, true, &[])).is_empty());
        assert!(validate(&[v("-bad", None)]).is_err());
        assert!(validate(&[v("a", Some("lxc"))]).is_err());
        assert!(validate(&[v("a", None), v("a", None)])
            .unwrap_err()
            .contains("twice"));
    }
}
