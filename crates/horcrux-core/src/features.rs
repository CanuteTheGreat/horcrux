//! Build-time feature set (Gentoo USE flags -> Cargo features).
//!
//! Every USE flag in `gentoo/app-emulation/horcrux/horcrux-*.ebuild` maps to
//! a Cargo feature of the same name (`src_configure()`), and every one of
//! those features gates real code: a disabled backend's module is not
//! compiled, its API routes answer 404, and it is left out of
//! `GET /api/features`, which the web UI reads to hide pages, menu entries
//! and form options for features this build does not have.
//!
//! `gentoo/ci/use-matrix.sh` (CI job "Gentoo USE-flag matrix") emerges
//! contrasting USE sets and checks all three effects against the installed
//! binary; the "USE-flag feature builds" job checks that [`ALL`] lists
//! every feature declared in `Cargo.toml`.

use axum::{
    body::{to_bytes, Body},
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use horcrux_common::{ContainerRuntime, VmHypervisor};
use serde::Serialize;

macro_rules! feature_table {
    ($($name:literal),* $(,)?) => {
        /// Every USE-flag-selectable Cargo feature and whether this binary
        /// was built with it.
        pub const ALL: &[(&str, bool)] = &[$(($name, cfg!(feature = $name))),*];
    };
}

feature_table!(
    // Virtualization backends
    "qemu",
    "lxd",
    "incus",
    // Container runtimes
    "lxc",
    "docker",
    "podman",
    // Platform features
    "pam",
    "cluster",
    "backup",
    "san",
    "kubernetes",
    // NAS
    "nas",
    "smb",
    "nfs-server",
    "afp",
    "webdav",
    "ftp",
    "nas-auth",
    "ldap",
    "ldap-server",
    "kerberos",
    "ad",
    "timemachine",
    "s3-gateway",
    "iscsi-target",
    "rsync-server",
    "nas-zfs",
    "nas-btrfs",
    "nas-mdraid",
    "nas-lvm",
);

/// Names of the features compiled into this binary.
pub fn enabled() -> Vec<&'static str> {
    ALL.iter().filter(|(_, on)| *on).map(|(n, _)| *n).collect()
}

pub fn is_enabled(name: &str) -> bool {
    ALL.iter().any(|(n, on)| *n == name && *on)
}

pub fn container_runtime_feature(rt: &ContainerRuntime) -> &'static str {
    match rt {
        ContainerRuntime::Lxc => "lxc",
        ContainerRuntime::Lxd => "lxd",
        ContainerRuntime::Incus => "incus",
        ContainerRuntime::Docker => "docker",
        ContainerRuntime::Podman => "podman",
    }
}

pub fn vm_hypervisor_feature(h: &VmHypervisor) -> &'static str {
    match h {
        VmHypervisor::Qemu => "qemu",
        VmHypervisor::Lxd => "lxd",
        VmHypervisor::Incus => "incus",
    }
}

/// The error every gated dispatch returns for a backend that was not
/// compiled in (HTTP 422 through `ApiError`).
pub fn not_compiled(what: &str, feature: &str) -> horcrux_common::Error {
    horcrux_common::Error::Validation(format!(
        "{what} '{feature}' is not compiled into this Horcrux build \
         (rebuild with USE={feature})"
    ))
}

pub fn runtime_not_compiled(rt: &ContainerRuntime) -> horcrux_common::Error {
    not_compiled("Container runtime", container_runtime_feature(rt))
}

pub fn require_container_runtime(rt: &ContainerRuntime) -> horcrux_common::Result<()> {
    let f = container_runtime_feature(rt);
    if is_enabled(f) {
        Ok(())
    } else {
        Err(not_compiled("Container runtime", f))
    }
}

pub fn hypervisor_not_compiled(h: &VmHypervisor) -> horcrux_common::Error {
    not_compiled("VM hypervisor", vm_hypervisor_feature(h))
}

pub fn require_vm_hypervisor(h: &VmHypervisor) -> horcrux_common::Result<()> {
    let f = vm_hypervisor_feature(h);
    if is_enabled(f) {
        Ok(())
    } else {
        Err(not_compiled("VM hypervisor", f))
    }
}

/// Pure preference order for the OCI runtime used when a request names
/// none: Podman first, Docker second.
///
/// * both compiled in: Podman when its binary is installed, else Docker when
///   Docker is installed, else Podman (the preferred one reports "missing").
/// * only one compiled in: that one.
/// * neither compiled in (route group is gated off anyway): Podman.
pub fn choose_oci_runtime(
    podman_compiled: bool,
    docker_compiled: bool,
    podman_present: bool,
    docker_present: bool,
) -> ContainerRuntime {
    match (podman_compiled, docker_compiled) {
        (true, false) | (false, false) => ContainerRuntime::Podman,
        (false, true) => ContainerRuntime::Docker,
        (true, true) => {
            if !podman_present && docker_present {
                ContainerRuntime::Docker
            } else {
                ContainerRuntime::Podman
            }
        }
    }
}

fn binary_on_path(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
        .unwrap_or(false)
}

/// The Docker/Podman runtime used when a request does not name one
/// (`HORCRUX_OCI_RUNTIME=docker|podman` overrides the detection).
pub fn default_oci_runtime() -> ContainerRuntime {
    match std::env::var("HORCRUX_OCI_RUNTIME").ok().as_deref() {
        Some("docker") if cfg!(feature = "docker") => return ContainerRuntime::Docker,
        Some("podman") if cfg!(feature = "podman") => return ContainerRuntime::Podman,
        _ => {}
    }
    choose_oci_runtime(
        cfg!(feature = "podman"),
        cfg!(feature = "docker"),
        binary_on_path("podman"),
        binary_on_path("docker"),
    )
}

/// System-container/VM runtimes in the order Horcrux prefers them: Incus
/// first (the maintained fork, owner rule "favor incus over lxd"), LXD as
/// the supported secondary. Every place that picks between the two when
/// the caller named none (defaults, menu order, reconcile order) goes
/// through this list.
pub const INSTANCE_RUNTIME_PREFERENCE: [ContainerRuntime; 2] =
    [ContainerRuntime::Incus, ContainerRuntime::Lxd];

/// Same preference for VM hypervisors (QEMU is a different kind of
/// backend and is never chosen through this).
pub const INSTANCE_HYPERVISOR_PREFERENCE: [VmHypervisor; 2] =
    [VmHypervisor::Incus, VmHypervisor::Lxd];

/// First of Incus, LXD for which `usable` holds.
pub fn preferred_instance_runtime(
    usable: impl Fn(&ContainerRuntime) -> bool,
) -> Option<ContainerRuntime> {
    INSTANCE_RUNTIME_PREFERENCE
        .iter()
        .find(|r| usable(r))
        .cloned()
}

/// First of Incus, LXD for which `usable` holds.
pub fn preferred_instance_hypervisor(
    usable: impl Fn(&VmHypervisor) -> bool,
) -> Option<VmHypervisor> {
    INSTANCE_HYPERVISOR_PREFERENCE
        .iter()
        .find(|h| usable(h))
        .cloned()
}

#[derive(Debug, Serialize)]
pub struct FeaturesResponse {
    pub version: &'static str,
    /// Enabled features (USE flags), sorted as in the ebuild.
    pub features: Vec<&'static str>,
    /// Every feature this version knows about, enabled or not.
    pub all: Vec<&'static str>,
}

pub fn features_response() -> FeaturesResponse {
    FeaturesResponse {
        version: env!("CARGO_PKG_VERSION"),
        features: enabled(),
        all: ALL.iter().map(|(n, _)| *n).collect(),
    }
}

async fn get_features() -> Json<FeaturesResponse> {
    Json(features_response())
}

/// `GET /api/features` (public: the UI needs it to build its menu, and it
/// only says which backends were compiled in).
pub fn routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new().route("/api/features", get(get_features))
}

/// Largest request body the OCI runtime gate buffers to look for a
/// `"runtime"` field (compose files are the biggest bodies on these routes).
const GATE_BODY_LIMIT: usize = 16 * 1024 * 1024;

/// Middleware for the Docker/Podman-only route groups (images, volumes,
/// container networks, stacks, container WebSocket tickets): a request
/// that names a runtime this build does not have - `?runtime=docker` or a
/// JSON body with `"runtime": "podman"` - is refused with 422 before the
/// handler shells out to a CLI that was never meant to be used.
pub async fn oci_runtime_gate(req: Request, next: Next) -> Response {
    if let Some(rt) = req.uri().query().and_then(runtime_from_query) {
        if let Err(e) = check_named_runtime(&rt) {
            return e;
        }
    }
    let is_json = req
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    if !is_json {
        return next.run(req).await;
    }
    let (parts, body) = req.into_parts();
    let bytes = match to_bytes(body, GATE_BODY_LIMIT).await {
        Ok(b) => b,
        Err(_) => return (StatusCode::PAYLOAD_TOO_LARGE, "request body too large").into_response(),
    };
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_slice(&bytes) {
        if let Some(serde_json::Value::String(rt)) = map.get("runtime") {
            if let Err(e) = check_named_runtime(rt) {
                return e;
            }
        }
    }
    next.run(Request::from_parts(parts, Body::from(bytes)))
        .await
}

fn runtime_from_query(q: &str) -> Option<String> {
    url::form_urlencoded::parse(q.as_bytes())
        .find(|(k, _)| k == "runtime")
        .map(|(_, v)| v.into_owned())
}

#[allow(clippy::result_large_err)]
fn check_named_runtime(rt: &str) -> Result<(), Response> {
    let feature = match rt {
        "docker" | "podman" => rt,
        // Unknown values fall through to the handler's own parsing.
        _ => return Ok(()),
    };
    if is_enabled(feature) {
        return Ok(());
    }
    Err(crate::error::ApiError::from(not_compiled("Container runtime", feature)).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incus_is_preferred_over_lxd() {
        // both usable: Incus
        assert_eq!(
            preferred_instance_runtime(|_| true),
            Some(ContainerRuntime::Incus)
        );
        assert_eq!(
            preferred_instance_hypervisor(|_| true),
            Some(VmHypervisor::Incus)
        );
        // only LXD usable: LXD stays supported
        assert_eq!(
            preferred_instance_runtime(|r| *r == ContainerRuntime::Lxd),
            Some(ContainerRuntime::Lxd)
        );
        assert_eq!(
            preferred_instance_hypervisor(|h| *h == VmHypervisor::Lxd),
            Some(VmHypervisor::Lxd)
        );
        // neither: no choice is invented
        assert_eq!(preferred_instance_runtime(|_| false), None);
        assert_eq!(preferred_instance_hypervisor(|_| false), None);
        // the lists themselves put Incus first
        assert_eq!(INSTANCE_RUNTIME_PREFERENCE[0], ContainerRuntime::Incus);
        assert_eq!(INSTANCE_HYPERVISOR_PREFERENCE[0], VmHypervisor::Incus);
    }

    #[test]
    fn table_matches_compiled_features() {
        assert_eq!(is_enabled("docker"), cfg!(feature = "docker"));
        assert_eq!(is_enabled("backup"), cfg!(feature = "backup"));
        assert_eq!(is_enabled("cluster"), cfg!(feature = "cluster"));
        assert!(!is_enabled("no-such-feature"));
        let r = features_response();
        assert_eq!(r.all.len(), ALL.len());
        assert!(r.features.iter().all(|f| is_enabled(f)));
    }

    #[test]
    fn runtime_and_hypervisor_gates_follow_features() {
        assert_eq!(
            require_container_runtime(&ContainerRuntime::Podman).is_ok(),
            cfg!(feature = "podman")
        );
        assert_eq!(
            require_vm_hypervisor(&VmHypervisor::Incus).is_ok(),
            cfg!(feature = "incus")
        );
        let e = not_compiled("Container runtime", "podman").to_string();
        assert!(e.contains("USE=podman"), "{e}");
    }

    #[test]
    fn oci_runtime_prefers_podman_over_docker() {
        use ContainerRuntime::{Docker, Podman};
        assert_eq!(ContainerRuntime::default(), Podman);
        // both compiled, both installed / neither installed: podman
        assert_eq!(choose_oci_runtime(true, true, true, true), Podman);
        assert_eq!(choose_oci_runtime(true, true, false, false), Podman);
        assert_eq!(choose_oci_runtime(true, true, true, false), Podman);
        // docker only when podman is absent and docker present
        assert_eq!(choose_oci_runtime(true, true, false, true), Docker);
        // single compiled-in runtime wins; neither -> podman
        assert_eq!(choose_oci_runtime(false, true, true, true), Docker);
        assert_eq!(choose_oci_runtime(true, false, false, true), Podman);
        assert_eq!(choose_oci_runtime(false, false, false, false), Podman);
    }

    #[test]
    fn query_runtime_is_parsed() {
        assert_eq!(
            runtime_from_query("a=1&runtime=podman").as_deref(),
            Some("podman")
        );
        assert_eq!(runtime_from_query("a=1"), None);
        assert!(check_named_runtime("something-else").is_ok());
        assert_eq!(
            check_named_runtime("docker").is_ok(),
            cfg!(feature = "docker")
        );
    }
}
