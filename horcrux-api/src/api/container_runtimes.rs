//! `GET /api/containers/runtimes`: which container runtimes work on this
//! node right now (live probe, see horcrux_container::container::probe),
//! plus the host's CPU/RAM/disk bounds for the Create Container sliders.
use crate::api::vm_hw::{host_capabilities, HostCapabilities};
use crate::container::probe::{probe_all_cached, RuntimeProbe};
use crate::AppState;
use axum::{extract::Query, routing::get, Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/api/containers/runtimes", get(runtimes))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerRuntimes {
    /// Every runtime in menu order (Podman, Docker, LXC, Incus, LXD), each
    /// with `available` and, when not, why and how to enable it.
    pub runtimes: Vec<RuntimeProbe>,
    /// The runtime the form preselects: the first available one.
    pub default: Option<String>,
    /// Bounds for the CPU/RAM sliders (same as the Create VM form).
    pub host: HostCapabilities,
    /// GB free where LXC root filesystems go (bound of the disk slider).
    pub lxc_disk_available_gb: u64,
}

#[derive(Deserialize, Default)]
struct RuntimesQuery {
    /// `?refresh=1` skips the few-seconds probe cache ("Check again").
    #[serde(default)]
    refresh: Option<String>,
}

/// The preselected runtime: the first available one in menu order.
pub fn default_runtime(runtimes: &[RuntimeProbe]) -> Option<String> {
    runtimes
        .iter()
        .find(|r| r.available)
        .map(|r| r.runtime.clone())
}

fn lxc_disk_available_gb() -> u64 {
    let p = std::env::var("HORCRUX_LXC_PATH").unwrap_or_else(|_| "/var/lib/lxc".into());
    let p = crate::api::vm_hw::nearest_existing(std::path::Path::new(&p));
    crate::storage::fs_capacity_gb(&p.to_string_lossy())
        .map(|c| c.1)
        .unwrap_or(0)
}

async fn runtimes(Query(q): Query<RuntimesQuery>) -> Json<ContainerRuntimes> {
    let refresh = matches!(q.refresh.as_deref(), Some("1" | "true" | "yes"));
    let runtimes = probe_all_cached(refresh).await;
    Json(ContainerRuntimes {
        default: default_runtime(&runtimes),
        runtimes,
        host: host_capabilities(0),
        lxc_disk_available_gb: lxc_disk_available_gb(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container::probe::{classify, ProbeOutcome};
    use horcrux_common::ContainerRuntime;

    #[test]
    fn the_first_working_runtime_is_preselected() {
        let v = vec![
            classify(&ContainerRuntime::Podman, true, Some(ProbeOutcome::Missing)),
            classify(
                &ContainerRuntime::Docker,
                true,
                Some(ProbeOutcome::Failed("Cannot connect".into())),
            ),
            classify(
                &ContainerRuntime::Lxc,
                true,
                Some(ProbeOutcome::Ok("6.0.4".into())),
            ),
            classify(
                &ContainerRuntime::Incus,
                true,
                Some(ProbeOutcome::Ok("{}".into())),
            ),
        ];
        assert_eq!(default_runtime(&v).as_deref(), Some("lxc"));
        assert_eq!(default_runtime(&v[..2]), None);
    }
}
