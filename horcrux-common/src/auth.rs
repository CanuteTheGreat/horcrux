//! Authentication and authorization types
use serde::{Deserialize, Serialize};

/// User account
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
    /// Argon2 hash. Never serialized back out (user lists used to return it).
    #[serde(default, skip_serializing)]
    pub password_hash: String,
    pub email: String,
    pub role: String,  // Single primary role
    pub realm: String, // pam, ldap, ad, etc.
    pub enabled: bool,
    #[serde(default)]
    pub roles: Vec<String>, // Additional roles for compatibility
    pub comment: Option<String>,
}

/// Authentication realm type
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RealmType {
    Pam,    // Linux PAM
    Ldap,   // LDAP server
    Ad,     // Active Directory
    OpenId, // OpenID Connect
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
    pub path: String, // e.g., "/vms/100", "/storage/zfs-pool"
    pub privileges: Vec<Privilege>,
}

/// Privilege (Proxmox VE naming: `VM.Audit`, `Sys.Modify`, ...).
///
/// Serialized with the PVE-style dotted name; the older PascalCase names
/// (`VmAudit`) are still accepted on input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Privilege {
    // VM / container privileges
    #[serde(rename = "VM.Audit", alias = "VmAudit")]
    VmAudit, // View VM
    #[serde(rename = "VM.Console", alias = "VmConsole")]
    VmConsole, // Access console
    #[serde(rename = "VM.Config", alias = "VmConfig")]
    VmConfig, // Modify config
    #[serde(rename = "VM.PowerMgmt", alias = "VmPowerMgmt")]
    VmPowerMgmt, // Start/stop/reboot
    #[serde(rename = "VM.Allocate", alias = "VmAllocate")]
    VmAllocate, // Create/delete VM
    #[serde(rename = "VM.Migrate", alias = "VmMigrate")]
    VmMigrate, // Migrate VM
    #[serde(rename = "VM.Snapshot", alias = "VmSnapshot")]
    VmSnapshot, // Create snapshots
    #[serde(rename = "VM.Backup", alias = "VmBackup")]
    VmBackup, // Backup VM
    #[serde(rename = "VM.Clone", alias = "VmClone")]
    VmClone, // Clone VM / container

    // Storage privileges
    #[serde(rename = "Datastore.Audit", alias = "DatastoreAudit")]
    DatastoreAudit,
    #[serde(rename = "Datastore.Allocate", alias = "DatastoreAllocate")]
    DatastoreAllocate,
    #[serde(rename = "Datastore.AllocateSpace", alias = "DatastoreAllocateSpace")]
    DatastoreAllocateSpace,

    // Pool privileges
    #[serde(rename = "Pool.Audit", alias = "PoolAudit")]
    PoolAudit,
    #[serde(rename = "Pool.Allocate", alias = "PoolAllocate")]
    PoolAllocate,

    // System privileges
    #[serde(rename = "Sys.Audit", alias = "SysAudit")]
    SysAudit,
    #[serde(rename = "Sys.Modify", alias = "SysModify")]
    SysModify,
    #[serde(rename = "Sys.Console", alias = "SysConsole")]
    SysConsole,

    // Network (SDN, firewall, container networks)
    #[serde(rename = "SDN.Audit", alias = "SdnAudit")]
    SdnAudit,
    #[serde(rename = "SDN.Allocate", alias = "SdnAllocate")]
    SdnAllocate,

    // User privileges
    #[serde(rename = "User.Modify", alias = "UserModify")]
    UserModify,
    #[serde(rename = "Permissions.Modify", alias = "PermissionsModify")]
    PermissionsModify,
}

impl Privilege {
    /// Every privilege, in a stable order.
    pub const ALL: [Privilege; 21] = [
        Privilege::VmAudit,
        Privilege::VmConsole,
        Privilege::VmConfig,
        Privilege::VmPowerMgmt,
        Privilege::VmAllocate,
        Privilege::VmMigrate,
        Privilege::VmSnapshot,
        Privilege::VmBackup,
        Privilege::VmClone,
        Privilege::DatastoreAudit,
        Privilege::DatastoreAllocate,
        Privilege::DatastoreAllocateSpace,
        Privilege::PoolAudit,
        Privilege::PoolAllocate,
        Privilege::SysAudit,
        Privilege::SysModify,
        Privilege::SysConsole,
        Privilege::SdnAudit,
        Privilege::SdnAllocate,
        Privilege::UserModify,
        Privilege::PermissionsModify,
    ];

    /// The PVE-style name (`VM.Audit`), same as the serialized form.
    pub fn name(&self) -> &'static str {
        match self {
            Privilege::VmAudit => "VM.Audit",
            Privilege::VmConsole => "VM.Console",
            Privilege::VmConfig => "VM.Config",
            Privilege::VmPowerMgmt => "VM.PowerMgmt",
            Privilege::VmAllocate => "VM.Allocate",
            Privilege::VmMigrate => "VM.Migrate",
            Privilege::VmSnapshot => "VM.Snapshot",
            Privilege::VmBackup => "VM.Backup",
            Privilege::VmClone => "VM.Clone",
            Privilege::DatastoreAudit => "Datastore.Audit",
            Privilege::DatastoreAllocate => "Datastore.Allocate",
            Privilege::DatastoreAllocateSpace => "Datastore.AllocateSpace",
            Privilege::PoolAudit => "Pool.Audit",
            Privilege::PoolAllocate => "Pool.Allocate",
            Privilege::SysAudit => "Sys.Audit",
            Privilege::SysModify => "Sys.Modify",
            Privilege::SysConsole => "Sys.Console",
            Privilege::SdnAudit => "SDN.Audit",
            Privilege::SdnAllocate => "SDN.Allocate",
            Privilege::UserModify => "User.Modify",
            Privilege::PermissionsModify => "Permissions.Modify",
        }
    }

    /// Parse either naming style (`VM.Audit` or `VmAudit`).
    pub fn parse(s: &str) -> Option<Privilege> {
        Privilege::ALL
            .iter()
            .copied()
            .find(|p| p.name() == s || format!("{:?}", p) == s)
    }
}

/// API token for authentication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiToken {
    pub id: String,
    pub user: String,
    pub enabled: bool,
    pub expire: Option<i64>, // Unix timestamp
    pub comment: Option<String>,
}

/// Session information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub user_id: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    // Legacy fields for compatibility
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub realm: String,
    #[serde(default)]
    pub created: i64,
    #[serde(default)]
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

/// Returned by `/api/auth/login` instead of a [`LoginResponse`] when the
/// password was right but a second factor is still needed. Finish with
/// `POST /api/auth/tfa` (`ticket` + one of `methods`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TfaChallenge {
    pub tfa_required: bool,
    pub ticket: String,
    pub username: String,
    /// Any of `totp`, `webauthn`, `recovery`.
    pub methods: Vec<String>,
    /// WebAuthn `PublicKeyCredentialRequestOptions` (base64url fields) when
    /// the user has a security key registered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webauthn: Option<serde_json::Value>,
}
