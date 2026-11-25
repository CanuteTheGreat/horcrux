//! API access log (Proxmox VE 9.3 "API access log" parity): every request to
//! the node is recorded - who (user, realm, how they authenticated, which API
//! token), from where (TCP peer, or `X-Forwarded-For` behind a trusted
//! proxy), what (method, path, route template), and how it ended (status,
//! time, size) - so an admin can answer "who accessed the API, which requests
//! failed, in this exact time range" and audit by API token.
//!
//! * recording: [`layer`], an outermost axum layer over the protected AND the
//!   public routers. Entries go through a bounded queue to a writer task
//!   that inserts them in batches; a full queue drops the entry and counts it
//!   (`collector.dropped`), so logging never slows a request.
//! * storage: `<data_dir>/<db_file>` (SQLite, [`store`]), pruned by age and
//!   size. In a cluster each node keeps its own; a query may name one node or
//!   `all` (peers answer on `/api/ha/peer/access-log/*`).
//! * API (privilege `Sys.Audit`; changing settings `Sys.Modify`):
//!   `GET /api/access-log`, `GET /api/access-log/stats`,
//!   `GET /api/access-log/:id`, `GET|PUT /api/access-log/settings`.
//! * settings: typed keys with documented defaults
//!   (`horcrux_common::access_log::AccessLogSettings`), in the declarative
//!   spec as `access_log:`.
pub mod store;

use crate::error::ApiError;
use crate::middleware::auth::AccessSlot;
use crate::AppState;
#[cfg(feature = "cluster")]
use axum::routing::post;
use axum::{
    extract::{ConnectInfo, MatchedPath, Path, Query, Request, State},
    middleware::Next,
    response::Response,
    routing::get,
    Json, Router,
};
use horcrux_common::access_log::{client_address, path_group, AccessLogSettings, Cidr};
use hyper::body::Body as _;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use store::{Entry, Filter, Stats, Store, TopItem};
use tokio::sync::{mpsc, oneshot};

const SETTINGS_KIND: &str = "access_log_settings";
const MAX_PATH_LEN: usize = 2048;

enum Msg {
    Entry(Box<Entry>),
    Flush(oneshot::Sender<()>),
}

struct Runtime {
    settings: AccessLogSettings,
    proxies: Vec<Cidr>,
}

/// The running recorder of this node.
pub struct AccessLog {
    tx: mpsc::Sender<Msg>,
    store: Store,
    rt: RwLock<Arc<Runtime>>,
    node: RwLock<String>,
    dropped: AtomicU64,
    written: AtomicU64,
    write_errors: AtomicU64,
    queue_capacity: u32,
}

static CURRENT: Mutex<Option<Arc<AccessLog>>> = Mutex::new(None);

/// The recorder of this process (None when its database could not be opened).
pub fn current() -> Option<Arc<AccessLog>> {
    CURRENT.lock().ok().and_then(|g| g.clone())
}

fn runtime_of(s: &AccessLogSettings) -> Arc<Runtime> {
    Arc::new(Runtime {
        proxies: s.proxies(),
        settings: s.clone(),
    })
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl AccessLog {
    fn rt(&self) -> Arc<Runtime> {
        self.rt
            .read()
            .map(|g| g.clone())
            .unwrap_or_else(|_| runtime_of(&AccessLogSettings::default()))
    }

    pub fn settings(&self) -> AccessLogSettings {
        self.rt().settings.clone()
    }

    fn node(&self) -> String {
        self.node.read().map(|n| n.clone()).unwrap_or_default()
    }

    pub fn set_settings(&self, s: AccessLogSettings) {
        if let Ok(mut g) = self.rt.write() {
            *g = runtime_of(&s);
        }
    }

    /// Queue an entry; a full queue counts a drop instead of waiting.
    pub fn record(&self, e: Entry) {
        if self.tx.try_send(Msg::Entry(Box::new(e))).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Wait until everything queued so far is in the database (queries call
    /// this so a request is visible to the query that follows it).
    pub async fn flush(&self) {
        let (tx, rx) = oneshot::channel();
        if tokio::time::timeout(Duration::from_secs(2), self.tx.send(Msg::Flush(tx)))
            .await
            .is_ok()
        {
            let _ = tokio::time::timeout(Duration::from_secs(5), rx).await;
        }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub async fn collector(&self) -> Collector {
        let s = self.settings();
        Collector {
            enabled: s.enabled,
            dropped: self.dropped.load(Ordering::Relaxed),
            written: self.written.load(Ordering::Relaxed),
            write_errors: self.write_errors.load(Ordering::Relaxed),
            queue_capacity: self.queue_capacity,
            entries: self.store.count().await.unwrap_or(0),
            db_bytes: self.store.used_bytes().await.unwrap_or(0),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Collector {
    pub enabled: bool,
    /// Entries dropped because the queue was full (since start).
    pub dropped: u64,
    pub written: u64,
    pub write_errors: u64,
    pub queue_capacity: u32,
    pub entries: i64,
    pub db_bytes: u64,
}

/// Settings from the database (defaults when none were saved).
pub async fn load_settings(state: &AppState) -> AccessLogSettings {
    crate::db::config_store::ConfigStore::new(&state.database)
        .load_values::<AccessLogSettings>(SETTINGS_KIND)
        .await
        .ok()
        .and_then(|v| v.into_iter().next())
        .unwrap_or_default()
}

/// Store new settings (API and declarative apply). `db_file` and
/// `queue_capacity` take effect at the next start; everything else at once.
pub async fn set_settings(state: &AppState, s: AccessLogSettings) -> Result<(), String> {
    s.validate()?;
    crate::db::config_store::ConfigStore::new(&state.database)
        .put(SETTINGS_KIND, "default", &s)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(log) = current() {
        log.set_settings(s);
    }
    Ok(())
}

/// Open the database and start the writer and pruning tasks. Called once
/// while the router is built; `None` when the log cannot be stored (the
/// layer then records nothing and the API answers 424).
pub async fn start(state: &Arc<AppState>) -> Option<Arc<AccessLog>> {
    let settings = load_settings(state).await;
    let path = state.config.paths.data_dir.join(&settings.db_file);
    let store = match Store::open(&path).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("access log disabled: cannot open {}: {e}", path.display());
            return None;
        }
    };
    let node = state
        .cluster_manager
        .get_local_node_name()
        .await
        .unwrap_or_else(|_| "localhost".into());
    let (tx, rx) = mpsc::channel(settings.queue_capacity as usize);
    let log = Arc::new(AccessLog {
        tx,
        store,
        rt: RwLock::new(runtime_of(&settings)),
        node: RwLock::new(node),
        dropped: AtomicU64::new(0),
        written: AtomicU64::new(0),
        write_errors: AtomicU64::new(0),
        queue_capacity: settings.queue_capacity,
    });
    tokio::spawn(writer(log.clone(), rx));
    tokio::spawn(pruner(log.clone(), state.clone()));
    if let Ok(mut g) = CURRENT.lock() {
        *g = Some(log.clone());
    }
    Some(log)
}

async fn writer(log: Arc<AccessLog>, mut rx: mpsc::Receiver<Msg>) {
    let mut buf: Vec<Entry> = Vec::new();
    let mut waiters: Vec<oneshot::Sender<()>> = Vec::new();
    while let Some(first) = rx.recv().await {
        let s = log.settings();
        let deadline =
            tokio::time::Instant::now() + Duration::from_millis(s.flush_interval_ms.into());
        let mut next = Some(first);
        loop {
            match next.take() {
                Some(Msg::Entry(e)) => buf.push(*e),
                Some(Msg::Flush(w)) => {
                    waiters.push(w);
                    break;
                }
                None => {}
            }
            if buf.len() >= s.batch_size as usize {
                break;
            }
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Some(m)) => next = Some(m),
                _ => break,
            }
        }
        if !buf.is_empty() {
            match log.store.insert_batch(&buf).await {
                Ok(()) => {
                    log.written.fetch_add(buf.len() as u64, Ordering::Relaxed);
                }
                Err(e) => {
                    log.write_errors
                        .fetch_add(buf.len() as u64, Ordering::Relaxed);
                    tracing::warn!("access log: writing {} entries failed: {e}", buf.len());
                }
            }
            buf.clear();
        }
        for w in waiters.drain(..) {
            let _ = w.send(());
        }
    }
}

async fn pruner(log: Arc<AccessLog>, state: Arc<AppState>) {
    loop {
        let s = log.settings();
        if let Ok(n) = state.cluster_manager.get_local_node_name().await {
            if let Ok(mut g) = log.node.write() {
                *g = n;
            }
        }
        match log.store.prune(s.retention_days, s.max_mb, now_ms()).await {
            Ok(0) => {}
            Ok(n) => tracing::info!("access log: pruned {n} entries"),
            Err(e) => tracing::warn!("access log: pruning failed: {e}"),
        }
        tokio::time::sleep(Duration::from_secs(s.prune_interval_secs.into())).await;
    }
}

// ------------------------------------------------------------------ layer

/// The recording layer (outermost: sees rate-limit, CORS and auth refusals).
pub async fn layer(
    State(log): State<Option<Arc<AccessLog>>>,
    mut req: Request,
    next: Next,
) -> Response {
    let Some(log) = log else {
        return next.run(req).await;
    };
    let rt = log.rt();
    let s = &rt.settings;
    if !s.enabled {
        return next.run(req).await;
    }
    let path = req.uri().path();
    if (!s.log_static && !path.starts_with("/api") && !path.starts_with("/metrics"))
        || s.exclude_path_prefixes
            .iter()
            .any(|p| path.starts_with(p.as_str()))
    {
        return next.run(req).await;
    }
    let started = Instant::now();
    let ts_ms = now_ms();
    let method = req.method().to_string();
    let path: String = path.chars().take(MAX_PATH_LEN).collect();
    let group = path_group(req.extensions().get::<MatchedPath>().map(|m| m.as_str()));
    let ua: Option<String> = req
        .headers()
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.chars().take(s.max_user_agent_len as usize).collect());
    let xff: Option<String> = req
        .headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let client = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| client_address(c.0.ip(), xff.as_deref(), &rt.proxies).to_string())
        .unwrap_or_else(|| "unknown".into());
    let slot = AccessSlot::default();
    req.extensions_mut().insert(slot.clone());

    let resp = next.run(req).await;

    let who = slot.get();
    let bytes_out = resp.body().size_hint().exact().map(|n| n as i64);
    log.record(Entry {
        id: 0,
        ts_ms,
        time: String::new(),
        node: log.node(),
        user: who.user,
        realm: who.realm,
        auth_method: who.auth_method,
        token_id: who.token_id,
        token_name: who.token_name,
        client,
        method,
        path,
        path_group: group,
        status: resp.status().as_u16(),
        duration_ms: started.elapsed().as_secs_f64() * 1000.0,
        bytes_out,
        user_agent: ua,
    });
    resp
}

// ----------------------------------------------------------------- routes

pub fn routes() -> Router<Arc<AppState>> {
    let r = Router::new()
        .route("/api/access-log", get(list))
        .route("/api/access-log/stats", get(stats))
        .route(
            "/api/access-log/settings",
            get(get_settings).put(put_settings),
        )
        .route("/api/access-log/:id", get(detail));
    #[cfg(feature = "cluster")]
    let r = r
        .route("/api/ha/peer/access-log/list", post(peer_list))
        .route("/api/ha/peer/access-log/stats", post(peer_stats))
        .route("/api/ha/peer/access-log/get", post(peer_get));
    r
}

fn unavailable() -> ApiError {
    ApiError::NotAvailable {
        reason: "access_log_unavailable",
        message: "The access log database on this node could not be opened, so requests are not being recorded."
            .into(),
        hint: "Check that the data directory is writable and has free space, then restart horcrux-api."
            .into(),
    }
}

fn bad(e: String) -> ApiError {
    ApiError::BadRequest(e)
}

#[derive(Debug, Clone, PartialEq)]
enum NodeSel {
    Local,
    All,
    Named(String),
}

async fn select_node(state: &Arc<AppState>, f: &Filter) -> NodeSel {
    let want = f.node.as_deref().map(str::trim).unwrap_or("");
    match want {
        "" | "local" => NodeSel::Local,
        "all" => NodeSel::All,
        n => {
            let me = state
                .cluster_manager
                .get_local_node_name()
                .await
                .unwrap_or_default();
            if n == me {
                NodeSel::Local
            } else {
                NodeSel::Named(n.to_string())
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeResult {
    pub node: String,
    pub total: i64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page {
    pub total: i64,
    pub limit: u32,
    pub offset: u32,
    pub entries: Vec<Entry>,
    pub nodes: Vec<NodeResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatsResponse {
    #[serde(flatten)]
    pub stats: Stats,
    pub nodes: Vec<NodeResult>,
    pub collector: Collector,
}

struct Paging {
    limit: u32,
    offset: u32,
}

fn paging(f: &Filter, s: &AccessLogSettings) -> Result<Paging, ApiError> {
    let limit = Filter::int(&f.limit, "limit")
        .map_err(bad)?
        .unwrap_or(s.default_page_size)
        .clamp(1, s.max_page_size);
    let offset = Filter::int(&f.offset, "offset").map_err(bad)?.unwrap_or(0);
    Ok(Paging { limit, offset })
}

async fn local_list(
    f: &Filter,
    p: &Paging,
    log: &AccessLog,
) -> Result<(i64, Vec<Entry>), ApiError> {
    let r = f.resolve().map_err(bad)?;
    log.flush().await;
    log.store
        .list(&r, p.limit, p.offset)
        .await
        .map_err(ApiError::Internal)
}

async fn list(
    State(state): State<Arc<AppState>>,
    Query(f): Query<Filter>,
) -> Result<Json<Page>, ApiError> {
    let log = current().ok_or_else(unavailable)?;
    let s = log.settings();
    let p = paging(&f, &s)?;
    f.resolve().map_err(bad)?;
    let me = log.node();
    match select_node(&state, &f).await {
        NodeSel::Local => {
            let (total, entries) = local_list(&f, &p, &log).await?;
            Ok(Json(Page {
                total,
                limit: p.limit,
                offset: p.offset,
                entries,
                nodes: vec![NodeResult {
                    node: me,
                    total,
                    error: None,
                }],
            }))
        }
        sel => cluster_list(&state, &log, f, p, sel).await.map(Json),
    }
}

async fn stats(
    State(state): State<Arc<AppState>>,
    Query(f): Query<Filter>,
) -> Result<Json<StatsResponse>, ApiError> {
    let log = current().ok_or_else(unavailable)?;
    let s = log.settings();
    let buckets = Filter::int(&f.buckets, "buckets")
        .map_err(bad)?
        .unwrap_or(s.stats_buckets)
        .clamp(1, 500);
    f.resolve().map_err(bad)?;
    let me = log.node();
    match select_node(&state, &f).await {
        NodeSel::Local => {
            log.flush().await;
            let st = log
                .store
                .stats(&f.resolve().map_err(bad)?, buckets, s.stats_top_n)
                .await
                .map_err(ApiError::Internal)?;
            let nodes = vec![NodeResult {
                node: me,
                total: st.total,
                error: None,
            }];
            Ok(Json(StatsResponse {
                stats: st,
                nodes,
                collector: log.collector().await,
            }))
        }
        sel => cluster_stats(&state, &log, f, buckets, sel).await.map(Json),
    }
}

#[derive(Debug, Deserialize, Default)]
struct DetailQuery {
    node: Option<String>,
}

async fn detail(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Query(q): Query<DetailQuery>,
) -> Result<Json<Entry>, ApiError> {
    let log = current().ok_or_else(unavailable)?;
    let f = Filter {
        node: q.node,
        ..Default::default()
    };
    match select_node(&state, &f).await {
        NodeSel::Local | NodeSel::All => {
            log.flush().await;
            log.store
                .get(id)
                .await
                .map_err(ApiError::Internal)?
                .map(Json)
                .ok_or_else(|| ApiError::NotFound(format!("no access log entry {id} on this node")))
        }
        NodeSel::Named(n) => remote_get(&state, &n, id).await.map(Json),
    }
}

async fn get_settings(State(state): State<Arc<AppState>>) -> Json<AccessLogSettings> {
    Json(match current() {
        Some(l) => l.settings(),
        None => load_settings(&state).await,
    })
}

async fn put_settings(
    State(state): State<Arc<AppState>>,
    Json(s): Json<AccessLogSettings>,
) -> Result<Json<AccessLogSettings>, ApiError> {
    set_settings(&state, s.clone()).await.map_err(bad)?;
    Ok(Json(s))
}

// -------------------------------------------------------------- merging

/// Combine per-node statistics computed over the same range and bucket
/// width into one.
pub fn merge_stats(parts: Vec<Stats>, top_n: usize) -> Stats {
    let mut it = parts.into_iter();
    let Some(mut out) = it.next() else {
        return Stats::default();
    };
    let mut weighted = out.avg_duration_ms * out.total as f64;
    for p in it {
        out.total += p.total;
        out.errors += p.errors;
        out.bytes_out += p.bytes_out;
        weighted += p.avg_duration_ms * p.total as f64;
        out.start_ms = match (out.start_ms, p.start_ms) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        out.end_ms = match (out.end_ms, p.end_ms) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        for b in p.buckets {
            match out.buckets.iter_mut().find(|x| x.start_ms == b.start_ms) {
                Some(x) => {
                    x.total += b.total;
                    x.s2xx += b.s2xx;
                    x.s3xx += b.s3xx;
                    x.s4xx += b.s4xx;
                    x.s5xx += b.s5xx;
                }
                None => out.buckets.push(b),
            }
        }
        merge_top(&mut out.top_clients, p.top_clients);
        merge_top(&mut out.top_users, p.top_users);
        merge_top(&mut out.top_tokens, p.top_tokens);
        merge_top(&mut out.top_groups, p.top_groups);
        merge_top(&mut out.top_statuses, p.top_statuses);
        merge_top(&mut out.top_methods, p.top_methods);
    }
    out.buckets.sort_by_key(|b| b.start_ms);
    out.avg_duration_ms = if out.total > 0 {
        weighted / out.total as f64
    } else {
        0.0
    };
    for t in [
        &mut out.top_clients,
        &mut out.top_users,
        &mut out.top_tokens,
        &mut out.top_groups,
        &mut out.top_statuses,
        &mut out.top_methods,
    ] {
        t.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
        t.truncate(top_n);
    }
    out
}

fn merge_top(into: &mut Vec<TopItem>, from: Vec<TopItem>) {
    for f in from {
        match into.iter_mut().find(|x| x.value == f.value) {
            Some(x) => {
                x.count += f.count;
                if x.label.is_none() {
                    x.label = f.label;
                }
            }
            None => into.push(f),
        }
    }
}

/// Combine per-node pages (each holding that node's newest `offset+limit`
/// entries) into one page, newest first.
pub fn merge_pages(parts: Vec<Vec<Entry>>, limit: u32, offset: u32) -> Vec<Entry> {
    let mut all: Vec<Entry> = parts.into_iter().flatten().collect();
    all.sort_by(|a, b| {
        b.ts_ms
            .cmp(&a.ts_ms)
            .then_with(|| a.node.cmp(&b.node))
            .then_with(|| b.id.cmp(&a.id))
    });
    all.into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .collect()
}

// ----------------------------------------------------------- cluster fan-out

#[cfg(not(feature = "cluster"))]
async fn cluster_list(
    state: &Arc<AppState>,
    log: &Arc<AccessLog>,
    f: Filter,
    p: Paging,
    sel: NodeSel,
) -> Result<Page, ApiError> {
    match sel {
        NodeSel::Named(n) => Err(ApiError::NotFound(format!("unknown node {n}"))),
        _ => {
            let _ = state;
            let (total, entries) = local_list(&f, &p, log).await?;
            Ok(Page {
                total,
                limit: p.limit,
                offset: p.offset,
                entries,
                nodes: vec![NodeResult {
                    node: log.node(),
                    total,
                    error: None,
                }],
            })
        }
    }
}

#[cfg(not(feature = "cluster"))]
async fn cluster_stats(
    _state: &Arc<AppState>,
    log: &Arc<AccessLog>,
    f: Filter,
    buckets: u32,
    sel: NodeSel,
) -> Result<StatsResponse, ApiError> {
    if let NodeSel::Named(n) = sel {
        return Err(ApiError::NotFound(format!("unknown node {n}")));
    }
    log.flush().await;
    let st = log
        .store
        .stats(
            &f.resolve().map_err(bad)?,
            buckets,
            log.settings().stats_top_n,
        )
        .await
        .map_err(ApiError::Internal)?;
    let nodes = vec![NodeResult {
        node: log.node(),
        total: st.total,
        error: None,
    }];
    Ok(StatsResponse {
        stats: st,
        nodes,
        collector: log.collector().await,
    })
}

#[cfg(not(feature = "cluster"))]
async fn remote_get(_state: &Arc<AppState>, node: &str, _id: i64) -> Result<Entry, ApiError> {
    Err(ApiError::NotFound(format!("unknown node {node}")))
}

#[cfg(feature = "cluster")]
use cluster_fanout::*;

#[cfg(feature = "cluster")]
mod cluster_fanout {
    use super::*;
    use crate::api::drain::{call_peer, peer_nodes};
    use crate::middleware::auth::CLUSTER_HA_ROLE;
    use axum::Extension;
    use horcrux_cluster::cluster::node::Node;

    #[derive(Debug, Serialize, Deserialize)]
    pub struct PeerListReq {
        pub filter: Filter,
        pub limit: u32,
        pub offset: u32,
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub struct PeerStatsReq {
        pub filter: Filter,
        pub buckets: u32,
        pub top_n: u32,
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub struct PeerGetReq {
        pub id: i64,
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub struct PeerList {
        pub total: i64,
        pub entries: Vec<Entry>,
    }

    fn require_peer(
        user: &Option<Extension<crate::middleware::auth::AuthUser>>,
    ) -> Result<(), ApiError> {
        match user {
            Some(Extension(u)) if u.role == CLUSTER_HA_ROLE => Ok(()),
            _ => Err(ApiError::Forbidden(
                "the access log peer API is only for other cluster nodes".into(),
            )),
        }
    }

    pub async fn peer_list(
        user: Option<Extension<crate::middleware::auth::AuthUser>>,
        Json(req): Json<PeerListReq>,
    ) -> Result<Json<PeerList>, ApiError> {
        require_peer(&user)?;
        let log = current().ok_or_else(unavailable)?;
        let p = Paging {
            limit: req.limit,
            offset: req.offset,
        };
        let (total, entries) = local_list(&req.filter, &p, &log).await?;
        Ok(Json(PeerList { total, entries }))
    }

    pub async fn peer_stats(
        user: Option<Extension<crate::middleware::auth::AuthUser>>,
        Json(req): Json<PeerStatsReq>,
    ) -> Result<Json<Stats>, ApiError> {
        require_peer(&user)?;
        let log = current().ok_or_else(unavailable)?;
        log.flush().await;
        log.store
            .stats(&req.filter.resolve().map_err(bad)?, req.buckets, req.top_n)
            .await
            .map(Json)
            .map_err(ApiError::Internal)
    }

    pub async fn peer_get(
        user: Option<Extension<crate::middleware::auth::AuthUser>>,
        Json(req): Json<PeerGetReq>,
    ) -> Result<Json<Entry>, ApiError> {
        require_peer(&user)?;
        let log = current().ok_or_else(unavailable)?;
        log.flush().await;
        log.store
            .get(req.id)
            .await
            .map_err(ApiError::Internal)?
            .map(Json)
            .ok_or_else(|| ApiError::NotFound(format!("no access log entry {}", req.id)))
    }

    async fn peers(
        state: &Arc<AppState>,
        log: &AccessLog,
        sel: &NodeSel,
    ) -> Result<Vec<Node>, ApiError> {
        let me = log.node();
        let all = state.cluster_manager.list_nodes().await;
        match sel {
            NodeSel::Named(n) => all
                .into_iter()
                .find(|x| &x.name == n)
                .map(|x| vec![x])
                .ok_or_else(|| ApiError::NotFound(format!("unknown node {n}"))),
            _ => Ok(peer_nodes(&all, &me)),
        }
    }

    async fn ask<B: Serialize, T: serde::de::DeserializeOwned>(
        state: &Arc<AppState>,
        node: &Node,
        path: &str,
        body: &B,
        timeout: u32,
    ) -> Result<T, String> {
        match tokio::time::timeout(
            Duration::from_secs(timeout.into()),
            call_peer::<B, T>(state, node, path, body),
        )
        .await
        {
            Ok(r) => r,
            Err(_) => Err(format!("no answer within {timeout} s")),
        }
    }

    pub async fn cluster_list(
        state: &Arc<AppState>,
        log: &Arc<AccessLog>,
        f: Filter,
        p: Paging,
        sel: NodeSel,
    ) -> Result<Page, ApiError> {
        let s = log.settings();
        let want = p.offset.saturating_add(p.limit).min(10_000);
        let mut parts: Vec<Vec<Entry>> = Vec::new();
        let mut nodes: Vec<NodeResult> = Vec::new();
        let mut total = 0;
        if sel == NodeSel::All {
            let lp = Paging {
                limit: want,
                offset: 0,
            };
            let (t, e) = local_list(&f, &lp, log).await?;
            total += t;
            parts.push(e);
            nodes.push(NodeResult {
                node: log.node(),
                total: t,
                error: None,
            });
        }
        let targets = peers(state, log, &sel).await?;
        let req = PeerListReq {
            filter: f.clone(),
            limit: want,
            offset: 0,
        };
        let answers = futures::future::join_all(targets.iter().map(|n| {
            let (state, req) = (state.clone(), &req);
            async move {
                ask::<_, PeerList>(
                    &state,
                    n,
                    "/api/ha/peer/access-log/list",
                    req,
                    s.peer_timeout_secs,
                )
                .await
            }
        }))
        .await;
        for (n, a) in targets.iter().zip(answers) {
            match a {
                Ok(r) => {
                    total += r.total;
                    nodes.push(NodeResult {
                        node: n.name.clone(),
                        total: r.total,
                        error: None,
                    });
                    parts.push(r.entries);
                }
                Err(e) => nodes.push(NodeResult {
                    node: n.name.clone(),
                    total: 0,
                    error: Some(e),
                }),
            }
        }
        if sel != NodeSel::All {
            if let Some(err) = nodes.first().and_then(|n| n.error.clone()) {
                return Err(ApiError::ServiceUnavailable(format!(
                    "node {} did not answer: {err}",
                    nodes[0].node
                )));
            }
        }
        Ok(Page {
            total,
            limit: p.limit,
            offset: p.offset,
            entries: merge_pages(parts, p.limit, p.offset),
            nodes,
        })
    }

    pub async fn cluster_stats(
        state: &Arc<AppState>,
        log: &Arc<AccessLog>,
        mut f: Filter,
        buckets: u32,
        sel: NodeSel,
    ) -> Result<StatsResponse, ApiError> {
        let s = log.settings();
        // Every node must bucket the same range: fix an open range now.
        let r = f.resolve().map_err(bad)?;
        if r.start_ms.is_none() || r.end_ms.is_none() {
            let end = r.end_ms.unwrap_or_else(now_ms);
            let start = r.start_ms.unwrap_or(end - 86_400_000);
            f.start = Some(start.to_string());
            f.end = Some(end.to_string());
        }
        let mut parts: Vec<Stats> = Vec::new();
        let mut nodes: Vec<NodeResult> = Vec::new();
        if sel == NodeSel::All {
            log.flush().await;
            let st = log
                .store
                .stats(&f.resolve().map_err(bad)?, buckets, s.stats_top_n)
                .await
                .map_err(ApiError::Internal)?;
            nodes.push(NodeResult {
                node: log.node(),
                total: st.total,
                error: None,
            });
            parts.push(st);
        }
        let targets = peers(state, log, &sel).await?;
        let req = PeerStatsReq {
            filter: f.clone(),
            buckets,
            top_n: s.stats_top_n,
        };
        let answers = futures::future::join_all(targets.iter().map(|n| {
            let (state, req) = (state.clone(), &req);
            async move {
                ask::<_, Stats>(
                    &state,
                    n,
                    "/api/ha/peer/access-log/stats",
                    req,
                    s.peer_timeout_secs,
                )
                .await
            }
        }))
        .await;
        for (n, a) in targets.iter().zip(answers) {
            match a {
                Ok(st) => {
                    nodes.push(NodeResult {
                        node: n.name.clone(),
                        total: st.total,
                        error: None,
                    });
                    parts.push(st);
                }
                Err(e) => nodes.push(NodeResult {
                    node: n.name.clone(),
                    total: 0,
                    error: Some(e),
                }),
            }
        }
        if sel != NodeSel::All {
            if let Some(err) = nodes.first().and_then(|n| n.error.clone()) {
                return Err(ApiError::ServiceUnavailable(format!(
                    "node {} did not answer: {err}",
                    nodes[0].node
                )));
            }
        }
        Ok(StatsResponse {
            stats: merge_stats(parts, s.stats_top_n as usize),
            nodes,
            collector: log.collector().await,
        })
    }

    pub async fn remote_get(state: &Arc<AppState>, node: &str, id: i64) -> Result<Entry, ApiError> {
        let log = current().ok_or_else(unavailable)?;
        let sel = NodeSel::Named(node.to_string());
        let n = peers(state, &log, &sel).await?.remove(0);
        ask::<_, Entry>(
            state,
            &n,
            "/api/ha/peer/access-log/get",
            &PeerGetReq { id },
            log.settings().peer_timeout_secs,
        )
        .await
        .map_err(|e| {
            if e.starts_with("404") {
                ApiError::NotFound(format!("no access log entry {id} on {node}"))
            } else {
                ApiError::ServiceUnavailable(format!("node {node} did not answer: {e}"))
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use store::Bucket;

    fn top(v: &str, n: i64) -> TopItem {
        TopItem {
            value: v.into(),
            label: None,
            count: n,
        }
    }

    fn st(total: i64, users: Vec<TopItem>, b: i64) -> Stats {
        Stats {
            total,
            errors: 1,
            start_ms: Some(0),
            end_ms: Some(2000),
            bucket_ms: 1000,
            buckets: (0..2)
                .map(|i| Bucket {
                    start_ms: i * 1000,
                    total: b,
                    s2xx: b,
                    ..Default::default()
                })
                .collect(),
            avg_duration_ms: 2.0,
            bytes_out: 5,
            top_users: users,
            ..Default::default()
        }
    }

    #[test]
    fn stats_merge_sums_and_ranks() {
        let a = st(10, vec![top("alice", 6), top("bob", 4)], 5);
        let b = st(30, vec![top("bob", 20), top("carol", 10)], 15);
        let m = merge_stats(vec![a, b], 2);
        assert_eq!(m.total, 40);
        assert_eq!(m.errors, 2);
        assert_eq!(m.bytes_out, 10);
        assert_eq!(m.buckets.len(), 2);
        assert_eq!(m.buckets[0].total, 20);
        assert_eq!(m.top_users.len(), 2);
        assert_eq!(m.top_users[0].value, "bob");
        assert_eq!(m.top_users[0].count, 24);
        assert_eq!(m.top_users[1].value, "carol");
        assert!((m.avg_duration_ms - 2.0).abs() < 1e-9);
        assert_eq!(merge_stats(vec![], 5), Stats::default());
    }

    fn e(ts: i64, node: &str, id: i64) -> Entry {
        Entry {
            id,
            ts_ms: ts,
            time: String::new(),
            node: node.into(),
            user: None,
            realm: None,
            auth_method: None,
            token_id: None,
            token_name: None,
            client: "c".into(),
            method: "GET".into(),
            path: "/api/x".into(),
            path_group: "/api/x".into(),
            status: 200,
            duration_ms: 0.0,
            bytes_out: None,
            user_agent: None,
        }
    }

    #[test]
    fn pages_merge_newest_first_and_page() {
        let a = vec![e(30, "a", 3), e(10, "a", 1)];
        let b = vec![e(40, "b", 9), e(20, "b", 8)];
        let m = merge_pages(vec![a, b], 2, 1);
        assert_eq!(m.iter().map(|x| x.ts_ms).collect::<Vec<_>>(), vec![30, 20]);
    }

    #[tokio::test]
    async fn queue_drops_instead_of_blocking_and_writer_batches() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::open(&d.path().join("q.db")).await.unwrap();
        let (tx, rx) = mpsc::channel(4);
        let log = Arc::new(AccessLog {
            tx,
            store,
            rt: RwLock::new(runtime_of(&AccessLogSettings::default())),
            node: RwLock::new("n".into()),
            dropped: AtomicU64::new(0),
            written: AtomicU64::new(0),
            write_errors: AtomicU64::new(0),
            queue_capacity: 4,
        });
        // no writer yet: the 5th and 6th entries cannot be queued
        for i in 0..6 {
            log.record(e(i, "n", 0));
        }
        assert_eq!(log.dropped.load(Ordering::Relaxed), 2);
        tokio::spawn(writer(log.clone(), rx));
        log.flush().await;
        assert_eq!(log.store.count().await.unwrap(), 4);
        assert_eq!(log.written.load(Ordering::Relaxed), 4);
    }
}
