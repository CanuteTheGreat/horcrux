//! RBAC (Role-Based Access Control) middleware
//!
//! Runs after `auth_middleware` on every protected route. Looks up what the
//! route needs (`auth::privileges::requirement`) and checks it against the
//! caller's ACL-derived privileges (`auth::store::Principal`). Collection
//! reads a user is only partly allowed to see (e.g. `GET /api/vms` for
//! someone with rights on one VM) are filtered item by item.
use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use horcrux_common::auth::Privilege;
use serde::Serialize;
use std::sync::Arc;

use crate::auth::privileges::{requirement, Requirement};
use crate::auth::store::Principal;
use crate::middleware::auth::{AuthMethod, AuthUser, SCOPE_TFA_SETUP};

/// RBAC error response
#[derive(Serialize)]
pub struct RbacError {
    pub error: String,
    pub message: String,
    pub required_privilege: String,
    pub path: String,
}

impl IntoResponse for RbacError {
    fn into_response(self) -> Response {
        let status = if self.error == "unauthenticated" {
            StatusCode::UNAUTHORIZED
        } else {
            StatusCode::FORBIDDEN
        };
        (status, Json(self)).into_response()
    }
}

fn forbidden(privilege: Privilege, path: &str) -> RbacError {
    RbacError {
        error: "forbidden".into(),
        message: format!("Permission check failed: {} on {}", privilege.name(), path),
        required_privilege: privilege.name().into(),
        path: path.into(),
    }
}

/// Endpoints usable with the restricted second-factor-setup login.
pub fn allowed_in_tfa_setup(path: &str) -> bool {
    path.starts_with("/api/auth/mfa/")
        || matches!(
            path,
            "/api/auth/logout" | "/api/auth/verify" | "/api/auth/me" | "/api/auth/mfa"
        )
}

/// RBAC middleware - enforces the route's requirement for the caller.
pub async fn rbac_middleware<S: super::AuthState>(
    State(state): State<Arc<S>>,
    mut request: Request,
    next: Next,
) -> Result<Response, RbacError> {
    let Some(user) = request.extensions().get::<AuthUser>().cloned() else {
        return Err(RbacError {
            error: "unauthenticated".into(),
            message: "Authentication required".into(),
            required_privilege: String::new(),
            path: String::new(),
        });
    };
    // Cluster service tokens were already limited to their one route.
    if user.method == AuthMethod::ClusterNode {
        return Ok(next.run(request).await);
    }
    let path = request.uri().path().to_string();
    if user.scope.as_deref() == Some(SCOPE_TFA_SETUP) && !allowed_in_tfa_setup(&path) {
        return Err(RbacError {
            error: "tfa_setup_required".into(),
            message: "A second factor is required for this account: set one up first".into(),
            required_privilege: String::new(),
            path,
        });
    }
    let req = requirement(request.method().as_str(), &path);
    let pool = state.database().pool();
    let principal = Principal::load(pool, &user.user_id)
        .await
        .map_err(|e| RbacError {
            error: "rbac_check_failed".into(),
            message: e.to_string(),
            required_privilege: String::new(),
            path: path.clone(),
        })?;

    let mut filter: Option<(Privilege, String)> = None;
    match &req {
        Requirement::Public | Requirement::Authenticated => {}
        Requirement::Privilege {
            privilege,
            path: acl_path,
        } => {
            if !principal.has(*privilege, acl_path) {
                return Err(forbidden(*privilege, acl_path));
            }
        }
        Requirement::AnyBelow { privilege, prefix } => {
            if !principal.has(*privilege, prefix) {
                if !principal.has_below(*privilege, prefix) {
                    return Err(forbidden(*privilege, prefix));
                }
                filter = Some((*privilege, prefix.clone()));
            }
        }
        Requirement::SelfOr {
            user: target,
            privilege,
            path: acl_path,
        } => {
            let own = *target == user.user_id || *target == user.username;
            if !own && !principal.has(*privilege, acl_path) {
                return Err(forbidden(*privilege, acl_path));
            }
        }
    }

    request.extensions_mut().insert(principal.clone());
    let response = next.run(request).await;

    match filter {
        Some((privilege, prefix)) if response.status().is_success() => {
            Ok(filter_collection(response, &principal, privilege, &prefix).await)
        }
        _ => Ok(response),
    }
}

/// Keep only the items of a JSON array response the user may see.
async fn filter_collection(
    response: Response,
    principal: &Principal,
    privilege: Privilege,
    prefix: &str,
) -> Response {
    let (mut parts, body) = response.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, 256 * 1024 * 1024).await else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "response too large to filter",
        )
            .into_response();
    };
    let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        // not JSON: nothing item-shaped to filter, and the user holds the
        // privilege somewhere below - but don't leak it either
        return (StatusCode::FORBIDDEN, "forbidden").into_response();
    };
    retain_visible(&mut value, principal, privilege, prefix);
    let out = serde_json::to_vec(&value).unwrap_or_default();
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, Body::from(out))
}

/// Filter `[ {id|name|vm_id|container_id: ...}, ... ]` (or `{ "items": [...] }`).
pub fn retain_visible(
    value: &mut serde_json::Value,
    principal: &Principal,
    privilege: Privilege,
    prefix: &str,
) {
    let keep = |item: &serde_json::Value| -> bool {
        let id = ["id", "vm_id", "container_id", "name"]
            .iter()
            .find_map(|k| item.get(*k).and_then(|v| v.as_str()));
        match id {
            Some(id) => principal.has(privilege, &format!("{}/{}", prefix, id)),
            None => false,
        }
    };
    match value {
        serde_json::Value::Array(items) => items.retain(keep),
        serde_json::Value::Object(map) => {
            for v in map.values_mut() {
                if let serde_json::Value::Array(items) = v {
                    items.retain(keep);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::acl::AclEntry;
    use std::collections::HashMap;

    #[test]
    fn tfa_setup_scope_is_limited() {
        assert!(allowed_in_tfa_setup("/api/auth/mfa/totp/setup"));
        assert!(allowed_in_tfa_setup("/api/auth/logout"));
        assert!(!allowed_in_tfa_setup("/api/vms"));
        assert!(!allowed_in_tfa_setup("/api/users"));
    }

    #[test]
    fn collection_filtering_by_id() {
        let mut roles = HashMap::new();
        roles.insert("VMUser".to_string(), vec![Privilege::VmAudit]);
        let p = Principal {
            user_id: "u".into(),
            groups: vec![],
            acl: vec![AclEntry {
                id: 1,
                path: "/vms/b".into(),
                subject_type: "user".into(),
                subject: "u".into(),
                role: "VMUser".into(),
                propagate: true,
            }],
            roles,
        };
        let mut v = serde_json::json!([{"id": "a"}, {"id": "b"}, {"name": "x"}, {"nope": 1}]);
        retain_visible(&mut v, &p, Privilege::VmAudit, "/vms");
        assert_eq!(v, serde_json::json!([{"id": "b"}]));
    }
}
