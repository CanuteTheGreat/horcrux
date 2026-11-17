//! Containers: Podman (default) / Docker / LXC / Incus / LXD.
//!
//! ```yaml
//! containers:
//!   - name: web
//!     image: docker.io/library/nginx:1.27   # rootfs/template for LXC, image for the rest
//!     runtime: podman      # podman | docker | lxc | lxd | incus | system (incus, else lxd)
//!                          # absent = podman, falling back to docker when podman is missing
//!     memory: 256          # MB, cpus: 1 (absent = not limited / not compared)
//!     cpus: 1
//!     ports: ["8080:80"]
//!     env: {TZ: UTC}       # non-secret only: PASSWORD/TOKEN/SECRET... keys are refused
//!     mounts: ["data:/var/lib/data", "/srv/www:/usr/share/nginx/html:ro"]
//!     command: [nginx, -g, "daemon off;"]
//!     network: backend
//!     restart: unless-stopped        # no | always | unless-stopped | on-failure[:N]
//!     power: running
//!     replace: false       # true: a differing container is deleted and re-created
//! ```
//!
//! What the runtime lets us read back is limited: Horcrux tracks a
//! container's limits and options in memory, so after an API restart a
//! container looks like "limits 0, no options" until it is re-created. The
//! plan treats those as *unknown*, never as drift.
use super::{ch, items, to_json, Kind, KindPlan};
use crate::declarative::{Action, Change, Live, Power, Spec};
use crate::AppState;
use async_trait::async_trait;
use horcrux_common::{ContainerConfig, ContainerOptions, ContainerStatus};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ContainerSpec {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    pub image: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpus: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mounts: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power: Option<Power>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replace: bool,
}

impl ContainerSpec {
    pub fn container_id(&self) -> String {
        self.id.clone().unwrap_or_else(|| self.name.clone())
    }

    /// The creation options this spec asks for.
    pub fn options(&self) -> ContainerOptions {
        ContainerOptions {
            env: self.env.iter().map(|(k, v)| format!("{k}={v}")).collect(),
            ports: self.ports.clone(),
            volumes: self.mounts.clone(),
            command: self.command.clone(),
            network: self.network.clone(),
            network_aliases: Vec::new(),
            restart: self.restart.clone(),
        }
    }
}

/// One runtime as the live probe sees it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RuntimeInfo {
    pub runtime: String,
    pub available: bool,
    pub reason: Option<String>,
    pub message: Option<String>,
    pub hint: Option<String>,
}

const RUNTIMES: &[&str] = &["podman", "docker", "lxc", "lxd", "incus", "system"];
const OCI: &[&str] = &["podman", "docker"];

pub fn runtime_name(r: &horcrux_common::ContainerRuntime) -> &'static str {
    use horcrux_common::ContainerRuntime as R;
    match r {
        R::Podman => "podman",
        R::Docker => "docker",
        R::Lxc => "lxc",
        R::Lxd => "lxd",
        R::Incus => "incus",
    }
}

pub fn parse_runtime(s: &str) -> Option<horcrux_common::ContainerRuntime> {
    use horcrux_common::ContainerRuntime as R;
    Some(match s {
        "podman" => R::Podman,
        "docker" => R::Docker,
        "lxc" => R::Lxc,
        "lxd" => R::Lxd,
        "incus" => R::Incus,
        _ => return None,
    })
}

fn is_available(rts: &[RuntimeInfo], name: &str) -> bool {
    // No probe data (unit tests, probe failed): assume it works and let the
    // manager answer.
    rts.is_empty() || rts.iter().any(|r| r.runtime == name && r.available)
}

/// The runtime a spec means. Podman is the default; Docker is the fallback
/// when only it works; `system` prefers Incus over LXD.
pub fn resolve_runtime(spec: Option<&str>, rts: &[RuntimeInfo]) -> String {
    match spec {
        Some("system") => ["incus", "lxd"]
            .into_iter()
            .find(|r| is_available(rts, r) && !rts.is_empty())
            .unwrap_or("incus")
            .to_string(),
        Some(r) => r.to_string(),
        None => OCI
            .iter()
            .find(|r| is_available(rts, r) && !rts.is_empty())
            .copied()
            .unwrap_or("podman")
            .to_string(),
    }
}

/// `docker.io/library/nginx` == `nginx:latest`.
pub fn normalize_image(s: &str) -> String {
    let mut s = s.trim();
    for p in ["docker.io/", "index.docker.io/", "localhost/"] {
        if let Some(r) = s.strip_prefix(p) {
            s = r;
        }
    }
    let s = s.strip_prefix("library/").unwrap_or(s);
    let last = s.rsplit('/').next().unwrap_or(s);
    if last.contains(':') || s.contains('@') {
        s.to_string()
    } else {
        format!("{s}:latest")
    }
}

const SECRET_WORDS: &[&str] = &[
    "PASSWORD",
    "PASSWD",
    "SECRET",
    "TOKEN",
    "API_KEY",
    "APIKEY",
    "PRIVATE_KEY",
    "CREDENTIAL",
    "ACCESS_KEY",
];

/// Env keys that look like they hold a secret: refused in a spec and never
/// exported.
pub fn secret_looking(key: &str) -> bool {
    let k = key.to_ascii_uppercase();
    SECRET_WORDS.iter().any(|w| k.contains(w))
}

fn norm_port(p: &str) -> String {
    if p.contains('/') {
        p.to_string()
    } else {
        format!("{p}/tcp")
    }
}

fn norm_mount(m: &str) -> String {
    if m.split(':').count() == 2 {
        format!("{m}:rw")
    } else {
        m.to_string()
    }
}

fn set_of<I: IntoIterator<Item = String>>(i: I) -> BTreeSet<String> {
    i.into_iter().collect()
}

fn live_env(o: &ContainerOptions) -> BTreeMap<String, String> {
    o.env
        .iter()
        .filter_map(|e| e.split_once('='))
        .filter(|(k, _)| !secret_looking(k))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

pub fn validate(v: &[ContainerSpec]) -> Result<(), String> {
    let mut names = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for c in v {
        let n = &c.name;
        if !names.insert(n.clone()) {
            return Err(format!("container {n:?} is listed twice"));
        }
        if !ids.insert(c.container_id()) {
            return Err(format!("container id {:?} is used twice", c.container_id()));
        }
        if n.trim().is_empty() || c.image.trim().is_empty() {
            return Err(format!("container {n:?}: name and image are required"));
        }
        let id = c.container_id();
        if id.is_empty()
            || !id
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-'))
        {
            return Err(format!(
                "container {n}: id {id:?} may only hold letters, digits, _ . -"
            ));
        }
        if let Some(r) = c.runtime.as_deref() {
            if !RUNTIMES.contains(&r) {
                return Err(format!(
                    "container {n}: unknown runtime {r:?} (podman, docker, lxc, lxd, incus or system)"
                ));
            }
        }
        if c.memory == Some(0) || c.cpus == Some(0) {
            return Err(format!("container {n}: memory and cpus must be > 0"));
        }
        for k in c.env.keys() {
            if secret_looking(k) {
                return Err(format!(
                    "container {n}: env {k} looks like a secret; a spec never holds secret values \
                     (mount a secret file instead)"
                ));
            }
        }
        let oci = c
            .runtime
            .as_deref()
            .map(|r| OCI.contains(&r))
            .unwrap_or(true);
        let o = c.options();
        if !oci && (!o.is_empty() || c.restart.is_some()) {
            return Err(format!(
                "container {n}: ports/env/mounts/command/network/restart only apply to the podman and docker runtimes"
            ));
        }
        o.validate().map_err(|e| format!("container {n}: {e}"))?;
        if let Some(r) = &c.restart {
            let ok = matches!(
                r.as_str(),
                "no" | "always" | "unless-stopped" | "on-failure"
            ) || r
                .strip_prefix("on-failure:")
                .map(|x| x.parse::<u32>().is_ok())
                .unwrap_or(false);
            if !ok {
                return Err(format!(
                    "container {n}: restart must be no, always, unless-stopped or on-failure[:N]"
                ));
            }
        }
    }
    Ok(())
}

fn lacks(info: &RuntimeInfo) -> String {
    format!(
        "node lacks {}: {} {}",
        info.reason.as_deref().unwrap_or("runtime"),
        info.message.as_deref().unwrap_or("runtime not available"),
        info.hint.as_deref().unwrap_or("")
    )
    .trim()
    .to_string()
}

/// Does the live container carry options we can read back? (After an API
/// restart Horcrux no longer knows what a container was created with.)
fn tracked(l: &ContainerSpec) -> bool {
    !(l.ports.is_empty()
        && l.env.is_empty()
        && l.mounts.is_empty()
        && l.command.is_empty()
        && l.network.is_none()
        && l.restart.is_none())
}

/// Differences between the spec and a live container (in spec shape); only
/// what the spec states and what the runtime reports reliably.
pub fn diff(w: &ContainerSpec, l: &ContainerSpec, rts: &[RuntimeInfo]) -> Vec<String> {
    let mut d = Vec::new();
    if let (Some(_), Some(have)) = (&w.runtime, &l.runtime) {
        let want = resolve_runtime(w.runtime.as_deref(), rts);
        // a `system` spec accepts either system runtime
        let ok = *have == want || (w.runtime.as_deref() == Some("system") && have == "lxd");
        if !ok {
            d.push(format!("runtime: {have} -> {want}"));
        }
    }
    if normalize_image(&l.image) != normalize_image(&w.image) {
        d.push(format!("image: {} -> {}", l.image, w.image));
    }
    if let (Some(m), Some(lm)) = (w.memory, l.memory) {
        if lm != m {
            d.push(format!("memory: {lm} -> {m}"));
        }
    }
    if let (Some(c), Some(lc)) = (w.cpus, l.cpus) {
        if lc != c {
            d.push(format!("cpus: {lc} -> {c}"));
        }
    }
    // Options are only readable for containers Horcrux itself tracks.
    if tracked(l) {
        if !w.ports.is_empty() {
            let (a, b) = (
                set_of(l.ports.iter().map(|p| norm_port(p))),
                set_of(w.ports.iter().map(|p| norm_port(p))),
            );
            if a != b {
                d.push(format!("ports: {a:?} -> {b:?}"));
            }
        }
        if !w.mounts.is_empty() {
            let (a, b) = (
                set_of(l.mounts.iter().map(|p| norm_mount(p))),
                set_of(w.mounts.iter().map(|p| norm_mount(p))),
            );
            if a != b {
                d.push(format!("mounts: {a:?} -> {b:?}"));
            }
        }
        if !w.env.is_empty() && l.env != w.env {
            d.push(format!("env: {:?} -> {:?}", l.env, w.env));
        }
        if !w.command.is_empty() && l.command != w.command {
            d.push(format!("command: {:?} -> {:?}", l.command, w.command));
        }
        if w.network.is_some() && l.network != w.network {
            d.push(format!("network: {:?} -> {:?}", l.network, w.network));
        }
        if w.restart.is_some() && l.restart != w.restart {
            d.push(format!("restart: {:?} -> {:?}", l.restart, w.restart));
        }
    }
    d
}

/// `live` is the exported live list (spec shape): `power` is `None` for a
/// paused or unknown container.
pub fn plan(
    want: &[ContainerSpec],
    live: &[ContainerSpec],
    runtimes: &[RuntimeInfo],
    prune: bool,
) -> KindPlan {
    let mut p = KindPlan::default();
    for w in want {
        let rt = resolve_runtime(w.runtime.as_deref(), runtimes);
        match live.iter().find(|l| l.name == w.name) {
            None => {
                if let Some(info) = runtimes.iter().find(|r| r.runtime == rt && !r.available) {
                    p.changes
                        .push(ch("container", &w.name, Action::Blocked, vec![lacks(info)]));
                    continue;
                }
                p.changes.push(ch(
                    "container",
                    &w.name,
                    Action::Create,
                    vec![format!("{rt} {}", w.image)],
                ));
                if w.power == Some(Power::Running) {
                    p.changes
                        .push(ch("container", &w.name, Action::Start, vec![]));
                }
            }
            Some(l) => {
                let d = diff(w, l, runtimes);
                let mut replaced = false;
                if !d.is_empty() {
                    let mut d = d;
                    if w.replace {
                        d.push("replace: deleted and re-created (named volumes are kept)".into());
                        p.changes.push(ch("container", &w.name, Action::Update, d));
                        replaced = true;
                    } else {
                        d.push(
                            "containers can't be reconfigured in place: set `replace: true` to \
                             delete and re-create it"
                                .into(),
                        );
                        p.changes.push(ch("container", &w.name, Action::Blocked, d));
                    }
                }
                if !replaced {
                    match (w.power, l.power) {
                        (Some(Power::Running), Some(Power::Running)) => {}
                        (Some(Power::Running), _) => {
                            p.changes
                                .push(ch("container", &w.name, Action::Start, vec![]))
                        }
                        (Some(Power::Stopped), Some(Power::Stopped)) => {}
                        (Some(Power::Stopped), _) => {
                            p.changes
                                .push(ch("container", &w.name, Action::Stop, vec![]))
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    if prune {
        for l in live
            .iter()
            .filter(|l| !want.iter().any(|w| w.name == l.name))
        {
            if l.power != Some(Power::Stopped) {
                p.deletes.push(ch(
                    "container",
                    &l.name,
                    Action::Blocked,
                    vec!["prune: the container is running; stop it first".into()],
                ));
            } else {
                p.deletes
                    .push(ch("container", &l.name, Action::Delete, vec![]));
            }
        }
    }
    p
}

/// The live containers as spec items (what `gather` returns and `export` writes).
pub fn export(live: &[ContainerConfig]) -> Vec<ContainerSpec> {
    let mut v: Vec<ContainerSpec> = live
        .iter()
        .map(|l| ContainerSpec {
            name: l.name.clone(),
            id: (l.id != l.name).then(|| l.id.clone()),
            runtime: Some(runtime_name(&l.runtime).to_string()),
            image: l.rootfs.clone(),
            memory: (l.memory > 0).then_some(l.memory),
            cpus: (l.cpus > 0).then_some(l.cpus),
            ports: l.options.ports.clone(),
            env: live_env(&l.options),
            mounts: l.options.volumes.clone(),
            command: l.options.command.clone(),
            network: l.options.network.clone(),
            restart: l.options.restart.clone(),
            power: match l.status {
                ContainerStatus::Running => Some(Power::Running),
                ContainerStatus::Stopped => Some(Power::Stopped),
                _ => None,
            },
            replace: false,
        })
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

// ------------------------------------------------------------------ kind

fn lacks_err(e: horcrux_common::Error) -> String {
    match e {
        horcrux_common::Error::NodeLacks {
            reason,
            message,
            hint,
        } => format!("node lacks {reason}: {message} {hint}"),
        other => format!("{other:?}"),
    }
}

pub struct Containers;

#[async_trait]
impl Kind for Containers {
    fn key(&self) -> &'static str {
        "containers"
    }
    fn change_kinds(&self) -> &'static [&'static str] {
        &["container"]
    }
    fn rank(&self) -> u32 {
        60
    }
    fn validate(&self, section: &Value) -> Result<(), String> {
        validate(&items::<ContainerSpec>(section)?)
    }
    async fn gather(&self, state: &Arc<AppState>) -> Result<Value, String> {
        to_json(&export(&state.container_manager.list_containers().await))
    }
    fn plan(
        &self,
        section: &Value,
        live: &Value,
        prune: bool,
        whole: (&Spec, &Live),
        out: &mut KindPlan,
    ) {
        match (
            items::<ContainerSpec>(section),
            items::<ContainerSpec>(live),
        ) {
            (Ok(w), Ok(l)) => out.merge(plan(&w, &l, &whole.1.kinds.runtimes, prune)),
            (Err(e), _) | (_, Err(e)) => out.errors.push(format!("containers: {e}")),
        }
    }
    async fn apply(
        &self,
        state: &Arc<AppState>,
        section: &Value,
        live: &Value,
        c: &Change,
    ) -> Result<Option<String>, String> {
        apply(state, section, live, c).await
    }
}

async fn apply(
    state: &Arc<AppState>,
    section: &Value,
    live: &Value,
    c: &Change,
) -> Result<Option<String>, String> {
    let want: Vec<ContainerSpec> = items(section)?;
    let live_items: Vec<ContainerSpec> = items(live)?;
    let rts = crate::declarative::kinds::ctx::gather_ctx(state)
        .await
        .runtimes;
    let mgr = state.container_manager.clone();
    let w = want.iter().find(|w| w.name == c.name);
    let live_c = live_items.iter().find(|l| l.name == c.name);
    let id_of = || -> Result<String, String> {
        live_c
            .map(|l| l.container_id())
            .or_else(|| w.map(|w| w.container_id()))
            .ok_or_else(|| format!("container {} not found", c.name))
    };
    let create = |state: Arc<AppState>, w: ContainerSpec, rts: Vec<RuntimeInfo>| async move {
        let rt = resolve_runtime(w.runtime.as_deref(), &rts);
        let runtime = parse_runtime(&rt).ok_or_else(|| format!("unknown runtime {rt}"))?;
        let system = !matches!(rt.as_str(), "podman" | "docker");
        let cfg = ContainerConfig {
            id: w.container_id(),
            name: w.name.clone(),
            runtime,
            memory: w.memory.unwrap_or(if system { 512 } else { 0 }),
            cpus: w.cpus.unwrap_or(if system { 1 } else { 0 }),
            rootfs: w.image.clone(),
            status: ContainerStatus::Stopped,
            options: w.options(),
            created_at: None,
            lxc: None,
        };
        let m = state.container_manager.clone();
        let created = tokio::spawn(async move { m.create_container(cfg).await })
            .await
            .map_err(|e| format!("container create task failed: {e}"))?
            .map_err(lacks_err)?;
        Ok::<String, String>(created.id)
    };
    match c.action {
        Action::Create => {
            let id = create(state.clone(), w.ok_or("not in spec")?.clone(), rts).await?;
            Ok(Some(format!("id {id}")))
        }
        Action::Update => {
            // replace: stop, delete, create again (and start it if it ran)
            let w = w.ok_or("not in spec")?.clone();
            let id = id_of()?;
            let was = live_c.and_then(|l| l.power);
            if was != Some(Power::Stopped) {
                let m = mgr.clone();
                let sid = id.clone();
                let _ = tokio::spawn(async move { m.stop_container(&sid).await }).await;
            }
            let m = mgr.clone();
            let did = id.clone();
            tokio::spawn(async move { m.delete_container(&did).await })
                .await
                .map_err(|e| format!("container delete task failed: {e}"))?
                .map_err(lacks_err)?;
            let nid = create(state.clone(), w.clone(), rts).await?;
            if w.power == Some(Power::Running) || (w.power.is_none() && was == Some(Power::Running))
            {
                mgr.start_container(&nid).await.map_err(lacks_err)?;
            }
            Ok(Some(format!("re-created as id {nid}")))
        }
        Action::Start => {
            let id = id_of()?;
            // a paused container is resumed, anything else started
            let status = mgr.get_container(&id).await.map(|x| x.status);
            if matches!(status, Ok(ContainerStatus::Paused)) {
                mgr.resume_container(&id).await.map_err(lacks_err)?;
            } else {
                let m = mgr.clone();
                tokio::spawn(async move { m.start_container(&id).await })
                    .await
                    .map_err(|e| format!("container start task failed: {e}"))?
                    .map_err(lacks_err)?;
            }
            Ok(None)
        }
        Action::Stop => {
            let id = id_of()?;
            let m = mgr.clone();
            tokio::spawn(async move { m.stop_container(&id).await })
                .await
                .map_err(|e| format!("container stop task failed: {e}"))?
                .map_err(lacks_err)?;
            Ok(None)
        }
        Action::Delete => {
            let id = id_of()?;
            let m = mgr.clone();
            let did = id.clone();
            tokio::spawn(async move { m.delete_container(&did).await })
                .await
                .map_err(|e| format!("container delete task failed: {e}"))?
                .map_err(lacks_err)?;
            if let Err(e) = state
                .firewall_manager
                .remove_guest_rules(crate::firewall::FirewallScope::Container(id))
                .await
            {
                return Ok(Some(format!("deleted, but its firewall rules stayed: {e}")));
            }
            Ok(None)
        }
        Action::Blocked => Err(c.details.join("; ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horcrux_common::ContainerRuntime as R;

    fn spec(name: &str) -> ContainerSpec {
        ContainerSpec {
            name: name.into(),
            id: None,
            runtime: None,
            image: "nginx".into(),
            memory: None,
            cpus: None,
            ports: vec![],
            env: BTreeMap::new(),
            mounts: vec![],
            command: vec![],
            network: None,
            restart: None,
            power: None,
            replace: false,
        }
    }

    /// A live container the way `gather` reports it (spec shape).
    fn live(name: &str, rt: &str, power: Option<Power>) -> ContainerSpec {
        ContainerSpec {
            runtime: Some(rt.into()),
            image: "docker.io/library/nginx:latest".into(),
            power,
            ..spec(name)
        }
    }

    fn rt(name: &str, ok: bool) -> RuntimeInfo {
        RuntimeInfo {
            runtime: name.into(),
            available: ok,
            reason: (!ok).then(|| "not-installed".into()),
            message: (!ok).then(|| format!("{name} is not installed.")),
            hint: (!ok).then(|| "Install it.".into()),
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
    fn podman_is_the_default_and_docker_the_fallback() {
        assert_eq!(resolve_runtime(None, &[]), "podman");
        let both = vec![rt("podman", true), rt("docker", true)];
        assert_eq!(resolve_runtime(None, &both), "podman");
        let only_docker = vec![rt("podman", false), rt("docker", true)];
        assert_eq!(resolve_runtime(None, &only_docker), "docker");
        let none = vec![rt("podman", false), rt("docker", false)];
        assert_eq!(resolve_runtime(None, &none), "podman");
        assert_eq!(resolve_runtime(Some("docker"), &both), "docker");
    }

    #[test]
    fn system_prefers_incus_over_lxd() {
        let both = vec![rt("incus", true), rt("lxd", true)];
        assert_eq!(resolve_runtime(Some("system"), &both), "incus");
        let lxd = vec![rt("incus", false), rt("lxd", true)];
        assert_eq!(resolve_runtime(Some("system"), &lxd), "lxd");
        let neither = vec![rt("incus", false), rt("lxd", false)];
        assert_eq!(resolve_runtime(Some("system"), &neither), "incus");
    }

    #[test]
    fn image_names_compare_after_normalising() {
        assert_eq!(
            normalize_image("nginx"),
            normalize_image("docker.io/library/nginx:latest")
        );
        assert_eq!(normalize_image("localhost/app:1"), "app:1");
        assert_ne!(normalize_image("nginx:1.27"), normalize_image("nginx"));
        assert_eq!(normalize_image("ghcr.io/o/app"), "ghcr.io/o/app:latest");
        assert_eq!(
            normalize_image("registry:5000/app"),
            "registry:5000/app:latest"
        );
    }

    #[test]
    fn create_start_and_ordering_follow_the_spec() {
        let mut a = spec("a");
        a.power = Some(Power::Running);
        let p = plan(&[a, spec("b")], &[], &[], false);
        assert_eq!(acts(&p), ["Create a", "Start a", "Create b"]);
    }

    #[test]
    fn missing_runtime_blocks_creation_with_the_node_lacks_reason() {
        let mut a = spec("a");
        a.power = Some(Power::Running);
        let rts = vec![rt("podman", false), rt("docker", false)];
        let p = plan(&[a], &[], &rts, false);
        assert_eq!(acts(&p), ["Blocked a"]);
        assert!(p.changes[0].details[0].contains("node lacks not-installed"));
        assert!(p.changes[0].details[0].contains("podman is not installed"));
    }

    #[test]
    fn falls_back_to_docker_when_podman_is_missing() {
        let rts = vec![rt("podman", false), rt("docker", true)];
        let p = plan(&[spec("a")], &[], &rts, false);
        assert_eq!(acts(&p), ["Create a"]);
        assert!(p.changes[0].details[0].starts_with("docker"));
    }

    #[test]
    fn in_sync_container_gives_an_empty_plan_and_unknown_limits_are_not_drift() {
        let mut a = spec("a");
        a.memory = Some(256);
        a.cpus = Some(2);
        a.ports = vec!["8080:80".into()];
        a.power = Some(Power::Running);
        // after an API restart: limits unknown, options empty -> unknown
        let l = live("a", "podman", Some(Power::Running));
        let p = plan(&[a], &[l], &[], true);
        assert!(acts(&p).is_empty(), "{:?}", acts(&p));
    }

    #[test]
    fn tracked_differences_block_unless_replace_is_set() {
        let mut a = spec("a");
        a.ports = vec!["8080:80".into()];
        a.env.insert("TZ".into(), "UTC".into());
        let mut l = live("a", "podman", Some(Power::Running));
        l.ports = vec!["9090:80".into()];
        l.env.insert("TZ".into(), "UTC".into());
        let p = plan(&[a.clone()], &[l.clone()], &[], false);
        assert_eq!(acts(&p), ["Blocked a"]);
        assert!(p.changes[0].details.iter().any(|d| d.starts_with("ports")));
        // the structured diff is there for the drift view
        assert_eq!(p.changes[0].fields[0].field, "ports");
        a.replace = true;
        a.power = Some(Power::Running);
        let p = plan(&[a], &[l], &[], false);
        assert_eq!(acts(&p), ["Update a"]);
    }

    #[test]
    fn power_is_reconciled_for_existing_containers() {
        let mut a = spec("a");
        a.power = Some(Power::Running);
        let l = live("a", "podman", Some(Power::Stopped));
        let p = plan(&[a.clone()], &[l], &[], false);
        assert_eq!(acts(&p), ["Start a"]);
        // paused/unknown counts as not running
        let l = live("a", "podman", None);
        assert_eq!(acts(&plan(&[a.clone()], &[l], &[], false)), ["Start a"]);
        a.power = Some(Power::Stopped);
        let l = live("a", "podman", Some(Power::Running));
        let p = plan(&[a.clone()], &[l], &[], false);
        assert_eq!(acts(&p), ["Stop a"]);
        a.power = None;
        let l = live("a", "podman", Some(Power::Running));
        assert!(acts(&plan(&[a], &[l], &[], false)).is_empty());
    }

    #[test]
    fn runtime_and_image_changes_are_detected() {
        let mut a = spec("a");
        a.runtime = Some("docker".into());
        a.image = "nginx:1.27".into();
        let l = live("a", "podman", Some(Power::Stopped));
        let d = diff(&a, &l, &[]);
        assert!(d.iter().any(|x| x.starts_with("runtime")));
        assert!(d.iter().any(|x| x.starts_with("image")));
    }

    #[test]
    fn prune_only_touches_the_managed_list_and_never_a_running_container() {
        let l = vec![
            live("keep", "podman", Some(Power::Stopped)),
            live("old", "podman", Some(Power::Stopped)),
            live("busy", "podman", Some(Power::Running)),
        ];
        let p = plan(&[spec("keep")], &l, &[], true);
        assert_eq!(acts(&p), ["Delete old", "Blocked busy"]);
        let p = plan(&[spec("keep")], &l, &[], false);
        assert!(acts(&p).is_empty());
    }

    #[test]
    fn export_is_a_fixed_point_and_never_leaks_secret_env() {
        let mut c = ContainerConfig {
            id: "web".into(),
            name: "web".into(),
            runtime: R::Podman,
            rootfs: "docker.io/library/nginx:latest".into(),
            memory: 256,
            cpus: 1,
            status: ContainerStatus::Running,
            ..Default::default()
        };
        c.options.env = vec!["TZ=UTC".into(), "API_TOKEN=abc".into(), "BARE".into()];
        c.options.ports = vec!["8080:80".into()];
        c.options.restart = Some("always".into());
        let other = ContainerConfig {
            id: "x".into(),
            name: "x".into(),
            runtime: R::Docker,
            rootfs: "busybox".into(),
            status: ContainerStatus::Stopped,
            ..Default::default()
        };
        let spec = export(&[c, other]);
        let yaml = serde_yaml::to_string(&spec).unwrap();
        assert!(
            !yaml.contains("abc") && !yaml.contains("API_TOKEN"),
            "{yaml}"
        );
        assert_eq!(spec[0].name, "web");
        assert_eq!(spec[0].env.get("TZ").unwrap(), "UTC");
        validate(&spec).unwrap();
        let p = plan(&spec, &spec, &[], true);
        assert!(acts(&p).is_empty(), "{:?}", acts(&p));
        // through JSON, as the engine stores it
        let v = to_json(&spec).unwrap();
        assert_eq!(items::<ContainerSpec>(&v).unwrap(), spec);
    }

    #[test]
    fn validation_refuses_secrets_bad_runtimes_and_runtime_mismatched_options() {
        let mut a = spec("a");
        a.env.insert("DB_PASSWORD".into(), "x".into());
        assert!(validate(&[a]).unwrap_err().contains("looks like a secret"));
        let mut a = spec("a");
        a.runtime = Some("kata".into());
        assert!(validate(&[a]).unwrap_err().contains("unknown runtime"));
        let mut a = spec("a");
        a.runtime = Some("incus".into());
        a.ports = vec!["80:80".into()];
        assert!(validate(&[a]).unwrap_err().contains("podman and docker"));
        let mut a = spec("a");
        a.restart = Some("sometimes".into());
        assert!(validate(&[a]).unwrap_err().contains("restart"));
        let mut a = spec("a");
        a.restart = Some("on-failure:3".into());
        validate(&[a]).unwrap();
        let mut a = spec("a");
        a.ports = vec!["not-a-port".into()];
        assert!(validate(&[a]).is_err());
        assert!(validate(&[spec("a"), spec("a")])
            .unwrap_err()
            .contains("twice"));
        let mut a = spec("a");
        a.cpus = Some(0);
        assert!(validate(&[a]).is_err());
    }

    #[test]
    fn spec_roundtrips_and_rejects_unknown_keys() {
        let y = "name: a\nimage: nginx\nports: ['80:80']\nrestart: always\npower: running\n";
        let s: ContainerSpec = serde_yaml::from_str(y).unwrap();
        let back: ContainerSpec =
            serde_yaml::from_str(&serde_yaml::to_string(&s).unwrap()).unwrap();
        assert_eq!(s, back);
        assert!(serde_yaml::from_str::<ContainerSpec>("name: a\nimage: x\nmemroy: 5").is_err());
    }

    #[test]
    fn system_runtime_spec_matches_a_live_lxd_container() {
        let mut a = spec("a");
        a.runtime = Some("system".into());
        let l = live("a", "lxd", Some(Power::Stopped));
        assert!(!diff(&a, &l, &[rt("incus", false), rt("lxd", true)])
            .iter()
            .any(|x| x.starts_with("runtime")));
    }
}
