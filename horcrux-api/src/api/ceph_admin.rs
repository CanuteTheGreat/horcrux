//! Ceph management (Proxmox "Node > Ceph"): cluster status, monitors, OSDs
//! (in / out), pools (create / delete), CRUSH rules and cluster flags, through
//! the `ceph` command line of this node (its /etc/ceph config and keyring).
//!
//! * `GET  /api/ceph/status`                health, monitors, mgr, OSD and PG counts, capacity
//! * `GET  /api/ceph/monitors`, `/api/ceph/osds`, `/api/ceph/pools`, `/api/ceph/crush-rules`
//! * `POST /api/ceph/osds/:id/:action`      `in` | `out`
//! * `POST /api/ceph/pools`                 `{name, pg_num, size, application}`
//! * `DELETE /api/ceph/pools/:name?confirm=:name`
//! * `GET  /api/ceph/flags`, `POST /api/ceph/flags/:flag` `{set: bool}` (noout, norebalance, ...)
//!
//! Where `ceph` is not installed or no cluster answers, the read routes say
//! so (`available` / `error`) instead of failing. Creating/destroying OSDs
//! and monitors (disk wiping, daemon deployment) is deliberately not done
//! here.
use crate::api::system_admin::{run_secs, which};
use crate::api::tasks;
use crate::error::ApiError;
use crate::AppState;
use axum::{
    extract::{Path, Query},
    routing::{get, post},
    Extension, Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/ceph/status", get(status))
        .route("/api/ceph/monitors", get(monitors))
        .route("/api/ceph/osds", get(osds))
        .route("/api/ceph/osds/:id/:action", post(osd_action))
        .route("/api/ceph/pools", get(pools).post(create_pool))
        .route("/api/ceph/pools/:name", axum::routing::delete(delete_pool))
        .route("/api/ceph/crush-rules", get(crush_rules))
        .route("/api/ceph/flags", get(flags))
        .route("/api/ceph/flags/:flag", post(set_flag))
}

type Res<T> = Result<Json<T>, ApiError>;
type AuthExt = Option<Extension<crate::middleware::auth::AuthUser>>;

/// Flags worth toggling during maintenance.
const FLAGS: &[&str] = &[
    "noout",
    "noin",
    "nodown",
    "noup",
    "norebalance",
    "norecover",
    "nobackfill",
    "noscrub",
    "nodeep-scrub",
    "pause",
];

async fn ceph(args: &[&str]) -> Result<Value, ApiError> {
    if which("ceph").is_none() {
        return Err(ApiError::ServiceUnavailable(
            "Ceph is not installed on this node (the `ceph` command is missing)".into(),
        ));
    }
    let mut a: Vec<&str> = args.to_vec();
    a.extend(["--connect-timeout", "8", "-f", "json"]);
    let out = run_secs("ceph", &a, 30)
        .await
        .map_err(ApiError::ServiceUnavailable)?;
    serde_json::from_str(&out)
        .map_err(|e| ApiError::Internal(format!("ceph {}: unreadable JSON: {e}", args.join(" "))))
}

async fn ceph_do(args: &[&str]) -> Result<String, ApiError> {
    if which("ceph").is_none() {
        return Err(ApiError::ServiceUnavailable(
            "Ceph is not installed on this node (the `ceph` command is missing)".into(),
        ));
    }
    let mut a: Vec<&str> = args.to_vec();
    a.extend(["--connect-timeout", "8"]);
    run_secs("ceph", &a, 60).await.map_err(ApiError::Conflict)
}

fn n(v: &Value) -> u64 {
    v.as_u64().unwrap_or(0)
}

// ---------------------------------------------------------------- status

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct HealthCheck {
    pub name: String,
    pub severity: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct CephStatus {
    /// the `ceph` command exists on this node
    pub available: bool,
    /// a cluster answered
    pub reachable: bool,
    pub health: String,
    pub health_checks: Vec<HealthCheck>,
    pub fsid: Option<String>,
    pub mons_total: u64,
    pub mons_in_quorum: Vec<String>,
    pub mgr_available: bool,
    pub mgr_standbys: u64,
    pub osds_total: u64,
    pub osds_up: u64,
    pub osds_in: u64,
    pub pgs_total: u64,
    pub pools: u64,
    pub pg_states: Vec<(String, u64)>,
    pub bytes_total: u64,
    pub bytes_used: u64,
    pub bytes_avail: u64,
    pub error: Option<String>,
}

pub fn parse_status(v: &Value) -> CephStatus {
    let mut checks: Vec<HealthCheck> = v["health"]["checks"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(k, c)| HealthCheck {
            name: k.clone(),
            severity: c["severity"].as_str().unwrap_or("").to_string(),
            summary: c["summary"]["message"].as_str().unwrap_or("").to_string(),
        })
        .collect();
    checks.sort_by(|a, b| a.name.cmp(&b.name));
    CephStatus {
        available: true,
        reachable: true,
        health: v["health"]["status"]
            .as_str()
            .unwrap_or("UNKNOWN")
            .to_string(),
        health_checks: checks,
        fsid: v["fsid"].as_str().map(String::from),
        mons_total: n(&v["monmap"]["num_mons"]),
        mons_in_quorum: v["quorum_names"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| m.as_str().map(String::from))
            .collect(),
        mgr_available: v["mgrmap"]["available"].as_bool().unwrap_or(false),
        mgr_standbys: n(&v["mgrmap"]["num_standbys"]),
        osds_total: n(&v["osdmap"]["num_osds"]),
        osds_up: n(&v["osdmap"]["num_up_osds"]),
        osds_in: n(&v["osdmap"]["num_in_osds"]),
        pgs_total: n(&v["pgmap"]["num_pgs"]),
        pools: n(&v["pgmap"]["num_pools"]),
        pg_states: v["pgmap"]["pgs_by_state"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|s| {
                (
                    s["state_name"].as_str().unwrap_or("").to_string(),
                    n(&s["count"]),
                )
            })
            .collect(),
        bytes_total: n(&v["pgmap"]["bytes_total"]),
        bytes_used: n(&v["pgmap"]["bytes_used"]),
        bytes_avail: n(&v["pgmap"]["bytes_avail"]),
        error: None,
    }
}

async fn status() -> Json<CephStatus> {
    if which("ceph").is_none() {
        return Json(CephStatus {
            health: "NOT_INSTALLED".into(),
            error: Some(
                "Ceph is not installed on this node (the `ceph` command is missing)".into(),
            ),
            ..Default::default()
        });
    }
    match ceph(&["status"]).await {
        Ok(v) => Json(parse_status(&v)),
        Err(e) => Json(CephStatus {
            available: true,
            health: "UNREACHABLE".into(),
            error: Some(match e {
                ApiError::ServiceUnavailable(m) | ApiError::Internal(m) | ApiError::Conflict(m) => {
                    m
                }
                other => format!("{other:?}"),
            }),
            ..Default::default()
        }),
    }
}

// --------------------------------------------------------------- monitors

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Monitor {
    pub name: String,
    pub rank: u64,
    pub address: String,
    pub in_quorum: bool,
}

pub fn parse_monitors(v: &Value) -> Vec<Monitor> {
    let quorum: Vec<u64> = v["quorum"]
        .as_array()
        .into_iter()
        .flatten()
        .map(n)
        .collect();
    v["mons"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| {
            let rank = n(&m["rank"]);
            // prefer the v2 (msgr2) address, fall back to the legacy field
            let addr = m["public_addrs"]["addrvec"]
                .as_array()
                .and_then(|a| a.iter().find(|x| x["type"] == "v2").or_else(|| a.first()))
                .and_then(|x| x["addr"].as_str())
                .or_else(|| m["addr"].as_str())
                .unwrap_or("")
                .to_string();
            Monitor {
                name: m["name"].as_str().unwrap_or("").to_string(),
                rank,
                address: addr,
                in_quorum: quorum.contains(&rank),
            }
        })
        .collect()
}

async fn monitors() -> Res<Vec<Monitor>> {
    Ok(Json(parse_monitors(&ceph(&["mon", "dump"]).await?)))
}

// ------------------------------------------------------------------ OSDs

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Osd {
    pub id: u64,
    pub name: String,
    pub host: String,
    /// up | down
    pub status: String,
    /// 0 = out of the cluster, 1 = fully in
    pub reweight: f64,
    pub crush_weight: f64,
    pub device_class: Option<String>,
    pub is_in: bool,
}

/// `ceph osd tree`: each OSD with the host bucket that lists it.
pub fn parse_osd_tree(v: &Value) -> Vec<Osd> {
    let nodes = v["nodes"].as_array().cloned().unwrap_or_default();
    let mut host_of: HashMap<i64, String> = HashMap::new();
    for h in nodes.iter().filter(|x| x["type"] == "host") {
        for c in h["children"].as_array().into_iter().flatten() {
            if let (Some(id), Some(name)) = (c.as_i64(), h["name"].as_str()) {
                host_of.insert(id, name.to_string());
            }
        }
    }
    let mut out: Vec<Osd> = nodes
        .iter()
        .filter(|x| x["type"] == "osd")
        .map(|o| {
            let id = o["id"].as_i64().unwrap_or(-1);
            let reweight = o["reweight"].as_f64().unwrap_or(0.0);
            Osd {
                id: id.max(0) as u64,
                name: o["name"].as_str().unwrap_or("").to_string(),
                host: host_of.get(&id).cloned().unwrap_or_default(),
                status: o["status"].as_str().unwrap_or("unknown").to_string(),
                reweight,
                crush_weight: o["crush_weight"].as_f64().unwrap_or(0.0),
                device_class: o["device_class"].as_str().map(String::from),
                is_in: reweight > 0.0,
            }
        })
        .collect();
    out.sort_by_key(|o| o.id);
    out
}

async fn osds() -> Res<Vec<Osd>> {
    Ok(Json(parse_osd_tree(&ceph(&["osd", "tree"]).await?)))
}

async fn osd_action(
    u: AuthExt,
    Path((id, action)): Path<(String, String)>,
) -> Res<serde_json::Value> {
    let osd: u64 = id
        .parse()
        .map_err(|_| ApiError::BadRequest(format!("'{id}' is not an OSD id")))?;
    if action != "in" && action != "out" {
        return Err(ApiError::BadRequest("action must be in or out".into()));
    }
    // an OSD that does not exist is a 404, not a silent no-op
    if !parse_osd_tree(&ceph(&["osd", "tree"]).await?)
        .iter()
        .any(|o| o.id == osd)
    {
        return Err(ApiError::NotFound(format!(
            "osd.{osd} is not in this cluster"
        )));
    }
    let r = ceph_do(&["osd", &action, &osd.to_string()]).await;
    tasks::record_done(
        "ceph-osd",
        &format!("osd.{osd}:{action}"),
        &super::system_admin::user_of(&u),
        &r.as_ref()
            .map(|s| s.trim().to_string())
            .map_err(|e| format!("{e:?}")),
    );
    r?;
    Ok(Json(serde_json::json!({"osd": osd, "action": action})))
}

// ----------------------------------------------------------------- pools

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Pool {
    pub id: u64,
    pub name: String,
    pub size: u64,
    pub min_size: u64,
    pub pg_num: u64,
    pub crush_rule: u64,
    pub applications: Vec<String>,
    pub stored_bytes: u64,
    pub objects: u64,
    pub max_avail: u64,
}

pub fn parse_pools(detail: &Value, df: &Value) -> Vec<Pool> {
    let usage: HashMap<String, &Value> = df["pools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p["name"].as_str().map(|n| (n.to_string(), &p["stats"])))
        .collect();
    let mut out: Vec<Pool> = detail
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| {
            let name = p["pool_name"].as_str().unwrap_or("").to_string();
            let u = usage.get(&name);
            let mut apps: Vec<String> = p["application_metadata"]
                .as_object()
                .map(|o| o.keys().cloned().collect())
                .unwrap_or_default();
            apps.sort();
            Pool {
                id: n(&p["pool"]),
                size: n(&p["size"]),
                min_size: n(&p["min_size"]),
                pg_num: n(&p["pg_num"]),
                crush_rule: n(&p["crush_rule"]),
                applications: apps,
                stored_bytes: u.map(|s| n(&s["stored"])).unwrap_or(0),
                objects: u.map(|s| n(&s["objects"])).unwrap_or(0),
                max_avail: u.map(|s| n(&s["max_avail"])).unwrap_or(0),
                name,
            }
        })
        .collect();
    out.sort_by_key(|p| p.id);
    out
}

async fn pools() -> Res<Vec<Pool>> {
    let detail = ceph(&["osd", "pool", "ls", "detail"]).await?;
    let df = ceph(&["df"]).await.unwrap_or(Value::Null);
    Ok(Json(parse_pools(&detail, &df)))
}

#[derive(Debug, Deserialize)]
pub struct NewPool {
    pub name: String,
    pub pg_num: u32,
    #[serde(default = "default_size")]
    pub size: u32,
    /// rbd | cephfs | rgw (omit for none)
    #[serde(default)]
    pub application: Option<String>,
}

fn default_size() -> u32 {
    3
}

pub fn validate_pool(p: &NewPool) -> Result<(), String> {
    let ok = !p.name.is_empty()
        && p.name.len() <= 64
        && !p.name.starts_with(['.', '-'])
        && p.name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    if !ok {
        return Err("pool name: 1-64 letters, digits, _ . - (not starting with . or -)".into());
    }
    if p.pg_num == 0 || p.pg_num > 32768 || !p.pg_num.is_power_of_two() {
        return Err("pg_num must be a power of two between 1 and 32768".into());
    }
    if !(1..=7).contains(&p.size) {
        return Err("size (replicas) must be 1-7".into());
    }
    if let Some(a) = p.application.as_deref() {
        if !["rbd", "cephfs", "rgw"].contains(&a) {
            return Err("application must be rbd, cephfs or rgw".into());
        }
    }
    Ok(())
}

async fn create_pool(u: AuthExt, Json(p): Json<NewPool>) -> Res<serde_json::Value> {
    validate_pool(&p).map_err(ApiError::ValidationError)?;
    let existing = parse_pools(&ceph(&["osd", "pool", "ls", "detail"]).await?, &Value::Null);
    if existing.iter().any(|x| x.name == p.name) {
        return Err(ApiError::Conflict(format!(
            "pool {} already exists",
            p.name
        )));
    }
    let who = super::system_admin::user_of(&u);
    let steps: Vec<Vec<String>> = {
        let mut s = vec![vec![
            "osd".into(),
            "pool".into(),
            "create".into(),
            p.name.clone(),
            p.pg_num.to_string(),
        ]];
        s.push(vec![
            "osd".into(),
            "pool".into(),
            "set".into(),
            p.name.clone(),
            "size".into(),
            p.size.to_string(),
        ]);
        if let Some(a) = &p.application {
            s.push(vec![
                "osd".into(),
                "pool".into(),
                "application".into(),
                "enable".into(),
                p.name.clone(),
                a.clone(),
            ]);
        }
        s
    };
    for step in &steps {
        let refs: Vec<&str> = step.iter().map(|x| x.as_str()).collect();
        if let Err(e) = ceph_do(&refs).await {
            tasks::record_done("ceph-pool", &p.name, &who, &Err(format!("{e:?}")));
            return Err(e);
        }
    }
    tasks::record_done(
        "ceph-pool",
        &p.name,
        &who,
        &Ok(format!("created: {} PGs, size {}", p.pg_num, p.size)),
    );
    Ok(Json(serde_json::json!({"created": p.name})))
}

#[derive(Deserialize)]
struct ConfirmQ {
    confirm: Option<String>,
}

async fn delete_pool(
    u: AuthExt,
    Path(name): Path<String>,
    Query(q): Query<ConfirmQ>,
) -> Res<serde_json::Value> {
    if q.confirm.as_deref() != Some(name.as_str()) {
        return Err(ApiError::BadRequest(
            "deleting a pool destroys its data: repeat the pool name as ?confirm=<name>".into(),
        ));
    }
    if name.starts_with('.') || name.contains(char::is_whitespace) || name.is_empty() {
        return Err(ApiError::BadRequest(
            "refusing to delete a system pool or an invalid name".into(),
        ));
    }
    let existing = parse_pools(&ceph(&["osd", "pool", "ls", "detail"]).await?, &Value::Null);
    if !existing.iter().any(|x| x.name == name) {
        return Err(ApiError::NotFound(format!("pool {name} not found")));
    }
    let r = ceph_do(&[
        "osd",
        "pool",
        "delete",
        &name,
        &name,
        "--yes-i-really-really-mean-it",
    ])
    .await;
    tasks::record_done(
        "ceph-pool",
        &format!("{name}:delete"),
        &super::system_admin::user_of(&u),
        &r.as_ref()
            .map(|s| s.trim().to_string())
            .map_err(|e| format!("{e:?}")),
    );
    r?;
    Ok(Json(serde_json::json!({"deleted": name})))
}

// ------------------------------------------------------------ CRUSH, flags

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CrushRule {
    pub id: u64,
    pub name: String,
    /// failure domain of the chooseleaf step (host, osd, rack...)
    pub failure_domain: Option<String>,
    /// root bucket the rule starts at (with `~class` for a device class)
    pub root: Option<String>,
}

pub fn parse_crush_rules(v: &Value) -> Vec<CrushRule> {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|r| {
            let steps = r["steps"].as_array().cloned().unwrap_or_default();
            CrushRule {
                id: n(&r["rule_id"]),
                name: r["rule_name"].as_str().unwrap_or("").to_string(),
                failure_domain: steps
                    .iter()
                    .find(|s| s["op"].as_str().is_some_and(|o| o.starts_with("choose")))
                    .and_then(|s| s["type"].as_str().map(String::from)),
                root: steps
                    .iter()
                    .find(|s| s["op"] == "take")
                    .and_then(|s| s["item_name"].as_str().map(String::from)),
            }
        })
        .collect()
}

async fn crush_rules() -> Res<Vec<CrushRule>> {
    Ok(Json(parse_crush_rules(
        &ceph(&["osd", "crush", "rule", "dump"]).await?,
    )))
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CephFlag {
    pub name: String,
    pub set: bool,
}

pub fn parse_flags(dump: &Value) -> Vec<CephFlag> {
    let set: Vec<&str> = dump["flags"].as_str().unwrap_or("").split(',').collect();
    FLAGS
        .iter()
        .map(|f| CephFlag {
            name: f.to_string(),
            set: set.contains(f),
        })
        .collect()
}

async fn flags() -> Res<Vec<CephFlag>> {
    Ok(Json(parse_flags(&ceph(&["osd", "dump"]).await?)))
}

#[derive(Deserialize)]
struct FlagBody {
    set: bool,
}

async fn set_flag(
    u: AuthExt,
    Path(flag): Path<String>,
    Json(b): Json<FlagBody>,
) -> Res<Vec<CephFlag>> {
    if !FLAGS.contains(&flag.as_str()) {
        return Err(ApiError::BadRequest(format!(
            "flag must be one of {}",
            FLAGS.join(", ")
        )));
    }
    let r = ceph_do(&["osd", if b.set { "set" } else { "unset" }, &flag]).await;
    tasks::record_done(
        "ceph-flag",
        &format!("{flag}:{}", if b.set { "set" } else { "unset" }),
        &super::system_admin::user_of(&u),
        &r.as_ref()
            .map(|s| s.trim().to_string())
            .map_err(|e| format!("{e:?}")),
    );
    r?;
    flags().await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fx(name: &str) -> Value {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/ceph")
            .join(name);
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
    }

    #[test]
    fn status_json_becomes_the_dashboard() {
        let s = parse_status(&fx("status.json"));
        assert_eq!(s.health, "HEALTH_WARN");
        assert_eq!(s.health_checks.len(), 2);
        assert_eq!(s.health_checks[0].name, "OSD_DOWN");
        assert_eq!(s.health_checks[0].summary, "1 osds down");
        assert_eq!((s.mons_total, s.mons_in_quorum.len()), (3, 3));
        assert_eq!((s.osds_total, s.osds_up, s.osds_in), (3, 2, 3));
        assert_eq!((s.pgs_total, s.pools), (32, 2));
        assert_eq!(
            s.pg_states[1],
            ("active+undersized+degraded".to_string(), 9)
        );
        assert_eq!(
            (s.bytes_total, s.bytes_used, s.bytes_avail),
            (3489660928, 805306368, 2684354560)
        );
        assert!(s.mgr_available && s.mgr_standbys == 1 && s.reachable);
    }

    #[test]
    fn monitors_prefer_the_v2_address_and_know_quorum() {
        let m = parse_monitors(&fx("mon_dump.json"));
        assert_eq!(m.len(), 3);
        assert_eq!(
            (m[0].name.as_str(), m[0].address.as_str(), m[0].in_quorum),
            ("a", "10.0.0.11:3300", true)
        );
        assert_eq!(m[2].address, "10.0.0.13:3300");
        let mut v = fx("mon_dump.json");
        v["quorum"] = serde_json::json!([0, 2]);
        assert!(!parse_monitors(&v)[1].in_quorum);
    }

    #[test]
    fn osd_tree_gives_each_osd_its_host_and_state() {
        let o = parse_osd_tree(&fx("osd_tree.json"));
        assert_eq!(o.len(), 3);
        assert_eq!(
            (
                o[0].name.as_str(),
                o[0].host.as_str(),
                o[0].status.as_str(),
                o[0].is_in
            ),
            ("osd.0", "pve1", "up", true)
        );
        assert_eq!(
            (o[1].status.as_str(), o[1].is_in, o[1].reweight),
            ("down", false, 0.0)
        );
        assert_eq!(
            (
                o[2].host.as_str(),
                o[2].device_class.as_deref(),
                o[2].crush_weight
            ),
            ("pve2", Some("hdd"), 1.82)
        );
    }

    #[test]
    fn pools_join_detail_and_usage() {
        let p = parse_pools(&fx("pools.json"), &fx("df.json"));
        assert_eq!(p.len(), 2);
        assert_eq!(
            (p[1].name.as_str(), p[1].size, p[1].min_size, p[1].pg_num),
            ("vmdata", 3, 2, 32)
        );
        assert_eq!(p[1].applications, vec!["rbd".to_string()]);
        assert_eq!((p[1].stored_bytes, p[1].objects), (134217728, 32));
        // usage is optional
        let bare = parse_pools(&fx("pools.json"), &Value::Null);
        assert_eq!(bare[1].stored_bytes, 0);
    }

    #[test]
    fn crush_rules_and_flags() {
        let r = parse_crush_rules(&fx("crush_rules.json"));
        assert_eq!(
            (
                r[0].name.as_str(),
                r[0].failure_domain.as_deref(),
                r[0].root.as_deref()
            ),
            ("replicated_rule", Some("host"), Some("default"))
        );
        assert_eq!(r[1].root.as_deref(), Some("default~ssd"));
        let f = parse_flags(&fx("osd_dump.json"));
        assert!(f.iter().find(|x| x.name == "noout").unwrap().set);
        assert!(!f.iter().find(|x| x.name == "norebalance").unwrap().set);
        assert_eq!(f.len(), FLAGS.len());
    }

    #[test]
    fn pool_requests_are_validated() {
        let p = |name: &str, pg: u32, size: u32, app: Option<&str>| NewPool {
            name: name.into(),
            pg_num: pg,
            size,
            application: app.map(String::from),
        };
        assert!(validate_pool(&p("vmdata", 32, 3, Some("rbd"))).is_ok());
        for bad in [
            p("", 32, 3, None),
            p(".mgr", 32, 3, None),
            p("a b", 32, 3, None),
            p("x", 33, 3, None),
            p("x", 0, 3, None),
            p("x", 32, 0, None),
            p("x", 32, 9, None),
            p("x", 32, 3, Some("nfs")),
        ] {
            assert!(validate_pool(&bad).is_err(), "{:?}", bad.name);
        }
    }
}
