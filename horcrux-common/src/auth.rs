///! Authentication and authorization types

use serde::{Deserialize, Serialize};

/// User account
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub username: String,
    pub realm: String,  // pam, ldap, ad, etc.
    pub enabled: bool,
    pub roles: Vec<String>,
    pub email: Option<String>,
    pub comment: Option<String>,
}

/// Authentication realm type
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RealmType {
    Pam,        // Linux PAM
    Ldap,       // LDAP server
    Ad,         // Active Directory
    OpenId,     // OpenID Connect
}

/// Authentication realm configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Realm {
    pub name: String,
    pub realm_type: RealmType,
    pub enabled: bool,
    pub config: RealmConfig,
}

/// Realm-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RealmConfig {
    Pam(PamConfig),
    Ldap(LdapConfig),
    Ad(AdConfig),
    OpenId(OpenIdConfig),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PamConfig {
    pub default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LdapConfig {
    pub server: String,
    pub port: u16,
    pub base_dn: String,
    pub user_attr: String,
    pub bind_dn: Option<String>,
    pub bind_password: Option<String>,
    pub use_ssl: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdConfig {
    pub server: String,
    pub port: u16,
    pub domain: String,
    pub use_ssl: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenIdConfig {
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret: String,
}

/// Role definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Role {
    pub name: String,
    pub description: String,
    pub permissions: Vec<Permission>,
}

/// Permission for resources
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Permission {
    pub path: String,       // e.g., "/vms/100", "/storage/zfs-pool"
    pub privileges: Vec<Privilege>,
}

/// Privilege level
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum Privilege {
    // VM privileges
    VmAudit,        // View VM
    VmConsole,      // Access console
    VmConfig,       // Modify config
    VmPowerMgmt,    // Start/stop/reboot
    VmAllocate,     // Create/delete VM
    VmMigrate,      // Migrate VM
    VmSnapshot,     // Create snapshots
    VmBackup,       // Backup VM

    // Storage privileges
    DatastoreAudit,
    DatastoreAllocate,
    DatastoreAllocateSpace,

    // Pool privileges
    PoolAudit,
    PoolAllocate,

    // System privileges
    SysAudit,
    SysModify,
    SysConsole,

    // User privileges
    UserModify,
    PermissionsModify,
}

/// API token for authentication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiToken {
    pub id: String,
    pub user: String,
    pub enabled: bool,
    pub expire: Option<i64>,  // Unix timestamp
    pub comment: Option<String>,
}

/// Session information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub session_id: String,
    pub username: String,
    pub realm: String,
    pub created: i64,
    pub expires: i64,
}

/// Login request
#[derive(Debug, Clone, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    pub realm: Option<String>,
}

/// Login response
#[derive(Debug, Clone, Serialize)]
pub struct LoginResponse {
    pub ticket: String,
    pub csrf_token: String,
    pub username: String,
    pub roles: Vec<String>,
}
