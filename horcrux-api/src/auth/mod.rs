///! Authentication and authorization module

pub mod pam;
pub mod ldap;
pub mod session;
pub mod rbac;
pub mod oidc;
pub mod password;

use horcrux_common::auth::*;
use horcrux_common::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Authentication manager
pub struct AuthManager {
    users: Arc<RwLock<HashMap<String, User>>>,
    realms: Arc<RwLock<HashMap<String, Realm>>>,
    roles: Arc<RwLock<HashMap<String, Role>>>,
    sessions: Arc<RwLock<HashMap<String, Session>>>,
    api_tokens: Arc<RwLock<HashMap<String, ApiToken>>>,
    pam: pam::PamAuthenticator,
    ldap: ldap::LdapAuthenticator,
    session_manager: session::SessionManager,
    rbac: rbac::RbacManager,
}

impl AuthManager {
    pub fn new() -> Self {
        let mut roles = HashMap::new();

        // Define built-in roles
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
                        Privilege::DatastoreAllocate,
                        Privilege::SysModify,
                        Privilege::UserModify,
                        Privilege::PermissionsModify,
                    ],
                }],
            },
        );

        roles.insert(
            "PVEVMUser".to_string(),
            Role {
                name: "PVEVMUser".to_string(),
                description: "VM user (console access only)".to_string(),
                permissions: vec![Permission {
                    path: "/vms".to_string(),
                    privileges: vec![Privilege::VmConsole, Privilege::VmAudit],
                }],
            },
        );

        roles.insert(
            "PVEAdmin".to_string(),
            Role {
                name: "PVEAdmin".to_string(),
                description: "VM administrator".to_string(),
                permissions: vec![Permission {
                    path: "/vms".to_string(),
                    privileges: vec![
                        Privilege::VmAllocate,
                        Privilege::VmConfig,
                        Privilege::VmPowerMgmt,
                        Privilege::VmConsole,
                        Privilege::VmSnapshot,
                        Privilege::VmBackup,
                    ],
                }],
            },
        );

        Self {
            users: Arc::new(RwLock::new(HashMap::new())),
            realms: Arc::new(RwLock::new(HashMap::new())),
            roles: Arc::new(RwLock::new(roles)),
            sessions: Arc::new(RwLock::new(HashMap::new())),
            api_tokens: Arc::new(RwLock::new(HashMap::new())),
            pam: pam::PamAuthenticator::new(),
            ldap: ldap::LdapAuthenticator::new(),
            session_manager: session::SessionManager::new(),
            rbac: rbac::RbacManager::new(),
        }
    }

    /// Authenticate user and create session
    pub async fn login(&self, request: LoginRequest) -> Result<LoginResponse> {
        let realm = request.realm.unwrap_or_else(|| "pam".to_string());

        // Authenticate based on realm
        let authenticated = match realm.as_str() {
            "pam" => self.pam.authenticate(&request.username, &request.password).await?,
            "ldap" => {
                let realms = self.realms.read().await;
                let realm_config = realms
                    .get("ldap")
                    .ok_or_else(|| horcrux_common::Error::System("LDAP realm not configured".to_string()))?;

                if let RealmConfig::Ldap(config) = &realm_config.config {
                    self.ldap.authenticate(&request.username, &request.password, config).await?
                } else {
                    false
                }
            }
            _ => {
                return Err(horcrux_common::Error::InvalidConfig(format!(
                    "Unknown realm: {}",
                    realm
                )))
            }
        };

        if !authenticated {
            return Err(horcrux_common::Error::System("Authentication failed".to_string()));
        }

        // Get user info
        let users = self.users.read().await;
        let user_key = format!("{}@{}", request.username, realm);
        let user = users
            .get(&user_key)
            .ok_or_else(|| horcrux_common::Error::System("User not found".to_string()))?;

        if !user.enabled {
            return Err(horcrux_common::Error::System("User account is disabled".to_string()));
        }

        // Create session
        let session = self.session_manager.create_session(&user.id, &user.username, &realm).await;
        let csrf_token = self.session_manager.generate_csrf_token();

        let mut sessions = self.sessions.write().await;
        sessions.insert(session.session_id.clone(), session.clone());

        Ok(LoginResponse {
            ticket: session.session_id,
            csrf_token,
            username: user.username.clone(),
            roles: user.roles.clone(),
        })
    }

    /// Validate session and check permissions
    pub async fn check_permission(
        &self,
        session_id: &str,
        resource_path: &str,
        required_privilege: Privilege,
    ) -> Result<bool> {
        // Validate session
        let sessions = self.sessions.read().await;
        let session = sessions
            .get(session_id)
            .ok_or_else(|| horcrux_common::Error::System("Invalid session".to_string()))?;

        // Check if session expired
        let now = chrono::Utc::now().timestamp();
        if session.expires < now {
            return Err(horcrux_common::Error::System("Session expired".to_string()));
        }

        // Get user
        let users = self.users.read().await;
        let user_key = format!("{}@{}", session.username, session.realm);
        let user = users
            .get(&user_key)
            .ok_or_else(|| horcrux_common::Error::System("User not found".to_string()))?;

        // Check permissions via RBAC
        let roles = self.roles.read().await;
        self.rbac
            .check_permission(user, &roles, resource_path, required_privilege)
            .await
    }

    /// Add a new user
    pub async fn add_user(&self, user: User) -> Result<()> {
        let mut users = self.users.write().await;
        let key = format!("{}@{}", user.username, user.realm);

        if users.contains_key(&key) {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "User {} already exists",
                key
            )));
        }

        users.insert(key, user);
        Ok(())
    }

    /// Remove a user
    pub async fn remove_user(&self, username: &str, realm: &str) -> Result<()> {
        let mut users = self.users.write().await;
        let key = format!("{}@{}", username, realm);

        if users.remove(&key).is_none() {
            return Err(horcrux_common::Error::System(format!("User {} not found", key)));
        }

        Ok(())
    }

    /// List all users
    pub async fn list_users(&self) -> Vec<User> {
        let users = self.users.read().await;
        users.values().cloned().collect()
    }

    /// Add a realm
    pub async fn add_realm(&self, realm: Realm) -> Result<()> {
        let mut realms = self.realms.write().await;

        if realms.contains_key(&realm.name) {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Realm {} already exists",
                realm.name
            )));
        }

        realms.insert(realm.name.clone(), realm);
        Ok(())
    }

    /// List all realms
    pub async fn list_realms(&self) -> Vec<Realm> {
        let realms = self.realms.read().await;
        realms.values().cloned().collect()
    }

    /// Create API token
    pub async fn create_api_token(&self, user: &str, comment: Option<String>) -> Result<ApiToken> {
        let token_id = uuid::Uuid::new_v4().to_string();

        let token = ApiToken {
            id: token_id.clone(),
            user: user.to_string(),
            enabled: true,
            expire: None,
            comment,
        };

        let mut tokens = self.api_tokens.write().await;
        tokens.insert(token_id, token.clone());

        Ok(token)
    }

    /// Validate API token
    pub async fn validate_api_token(&self, token_id: &str) -> Result<String> {
        let tokens = self.api_tokens.read().await;
        let token = tokens
            .get(token_id)
            .ok_or_else(|| horcrux_common::Error::System("Invalid API token".to_string()))?;

        if !token.enabled {
            return Err(horcrux_common::Error::System("API token is disabled".to_string()));
        }

        if let Some(expire) = token.expire {
            let now = chrono::Utc::now().timestamp();
            if expire < now {
                return Err(horcrux_common::Error::System("API token expired".to_string()));
            }
        }

        Ok(token.user.clone())
    }

    /// Logout - invalidate session
    pub async fn logout(&self, session_id: &str) -> Result<()> {
        let mut sessions = self.sessions.write().await;
        sessions.remove(session_id);
        Ok(())
    }

    /// Verify session is valid
    pub async fn verify_session(&self, session_id: &str) -> Result<bool> {
        let sessions = self.sessions.read().await;
        if let Some(session) = sessions.get(session_id) {
            let now = chrono::Utc::now().timestamp();
            Ok(session.expires > now)
        } else {
            Ok(false)
        }
    }

    /// Delete user by ID (username@realm format)
    pub async fn delete_user(&self, user_id: &str) -> Result<()> {
        let mut users = self.users.write().await;
        if users.remove(user_id).is_none() {
            return Err(horcrux_common::Error::System(format!("User {} not found", user_id)));
        }
        Ok(())
    }

    /// List all roles
    pub async fn list_roles(&self) -> Vec<Role> {
        let roles = self.roles.read().await;
        roles.values().cloned().collect()
    }

    /// Get user permissions
    pub async fn get_user_permissions(&self, user_id: &str) -> Vec<Permission> {
        let users = self.users.read().await;
        if let Some(user) = users.get(user_id) {
            let roles = self.roles.blocking_read();
            user.roles
                .iter()
                .filter_map(|role_name| roles.get(role_name))
                .flat_map(|role| role.permissions.clone())
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Add permission to user
    pub async fn add_permission(&self, user_id: &str, permission: Permission) -> Result<()> {
        // For simplicity, we'll add permissions by creating/updating a custom role per user
        let mut roles = self.roles.write().await;
        let role_name = format!("custom-{}", user_id);

        roles
            .entry(role_name.clone())
            .and_modify(|role| role.permissions.push(permission.clone()))
            .or_insert_with(|| Role {
                name: role_name.clone(),
                description: format!("Custom role for {}", user_id),
                permissions: vec![permission],
            });

        // Add role to user
        let mut users = self.users.write().await;
        if let Some(user) = users.get_mut(user_id) {
            if !user.roles.contains(&role_name) {
                user.roles.push(role_name);
            }
        }

        Ok(())
    }
}
