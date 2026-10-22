//! Host integration: the unit/service that runs `horcrux-api guests stop`
//! when the host shuts down and `horcrux-api guests boot` when it comes up.
//!
//! The command talks to the running API over loopback with the host-agent
//! token (`<data dir>/saved-state/host-agent.token`, root-only, created at
//! API start), so it needs no user account and the API stays the single
//! owner of VM/container state. The routes are outside the login
//! middleware and accept nothing but that token.
use super::engine;
use crate::error::ApiError;
use crate::AppState;
use axum::{extract::State, http::HeaderMap, routing::{get, post}, Json, Router};
use std::path::Path;
use std::sync::Arc;
use subtle::ConstantTimeEq;

pub const TOKEN_HEADER: &str = "x-horcrux-host-token";

/// Public-router routes (merged outside the auth layer in app/mod.rs).
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/host-agent/stop-guests", post(stop_guests))
        .route("/api/host-agent/boot-guests", post(boot_guests))
        .route("/api/host-agent/status", get(status))
}

/// Create the token file if missing (mode 0600).
pub fn ensure_token(data_dir: &Path) -> std::io::Result<()> {
    let p = engine::token_path(data_dir);
    if p.exists() {
        return Ok(());
    }
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut bytes = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut bytes);
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&p)?;
    f.write_all(hex::encode(bytes).as_bytes())
}

fn check(headers: &HeaderMap, state: &AppState) -> Result<(), ApiError> {
    let want = std::fs::read_to_string(engine::token_path(&state.config.paths.data_dir))
        .map_err(|_| ApiError::Forbidden("host-agent token not available".into()))?;
    let got = headers
        .get(TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if want.trim().is_empty() || !bool::from(want.trim().as_bytes().ct_eq(got.as_bytes())) {
        return Err(ApiError::Forbidden("invalid host-agent token".into()));
    }
    Ok(())
}

async fn stop_guests(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<engine::Report>, ApiError> {
    check(&headers, &state)?;
    Ok(Json(engine::stop_all(&state).await))
}

async fn boot_guests(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<engine::Report>, ApiError> {
    check(&headers, &state)?;
    Ok(Json(engine::boot_all(&state, false).await))
}

async fn status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    check(&headers, &state)?;
    Ok(Json(serde_json::json!({
        "boot_id": engine::boot_id(),
        "manifest": engine::read_manifest(&state),
    })))
}

/// `horcrux-api guests <stop|boot|status>`: what the unit runs. Returns the
/// process exit code. A failure to reach the API never blocks a shutdown
/// (exit 0 with a warning); a boot that cannot reach it exits 1.
pub async fn cli_main(args: &[String]) -> i32 {
    let cmd = args.first().map(String::as_str).unwrap_or("");
    let (path, method, wait) = match cmd {
        "stop" => ("stop-guests", "POST", 0u64),
        "boot" => ("boot-guests", "POST", 300),
        "status" => ("status", "GET", 0),
        _ => {
            eprintln!("usage: horcrux-api guests <stop|boot|status>");
            return 2;
        }
    };
    let cfg = crate::config::HorcruxConfig::load();
    let host = match cfg.server.host.as_str() {
        "0.0.0.0" | "::" | "" => "127.0.0.1".to_string(),
        h => h.to_string(),
    };
    let scheme = if cfg.server.tls_enabled { "https" } else { "http" };
    let base = format!("{scheme}://{host}:{}", cfg.server.port);
    let token = match std::fs::read_to_string(engine::token_path(&cfg.paths.data_dir)) {
        Ok(t) => t.trim().to_string(),
        Err(e) => {
            eprintln!("horcrux guests {cmd}: cannot read the host-agent token: {e}");
            return if cmd == "stop" { 0 } else { 1 };
        }
    };
    let client = match reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("horcrux guests {cmd}: {e}");
            return 1;
        }
    };
    // Boot: the API may still be starting.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(wait);
    loop {
        let up = client
            .get(format!("{base}/api/health"))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false);
        if up {
            break;
        }
        if std::time::Instant::now() >= deadline {
            eprintln!("horcrux guests {cmd}: the API at {base} is not answering");
            return if cmd == "stop" { 0 } else { 1 };
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    let req = if method == "POST" {
        client.post(format!("{base}/api/host-agent/{path}"))
    } else {
        client.get(format!("{base}/api/host-agent/{path}"))
    };
    let resp = match req.header(TOKEN_HEADER, token).send().await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("horcrux guests {cmd}: {e}");
            return if cmd == "stop" { 0 } else { 1 };
        }
    };
    let ok = resp.status().is_success();
    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    if let Some(list) = body["outcomes"].as_array() {
        for o in list {
            println!(
                "{} {} {} ({}){}",
                o["action"].as_str().unwrap_or("?"),
                o["kind"].as_str().unwrap_or("?"),
                o["name"].as_str().unwrap_or("?"),
                o["id"].as_str().unwrap_or("?"),
                o["message"]
                    .as_str()
                    .map(|m| format!(" - {m}"))
                    .unwrap_or_default()
            );
        }
        println!("{}", body["message"].as_str().unwrap_or(""));
    } else {
        println!("{}", serde_json::to_string_pretty(&body).unwrap_or_default());
    }
    if ok || cmd == "stop" {
        0
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    /// The unit must be ordered after the API, the container runtimes and
    /// storage/network (so that on shutdown it stops before them), save on
    /// stop and never kill the guests with its own cgroup.
    #[test]
    fn systemd_unit_orders_stop_before_storage_and_network() {
        let unit = include_str!("../../../../deploy/systemd/horcrux-guests.service");
        let after = unit
            .lines()
            .find(|l| l.starts_with("After="))
            .expect("After=");
        for dep in [
            "horcrux.service",
            "network-online.target",
            "local-fs.target",
            "remote-fs.target",
            "docker.service",
            "incus.service",
        ] {
            assert!(after.contains(dep), "After= must list {dep}");
        }
        assert!(unit.contains("ExecStop=/usr/bin/horcrux-api guests stop"));
        assert!(unit.contains("ExecStart=/usr/bin/horcrux-api guests boot"));
        assert!(unit.contains("KillMode=none"));
        assert!(unit.contains("RemainAfterExit=yes"));
        let stop = unit
            .lines()
            .find_map(|l| l.strip_prefix("TimeoutStopSec="))
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap();
        assert!(stop >= 600, "enough time to save every guest");
    }

    #[test]
    fn openrc_service_needs_the_api_and_storage() {
        let rc = include_str!("../../../../deploy/openrc/horcrux-guests");
        assert!(rc.starts_with("#!/sbin/openrc-run"));
        assert!(rc.contains("need horcrux localmount"));
        assert!(rc.contains("horcrux-api guests stop"));
        assert!(rc.contains("horcrux-api guests boot"));
    }
}
