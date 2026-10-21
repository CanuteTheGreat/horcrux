//! SDN: CNI networks and their attachments, and network policies.
//!
//! - `GET/POST   /api/cni/networks`, `GET/DELETE /api/cni/networks/:name`
//!   (409 while containers are attached)
//! - `POST /api/cni/attach`  `{network_name, container_id, interface_name?,
//!   netns_path?, namespace?, labels?}`: without `netns_path` the container
//!   is a Horcrux container (the UI's picker) and its network namespace is
//!   looked up from its runtime (`/proc/<pid>/ns/net`).
//! - `POST /api/cni/detach`  `{network_name, container_id}` (interface and
//!   netns default to the recorded ones; DEL releases the IPAM lease)
//! - `POST /api/cni/check`, `GET /api/cni/attachments[/:container_id]`,
//!   `GET /api/cni/capabilities/:plugin_type`
//! - `/api/network-policies...` (list, create, get, delete, namespace list,
//!   generated iptables/nftables text)
//!
//! Every network name goes through `validation::validate_cni_network_name`
//! here (400) and again in `CniManager`'s path builder.
use crate::error::ApiError;
use crate::sdn;
use crate::AppState;
use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use horcrux_common::{ContainerRuntime, ContainerStatus};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/cni/networks",
            get(list_cni_networks).post(create_cni_network),
        )
        .route(
            "/api/cni/networks/:name",
            get(get_cni_network).delete(delete_cni_network),
        )
        .route("/api/cni/attach", post(attach_container_to_network))
        .route("/api/cni/detach", post(detach_container_from_network))
        .route("/api/cni/check", post(check_container_network))
        .route("/api/cni/attachments", get(list_all_attachments))
        .route(
            "/api/cni/attachments/:container_id",
            get(list_container_attachments),
        )
        .route(
            "/api/cni/capabilities/:plugin_type",
            get(get_cni_plugin_capabilities),
        )
        .route(
            "/api/network-policies",
            get(list_network_policies).post(create_network_policy),
        )
        .route(
            "/api/network-policies/:id",
            get(get_network_policy).delete(delete_network_policy),
        )
        .route(
            "/api/network-policies/namespace/:namespace",
            get(list_policies_in_namespace),
        )
        .route(
            "/api/network-policies/:id/iptables",
            get(get_policy_iptables_rules),
        )
        .route(
            "/api/network-policies/:id/nftables",
            get(get_policy_nftables_rules),
        )
}

/// Bad input is a 400 (not the generic 422), everything else keeps its
/// usual status (404 unknown network, 409 conflicts, 500 plugin failures).
fn err(e: horcrux_common::Error) -> ApiError {
    match e {
        horcrux_common::Error::Validation(m) | horcrux_common::Error::InvalidConfig(m) => {
            ApiError::BadRequest(m)
        }
        other => ApiError::from(other),
    }
}

fn check_name(name: &str) -> Result<(), ApiError> {
    crate::validation::validate_cni_network_name(name).map_err(err)
}

async fn list_cni_networks(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<sdn::cni::CniConfig>>, ApiError> {
    Ok(Json(state.cni_manager.read().await.list_networks()))
}

#[derive(Debug, Deserialize)]
pub struct IpamRequest {
    #[serde(default)]
    pub ipam_type: Option<String>,
    #[serde(default)]
    pub subnet: Option<String>,
    #[serde(default)]
    pub range_start: Option<std::net::IpAddr>,
    #[serde(default)]
    pub range_end: Option<std::net::IpAddr>,
    #[serde(default)]
    pub gateway: Option<std::net::IpAddr>,
}

/// The UI's flat form (`subnet`, `gateway`, ...) or a full CniConfig-shaped
/// body with a nested `ipam` object; flat fields win.
#[derive(Debug, Deserialize)]
pub struct CreateCniNetworkRequest {
    pub name: String,
    pub plugin_type: sdn::cni::CniPluginType,
    #[serde(default)]
    pub bridge: Option<String>,
    #[serde(default)]
    pub subnet: Option<String>,
    #[serde(default)]
    pub gateway: Option<std::net::IpAddr>,
    #[serde(default)]
    pub range_start: Option<std::net::IpAddr>,
    #[serde(default)]
    pub range_end: Option<std::net::IpAddr>,
    #[serde(default)]
    pub ipam: Option<IpamRequest>,
}

async fn create_cni_network(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateCniNetworkRequest>,
) -> Result<Json<sdn::cni::CniConfig>, ApiError> {
    check_name(&req.name)?;
    let nested = req.ipam.unwrap_or(IpamRequest {
        ipam_type: None,
        subnet: None,
        range_start: None,
        range_end: None,
        gateway: None,
    });
    let blank = |s: Option<String>| s.filter(|v| !v.trim().is_empty());
    let config = sdn::cni::CniConfig {
        cni_version: "1.0.0".to_string(),
        name: req.name,
        plugin_type: req.plugin_type,
        bridge: blank(req.bridge),
        ipam: sdn::cni::IpamConfig {
            ipam_type: nested.ipam_type.unwrap_or_else(|| "host-local".to_string()),
            subnet: blank(req.subnet).or(blank(nested.subnet)),
            range_start: req.range_start.or(nested.range_start),
            range_end: req.range_end.or(nested.range_end),
            gateway: req.gateway.or(nested.gateway),
            routes: vec![sdn::cni::RouteConfig {
                dst: "0.0.0.0/0".to_string(),
                gw: None,
            }],
        },
        dns: None,
        capabilities: HashMap::new(),
    };
    // Default route matches the subnet's address family.
    let mut config = config;
    if let Some((addr, _)) = config
        .ipam
        .subnet
        .as_deref()
        .and_then(|s| sdn::cni::parse_cidr(s).ok())
    {
        if addr.is_ipv6() {
            config.ipam.routes[0].dst = "::/0".to_string();
        }
    }

    state
        .cni_manager
        .write()
        .await
        .create_network(config.clone())
        .await
        .map_err(err)?;
    Ok(Json(config))
}

async fn get_cni_network(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<sdn::cni::CniConfig>, ApiError> {
    check_name(&name)?;
    state
        .cni_manager
        .read()
        .await
        .get_network(&name)
        .cloned()
        .ok_or_else(|| ApiError::NotFound(format!("Network {} not found", name)))
        .map(Json)
}

async fn delete_cni_network(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<&'static str>, ApiError> {
    check_name(&name)?;
    state
        .cni_manager
        .write()
        .await
        .delete_network(&name)
        .await
        .map_err(err)?;
    Ok(Json("Network deleted"))
}

/// PID of a running Horcrux container's init process, from its runtime.
async fn container_pid(state: &AppState, id: &str) -> Result<(u32, String), ApiError> {
    let ct = state.container_manager.get_container(id).await?;
    if ct.status != ContainerStatus::Running {
        return Err(ApiError::Conflict(format!(
            "Container {} is not running; start it before attaching it to a network",
            id
        )));
    }
    let run = |prog: &str, args: Vec<String>| {
        let prog = prog.to_string();
        async move {
            let out = tokio::process::Command::new(&prog)
                .args(&args)
                .output()
                .await
                .map_err(|e| ApiError::Internal(format!("{}: {}", prog, e)))?;
            if !out.status.success() {
                return Err(ApiError::Internal(format!(
                    "{} {}: {}",
                    prog,
                    args.join(" "),
                    String::from_utf8_lossy(&out.stderr).trim()
                )));
            }
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
        }
    };
    let name = ct.name.clone();
    let out = match ct.runtime {
        ContainerRuntime::Docker | ContainerRuntime::Podman => {
            let cli = if ct.runtime == ContainerRuntime::Docker {
                "docker"
            } else {
                "podman"
            };
            run(
                cli,
                vec![
                    "inspect".into(),
                    "-f".into(),
                    "{{.State.Pid}}".into(),
                    name.clone(),
                ],
            )
            .await?
        }
        ContainerRuntime::Lxc => {
            #[allow(unused_mut)]
            let mut args: Vec<String> = Vec::new();
            #[cfg(feature = "lxc")]
            {
                args.push("-P".into());
                args.push(
                    state
                        .container_manager
                        .lxc()
                        .lxc_path()
                        .to_string_lossy()
                        .to_string(),
                );
            }
            args.extend(["-n".into(), name.clone(), "-p".into(), "-H".into()]);
            run("lxc-info", args).await?
        }
        ContainerRuntime::Lxd | ContainerRuntime::Incus => {
            let cli = if ct.runtime == ContainerRuntime::Lxd {
                "lxc"
            } else {
                "incus"
            };
            let json = run(
                cli,
                vec!["query".into(), format!("/1.0/instances/{}/state", name)],
            )
            .await?;
            serde_json::from_str::<serde_json::Value>(&json)
                .ok()
                .and_then(|v| v["pid"].as_u64())
                .map(|p| p.to_string())
                .unwrap_or_default()
        }
    };
    let pid = parse_pid(&out).ok_or_else(|| {
        ApiError::Conflict(format!(
            "Container {} has no running init process (runtime reported {:?})",
            id, out
        ))
    })?;
    Ok((pid, ct.id))
}

/// A runtime's PID output (`1234`, `PID: 1234`), refusing 0.
fn parse_pid(out: &str) -> Option<u32> {
    out.split(|c: char| !c.is_ascii_digit())
        .find(|s| !s.is_empty())
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|p| *p > 0)
}

#[derive(Debug, Deserialize)]
pub struct AttachContainerRequest {
    pub container_id: String,
    pub network_name: String,
    /// Interface inside the container; default: the first free `netN`.
    #[serde(default)]
    pub interface_name: Option<String>,
    /// Raw network namespace (e.g. `/run/netns/x`). Omit it to attach a
    /// Horcrux container (looked up by `container_id`).
    #[serde(default)]
    pub netns_path: Option<String>,
    /// Network-policy namespace (default `default`) and labels.
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

async fn attach_container_to_network(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AttachContainerRequest>,
) -> Result<Json<sdn::cni::CniAttachment>, ApiError> {
    check_name(&req.network_name)?;
    crate::validation::validate_cni_name("Container id", &req.container_id).map_err(err)?;
    let (netns, guest) = match req.netns_path.filter(|p| !p.is_empty()) {
        Some(p) => (p, None),
        None => {
            let (pid, id) = container_pid(&state, &req.container_id).await?;
            (format!("/proc/{}/ns/net", pid), Some(id))
        }
    };
    let mut manager = state.cni_manager.write().await;
    let ifname = match req.interface_name.filter(|s| !s.is_empty()) {
        Some(i) => i,
        None => {
            let used: Vec<String> = manager
                .list_attachments(&req.container_id)
                .into_iter()
                .map(|a| a.interface_name)
                .collect();
            (0..)
                .map(|n| format!("net{}", n))
                .find(|n| !used.contains(n))
                .unwrap_or_else(|| "net0".to_string())
        }
    };
    let (_, attachment) = manager
        .attach(
            &req.container_id,
            &req.network_name,
            &ifname,
            &netns,
            guest,
            req.namespace
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "default".to_string()),
            req.labels,
        )
        .await
        .map_err(err)?;
    Ok(Json(attachment))
}

#[derive(Debug, Deserialize)]
pub struct DetachContainerRequest {
    pub container_id: String,
    pub network_name: String,
    #[serde(default)]
    pub interface_name: Option<String>,
    #[serde(default)]
    pub netns_path: Option<String>,
}

async fn detach_container_from_network(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DetachContainerRequest>,
) -> Result<Json<&'static str>, ApiError> {
    check_name(&req.network_name)?;
    let recorded = state
        .cni_manager
        .read()
        .await
        .get_attachment(&req.container_id, &req.network_name);
    // A picker attachment: the container may have restarted (new PID), so
    // use its current netns while it runs. A stopped/deleted container's
    // old path is gone, which DEL handles (it still releases the lease).
    let mut netns = req.netns_path.unwrap_or_default();
    if netns.is_empty() {
        if let Some(guest) = recorded.as_ref().and_then(|a| a.guest.clone()) {
            if let Ok((pid, _)) = container_pid(&state, &guest).await {
                netns = format!("/proc/{}/ns/net", pid);
            }
        }
    }
    state
        .cni_manager
        .write()
        .await
        .del_container(
            &req.container_id,
            &req.network_name,
            req.interface_name.as_deref().unwrap_or(""),
            &netns,
        )
        .await
        .map_err(err)?;
    Ok(Json("Container detached from network"))
}

#[derive(Debug, Deserialize)]
pub struct CheckContainerRequest {
    pub container_id: String,
    pub network_name: String,
    #[serde(default)]
    pub interface_name: Option<String>,
    #[serde(default)]
    pub netns_path: Option<String>,
}

async fn check_container_network(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CheckContainerRequest>,
) -> Result<Json<&'static str>, ApiError> {
    check_name(&req.network_name)?;
    state
        .cni_manager
        .read()
        .await
        .check_container(
            &req.container_id,
            &req.network_name,
            req.interface_name.as_deref().unwrap_or(""),
            req.netns_path.as_deref().unwrap_or(""),
        )
        .await
        .map_err(err)?;
    Ok(Json("Container network is healthy"))
}

async fn list_all_attachments(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<sdn::cni::CniAttachment>>, ApiError> {
    Ok(Json(state.cni_manager.read().await.all_attachments()))
}

async fn list_container_attachments(
    State(state): State<Arc<AppState>>,
    Path(container_id): Path<String>,
) -> Result<Json<Vec<sdn::cni::CniAttachment>>, ApiError> {
    Ok(Json(
        state
            .cni_manager
            .read()
            .await
            .list_attachments(&container_id),
    ))
}

async fn get_cni_plugin_capabilities(
    State(state): State<Arc<AppState>>,
    Path(plugin_type): Path<String>,
) -> Result<Json<Vec<String>>, ApiError> {
    let plugin_type: sdn::cni::CniPluginType =
        serde_json::from_str(&format!("\"{}\"", plugin_type))
            .map_err(|e| ApiError::BadRequest(format!("Invalid plugin type: {}", e)))?;
    let capabilities = state
        .cni_manager
        .read()
        .await
        .get_capabilities(&plugin_type)
        .await
        .map_err(err)?;
    Ok(Json(capabilities))
}

// Network Policy handlers

async fn list_network_policies(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<sdn::policy::NetworkPolicy>>, ApiError> {
    Ok(Json(
        state.network_policy_manager.read().await.list_policies(),
    ))
}

async fn create_network_policy(
    State(state): State<Arc<AppState>>,
    Json(policy): Json<sdn::policy::NetworkPolicy>,
) -> Result<Json<&'static str>, ApiError> {
    state
        .network_policy_manager
        .write()
        .await
        .create_policy(policy)
        .map_err(err)?;
    Ok(Json("Network policy created"))
}

async fn get_network_policy(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<sdn::policy::NetworkPolicy>, ApiError> {
    state
        .network_policy_manager
        .read()
        .await
        .get_policy(&id)
        .cloned()
        .ok_or_else(|| ApiError::NotFound(format!("Network policy {} not found", id)))
        .map(Json)
}

async fn delete_network_policy(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<&'static str>, ApiError> {
    state
        .network_policy_manager
        .write()
        .await
        .delete_policy(&id)
        .map_err(err)?;
    Ok(Json("Network policy deleted"))
}

async fn list_policies_in_namespace(
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
) -> Result<Json<Vec<sdn::policy::NetworkPolicy>>, ApiError> {
    Ok(Json(
        state
            .network_policy_manager
            .read()
            .await
            .list_policies_in_namespace(&namespace),
    ))
}

async fn get_policy_iptables_rules(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Vec<String>>, ApiError> {
    Ok(Json(
        state
            .network_policy_manager
            .read()
            .await
            .generate_iptables_rules(&id),
    ))
}

async fn get_policy_nftables_rules(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Vec<String>>, ApiError> {
    Ok(Json(
        state
            .network_policy_manager
            .read()
            .await
            .generate_nftables_rules(&id),
    ))
}

#[cfg(test)]
mod tests {
    use super::parse_pid;

    #[test]
    fn runtime_pid_output_is_parsed() {
        assert_eq!(parse_pid("1234\n"), Some(1234));
        assert_eq!(parse_pid("PID:          4321"), Some(4321));
        assert_eq!(parse_pid("0"), None);
        assert_eq!(parse_pid(""), None);
    }
}
