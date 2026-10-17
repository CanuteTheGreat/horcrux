//! API client for communicating with Horcrux backend

#![allow(dead_code)]

use horcrux_common::VmConfig;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

const AUTH_TOKEN_KEY: &str = "horcrux_auth_token";

/// Relative API base - see `resolve_api_url`'s doc comment for why this must
/// never be an absolute `http://host:port` URL again.
const API_BASE: &str = "/api";

/// Resolve an API path (e.g. "/vms") to the URL actually used for the request.
/// Always same-origin/relative by default, so this works regardless of what
/// host:port the page itself was loaded from: production (horcrux-api serves
/// horcrux-ui/dist itself on one port - see main.rs's ServeDir fallback), a
/// local `trunk serve` dev server (proxied to the backend via Trunk.toml), or
/// a remote/tunneled QA run that only forwards the frontend's port.
///
/// This used to be a hardcoded `http://localhost:8006` absolute URL, which
/// broke as soon as the browser's own "localhost" wasn't literally the
/// backend host - e.g. an SSM port-forward tunnel forwarding only the
/// frontend's port, or any real split-host deployment. The backend also has
/// no CorsLayer, so that cross-origin call failed before it even reached the
/// network tab as a real HTTP response - it surfaced as a bare
/// "TypeError: Failed to fetch" (2026-10-12 QA finding on the /vms page).
fn resolve_api_url(path: &str) -> String {
    if path.starts_with("http") || path.starts_with("/api") {
        path.to_string()
    } else {
        format!("/api{}", path)
    }
}

/// Store the JWT auth token in browser localStorage after a successful login.
pub fn set_auth_token(token: &str) {
    if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = storage.set_item(AUTH_TOKEN_KEY, token);
    }
}

/// Read the stored JWT auth token, if any.
pub fn get_auth_token() -> Option<String> {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item(AUTH_TOKEN_KEY).ok().flatten())
}

/// Clear the stored JWT auth token (on logout).
pub fn clear_auth_token() {
    if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = storage.remove_item(AUTH_TOKEN_KEY);
    }
}

/// Apply the stored Authorization header to a request builder, if a token is present.
fn with_auth(request: reqwasm::http::Request) -> reqwasm::http::Request {
    if let Some(token) = get_auth_token() {
        request.header("Authorization", &format!("Bearer {}", token))
    } else {
        request
    }
}

/// Build an ApiError from a non-2xx response, keeping the server's error
/// message (e.g. a 422 validation reason) so forms can show the user why.
async fn http_error(response: reqwasm::http::Response) -> ApiError {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    ApiError {
        message: error_text(status, &body),
    }
}

/// User-facing text for a failed request: the API's `message`, plus its
/// `details` (the real cause; a 500 used to show only "An internal server
/// error occurred").
pub fn error_text(status: u16, body: &str) -> String {
    let v = serde_json::from_str::<serde_json::Value>(body).ok();
    let field = |k: &str| {
        v.as_ref()
            .and_then(|v| v.get(k))
            .and_then(|m| m.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let detail = match (
        field("message").or_else(|| field("error")),
        field("details"),
    ) {
        (Some(m), Some(d)) if m != d => format!("{}: {}", m, d),
        (Some(m), _) => m,
        (None, Some(d)) => d,
        (None, None) if v.is_none() => body.trim().to_string(),
        (None, None) => String::new(),
    };
    if detail.is_empty() {
        format!("HTTP {}", status)
    } else {
        format!("HTTP {}: {}", status, detail)
    }
}

/// Generic JSON fetch helper
pub async fn fetch_json<T: DeserializeOwned>(path: &str) -> Result<T, ApiError> {
    let url = resolve_api_url(path);

    let response = with_auth(reqwasm::http::Request::get(&url))
        .send()
        .await
        .map_err(|e| ApiError {
            message: e.to_string(),
        })?;

    if response.ok() {
        let text = response.text().await.map_err(|e| ApiError {
            message: e.to_string(),
        })?;
        decode_body(&text)
    } else {
        Err(ApiError {
            message: format!("HTTP {}", response.status()),
        })
    }
}

/// POST request helper
pub async fn post_json<T: DeserializeOwned, B: Serialize>(
    path: &str,
    body: &B,
) -> Result<T, ApiError> {
    let url = resolve_api_url(path);

    let response = with_auth(reqwasm::http::Request::post(&url))
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(body).unwrap())
        .send()
        .await
        .map_err(|e| ApiError {
            message: e.to_string(),
        })?;

    if response.ok() {
        let text = response.text().await.map_err(|e| ApiError {
            message: e.to_string(),
        })?;
        decode_body(&text)
    } else {
        Err(http_error(response).await)
    }
}

/// PUT request helper
pub async fn put_json<T: DeserializeOwned, B: Serialize>(
    path: &str,
    body: &B,
) -> Result<T, ApiError> {
    let url = resolve_api_url(path);

    let response = with_auth(reqwasm::http::Request::put(&url))
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(body).unwrap())
        .send()
        .await
        .map_err(|e| ApiError {
            message: e.to_string(),
        })?;

    if response.ok() {
        let text = response.text().await.map_err(|e| ApiError {
            message: e.to_string(),
        })?;
        decode_body(&text)
    } else {
        Err(http_error(response).await)
    }
}

/// Decode a successful response body. Endpoints that answer with a bare
/// status code (201/204, empty body) decode as JSON `null`, so callers
/// typed `Result<(), _>` succeed instead of reporting "EOF while parsing"
/// for an action that actually worked (run backup job, restore, ...).
pub fn decode_body<T: DeserializeOwned>(text: &str) -> Result<T, ApiError> {
    let text = if text.trim().is_empty() { "null" } else { text };
    serde_json::from_str(text).map_err(|e| ApiError {
        message: e.to_string(),
    })
}

/// DELETE request helper
pub async fn delete_json(path: &str) -> Result<(), ApiError> {
    let url = resolve_api_url(path);

    let response = with_auth(reqwasm::http::Request::delete(&url))
        .send()
        .await
        .map_err(|e| ApiError {
            message: e.to_string(),
        })?;

    if response.ok() {
        Ok(())
    } else {
        Err(http_error(response).await)
    }
}

/// POST without body helper
pub async fn post_empty(path: &str) -> Result<(), ApiError> {
    let url = resolve_api_url(path);

    let response = with_auth(reqwasm::http::Request::post(&url))
        .send()
        .await
        .map_err(|e| ApiError {
            message: e.to_string(),
        })?;

    if response.ok() {
        Ok(())
    } else {
        Err(http_error(response).await)
    }
}

/// POST with a JSON body for endpoints that reply with a bare status code
/// (no JSON body) — `post_json` would wrongly report these as failures
/// because it always tries to parse a JSON response.
pub async fn post_json_no_response<B: Serialize>(path: &str, body: &B) -> Result<(), ApiError> {
    let url = resolve_api_url(path);

    let response = with_auth(reqwasm::http::Request::post(&url))
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(body).unwrap())
        .send()
        .await
        .map_err(|e| ApiError {
            message: e.to_string(),
        })?;

    if response.ok() {
        Ok(())
    } else {
        Err(http_error(response).await)
    }
}

/// Generic text fetch helper
pub async fn fetch_text(path: &str) -> Result<String, ApiError> {
    let url = resolve_api_url(path);

    let response = with_auth(reqwasm::http::Request::get(&url))
        .send()
        .await
        .map_err(|e| ApiError {
            message: e.to_string(),
        })?;

    if response.ok() {
        response.text().await.map_err(|e| ApiError {
            message: e.to_string(),
        })
    } else {
        Err(ApiError {
            message: format!("HTTP {}", response.status()),
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiError {
    pub message: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ApiError {}

/// Get all VMs
pub async fn get_vms() -> Result<Vec<VmConfig>, ApiError> {
    fetch_json("/vms").await
}

/// Create a new VM
pub async fn create_vm(config: VmConfig) -> Result<VmConfig, ApiError> {
    post_json("/vms", &config).await
}

/// Start a VM
pub async fn start_vm(vm_id: &str) -> Result<(), ApiError> {
    post_empty(&format!("/vms/{}/start", vm_id)).await
}

/// Stop a VM
pub async fn stop_vm(vm_id: &str) -> Result<(), ApiError> {
    post_empty(&format!("/vms/{}/stop", vm_id)).await
}

/// Delete a VM
pub async fn delete_vm(vm_id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/vms/{}", vm_id)).await
}

/// Host metrics shown on the dashboard (flattened view of the API's
/// `monitoring::NodeMetrics`, whose CPU/memory figures are nested).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(from = "NodeMetricsWire")]
pub struct NodeMetrics {
    pub hostname: String,
    pub timestamp: i64,
    pub cpu_usage: f64,
    pub memory_used: u64,
    pub memory_total: u64,
    pub uptime_seconds: u64,
}

#[derive(Deserialize)]
struct NodeMetricsWire {
    hostname: String,
    timestamp: i64,
    cpu: NodeCpuWire,
    memory: NodeMemoryWire,
    uptime_seconds: u64,
}

#[derive(Deserialize)]
struct NodeCpuWire {
    usage_percent: f64,
}

#[derive(Deserialize)]
struct NodeMemoryWire {
    total_bytes: u64,
    used_bytes: u64,
}

impl From<NodeMetricsWire> for NodeMetrics {
    fn from(w: NodeMetricsWire) -> Self {
        Self {
            hostname: w.hostname,
            timestamp: w.timestamp,
            cpu_usage: w.cpu.usage_percent,
            memory_used: w.memory.used_bytes,
            memory_total: w.memory.total_bytes,
            uptime_seconds: w.uptime_seconds,
        }
    }
}

/// Get node metrics. `None` until the collector has taken its first sample.
pub async fn get_node_metrics() -> Result<Option<NodeMetrics>, ApiError> {
    fetch_json("/monitoring/node").await
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Alert {
    pub id: String,
    pub rule_name: String,
    pub severity: String,
    pub status: String,
    pub message: String,
    pub target: String,
    pub fired_at: i64,
}

/// Get active alerts
pub async fn get_active_alerts() -> Result<Vec<ActiveAlert>, ApiError> {
    fetch_json("/alerts/active").await
}

/// A cluster node exactly as `GET /api/cluster/nodes` returns it
/// (horcrux-api `cluster::node::Node`). `memory_total` is in **bytes**.
///
/// This used to carry a nested `resources` object (cpu_cores/memory_mb/
/// disk_gb) that the API has never sent; with `#[serde(default)]` it
/// silently zeroed out, so every node on /ha/cluster showed
/// "0 CPU cores / 0 MB RAM".
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClusterNode {
    #[serde(default)]
    pub id: u32,
    pub name: String,
    pub ip: String,
    /// "online" | "offline" | "unknown"
    pub status: String,
    /// Placement/HA weight (0-1000, default 100): higher-weighted nodes are
    /// preferred for placement and HA failover.
    #[serde(default = "default_node_priority")]
    pub priority: u32,
    #[serde(default)]
    pub is_local: bool,
    pub architecture: String,
    pub cpu_cores: u32,
    /// Total RAM in bytes.
    pub memory_total: u64,
    /// Live CPU utilisation in percent, reported by the node's own
    /// heartbeat. `None` = unknown (unreachable / never reported) -- show it
    /// as unknown, never as 0%.
    #[serde(default)]
    pub cpu_usage: Option<f64>,
    /// Live RAM in use, bytes. `None` = unknown, same rules as `cpu_usage`.
    #[serde(default)]
    pub memory_used: Option<u64>,
    /// Unix seconds of the node's last successful usage report.
    #[serde(default)]
    pub last_seen: Option<i64>,
}

fn default_node_priority() -> u32 {
    100
}

impl ClusterNode {
    /// Fraction (0.0-1.0) of RAM in use, when both figures are known.
    pub fn memory_used_fraction(&self) -> Option<f64> {
        match self.memory_used {
            Some(used) if self.memory_total > 0 => Some(used as f64 / self.memory_total as f64),
            _ => None,
        }
    }

    /// Combined load score in 0.0-1.0 (mean of CPU and RAM utilisation);
    /// `None` unless both live figures are known.
    pub fn load_score(&self) -> Option<f64> {
        let cpu = self.cpu_usage?.clamp(0.0, 100.0) / 100.0;
        let mem = self.memory_used_fraction()?.clamp(0.0, 1.0);
        Some((cpu + mem) / 2.0)
    }

    /// Human-readable live usage, e.g. "12.5% CPU, 3.2 / 16.0 GB RAM", with
    /// each unknown figure spelled out as unknown rather than 0.
    pub fn usage_label(&self) -> String {
        const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
        let cpu = match self.cpu_usage {
            Some(c) => format!("{:.1}% CPU", c),
            None => "CPU unknown".to_string(),
        };
        let mem = match self.memory_used {
            Some(used) if self.memory_total > 0 => format!(
                "{:.1} / {:.1} GB RAM",
                used as f64 / GIB,
                self.memory_total as f64 / GIB
            ),
            Some(used) => format!("{:.1} GB RAM used", used as f64 / GIB),
            None => "RAM unknown".to_string(),
        };
        format!("{}, {}", cpu, mem)
    }
}

/// Pick a migration target: the online node (other than `source_node`)
/// with the lowest combined CPU+RAM load, tie-broken by higher placement
/// weight. Only nodes reporting real live usage are eligible -- a node whose
/// usage is unknown is never auto-picked (the operator can still choose it
/// by hand). Returns `None` when no node qualifies.
pub fn pick_migration_target(nodes: &[ClusterNode], source_node: Option<&str>) -> Option<String> {
    nodes
        .iter()
        .filter(|n| n.status == "online")
        .filter(|n| source_node != Some(n.name.as_str()))
        .filter_map(|n| n.load_score().map(|score| (n, score)))
        .min_by(|(a, sa), (b, sb)| {
            sa.partial_cmp(sb)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.priority.cmp(&a.priority))
        })
        .map(|(n, _)| n.name.clone())
}

/// Get cluster nodes
pub async fn get_cluster_nodes() -> Result<Vec<ClusterNode>, ApiError> {
    fetch_json("/cluster/nodes").await
}

/// Set a node's placement/HA weight (0-1000).
pub async fn set_node_priority(name: &str, priority: u32) -> Result<(), ApiError> {
    #[derive(Serialize)]
    struct Body {
        priority: u32,
    }
    let url = resolve_api_url(&format!("/cluster/nodes/{}/priority", name));
    let response = with_auth(reqwasm::http::Request::patch(&url))
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(&Body { priority }).unwrap())
        .send()
        .await
        .map_err(|e| ApiError {
            message: e.to_string(),
        })?;

    if response.ok() {
        Ok(())
    } else {
        let body = response.text().await.unwrap_or_default();
        Err(ApiError {
            message: if body.is_empty() {
                format!("HTTP {}", response.status())
            } else {
                format!("HTTP {}: {}", response.status(), body)
            },
        })
    }
}

// Container Management APIs

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Container {
    pub id: String,
    #[serde(default)]
    pub vmid: String,
    pub name: String,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub node: String,
    pub runtime: String,
    /// The API's ContainerConfig carries the image name as `rootfs`.
    #[serde(alias = "rootfs")]
    pub image: String,
    pub status: String,
    #[serde(default)]
    pub cpus: u32,
    #[serde(default)]
    pub maxmem: u64,
    pub created_at: Option<String>,
}

/// Get all containers
pub async fn get_containers() -> Result<Vec<Container>, ApiError> {
    fetch_json("/containers").await
}

/// Body of `POST /api/containers` (horcrux_common::ContainerConfig).
#[derive(Clone, Debug, Default, Serialize)]
pub struct CreateContainerRequest {
    pub id: String,
    pub name: String,
    pub runtime: String,
    pub memory: u64,
    pub cpus: u32,
    /// Image (docker/podman) or rootfs/template (lxc/lxd/incus).
    pub rootfs: String,
    pub status: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub volumes: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
}

/// Create a container
pub async fn create_container(req: &CreateContainerRequest) -> Result<Container, ApiError> {
    post_json("/containers", req).await
}

/// Start a container
pub async fn start_container(container_id: &str) -> Result<(), ApiError> {
    post_json_no_response(
        &format!("/containers/{}/start", container_id),
        &serde_json::json!({}),
    )
    .await
}

/// Stop a container
pub async fn stop_container(container_id: &str) -> Result<(), ApiError> {
    post_json_no_response(
        &format!("/containers/{}/stop", container_id),
        &serde_json::json!({}),
    )
    .await
}

/// Delete a container
pub async fn delete_container(container_id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/containers/{}", container_id)).await
}

// Snapshot Management APIs

/// Get snapshots for a VM
pub async fn get_vm_snapshots(vm_id: &str) -> Result<Vec<VmSnapshot>, ApiError> {
    fetch_json(&format!("/vms/{}/snapshots", vm_id)).await
}

/// Create a snapshot
pub async fn create_snapshot(
    vm_id: &str,
    name: &str,
    description: Option<String>,
    include_memory: bool,
) -> Result<VmSnapshot, ApiError> {
    post_json(
        &format!("/vms/{}/snapshots", vm_id),
        &serde_json::json!({
            "name": name,
            "description": description,
            "include_memory": include_memory,
        }),
    )
    .await
}

/// Restore a snapshot
pub async fn restore_snapshot(vm_id: &str, snapshot_id: &str) -> Result<(), ApiError> {
    post_json_no_response(
        &format!("/vms/{}/snapshots/{}/restore", vm_id, snapshot_id),
        &serde_json::json!({}),
    )
    .await
}

/// Delete a snapshot
pub async fn delete_snapshot(vm_id: &str, snapshot_id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/vms/{}/snapshots/{}", vm_id, snapshot_id)).await
}

// Clone Management APIs

/// Clone job as shown on the Clones page (view of the API's
/// `vm::clone_progress::CloneJob`).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(from = "CloneJobWire")]
pub struct CloneJob {
    pub job_id: String,
    pub source_vm_id: String,
    pub target_vm_name: String,
    /// Current stage (e.g. "copying_disks"), or "-" when not started.
    pub clone_type: String,
    pub status: String,
    /// 0.0 - 1.0
    pub progress: f64,
    pub created_at: String,
}

#[derive(Deserialize)]
struct CloneJobWire {
    id: String,
    source_vm_id: String,
    target_vm_name: String,
    state: String,
    #[serde(default)]
    stage: Option<serde_json::Value>,
    progress: u8,
    created_at: String,
}

impl From<CloneJobWire> for CloneJob {
    fn from(w: CloneJobWire) -> Self {
        let stage = match w.stage {
            Some(serde_json::Value::String(s)) => s,
            Some(serde_json::Value::Object(m)) => m.keys().next().cloned().unwrap_or_default(),
            _ => String::new(),
        };
        Self {
            job_id: w.id,
            source_vm_id: w.source_vm_id,
            target_vm_name: w.target_vm_name,
            clone_type: if stage.is_empty() {
                "-".to_string()
            } else {
                stage
            },
            status: w.state,
            progress: f64::from(w.progress.min(100)) / 100.0,
            created_at: crate::utils::format_rfc3339(&w.created_at),
        }
    }
}

/// Get all clone jobs
pub async fn get_clone_jobs() -> Result<Vec<CloneJob>, ApiError> {
    fetch_json("/clone-jobs").await
}

// Replication Management APIs

/// Replication job as shown on the Replication page (view of the API's
/// `vm::replication::ReplicationJob`).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(from = "ReplicationJobWire")]
pub struct ReplicationJob {
    pub id: String,
    pub vm_id: String,
    pub source_node: String,
    pub target_node: String,
    pub schedule: String,
    pub enabled: bool,
    pub last_sync: Option<String>,
    pub status: String,
}

#[derive(Deserialize)]
struct ReplicationJobWire {
    id: String,
    source_vm_id: String,
    target_node: String,
    #[serde(default)]
    target_pool: String,
    schedule: serde_json::Value,
    enabled: bool,
    #[serde(default)]
    last_run: Option<i64>,
}

/// "hourly" | {"daily":{"hour":2}} | {"weekly":{"day":1,"hour":3}} | ...
pub fn describe_replication_schedule(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(m) => m
            .iter()
            .next()
            .map(|(kind, args)| match args.as_object() {
                Some(a) if !a.is_empty() => {
                    let mut parts: Vec<String> =
                        a.iter().map(|(k, v)| format!("{k}={v}")).collect();
                    parts.sort();
                    format!("{kind} ({})", parts.join(", "))
                }
                _ => kind.clone(),
            })
            .unwrap_or_default(),
        other => other.to_string(),
    }
}

impl From<ReplicationJobWire> for ReplicationJob {
    fn from(w: ReplicationJobWire) -> Self {
        Self {
            id: w.id,
            vm_id: w.source_vm_id,
            source_node: "local".to_string(),
            target_node: if w.target_pool.is_empty() {
                w.target_node
            } else {
                format!("{}:{}", w.target_node, w.target_pool)
            },
            schedule: describe_replication_schedule(&w.schedule),
            enabled: w.enabled,
            last_sync: w.last_run.map(crate::utils::format_unix_secs),
            status: if !w.enabled {
                "disabled".to_string()
            } else if w.last_run.is_some() {
                "ok".to_string()
            } else {
                "scheduled".to_string()
            },
        }
    }
}

/// Get all replication jobs
pub async fn get_replication_jobs() -> Result<Vec<ReplicationJob>, ApiError> {
    fetch_json("/replication/jobs").await
}

/// Execute a replication job
pub async fn execute_replication(job_id: &str) -> Result<(), ApiError> {
    post_json_no_response(
        &format!("/replication/jobs/{}/execute", job_id),
        &serde_json::json!({}),
    )
    .await
}

/// Body of POST /api/replication/jobs (the API's CreateReplicationJobRequest).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CreateReplicationRequest {
    pub source_vm_id: String,
    pub source_snapshot: String,
    pub target_node: String,
    pub target_pool: String,
    /// "hourly" | "daily" | "weekly" | "manual"
    pub schedule: String,
    pub retention_count: u32,
    pub enabled: bool,
}

/// Create a replication job
pub async fn create_replication(
    req: &CreateReplicationRequest,
) -> Result<ReplicationJob, ApiError> {
    post_json("/replication/jobs", req).await
}

/// Delete a replication job
pub async fn delete_replication(job_id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/replication/jobs/{}", job_id)).await
}

// =============================================================================
// Console APIs
// =============================================================================

/// `POST /api/console/:vm_id/{vnc,serial}` response (horcrux-api
/// `console::ConsoleInfo`). Only the one-time `ticket` matters to the
/// browser: it authenticates the console WebSocket.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConsoleInfo {
    pub vm_id: String,
    pub console_type: String,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: u16,
    pub ticket: String,
    #[serde(default)]
    pub ws_port: u16,
}

/// Server-rendered noVNC page for a VM. It creates its own console ticket
/// and derives ws/wss + host:port from wherever it is loaded, so it works
/// both embedded in an iframe and opened in its own tab.
pub fn novnc_page_url(vm_id: &str) -> String {
    format!("{}/console/{}/novnc", API_BASE, urlencoding::encode(vm_id))
}

/// Create a serial console session; returns the ticket for
/// [`serial_console_ws_path`].
pub async fn create_serial_console(vm_id: &str) -> Result<ConsoleInfo, ApiError> {
    post_json(
        &format!("/console/{}/serial", urlencoding::encode(vm_id)),
        &(),
    )
    .await
}

/// Path of the serial-console WebSocket for a ticket (pass it through
/// [`resolve_ws_url`]).
pub fn serial_console_ws_path(ticket: &str) -> String {
    format!("{}/console/ws/serial/{}", API_BASE, ticket)
}

/// Extended VM information for backup and template operations. The API
/// returns `horcrux_common::VmConfig` (`status`, no `state`/`vmid`/`node`),
/// so this is built from that shape instead of failing to deserialize
/// ("missing field `state`" broke the HA groups and migration pages).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(from = "VmInfoWire")]
pub struct VmInfo {
    pub id: String,
    pub vmid: String,
    pub name: String,
    pub state: String,
    pub status: String,
    pub node: String,
    pub cpus: u32,
    pub maxcpu: u32,
    pub memory: u64,
    pub maxmem: u64,
    pub disk_size: u64,
}

#[derive(Deserialize)]
struct VmInfoWire {
    id: String,
    name: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    node: Option<String>,
    #[serde(default)]
    cpus: u32,
    /// MB
    #[serde(default)]
    memory: u64,
    #[serde(default)]
    disk_size: u64,
}

impl From<VmInfoWire> for VmInfo {
    fn from(w: VmInfoWire) -> Self {
        Self {
            vmid: w.id.clone(),
            id: w.id,
            name: w.name,
            state: w.status.clone(),
            status: w.status,
            node: w.node.unwrap_or_else(|| "local".to_string()),
            cpus: w.cpus,
            maxcpu: w.cpus,
            memory: w.memory,
            maxmem: w.memory * 1024 * 1024,
            disk_size: w.disk_size,
        }
    }
}

/// Virtual machine representation for HA operations
pub type VirtualMachine = VmInfo;

/// Get virtual machines
pub async fn get_virtual_machines() -> Result<Vec<VirtualMachine>, ApiError> {
    fetch_json("/vms").await
}

// =============================================================================
// Metrics Types
// =============================================================================

/// Query for metrics
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetricQuery {
    pub metric: String,
    #[serde(default)]
    pub labels: std::collections::HashMap<String, String>,
    pub start_time: Option<chrono::DateTime<chrono::Utc>>,
    pub end_time: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default = "default_step")]
    pub step: u64,
}

fn default_step() -> u64 {
    60
}

/// A single data point in a metric result
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetricDataPoint {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub value: f64,
    #[serde(default)]
    pub labels: std::collections::HashMap<String, String>,
}

/// Result from a metrics query
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetricResult {
    pub metric: String,
    #[serde(default)]
    pub labels: std::collections::HashMap<String, String>,
    pub values: Vec<MetricDataPoint>,
}

/// Component for building PromQL queries
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum QueryComponent {
    Metric {
        name: String,
        labels: std::collections::HashMap<String, String>,
    },
    Function {
        name: String,
        args: Vec<String>,
        modifiers: Vec<String>,
    },
    Operator {
        operator: String,
    },
}

/// PromQL function definition
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PromQLFunction {
    pub name: String,
    pub description: String,
    pub syntax: String,
    pub category: String,
    pub example: String,
}

/// Query validation result
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueryValidationResult {
    pub valid: bool,
    #[serde(default)]
    pub message: String,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    #[serde(default)]
    pub suggestions: Vec<String>,
}

// =============================================================================
// Authentication & User Management APIs
// =============================================================================

/// User information from API
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
    pub email: String,
    pub role: String,
    pub realm: String,
    pub enabled: bool,
    pub roles: Vec<String>,
    pub comment: Option<String>,
    pub last_login: Option<String>,
}

/// Role definition
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Role {
    pub name: String,
    pub description: String,
    pub permissions: Vec<Permission>,
}

/// Permission for a resource path
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Permission {
    pub path: String,
    pub privileges: Vec<String>,
}

/// API Token information
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiToken {
    pub id: String,
    pub user: String,
    pub enabled: bool,
    pub expire: Option<i64>,
    pub comment: Option<String>,
    pub created_at: Option<String>,
    pub last_used: Option<String>,
}

/// User session information
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserSession {
    pub session_id: String,
    pub username: String,
    pub realm: String,
    pub created: i64,
    pub expires: i64,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
}

/// Login request
#[derive(Clone, Debug, Serialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    pub realm: Option<String>,
}

/// Login response
#[derive(Clone, Debug, Deserialize)]
pub struct LoginResponse {
    pub ticket: String,
    pub csrf_token: String,
    pub username: String,
    pub roles: Vec<String>,
}

/// Create user request
#[derive(Clone, Debug, Serialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub password: String,
    pub email: String,
    pub role: String,
    pub realm: String,
    pub enabled: bool,
    pub comment: Option<String>,
}

/// Update user request
#[derive(Clone, Debug, Serialize)]
pub struct UpdateUserRequest {
    pub email: Option<String>,
    pub role: Option<String>,
    pub enabled: Option<bool>,
    pub comment: Option<String>,
}

/// Change password request
#[derive(Clone, Debug, Serialize)]
pub struct ChangePasswordRequest {
    pub old_password: String,
    pub new_password: String,
}

/// Create API token request
#[derive(Clone, Debug, Serialize)]
pub struct CreateApiTokenRequest {
    pub comment: Option<String>,
    pub expire: Option<i64>,
    pub permissions: Option<Vec<String>>,
}

// ============================================================================
// Kubernetes Workload Data Structures
// ============================================================================

/// Human "age" (5m, 3h, 2d) of a Kubernetes object from its RFC 3339
/// creation time, as `kubectl get` shows it.
pub fn k8s_age(created_at: Option<&str>) -> String {
    let Some(t) = created_at.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()) else {
        return "-".to_string();
    };
    let secs = (chrono::Utc::now() - t.with_timezone(&chrono::Utc))
        .num_seconds()
        .max(0);
    match secs {
        s if s < 120 => format!("{}s", s),
        s if s < 7200 => format!("{}m", s / 60),
        s if s < 172_800 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

/// Kubernetes pod (view of the API's `kubernetes::types::PodInfo`).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(from = "KubernetesPodWire")]
pub struct KubernetesPod {
    pub name: String,
    pub namespace: String,
    pub status: PodStatus,
    pub ready: String,
    pub restarts: u32,
    pub age: String,
    pub node: Option<String>,
    pub containers: Vec<PodContainer>,
    pub labels: std::collections::HashMap<String, String>,
    pub annotations: std::collections::HashMap<String, String>,
}

/// One container of a pod.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PodContainer {
    pub name: String,
    #[serde(default)]
    pub image: String,
    #[serde(default)]
    pub ready: bool,
    #[serde(default)]
    pub restart_count: i32,
}

#[derive(Deserialize)]
struct KubernetesPodWire {
    name: String,
    namespace: String,
    status: String,
    #[serde(default)]
    node_name: Option<String>,
    #[serde(default)]
    containers: Vec<PodContainer>,
    #[serde(default)]
    labels: std::collections::HashMap<String, String>,
    #[serde(default)]
    annotations: std::collections::HashMap<String, String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    restart_count: i32,
}

impl From<KubernetesPodWire> for KubernetesPod {
    fn from(w: KubernetesPodWire) -> Self {
        let mut phase = w.status;
        if let Some(first) = phase.get(..1) {
            phase = first.to_uppercase() + &phase[1..];
        }
        let ready = w.containers.iter().filter(|c| c.ready).count();
        Self {
            ready: format!("{}/{}", ready, w.containers.len()),
            restarts: w.restart_count.max(0) as u32,
            age: k8s_age(w.created_at.as_deref()),
            name: w.name,
            namespace: w.namespace,
            status: PodStatus {
                phase,
                reason: None,
                message: None,
            },
            node: w.node_name,
            containers: w.containers,
            labels: w.labels,
            annotations: w.annotations,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PodStatus {
    pub phase: String,
    pub reason: Option<String>,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContainerStatus {
    pub name: String,
    pub ready: bool,
    pub restart_count: u32,
    pub state: ContainerState,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContainerState {
    pub running: Option<ContainerStateRunning>,
    pub waiting: Option<ContainerStateWaiting>,
    pub terminated: Option<ContainerStateTerminated>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContainerStateRunning {
    pub started_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContainerStateWaiting {
    pub reason: Option<String>,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContainerStateTerminated {
    pub exit_code: i32,
    pub reason: Option<String>,
    pub message: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

/// Kubernetes deployment (view of the API's `DeploymentInfo`).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(from = "KubernetesDeploymentWire")]
pub struct KubernetesDeployment {
    pub name: String,
    pub namespace: String,
    pub replicas: DeploymentReplicas,
    pub strategy: String,
    pub age: String,
    pub labels: std::collections::HashMap<String, String>,
    pub annotations: std::collections::HashMap<String, String>,
    pub conditions: Vec<DeploymentCondition>,
}

#[derive(Deserialize)]
struct KubernetesDeploymentWire {
    name: String,
    namespace: String,
    #[serde(default)]
    replicas: i32,
    #[serde(default)]
    ready_replicas: i32,
    #[serde(default)]
    available_replicas: i32,
    #[serde(default)]
    updated_replicas: i32,
    #[serde(default)]
    labels: std::collections::HashMap<String, String>,
    #[serde(default)]
    strategy: String,
    #[serde(default)]
    created_at: Option<String>,
}

impl From<KubernetesDeploymentWire> for KubernetesDeployment {
    fn from(w: KubernetesDeploymentWire) -> Self {
        let n = |v: i32| v.max(0) as u32;
        let unavailable = n(w.replicas).saturating_sub(n(w.available_replicas));
        Self {
            replicas: DeploymentReplicas {
                desired: n(w.replicas),
                current: n(w.updated_replicas),
                ready: n(w.ready_replicas),
                available: n(w.available_replicas),
                unavailable: (unavailable > 0).then_some(unavailable),
            },
            age: k8s_age(w.created_at.as_deref()),
            name: w.name,
            namespace: w.namespace,
            strategy: w.strategy,
            labels: w.labels,
            annotations: Default::default(),
            conditions: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeploymentReplicas {
    pub desired: u32,
    pub current: u32,
    pub ready: u32,
    pub available: u32,
    pub unavailable: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeploymentCondition {
    pub condition_type: String,
    pub status: String,
    pub reason: Option<String>,
    pub message: Option<String>,
    pub last_update_time: String,
}

/// Kubernetes service (view of the API's `ServiceInfo`).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(from = "KubernetesServiceWire")]
pub struct KubernetesService {
    pub name: String,
    pub namespace: String,
    pub service_type: String,
    pub cluster_ip: String,
    pub external_ips: Vec<String>,
    pub ports: Vec<ServicePort>,
    pub age: String,
    pub selector: std::collections::HashMap<String, String>,
    pub labels: std::collections::HashMap<String, String>,
}

#[derive(Deserialize)]
struct KubernetesServiceWire {
    name: String,
    namespace: String,
    service_type: String,
    #[serde(default)]
    cluster_ip: Option<String>,
    #[serde(default)]
    external_ip: Option<String>,
    #[serde(default)]
    ports: Vec<ServicePort>,
    #[serde(default)]
    selector: std::collections::HashMap<String, String>,
    #[serde(default)]
    labels: std::collections::HashMap<String, String>,
    #[serde(default)]
    created_at: Option<String>,
}

impl From<KubernetesServiceWire> for KubernetesService {
    fn from(w: KubernetesServiceWire) -> Self {
        Self {
            age: k8s_age(w.created_at.as_deref()),
            name: w.name,
            namespace: w.namespace,
            service_type: w.service_type,
            cluster_ip: w.cluster_ip.unwrap_or_else(|| "None".to_string()),
            external_ips: w.external_ip.into_iter().collect(),
            ports: w.ports,
            selector: w.selector,
            labels: w.labels,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServicePort {
    pub name: Option<String>,
    pub protocol: String,
    pub port: u16,
    #[serde(default)]
    pub target_port: Option<String>,
    #[serde(default)]
    pub node_port: Option<u16>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateServiceRequest {
    pub name: String,
    pub service_type: String,
    pub ports: Vec<ServicePort>,
    pub selector: std::collections::HashMap<String, String>,
    pub labels: Option<std::collections::HashMap<String, String>>,
}

/// Kubernetes ingress (view of the API's `IngressInfo`).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(from = "KubernetesIngressWire")]
pub struct KubernetesIngress {
    pub name: String,
    pub namespace: String,
    pub class: Option<String>,
    pub hosts: Vec<String>,
    pub age: String,
    pub rules: Vec<IngressRule>,
    pub tls: Vec<IngressTLS>,
    pub labels: std::collections::HashMap<String, String>,
}

#[derive(Deserialize)]
struct KubernetesIngressWire {
    name: String,
    namespace: String,
    #[serde(default)]
    ingress_class: Option<String>,
    #[serde(default)]
    rules: Vec<IngressRule>,
    #[serde(default)]
    tls: Vec<IngressTLS>,
    #[serde(default)]
    labels: std::collections::HashMap<String, String>,
    #[serde(default)]
    created_at: Option<String>,
}

impl From<KubernetesIngressWire> for KubernetesIngress {
    fn from(w: KubernetesIngressWire) -> Self {
        let mut hosts: Vec<String> = w.rules.iter().filter_map(|r| r.host.clone()).collect();
        hosts.dedup();
        Self {
            age: k8s_age(w.created_at.as_deref()),
            name: w.name,
            namespace: w.namespace,
            class: w.ingress_class,
            hosts,
            rules: w.rules,
            tls: w.tls,
            labels: w.labels,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IngressRule {
    pub host: Option<String>,
    pub paths: Vec<IngressPath>,
}

/// Ingress path. Sent as-is on create; read from the API's
/// `{path, path_type, backend_service, backend_port}`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IngressPath {
    pub path: String,
    pub path_type: String,
    #[serde(alias = "backend_service")]
    pub service_name: String,
    #[serde(
        alias = "backend_port",
        deserialize_with = "port_from_string_or_number"
    )]
    pub service_port: u16,
}

fn port_from_string_or_number<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u16, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::Number(n) => n.as_u64().unwrap_or(0) as u16,
        serde_json::Value::String(s) => s.parse().unwrap_or(0),
        _ => 0,
    })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IngressTLS {
    pub hosts: Vec<String>,
    #[serde(default, deserialize_with = "string_or_null")]
    pub secret_name: String,
}

fn string_or_null<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(d)?.unwrap_or_default())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateIngressRequest {
    pub name: String,
    pub ingress_class: Option<String>,
    pub rules: Vec<IngressRule>,
    pub tls: Option<Vec<IngressTLS>>,
    pub labels: Option<std::collections::HashMap<String, String>>,
    pub annotations: Option<std::collections::HashMap<String, String>>,
}

/// Get all users
pub async fn get_users() -> Result<Vec<User>, ApiError> {
    fetch_json("/users").await
}

/// Get a specific user by ID
pub async fn get_user(user_id: &str) -> Result<User, ApiError> {
    fetch_json(&format!("/users/{}", user_id)).await
}

/// Create a new user
pub async fn create_user(request: CreateUserRequest) -> Result<User, ApiError> {
    post_json("/users", &request).await
}

/// Update a user
pub async fn update_user(user_id: &str, request: UpdateUserRequest) -> Result<User, ApiError> {
    post_json(&format!("/users/{}", user_id), &request).await
}

/// Delete a user
pub async fn delete_user(user_id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/users/{}", user_id)).await
}

/// Enable/disable a user
pub async fn toggle_user(user_id: &str, enabled: bool) -> Result<(), ApiError> {
    post_json(
        &format!("/users/{}/toggle", user_id),
        &serde_json::json!({"enabled": enabled}),
    )
    .await
}

/// Get all roles
pub async fn get_roles() -> Result<Vec<Role>, ApiError> {
    fetch_json("/roles").await
}

/// Get a specific role
pub async fn get_role(role_name: &str) -> Result<Role, ApiError> {
    fetch_json(&format!("/roles/{}", role_name)).await
}

/// Create a new role
pub async fn create_role(role: Role) -> Result<Role, ApiError> {
    post_json("/roles", &role).await
}

/// Update a role
pub async fn update_role(role_name: &str, role: Role) -> Result<Role, ApiError> {
    post_json(&format!("/roles/{}", role_name), &role).await
}

/// Delete a role
pub async fn delete_role(role_name: &str) -> Result<(), ApiError> {
    delete_json(&format!("/roles/{}", role_name)).await
}

/// Get permissions for a user
pub async fn get_user_permissions(user_id: &str) -> Result<Vec<Permission>, ApiError> {
    fetch_json(&format!("/users/{}/permissions", user_id)).await
}

/// Add permission to a user
pub async fn add_user_permission(user_id: &str, permission: Permission) -> Result<(), ApiError> {
    post_json(&format!("/users/{}/permissions", user_id), &permission).await
}

/// Remove permission from a user
pub async fn remove_user_permission(user_id: &str, path: &str) -> Result<(), ApiError> {
    delete_json(&format!("/users/{}/permissions?path={}", user_id, path)).await
}

/// Get all active sessions
pub async fn get_active_sessions() -> Result<Vec<UserSession>, ApiError> {
    fetch_json("/auth/sessions").await
}

/// Terminate a user session
pub async fn terminate_session(session_id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/auth/sessions/{}", session_id)).await
}

/// Terminate all sessions for a user
pub async fn terminate_user_sessions(user_id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/users/{}/sessions", user_id)).await
}

/// Get API tokens for a user
pub async fn get_user_api_tokens(user_id: &str) -> Result<Vec<ApiToken>, ApiError> {
    fetch_json(&format!("/users/{}/api-keys", user_id)).await
}

/// Create a new API token for a user
pub async fn create_api_token(
    user_id: &str,
    request: CreateApiTokenRequest,
) -> Result<ApiToken, ApiError> {
    post_json(&format!("/users/{}/api-keys", user_id), &request).await
}

/// Update an API token
pub async fn update_api_token(
    user_id: &str,
    token_id: &str,
    request: CreateApiTokenRequest,
) -> Result<ApiToken, ApiError> {
    post_json(
        &format!("/users/{}/api-keys/{}", user_id, token_id),
        &request,
    )
    .await
}

/// Delete an API token
pub async fn delete_api_token(user_id: &str, token_id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/users/{}/api-keys/{}", user_id, token_id)).await
}

/// Toggle an API token (enable/disable)
pub async fn toggle_api_token(
    user_id: &str,
    token_id: &str,
    enabled: bool,
) -> Result<(), ApiError> {
    post_json(
        &format!("/users/{}/api-keys/{}/toggle", user_id, token_id),
        &serde_json::json!({"enabled": enabled}),
    )
    .await
}

/// Change user password
pub async fn change_password(
    user_id: &str,
    request: ChangePasswordRequest,
) -> Result<(), ApiError> {
    post_json(&format!("/users/{}/password", user_id), &request).await
}

/// Login user
pub async fn login(request: LoginRequest) -> Result<LoginResponse, ApiError> {
    let response: LoginResponse = post_json("/auth/login", &request).await?;
    set_auth_token(&response.ticket);
    Ok(response)
}

/// Logout current user
pub async fn logout() -> Result<(), ApiError> {
    let result = post_empty("/auth/logout").await;
    clear_auth_token();
    result
}

/// Verify current session
pub async fn verify_session() -> Result<bool, ApiError> {
    match fetch_json::<serde_json::Value>("/auth/verify").await {
        Ok(v) => Ok(v["valid"].as_bool().unwrap_or(false)),
        Err(_) => Ok(false),
    }
}

// ============================================================================
// Kubernetes Workload Management APIs
// ============================================================================

/// Get pods in a namespace
pub async fn get_pods(cluster_id: &str, namespace: &str) -> Result<Vec<KubernetesPod>, ApiError> {
    fetch_json(&format!(
        "/k8s/clusters/{}/namespaces/{}/pods",
        cluster_id, namespace
    ))
    .await
}

/// Get pod details
pub async fn get_pod(
    cluster_id: &str,
    namespace: &str,
    pod_name: &str,
) -> Result<KubernetesPod, ApiError> {
    fetch_json(&format!(
        "/k8s/clusters/{}/namespaces/{}/pods/{}",
        cluster_id, namespace, pod_name
    ))
    .await
}

/// Delete a pod
pub async fn delete_pod(cluster_id: &str, namespace: &str, pod_name: &str) -> Result<(), ApiError> {
    delete_json(&format!(
        "/k8s/clusters/{}/namespaces/{}/pods/{}",
        cluster_id, namespace, pod_name
    ))
    .await
}

/// Get pod logs
pub async fn get_pod_logs(
    cluster_id: &str,
    namespace: &str,
    pod_name: &str,
    container: Option<&str>,
) -> Result<String, ApiError> {
    let mut url = format!(
        "/k8s/clusters/{}/namespaces/{}/pods/{}/logs",
        cluster_id, namespace, pod_name
    );
    if let Some(container) = container {
        url = format!("{}?container={}", url, container);
    }
    fetch_text(&url).await
}

/// Get deployments in a namespace
pub async fn get_deployments(
    cluster_id: &str,
    namespace: &str,
) -> Result<Vec<KubernetesDeployment>, ApiError> {
    fetch_json(&format!(
        "/k8s/clusters/{}/namespaces/{}/deployments",
        cluster_id, namespace
    ))
    .await
}

/// Get deployment details
pub async fn get_deployment(
    cluster_id: &str,
    namespace: &str,
    deployment_name: &str,
) -> Result<KubernetesDeployment, ApiError> {
    fetch_json(&format!(
        "/k8s/clusters/{}/namespaces/{}/deployments/{}",
        cluster_id, namespace, deployment_name
    ))
    .await
}

/// Scale deployment
pub async fn scale_deployment(
    cluster_id: &str,
    namespace: &str,
    deployment_name: &str,
    replicas: u32,
) -> Result<(), ApiError> {
    post_json(
        &format!(
            "/k8s/clusters/{}/namespaces/{}/deployments/{}/scale",
            cluster_id, namespace, deployment_name
        ),
        &serde_json::json!({"replicas": replicas}),
    )
    .await
}

/// Restart deployment
pub async fn restart_deployment(
    cluster_id: &str,
    namespace: &str,
    deployment_name: &str,
) -> Result<(), ApiError> {
    post_empty(&format!(
        "/k8s/clusters/{}/namespaces/{}/deployments/{}/restart",
        cluster_id, namespace, deployment_name
    ))
    .await
}

/// Delete deployment
pub async fn delete_deployment(
    cluster_id: &str,
    namespace: &str,
    deployment_name: &str,
) -> Result<(), ApiError> {
    delete_json(&format!(
        "/k8s/clusters/{}/namespaces/{}/deployments/{}",
        cluster_id, namespace, deployment_name
    ))
    .await
}

/// Get services in a namespace
pub async fn get_services(
    cluster_id: &str,
    namespace: &str,
) -> Result<Vec<KubernetesService>, ApiError> {
    fetch_json(&format!(
        "/k8s/clusters/{}/namespaces/{}/services",
        cluster_id, namespace
    ))
    .await
}

/// Get service details
pub async fn get_service(
    cluster_id: &str,
    namespace: &str,
    service_name: &str,
) -> Result<KubernetesService, ApiError> {
    fetch_json(&format!(
        "/k8s/clusters/{}/namespaces/{}/services/{}",
        cluster_id, namespace, service_name
    ))
    .await
}

/// Create service
pub async fn create_service(
    cluster_id: &str,
    namespace: &str,
    service: CreateServiceRequest,
) -> Result<KubernetesService, ApiError> {
    post_json(
        &format!(
            "/k8s/clusters/{}/namespaces/{}/services",
            cluster_id, namespace
        ),
        &service,
    )
    .await
}

/// Delete service
pub async fn delete_service(
    cluster_id: &str,
    namespace: &str,
    service_name: &str,
) -> Result<(), ApiError> {
    delete_json(&format!(
        "/k8s/clusters/{}/namespaces/{}/services/{}",
        cluster_id, namespace, service_name
    ))
    .await
}

/// Get ingresses in a namespace
pub async fn get_ingresses(
    cluster_id: &str,
    namespace: &str,
) -> Result<Vec<KubernetesIngress>, ApiError> {
    fetch_json(&format!(
        "/k8s/clusters/{}/namespaces/{}/ingresses",
        cluster_id, namespace
    ))
    .await
}

/// Get ingress details
pub async fn get_ingress(
    cluster_id: &str,
    namespace: &str,
    ingress_name: &str,
) -> Result<KubernetesIngress, ApiError> {
    fetch_json(&format!(
        "/k8s/clusters/{}/namespaces/{}/ingresses/{}",
        cluster_id, namespace, ingress_name
    ))
    .await
}

/// Create ingress
pub async fn create_ingress(
    cluster_id: &str,
    namespace: &str,
    ingress: CreateIngressRequest,
) -> Result<KubernetesIngress, ApiError> {
    post_json(
        &format!(
            "/k8s/clusters/{}/namespaces/{}/ingresses",
            cluster_id, namespace
        ),
        &ingress,
    )
    .await
}

/// Delete ingress
pub async fn delete_ingress(
    cluster_id: &str,
    namespace: &str,
    ingress_name: &str,
) -> Result<(), ApiError> {
    delete_json(&format!(
        "/k8s/clusters/{}/namespaces/{}/ingresses/{}",
        cluster_id, namespace, ingress_name
    ))
    .await
}

// ============================================================================
// HA Management Data Structures
// ============================================================================

/// HA-managed resource (view of the API's `ha::HaResource`).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(from = "HaResourceWire")]
pub struct HaResource {
    pub resource_id: String,
    pub resource_type: String, // vm, container
    pub priority: u32,
    pub state: String, // started, stopped, migrating, error, disabled
    #[serde(default)]
    pub enabled: bool,
    pub group: String,
    pub current_node: Option<String>,
}

#[derive(Deserialize)]
struct HaResourceWire {
    vm_id: String,
    #[serde(default)]
    group: String,
    state: String,
    #[serde(default)]
    current_node: Option<String>,
    #[serde(default)]
    max_restart: u32,
}

impl From<HaResourceWire> for HaResource {
    fn from(w: HaResourceWire) -> Self {
        let state = w.state.to_lowercase();
        Self {
            resource_id: w.vm_id,
            resource_type: "vm".to_string(),
            priority: w.max_restart,
            enabled: state != "disabled",
            state,
            group: w.group,
            current_node: w.current_node,
        }
    }
}

/// HA group (view of the API's `ha::HaGroup`, which is keyed by name).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(from = "HaGroupWire")]
pub struct HaGroup {
    pub id: String,
    pub name: String,
    pub priority: u32,
    pub max_restart: u32,
    pub max_relocate: u32,
    pub enabled: bool,
    pub comment: Option<String>,
    pub resources: Vec<HaResource>,
    #[serde(default)]
    pub vm_ids: Vec<String>,
    pub nodes: Vec<String>,
    pub state: String, // active, inactive, maintenance
    pub restricted: bool,
    pub nofailback: bool,
}

#[derive(Deserialize)]
struct HaGroupWire {
    name: String,
    #[serde(default)]
    nodes: Vec<String>,
    #[serde(default)]
    restricted: bool,
    #[serde(default)]
    no_failback: bool,
}

impl From<HaGroupWire> for HaGroup {
    fn from(w: HaGroupWire) -> Self {
        Self {
            id: w.name.clone(),
            name: w.name,
            priority: 0,
            max_restart: 0,
            max_relocate: 0,
            enabled: true,
            comment: None,
            resources: Vec::new(),
            vm_ids: Vec::new(),
            nodes: w.nodes,
            state: "active".to_string(),
            restricted: w.restricted,
            nofailback: w.no_failback,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HaResourceAssignment {
    pub group_id: String,
    pub resource_id: String,
    pub resource_type: String,
    pub priority: u32,
    pub state: String,
}

// ============================================================================
// Migration Management Data Structures
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MigrationJob {
    pub job_id: String,
    pub resource_type: String, // vm, container
    pub resource_id: String,
    pub source_node: String,
    pub target_node: String,
    pub migration_type: String, // online, offline
    pub status: String,         // pending, running, completed, failed, cancelled
    pub progress: u32,          // 0-100
    pub start_time: chrono::DateTime<chrono::Utc>,
    pub end_time: Option<chrono::DateTime<chrono::Utc>>,
    pub error: Option<String>,
    pub bandwidth_limit: Option<u64>,
    pub timeout: u32,
    pub force: bool,
    pub with_local_disks: bool,
    pub estimated_duration: Option<u32>, // minutes
    pub transferred_bytes: u64,
    pub remaining_bytes: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BulkMigrationJob {
    pub job_id: String,
    pub target_node: String,
    pub resources: Vec<(String, String)>, // (type, id) pairs
    pub max_workers: u32,
    pub migration_type: String,
    pub status: String,
    pub start_time: chrono::DateTime<chrono::Utc>,
    pub end_time: Option<chrono::DateTime<chrono::Utc>>,
    pub completed_count: usize,
    pub failed_count: usize,
    pub total_count: usize,
}

// ============================================================================
// Helm Management Data Structures
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HelmRepository {
    pub name: String,
    pub url: String,
    pub description: Option<String>,
    /// Set for repositories added through Horcrux (helm records no date).
    #[serde(default)]
    pub added_at: Option<String>,
    pub status: String,
    pub last_update: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AddHelmRepoRequest {
    pub name: String,
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub force_update: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HelmChart {
    pub name: String,
    pub version: String,
    pub app_version: Option<String>,
    pub description: String,
    pub repository: String,
    pub icon: Option<String>,
    pub keywords: Vec<String>,
    pub maintainers: Vec<HelmMaintainer>,
    pub created: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HelmMaintainer {
    pub name: String,
    pub email: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HelmRelease {
    pub name: String,
    pub namespace: String,
    pub revision: u32,
    pub status: String,
    pub chart: String,
    pub app_version: Option<String>,
    pub updated: String,
    pub description: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HelmInstallRequest {
    pub name: String,
    pub chart: String,
    pub version: Option<String>,
    pub values: Option<serde_json::Value>,
    pub create_namespace: Option<bool>,
    pub wait: Option<bool>,
    pub timeout: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HelmValues {
    pub values: serde_json::Value,
    pub computed_values: serde_json::Value,
    pub user_supplied_values: serde_json::Value,
}

/// Kubernetes ConfigMap (view of the API's `ConfigMapInfo`).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(from = "KubernetesConfigMapWire")]
pub struct KubernetesConfigMap {
    pub name: String,
    pub namespace: String,
    pub data: std::collections::HashMap<String, String>,
    pub binary_data: Option<std::collections::HashMap<String, String>>,
    pub labels: std::collections::HashMap<String, String>,
    pub annotations: std::collections::HashMap<String, String>,
    pub age: String,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Deserialize)]
struct KubernetesConfigMapWire {
    name: String,
    namespace: String,
    #[serde(default)]
    data: std::collections::HashMap<String, String>,
    #[serde(default)]
    binary_data_keys: Vec<String>,
    #[serde(default)]
    labels: std::collections::HashMap<String, String>,
    #[serde(default)]
    annotations: std::collections::HashMap<String, String>,
    #[serde(default)]
    created_at: Option<String>,
}

impl From<KubernetesConfigMapWire> for KubernetesConfigMap {
    fn from(w: KubernetesConfigMapWire) -> Self {
        Self {
            age: k8s_age(w.created_at.as_deref()),
            created_at: w
                .created_at
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|t| t.with_timezone(&chrono::Utc)),
            binary_data: (!w.binary_data_keys.is_empty()).then(|| {
                w.binary_data_keys
                    .into_iter()
                    .map(|k| (k, String::new()))
                    .collect()
            }),
            name: w.name,
            namespace: w.namespace,
            data: w.data,
            labels: w.labels,
            annotations: w.annotations,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateConfigMapRequest {
    pub name: String,
    pub data: std::collections::HashMap<String, String>,
    pub binary_data: Option<std::collections::HashMap<String, String>>,
    pub labels: Option<std::collections::HashMap<String, String>>,
    pub annotations: Option<std::collections::HashMap<String, String>>,
}

/// Kubernetes Secret (view of the API's `SecretInfo`). The API never sends
/// secret values, only the key names: `data` maps each key to "".
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(from = "KubernetesSecretWire")]
pub struct KubernetesSecret {
    pub name: String,
    pub namespace: String,
    pub secret_type: Option<String>,
    pub data: std::collections::HashMap<String, String>,
    pub labels: Option<std::collections::HashMap<String, String>>,
    pub annotations: Option<std::collections::HashMap<String, String>>,
    pub age: String,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Deserialize)]
struct KubernetesSecretWire {
    name: String,
    namespace: String,
    #[serde(default)]
    secret_type: Option<String>,
    #[serde(default)]
    data_keys: Vec<String>,
    #[serde(default)]
    labels: Option<std::collections::HashMap<String, String>>,
    #[serde(default)]
    annotations: Option<std::collections::HashMap<String, String>>,
    #[serde(default)]
    created_at: Option<String>,
}

impl From<KubernetesSecretWire> for KubernetesSecret {
    fn from(w: KubernetesSecretWire) -> Self {
        Self {
            age: k8s_age(w.created_at.as_deref()),
            created_at: w
                .created_at
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|t| t.with_timezone(&chrono::Utc)),
            data: w
                .data_keys
                .into_iter()
                .map(|k| (k, String::new()))
                .collect(),
            name: w.name,
            namespace: w.namespace,
            secret_type: w.secret_type,
            labels: w.labels,
            annotations: w.annotations,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateSecretRequest {
    pub name: String,
    pub secret_type: String,
    pub data: std::collections::HashMap<String, String>,
    pub string_data: Option<std::collections::HashMap<String, String>>,
    pub labels: Option<std::collections::HashMap<String, String>>,
    pub annotations: Option<std::collections::HashMap<String, String>>,
}

// ============================================================================
// Helm Management APIs
// ============================================================================

/// Get Helm repositories
pub async fn get_helm_repositories() -> Result<Vec<HelmRepository>, ApiError> {
    fetch_json("/k8s/helm/repos").await
}

/// Add Helm repository
pub async fn add_helm_repository(request: AddHelmRepoRequest) -> Result<HelmRepository, ApiError> {
    post_json("/k8s/helm/repos", &request).await
}

/// Update Helm repository
pub async fn update_helm_repository(repo_name: &str) -> Result<(), ApiError> {
    post_empty(&format!("/k8s/helm/repos/{}/update", repo_name)).await
}

/// Remove Helm repository
pub async fn remove_helm_repository(repo_name: &str) -> Result<(), ApiError> {
    delete_json(&format!("/k8s/helm/repos/{}", repo_name)).await
}

/// Search Helm charts
pub async fn search_helm_charts(
    query: &str,
    repo: Option<&str>,
) -> Result<Vec<HelmChart>, ApiError> {
    let mut url = format!("/k8s/helm/charts?query={}", query);
    if let Some(repo) = repo {
        url = format!("{}&repo={}", url, repo);
    }
    fetch_json(&url).await
}

/// Get chart details
pub async fn get_helm_chart(repo: &str, chart: &str) -> Result<HelmChart, ApiError> {
    fetch_json(&format!("/k8s/helm/charts/{}/{}", repo, chart)).await
}

/// Get chart versions
pub async fn get_helm_chart_versions(repo: &str, chart: &str) -> Result<Vec<String>, ApiError> {
    fetch_json(&format!("/k8s/helm/charts/{}/{}/versions", repo, chart)).await
}

/// Get chart values
pub async fn get_helm_chart_values(
    repo: &str,
    chart: &str,
    version: Option<&str>,
) -> Result<serde_json::Value, ApiError> {
    let mut url = format!("/k8s/helm/charts/{}/{}/values", repo, chart);
    if let Some(version) = version {
        url = format!("{}?version={}", url, version);
    }
    fetch_json(&url).await
}

/// Get Helm releases
pub async fn get_helm_releases(
    cluster_id: &str,
    namespace: Option<&str>,
) -> Result<Vec<HelmRelease>, ApiError> {
    let mut url = format!("/k8s/clusters/{}/helm/releases", cluster_id);
    if let Some(namespace) = namespace {
        url = format!("{}?namespace={}", url, namespace);
    }
    fetch_json(&url).await
}

/// Get Helm release details
pub async fn get_helm_release(
    cluster_id: &str,
    namespace: &str,
    release_name: &str,
) -> Result<HelmRelease, ApiError> {
    fetch_json(&format!(
        "/k8s/clusters/{}/helm/releases/{}/{}",
        cluster_id, namespace, release_name
    ))
    .await
}

/// Install Helm chart
pub async fn install_helm_chart(
    cluster_id: &str,
    namespace: &str,
    request: HelmInstallRequest,
) -> Result<HelmRelease, ApiError> {
    post_json(
        &format!(
            "/k8s/clusters/{}/namespaces/{}/helm/install",
            cluster_id, namespace
        ),
        &request,
    )
    .await
}

/// Upgrade Helm release
pub async fn upgrade_helm_release(
    cluster_id: &str,
    namespace: &str,
    release_name: &str,
    request: HelmInstallRequest,
) -> Result<HelmRelease, ApiError> {
    post_json(
        &format!(
            "/k8s/clusters/{}/namespaces/{}/helm/releases/{}/upgrade",
            cluster_id, namespace, release_name
        ),
        &request,
    )
    .await
}

/// Uninstall Helm release
pub async fn uninstall_helm_release(
    cluster_id: &str,
    namespace: &str,
    release_name: &str,
) -> Result<(), ApiError> {
    delete_json(&format!(
        "/k8s/clusters/{}/namespaces/{}/helm/releases/{}",
        cluster_id, namespace, release_name
    ))
    .await
}

/// Get Helm release values
pub async fn get_helm_release_values(
    cluster_id: &str,
    namespace: &str,
    release_name: &str,
) -> Result<HelmValues, ApiError> {
    fetch_json(&format!(
        "/k8s/clusters/{}/namespaces/{}/helm/releases/{}/values",
        cluster_id, namespace, release_name
    ))
    .await
}

/// Get Helm release history
pub async fn get_helm_release_history(
    cluster_id: &str,
    namespace: &str,
    release_name: &str,
) -> Result<Vec<HelmRelease>, ApiError> {
    fetch_json(&format!(
        "/k8s/clusters/{}/namespaces/{}/helm/releases/{}/history",
        cluster_id, namespace, release_name
    ))
    .await
}

/// Rollback Helm release
pub async fn rollback_helm_release(
    cluster_id: &str,
    namespace: &str,
    release_name: &str,
    revision: u32,
) -> Result<HelmRelease, ApiError> {
    post_json(
        &format!(
            "/k8s/clusters/{}/namespaces/{}/helm/releases/{}/rollback",
            cluster_id, namespace, release_name
        ),
        &serde_json::json!({"revision": revision}),
    )
    .await
}

// ============================================================================
// Configuration Management APIs
// ============================================================================

/// Get ConfigMaps in a namespace
pub async fn get_configmaps(
    cluster_id: &str,
    namespace: &str,
) -> Result<Vec<KubernetesConfigMap>, ApiError> {
    fetch_json(&format!(
        "/k8s/clusters/{}/namespaces/{}/configmaps",
        cluster_id, namespace
    ))
    .await
}

/// Get ConfigMap details
pub async fn get_configmap(
    cluster_id: &str,
    namespace: &str,
    configmap_name: &str,
) -> Result<KubernetesConfigMap, ApiError> {
    fetch_json(&format!(
        "/k8s/clusters/{}/namespaces/{}/configmaps/{}",
        cluster_id, namespace, configmap_name
    ))
    .await
}

/// Create ConfigMap
pub async fn create_configmap(
    cluster_id: &str,
    namespace: &str,
    request: CreateConfigMapRequest,
) -> Result<KubernetesConfigMap, ApiError> {
    post_json(
        &format!(
            "/k8s/clusters/{}/namespaces/{}/configmaps",
            cluster_id, namespace
        ),
        &request,
    )
    .await
}

/// Update ConfigMap
pub async fn update_configmap(
    cluster_id: &str,
    namespace: &str,
    configmap_name: &str,
    request: CreateConfigMapRequest,
) -> Result<KubernetesConfigMap, ApiError> {
    post_json(
        &format!(
            "/k8s/clusters/{}/namespaces/{}/configmaps/{}",
            cluster_id, namespace, configmap_name
        ),
        &request,
    )
    .await
}

/// Delete ConfigMap
pub async fn delete_configmap(
    cluster_id: &str,
    namespace: &str,
    configmap_name: &str,
) -> Result<(), ApiError> {
    delete_json(&format!(
        "/k8s/clusters/{}/namespaces/{}/configmaps/{}",
        cluster_id, namespace, configmap_name
    ))
    .await
}

/// Get Secrets in a namespace
pub async fn get_secrets(
    cluster_id: &str,
    namespace: &str,
) -> Result<Vec<KubernetesSecret>, ApiError> {
    fetch_json(&format!(
        "/k8s/clusters/{}/namespaces/{}/secrets",
        cluster_id, namespace
    ))
    .await
}

/// Get Secret details
pub async fn get_secret(
    cluster_id: &str,
    namespace: &str,
    secret_name: &str,
) -> Result<KubernetesSecret, ApiError> {
    fetch_json(&format!(
        "/k8s/clusters/{}/namespaces/{}/secrets/{}",
        cluster_id, namespace, secret_name
    ))
    .await
}

/// Create Secret
pub async fn create_secret(
    cluster_id: &str,
    namespace: &str,
    request: CreateSecretRequest,
) -> Result<KubernetesSecret, ApiError> {
    post_json(
        &format!(
            "/k8s/clusters/{}/namespaces/{}/secrets",
            cluster_id, namespace
        ),
        &request,
    )
    .await
}

/// Update Secret
pub async fn update_secret(
    cluster_id: &str,
    namespace: &str,
    secret_name: &str,
    request: CreateSecretRequest,
) -> Result<KubernetesSecret, ApiError> {
    post_json(
        &format!(
            "/k8s/clusters/{}/namespaces/{}/secrets/{}",
            cluster_id, namespace, secret_name
        ),
        &request,
    )
    .await
}

/// Delete Secret
pub async fn delete_secret(
    cluster_id: &str,
    namespace: &str,
    secret_name: &str,
) -> Result<(), ApiError> {
    delete_json(&format!(
        "/k8s/clusters/{}/namespaces/{}/secrets/{}",
        cluster_id, namespace, secret_name
    ))
    .await
}

// ============================================================================
// Kubernetes API Aliases (for consistency with page imports)
// ============================================================================

/// Get ConfigMaps in a namespace (alias)
pub async fn get_kubernetes_configmaps(
    cluster_id: &str,
    namespace: Option<&str>,
) -> Result<Vec<KubernetesConfigMap>, ApiError> {
    get_configmaps(cluster_id, namespace.unwrap_or("default")).await
}

/// Create ConfigMap (alias)
pub async fn create_kubernetes_configmap(
    cluster_id: &str,
    namespace: &str,
    request: CreateConfigMapRequest,
) -> Result<KubernetesConfigMap, ApiError> {
    create_configmap(cluster_id, namespace, request).await
}

/// Update ConfigMap (alias)
pub async fn update_kubernetes_configmap(
    cluster_id: &str,
    namespace: &str,
    configmap_name: &str,
    request: CreateConfigMapRequest,
) -> Result<KubernetesConfigMap, ApiError> {
    update_configmap(cluster_id, namespace, configmap_name, request).await
}

/// Delete ConfigMap (alias)
pub async fn delete_kubernetes_configmap(
    cluster_id: &str,
    namespace: &str,
    configmap_name: &str,
) -> Result<(), ApiError> {
    delete_configmap(cluster_id, namespace, configmap_name).await
}

/// Get Secrets in a namespace (alias)
pub async fn get_kubernetes_secrets(
    cluster_id: &str,
    namespace: Option<&str>,
) -> Result<Vec<KubernetesSecret>, ApiError> {
    get_secrets(cluster_id, namespace.unwrap_or("default")).await
}

/// Create Secret (alias)
pub async fn create_kubernetes_secret(
    cluster_id: &str,
    namespace: &str,
    request: CreateSecretRequest,
) -> Result<KubernetesSecret, ApiError> {
    create_secret(cluster_id, namespace, request).await
}

/// Update Secret (alias)
pub async fn update_kubernetes_secret(
    cluster_id: &str,
    namespace: &str,
    secret_name: &str,
    request: CreateSecretRequest,
) -> Result<KubernetesSecret, ApiError> {
    update_secret(cluster_id, namespace, secret_name, request).await
}

/// Delete Secret (alias)
pub async fn delete_kubernetes_secret(
    cluster_id: &str,
    namespace: &str,
    secret_name: &str,
) -> Result<(), ApiError> {
    delete_secret(cluster_id, namespace, secret_name).await
}

// =============================================================================
// Backup & restore: types and calls live in api/backup.rs (lane D)
// =============================================================================
pub mod backup;

/// Snapshot-schedule retention (used by the snapshot pages).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionPolicy {
    pub keep_hourly: Option<u32>,
    pub keep_daily: Option<u32>,
    pub keep_weekly: Option<u32>,
    pub keep_monthly: Option<u32>,
    pub keep_yearly: Option<u32>,
    pub max_age_days: Option<u32>,
}

/// VM snapshot as shown in the UI (view of the API's
/// `vm::snapshot::VmSnapshot`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "VmSnapshotWire")]
pub struct VmSnapshot {
    pub id: String,
    pub vm_id: String,
    pub name: String,
    pub description: Option<String>,
    pub parent_id: Option<String>,
    pub created_at: String,
    pub memory_included: bool,
    pub size_mb: u64,
    pub children: Vec<String>,
}

#[derive(Deserialize)]
struct VmSnapshotWire {
    id: String,
    vm_id: String,
    name: String,
    #[serde(default)]
    description: Option<String>,
    created_at: i64,
    #[serde(default)]
    parent_snapshot: Option<String>,
    #[serde(default)]
    memory_snapshot: Option<String>,
    #[serde(default)]
    disk_snapshots: Vec<DiskSnapshotWire>,
}

#[derive(Deserialize)]
struct DiskSnapshotWire {
    #[serde(default)]
    size_bytes: u64,
}

impl From<VmSnapshotWire> for VmSnapshot {
    fn from(w: VmSnapshotWire) -> Self {
        let bytes: u64 = w.disk_snapshots.iter().map(|d| d.size_bytes).sum();
        Self {
            id: w.id,
            vm_id: w.vm_id,
            name: w.name,
            description: w.description,
            parent_id: w.parent_snapshot,
            created_at: crate::utils::format_unix_secs(w.created_at),
            memory_included: w.memory_snapshot.is_some(),
            size_mb: bytes / (1024 * 1024),
            children: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSnapshotRequest {
    pub name: String,
    pub description: Option<String>,
    pub include_memory: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreSnapshotRequest {
    pub restore_memory: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotTreeNode {
    pub snapshot: VmSnapshot,
    pub children: Vec<SnapshotTreeNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotSchedule {
    pub id: String,
    pub vm_id: String,
    pub name: String,
    pub description: Option<String>,
    pub schedule: String, // Cron expression
    pub retention_policy: RetentionPolicy,
    pub include_memory: bool,
    pub enabled: bool,
    pub created_at: String,
    pub last_run: Option<String>,
    pub next_run: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSnapshotScheduleRequest {
    pub vm_id: String,
    pub name: String,
    pub description: Option<String>,
    pub schedule: String,
    pub retention_policy: RetentionPolicy,
    pub include_memory: bool,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotQuota {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub quota_type: QuotaType,
    pub limit_value: u64,
    pub storage_path: String,
    pub cleanup_policy: CleanupPolicy,
    pub enabled: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::enum_variant_names)]
pub enum QuotaType {
    MaxCount,
    MaxSize,
    MaxAge,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CleanupPolicy {
    OldestFirst,
    LargestFirst,
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSnapshotQuotaRequest {
    pub name: String,
    pub description: Option<String>,
    pub quota_type: QuotaType,
    pub limit_value: u64,
    pub storage_path: String,
    pub cleanup_policy: CleanupPolicy,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaUsage {
    pub quota_id: String,
    pub current_count: u64,
    pub current_size: u64,
    pub oldest_snapshot: Option<String>,
    pub newest_snapshot: Option<String>,
    pub exceeds_quota: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaSummary {
    pub total_quotas: u64,
    pub exceeded_quotas: u64,
    pub warning_quotas: u64,
    pub healthy_quotas: u64,
    pub total_size_bytes: u64,
    pub total_max_bytes: u64,
    pub overall_usage_percent: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnforceQuotaRequest {
    pub snapshot_ids: Vec<String>,
}

/// Template as the Templates page shows it, built from the API's
/// `template::Template` (`created` unix time, `disk_path`, memory/cpus).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "TemplateWire")]
pub struct VmTemplate {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub source_vm_id: String,
    pub created_at: String,
    pub config: serde_json::Value,
    pub size_mb: u64,
    pub storage_location: String,
}

#[derive(Deserialize)]
struct TemplateWire {
    id: String,
    name: String,
    #[serde(default)]
    description: Option<String>,
    source_vm_id: String,
    #[serde(default)]
    created: i64,
    #[serde(default)]
    disk_path: String,
    #[serde(default)]
    storage_type: String,
    #[serde(default)]
    memory: u64,
    #[serde(default)]
    cpus: u32,
    #[serde(default)]
    os_type: serde_json::Value,
}

impl From<TemplateWire> for VmTemplate {
    fn from(w: TemplateWire) -> Self {
        Self {
            id: w.id,
            name: w.name,
            description: w.description,
            source_vm_id: w.source_vm_id,
            created_at: crate::utils::format_unix_secs(w.created),
            config: serde_json::json!({
                "memory": w.memory,
                "cpus": w.cpus,
                "os_type": w.os_type,
                "storage_type": w.storage_type,
            }),
            size_mb: 0,
            storage_location: w.disk_path,
        }
    }
}

/// Body of POST /api/templates (the API names the source VM `vm_id`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTemplateRequest {
    pub name: String,
    pub description: Option<String>,
    #[serde(rename = "vm_id", alias = "source_vm_id")]
    pub source_vm_id: String,
}

// Duplicate VM Snapshot function removed - using first definition

/// Get snapshot details
pub async fn get_vm_snapshot(vm_id: &str, snapshot_id: &str) -> Result<VmSnapshot, ApiError> {
    fetch_json(&format!("/vms/{}/snapshots/{}", vm_id, snapshot_id)).await
}

/// Create a new VM snapshot
pub async fn create_vm_snapshot(
    vm_id: &str,
    request: CreateSnapshotRequest,
) -> Result<VmSnapshot, ApiError> {
    post_json(&format!("/vms/{}/snapshots", vm_id), &request).await
}

/// Delete a VM snapshot
pub async fn delete_vm_snapshot(vm_id: &str, snapshot_id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/vms/{}/snapshots/{}", vm_id, snapshot_id)).await
}

/// Restore a VM snapshot
pub async fn restore_vm_snapshot(
    vm_id: &str,
    snapshot_id: &str,
    request: RestoreSnapshotRequest,
) -> Result<(), ApiError> {
    post_json(
        &format!("/vms/{}/snapshots/{}/restore", vm_id, snapshot_id),
        &request,
    )
    .await
}

/// Get snapshot tree for a VM
pub async fn get_vm_snapshot_tree(vm_id: &str) -> Result<Vec<SnapshotTreeNode>, ApiError> {
    fetch_json(&format!("/vms/{}/snapshots/tree", vm_id)).await
}

// Snapshot Schedule API Functions
/// List all snapshot schedules
pub async fn get_snapshot_schedules() -> Result<Vec<SnapshotSchedule>, ApiError> {
    fetch_json("/snapshot-schedules").await
}

/// Get snapshot schedule details
pub async fn get_snapshot_schedule(id: &str) -> Result<SnapshotSchedule, ApiError> {
    fetch_json(&format!("/snapshot-schedules/{}", id)).await
}

/// Create a new snapshot schedule
pub async fn create_snapshot_schedule(
    request: CreateSnapshotScheduleRequest,
) -> Result<SnapshotSchedule, ApiError> {
    post_json("/snapshot-schedules", &request).await
}

/// Update a snapshot schedule
pub async fn update_snapshot_schedule(
    id: &str,
    request: CreateSnapshotScheduleRequest,
) -> Result<SnapshotSchedule, ApiError> {
    put_json(&format!("/snapshot-schedules/{}", id), &request).await
}

/// Delete a snapshot schedule
pub async fn delete_snapshot_schedule(id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/snapshot-schedules/{}", id)).await
}

// Snapshot Quota API Functions
/// List all snapshot quotas
pub async fn get_snapshot_quotas() -> Result<Vec<SnapshotQuota>, ApiError> {
    fetch_json("/snapshot-quotas").await
}

/// Get snapshot quota details
pub async fn get_snapshot_quota(id: &str) -> Result<SnapshotQuota, ApiError> {
    fetch_json(&format!("/snapshot-quotas/{}", id)).await
}

/// Create a new snapshot quota
pub async fn create_snapshot_quota(
    request: CreateSnapshotQuotaRequest,
) -> Result<SnapshotQuota, ApiError> {
    post_json("/snapshot-quotas", &request).await
}

/// Update a snapshot quota
pub async fn update_snapshot_quota(
    id: &str,
    request: CreateSnapshotQuotaRequest,
) -> Result<SnapshotQuota, ApiError> {
    put_json(&format!("/snapshot-quotas/{}", id), &request).await
}

/// Delete a snapshot quota
pub async fn delete_snapshot_quota(id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/snapshot-quotas/{}", id)).await
}

/// Get snapshot quota usage
pub async fn get_snapshot_quota_usage(id: &str) -> Result<QuotaUsage, ApiError> {
    fetch_json(&format!("/snapshot-quotas/{}/usage", id)).await
}

/// Get snapshot quota summary
pub async fn get_snapshot_quota_summary() -> Result<QuotaSummary, ApiError> {
    fetch_json("/snapshot-quotas/summary").await
}

/// Enforce snapshot quota
pub async fn enforce_snapshot_quota(
    id: &str,
    request: EnforceQuotaRequest,
) -> Result<(), ApiError> {
    post_json(&format!("/snapshot-quotas/{}/enforce", id), &request).await
}

// Template API Functions
/// List all templates
pub async fn get_templates() -> Result<Vec<VmTemplate>, ApiError> {
    fetch_json("/templates").await
}

/// Get template details
pub async fn get_template(id: &str) -> Result<VmTemplate, ApiError> {
    fetch_json(&format!("/templates/{}", id)).await
}

/// Create a new template
pub async fn create_template(request: CreateTemplateRequest) -> Result<VmTemplate, ApiError> {
    post_json("/templates", &request).await
}

/// Delete a template
pub async fn delete_template(id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/templates/{}", id)).await
}

/// VM id for a clone named `name` ("Web Server 2" -> "web-server-2").
pub fn vm_id_from_name(name: &str) -> String {
    let mut id = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            id.push(c.to_ascii_lowercase());
        } else if !id.ends_with('-') && !id.is_empty() {
            id.push('-');
        }
    }
    id.trim_end_matches('-').to_string()
}

/// Body of POST /api/templates/:id/clone (the API's template::CloneRequest).
pub fn clone_template_body(name: &str) -> serde_json::Value {
    serde_json::json!({
        "new_vm_id": vm_id_from_name(name),
        "new_vm_name": name.trim(),
        "clone_type": "full",
        "storage_pool": null,
        "cloudinit_config": null,
    })
}

/// Clone a template into a new VM; returns the API's clone response.
pub async fn clone_template(id: &str, name: String) -> Result<serde_json::Value, ApiError> {
    post_json(
        &format!("/templates/{}/clone", id),
        &clone_template_body(&name),
    )
    .await
}

/// Clone a VM (POST /api/vms/:id/clone); returns the new VM.
pub async fn clone_vm(
    vm_id: &str,
    name: &str,
    mode: &str,
) -> Result<horcrux_common::VmConfig, ApiError> {
    post_json(
        &format!("/vms/{}/clone", vm_id),
        &serde_json::json!({"name": name.trim(), "id": vm_id_from_name(name), "mode": mode}),
    )
    .await
}

// =============================================================================
// High Availability & Clustering API
// =============================================================================

// Duplicate removed - using the first ClusterNode definition

/// Body of `POST /api/cluster/nodes/:name`. Only the address is required;
/// the server fills in the rest (architecture defaults to "unknown").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddNodeRequest {
    pub address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub architecture: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<u32>,
}

/// `GET /api/cluster/architecture` exactly as horcrux-api sends it
/// (`cluster::ArchitectureSummary`). This is the original, documented
/// contract (openapi.yaml, CLI); the UI previously expected an invented
/// `{total_nodes, online_nodes, total_vms, architectures, load_balance_mode}`
/// shape and failed with "missing field `total_nodes`".
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClusterArchitecture {
    pub x86_64_nodes: u32,
    pub aarch64_nodes: u32,
    pub other_nodes: u32,
    pub is_mixed: bool,
}

impl ClusterArchitecture {
    pub fn total_nodes(&self) -> u32 {
        self.x86_64_nodes + self.aarch64_nodes + self.other_nodes
    }
}

/// Body of `POST /api/cluster/find-node`; the response is the chosen
/// node's name as a bare JSON string.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindNodeRequest {
    pub architecture: String,
    pub memory_mb: u64,
    pub cpu_cores: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_priority: Option<u32>,
}

// Removed duplicate HA/Migration structs - using first definitions

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateHaResourceRequest {
    pub vm_id: String,
    pub priority: u32,
    pub group_id: Option<String>,
    pub failover_domain: Vec<String>,
    pub auto_failover: bool,
    pub max_restart_attempts: u32,
    pub restart_policy_delay: u32,
}

/// HA overview (view of `GET /api/ha/status`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "HaStatusWire")]
pub struct HaStatus {
    pub cluster_status: HaClusterStatus,
    pub resources: Vec<HaResourceStatus>,
    pub last_failover: Option<String>,
    pub total_failovers: u32,
}

#[derive(Deserialize)]
struct HaStatusWire {
    #[serde(default)]
    migrating: usize,
    #[serde(default)]
    resources: Vec<HaStatusResourceWire>,
}

#[derive(Deserialize)]
struct HaStatusResourceWire {
    vm_id: String,
    state: String,
    #[serde(default)]
    current_node: Option<String>,
    #[serde(default)]
    preferred_node: Option<String>,
    #[serde(default)]
    restart_count: u32,
}

impl From<HaStatusWire> for HaStatus {
    fn from(w: HaStatusWire) -> Self {
        let resources: Vec<HaResourceStatus> = w
            .resources
            .into_iter()
            .map(|r| {
                let (status, health) = match r.state.as_str() {
                    "Started" => (HaResourceState::Running, HealthCheckStatus::Healthy),
                    "Stopped" | "Disabled" => {
                        (HaResourceState::Stopped, HealthCheckStatus::Unknown)
                    }
                    "Migrating" => (HaResourceState::Migrating, HealthCheckStatus::Warning),
                    "Error" => (HaResourceState::Failed, HealthCheckStatus::Critical),
                    _ => (HaResourceState::Unknown, HealthCheckStatus::Unknown),
                };
                HaResourceStatus {
                    vm_id: r.vm_id,
                    current_node: r.current_node.unwrap_or_else(|| "-".to_string()),
                    desired_node: r.preferred_node,
                    status,
                    health_check: health,
                    last_migration: None,
                    restart_count: r.restart_count,
                }
            })
            .collect();
        let failed = resources
            .iter()
            .any(|r| matches!(r.status, HaResourceState::Failed));
        Self {
            cluster_status: if failed {
                HaClusterStatus::Failed
            } else if w.migrating > 0 {
                HaClusterStatus::Degraded
            } else {
                HaClusterStatus::Active
            },
            resources,
            last_failover: None,
            total_failovers: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HaClusterStatus {
    Active,
    Degraded,
    Failed,
    Maintenance,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HaResourceStatus {
    pub vm_id: String,
    pub current_node: String,
    pub desired_node: Option<String>,
    pub status: HaResourceState,
    pub health_check: HealthCheckStatus,
    pub last_migration: Option<String>,
    pub restart_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HaResourceState {
    Running,
    Stopped,
    Migrating,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HealthCheckStatus {
    Healthy,
    Warning,
    Critical,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PlacementPolicy {
    Balanced,
    Consolidated,
    AntiAffinity,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateHaGroupRequest {
    pub name: String,
    pub description: Option<String>,
    pub priority: u32,
    pub failover_domain: Vec<String>,
    pub placement_policy: PlacementPolicy,
    pub vm_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MigrationType {
    Live,
    Offline,
    Online,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MigrationStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationRequest {
    pub target_node: String,
    pub migration_type: Option<String>,
    pub bandwidth_limit: Option<u64>,
    pub force: Option<bool>,
}

// Cluster Management API Functions (duplicate get_cluster_nodes removed)

/// Add a new node to the cluster (server replies 201 with no body).
pub async fn add_cluster_node(name: &str, request: &AddNodeRequest) -> Result<(), ApiError> {
    post_json_no_response(&format!("/cluster/nodes/{}", name), request).await
}

/// Get cluster architecture information
pub async fn get_cluster_architecture() -> Result<ClusterArchitecture, ApiError> {
    fetch_json("/cluster/architecture").await
}

/// Find the best node for a VM; returns the node name.
pub async fn find_best_node_for_vm(request: &FindNodeRequest) -> Result<String, ApiError> {
    post_json("/cluster/find-node", request).await
}

// HA Management API Functions
/// List all HA resources
pub async fn get_ha_resources() -> Result<Vec<HaResource>, ApiError> {
    fetch_json("/ha/resources").await
}

/// Add a VM to HA management
pub async fn add_ha_resource(request: CreateHaResourceRequest) -> Result<(), ApiError> {
    post_json_no_response(
        "/ha/resources",
        &serde_json::json!({
            "vm_id": request.vm_id,
            "group": request.group_id.unwrap_or_default(),
            "priority": request.priority,
        }),
    )
    .await
}

/// Remove a VM from HA management
pub async fn remove_ha_resource(vm_id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/ha/resources/{}", vm_id)).await
}

/// Get HA cluster status
pub async fn get_ha_status() -> Result<HaStatus, ApiError> {
    fetch_json("/ha/status").await
}

/// List all HA groups
pub async fn get_ha_groups() -> Result<Vec<HaGroup>, ApiError> {
    fetch_json("/ha/groups").await
}

/// Create a new HA group
pub async fn create_ha_group(group: HaGroup) -> Result<HaGroup, ApiError> {
    post_json_no_response(
        "/ha/groups",
        &serde_json::json!({
            "name": group.name,
            "nodes": group.nodes,
            "restricted": group.restricted,
            "no_failback": group.nofailback,
        }),
    )
    .await?;
    Ok(group)
}

// Migration API Functions
/// Start VM migration
pub async fn migrate_vm(vm_id: &str, request: MigrationRequest) -> Result<MigrationJob, ApiError> {
    post_json(&format!("/migrate/{}", vm_id), &request).await
}

/// Get migration status
pub async fn get_migration_status(vm_id: &str) -> Result<MigrationJob, ApiError> {
    fetch_json(&format!("/migrate/{}/status", vm_id)).await
}

// Additional HA Management API Functions for the UI components
/// Update an existing HA group
pub async fn update_ha_group(group: HaGroup) -> Result<HaGroup, ApiError> {
    let url = resolve_api_url(&format!("/ha/groups/{}", group.name));
    let body = serde_json::json!({
        "nodes": group.nodes,
        "restricted": group.restricted,
        "no_failback": group.nofailback,
    });
    let response = with_auth(reqwasm::http::Request::put(&url))
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .map_err(|e| ApiError {
            message: e.to_string(),
        })?;
    if response.ok() {
        Ok(group)
    } else {
        Err(http_error(response).await)
    }
}

/// Delete an HA group
pub async fn delete_ha_group(group_id: String) -> Result<(), ApiError> {
    delete_json(&format!("/ha/groups/{}", group_id)).await
}

/// Assign resource to HA group
pub async fn assign_resource_to_ha_group(assignment: HaResourceAssignment) -> Result<(), ApiError> {
    post_json_no_response(
        &format!("/ha/groups/{}/resources", assignment.group_id),
        &serde_json::json!({
            "resource_id": assignment.resource_id,
            "priority": assignment.priority,
        }),
    )
    .await
}

// Migration Management API Functions for the UI components
/// Get all migration jobs
pub async fn get_migration_jobs() -> Result<Vec<MigrationJob>, ApiError> {
    fetch_json("/migration/jobs").await
}

/// Create migration job
pub async fn create_migration_job(job: MigrationJob) -> Result<MigrationJob, ApiError> {
    post_json("/migration/jobs", &job).await
}

/// Cancel migration job
pub async fn cancel_migration_job(job_id: String) -> Result<(), ApiError> {
    delete_json(&format!("/migration/jobs/{}", job_id)).await
}

/// Create bulk migration job
pub async fn create_bulk_migration_job(
    job: BulkMigrationJob,
) -> Result<BulkMigrationJob, ApiError> {
    post_json("/migration/bulk", &job).await
}

// ============================================================================
// Monitoring & Alerting Data Structures
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AlertRule {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub metric: String,
    pub condition: String, // greater_than, less_than, equal_to, not_equal_to
    pub threshold: f64,
    pub duration_seconds: u32,
    pub severity: String, // info, warning, critical
    pub enabled: bool,
    pub labels: std::collections::HashMap<String, String>,
    pub annotations: std::collections::HashMap<String, String>,
    pub notification_channels: Vec<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_triggered: Option<chrono::DateTime<chrono::Utc>>,
    pub trigger_count: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActiveAlert {
    pub id: String,
    pub rule_id: String,
    pub rule_name: String,
    pub metric: String,
    pub condition: String,
    pub threshold: f64,
    pub current_value: f64,
    pub severity: String,
    pub status: String, // firing, pending, resolved, silenced
    pub message: String,
    pub labels: std::collections::HashMap<String, String>,
    pub annotations: std::collections::HashMap<String, String>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub resolved_at: Option<chrono::DateTime<chrono::Utc>>,
    pub acknowledged_at: Option<chrono::DateTime<chrono::Utc>>,
    pub silenced_until: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NotificationChannel {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub channel_type: String, // email, slack, teams, discord, pagerduty, webhook
    pub enabled: bool,
    pub config: NotificationChannelConfig,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_used: Option<chrono::DateTime<chrono::Utc>>,
    pub success_count: u32,
    pub failure_count: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum NotificationChannelConfig {
    Email {
        address: String,
        smtp_server: String,
        smtp_port: u16,
        username: String,
        password: String,
        use_tls: bool,
    },
    Slack {
        webhook_url: String,
        channel: Option<String>,
        username: Option<String>,
        icon_emoji: Option<String>,
    },
    Teams {
        webhook_url: String,
    },
    Discord {
        webhook_url: String,
        username: Option<String>,
        avatar_url: Option<String>,
    },
    PagerDuty {
        integration_key: String,
        #[serde(default)]
        routing_key: String,
        severity: String,
    },
    Webhook {
        url: String,
        method: String,
        headers: std::collections::HashMap<String, String>,
        auth: Option<WebhookAuth>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WebhookAuth {
    BasicAuth { username: String, password: String },
    BearerToken { token: String },
    ApiKey { key: String, header: String },
}

// ============================================================================
// Metrics & Query Management Data Structures
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MetricDefinition {
    pub name: String,
    pub description: String,
    pub metric_type: String, // counter, gauge, histogram, summary
    pub labels: Vec<String>,
    pub unit: Option<String>,
    pub help: String,
    pub scrape_interval: String,
    pub retention: String,
    pub cardinality: u64,
    pub last_scraped: String,
    pub source: String,   // prometheus, node_exporter, custom, etc.
    pub category: String, // system, application, network, storage
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetricSample {
    pub timestamp: String,
    pub value: f64,
    pub labels: std::collections::HashMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetricSeries {
    pub metric: String,
    pub samples: Vec<MetricSample>,
    pub min_value: f64,
    pub max_value: f64,
    pub avg_value: f64,
    pub sample_count: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueryHistoryEntry {
    pub id: String,
    pub query: String,
    #[serde(default)]
    pub time_range: String,
    pub query_type: String, // promql, logql, sql
    pub timestamp: String,
    #[serde(default)]
    pub executed_at: chrono::DateTime<chrono::Utc>,
    pub execution_time_ms: f64,
    pub result_count: u64,
    pub status: String, // success, error, timeout
    pub error_message: Option<String>,
    pub user: String,
    pub source: String, // metrics_explorer, dashboard, alert_rule, api
    pub tags: Vec<String>,
    pub favorite: bool,
    pub shared: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueryTemplate {
    pub id: String,
    pub name: String,
    pub description: String,
    pub query: String,
    pub category: String,
    pub variables: Vec<QueryVariable>,
    pub created_by: String,
    pub created_at: String,
    pub usage_count: u64,
    pub public: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueryVariable {
    pub name: String,
    pub description: String,
    pub var_type: String, // string, number, metric, label_value
    pub default_value: Option<String>,
    pub options: Vec<String>,
    pub required: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateQueryTemplateRequest {
    pub name: String,
    pub description: String,
    pub query: String,
    pub category: String,
    pub public: bool,
    pub variables: Vec<QueryVariable>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AlertPreview {
    pub query_valid: bool,
    pub current_value: Option<f64>,
    pub would_trigger: bool,
    pub sample_data: Vec<MetricSample>,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateAlertRuleRequest {
    pub name: String,
    pub description: String,
    pub query: String,
    pub condition: String,
    pub threshold: f64,
    pub duration: String,
    pub severity: String,
    pub labels: std::collections::HashMap<String, String>,
    pub annotations: std::collections::HashMap<String, String>,
    pub notification_channels: Vec<String>,
}

// Metrics API Functions
pub async fn get_metrics_catalog() -> Result<Vec<MetricDefinition>, ApiError> {
    fetch_json("/metrics/catalog").await
}

pub async fn get_metric_samples(
    metric_name: String,
    time_range: String,
) -> Result<MetricSeries, ApiError> {
    fetch_json(&format!(
        "/metrics/{}/samples?range={}",
        metric_name, time_range
    ))
    .await
}

pub async fn get_query_history(time_range: String) -> Result<Vec<QueryHistoryEntry>, ApiError> {
    fetch_json(&format!("/metrics/query-history?range={}", time_range)).await
}

pub async fn toggle_query_favorite(entry_id: String) -> Result<(), ApiError> {
    post_json(
        &format!("/metrics/query-history/{}/favorite", entry_id),
        &(),
    )
    .await
}

pub async fn get_query_templates() -> Result<Vec<QueryTemplate>, ApiError> {
    fetch_json("/metrics/query-templates").await
}

pub async fn create_query_template(
    request: CreateQueryTemplateRequest,
) -> Result<QueryTemplate, ApiError> {
    post_json("/metrics/query-templates", &request).await
}

pub async fn delete_query_history_entry(entry_id: String) -> Result<(), ApiError> {
    delete_json(&format!("/metrics/query-history/{}", entry_id)).await
}

pub async fn get_alert_rules() -> Result<Vec<AlertRule>, ApiError> {
    fetch_json("/alerts/rules").await
}

pub async fn create_alert_rule(rule: AlertRule) -> Result<AlertRule, ApiError> {
    post_json("/alerts/rules", &rule).await
}

pub async fn update_alert_rule(rule: AlertRule) -> Result<AlertRule, ApiError> {
    put_json(&format!("/alerts/rules/{}", rule.id), &rule).await
}

pub async fn delete_alert_rule(rule_id: String) -> Result<(), ApiError> {
    delete_json(&format!("/alerts/rules/{}", rule_id)).await
}

pub async fn toggle_alert_rule(rule_id: String, enabled: bool) -> Result<(), ApiError> {
    put_json(
        &format!("/alerts/rules/{}/toggle", rule_id),
        &serde_json::json!({"enabled": enabled}),
    )
    .await
}

pub async fn preview_alert_rule(
    query: String,
    condition: String,
    threshold: f64,
) -> Result<AlertPreview, ApiError> {
    let request = serde_json::json!({
        "query": query,
        "condition": condition,
        "threshold": threshold
    });
    post_json("/alerts/preview", &request).await
}

pub async fn get_notification_channels() -> Result<Vec<NotificationChannel>, ApiError> {
    fetch_json("/notifications/channels").await
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WebhookConfig {
    pub id: String,
    pub name: String,
    pub url: String,
    pub method: String,
    pub headers: std::collections::HashMap<String, String>,
    pub events: Vec<String>,
    pub enabled: bool,
    pub secret: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_triggered: Option<chrono::DateTime<chrono::Utc>>,
    pub trigger_count: u32,
    pub success_count: u32,
    pub failure_count: u32,
}

// (Duplicate alert/notification functions removed - already defined above)
pub async fn delete_notification_channel(channel_id: String) -> Result<(), ApiError> {
    delete_json(&format!("/notifications/channels/{}", channel_id)).await
}

/// Test notification channel
pub async fn test_notification_channel(channel_id: String) -> Result<(), ApiError> {
    post_json(&format!("/notifications/channels/{}/test", channel_id), &()).await
}

/// Test alert rule
pub async fn test_alert_rule(rule_id: String) -> Result<(), ApiError> {
    post_json(&format!("/alerts/rules/{}/test", rule_id), &()).await
}

/// Acknowledge alert
pub async fn acknowledge_alert(alert_id: String) -> Result<(), ApiError> {
    post_json(&format!("/alerts/{}/acknowledge", alert_id), &()).await
}

/// Silence alert for a duration
pub async fn silence_alert(alert_id: String, duration_seconds: u32) -> Result<(), ApiError> {
    let body = serde_json::json!({ "duration_seconds": duration_seconds });
    post_json(&format!("/alerts/{}/silence", alert_id), &body).await
}

/// Create notification channel
pub async fn create_notification_channel(
    channel: NotificationChannel,
) -> Result<NotificationChannel, ApiError> {
    post_json("/notifications/channels", &channel).await
}

/// Update notification channel
pub async fn update_notification_channel(
    channel_id: &str,
    channel: NotificationChannel,
) -> Result<NotificationChannel, ApiError> {
    put_json(&format!("/notifications/channels/{}", channel_id), &channel).await
}

/// Get webhook configurations
pub async fn get_webhook_configs() -> Result<Vec<WebhookConfig>, ApiError> {
    fetch_json("/webhooks").await
}

/// Query metrics
pub async fn query_metrics(query: MetricQuery) -> Result<MetricResult, ApiError> {
    post_json("/metrics/query", &query).await
}

/// Get available metrics
pub async fn get_available_metrics() -> Result<Vec<MetricDefinition>, ApiError> {
    fetch_json("/metrics/definitions").await
}

// ============================================================================
// System Configuration Data Structures
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SystemConfiguration {
    pub hostname: String,
    pub domain: Option<String>,
    pub timezone: String,
    pub locale: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetworkInterface {
    pub name: String,
    pub method: String, // dhcp, static, manual
    pub address: Option<String>,
    pub netmask: Option<String>,
    pub gateway: Option<String>,
    pub mtu: Option<u16>,
    pub auto: bool,
    pub bridge: Option<String>,
    pub bridge_ports: Option<Vec<String>>,
    pub vlan_id: Option<u16>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DnsConfiguration {
    pub servers: Vec<String>,
    pub search_domains: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NtpConfiguration {
    pub servers: Vec<String>,
    pub timezone: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SystemHealth {
    pub hostname: String,
    pub uptime: u64,
    pub load_average: [f64; 3],
    pub memory_usage: MemoryUsage,
    pub disk_usage: Vec<DiskUsage>,
    pub network_stats: Vec<NetworkStats>,
    pub system_info: SystemInfo,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryUsage {
    pub total: u64,
    pub used: u64,
    pub free: u64,
    pub available: u64,
    pub buffers: u64,
    pub cached: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiskUsage {
    pub device: String,
    pub mount_point: String,
    pub total: u64,
    pub used: u64,
    pub available: u64,
    pub filesystem: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetworkStats {
    pub interface: String,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_packets: u64,
    pub tx_packets: u64,
    pub rx_errors: u64,
    pub tx_errors: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SystemInfo {
    pub kernel_version: String,
    pub arch: String,
    pub cpu_model: String,
    pub cpu_cores: u32,
    pub total_memory: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServiceStatus {
    pub name: String,
    pub status: String, // active, inactive, failed, unknown
    pub enabled: bool,
    pub description: String,
    pub pid: Option<u32>,
    pub memory_usage: Option<u64>,
    pub cpu_usage: Option<f64>,
}

// Removing duplicates - using extended versions below

// System Configuration API Functions
/// Get system configuration
pub async fn get_system_configuration() -> Result<SystemConfiguration, ApiError> {
    fetch_json("/system/config").await
}

/// Update system configuration
pub async fn update_system_configuration(config: SystemConfiguration) -> Result<(), ApiError> {
    post_json("/system/config", &config).await
}

/// Get network interfaces
pub async fn get_network_interfaces() -> Result<Vec<NetworkInterface>, ApiError> {
    fetch_json("/system/network/interfaces").await
}

/// Update network interface
pub async fn update_network_interface(interface: NetworkInterface) -> Result<(), ApiError> {
    post_json(
        &format!("/system/network/interfaces/{}", interface.name),
        &interface,
    )
    .await
}

/// Get DNS configuration
pub async fn get_dns_configuration() -> Result<DnsConfiguration, ApiError> {
    fetch_json("/system/network/dns").await
}

/// Update DNS configuration
pub async fn update_dns_configuration(config: DnsConfiguration) -> Result<(), ApiError> {
    post_json("/system/network/dns", &config).await
}

/// Get NTP configuration
pub async fn get_ntp_configuration() -> Result<NtpConfiguration, ApiError> {
    fetch_json("/system/time/ntp").await
}

/// Update NTP configuration
pub async fn update_ntp_configuration(config: NtpConfiguration) -> Result<(), ApiError> {
    post_json("/system/time/ntp", &config).await
}

/// Get system health
pub async fn get_system_health() -> Result<SystemHealth, ApiError> {
    fetch_json("/system/health").await
}

/// Get services status
pub async fn get_services_status() -> Result<Vec<ServiceStatus>, ApiError> {
    fetch_json("/system/services").await
}

/// Control service (start, stop, restart, enable, disable)
pub async fn control_service(service_name: &str, action: &str) -> Result<(), ApiError> {
    post_json(
        &format!("/system/services/{}/{}", service_name, action),
        &(),
    )
    .await
}

// Removing old get_system_logs - using the extended version with filters below

/// Search packages
pub async fn search_packages(query: &str) -> Result<Vec<PackageInfo>, ApiError> {
    fetch_json(&format!("/system/packages/search?q={}", query)).await
}

/// Install package
pub async fn install_package(package_name: &str) -> Result<(), ApiError> {
    post_json(&format!("/system/packages/{}/install", package_name), &()).await
}

/// Remove package
pub async fn remove_package(package_name: &str) -> Result<(), ApiError> {
    post_json(&format!("/system/packages/{}/remove", package_name), &()).await
}

/// Update package database
pub async fn update_package_database() -> Result<(), ApiError> {
    post_json("/system/packages/update", &()).await
}

// ============================================================================
// System Management Extensions for Logs, Package Manager, and Diagnostics
// ============================================================================

// Extended LogEntry structure to support the logs page
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogFilters {
    pub level: Option<String>,
    pub service: Option<String>,
    pub search: Option<String>,
    pub limit: Option<u32>,
}

/// Get system logs with extended filtering
pub async fn get_system_logs(filters: LogFilters) -> Result<Vec<LogEntry>, ApiError> {
    let mut url = "/system/logs".to_string();
    let mut params = Vec::new();

    if let Some(level) = filters.level {
        params.push(format!("level={}", level));
    }
    if let Some(service) = filters.service {
        params.push(format!("service={}", service));
    }
    if let Some(search) = filters.search {
        params.push(format!("search={}", search));
    }
    if let Some(limit) = filters.limit {
        params.push(format!("limit={}", limit));
    }

    if !params.is_empty() {
        url.push('?');
        url.push_str(&params.join("&"));
    }

    fetch_json(&url).await
}

/// Clear system logs
pub async fn clear_system_logs() -> Result<(), ApiError> {
    delete_json("/system/logs").await
}

/// Export system logs
pub async fn export_system_logs(filters: LogFilters, format: String) -> Result<String, ApiError> {
    let mut url = format!("/system/logs/export?format={}", format);
    let mut params = Vec::new();

    if let Some(level) = filters.level {
        params.push(format!("level={}", level));
    }
    if let Some(service) = filters.service {
        params.push(format!("service={}", service));
    }
    if let Some(search) = filters.search {
        params.push(format!("search={}", search));
    }

    if !params.is_empty() {
        url.push('&');
        url.push_str(&params.join("&"));
    }

    fetch_text(&url).await
}

// Package Manager Extensions
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PackageInfo {
    pub name: String,
    pub description: String,
    pub status: String, // installed, upgradeable, not-installed
    pub installed_version: Option<String>,
    pub available_version: Option<String>,
    pub size: Option<u64>,
}

/// Get installed packages
pub async fn get_installed_packages() -> Result<Vec<PackageInfo>, ApiError> {
    fetch_json("/system/packages").await
}

/// Upgrade package
pub async fn upgrade_package(package_name: &str) -> Result<(), ApiError> {
    post_json(&format!("/system/packages/{}/upgrade", package_name), &()).await
}

/// Upgrade all packages
pub async fn upgrade_all_packages() -> Result<u32, ApiError> {
    post_json("/system/packages/upgrade-all", &()).await
}

/// Bulk install packages
pub async fn bulk_install_packages(package_names: Vec<String>) -> Result<(), ApiError> {
    post_json("/system/packages/bulk-install", &package_names).await
}

// Diagnostics System
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SystemDiagnostics {
    pub overall_health_score: f64,
    pub critical_issues_count: u32,
    pub warning_count: u32,
    pub last_check_time: String,
    pub performance_metrics: PerformanceMetrics,
    pub security_status: SecurityStatus,
    pub available_tests: Vec<DiagnosticTest>,
    pub system_issues: Vec<SystemIssue>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PerformanceMetrics {
    pub cpu_score: f64,
    pub memory_score: f64,
    pub disk_score: f64,
    pub network_score: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SecurityStatus {
    pub score: f64,
    pub vulnerability_count: u32,
    pub compliance_level: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiagnosticTest {
    pub name: String,
    pub category: String,
    pub description: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiagnosticTestResult {
    pub passed: bool,
    pub message: String,
    pub duration_ms: u64,
    pub error_details: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SystemIssue {
    pub severity: String,
    pub category: String,
    pub description: String,
    pub recommendation: Option<String>,
}

/// Get system diagnostics
pub async fn get_system_diagnostics() -> Result<SystemDiagnostics, ApiError> {
    fetch_json("/system/diagnostics").await
}

/// Run diagnostic test
pub async fn run_diagnostic_test(test_name: &str) -> Result<DiagnosticTestResult, ApiError> {
    post_json(&format!("/system/diagnostics/tests/{}", test_name), &()).await
}

/// Run all diagnostic tests
pub async fn run_all_diagnostic_tests(
) -> Result<std::collections::HashMap<String, DiagnosticTestResult>, ApiError> {
    post_json("/system/diagnostics/tests/run-all", &()).await
}

/// Generate diagnostic report
pub async fn generate_diagnostic_report(format: &str) -> Result<String, ApiError> {
    fetch_text(&format!("/system/diagnostics/report?format={}", format)).await
}

// Extended LogEntry to support service field and details
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub level: String,
    pub service: Option<String>,
    pub message: String,
    pub details: Option<String>,
}

// ============================================================================
// Custom Dashboard System Data Structures
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CustomDashboard {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub category: String,
    pub layout: String,
    pub refresh_interval: u32,
    pub public: bool,
    pub tags: Vec<String>,
    pub widget_count: u32,
    pub usage_count: u32,
    pub rating: Option<f64>,
    pub created_by: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateDashboardRequest {
    pub name: String,
    pub description: Option<String>,
    pub category: String,
    pub layout: String,
    pub refresh_interval: u32,
    pub public: bool,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdateDashboardRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub category: Option<String>,
    pub layout: Option<String>,
    pub refresh_interval: Option<u32>,
    pub public: Option<bool>,
    pub tags: Option<Vec<String>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CloneDashboardRequest {
    pub name: String,
    pub copy_widgets: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportDashboardRequest {
    pub source_dashboard_id: String,
    pub name: String,
    pub import_widgets: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DashboardWidget {
    pub id: String,
    pub dashboard_id: String,
    pub dashboard_name: String,
    pub title: String,
    pub widget_type: String,
    pub metric: String,
    pub width: u32,
    pub height: u32,
    pub position_x: u32,
    pub position_y: u32,
    pub config: WidgetConfig,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct WidgetConfig {
    pub colors: Option<Vec<String>>,
    pub theme: Option<String>,
    pub show_legend: Option<bool>,
    pub show_grid: Option<bool>,
    pub time_range: Option<String>,
    pub aggregation: Option<String>,
    pub threshold: Option<f64>,
    pub unit: Option<String>,
    pub decimal_places: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateWidgetRequest {
    pub title: String,
    pub widget_type: String,
    pub metric: String,
    pub width: u32,
    pub height: u32,
    pub position_x: u32,
    pub position_y: u32,
    pub config: WidgetConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChartTemplate {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub category: String,
    pub chart_type: String,
    pub config: serde_json::Value,
    pub usage_count: u32,
    pub custom: bool,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateChartTemplateRequest {
    pub name: String,
    pub description: Option<String>,
    pub category: String,
    pub chart_type: String,
    pub config: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DashboardCategory {
    pub name: String,
    pub description: Option<String>,
    pub dashboard_count: u32,
}

// ============================================================================
// Custom Dashboard System API Functions
// ============================================================================

/// Get all custom dashboards
pub async fn get_custom_dashboards() -> Result<Vec<CustomDashboard>, ApiError> {
    fetch_json("/dashboards/custom").await
}

/// Get specific custom dashboard
pub async fn get_custom_dashboard(id: &str) -> Result<CustomDashboard, ApiError> {
    fetch_json(&format!("/dashboards/custom/{}", id)).await
}

/// Create custom dashboard
pub async fn create_custom_dashboard(
    request: CreateDashboardRequest,
) -> Result<CustomDashboard, ApiError> {
    post_json("/dashboards/custom", &request).await
}

/// Update custom dashboard
pub async fn update_custom_dashboard(
    id: &str,
    request: UpdateDashboardRequest,
) -> Result<CustomDashboard, ApiError> {
    put_json(&format!("/dashboards/custom/{}", id), &request).await
}

/// Delete custom dashboard
pub async fn delete_custom_dashboard(id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/dashboards/custom/{}", id)).await
}

/// Clone custom dashboard
pub async fn clone_custom_dashboard(
    id: &str,
    request: CloneDashboardRequest,
) -> Result<CustomDashboard, ApiError> {
    post_json(&format!("/dashboards/custom/{}/clone", id), &request).await
}

/// Export dashboard
pub async fn export_dashboard(id: &str) -> Result<String, ApiError> {
    fetch_text(&format!("/dashboards/custom/{}/export", id)).await
}

/// Import dashboard
pub async fn import_dashboard(
    request: ImportDashboardRequest,
) -> Result<CustomDashboard, ApiError> {
    post_json("/dashboards/custom/import", &request).await
}

/// Get dashboard widgets
pub async fn get_dashboard_widgets(dashboard_id: &str) -> Result<Vec<DashboardWidget>, ApiError> {
    fetch_json(&format!("/dashboards/custom/{}/widgets", dashboard_id)).await
}

/// Add widget to dashboard
pub async fn add_dashboard_widget(
    dashboard_id: &str,
    request: CreateWidgetRequest,
) -> Result<DashboardWidget, ApiError> {
    post_json(
        &format!("/dashboards/custom/{}/widgets", dashboard_id),
        &request,
    )
    .await
}

/// Update widget position
pub async fn update_widget_position(
    dashboard_id: &str,
    widget_id: &str,
    x: u32,
    y: u32,
) -> Result<(), ApiError> {
    put_json(
        &format!(
            "/dashboards/custom/{}/widgets/{}/position",
            dashboard_id, widget_id
        ),
        &serde_json::json!({"x": x, "y": y}),
    )
    .await
}

/// Remove widget from dashboard
pub async fn remove_dashboard_widget(dashboard_id: &str, widget_id: &str) -> Result<(), ApiError> {
    delete_json(&format!(
        "/dashboards/custom/{}/widgets/{}",
        dashboard_id, widget_id
    ))
    .await
}

/// Save dashboard layout
pub async fn save_dashboard_layout(
    dashboard_id: &str,
    widgets: Vec<DashboardWidget>,
) -> Result<(), ApiError> {
    put_json(
        &format!("/dashboards/custom/{}/layout", dashboard_id),
        &widgets,
    )
    .await
}

/// Get all widgets across dashboards
pub async fn get_all_widgets() -> Result<Vec<DashboardWidget>, ApiError> {
    fetch_json("/dashboards/widgets").await
}

/// Bulk delete widgets
pub async fn bulk_delete_widgets(widget_ids: Vec<String>) -> Result<(), ApiError> {
    post_json("/dashboards/widgets/bulk-delete", &widget_ids).await
}

/// Bulk move widgets
pub async fn bulk_move_widgets(
    widget_ids: Vec<String>,
    target_dashboard_id: String,
) -> Result<(), ApiError> {
    post_json(
        "/dashboards/widgets/bulk-move",
        &serde_json::json!({"widget_ids": widget_ids, "target_dashboard_id": target_dashboard_id}),
    )
    .await
}

/// Get chart templates
pub async fn get_chart_templates() -> Result<Vec<ChartTemplate>, ApiError> {
    fetch_json("/dashboards/chart-templates").await
}

/// Create chart template
pub async fn create_chart_template(
    request: CreateChartTemplateRequest,
) -> Result<ChartTemplate, ApiError> {
    post_json("/dashboards/chart-templates", &request).await
}

/// Delete chart template
pub async fn delete_chart_template(id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/dashboards/chart-templates/{}", id)).await
}

/// Export chart template
pub async fn export_chart_template(id: &str) -> Result<String, ApiError> {
    fetch_text(&format!("/dashboards/chart-templates/{}/export", id)).await
}

/// Get featured dashboards
pub async fn get_featured_dashboards() -> Result<Vec<CustomDashboard>, ApiError> {
    fetch_json("/dashboards/gallery/featured").await
}

/// Get public dashboards
pub async fn get_public_dashboards() -> Result<Vec<CustomDashboard>, ApiError> {
    fetch_json("/dashboards/gallery/public").await
}

/// Get dashboard categories
pub async fn get_dashboard_categories() -> Result<Vec<DashboardCategory>, ApiError> {
    fetch_json("/dashboards/categories").await
}

// ============================================================================
// Audit Log & Compliance Data Structures
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuditEvent {
    pub id: String,
    pub timestamp: String,
    pub event_type: String,
    pub severity: String,
    pub user: Option<String>,
    pub source_ip: Option<String>,
    pub user_agent: Option<String>,
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    pub action: String,
    pub details: serde_json::Value,
    pub success: bool,
    pub error_message: Option<String>,
    pub session_id: Option<String>,
    pub correlation_id: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuditFilter {
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub event_types: Vec<String>,
    pub users: Vec<String>,
    pub resource_types: Vec<String>,
    pub actions: Vec<String>,
    pub severity_levels: Vec<String>,
    pub success_filter: Option<bool>,
    pub source_ips: Vec<String>,
    pub search_term: Option<String>,
    pub correlation_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuditFilterOptions {
    pub event_types: Vec<String>,
    pub users: Vec<String>,
    pub resource_types: Vec<String>,
    pub actions: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuditExportRequest {
    pub format: String,
    pub filter: AuditFilter,
    pub fields: Vec<String>,
    pub include_details: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SecurityEvent {
    pub id: String,
    pub timestamp: String,
    pub event_type: String,
    pub severity: String,
    pub source_ip: String,
    pub target_user: Option<String>,
    pub target_resource: Option<String>,
    pub description: String,
    pub details: serde_json::Value,
    pub status: String,
    pub assigned_to: Option<String>,
    pub resolution: Option<String>,
    pub related_events: Vec<String>,
    pub indicators: Vec<SecurityIndicator>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SecurityIndicator {
    pub indicator_type: String,
    pub value: String,
    pub confidence: f64,
    pub description: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SecurityThreat {
    pub id: String,
    pub name: String,
    pub description: String,
    pub severity: String,
    pub affected_events: Vec<String>,
    pub detection_time: String,
    pub status: String,
    pub mitigations: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SecurityStats {
    pub total_events_24h: u64,
    pub critical_events_24h: u64,
    pub blocked_ips_24h: u64,
    pub failed_logins_24h: u64,
    pub active_threats: u64,
    pub events_by_type: std::collections::HashMap<String, u64>,
    pub events_by_severity: std::collections::HashMap<String, u64>,
    pub top_source_ips: Vec<(String, u64)>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ComplianceFramework {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub enabled: bool,
    pub total_controls: u32,
    pub passing_controls: u32,
    pub failing_controls: u32,
    pub not_applicable_controls: u32,
    pub last_assessment: String,
    pub score: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ComplianceControl {
    pub id: String,
    pub framework_id: String,
    pub control_id: String,
    pub title: String,
    pub description: String,
    pub category: String,
    pub severity: String,
    pub status: String,
    pub evidence: Vec<ComplianceEvidence>,
    pub recommendations: Vec<String>,
    pub last_checked: String,
    pub manual_override: Option<ManualOverride>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ComplianceEvidence {
    pub evidence_type: String,
    pub source: String,
    pub timestamp: String,
    pub data: serde_json::Value,
    pub verified: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ManualOverride {
    pub status: String,
    pub reason: String,
    pub user: String,
    pub timestamp: String,
    pub expires: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ComplianceReport {
    pub id: String,
    pub framework_id: String,
    pub framework_name: String,
    pub report_type: String,
    pub generated_at: String,
    pub generated_by: String,
    pub period_start: String,
    pub period_end: String,
    pub overall_score: f64,
    pub status: String,
    pub download_url: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GenerateReportRequest {
    pub framework_id: String,
    pub report_type: String,
    pub period_start: String,
    pub period_end: String,
    pub include_evidence: bool,
    pub format: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Investigation {
    pub id: String,
    pub title: String,
    pub description: String,
    pub status: String,
    pub severity: String,
    pub created_at: String,
    pub updated_at: String,
    pub created_by: String,
    pub assigned_to: Option<String>,
    pub related_events: Vec<String>,
    pub timeline: Vec<TimelineEntry>,
    pub findings: Vec<Finding>,
    pub artifacts: Vec<Artifact>,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TimelineEntry {
    pub id: String,
    pub timestamp: String,
    pub event_type: String,
    pub description: String,
    pub source: String,
    pub details: serde_json::Value,
    pub important: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Finding {
    pub id: String,
    pub title: String,
    pub description: String,
    pub finding_type: String,
    pub severity: String,
    pub evidence: Vec<String>,
    pub created_at: String,
    pub created_by: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Artifact {
    pub id: String,
    pub name: String,
    pub artifact_type: String,
    pub description: String,
    pub size_bytes: u64,
    pub hash: String,
    pub collected_at: String,
    pub collected_by: String,
    pub download_url: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CreateInvestigationRequest {
    pub title: String,
    pub description: String,
    pub severity: String,
    pub related_events: Vec<String>,
    pub tags: Vec<String>,
}

// Audit Log API Functions
pub async fn get_audit_events(
    filter: AuditFilter,
    page: u64,
    page_size: u64,
) -> Result<(Vec<AuditEvent>, u64), ApiError> {
    let request = serde_json::json!({
        "filter": filter,
        "page": page,
        "page_size": page_size
    });
    post_json("/audit/events/search", &request).await
}

pub async fn get_audit_filter_options() -> Result<AuditFilterOptions, ApiError> {
    fetch_json("/audit/filter-options").await
}

pub async fn export_audit_events(request: AuditExportRequest) -> Result<String, ApiError> {
    post_json("/audit/export", &request).await
}

// Security Events API Functions
pub async fn get_security_events(time_range: String) -> Result<Vec<SecurityEvent>, ApiError> {
    fetch_json(&format!("/security/events?range={}", time_range)).await
}

pub async fn update_security_event_status(
    event_id: String,
    status: String,
) -> Result<(), ApiError> {
    put_json(
        &format!("/security/events/{}/status", event_id),
        &serde_json::json!({"status": status}),
    )
    .await
}

pub async fn get_active_threats() -> Result<Vec<SecurityThreat>, ApiError> {
    fetch_json("/security/threats/active").await
}

pub async fn get_security_stats() -> Result<SecurityStats, ApiError> {
    fetch_json("/security/stats").await
}

pub async fn block_ip_address(ip: String) -> Result<(), ApiError> {
    post_json("/security/block-ip", &serde_json::json!({"ip": ip})).await
}

// Compliance API Functions
pub async fn get_compliance_frameworks() -> Result<Vec<ComplianceFramework>, ApiError> {
    fetch_json("/compliance/frameworks").await
}

pub async fn get_compliance_controls(
    framework_id: String,
) -> Result<Vec<ComplianceControl>, ApiError> {
    fetch_json(&format!("/compliance/frameworks/{}/controls", framework_id)).await
}

pub async fn run_compliance_assessment(framework_id: String) -> Result<(), ApiError> {
    post_json(
        &format!("/compliance/frameworks/{}/assess", framework_id),
        &(),
    )
    .await
}

pub async fn set_control_override(
    control_id: String,
    override_data: ManualOverride,
) -> Result<(), ApiError> {
    put_json(
        &format!("/compliance/controls/{}/override", control_id),
        &override_data,
    )
    .await
}

pub async fn get_compliance_reports() -> Result<Vec<ComplianceReport>, ApiError> {
    fetch_json("/compliance/reports").await
}

pub async fn generate_compliance_report(
    request: GenerateReportRequest,
) -> Result<ComplianceReport, ApiError> {
    post_json("/compliance/reports/generate", &request).await
}

// Forensics API Functions
pub async fn get_investigations() -> Result<Vec<Investigation>, ApiError> {
    fetch_json("/forensics/investigations").await
}

pub async fn get_investigation(id: String) -> Result<Investigation, ApiError> {
    fetch_json(&format!("/forensics/investigations/{}", id)).await
}

pub async fn create_investigation(
    request: CreateInvestigationRequest,
) -> Result<Investigation, ApiError> {
    post_json("/forensics/investigations", &request).await
}

pub async fn update_investigation_status(id: String, status: String) -> Result<(), ApiError> {
    put_json(
        &format!("/forensics/investigations/{}/status", id),
        &serde_json::json!({"status": status}),
    )
    .await
}

pub async fn add_investigation_finding(
    investigation_id: String,
    finding: Finding,
) -> Result<(), ApiError> {
    post_json(
        &format!("/forensics/investigations/{}/findings", investigation_id),
        &finding,
    )
    .await
}

pub async fn collect_investigation_artifact(
    investigation_id: String,
    request: serde_json::Value,
) -> Result<Artifact, ApiError> {
    post_json(
        &format!(
            "/forensics/investigations/{}/artifacts/collect",
            investigation_id
        ),
        &request,
    )
    .await
}

// ============================================================================
// MFA Management Data Structures
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MfaStatus {
    pub enabled: bool,
    pub methods: Vec<MfaMethod>,
    pub backup_codes_remaining: u32,
    pub last_verified: Option<String>,
    pub trusted_devices: Vec<TrustedDevice>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MfaMethod {
    pub id: String,
    pub method_type: String,
    pub name: String,
    pub enabled: bool,
    pub registered_at: String,
    pub last_used: Option<String>,
    pub is_primary: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TotpSetup {
    pub secret: String,
    pub qr_code_url: String,
    pub backup_codes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrustedDevice {
    pub id: String,
    pub name: String,
    pub device_type: String,
    pub browser: String,
    pub os: String,
    pub ip_address: String,
    pub location: Option<String>,
    pub trusted_at: String,
    pub expires_at: String,
    pub last_used: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MfaEnforcementPolicy {
    pub global_enforcement: bool,
    pub grace_period_days: u32,
    pub required_for_roles: Vec<String>,
    pub allowed_methods: Vec<String>,
    pub require_backup_codes: bool,
    pub max_trusted_devices: u32,
    pub trusted_device_expiry_days: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct UserMfaStatus {
    pub user_id: String,
    pub username: String,
    pub email: String,
    pub mfa_enabled: bool,
    pub methods_count: u32,
    pub last_mfa_activity: Option<String>,
    pub enforcement_status: String,
}

// MFA API Functions
pub async fn get_mfa_status() -> Result<MfaStatus, ApiError> {
    fetch_json("/auth/mfa/status").await
}

pub async fn initiate_totp_setup() -> Result<TotpSetup, ApiError> {
    post_json("/auth/mfa/totp/setup", &()).await
}

pub async fn verify_totp_setup(code: String) -> Result<Vec<String>, ApiError> {
    post_json("/auth/mfa/totp/verify", &serde_json::json!({"code": code})).await
}

pub async fn register_webauthn_credential(name: String) -> Result<(), ApiError> {
    post_json(
        "/auth/mfa/webauthn/register",
        &serde_json::json!({"name": name}),
    )
    .await
}

pub async fn regenerate_mfa_backup_codes() -> Result<Vec<String>, ApiError> {
    post_json("/auth/mfa/backup-codes/regenerate", &()).await
}

pub async fn revoke_trusted_device(device_id: String) -> Result<(), ApiError> {
    delete_json(&format!("/auth/mfa/trusted-devices/{}", device_id)).await
}

pub async fn disable_mfa(method_id: String, verification_code: String) -> Result<(), ApiError> {
    post_json(
        &format!("/auth/mfa/methods/{}/disable", method_id),
        &serde_json::json!({"code": verification_code}),
    )
    .await
}

pub async fn set_primary_mfa_method(method_id: String) -> Result<(), ApiError> {
    put_json(&format!("/auth/mfa/methods/{}/primary", method_id), &()).await
}

pub async fn get_mfa_enforcement_policy() -> Result<MfaEnforcementPolicy, ApiError> {
    fetch_json("/auth/mfa/policy").await
}

pub async fn update_mfa_enforcement_policy(policy: MfaEnforcementPolicy) -> Result<(), ApiError> {
    put_json("/auth/mfa/policy", &policy).await
}

pub async fn get_users_mfa_status() -> Result<Vec<UserMfaStatus>, ApiError> {
    fetch_json("/auth/mfa/users").await
}

pub async fn reset_user_mfa(user_id: String) -> Result<(), ApiError> {
    post_json(&format!("/auth/mfa/users/{}/reset", user_id), &()).await
}

pub async fn enforce_user_mfa(user_id: String) -> Result<(), ApiError> {
    post_json(&format!("/auth/mfa/users/{}/enforce", user_id), &()).await
}

// =============================================================================
// Storage Management API
// =============================================================================

// Storage Pool Info (for volume management)
/// Storage pool as the Storage page shows it. The API returns
/// `storage::StoragePool` (`storage_type`, sizes in GB, `enabled`), which
/// this used to fail to deserialize ("missing field `pool_type`"), so the
/// page broke as soon as one pool existed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "StoragePoolWire")]
pub struct StoragePoolInfo {
    pub id: String,
    pub name: String,
    pub pool_type: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub status: String,
    pub path: Option<String>,
}

#[derive(Deserialize)]
struct StoragePoolWire {
    id: String,
    name: String,
    storage_type: String,
    #[serde(default)]
    path: Option<String>,
    /// GB
    #[serde(default)]
    available: u64,
    /// GB
    #[serde(default)]
    total: u64,
    #[serde(default = "default_true")]
    enabled: bool,
}

fn default_true() -> bool {
    true
}

impl From<StoragePoolWire> for StoragePoolInfo {
    fn from(w: StoragePoolWire) -> Self {
        const GB: u64 = 1024 * 1024 * 1024;
        Self {
            id: w.id,
            name: w.name,
            pool_type: w.storage_type,
            total_bytes: w.total * GB,
            used_bytes: w.total.saturating_sub(w.available) * GB,
            available_bytes: w.available * GB,
            status: if w.enabled { "active" } else { "disabled" }.to_string(),
            path: w.path,
        }
    }
}

// Disk Management Types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    pub used_bytes: u64,
    pub model: String,
    pub serial: String,
    pub disk_type: String,
    pub interface: String,
    pub partitions: Vec<PartitionInfo>,
    pub smart_status: String,
    pub temperature: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartitionInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    pub used_bytes: u64,
    pub filesystem: String,
    pub mount_point: Option<String>,
    pub label: Option<String>,
}

// Volume Management Types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeInfo {
    pub id: String,
    pub name: String,
    pub pool_id: String,
    pub pool_name: String,
    pub size_bytes: u64,
    pub used_bytes: u64,
    pub volume_type: String,
    pub format: String,
    pub attached_to: Option<String>,
    pub attached_name: Option<String>,
    pub snapshots: u32,
    pub created_at: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeSnapshot {
    pub id: String,
    pub name: String,
    pub volume_id: String,
    pub size_bytes: u64,
    pub created_at: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VolumeFilters {
    pub search: String,
    pub pool_id: Option<String>,
    pub volume_type: Option<String>,
    pub status: Option<String>,
    pub attached_only: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateVolumeForm {
    pub name: String,
    pub pool_id: String,
    pub size_gb: u64,
    pub volume_type: String,
    pub format: String,
    pub thin_provisioned: bool,
    pub description: String,
}

// SMART Monitoring Types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartDiskInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub model: String,
    pub serial: String,
    pub firmware: String,
    pub capacity_bytes: u64,
    pub disk_type: String,
    pub interface: String,
    pub smart_enabled: bool,
    pub smart_status: SmartStatus,
    pub temperature: Option<u32>,
    pub power_on_hours: Option<u64>,
    pub power_cycle_count: Option<u64>,
    pub attributes: Vec<SmartAttribute>,
    pub last_test: Option<SmartTestResult>,
    pub health_score: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SmartStatus {
    Healthy,
    Warning,
    Critical,
    Unknown,
}

impl SmartStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            SmartStatus::Healthy => "healthy",
            SmartStatus::Warning => "warning",
            SmartStatus::Critical => "critical",
            SmartStatus::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartAttribute {
    pub id: u8,
    pub name: String,
    pub value: u64,
    pub worst: u64,
    pub threshold: u64,
    pub raw_value: String,
    pub status: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartTestResult {
    pub test_type: String,
    pub status: String,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub lifetime_hours: u64,
    pub error_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartAlert {
    pub id: String,
    pub disk_id: String,
    pub disk_name: String,
    pub alert_type: String,
    pub severity: String,
    pub message: String,
    pub attribute_id: Option<u8>,
    pub attribute_name: Option<String>,
    pub created_at: String,
    pub acknowledged: bool,
}

// Disk Management API
pub async fn get_disk_list() -> Result<Vec<DiskInfo>, ApiError> {
    fetch_json("/storage/disks").await
}

pub async fn get_disk_details(disk_id: &str) -> Result<DiskInfo, ApiError> {
    fetch_json(&format!("/storage/disks/{}", disk_id)).await
}

pub async fn get_disk_partitions(disk_id: &str) -> Result<Vec<PartitionInfo>, ApiError> {
    fetch_json(&format!("/storage/disks/{}/partitions", disk_id)).await
}

pub async fn create_partition(
    disk_id: &str,
    size_bytes: u64,
    filesystem: &str,
) -> Result<PartitionInfo, ApiError> {
    post_json(
        &format!("/storage/disks/{}/partitions", disk_id),
        &serde_json::json!({
            "size_bytes": size_bytes,
            "filesystem": filesystem
        }),
    )
    .await
}

pub async fn delete_partition(disk_id: &str, partition_id: &str) -> Result<(), ApiError> {
    delete_json(&format!(
        "/storage/disks/{}/partitions/{}",
        disk_id, partition_id
    ))
    .await
}

pub async fn format_partition(
    disk_id: &str,
    partition_id: &str,
    filesystem: &str,
) -> Result<(), ApiError> {
    post_json(
        &format!(
            "/storage/disks/{}/partitions/{}/format",
            disk_id, partition_id
        ),
        &serde_json::json!({"filesystem": filesystem}),
    )
    .await
}

pub async fn mount_partition(
    disk_id: &str,
    partition_id: &str,
    mount_point: &str,
) -> Result<(), ApiError> {
    post_json(
        &format!(
            "/storage/disks/{}/partitions/{}/mount",
            disk_id, partition_id
        ),
        &serde_json::json!({"mount_point": mount_point}),
    )
    .await
}

pub async fn unmount_partition(disk_id: &str, partition_id: &str) -> Result<(), ApiError> {
    post_json(
        &format!(
            "/storage/disks/{}/partitions/{}/unmount",
            disk_id, partition_id
        ),
        &(),
    )
    .await
}

// Volume Management API
pub async fn get_volumes(filters: VolumeFilters) -> Result<Vec<VolumeInfo>, ApiError> {
    let mut query_params = Vec::new();
    if !filters.search.is_empty() {
        query_params.push(format!("search={}", filters.search));
    }
    if let Some(ref pool_id) = filters.pool_id {
        if !pool_id.is_empty() {
            query_params.push(format!("pool_id={}", pool_id));
        }
    }
    if let Some(ref volume_type) = filters.volume_type {
        if !volume_type.is_empty() {
            query_params.push(format!("type={}", volume_type));
        }
    }
    if let Some(ref status) = filters.status {
        if !status.is_empty() {
            query_params.push(format!("status={}", status));
        }
    }
    if filters.attached_only {
        query_params.push("attached=true".to_string());
    }

    let query_string = if query_params.is_empty() {
        String::new()
    } else {
        format!("?{}", query_params.join("&"))
    };

    fetch_json(&format!("/storage/volumes{}", query_string)).await
}

pub async fn get_volume(volume_id: &str) -> Result<VolumeInfo, ApiError> {
    fetch_json(&format!("/storage/volumes/{}", volume_id)).await
}

pub async fn create_new_volume(form: CreateVolumeForm) -> Result<VolumeInfo, ApiError> {
    post_json(
        "/storage/volumes",
        &serde_json::json!({
            "name": form.name,
            "pool_id": form.pool_id,
            "size_gb": form.size_gb,
            "volume_type": form.volume_type,
            "format": form.format,
            "thin_provisioned": form.thin_provisioned,
            "description": form.description
        }),
    )
    .await
}

pub async fn resize_volume_api(volume_id: &str, new_size_gb: u64) -> Result<(), ApiError> {
    put_json(
        &format!("/storage/volumes/{}/resize", volume_id),
        &serde_json::json!({"size_gb": new_size_gb}),
    )
    .await
}

pub async fn delete_volume_api(volume_id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/storage/volumes/{}", volume_id)).await
}

pub async fn attach_volume(
    volume_id: &str,
    vm_id: &str,
    device: Option<&str>,
) -> Result<(), ApiError> {
    post_json(
        &format!("/storage/volumes/{}/attach", volume_id),
        &serde_json::json!({
            "vm_id": vm_id,
            "device": device
        }),
    )
    .await
}

pub async fn detach_volume(volume_id: &str) -> Result<(), ApiError> {
    post_json(&format!("/storage/volumes/{}/detach", volume_id), &()).await
}

pub async fn get_volume_snapshots(volume_id: &str) -> Result<Vec<VolumeSnapshot>, ApiError> {
    fetch_json(&format!("/storage/volumes/{}/snapshots", volume_id)).await
}

pub async fn create_volume_snapshot(
    volume_id: &str,
    name: &str,
    description: Option<&str>,
) -> Result<VolumeSnapshot, ApiError> {
    post_json(
        &format!("/storage/volumes/{}/snapshots", volume_id),
        &serde_json::json!({
            "name": name,
            "description": description
        }),
    )
    .await
}

pub async fn delete_volume_snapshot(volume_id: &str, snapshot_id: &str) -> Result<(), ApiError> {
    delete_json(&format!(
        "/storage/volumes/{}/snapshots/{}",
        volume_id, snapshot_id
    ))
    .await
}

pub async fn restore_volume_snapshot(volume_id: &str, snapshot_id: &str) -> Result<(), ApiError> {
    post_json(
        &format!(
            "/storage/volumes/{}/snapshots/{}/restore",
            volume_id, snapshot_id
        ),
        &(),
    )
    .await
}

// SMART Monitoring API
pub async fn get_smart_disk_info() -> Result<Vec<SmartDiskInfo>, ApiError> {
    fetch_json("/storage/smart/disks").await
}

pub async fn get_smart_disk_details(disk_id: &str) -> Result<SmartDiskInfo, ApiError> {
    fetch_json(&format!("/storage/smart/disks/{}", disk_id)).await
}

pub async fn get_smart_alerts() -> Result<Vec<SmartAlert>, ApiError> {
    fetch_json("/storage/smart/alerts").await
}

pub async fn acknowledge_smart_alert(alert_id: &str) -> Result<(), ApiError> {
    put_json(
        &format!("/storage/smart/alerts/{}/acknowledge", alert_id),
        &(),
    )
    .await
}

pub async fn start_smart_test(disk_id: &str, test_type: &str) -> Result<(), ApiError> {
    post_json(
        &format!("/storage/smart/disks/{}/test", disk_id),
        &serde_json::json!({"test_type": test_type}),
    )
    .await
}

pub async fn get_smart_test_status(disk_id: &str) -> Result<serde_json::Value, ApiError> {
    fetch_json(&format!("/storage/smart/disks/{}/test/status", disk_id)).await
}

pub async fn enable_smart_monitoring(disk_id: &str) -> Result<(), ApiError> {
    post_json(&format!("/storage/smart/disks/{}/enable", disk_id), &()).await
}

pub async fn disable_smart_monitoring(disk_id: &str) -> Result<(), ApiError> {
    post_json(&format!("/storage/smart/disks/{}/disable", disk_id), &()).await
}

// Storage Migration API
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MigrationStoragePool {
    pub id: String,
    pub name: String,
    pub pool_type: String,
    pub node: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub status: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MigratableResource {
    pub id: String,
    pub name: String,
    pub resource_type: String, // vm, container, volume
    pub size_bytes: u64,
    pub current_pool: String,
    pub status: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateMigrationRequest {
    pub source_pool_id: String,
    pub target_pool_id: String,
    pub resource_ids: Vec<String>,
    pub options: MigrationOptions,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MigrationOptions {
    pub live_migrate: bool,
    pub verify_data: bool,
    pub compress_transfer: bool,
    pub bandwidth_limit_mbps: Option<u32>,
    pub schedule_time: Option<String>,
    pub delete_source_after: bool,
    pub priority: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StorageMigrationJob {
    pub id: String,
    pub source_pool: String,
    pub target_pool: String,
    pub status: String, // pending, running, paused, completed, failed, cancelled
    pub progress: f32,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub resources: Vec<MigrationResourceStatus>,
    pub options: MigrationOptions,
    pub error_message: Option<String>,
    pub bytes_transferred: u64,
    pub bytes_total: u64,
    pub transfer_rate_mbps: Option<f32>,
    pub estimated_completion: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MigrationResourceStatus {
    pub resource_id: String,
    pub resource_name: String,
    pub status: String, // pending, migrating, verifying, completed, failed
    pub progress: f32,
    pub error_message: Option<String>,
}

pub async fn get_migration_storage_pools() -> Result<Vec<MigrationStoragePool>, ApiError> {
    fetch_json("/storage/pools").await
}

pub async fn get_migratable_resources(pool_id: &str) -> Result<Vec<MigratableResource>, ApiError> {
    fetch_json(&format!("/storage/pools/{}/resources", pool_id)).await
}

pub async fn create_storage_migration(
    request: CreateMigrationRequest,
) -> Result<StorageMigrationJob, ApiError> {
    post_json("/storage/migrations", &request).await
}

pub async fn get_storage_migrations() -> Result<Vec<StorageMigrationJob>, ApiError> {
    fetch_json("/storage/migrations").await
}

pub async fn get_storage_migration(migration_id: &str) -> Result<StorageMigrationJob, ApiError> {
    fetch_json(&format!("/storage/migrations/{}", migration_id)).await
}

pub async fn pause_storage_migration(migration_id: &str) -> Result<(), ApiError> {
    post_json(&format!("/storage/migrations/{}/pause", migration_id), &()).await
}

pub async fn resume_storage_migration(migration_id: &str) -> Result<(), ApiError> {
    post_json(&format!("/storage/migrations/{}/resume", migration_id), &()).await
}

pub async fn cancel_storage_migration(migration_id: &str) -> Result<(), ApiError> {
    post_json(&format!("/storage/migrations/{}/cancel", migration_id), &()).await
}

pub async fn retry_storage_migration(migration_id: &str) -> Result<(), ApiError> {
    post_json(&format!("/storage/migrations/{}/retry", migration_id), &()).await
}

pub async fn delete_storage_migration(migration_id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/storage/migrations/{}", migration_id)).await
}

// Storage Pools API (generic)
pub async fn get_storage_pools() -> Result<Vec<StoragePoolInfo>, ApiError> {
    fetch_json("/storage/pools").await
}

pub async fn get_storage_pool(pool_id: &str) -> Result<StoragePoolInfo, ApiError> {
    fetch_json(&format!("/storage/pools/{}", pool_id)).await
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateStoragePoolRequest {
    pub name: String,
    pub pool_type: String,
    pub path: Option<String>,
    pub config: serde_json::Value,
}

pub async fn create_storage_pool(
    request: CreateStoragePoolRequest,
) -> Result<StoragePoolInfo, ApiError> {
    post_json("/storage/pools", &request).await
}

pub async fn delete_storage_pool(pool_id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/storage/pools/{}", pool_id)).await
}

pub async fn get_storage_pool_health(pool_id: &str) -> Result<serde_json::Value, ApiError> {
    fetch_json(&format!("/storage/pools/{}/health", pool_id)).await
}

pub async fn scan_storage_pool(pool_id: &str) -> Result<(), ApiError> {
    post_json(&format!("/storage/pools/{}/scan", pool_id), &()).await
}

// ---------------------------------------------------------------------
// Container images / volumes / networks / stacks / WebSocket tickets
// (Portainer-parity container management - see PORTAINER_COMPARISON.md)
// ---------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImageInfo {
    pub id: String,
    pub repo_tags: Vec<String>,
    pub size: i64,
    pub created: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContainerVolumeInfo {
    pub name: String,
    pub driver: String,
    pub mountpoint: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContainerNetworkInfo {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub scope: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StackServiceSummary {
    pub service_name: String,
    pub container_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StackSummary {
    pub id: String,
    pub name: String,
    pub runtime: String,
    pub services: Vec<StackServiceSummary>,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default)]
    pub unsupported_features: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WsTicketResponse {
    pub ticket: String,
    pub ws_path: String,
}

pub async fn get_images(runtime: &str) -> Result<Vec<ImageInfo>, ApiError> {
    fetch_json(&format!("/images?runtime={}", runtime)).await
}

#[derive(Serialize)]
struct PullImageBody<'a> {
    name: &'a str,
    tag: &'a str,
    runtime: &'a str,
}

#[derive(Deserialize)]
pub struct PullImageResponse {
    pub lines: Vec<serde_json::Value>,
}

pub async fn pull_image(name: &str, tag: &str, runtime: &str) -> Result<(), ApiError> {
    let _: PullImageResponse =
        post_json("/images/pull", &PullImageBody { name, tag, runtime }).await?;
    Ok(())
}

pub async fn remove_image(id: &str, runtime: &str) -> Result<(), ApiError> {
    delete_json(&format!("/images/{}?runtime={}", id, runtime)).await
}

pub async fn get_container_volumes(runtime: &str) -> Result<Vec<ContainerVolumeInfo>, ApiError> {
    fetch_json(&format!("/volumes?runtime={}", runtime)).await
}

#[derive(Serialize)]
struct CreateVolumeBody<'a> {
    name: &'a str,
    runtime: &'a str,
}

pub async fn create_volume(name: &str, runtime: &str) -> Result<ContainerVolumeInfo, ApiError> {
    post_json("/volumes", &CreateVolumeBody { name, runtime }).await
}

pub async fn remove_volume(name: &str, runtime: &str) -> Result<(), ApiError> {
    delete_json(&format!("/volumes/{}?runtime={}", name, runtime)).await
}

pub async fn get_container_networks(runtime: &str) -> Result<Vec<ContainerNetworkInfo>, ApiError> {
    fetch_json(&format!("/container-networks?runtime={}", runtime)).await
}

#[derive(Serialize)]
struct CreateNetworkBody<'a> {
    name: &'a str,
    runtime: &'a str,
}

pub async fn create_container_network(name: &str, runtime: &str) -> Result<String, ApiError> {
    post_json("/container-networks", &CreateNetworkBody { name, runtime }).await
}

pub async fn remove_container_network(id: &str, runtime: &str) -> Result<(), ApiError> {
    delete_json(&format!("/container-networks/{}?runtime={}", id, runtime)).await
}

pub async fn get_stacks() -> Result<Vec<StackSummary>, ApiError> {
    fetch_json("/stacks").await
}

#[derive(Serialize)]
struct DeployStackBody<'a> {
    name: &'a str,
    compose_yaml: &'a str,
    runtime: &'a str,
}

pub async fn deploy_stack(
    name: &str,
    compose_yaml: &str,
    runtime: &str,
) -> Result<StackSummary, ApiError> {
    post_json(
        "/stacks",
        &DeployStackBody {
            name,
            compose_yaml,
            runtime,
        },
    )
    .await
}

pub async fn start_stack(id: &str) -> Result<(), ApiError> {
    post_empty(&format!("/stacks/{}/start", id)).await
}

pub async fn stop_stack(id: &str) -> Result<(), ApiError> {
    post_empty(&format!("/stacks/{}/stop", id)).await
}

pub async fn remove_stack(id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/stacks/{}", id)).await
}

pub async fn get_container_logs_ticket(
    container_id: &str,
    runtime: &str,
) -> Result<WsTicketResponse, ApiError> {
    post_json(
        &format!(
            "/containers/{}/logs-ticket?runtime={}",
            container_id, runtime
        ),
        &(),
    )
    .await
}

#[derive(Serialize)]
struct ExecTicketBody<'a> {
    command: Vec<&'a str>,
    runtime: &'a str,
}

pub async fn get_container_exec_ticket(
    container_id: &str,
    command: &str,
    runtime: &str,
) -> Result<WsTicketResponse, ApiError> {
    let cmd: Vec<&str> = if command.trim().is_empty() {
        vec!["/bin/sh"]
    } else {
        command.split_whitespace().collect()
    };
    post_json(
        &format!("/containers/{}/exec-ticket", container_id),
        &ExecTicketBody {
            command: cmd,
            runtime,
        },
    )
    .await
}

pub async fn get_container_events_ticket(runtime: &str) -> Result<WsTicketResponse, ApiError> {
    post_json(
        &format!("/containers/events-ticket?runtime={}", runtime),
        &(),
    )
    .await
}

/// Build an absolute `ws://`/`wss://` URL for a relative API path such as
/// the `ws_path` returned by the ticket endpoints above - matching how the
/// VM console page derives its WebSocket URL from the current page origin.
pub fn resolve_ws_url(ws_path: &str) -> String {
    let window = web_sys::window().expect("no window");
    let location = window.location();
    let protocol = if location.protocol().unwrap_or_default() == "https:" {
        "wss:"
    } else {
        "ws:"
    };
    let host = location.host().unwrap_or_default();
    format!("{protocol}//{host}{ws_path}")
}

#[cfg(test)]
mod tests {
    //! Contract tests: the UI structs must deserialize the JSON horcrux-api
    //! actually sends. Each sample below is the real serialized shape of the
    //! named server type.
    use super::*;

    #[test]
    fn error_text_includes_the_api_details() {
        let body = r#"{"status":500,"error":"INTERNAL_ERROR","message":"An internal server error occurred","details":"nftables: permission denied"}"#;
        assert_eq!(
            error_text(500, body),
            "HTTP 500: An internal server error occurred: nftables: permission denied"
        );
        assert_eq!(error_text(404, ""), "HTTP 404");
        assert_eq!(error_text(502, "bad gateway"), "HTTP 502: bad gateway");
    }

    #[test]
    fn empty_success_body_decodes_as_unit() {
        let r: Result<(), ApiError> = decode_body("");
        assert!(r.is_ok());
        let v: serde_json::Value = decode_body("{\"id\":\"x\"}").unwrap();
        assert_eq!(v["id"], "x");
    }

    #[test]
    fn template_and_storage_pool_read_the_api_shapes() {
        let t: VmTemplate = serde_json::from_str(r#"{"id":"template-1","name":"t","description":null,"source_vm_id":"vm1","created":1792200000,"disk_path":"/var/lib/horcrux/vms/templates/template-1.qcow2","storage_type":"directory","memory":512,"cpus":2,"os_type":"linux","cloudinit_template":null}"#).unwrap();
        assert_eq!(t.source_vm_id, "vm1");
        assert!(t.storage_location.ends_with("template-1.qcow2"));
        let p: StoragePoolInfo = serde_json::from_str(r#"{"id":"p1","name":"dir","storage_type":"directory","path":"/srv","available":10,"total":20,"enabled":true}"#).unwrap();
        assert_eq!(p.pool_type, "directory");
        assert_eq!(p.used_bytes, 10 * 1024 * 1024 * 1024);
        let req = serde_json::to_value(CreateTemplateRequest {
            name: "n".into(),
            description: None,
            source_vm_id: "vm1".into(),
        })
        .unwrap();
        assert_eq!(req["vm_id"], "vm1");
        assert_eq!(
            clone_template_body("Web Server 2")["new_vm_id"],
            "web-server-2"
        );
    }

    /// horcrux-api `cluster::node::Node` (GET /api/cluster/nodes).
    const NODES_JSON: &str = r#"[
        {"id":1,"name":"node-primary","ip":"10.0.0.1","status":"online","priority":300,
         "is_local":true,"architecture":"x86_64","cpu_cores":8,"memory_total":17179869184},
        {"id":2,"name":"node-arm","ip":"10.0.0.2","status":"offline","priority":50,
         "is_local":false,"architecture":"aarch64","cpu_cores":4,"memory_total":4294967296}
    ]"#;

    #[test]
    fn cluster_nodes_deserialize_from_real_api_shape() {
        let nodes: Vec<ClusterNode> = serde_json::from_str(NODES_JSON).expect("must deserialize");
        assert_eq!(nodes.len(), 2);
        let n = &nodes[0];
        assert_eq!(n.name, "node-primary");
        assert_eq!(n.ip, "10.0.0.1");
        assert_eq!(n.status, "online");
        assert_eq!(n.architecture, "x86_64");
        // Top-level fields, not a nested `resources` object the API never sends.
        assert_eq!(n.cpu_cores, 8);
        assert_eq!(n.memory_total, 17_179_869_184);
        assert_eq!(n.priority, 300);
        assert!(n.is_local);
        assert_eq!(nodes[1].priority, 50);
        assert_eq!(nodes[1].cpu_cores, 4);
    }

    #[test]
    fn cluster_node_usage_is_unknown_when_api_omits_or_nulls_it() {
        let nodes: Vec<ClusterNode> = serde_json::from_str(NODES_JSON).unwrap();
        assert_eq!(nodes[0].cpu_usage, None);
        assert_eq!(nodes[0].memory_used, None);
        assert_eq!(nodes[0].usage_label(), "CPU unknown, RAM unknown");
        let n: ClusterNode = serde_json::from_str(
            r#"{"name":"n","ip":"10.0.0.9","status":"online","architecture":"x86_64",
                "cpu_cores":2,"memory_total":1073741824,
                "cpu_usage":null,"memory_used":null,"last_seen":null}"#,
        )
        .unwrap();
        assert_eq!(n.cpu_usage, None);
        assert_eq!(n.load_score(), None);
    }

    #[test]
    fn cluster_node_live_usage_deserializes() {
        // horcrux-api `cluster::node::Node` with live usage filled in by the
        // node heartbeat.
        let n: ClusterNode = serde_json::from_str(
            r#"{"id":3,"name":"n","ip":"10.0.0.9","status":"online","priority":100,
                "is_local":false,"architecture":"x86_64","cpu_cores":8,
                "memory_total":17179869184,"cpu_usage":12.5,
                "memory_used":4294967296,"last_seen":1760000000}"#,
        )
        .unwrap();
        assert_eq!(n.cpu_usage, Some(12.5));
        assert_eq!(n.memory_used, Some(4_294_967_296));
        assert_eq!(n.last_seen, Some(1_760_000_000));
        assert_eq!(n.memory_used_fraction(), Some(0.25));
        assert_eq!(n.usage_label(), "12.5% CPU, 4.0 / 16.0 GB RAM");
    }

    fn usage_node(name: &str, status: &str, cpu: Option<f64>, used_gb: Option<u64>) -> ClusterNode {
        ClusterNode {
            id: 0,
            name: name.into(),
            ip: "10.0.0.1".into(),
            status: status.into(),
            priority: 100,
            is_local: false,
            architecture: "x86_64".into(),
            cpu_cores: 8,
            memory_total: 16 << 30,
            cpu_usage: cpu,
            memory_used: used_gb.map(|g| g << 30),
            last_seen: None,
        }
    }

    #[test]
    fn migration_target_is_least_loaded_reporting_node() {
        let nodes = vec![
            usage_node("busy", "online", Some(90.0), Some(14)),
            usage_node("idle", "online", Some(5.0), Some(2)),
            usage_node("mid", "online", Some(40.0), Some(8)),
        ];
        assert_eq!(pick_migration_target(&nodes, None).as_deref(), Some("idle"));
        // Never the node the resource already lives on.
        assert_eq!(
            pick_migration_target(&nodes, Some("idle")).as_deref(),
            Some("mid")
        );
    }

    #[test]
    fn migration_target_never_picks_unknown_or_offline_nodes() {
        // The old picker treated missing usage as 0 and so always chose
        // whichever node it knew nothing about.
        let nodes = vec![
            usage_node("unknown", "online", None, None),
            usage_node("half-known", "online", Some(1.0), None),
            usage_node("down", "offline", Some(0.0), Some(0)),
            usage_node("loaded", "online", Some(70.0), Some(12)),
        ];
        assert_eq!(
            pick_migration_target(&nodes, None).as_deref(),
            Some("loaded")
        );
        let none_known = vec![usage_node("unknown", "online", None, None)];
        assert_eq!(pick_migration_target(&none_known, None), None);
    }

    #[test]
    fn migration_target_tie_breaks_on_placement_weight() {
        let mut a = usage_node("a", "online", Some(10.0), Some(4));
        let mut b = usage_node("b", "online", Some(10.0), Some(4));
        a.priority = 100;
        b.priority = 500;
        assert_eq!(pick_migration_target(&[a, b], None).as_deref(), Some("b"));
    }

    #[test]
    fn cluster_node_priority_defaults_for_older_servers() {
        let n: ClusterNode = serde_json::from_str(
            r#"{"name":"n","ip":"10.0.0.9","status":"unknown","architecture":"x86_64",
                "cpu_cores":2,"memory_total":1073741824}"#,
        )
        .unwrap();
        assert_eq!(n.priority, 100);
    }

    #[test]
    fn cluster_architecture_deserializes_from_real_api_shape() {
        // horcrux-api `cluster::ArchitectureSummary` (GET /api/cluster/architecture).
        let json = r#"{"x86_64_nodes":2,"aarch64_nodes":1,"other_nodes":1,"is_mixed":true}"#;
        let arch: ClusterArchitecture = serde_json::from_str(json).expect("must deserialize");
        assert_eq!(arch.x86_64_nodes, 2);
        assert_eq!(arch.aarch64_nodes, 1);
        assert_eq!(arch.other_nodes, 1);
        assert!(arch.is_mixed);
        assert_eq!(arch.total_nodes(), 4);
    }

    #[test]
    fn add_node_request_serializes_what_the_api_accepts() {
        let body = serde_json::to_value(AddNodeRequest {
            address: "192.168.1.100".into(),
            architecture: Some("aarch64".into()),
            priority: Some(200),
        })
        .unwrap();
        assert_eq!(
            body,
            serde_json::json!({"address":"192.168.1.100","architecture":"aarch64","priority":200})
        );
        let body = serde_json::to_value(AddNodeRequest {
            address: "10.0.0.5".into(),
            architecture: None,
            priority: None,
        })
        .unwrap();
        assert_eq!(body, serde_json::json!({"address":"10.0.0.5"}));
    }

    #[test]
    fn find_node_request_serializes_what_the_api_accepts() {
        // horcrux-api main.rs `FindNodeRequest {architecture, memory_mb, cpu_cores, min_priority}`.
        let body = serde_json::to_value(FindNodeRequest {
            architecture: "x86_64".into(),
            memory_mb: 2048,
            cpu_cores: 2,
            min_priority: Some(150),
        })
        .unwrap();
        assert_eq!(
            body,
            serde_json::json!({"architecture":"x86_64","memory_mb":2048,"cpu_cores":2,"min_priority":150})
        );
        // The response is the node name as a bare JSON string.
        let node: String = serde_json::from_str(r#""node-primary""#).unwrap();
        assert_eq!(node, "node-primary");
    }

    #[test]
    fn console_info_deserializes_from_real_api_shape() {
        // horcrux-api `console::ConsoleInfo` (POST /api/console/:vm_id/serial).
        let json = r#"{"vm_id":"vm-100","console_type":"serial","host":"127.0.0.1",
                       "port":0,"ticket":"abc123","ws_port":0}"#;
        let info: ConsoleInfo = serde_json::from_str(json).expect("must deserialize");
        assert_eq!(info.ticket, "abc123");
        assert_eq!(info.console_type, "serial");
    }

    #[test]
    fn console_urls_point_at_real_server_routes() {
        // Routes registered in horcrux-api main.rs:
        //   GET /api/console/:vm_id/novnc and GET /api/console/ws/serial/:ticket_id
        assert_eq!(novnc_page_url("vm-100"), "/api/console/vm-100/novnc");
        assert_eq!(novnc_page_url("a b"), "/api/console/a%20b/novnc");
        assert_eq!(serial_console_ws_path("t1"), "/api/console/ws/serial/t1");
        // No hardcoded host/port: the noVNC page derives them from its origin.
        assert!(!novnc_page_url("vm-100").contains("localhost"));
    }
}
