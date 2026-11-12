//! Horcrux API Library
//!
//! This module exposes the core functionality of the Horcrux virtualization platform
//! for use by tests and external integrations.

// Allow dead code for library modules that may be used by API consumers
#![allow(dead_code)]

// Core modules
pub use horcrux_core::config;
pub use horcrux_core::error;
pub use horcrux_core::features;
pub use horcrux_core::validation;

// Application state
pub mod state;

/// Capability probes shared by unit tests that need the sandbox to behave
/// like a normal Linux box.
#[cfg(test)]
pub(crate) mod test_env {
    /// True when this process may signal its own child. Some CI job
    /// containers (rw-man-03 burst runner: `docker exec` processes carry the
    /// AppArmor label `docker-default//&unconfined`) deny EVERY signal, even
    /// SIGKILL from root to its own child (EACCES/EPERM), so tests that assert a
    /// kill took effect cannot say anything there. A test that needs it
    /// calls this first and, if false, prints why and returns; the
    /// assertions stay untouched everywhere signals work.
    pub(crate) fn can_signal_own_children() -> bool {
        let Ok(mut c) = std::process::Command::new("sleep").arg("30").spawn() else {
            return true; // cannot probe: run the real assertions
        };
        // The denial applies once the child has exec'd into its own AppArmor
        // label; before that (still our forked image) a kill succeeds, so
        // wait for the exec first.
        for _ in 0..150 {
            let cmd = std::fs::read(format!("/proc/{}/cmdline", c.id())).unwrap_or_default();
            if cmd.starts_with(b"sleep") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let r = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(c.id() as i32),
            nix::sys::signal::Signal::SIGKILL,
        );
        let _ = c.kill();
        let _ = c.wait();
        match r {
            Err(nix::errno::Errno::EPERM | nix::errno::Errno::EACCES) => {
                eprintln!(
                    "SKIPPED (sandbox): kill(2) of our own child is denied (EPERM/EACCES) \
                     (AppArmor/seccomp denies signals in this container)"
                );
                false
            }
            _ => true,
        }
    }
}
pub use state::AppState;

// The API server (startup, router, HTTP handlers) and route modules
pub mod api;
pub mod app;
#[cfg(feature = "lxc")]
#[path = "api/lxc.rs"]
pub mod lxc_api;
pub mod persistence;
pub mod static_ui;
pub mod vm_console_api;

// Authentication & Authorization
pub use horcrux_auth::audit;
pub use horcrux_auth::auth;
pub use horcrux_auth::middleware;

// Virtualization
pub use horcrux_container::container;
pub use horcrux_vm::vm;

// Storage
#[cfg(feature = "backup")]
pub use horcrux_backup::backup;
pub use horcrux_storage::storage;

// Networking
pub use horcrux_sdn::firewall;
pub use horcrux_sdn::sdn;

// Clustering
pub use horcrux_cluster::cluster;
#[cfg(feature = "cluster")]
pub use horcrux_cluster::ha;
pub use horcrux_cluster::migration;

// Monitoring
pub use horcrux_monitoring::alerts;
pub use horcrux_monitoring::metrics;
pub mod metrics_collector;
pub use horcrux_monitoring::metrics_explorer;
pub use horcrux_monitoring::monitoring;
pub use horcrux_monitoring::notification_channels;
pub use horcrux_monitoring::observability;

// Console access
pub use horcrux_vm::console;

// Cloud-init
pub use horcrux_storage::cloudinit;

// GPU passthrough
pub use horcrux_vm::gpu;

// Kubernetes integration
#[cfg(feature = "kubernetes")]
pub use horcrux_k8s::kubernetes;

// NAS (Network Attached Storage)
#[cfg(feature = "nas")]
pub use horcrux_nas::nas;

// Database
pub use horcrux_core::db;

// Secrets management
pub use horcrux_core::secrets;

// TLS
pub use horcrux_auth::tls;

// Encryption
pub use horcrux_core::encryption;

// Templates
pub use horcrux_storage::template;

// Webhooks
pub use horcrux_monitoring::webhooks;

// WebSocket
pub use horcrux_auth::websocket;

// Prometheus metrics
pub use horcrux_monitoring::prometheus;

// OpenAPI documentation
pub mod openapi;

// Logging configuration
pub use horcrux_core::logging;

// Health checks and readiness probes
pub mod health;

// Graceful shutdown handling
pub mod shutdown;
