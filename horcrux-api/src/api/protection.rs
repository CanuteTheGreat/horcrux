//! Protection flag (Proxmox "Protection"): a protected VM or container can't
//! be deleted until the flag is cleared, so a mistyped id or a stray script
//! can't destroy a guest that matters.
//!
//! - `GET|PUT /api/vms/:id/protection`, `GET|PUT /api/containers/:id/protection`
//!   body `{"protected": true}`
//!
//! Enforced by `ensure_unprotected`, called from the delete handlers.
use crate::db::config_store::ConfigStore;
use crate::error::ApiError;
use crate::AppState;
use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const KIND: &str = "guest_protection";

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/vms/:id/protection",
            get(get_vm_protection).put(put_vm_protection),
        )
        .route(
            "/api/containers/:id/protection",
            get(get_ct_protection).put(put_ct_protection),
        )
}

#[derive(Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct Protection {
    pub protected: bool,
}

pub fn vm_key(id: &str) -> String {
    format!("vm:{id}")
}

pub fn ct_key(id: &str) -> String {
    format!("ct:{id}")
}

/// The refusal message for a protected guest.
pub fn refusal(kind: &str, id: &str) -> String {
    format!(
        "{kind} {id} is protected: clear its protection flag (PUT /api/{}s/{id}/protection {{\"protected\": false}}) before deleting it",
        if kind == "VM" { "vm" } else { "container" }
    )
}

async fn is_protected(state: &AppState, key: &str) -> bool {
    ConfigStore::new(&state.database)
        .load::<Protection>(KIND)
        .await
        .unwrap_or_default()
        .into_iter()
        .any(|(k, p)| k == key && p.protected)
}

/// Err(409) when the VM is protected.
pub async fn ensure_vm_unprotected(state: &AppState, id: &str) -> Result<(), ApiError> {
    if is_protected(state, &vm_key(id)).await {
        return Err(ApiError::Conflict(refusal("VM", id)));
    }
    Ok(())
}

/// Err(409) when the container is protected.
pub async fn ensure_ct_unprotected(state: &AppState, id: &str) -> Result<(), ApiError> {
    if is_protected(state, &ct_key(id)).await {
        return Err(ApiError::Conflict(refusal("container", id)));
    }
    Ok(())
}

/// Drop the flag when the guest is gone, so a new guest with the same id starts unprotected.
pub async fn forget(state: &AppState, key: &str) {
    let _ = ConfigStore::new(&state.database).delete(KIND, key).await;
}

async fn set(state: &AppState, key: String, on: bool) -> Result<Json<Protection>, ApiError> {
    let store = ConfigStore::new(&state.database);
    if on {
        store
            .put(KIND, &key, &Protection { protected: true })
            .await?;
    } else {
        store.delete(KIND, &key).await?;
    }
    Ok(Json(Protection { protected: on }))
}

async fn get_vm_protection(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Protection>, ApiError> {
    state
        .vm_manager
        .get_vm(&id)
        .await
        .map_err(|_| ApiError::NotFound(format!("VM {id} not found")))?;
    Ok(Json(Protection {
        protected: is_protected(&state, &vm_key(&id)).await,
    }))
}

async fn put_vm_protection(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(b): Json<Protection>,
) -> Result<Json<Protection>, ApiError> {
    state
        .vm_manager
        .get_vm(&id)
        .await
        .map_err(|_| ApiError::NotFound(format!("VM {id} not found")))?;
    set(&state, vm_key(&id), b.protected).await
}

async fn get_ct_protection(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Protection>, ApiError> {
    state
        .container_manager
        .get_container(&id)
        .await
        .map_err(|_| ApiError::NotFound(format!("container {id} not found")))?;
    Ok(Json(Protection {
        protected: is_protected(&state, &ct_key(&id)).await,
    }))
}

async fn put_ct_protection(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(b): Json<Protection>,
) -> Result<Json<Protection>, ApiError> {
    state
        .container_manager
        .get_container(&id)
        .await
        .map_err(|_| ApiError::NotFound(format!("container {id} not found")))?;
    set(&state, ct_key(&id), b.protected).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_per_kind() {
        assert_eq!(vm_key("100"), "vm:100");
        assert_eq!(ct_key("100"), "ct:100");
    }

    #[test]
    fn refusal_names_the_way_out() {
        let m = refusal("VM", "7");
        assert!(m.contains("protected") && m.contains("/api/vms/7/protection"));
        assert!(refusal("container", "c1").contains("/api/containers/c1/protection"));
    }
}
