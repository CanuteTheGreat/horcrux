//! Open vSwitch bridges, ports and OpenFlow rules over the API (`/api/sdn/ovs/...`).
//!
//! A thin, validated layer over `horcrux_sdn::sdn::ovs::OvsManager` (which drives
//! `ovs-vsctl` / `ovs-ofctl`). A node without Open vSwitch answers a structured 424
//! (`ovs-missing`), never a 500. Names are checked here so a value can never reach
//! the tool as an option.
use crate::error::ApiError;
use crate::AppState;
use axum::{
    extract::Path,
    routing::{delete, get, post},
    Json, Router,
};
use horcrux_sdn::sdn::ovs::{BridgeInfo, OvsBridge, OvsFlow, OvsManager, OvsPort};
use serde::Serialize;
use std::sync::Arc;

pub fn routes(_state: &Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/sdn/ovs/bridges",
            get(list_bridges).post(create_bridge),
        )
        .route(
            "/api/sdn/ovs/bridges/:bridge",
            get(get_bridge).delete(delete_bridge),
        )
        .route("/api/sdn/ovs/bridges/:bridge/ports", post(add_port))
        .route(
            "/api/sdn/ovs/bridges/:bridge/ports/:port",
            delete(delete_port),
        )
        .route(
            "/api/sdn/ovs/bridges/:bridge/flows",
            get(show_flows).post(add_flow).delete(delete_flows),
        )
}

/// OVS interface names: 1-15 chars of `[A-Za-z0-9_.-]`, not starting with `-`.
pub fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 15
        && !s.starts_with('-')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

fn name(kind: &str, s: &str) -> Result<(), ApiError> {
    if valid_name(s) {
        Ok(())
    } else {
        Err(ApiError::ValidationError(format!(
            "{kind} name must be 1-15 characters of letters, digits, '_', '.' or '-' (not starting with '-')"
        )))
    }
}

fn available() -> Result<(), ApiError> {
    if OvsManager::check_ovs_available() {
        Ok(())
    } else {
        Err(ApiError::NotAvailable {
            reason: "ovs-missing",
            message: "Open vSwitch isn't installed or running on this node.".into(),
            hint: "Install and start Open vSwitch (net-misc/openvswitch, then rc-service ovs-vswitchd start) and reload this page.".into(),
        })
    }
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .map_err(|e| {
            if e.contains("no bridge named") || e.contains("no port named") {
                ApiError::NotFound(e)
            } else {
                ApiError::Internal(e)
            }
        })
}

#[derive(Serialize)]
struct Done {
    ok: bool,
}

async fn list_bridges() -> Result<Json<Vec<String>>, ApiError> {
    available()?;
    Ok(Json(blocking(OvsManager::list_bridges).await?))
}

async fn create_bridge(Json(b): Json<OvsBridge>) -> Result<Json<BridgeInfo>, ApiError> {
    name("bridge", &b.name)?;
    available()?;
    let n = b.name.clone();
    if blocking(OvsManager::list_bridges).await?.contains(&n) {
        return Err(ApiError::Conflict(format!("bridge {n} already exists")));
    }
    blocking(move || OvsManager::create_bridge(&b)).await?;
    Ok(Json(
        blocking(move || OvsManager::get_bridge_info(&n)).await?,
    ))
}

async fn get_bridge(Path(bridge): Path<String>) -> Result<Json<BridgeInfo>, ApiError> {
    name("bridge", &bridge)?;
    available()?;
    existing(&bridge).await?;
    Ok(Json(
        blocking(move || OvsManager::get_bridge_info(&bridge)).await?,
    ))
}

async fn existing(bridge: &str) -> Result<(), ApiError> {
    if blocking(OvsManager::list_bridges)
        .await?
        .iter()
        .any(|b| b == bridge)
    {
        Ok(())
    } else {
        Err(ApiError::NotFound(format!("bridge {bridge} not found")))
    }
}

async fn delete_bridge(Path(bridge): Path<String>) -> Result<Json<Done>, ApiError> {
    name("bridge", &bridge)?;
    available()?;
    existing(&bridge).await?;
    blocking(move || OvsManager::delete_bridge(&bridge)).await?;
    Ok(Json(Done { ok: true }))
}

async fn add_port(
    Path(bridge): Path<String>,
    Json(p): Json<OvsPort>,
) -> Result<Json<BridgeInfo>, ApiError> {
    name("bridge", &bridge)?;
    name("port", &p.name)?;
    if p.tag.is_some_and(|t| t == 0 || t > 4095) || p.trunks.iter().any(|t| *t == 0 || *t > 4095) {
        return Err(ApiError::ValidationError("VLAN ids are 1-4095".into()));
    }
    available()?;
    existing(&bridge).await?;
    let b = bridge.clone();
    blocking(move || OvsManager::add_port(&b, &p)).await?;
    Ok(Json(
        blocking(move || OvsManager::get_bridge_info(&bridge)).await?,
    ))
}

async fn delete_port(Path((bridge, port)): Path<(String, String)>) -> Result<Json<Done>, ApiError> {
    name("bridge", &bridge)?;
    name("port", &port)?;
    available()?;
    existing(&bridge).await?;
    let b = bridge.clone();
    let ports = blocking(move || OvsManager::list_ports(&b)).await?;
    if !ports.contains(&port) {
        return Err(ApiError::NotFound(format!(
            "port {port} not on bridge {bridge}"
        )));
    }
    blocking(move || OvsManager::delete_port(&bridge, &port)).await?;
    Ok(Json(Done { ok: true }))
}

async fn show_flows(Path(bridge): Path<String>) -> Result<Json<Vec<String>>, ApiError> {
    name("bridge", &bridge)?;
    available()?;
    existing(&bridge).await?;
    let out = blocking(move || OvsManager::show_flows(&bridge)).await?;
    Ok(Json(
        out.lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect(),
    ))
}

async fn add_flow(
    Path(bridge): Path<String>,
    Json(f): Json<OvsFlow>,
) -> Result<Json<Done>, ApiError> {
    name("bridge", &bridge)?;
    let bad = |s: &str| s.contains(char::is_whitespace);
    if f.actions.is_empty() || f.actions.iter().any(|a| a.trim().is_empty()) {
        return Err(ApiError::ValidationError("a flow needs actions".into()));
    }
    if f.matches
        .iter()
        .any(|(k, v)| k.is_empty() || bad(k) || bad(v) || k.contains(['=', ',']) || v.contains(','))
    {
        return Err(ApiError::ValidationError(
            "flow match keys and values must not contain spaces, commas or '='".into(),
        ));
    }
    available()?;
    existing(&bridge).await?;
    blocking(move || OvsManager::add_flow(&bridge, &f)).await?;
    Ok(Json(Done { ok: true }))
}

async fn delete_flows(Path(bridge): Path<String>) -> Result<Json<Done>, ApiError> {
    name("bridge", &bridge)?;
    available()?;
    existing(&bridge).await?;
    blocking(move || OvsManager::delete_flows(&bridge)).await?;
    Ok(Json(Done { ok: true }))
}

#[cfg(test)]
mod tests {
    use super::valid_name;

    #[test]
    fn names_cannot_be_options_or_too_long() {
        assert!(valid_name("ovsbr0"));
        assert!(valid_name("vxlan-100.a_b"));
        assert!(!valid_name(""));
        assert!(!valid_name("--help"));
        assert!(!valid_name("-x"));
        assert!(!valid_name("has space"));
        assert!(!valid_name("a;b"));
        assert!(!valid_name("0123456789abcdef"));
    }
}
