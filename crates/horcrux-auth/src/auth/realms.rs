//! Authentication realms: where a password (or an OpenID Connect login) is
//! checked. `local` is the Horcrux user database itself; `pam` checks host
//! accounts through Linux-PAM; `ldap` and `ad` bind against a directory;
//! `openid` redirects to an OpenID Connect provider.
//!
//! Users from external realms are kept in the same `users` table as
//! `<name>@<realm>` (like Proxmox VE's `alice@ldap`), created on first
//! successful login, so ACLs, groups, second factors and sessions work the
//! same for every realm. Their password hash column holds an unusable value:
//! the realm, not Horcrux, checks the password.

use super::store::RealmRecord;
use horcrux_common::{Error, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const REALM_TYPES: &[&str] = &["pam", "ldap", "ad", "openid"];

/// Password-hash placeholder for externally authenticated users. Not a valid
/// Argon2 PHC string, so a local password check can never succeed with it.
pub const EXTERNAL_PASSWORD: &str = "!external";

/// LDAP / Active Directory realm settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LdapRealmConfig {
    pub server: String,
    #[serde(default)]
    pub port: Option<u16>,
    /// `ldap` (plain), `starttls` or `ldaps`.
    #[serde(default = "default_mode")]
    pub mode: String,
    /// Verify the server certificate (on by default).
    #[serde(default = "yes")]
    pub verify_tls: bool,
    /// Extra trusted CA (PEM file path) for a private directory CA.
    #[serde(default)]
    pub ca_cert: Option<String>,
    /// Search base for users, e.g. `ou=people,dc=example,dc=org`.
    #[serde(default)]
    pub base_dn: String,
    /// Attribute holding the login name (`uid`; AD: `sAMAccountName`).
    #[serde(default)]
    pub user_attr: Option<String>,
    /// Extra LDAP filter users must match, e.g. `(memberOf=cn=hx,ou=groups,...)`.
    #[serde(default)]
    pub filter: Option<String>,
    /// Service account for the user search (anonymous search if unset).
    #[serde(default)]
    pub bind_dn: Option<String>,
    #[serde(default)]
    pub bind_password: Option<String>,
    /// AD only: the domain for `user@domain` binds.
    #[serde(default)]
    pub domain: Option<String>,
}

fn default_mode() -> String {
    "ldap".into()
}
fn yes() -> bool {
    true
}

/// OpenID Connect realm settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenIdRealmConfig {
    pub issuer_url: String,
    pub client_id: String,
    #[serde(default)]
    pub client_secret: String,
    /// Callback registered at the provider. Defaults to
    /// `https://<host the browser used>/api/auth/oidc/callback`.
    #[serde(default)]
    pub redirect_uri: Option<String>,
    #[serde(default)]
    pub scopes: Option<Vec<String>>,
    /// Claim used as the Horcrux user name: `sub` (default, like Proxmox
    /// VE: the one claim users can't change at most providers), `email`,
    /// `preferred_username`, or any other string claim.
    #[serde(default)]
    pub username_claim: Option<String>,
    /// Create the user on first login (default true). When false, an
    /// administrator must create `<name>@<realm>` first.
    #[serde(default = "yes")]
    pub autocreate: bool,
}

/// Validate a realm definition before it's stored.
pub fn validate(realm: &RealmRecord) -> Result<()> {
    if realm.name.is_empty()
        || realm.name == "local"
        || !realm
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(Error::Validation(
            "realm names are letters, digits, - and _ (and 'local' is reserved)".into(),
        ));
    }
    match realm.realm_type.as_str() {
        "pam" => {
            if !cfg!(feature = "pam") {
                return Err(Error::Validation(
                    "this build has no PAM support (USE=pam / cargo feature `pam`)".into(),
                ));
            }
        }
        "ldap" | "ad" => {
            let c: LdapRealmConfig = serde_json::from_value(realm.config.clone()).map_err(|e| {
                Error::Validation(format!("invalid {} settings: {}", realm.realm_type, e))
            })?;
            if c.server.trim().is_empty() {
                return Err(Error::Validation("server is required".into()));
            }
            if !["ldap", "starttls", "ldaps"].contains(&c.mode.as_str()) {
                return Err(Error::Validation(
                    "mode must be ldap, starttls or ldaps".into(),
                ));
            }
            if realm.realm_type == "ldap" && c.base_dn.trim().is_empty() {
                return Err(Error::Validation("base_dn is required".into()));
            }
            if realm.realm_type == "ad" && c.domain.as_deref().unwrap_or("").is_empty() {
                return Err(Error::Validation(
                    "domain is required for Active Directory".into(),
                ));
            }
        }
        "openid" => {
            let c: OpenIdRealmConfig = serde_json::from_value(realm.config.clone())
                .map_err(|e| Error::Validation(format!("invalid openid settings: {}", e)))?;
            if !(c.issuer_url.starts_with("https://") || c.issuer_url.starts_with("http://")) {
                return Err(Error::Validation(
                    "issuer_url must be an http(s) URL".into(),
                ));
            }
            if c.client_id.is_empty() {
                return Err(Error::Validation("client_id is required".into()));
            }
        }
        other => {
            return Err(Error::Validation(format!(
                "unknown realm type '{}' (expected one of {:?})",
                other, REALM_TYPES
            )))
        }
    }
    Ok(())
}

/// Hide secrets when a realm is shown through the API.
pub fn redact(mut realm: RealmRecord) -> RealmRecord {
    if let Some(obj) = realm.config.as_object_mut() {
        for k in ["bind_password", "client_secret"] {
            if obj.get(k).map(|v| !v.is_null() && v != "").unwrap_or(false) {
                obj.insert(k.into(), serde_json::Value::String("********".into()));
            }
        }
    }
    realm
}

/// When an update sends the redacted placeholder back, keep the stored secret.
pub fn merge_secrets(new: &mut RealmRecord, old: &RealmRecord) {
    if let (Some(n), Some(o)) = (new.config.as_object_mut(), old.config.as_object()) {
        for k in ["bind_password", "client_secret"] {
            if n.get(k).and_then(|v| v.as_str()) == Some("********") {
                if let Some(v) = o.get(k) {
                    n.insert(k.into(), v.clone());
                }
            }
        }
    }
}

/// Result of a successful external password check.
#[derive(Debug, Clone)]
pub struct ExternalIdentity {
    pub email: Option<String>,
    pub display: Option<String>,
}

/// Check a password against an `ldap`, `ad` or `pam` realm.
pub async fn authenticate_password(
    realm: &RealmRecord,
    username: &str,
    password: &str,
) -> Result<Option<ExternalIdentity>> {
    if password.is_empty() || username.is_empty() {
        // an empty password is an *unauthenticated* LDAP bind, which succeeds
        return Ok(None);
    }
    match realm.realm_type.as_str() {
        "ldap" | "ad" => {
            let cfg: LdapRealmConfig = serde_json::from_value(realm.config.clone())
                .map_err(|e| Error::InvalidConfig(format!("realm {}: {}", realm.name, e)))?;
            ldap_authenticate(&cfg, realm.realm_type == "ad", username, password).await
        }
        "pam" => pam_authenticate(username, password).await,
        _ => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// LDAP / AD
// ---------------------------------------------------------------------------

fn ldap_url(cfg: &LdapRealmConfig) -> String {
    let scheme = if cfg.mode == "ldaps" { "ldaps" } else { "ldap" };
    let port = cfg
        .port
        .unwrap_or(if cfg.mode == "ldaps" { 636 } else { 389 });
    let host = if cfg.server.contains(':') && !cfg.server.starts_with('[') {
        format!("[{}]", cfg.server)
    } else {
        cfg.server.clone()
    };
    format!("{}://{}:{}", scheme, host, port)
}

fn ldap_settings(cfg: &LdapRealmConfig) -> Result<ldap3::LdapConnSettings> {
    let mut s = ldap3::LdapConnSettings::new()
        .set_conn_timeout(Duration::from_secs(10))
        .set_starttls(cfg.mode == "starttls");
    if cfg.mode != "ldap" {
        if !cfg.verify_tls {
            s = s.set_no_tls_verify(true);
        } else if let Some(ca) = &cfg.ca_cert {
            let pem = std::fs::read(ca)
                .map_err(|e| Error::InvalidConfig(format!("reading CA {}: {}", ca, e)))?;
            let mut roots = rustls::RootCertStore::empty();
            for c in rustls_pemfile::certs(&mut &pem[..]).flatten() {
                roots
                    .add(c)
                    .map_err(|e| Error::InvalidConfig(format!("CA {}: {}", ca, e)))?;
            }
            let tls = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .map_err(|e| Error::System(format!("rustls: {}", e)))?
            .with_root_certificates(roots)
            .with_no_client_auth();
            s = s.set_config(std::sync::Arc::new(tls));
        }
    }
    Ok(s)
}

async fn ldap_connect(cfg: &LdapRealmConfig) -> Result<ldap3::Ldap> {
    let (conn, ldap) = ldap3::LdapConnAsync::with_settings(ldap_settings(cfg)?, &ldap_url(cfg))
        .await
        .map_err(|e| Error::System(format!("LDAP connect to {}: {}", ldap_url(cfg), e)))?;
    ldap3::drive!(conn);
    Ok(ldap)
}

/// Search-then-bind: find the user's DN (as the service account, or
/// anonymously), then bind as that DN with the user's password. AD realms
/// bind directly as `user@domain` and read `mail` from the user object.
pub async fn ldap_authenticate(
    cfg: &LdapRealmConfig,
    is_ad: bool,
    username: &str,
    password: &str,
) -> Result<Option<ExternalIdentity>> {
    if username.chars().any(|c| {
        c.is_control()
            || matches!(
                c,
                '*' | '(' | ')' | '\\' | ',' | '=' | '+' | '<' | '>' | ';' | '"'
            )
    }) {
        return Ok(None);
    }
    let user_attr = cfg.user_attr.clone().unwrap_or_else(|| {
        if is_ad {
            "sAMAccountName".into()
        } else {
            "uid".into()
        }
    });
    let mut filter = format!("({}={})", user_attr, ldap3::ldap_escape(username));
    if let Some(extra) = cfg.filter.as_deref().filter(|f| !f.trim().is_empty()) {
        filter = format!("(&{}{})", filter, extra);
    }

    if is_ad {
        let upn = format!("{}@{}", username, cfg.domain.clone().unwrap_or_default());
        let mut ldap = ldap_connect(cfg).await?;
        let ok = ldap
            .simple_bind(&upn, password)
            .await
            .map(|r| r.rc == 0)
            .unwrap_or(false);
        if !ok {
            let _ = ldap.unbind().await;
            return Ok(None);
        }
        let mut ident = ExternalIdentity {
            email: None,
            display: None,
        };
        if !cfg.base_dn.is_empty() {
            if let Ok(res) = ldap
                .search(
                    &cfg.base_dn,
                    ldap3::Scope::Subtree,
                    &filter,
                    vec!["mail", "displayName"],
                )
                .await
            {
                if let Ok((entries, _)) = res.success() {
                    if entries.is_empty() {
                        // bound fine but outside the configured base/filter
                        let _ = ldap.unbind().await;
                        return Ok(None);
                    }
                    let e = ldap3::SearchEntry::construct(entries[0].clone());
                    ident.email = e.attrs.get("mail").and_then(|v| v.first().cloned());
                    ident.display = e.attrs.get("displayName").and_then(|v| v.first().cloned());
                }
            }
        }
        let _ = ldap.unbind().await;
        return Ok(Some(ident));
    }

    // 1. find the DN
    let mut ldap = ldap_connect(cfg).await?;
    if let (Some(dn), Some(pw)) = (&cfg.bind_dn, &cfg.bind_password) {
        let r = ldap
            .simple_bind(dn, pw)
            .await
            .map_err(|e| Error::System(format!("LDAP service bind: {}", e)))?;
        if r.rc != 0 {
            return Err(Error::InvalidConfig(format!(
                "LDAP service account bind failed: {}",
                r.text
            )));
        }
    }
    let (entries, _res) = ldap
        .search(
            &cfg.base_dn,
            ldap3::Scope::Subtree,
            &filter,
            vec!["mail", "cn"],
        )
        .await
        .map_err(|e| Error::System(format!("LDAP search: {}", e)))?
        .success()
        .map_err(|e| Error::System(format!("LDAP search: {}", e)))?;
    let _ = ldap.unbind().await;
    if entries.len() != 1 {
        return Ok(None);
    }
    let entry = ldap3::SearchEntry::construct(entries.into_iter().next().unwrap());

    // 2. bind as the user
    let mut ldap = ldap_connect(cfg).await?;
    let ok = ldap
        .simple_bind(&entry.dn, password)
        .await
        .map(|r| r.rc == 0)
        .unwrap_or(false);
    let _ = ldap.unbind().await;
    if !ok {
        return Ok(None);
    }
    Ok(Some(ExternalIdentity {
        email: entry.attrs.get("mail").and_then(|v| v.first().cloned()),
        display: entry.attrs.get("cn").and_then(|v| v.first().cloned()),
    }))
}

/// Connectivity check used by the realm "Test" button: connect (and bind
/// with the service account if one is configured).
pub async fn ldap_test(cfg: &LdapRealmConfig) -> Result<String> {
    let mut ldap = ldap_connect(cfg).await?;
    if let (Some(dn), Some(pw)) = (&cfg.bind_dn, &cfg.bind_password) {
        let r = ldap
            .simple_bind(dn, pw)
            .await
            .map_err(|e| Error::System(format!("bind: {}", e)))?;
        if r.rc != 0 {
            return Err(Error::Validation(format!(
                "service account bind failed: {}",
                r.text
            )));
        }
    }
    let _ = ldap.unbind().await;
    Ok(format!("connected to {}", ldap_url(cfg)))
}

// ---------------------------------------------------------------------------
// PAM
// ---------------------------------------------------------------------------

/// PAM service name (`/etc/pam.d/horcrux`, installed by the ebuild).
pub const PAM_SERVICE: &str = "horcrux";

#[cfg(feature = "pam")]
mod pam_ffi {
    use std::ffi::CString;
    use std::os::raw::{c_char, c_int, c_void};

    const PAM_SUCCESS: c_int = 0;
    const PAM_PROMPT_ECHO_OFF: c_int = 1;
    const PAM_PROMPT_ECHO_ON: c_int = 2;
    const PAM_BUF_ERR: c_int = 5;
    const PAM_CONV_ERR: c_int = 19;

    #[repr(C)]
    struct PamMessage {
        msg_style: c_int,
        msg: *const c_char,
    }

    #[repr(C)]
    struct PamResponse {
        resp: *mut c_char,
        resp_retcode: c_int,
    }

    #[repr(C)]
    struct PamConv {
        conv: extern "C" fn(
            c_int,
            *mut *const PamMessage,
            *mut *mut PamResponse,
            *mut c_void,
        ) -> c_int,
        appdata_ptr: *mut c_void,
    }

    #[link(name = "pam")]
    extern "C" {
        fn pam_start(
            service: *const c_char,
            user: *const c_char,
            conv: *const PamConv,
            pamh: *mut *mut c_void,
        ) -> c_int;
        fn pam_authenticate(pamh: *mut c_void, flags: c_int) -> c_int;
        fn pam_acct_mgmt(pamh: *mut c_void, flags: c_int) -> c_int;
        fn pam_end(pamh: *mut c_void, status: c_int) -> c_int;
    }

    extern "C" {
        fn calloc(n: usize, size: usize) -> *mut c_void;
        fn strdup(s: *const c_char) -> *mut c_char;
    }

    /// Answers every password / visible prompt with the password. PAM frees
    /// the response array and strings with free(), so they're malloc'ed.
    extern "C" fn converse(
        num_msg: c_int,
        msg: *mut *const PamMessage,
        resp: *mut *mut PamResponse,
        appdata: *mut c_void,
    ) -> c_int {
        if num_msg <= 0 || msg.is_null() || resp.is_null() || appdata.is_null() {
            return PAM_CONV_ERR;
        }
        unsafe {
            let password = appdata as *const c_char;
            let out =
                calloc(num_msg as usize, std::mem::size_of::<PamResponse>()) as *mut PamResponse;
            if out.is_null() {
                return PAM_BUF_ERR;
            }
            for i in 0..num_msg as usize {
                let m = *msg.add(i);
                if m.is_null() {
                    continue;
                }
                match (*m).msg_style {
                    PAM_PROMPT_ECHO_OFF | PAM_PROMPT_ECHO_ON => {
                        (*out.add(i)).resp = strdup(password);
                    }
                    _ => {} // info / error text: no answer
                }
            }
            *resp = out;
        }
        PAM_SUCCESS
    }

    pub fn authenticate(service: &str, user: &str, password: &str) -> Result<bool, String> {
        let service = CString::new(service).map_err(|e| e.to_string())?;
        let user = CString::new(user).map_err(|e| e.to_string())?;
        let password = CString::new(password).map_err(|e| e.to_string())?;
        let conv = PamConv {
            conv: converse,
            appdata_ptr: password.as_ptr() as *mut c_void,
        };
        let mut handle: *mut c_void = std::ptr::null_mut();
        unsafe {
            let rc = pam_start(service.as_ptr(), user.as_ptr(), &conv, &mut handle);
            if rc != PAM_SUCCESS || handle.is_null() {
                return Err(format!("pam_start failed ({})", rc));
            }
            let mut rc = pam_authenticate(handle, 0);
            if rc == PAM_SUCCESS {
                rc = pam_acct_mgmt(handle, 0);
            }
            pam_end(handle, rc);
            Ok(rc == PAM_SUCCESS)
        }
    }
}

#[cfg(feature = "pam")]
pub async fn pam_authenticate(username: &str, password: &str) -> Result<Option<ExternalIdentity>> {
    if !username
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        return Ok(None);
    }
    let (u, p) = (username.to_string(), password.to_string());
    let ok = tokio::task::spawn_blocking(move || pam_ffi::authenticate(PAM_SERVICE, &u, &p))
        .await
        .map_err(|e| Error::System(format!("PAM task: {}", e)))?
        .map_err(Error::System)?;
    Ok(ok.then_some(ExternalIdentity {
        email: None,
        display: None,
    }))
}

#[cfg(not(feature = "pam"))]
pub async fn pam_authenticate(
    _username: &str,
    _password: &str,
) -> Result<Option<ExternalIdentity>> {
    Err(Error::InvalidConfig(
        "this build has no PAM support (USE=pam / cargo feature `pam`)".into(),
    ))
}

// ---------------------------------------------------------------------------
// OpenID Connect
// ---------------------------------------------------------------------------

pub fn openid_provider(cfg: &OpenIdRealmConfig, redirect_uri: &str) -> super::oidc::OidcProvider {
    super::oidc::OidcProvider::new(super::oidc::OidcConfig {
        enabled: true,
        issuer_url: cfg.issuer_url.trim_end_matches('/').to_string(),
        client_id: cfg.client_id.clone(),
        client_secret: cfg.client_secret.clone(),
        redirect_uri: redirect_uri.to_string(),
        scopes: cfg
            .scopes
            .clone()
            .unwrap_or_else(|| vec!["openid".into(), "profile".into(), "email".into()]),
        auto_create_users: cfg.autocreate,
        role_claim: None,
        role_mapping: Default::default(),
    })
}

/// Pick the Horcrux user name out of verified ID-token claims.
pub fn openid_username(
    cfg: &OpenIdRealmConfig,
    claims: &super::oidc::IdTokenClaims,
) -> Option<String> {
    let pick = |name: &str| -> Option<String> {
        match name {
            "sub" => Some(claims.sub.clone()),
            "email" => claims.email.clone(),
            "name" => claims.name.clone(),
            other => claims
                .additional_claims
                .get(other)
                .and_then(|v| v.as_str())
                .map(str::to_string),
        }
    };
    let name = pick(cfg.username_claim.as_deref().unwrap_or("sub"))?;
    let name = name.trim().to_string();
    (!name.is_empty()
        && name.len() <= 128
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '@')))
    .then_some(name)
}

/// RFC 7636 S256 code challenge for a verifier.
pub fn pkce_challenge(verifier: &str) -> String {
    use base64::Engine;
    use sha2::Digest;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(sha2::Sha256::digest(verifier.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn realm(t: &str, config: serde_json::Value) -> RealmRecord {
        RealmRecord {
            name: "corp".into(),
            realm_type: t.into(),
            enabled: true,
            comment: None,
            config,
            default_group: None,
            tfa_required: false,
        }
    }

    #[test]
    fn validates_realm_definitions() {
        assert!(validate(&realm(
            "ldap",
            serde_json::json!({"server": "ldap", "base_dn": "dc=x"})
        ))
        .is_ok());
        assert!(validate(&realm("ldap", serde_json::json!({"server": "ldap"}))).is_err());
        assert!(validate(&realm("ad", serde_json::json!({"server": "dc1"}))).is_err());
        assert!(validate(&realm(
            "openid",
            serde_json::json!({"issuer_url": "https://idp", "client_id": "hx"})
        ))
        .is_ok());
        assert!(validate(&realm("kerberos", serde_json::json!({}))).is_err());
        let mut local = realm("ldap", serde_json::json!({"server": "x", "base_dn": "y"}));
        local.name = "local".into();
        assert!(validate(&local).is_err());
    }

    #[test]
    fn secrets_are_redacted_and_kept_on_update() {
        let stored = realm(
            "ldap",
            serde_json::json!({"server": "x", "base_dn": "y", "bind_password": "s3cret"}),
        );
        let shown = redact(stored.clone());
        assert_eq!(shown.config["bind_password"], "********");
        let mut updated = shown.clone();
        merge_secrets(&mut updated, &stored);
        assert_eq!(updated.config["bind_password"], "s3cret");
    }

    #[test]
    fn ldap_urls() {
        let mut c: LdapRealmConfig =
            serde_json::from_value(serde_json::json!({"server": "ldap.lan", "base_dn": "x"}))
                .unwrap();
        assert_eq!(ldap_url(&c), "ldap://ldap.lan:389");
        c.mode = "ldaps".into();
        assert_eq!(ldap_url(&c), "ldaps://ldap.lan:636");
        c.server = "fd00::5".into();
        c.port = Some(1636);
        assert_eq!(ldap_url(&c), "ldaps://[fd00::5]:1636");
    }

    #[tokio::test]
    async fn empty_password_never_authenticates() {
        // would be an unauthenticated (always successful) LDAP bind
        let r = realm(
            "ldap",
            serde_json::json!({"server": "127.0.0.1", "port": 1, "base_dn": "x"}),
        );
        assert!(authenticate_password(&r, "alice", "")
            .await
            .unwrap()
            .is_none());
    }

    #[test]
    fn pkce_rfc7636_vector() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
}
