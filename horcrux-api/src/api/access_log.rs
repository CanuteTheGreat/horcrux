//! API access log: one entry per `/api` request with the caller, the client
//! address, the route group and how it ended (status, duration, bytes).
//!
//! * `GET /api/access-log?start=&end=&user=&client=&method=&path=&status=&limit=&offset=`
//!   -> `{total, entries}` newest first. `start`/`end` are unix seconds;
//!   `status` is a class (`4xx`) or an exact code (`404`).
//! * `GET /api/access-log/stats?<same filters>&bucket_secs=` -> counts over
//!   time and by client, user, path group and status.
//!
//! Retention is bounded by `[access_log] max_entries` and `max_age_days`;
//! entries are also appended to `<data_dir>/access-log.jsonl` and reloaded
//! when the API starts. Query strings (which can carry secrets) and token
//! values are never recorded. The client address is the TCP peer, or the
//! first `X-Forwarded-For` hop only when the peer is listed in
//! `[access_log] trusted_proxies`.
use crate::error::ApiError;
use crate::AppState;
use axum::{
    extract::{ConnectInfo, Query, Request},
    middleware::Next,
    response::Response,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AccessEntry {
    /// Unix time in milliseconds.
    pub ts_ms: i64,
    pub node: String,
    pub user: Option<String>,
    /// `Token`, `Cookie`, `ApiKey` or `ClusterNode`; never the secret.
    pub auth: Option<String>,
    pub client: String,
    pub method: String,
    pub path: String,
    /// First path segment after `/api/` (`vms`, `storage`, ...).
    pub path_group: String,
    pub status: u16,
    pub duration_ms: u64,
    pub bytes: Option<u64>,
    pub user_agent: Option<String>,
}

pub struct AccessLog {
    enabled: bool,
    max_entries: usize,
    max_age_ms: i64,
    trusted: Vec<String>,
    node: String,
    file: Option<PathBuf>,
    inner: Mutex<Inner>,
}

struct Inner {
    entries: VecDeque<AccessEntry>,
    appended: usize,
}

static LOG: OnceLock<Arc<AccessLog>> = OnceLock::new();

/// Filled by [`identify`] (inside the auth layer), read by the recorder.
#[derive(Clone, Default)]
struct Who(Arc<Mutex<Option<(String, String)>>>);

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn node_name() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

/// `/api/vms/abc/start` -> `vms`; other paths -> `other`.
pub fn path_group(path: &str) -> String {
    path.strip_prefix("/api/")
        .and_then(|r| r.split('/').find(|s| !s.is_empty()))
        .unwrap_or("other")
        .to_string()
}

/// The client address: the peer, unless the peer is a trusted proxy and
/// sent `X-Forwarded-For` (then its first hop).
pub fn client_address(peer: &str, forwarded: Option<&str>, trusted: &[String]) -> String {
    if trusted.iter().any(|t| t == peer) {
        if let Some(first) = forwarded
            .and_then(|f| f.split(',').next())
            .map(str::trim)
            .filter(|s| !s.is_empty() && s.len() <= 64)
        {
            return first.to_string();
        }
    }
    peer.to_string()
}

impl AccessLog {
    pub fn new(cfg: &horcrux_core::config::AccessLogConfig, file: Option<PathBuf>) -> Self {
        let log = AccessLog {
            enabled: cfg.enabled,
            max_entries: cfg.max_entries.max(1),
            max_age_ms: i64::from(cfg.max_age_days.max(1)) * 86_400_000,
            trusted: cfg.trusted_proxies.clone(),
            node: node_name(),
            file,
            inner: Mutex::new(Inner {
                entries: VecDeque::new(),
                appended: 0,
            }),
        };
        log.load();
        log
    }

    fn load(&self) {
        let Some(path) = &self.file else { return };
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        let mut g = self.inner.lock().unwrap();
        for line in text.lines() {
            if let Ok(e) = serde_json::from_str::<AccessEntry>(line) {
                g.entries.push_back(e);
            }
        }
        self.trim(&mut g.entries, now_ms());
        let kept: Vec<AccessEntry> = g.entries.iter().cloned().collect();
        g.appended = kept.len();
        drop(g);
        self.rewrite(&kept);
    }

    fn trim(&self, v: &mut VecDeque<AccessEntry>, now: i64) {
        while v.len() > self.max_entries {
            v.pop_front();
        }
        while v.front().is_some_and(|e| now - e.ts_ms > self.max_age_ms) {
            v.pop_front();
        }
    }

    fn rewrite(&self, entries: &[AccessEntry]) {
        let Some(path) = &self.file else { return };
        let tmp = path.with_extension("jsonl.tmp");
        let mut out = String::new();
        for e in entries {
            if let Ok(l) = serde_json::to_string(e) {
                out.push_str(&l);
                out.push('\n');
            }
        }
        if std::fs::write(&tmp, out).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }

    pub fn record(&self, e: AccessEntry) {
        if !self.enabled {
            return;
        }
        let line = serde_json::to_string(&e).ok();
        let mut g = self.inner.lock().unwrap();
        g.entries.push_back(e);
        self.trim(&mut g.entries, now_ms());
        g.appended += 1;
        if g.appended > self.max_entries.saturating_mul(2) {
            let kept: Vec<AccessEntry> = g.entries.iter().cloned().collect();
            g.appended = kept.len();
            drop(g);
            self.rewrite(&kept);
            return;
        }
        drop(g);
        if let (Some(path), Some(line)) = (&self.file, line) {
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                let _ = writeln!(f, "{line}");
            }
        }
    }

    pub fn all(&self) -> Vec<AccessEntry> {
        self.inner.lock().unwrap().entries.iter().cloned().collect()
    }
}

/// Create the process-wide log from the node configuration.
pub fn init(cfg: &horcrux_core::config::HorcruxConfig) -> Arc<AccessLog> {
    let file = cfg.paths.data_dir.join("access-log.jsonl");
    let log = Arc::new(AccessLog::new(&cfg.access_log, Some(file)));
    let _ = LOG.set(log.clone());
    log
}

/// Outer layer: times the request, then records it.
pub async fn record_middleware(
    log: Arc<AccessLog>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    mut request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_string();
    let wanted = log.enabled
        && path.starts_with("/api/")
        && !path.starts_with("/api/health")
        && path != "/api/ws";
    if !wanted {
        return next.run(request).await;
    }
    let who = Who::default();
    request.extensions_mut().insert(who.clone());
    let method = request.method().to_string();
    // Scoped so the header closure (which borrows the non-Sync request) is
    // dropped before the request is moved into `next.run(..).await`.
    let (client, user_agent) = {
        let header = |n: &str| {
            request
                .headers()
                .get(n)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.chars().take(200).collect::<String>())
        };
        let client = client_address(
            &peer.ip().to_string(),
            header("x-forwarded-for").as_deref(),
            &log.trusted,
        );
        (client, header("user-agent"))
    };
    let started = Instant::now();
    let ts_ms = now_ms();
    let response = next.run(request).await;
    let bytes = response
        .headers()
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let (user, auth) = match who.0.lock().unwrap().clone() {
        Some((u, a)) => (Some(u), Some(a)),
        None => (None, None),
    };
    log.record(AccessEntry {
        ts_ms,
        node: log.node.clone(),
        user,
        auth,
        client,
        method,
        path_group: path_group(&path),
        path,
        status: response.status().as_u16(),
        duration_ms: started.elapsed().as_millis() as u64,
        bytes,
        user_agent,
    });
    response
}

/// Inner layer (inside authentication): names the caller for the recorder.
pub async fn identify(request: Request, next: Next) -> Response {
    let who = request.extensions().get::<Who>().cloned();
    let user = request
        .extensions()
        .get::<crate::middleware::auth::AuthUser>()
        .map(|u| (u.username.clone(), format!("{:?}", u.method)));
    if let (Some(who), Some(user)) = (who, user) {
        *who.0.lock().unwrap() = Some(user);
    }
    next.run(request).await
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct AccessQuery {
    pub start: Option<i64>,
    pub end: Option<i64>,
    pub user: Option<String>,
    pub client: Option<String>,
    pub method: Option<String>,
    pub path: Option<String>,
    pub group: Option<String>,
    pub status: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub bucket_secs: Option<i64>,
}

fn pick(o: &Option<String>) -> Option<&str> {
    o.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "all")
}

fn status_matches(want: &str, status: u16) -> bool {
    match want.strip_suffix("xx").and_then(|d| d.parse::<u16>().ok()) {
        Some(class) => status / 100 == class,
        None => want.parse::<u16>().is_ok_and(|c| c == status),
    }
}

/// Entries matching `q`, newest first (not yet paged).
pub fn filter_entries(all: Vec<AccessEntry>, q: &AccessQuery) -> Vec<AccessEntry> {
    let path = pick(&q.path).map(str::to_ascii_lowercase);
    let mut v: Vec<AccessEntry> = all
        .into_iter()
        .filter(|e| q.start.is_none_or(|t| e.ts_ms >= t.saturating_mul(1000)))
        .filter(|e| q.end.is_none_or(|t| e.ts_ms <= t.saturating_mul(1000)))
        .filter(|e| pick(&q.user).is_none_or(|u| e.user.as_deref() == Some(u)))
        .filter(|e| pick(&q.client).is_none_or(|c| e.client == c))
        .filter(|e| pick(&q.method).is_none_or(|m| e.method.eq_ignore_ascii_case(m)))
        .filter(|e| pick(&q.group).is_none_or(|g| e.path_group == g))
        .filter(|e| pick(&q.status).is_none_or(|s| status_matches(s, e.status)))
        .filter(|e| {
            path.as_ref()
                .is_none_or(|p| e.path.to_ascii_lowercase().contains(p.as_str()))
        })
        .collect();
    v.reverse();
    v
}

#[derive(Serialize)]
pub struct AccessPage {
    pub enabled: bool,
    pub total: usize,
    pub entries: Vec<AccessEntry>,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Count {
    pub key: String,
    pub count: usize,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Bucket {
    /// Bucket start, unix seconds.
    pub start: i64,
    pub count: usize,
    /// Responses with status >= 400.
    pub failed: usize,
}

#[derive(Serialize, Debug)]
pub struct AccessStats {
    pub total: usize,
    pub failed: usize,
    pub by_client: Vec<Count>,
    pub by_user: Vec<Count>,
    pub by_group: Vec<Count>,
    pub by_status: Vec<Count>,
    pub over_time: Vec<Bucket>,
}

fn top(m: BTreeMap<String, usize>) -> Vec<Count> {
    let mut v: Vec<Count> = m
        .into_iter()
        .map(|(key, count)| Count { key, count })
        .collect();
    v.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.key.cmp(&b.key)));
    v.truncate(25);
    v
}

pub fn compute_stats(entries: &[AccessEntry], bucket_secs: i64) -> AccessStats {
    let bucket = bucket_secs.clamp(10, 86_400);
    let (mut c, mut u, mut g, mut s) = (
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
    );
    let mut t: BTreeMap<i64, (usize, usize)> = BTreeMap::new();
    let mut failed = 0;
    for e in entries {
        *c.entry(e.client.clone()).or_insert(0) += 1;
        *u.entry(e.user.clone().unwrap_or_else(|| "(none)".into()))
            .or_insert(0) += 1;
        *g.entry(e.path_group.clone()).or_insert(0) += 1;
        *s.entry(e.status.to_string()).or_insert(0) += 1;
        let b = (e.ts_ms / 1000).div_euclid(bucket) * bucket;
        let slot = t.entry(b).or_insert((0, 0));
        slot.0 += 1;
        if e.status >= 400 {
            slot.1 += 1;
            failed += 1;
        }
    }
    AccessStats {
        total: entries.len(),
        failed,
        by_client: top(c),
        by_user: top(u),
        by_group: top(g),
        by_status: top(s),
        over_time: t
            .into_iter()
            .map(|(start, (count, failed))| Bucket {
                start,
                count,
                failed,
            })
            .collect(),
    }
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/access-log", get(list))
        .route("/api/access-log/stats", get(stats))
}

fn current() -> Result<&'static Arc<AccessLog>, ApiError> {
    LOG.get()
        .ok_or_else(|| ApiError::Internal("the access log is not initialised".into()))
}

async fn list(Query(q): Query<AccessQuery>) -> Result<Json<AccessPage>, ApiError> {
    let log = current()?;
    let v = filter_entries(log.all(), &q);
    let total = v.len();
    let limit = q.limit.unwrap_or(100).clamp(1, 5000);
    let entries = v
        .into_iter()
        .skip(q.offset.unwrap_or(0))
        .take(limit)
        .collect();
    Ok(Json(AccessPage {
        enabled: log.enabled,
        total,
        entries,
    }))
}

async fn stats(Query(q): Query<AccessQuery>) -> Result<Json<AccessStats>, ApiError> {
    let v = filter_entries(current()?.all(), &q);
    Ok(Json(compute_stats(&v, q.bucket_secs.unwrap_or(3600))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(ts: i64, user: Option<&str>, client: &str, path: &str, status: u16) -> AccessEntry {
        AccessEntry {
            ts_ms: ts * 1000,
            node: "n1".into(),
            user: user.map(String::from),
            auth: None,
            client: client.into(),
            method: "GET".into(),
            path: path.into(),
            path_group: path_group(path),
            status,
            duration_ms: 1,
            bytes: None,
            user_agent: None,
        }
    }

    #[test]
    fn groups_and_client_trust() {
        assert_eq!(path_group("/api/vms/abc/start"), "vms");
        assert_eq!(path_group("/api/"), "other");
        assert_eq!(path_group("/metrics"), "other");
        let trusted = vec!["10.0.0.1".to_string()];
        assert_eq!(
            client_address("10.0.0.1", Some("1.2.3.4, 10.0.0.1"), &trusted),
            "1.2.3.4"
        );
        // an untrusted peer cannot forge its address
        assert_eq!(
            client_address("9.9.9.9", Some("1.2.3.4"), &trusted),
            "9.9.9.9"
        );
        assert_eq!(client_address("10.0.0.1", None, &trusted), "10.0.0.1");
    }

    #[test]
    fn filters_by_time_status_user_and_path() {
        let all = vec![
            e(100, Some("admin"), "1.1.1.1", "/api/vms", 200),
            e(200, Some("bob"), "2.2.2.2", "/api/vms/x/start", 403),
            e(300, None, "2.2.2.2", "/api/storage", 401),
        ];
        let q = |f: &dyn Fn(&mut AccessQuery)| {
            let mut q = AccessQuery::default();
            f(&mut q);
            filter_entries(all.clone(), &q)
        };
        assert_eq!(q(&|_| {}).first().unwrap().ts_ms, 300_000, "newest first");
        assert_eq!(q(&|q| q.status = Some("4xx".into())).len(), 2);
        assert_eq!(q(&|q| q.status = Some("403".into())).len(), 1);
        assert_eq!(q(&|q| q.user = Some("bob".into())).len(), 1);
        assert_eq!(q(&|q| q.client = Some("2.2.2.2".into())).len(), 2);
        assert_eq!(q(&|q| q.path = Some("START".into())).len(), 1);
        assert_eq!(q(&|q| q.group = Some("storage".into())).len(), 1);
        assert_eq!(
            q(&|q| {
                q.start = Some(150);
                q.end = Some(250);
            })
            .len(),
            1
        );
        assert_eq!(q(&|q| q.status = Some("all".into())).len(), 3);
    }

    #[test]
    fn stats_count_by_client_user_group_status_and_time() {
        let all = vec![
            e(100, Some("admin"), "1.1.1.1", "/api/vms", 200),
            e(110, Some("admin"), "1.1.1.1", "/api/vms", 500),
            e(4000, None, "2.2.2.2", "/api/storage", 401),
        ];
        let s = compute_stats(&all, 3600);
        assert_eq!((s.total, s.failed), (3, 2));
        assert_eq!(
            s.by_client[0],
            Count {
                key: "1.1.1.1".into(),
                count: 2
            }
        );
        assert_eq!(s.by_user[0].key, "admin");
        assert_eq!(s.by_user[1].key, "(none)");
        assert_eq!(
            s.by_group[0],
            Count {
                key: "vms".into(),
                count: 2
            }
        );
        assert_eq!(
            s.over_time,
            vec![
                Bucket {
                    start: 0,
                    count: 2,
                    failed: 1
                },
                Bucket {
                    start: 3600,
                    count: 1,
                    failed: 1
                },
            ]
        );
    }

    #[test]
    fn retention_is_bounded_and_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("hx-accesslog-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("access-log.jsonl");
        let _ = std::fs::remove_file(&file);
        let cfg = horcrux_core::config::AccessLogConfig {
            max_entries: 3,
            ..Default::default()
        };
        let log = AccessLog::new(&cfg, Some(file.clone()));
        let now = now_ms() / 1000;
        for i in 0..5 {
            log.record(e(now - 5 + i, Some("a"), "c", &format!("/api/p{i}"), 200));
        }
        assert_eq!(log.all().len(), 3);
        assert_eq!(log.all()[0].path, "/api/p2");
        let again = AccessLog::new(&cfg, Some(file));
        assert_eq!(again.all().len(), 3);
        assert_eq!(again.all()[2].path, "/api/p4");
        // an old entry is dropped on load
        let old = AccessLog::new(&cfg, None);
        old.record(e(1, Some("a"), "c", "/api/old", 200));
        assert!(old.all().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
