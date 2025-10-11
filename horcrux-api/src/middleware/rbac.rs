///! RBAC (Role-Based Access Control) middleware
///!
///! Enforces permissions based on user roles and resource paths

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use horcrux_common::auth::Privilege;
use serde::Serialize;
use std::sync::Arc;

use crate::middleware::auth::AuthUser;

/// RBAC error response
#[derive(Serialize)]
pub struct RbacError {
    pub error: String,
    pub message: String,
    pub required_privilege: String,
}

impl IntoResponse for RbacError {
    fn into_response(self) -> Response {
        let json = Json(self);
        (StatusCode::FORBIDDEN, json).into_response()
    }
}

/// RBAC middleware - checks if user has required privilege for the resource
/// This is a simple check based on user role from the authenticated session
pub async fn rbac_middleware(
    State(state): State<Arc<crate::AppState>>,
    request: Request,
    next: Next,
) -> Result<Response, RbacError> {
    // For now, just verify the user is authenticated
    // Full RBAC will be enforced in individual handlers based on resource and action
    // Get authenticated user from request extensions (set by auth middleware)
    let _auth_user = request.extensions().get::<AuthUser>()
        .ok_or_else(|| RbacError {
            error: "unauthenticated".to_string(),
            message: "Authentication required before RBAC check".to_string(),
            required_privilege: "N/A".to_string(),
        })?;

    // For now, just pass through - full RBAC enforcement is in handlers
    // This middleware ensures authentication is present
    Ok(next.run(request).await)
}

/// Check if user has a specific privilege for a resource
/// This function is called from individual API handlers
pub async fn check_user_privilege(
    state: &Arc<crate::AppState>,
    auth_user: &AuthUser,
    resource_path: &str,
    required_privilege: Privilege,
) -> Result<bool, RbacError> {
    // Get user details from database
    let user = crate::db::users::get_user_by_username(state.database.pool(), &auth_user.username)
        .await
        .map_err(|_| RbacError {
            error: "user_not_found".to_string(),
            message: format!("User {} not found", auth_user.username),
            required_privilege: format!("{:?}", required_privilege),
        })?;

    // Load roles from database/config (for now, use in-memory default roles)
    let roles = get_default_roles();

    // Check permission using RBAC manager
    let rbac = crate::auth::rbac::RbacManager::new();
    let has_permission = rbac.check_permission(&user, &roles, resource_path, required_privilege)
        .await
        .map_err(|e| RbacError {
            error: "rbac_check_failed".to_string(),
            message: format!("RBAC check failed: {}", e),
            required_privilege: "N/A".to_string(),
        })?;

    Ok(has_permission)
}

/// Get default role definitions
/// In production, these should be loaded from database or config
fn get_default_roles() -> std::collections::HashMap<String, horcrux_common::auth::Role> {
    use horcrux_common::auth::{Permission, Privilege, Role};
    use std::collections::HashMap;

    let mut roles = HashMap::new();

    // Administrator role - full access
    roles.insert(
        "Administrator".to_string(),
        Role {
            name: "Administrator".to_string(),
            description: "Full system access".to_string(),
            permissions: vec![Permission {
                path: "/".to_string(),
                privileges: vec![
                    Privilege::VmAllocate,
                    Privilege::VmConfig,
                    Privilege::VmPowerMgmt,
                    Privilege::VmMigrate,
                    Privilege::VmSnapshot,
                    Privilege::VmBackup,
                    Privilege::VmAudit,
                    Privilege::DatastoreAllocate,
                    Privilege::DatastoreAudit,
                    Privilege::SysModify,
                    Privilege::SysAudit,
                    Privilege::UserModify,
                    Privilege::PoolAllocate,
                ],
            }],
        },
    );

    // VM Admin role - VM management only
    roles.insert(
        "VmAdmin".to_string(),
        Role {
            name: "VmAdmin".to_string(),
            description: "VM management access".to_string(),
            permissions: vec![Permission {
                path: "/api/vms/**".to_string(),
                privileges: vec![
                    Privilege::VmAllocate,
                    Privilege::VmConfig,
                    Privilege::VmPowerMgmt,
                    Privilege::VmSnapshot,
                    Privilege::VmBackup,
                    Privilege::VmAudit,
                ],
            }],
        },
    );

    // VM User role - basic VM operations
    roles.insert(
        "VmUser".to_string(),
        Role {
            name: "VmUser".to_string(),
            description: "Basic VM access (start/stop/view)".to_string(),
            permissions: vec![Permission {
                path: "/api/vms/**".to_string(),
                privileges: vec![
                    Privilege::VmPowerMgmt,
                    Privilege::VmAudit,
                ],
            }],
        },
    );

    // Storage Admin role - storage management
    roles.insert(
        "StorageAdmin".to_string(),
        Role {
            name: "StorageAdmin".to_string(),
            description: "Storage pool and datastore management".to_string(),
            permissions: vec![
                Permission {
                    path: "/api/storage/**".to_string(),
                    privileges: vec![
                        Privilege::DatastoreAllocate,
                        Privilege::DatastoreAudit,
                        Privilege::PoolAllocate,
                    ],
                },
            ],
        },
    );

    // Auditor role - read-only access
    roles.insert(
        "Auditor".to_string(),
        Role {
            name: "Auditor".to_string(),
            description: "Read-only system access".to_string(),
            permissions: vec![Permission {
                path: "/api/**".to_string(),
                privileges: vec![
                    Privilege::VmAudit,
                    Privilege::DatastoreAudit,
                    Privilege::SysAudit,
                ],
            }],
        },
    );

    roles
}

/// Helper macro to check RBAC in handlers
/// Usage: require_privilege!(state, auth_user, "/api/vms/100", Privilege::VmPowerMgmt)?;
#[macro_export]
macro_rules! require_privilege {
    ($state:expr, $auth_user:expr, $resource:expr, $privilege:expr) => {
        if !$crate::middleware::rbac::check_user_privilege(&$state, &$auth_user, $resource, $privilege)
            .await
            .map_err(|_| $crate::ApiError::Forbidden("Insufficient permissions".to_string()))?
        {
            return Err($crate::ApiError::Forbidden(format!(
                "User does not have privilege '{:?}' for resource '{}'",
                $privilege, $resource
            )));
        }
    };
}
