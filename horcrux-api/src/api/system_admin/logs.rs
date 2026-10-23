//! System log viewer: journald when the host runs it, otherwise the syslog
//! file (`/var/log/messages` on Gentoo with syslog-ng/metalog, or `syslog`).
use super::{rooted, run, run_secs, user_of, which};
use crate::api::tasks;
use crate::error::ApiError;
use crate::AppState;
use axum::{
    extract::Query, http::header, response::IntoResponse, routing::get, Extension, Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/system/logs", get(list).delete(clear))
        .route("/api/system/logs/export", get(export))
}

type AuthExt = Option<Extension<crate::middleware::auth::AuthUser>>;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LogEntry {
    pub timestamp: String,
    pub level: String,
    pub service: Option<String>,
    pub message: String,
    pub details: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct Filters {
    pub level: Option<String>,
    pub service: Option<String>,
    pub search: Option<String>,
    pub limit: Option<usize>,
    pub format: Option<String>,
}

const MAX: usize = 5000;

fn journald_present() -> bool {
    which("journalctl").is_some()
        && (rooted("run/log/journal").is_dir() || rooted("var/log/journal").is_dir())
}

fn syslog_file() -> Option<std::path::PathBuf> {
    [
        "var/log/messages",
        "var/log/syslog",
        "var/log/everything/current",
    ]
    .iter()
    .map(|p| rooted(p))
    .find(|p| p.is_file())
}

pub fn level_rank(l: &str) -> u8 {
    match l {
        "error" => 3,
        "warn" | "warning" => 2,
        "info" => 1,
        _ => 0,
    }
}

pub fn infer_level(msg: &str) -> &'static str {
    let m = msg.to_ascii_lowercase();
    if ["error", "fail", "fatal", "crit", "panic", "denied"]
        .iter()
        .any(|k| m.contains(k))
    {
        "error"
    } else if m.contains("warn") {
        "warn"
    } else if m.contains("debug") {
        "debug"
    } else {
        "info"
    }
}

/// One syslog line, either classic (`Oct 22 12:00:01 host prog[12]: msg`) or
/// ISO (`2026-10-22T12:00:01.1+00:00 host prog[12]: msg`).
pub fn parse_syslog_line(line: &str, year: i32) -> Option<LogEntry> {
    let line = line.trim_end();
    let (ts, rest) = if line.len() > 20 && line.as_bytes()[4] == b'-' {
        let (t, r) = line.split_once(' ')?;
        (t.to_string(), r)
    } else if line.len() > 16 {
        let (head, r) = (&line[..15], &line[15..]);
        let mut it = head.split_whitespace();
        let (mon, day, time) = (it.next()?, it.next()?, it.next()?);
        let m = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ]
        .iter()
        .position(|x| *x == mon)?
            + 1;
        (
            format!("{year}-{m:02}-{:02}T{time}", day.parse::<u32>().ok()?),
            r.trim_start(),
        )
    } else {
        return None;
    };
    let rest = rest.trim_start();
    let (_host, rest) = rest.split_once(' ')?;
    let (tag, msg) = match rest.split_once(": ") {
        Some((t, m)) if !t.contains(' ') => (Some(t), m),
        _ => (None, rest),
    };
    let service = tag.map(|t| t.split('[').next().unwrap_or(t).to_string());
    Some(LogEntry {
        timestamp: ts,
        level: infer_level(msg).into(),
        service,
        message: msg.to_string(),
        details: None,
    })
}

/// One `journalctl -o json` object.
pub fn parse_journal_json(line: &str) -> Option<LogEntry> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(|x| x.to_string());
    let micros: i64 = s("__REALTIME_TIMESTAMP")?.parse().ok()?;
    let ts =
        chrono::DateTime::from_timestamp(micros / 1_000_000, ((micros % 1_000_000) * 1000) as u32)?
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let level = match s("PRIORITY")
        .and_then(|p| p.parse::<u8>().ok())
        .unwrap_or(6)
    {
        0..=3 => "error",
        4 => "warn",
        5 | 6 => "info",
        _ => "debug",
    };
    // MESSAGE is an array of bytes for binary messages.
    let message = match v.get("MESSAGE") {
        Some(serde_json::Value::String(m)) => m.clone(),
        Some(serde_json::Value::Array(b)) => String::from_utf8_lossy(
            &b.iter()
                .filter_map(|x| x.as_u64().map(|n| n as u8))
                .collect::<Vec<_>>(),
        )
        .to_string(),
        _ => String::new(),
    };
    Some(LogEntry {
        timestamp: ts,
        level: level.into(),
        service: s("SYSLOG_IDENTIFIER").or_else(|| s("_SYSTEMD_UNIT")),
        details: s("_SYSTEMD_UNIT").filter(|u| Some(u) != s("SYSLOG_IDENTIFIER").as_ref()),
        message,
    })
}

pub fn apply_filters(mut v: Vec<LogEntry>, f: &Filters) -> Vec<LogEntry> {
    if let Some(l) = f.level.as_deref().filter(|l| !l.is_empty() && *l != "all") {
        let min = level_rank(l);
        v.retain(|e| level_rank(&e.level) >= min);
    }
    if let Some(s) = f
        .service
        .as_deref()
        .filter(|s| !s.is_empty() && *s != "all")
    {
        v.retain(|e| e.service.as_deref().is_some_and(|x| x.contains(s)));
    }
    if let Some(q) = f.search.as_deref().filter(|s| !s.is_empty()) {
        let q = q.to_ascii_lowercase();
        v.retain(|e| {
            e.message.to_ascii_lowercase().contains(&q)
                || e.service
                    .as_deref()
                    .is_some_and(|s| s.to_ascii_lowercase().contains(&q))
        });
    }
    v
}

fn tail_lines(path: &std::path::Path, max_bytes: u64) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return vec![];
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(max_bytes);
    let _ = f.seek(SeekFrom::Start(start));
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    let mut lines = text.lines();
    if start > 0 {
        lines.next(); // a cut-off first line
    }
    lines.map(|l| l.to_string()).collect()
}

/// Newest-first log entries after filtering.
pub async fn read_logs(f: &Filters) -> Result<Vec<LogEntry>, ApiError> {
    let limit = f.limit.unwrap_or(200).clamp(1, MAX);
    let mut entries: Vec<LogEntry> = if journald_present() {
        let n = MAX.to_string();
        let mut args = vec!["-o", "json", "--no-pager", "-r", "-n", n.as_str()];
        let prio;
        if let Some(l) = f.level.as_deref() {
            prio = match l {
                "error" => Some("3"),
                "warn" | "warning" => Some("4"),
                "info" => Some("6"),
                "debug" => Some("7"),
                _ => None,
            };
            if let Some(p) = prio {
                args.push("-p");
                args.push(p);
            }
        }
        let out = run_secs("journalctl", &args, 30)
            .await
            .map_err(ApiError::Internal)?;
        out.lines().filter_map(parse_journal_json).collect()
    } else if let Some(file) = syslog_file() {
        let year = chrono::Datelike::year(&chrono::Local::now());
        let mut v: Vec<LogEntry> = tail_lines(&file, 8 * 1024 * 1024)
            .iter()
            .filter_map(|l| parse_syslog_line(l, year))
            .collect();
        v.reverse();
        v
    } else {
        return Err(ApiError::ServiceUnavailable(
            "no system log source: journald is not running and /var/log/messages (syslog) does not exist".into(),
        ));
    };
    entries = apply_filters(entries, f);
    entries.truncate(limit);
    Ok(entries)
}

async fn list(Query(f): Query<Filters>) -> Result<Json<Vec<LogEntry>>, ApiError> {
    Ok(Json(read_logs(&f).await?))
}

async fn export(Query(f): Query<Filters>) -> Result<impl IntoResponse, ApiError> {
    let f = Filters {
        limit: Some(f.limit.unwrap_or(MAX)),
        ..f
    };
    let entries = read_logs(&f).await?;
    let (ctype, body) = match f.format.as_deref().unwrap_or("txt") {
        "json" => (
            "application/json",
            serde_json::to_string_pretty(&entries).unwrap_or_default(),
        ),
        "csv" => ("text/csv", render_csv(&entries)),
        _ => ("text/plain", render_text(&entries)),
    };
    Ok(([(header::CONTENT_TYPE, ctype)], body))
}

pub fn render_text(v: &[LogEntry]) -> String {
    v.iter()
        .map(|e| {
            format!(
                "{} [{}] {}: {}\n",
                e.timestamp,
                e.level,
                e.service.as_deref().unwrap_or("-"),
                e.message
            )
        })
        .collect()
}

fn csv_field(s: &str) -> String {
    // Quote, and defuse spreadsheet formulas in untrusted log text.
    let s = if s.starts_with(['=', '+', '-', '@']) {
        format!("'{s}")
    } else {
        s.to_string()
    };
    format!("\"{}\"", s.replace('"', "\"\""))
}

pub fn render_csv(v: &[LogEntry]) -> String {
    let mut out = String::from("timestamp,level,service,message\n");
    for e in v {
        out += &format!(
            "{},{},{},{}\n",
            csv_field(&e.timestamp),
            csv_field(&e.level),
            csv_field(e.service.as_deref().unwrap_or("")),
            csv_field(&e.message)
        );
    }
    out
}

async fn clear(u: AuthExt) -> Result<Json<serde_json::Value>, ApiError> {
    let who = user_of(&u);
    let r: Result<String, String> = if journald_present() {
        match run("journalctl", &["--rotate"]).await {
            Ok(_) => run("journalctl", &["--vacuum-time=1s"])
                .await
                .map(|o| format!("journald vacuumed: {}", o.trim())),
            Err(e) => Err(e),
        }
    } else if let Some(f) = syslog_file() {
        std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&f)
            .map(|_| format!("{} truncated", f.display()))
            .map_err(|e| format!("{}: {e}", f.display()))
    } else {
        Err("no system log source".into())
    };
    tasks::record_done("log-clear", "system-logs", &who, &r);
    r.map_err(ApiError::Internal)?;
    Ok(Json(serde_json::json!({"cleared": true})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classic_and_iso_syslog_lines_parse() {
        let e = parse_syslog_line(
            "Oct  2 07:01:09 gw sshd[411]: Failed password for root",
            2026,
        )
        .unwrap();
        assert_eq!(e.timestamp, "2026-10-02T07:01:09");
        assert_eq!(
            (e.service.as_deref(), e.level.as_str()),
            (Some("sshd"), "error")
        );
        let e = parse_syslog_line(
            "2026-10-22T12:00:01.123456-07:00 gw horcrux-api: started on :8006",
            2026,
        )
        .unwrap();
        assert_eq!(
            (e.service.as_deref(), e.level.as_str()),
            (Some("horcrux-api"), "info")
        );
        assert_eq!(e.message, "started on :8006");
        assert!(parse_syslog_line("garbage", 2026).is_none());
    }

    #[test]
    fn journal_json_parses_priorities_and_binary_messages() {
        let e = parse_journal_json(
            r#"{"__REALTIME_TIMESTAMP":"1792720241123456","PRIORITY":"3","SYSLOG_IDENTIFIER":"kernel","MESSAGE":"oops"}"#,
        )
        .unwrap();
        assert_eq!(
            (e.level.as_str(), e.service.as_deref(), e.message.as_str()),
            ("error", Some("kernel"), "oops")
        );
        assert_eq!(e.timestamp, "2026-10-23T01:50:41.123Z");
        let b = parse_journal_json(
            r#"{"__REALTIME_TIMESTAMP":"1000000","MESSAGE":[104,105],"_SYSTEMD_UNIT":"x.service"}"#,
        )
        .unwrap();
        assert_eq!(
            (b.message.as_str(), b.service.as_deref()),
            ("hi", Some("x.service"))
        );
    }

    fn e(l: &str, s: &str, m: &str) -> LogEntry {
        LogEntry {
            timestamp: "t".into(),
            level: l.into(),
            service: Some(s.into()),
            message: m.into(),
            details: None,
        }
    }

    #[test]
    fn filters_apply_severity_service_and_search() {
        let v = vec![
            e("debug", "a", "x"),
            e("info", "b", "hello"),
            e("warn", "b", "low disk"),
            e("error", "c", "boom"),
        ];
        let f = |level: &str, service: &str, search: &str| {
            apply_filters(
                v.clone(),
                &Filters {
                    level: Some(level.into()),
                    service: Some(service.into()),
                    search: Some(search.into()),
                    ..Default::default()
                },
            )
            .len()
        };
        assert_eq!(f("all", "all", ""), 4);
        assert_eq!(f("warn", "all", ""), 2, "warn means warn and worse");
        assert_eq!(f("all", "b", ""), 2);
        assert_eq!(f("all", "all", "DISK"), 1);
    }

    #[test]
    fn csv_export_quotes_and_defuses_formulas() {
        let c = render_csv(&[e("info", "s", "=cmd|' /C calc'!A0 \"x\"")]);
        assert!(c.lines().nth(1).unwrap().contains("\"'=cmd"));
        assert!(c.contains("\"\"x\"\""));
    }

    #[test]
    fn the_tail_of_a_big_file_drops_the_cut_line() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("m");
        std::fs::write(&p, "aaaaaaaa\nline two\nline three\n").unwrap();
        let t = tail_lines(&p, 25);
        assert_eq!(t, vec!["line two".to_string(), "line three".into()]);
        assert_eq!(tail_lines(&p, 1000).len(), 3);
    }
}
