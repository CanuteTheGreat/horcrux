//! System administration (Proxmox "Node > System"): host name / time zone /
//! locale, network interfaces, DNS, NTP, health, services, logs, packages
//! (Portage on Gentoo, apt elsewhere) and diagnostics. Every route changes
//! or reads the real host; nothing is held in memory only.
//!
//! * `GET|POST /api/system/config`            hostname, domain, time zone, locale
//! * `GET /api/system/network/interfaces`, `POST .../interfaces/:name`
//! * `GET|POST /api/system/network/dns`       `/etc/resolv.conf`
//! * `GET|POST /api/system/time/ntp`          chrony / systemd-timesyncd / ntpd
//! * `GET /api/system/health`                 `/proc` + statvfs
//! * `GET /api/system/services`, `POST .../services/:name/:action`
//! * `GET|DELETE /api/system/logs`, `GET .../logs/export`
//! * `GET /api/system/packages`, `.../packages/search?q=`, install / remove /
//!   upgrade / upgrade-all / bulk-install / update (long ones run as tasks)
//! * `GET /api/system/diagnostics`, `POST .../tests/:name`, `.../tests/run-all`,
//!   `GET .../report`
//!
//! Tests point `HORCRUX_SYSROOT` at a scratch tree to exercise the file
//! editing; the CI node e2e (tests/ha_failover_e2e.py) drives the commands
//! on real hosts.
use crate::AppState;
use axum::Router;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub mod diagnostics;
pub mod host;
pub mod logs;
pub mod packages;
pub mod services;

pub fn routes(state: &Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .merge(host::routes(state))
        .merge(services::routes())
        .merge(logs::routes())
        .merge(packages::routes())
        .merge(diagnostics::routes())
}

/// Root the system files live under (`/` unless a test overrides it).
pub(crate) fn sysroot() -> PathBuf {
    std::env::var_os("HORCRUX_SYSROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub(crate) fn rooted(p: &str) -> PathBuf {
    sysroot().join(p.trim_start_matches('/'))
}

pub(crate) fn data_dir() -> PathBuf {
    PathBuf::from(std::env::var("HORCRUX_DATA_DIR").unwrap_or_else(|_| "/var/lib/horcrux".into()))
}

/// First `bin` found on PATH or in the sbin directories a service user's
/// PATH often lacks.
pub(crate) fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var("PATH").unwrap_or_default();
    let extra = "/usr/local/sbin:/usr/sbin:/sbin:/usr/local/bin:/usr/bin:/bin";
    for dir in path.split(':').chain(extra.split(':')) {
        if dir.is_empty() {
            continue;
        }
        let p = Path::new(dir).join(bin);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// Run a command to completion (60 s cap). Ok = stdout; Err = a message that
/// carries stderr.
pub(crate) async fn run(program: &str, args: &[&str]) -> Result<String, String> {
    run_secs(program, args, 60).await
}

pub(crate) async fn run_secs(program: &str, args: &[&str], secs: u64) -> Result<String, String> {
    let bin = which(program).ok_or_else(|| format!("{program} is not installed"))?;
    let child = tokio::process::Command::new(&bin)
        .args(args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(std::time::Duration::from_secs(secs), child)
        .await
        .map_err(|_| format!("{program} timed out after {secs}s"))?
        .map_err(|e| format!("{program}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = if err.trim().is_empty() {
            String::from_utf8_lossy(&out.stdout)
        } else {
            err
        };
        Err(format!("{program} {}: {}", args.join(" "), err.trim()))
    }
}

pub(crate) fn user_of(u: &Option<axum::Extension<crate::middleware::auth::AuthUser>>) -> String {
    u.as_ref()
        .map(|e| e.0.username.clone())
        .unwrap_or_else(|| "system".into())
}

/// Which init system runs this host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Init {
    OpenRc,
    Systemd,
    None,
}

pub(crate) fn init_system() -> Init {
    if Path::new("/run/systemd/system").is_dir() && which("systemctl").is_some() {
        Init::Systemd
    } else if which("rc-service").is_some() && which("rc-status").is_some() {
        Init::OpenRc
    } else {
        Init::None
    }
}

/// Tests that point `HORCRUX_SYSROOT` somewhere share one process-wide env.
#[cfg(test)]
pub(crate) static SYSROOT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
