//! "This node lacks X" answers: 424 `NOT_AVAILABLE` with a reason code, a plain
//! sentence on what is missing and one on what to do. The UI shows these as an
//! empty state (not a red error banner), and `ci/ux-qa/ux_crawl.py` fails on any
//! 5xx a page provokes, so a missing engine must never be a 500/503.
use crate::error::ApiError;
use std::path::Path;

fn in_path(bin: &str) -> bool {
    crate::api::system_admin::which(bin).is_some()
}

/// The Docker socket the engine client would use (DOCKER_HOST unix://, else the default).
fn docker_socket_present() -> bool {
    match std::env::var("DOCKER_HOST") {
        Ok(h) if !h.is_empty() => h
            .strip_prefix("unix://")
            .map(|p| Path::new(p).exists())
            .unwrap_or(true), // tcp:// etc.: let the client try
        _ => Path::new("/var/run/docker.sock").exists(),
    }
}

/// Why `runtime` (`docker` / `podman`) cannot be used on this node, if it cannot.
pub fn container_engine(runtime: &str) -> Result<(), ApiError> {
    engine_check(
        runtime,
        in_path(runtime_bin(runtime)),
        docker_socket_present(),
    )
}

fn runtime_bin(runtime: &str) -> &'static str {
    if runtime == "podman" {
        "podman"
    } else {
        "docker"
    }
}

/// The decision behind [`container_engine`], free of host probing so it can
/// be unit-tested: `have_bin` = the CLI is on PATH, `socket` = the Docker
/// socket exists (Docker only).
pub fn engine_check(runtime: &str, have_bin: bool, socket: bool) -> Result<(), ApiError> {
    match runtime {
        "podman" => {
            if have_bin {
                Ok(())
            } else {
                Err(ApiError::NotAvailable {
                    reason: "podman-missing",
                    message: "Podman isn't installed on this node.".into(),
                    hint: "Install app-containers/podman, or use Docker or LXC containers instead."
                        .into(),
                })
            }
        }
        _ => {
            if !have_bin && !socket {
                Err(ApiError::NotAvailable {
                    reason: "docker-missing",
                    message: "Docker isn't installed on this node.".into(),
                    hint: "Install app-containers/docker, or use LXC or Podman containers instead."
                        .into(),
                })
            } else if !socket {
                Err(ApiError::NotAvailable {
                    reason: "docker-not-running",
                    message: "The Docker service isn't running on this node.".into(),
                    hint: "Start it (rc-service docker start, or systemctl start docker) and reload this page."
                        .into(),
                })
            } else {
                Ok(())
            }
        }
    }
}

/// One OCI runtime's availability, for the container create form.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct RuntimeStatus {
    pub runtime: String,
    pub available: bool,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub hint: Option<String>,
}

pub fn runtime_status(runtime: &str, check: Result<(), ApiError>) -> RuntimeStatus {
    match check {
        Ok(()) => RuntimeStatus {
            runtime: runtime.into(),
            available: true,
            reason: None,
            message: None,
            hint: None,
        },
        Err(ApiError::NotAvailable {
            reason,
            message,
            hint,
        }) => RuntimeStatus {
            runtime: runtime.into(),
            available: false,
            reason: Some(reason.into()),
            message: Some(message),
            hint: Some(hint),
        },
        Err(e) => RuntimeStatus {
            runtime: runtime.into(),
            available: false,
            reason: Some("unknown".into()),
            message: Some(format!("{e:?}")),
            hint: None,
        },
    }
}

/// `GET /api/container-runtimes`: whether Podman and Docker are usable here.
pub async fn container_runtimes() -> axum::Json<Vec<RuntimeStatus>> {
    axum::Json(
        ["podman", "docker"]
            .iter()
            .map(|r| runtime_status(r, container_engine(r)))
            .collect(),
    )
}

pub fn no_log_source(detail: &str) -> ApiError {
    ApiError::NotAvailable {
        reason: "no-log-source",
        message: "This node has no system log to read.".into(),
        hint: format!(
            "Run a system logger (journald, or syslog-ng/metalog writing /var/log/messages). {detail}"
        ),
    }
}

pub fn no_init_system() -> ApiError {
    ApiError::NotAvailable {
        reason: "no-init-system",
        message: "No supported init system (OpenRC or systemd) is running on this node.".into(),
        hint: "Services can be listed and controlled on an installed Horcrux host; this node looks like a container or chroot."
            .into(),
    }
}

pub fn no_kernel_log(detail: &str) -> ApiError {
    ApiError::NotAvailable {
        reason: "no-kernel-log",
        message: "This node can't read the kernel log, so firewall log lines are not available."
            .into(),
        hint: format!(
            "Install nftables and journald (or allow dmesg) on the host, then reload. {detail}"
        ),
    }
}

/// A node tool a page needs (`zpool`, `tgtadm`, `ip`) is not installed: 424, never a 500.
pub fn missing_tool(
    bin: &str,
    reason: &'static str,
    what: &str,
    todo: &str,
) -> Result<(), ApiError> {
    if in_path(bin) {
        Ok(())
    } else {
        Err(ApiError::NotAvailable {
            reason,
            message: what.into(),
            hint: todo.into(),
        })
    }
}

pub fn zfs() -> Result<(), ApiError> {
    missing_tool(
        "zpool",
        "zfs-missing",
        "ZFS isn't installed on this node (no zpool command), so there are no NAS pools.",
        "Install sys-fs/zfs (and the zfs kernel module), then check again.",
    )
}

pub fn iscsi_target() -> Result<(), ApiError> {
    missing_tool(
        "tgtadm",
        "iscsi-target-missing",
        "The iSCSI target framework (tgtadm) isn't installed on this node.",
        "Install sys-block/tgt and start the tgtd service, then check again.",
    )
}

pub fn iproute2() -> ApiError {
    ApiError::NotAvailable {
        reason: "iproute2-missing",
        message: "The ip command (iproute2) isn't available on this node, so its interfaces can't be listed.".into(),
        hint: "Install sys-apps/iproute2, then check again.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_status_says_why_a_runtime_is_unusable() {
        let ok = runtime_status("podman", engine_check("podman", true, false));
        assert!(ok.available && ok.reason.is_none());
        let p = runtime_status("podman", engine_check("podman", false, false));
        assert!(!p.available);
        assert_eq!(p.reason.as_deref(), Some("podman-missing"));
        assert!(p.hint.as_deref().unwrap().contains("Install"));
        let d = runtime_status("docker", engine_check("docker", false, false));
        assert_eq!(d.reason.as_deref(), Some("docker-missing"));
        let off = runtime_status("docker", engine_check("docker", true, false));
        assert_eq!(off.reason.as_deref(), Some("docker-not-running"));
        assert!(runtime_status("docker", engine_check("docker", true, true)).available);
    }

    #[test]
    fn a_missing_tool_is_a_424_not_a_500() {
        match missing_tool(
            "horcrux-no-such-tool-xyz",
            "x-missing",
            "X is missing.",
            "Install X.",
        ) {
            Err(ApiError::NotAvailable { reason, .. }) => assert_eq!(reason, "x-missing"),
            other => panic!("expected NotAvailable, got {other:?}"),
        }
        assert!(missing_tool("sh", "sh-missing", "", "").is_ok());
    }
}
