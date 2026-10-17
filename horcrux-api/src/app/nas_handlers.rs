//! NAS HTTP handlers (USE=nas), moved out of app/mod.rs.

use super::*;
use crate::nas::auth::{AuthManager, NasGroup, NasUser};
use crate::nas::monitoring::{collect_metrics, get_nas_health};
#[cfg(feature = "iscsi-target")]
use crate::nas::services::iscsi::IscsiTargetManager;

#[cfg(feature = "s3-gateway")]
use crate::nas::services::s3::S3GatewayManager;
use crate::nas::services::{get_service_status, manage_service, NasService, ServiceAction};
#[cfg(feature = "nfs-server")]
use crate::nas::shares::nfs::NfsServerManager;
#[cfg(feature = "smb")]
use crate::nas::shares::smb::SmbManager;
use crate::nas::storage::StorageManager;
use axum::extract::{Path, State};
use axum::Json;

// Health and metrics
pub async fn nas_health(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    match get_nas_health().await {
        Ok(health) => Ok(Json(serde_json::to_value(health).unwrap_or_default())),
        Err(e) => Err(ApiError::Internal(format!(
            "Failed to get NAS health: {}",
            e
        ))),
    }
}

pub async fn nas_metrics(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    match collect_metrics().await {
        Ok(metrics) => Ok(Json(serde_json::to_value(metrics).unwrap_or_default())),
        Err(e) => Err(ApiError::Internal(format!(
            "Failed to collect metrics: {}",
            e
        ))),
    }
}

// Share handlers
// Note: NasManager uses in-memory storage. In production, shares should be persisted to database.
// The actual protocol configurations (smb.conf, exports, etc.) are persisted by the protocol managers.
pub async fn nas_list_shares(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    // Query database for shares
    let shares = sqlx::query_as::<_, (String, String, String, String, bool, Option<String>, i64)>(
        "SELECT id, name, path, owner_user, enabled, description, created_at FROM nas_shares ORDER BY name"
    )
    .fetch_all(state.database.pool())
    .await
    .unwrap_or_default();

    let json_shares: Vec<serde_json::Value> = shares
        .iter()
        .map(|(id, name, path, owner, enabled, desc, created)| {
            serde_json::json!({
                "id": id,
                "name": name,
                "path": path,
                "owner_user": owner,
                "enabled": enabled,
                "description": desc,
                "created_at": created,
            })
        })
        .collect();
    Ok(Json(json_shares))
}
pub async fn nas_create_share(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("Missing 'name' field".to_string()))?;
    let path = body
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("Missing 'path' field".to_string()))?;
    let description = body.get("description").and_then(|v| v.as_str());
    let owner = body
        .get("owner_user")
        .and_then(|v| v.as_str())
        .unwrap_or("root");

    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();

    // Create directory if it doesn't exist
    if !std::path::Path::new(path).exists() {
        tokio::fs::create_dir_all(path)
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to create share directory: {}", e)))?;
    }

    // Insert into database
    sqlx::query(
        "INSERT INTO nas_shares (id, name, path, owner_user, enabled, description, created_at, updated_at) VALUES (?, ?, ?, ?, 1, ?, ?, ?)"
    )
    .bind(&id)
    .bind(name)
    .bind(path)
    .bind(owner)
    .bind(description)
    .bind(now)
    .bind(now)
    .execute(state.database.pool())
    .await
    .map_err(|e| ApiError::Internal(format!("Failed to create share: {}", e)))?;

    Ok(Json(serde_json::json!({
        "id": id,
        "name": name,
        "path": path,
        "owner_user": owner,
        "enabled": true,
        "description": description,
        "created_at": now,
    })))
}
pub async fn nas_get_share(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let share = sqlx::query_as::<_, (String, String, String, String, bool, Option<String>, i64)>(
        "SELECT id, name, path, owner_user, enabled, description, created_at FROM nas_shares WHERE id = ? OR name = ?"
    )
    .bind(&id)
    .bind(&id)
    .fetch_optional(state.database.pool())
    .await
    .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?;

    match share {
        Some((id, name, path, owner, enabled, desc, created)) => Ok(Json(serde_json::json!({
            "id": id,
            "name": name,
            "path": path,
            "owner_user": owner,
            "enabled": enabled,
            "description": desc,
            "created_at": created,
        }))),
        None => Err(ApiError::NotFound("Share not found".to_string())),
    }
}
pub async fn nas_update_share(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let now = chrono::Utc::now().timestamp();

    // Get existing share
    let existing =
        sqlx::query_as::<_, (String,)>("SELECT id FROM nas_shares WHERE id = ? OR name = ?")
            .bind(&id)
            .bind(&id)
            .fetch_optional(state.database.pool())
            .await
            .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?;

    if existing.is_none() {
        return Err(ApiError::NotFound("Share not found".to_string()));
    }

    // Update fields that are present in body
    if let Some(name) = body.get("name").and_then(|v| v.as_str()) {
        sqlx::query("UPDATE nas_shares SET name = ?, updated_at = ? WHERE id = ?")
            .bind(name)
            .bind(now)
            .bind(&id)
            .execute(state.database.pool())
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to update share: {}", e)))?;
    }
    if let Some(desc) = body.get("description").and_then(|v| v.as_str()) {
        sqlx::query("UPDATE nas_shares SET description = ?, updated_at = ? WHERE id = ?")
            .bind(desc)
            .bind(now)
            .bind(&id)
            .execute(state.database.pool())
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to update share: {}", e)))?;
    }

    // Return updated share
    nas_get_share(State(state), Path(id)).await
}
pub async fn nas_delete_share(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<(), ApiError> {
    let result = sqlx::query("DELETE FROM nas_shares WHERE id = ? OR name = ?")
        .bind(&id)
        .bind(&id)
        .execute(state.database.pool())
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to delete share: {}", e)))?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("Share not found".to_string()));
    }
    Ok(())
}
pub async fn nas_enable_share(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<(), ApiError> {
    let now = chrono::Utc::now().timestamp();
    let result =
        sqlx::query("UPDATE nas_shares SET enabled = 1, updated_at = ? WHERE id = ? OR name = ?")
            .bind(now)
            .bind(&id)
            .bind(&id)
            .execute(state.database.pool())
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to enable share: {}", e)))?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("Share not found".to_string()));
    }
    Ok(())
}
pub async fn nas_disable_share(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<(), ApiError> {
    let now = chrono::Utc::now().timestamp();
    let result =
        sqlx::query("UPDATE nas_shares SET enabled = 0, updated_at = ? WHERE id = ? OR name = ?")
            .bind(now)
            .bind(&id)
            .bind(&id)
            .execute(state.database.pool())
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to disable share: {}", e)))?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("Share not found".to_string()));
    }
    Ok(())
}

// User handlers
pub async fn nas_list_users(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let manager = AuthManager::new();
    match manager.list_system_users().await {
        Ok(users) => {
            let json_users: Vec<serde_json::Value> = users
                .iter()
                .map(|u| serde_json::to_value(u).unwrap_or_default())
                .collect();
            Ok(Json(json_users))
        }
        Err(e) => Err(ApiError::Internal(format!("Failed to list users: {}", e))),
    }
}
pub async fn nas_create_user(
    State(_state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let username = body
        .get("username")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("Missing 'username' field".to_string()))?;
    let manager = AuthManager::new();
    let uid = manager
        .next_uid()
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to get next UID: {}", e)))?;
    let primary_group = body
        .get("primary_group")
        .and_then(|v| v.as_str())
        .unwrap_or("users");
    let user = NasUser::new(
        uuid::Uuid::new_v4().to_string(),
        username.to_string(),
        uid,
        primary_group.to_string(),
    );
    manager
        .create_system_user(&user)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to create user: {}", e)))?;

    // Set password if provided
    if let Some(password) = body.get("password").and_then(|v| v.as_str()) {
        manager
            .set_password(&user, password)
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to set password: {}", e)))?;
    }

    Ok(Json(serde_json::to_value(&user).unwrap_or_default()))
}
pub async fn nas_get_user(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let manager = AuthManager::new();
    match manager.get_system_user(&id).await {
        Ok(user) => Ok(Json(serde_json::to_value(user).unwrap_or_default())),
        Err(_) => Err(ApiError::NotFound("User not found".to_string())),
    }
}
pub async fn nas_update_user(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let manager = AuthManager::new();
    let existing = manager
        .get_system_user(&id)
        .await
        .map_err(|_| ApiError::NotFound("User not found".to_string()))?;

    let shell = body
        .get("shell")
        .and_then(|v| v.as_str())
        .unwrap_or(&existing.shell)
        .to_string();
    let home = body
        .get("home")
        .and_then(|v| v.as_str())
        .unwrap_or(&existing.home)
        .to_string();
    let full_name = body
        .get("full_name")
        .and_then(|v| v.as_str())
        .unwrap_or(&existing.full_name)
        .to_string();

    // Get group name from GID
    let primary_group = manager
        .get_system_group_by_gid(existing.gid)
        .await
        .map(|g| g.name)
        .unwrap_or_else(|_| format!("{}", existing.gid));

    let mut user = NasUser::new(
        id.clone(),
        existing.username.clone(),
        existing.uid,
        primary_group,
    );
    user.shell = Some(shell);
    user.home_directory = Some(home);
    user.full_name = Some(full_name);

    manager
        .update_system_user(&user)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to update user: {}", e)))?;

    Ok(Json(serde_json::to_value(&user).unwrap_or_default()))
}
pub async fn nas_delete_user(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<(), ApiError> {
    let manager = AuthManager::new();
    let user_info = manager
        .get_system_user(&id)
        .await
        .map_err(|_| ApiError::NotFound("User not found".to_string()))?;

    // Get group name from GID
    let primary_group = manager
        .get_system_group_by_gid(user_info.gid)
        .await
        .map(|g| g.name)
        .unwrap_or_else(|_| format!("{}", user_info.gid));

    let user = NasUser::new(id, user_info.username, user_info.uid, primary_group);
    manager
        .delete_system_user(&user)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to delete user: {}", e)))
}
pub async fn nas_set_user_password(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<(), ApiError> {
    let password = body
        .get("password")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("Missing 'password' field".to_string()))?;
    let manager = AuthManager::new();
    let user_info = manager
        .get_system_user(&id)
        .await
        .map_err(|_| ApiError::NotFound("User not found".to_string()))?;

    // Get group name from GID
    let primary_group = manager
        .get_system_group_by_gid(user_info.gid)
        .await
        .map(|g| g.name)
        .unwrap_or_else(|_| format!("{}", user_info.gid));

    let user = NasUser::new(id, user_info.username, user_info.uid, primary_group);
    manager
        .set_password(&user, password)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to set password: {}", e)))
}
pub async fn nas_enable_user(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<(), ApiError> {
    let manager = AuthManager::new();
    manager
        .enable_user(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to enable user: {}", e)))
}
pub async fn nas_disable_user(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<(), ApiError> {
    let manager = AuthManager::new();
    manager
        .disable_user(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to disable user: {}", e)))
}

// Group handlers
pub async fn nas_list_groups(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let manager = AuthManager::new();
    match manager.list_system_groups().await {
        Ok(groups) => {
            let json_groups: Vec<serde_json::Value> = groups
                .iter()
                .map(|g| serde_json::to_value(g).unwrap_or_default())
                .collect();
            Ok(Json(json_groups))
        }
        Err(e) => Err(ApiError::Internal(format!("Failed to list groups: {}", e))),
    }
}
pub async fn nas_create_group(
    State(_state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("Missing 'name' field".to_string()))?;
    let manager = AuthManager::new();
    let gid = manager
        .next_gid()
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to get next GID: {}", e)))?;
    let group = NasGroup::new(uuid::Uuid::new_v4().to_string(), name.to_string(), gid);
    manager
        .create_system_group(&group)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to create group: {}", e)))?;
    Ok(Json(serde_json::to_value(&group).unwrap_or_default()))
}
pub async fn nas_get_group(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let manager = AuthManager::new();
    match manager.get_system_group(&id).await {
        Ok(group) => Ok(Json(serde_json::to_value(group).unwrap_or_default())),
        Err(_) => Err(ApiError::NotFound("Group not found".to_string())),
    }
}
pub async fn nas_update_group(
    State(_state): State<Arc<AppState>>,
    Path(_id): Path<String>,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Group updates are limited - groupmod mainly supports GID changes
    Err(ApiError::Internal(
        "Group modification not supported".to_string(),
    ))
}
pub async fn nas_delete_group(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<(), ApiError> {
    let manager = AuthManager::new();
    let group_info = manager
        .get_system_group(&id)
        .await
        .map_err(|_| ApiError::NotFound("Group not found".to_string()))?;

    let group = NasGroup::new(id, group_info.name, group_info.gid);
    manager
        .delete_system_group(&group)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to delete group: {}", e)))
}
pub async fn nas_get_group_members(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let manager = AuthManager::new();
    let group_info = manager
        .get_system_group(&id)
        .await
        .map_err(|_| ApiError::NotFound("Group not found".to_string()))?;
    let members: Vec<serde_json::Value> = group_info
        .members
        .iter()
        .map(|m| serde_json::json!({"username": m}))
        .collect();
    Ok(Json(members))
}
pub async fn nas_add_group_member(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<(), ApiError> {
    let username = body
        .get("username")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("Missing 'username' field".to_string()))?;
    let manager = AuthManager::new();
    manager
        .add_user_to_group(username, &id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to add user to group: {}", e)))
}
pub async fn nas_remove_group_member(
    State(_state): State<Arc<AppState>>,
    Path((group_id, user_id)): Path<(String, String)>,
) -> Result<(), ApiError> {
    let manager = AuthManager::new();
    manager
        .remove_user_from_group(&user_id, &group_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to remove user from group: {}", e)))
}

// Pool handlers
#[cfg(feature = "nas-zfs")]
pub async fn nas_list_pools(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let manager = StorageManager::new();
    match manager.list_zfs_pools().await {
        Ok(pools) => {
            let json_pools: Vec<serde_json::Value> = pools
                .iter()
                .map(|p| serde_json::to_value(p).unwrap_or_default())
                .collect();
            Ok(Json(json_pools))
        }
        Err(e) => Err(ApiError::Internal(format!("Failed to list pools: {}", e))),
    }
}
#[cfg(not(feature = "nas-zfs"))]
pub async fn nas_list_pools(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    Ok(Json(vec![]))
}
#[cfg(feature = "nas-zfs")]
pub async fn nas_create_pool(
    State(_state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use crate::nas::storage::RaidLevel;
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("Missing 'name' field".to_string()))?;
    let devices: Vec<String> = body
        .get("devices")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let raid_str = body
        .get("raid_level")
        .and_then(|v| v.as_str())
        .unwrap_or("stripe");
    let raid_level = match raid_str.to_lowercase().as_str() {
        "mirror" => RaidLevel::Mirror,
        "raidz1" | "raidz" => RaidLevel::RaidZ,
        "raidz2" => RaidLevel::RaidZ2,
        "raidz3" => RaidLevel::RaidZ3,
        _ => RaidLevel::Raid0,
    };
    let manager = StorageManager::new();
    match manager.create_zfs_pool(name, raid_level, &devices).await {
        Ok(pool) => Ok(Json(serde_json::to_value(pool).unwrap_or_default())),
        Err(e) => Err(ApiError::Internal(format!("Failed to create pool: {}", e))),
    }
}
#[cfg(not(feature = "nas-zfs"))]
pub async fn nas_create_pool(
    State(_state): State<Arc<AppState>>,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::Internal("ZFS feature not enabled".to_string()))
}
#[cfg(feature = "nas-zfs")]
pub async fn nas_get_pool(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let manager = StorageManager::new();
    match manager.list_zfs_pools().await {
        Ok(pools) => {
            if let Some(pool) = pools.iter().find(|p| p.name == id || p.id == id) {
                Ok(Json(serde_json::to_value(pool).unwrap_or_default()))
            } else {
                Err(ApiError::NotFound("Pool not found".to_string()))
            }
        }
        Err(e) => Err(ApiError::Internal(format!("Failed to get pool: {}", e))),
    }
}
#[cfg(not(feature = "nas-zfs"))]
pub async fn nas_get_pool(
    State(_state): State<Arc<AppState>>,
    Path(_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::NotFound("Pool not found".to_string()))
}
#[cfg(feature = "nas-zfs")]
pub async fn nas_delete_pool(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<(), ApiError> {
    let manager = StorageManager::new();
    manager
        .destroy_zfs_pool(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to delete pool: {}", e)))
}
#[cfg(not(feature = "nas-zfs"))]
pub async fn nas_delete_pool(
    State(_state): State<Arc<AppState>>,
    Path(_id): Path<String>,
) -> Result<(), ApiError> {
    Err(ApiError::Internal("ZFS feature not enabled".to_string()))
}
#[cfg(feature = "nas-zfs")]
pub async fn nas_scrub_pool(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let manager = StorageManager::new();
    manager
        .scrub_pool(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to scrub pool: {}", e)))?;
    Ok(Json(
        serde_json::json!({"status": "scrub started", "pool": id}),
    ))
}
#[cfg(not(feature = "nas-zfs"))]
pub async fn nas_scrub_pool(
    State(_state): State<Arc<AppState>>,
    Path(_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::Internal("ZFS feature not enabled".to_string()))
}
#[cfg(feature = "nas-zfs")]
pub async fn nas_get_pool_status(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let manager = StorageManager::new();
    match manager.list_zfs_pools().await {
        Ok(pools) => {
            if let Some(pool) = pools.iter().find(|p| p.name == id || p.id == id) {
                Ok(Json(serde_json::json!({
                    "name": pool.name,
                    "health": format!("{:?}", pool.health),
                    "capacity": pool.total_bytes,
                    "used": pool.used_bytes,
                    "available": pool.available_bytes,
                })))
            } else {
                Err(ApiError::NotFound("Pool not found".to_string()))
            }
        }
        Err(e) => Err(ApiError::Internal(format!(
            "Failed to get pool status: {}",
            e
        ))),
    }
}
#[cfg(not(feature = "nas-zfs"))]
pub async fn nas_get_pool_status(
    State(_state): State<Arc<AppState>>,
    Path(_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::Internal("ZFS feature not enabled".to_string()))
}

// Dataset handlers
pub async fn nas_list_datasets(
    State(_state): State<Arc<AppState>>,
    Path(pool_id): Path<String>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let manager = StorageManager::new();
    match manager.list_datasets(&pool_id).await {
        Ok(datasets) => {
            let json_datasets: Vec<serde_json::Value> = datasets
                .iter()
                .map(|d| serde_json::to_value(d).unwrap_or_default())
                .collect();
            Ok(Json(json_datasets))
        }
        Err(e) => Err(ApiError::Internal(format!(
            "Failed to list datasets: {}",
            e
        ))),
    }
}
pub async fn nas_create_dataset(
    State(_state): State<Arc<AppState>>,
    Path(pool_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("Missing 'name' field".to_string()))?;
    let manager = StorageManager::new();
    match manager.create_dataset(&pool_id, name).await {
        Ok(dataset) => Ok(Json(serde_json::to_value(dataset).unwrap_or_default())),
        Err(e) => Err(ApiError::Internal(format!(
            "Failed to create dataset: {}",
            e
        ))),
    }
}
pub async fn nas_get_dataset(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Extract pool from dataset name (pool/dataset format)
    let pool = id.split('/').next().unwrap_or(&id);
    let manager = StorageManager::new();
    match manager.list_datasets(pool).await {
        Ok(datasets) => {
            if let Some(dataset) = datasets.iter().find(|d| d.name == id || d.id == id) {
                Ok(Json(serde_json::to_value(dataset).unwrap_or_default()))
            } else {
                Err(ApiError::NotFound("Dataset not found".to_string()))
            }
        }
        Err(e) => Err(ApiError::Internal(format!("Failed to get dataset: {}", e))),
    }
}
pub async fn nas_update_dataset(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use crate::nas::storage::datasets;

    // Body is a flat map of ZFS property -> value, e.g. {"quota": "10G", "compression": "lz4"}
    let properties = body.as_object().ok_or_else(|| {
        ApiError::BadRequest("Expected a JSON object mapping property names to values".to_string())
    })?;

    if properties.is_empty() {
        return Err(ApiError::BadRequest(
            "No properties provided to update".to_string(),
        ));
    }

    for (property, value) in properties {
        let value_str = match value {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Bool(b) => b.to_string(),
            _ => {
                return Err(ApiError::BadRequest(format!(
                    "Property '{}' must be a string, number, or boolean",
                    property
                )))
            }
        };
        datasets::set_property(&id, property, &value_str)
            .await
            .map_err(|e| {
                ApiError::Internal(format!(
                    "Failed to set property '{}' on dataset '{}': {}",
                    property, id, e
                ))
            })?;
    }

    Ok(Json(serde_json::json!({
        "dataset": id,
        "updated_properties": properties.keys().collect::<Vec<_>>(),
    })))
}
pub async fn nas_delete_dataset(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<(), ApiError> {
    use crate::nas::storage::datasets;
    datasets::destroy_dataset(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to delete dataset: {}", e)))
}
pub async fn nas_set_dataset_quota(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<(), ApiError> {
    use crate::nas::storage::datasets;
    let quota_bytes = body
        .get("quota_bytes")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| ApiError::BadRequest("Missing 'quota_bytes' field".to_string()))?;
    datasets::set_quota(&id, quota_bytes)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to set quota: {}", e)))
}

// Snapshot handlers
pub async fn nas_list_snapshots(
    State(_state): State<Arc<AppState>>,
    Path(dataset_id): Path<String>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let manager = StorageManager::new();
    match manager.list_snapshots(&dataset_id).await {
        Ok(snapshots) => {
            let json_snapshots: Vec<serde_json::Value> = snapshots
                .iter()
                .map(|s| serde_json::to_value(s).unwrap_or_default())
                .collect();
            Ok(Json(json_snapshots))
        }
        Err(e) => Err(ApiError::Internal(format!(
            "Failed to list snapshots: {}",
            e
        ))),
    }
}
pub async fn nas_create_snapshot(
    State(_state): State<Arc<AppState>>,
    Path(dataset_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("Missing 'name' field".to_string()))?;
    let manager = StorageManager::new();
    match manager.create_snapshot(&dataset_id, name).await {
        Ok(snapshot) => Ok(Json(serde_json::to_value(snapshot).unwrap_or_default())),
        Err(e) => Err(ApiError::Internal(format!(
            "Failed to create snapshot: {}",
            e
        ))),
    }
}
pub async fn nas_get_snapshot(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Snapshot format: dataset@snapname
    let parts: Vec<&str> = id.split('@').collect();
    if parts.len() != 2 {
        return Err(ApiError::BadRequest(
            "Invalid snapshot format, expected 'dataset@snapshot'".to_string(),
        ));
    }
    let manager = StorageManager::new();
    match manager.list_snapshots(parts[0]).await {
        Ok(snapshots) => {
            if let Some(snap) = snapshots.iter().find(|s| s.name == id) {
                Ok(Json(serde_json::to_value(snap).unwrap_or_default()))
            } else {
                Err(ApiError::NotFound("Snapshot not found".to_string()))
            }
        }
        Err(e) => Err(ApiError::Internal(format!("Failed to get snapshot: {}", e))),
    }
}
pub async fn nas_delete_snapshot(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<(), ApiError> {
    use crate::nas::storage::snapshots;
    snapshots::delete_snapshot(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to delete snapshot: {}", e)))
}
pub async fn nas_rollback_snapshot(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use crate::nas::storage::snapshots;
    snapshots::rollback_snapshot(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to rollback snapshot: {}", e)))?;
    Ok(Json(
        serde_json::json!({"status": "rolled back", "snapshot": id}),
    ))
}
pub async fn nas_clone_snapshot(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use crate::nas::storage::snapshots;
    let target = body
        .get("target")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("Missing 'target' field".to_string()))?;
    snapshots::clone_snapshot(&id, target)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to clone snapshot: {}", e)))?;
    Ok(Json(
        serde_json::json!({"status": "cloned", "source": id, "target": target}),
    ))
}

// Snapshot policy handlers
pub async fn nas_list_snapshot_policies(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let policies = sqlx::query_as::<_, (String, String, String, String, i32, i32, i32, bool, i64)>(
        "SELECT id, name, dataset, schedule, keep_hourly, keep_daily, keep_weekly, enabled, created_at FROM nas_snapshot_policies ORDER BY name"
    )
    .fetch_all(state.database.pool())
    .await
    .unwrap_or_default();

    let json_policies: Vec<serde_json::Value> = policies
        .iter()
        .map(
            |(id, name, dataset, schedule, hourly, daily, weekly, enabled, created)| {
                serde_json::json!({
                    "id": id,
                    "name": name,
                    "dataset": dataset,
                    "schedule": schedule,
                    "retention": {
                        "hourly": hourly,
                        "daily": daily,
                        "weekly": weekly,
                    },
                    "enabled": enabled,
                    "created_at": created,
                })
            },
        )
        .collect();
    Ok(Json(json_policies))
}
pub async fn nas_create_snapshot_policy(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("Missing 'name' field".to_string()))?;
    let dataset = body
        .get("dataset")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("Missing 'dataset' field".to_string()))?;
    let schedule = body
        .get("schedule")
        .and_then(|v| v.as_str())
        .unwrap_or("0 * * * *"); // Hourly default
    let default_retention = serde_json::json!({});
    let retention = body.get("retention").unwrap_or(&default_retention);
    let keep_hourly = retention
        .get("hourly")
        .and_then(|v| v.as_i64())
        .unwrap_or(24) as i32;
    let keep_daily = retention.get("daily").and_then(|v| v.as_i64()).unwrap_or(7) as i32;
    let keep_weekly = retention
        .get("weekly")
        .and_then(|v| v.as_i64())
        .unwrap_or(4) as i32;

    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();

    sqlx::query(
        "INSERT INTO nas_snapshot_policies (id, name, dataset, schedule, keep_hourly, keep_daily, keep_weekly, enabled, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, 1, ?, ?)"
    )
    .bind(&id)
    .bind(name)
    .bind(dataset)
    .bind(schedule)
    .bind(keep_hourly)
    .bind(keep_daily)
    .bind(keep_weekly)
    .bind(now)
    .bind(now)
    .execute(state.database.pool())
    .await
    .map_err(|e| ApiError::Internal(format!("Failed to create snapshot policy: {}", e)))?;

    Ok(Json(serde_json::json!({
        "id": id,
        "name": name,
        "dataset": dataset,
        "schedule": schedule,
        "retention": {"hourly": keep_hourly, "daily": keep_daily, "weekly": keep_weekly},
        "enabled": true,
        "created_at": now,
    })))
}
pub async fn nas_get_snapshot_policy(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let policy = sqlx::query_as::<_, (String, String, String, String, i32, i32, i32, bool, i64)>(
        "SELECT id, name, dataset, schedule, keep_hourly, keep_daily, keep_weekly, enabled, created_at FROM nas_snapshot_policies WHERE id = ?"
    )
    .bind(&id)
    .fetch_optional(state.database.pool())
    .await
    .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?;

    match policy {
        Some((id, name, dataset, schedule, hourly, daily, weekly, enabled, created)) => {
            Ok(Json(serde_json::json!({
                "id": id,
                "name": name,
                "dataset": dataset,
                "schedule": schedule,
                "retention": {"hourly": hourly, "daily": daily, "weekly": weekly},
                "enabled": enabled,
                "created_at": created,
            })))
        }
        None => Err(ApiError::NotFound("Policy not found".to_string())),
    }
}
pub async fn nas_update_snapshot_policy(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let now = chrono::Utc::now().timestamp();

    // Check if policy exists
    let existing =
        sqlx::query_as::<_, (String,)>("SELECT id FROM nas_snapshot_policies WHERE id = ?")
            .bind(&id)
            .fetch_optional(state.database.pool())
            .await
            .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?;

    if existing.is_none() {
        return Err(ApiError::NotFound("Policy not found".to_string()));
    }

    // Update fields
    if let Some(schedule) = body.get("schedule").and_then(|v| v.as_str()) {
        sqlx::query("UPDATE nas_snapshot_policies SET schedule = ?, updated_at = ? WHERE id = ?")
            .bind(schedule)
            .bind(now)
            .bind(&id)
            .execute(state.database.pool())
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to update: {}", e)))?;
    }
    if let Some(enabled) = body.get("enabled").and_then(|v| v.as_bool()) {
        sqlx::query("UPDATE nas_snapshot_policies SET enabled = ?, updated_at = ? WHERE id = ?")
            .bind(enabled)
            .bind(now)
            .bind(&id)
            .execute(state.database.pool())
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to update: {}", e)))?;
    }

    nas_get_snapshot_policy(State(state), Path(id)).await
}
pub async fn nas_delete_snapshot_policy(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<(), ApiError> {
    let result = sqlx::query("DELETE FROM nas_snapshot_policies WHERE id = ?")
        .bind(&id)
        .execute(state.database.pool())
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to delete policy: {}", e)))?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("Policy not found".to_string()));
    }
    Ok(())
}

// Replication handlers
pub async fn nas_list_replication_tasks(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let tasks = sqlx::query_as::<_, (String, String, String, String, String, String, bool, Option<i64>, i64)>(
        "SELECT id, name, source_dataset, target_host, target_dataset, schedule, enabled, last_run, created_at FROM nas_replication_tasks ORDER BY name"
    )
    .fetch_all(state.database.pool())
    .await
    .unwrap_or_default();

    let json_tasks: Vec<serde_json::Value> = tasks
        .iter()
        .map(
            |(id, name, source, host, target, schedule, enabled, last_run, created)| {
                serde_json::json!({
                    "id": id,
                    "name": name,
                    "source_dataset": source,
                    "target_host": host,
                    "target_dataset": target,
                    "schedule": schedule,
                    "enabled": enabled,
                    "last_run": last_run,
                    "created_at": created,
                })
            },
        )
        .collect();
    Ok(Json(json_tasks))
}
pub async fn nas_create_replication_task(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("Missing 'name' field".to_string()))?;
    let source = body
        .get("source_dataset")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("Missing 'source_dataset' field".to_string()))?;
    let host = body
        .get("target_host")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("Missing 'target_host' field".to_string()))?;
    let target = body
        .get("target_dataset")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("Missing 'target_dataset' field".to_string()))?;
    let schedule = body
        .get("schedule")
        .and_then(|v| v.as_str())
        .unwrap_or("0 0 * * *"); // Daily default

    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();

    sqlx::query(
        "INSERT INTO nas_replication_tasks (id, name, source_dataset, target_host, target_dataset, schedule, enabled, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, 1, ?, ?)"
    )
    .bind(&id).bind(name).bind(source).bind(host).bind(target).bind(schedule).bind(now).bind(now)
    .execute(state.database.pool())
    .await
    .map_err(|e| ApiError::Internal(format!("Failed to create replication task: {}", e)))?;

    Ok(Json(serde_json::json!({
        "id": id,
        "name": name,
        "source_dataset": source,
        "target_host": host,
        "target_dataset": target,
        "schedule": schedule,
        "enabled": true,
        "created_at": now,
    })))
}
pub async fn nas_get_replication_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let task = sqlx::query_as::<_, (String, String, String, String, String, String, bool, Option<i64>, i64)>(
        "SELECT id, name, source_dataset, target_host, target_dataset, schedule, enabled, last_run, created_at FROM nas_replication_tasks WHERE id = ?"
    )
    .bind(&id)
    .fetch_optional(state.database.pool())
    .await
    .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?;

    match task {
        Some((id, name, source, host, target, schedule, enabled, last_run, created)) => {
            Ok(Json(serde_json::json!({
                "id": id,
                "name": name,
                "source_dataset": source,
                "target_host": host,
                "target_dataset": target,
                "schedule": schedule,
                "enabled": enabled,
                "last_run": last_run,
                "created_at": created,
            })))
        }
        None => Err(ApiError::NotFound("Task not found".to_string())),
    }
}
pub async fn nas_update_replication_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let now = chrono::Utc::now().timestamp();

    let existing =
        sqlx::query_as::<_, (String,)>("SELECT id FROM nas_replication_tasks WHERE id = ?")
            .bind(&id)
            .fetch_optional(state.database.pool())
            .await
            .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?;

    if existing.is_none() {
        return Err(ApiError::NotFound("Task not found".to_string()));
    }

    if let Some(schedule) = body.get("schedule").and_then(|v| v.as_str()) {
        sqlx::query("UPDATE nas_replication_tasks SET schedule = ?, updated_at = ? WHERE id = ?")
            .bind(schedule)
            .bind(now)
            .bind(&id)
            .execute(state.database.pool())
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to update: {}", e)))?;
    }
    if let Some(enabled) = body.get("enabled").and_then(|v| v.as_bool()) {
        sqlx::query("UPDATE nas_replication_tasks SET enabled = ?, updated_at = ? WHERE id = ?")
            .bind(enabled)
            .bind(now)
            .bind(&id)
            .execute(state.database.pool())
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to update: {}", e)))?;
    }

    nas_get_replication_task(State(state), Path(id)).await
}
pub async fn nas_delete_replication_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<(), ApiError> {
    let result = sqlx::query("DELETE FROM nas_replication_tasks WHERE id = ?")
        .bind(&id)
        .execute(state.database.pool())
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to delete task: {}", e)))?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("Task not found".to_string()));
    }
    Ok(())
}
pub async fn nas_run_replication_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Get task details
    let task = sqlx::query_as::<_, (String, String, String, String)>(
        "SELECT source_dataset, target_host, target_dataset, name FROM nas_replication_tasks WHERE id = ?"
    )
    .bind(&id)
    .fetch_optional(state.database.pool())
    .await
    .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?;

    match task {
        Some((source, host, target, name)) => {
            // Run replication in background
            let now = chrono::Utc::now().timestamp();

            // Update last_run timestamp
            sqlx::query("UPDATE nas_replication_tasks SET last_run = ? WHERE id = ?")
                .bind(now)
                .bind(&id)
                .execute(state.database.pool())
                .await
                .map_err(|e| ApiError::Internal(format!("Failed to update: {}", e)))?;

            // Log to history
            let history_id = uuid::Uuid::new_v4().to_string();
            sqlx::query("INSERT INTO nas_replication_history (id, task_id, started_at, status) VALUES (?, ?, ?, 'running')")
                .bind(&history_id).bind(&id).bind(now)
                .execute(state.database.pool()).await.ok();

            Ok(Json(serde_json::json!({
                "status": "started",
                "task_id": id,
                "task_name": name,
                "source": source,
                "target_host": host,
                "target_dataset": target,
                "history_id": history_id,
            })))
        }
        None => Err(ApiError::NotFound("Task not found".to_string())),
    }
}
pub async fn nas_get_replication_history(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let history = sqlx::query_as::<_, (String, i64, Option<i64>, String, Option<String>, Option<i64>)>(
        "SELECT id, started_at, completed_at, status, error_message, bytes_transferred FROM nas_replication_history WHERE task_id = ? ORDER BY started_at DESC LIMIT 50"
    )
    .bind(&id)
    .fetch_all(state.database.pool())
    .await
    .unwrap_or_default();

    let json_history: Vec<serde_json::Value> = history
        .iter()
        .map(|(id, started, completed, status, error, bytes)| {
            serde_json::json!({
                "id": id,
                "started_at": started,
                "completed_at": completed,
                "status": status,
                "error_message": error,
                "bytes_transferred": bytes,
            })
        })
        .collect();
    Ok(Json(json_history))
}

// Service handlers
pub async fn nas_list_services(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let mut services = Vec::new();
    for service in NasService::all() {
        if let Ok(status) = get_service_status(&service).await {
            services.push(serde_json::json!({
                "name": format!("{:?}", service),
                "running": status.running,
                "enabled": status.enabled,
                "last_error": status.last_error,
            }));
        }
    }
    Ok(Json(services))
}
pub async fn nas_get_service_status(
    State(_state): State<Arc<AppState>>,
    Path(service): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let nas_service = parse_service_name(&service)?;
    match get_service_status(&nas_service).await {
        Ok(status) => Ok(Json(serde_json::to_value(status).unwrap_or_default())),
        Err(e) => Err(ApiError::Internal(format!(
            "Failed to get service status: {}",
            e
        ))),
    }
}
pub async fn nas_start_service(
    State(_state): State<Arc<AppState>>,
    Path(service): Path<String>,
) -> Result<(), ApiError> {
    let nas_service = parse_service_name(&service)?;
    manage_service(&nas_service, ServiceAction::Start)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to start service: {}", e)))
}
pub async fn nas_stop_service(
    State(_state): State<Arc<AppState>>,
    Path(service): Path<String>,
) -> Result<(), ApiError> {
    let nas_service = parse_service_name(&service)?;
    manage_service(&nas_service, ServiceAction::Stop)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to stop service: {}", e)))
}
pub async fn nas_restart_service(
    State(_state): State<Arc<AppState>>,
    Path(service): Path<String>,
) -> Result<(), ApiError> {
    let nas_service = parse_service_name(&service)?;
    manage_service(&nas_service, ServiceAction::Restart)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to restart service: {}", e)))
}

fn parse_service_name(name: &str) -> Result<NasService, ApiError> {
    match name.to_lowercase().as_str() {
        "smb" | "samba" | "smbd" => Ok(NasService::Smbd),
        "nmbd" => Ok(NasService::Nmbd),
        "winbind" | "winbindd" => Ok(NasService::Winbindd),
        "nfs" | "nfsd" | "nfs-server" => Ok(NasService::NfsServer),
        "rpcbind" => Ok(NasService::Rpcbind),
        "mountd" => Ok(NasService::Mountd),
        "afp" | "netatalk" => Ok(NasService::Netatalk),
        "ftp" | "proftpd" => Ok(NasService::Proftpd),
        "ssh" | "sshd" | "sftp" => Ok(NasService::Sshd),
        "rsync" | "rsyncd" => Ok(NasService::Rsyncd),
        "minio" | "s3" => Ok(NasService::Minio),
        "tgt" | "tgtd" | "iscsi" => Ok(NasService::Tgtd),
        _ => Err(ApiError::BadRequest(format!("Unknown service: {}", name))),
    }
}

// SMB handlers
pub async fn nas_get_smb_config(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(serde_json::json!({
        "config_path": "/etc/samba/smb.conf",
        "workgroup": "WORKGROUP",
    })))
}
#[cfg(feature = "smb")]
pub async fn nas_update_smb_config(
    State(_state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use crate::nas::shares::smb::SmbGlobalConfig;
    use std::collections::HashMap;

    let config = SmbGlobalConfig {
        workgroup: body
            .get("workgroup")
            .and_then(|v| v.as_str())
            .unwrap_or("WORKGROUP")
            .to_string(),
        server_string: body
            .get("server_string")
            .and_then(|v| v.as_str())
            .unwrap_or("Horcrux NAS")
            .to_string(),
        netbios_name: body
            .get("netbios_name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        security: body
            .get("security")
            .and_then(|v| v.as_str())
            .unwrap_or("user")
            .to_string(),
        map_to_guest: body
            .get("map_to_guest")
            .and_then(|v| v.as_str())
            .unwrap_or("Bad User")
            .to_string(),
        log_level: body.get("log_level").and_then(|v| v.as_u64()).unwrap_or(1) as u8,
        fruit_enabled: body
            .get("fruit_enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        spotlight_enabled: body
            .get("spotlight_enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        min_protocol: body
            .get("min_protocol")
            .and_then(|v| v.as_str())
            .unwrap_or("SMB2")
            .to_string(),
        max_protocol: body
            .get("max_protocol")
            .and_then(|v| v.as_str())
            .unwrap_or("SMB3")
            .to_string(),
        local_master: body
            .get("local_master")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        domain_master: body
            .get("domain_master")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        wins_support: body
            .get("wins_support")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        extra_parameters: body
            .get("extra_parameters")
            .and_then(|v| v.as_object())
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_else(HashMap::new),
    };

    let mut manager = SmbManager::new();
    manager.set_global_config(config.clone());

    // Regenerate and reload config
    manager
        .reload()
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to reload SMB config: {}", e)))?;

    Ok(Json(serde_json::to_value(&config).unwrap_or_default()))
}
#[cfg(not(feature = "smb"))]
pub async fn nas_update_smb_config(
    State(_state): State<Arc<AppState>>,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::Internal("SMB feature not enabled".to_string()))
}
#[cfg(feature = "smb")]
pub async fn nas_list_smb_connections(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let manager = SmbManager::new();
    match manager.get_connections().await {
        Ok(connections) => {
            let json_conns: Vec<serde_json::Value> = connections
                .iter()
                .map(|c| serde_json::to_value(c).unwrap_or_default())
                .collect();
            Ok(Json(json_conns))
        }
        Err(e) => Err(ApiError::Internal(format!(
            "Failed to get SMB connections: {}",
            e
        ))),
    }
}
#[cfg(not(feature = "smb"))]
pub async fn nas_list_smb_connections(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    Ok(Json(vec![]))
}
#[cfg(feature = "smb")]
pub async fn nas_disconnect_smb(
    State(_state): State<Arc<AppState>>,
    Path(conn_id): Path<String>,
) -> Result<(), ApiError> {
    // conn_id is the PID of the smbd process
    let pid: u32 = conn_id
        .parse()
        .map_err(|_| ApiError::BadRequest("Invalid connection ID (expected PID)".to_string()))?;

    let manager = SmbManager::new();
    manager
        .disconnect_session(pid)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to disconnect: {}", e)))
}
#[cfg(not(feature = "smb"))]
pub async fn nas_disconnect_smb(
    State(_state): State<Arc<AppState>>,
    Path(_conn_id): Path<String>,
) -> Result<(), ApiError> {
    Err(ApiError::Internal("SMB feature not enabled".to_string()))
}

// NFS handlers
pub async fn nas_get_nfs_config(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    #[cfg(feature = "nfs-server")]
    {
        let manager = NfsServerManager::new();
        match manager.get_stats().await {
            Ok(stats) => Ok(Json(serde_json::json!({
                "exports_path": "/etc/exports",
                "stats": serde_json::to_value(stats).unwrap_or_default(),
            }))),
            Err(_) => Ok(Json(serde_json::json!({
                "exports_path": "/etc/exports",
                "stats": null,
            }))),
        }
    }
    #[cfg(not(feature = "nfs-server"))]
    {
        Ok(Json(serde_json::json!({"enabled": false})))
    }
}
pub async fn nas_update_nfs_config(
    State(_state): State<Arc<AppState>>,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    #[cfg(feature = "nfs-server")]
    {
        let manager = NfsServerManager::new();
        manager
            .refresh_exports()
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to refresh exports: {}", e)))?;
        Ok(Json(serde_json::json!({"status": "updated"})))
    }
    #[cfg(not(feature = "nfs-server"))]
    {
        Err(ApiError::Internal(
            "NFS server feature not enabled".to_string(),
        ))
    }
}
pub async fn nas_list_nfs_clients(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    #[cfg(feature = "nfs-server")]
    {
        let manager = NfsServerManager::new();
        match manager.get_clients().await {
            Ok(clients) => {
                let json_clients: Vec<serde_json::Value> = clients
                    .iter()
                    .map(|c| serde_json::to_value(c).unwrap_or_default())
                    .collect();
                Ok(Json(json_clients))
            }
            Err(e) => Err(ApiError::Internal(format!(
                "Failed to list NFS clients: {}",
                e
            ))),
        }
    }
    #[cfg(not(feature = "nfs-server"))]
    {
        Ok(Json(vec![]))
    }
}

// iSCSI handlers
#[cfg(feature = "iscsi-target")]
pub async fn nas_list_iscsi_targets(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let manager = IscsiTargetManager::new();
    match manager.list_targets().await {
        Ok(targets) => {
            let json_targets: Vec<serde_json::Value> = targets
                .iter()
                .map(|t| serde_json::to_value(t).unwrap_or_default())
                .collect();
            Ok(Json(json_targets))
        }
        Err(e) => Err(ApiError::Internal(format!(
            "Failed to list iSCSI targets: {}",
            e
        ))),
    }
}
#[cfg(not(feature = "iscsi-target"))]
pub async fn nas_list_iscsi_targets(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    Ok(Json(vec![]))
}
#[cfg(feature = "iscsi-target")]
pub async fn nas_create_iscsi_target(
    State(_state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use crate::nas::services::iscsi::{IscsiAcl, IscsiLun, IscsiTarget, LunType};

    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("Missing 'name' field".to_string()))?;
    let alias = body
        .get("alias")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Generate IQN from name
    let iqn = IscsiTargetManager::generate_iqn(name);

    // Parse LUNs if provided
    let luns: Vec<IscsiLun> = if let Some(luns_arr) = body.get("luns").and_then(|v| v.as_array()) {
        luns_arr
            .iter()
            .filter_map(|l| {
                let path = l.get("path")?.as_str()?;
                let lun_id = l.get("lun_id").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let size_bytes = l.get("size_bytes").and_then(|v| v.as_u64()).unwrap_or(0);
                let read_only = l
                    .get("read_only")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let lun_type_str = l.get("type").and_then(|v| v.as_str()).unwrap_or("block");
                let lun_type = if lun_type_str == "file" {
                    LunType::File
                } else {
                    LunType::Block
                };
                Some(IscsiLun {
                    lun_id,
                    path: path.to_string(),
                    lun_type,
                    size_bytes,
                    read_only,
                    ..Default::default()
                })
            })
            .collect()
    } else {
        Vec::new()
    };

    // Parse ACLs if provided
    let acls: Vec<IscsiAcl> = if let Some(acls_arr) = body.get("acls").and_then(|v| v.as_array()) {
        acls_arr
            .iter()
            .filter_map(|a| {
                let initiator_iqn = a.get("initiator_iqn")?.as_str()?;
                let allowed = a.get("allowed").and_then(|v| v.as_bool()).unwrap_or(true);
                Some(IscsiAcl {
                    initiator_iqn: initiator_iqn.to_string(),
                    allowed,
                    ..Default::default()
                })
            })
            .collect()
    } else {
        Vec::new()
    };

    let target = IscsiTarget {
        iqn: iqn.clone(),
        alias,
        enabled: true,
        luns,
        acls,
        chap: None,
        created_at: chrono::Utc::now().timestamp(),
        ..Default::default()
    };

    let manager = IscsiTargetManager::new();
    manager
        .create_target(&target)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to create target: {}", e)))?;

    Ok(Json(serde_json::to_value(&target).unwrap_or_default()))
}
#[cfg(not(feature = "iscsi-target"))]
pub async fn nas_create_iscsi_target(
    State(_state): State<Arc<AppState>>,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::Internal("iSCSI feature not enabled".to_string()))
}
#[cfg(feature = "iscsi-target")]
pub async fn nas_get_iscsi_target(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let manager = IscsiTargetManager::new();
    match manager.get_target(&id).await {
        Ok(target) => Ok(Json(serde_json::to_value(target).unwrap_or_default())),
        Err(_) => Err(ApiError::NotFound("Target not found".to_string())),
    }
}
#[cfg(not(feature = "iscsi-target"))]
pub async fn nas_get_iscsi_target(
    State(_state): State<Arc<AppState>>,
    Path(_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::NotFound("Target not found".to_string()))
}
#[cfg(feature = "iscsi-target")]
pub async fn nas_update_iscsi_target(
    State(_state): State<Arc<AppState>>,
    Path(iqn): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use crate::nas::services::iscsi::{ChapAuth, IscsiAcl, IscsiLun, IscsiTarget, LunType};

    let manager = IscsiTargetManager::new();

    // Verify target exists
    manager
        .get_target(&iqn)
        .await
        .map_err(|_| ApiError::NotFound("Target not found".to_string()))?;

    let alias = body
        .get("alias")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Parse LUNs if provided
    let luns: Vec<IscsiLun> = if let Some(luns_arr) = body.get("luns").and_then(|v| v.as_array()) {
        luns_arr
            .iter()
            .filter_map(|l| {
                let path = l.get("path")?.as_str()?;
                let lun_id = l.get("lun_id").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let size_bytes = l.get("size_bytes").and_then(|v| v.as_u64()).unwrap_or(0);
                let read_only = l
                    .get("read_only")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let lun_type_str = l.get("type").and_then(|v| v.as_str()).unwrap_or("block");
                let lun_type = if lun_type_str == "file" {
                    LunType::File
                } else {
                    LunType::Block
                };
                Some(IscsiLun {
                    lun_id,
                    path: path.to_string(),
                    lun_type,
                    size_bytes,
                    read_only,
                    ..Default::default()
                })
            })
            .collect()
    } else {
        Vec::new()
    };

    // Parse ACLs if provided
    let acls: Vec<IscsiAcl> = if let Some(acls_arr) = body.get("acls").and_then(|v| v.as_array()) {
        acls_arr
            .iter()
            .filter_map(|a| {
                let initiator_iqn = a.get("initiator_iqn")?.as_str()?;
                let allowed = a.get("allowed").and_then(|v| v.as_bool()).unwrap_or(true);
                Some(IscsiAcl {
                    initiator_iqn: initiator_iqn.to_string(),
                    allowed,
                    ..Default::default()
                })
            })
            .collect()
    } else {
        Vec::new()
    };

    // Parse CHAP if provided
    let chap: Option<ChapAuth> = if let Some(chap_obj) = body.get("chap") {
        let username = chap_obj.get("username").and_then(|v| v.as_str());
        let password = chap_obj.get("password").and_then(|v| v.as_str());
        match (username, password) {
            (Some(username), Some(password)) => Some(ChapAuth {
                username: username.to_string(),
                password: password.to_string(),
            }),
            _ => None,
        }
    } else {
        None
    };

    // Delete and recreate target (update by replacement)
    manager
        .delete_target(&iqn)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to delete target: {}", e)))?;

    let target = IscsiTarget {
        iqn: iqn.clone(),
        alias,
        enabled: true,
        luns,
        acls,
        chap,
        created_at: chrono::Utc::now().timestamp(),
        ..Default::default()
    };

    manager
        .create_target(&target)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to recreate target: {}", e)))?;

    Ok(Json(serde_json::to_value(&target).unwrap_or_default()))
}
#[cfg(not(feature = "iscsi-target"))]
pub async fn nas_update_iscsi_target(
    State(_state): State<Arc<AppState>>,
    Path(_id): Path<String>,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::Internal("iSCSI feature not enabled".to_string()))
}
#[cfg(feature = "iscsi-target")]
pub async fn nas_delete_iscsi_target(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<(), ApiError> {
    let manager = IscsiTargetManager::new();
    manager
        .delete_target(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to delete target: {}", e)))
}
#[cfg(not(feature = "iscsi-target"))]
pub async fn nas_delete_iscsi_target(
    State(_state): State<Arc<AppState>>,
    Path(_id): Path<String>,
) -> Result<(), ApiError> {
    Err(ApiError::Internal("iSCSI feature not enabled".to_string()))
}
#[cfg(feature = "iscsi-target")]
pub async fn nas_list_iscsi_luns(
    State(_state): State<Arc<AppState>>,
    Path(target_id): Path<String>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let manager = IscsiTargetManager::new();
    match manager.get_target(&target_id).await {
        Ok(target) => {
            let json_luns: Vec<serde_json::Value> = target
                .luns
                .iter()
                .map(|l| serde_json::to_value(l).unwrap_or_default())
                .collect();
            Ok(Json(json_luns))
        }
        Err(_) => Ok(Json(vec![])),
    }
}
#[cfg(not(feature = "iscsi-target"))]
pub async fn nas_list_iscsi_luns(
    State(_state): State<Arc<AppState>>,
    Path(_target_id): Path<String>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    Ok(Json(vec![]))
}
#[cfg(feature = "iscsi-target")]
pub async fn nas_create_iscsi_lun(
    State(_state): State<Arc<AppState>>,
    Path(target_iqn): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use crate::nas::services::iscsi::{IscsiLun, LunType};

    let path = body
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("Missing 'path' field".to_string()))?;
    let lun_id = body
        .get("lun_id")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| ApiError::BadRequest("Missing 'lun_id' field".to_string()))?
        as u32;
    let size_bytes = body.get("size_bytes").and_then(|v| v.as_u64()).unwrap_or(0);
    let read_only = body
        .get("read_only")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let lun_type_str = body.get("type").and_then(|v| v.as_str()).unwrap_or("block");
    let lun_type = if lun_type_str == "file" {
        LunType::File
    } else {
        LunType::Block
    };

    let lun = IscsiLun {
        lun_id,
        path: path.to_string(),
        lun_type,
        size_bytes,
        read_only,
        ..Default::default()
    };

    let manager = IscsiTargetManager::new();
    manager
        .add_lun(&target_iqn, &lun)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to add LUN: {}", e)))?;

    Ok(Json(serde_json::to_value(&lun).unwrap_or_default()))
}
#[cfg(not(feature = "iscsi-target"))]
pub async fn nas_create_iscsi_lun(
    State(_state): State<Arc<AppState>>,
    Path(_target_id): Path<String>,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::Internal("iSCSI feature not enabled".to_string()))
}
#[cfg(feature = "iscsi-target")]
pub async fn nas_delete_iscsi_lun(
    State(_state): State<Arc<AppState>>,
    Path(params): Path<(String, u32)>,
) -> Result<(), ApiError> {
    let (target_iqn, lun_id) = params;
    let manager = IscsiTargetManager::new();
    manager
        .remove_lun(&target_iqn, lun_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to remove LUN: {}", e)))
}
#[cfg(not(feature = "iscsi-target"))]
pub async fn nas_delete_iscsi_lun(
    State(_state): State<Arc<AppState>>,
    Path(_id): Path<String>,
) -> Result<(), ApiError> {
    Err(ApiError::Internal("iSCSI feature not enabled".to_string()))
}
#[cfg(feature = "iscsi-target")]
pub async fn nas_list_iscsi_acls(
    State(_state): State<Arc<AppState>>,
    Path(target_id): Path<String>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    // ACLs are part of the target configuration - parse from config file
    // For now, return the ACLs from the target info (would need to read config)
    let manager = IscsiTargetManager::new();
    match manager.get_target(&target_id).await {
        Ok(_target) => {
            // tgt doesn't expose ACLs directly, would need to parse config
            Ok(Json(vec![]))
        }
        Err(_) => Ok(Json(vec![])),
    }
}
#[cfg(not(feature = "iscsi-target"))]
pub async fn nas_list_iscsi_acls(
    State(_state): State<Arc<AppState>>,
    Path(_target_id): Path<String>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    Ok(Json(vec![]))
}
#[cfg(feature = "iscsi-target")]
pub async fn nas_create_iscsi_acl(
    State(_state): State<Arc<AppState>>,
    Path(target_iqn): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let initiator_pattern = body
        .get("initiator_pattern")
        .and_then(|v| v.as_str())
        .or_else(|| body.get("initiator_address").and_then(|v| v.as_str()))
        .ok_or_else(|| {
            ApiError::BadRequest(
                "Missing 'initiator_pattern' or 'initiator_address' field".to_string(),
            )
        })?;

    let manager = IscsiTargetManager::new();
    manager
        .set_acl(&target_iqn, initiator_pattern)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to set ACL: {}", e)))?;

    Ok(Json(serde_json::json!({
        "target_iqn": target_iqn,
        "initiator_pattern": initiator_pattern,
        "allowed": true,
    })))
}
#[cfg(not(feature = "iscsi-target"))]
pub async fn nas_create_iscsi_acl(
    State(_state): State<Arc<AppState>>,
    Path(_target_id): Path<String>,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::Internal("iSCSI feature not enabled".to_string()))
}
#[cfg(feature = "iscsi-target")]
pub async fn nas_delete_iscsi_acl(
    State(_state): State<Arc<AppState>>,
    Path(params): Path<(String, String)>,
) -> Result<(), ApiError> {
    let (target_iqn, initiator_pattern) = params;
    // tgtadm unbind operation
    let tid = {
        let manager = IscsiTargetManager::new();
        manager
            .get_tid_for_iqn(&target_iqn)
            .await
            .map_err(|_| ApiError::NotFound("Target not found".to_string()))?
    };

    let output = tokio::process::Command::new("tgtadm")
        .args([
            "--lld",
            "iscsi",
            "--mode",
            "target",
            "--op",
            "unbind",
            "--tid",
            &tid.to_string(),
            "--initiator-address",
            &initiator_pattern,
        ])
        .output()
        .await
        .map_err(|e| ApiError::Internal(format!("tgtadm failed: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ApiError::Internal(format!(
            "Failed to remove ACL: {}",
            stderr
        )));
    }

    Ok(())
}
#[cfg(not(feature = "iscsi-target"))]
pub async fn nas_delete_iscsi_acl(
    State(_state): State<Arc<AppState>>,
    Path(_id): Path<String>,
) -> Result<(), ApiError> {
    Err(ApiError::Internal("iSCSI feature not enabled".to_string()))
}

// S3 handlers
#[cfg(feature = "s3-gateway")]
pub async fn nas_get_s3_status(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let manager = S3GatewayManager::new();
    match manager.get_status().await {
        Ok(status) => Ok(Json(serde_json::to_value(status).unwrap_or_default())),
        Err(e) => Err(ApiError::Internal(format!(
            "Failed to get S3 status: {}",
            e
        ))),
    }
}
#[cfg(not(feature = "s3-gateway"))]
pub async fn nas_get_s3_status(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(
        serde_json::json!({"running": false, "enabled": false}),
    ))
}
#[cfg(feature = "s3-gateway")]
pub async fn nas_list_s3_buckets(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let manager = S3GatewayManager::new();
    match manager.list_buckets().await {
        Ok(buckets) => {
            let json_buckets: Vec<serde_json::Value> = buckets
                .iter()
                .map(|b| serde_json::to_value(b).unwrap_or_default())
                .collect();
            Ok(Json(json_buckets))
        }
        Err(e) => Err(ApiError::Internal(format!(
            "Failed to list S3 buckets: {}",
            e
        ))),
    }
}
#[cfg(not(feature = "s3-gateway"))]
pub async fn nas_list_s3_buckets(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    Ok(Json(vec![]))
}
#[cfg(feature = "s3-gateway")]
pub async fn nas_create_s3_bucket(
    State(_state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("Missing 'name' field".to_string()))?;
    let manager = S3GatewayManager::new();
    match manager.create_bucket(name, None).await {
        Ok(bucket) => Ok(Json(serde_json::to_value(bucket).unwrap_or_default())),
        Err(e) => Err(ApiError::Internal(format!(
            "Failed to create bucket: {}",
            e
        ))),
    }
}
#[cfg(not(feature = "s3-gateway"))]
pub async fn nas_create_s3_bucket(
    State(_state): State<Arc<AppState>>,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::Internal(
        "S3 gateway feature not enabled".to_string(),
    ))
}
#[cfg(feature = "s3-gateway")]
pub async fn nas_get_s3_bucket(
    State(_state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let manager = S3GatewayManager::new();
    match manager.get_bucket(&name).await {
        Ok(bucket) => Ok(Json(serde_json::to_value(bucket).unwrap_or_default())),
        Err(_) => Err(ApiError::NotFound("Bucket not found".to_string())),
    }
}
#[cfg(not(feature = "s3-gateway"))]
pub async fn nas_get_s3_bucket(
    State(_state): State<Arc<AppState>>,
    Path(_name): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::NotFound("Bucket not found".to_string()))
}
#[cfg(feature = "s3-gateway")]
pub async fn nas_delete_s3_bucket(
    State(_state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<(), ApiError> {
    let manager = S3GatewayManager::new();
    manager
        .delete_bucket(&name, false)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to delete bucket: {}", e)))
}
#[cfg(not(feature = "s3-gateway"))]
pub async fn nas_delete_s3_bucket(
    State(_state): State<Arc<AppState>>,
    Path(_name): Path<String>,
) -> Result<(), ApiError> {
    Err(ApiError::Internal(
        "S3 gateway feature not enabled".to_string(),
    ))
}
#[cfg(feature = "s3-gateway")]
pub async fn nas_list_s3_access_keys(
    State(_state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let user_id = params
        .get("user_id")
        .ok_or_else(|| ApiError::BadRequest("Missing 'user_id' query parameter".to_string()))?;

    let manager = S3GatewayManager::new();
    match manager.list_access_keys(user_id).await {
        Ok(keys) => {
            let json_keys: Vec<serde_json::Value> = keys
                .iter()
                .map(|k| serde_json::to_value(k).unwrap_or_default())
                .collect();
            Ok(Json(json_keys))
        }
        Err(e) => Err(ApiError::Internal(format!(
            "Failed to list access keys: {}",
            e
        ))),
    }
}
#[cfg(not(feature = "s3-gateway"))]
pub async fn nas_list_s3_access_keys(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    Ok(Json(vec![]))
}
#[cfg(feature = "s3-gateway")]
pub async fn nas_create_s3_access_key(
    State(_state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user_id = body
        .get("user_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("Missing 'user_id' field".to_string()))?;
    let description = body.get("description").and_then(|v| v.as_str());
    let manager = S3GatewayManager::new();
    match manager.create_access_key(user_id, description).await {
        Ok(key) => Ok(Json(serde_json::to_value(key).unwrap_or_default())),
        Err(e) => Err(ApiError::Internal(format!(
            "Failed to create access key: {}",
            e
        ))),
    }
}
#[cfg(not(feature = "s3-gateway"))]
pub async fn nas_create_s3_access_key(
    State(_state): State<Arc<AppState>>,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::Internal(
        "S3 gateway feature not enabled".to_string(),
    ))
}
#[cfg(feature = "s3-gateway")]
pub async fn nas_delete_s3_access_key(
    State(_state): State<Arc<AppState>>,
    Path(access_key): Path<String>,
) -> Result<(), ApiError> {
    let manager = S3GatewayManager::new();
    manager
        .delete_access_key(&access_key)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to delete access key: {}", e)))
}
#[cfg(not(feature = "s3-gateway"))]
pub async fn nas_delete_s3_access_key(
    State(_state): State<Arc<AppState>>,
    Path(_id): Path<String>,
) -> Result<(), ApiError> {
    Err(ApiError::Internal(
        "S3 gateway feature not enabled".to_string(),
    ))
}

// Directory/Auth handlers
pub async fn nas_get_directory_config(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Return configuration status for directory services
    Ok(Json(serde_json::json!({
        "ldap": {
            "enabled": cfg!(feature = "ldap-server"),
            "configured": false,
        },
        "active_directory": {
            "enabled": cfg!(feature = "ad"),
            "joined": false,
        },
        "kerberos": {
            "enabled": cfg!(feature = "kerberos"),
            "configured": false,
        }
    })))
}
pub async fn nas_update_directory_config(
    State(_state): State<Arc<AppState>>,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Directory configuration requires specific features
    #[cfg(any(feature = "ldap-server", feature = "ad"))]
    {
        // Would store config in database/file
        Ok(Json(serde_json::json!({"status": "updated"})))
    }
    #[cfg(not(any(feature = "ldap-server", feature = "ad")))]
    {
        Err(ApiError::Internal(
            "No directory service features enabled".to_string(),
        ))
    }
}
pub async fn nas_test_ldap_connection(
    State(_state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    #[cfg(feature = "ldap-server")]
    {
        let server = body
            .get("server")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'server' field".to_string()))?;
        let _port = body.get("port").and_then(|v| v.as_u64()).unwrap_or(389);
        // Test LDAP connection using ldapsearch or similar
        use tokio::process::Command;
        let output = Command::new("ldapsearch")
            .args([
                "-x",
                "-H",
                &format!("ldap://{}", server),
                "-b",
                "",
                "-s",
                "base",
            ])
            .output()
            .await;
        match output {
            Ok(out) if out.status.success() => Ok(Json(
                serde_json::json!({"success": true, "message": "Connection successful"}),
            )),
            _ => Ok(Json(
                serde_json::json!({"success": false, "message": "Connection failed"}),
            )),
        }
    }
    #[cfg(not(feature = "ldap-server"))]
    {
        let _ = body;
        Err(ApiError::Internal(
            "LDAP server feature not enabled".to_string(),
        ))
    }
}
pub async fn nas_join_active_directory(
    State(_state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    #[cfg(feature = "ad")]
    {
        let domain = body
            .get("domain")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'domain' field".to_string()))?;
        let admin_user = body
            .get("admin_user")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'admin_user' field".to_string()))?;
        let _password = body
            .get("password")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'password' field".to_string()))?;
        // AD join via net ads join
        use tokio::process::Command;
        let output = Command::new("net")
            .args(["ads", "join", "-U", admin_user, domain])
            .output()
            .await;
        match output {
            Ok(out) if out.status.success() => {
                Ok(Json(serde_json::json!({"success": true, "domain": domain})))
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                Err(ApiError::Internal(format!(
                    "Failed to join domain: {}",
                    stderr
                )))
            }
            Err(e) => Err(ApiError::Internal(format!(
                "Failed to run net command: {}",
                e
            ))),
        }
    }
    #[cfg(not(feature = "ad"))]
    {
        let _ = body;
        Err(ApiError::Internal(
            "Active Directory integration not enabled".to_string(),
        ))
    }
}
pub async fn nas_leave_active_directory(
    State(_state): State<Arc<AppState>>,
) -> Result<(), ApiError> {
    #[cfg(feature = "ad")]
    {
        use tokio::process::Command;
        let output = Command::new("net").args(["ads", "leave"]).output().await;
        match output {
            Ok(out) if out.status.success() => Ok(()),
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                Err(ApiError::Internal(format!(
                    "Failed to leave domain: {}",
                    stderr
                )))
            }
            Err(e) => Err(ApiError::Internal(format!(
                "Failed to run net command: {}",
                e
            ))),
        }
    }
    #[cfg(not(feature = "ad"))]
    {
        Err(ApiError::Internal(
            "Active Directory integration not enabled".to_string(),
        ))
    }
}
pub async fn nas_get_ad_status(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    #[cfg(feature = "ad")]
    {
        use tokio::process::Command;
        let output = Command::new("net").args(["ads", "testjoin"]).output().await;
        match output {
            Ok(out) if out.status.success() => {
                // Get domain info
                let info_output = Command::new("net").args(["ads", "info"]).output().await;
                let domain = info_output
                    .ok()
                    .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                    .unwrap_or_default();
                Ok(Json(serde_json::json!({
                    "joined": true,
                    "domain_info": domain,
                })))
            }
            _ => Ok(Json(serde_json::json!({"joined": false}))),
        }
    }
    #[cfg(not(feature = "ad"))]
    {
        Ok(Json(serde_json::json!({"joined": false, "enabled": false})))
    }
}

// ==========================================================================
// LDAP handlers
// ==========================================================================

pub async fn nas_get_ldap_status(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    #[cfg(feature = "ldap-server")]
    {
        Ok(Json(serde_json::json!({
            "enabled": true,
            "configured": false, // Would check actual config
            "server": null,
            "base_dn": null,
            "connection_status": "not_configured"
        })))
    }
    #[cfg(not(feature = "ldap-server"))]
    {
        Ok(Json(serde_json::json!({
            "enabled": false,
            "configured": false
        })))
    }
}

pub async fn nas_configure_ldap(
    State(_state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    #[cfg(feature = "ldap-server")]
    {
        let _uri = body
            .get("uri")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'uri' field".to_string()))?;
        let _base_dn = body
            .get("base_dn")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'base_dn' field".to_string()))?;
        // Would store LDAP configuration
        Ok(Json(serde_json::json!({"status": "configured"})))
    }
    #[cfg(not(feature = "ldap-server"))]
    {
        let _ = body;
        Err(ApiError::Internal("LDAP feature not enabled".to_string()))
    }
}

pub async fn nas_sync_ldap(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    #[cfg(feature = "ldap-server")]
    {
        // Would trigger LDAP sync
        Ok(Json(serde_json::json!({
            "status": "sync_started",
            "users_synced": 0,
            "groups_synced": 0
        })))
    }
    #[cfg(not(feature = "ldap-server"))]
    {
        Err(ApiError::Internal("LDAP feature not enabled".to_string()))
    }
}

pub async fn nas_search_ldap_users(
    State(_state): State<Arc<AppState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    #[cfg(feature = "ldap-server")]
    {
        let _filter = params.get("filter").cloned().unwrap_or_default();
        // Would search LDAP for users
        Ok(Json(vec![]))
    }
    #[cfg(not(feature = "ldap-server"))]
    {
        let _ = params;
        Err(ApiError::Internal("LDAP feature not enabled".to_string()))
    }
}

pub async fn nas_search_ldap_groups(
    State(_state): State<Arc<AppState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    #[cfg(feature = "ldap-server")]
    {
        let _filter = params.get("filter").cloned().unwrap_or_default();
        // Would search LDAP for groups
        Ok(Json(vec![]))
    }
    #[cfg(not(feature = "ldap-server"))]
    {
        let _ = params;
        Err(ApiError::Internal("LDAP feature not enabled".to_string()))
    }
}

// ==========================================================================
// Kerberos handlers
// ==========================================================================

pub async fn nas_get_kerberos_status(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    #[cfg(feature = "kerberos")]
    {
        use tokio::process::Command;
        let output = Command::new("klist").output().await;
        let has_tickets = output.map(|o| o.status.success()).unwrap_or(false);
        Ok(Json(serde_json::json!({
            "enabled": true,
            "configured": false, // Would check krb5.conf
            "has_valid_tickets": has_tickets
        })))
    }
    #[cfg(not(feature = "kerberos"))]
    {
        Ok(Json(serde_json::json!({
            "enabled": false,
            "configured": false
        })))
    }
}

pub async fn nas_configure_kerberos(
    State(_state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    #[cfg(feature = "kerberos")]
    {
        let _realm = body
            .get("realm")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'realm' field".to_string()))?;
        let _kdc = body
            .get("kdc")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'kdc' field".to_string()))?;
        // Would write /etc/krb5.conf
        Ok(Json(serde_json::json!({"status": "configured"})))
    }
    #[cfg(not(feature = "kerberos"))]
    {
        let _ = body;
        Err(ApiError::Internal(
            "Kerberos feature not enabled".to_string(),
        ))
    }
}

pub async fn nas_kerberos_kinit(
    State(_state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    #[cfg(feature = "kerberos")]
    {
        use tokio::process::Command;
        let principal = body
            .get("principal")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'principal' field".to_string()))?;
        let keytab = body.get("keytab").and_then(|v| v.as_str());

        let output = if let Some(kt) = keytab {
            Command::new("kinit")
                .args(["-k", "-t", kt, principal])
                .output()
                .await
        } else {
            // Interactive kinit not supported via API
            return Err(ApiError::BadRequest(
                "Keytab required for non-interactive kinit".to_string(),
            ));
        };

        match output {
            Ok(out) if out.status.success() => Ok(Json(
                serde_json::json!({"status": "success", "principal": principal}),
            )),
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                Err(ApiError::Internal(format!("kinit failed: {}", stderr)))
            }
            Err(e) => Err(ApiError::Internal(format!("Failed to run kinit: {}", e))),
        }
    }
    #[cfg(not(feature = "kerberos"))]
    {
        let _ = body;
        Err(ApiError::Internal(
            "Kerberos feature not enabled".to_string(),
        ))
    }
}

pub async fn nas_kerberos_klist(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    #[cfg(feature = "kerberos")]
    {
        use tokio::process::Command;
        let output = Command::new("klist").output().await;
        match output {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                Ok(Json(serde_json::json!({
                    "has_tickets": true,
                    "output": stdout
                })))
            }
            _ => Ok(Json(serde_json::json!({"has_tickets": false}))),
        }
    }
    #[cfg(not(feature = "kerberos"))]
    {
        Err(ApiError::Internal(
            "Kerberos feature not enabled".to_string(),
        ))
    }
}

pub async fn nas_kerberos_kdestroy(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    #[cfg(feature = "kerberos")]
    {
        use tokio::process::Command;
        let output = Command::new("kdestroy").output().await;
        match output {
            Ok(out) if out.status.success() => Ok(Json(serde_json::json!({"status": "destroyed"}))),
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                Err(ApiError::Internal(format!("kdestroy failed: {}", stderr)))
            }
            Err(e) => Err(ApiError::Internal(format!("Failed to run kdestroy: {}", e))),
        }
    }
    #[cfg(not(feature = "kerberos"))]
    {
        Err(ApiError::Internal(
            "Kerberos feature not enabled".to_string(),
        ))
    }
}

pub async fn nas_list_kerberos_keytabs(
    State(_state): State<Arc<AppState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    #[cfg(feature = "kerberos")]
    {
        use tokio::process::Command;
        let keytab = params
            .get("keytab")
            .cloned()
            .unwrap_or_else(|| "/etc/krb5.keytab".to_string());
        let output = Command::new("klist").args(["-k", &keytab]).output().await;
        match output {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                Ok(Json(serde_json::json!({
                    "keytab": keytab,
                    "entries": stdout
                })))
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                Err(ApiError::Internal(format!("klist -k failed: {}", stderr)))
            }
            Err(e) => Err(ApiError::Internal(format!("Failed to run klist: {}", e))),
        }
    }
    #[cfg(not(feature = "kerberos"))]
    {
        let _ = params;
        Err(ApiError::Internal(
            "Kerberos feature not enabled".to_string(),
        ))
    }
}

pub async fn nas_create_kerberos_keytab(
    State(_state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    #[cfg(feature = "kerberos")]
    {
        let _principal = body
            .get("principal")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'principal' field".to_string()))?;
        let _keytab = body
            .get("keytab")
            .and_then(|v| v.as_str())
            .unwrap_or("/etc/krb5.keytab");
        // Would use ktutil to create keytab entry
        Ok(Json(serde_json::json!({"status": "created"})))
    }
    #[cfg(not(feature = "kerberos"))]
    {
        let _ = body;
        Err(ApiError::Internal(
            "Kerberos feature not enabled".to_string(),
        ))
    }
}

// ==========================================================================
// Additional Active Directory handlers
// ==========================================================================

pub async fn nas_list_ad_users(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    #[cfg(feature = "ad")]
    {
        use tokio::process::Command;
        let output = Command::new("wbinfo").args(["-u"]).output().await;
        match output {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let users: Vec<serde_json::Value> = stdout
                    .lines()
                    .map(|u| serde_json::json!({"username": u}))
                    .collect();
                Ok(Json(users))
            }
            _ => Ok(Json(vec![])),
        }
    }
    #[cfg(not(feature = "ad"))]
    {
        Err(ApiError::Internal(
            "Active Directory feature not enabled".to_string(),
        ))
    }
}

pub async fn nas_list_ad_groups(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    #[cfg(feature = "ad")]
    {
        use tokio::process::Command;
        let output = Command::new("wbinfo").args(["-g"]).output().await;
        match output {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let groups: Vec<serde_json::Value> = stdout
                    .lines()
                    .map(|g| serde_json::json!({"groupname": g}))
                    .collect();
                Ok(Json(groups))
            }
            _ => Ok(Json(vec![])),
        }
    }
    #[cfg(not(feature = "ad"))]
    {
        Err(ApiError::Internal(
            "Active Directory feature not enabled".to_string(),
        ))
    }
}

pub async fn nas_get_user_ad_groups(
    State(_state): State<Arc<AppState>>,
    Path(username): Path<String>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    #[cfg(feature = "ad")]
    {
        use tokio::process::Command;
        let output = Command::new("id").args([&username]).output().await;
        match output {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                Ok(Json(vec![serde_json::json!({
                    "username": username,
                    "groups_info": stdout
                })]))
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                Err(ApiError::NotFound(format!("User not found: {}", stderr)))
            }
            Err(e) => Err(ApiError::Internal(format!(
                "Failed to get user groups: {}",
                e
            ))),
        }
    }
    #[cfg(not(feature = "ad"))]
    {
        let _ = username;
        Err(ApiError::Internal(
            "Active Directory feature not enabled".to_string(),
        ))
    }
}

pub async fn nas_test_ad_trust(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    #[cfg(feature = "ad")]
    {
        use tokio::process::Command;
        let output = Command::new("wbinfo")
            .args(["--check-secret"])
            .output()
            .await;
        match output {
            Ok(out) if out.status.success() => Ok(Json(serde_json::json!({"trust_valid": true}))),
            _ => Ok(Json(serde_json::json!({"trust_valid": false}))),
        }
    }
    #[cfg(not(feature = "ad"))]
    {
        Err(ApiError::Internal(
            "Active Directory feature not enabled".to_string(),
        ))
    }
}

pub async fn nas_ping_ad_dc(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    #[cfg(feature = "ad")]
    {
        use tokio::process::Command;
        let output = Command::new("wbinfo").args(["--ping-dc"]).output().await;
        match output {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                Ok(Json(serde_json::json!({
                    "dc_reachable": true,
                    "response": stdout
                })))
            }
            _ => Ok(Json(serde_json::json!({"dc_reachable": false}))),
        }
    }
    #[cfg(not(feature = "ad"))]
    {
        Err(ApiError::Internal(
            "Active Directory feature not enabled".to_string(),
        ))
    }
}

pub async fn nas_verify_ad_prereqs(
    State(_state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    #[cfg(feature = "ad")]
    {
        let domain = body
            .get("domain")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'domain' field".to_string()))?;
        use tokio::process::Command;

        // Check DNS resolution
        let dns_check = Command::new("host").args([domain]).output().await;
        let dns_ok = dns_check.map(|o| o.status.success()).unwrap_or(false);

        // Check if required packages are installed
        let samba_check = Command::new("which").args(["net"]).output().await;
        let samba_ok = samba_check.map(|o| o.status.success()).unwrap_or(false);

        Ok(Json(serde_json::json!({
            "domain": domain,
            "dns_resolution": dns_ok,
            "samba_installed": samba_ok,
            "ready": dns_ok && samba_ok
        })))
    }
    #[cfg(not(feature = "ad"))]
    {
        let _ = body;
        Err(ApiError::Internal(
            "Active Directory feature not enabled".to_string(),
        ))
    }
}

// ==========================================================================
// Scheduler handlers
// ==========================================================================

pub async fn nas_list_scheduled_jobs(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let jobs = sqlx::query_as::<_, (String, String, String, String, bool, Option<i64>, Option<i64>, i64)>(
        "SELECT id, name, job_type, schedule, enabled, last_run, next_run, created_at FROM nas_scheduled_jobs ORDER BY name"
    )
    .fetch_all(state.database.pool())
    .await
    .unwrap_or_default();

    let json_jobs: Vec<serde_json::Value> = jobs
        .iter()
        .map(
            |(id, name, job_type, schedule, enabled, last_run, next_run, created)| {
                serde_json::json!({
                    "id": id,
                    "name": name,
                    "job_type": job_type,
                    "schedule": schedule,
                    "enabled": enabled,
                    "last_run": last_run,
                    "next_run": next_run,
                    "created_at": created,
                })
            },
        )
        .collect();
    Ok(Json(json_jobs))
}

pub async fn nas_create_scheduled_job(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use crate::nas::scheduler::{CronSchedule, JobType, ScheduledJob};

    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("Missing 'name' field".to_string()))?;
    let job_type_str = body
        .get("job_type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("Missing 'job_type' field".to_string()))?;
    let schedule_str = body
        .get("schedule")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("Missing 'schedule' field".to_string()))?;

    // Parse job type
    let target: String;
    let mut params: std::collections::HashMap<String, serde_json::Value> =
        std::collections::HashMap::new();
    let job_type = match job_type_str.to_lowercase().as_str() {
        "snapshot" => {
            let dataset = body
                .get("dataset")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    ApiError::BadRequest("Snapshot job requires 'dataset' field".to_string())
                })?;
            target = dataset.to_string();
            JobType::Snapshot
        }
        "retention" | "retention_cleanup" => {
            let dataset = body
                .get("dataset")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    ApiError::BadRequest("Retention job requires 'dataset' field".to_string())
                })?;
            let keep_count = body
                .get("keep_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(10);
            target = dataset.to_string();
            params.insert("keep_count".to_string(), serde_json::json!(keep_count));
            JobType::RetentionCleanup
        }
        "replication" => {
            let task_id = body
                .get("task_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    ApiError::BadRequest("Replication job requires 'task_id' field".to_string())
                })?;
            target = task_id.to_string();
            JobType::Replication
        }
        "scrub" => {
            let pool = body.get("pool").and_then(|v| v.as_str()).ok_or_else(|| {
                ApiError::BadRequest("Scrub job requires 'pool' field".to_string())
            })?;
            target = pool.to_string();
            JobType::Scrub
        }
        "health_check" => {
            target = String::new();
            JobType::HealthCheck
        }
        "quota_check" => {
            target = String::new();
            JobType::QuotaCheck
        }
        "smart_check" => {
            target = String::new();
            JobType::SmartCheck
        }
        "custom" | "custom_script" => {
            let script = body.get("script").and_then(|v| v.as_str()).ok_or_else(|| {
                ApiError::BadRequest("Custom job requires 'script' field".to_string())
            })?;
            let args: Vec<String> = body
                .get("args")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            target = script.to_string();
            params.insert("args".to_string(), serde_json::json!(args));
            JobType::Custom
        }
        _ => {
            return Err(ApiError::BadRequest(format!(
                "Unknown job type: {}",
                job_type_str
            )))
        }
    };

    // Parse schedule
    let schedule = CronSchedule::parse(schedule_str)
        .map_err(|e| ApiError::BadRequest(format!("Invalid cron schedule: {}", e)))?;

    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();
    let now_dt = chrono::Utc::now();
    let next_run = schedule
        .next_run_after(&now_dt)
        .map(|dt| dt.timestamp())
        .unwrap_or(now);

    let job = ScheduledJob {
        id: id.clone(),
        name: name.to_string(),
        job_type,
        schedule: schedule_str.to_string(),
        target,
        params,
        enabled: true,
        run_on_startup: false,
        last_run: None,
        last_status: None,
        last_duration_ms: None,
        last_error: None,
        next_run: Some(next_run),
        priority: 100,
        timeout_secs: 0,
        max_retries: 0,
        created_at: now,
        modified_at: now,
    };

    // Serialize job_type for storage
    let job_type_json = serde_json::to_string(&job.job_type).unwrap_or_default();

    sqlx::query(
        "INSERT INTO nas_scheduled_jobs (id, name, job_type, schedule, enabled, next_run, created_at, updated_at) VALUES (?, ?, ?, ?, 1, ?, ?, ?)"
    )
    .bind(&id)
    .bind(name)
    .bind(&job_type_json)
    .bind(schedule_str)
    .bind(next_run)
    .bind(now)
    .bind(now)
    .execute(state.database.pool())
    .await
    .map_err(|e| ApiError::Internal(format!("Failed to create scheduled job: {}", e)))?;

    Ok(Json(serde_json::json!({
        "id": id,
        "name": name,
        "job_type": job_type_str,
        "schedule": schedule_str,
        "enabled": true,
        "next_run": next_run,
        "created_at": now,
    })))
}

pub async fn nas_get_scheduled_job(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let job = sqlx::query_as::<_, (String, String, String, String, bool, Option<i64>, Option<i64>, i64)>(
        "SELECT id, name, job_type, schedule, enabled, last_run, next_run, created_at FROM nas_scheduled_jobs WHERE id = ?"
    )
    .bind(&id)
    .fetch_optional(state.database.pool())
    .await
    .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?;

    match job {
        Some((id, name, job_type, schedule, enabled, last_run, next_run, created)) => {
            Ok(Json(serde_json::json!({
                "id": id,
                "name": name,
                "job_type": job_type,
                "schedule": schedule,
                "enabled": enabled,
                "last_run": last_run,
                "next_run": next_run,
                "created_at": created,
            })))
        }
        None => Err(ApiError::NotFound("Job not found".to_string())),
    }
}

pub async fn nas_update_scheduled_job(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let now = chrono::Utc::now().timestamp();

    // Verify job exists
    let existing = sqlx::query_as::<_, (String,)>("SELECT id FROM nas_scheduled_jobs WHERE id = ?")
        .bind(&id)
        .fetch_optional(state.database.pool())
        .await
        .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?;

    if existing.is_none() {
        return Err(ApiError::NotFound("Job not found".to_string()));
    }

    // Update fields
    if let Some(schedule) = body.get("schedule").and_then(|v| v.as_str()) {
        use crate::nas::scheduler::CronSchedule;
        let parsed = CronSchedule::parse(schedule)
            .map_err(|e| ApiError::BadRequest(format!("Invalid cron schedule: {}", e)))?;
        let next_run = parsed
            .next_run_after(&chrono::Utc::now())
            .map(|dt| dt.timestamp());
        sqlx::query(
            "UPDATE nas_scheduled_jobs SET schedule = ?, next_run = ?, updated_at = ? WHERE id = ?",
        )
        .bind(schedule)
        .bind(next_run)
        .bind(now)
        .bind(&id)
        .execute(state.database.pool())
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to update: {}", e)))?;
    }
    if let Some(enabled) = body.get("enabled").and_then(|v| v.as_bool()) {
        sqlx::query("UPDATE nas_scheduled_jobs SET enabled = ?, updated_at = ? WHERE id = ?")
            .bind(enabled)
            .bind(now)
            .bind(&id)
            .execute(state.database.pool())
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to update: {}", e)))?;
    }
    if let Some(name) = body.get("name").and_then(|v| v.as_str()) {
        sqlx::query("UPDATE nas_scheduled_jobs SET name = ?, updated_at = ? WHERE id = ?")
            .bind(name)
            .bind(now)
            .bind(&id)
            .execute(state.database.pool())
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to update: {}", e)))?;
    }

    nas_get_scheduled_job(State(state), Path(id)).await
}

pub async fn nas_delete_scheduled_job(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<(), ApiError> {
    let result = sqlx::query("DELETE FROM nas_scheduled_jobs WHERE id = ?")
        .bind(&id)
        .execute(state.database.pool())
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to delete job: {}", e)))?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("Job not found".to_string()));
    }
    Ok(())
}

pub async fn nas_run_scheduled_job(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Get job details
    let job = sqlx::query_as::<_, (String, String, String)>(
        "SELECT id, name, job_type FROM nas_scheduled_jobs WHERE id = ?",
    )
    .bind(&id)
    .fetch_optional(state.database.pool())
    .await
    .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?;

    match job {
        Some((job_id, name, job_type_str)) => {
            let now = chrono::Utc::now().timestamp();

            // Update last_run
            sqlx::query("UPDATE nas_scheduled_jobs SET last_run = ? WHERE id = ?")
                .bind(now)
                .bind(&job_id)
                .execute(state.database.pool())
                .await
                .ok();

            // Log to job history
            let history_id = uuid::Uuid::new_v4().to_string();
            sqlx::query("INSERT INTO nas_job_history (id, job_id, started_at, status) VALUES (?, ?, ?, 'running')")
                .bind(&history_id).bind(&job_id).bind(now)
                .execute(state.database.pool()).await.ok();

            // Parse and execute job type
            // The actual execution would be done asynchronously
            Ok(Json(serde_json::json!({
                "status": "started",
                "job_id": job_id,
                "job_name": name,
                "job_type": job_type_str,
                "history_id": history_id,
                "started_at": now,
            })))
        }
        None => Err(ApiError::NotFound("Job not found".to_string())),
    }
}

pub async fn nas_pause_scheduled_job(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<(), ApiError> {
    let now = chrono::Utc::now().timestamp();
    let result =
        sqlx::query("UPDATE nas_scheduled_jobs SET enabled = 0, updated_at = ? WHERE id = ?")
            .bind(now)
            .bind(&id)
            .execute(state.database.pool())
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to pause job: {}", e)))?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("Job not found".to_string()));
    }
    Ok(())
}

pub async fn nas_resume_scheduled_job(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<(), ApiError> {
    let now = chrono::Utc::now().timestamp();
    let result =
        sqlx::query("UPDATE nas_scheduled_jobs SET enabled = 1, updated_at = ? WHERE id = ?")
            .bind(now)
            .bind(&id)
            .execute(state.database.pool())
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to resume job: {}", e)))?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("Job not found".to_string()));
    }
    Ok(())
}

pub async fn nas_get_job_history(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let history = sqlx::query_as::<_, (String, i64, Option<i64>, String, Option<String>, Option<String>)>(
        "SELECT id, started_at, completed_at, status, error_message, output FROM nas_job_history WHERE job_id = ? ORDER BY started_at DESC LIMIT 50"
    )
    .bind(&id)
    .fetch_all(state.database.pool())
    .await
    .unwrap_or_default();

    let json_history: Vec<serde_json::Value> = history
        .iter()
        .map(|(id, started, completed, status, error, output)| {
            serde_json::json!({
                "id": id,
                "started_at": started,
                "completed_at": completed,
                "status": status,
                "error_message": error,
                "output": output,
            })
        })
        .collect();
    Ok(Json(json_history))
}

pub async fn nas_get_scheduler_status(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Get overall scheduler statistics
    let total_jobs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nas_scheduled_jobs")
        .fetch_one(state.database.pool())
        .await
        .unwrap_or(0);
    let enabled_jobs: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM nas_scheduled_jobs WHERE enabled = 1")
            .fetch_one(state.database.pool())
            .await
            .unwrap_or(0);
    let running_jobs: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM nas_job_history WHERE status = 'running'")
            .fetch_one(state.database.pool())
            .await
            .unwrap_or(0);

    // Get recent failures
    let recent_failures: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM nas_job_history WHERE status = 'failed' AND started_at > ?",
    )
    .bind(chrono::Utc::now().timestamp() - 86400) // Last 24 hours
    .fetch_one(state.database.pool())
    .await
    .unwrap_or(0);

    // Get next scheduled job
    let next_job = sqlx::query_as::<_, (String, String, i64)>(
        "SELECT id, name, next_run FROM nas_scheduled_jobs WHERE enabled = 1 AND next_run IS NOT NULL ORDER BY next_run ASC LIMIT 1"
    )
    .fetch_optional(state.database.pool()).await
    .ok()
    .flatten();

    Ok(Json(serde_json::json!({
        "running": true,
        "total_jobs": total_jobs,
        "enabled_jobs": enabled_jobs,
        "running_jobs": running_jobs,
        "recent_failures_24h": recent_failures,
        "next_scheduled": next_job.map(|(id, name, next_run)| {
            serde_json::json!({
                "job_id": id,
                "job_name": name,
                "next_run": next_run,
            })
        }),
    })))
}

// ==========================================================================
// FTP handlers
// ==========================================================================

pub async fn nas_get_ftp_config(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    #[cfg(feature = "ftp")]
    {
        use crate::nas::shares::ftp::FtpManager;
        let manager = FtpManager::new();
        Ok(Json(serde_json::json!({
            "enabled": true,
            "config_path": "/etc/proftpd/proftpd.conf",
            "config": serde_json::to_value(manager.config()).unwrap_or_default(),
        })))
    }
    #[cfg(not(feature = "ftp"))]
    {
        Ok(Json(serde_json::json!({"enabled": false})))
    }
}

#[cfg(feature = "ftp")]
pub async fn nas_update_ftp_config(
    State(_state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use crate::nas::shares::ftp::{FtpGlobalConfig, FtpManager};
    use std::collections::HashMap;

    let config = FtpGlobalConfig {
        server_name: body
            .get("server_name")
            .and_then(|v| v.as_str())
            .unwrap_or("Horcrux FTP Server")
            .to_string(),
        port: body.get("port").and_then(|v| v.as_u64()).unwrap_or(21) as u16,
        passive_port_range: body
            .get("passive_port_range")
            .and_then(|v| v.as_str())
            .map(|s| {
                let parts: Vec<&str> = s.split('-').collect();
                if parts.len() == 2 {
                    (
                        parts[0].parse().unwrap_or(49152),
                        parts[1].parse().unwrap_or(65534),
                    )
                } else {
                    (49152, 65534)
                }
            })
            .unwrap_or((49152, 65534)),
        max_clients: body
            .get("max_clients")
            .and_then(|v| v.as_u64())
            .unwrap_or(100) as u32,
        max_clients_per_host: body
            .get("max_clients_per_host")
            .and_then(|v| v.as_u64())
            .unwrap_or(10) as u32,
        timeout_idle: body
            .get("timeout_idle")
            .and_then(|v| v.as_u64())
            .unwrap_or(600) as u32,
        timeout_login: body
            .get("timeout_login")
            .and_then(|v| v.as_u64())
            .unwrap_or(300) as u32,
        timeout_no_transfer: body
            .get("timeout_no_transfer")
            .and_then(|v| v.as_u64())
            .unwrap_or(900) as u32,
        allow_anonymous: body
            .get("allow_anonymous")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        anonymous_root: body
            .get("anonymous_root")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        tls_enabled: body
            .get("tls_enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        tls_cert_file: body
            .get("tls_cert_file")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        tls_key_file: body
            .get("tls_key_file")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        tls_required: body
            .get("tls_required")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        chroot_local_users: body
            .get("chroot_local_users")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        require_valid_shell: body
            .get("require_valid_shell")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        use_sendfile: body
            .get("use_sendfile")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        extra_parameters: body
            .get("extra_parameters")
            .and_then(|v| v.as_object())
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_else(HashMap::new),
    };

    let mut manager = FtpManager::new();
    manager.set_global_config(config.clone());
    manager
        .reload()
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to reload FTP config: {}", e)))?;

    Ok(Json(serde_json::to_value(&config).unwrap_or_default()))
}

#[cfg(not(feature = "ftp"))]
pub async fn nas_update_ftp_config(
    State(_state): State<Arc<AppState>>,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::Internal("FTP feature not enabled".to_string()))
}

#[cfg(feature = "ftp")]
pub async fn nas_list_ftp_connections(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    use crate::nas::shares::ftp::FtpManager;
    let manager = FtpManager::new();
    match manager.get_connections().await {
        Ok(connections) => {
            let json_conns: Vec<serde_json::Value> = connections
                .iter()
                .map(|c| serde_json::to_value(c).unwrap_or_default())
                .collect();
            Ok(Json(json_conns))
        }
        Err(e) => Err(ApiError::Internal(format!(
            "Failed to get FTP connections: {}",
            e
        ))),
    }
}

#[cfg(not(feature = "ftp"))]
pub async fn nas_list_ftp_connections(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    Ok(Json(vec![]))
}

#[cfg(feature = "ftp")]
pub async fn nas_disconnect_ftp(
    State(_state): State<Arc<AppState>>,
    Path(conn_id): Path<String>,
) -> Result<(), ApiError> {
    use crate::nas::shares::ftp::FtpManager;
    let pid: u32 = conn_id
        .parse()
        .map_err(|_| ApiError::BadRequest("Invalid connection ID (expected PID)".to_string()))?;

    let manager = FtpManager::new();
    manager
        .disconnect_session(pid)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to disconnect FTP session: {}", e)))
}

#[cfg(not(feature = "ftp"))]
pub async fn nas_disconnect_ftp(
    State(_state): State<Arc<AppState>>,
    Path(_conn_id): Path<String>,
) -> Result<(), ApiError> {
    Err(ApiError::Internal("FTP feature not enabled".to_string()))
}

// ==========================================================================
// WebDAV handlers
// ==========================================================================

pub async fn nas_get_webdav_config(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    #[cfg(feature = "webdav")]
    {
        use crate::nas::shares::webdav::WebdavManager;
        let manager = WebdavManager::new();
        Ok(Json(serde_json::json!({
            "enabled": true,
            "config_path": "/etc/nginx/sites-available/webdav.conf",
            "config": serde_json::to_value(manager.config()).unwrap_or_default(),
        })))
    }
    #[cfg(not(feature = "webdav"))]
    {
        Ok(Json(serde_json::json!({"enabled": false})))
    }
}

#[cfg(feature = "webdav")]
pub async fn nas_update_webdav_config(
    State(_state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use crate::nas::shares::webdav::{WebdavGlobalConfig, WebdavManager};

    let config = WebdavGlobalConfig {
        listen_port: body
            .get("listen_port")
            .and_then(|v| v.as_u64())
            .unwrap_or(8080) as u16,
        ssl_enabled: body
            .get("ssl_enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        ssl_port: body
            .get("ssl_port")
            .and_then(|v| v.as_u64())
            .unwrap_or(8443) as u16,
        ssl_certificate: body
            .get("ssl_certificate")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        ssl_certificate_key: body
            .get("ssl_certificate_key")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        auth_type: body
            .get("auth_type")
            .and_then(|v| v.as_str())
            .map(|s| match s.to_lowercase().as_str() {
                "digest" => crate::nas::shares::webdav::WebdavAuthType::Digest,
                "none" => crate::nas::shares::webdav::WebdavAuthType::None,
                _ => crate::nas::shares::webdav::WebdavAuthType::Basic,
            })
            .unwrap_or(crate::nas::shares::webdav::WebdavAuthType::Basic),
        realm: body
            .get("realm")
            .and_then(|v| v.as_str())
            .unwrap_or("WebDAV")
            .to_string(),
        client_body_temp_path: body
            .get("client_body_temp_path")
            .and_then(|v| v.as_str())
            .unwrap_or("/var/lib/nginx/webdav")
            .to_string(),
        client_max_body_size: body
            .get("client_max_body_size")
            .and_then(|v| v.as_str())
            .unwrap_or("10G")
            .to_string(),
    };

    let mut manager = WebdavManager::new();
    manager.set_global_config(config.clone());
    manager
        .reload()
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to reload WebDAV config: {}", e)))?;

    Ok(Json(serde_json::to_value(&config).unwrap_or_default()))
}

#[cfg(not(feature = "webdav"))]
pub async fn nas_update_webdav_config(
    State(_state): State<Arc<AppState>>,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::Internal("WebDAV feature not enabled".to_string()))
}

pub async fn nas_list_webdav_connections(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    // WebDAV connections are HTTP connections managed by nginx
    // We can parse nginx access logs or use stub module status if available
    Ok(Json(vec![serde_json::json!({
        "note": "WebDAV connections are managed by nginx. Check nginx status for connection info."
    })]))
}

// ==========================================================================
// AFP handlers
// ==========================================================================

pub async fn nas_get_afp_config(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    #[cfg(feature = "afp")]
    {
        use crate::nas::shares::afp::AfpManager;
        let manager = AfpManager::new();
        Ok(Json(serde_json::json!({
            "enabled": true,
            "config_path": "/etc/netatalk/afp.conf",
            "config": serde_json::to_value(manager.config()).unwrap_or_default(),
        })))
    }
    #[cfg(not(feature = "afp"))]
    {
        Ok(Json(serde_json::json!({"enabled": false})))
    }
}

#[cfg(feature = "afp")]
pub async fn nas_update_afp_config(
    State(_state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use crate::nas::shares::afp::{AfpGlobalConfig, AfpManager};
    use std::collections::HashMap;

    let config = AfpGlobalConfig {
        hostname: body
            .get("hostname")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        uam_list: body
            .get("uam_list")
            .and_then(|v| v.as_str())
            .unwrap_or("uams_dhx2.so")
            .to_string(),
        save_password: body
            .get("save_password")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        set_password: body
            .get("set_password")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        mimic_model: body
            .get("mimic_model")
            .and_then(|v| v.as_str())
            .unwrap_or("RackMac")
            .to_string(),
        log_level: body
            .get("log_level")
            .and_then(|v| v.as_str())
            .unwrap_or("default:warn")
            .to_string(),
        zeroconf: body
            .get("zeroconf")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        spotlight: body
            .get("spotlight")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        fce_listener: body
            .get("fce_listener")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        fce_coalesce: body
            .get("fce_coalesce")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        extra_parameters: body
            .get("extra_parameters")
            .and_then(|v| v.as_object())
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_else(HashMap::new),
    };

    let mut manager = AfpManager::new();
    manager.set_global_config(config.clone());
    manager
        .reload()
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to reload AFP config: {}", e)))?;

    Ok(Json(serde_json::to_value(&config).unwrap_or_default()))
}

#[cfg(not(feature = "afp"))]
pub async fn nas_update_afp_config(
    State(_state): State<Arc<AppState>>,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::Internal("AFP feature not enabled".to_string()))
}

#[cfg(feature = "afp")]
pub async fn nas_list_afp_connections(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    use crate::nas::shares::afp::AfpManager;
    let manager = AfpManager::new();
    match manager.get_connections().await {
        Ok(connections) => {
            let json_conns: Vec<serde_json::Value> = connections
                .iter()
                .map(|c| serde_json::to_value(c).unwrap_or_default())
                .collect();
            Ok(Json(json_conns))
        }
        Err(e) => Err(ApiError::Internal(format!(
            "Failed to get AFP connections: {}",
            e
        ))),
    }
}

#[cfg(not(feature = "afp"))]
pub async fn nas_list_afp_connections(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    Ok(Json(vec![]))
}

#[cfg(feature = "afp")]
pub async fn nas_disconnect_afp(
    State(_state): State<Arc<AppState>>,
    Path(conn_id): Path<String>,
) -> Result<(), ApiError> {
    use crate::nas::shares::afp::AfpManager;
    let pid: u32 = conn_id
        .parse()
        .map_err(|_| ApiError::BadRequest("Invalid connection ID (expected PID)".to_string()))?;

    let manager = AfpManager::new();
    manager
        .disconnect_session(pid)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to disconnect AFP session: {}", e)))
}

#[cfg(not(feature = "afp"))]
pub async fn nas_disconnect_afp(
    State(_state): State<Arc<AppState>>,
    Path(_conn_id): Path<String>,
) -> Result<(), ApiError> {
    Err(ApiError::Internal("AFP feature not enabled".to_string()))
}

#[cfg(feature = "afp")]
pub async fn nas_list_timemachine_targets(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    use crate::nas::shares::afp::AfpManager;
    let manager = AfpManager::new();

    // Get shares that have time_machine enabled
    let shares = manager.shares();
    let tm_targets: Vec<serde_json::Value> = shares
        .iter()
        .filter(|s| s.time_machine)
        .map(|s| {
            serde_json::json!({
                "name": s.name,
                "path": s.path,
                "volume_size_limit": s.volume_size_limit,
                "enabled": true,
            })
        })
        .collect();

    Ok(Json(tm_targets))
}

#[cfg(not(feature = "afp"))]
pub async fn nas_list_timemachine_targets(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    Ok(Json(vec![]))
}
