//! `GET /api/system/reboot-required`: does this host need a restart to run what is installed?
//! Computed live (not from a marker only the auto-updater writes), so a kernel merged through the
//! Package Manager or by hand is noticed too. Nothing here reboots.
//!
//! * a newer kernel in `/lib/modules` than the running one
//! * the reasons horcrux-autoupdate recorded in `/var/lib/horcrux-autoupdate/reboot-required`
//!   (ignored when older than this boot)
use super::rooted;
use crate::AppState;
use axum::{routing::get, Json, Router};
use serde_json::{json, Value};
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/api/system/reboot-required", get(status))
}

fn version_key(v: &str) -> Vec<(u8, u64, String)> {
    v.split(['.', '-', '+', '_'])
        .map(|p| match p.parse::<u64>() {
            Ok(n) => (0, n, String::new()),
            Err(_) => (1, 0, p.to_string()),
        })
        .collect()
}

/// Newest of the installed kernel versions (directory names under /lib/modules).
pub fn newest_kernel(names: &[String]) -> Option<String> {
    names
        .iter()
        .filter(|n| n.starts_with(|c: char| c.is_ascii_digit()))
        .max_by_key(|n| version_key(n))
        .cloned()
}

pub fn reasons(running: &str, installed: &[String], marker: &[String]) -> Vec<String> {
    let mut out = vec![];
    if let Some(newest) = newest_kernel(installed) {
        if !running.is_empty() && newest != running && version_key(&newest) > version_key(running) {
            out.push(format!(
                "kernel {running} is running, {newest} is installed"
            ));
        }
    }
    for m in marker {
        let m = m.trim();
        // the kernel line is recomputed above; do not list it twice
        if !m.is_empty() && !m.starts_with("kernel ") && !out.iter().any(|o| o == m) {
            out.push(m.to_string());
        }
    }
    out
}

fn running_kernel() -> String {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn marker_lines() -> Vec<String> {
    let dir = std::env::var("HX_AU_STATE").unwrap_or_else(|_| "/var/lib/horcrux-autoupdate".into());
    let p = std::path::Path::new(&dir).join("reboot-required");
    let Ok(mt) = std::fs::metadata(&p).and_then(|m| m.modified()) else {
        return vec![];
    };
    // the marker belongs to the boot it was written in
    let boot = std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| {
            s.split_whitespace()
                .next()
                .and_then(|v| v.parse::<f64>().ok())
        })
        .map(|up| std::time::SystemTime::now() - std::time::Duration::from_secs_f64(up));
    if let Some(b) = boot {
        if mt < b {
            return vec![];
        }
    }
    std::fs::read_to_string(&p)
        .map(|s| s.lines().map(|l| l.to_string()).collect())
        .unwrap_or_default()
}

async fn status() -> Json<Value> {
    let installed: Vec<String> = std::fs::read_dir(rooted("lib/modules"))
        .map(|d| {
            d.filter_map(|e| e.ok())
                .filter(|e| {
                    e.path().join("modules.dep").exists() || e.path().join("kernel").is_dir()
                })
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default();
    let running = running_kernel();
    let why = reasons(&running, &installed, &marker_lines());
    Json(json!({
        "required": !why.is_empty(),
        "reasons": why,
        "running_kernel": running,
        "newest_installed_kernel": newest_kernel(&installed),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn newer_kernel_than_running_is_a_reason() {
        let r = reasons(
            "6.18.64-gentoo-dist",
            &s(&["6.18.64-gentoo-dist", "6.18.65-gentoo-dist"]),
            &[],
        );
        assert_eq!(
            r,
            s(&["kernel 6.18.64-gentoo-dist is running, 6.18.65-gentoo-dist is installed"])
        );
    }

    #[test]
    fn numeric_not_lexical_ordering() {
        assert_eq!(
            newest_kernel(&s(&["6.9.1", "6.18.2", "6.10.0"])).as_deref(),
            Some("6.18.2")
        );
    }

    #[test]
    fn running_newest_means_no_reboot() {
        assert!(reasons("6.18.65", &s(&["6.18.64", "6.18.65"]), &[]).is_empty());
        // an older kernel left on disk after booting a newer one is not a reason
        assert!(reasons("6.18.65", &s(&["6.18.60"]), &[]).is_empty());
    }

    #[test]
    fn marker_reasons_are_kept_without_duplicating_the_kernel_line() {
        let r = reasons(
            "6.18.64",
            &s(&["6.18.65"]),
            &s(&[
                "kernel 6.18.64 is running, 6.18.65 is installed",
                "sys-fs/zfs-kmod updated",
            ]),
        );
        assert_eq!(r.len(), 2);
        assert_eq!(r[1], "sys-fs/zfs-kmod updated");
    }
}
