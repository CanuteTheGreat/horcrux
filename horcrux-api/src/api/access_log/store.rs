//! SQLite storage of the access log: schema, batch insert, filtered list,
//! statistics, retention. One file per node, separate from the main database
//! so request logging can never contend with it.
use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Row, SqlitePool};
use std::path::Path;

/// One recorded request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Entry {
    #[serde(default)]
    pub id: i64,
    /// Request arrival, milliseconds since the Unix epoch.
    pub ts_ms: i64,
    /// RFC 3339 form of `ts_ms` (filled in when read).
    #[serde(default)]
    pub time: String,
    pub node: String,
    pub user: Option<String>,
    pub realm: Option<String>,
    /// `jwt`, `session`, `api_key`, `cluster`; `None` = no credential.
    pub auth_method: Option<String>,
    /// API token id and name (never the secret).
    pub token_id: Option<String>,
    pub token_name: Option<String>,
    pub client: String,
    pub method: String,
    pub path: String,
    /// The route template (`/api/vms/:id/start`).
    pub path_group: String,
    pub status: u16,
    /// Time until the response headers were ready.
    pub duration_ms: f64,
    /// Response size when it is known up front (`None` for streams).
    pub bytes_out: Option<i64>,
    pub user_agent: Option<String>,
}

pub fn iso(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .unwrap_or_default()
}

/// A filter as the API receives it (everything optional, plain strings so
/// an empty form field is simply "no filter").
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Filter {
    /// Start of the range: Unix seconds (or ms), or RFC 3339 / `YYYY-MM-DDTHH:MM:SS` (UTC).
    pub start: Option<String>,
    /// End of the range (inclusive of that second), same formats.
    pub end: Option<String>,
    pub user: Option<String>,
    pub realm: Option<String>,
    /// API token id or name.
    pub token: Option<String>,
    pub client: Option<String>,
    pub auth_method: Option<String>,
    pub method: Option<String>,
    /// Substring of the request path (case-insensitive).
    pub path: Option<String>,
    /// Route template, exact.
    pub group: Option<String>,
    /// `404`, or a class: `2xx`, `4xx`, `5xx`.
    pub status: Option<String>,
    /// `local` (default), `all` or a node name (cluster).
    pub node: Option<String>,
    pub limit: Option<String>,
    pub offset: Option<String>,
    pub buckets: Option<String>,
}

fn clean(o: &Option<String>) -> Option<String> {
    o.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Parse a time bound. `end` bounds are inclusive of their last millisecond
/// when given with second precision.
pub fn parse_time(s: &str, is_end: bool) -> Result<i64, String> {
    let s = s.trim();
    if let Ok(n) = s.parse::<i64>() {
        // Unix seconds, or milliseconds when too large to be seconds.
        return Ok(if n.abs() >= 100_000_000_000 {
            n
        } else {
            n * 1000 + if is_end { 999 } else { 0 }
        });
    }
    if let Ok(d) = chrono::DateTime::parse_from_rfc3339(s) {
        let ms = d.timestamp_millis();
        let whole = d.timestamp_subsec_millis() == 0;
        return Ok(if is_end && whole { ms + 999 } else { ms });
    }
    for fmt in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M"] {
        if let Ok(d) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            let ms = d.and_utc().timestamp_millis();
            return Ok(if is_end { ms + 999 } else { ms });
        }
    }
    Err(format!(
        "{s:?} is not a time (Unix seconds, or 2026-11-25T14:03:07 / RFC 3339, UTC)"
    ))
}

/// A filter with parsed values, ready for SQL.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Resolved {
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub user: Option<String>,
    pub realm: Option<String>,
    pub token: Option<String>,
    pub client: Option<String>,
    pub auth_method: Option<String>,
    pub method: Option<String>,
    pub path: Option<String>,
    pub group: Option<String>,
    pub status: Option<(u16, u16)>,
}

impl Filter {
    pub fn resolve(&self) -> Result<Resolved, String> {
        let start_ms = clean(&self.start)
            .map(|s| parse_time(&s, false))
            .transpose()?;
        let end_ms = clean(&self.end).map(|s| parse_time(&s, true)).transpose()?;
        if let (Some(a), Some(b)) = (start_ms, end_ms) {
            if a > b {
                return Err("start is after end".into());
            }
        }
        let status = match clean(&self.status) {
            None => None,
            Some(s) => Some(
                horcrux_common::access_log::parse_status_filter(&s)
                    .ok_or_else(|| format!("status {s:?}: use 404 or 2xx/3xx/4xx/5xx"))?,
            ),
        };
        Ok(Resolved {
            start_ms,
            end_ms,
            user: clean(&self.user),
            realm: clean(&self.realm),
            token: clean(&self.token),
            client: clean(&self.client),
            auth_method: clean(&self.auth_method),
            method: clean(&self.method).map(|m| m.to_ascii_uppercase()),
            path: clean(&self.path),
            group: clean(&self.group),
            status,
        })
    }

    pub fn int(v: &Option<String>, name: &str) -> Result<Option<u32>, String> {
        match clean(v) {
            None => Ok(None),
            Some(s) => s
                .parse::<u32>()
                .map(Some)
                .map_err(|_| format!("{name}: a whole number")),
        }
    }
}

#[derive(Debug, Clone)]
enum Val {
    I(i64),
    S(String),
}

impl Resolved {
    /// `WHERE` conditions (no keyword) and their values, in placeholder order.
    fn conditions(&self) -> (String, Vec<Val>) {
        let mut c: Vec<String> = Vec::new();
        let mut v: Vec<Val> = Vec::new();
        if let Some(t) = self.start_ms {
            c.push("ts >= ?".into());
            v.push(Val::I(t));
        }
        if let Some(t) = self.end_ms {
            c.push("ts <= ?".into());
            v.push(Val::I(t));
        }
        for (col, val) in [
            ("user", &self.user),
            ("realm", &self.realm),
            ("client", &self.client),
            ("auth_method", &self.auth_method),
            ("method", &self.method),
            ("path_group", &self.group),
        ] {
            if let Some(x) = val {
                c.push(format!("{col} = ?"));
                v.push(Val::S(x.clone()));
            }
        }
        if let Some(t) = &self.token {
            c.push("(token_id = ? OR token_name = ?)".into());
            v.push(Val::S(t.clone()));
            v.push(Val::S(t.clone()));
        }
        if let Some(p) = &self.path {
            c.push("instr(lower(path), lower(?)) > 0".into());
            v.push(Val::S(p.clone()));
        }
        if let Some((lo, hi)) = self.status {
            c.push("status BETWEEN ? AND ?".into());
            v.push(Val::I(lo as i64));
            v.push(Val::I(hi as i64));
        }
        let sql = if c.is_empty() {
            "1=1".to_string()
        } else {
            c.join(" AND ")
        };
        (sql, v)
    }
}

fn bind<'q>(
    mut q: sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>,
    vals: &'q [Val],
) -> sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>> {
    for v in vals {
        q = match v {
            Val::I(i) => q.bind(*i),
            Val::S(s) => q.bind(s.as_str()),
        };
    }
    q
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TopItem {
    pub value: String,
    pub label: Option<String>,
    pub count: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Bucket {
    pub start_ms: i64,
    pub total: i64,
    pub s2xx: i64,
    pub s3xx: i64,
    pub s4xx: i64,
    pub s5xx: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Stats {
    pub total: i64,
    /// 4xx plus 5xx.
    pub errors: i64,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub bucket_ms: i64,
    pub buckets: Vec<Bucket>,
    pub avg_duration_ms: f64,
    pub bytes_out: i64,
    pub top_clients: Vec<TopItem>,
    pub top_users: Vec<TopItem>,
    pub top_tokens: Vec<TopItem>,
    pub top_groups: Vec<TopItem>,
    pub top_statuses: Vec<TopItem>,
    pub top_methods: Vec<TopItem>,
}

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
    path: std::path::PathBuf,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS access_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ts INTEGER NOT NULL,
    node TEXT NOT NULL,
    user TEXT,
    realm TEXT,
    auth_method TEXT,
    token_id TEXT,
    token_name TEXT,
    client TEXT NOT NULL,
    method TEXT NOT NULL,
    path TEXT NOT NULL,
    path_group TEXT NOT NULL,
    status INTEGER NOT NULL,
    duration_us INTEGER NOT NULL,
    bytes_out INTEGER,
    user_agent TEXT
);
CREATE INDEX IF NOT EXISTS idx_access_log_ts ON access_log(ts);
CREATE INDEX IF NOT EXISTS idx_access_log_user ON access_log(user, ts);
CREATE INDEX IF NOT EXISTS idx_access_log_token ON access_log(token_id, ts);
CREATE INDEX IF NOT EXISTS idx_access_log_client ON access_log(client, ts);
CREATE INDEX IF NOT EXISTS idx_access_log_status ON access_log(status, ts);
CREATE INDEX IF NOT EXISTS idx_access_log_group ON access_log(path_group, ts);
";

const COLS: &str = "id, ts, node, user, realm, auth_method, token_id, token_name, client, method, \
                    path, path_group, status, duration_us, bytes_out, user_agent";

fn row_entry(r: &sqlx::sqlite::SqliteRow) -> Entry {
    let ts: i64 = r.get("ts");
    Entry {
        id: r.get("id"),
        ts_ms: ts,
        time: iso(ts),
        node: r.get("node"),
        user: r.get("user"),
        realm: r.get("realm"),
        auth_method: r.get("auth_method"),
        token_id: r.get("token_id"),
        token_name: r.get("token_name"),
        client: r.get("client"),
        method: r.get("method"),
        path: r.get("path"),
        path_group: r.get("path_group"),
        status: r.get::<i64, _>("status") as u16,
        duration_ms: r.get::<i64, _>("duration_us") as f64 / 1000.0,
        bytes_out: r.get("bytes_out"),
        user_agent: r.get("user_agent"),
    }
}

impl Store {
    pub async fn open(path: &Path) -> Result<Store, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(std::time::Duration::from_secs(10))
            // must be set before the first table exists to take effect
            .pragma("auto_vacuum", "INCREMENTAL");
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await
            .map_err(|e| format!("{}: {e}", path.display()))?;
        sqlx::raw_sql(SCHEMA)
            .execute(&pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(Store {
            pool,
            path: path.to_path_buf(),
        })
    }

    pub async fn insert_batch(&self, batch: &[Entry]) -> Result<(), String> {
        if batch.is_empty() {
            return Ok(());
        }
        let mut tx = self.pool.begin().await.map_err(|e| e.to_string())?;
        for e in batch {
            sqlx::query(
                "INSERT INTO access_log (ts, node, user, realm, auth_method, token_id, token_name, \
                 client, method, path, path_group, status, duration_us, bytes_out, user_agent) \
                 VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            )
            .bind(e.ts_ms)
            .bind(&e.node)
            .bind(&e.user)
            .bind(&e.realm)
            .bind(&e.auth_method)
            .bind(&e.token_id)
            .bind(&e.token_name)
            .bind(&e.client)
            .bind(&e.method)
            .bind(&e.path)
            .bind(&e.path_group)
            .bind(e.status as i64)
            .bind((e.duration_ms * 1000.0).round() as i64)
            .bind(e.bytes_out)
            .bind(&e.user_agent)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        }
        tx.commit().await.map_err(|e| e.to_string())
    }

    /// Newest first. Returns the number of matching rows and one page.
    pub async fn list(
        &self,
        f: &Resolved,
        limit: u32,
        offset: u32,
    ) -> Result<(i64, Vec<Entry>), String> {
        let (cond, vals) = f.conditions();
        let total: i64 = bind(
            sqlx::query(&format!("SELECT COUNT(*) FROM access_log WHERE {cond}")),
            &vals,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| e.to_string())?
        .get(0);
        let sql = format!(
            "SELECT {COLS} FROM access_log WHERE {cond} ORDER BY ts DESC, id DESC LIMIT {limit} OFFSET {offset}"
        );
        let rows = bind(sqlx::query(&sql), &vals)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok((total, rows.iter().map(row_entry).collect()))
    }

    pub async fn get(&self, id: i64) -> Result<Option<Entry>, String> {
        let r = sqlx::query(&format!("SELECT {COLS} FROM access_log WHERE id = ?"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(r.as_ref().map(row_entry))
    }

    async fn top(
        &self,
        cond: &str,
        vals: &[Val],
        col: &str,
        label: Option<&str>,
        n: u32,
    ) -> Result<Vec<TopItem>, String> {
        let label_sql = label.map(|l| format!(", MAX({l})")).unwrap_or_default();
        let sql = format!(
            "SELECT CAST({col} AS TEXT) AS v, COUNT(*) AS n{label_sql} FROM access_log \
             WHERE {cond} AND {col} IS NOT NULL AND CAST({col} AS TEXT) != '' \
             GROUP BY {col} ORDER BY n DESC, v ASC LIMIT {n}"
        );
        let rows = bind(sqlx::query(&sql), vals)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(rows
            .iter()
            .map(|r| TopItem {
                value: r.get("v"),
                count: r.get("n"),
                label: if label.is_some() { r.get(2) } else { None },
            })
            .collect())
    }

    pub async fn stats(&self, f: &Resolved, buckets: u32, top_n: u32) -> Result<Stats, String> {
        let (cond, vals) = f.conditions();
        let r = bind(
            sqlx::query(&format!(
                "SELECT COUNT(*), MIN(ts), MAX(ts), COALESCE(AVG(duration_us),0.0), \
                 COALESCE(SUM(bytes_out),0), COALESCE(SUM(status >= 400),0) \
                 FROM access_log WHERE {cond}"
            )),
            &vals,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        let total: i64 = r.get(0);
        let mut st = Stats {
            total,
            errors: r.get(5),
            avg_duration_ms: r.get::<f64, _>(3) / 1000.0,
            bytes_out: r.get(4),
            ..Default::default()
        };
        if total == 0 {
            st.start_ms = f.start_ms;
            st.end_ms = f.end_ms;
            st.bucket_ms = bucket_width(f.start_ms, f.end_ms, buckets);
            st.buckets = empty_buckets(f.start_ms, f.end_ms, st.bucket_ms);
            return Ok(st);
        }
        let first: i64 = r.get(1);
        let last: i64 = r.get(2);
        let start = f.start_ms.unwrap_or(first);
        let end = f.end_ms.unwrap_or(last);
        st.start_ms = Some(start);
        st.end_ms = Some(end);
        st.bucket_ms = bucket_width(Some(start), Some(end), buckets);
        let mut out = empty_buckets(Some(start), Some(end), st.bucket_ms);
        let sql = format!(
            "SELECT (ts - ?) / ? AS b, COUNT(*), \
             COALESCE(SUM(status BETWEEN 200 AND 299),0), COALESCE(SUM(status BETWEEN 300 AND 399),0), \
             COALESCE(SUM(status BETWEEN 400 AND 499),0), COALESCE(SUM(status >= 500),0) \
             FROM access_log WHERE {cond} GROUP BY b"
        );
        let mut all = vec![Val::I(start), Val::I(st.bucket_ms)];
        all.extend(vals.iter().cloned());
        let rows = bind(sqlx::query(&sql), &all)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        for row in rows {
            let b: i64 = row.get(0);
            if let Some(slot) = usize::try_from(b).ok().and_then(|i| out.get_mut(i)) {
                slot.total = row.get(1);
                slot.s2xx = row.get(2);
                slot.s3xx = row.get(3);
                slot.s4xx = row.get(4);
                slot.s5xx = row.get(5);
            }
        }
        st.buckets = out;
        st.top_clients = self.top(&cond, &vals, "client", None, top_n).await?;
        st.top_users = self.top(&cond, &vals, "user", None, top_n).await?;
        st.top_tokens = self
            .top(&cond, &vals, "token_id", Some("token_name"), top_n)
            .await?;
        st.top_groups = self.top(&cond, &vals, "path_group", None, top_n).await?;
        st.top_statuses = self.top(&cond, &vals, "status", None, top_n).await?;
        st.top_methods = self.top(&cond, &vals, "method", None, top_n).await?;
        Ok(st)
    }

    /// Bytes the entries occupy (free pages do not count).
    pub async fn used_bytes(&self) -> Result<u64, String> {
        let pages: i64 = sqlx::query("PRAGMA page_count")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| e.to_string())?
            .get(0);
        let free: i64 = sqlx::query("PRAGMA freelist_count")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| e.to_string())?
            .get(0);
        let size: i64 = sqlx::query("PRAGMA page_size")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| e.to_string())?
            .get(0);
        Ok(((pages - free).max(0) * size) as u64)
    }

    pub async fn count(&self) -> Result<i64, String> {
        Ok(sqlx::query("SELECT COUNT(*) FROM access_log")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| e.to_string())?
            .get(0))
    }

    /// Enforce retention: entries older than `retention_days` go, then the
    /// oldest entries until the data fits in `max_mb`. Returns the number of
    /// deleted entries.
    pub async fn prune(
        &self,
        retention_days: u32,
        max_mb: u32,
        now_ms: i64,
    ) -> Result<u64, String> {
        let cutoff = now_ms - i64::from(retention_days) * 86_400_000;
        let mut deleted = sqlx::query("DELETE FROM access_log WHERE ts < ?")
            .bind(cutoff)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?
            .rows_affected();
        let limit = u64::from(max_mb) * 1024 * 1024;
        for _ in 0..8 {
            let used = self.used_bytes().await?;
            if used <= limit {
                break;
            }
            let rows = self.count().await?;
            if rows == 0 {
                break;
            }
            // Drop the oldest share that is over the limit, plus 10 % slack
            // so the next pruning does not immediately find it full again.
            let over = (used - limit) as f64 / used as f64;
            let n = ((rows as f64) * (over + 0.1)).ceil().max(1.0) as i64;
            deleted += sqlx::query(
                "DELETE FROM access_log WHERE id IN (SELECT id FROM access_log ORDER BY ts ASC, id ASC LIMIT ?)",
            )
            .bind(n)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?
            .rows_affected();
            let _ = sqlx::query("PRAGMA incremental_vacuum")
                .execute(&self.pool)
                .await;
        }
        if deleted > 0 {
            let _ = sqlx::query("PRAGMA incremental_vacuum")
                .execute(&self.pool)
                .await;
            let _ = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
                .execute(&self.pool)
                .await;
        }
        Ok(deleted)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Width of one statistics bucket: the range split into at most `buckets`
/// parts, in whole seconds (at least one).
pub fn bucket_width(start: Option<i64>, end: Option<i64>, buckets: u32) -> i64 {
    let (Some(s), Some(e)) = (start, end) else {
        return 1000;
    };
    let span = (e - s + 1).max(1);
    let n = i64::from(buckets.max(1));
    ((span + n - 1) / n + 999) / 1000 * 1000
}

fn empty_buckets(start: Option<i64>, end: Option<i64>, width: i64) -> Vec<Bucket> {
    let (Some(s), Some(e)) = (start, end) else {
        return Vec::new();
    };
    let n = ((e - s) / width + 1).clamp(1, 5000);
    (0..n)
        .map(|i| Bucket {
            start_ms: s + i * width,
            ..Default::default()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(ts: i64, user: &str, status: u16, group: &str, client: &str) -> Entry {
        Entry {
            id: 0,
            ts_ms: ts,
            time: String::new(),
            node: "n1".into(),
            user: Some(user.into()),
            realm: Some("local".into()),
            auth_method: Some("jwt".into()),
            token_id: None,
            token_name: None,
            client: client.into(),
            method: "GET".into(),
            path: group.replace(":id", "7"),
            path_group: group.into(),
            status,
            duration_ms: 1.5,
            bytes_out: Some(10),
            user_agent: None,
        }
    }

    async fn store() -> (Store, tempfile::TempDir) {
        let d = tempfile::tempdir().unwrap();
        (Store::open(&d.path().join("a.db")).await.unwrap(), d)
    }

    #[test]
    fn time_parsing_has_second_precision() {
        assert_eq!(parse_time("1700000000", false).unwrap(), 1_700_000_000_000);
        assert_eq!(parse_time("1700000000", true).unwrap(), 1_700_000_000_999);
        assert_eq!(
            parse_time("2026-11-25T14:03:07", false).unwrap(),
            parse_time("2026-11-25T14:03:07Z", false).unwrap()
        );
        assert_eq!(
            parse_time("2026-11-25T14:03:07", true).unwrap()
                - parse_time("2026-11-25T14:03:07", false).unwrap(),
            999
        );
        assert!(parse_time("yesterday", false).is_err());
        assert!(Filter {
            start: Some("2026-11-25T14:00:00".into()),
            end: Some("2026-11-25T13:00:00".into()),
            ..Default::default()
        }
        .resolve()
        .is_err());
    }

    #[test]
    fn bucket_widths() {
        assert_eq!(bucket_width(Some(0), Some(59_999), 60), 1000);
        assert_eq!(bucket_width(Some(0), Some(3_599_999), 60), 60_000);
        assert_eq!(bucket_width(None, None, 10), 1000);
    }

    #[tokio::test]
    async fn filters_list_and_stats() {
        let (s, _d) = store().await;
        let base = 1_700_000_000_000;
        let mut batch = vec![
            entry(base, "alice", 200, "/api/vms", "198.51.100.1"),
            entry(base + 1000, "alice", 404, "/api/vms/:id", "198.51.100.1"),
            entry(base + 2000, "bob", 500, "/api/vms/:id/start", "203.0.113.4"),
            entry(base + 3000, "bob", 200, "/api/vms", "203.0.113.4"),
        ];
        batch[3].token_id = Some("key-1".into());
        batch[3].token_name = Some("ci".into());
        s.insert_batch(&batch).await.unwrap();

        let q = |f: Filter| f.resolve().unwrap();
        let (n, rows) = s.list(&q(Filter::default()), 10, 0).await.unwrap();
        assert_eq!(n, 4);
        assert_eq!(rows[0].ts_ms, base + 3000, "newest first");
        assert_eq!(rows[0].token_name.as_deref(), Some("ci"));
        assert!(rows[0].time.ends_with('Z'));

        let by = |f: Filter| {
            let s = s.clone();
            async move { s.list(&f.resolve().unwrap(), 10, 0).await.unwrap() }
        };
        assert_eq!(
            by(Filter {
                user: Some("bob".into()),
                ..Default::default()
            })
            .await
            .0,
            2
        );
        assert_eq!(
            by(Filter {
                token: Some("key-1".into()),
                ..Default::default()
            })
            .await
            .0,
            1
        );
        assert_eq!(
            by(Filter {
                token: Some("ci".into()),
                ..Default::default()
            })
            .await
            .0,
            1
        );
        assert_eq!(
            by(Filter {
                client: Some("198.51.100.1".into()),
                ..Default::default()
            })
            .await
            .0,
            2
        );
        assert_eq!(
            by(Filter {
                status: Some("4xx".into()),
                ..Default::default()
            })
            .await
            .0,
            1
        );
        assert_eq!(
            by(Filter {
                status: Some("5xx".into()),
                ..Default::default()
            })
            .await
            .0,
            1
        );
        assert_eq!(
            by(Filter {
                status: Some("200".into()),
                ..Default::default()
            })
            .await
            .0,
            2
        );
        assert_eq!(
            by(Filter {
                path: Some("START".into()),
                ..Default::default()
            })
            .await
            .0,
            1,
            "path substring, any case"
        );
        assert_eq!(
            by(Filter {
                group: Some("/api/vms/:id".into()),
                ..Default::default()
            })
            .await
            .0,
            1
        );
        assert_eq!(
            by(Filter {
                method: Some("get".into()),
                ..Default::default()
            })
            .await
            .0,
            4
        );
        // second precision: base is a whole second, so [base, base] holds one
        let sec = (base / 1000).to_string();
        assert_eq!(
            by(Filter {
                start: Some(sec.clone()),
                end: Some(sec),
                ..Default::default()
            })
            .await
            .0,
            1
        );
        // paging
        let (n, p2) = s.list(&q(Filter::default()), 2, 2).await.unwrap();
        assert_eq!((n, p2.len()), (4, 2));
        assert_eq!(p2[0].ts_ms, base + 1000);
        // detail
        let id = rows[0].id;
        assert_eq!(s.get(id).await.unwrap().unwrap().ts_ms, base + 3000);
        assert!(s.get(9999).await.unwrap().is_none());

        let st = s.stats(&q(Filter::default()), 4, 5).await.unwrap();
        assert_eq!(st.total, 4);
        assert_eq!(st.errors, 2);
        assert_eq!(st.buckets.iter().map(|b| b.total).sum::<i64>(), 4);
        assert_eq!(st.top_users[0].count, 2);
        assert_eq!(st.top_groups[0].value, "/api/vms");
        assert_eq!(st.top_tokens[0].value, "key-1");
        assert_eq!(st.top_tokens[0].label.as_deref(), Some("ci"));
        assert_eq!(st.top_clients.len(), 2);
        assert!(st.top_statuses.iter().any(|t| t.value == "404"));
        let st = s
            .stats(
                &q(Filter {
                    user: Some("alice".into()),
                    ..Default::default()
                }),
                4,
                5,
            )
            .await
            .unwrap();
        assert_eq!((st.total, st.errors), (2, 1));
        // empty result
        let st = s
            .stats(
                &q(Filter {
                    user: Some("nobody".into()),
                    ..Default::default()
                }),
                4,
                5,
            )
            .await
            .unwrap();
        assert_eq!(st.total, 0);
        assert!(st.top_users.is_empty());
    }

    #[tokio::test]
    async fn retention_by_age_and_size() {
        let (s, _d) = store().await;
        let now = 1_700_000_000_000;
        let day = 86_400_000;
        let mut v = vec![entry(now - 40 * day, "old", 200, "/api/x", "1.1.1.1")];
        v.push(entry(now - 10 * day, "mid", 200, "/api/x", "1.1.1.1"));
        v.push(entry(now, "new", 200, "/api/x", "1.1.1.1"));
        s.insert_batch(&v).await.unwrap();
        assert_eq!(s.prune(30, 256, now).await.unwrap(), 1);
        assert_eq!(s.count().await.unwrap(), 2);
        assert_eq!(s.prune(5, 256, now).await.unwrap(), 1);
        let (_, rows) = s.list(&Resolved::default(), 10, 0).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].user.as_deref(), Some("new"));

        // size limit: fill ~3 MiB, keep 1 MiB; the newest entries survive
        let big: Vec<Entry> = (0..20_000)
            .map(|i| {
                let mut e = entry(now - 20_000 + i, "u", 200, "/api/bulk", "2.2.2.2");
                e.user_agent = Some("x".repeat(100));
                e.path = format!("/api/bulk/{}", "p".repeat(60));
                e
            })
            .collect();
        s.insert_batch(&big).await.unwrap();
        let before = s.used_bytes().await.unwrap();
        assert!(before > 1024 * 1024, "{before}");
        let deleted = s.prune(30, 1, now).await.unwrap();
        assert!(deleted > 0);
        assert!(s.used_bytes().await.unwrap() <= 1024 * 1024);
        let (_, newest) = s.list(&Resolved::default(), 1, 0).await.unwrap();
        assert_eq!(newest[0].user.as_deref(), Some("new"));
    }
}
