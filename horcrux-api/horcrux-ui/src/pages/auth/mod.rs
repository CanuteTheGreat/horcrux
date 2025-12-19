//! Authentication and User Management Pages
//!
//! This module contains all authentication-related pages for the Horcrux web UI.
//! Provides comprehensive user management, role-based access control (RBAC),
//! session monitoring, API key management, and MFA configuration capabilities.

pub mod users;
pub mod roles;
pub mod sessions;
pub mod api_keys;
pub mod mfa;

// Re-export components for easy access
pub use users::UsersPage;
pub use roles::RolesPage;
pub use sessions::SessionsPage;
pub use api_keys::ApiKeysPage;
