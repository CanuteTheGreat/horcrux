//! Create / join a cluster over the API (USE=cluster).
//!
//! * `POST /api/cluster/create {name, address}`: this node becomes the first
//!   member; `address` is how the other nodes reach its API.
//! * `POST /api/cluster/join {peer, username, password, address}`: this
//!   node logs in to `peer` (a member's API address) with an administrator
//!   account, asks it to register this node (`POST /api/cluster/join-request`),
//!   adopts the cluster's service secret (node-to-node tokens) and its node
//!   list, and tells the other members about itself.
//! * `POST /api/cluster/join-request`: the member's side of a join.
//!
//! Membership and heartbeat are Horcrux's own (see `cluster::usage`); no
//! corosync/pacemaker is required.
use crate::error::ApiError;
use crate::AppState;
use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/cluster/create", post(create))
        .route("/api/cluster/join", post(join))
        .route("/api/cluster/join-request", post(join_request))
}

#[derive(Deserialize)]
struct CreateRequest {
    name: String,
    /// Address (IP or host name) the other nodes use to reach this node.
    address: String,
}

fn check_address(address: &str) -> Result<(), ApiError> {
    let a = address.trim();
    if a.is_empty() || a.contains('/') || a.contains(' ') || a.contains('@') {
        return Err(ApiError::ValidationError(
            "address must be an IP address or host name (no scheme or path)".into(),
        ));
    }
    Ok(())
}

async fn create(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateRequest>,
) -> Result<(StatusCode, Json<crate::cluster::node::Node>), ApiError> {
    check_address(&req.address)?;
    let local = state.cluster_manager.get_local_node_name().await?;
    let node = state
        .cluster_manager
        .create_cluster(req.name, local, req.address.trim().to_string())
        .await?;
    Ok((StatusCode::CREATED, Json(node)))
}

#[derive(Serialize, Deserialize)]
struct JoinRequest {
    /// The joining node's name and the address peers reach it on.
    name: String,
    address: String,
}

#[derive(Serialize, Deserialize)]
struct JoinReply {
    cluster_name: String,
    /// Secret for node-to-node service tokens.
    secret: String,
    nodes: Vec<crate::cluster::node::Node>,
}

/// A member registers a joining node and hands over the cluster state.
async fn join_request(
    State(state): State<Arc<AppState>>,
    Json(req): Json<JoinRequest>,
) -> Result<Json<JoinReply>, ApiError> {
    check_address(&req.address)?;
    let Some(cluster_name) = state.cluster_manager.cluster_name().await else {
        return Err(ApiError::Conflict(
            "this node is not in a cluster yet: create one first (POST /api/cluster/create)".into(),
        ));
    };
    crate::validation::validate_hostname(&req.name)
        .map_err(|e| ApiError::ValidationError(format!("node name: {e}")))?;
    let existing = state.cluster_manager.list_nodes().await;
    state
        .cluster_manager
        .accept_member(&req.name, req.address.trim())
        .await?;
    let nodes = state.cluster_manager.list_nodes().await;
    // Tell the other members (best effort: they also learn it from the
    // next membership sync of the HA supervisor).
    let local = state.cluster_manager.get_local_node_name().await?;
    let port = state.config.server.port;
    let client = peer_client()?;
    for n in existing.iter().filter(|n| n.name != local && !n.is_local) {
        let token = crate::middleware::auth::generate_cluster_ha_token(&local)
            .map_err(ApiError::Internal)?;
        let r = client
            .post(format!("{}/api/ha/peer/members", n.api_url(port)))
            .bearer_auth(token)
            .json(&serde_json::json!({"name": req.name, "address": req.address.trim()}))
            .send()
            .await;
        if let Err(e) = r {
            tracing::warn!(
                "could not tell {} about new member {}: {e}",
                n.name,
                req.name
            );
        }
    }
    Ok(Json(JoinReply {
        cluster_name,
        secret: crate::middleware::auth::cluster_service_secret(),
        nodes,
    }))
}

fn peer_client() -> Result<reqwest::Client, ApiError> {
    // Peers serve HTTPS with their own node-local CA (see the usage monitor).
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(30))
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|e| ApiError::Internal(e.to_string()))
}

#[derive(Deserialize)]
struct JoinBody {
    /// A cluster member: `host` or `host:port` (default port: this node's).
    peer: String,
    username: String,
    password: String,
    /// Address (IP or host name) the members use to reach THIS node.
    address: String,
}

async fn join(
    State(state): State<Arc<AppState>>,
    Json(req): Json<JoinBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    check_address(&req.address)?;
    check_address(&req.peer)?;
    if state.cluster_manager.cluster_name().await.is_some() {
        return Err(ApiError::Conflict(
            "this node is already part of a cluster".into(),
        ));
    }
    let local = state.cluster_manager.get_local_node_name().await?;
    let base = if req.peer.contains(':')
        && !req.peer.starts_with('[')
        && req.peer.matches(':').count() == 1
    {
        format!("https://{}", req.peer)
    } else {
        format!("https://{}:{}", req.peer, state.config.server.port)
    };
    let client = peer_client()?;
    let login = client
        .post(format!("{base}/api/auth/login"))
        .json(&serde_json::json!({"username": req.username, "password": req.password}))
        .send()
        .await
        .map_err(|e| ApiError::BadRequest(format!("cannot reach {}: {e}", req.peer)))?;
    if !login.status().is_success() {
        return Err(ApiError::BadRequest(format!(
            "login at {} failed ({}): check the administrator user name and password",
            req.peer,
            login.status()
        )));
    }
    let body: serde_json::Value = login
        .json()
        .await
        .map_err(|e| ApiError::BadRequest(format!("unexpected login reply: {e}")))?;
    let token = body
        .get("token")
        .or_else(|| body.get("ticket"))
        .and_then(|t| t.as_str())
        .ok_or_else(|| {
            ApiError::BadRequest(
                "the login did not return a token (accounts with a second factor must use an API key)"
                    .into(),
            )
        })?
        .to_string();
    let resp = client
        .post(format!("{base}/api/cluster/join-request"))
        .bearer_auth(&token)
        .json(&JoinRequest {
            name: local.clone(),
            address: req.address.trim().to_string(),
        })
        .send()
        .await
        .map_err(|e| ApiError::BadRequest(format!("join request failed: {e}")))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(ApiError::BadRequest(format!(
            "{} refused the join ({status}): {text}",
            req.peer
        )));
    }
    let reply: JoinReply = serde_json::from_str(&text)
        .map_err(|e| ApiError::BadRequest(format!("unexpected join reply: {e}")))?;
    crate::middleware::auth::set_cluster_secret(&state.config.paths.data_dir, &reply.secret)
        .map_err(|e| ApiError::Internal(format!("saving the cluster secret: {e}")))?;
    state
        .cluster_manager
        .join_cluster(reply.cluster_name.clone(), &local, reply.nodes)
        .await?;
    Ok(Json(serde_json::json!({
        "cluster": reply.cluster_name,
        "nodes": state.cluster_manager.list_nodes().await.len(),
    })))
}
