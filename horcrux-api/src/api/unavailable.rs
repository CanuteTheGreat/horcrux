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
    match runtime {
        "podman" => {
            if in_path("podman") {
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
            if !in_path("docker") && !docker_socket_present() {
                Err(ApiError::NotAvailable {
                    reason: "docker-missing",
                    message: "Docker isn't installed on this node.".into(),
                    hint: "Install app-containers/docker, or use LXC or Podman containers instead."
                        .into(),
                })
            } else if !docker_socket_present() {
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
