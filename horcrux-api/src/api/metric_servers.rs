//! External metric servers (Proxmox "Metric Server": InfluxDB, Graphite).
//!
//! * `GET/POST   /api/observability/metric-servers`
//! * `GET/PUT/DELETE /api/observability/metric-servers/:id`
//! * `POST       /api/observability/metric-servers/:id/test`  send one batch now
//!   (200 with the point count, 502 with the server's error)
//!
//! Every enabled server gets a sample of this node every `interval_secs`
//! (node CPU/memory, every VM's CPU/memory/disk/network rates, storage
//! pools):
//! * InfluxDB 2.x (`api_version: v2`, org/bucket/token) and 1.x (database,
//!   optional user/password) over HTTP(S) in line protocol, or 1.x over UDP;
//! * Graphite plaintext (`<prefix>.nodes.<node>.cpu_percent 12.3 <ts>`) over
//!   TCP or UDP.
//!
//! Tokens and passwords are stored, never returned (`********`); sending the
//! mask back on update keeps the stored value. The last success / error of
//! each server is shown with it, so a dead target explains itself.
use crate::db::config_store::{kinds, ConfigStore};
use crate::error::{ApiError, ErrorResponse};
use crate::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

pub const SECRET_MASK: &str = "********";
const MAX_SERVERS: usize = 16;
/// UDP datagrams stay below a typical MTU.
const UDP_CHUNK: usize = 1400;
/// Lines per HTTP write.
const HTTP_CHUNK: usize = 5000;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ServerKind {
    Influxdb,
    Graphite,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MetricServer {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: ServerKind,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub host: String,
    /// 0 = default (InfluxDB 8086, Graphite 2003).
    #[serde(default)]
    pub port: u16,
    /// InfluxDB: `http` (default), `https`, `udp`; Graphite: `tcp` (default), `udp`.
    #[serde(default)]
    pub protocol: String,
    /// InfluxDB: `v2` (default) or `v1`.
    #[serde(default)]
    pub api_version: String,
    #[serde(default)]
    pub organization: String,
    #[serde(default)]
    pub bucket: String,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    /// Check the HTTPS certificate (default on).
    #[serde(default = "yes")]
    pub verify_tls: bool,
    /// Graphite path prefix (default `horcrux`).
    #[serde(default)]
    pub prefix: String,
    /// Seconds between samples (default 30, 5..=3600).
    #[serde(default)]
    pub interval_secs: u64,
    /// Filled on read: what the last send did.
    #[serde(default, skip_deserializing)]
    pub status: Option<SendStatus>,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct SendStatus {
    pub last_success: Option<i64>,
    pub last_error: Option<String>,
    pub last_error_at: Option<i64>,
    pub points_sent: u64,
}

impl MetricServer {
    fn masked(&self) -> Self {
        let mut c = self.clone();
        for s in [&mut c.token, &mut c.password] {
            if !s.is_empty() {
                *s = SECRET_MASK.into();
            }
        }
        c
    }

    fn effective_port(&self) -> u16 {
        match (self.port, self.kind) {
            (0, ServerKind::Influxdb) => 8086,
            (0, ServerKind::Graphite) => 2003,
            (p, _) => p,
        }
    }

    fn interval(&self) -> u64 {
        if self.interval_secs == 0 {
            30
        } else {
            self.interval_secs
        }
    }

    /// Fill defaults and check the settings; the error says what to fix.
    pub fn normalize(&mut self) -> Result<(), String> {
        self.name = self.name.trim().to_string();
        self.host = self.host.trim().to_string();
        if self.name.is_empty() || self.name.len() > 64 {
            return Err("a metric server needs a name (1-64 characters)".into());
        }
        if self.host.is_empty()
            || self.host.len() > 253
            || !self.host.chars().all(|c| {
                c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_' | '[' | ']')
            })
        {
            return Err(format!("{:?} is not a host name or address", self.host));
        }
        self.port = self.effective_port();
        if self.interval_secs == 0 {
            self.interval_secs = 30;
        }
        if !(5..=3600).contains(&self.interval_secs) {
            return Err("the interval must be 5 to 3600 seconds".into());
        }
        match self.kind {
            ServerKind::Influxdb => {
                if self.protocol.is_empty() {
                    self.protocol = "http".into();
                }
                if !matches!(self.protocol.as_str(), "http" | "https" | "udp") {
                    return Err("InfluxDB protocol must be http, https or udp".into());
                }
                if self.api_version.is_empty() {
                    self.api_version = if self.protocol == "udp" || !self.database.is_empty() {
                        "v1".into()
                    } else {
                        "v2".into()
                    };
                }
                match self.api_version.as_str() {
                    "v2" => {
                        if self.protocol == "udp" {
                            return Err(
                                "InfluxDB 2.x has no UDP input: use http(s), or api_version v1"
                                    .into(),
                            );
                        }
                        if self.organization.trim().is_empty() || self.bucket.trim().is_empty() {
                            return Err(
                                "InfluxDB 2.x needs an organization and a bucket (and a token)"
                                    .into(),
                            );
                        }
                    }
                    "v1" => {
                        if self.database.trim().is_empty() {
                            return Err("InfluxDB 1.x needs a database".into());
                        }
                    }
                    _ => return Err("api_version must be v2 or v1".into()),
                }
            }
            ServerKind::Graphite => {
                if self.protocol.is_empty() {
                    self.protocol = "tcp".into();
                }
                if !matches!(self.protocol.as_str(), "tcp" | "udp") {
                    return Err("Graphite protocol must be tcp or udp".into());
                }
                if self.prefix.trim().is_empty() {
                    self.prefix = "horcrux".into();
                }
                // dots separate levels of the Graphite tree, so they stay
                self.prefix = self
                    .prefix
                    .split('.')
                    .filter(|p| !p.trim().is_empty())
                    .map(path_part)
                    .collect::<Vec<_>>()
                    .join(".");
            }
        }
        Ok(())
    }

    fn unmask_from(&mut self, old: &MetricServer) {
        if self.token == SECRET_MASK {
            self.token = old.token.clone();
        }
        if self.password == SECRET_MASK {
            self.password = old.password.clone();
        }
    }
}

// ------------------------------------------------------------- samples

#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    F(f64),
    I(i64),
}

impl Val {
    fn num(&self) -> String {
        match self {
            Val::F(f) => format!("{f}"),
            Val::I(i) => i.to_string(),
        }
    }
}

/// One measurement: tags + numeric fields at a time.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub measurement: &'static str,
    pub tags: BTreeMap<String, String>,
    pub fields: BTreeMap<&'static str, Val>,
    pub ts_secs: i64,
}

fn influx_escape(s: &str, extra: &[char]) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        if c == ',' || c == ' ' || extra.contains(&c) {
            o.push('\\');
        }
        if c == '\n' || c == '\r' {
            o.push(' ');
            continue;
        }
        o.push(c);
    }
    o
}

/// InfluxDB line protocol, one line per sample (nanosecond timestamps).
pub fn line_protocol(samples: &[Sample]) -> Vec<String> {
    samples
        .iter()
        .filter(|s| !s.fields.is_empty())
        .map(|s| {
            let mut l = influx_escape(s.measurement, &[]);
            for (k, v) in &s.tags {
                if v.is_empty() {
                    continue;
                }
                l.push(',');
                l.push_str(&influx_escape(k, &['=']));
                l.push('=');
                l.push_str(&influx_escape(v, &['=']));
            }
            l.push(' ');
            let f: Vec<String> = s
                .fields
                .iter()
                .map(|(k, v)| {
                    format!(
                        "{}={}",
                        influx_escape(k, &['=']),
                        match v {
                            Val::F(f) => format!("{f}"),
                            Val::I(i) => format!("{i}i"),
                        }
                    )
                })
                .collect();
            l.push_str(&f.join(","));
            l.push_str(&format!(" {}", s.ts_secs * 1_000_000_000));
            l
        })
        .collect()
}

fn path_part(s: &str) -> String {
    let p: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if p.is_empty() {
        "_".into()
    } else {
        p
    }
}

/// Graphite plaintext lines: `<prefix>.<kind>.<names>.<field> <value> <ts>`.
pub fn graphite_lines(prefix: &str, samples: &[Sample]) -> Vec<String> {
    let mut out = Vec::new();
    for s in samples {
        let tag = |k: &str| s.tags.get(k).map(|v| path_part(v)).unwrap_or_default();
        let path = match s.measurement {
            "horcrux_node" => format!("{prefix}.nodes.{}", tag("node")),
            "horcrux_vm" => format!("{prefix}.vms.{}.{}", tag("node"), tag("vmid")),
            "horcrux_storage" => format!("{prefix}.storage.{}.{}", tag("node"), tag("storage")),
            other => format!("{prefix}.{}", path_part(other)),
        };
        for (f, v) in &s.fields {
            out.push(format!("{path}.{f} {} {}", v.num(), s.ts_secs));
        }
    }
    out
}

/// Sample this node now.
pub async fn collect(state: &AppState) -> Vec<Sample> {
    let now = chrono::Utc::now().timestamp();
    let node = state
        .cluster_manager
        .get_local_node_name()
        .await
        .unwrap_or_else(|_| "localhost".to_string());
    let mut out = Vec::new();
    let mut tags = BTreeMap::new();
    tags.insert("node".to_string(), node.clone());
    if let Some(u) = state.cluster_manager.local_usage().await {
        let mut f = BTreeMap::new();
        f.insert("cpu_percent", Val::F(u.cpu_usage));
        f.insert("cpu_cores", Val::I(u.cpu_cores as i64));
        f.insert("memory_used_bytes", Val::I(u.memory_used as i64));
        f.insert("memory_total_bytes", Val::I(u.memory_total as i64));
        out.push(Sample {
            measurement: "horcrux_node",
            tags: tags.clone(),
            fields: f,
            ts_secs: now,
        });
    }
    let vms = state.vm_manager.list_vms().await;
    for m in state.monitoring_manager.list_vm_metrics().await {
        if !vms.iter().any(|v| v.id == m.id) {
            continue;
        }
        let mut t = tags.clone();
        t.insert("vmid".into(), m.id.clone());
        t.insert("name".into(), m.name.clone());
        let mut f = BTreeMap::new();
        f.insert("cpu_percent", Val::F(m.cpu.usage_percent));
        f.insert("memory_used_bytes", Val::I(m.memory.used_bytes as i64));
        f.insert("memory_total_bytes", Val::I(m.memory.total_bytes as i64));
        f.insert(
            "disk_read_bytes_per_sec",
            Val::I(m.disk.read_bytes_per_sec as i64),
        );
        f.insert(
            "disk_write_bytes_per_sec",
            Val::I(m.disk.write_bytes_per_sec as i64),
        );
        f.insert(
            "net_rx_bytes_per_sec",
            Val::I(m.network.rx_bytes_per_sec as i64),
        );
        f.insert(
            "net_tx_bytes_per_sec",
            Val::I(m.network.tx_bytes_per_sec as i64),
        );
        out.push(Sample {
            measurement: "horcrux_vm",
            tags: t,
            fields: f,
            ts_secs: now,
        });
    }
    for s in state.monitoring_manager.list_storage_metrics().await {
        let mut t = tags.clone();
        t.insert("storage".into(), s.name.clone());
        t.insert("type".into(), s.storage_type.clone());
        let mut f = BTreeMap::new();
        f.insert("total_bytes", Val::I(s.total_bytes as i64));
        f.insert("used_bytes", Val::I(s.used_bytes as i64));
        f.insert("available_bytes", Val::I(s.available_bytes as i64));
        out.push(Sample {
            measurement: "horcrux_storage",
            tags: t,
            fields: f,
            ts_secs: now,
        });
    }
    out
}

// ------------------------------------------------------------- sending

fn chunks(lines: &[String], max_bytes: usize, max_lines: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut n = 0;
    for l in lines {
        if !cur.is_empty() && (cur.len() + l.len() + 1 > max_bytes || n >= max_lines) {
            out.push(std::mem::take(&mut cur));
            n = 0;
        }
        cur.push_str(l);
        cur.push('\n');
        n += 1;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

async fn resolve(host: &str, port: u16) -> Result<std::net::SocketAddr, String> {
    let h = host.trim_matches(['[', ']']);
    tokio::net::lookup_host((h, port))
        .await
        .map_err(|e| format!("cannot resolve {host}: {e}"))?
        .next()
        .ok_or_else(|| format!("{host} has no address"))
}

async fn send_udp(host: &str, port: u16, body: &[String]) -> Result<(), String> {
    let addr = resolve(host, port).await?;
    let bind = if addr.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let sock = tokio::net::UdpSocket::bind(bind)
        .await
        .map_err(|e| e.to_string())?;
    for c in body {
        sock.send_to(c.as_bytes(), addr)
            .await
            .map_err(|e| format!("UDP send to {addr}: {e}"))?;
    }
    Ok(())
}

/// Send `samples` to one server; returns the number of points sent.
pub async fn send(srv: &MetricServer, samples: &[Sample]) -> Result<usize, String> {
    let port = srv.effective_port();
    match srv.kind {
        ServerKind::Graphite => {
            let lines = graphite_lines(&srv.prefix, samples);
            if lines.is_empty() {
                return Ok(0);
            }
            if srv.protocol == "udp" {
                send_udp(&srv.host, port, &chunks(&lines, UDP_CHUNK, usize::MAX)).await?;
            } else {
                use tokio::io::AsyncWriteExt;
                let addr = resolve(&srv.host, port).await?;
                let mut s = tokio::time::timeout(
                    Duration::from_secs(5),
                    tokio::net::TcpStream::connect(addr),
                )
                .await
                .map_err(|_| format!("connecting to {addr} timed out"))?
                .map_err(|e| format!("connecting to {addr}: {e}"))?;
                let body = lines.join("\n") + "\n";
                tokio::time::timeout(Duration::from_secs(10), async {
                    s.write_all(body.as_bytes()).await?;
                    s.shutdown().await
                })
                .await
                .map_err(|_| format!("writing to {addr} timed out"))?
                .map_err(|e| format!("writing to {addr}: {e}"))?;
            }
            Ok(lines.len())
        }
        ServerKind::Influxdb => {
            let lines = line_protocol(samples);
            if lines.is_empty() {
                return Ok(0);
            }
            if srv.protocol == "udp" {
                send_udp(&srv.host, port, &chunks(&lines, UDP_CHUNK, usize::MAX)).await?;
                return Ok(lines.len());
            }
            let host = if srv.host.contains(':') && !srv.host.starts_with('[') {
                format!("[{}]", srv.host)
            } else {
                srv.host.clone()
            };
            let base = format!("{}://{}:{}", srv.protocol, host, port);
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .connect_timeout(Duration::from_secs(5))
                .danger_accept_invalid_certs(!srv.verify_tls)
                .build()
                .map_err(|e| e.to_string())?;
            for body in chunks(&lines, usize::MAX, HTTP_CHUNK) {
                let req = if srv.api_version == "v2" {
                    let r = client.post(format!("{base}/api/v2/write")).query(&[
                        ("org", srv.organization.as_str()),
                        ("bucket", srv.bucket.as_str()),
                        ("precision", "ns"),
                    ]);
                    if srv.token.is_empty() {
                        r
                    } else {
                        r.header("Authorization", format!("Token {}", srv.token))
                    }
                } else {
                    let r = client
                        .post(format!("{base}/write"))
                        .query(&[("db", srv.database.as_str()), ("precision", "ns")]);
                    if srv.username.is_empty() {
                        r
                    } else {
                        r.basic_auth(&srv.username, Some(&srv.password))
                    }
                };
                let resp = req
                    .header("Content-Type", "text/plain; charset=utf-8")
                    .body(body)
                    .send()
                    .await
                    .map_err(|e| format!("{base}: {e}"))?;
                if !resp.status().is_success() {
                    let st = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    return Err(format!(
                        "{base} answered {st}: {}",
                        text.trim().chars().take(300).collect::<String>()
                    ));
                }
            }
            Ok(lines.len())
        }
    }
}

// ------------------------------------------------------------- state + loop

static STATUS: OnceLock<Mutex<HashMap<String, SendStatus>>> = OnceLock::new();

fn statuses() -> &'static Mutex<HashMap<String, SendStatus>> {
    STATUS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn record(id: &str, r: &Result<usize, String>) {
    let now = chrono::Utc::now().timestamp();
    if let Ok(mut m) = statuses().lock() {
        let s = m.entry(id.to_string()).or_default();
        match r {
            Ok(n) => {
                s.last_success = Some(now);
                s.last_error = None;
                s.points_sent += *n as u64;
            }
            Err(e) => {
                s.last_error = Some(e.clone());
                s.last_error_at = Some(now);
            }
        }
    }
}

fn with_status(mut s: MetricServer) -> MetricServer {
    s.status = statuses().lock().ok().and_then(|m| m.get(&s.id).cloned());
    s.masked()
}

fn store(state: &AppState) -> ConfigStore {
    ConfigStore::new(&state.database)
}

async fn load(state: &AppState) -> Result<Vec<MetricServer>, ApiError> {
    Ok(store(state)
        .load::<MetricServer>(kinds::METRIC_SERVER)
        .await?
        .into_iter()
        .map(|(id, mut s)| {
            s.id = id;
            s
        })
        .collect())
}

fn spawn_loop(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut last: HashMap<String, i64> = HashMap::new();
        let mut tick = tokio::time::interval(Duration::from_secs(5));
        loop {
            tick.tick().await;
            let Ok(servers) = load(&state).await else {
                continue;
            };
            let now = chrono::Utc::now().timestamp();
            let due: Vec<&MetricServer> = servers
                .iter()
                .filter(|s| s.enabled)
                .filter(|s| now - last.get(&s.id).copied().unwrap_or(0) >= s.interval() as i64)
                .collect();
            if due.is_empty() {
                continue;
            }
            let samples = collect(&state).await;
            for s in due {
                last.insert(s.id.clone(), now);
                let r = send(s, &samples).await;
                if let Err(e) = &r {
                    tracing::warn!("metric server {}: {e}", s.name);
                }
                record(&s.id, &r);
            }
            last.retain(|id, _| servers.iter().any(|s| &s.id == id));
        }
    });
}

// ------------------------------------------------------------- routes

pub fn routes(state: &Arc<AppState>) -> Router<Arc<AppState>> {
    spawn_loop(state.clone());
    Router::new()
        .route(
            "/api/observability/metric-servers",
            get(list_servers).post(create_server),
        )
        .route(
            "/api/observability/metric-servers/:id",
            get(get_server).put(update_server).delete(delete_server),
        )
        .route(
            "/api/observability/metric-servers/:id/test",
            axum::routing::post(test_server),
        )
}

async fn list_servers(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<MetricServer>>, ApiError> {
    Ok(Json(
        load(&state).await?.into_iter().map(with_status).collect(),
    ))
}

async fn find(state: &AppState, id: &str) -> Result<MetricServer, ApiError> {
    load(state)
        .await?
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| ApiError::NotFound(format!("metric server {id} not found")))
}

async fn get_server(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<MetricServer>, ApiError> {
    Ok(Json(with_status(find(&state, &id).await?)))
}

async fn create_server(
    State(state): State<Arc<AppState>>,
    Json(mut s): Json<MetricServer>,
) -> Result<(StatusCode, Json<MetricServer>), ApiError> {
    s.normalize().map_err(ApiError::ValidationError)?;
    if s.token == SECRET_MASK || s.password == SECRET_MASK {
        return Err(ApiError::ValidationError(
            "the secret field holds the placeholder, not a value".into(),
        ));
    }
    let all = load(&state).await?;
    if all.len() >= MAX_SERVERS {
        return Err(ApiError::Conflict(format!(
            "at most {MAX_SERVERS} metric servers"
        )));
    }
    if all.iter().any(|o| o.name == s.name) {
        return Err(ApiError::Conflict(format!(
            "a metric server named {} exists",
            s.name
        )));
    }
    s.id = uuid::Uuid::new_v4().to_string();
    s.status = None;
    store(&state).put(kinds::METRIC_SERVER, &s.id, &s).await?;
    Ok((StatusCode::CREATED, Json(with_status(s))))
}

async fn update_server(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(mut s): Json<MetricServer>,
) -> Result<Json<MetricServer>, ApiError> {
    let old = find(&state, &id).await?;
    s.unmask_from(&old);
    s.normalize().map_err(ApiError::ValidationError)?;
    if load(&state)
        .await?
        .iter()
        .any(|o| o.id != id && o.name == s.name)
    {
        return Err(ApiError::Conflict(format!(
            "a metric server named {} exists",
            s.name
        )));
    }
    s.id = id.clone();
    s.status = None;
    store(&state).put(kinds::METRIC_SERVER, &id, &s).await?;
    Ok(Json(with_status(s)))
}

async fn delete_server(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    find(&state, &id).await?;
    store(&state).delete(kinds::METRIC_SERVER, &id).await?;
    if let Ok(mut m) = statuses().lock() {
        m.remove(&id);
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct TestResult {
    points: usize,
}

async fn test_server(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let s = find(&state, &id).await?;
    let samples = collect(&state).await;
    let r = send(&s, &samples).await;
    record(&s.id, &r);
    match r {
        Ok(points) => Ok(Json(TestResult { points }).into_response()),
        Err(e) => Ok((
            StatusCode::BAD_GATEWAY,
            Json(ErrorResponse::new(
                502,
                "METRIC_SERVER_UNREACHABLE",
                format!("metrics were not delivered to {}: {e}", s.name),
            )),
        )
            .into_response()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn sample() -> Vec<Sample> {
        let mut t = BTreeMap::new();
        t.insert("node".into(), "pve 1".into());
        t.insert("vmid".into(), "vm,a=b".into());
        let mut f = BTreeMap::new();
        f.insert("cpu_percent", Val::F(12.5));
        f.insert("memory_used_bytes", Val::I(1024));
        vec![
            Sample {
                measurement: "horcrux_vm",
                tags: t,
                fields: f,
                ts_secs: 1_700_000_000,
            },
            Sample {
                measurement: "horcrux_node",
                tags: [("node".to_string(), "n1.lab".to_string())].into(),
                fields: [("cpu_percent", Val::F(3.0))].into(),
                ts_secs: 1_700_000_000,
            },
        ]
    }

    fn base() -> MetricServer {
        serde_json::from_value(serde_json::json!({
            "name": "x", "type": "influxdb", "host": "127.0.0.1",
            "organization": "o", "bucket": "b", "token": "secret-token"
        }))
        .unwrap()
    }

    #[test]
    fn line_protocol_escapes_and_types_fields() {
        let l = line_protocol(&sample());
        assert_eq!(
            l[0],
            "horcrux_vm,node=pve\\ 1,vmid=vm\\,a\\=b cpu_percent=12.5,memory_used_bytes=1024i 1700000000000000000"
        );
        assert_eq!(
            l[1],
            "horcrux_node,node=n1.lab cpu_percent=3 1700000000000000000"
        );
    }

    #[test]
    fn graphite_paths_are_sanitised() {
        let l = graphite_lines("horcrux", &sample());
        assert!(
            l.contains(&"horcrux.vms.pve_1.vm_a_b.cpu_percent 12.5 1700000000".to_string()),
            "{l:?}"
        );
        assert!(
            l.contains(&"horcrux.nodes.n1_lab.cpu_percent 3 1700000000".to_string()),
            "{l:?}"
        );
    }

    #[test]
    fn normalize_defaults_and_refusals() {
        let mut s = base();
        s.normalize().unwrap();
        assert_eq!(
            (
                s.port,
                s.protocol.as_str(),
                s.api_version.as_str(),
                s.interval_secs
            ),
            (8086, "http", "v2", 30)
        );
        let mut g: MetricServer = serde_json::from_value(
            serde_json::json!({"name": "g", "type": "graphite", "host": "g.lab"}),
        )
        .unwrap();
        g.normalize().unwrap();
        assert_eq!(
            (g.port, g.protocol.as_str(), g.prefix.as_str()),
            (2003, "tcp", "horcrux")
        );
        for (mutate, what) in [
            (
                Box::new(|s: &mut MetricServer| s.host = "a b/c".into())
                    as Box<dyn Fn(&mut MetricServer)>,
                "host name",
            ),
            (Box::new(|s| s.bucket.clear()), "bucket"),
            (
                Box::new(|s| {
                    s.protocol = "udp".into();
                    s.api_version = "v2".into()
                }),
                "no UDP",
            ),
            (Box::new(|s| s.interval_secs = 2), "interval"),
            (Box::new(|s| s.name.clear()), "name"),
            (Box::new(|s| s.api_version = "v3".into()), "api_version"),
        ] {
            let mut x = base();
            mutate(&mut x);
            let e = x.normalize().unwrap_err();
            assert!(e.contains(what), "{what}: {e}");
        }
    }

    #[test]
    fn secrets_are_masked_and_restored() {
        let mut s = base();
        s.password = "pw".into();
        let m = s.masked();
        assert_eq!(
            (m.token.as_str(), m.password.as_str()),
            (SECRET_MASK, SECRET_MASK)
        );
        assert!(!serde_json::to_string(&m).unwrap().contains("secret-token"));
        let mut upd = m.clone();
        upd.unmask_from(&s);
        assert_eq!(
            (upd.token.as_str(), upd.password.as_str()),
            ("secret-token", "pw")
        );
    }

    #[test]
    fn chunking_keeps_datagrams_small() {
        let lines: Vec<String> = (0..100).map(|i| format!("m{i} v=1 1")).collect();
        let c = chunks(&lines, 200, usize::MAX);
        assert!(c.len() > 1 && c.iter().all(|x| x.len() <= 200));
        assert_eq!(c.concat().lines().count(), 100);
    }

    /// A one-shot HTTP server: returns (port, the request text received).
    async fn http_server(status: &str) -> (u16, tokio::task::JoinHandle<String>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let status = status.to_string();
        let h = tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut buf = vec![0u8; 16384];
            let mut got = Vec::new();
            loop {
                let n = s.read(&mut buf).await.unwrap();
                got.extend_from_slice(&buf[..n]);
                let t = String::from_utf8_lossy(&got).to_string();
                if let Some((head, body)) = t.split_once("\r\n\r\n") {
                    let len = head
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if body.len() >= len {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let msg = if status.starts_with("204") {
                ""
            } else {
                "{\"message\":\"bad token\"}"
            };
            let _ = s
                .write_all(
                    format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{msg}",
                        msg.len()
                    )
                    .as_bytes(),
                )
                .await;
            String::from_utf8_lossy(&got).to_string()
        });
        (port, h)
    }

    #[tokio::test]
    async fn influx_v2_http_write_carries_token_org_bucket_and_lines() {
        let (port, h) = http_server("204 No Content").await;
        let mut s = base();
        s.port = port;
        s.normalize().unwrap();
        assert_eq!(send(&s, &sample()).await.unwrap(), 2);
        let req = h.await.unwrap();
        assert!(
            req.starts_with("POST /api/v2/write?org=o&bucket=b&precision=ns"),
            "{req}"
        );
        assert!(
            req.to_ascii_lowercase()
                .contains("authorization: token secret-token"),
            "{req}"
        );
        assert!(
            req.contains("horcrux_node,node=n1.lab cpu_percent=3 1700000000000000000"),
            "{req}"
        );
    }

    #[tokio::test]
    async fn influx_v1_uses_db_and_basic_auth_and_errors_explain_themselves() {
        let (port, h) = http_server("401 Unauthorized").await;
        let mut s: MetricServer = serde_json::from_value(serde_json::json!({
            "name": "v1", "type": "influxdb", "host": "127.0.0.1", "port": port,
            "api_version": "v1", "database": "hx", "username": "u", "password": "p"
        }))
        .unwrap();
        s.normalize().unwrap();
        let e = send(&s, &sample()).await.unwrap_err();
        assert!(e.contains("401") && e.contains("bad token"), "{e}");
        let req = h.await.unwrap();
        assert!(req.starts_with("POST /write?db=hx&precision=ns"), "{req}");
        assert!(
            req.to_ascii_lowercase()
                .contains("authorization: basic dtpw"),
            "{req}"
        );
    }

    #[tokio::test]
    async fn graphite_tcp_and_udp_deliver_plaintext() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let h = tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut v = String::new();
            s.read_to_string(&mut v).await.unwrap();
            v
        });
        let mut g: MetricServer = serde_json::from_value(
            serde_json::json!({"name": "g", "type": "graphite", "host": "127.0.0.1", "port": port}),
        )
        .unwrap();
        g.normalize().unwrap();
        assert_eq!(send(&g, &sample()).await.unwrap(), 3);
        let got = h.await.unwrap();
        assert!(
            got.contains("horcrux.nodes.n1_lab.cpu_percent 3 1700000000\n"),
            "{got}"
        );

        let u = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = u.local_addr().unwrap().port();
        g.protocol = "udp".into();
        g.port = port;
        send(&g, &sample()).await.unwrap();
        let mut buf = [0u8; 2048];
        let n = tokio::time::timeout(Duration::from_secs(3), u.recv(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert!(String::from_utf8_lossy(&buf[..n])
            .contains("horcrux.vms.pve_1.vm_a_b.memory_used_bytes 1024"));
    }

    #[tokio::test]
    async fn influx_udp_line_protocol() {
        let u = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = u.local_addr().unwrap().port();
        let mut s: MetricServer = serde_json::from_value(serde_json::json!({
            "name": "u", "type": "influxdb", "host": "127.0.0.1", "port": port,
            "protocol": "udp", "database": "hx"
        }))
        .unwrap();
        s.normalize().unwrap();
        assert_eq!(s.api_version, "v1");
        send(&s, &sample()).await.unwrap();
        let mut buf = [0u8; 2048];
        let n = tokio::time::timeout(Duration::from_secs(3), u.recv(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&buf[..n]).contains("horcrux_node,node=n1.lab cpu_percent=3 ")
        );
    }

    #[tokio::test]
    async fn a_dead_target_is_an_error_not_a_hang() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let mut g: MetricServer = serde_json::from_value(
            serde_json::json!({"name": "g", "type": "graphite", "host": "127.0.0.1", "port": port}),
        )
        .unwrap();
        g.normalize().unwrap();
        assert!(send(&g, &sample()).await.is_err());
    }
}
