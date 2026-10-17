//! Authentication and User Management Pages
//!
//! Users, roles, groups and permission grants, sessions, API keys, second
//! factors, authentication realms and the HTTPS certificate.

pub mod access;
pub mod api_keys;
pub mod certificates;
pub mod mfa;
pub mod roles;
pub mod sessions;
pub mod user_menu;
pub mod users;

// Re-export components for easy access
pub use access::{PermissionsPage, RealmsPage};
pub use api_keys::ApiKeysPage;
pub use certificates::CertificatesPage;
pub use mfa::MfaPage;
pub use roles::RolesPage;
pub use sessions::SessionsPage;
pub use user_menu::UserMenu;
pub use users::UsersPage;
