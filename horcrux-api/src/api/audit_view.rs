//! Audit log viewer API: search with filters, the filter choices, and an
//! export. Reads the audit logger's events (which are also appended to the
//! audit log file and reloaded from it when the API starts).
//!
//! * `GET /api/audit/search?event_type=&user=&severity=&result=&search=&start_time=&end_time=&limit=&offset=`
//!   -> `{total, events}` newest first
//! * `GET /api/audit/filter-options` -> the values present in the log
//! * `GET /api/audit/export?format=csv|json&<same filters>`
//!
//! (`/api/audit/events`, `/stats`, `/security-events`, `/failed-logins` and
//! `/brute-force` live in the main router.)
use crate::audit::{AuditEvent, AuditResult, AuditSeverity};
use crate::error::ApiError;
use crate::AppState;
use axum::{
    extract::{Query, State},
    http::header,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/audit/search", get(search))
        .route("/api/audit/filter-options", get(filter_options))
        .route("/api/audit/export", get(export))
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct AuditQuery {
    pub event_type: Option<String>,
    pub user: Option<String>,
    pub severity: Option<String>,
    pub result: Option<String>,
    pub search: Option<String>,
    pub start_time: Option<i64>,
    pub end_time: Option<i64>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub format: Option<String>,
}

#[derive(Serialize)]
pub struct AuditPage {
    pub total: usize,
    pub events: Vec<AuditEvent>,
}

#[derive(Serialize)]
pub struct FilterOptions {
    pub event_types: Vec<String>,
    pub users: Vec<String>,
    pub severities: Vec<String>,
    pub results: Vec<String>,
    pub source_ips: Vec<String>,
}

fn name<T: std::fmt::Debug>(v: &T) -> String {
    format!("{v:?}")
}

fn pick(opt: &Option<String>) -> Option<&str> {
    opt.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "all")
}

/// Events matching `q`, newest first (not yet paged).
pub fn filter_events(all: Vec<AuditEvent>, q: &AuditQuery) -> Vec<AuditEvent> {
    let needle = pick(&q.search).map(|s| s.to_ascii_lowercase());
    let mut v: Vec<AuditEvent> = all
        .into_iter()
        .filter(|e| pick(&q.event_type).is_none_or(|t| name(&e.event_type) == t))
        .filter(|e| pick(&q.user).is_none_or(|u| e.user.as_deref() == Some(u)))
        .filter(|e| pick(&q.severity).is_none_or(|s| name(&e.severity) == s))
        .filter(|e| pick(&q.result).is_none_or(|r| name(&e.result) == r))
        .filter(|e| q.start_time.is_none_or(|t| e.timestamp.timestamp() >= t))
        .filter(|e| q.end_time.is_none_or(|t| e.timestamp.timestamp() <= t))
        .filter(|e| {
            needle.as_ref().is_none_or(|n| {
                [
                    Some(e.action.as_str()),
                    e.user.as_deref(),
                    e.resource.as_deref(),
                    e.details.as_deref(),
                    e.source_ip.as_deref(),
                ]
                .into_iter()
                .flatten()
                .any(|f| f.to_ascii_lowercase().contains(n.as_str()))
                    || name(&e.event_type)
                        .to_ascii_lowercase()
                        .contains(n.as_str())
            })
        })
        .collect();
    v.reverse();
    v
}

async fn search(
    State(state): State<Arc<AppState>>,
    Query(q): Query<AuditQuery>,
) -> Json<AuditPage> {
    let v = filter_events(state.audit_logger.all().await, &q);
    let total = v.len();
    let limit = q.limit.unwrap_or(100).clamp(1, 5000);
    let events = v
        .into_iter()
        .skip(q.offset.unwrap_or(0))
        .take(limit)
        .collect();
    Json(AuditPage { total, events })
}

async fn filter_options(State(state): State<Arc<AppState>>) -> Json<FilterOptions> {
    let all = state.audit_logger.all().await;
    let set = |f: &dyn Fn(&AuditEvent) -> Option<String>| -> Vec<String> {
        all.iter()
            .filter_map(f)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    };
    let _ = (AuditSeverity::Info, AuditResult::Success);
    Json(FilterOptions {
        event_types: set(&|e| Some(name(&e.event_type))),
        users: set(&|e| e.user.clone()),
        severities: set(&|e| Some(name(&e.severity))),
        results: set(&|e| Some(name(&e.result))),
        source_ips: set(&|e| e.source_ip.clone()),
    })
}

fn csv_field(s: &str) -> String {
    // quote, and defuse spreadsheet formulas in attacker-influenced text
    let s = if s.starts_with(['=', '+', '-', '@']) {
        format!("'{s}")
    } else {
        s.to_string()
    };
    format!("\"{}\"", s.replace('"', "\"\""))
}

pub fn render_csv(events: &[AuditEvent]) -> String {
    let mut out = String::from(
        "timestamp,event_type,severity,user,source_ip,resource,action,result,details\n",
    );
    for e in events {
        let f = [
            e.timestamp.to_rfc3339(),
            name(&e.event_type),
            name(&e.severity),
            e.user.clone().unwrap_or_default(),
            e.source_ip.clone().unwrap_or_default(),
            e.resource.clone().unwrap_or_default(),
            e.action.clone(),
            name(&e.result),
            e.details.clone().unwrap_or_default(),
        ];
        out += &f.iter().map(|x| csv_field(x)).collect::<Vec<_>>().join(",");
        out.push('\n');
    }
    out
}

async fn export(
    State(state): State<Arc<AppState>>,
    Query(q): Query<AuditQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let events = filter_events(state.audit_logger.all().await, &q);
    let events: Vec<AuditEvent> = events
        .into_iter()
        .take(q.limit.unwrap_or(10_000).min(10_000))
        .collect();
    let (ctype, ext, body) = match q.format.as_deref().unwrap_or("csv") {
        "csv" => ("text/csv", "csv", render_csv(&events)),
        "json" => (
            "application/json",
            "json",
            serde_json::to_string_pretty(&events).map_err(|e| ApiError::Internal(e.to_string()))?,
        ),
        f => {
            return Err(ApiError::BadRequest(format!(
                "format must be csv or json, not '{f}'"
            )))
        }
    };
    Ok((
        [
            (header::CONTENT_TYPE, ctype.to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"audit-log.{ext}\""),
            ),
        ],
        body,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{create_event, AuditEventType};

    fn ev(
        t: AuditEventType,
        sev: AuditSeverity,
        user: &str,
        action: &str,
        res: AuditResult,
    ) -> AuditEvent {
        create_event(
            t,
            sev,
            Some(user.into()),
            Some("10.0.0.9".into()),
            action.into(),
            res,
        )
    }

    fn sample() -> Vec<AuditEvent> {
        vec![
            ev(
                AuditEventType::Login,
                AuditSeverity::Info,
                "alice",
                "login",
                AuditResult::Success,
            ),
            ev(
                AuditEventType::LoginFailed,
                AuditSeverity::Warning,
                "bob",
                "login failed: bad password",
                AuditResult::Failure,
            ),
            ev(
                AuditEventType::VmCreated,
                AuditSeverity::Info,
                "alice",
                "created vm 100",
                AuditResult::Success,
            ),
        ]
    }

    #[test]
    fn filters_combine_and_results_are_newest_first() {
        let q = |f: &dyn Fn(&mut AuditQuery)| {
            let mut q = AuditQuery::default();
            f(&mut q);
            filter_events(sample(), &q)
        };
        assert_eq!(q(&|_| {}).len(), 3);
        assert_eq!(q(&|_| {})[0].action, "created vm 100");
        assert_eq!(q(&|q| q.user = Some("alice".into())).len(), 2);
        assert_eq!(q(&|q| q.event_type = Some("LoginFailed".into())).len(), 1);
        assert_eq!(q(&|q| q.severity = Some("Warning".into())).len(), 1);
        assert_eq!(q(&|q| q.result = Some("Failure".into())).len(), 1);
        assert_eq!(q(&|q| q.search = Some("BAD PASS".into())).len(), 1);
        assert_eq!(q(&|q| q.search = Some("10.0.0.9".into())).len(), 3);
        assert_eq!(
            q(&|q| q.user = Some("all".into())).len(),
            3,
            "'all' is no filter"
        );
        assert_eq!(
            q(&|q| {
                q.user = Some("alice".into());
                q.result = Some("Failure".into())
            })
            .len(),
            0
        );
        let now = chrono::Utc::now().timestamp();
        assert_eq!(q(&|q| q.start_time = Some(now + 100)).len(), 0);
        assert_eq!(q(&|q| q.end_time = Some(now + 100)).len(), 3);
    }

    #[test]
    fn csv_is_quoted_and_formula_safe() {
        let mut e = ev(
            AuditEventType::Login,
            AuditSeverity::Info,
            "a\"b",
            "=HYPERLINK(\"x\")",
            AuditResult::Success,
        );
        e.details = Some("line1, line2".into());
        let c = render_csv(&[e]);
        let row = c.lines().nth(1).unwrap();
        assert!(
            row.contains("\"a\"\"b\"")
                && row.contains("\"'=HYPERLINK")
                && row.contains("\"line1, line2\"")
        );
        assert!(c.starts_with("timestamp,event_type,severity,user,"));
    }
}
