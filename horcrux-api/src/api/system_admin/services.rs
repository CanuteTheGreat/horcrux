//! Host services (OpenRC on Gentoo, systemd elsewhere): list with state, pid
//! and memory, and start / stop / restart / enable / disable.
use super::{init_system, run, user_of, which, Init};
use crate::api::tasks;
use crate::error::ApiError;
use crate::AppState;
use axum::{
    extract::Path,
    routing::{get, post},
    Extension, Json, Router,
};
use serde::Serialize;
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/system/services", get(list))
        .route("/api/system/services/:name/:action", post(control))
}

type AuthExt = Option<Extension<crate::middleware::auth::AuthUser>>;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ServiceStatus {
    pub name: String,
    /// active | inactive | failed | unknown
    pub status: String,
    pub enabled: bool,
    pub description: String,
    pub pid: Option<u32>,
    pub memory_usage: Option<u64>,
    pub cpu_usage: Option<f64>,
}

const ACTIONS: &[&str] = &["start", "stop", "restart", "enable", "disable"];

pub fn valid_service_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 100
        && !n.starts_with('-')
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | '-' | ':' | '\\'))
}

/// ` * status: started` -> `started`.
pub fn parse_rc_service_status(text: &str) -> String {
    text.lines()
        .find_map(|l| {
            l.trim()
                .trim_start_matches('*')
                .trim()
                .strip_prefix("status:")
        })
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

async fn rc_state(name: &str) -> String {
    let Some(bin) = which("rc-service") else {
        return "unknown".into();
    };
    match tokio::process::Command::new(bin)
        .args([name, "status"])
        .stdin(std::process::Stdio::null())
        .output()
        .await
    {
        Ok(o) => parse_rc_service_status(&String::from_utf8_lossy(&o.stdout)),
        Err(_) => "unknown".into(),
    }
}

/// `rc-update show` lines: ` sshd | default`.
pub fn parse_rc_update(text: &str) -> std::collections::HashSet<String> {
    text.lines()
        .filter_map(|l| {
            let (name, levels) = l.split_once('|')?;
            if levels.trim().is_empty() {
                None
            } else {
                Some(name.trim().to_string())
            }
        })
        .collect()
}

/// `systemctl list-units --type=service --all --plain --no-legend` rows.
pub fn parse_systemd_units(text: &str) -> Vec<(String, String, String)> {
    text.lines()
        .filter_map(|l| {
            let l = l.trim_start_matches(['●', '*', ' ']).trim();
            let mut it = l.split_whitespace();
            let unit = it.next()?;
            let _load = it.next()?;
            let active = it.next()?;
            let _sub = it.next()?;
            let desc: Vec<&str> = it.collect();
            Some((
                unit.strip_suffix(".service")?.to_string(),
                active.to_string(),
                desc.join(" "),
            ))
        })
        .collect()
}

pub fn parse_systemd_enabled(text: &str) -> std::collections::HashSet<String> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let unit = it.next()?;
            let state = it.next()?;
            if matches!(state, "enabled" | "enabled-runtime" | "alias") {
                unit.strip_suffix(".service").map(|s| s.to_string())
            } else {
                None
            }
        })
        .collect()
}

fn rss_bytes(pid: u32) -> Option<u64> {
    let t = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
    t.split_whitespace()
        .nth(1)?
        .parse::<u64>()
        .ok()
        .map(|p| p * 4096)
}

fn openrc_pid(name: &str) -> Option<u32> {
    for p in [
        format!("/run/{name}.pid"),
        format!("/run/{name}/{name}.pid"),
        format!("/var/run/{name}.pid"),
    ] {
        if let Some(pid) = std::fs::read_to_string(p)
            .ok()
            .and_then(|t| t.trim().parse().ok())
        {
            if std::path::Path::new(&format!("/proc/{pid}")).exists() {
                return Some(pid);
            }
        }
    }
    None
}

fn openrc_description(name: &str) -> String {
    std::fs::read_to_string(format!("/etc/init.d/{name}"))
        .ok()
        .and_then(|t| {
            t.lines().find_map(|l| {
                l.trim()
                    .strip_prefix("description=")
                    .map(|d| d.trim_matches(['"', '\'']).to_string())
            })
        })
        .unwrap_or_default()
}

pub async fn list_services() -> Result<Vec<ServiceStatus>, ApiError> {
    match init_system() {
        Init::OpenRc => {
            // `rc-status` lists only services that are in a runlevel; every
            // init script is a service, so ask `rc-service` for the names and
            // each one's real state.
            let names = run("rc-service", &["-l"])
                .await
                .map_err(ApiError::Internal)?;
            let enabled = parse_rc_update(&run("rc-update", &["show"]).await.unwrap_or_default());
            let mut out = Vec::new();
            for name in names
                .lines()
                .map(str::trim)
                .filter(|n| !n.is_empty() && *n != "rc" && *n != "rcS")
            {
                if !valid_service_name(name) {
                    continue;
                }
                let state = rc_state(name).await;
                let pid = openrc_pid(name);
                out.push(ServiceStatus {
                    status: match state.as_str() {
                        "started" | "starting" => "active",
                        "stopped" | "stopping" | "inactive" => "inactive",
                        "crashed" => "failed",
                        _ => "unknown",
                    }
                    .into(),
                    enabled: enabled.contains(name),
                    description: openrc_description(name),
                    memory_usage: pid.and_then(rss_bytes),
                    pid,
                    cpu_usage: None,
                    name: name.to_string(),
                });
            }
            Ok(out)
        }
        Init::Systemd => {
            let units = run(
                "systemctl",
                &[
                    "list-units",
                    "--type=service",
                    "--all",
                    "--plain",
                    "--no-legend",
                    "--no-pager",
                ],
            )
            .await
            .map_err(ApiError::Internal)?;
            let enabled = parse_systemd_enabled(
                &run(
                    "systemctl",
                    &[
                        "list-unit-files",
                        "--type=service",
                        "--plain",
                        "--no-legend",
                        "--no-pager",
                    ],
                )
                .await
                .unwrap_or_default(),
            );
            let pids = run(
                "systemctl",
                &[
                    "show",
                    "--type=service",
                    "--property=Id,MainPID",
                    "--no-pager",
                ],
            )
            .await
            .unwrap_or_default();
            let mut pid_of = std::collections::HashMap::new();
            for block in pids.split("\n\n") {
                let (mut id, mut pid) = (None, 0u32);
                for l in block.lines() {
                    if let Some(v) = l.strip_prefix("Id=") {
                        id = v.strip_suffix(".service").map(|s| s.to_string());
                    } else if let Some(v) = l.strip_prefix("MainPID=") {
                        pid = v.parse().unwrap_or(0);
                    }
                }
                if let (Some(i), true) = (id, pid > 0) {
                    pid_of.insert(i, pid);
                }
            }
            Ok(parse_systemd_units(&units)
                .into_iter()
                .map(|(name, active, description)| {
                    let pid = pid_of.get(&name).copied();
                    ServiceStatus {
                        status: match active.as_str() {
                            "active" | "activating" | "reloading" => "active",
                            "inactive" | "deactivating" => "inactive",
                            "failed" => "failed",
                            _ => "unknown",
                        }
                        .into(),
                        enabled: enabled.contains(&name),
                        description,
                        memory_usage: pid.and_then(rss_bytes),
                        pid,
                        cpu_usage: None,
                        name,
                    }
                })
                .collect())
        }
        Init::None => Err(ApiError::ServiceUnavailable(
            "no supported init system (OpenRC or systemd) is running on this host".into(),
        )),
    }
}

pub async fn is_active(name: &str) -> Result<bool, String> {
    if !valid_service_name(name) {
        return Err("bad service name".into());
    }
    match init_system() {
        Init::Systemd => Ok(run("systemctl", &["is-active", name]).await.is_ok()),
        Init::OpenRc => Ok(run("rc-service", &[name, "status"]).await.is_ok()),
        Init::None => Err("no init system".into()),
    }
}

/// Do `action` on `name` with the host's init system.
pub async fn service_action(name: &str, action: &str) -> Result<String, String> {
    if !valid_service_name(name) || !ACTIONS.contains(&action) {
        return Err("bad service name or action".into());
    }
    match init_system() {
        Init::Systemd => run("systemctl", &[action, name])
            .await
            .map(|_| format!("{action} {name}")),
        Init::OpenRc => match action {
            "enable" => run("rc-update", &["add", name, "default"]).await,
            "disable" => run("rc-update", &["del", name]).await,
            a => run("rc-service", &[name, a]).await,
        }
        .map(|_| format!("{action} {name}")),
        Init::None => Err("no supported init system is running".into()),
    }
}

/// The API itself: stopping or disabling it would end the session managing it.
fn is_self(name: &str) -> bool {
    name == "horcrux-api" || name == "horcrux" || name.starts_with("horcrux-api.")
}

async fn list() -> Result<Json<Vec<ServiceStatus>>, ApiError> {
    Ok(Json(list_services().await?))
}

async fn control(
    u: AuthExt,
    Path((name, action)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !ACTIONS.contains(&action.as_str()) {
        return Err(ApiError::BadRequest(format!(
            "action must be one of {}",
            ACTIONS.join(", ")
        )));
    }
    if !valid_service_name(&name) {
        return Err(ApiError::BadRequest("bad service name".into()));
    }
    // Only services the init system knows (no arbitrary unit/script names).
    if !list_services().await?.iter().any(|s| s.name == name) {
        return Err(ApiError::NotFound(format!("service {name} not found")));
    }
    if is_self(&name) && (action == "stop" || action == "disable") {
        return Err(ApiError::Conflict(
            "refusing to stop or disable the Horcrux API from its own UI".into(),
        ));
    }
    let who = user_of(&u);
    if is_self(&name) && action == "restart" {
        // The restart kills this process: answer first, restart a moment later.
        let n = name.clone();
        tasks::record_done(
            "service",
            &format!("{name}:restart"),
            &who,
            &Ok("restart scheduled".into()),
        );
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(800)).await;
            let _ = service_action(&n, "restart").await;
        });
        return Ok(Json(serde_json::json!({"ok": true, "scheduled": true})));
    }
    let r = service_action(&name, &action).await;
    tasks::record_done("service", &format!("{name}:{action}"), &who, &r);
    r.map_err(ApiError::Internal)?;
    Ok(Json(serde_json::json!({"ok": true})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rc_update_output_parses() {
        let en = parse_rc_update(" sshd | default\n chronyd |\n local | default nonetwork\n");
        assert!(en.contains("sshd") && en.contains("local") && !en.contains("chronyd"));
    }

    #[test]
    fn rc_service_status_text_parses() {
        assert_eq!(parse_rc_service_status(" * status: started\n"), "started");
        assert_eq!(parse_rc_service_status(" * status: crashed\n"), "crashed");
        assert_eq!(parse_rc_service_status("garbage"), "unknown");
    }

    #[test]
    fn systemd_output_parses() {
        let u = "ssh.service loaded active running OpenBSD Secure Shell server\n● bad.service loaded failed failed A broken thing\n";
        let v = parse_systemd_units(u);
        assert_eq!(
            v[0],
            (
                "ssh".into(),
                "active".into(),
                "OpenBSD Secure Shell server".into()
            )
        );
        assert_eq!(v[1].0, "bad");
        assert_eq!(v[1].1, "failed");
        let e = parse_systemd_enabled(
            "ssh.service enabled enabled\ncron.service disabled enabled\ndbus.service static -\n",
        );
        assert!(e.contains("ssh") && !e.contains("cron") && !e.contains("dbus"));
    }

    #[test]
    fn names_are_vetted_and_the_api_protects_itself() {
        assert!(valid_service_name("systemd-timesyncd") && valid_service_name("getty@tty1"));
        assert!(
            !valid_service_name("a b")
                && !valid_service_name("-rf")
                && !valid_service_name("a;b")
                && !valid_service_name("")
        );
        assert!(is_self("horcrux-api") && !is_self("sshd"));
    }
}
