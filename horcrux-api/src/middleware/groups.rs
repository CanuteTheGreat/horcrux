//! User Groups and Permission Inheritance
//!
//! Provides group-based permission management with inheritance:
//! - User groups for easier permission assignment
//! - Permission inheritance from groups to users
//! - Nested groups with hierarchical permissions
//! - Resource pools with delegated access

#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;
use horcrux_common::Result;

// Permission and Privilege structures - duplicated from rbac for now
// In production, these would be exported from rbac module

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Privilege {
    VmAllocate,
    VmConfig,
    VmPowerMgmt,
    VmSnapshot,
    VmBackup,
    VmAudit,
    DatastoreAllocate,
    DatastoreAudit,
    PoolAllocate,
    SysAudit,
    SysAdmin,
}

impl std::fmt::Display for Privilege {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Privilege::VmAllocate => "VmAllocate",
            Privilege::VmConfig => "VmConfig",
            Privilege::VmPowerMgmt => "VmPowerMgmt",
            Privilege::VmSnapshot => "VmSnapshot",
            Privilege::VmBackup => "VmBackup",
            Privilege::VmAudit => "VmAudit",
            Privilege::DatastoreAllocate => "DatastoreAllocate",
            Privilege::DatastoreAudit => "DatastoreAudit",
            Privilege::PoolAllocate => "PoolAllocate",
            Privilege::SysAudit => "SysAudit",
            Privilege::SysAdmin => "SysAdmin",
        };
        write!(f, "{}", s)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Permission {
    pub path: String,
    pub privilege: Privilege,
}

impl Permission {
    /// Check if this permission matches a given path
    pub fn matches(&self, path: &str) -> bool {
        if self.path == "/" {
            return true;
        }

        if self.path.ends_with("/**") {
            let prefix = &self.path[..self.path.len() - 3];
            return path.starts_with(prefix);
        }

        if self.path.ends_with("/*") {
            let prefix = &self.path[..self.path.len() - 2];
            if !path.starts_with(prefix) {
                return false;
            }
            let remainder = &path[prefix.len()..];
            return !remainder.contains('/') || remainder == "/";
        }

        path == self.path
    }

    /// Check if this permission has a specific privilege
    pub fn has_privilege(&self, required: &Privilege) -> bool {
        self.privilege == *required || matches!(self.privilege, Privilege::SysAdmin)
    }
}

/// User group definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserGroup {
    pub id: String,
    pub name: String,
    pub description: String,
    pub permissions: Vec<Permission>,
    pub parent_groups: Vec<String>, // For nested groups
    pub members: Vec<String>,       // User IDs
    pub created_at: i64,
    pub updated_at: i64,
}

/// Resource pool for delegated permissions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourcePool {
    pub id: String,
    pub name: String,
    pub description: String,
    pub resource_type: ResourceType,
    pub resources: Vec<String>,     // Resource IDs
    pub allowed_groups: Vec<String>, // Groups with access
    pub allowed_users: Vec<String>,  // Individual users with access
    pub inherited_permissions: Vec<Permission>,
    pub created_at: i64,
}

/// Resource pool type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ResourceType {
    Vm,
    Container,
    Storage,
    Network,
    All,
}

/// Group manager
pub struct GroupManager {
    groups: Arc<RwLock<HashMap<String, UserGroup>>>,
    pools: Arc<RwLock<HashMap<String, ResourcePool>>>,
}

impl GroupManager {
    pub fn new() -> Self {
        Self {
            groups: Arc::new(RwLock::new(HashMap::new())),
            pools: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a new user group
    pub async fn create_group(&self, mut group: UserGroup) -> Result<UserGroup> {
        let now = chrono::Utc::now().timestamp();
        group.created_at = now;
        group.updated_at = now;

        let mut groups = self.groups.write().await;

        if groups.contains_key(&group.id) {
            return Err(horcrux_common::Error::InvalidConfig(
                format!("Group {} already exists", group.id)
            ));
        }

        groups.insert(group.id.clone(), group.clone());
        tracing::info!("Created user group: {}", group.name);

        Ok(group)
    }

    /// Get a user group
    pub async fn get_group(&self, group_id: &str) -> Result<UserGroup> {
        let groups = self.groups.read().await;
        groups
            .get(group_id)
            .cloned()
            .ok_or_else(|| horcrux_common::Error::System(
                format!("Group {} not found", group_id)
            ))
    }

    /// List all groups
    pub async fn list_groups(&self) -> Vec<UserGroup> {
        let groups = self.groups.read().await;
        groups.values().cloned().collect()
    }

    /// Update a user group
    pub async fn update_group(&self, group: UserGroup) -> Result<UserGroup> {
        let mut groups = self.groups.write().await;

        if !groups.contains_key(&group.id) {
            return Err(horcrux_common::Error::System(
                format!("Group {} not found", group.id)
            ));
        }

        let mut updated_group = group;
        updated_group.updated_at = chrono::Utc::now().timestamp();

        groups.insert(updated_group.id.clone(), updated_group.clone());
        tracing::info!("Updated user group: {}", updated_group.name);

        Ok(updated_group)
    }

    /// Delete a user group
    pub async fn delete_group(&self, group_id: &str) -> Result<()> {
        let mut groups = self.groups.write().await;

        if groups.remove(group_id).is_none() {
            return Err(horcrux_common::Error::System(
                format!("Group {} not found", group_id)
            ));
        }

        tracing::info!("Deleted user group: {}", group_id);
        Ok(())
    }

    /// Add user to a group
    pub async fn add_user_to_group(&self, group_id: &str, user_id: &str) -> Result<()> {
        let mut groups = self.groups.write().await;

        let group = groups.get_mut(group_id)
            .ok_or_else(|| horcrux_common::Error::System(
                format!("Group {} not found", group_id)
            ))?;

        if !group.members.contains(&user_id.to_string()) {
            group.members.push(user_id.to_string());
            group.updated_at = chrono::Utc::now().timestamp();
            tracing::info!("Added user {} to group {}", user_id, group_id);
        }

        Ok(())
    }

    /// Remove user from a group
    pub async fn remove_user_from_group(&self, group_id: &str, user_id: &str) -> Result<()> {
        let mut groups = self.groups.write().await;

        let group = groups.get_mut(group_id)
            .ok_or_else(|| horcrux_common::Error::System(
                format!("Group {} not found", group_id)
            ))?;

        group.members.retain(|id| id != user_id);
        group.updated_at = chrono::Utc::now().timestamp();
        tracing::info!("Removed user {} from group {}", user_id, group_id);

        Ok(())
    }

    /// Get all groups a user belongs to (including nested groups)
    pub async fn get_user_groups(&self, user_id: &str) -> Vec<UserGroup> {
        let groups = self.groups.read().await;
        let mut user_groups = Vec::new();
        let mut visited = HashSet::new();

        // Find direct group memberships
        for group in groups.values() {
            if group.members.contains(&user_id.to_string()) {
                Self::collect_group_hierarchy(group, &groups, &mut user_groups, &mut visited);
            }
        }

        user_groups
    }

    /// Recursively collect group and all parent groups
    fn collect_group_hierarchy(
        group: &UserGroup,
        all_groups: &HashMap<String, UserGroup>,
        result: &mut Vec<UserGroup>,
        visited: &mut HashSet<String>,
    ) {
        // Avoid circular dependencies
        if visited.contains(&group.id) {
            return;
        }

        visited.insert(group.id.clone());
        result.push(group.clone());

        // Recursively collect parent groups
        for parent_id in &group.parent_groups {
            if let Some(parent) = all_groups.get(parent_id) {
                Self::collect_group_hierarchy(parent, all_groups, result, visited);
            }
        }
    }

    /// Get all permissions for a user (from all groups they belong to)
    pub async fn get_user_inherited_permissions(&self, user_id: &str) -> Vec<Permission> {
        let user_groups = self.get_user_groups(user_id).await;
        let mut permissions = Vec::new();

        for group in user_groups {
            permissions.extend(group.permissions);
        }

        // Remove duplicates
        permissions.sort_by(|a, b| {
            a.path.cmp(&b.path)
                .then(a.privilege.to_string().cmp(&b.privilege.to_string()))
        });
        permissions.dedup_by(|a, b| {
            a.path == b.path && a.privilege.to_string() == b.privilege.to_string()
        });

        permissions
    }

    /// Check if user has a specific privilege (considering group inheritance)
    pub async fn check_user_group_privilege(
        &self,
        user_id: &str,
        required_privilege: &Privilege,
        path: &str,
    ) -> bool {
        let permissions = self.get_user_inherited_permissions(user_id).await;

        for permission in permissions {
            if permission.matches(path) && permission.has_privilege(required_privilege) {
                return true;
            }
        }

        false
    }

    /// Create a resource pool
    pub async fn create_pool(&self, mut pool: ResourcePool) -> Result<ResourcePool> {
        pool.created_at = chrono::Utc::now().timestamp();

        let mut pools = self.pools.write().await;

        if pools.contains_key(&pool.id) {
            return Err(horcrux_common::Error::InvalidConfig(
                format!("Pool {} already exists", pool.id)
            ));
        }

        pools.insert(pool.id.clone(), pool.clone());
        tracing::info!("Created resource pool: {}", pool.name);

        Ok(pool)
    }

    /// Get a resource pool
    pub async fn get_pool(&self, pool_id: &str) -> Result<ResourcePool> {
        let pools = self.pools.read().await;
        pools
            .get(pool_id)
            .cloned()
            .ok_or_else(|| horcrux_common::Error::System(
                format!("Pool {} not found", pool_id)
            ))
    }

    /// List all resource pools
    pub async fn list_pools(&self) -> Vec<ResourcePool> {
        let pools = self.pools.read().await;
        pools.values().cloned().collect()
    }

    /// Check if user has access to a resource pool
    pub async fn check_pool_access(&self, pool_id: &str, user_id: &str) -> bool {
        let pools = self.pools.read().await;

        if let Some(pool) = pools.get(pool_id) {
            // Check if user is directly allowed
            if pool.allowed_users.contains(&user_id.to_string()) {
                return true;
            }

            // Check if user belongs to an allowed group
            drop(pools);
            let user_groups = self.get_user_groups(user_id).await;

            let pools = self.pools.read().await;
            if let Some(pool) = pools.get(pool_id) {
                for group in user_groups {
                    if pool.allowed_groups.contains(&group.id) {
                        return true;
                    }
                }
            }
        }

        false
    }

    /// Get all resources in pools accessible by a user
    pub async fn get_user_accessible_resources(&self, user_id: &str, resource_type: &ResourceType) -> Vec<String> {
        let pools = self.list_pools().await;
        let mut resources = Vec::new();

        for pool in pools {
            if (pool.resource_type == *resource_type || pool.resource_type == ResourceType::All)
                && self.check_pool_access(&pool.id, user_id).await
            {
                resources.extend(pool.resources);
            }
        }

        resources.sort();
        resources.dedup();
        resources
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_group() {
        let manager = GroupManager::new();

        let group = UserGroup {
            id: "devs".to_string(),
            name: "Developers".to_string(),
            description: "Development team".to_string(),
            permissions: vec![],
            parent_groups: vec![],
            members: vec![],
            created_at: 0,
            updated_at: 0,
        };

        let result = manager.create_group(group).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_add_user_to_group() {
        let manager = GroupManager::new();

        let group = UserGroup {
            id: "admins".to_string(),
            name: "Administrators".to_string(),
            description: "Admin team".to_string(),
            permissions: vec![],
            parent_groups: vec![],
            members: vec![],
            created_at: 0,
            updated_at: 0,
        };

        manager.create_group(group).await.unwrap();
        manager.add_user_to_group("admins", "user123").await.unwrap();

        let groups = manager.get_user_groups("user123").await;
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].id, "admins");
    }

    #[tokio::test]
    async fn test_nested_groups() {
        let manager = GroupManager::new();

        // Create parent group
        let parent = UserGroup {
            id: "staff".to_string(),
            name: "Staff".to_string(),
            description: "All staff".to_string(),
            permissions: vec![
                Permission {
                    path: "/api/vms/*".to_string(),
                    privilege: Privilege::VmAudit,
                }
            ],
            parent_groups: vec![],
            members: vec![],
            created_at: 0,
            updated_at: 0,
        };

        // Create child group
        let child = UserGroup {
            id: "developers".to_string(),
            name: "Developers".to_string(),
            description: "Development team".to_string(),
            permissions: vec![
                Permission {
                    path: "/api/vms/*".to_string(),
                    privilege: Privilege::VmConfig,
                }
            ],
            parent_groups: vec!["staff".to_string()],
            members: vec!["dev1".to_string()],
            created_at: 0,
            updated_at: 0,
        };

        manager.create_group(parent).await.unwrap();
        manager.create_group(child).await.unwrap();

        // User should inherit permissions from both groups
        let permissions = manager.get_user_inherited_permissions("dev1").await;
        assert_eq!(permissions.len(), 2); // VmAudit from parent, VmConfig from child
    }

    #[tokio::test]
    async fn test_resource_pool() {
        let manager = GroupManager::new();

        let pool = ResourcePool {
            id: "prod-vms".to_string(),
            name: "Production VMs".to_string(),
            description: "Production VM pool".to_string(),
            resource_type: ResourceType::Vm,
            resources: vec!["vm-1".to_string(), "vm-2".to_string()],
            allowed_groups: vec!["admins".to_string()],
            allowed_users: vec![],
            inherited_permissions: vec![],
            created_at: 0,
        };

        let result = manager.create_pool(pool).await;
        assert!(result.is_ok());

        // Create admin group with a user
        let group = UserGroup {
            id: "admins".to_string(),
            name: "Administrators".to_string(),
            description: "Admin team".to_string(),
            permissions: vec![],
            parent_groups: vec![],
            members: vec!["admin1".to_string()],
            created_at: 0,
            updated_at: 0,
        };

        manager.create_group(group).await.unwrap();

        // Check if admin has access
        assert!(manager.check_pool_access("prod-vms", "admin1").await);

        // Get accessible resources
        let resources = manager.get_user_accessible_resources("admin1", &ResourceType::Vm).await;
        assert_eq!(resources.len(), 2);
    }
}
