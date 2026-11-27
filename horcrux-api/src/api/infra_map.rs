//! Infrastructure map (docs/design/infra-map.md): the `infra-map/v1` graph of what this node manages
//! (host, VMs, containers, storage, bridges), merged with configured sources: a Patronus router's
//! network map (cross-linked by MAC: a VM NIC that Patronus also sees gets a `same` edge) and a
//! read-only Proxmox VE import. Drawn by the shared component `ui-kit/map/map.js` (also served here).
//!
//! Signed in:
//! - `GET  /api/infra-map`                       graph (RBAC-filtered like `/api/tree`)
//! - `GET/POST /api/infra-map/sources`, `DELETE /api/infra-map/sources/:id`   (Sys.Modify to change)
//! - `GET/POST /api/infra-map/keys`, `DELETE /api/infra-map/keys/:id`         read-only share keys
//!
//! Public (share key in `X-Map-Key` or `?key=`):
//! - `GET /map/live` (key in the URL fragment), `GET /api/infra-map/shared/graph`,
//!   `GET /api/infra-map/stats` (Homepage `customapi`), `GET /ui-map/map.js|map.css`
//!
//! Source secrets are stored like metric-server tokens: never returned (`********`), sending the
//! mask back keeps the stored one.
use crate::auth::store::Principal;
use crate::db::config_store::ConfigStore;
use crate::error::ApiError;
use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap},
    response::{Html, IntoResponse, Response},
    routing::{delete, get},
    Extension, Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

const SOURCE_KIND: &str = "infra_map_source";
const KEY_KIND: &str = "infra_map_key";
const SECRET_MASK: &str = "********";
const MAP_JS: &str = include_str!("../../../ui-kit/map/map.js");
const MAP_CSS: &str = include_str!("../../../ui-kit/map/map.css");
const LIVE_HTML: &str = include_str!("infra_map_live.html");
/// Largest source answer accepted (bytes).
const MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/infra-map", get(graph_handler))
        .route("/api/infra-map/sources", get(list_sources).post(add_source))
        .route("/api/infra-map/sources/:id", delete(del_source))
        .route("/api/infra-map/keys", get(list_keys).post(add_key))
        .route("/api/infra-map/keys/:id", delete(del_key))
}

/// No session: the handlers check the share key themselves.
pub fn public_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/infra-map/shared/graph", get(shared_graph))
        .route("/api/infra-map/stats", get(stats_handler))
        .route("/map/live", get(|| async { Html(LIVE_HTML) }))
        .route(
            "/ui-map/map.js",
            get(|| asset(MAP_JS, "text/javascript; charset=utf-8")),
        )
        .route(
            "/ui-map/map.css",
            get(|| asset(MAP_CSS, "text/css; charset=utf-8")),
        )
}

async fn asset(body: &'static str, ct: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, ct),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        body,
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// sources
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Source {
    #[serde(default)]
    pub id: String,
    /// shown as the group prefix: [a-z0-9_-]{1,32}
    pub name: String,
    /// patronus | proxmox | unifi | unraid
    pub kind: String,
    /// base URL, e.g. https://192.168.1.1 or https://pve:8006
    pub url: String,
    /// patronus: a share key (pmk_...); proxmox: `user@realm!tokenid=secret`
    #[serde(default)]
    pub secret: String,
    #[serde(default = "yes")]
    pub verify_tls: bool,
    #[serde(default = "thirty")]
    pub refresh_secs: u64,
    #[serde(default = "ten")]
    pub timeout_secs: u64,
}
fn yes() -> bool {
    true
}
fn thirty() -> u64 {
    30
}
fn ten() -> u64 {
    10
}

fn is_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

pub fn check_source(s: &Source) -> Result<(), ApiError> {
    if !is_name(&s.name) {
        return Err(ApiError::BadRequest(
            "source name: [a-z0-9_-], 1-32 characters".into(),
        ));
    }
    match s.kind.as_str() {
        "patronus" | "proxmox" => {}
        "unifi" | "unraid" => {
            return Err(ApiError::NotAvailable {
                reason: "infra-map-source-not-built",
                message: format!(
                    "{} import is specified (docs/design/infra-map.md) but not built yet",
                    s.kind
                ),
                hint: "Use a Patronus or Proxmox source for now.".into(),
            })
        }
        k => {
            return Err(ApiError::BadRequest(format!(
                "source kind {k:?}: patronus, proxmox, unifi or unraid"
            )))
        }
    }
    let u = s.url.trim();
    if !(u.starts_with("https://") || u.starts_with("http://"))
        || u.len() > 256
        || u.contains(char::is_whitespace)
    {
        return Err(ApiError::BadRequest(
            "source url: http(s)://host[:port]".into(),
        ));
    }
    if !(5..=3600).contains(&s.refresh_secs) || !(1..=120).contains(&s.timeout_secs) {
        return Err(ApiError::BadRequest(
            "refresh_secs 5-3600, timeout_secs 1-120".into(),
        ));
    }
    if s.secret.is_empty() {
        return Err(ApiError::BadRequest(match s.kind.as_str() {
            "patronus" => {
                "a Patronus map share key (Patronus: Network map > Sharing) is required".into()
            }
            _ => {
                "a Proxmox API token (user@realm!tokenid=secret, PVEAuditor is enough) is required"
                    .into()
            }
        }));
    }
    Ok(())
}

async fn sources(state: &AppState) -> Vec<Source> {
    ConfigStore::new(&state.database)
        .load_values(SOURCE_KIND)
        .await
        .unwrap_or_default()
}

async fn list_sources(State(state): State<Arc<AppState>>) -> Json<Vec<Source>> {
    Json(
        sources(&state)
            .await
            .into_iter()
            .map(|mut s| {
                s.secret = SECRET_MASK.into();
                s
            })
            .collect(),
    )
}

async fn add_source(
    State(state): State<Arc<AppState>>,
    Json(mut s): Json<Source>,
) -> Result<Json<Value>, ApiError> {
    let existing = sources(&state).await;
    let old = existing.iter().find(|x| x.name == s.name);
    if s.secret == SECRET_MASK {
        s.secret = old.map(|o| o.secret.clone()).unwrap_or_default();
    }
    s.url = s.url.trim().trim_end_matches('/').to_string();
    check_source(&s)?;
    s.id = s.name.clone();
    ConfigStore::new(&state.database)
        .put(SOURCE_KIND, &s.id, &s)
        .await?;
    if let Ok(mut c) = CACHE.lock() {
        c.remove(&s.id);
    }
    Ok(Json(json!({"id": s.id})))
}

async fn del_source(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    if !ConfigStore::new(&state.database)
        .delete(SOURCE_KIND, &id)
        .await?
    {
        return Err(ApiError::NotFound(format!("map source {id} not found")));
    }
    if let Ok(mut c) = CACHE.lock() {
        c.remove(&id);
    }
    Ok(Json(json!({"ok": true})))
}

/// Last good answer per source: (fetched at, graph or error).
type Cached = (i64, Result<Value, String>);
static CACHE: Mutex<BTreeMap<String, Cached>> = Mutex::new(BTreeMap::new());

async fn fetch_json(s: &Source, path: &str, headers: &[(&str, String)]) -> Result<Value, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(s.timeout_secs))
        .danger_accept_invalid_certs(!s.verify_tls)
        .build()
        .map_err(|e| e.to_string())?;
    let mut req = client.get(format!("{}{}", s.url, path));
    for (k, v) in headers {
        req = req.header(*k, v.as_str());
    }
    let r = req.send().await.map_err(|e| format!("{}: {e}", s.url))?;
    if !r.status().is_success() {
        return Err(format!("{} answered {}", s.url, r.status()));
    }
    if r.content_length()
        .map(|l| l as usize > MAX_SOURCE_BYTES)
        .unwrap_or(false)
    {
        return Err("answer larger than 4 MiB".into());
    }
    let b = r.bytes().await.map_err(|e| e.to_string())?;
    if b.len() > MAX_SOURCE_BYTES {
        return Err("answer larger than 4 MiB".into());
    }
    serde_json::from_slice(&b).map_err(|e| format!("not JSON: {e}"))
}

/// Proxmox `GET /api2/json/cluster/resources` -> infra-map/v1 (read-only import).
pub fn from_proxmox(v: &Value) -> Value {
    let mut groups = Vec::new();
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let items = v["data"].as_array().cloned().unwrap_or_default();
    let st = |x: &Value| match x["status"].as_str().unwrap_or("") {
        "running" | "online" | "available" => "up",
        "stopped" | "offline" | "unknown" => "down",
        "" => "unknown",
        _ => "warn",
    };
    for n in items.iter().filter(|x| x["type"] == "node") {
        let name = n["node"].as_str().unwrap_or("node");
        groups.push(
            json!({"id": format!("node:{name}"), "label": format!("Proxmox {name}"), "kind": "host",
            "sub": format!("{} CPUs", n["maxcpu"].as_u64().unwrap_or(0))}),
        );
        nodes.push(json!({"id": format!("host:{name}"), "label": name, "kind": "host", "group": format!("node:{name}"), "status": st(n)}));
    }
    for x in &items {
        let (kind, idp) = match x["type"].as_str().unwrap_or("") {
            "qemu" => ("vm", "vm"),
            "lxc" => ("container", "ct"),
            "storage" => ("storage", "st"),
            _ => continue,
        };
        let node = x["node"].as_str().unwrap_or("");
        let id = if kind == "storage" {
            format!("st:{node}/{}", x["storage"].as_str().unwrap_or(""))
        } else {
            format!("{idp}:{}", x["vmid"].as_u64().unwrap_or(0))
        };
        let label = x["name"]
            .as_str()
            .or(x["storage"].as_str())
            .unwrap_or("?")
            .to_string();
        nodes.push(json!({"id": id, "label": label, "kind": kind, "group": format!("node:{node}"), "status": st(x),
            "meta": {"vmid": x["vmid"], "max memory": x["maxmem"], "max disk": x["maxdisk"], "pool": x["pool"]}}));
        if kind != "storage" {
            edges.push(json!({"from": format!("host:{node}"), "to": id, "kind": "member"}));
        }
    }
    json!({"schema": "infra-map/v1", "source": {"product": "proxmox"}, "groups": groups, "nodes": nodes, "edges": edges})
}

async fn source_graph(s: &Source) -> Result<Value, String> {
    match s.kind.as_str() {
        "patronus" => {
            let g = fetch_json(
                s,
                "/api/map/shared/graph",
                &[("X-Map-Key", s.secret.clone())],
            )
            .await?;
            if g["schema"] != "infra-map/v1" {
                return Err("not an infra-map/v1 graph".into());
            }
            Ok(g)
        }
        "proxmox" => {
            let v = fetch_json(
                s,
                "/api2/json/cluster/resources",
                &[("Authorization", format!("PVEAPIToken={}", s.secret))],
            )
            .await?;
            Ok(from_proxmox(&v))
        }
        k => Err(format!("{k} import is not built yet")),
    }
}

async fn cached_graph(s: &Source) -> Result<Value, String> {
    let now = chrono::Utc::now().timestamp();
    if let Some((at, g)) = CACHE.lock().ok().and_then(|c| c.get(&s.id).cloned()) {
        if now - at < s.refresh_secs as i64 {
            return g;
        }
    }
    let g = source_graph(s).await;
    if let Ok(mut c) = CACHE.lock() {
        c.insert(s.id.clone(), (now, g.clone()));
    }
    g
}

/// Merge a source graph under the prefix `<name>/`: ids, groups and edges are prefixed, hubs become
/// members of a group named after the source, and the source's findings are kept.
pub fn merge_into(out: &mut Value, name: &str, g: &Value) {
    let p = |id: &Value| -> Value { json!(format!("{name}/{}", id.as_str().unwrap_or(""))) };
    let src_group = format!("{name}/");
    let product = g["source"]["product"].as_str().unwrap_or("source");
    let mut any_hub = false;
    for gr in g["groups"].as_array().cloned().unwrap_or_default() {
        let mut gr = gr.clone();
        gr["id"] = p(&gr["id"]);
        gr["label"] = json!(format!("{name}: {}", gr["label"].as_str().unwrap_or("")));
        push(out, "groups", gr);
    }
    for n in g["nodes"].as_array().cloned().unwrap_or_default() {
        let mut n = n.clone();
        n["id"] = p(&n["id"]);
        if let Some(o) = n.as_object_mut() {
            o.remove("href");
        }
        if n["group"].is_string() {
            n["group"] = p(&n["group"]);
        } else {
            any_hub = true;
            n["group"] = json!(src_group);
        }
        n["source"] = json!(name);
        push(out, "nodes", n);
    }
    if any_hub {
        push(
            out,
            "groups",
            json!({"id": src_group, "label": format!("{name} ({product})"), "kind": "source"}),
        );
    }
    for e in g["edges"].as_array().cloned().unwrap_or_default() {
        let mut e = e.clone();
        e["from"] = p(&e["from"]);
        e["to"] = p(&e["to"]);
        push(out, "edges", e);
    }
    for f in g["findings"].as_array().cloned().unwrap_or_default() {
        let mut f = f.clone();
        f["title"] = json!(format!("{name}: {}", f["title"].as_str().unwrap_or("")));
        if let Some(o) = f.as_object_mut() {
            o.remove("action");
        }
        push(out, "findings", f);
    }
}

fn push(out: &mut Value, k: &str, v: Value) {
    if let Some(a) = out[k].as_array_mut() {
        a.push(v);
    }
}

/// A VM NIC whose MAC a merged network source also sees: dashed `same` edge between the two.
pub fn link_by_mac(out: &mut Value) {
    let mut by_mac: BTreeMap<String, Vec<(String, bool)>> = BTreeMap::new();
    for n in out["nodes"].as_array().cloned().unwrap_or_default() {
        let id = n["id"].as_str().unwrap_or("").to_string();
        let external = n["source"].is_string();
        let macs: Vec<String> = match &n["macs"] {
            Value::Array(a) => a
                .iter()
                .filter_map(|m| m.as_str().map(str::to_ascii_lowercase))
                .collect(),
            _ => n["mac"]
                .as_str()
                .map(|m| vec![m.to_ascii_lowercase()])
                .unwrap_or_default(),
        };
        for m in macs {
            by_mac.entry(m).or_default().push((id.clone(), external));
        }
    }
    for (mac, ids) in by_mac {
        for (a, _) in ids.iter().filter(|(_, e)| !e) {
            for (b, _) in ids.iter().filter(|(_, e)| *e) {
                push(
                    out,
                    "edges",
                    json!({"from": a, "to": b, "kind": "same", "label": format!("same NIC {mac}")}),
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// the local graph
// ---------------------------------------------------------------------------

fn status_word(s: &str) -> &'static str {
    match s.to_ascii_lowercase().as_str() {
        "running" | "online" | "up" => "up",
        "stopped" | "offline" | "exited" | "dead" | "down" => "down",
        "paused" | "suspended" | "restarting" | "starting" | "stopping" => "warn",
        _ => "unknown",
    }
}

async fn local_graph(
    state: &AppState,
    principal: &Option<Extension<Principal>>,
    full: bool,
) -> Value {
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "node".into());
    let inv = super::inventory::inventory(state, principal).await;
    let allowed: BTreeSet<(String, String)> =
        inv.iter().map(|r| (r.kind.clone(), r.id.clone())).collect();
    let mut nodes = vec![
        json!({"id": "host:local", "label": host, "kind": "host", "status": "up", "href": "/"}),
    ];
    for n in state
        .cluster_manager
        .list_nodes()
        .await
        .into_iter()
        .filter(|n| !n.is_local)
    {
        let st = serde_json::to_value(&n.status)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        nodes.push(json!({"id": format!("host:{}", n.name), "label": n.name, "kind": "host", "status": status_word(&st),
            "ip": n.ip, "href": "/cluster"}));
    }
    let groups = vec![
        json!({"id": "g:vms", "label": "Virtual machines", "kind": "vm"}),
        json!({"id": "g:cts", "label": "Containers", "kind": "container"}),
        json!({"id": "g:storage", "label": "Storage", "kind": "storage"}),
        json!({"id": "g:net", "label": "Networks", "kind": "bridge"}),
    ];
    let mut edges = vec![
        json!({"from": "host:local", "to": "g:vms", "kind": "link"}),
        json!({"from": "host:local", "to": "g:cts", "kind": "link"}),
        json!({"from": "host:local", "to": "g:storage", "kind": "link"}),
        json!({"from": "host:local", "to": "g:net", "kind": "link"}),
    ];
    let pools = state.storage_manager.list_pools().await;
    for p in pools
        .iter()
        .filter(|p| allowed.contains(&("storage".to_string(), p.id.clone())))
    {
        let used = p.total.saturating_sub(p.available);
        let pct = (used * 100).checked_div(p.total).unwrap_or(0);
        nodes.push(json!({"id": format!("st:{}", p.id), "label": p.name, "kind": "storage", "group": "g:storage",
            "status": if !p.enabled { "down" } else if pct >= 90 { "warn" } else { "up" }, "href": "/storage",
            "meta": {"type": format!("{:?}", p.storage_type), "used": format!("{used} of {} GB ({pct}%)", p.total)}}));
    }
    let mut bridges: BTreeSet<String> = BTreeSet::new();
    for vm in state.vm_manager.list_vms().await {
        if !allowed.contains(&("vm".to_string(), vm.id.clone())) {
            continue;
        }
        let id = format!("vm:{}", vm.id);
        let macs: Vec<String> = vm
            .hardware
            .nets
            .iter()
            .filter_map(|n| n.mac.clone())
            .map(|m| m.to_ascii_lowercase())
            .collect();
        let mut meta = serde_json::Map::new();
        meta.insert("vCPUs".into(), json!(vm.cpus));
        meta.insert("memory".into(), json!(format!("{} MB", vm.memory)));
        if full && !macs.is_empty() {
            meta.insert("MACs".into(), json!(macs.join(", ")));
        }
        let st = serde_json::to_value(&vm.status)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        nodes.push(json!({"id": id, "label": vm.name, "kind": "vm", "group": "g:vms", "status": status_word(&st),
            "href": format!("/vms/{}", vm.id), "macs": if full { json!(macs) } else { json!([]) }, "meta": meta}));
        for n in &vm.hardware.nets {
            if n.bridge.is_empty() {
                continue;
            }
            bridges.insert(n.bridge.clone());
            edges.push(
                json!({"from": id, "to": format!("br:{}", n.bridge), "kind": "nic",
                "label": n.vlan.map(|v| format!("VLAN {v}")).unwrap_or_default()}),
            );
        }
        for d in &vm.disks {
            if let Some(p) = pools
                .iter()
                .find(|p| !p.path.is_empty() && d.path.starts_with(&p.path))
            {
                edges.push(json!({"from": id, "to": format!("st:{}", p.id), "kind": "disk"}));
            }
        }
    }
    for b in &bridges {
        nodes.push(json!({"id": format!("br:{b}"), "label": b, "kind": "bridge", "group": "g:net", "status": "up", "href": "/network/bridges"}));
    }
    for ct in state.container_manager.list_containers().await {
        if !allowed.contains(&("container".to_string(), ct.id.clone())) {
            continue;
        }
        let st = serde_json::to_value(&ct.status)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        nodes.push(json!({"id": format!("ct:{}", ct.id), "label": ct.name, "kind": "container", "group": "g:cts",
            "status": status_word(&st), "href": "/containers",
            "meta": {"runtime": format!("{:?}", ct.runtime).to_ascii_lowercase(), "memory": format!("{} MB", ct.memory)}}));
    }
    // a kind this node has none of gets no empty box (and no edge to it)
    let used: BTreeSet<String> = nodes
        .iter()
        .filter_map(|n| n["group"].as_str().map(str::to_string))
        .collect();
    let groups: Vec<Value> = groups
        .into_iter()
        .filter(|g| used.contains(g["id"].as_str().unwrap_or("")))
        .collect();
    edges.retain(|e| {
        !e["to"].as_str().unwrap_or("").starts_with("g:")
            || used.contains(e["to"].as_str().unwrap_or(""))
    });
    json!({
        "schema": "infra-map/v1",
        "source": {"product": "horcrux", "name": host, "generated_at": chrono::Utc::now().timestamp()},
        "refresh_secs": 30,
        "groups": groups, "nodes": nodes, "edges": edges, "findings": [],
    })
}

/// Local graph + every source. A source that fails shows as a warning node, never breaks the map.
pub async fn full_graph(
    state: &AppState,
    principal: &Option<Extension<Principal>>,
    full: bool,
) -> Value {
    let mut g = local_graph(state, principal, full).await;
    for s in sources(state).await {
        match cached_graph(&s).await {
            Ok(sg) => merge_into(&mut g, &s.name, &sg),
            Err(e) => push(
                &mut g,
                "nodes",
                json!({"id": format!("{}/error", s.name), "label": format!("{} ({})", s.name, s.kind),
                "kind": "external", "status": "warn", "source": s.name, "meta": {"error": e}}),
            ),
        }
    }
    link_by_mac(&mut g);
    let nodes = g["nodes"].as_array().cloned().unwrap_or_default();
    let up = nodes.iter().filter(|n| n["status"] == "up").count();
    let down = nodes.iter().filter(|n| n["status"] == "down").count();
    g["counts"] = json!({"nodes": nodes.len(), "up": up, "down": down});
    if !full {
        anonymize(&mut g);
    }
    g
}

/// Contains `hh:hh:hh:hh:hh:hh` anywhere.
fn looks_like_mac(s: &str) -> bool {
    let b = s.as_bytes();
    b.windows(17).any(|w| {
        w.iter().enumerate().all(|(i, c)| {
            if i % 3 == 2 {
                *c == b':'
            } else {
                c.is_ascii_hexdigit()
            }
        })
    })
}

/// A basic share carries no MAC address anywhere: ids that contain one (a merged router's `dev:<mac>`)
/// become `n<i>`, labels that are a MAC become "device <i>", details and admin links go.
pub fn anonymize(g: &mut Value) {
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    if let Some(a) = g["nodes"].as_array_mut() {
        for (i, n) in a.iter_mut().enumerate() {
            let id = n["id"].as_str().unwrap_or("").to_string();
            if looks_like_mac(&id) {
                let new = format!("n{i}");
                map.insert(id, new.clone());
                n["id"] = json!(new);
            }
            if n["label"].as_str().map(looks_like_mac).unwrap_or(false) {
                n["label"] = json!(format!("device {i}"));
            }
            if let Some(o) = n.as_object_mut() {
                o.remove("mac");
                o.remove("macs");
                o.remove("href");
                o.insert("meta".into(), json!({}));
            }
        }
    }
    if let Some(es) = g["edges"].as_array_mut() {
        for e in es.iter_mut() {
            for k in ["from", "to"] {
                if let Some(new) = e[k].as_str().and_then(|x| map.get(x)) {
                    e[k] = json!(new);
                }
            }
            if e["label"].as_str().map(looks_like_mac).unwrap_or(false) {
                e["label"] = json!("same NIC");
            }
        }
    }
    if let Some(fs) = g["findings"].as_array_mut() {
        for f in fs.iter_mut() {
            if let Some(o) = f.as_object_mut() {
                o.remove("nodes");
            }
        }
    }
}

async fn graph_handler(
    State(state): State<Arc<AppState>>,
    principal: Option<Extension<Principal>>,
) -> Json<Value> {
    let mut g = full_graph(&state, &principal, true).await;
    // MACs only feed the cross-links: the UI shows them in the details
    if let Some(a) = g["nodes"].as_array_mut() {
        for n in a {
            if let Some(o) = n.as_object_mut() {
                o.remove("macs");
            }
        }
    }
    Json(g)
}

// ---------------------------------------------------------------------------
// share keys
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ShareKey {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hash: String,
    pub scope: String,
    pub detail: String,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub last_used: Option<i64>,
}

pub fn hash_key(k: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(k.as_bytes()))
}

async fn keys(state: &AppState) -> Vec<ShareKey> {
    ConfigStore::new(&state.database)
        .load_values(KEY_KIND)
        .await
        .unwrap_or_default()
}

async fn list_keys(State(state): State<Arc<AppState>>) -> Json<Vec<ShareKey>> {
    Json(
        keys(&state)
            .await
            .into_iter()
            .map(|mut k| {
                k.hash.clear();
                k
            })
            .collect(),
    )
}

#[derive(Deserialize)]
struct KeyBody {
    name: String,
    #[serde(default = "view")]
    scope: String,
    #[serde(default = "basic")]
    detail: String,
    expires_days: Option<i64>,
}
fn view() -> String {
    "view".into()
}
fn basic() -> String {
    "basic".into()
}

async fn add_key(
    State(state): State<Arc<AppState>>,
    Json(b): Json<KeyBody>,
) -> Result<Json<Value>, ApiError> {
    if !is_name(&b.name) {
        return Err(ApiError::BadRequest(
            "key name: [a-z0-9_-], 1-32 characters".into(),
        ));
    }
    if !matches!(b.scope.as_str(), "view" | "stats")
        || !matches!(b.detail.as_str(), "basic" | "full")
    {
        return Err(ApiError::BadRequest(
            "scope view|stats, detail basic|full".into(),
        ));
    }
    if keys(&state).await.iter().any(|k| k.name == b.name) {
        return Err(ApiError::Conflict(format!(
            "share key {} already exists",
            b.name
        )));
    }
    use rand::RngCore;
    let mut raw = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    let key = format!("hmk_{}", hex::encode(raw));
    let now = chrono::Utc::now().timestamp();
    let k = ShareKey {
        id: b.name.clone(),
        name: b.name.clone(),
        hash: hash_key(&key),
        scope: b.scope,
        detail: b.detail,
        created_at: now,
        expires_at: b
            .expires_days
            .filter(|d| *d > 0)
            .map(|d| now + d.min(3650) * 86400),
        last_used: None,
    };
    ConfigStore::new(&state.database)
        .put(KEY_KIND, &k.id, &k)
        .await?;
    Ok(Json(
        json!({"id": k.id, "key": key, "live_path": format!("/map/live#k={key}")}),
    ))
}

async fn del_key(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    if !ConfigStore::new(&state.database)
        .delete(KEY_KIND, &id)
        .await?
    {
        return Err(ApiError::NotFound(format!("share key {id} not found")));
    }
    Ok(Json(json!({"ok": true})))
}

#[derive(Deserialize)]
struct KeyQuery {
    key: Option<String>,
}

async fn check_key(state: &AppState, headers: &HeaderMap, q: &KeyQuery) -> Option<ShareKey> {
    let k = headers
        .get("x-map-key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .or_else(|| q.key.clone())?;
    let k = k.trim();
    if !k.starts_with("hmk_") || k.len() > 80 {
        return None;
    }
    let h = hash_key(k);
    let now = chrono::Utc::now().timestamp();
    let mut found = keys(state).await.into_iter().find(|x| x.hash == h)?;
    if found.expires_at.map(|e| e <= now).unwrap_or(false) {
        return None;
    }
    if found.last_used.map(|t| now - t >= 60).unwrap_or(true) {
        found.last_used = Some(now);
        let _ = ConfigStore::new(&state.database)
            .put(KEY_KIND, &found.id, &found)
            .await;
    }
    Some(found)
}

fn denied() -> Response {
    (
        axum::http::StatusCode::UNAUTHORIZED,
        Json(json!({"error": "a valid map share key is required (X-Map-Key)"})),
    )
        .into_response()
}

async fn shared_graph(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<KeyQuery>,
) -> Response {
    let Some(k) = check_key(&state, &headers, &q).await else {
        return denied();
    };
    if k.scope != "view" {
        return (
            axum::http::StatusCode::FORBIDDEN,
            Json(json!({"error": "this key may only read stats"})),
        )
            .into_response();
    }
    // a share key sees what an administrator sees (no principal = no RBAC filter), minus details unless `full`
    let mut g = full_graph(&state, &None, k.detail == "full").await;
    if let Some(a) = g["nodes"].as_array_mut() {
        for n in a {
            if let Some(o) = n.as_object_mut() {
                o.remove("href");
                o.remove("macs");
            }
        }
    }
    Json(g).into_response()
}

async fn stats_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<KeyQuery>,
) -> Response {
    if check_key(&state, &headers, &q).await.is_none() {
        return denied();
    }
    let g = full_graph(&state, &None, false).await;
    let nodes = g["nodes"].as_array().cloned().unwrap_or_default();
    let count = |k: &str| {
        nodes
            .iter()
            .filter(|n| n["kind"] == k && !n["source"].is_string())
            .count()
    };
    let running = nodes
        .iter()
        .filter(|n| n["kind"] == "vm" && n["status"] == "up" && !n["source"].is_string())
        .count();
    Json(json!({
        "hosts": count("host"), "vms": count("vm"), "vms_running": running, "containers": count("container"),
        "storage": count("storage"), "down": g["counts"]["down"], "sources": sources(&state).await.len(),
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(kind: &str) -> Source {
        Source {
            id: String::new(),
            name: "router".into(),
            kind: kind.into(),
            url: "https://192.0.2.1".into(),
            secret: "pmk_x".into(),
            verify_tls: true,
            refresh_secs: 30,
            timeout_secs: 10,
        }
    }

    #[test]
    fn sources_are_validated() {
        assert!(check_source(&src("patronus")).is_ok());
        assert!(check_source(&src("proxmox")).is_ok());
        assert!(matches!(
            check_source(&src("unifi")),
            Err(ApiError::NotAvailable { .. })
        ));
        assert!(matches!(
            check_source(&src("bogus")),
            Err(ApiError::BadRequest(_))
        ));
        let mut s = src("patronus");
        s.url = "ftp://x".into();
        assert!(check_source(&s).is_err());
        s = src("patronus");
        s.secret.clear();
        assert!(check_source(&s).is_err());
        s = src("patronus");
        s.name = "Router 1".into();
        assert!(check_source(&s).is_err());
    }

    #[test]
    fn proxmox_resources_become_a_graph() {
        let v = json!({"data": [
            {"type": "node", "node": "pve1", "status": "online", "maxcpu": 8},
            {"type": "qemu", "node": "pve1", "vmid": 100, "name": "web", "status": "running"},
            {"type": "lxc", "node": "pve1", "vmid": 101, "name": "dns", "status": "stopped"},
            {"type": "storage", "node": "pve1", "storage": "local-zfs", "status": "available"},
            {"type": "sdn", "node": "pve1"}
        ]});
        let g = from_proxmox(&v);
        assert_eq!(g["groups"][0]["id"], "node:pve1");
        let n = g["nodes"].as_array().unwrap();
        assert_eq!(n.len(), 4);
        assert!(n
            .iter()
            .any(|x| x["id"] == "vm:100" && x["status"] == "up" && x["group"] == "node:pve1"));
        assert!(n
            .iter()
            .any(|x| x["id"] == "ct:101" && x["status"] == "down"));
        assert!(n
            .iter()
            .any(|x| x["id"] == "st:pve1/local-zfs" && x["kind"] == "storage"));
    }

    #[test]
    fn merge_prefixes_and_links_by_mac() {
        let mut out = json!({"schema": "infra-map/v1", "groups": [], "nodes": [
            {"id": "vm:1", "label": "web", "kind": "vm", "group": "g:vms", "macs": ["52:54:00:AA:00:01"]}
        ], "edges": [], "findings": []});
        let pat = json!({"schema": "infra-map/v1", "source": {"product": "patronus"},
            "groups": [{"id": "seg:@lan", "label": "LAN"}],
            "nodes": [{"id": "router", "kind": "router", "label": "gw", "href": "/"},
                      {"id": "dev:52:54:00:aa:00:01", "group": "seg:@lan", "mac": "52:54:00:aa:00:01", "label": "web"}],
            "edges": [{"from": "router", "to": "seg:@lan"}],
            "findings": [{"id": "flat-network", "title": "LAN is a flat network", "action": {"href": "/map"}}]});
        merge_into(&mut out, "home", &pat);
        link_by_mac(&mut out);
        let ids: Vec<&str> = out["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["id"].as_str().unwrap())
            .collect();
        assert!(ids.contains(&"home/router") && ids.contains(&"home/dev:52:54:00:aa:00:01"));
        assert!(out["groups"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g["id"] == "home/seg:@lan" && g["label"] == "home: LAN"));
        assert!(
            out["groups"]
                .as_array()
                .unwrap()
                .iter()
                .any(|g| g["id"] == "home/"),
            "hubs get a source group"
        );
        assert!(out["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n.get("href").is_none() || n["source"].is_null()));
        let e = out["edges"].as_array().unwrap();
        assert!(e
            .iter()
            .any(|x| x["from"] == "home/router" && x["to"] == "home/seg:@lan"));
        assert!(e.iter().any(|x| x["kind"] == "same"
            && x["from"] == "vm:1"
            && x["to"] == "home/dev:52:54:00:aa:00:01"));
        assert_eq!(out["findings"][0]["title"], "home: LAN is a flat network");
        assert!(out["findings"][0].get("action").is_none());
    }

    #[test]
    fn basic_shares_carry_no_mac_anywhere() {
        let mut g = json!({"nodes": [{"id": "vm:1", "label": "web", "macs": ["52:54:00:aa:00:01"]},
            {"id": "home/dev:52:54:00:aa:00:01", "label": "52:54:00:aa:00:01", "mac": "52:54:00:aa:00:01"}],
            "edges": [{"from": "vm:1", "to": "home/dev:52:54:00:aa:00:01", "kind": "same", "label": "same NIC 52:54:00:aa:00:01"}],
            "findings": [{"title": "x", "nodes": ["home/dev:52:54:00:aa:00:01"]}]});
        anonymize(&mut g);
        let t = g.to_string();
        assert!(!t.contains("52:54:00"), "{t}");
        assert_eq!(g["nodes"][0]["id"], "vm:1");
        assert_eq!(g["edges"][0]["to"], "n1");
        assert_eq!(g["nodes"][1]["label"], "device 1");
        assert!(
            looks_like_mac("home/dev:52:54:00:aa:00:01")
                && !looks_like_mac("vm:web1")
                && !looks_like_mac("st:local")
        );
    }

    #[test]
    fn statuses_map_to_tones() {
        assert_eq!(status_word("Running"), "up");
        assert_eq!(status_word("stopped"), "down");
        assert_eq!(status_word("paused"), "warn");
        assert_eq!(status_word("?"), "unknown");
        assert_eq!(hash_key("hmk_a").len(), 64);
    }
}
