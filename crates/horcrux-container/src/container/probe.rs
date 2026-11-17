//! Live check of which container runtimes actually work on this node.
//!
//! "Compiled in" (USE flags) is not enough: a node built with docker+podman
//! may have no Docker daemon running and no Podman installed. The Create
//! Container form used to offer every compiled runtime and the user only
//! learned at Create that none of them works (journey QA finding 13). The
//! form now asks `GET /api/containers/runtimes`, which runs each runtime's
//! cheapest real round trip (the daemon answers, the CLI can read its
//! storage) and says, for every one that does not work, why and how to
//! enable it.
use horcrux_common::ContainerRuntime;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tokio::process::Command;

/// What the probe found for one runtime.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeProbe {
    /// `podman`, `docker`, `lxc`, `incus`, `lxd`.
    pub runtime: String,
    pub label: String,
    /// Built into this server (USE flag).
    pub compiled: bool,
    /// A container can be created with it right now.
    pub available: bool,
    pub version: Option<String>,
    /// Machine-readable cause when not available: `not-compiled`,
    /// `not-installed`, `not-running`, `not-working`, `timeout`.
    pub reason: Option<String>,
    /// What is wrong, in plain words.
    pub message: Option<String>,
    /// How to enable it (OpenRC and systemd both named).
    pub hint: Option<String>,
}

/// A Linux bridge that a container runtime created for its own containers:
/// Docker's `docker0` and per-network `br-<12 hex>`, Podman's `podman0..N`
/// (netavark) and `cni-podman0`, CNI/Kubernetes `cni0` and `kube-bridge`.
/// The runtime owns its address plan and NAT/firewall rules there, so VMs
/// and LXC containers are never offered these (a VM NIC on docker0 gets no
/// DHCP and is firewalled by Docker). Real bridges (virbr0, vmbr*, br0,
/// br-lan, incusbr0, lxdbr0, lxcbr0) stay.
pub fn is_container_runtime_bridge(name: &str) -> bool {
    let docker_net = name.len() == 15
        && name.starts_with("br-")
        && name[3..].bytes().all(|b| b.is_ascii_hexdigit());
    docker_net
        || name.starts_with("docker")
        || name.starts_with("podman")
        || name.starts_with("cni")
        || name == "kube-bridge"
}

/// Menu order: Podman before Docker, Incus before LXD (product defaults).
pub const RUNTIME_ORDER: [ContainerRuntime; 5] = [
    ContainerRuntime::Podman,
    ContainerRuntime::Docker,
    ContainerRuntime::Lxc,
    ContainerRuntime::Incus,
    ContainerRuntime::Lxd,
];

pub fn runtime_key(rt: &ContainerRuntime) -> &'static str {
    horcrux_core::features::container_runtime_feature(rt)
}

pub fn runtime_label(rt: &ContainerRuntime) -> &'static str {
    match rt {
        ContainerRuntime::Podman => "Podman",
        ContainerRuntime::Docker => "Docker",
        ContainerRuntime::Lxc => "LXC",
        ContainerRuntime::Incus => "Incus",
        ContainerRuntime::Lxd => "LXD",
    }
}

/// The command whose success means "this runtime can create a container":
/// a round trip to the daemon (docker/incus/lxd) or a read of the storage
/// configuration (podman), not just `--version`.
pub fn probe_command(rt: &ContainerRuntime) -> (&'static str, Vec<&'static str>) {
    match rt {
        ContainerRuntime::Podman => ("podman", vec!["info", "--format", "{{.Version.Version}}"]),
        ContainerRuntime::Docker => ("docker", vec!["version", "--format", "{{.Server.Version}}"]),
        ContainerRuntime::Lxc => ("lxc-start", vec!["--version"]),
        ContainerRuntime::Incus => ("incus", vec!["query", "/1.0"]),
        // LXD's client is called `lxc` (LXC itself ships lxc-* tools only).
        ContainerRuntime::Lxd => ("lxc", vec!["query", "/1.0"]),
    }
}

/// How one probe command ended.
#[derive(Debug, Clone, PartialEq)]
pub enum ProbeOutcome {
    /// Exit 0 with this stdout.
    Ok(String),
    /// The binary does not exist.
    Missing,
    /// It ran and failed (stderr).
    Failed(String),
    /// No answer in time.
    TimedOut,
}

fn install_hint(rt: &ContainerRuntime) -> &'static str {
    match rt {
        ContainerRuntime::Podman => {
            "Install Podman (Gentoo: emerge app-containers/podman; other distros: the podman package), \
             then reload this page. Podman needs no daemon."
        }
        ContainerRuntime::Docker => {
            "Install Docker (Gentoo: emerge app-containers/docker app-containers/docker-cli), then start it: \
             rc-update add docker default && rc-service docker start (OpenRC) or \
             systemctl enable --now docker (systemd)."
        }
        ContainerRuntime::Lxc => {
            "Install LXC (Gentoo: emerge app-containers/lxc; other distros: the lxc package), then reload this page."
        }
        ContainerRuntime::Incus => {
            "Install Incus (Gentoo: emerge app-containers/incus), start it with \
             rc-update add incus default && rc-service incus start (OpenRC) or \
             systemctl enable --now incus (systemd), then run incus admin init --minimal."
        }
        ContainerRuntime::Lxd => {
            "Install LXD (Gentoo: emerge app-containers/lxd), start it with \
             rc-update add lxd default && rc-service lxd start (OpenRC) or \
             systemctl enable --now lxd (systemd), then run lxd init --minimal."
        }
    }
}

fn start_hint(rt: &ContainerRuntime) -> &'static str {
    match rt {
        ContainerRuntime::Docker => {
            "Start the Docker daemon: rc-service docker start (OpenRC) or systemctl start docker \
             (systemd), and enable it at boot. If Docker runs elsewhere, point DOCKER_HOST at it."
        }
        ContainerRuntime::Incus => {
            "Start the Incus daemon: rc-service incus start (OpenRC) or systemctl start incus (systemd); \
             on a new install run incus admin init --minimal."
        }
        ContainerRuntime::Lxd => {
            "Start the LXD daemon: rc-service lxd start (OpenRC) or systemctl start lxd (systemd); \
             on a new install run lxd init --minimal."
        }
        ContainerRuntime::Podman => {
            "Podman is installed but cannot read its storage or OCI runtime: run `podman info` as root \
             to see why (missing crun/runc, broken /etc/containers/storage.conf)."
        }
        ContainerRuntime::Lxc => {
            "LXC is installed but lxc-start does not run: reinstall it (emerge app-containers/lxc)."
        }
    }
}

/// First non-empty line of a runtime's stderr, trimmed for display.
fn first_line(s: &str) -> String {
    let l = s
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let mut l = l.to_string();
    if l.len() > 240 {
        let mut cut = 240;
        while !l.is_char_boundary(cut) {
            cut -= 1;
        }
        l.truncate(cut);
        l.push_str("...");
    }
    l
}

/// The probe result for a runtime: pure, so every branch is unit-tested.
pub fn classify(
    rt: &ContainerRuntime,
    compiled: bool,
    outcome: Option<ProbeOutcome>,
) -> RuntimeProbe {
    let key = runtime_key(rt);
    let label = runtime_label(rt);
    let mut p = RuntimeProbe {
        runtime: key.to_string(),
        label: label.to_string(),
        compiled,
        available: false,
        version: None,
        reason: None,
        message: None,
        hint: None,
    };
    if !compiled {
        p.reason = Some("not-compiled".into());
        p.message = Some(format!("This Horcrux build has no {label} support."));
        p.hint = Some(format!(
            "Rebuild Horcrux with USE={key} (Gentoo: add {key} to app-emulation/horcrux in package.use)."
        ));
        return p;
    }
    let (bin, _) = probe_command(rt);
    match outcome.unwrap_or(ProbeOutcome::Missing) {
        ProbeOutcome::Ok(out) => {
            p.available = true;
            let v = first_line(&out);
            // `lxc-start --version` / `podman info` print a bare version;
            // `incus|lxc query /1.0` prints JSON (no version needed).
            if !v.is_empty() && !v.starts_with('{') {
                p.version = Some(v);
            }
        }
        ProbeOutcome::Missing => {
            p.reason = Some("not-installed".into());
            p.message = Some(format!(
                "{label} is not installed on this node ({bin} not found)."
            ));
            p.hint = Some(install_hint(rt).to_string());
        }
        ProbeOutcome::TimedOut => {
            p.reason = Some("timeout".into());
            p.message = Some(format!(
                "{label} did not answer within a few seconds ({bin} hung)."
            ));
            p.hint = Some(start_hint(rt).to_string());
        }
        ProbeOutcome::Failed(err) => {
            let daemon = matches!(
                rt,
                ContainerRuntime::Docker | ContainerRuntime::Incus | ContainerRuntime::Lxd
            );
            p.reason = Some(if daemon { "not-running" } else { "not-working" }.into());
            let detail = first_line(&err);
            p.message = Some(if daemon {
                format!("{label} is installed but its daemon is not reachable: {detail}")
            } else {
                format!("{label} is installed but does not work: {detail}")
            });
            p.hint = Some(start_hint(rt).to_string());
        }
    }
    p
}

const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

/// Run one probe command (with a timeout; the child is killed on timeout).
pub async fn run_probe(bin: &str, args: &[&str]) -> ProbeOutcome {
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    match tokio::time::timeout(PROBE_TIMEOUT, cmd.output()).await {
        Err(_) => ProbeOutcome::TimedOut,
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => ProbeOutcome::Missing,
        Ok(Err(e)) => ProbeOutcome::Failed(e.to_string()),
        Ok(Ok(o)) if o.status.success() => {
            ProbeOutcome::Ok(String::from_utf8_lossy(&o.stdout).to_string())
        }
        Ok(Ok(o)) => {
            let mut err = String::from_utf8_lossy(&o.stderr).to_string();
            if err.trim().is_empty() {
                err = String::from_utf8_lossy(&o.stdout).to_string();
            }
            if err.trim().is_empty() {
                err = format!("{bin} exited with {}", o.status);
            }
            ProbeOutcome::Failed(err)
        }
    }
}

/// Probe every runtime (in menu order) concurrently.
pub async fn probe_all() -> Vec<RuntimeProbe> {
    let futs = RUNTIME_ORDER.iter().map(|rt| async move {
        let compiled = horcrux_core::features::is_enabled(runtime_key(rt));
        let outcome = if compiled {
            let (bin, args) = probe_command(rt);
            Some(run_probe(bin, &args).await)
        } else {
            None
        };
        classify(rt, compiled, outcome)
    });
    futures::future::join_all(futs).await
}

/// `probe_all`, cached for a few seconds: the form, the container list and
/// a page reload all ask, and a dead daemon's CLI can take seconds to fail.
pub async fn probe_all_cached(refresh: bool) -> Vec<RuntimeProbe> {
    static CACHE: tokio::sync::Mutex<Option<(Instant, Vec<RuntimeProbe>)>> =
        tokio::sync::Mutex::const_new(None);
    let mut c = CACHE.lock().await;
    if !refresh {
        if let Some((at, v)) = c.as_ref() {
            if at.elapsed() < Duration::from_secs(10) {
                return v.clone();
            }
        }
    }
    let v = probe_all().await;
    *c = Some((Instant::now(), v.clone()));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_runtime_bridges_are_told_from_real_bridges() {
        for n in [
            "docker0",
            "br-3f2a9c01b7de",
            "podman0",
            "podman1",
            "cni-podman0",
            "cni0",
            "kube-bridge",
        ] {
            assert!(is_container_runtime_bridge(n), "{n}");
        }
        for n in [
            "virbr0",
            "vmbr0",
            "vmbr1",
            "br0",
            "br-lan",
            "br-3f2a9c01b7dz",
            "incusbr0",
            "lxdbr0",
            "lxcbr0",
            "hxbr0",
        ] {
            assert!(!is_container_runtime_bridge(n), "{n}");
        }
    }

    #[test]
    fn a_working_runtime_is_available_with_its_version() {
        let p = classify(
            &ContainerRuntime::Podman,
            true,
            Some(ProbeOutcome::Ok("5.4.2\n".into())),
        );
        assert!(p.available && p.compiled);
        assert_eq!(p.version.as_deref(), Some("5.4.2"));
        assert_eq!((p.reason, p.hint), (None, None));
        // JSON from `incus query /1.0` is not shown as a version
        let p = classify(
            &ContainerRuntime::Incus,
            true,
            Some(ProbeOutcome::Ok("{\n \"api_version\": \"1.0\"}".into())),
        );
        assert!(p.available);
        assert_eq!(p.version, None);
    }

    #[test]
    fn a_stopped_docker_daemon_says_how_to_start_it_on_both_init_systems() {
        let p = classify(
            &ContainerRuntime::Docker,
            true,
            Some(ProbeOutcome::Failed(
                "\nCannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?\n"
                    .into(),
            )),
        );
        assert!(!p.available);
        assert_eq!(p.reason.as_deref(), Some("not-running"));
        assert!(p
            .message
            .as_deref()
            .unwrap()
            .contains("Cannot connect to the Docker daemon"));
        let h = p.hint.unwrap();
        assert!(
            h.contains("rc-service docker start") && h.contains("systemctl"),
            "{h}"
        );
    }

    #[test]
    fn missing_runtimes_get_an_install_hint_and_podman_is_not_a_daemon() {
        for rt in RUNTIME_ORDER {
            let p = classify(&rt, true, Some(ProbeOutcome::Missing));
            assert!(!p.available);
            assert_eq!(p.reason.as_deref(), Some("not-installed"), "{rt:?}");
            assert!(
                p.hint
                    .as_deref()
                    .unwrap()
                    .contains("emerge app-containers/"),
                "{rt:?}"
            );
        }
        let p = classify(
            &ContainerRuntime::Podman,
            true,
            Some(ProbeOutcome::Failed("x".into())),
        );
        assert_eq!(p.reason.as_deref(), Some("not-working"));
        let p = classify(&ContainerRuntime::Incus, true, Some(ProbeOutcome::TimedOut));
        assert_eq!(p.reason.as_deref(), Some("timeout"));
    }

    #[test]
    fn a_runtime_not_built_in_names_its_use_flag() {
        let p = classify(&ContainerRuntime::Lxd, false, None);
        assert!(!p.available && !p.compiled);
        assert_eq!(p.reason.as_deref(), Some("not-compiled"));
        assert!(p.hint.unwrap().contains("USE=lxd"));
    }

    #[test]
    fn long_runtime_errors_are_cut_for_display() {
        let p = classify(
            &ContainerRuntime::Lxc,
            true,
            Some(ProbeOutcome::Failed("é".repeat(400))),
        );
        assert!(p.message.unwrap().len() < 400);
    }

    #[tokio::test]
    async fn the_probe_runner_tells_missing_from_failing_binaries() {
        assert_eq!(
            run_probe("horcrux-no-such-runtime-binary", &["info"]).await,
            ProbeOutcome::Missing
        );
        assert!(matches!(
            run_probe("false", &[]).await,
            ProbeOutcome::Failed(_)
        ));
        assert_eq!(
            run_probe("echo", &["1.2.3"]).await,
            ProbeOutcome::Ok("1.2.3\n".into())
        );
    }

    #[tokio::test]
    async fn probe_all_lists_every_runtime_in_menu_order() {
        let v = probe_all().await;
        let keys: Vec<_> = v.iter().map(|p| p.runtime.as_str()).collect();
        assert_eq!(keys, ["podman", "docker", "lxc", "incus", "lxd"]);
        for p in &v {
            assert_eq!(p.available, p.reason.is_none(), "{p:?}");
            assert!(p.available || p.hint.is_some(), "{p:?}");
        }
    }
}
