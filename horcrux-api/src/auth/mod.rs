//! Authentication and authorization.
//!
//! - [`store`]: the database tables behind users, roles, groups, ACLs,
//!   realms and second factors (the single user store).
//! - [`privileges`]: which privilege each API route needs, and the built-in roles.
//! - [`acl`]: Proxmox-style ACL evaluation.
//! - [`realms`]: password checks against PAM / LDAP / Active Directory, and
//!   OpenID Connect ([`oidc`]).
//! - [`totp`], [`webauthn`]: second factors (plus one-time recovery codes).
//! - [`password`]: Argon2 password hashing and API key generation.
pub mod acl;
pub mod oidc;
pub mod password;
pub mod privileges;
pub mod realms;
pub mod store;
pub mod totp;
pub mod webauthn;

use rand::Rng;

/// A random URL-safe token of `len` characters.
pub fn random_token(len: usize) -> String {
    const ALPHA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| ALPHA[rng.gen_range(0..ALPHA.len())] as char)
        .collect()
}

/// Whether public self-registration is switched on. Off unless the operator
/// sets `HORCRUX_ALLOW_REGISTRATION=true` (or `[auth] allow_registration`
/// in the config file). Self-registered accounts get no permissions.
pub fn registration_enabled(config_flag: bool) -> bool {
    match std::env::var("HORCRUX_ALLOW_REGISTRATION") {
        Ok(v) => matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => config_flag,
    }
}

/// Generate a strong initial password for the bootstrap administrator.
pub fn generate_initial_password() -> String {
    random_token(24)
}
