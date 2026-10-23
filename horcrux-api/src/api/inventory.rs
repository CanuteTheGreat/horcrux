//! Inventory: tags on VMs/containers, one search over every resource, and
//! the resource tree (datacenter > node > VMs, containers, storages, with
//! the resource pools as a second grouping).
//!
//! - `PUT /api/vms/:id/tags`, `PUT /api/containers/:id/tags` `{tags: [..]}`
//! - `GET /api/tags`                         tag -> number of guests
//! - `GET /api/search?q=&kind=&tag=&status=&pool=`
//! - `GET /api/tree`
//!
//! Everything is filtered to what the caller may see (VM.Audit on the guest,
//! Datastore.Audit on the storage, Pool.Audit on the pool).
use crate::auth::store::Principal;
use crate::db::config_store::ConfigStore;
use crate::error::ApiError;
use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    routing::{get, put},
    Extension, Json, Router,
};
use horcrux_common::auth::Privilege;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

const KIND: &str = "guest_tags";
const MAX_TAGS: usize = 20;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/vms/:id/tags", put(put_vm_tags))
        .route("/api/containers/:id/tags", put(put_ct_tags))
        .route("/api/tags", get(list_tags))
        .route("/api/search", get(search))
        .route("/api/tree", get(tree))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Resource {
    /// vm | container | storage | pool | node
    pub kind: String,
    pub id: String,
    pub name: String,
    pub status: String,
    pub tags: Vec<String>,
    pub pool: Option<String>,
    /// UI route to open it.
    pub link: String,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct TreeNode {
    pub kind: String,
    pub id: String,
    pub name: String,
    pub status: String,
    pub tags: Vec<String>,
    pub link: String,
    pub children: Vec<TreeNode>,
}

/// Tags are lower-case words: `[a-z0-9][a-z0-9_.-]{0,31}`.
pub fn clean_tags(raw: Vec<String>) -> Result<Vec<String>, ApiError> {
    let mut out: Vec<String> = Vec::new();
    for t in raw {
        let t = t.trim().to_ascii_lowercase();
        let ok = !t.is_empty()
            && t.len() <= 32
            && t.starts_with(|c: char| c.is_ascii_alphanumeric())
            && t.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'));
        if !ok {
            return Err(ApiError::BadRequest(format!(
                "tag {t:?}: letters, digits, '_', '.', '-' (max 32), starting with a letter or digit"
            )));
        }
        if !out.contains(&t) {
            out.push(t);
        }
    }
    if out.len() > MAX_TAGS {
        return Err(ApiError::BadRequest(format!("at most {MAX_TAGS} tags")));
    }
    out.sort();
    Ok(out)
}

#[derive(Deserialize)]
struct TagsBody {
    tags: Vec<String>,
}

#[derive(Serialize, Deserialize, Default)]
struct StoredTags {
    tags: Vec<String>,
}

async fn all_tags(state: &AppState) -> BTreeMap<String, Vec<String>> {
    ConfigStore::new(&state.database)
        .load::<StoredTags>(KIND)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(k, v)| (k, v.tags))
        .collect()
}

async fn put_vm_tags(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(b): Json<TagsBody>,
) -> Result<Json<Vec<String>>, ApiError> {
    state
        .vm_manager
        .get_vm(&id)
        .await
        .map_err(|_| ApiError::NotFound(format!("VM {id} not found")))?;
    store_tags(&state, format!("vm:{id}"), b.tags).await
}

async fn put_ct_tags(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(b): Json<TagsBody>,
) -> Result<Json<Vec<String>>, ApiError> {
    state
        .container_manager
        .get_container(&id)
        .await
        .map_err(|_| ApiError::NotFound(format!("container {id} not found")))?;
    store_tags(&state, format!("ct:{id}"), b.tags).await
}

async fn store_tags(
    state: &AppState,
    key: String,
    tags: Vec<String>,
) -> Result<Json<Vec<String>>, ApiError> {
    let tags = clean_tags(tags)?;
    let store = ConfigStore::new(&state.database);
    if tags.is_empty() {
        store.delete(KIND, &key).await?;
    } else {
        store
            .put(KIND, &key, &StoredTags { tags: tags.clone() })
            .await?;
    }
    Ok(Json(tags))
}

fn status_of<T: Serialize>(s: &T) -> String {
    serde_json::to_value(s)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Every resource the caller may see.
async fn inventory(state: &AppState, principal: &Option<Extension<Principal>>) -> Vec<Resource> {
    let can = |p: Privilege, path: String| match principal {
        Some(Extension(pr)) => pr.has(p, &path),
        None => true,
    };
    let tags = all_tags(state).await;
    let pools: Vec<super::resource_pools::ResourcePool> = ConfigStore::new(&state.database)
        .load_values("resource_pool")
        .await
        .unwrap_or_default();
    let pool_of = |list: fn(&super::resource_pools::ResourcePool) -> &Vec<String>, id: &str| {
        pools
            .iter()
            .find(|p| list(p).iter().any(|m| m == id))
            .map(|p| p.id.clone())
    };
    let mut out = Vec::new();
    for vm in state.vm_manager.list_vms().await {
        if !can(Privilege::VmAudit, format!("/vms/{}", vm.id)) {
            continue;
        }
        out.push(Resource {
            kind: "vm".into(),
            tags: tags
                .get(&format!("vm:{}", vm.id))
                .cloned()
                .unwrap_or_default(),
            pool: pool_of(|p| &p.vms, &vm.id),
            link: format!("/vms/{}", vm.id),
            status: status_of(&vm.status),
            name: vm.name,
            id: vm.id,
        });
    }
    for ct in state.container_manager.list_containers().await {
        if !can(Privilege::VmAudit, format!("/containers/{}", ct.id)) {
            continue;
        }
        out.push(Resource {
            kind: "container".into(),
            tags: tags
                .get(&format!("ct:{}", ct.id))
                .cloned()
                .unwrap_or_default(),
            pool: pool_of(|p| &p.containers, &ct.id),
            link: "/containers".into(),
            status: status_of(&ct.status),
            name: ct.name,
            id: ct.id,
        });
    }
    for sp in state.storage_manager.list_pools().await {
        if !can(Privilege::DatastoreAudit, format!("/storage/{}", sp.id)) {
            continue;
        }
        out.push(Resource {
            kind: "storage".into(),
            tags: Vec::new(),
            pool: pool_of(|p| &p.storages, &sp.id),
            link: "/storage".into(),
            status: "online".into(),
            name: sp.name,
            id: sp.id,
        });
    }
    for p in &pools {
        if !can(Privilege::PoolAudit, format!("/pool/{}", p.id)) {
            continue;
        }
        out.push(Resource {
            kind: "pool".into(),
            id: p.id.clone(),
            name: p.id.clone(),
            status: format!(
                "{} members",
                p.vms.len() + p.containers.len() + p.storages.len()
            ),
            tags: Vec::new(),
            pool: None,
            link: "/pools".into(),
        });
    }
    out
}

#[derive(Deserialize)]
struct SearchQuery {
    q: Option<String>,
    kind: Option<String>,
    tag: Option<String>,
    status: Option<String>,
    pool: Option<String>,
}

pub fn matches(r: &Resource, q: &SearchQueryView) -> bool {
    if let Some(k) = q.kind {
        if r.kind != k {
            return false;
        }
    }
    if let Some(t) = q.tag {
        if !r.tags.iter().any(|x| x == &t.to_ascii_lowercase()) {
            return false;
        }
    }
    if let Some(s) = q.status {
        if !r.status.eq_ignore_ascii_case(s) {
            return false;
        }
    }
    if let Some(p) = q.pool {
        if r.pool.as_deref() != Some(p) {
            return false;
        }
    }
    if let Some(text) = q.text {
        let t = text.to_ascii_lowercase();
        let hay = format!(
            "{} {} {} {}",
            r.id,
            r.name,
            r.tags.join(" "),
            r.pool.clone().unwrap_or_default()
        )
        .to_ascii_lowercase();
        // every word must appear (AND)
        if !t.split_whitespace().all(|w| hay.contains(w)) {
            return false;
        }
    }
    true
}

pub struct SearchQueryView<'a> {
    pub text: Option<&'a str>,
    pub kind: Option<&'a str>,
    pub tag: Option<&'a str>,
    pub status: Option<&'a str>,
    pub pool: Option<&'a str>,
}

async fn search(
    State(state): State<Arc<AppState>>,
    principal: Option<Extension<Principal>>,
    Query(q): Query<SearchQuery>,
) -> Json<Vec<Resource>> {
    let view = SearchQueryView {
        text: q.q.as_deref().filter(|s| !s.trim().is_empty()),
        kind: q.kind.as_deref().filter(|s| !s.is_empty()),
        tag: q.tag.as_deref().filter(|s| !s.is_empty()),
        status: q.status.as_deref().filter(|s| !s.is_empty()),
        pool: q.pool.as_deref().filter(|s| !s.is_empty()),
    };
    let mut v: Vec<Resource> = inventory(&state, &principal)
        .await
        .into_iter()
        .filter(|r| matches(r, &view))
        .collect();
    v.sort_by(|a, b| (&a.kind, &a.name).cmp(&(&b.kind, &b.name)));
    Json(v)
}

#[derive(Serialize)]
struct TagCount {
    tag: String,
    count: usize,
}

async fn list_tags(
    State(state): State<Arc<AppState>>,
    principal: Option<Extension<Principal>>,
) -> Json<Vec<TagCount>> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for r in inventory(&state, &principal).await {
        for t in r.tags {
            *counts.entry(t).or_default() += 1;
        }
    }
    Json(
        counts
            .into_iter()
            .map(|(tag, count)| TagCount { tag, count })
            .collect(),
    )
}

fn node_of(r: Resource) -> TreeNode {
    TreeNode {
        kind: r.kind,
        id: r.id,
        name: r.name,
        status: r.status,
        tags: r.tags,
        link: r.link,
        children: Vec::new(),
    }
}

/// Datacenter > this node > (VMs, containers, storages), then Pools >
/// each pool > its members.
pub fn build_tree(node_name: &str, resources: Vec<Resource>) -> TreeNode {
    let group = |title: &str, kind: &str, rs: &[Resource]| TreeNode {
        kind: "group".into(),
        id: format!("{kind}s"),
        name: format!("{title} ({})", rs.len()),
        status: String::new(),
        tags: Vec::new(),
        link: String::new(),
        children: rs.iter().cloned().map(node_of).collect(),
    };
    let of =
        |k: &str| -> Vec<Resource> { resources.iter().filter(|r| r.kind == k).cloned().collect() };
    let (vms, cts, sts, pools) = (of("vm"), of("container"), of("storage"), of("pool"));
    let node = TreeNode {
        kind: "node".into(),
        id: node_name.into(),
        name: node_name.into(),
        status: "online".into(),
        tags: Vec::new(),
        link: "/system/health".into(),
        children: vec![
            group("Virtual machines", "vm", &vms),
            group("Containers", "container", &cts),
            group("Storage", "storage", &sts),
        ],
    };
    let pool_nodes: Vec<TreeNode> = pools
        .into_iter()
        .map(|p| {
            let members: Vec<TreeNode> = resources
                .iter()
                .filter(|r| r.pool.as_deref() == Some(p.id.as_str()))
                .cloned()
                .map(node_of)
                .collect();
            TreeNode {
                children: members,
                ..node_of(p)
            }
        })
        .collect();
    TreeNode {
        kind: "datacenter".into(),
        id: "dc".into(),
        name: "Datacenter".into(),
        status: String::new(),
        tags: Vec::new(),
        link: "/".into(),
        children: vec![
            node,
            TreeNode {
                kind: "group".into(),
                id: "pools".into(),
                name: format!("Pools ({})", pool_nodes.len()),
                status: String::new(),
                tags: Vec::new(),
                link: "/pools".into(),
                children: pool_nodes,
            },
        ],
    }
}

async fn tree(
    State(state): State<Arc<AppState>>,
    principal: Option<Extension<Principal>>,
) -> Json<TreeNode> {
    let node = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "node".into());
    Json(build_tree(&node, inventory(&state, &principal).await))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn res(
        kind: &str,
        id: &str,
        name: &str,
        status: &str,
        tags: &[&str],
        pool: Option<&str>,
    ) -> Resource {
        Resource {
            kind: kind.into(),
            id: id.into(),
            name: name.into(),
            status: status.into(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            pool: pool.map(str::to_string),
            link: String::new(),
        }
    }

    #[test]
    fn tags_are_normalised_and_validated() {
        assert_eq!(
            clean_tags(vec![" Prod ".into(), "db".into(), "prod".into()]).unwrap(),
            vec!["db".to_string(), "prod".to_string()]
        );
        for bad in ["", "-x", "a b", "a/b", "é"] {
            assert!(clean_tags(vec![bad.into()]).is_err(), "{bad:?}");
        }
        assert!(clean_tags((0..21).map(|i| format!("t{i}")).collect()).is_err());
    }

    #[test]
    fn search_filters_combine() {
        let r = res(
            "vm",
            "100",
            "Web Frontend",
            "running",
            &["prod", "web"],
            Some("team-a"),
        );
        let q = |text, kind, tag, status, pool| SearchQueryView {
            text,
            kind,
            tag,
            status,
            pool,
        };
        assert!(matches(&r, &q(Some("web front"), None, None, None, None)));
        assert!(matches(
            &r,
            &q(
                Some("team-a"),
                Some("vm"),
                Some("PROD"),
                Some("Running"),
                Some("team-a")
            )
        ));
        assert!(!matches(&r, &q(Some("db"), None, None, None, None)));
        assert!(!matches(&r, &q(None, Some("container"), None, None, None)));
        assert!(!matches(&r, &q(None, None, Some("dev"), None, None)));
        assert!(!matches(&r, &q(None, None, None, Some("stopped"), None)));
        assert!(!matches(&r, &q(None, None, None, None, Some("other"))));
    }

    #[test]
    fn tree_groups_by_node_and_pool() {
        let t = build_tree(
            "n1",
            vec![
                res("vm", "1", "a", "running", &[], Some("p")),
                res("vm", "2", "b", "stopped", &[], None),
                res("container", "c1", "c", "running", &[], None),
                res("storage", "s1", "local", "online", &[], None),
                res("pool", "p", "p", "1 members", &[], None),
            ],
        );
        assert_eq!(t.children[0].children[0].name, "Virtual machines (2)");
        assert_eq!(t.children[0].children[1].children.len(), 1);
        let pools = &t.children[1];
        assert_eq!(pools.children[0].children.len(), 1);
        assert_eq!(pools.children[0].children[0].id, "1");
    }
}
