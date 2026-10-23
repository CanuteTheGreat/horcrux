//! Firewall objects and options (Proxmox Datacenter > Firewall): IP sets,
//! aliases, macros, host default policies and the firewall log.
//!
//! - `GET|POST /api/firewall/ipsets`, `GET|PUT|DELETE /api/firewall/ipsets/:name`
//! - `POST /api/firewall/ipsets/:name/entries {cidr}`, `DELETE .../entries/:cidr`
//! - `GET|POST /api/firewall/aliases`, `PUT|DELETE /api/firewall/aliases/:name`
//! - `GET /api/firewall/macros`
//! - `GET|PUT /api/firewall/options` `{policy_in, policy_out}` (host chains)
//! - `GET /api/firewall/log?limit=N` kernel log lines of rules with `log`
//!
//! A rule's `source`/`dest` may be `+ipset` or an alias name, its `dport`
//! `macro:SSH`; they are expanded when the nft rules are built, and the
//! kernel rules are rebuilt when an IP set or alias changes.
use crate::error::ApiError;
use crate::firewall::objects::{Alias, FwOptions, IpSet, MacroInfo, MACROS};
use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/firewall/ipsets", get(list_ipsets).post(create_ipset))
        .route(
            "/api/firewall/ipsets/:name",
            get(get_ipset).put(replace_ipset).delete(delete_ipset),
        )
        .route("/api/firewall/ipsets/:name/entries", post(add_entry))
        .route(
            "/api/firewall/ipsets/:name/entries/:cidr",
            delete(remove_entry),
        )
        .route("/api/firewall/aliases", get(list_aliases).post(put_alias))
        .route(
            "/api/firewall/aliases/:name",
            axum::routing::put(replace_alias).delete(delete_alias),
        )
        .route("/api/firewall/macros", get(macros))
        .route("/api/firewall/options", get(get_options).put(set_options))
        .route("/api/firewall/log", get(firewall_log))
}

fn bad(e: horcrux_common::Error) -> ApiError {
    match e {
        horcrux_common::Error::InvalidConfig(m) | horcrux_common::Error::Validation(m) => {
            ApiError::BadRequest(m)
        }
        o => o.into(),
    }
}

async fn list_ipsets(State(s): State<Arc<AppState>>) -> Json<Vec<IpSet>> {
    Json(s.firewall_manager.list_ipsets())
}

async fn get_ipset(
    State(s): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<IpSet>, ApiError> {
    s.firewall_manager
        .list_ipsets()
        .into_iter()
        .find(|i| i.name == name)
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("IP set {name} not found")))
}

async fn create_ipset(
    State(s): State<Arc<AppState>>,
    Json(set): Json<IpSet>,
) -> Result<(StatusCode, Json<IpSet>), ApiError> {
    if s.firewall_manager
        .list_ipsets()
        .iter()
        .any(|i| i.name == set.name)
    {
        return Err(ApiError::Conflict(format!(
            "IP set {} already exists",
            set.name
        )));
    }
    s.firewall_manager
        .put_ipset(set.clone())
        .await
        .map_err(bad)?;
    Ok((StatusCode::CREATED, Json(set)))
}

#[derive(Deserialize)]
struct SetBody {
    comment: Option<String>,
    entries: Option<Vec<String>>,
}

async fn replace_ipset(
    State(s): State<Arc<AppState>>,
    Path(name): Path<String>,
    Json(b): Json<SetBody>,
) -> Result<Json<IpSet>, ApiError> {
    let Json(mut set) = get_ipset(State(s.clone()), Path(name)).await?;
    if let Some(c) = b.comment {
        set.comment = c;
    }
    if let Some(e) = b.entries {
        set.entries = e;
    }
    s.firewall_manager
        .put_ipset(set.clone())
        .await
        .map_err(bad)?;
    Ok(Json(set))
}

async fn delete_ipset(
    State(s): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    if s.firewall_manager.delete_ipset(&name).await.map_err(bad)? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound(format!("IP set {name} not found")))
    }
}

#[derive(Deserialize)]
struct EntryBody {
    cidr: String,
}

async fn add_entry(
    State(s): State<Arc<AppState>>,
    Path(name): Path<String>,
    Json(b): Json<EntryBody>,
) -> Result<Json<IpSet>, ApiError> {
    let Json(mut set) = get_ipset(State(s.clone()), Path(name)).await?;
    if !set.entries.contains(&b.cidr) {
        set.entries.push(b.cidr);
    }
    s.firewall_manager
        .put_ipset(set.clone())
        .await
        .map_err(bad)?;
    Ok(Json(set))
}

async fn remove_entry(
    State(s): State<Arc<AppState>>,
    Path((name, cidr)): Path<(String, String)>,
) -> Result<Json<IpSet>, ApiError> {
    let Json(mut set) = get_ipset(State(s.clone()), Path(name)).await?;
    let before = set.entries.len();
    set.entries.retain(|e| *e != cidr);
    if set.entries.len() == before {
        return Err(ApiError::NotFound(format!("{cidr} is not in the set")));
    }
    s.firewall_manager
        .put_ipset(set.clone())
        .await
        .map_err(bad)?;
    Ok(Json(set))
}

async fn list_aliases(State(s): State<Arc<AppState>>) -> Json<Vec<Alias>> {
    Json(s.firewall_manager.list_aliases())
}

async fn put_alias(
    State(s): State<Arc<AppState>>,
    Json(a): Json<Alias>,
) -> Result<(StatusCode, Json<Alias>), ApiError> {
    if s.firewall_manager
        .list_aliases()
        .iter()
        .any(|x| x.name == a.name)
    {
        return Err(ApiError::Conflict(format!(
            "alias {} already exists",
            a.name
        )));
    }
    s.firewall_manager.put_alias(a.clone()).await.map_err(bad)?;
    Ok((StatusCode::CREATED, Json(a)))
}

#[derive(Deserialize)]
struct AliasBody {
    cidr: String,
    #[serde(default)]
    comment: String,
}

async fn replace_alias(
    State(s): State<Arc<AppState>>,
    Path(name): Path<String>,
    Json(b): Json<AliasBody>,
) -> Result<Json<Alias>, ApiError> {
    if !s
        .firewall_manager
        .list_aliases()
        .iter()
        .any(|a| a.name == name)
    {
        return Err(ApiError::NotFound(format!("alias {name} not found")));
    }
    let a = Alias {
        name,
        cidr: b.cidr,
        comment: b.comment,
    };
    s.firewall_manager.put_alias(a.clone()).await.map_err(bad)?;
    Ok(Json(a))
}

async fn delete_alias(
    State(s): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    if s.firewall_manager.delete_alias(&name).await.map_err(bad)? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound(format!("alias {name} not found")))
    }
}

async fn macros() -> Json<&'static [MacroInfo]> {
    Json(MACROS)
}

async fn get_options(State(s): State<Arc<AppState>>) -> Json<FwOptions> {
    Json(s.firewall_manager.options())
}

async fn set_options(
    State(s): State<Arc<AppState>>,
    Json(o): Json<FwOptions>,
) -> Result<Json<FwOptions>, ApiError> {
    s.firewall_manager
        .set_options(o.clone())
        .await
        .map_err(bad)?;
    Ok(Json(o))
}

/// One kernel log line written by an nft `log` rule.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LogEntry {
    pub time: String,
    pub iface_in: String,
    pub iface_out: String,
    pub src: String,
    pub dst: String,
    pub proto: String,
    pub sport: String,
    pub dport: String,
    pub raw: String,
}

/// Parse `... hx-fw: IN=eth0 OUT= ... SRC=1.2.3.4 DST=5.6.7.8 ... PROTO=TCP SPT=1 DPT=22 ...`.
pub fn parse_log_line(line: &str) -> Option<LogEntry> {
    let at = line.find(crate::firewall::LOG_PREFIX)?;
    let fields = &line[at + crate::firewall::LOG_PREFIX.len()..];
    let get = |k: &str| {
        fields
            .split_whitespace()
            .find_map(|f| f.strip_prefix(k))
            .unwrap_or("")
            .to_string()
    };
    // the timestamp is `[12.3]` (dmesg) or the first word (journal / ISO)
    let pre = line[..at].trim();
    let time = match pre.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or("").trim().to_string(),
        None => pre.split_whitespace().next().unwrap_or("").to_string(),
    };
    Some(LogEntry {
        time,
        iface_in: get("IN="),
        iface_out: get("OUT="),
        src: get("SRC="),
        dst: get("DST="),
        proto: get("PROTO="),
        sport: get("SPT="),
        dport: get("DPT="),
        raw: line.trim().to_string(),
    })
}

#[derive(Deserialize)]
struct LogQuery {
    limit: Option<usize>,
}

/// The newest `limit` (default 200, max 2000) firewall log lines, from the
/// kernel log (`journalctl -k`, else `dmesg`). 503 where the node cannot
/// read it.
async fn firewall_log(Query(q): Query<LogQuery>) -> Result<Json<Vec<LogEntry>>, ApiError> {
    let limit = q.limit.unwrap_or(200).clamp(1, 2000);
    let attempts: [(&str, Vec<String>); 2] = [
        (
            "journalctl",
            vec![
                "-k".into(),
                "--no-pager".into(),
                "-o".into(),
                "short-iso".into(),
                "-n".into(),
                "20000".into(),
            ],
        ),
        ("dmesg", vec!["--time-format".into(), "iso".into()]),
    ];
    let mut last = String::new();
    for (cmd, args) in attempts {
        match tokio::process::Command::new(cmd).args(&args).output().await {
            Ok(o) if o.status.success() => {
                let text = String::from_utf8_lossy(&o.stdout);
                let mut v: Vec<LogEntry> = text.lines().filter_map(parse_log_line).collect();
                let skip = v.len().saturating_sub(limit);
                v.drain(..skip);
                v.reverse();
                return Ok(Json(v));
            }
            Ok(o) => last = String::from_utf8_lossy(&o.stderr).trim().to_string(),
            Err(e) => last = format!("{cmd}: {e}"),
        }
    }
    Err(ApiError::ServiceUnavailable(format!(
        "cannot read the kernel log: {last}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_kernel_log_line() {
        let l = "2026-10-22T21:40:01-0700 gw kernel: hx-fw: IN=eth0 OUT= MAC=aa:bb SRC=10.0.0.5 DST=10.0.0.1 LEN=60 TOS=0x00 PROTO=TCP SPT=51514 DPT=22 WINDOW=64240 SYN";
        let e = parse_log_line(l).unwrap();
        assert_eq!(
            (
                e.iface_in.as_str(),
                e.iface_out.as_str(),
                e.src.as_str(),
                e.dst.as_str()
            ),
            ("eth0", "", "10.0.0.5", "10.0.0.1")
        );
        assert_eq!(
            (e.proto.as_str(), e.sport.as_str(), e.dport.as_str()),
            ("TCP", "51514", "22")
        );
        assert_eq!(e.time, "2026-10-22T21:40:01-0700");
        assert!(parse_log_line("kernel: something else").is_none());
        let d = parse_log_line("[ 123.4] hx-fw: IN=lo OUT= SRC=1.1.1.1 DST=2.2.2.2 PROTO=ICMP")
            .unwrap();
        assert_eq!(d.time, "123.4");
    }
}
