//! Resource pools (Proxmox "pools"): a named group of VMs, containers and
//! storages that is managed and delegated as one.
//!
//! - `GET    /api/pools`        list (filtered to the pools the caller may see)
//! - `POST   /api/pools`        `{id, comment, vms, containers, storages}`
//! - `GET    /api/pools/:id`
//! - `PUT    /api/pools/:id`    replace comment and/or members
//! - `DELETE /api/pools/:id`
//!
//! Real effect: an ACL entry on `/pool/<id>` (Access > permissions) applies
//! to every member, so a user granted a role on the pool can manage exactly
//! its guests and storages (see `Principal::load`). A guest or storage is in
//! at most one pool; adding a member needs the allocate privilege on that
//! member as well as `Pool.Allocate` on the pool.
use crate::auth::store::Principal;
use crate::db::config_store::ConfigStore;
use crate::error::ApiError;
use crate::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::get,
    Extension, Json, Router,
};
use horcrux_common::auth::Privilege;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::Arc;

const KIND: &str = "resource_pool";

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/pools", get(list).post(create))
        .route("/api/pools/:id", get(show).put(update).delete(remove))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ResourcePool {
    pub id: String,
    #[serde(default)]
    pub comment: String,
    #[serde(default)]
    pub vms: Vec<String>,
    #[serde(default)]
    pub containers: Vec<String>,
    #[serde(default)]
    pub storages: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct UpdateBody {
    comment: Option<String>,
    vms: Option<Vec<String>>,
    containers: Option<Vec<String>>,
    storages: Option<Vec<String>>,
}

fn valid_id(id: &str) -> Result<(), ApiError> {
    let ok = !id.is_empty()
        && id.len() <= 64
        && id.starts_with(|c: char| c.is_ascii_alphanumeric())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
    if ok {
        Ok(())
    } else {
        Err(ApiError::BadRequest(
            "pool id: letters, digits, '-', '_' and '.', starting with a letter or digit".into(),
        ))
    }
}

async fn all(state: &AppState) -> Result<Vec<ResourcePool>, ApiError> {
    let mut v: Vec<ResourcePool> = ConfigStore::new(&state.database).load_values(KIND).await?;
    v.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(v)
}

fn dedup(v: Vec<String>) -> Vec<String> {
    v.into_iter().collect::<BTreeSet<_>>().into_iter().collect()
}

async fn list(
    State(state): State<Arc<AppState>>,
    principal: Option<Extension<Principal>>,
) -> Result<Json<Vec<ResourcePool>>, ApiError> {
    let mut pools = all(&state).await?;
    if let Some(Extension(p)) = principal {
        pools.retain(|x| p.has(Privilege::PoolAudit, &format!("/pool/{}", x.id)));
    }
    Ok(Json(pools))
}

async fn show(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ResourcePool>, ApiError> {
    all(&state)
        .await?
        .into_iter()
        .find(|p| p.id == id)
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("pool {id} not found")))
}

/// Members must exist, belong to no other pool, and the caller must be
/// allowed to allocate them.
async fn check_members(
    state: &AppState,
    principal: &Option<Extension<Principal>>,
    pool: &ResourcePool,
    others: &[ResourcePool],
) -> Result<(), ApiError> {
    let need = |privilege: Privilege, path: String| -> Result<(), ApiError> {
        match principal {
            Some(Extension(p)) if !p.has(privilege, &path) => Err(ApiError::Forbidden(format!(
                "Permission check failed: {privilege:?} on {path}"
            ))),
            _ => Ok(()),
        }
    };
    let taken = |kind: &str, id: &str, list: fn(&ResourcePool) -> &Vec<String>| {
        others
            .iter()
            .find(|o| o.id != pool.id && list(o).iter().any(|m| m == id))
            .map(|o| ApiError::Conflict(format!("{kind} {id} is already in pool {}", o.id)))
    };
    for id in &pool.vms {
        state
            .vm_manager
            .get_vm(id)
            .await
            .map_err(|_| ApiError::BadRequest(format!("VM {id} does not exist")))?;
        if let Some(e) = taken("VM", id, |p| &p.vms) {
            return Err(e);
        }
        need(Privilege::VmAllocate, format!("/vms/{id}"))?;
    }
    for id in &pool.containers {
        state
            .container_manager
            .get_container(id)
            .await
            .map_err(|_| ApiError::BadRequest(format!("container {id} does not exist")))?;
        if let Some(e) = taken("container", id, |p| &p.containers) {
            return Err(e);
        }
        need(Privilege::VmAllocate, format!("/containers/{id}"))?;
    }
    for id in &pool.storages {
        state
            .storage_manager
            .get_pool(id)
            .await
            .map_err(|_| ApiError::BadRequest(format!("storage {id} does not exist")))?;
        if let Some(e) = taken("storage", id, |p| &p.storages) {
            return Err(e);
        }
        need(Privilege::DatastoreAllocate, format!("/storage/{id}"))?;
    }
    Ok(())
}

async fn create(
    State(state): State<Arc<AppState>>,
    principal: Option<Extension<Principal>>,
    Json(mut req): Json<ResourcePool>,
) -> Result<(StatusCode, Json<ResourcePool>), ApiError> {
    valid_id(&req.id)?;
    let existing = all(&state).await?;
    if existing.iter().any(|p| p.id == req.id) {
        return Err(ApiError::Conflict(format!(
            "pool {} already exists",
            req.id
        )));
    }
    req.vms = dedup(req.vms);
    req.containers = dedup(req.containers);
    req.storages = dedup(req.storages);
    check_members(&state, &principal, &req, &existing).await?;
    ConfigStore::new(&state.database)
        .put(KIND, &req.id, &req)
        .await?;
    Ok((StatusCode::CREATED, Json(req)))
}

async fn update(
    State(state): State<Arc<AppState>>,
    principal: Option<Extension<Principal>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateBody>,
) -> Result<Json<ResourcePool>, ApiError> {
    let existing = all(&state).await?;
    let mut pool = existing
        .iter()
        .find(|p| p.id == id)
        .cloned()
        .ok_or_else(|| ApiError::NotFound(format!("pool {id} not found")))?;
    let before = pool.clone();
    if let Some(c) = req.comment {
        pool.comment = c;
    }
    if let Some(v) = req.vms {
        pool.vms = dedup(v);
    }
    if let Some(v) = req.containers {
        pool.containers = dedup(v);
    }
    if let Some(v) = req.storages {
        pool.storages = dedup(v);
    }
    // only newly added members are checked (existing ones stay as they are)
    let added = ResourcePool {
        id: pool.id.clone(),
        comment: String::new(),
        vms: pool
            .vms
            .iter()
            .filter(|m| !before.vms.contains(m))
            .cloned()
            .collect(),
        containers: pool
            .containers
            .iter()
            .filter(|m| !before.containers.contains(m))
            .cloned()
            .collect(),
        storages: pool
            .storages
            .iter()
            .filter(|m| !before.storages.contains(m))
            .cloned()
            .collect(),
    };
    check_members(&state, &principal, &added, &existing).await?;
    ConfigStore::new(&state.database)
        .put(KIND, &pool.id, &pool)
        .await?;
    Ok(Json(pool))
}

async fn remove(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    if !ConfigStore::new(&state.database).delete(KIND, &id).await? {
        return Err(ApiError::NotFound(format!("pool {id} not found")));
    }
    // grants on the pool go with it
    sqlx::query("DELETE FROM auth_acl WHERE path = ?")
        .bind(format!("/pool/{id}"))
        .execute(state.database.pool())
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_ids() {
        assert!(valid_id("web-prod_1").is_ok());
        for bad in ["", "-x", "a/b", "a b", "../x", &"a".repeat(65)] {
            assert!(valid_id(bad).is_err(), "{bad}");
        }
    }
}
