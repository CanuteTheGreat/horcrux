///! Role-Based Access Control (RBAC)

use horcrux_common::auth::{Privilege, Role, User};
use horcrux_common::Result;
use std::collections::HashMap;

/// RBAC manager
pub struct RbacManager {}

impl RbacManager {
    pub fn new() -> Self {
        Self {}
    }

    /// Check if user has permission to access resource
    pub async fn check_permission(
        &self,
        user: &User,
        roles: &HashMap<String, Role>,
        resource_path: &str,
        required_privilege: Privilege,
    ) -> Result<bool> {
        // Check all user's roles
        for role_name in &user.roles {
            if let Some(role) = roles.get(role_name) {
                for permission in &role.permissions {
                    // Check if resource path matches permission path
                    if self.path_matches(&permission.path, resource_path) {
                        // Check if privilege is granted
                        if permission.privileges.contains(&required_privilege) {
                            return Ok(true);
                        }
                    }
                }
            }
        }

        Ok(false)
    }

    /// Check if resource path matches permission path
    /// Supports wildcards: /vms/* matches /vms/100, /vms/101, etc.
    fn path_matches(&self, permission_path: &str, resource_path: &str) -> bool {
        // Exact match
        if permission_path == resource_path {
            return true;
        }

        // Root path matches everything
        if permission_path == "/" {
            return true;
        }

        // Wildcard match
        if permission_path.ends_with("/*") {
            let prefix = &permission_path[..permission_path.len() - 2];
            if resource_path.starts_with(prefix) {
                return true;
            }
        }

        // Recursive wildcard
        if permission_path.ends_with("/**") {
            let prefix = &permission_path[..permission_path.len() - 3];
            if resource_path.starts_with(prefix) {
                return true;
            }
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_matching() {
        let rbac = RbacManager::new();

        assert!(rbac.path_matches("/", "/vms/100"));
        assert!(rbac.path_matches("/vms/100", "/vms/100"));
        assert!(rbac.path_matches("/vms/*", "/vms/100"));
        assert!(rbac.path_matches("/vms/**", "/vms/100/disks/0"));
        assert!(!rbac.path_matches("/vms/100", "/vms/101"));
        assert!(!rbac.path_matches("/vms/*", "/containers/100"));
    }
}
