//! Ceph management (Proxmox "Node > Ceph"): cluster status, monitors, OSDs
//! (in / out), pools (create / delete), CRUSH rules and cluster flags, through
//! the `ceph` command line of this node (its /etc/ceph config and keyring).
//!
//! * `GET  /api/ceph/status`                health, monitors, mgr, OSD and PG counts, capacity
//! * `GET  /api/ceph/monitors`, `/api/ceph/osds`, `/api/ceph/pools`, `/api/ceph/crush-rules`
//! * `POST /api/ceph/osds/:id/:action`      `in` | `out`
//! * `POST /api/ceph/osds`                  `{device, confirm}` create an OSD on a blank whole disk of THIS node
//! * `POST /api/ceph/pools`                 `{name, pg_num, size, application}`
//! * `DELETE /api/ceph/pools/:name?confirm=:name`
//! * `GET  /api/ceph/flags`, `POST /api/ceph/flags/:flag` `{set: bool}` (noout, norebalance, ...)
//!
//! Where `ceph` is not installed or no cluster answers, the read routes say
//! so (`available` / `error`) instead of failing. Creating an OSD only takes
//! a blank whole disk (no partitions, signature, LVM or mount: nothing is
//! ever wiped) on a node whose role allows OSDs; destroying OSDs and
//! deploying monitors is not done here.
use crate::api::system_admin::{run_secs, which};
use crate::api::tasks;
use crate::error::ApiError;
use crate::AppState;
use axum::{
    extract::{Path, Query, State},
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
        .route("/api/ceph/osds", get(osds).post(create_osd))
        .route("/api/ceph/osds/:id/:action", post(osd_action))
        .route("/api/ceph/pools", get(pools).post(create_pool))
        .route("/api/ceph/pools/:name", axum::routing::delete(delete_pool))
        .route("/api/ceph/crush-rules", get(crush_rules).post(create_crush_rule))
        .route("/api/ceph/flags", get(flags))
        .route("/api/ceph/flags/:flag", post(set_flag))
        .route("/api/ceph/placement", get(placement))
        .route("/api/ceph/recovery-tuning", post(recovery_tuning))
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

// ------------------------------------------------------------ placement

#[derive(Debug, Deserialize)]
struct PlacementQ {
    /// Minimum uplink for an OSD host (default 10000 = 10 GbE).
    min_osd_mbps: Option<u64>,
    /// Minimum uplink for a MON host (default 1000).
    min_mon_mbps: Option<u64>,
    mon_count: Option<usize>,
    /// Remote nodes do not report NICs/disks yet: the operator states what
    /// they have. Default 0 = unknown, so a remote node is refused as OSD
    /// host rather than assumed fast.
    remote_uplink_mbps: Option<u64>,
    remote_data_disks: Option<u32>,
}

/// Unused-looking local NVMe namespaces (whole disks only).
fn local_nvme_disks() -> u32 {
    std::fs::read_dir("/sys/block")
        .map(|d| {
            d.flatten()
                .filter(|e| {
                    let n = e.file_name().to_string_lossy().to_string();
                    n.starts_with("nvme") && !n.contains('p')
                })
                .count() as u32
        })
        .unwrap_or(0)
}

/// `GET /api/ceph/placement` -- where OSDs and MONs may go, given node roles
/// (`PUT /api/cluster/nodes/:name/caps`) and uplink speed. Read-only advice:
/// OSDs only on storage-role nodes with a fast uplink and data disks, MONs on
/// an odd number of distinct nodes. Only the local node's NICs and disks are
/// probed; see the `remote_*` query parameters.
async fn placement(
    State(state): State<Arc<AppState>>,
    Query(q): Query<PlacementQ>,
) -> Json<crate::cluster::ceph_placement::CephPlan> {
    use crate::cluster::ceph_placement::{plan, CephCandidate};
    let local_up = crate::cluster::nic::best_speed_mbps(&crate::cluster::nic::scan_nics(
        std::path::Path::new("/sys"),
    ))
    .unwrap_or(0);
    let local_disks = local_nvme_disks();
    let nodes = state.cluster_manager.list_nodes_with_local().await;
    let cands: Vec<CephCandidate> = nodes
        .iter()
        .map(|n| CephCandidate {
            name: n.name.clone(),
            caps: n.caps.clone().unwrap_or_default(),
            uplink_mbps: if n.is_local {
                local_up
            } else {
                // The node's own report wins; the query value is the fallback
                // for older nodes that do not report yet.
                if n.uplink_mbps > 0 {
                    n.uplink_mbps
                } else {
                    q.remote_uplink_mbps.unwrap_or(0)
                }
            },
            data_disks: if n.is_local {
                local_disks
            } else {
                if n.nvme_disks > 0 {
                    n.nvme_disks
                } else {
                    q.remote_data_disks.unwrap_or(0)
                }
            },
        })
        .collect();
    let mut p = plan(
        &cands,
        q.min_osd_mbps.unwrap_or(10_000),
        q.min_mon_mbps.unwrap_or(1_000),
        q.mon_count.unwrap_or(3),
    );
    let unreported = nodes.iter().any(|n| !n.is_local && n.uplink_mbps == 0);
    if unreported && q.remote_uplink_mbps.is_none() {
        p.warnings.push(
            "remote nodes' uplink/disks are not probed: pass remote_uplink_mbps and remote_data_disks"
                .to_string(),
        );
    }
    Json(p)
}

// ------------------------------------------------------- recovery tuning

#[derive(Debug, Deserialize, Default)]
struct TuningQ {
    /// Link speed (Mbit/s) of the Ceph cluster network. Default: fastest
    /// local NIC.
    link_mbps: Option<u64>,
    /// Only return the `ceph config set` commands, change nothing.
    dry_run: Option<bool>,
}

fn set_osd(k: &str, v: String) -> Vec<String> {
    ["config", "set", "osd", k]
        .iter()
        .map(|x| x.to_string())
        .chain([v])
        .collect()
}

/// `ceph config set` argument lists that throttle backfill/recovery for a
/// link of `link_mbps`. Sleep is in seconds and set for both hdd and ssd
/// classes, since the class-specific options override the generic one.
pub fn recovery_tuning_cmds(link_mbps: u64) -> Vec<Vec<String>> {
    let (backfills, active, sleep_ms) = crate::cluster::nic::ceph_recovery_tuning(link_mbps);
    let sleep = format!("{}", f64::from(sleep_ms) / 1000.0);
    vec![
        set_osd("osd_max_backfills", backfills.to_string()),
        set_osd("osd_recovery_max_active", active.to_string()),
        set_osd("osd_recovery_sleep_hdd", sleep.clone()),
        set_osd("osd_recovery_sleep_ssd", sleep),
    ]
}

/// `POST /api/ceph/recovery-tuning` -- scale OSD backfill/recovery throttles
/// to the cluster-network link speed so a rebuild cannot saturate it.
async fn recovery_tuning(u: AuthExt, Query(q): Query<TuningQ>) -> Res<serde_json::Value> {
    let link = match q.link_mbps {
        Some(l) if l > 0 => l,
        Some(_) => return Err(ApiError::BadRequest("link_mbps must be > 0".into())),
        None => crate::cluster::nic::best_speed_mbps(&crate::cluster::nic::scan_nics(
            std::path::Path::new("/sys"),
        ))
        .ok_or_else(|| ApiError::BadRequest("no NIC speed detected: pass link_mbps".into()))?,
    };
    let cmds = recovery_tuning_cmds(link);
    let dry = q.dry_run.unwrap_or(false);
    if !dry {
        for c in &cmds {
            let a: Vec<&str> = c.iter().map(String::as_str).collect();
            let r = ceph_do(&a).await;
            tasks::record_done(
                "ceph-tuning",
                &c[3..].join("="),
                &super::system_admin::user_of(&u),
                &r.as_ref()
                    .map(|s| s.trim().to_string())
                    .map_err(|e| format!("{e:?}")),
            );
            r?;
        }
    }
    Ok(Json(serde_json::json!({
        "link_mbps": link,
        "applied": !dry,
        "commands": cmds.iter().map(|c| format!("ceph {}", c.join(" "))).collect::<Vec<_>>(),
    })))
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

// ------------------------------------------------------------ OSD create

#[derive(Debug, Deserialize)]
struct NewOsd {
    /// Whole-disk device of this node, e.g. /dev/nvme1n1.
    device: String,
    /// Must repeat `device`: the disk becomes an OSD.
    confirm: Option<String>,
}

/// A device path we are willing to hand to ceph-volume: /dev/<name>, plain
/// characters only (no `..`, no option-looking or shell-ish input).
pub fn validate_osd_device_path(dev: &str) -> Result<(), String> {
    let rest = dev.strip_prefix("/dev/").unwrap_or("");
    let ok = !rest.is_empty()
        && !rest.contains("..")
        && !rest.starts_with('/')
        && rest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '.'));
    if ok {
        Ok(())
    } else {
        Err("device must be a path like /dev/nvme1n1".into())
    }
}

fn lsblk_busy(node: &Value) -> Option<String> {
    let name = node["name"].as_str().unwrap_or("?");
    let mounted = |v: &Value| match v {
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => a.iter().any(|m| m.as_str().is_some_and(|s| !s.is_empty())),
        _ => false,
    };
    if mounted(&node["mountpoint"]) || mounted(&node["mountpoints"]) {
        return Some(format!("{name} is mounted"));
    }
    if let Some(kids) = node["children"].as_array() {
        if !kids.is_empty() {
            for k in kids {
                if let Some(b) = lsblk_busy(k) {
                    return Some(b);
                }
            }
            return Some(format!("{name} has partitions or volumes on it"));
        }
    }
    None
}

/// Decide from `lsblk -J -p -o NAME,TYPE,FSTYPE,MOUNTPOINT,RO <dev>` whether
/// `dev` is a blank whole disk. Anything that already holds something is
/// refused: Horcrux never wipes a disk on its own.
pub fn check_osd_disk(dev: &str, lsblk: &Value) -> Result<(), String> {
    let d = lsblk["blockdevices"]
        .as_array()
        .and_then(|a| a.first())
        .ok_or_else(|| format!("{dev} is not a block device of this node"))?;
    if d["type"].as_str() != Some("disk") {
        return Err(format!(
            "{dev} is a {} - an OSD takes a whole disk",
            d["type"].as_str().unwrap_or("non-disk")
        ));
    }
    let ro = &d["ro"];
    if ro == &Value::Bool(true) || ro.as_str() == Some("1") || ro.as_u64() == Some(1) {
        return Err(format!("{dev} is read-only"));
    }
    if let Some(b) = lsblk_busy(d) {
        return Err(format!("{dev} is in use: {b}"));
    }
    if let Some(fs) = d["fstype"].as_str().filter(|f| !f.is_empty()) {
        return Err(format!(
            "{dev} already carries a {fs} signature; wipe it yourself (wipefs/ceph-volume lvm zap) if it is really free"
        ));
    }
    Ok(())
}

/// `POST /api/ceph/osds` -- `ceph-volume lvm create --data <device>` on this
/// node. Gated by the node's role/uplink (same rule as /api/ceph/placement),
/// by an explicit `confirm`, and by the disk being blank.
async fn create_osd(
    State(state): State<Arc<AppState>>,
    u: AuthExt,
    Json(b): Json<NewOsd>,
) -> Res<serde_json::Value> {
    validate_osd_device_path(&b.device).map_err(ApiError::ValidationError)?;
    if b.confirm.as_deref() != Some(b.device.as_str()) {
        return Err(ApiError::BadRequest(
            "this turns the disk into an OSD: repeat the device as \"confirm\"".into(),
        ));
    }
    let local_up = crate::cluster::nic::best_speed_mbps(&crate::cluster::nic::scan_nics(
        std::path::Path::new("/sys"),
    ))
    .unwrap_or(0);
    let nodes = state.cluster_manager.list_nodes_with_local().await;
    if let Some(me) = nodes.iter().find(|n| n.is_local) {
        crate::cluster::roles::osd_allowed(&me.caps.clone().unwrap_or_default(), local_up, 10_000)
            .map_err(|e| {
                ApiError::Conflict(format!(
                    "this node may not host OSDs ({e}); set its role to storage with PUT /api/cluster/nodes/:name/caps"
                ))
            })?;
    }
    let out = run_secs(
        "lsblk",
        &[
            "-J",
            "-p",
            "-o",
            "NAME,TYPE,FSTYPE,MOUNTPOINT,RO",
            &b.device,
        ],
        15,
    )
    .await
    .map_err(|_| ApiError::NotFound(format!("{} is not a block device of this node", b.device)))?;
    let j: Value = serde_json::from_str(&out)
        .map_err(|e| ApiError::Internal(format!("lsblk: unreadable JSON: {e}")))?;
    check_osd_disk(&b.device, &j).map_err(ApiError::Conflict)?;
    let r = run_secs("ceph-volume", &["lvm", "create", "--data", &b.device], 600).await;
    tasks::record_done(
        "ceph-osd",
        &format!("create:{}", b.device),
        &super::system_admin::user_of(&u),
        &r.as_ref()
            .map(|_| "OSD created".to_string())
            .map_err(|e| e.clone()),
    );
    r.map_err(ApiError::Conflict)?;
    Ok(Json(serde_json::json!({"created": b.device})))
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
    /// Name of an existing CRUSH rule (e.g. an NVMe-only rule from
    /// `POST /api/ceph/crush-rules`); omit for the cluster default.
    #[serde(default)]
    pub crush_rule: Option<String>,
    /// Replicas needed to keep serving I/O (default: Ceph's own).
    #[serde(default)]
    pub min_size: Option<u32>,
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
    if let Some(m) = p.min_size {
        if m == 0 || m > p.size {
            return Err("min_size must be between 1 and size".into());
        }
    }
    if let Some(r) = p.crush_rule.as_deref() {
        valid_ceph_name(r, "crush_rule")?;
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
    if let Some(r) = &p.crush_rule {
        let rules = parse_crush_rules(&ceph(&["osd", "crush", "rule", "dump"]).await?);
        if !rules.iter().any(|x| &x.name == r) {
            return Err(ApiError::ValidationError(format!(
                "CRUSH rule {r} does not exist (create it first, or omit crush_rule)"
            )));
        }
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
        if let Some(r) = &p.crush_rule {
            s.push(vec![
                "osd".into(),
                "pool".into(),
                "set".into(),
                p.name.clone(),
                "crush_rule".into(),
                r.clone(),
            ]);
        }
        if let Some(m) = p.min_size {
            s.push(vec![
                "osd".into(),
                "pool".into(),
                "set".into(),
                p.name.clone(),
                "min_size".into(),
                m.to_string(),
            ]);
        }
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

fn valid_ceph_name(v: &str, what: &str) -> Result<(), String> {
    let ok = !v.is_empty()
        && v.len() <= 64
        && !v.starts_with(['.', '-'])
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    if ok {
        Ok(())
    } else {
        Err(format!(
            "{what}: 1-64 letters, digits, _ . - (not starting with . or -)"
        ))
    }
}

#[derive(Debug, Deserialize)]
pub struct NewCrushRule {
    pub name: String,
    /// CRUSH bucket type replicas must not share: host (default), osd, chassis, rack, row, room, datacenter
    #[serde(default = "default_failure_domain")]
    pub failure_domain: String,
    /// Only use OSDs of this device class: hdd, ssd or nvme (omit for all)
    #[serde(default)]
    pub device_class: Option<String>,
    /// CRUSH root bucket (default `default`)
    #[serde(default = "default_crush_root")]
    pub root: String,
}

fn default_failure_domain() -> String {
    "host".into()
}

fn default_crush_root() -> String {
    "default".into()
}

pub fn validate_crush_rule(r: &NewCrushRule) -> Result<(), String> {
    valid_ceph_name(&r.name, "rule name")?;
    valid_ceph_name(&r.root, "root")?;
    if !["osd", "host", "chassis", "rack", "row", "room", "datacenter"]
        .contains(&r.failure_domain.as_str())
    {
        return Err(
            "failure_domain must be osd, host, chassis, rack, row, room or datacenter".into(),
        );
    }
    if let Some(c) = r.device_class.as_deref() {
        if !["hdd", "ssd", "nvme"].contains(&c) {
            return Err("device_class must be hdd, ssd or nvme".into());
        }
    }
    Ok(())
}

/// `ceph osd crush rule create-replicated <name> <root> <type> [<class>]`
pub fn crush_rule_cmd(r: &NewCrushRule) -> Vec<String> {
    let mut v: Vec<String> = ["osd", "crush", "rule", "create-replicated"]
        .iter()
        .map(|x| x.to_string())
        .collect();
    v.extend([r.name.clone(), r.root.clone(), r.failure_domain.clone()]);
    if let Some(c) = &r.device_class {
        v.push(c.clone());
    }
    v
}

async fn create_crush_rule(u: AuthExt, Json(r): Json<NewCrushRule>) -> Res<serde_json::Value> {
    validate_crush_rule(&r).map_err(ApiError::ValidationError)?;
    let existing = parse_crush_rules(&ceph(&["osd", "crush", "rule", "dump"]).await?);
    if existing.iter().any(|x| x.name == r.name) {
        return Err(ApiError::Conflict(format!(
            "CRUSH rule {} already exists",
            r.name
        )));
    }
    let cmd = crush_rule_cmd(&r);
    let refs: Vec<&str> = cmd.iter().map(|x| x.as_str()).collect();
    let res = ceph_do(&refs).await;
    tasks::record_done(
        "ceph-crush-rule",
        &r.name,
        &super::system_admin::user_of(&u),
        &res.as_ref()
            .map(|_| {
                format!(
                    "created: {} over {}{}",
                    r.root,
                    r.failure_domain,
                    r.device_class
                        .as_deref()
                        .map(|c| format!(", class {c}"))
                        .unwrap_or_default()
                )
            })
            .map_err(|e| format!("{e:?}")),
    );
    res?;
    Ok(Json(serde_json::json!({"created": r.name})))
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

    #[test]
    fn osd_device_path_rules() {
        assert!(validate_osd_device_path("/dev/nvme1n1").is_ok());
        assert!(validate_osd_device_path("/dev/disk/by-id/nvme-X_1").is_ok());
        for bad in [
            "nvme1n1",
            "/dev/",
            "/dev/../etc/passwd",
            "/dev/sda; rm",
            "-x",
            "/etc/passwd",
        ] {
            assert!(validate_osd_device_path(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn osd_disk_only_when_blank() {
        let lsblk = |t: &str| serde_json::from_str::<Value>(t).unwrap();
        let blank = lsblk(
            r#"{"blockdevices":[{"name":"/dev/nvme1n1","type":"disk","fstype":null,"mountpoint":null,"ro":false}]}"#,
        );
        assert!(check_osd_disk("/dev/nvme1n1", &blank).is_ok());
        let part = lsblk(
            r#"{"blockdevices":[{"name":"/dev/sda","type":"disk","fstype":null,"mountpoint":null,"ro":false,"children":[{"name":"/dev/sda1","type":"part","fstype":"ext4","mountpoint":"/boot"}]}]}"#,
        );
        assert!(check_osd_disk("/dev/sda", &part)
            .unwrap_err()
            .contains("mounted"));
        let unmounted = lsblk(
            r#"{"blockdevices":[{"name":"/dev/sdb","type":"disk","fstype":null,"mountpoint":null,"ro":false,"children":[{"name":"/dev/sdb1","type":"part","fstype":"xfs","mountpoint":null}]}]}"#,
        );
        assert!(check_osd_disk("/dev/sdb", &unmounted)
            .unwrap_err()
            .contains("partitions"));
        let sig = lsblk(
            r#"{"blockdevices":[{"name":"/dev/sdc","type":"disk","fstype":"LVM2_member","mountpoint":null,"ro":false}]}"#,
        );
        assert!(check_osd_disk("/dev/sdc", &sig)
            .unwrap_err()
            .contains("LVM2_member"));
        let partdev = lsblk(
            r#"{"blockdevices":[{"name":"/dev/sdb1","type":"part","fstype":null,"mountpoint":null,"ro":false}]}"#,
        );
        assert!(check_osd_disk("/dev/sdb1", &partdev)
            .unwrap_err()
            .contains("whole disk"));
        let ro = lsblk(
            r#"{"blockdevices":[{"name":"/dev/sr0","type":"disk","fstype":null,"mountpoint":null,"ro":true}]}"#,
        );
        assert!(check_osd_disk("/dev/sr0", &ro)
            .unwrap_err()
            .contains("read-only"));
        let mps = lsblk(
            r#"{"blockdevices":[{"name":"/dev/sdd","type":"disk","fstype":null,"mountpoints":["/data"],"ro":false}]}"#,
        );
        assert!(check_osd_disk("/dev/sdd", &mps).is_err());
        assert!(check_osd_disk("/dev/x", &lsblk(r#"{"blockdevices":[]}"#)).is_err());
    }

    #[test]
    fn recovery_tuning_cmds_scale_with_link() {
        let flat = |m| {
            recovery_tuning_cmds(m)
                .iter()
                .map(|c| c.join(" "))
                .collect::<Vec<_>>()
        };
        let slow = flat(1_000);
        assert_eq!(slow[0], "config set osd osd_max_backfills 1");
        assert_eq!(slow[2], "config set osd osd_recovery_sleep_hdd 0.1");
        assert_eq!(slow[3], "config set osd osd_recovery_sleep_ssd 0.1");
        let fast = flat(100_000);
        assert_eq!(fast[0], "config set osd osd_max_backfills 4");
        assert_eq!(fast[1], "config set osd osd_recovery_max_active 8");
        assert_eq!(fast[3], "config set osd osd_recovery_sleep_ssd 0");
    }

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
            crush_rule: None,
            min_size: None,
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

    #[test]
    fn crush_rules_are_validated_and_built() {
        let r = |n: &str, fd: &str, dc: Option<&str>| NewCrushRule {
            name: n.into(),
            failure_domain: fd.into(),
            device_class: dc.map(String::from),
            root: "default".into(),
        };
        let good = r("nvme-host", "host", Some("nvme"));
        assert!(validate_crush_rule(&good).is_ok());
        assert_eq!(
            crush_rule_cmd(&good).join(" "),
            "osd crush rule create-replicated nvme-host default host nvme"
        );
        assert_eq!(
            crush_rule_cmd(&r("all", "osd", None)).join(" "),
            "osd crush rule create-replicated all default osd"
        );
        for bad in [
            r("", "host", None),
            r("a b", "host", None),
            r("x", "galaxy", None),
            r("x", "host", Some("tape")),
            r("-x", "host", None),
        ] {
            assert!(validate_crush_rule(&bad).is_err(), "{:?}", bad.name);
        }
    }

    #[test]
    fn pool_min_size_and_rule_are_validated() {
        let mut p = NewPool {
            name: "fast".into(),
            pg_num: 32,
            size: 3,
            application: None,
            crush_rule: Some("nvme-host".into()),
            min_size: Some(2),
        };
        assert!(validate_pool(&p).is_ok());
        p.min_size = Some(4);
        assert!(validate_pool(&p).is_err());
        p.min_size = Some(0);
        assert!(validate_pool(&p).is_err());
        p.min_size = None;
        p.crush_rule = Some("bad rule".into());
        assert!(validate_pool(&p).is_err());
    }
}
