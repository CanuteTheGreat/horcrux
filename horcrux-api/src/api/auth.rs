//! Access-control API: login (all realms, second factors), sessions,
//! self-service account and 2FA management, and the administration of users,
//! groups, roles, ACLs and realms. Everything is backed by the database
//! (`auth::store`); permissions are enforced by `middleware::rbac` plus the
//! extra target checks below (ACL path, administrator accounts).

use crate::audit::{AuditEvent, AuditEventType, AuditResult, AuditSeverity};
use crate::auth::store::{self, Principal, RealmRecord, RoleRecord, UserUpdate};
use crate::auth::{password, privileges, realms, totp, webauthn};
use crate::error::ApiError;
use crate::middleware::auth::{
    generate_session_token, AuthUser, SCOPE_TFA_SETUP, TFA_SETUP_TOKEN_TTL_SECS,
    USER_TOKEN_TTL_SECS,
};
use crate::AppState;
use axum::{
    extract::{ConnectInfo, Host, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{delete, get, post, put},
    Extension, Json, Router,
};
use horcrux_common::auth::{Privilege, User};
use serde::Deserialize;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;

type St = State<Arc<AppState>>;
type ApiResult<T> = Result<T, ApiError>;

/// Routes reachable without logging in (behind the strict auth rate limiter).
pub fn public_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/auth/login", post(login))
        .route("/api/auth/tfa", post(login_tfa))
        .route("/api/auth/register", post(register))
        .route("/api/auth/realms", get(login_realms))
        .route("/api/auth/oidc/:realm/login", get(oidc_login))
        .route("/api/auth/oidc/callback", get(oidc_callback))
        .route("/api/tls/ca.pem", get(super::tls::ca_pem))
}

/// Routes that need a login (auth + RBAC middleware applied by main.rs).
pub fn protected_routes() -> Router<Arc<AppState>> {
    Router::new()
        // own session / account
        .route("/api/auth/logout", post(logout))
        .route("/api/auth/verify", get(verify))
        .route("/api/auth/me", get(me))
        .route("/api/auth/sessions", get(list_sessions))
        .route("/api/auth/sessions/:id", delete(delete_session))
        .route("/api/auth/password", post(change_own_password))
        // second factors (self-service)
        .route("/api/auth/mfa/status", get(mfa_status))
        .route("/api/auth/mfa/totp/setup", post(totp_setup))
        .route("/api/auth/mfa/totp/verify", post(totp_verify))
        .route(
            "/api/auth/mfa/webauthn/register",
            post(webauthn_register_start),
        )
        .route(
            "/api/auth/mfa/webauthn/register/finish",
            post(webauthn_register_finish),
        )
        .route(
            "/api/auth/mfa/backup-codes/regenerate",
            post(regenerate_recovery_codes),
        )
        .route("/api/auth/mfa/methods/:id", delete(delete_factor))
        .route(
            "/api/auth/mfa/methods/:id/disable",
            post(delete_factor_post),
        )
        // second-factor administration
        .route(
            "/api/auth/mfa/policy",
            get(get_tfa_policy).put(put_tfa_policy),
        )
        .route("/api/auth/mfa/users", get(mfa_users))
        .route("/api/auth/mfa/users/:id/reset", post(mfa_reset_user))
        .route("/api/auth/mfa/users/:id/enforce", post(mfa_enforce_user))
        // users
        .route("/api/users", get(list_users).post(create_user))
        .route(
            "/api/users/:id",
            get(get_user)
                .put(update_user)
                .post(update_user)
                .delete(delete_user),
        )
        .route("/api/users/:id/toggle", post(toggle_user))
        .route("/api/users/:id/password", post(set_user_password))
        .route("/api/users/:id/sessions", delete(delete_user_sessions))
        .route("/api/users/:id/groups", put(set_user_groups))
        .route(
            "/api/users/:id/permissions",
            get(user_permissions)
                .post(add_user_permission)
                .delete(remove_user_permission),
        )
        .route(
            "/api/users/:id/api-keys",
            get(list_api_keys).post(create_api_key),
        )
        .route(
            "/api/users/:id/api-keys/:key_id",
            delete(revoke_api_key).post(update_api_key),
        )
        .route(
            "/api/users/:id/api-keys/:key_id/toggle",
            post(toggle_api_key),
        )
        .route(
            "/api/permissions/:id",
            get(user_permissions).post(add_user_permission),
        )
        // roles, privileges, groups, ACL
        .route("/api/roles", get(list_roles).post(create_role))
        .route(
            "/api/roles/:name",
            get(get_role)
                .put(update_role)
                .post(update_role)
                .delete(delete_role),
        )
        .route("/api/privileges", get(list_privileges))
        .route("/api/groups", get(list_groups).post(create_group))
        .route("/api/groups/:name", put(update_group).delete(delete_group))
        .route("/api/acl", get(list_acl).post(add_acl).delete(remove_acl))
        // realms
        .route("/api/access/realms", get(list_realms).post(create_realm))
        .route(
            "/api/access/realms/:name",
            get(get_realm).put(update_realm).delete(delete_realm),
        )
        .route("/api/access/realms/:name/test", post(test_realm))
}

// ===========================================================================
// first run
// ===========================================================================

/// Where a generated first-run administrator password is left for the operator.
pub fn initial_password_file(cfg: &crate::config::HorcruxConfig) -> std::path::PathBuf {
    cfg.paths.data_dir.join("initial-admin-password")
}

/// First start (no users at all): create the `admin` account with an
/// `Administrator` grant on `/`. The password comes from `ADMIN_PASSWORD`,
/// or is generated and written to `<data_dir>/initial-admin-password`
/// (mode 0600) - there is no built-in default password. That file is removed
/// once the admin password is changed. Nobody else can create accounts until
/// an administrator does (public registration is off by default).
pub async fn bootstrap_admin(db: &crate::db::Database, cfg: &crate::config::HorcruxConfig) {
    let pool = db.pool();
    match store::user_count(pool).await {
        Ok(0) => {}
        Ok(n) => {
            tracing::info!("Found {} existing user(s) in database", n);
            return;
        }
        Err(e) => {
            tracing::error!("Failed to count users: {}", e);
            return;
        }
    }
    let (pw, generated) = match std::env::var("ADMIN_PASSWORD") {
        Ok(p) if !p.is_empty() => (p, false),
        _ => (crate::auth::generate_initial_password(), true),
    };
    let user = User {
        id: uuid::Uuid::new_v4().to_string(),
        username: "admin".into(),
        password_hash: password::hash_password(&pw).expect("hash admin password"),
        email: "admin@localhost".into(),
        role: "admin".into(),
        realm: "local".into(),
        enabled: true,
        roles: vec![],
        comment: Some("Initial administrator".into()),
    };
    store::insert_user(pool, &user)
        .await
        .expect("create admin user");
    store::add_acl(pool, "/", "user", &user.id, "Administrator", true)
        .await
        .expect("grant admin role");
    if generated {
        let path = initial_password_file(cfg);
        let _ = std::fs::create_dir_all(&cfg.paths.data_dir);
        match crate::middleware::auth::write_private(&path, format!("{}\n", pw).as_bytes()) {
            Ok(()) => tracing::warn!(
                "Created administrator 'admin'; its generated password is in {} (readable by the service user only). Log in and change it.",
                path.display()
            ),
            Err(e) => tracing::error!(
                "Created administrator 'admin' but could not write {}: {}. Set ADMIN_PASSWORD and recreate the database.",
                path.display(),
                e
            ),
        }
    } else {
        if pw.len() < 12 {
            tracing::warn!("ADMIN_PASSWORD is short; change the admin password after logging in");
        }
        tracing::info!("Created administrator 'admin' with the password from ADMIN_PASSWORD");
    }
}

// ===========================================================================
// helpers
// ===========================================================================

fn me_user(u: Option<Extension<AuthUser>>) -> ApiResult<AuthUser> {
    u.map(|Extension(u)| u)
        .ok_or(ApiError::AuthenticationFailed)
}

async fn principal_of(state: &AppState, user: &AuthUser) -> ApiResult<Principal> {
    Ok(Principal::load(state.database.pool(), &user.user_id).await?)
}

fn client_ip(ci: &Option<ConnectInfo<SocketAddr>>, headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|s| s.trim().to_string())
        .or_else(|| ci.as_ref().map(|ConnectInfo(a)| a.ip().to_string()))
}

async fn audit(
    state: &AppState,
    kind: AuditEventType,
    ok: bool,
    user: Option<&str>,
    ip: Option<String>,
    action: &str,
    details: Option<String>,
) {
    state
        .audit_logger
        .log(AuditEvent {
            timestamp: chrono::Utc::now(),
            severity: if ok {
                AuditSeverity::Info
            } else {
                AuditSeverity::Warning
            },
            event_type: kind,
            user: user.map(str::to_string),
            source_ip: ip,
            resource: None,
            action: action.to_string(),
            result: if ok {
                AuditResult::Success
            } else {
                AuditResult::Failure
            },
            details,
            session_id: None,
        })
        .await;
}

async fn resolve_user(state: &AppState, id_or_name: &str) -> ApiResult<User> {
    store::find_user(state.database.pool(), id_or_name)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("user '{}' not found", id_or_name)))
}

/// Changing an administrator's account needs administrator rights: a mere
/// `User.Modify` holder must not be able to reset the root password.
async fn guard_admin_target(state: &AppState, caller: &Principal, target: &User) -> ApiResult<()> {
    if caller.user_id == target.id {
        return Ok(());
    }
    let target_p = Principal::load(state.database.pool(), &target.id).await?;
    if (target_p.is_admin() || target_p.has(Privilege::SysModify, "/")) && !caller.is_admin() {
        return Err(ApiError::Forbidden(
            "only an administrator can change another administrator's account".into(),
        ));
    }
    Ok(())
}

fn validate_username(name: &str) -> ApiResult<()> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        return Err(ApiError::ValidationError(
            "user names are 1-64 letters, digits, _ - . (no @: that marks a realm)".into(),
        ));
    }
    Ok(())
}

fn validate_password(pw: &str) -> ApiResult<()> {
    if pw.chars().count() < 8 {
        return Err(ApiError::ValidationError(
            "password must be at least 8 characters".into(),
        ));
    }
    Ok(())
}

fn session_cookie(session_id: &str, max_age: usize) -> HeaderValue {
    let secure = if crate::tls::serving_tls() {
        "; Secure"
    } else {
        ""
    };
    HeaderValue::from_str(&format!(
        "session_id={}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{}",
        session_id, max_age, secure
    ))
    .unwrap_or_else(|_| HeaderValue::from_static("session_id=; Path=/"))
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct TfaPolicy {
    /// Every account needs a second factor.
    pub global_enforcement: bool,
    /// Accounts holding any of these roles on `/` need a second factor.
    pub required_for_roles: Vec<String>,
}

async fn tfa_policy(state: &AppState) -> TfaPolicy {
    store::get_setting(state.database.pool(), "tfa_policy")
        .await
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Whether `user` must use a second factor (user flag, realm flag, policy).
async fn tfa_required(state: &AppState, user: &User) -> ApiResult<bool> {
    let pool = state.database.pool();
    if store::user_tfa_required(pool, &user.id).await? {
        return Ok(true);
    }
    if user.realm != "local" {
        if let Some(r) = store::get_realm(pool, &user.realm).await? {
            if r.tfa_required {
                return Ok(true);
            }
        }
    }
    let policy = tfa_policy(state).await;
    if policy.global_enforcement {
        return Ok(true);
    }
    if !policy.required_for_roles.is_empty() {
        let p = Principal::load(pool, &user.id).await?;
        if p.acl
            .iter()
            .any(|e| e.path == "/" && policy.required_for_roles.contains(&e.role))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Create the login session and the response the UI/CLI expect.
async fn issue_login(
    state: &AppState,
    user: &User,
    scope: Option<&str>,
    ip: Option<String>,
    user_agent: Option<&str>,
) -> ApiResult<Response> {
    let pool = state.database.pool();
    let ttl = if scope.is_some() {
        TFA_SETUP_TOKEN_TTL_SECS
    } else {
        USER_TOKEN_TTL_SECS
    };
    let sid =
        store::create_session(pool, &user.id, scope, ttl as i64, ip.as_deref(), user_agent).await?;
    let token = generate_session_token(&user.id, &user.username, &user.role, &sid, scope, ttl)
        .map_err(ApiError::Internal)?;
    store::touch_last_login(pool, &user.id).await?;
    let principal = Principal::load(pool, &user.id).await?;
    let roles: Vec<String> = principal
        .acl
        .iter()
        .filter(|e| e.path == "/")
        .map(|e| e.role.clone())
        .collect();
    audit(
        state,
        AuditEventType::Login,
        true,
        Some(&user.username),
        ip,
        "login",
        scope.map(|s| format!("scope={}", s)),
    )
    .await;
    let mut body = json!({
        "ticket": token,
        "csrf_token": sid,
        "username": user.username,
        "roles": roles,
    });
    if scope == Some(SCOPE_TFA_SETUP) {
        body["tfa_setup_required"] = json!(true);
    }
    let mut resp = Json(body).into_response();
    resp.headers_mut()
        .insert(header::SET_COOKIE, session_cookie(&sid, ttl));
    Ok(resp)
}

/// After the password (or realm) check: second factor, forced setup, or done.
async fn finish_primary_auth(
    state: &AppState,
    user: &User,
    host: &str,
    ip: Option<String>,
    user_agent: Option<&str>,
) -> ApiResult<Response> {
    let pool = state.database.pool();
    let factors: Vec<_> = store::list_tfa(pool, &user.id)
        .await?
        .into_iter()
        .filter(|f| f.enabled)
        .collect();
    if !factors.is_empty() {
        let mut methods = Vec::new();
        if factors.iter().any(|f| f.kind == "totp") {
            methods.push("totp");
        }
        let creds: Vec<webauthn::StoredCredential> = factors
            .iter()
            .filter(|f| f.kind == "webauthn")
            .filter_map(|f| serde_json::from_str(&f.data).ok())
            .collect();
        let mut wa = None;
        let mut data = json!({ "failures": 0 });
        if !creds.is_empty() {
            methods.push("webauthn");
            let rp = webauthn::RelyingParty::from_host(host);
            let ch = webauthn::new_challenge();
            wa = Some(webauthn::request_options(&rp, &ch, &creds));
            data["webauthn_challenge"] = json!(ch);
            data["rp_id"] = json!(rp.id);
            data["origins"] = json!(rp.origins);
        }
        if store::recovery_codes_remaining(pool, &user.id).await? > 0 {
            methods.push("recovery");
        }
        let ticket = store::put_challenge(pool, "tfa-login", Some(&user.id), &data, 300).await?;
        return Ok(Json(json!({
            "tfa_required": true,
            "ticket": ticket,
            "username": user.username,
            "methods": methods,
            "webauthn": wa,
        }))
        .into_response());
    }
    if tfa_required(state, user).await? {
        return issue_login(state, user, Some(SCOPE_TFA_SETUP), ip, user_agent).await;
    }
    issue_login(state, user, None, ip, user_agent).await
}

// ===========================================================================
// login
// ===========================================================================

#[derive(Deserialize)]
struct LoginBody {
    username: String,
    password: String,
    #[serde(default)]
    realm: Option<String>,
}

/// A real Argon2 hash of a random string, so unknown users cost the same
/// time as wrong passwords (no user enumeration by timing).
fn dummy_hash() -> &'static str {
    static H: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    H.get_or_init(|| password::hash_password(&crate::auth::random_token(24)).unwrap_or_default())
}

async fn login(
    State(state): St,
    Host(host): Host,
    ci: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
    Json(req): Json<LoginBody>,
) -> ApiResult<Response> {
    let ip = client_ip(&ci, &headers);
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok());
    let pool = state.database.pool();

    // "alice@corp" selects realm "corp" when no realm is given explicitly
    let (name, realm) = match req.realm.as_deref().filter(|r| !r.is_empty()) {
        Some(r) => (req.username.clone(), r.to_string()),
        None => match req.username.rsplit_once('@') {
            Some((n, r)) if store::get_realm(pool, r).await?.is_some() => {
                (n.to_string(), r.to_string())
            }
            _ => (req.username.clone(), "local".to_string()),
        },
    };
    let fail = |reason: &'static str| {
        let state = state.clone();
        let user = req.username.clone();
        let ip = ip.clone();
        async move {
            audit(
                &state,
                AuditEventType::LoginFailed,
                false,
                Some(&user),
                ip,
                "login",
                Some(reason.to_string()),
            )
            .await;
            ApiError::AuthenticationFailed
        }
    };

    let user = if realm == "local" || realm == "pve" {
        let found = store::find_user(pool, &name)
            .await?
            .filter(|u| u.realm == "local" && u.username == name);
        let hash = found
            .as_ref()
            .map(|u| u.password_hash.clone())
            .unwrap_or_else(|| dummy_hash().to_string());
        let ok = password::verify_password(&req.password, &hash).unwrap_or(false);
        match found {
            Some(u) if ok => u,
            _ => return Err(fail("bad credentials").await),
        }
    } else {
        let Some(r) = store::get_realm(pool, &realm).await?.filter(|r| r.enabled) else {
            return Err(fail("unknown realm").await);
        };
        if r.realm_type == "openid" {
            return Err(ApiError::BadRequest(format!(
                "realm '{}' uses OpenID Connect: start at /api/auth/oidc/{}/login",
                realm, realm
            )));
        }
        let ident = realms::authenticate_password(&r, &name, &req.password)
            .await
            .map_err(|e| {
                tracing::warn!("realm {} authentication error: {}", realm, e);
                ApiError::AuthenticationFailed
            })?;
        let Some(ident) = ident else {
            return Err(fail("bad credentials").await);
        };
        external_user(&state, &r, &name, ident.email).await?
    };
    if !user.enabled {
        return Err(fail("account disabled").await);
    }
    finish_primary_auth(&state, &user, &host, ip, ua).await
}

/// Find or create the shadow account `<name>@<realm>` for an external login.
async fn external_user(
    state: &AppState,
    realm: &RealmRecord,
    name: &str,
    email: Option<String>,
) -> ApiResult<User> {
    let pool = state.database.pool();
    let username = format!("{}@{}", name, realm.name);
    if let Some(u) = store::find_user(pool, &username).await? {
        return Ok(u);
    }
    let autocreate = realm
        .config
        .get("autocreate")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    if !autocreate {
        return Err(ApiError::Forbidden(format!(
            "no account '{}': ask an administrator to create it",
            username
        )));
    }
    let user = User {
        id: uuid::Uuid::new_v4().to_string(),
        username: username.clone(),
        password_hash: realms::EXTERNAL_PASSWORD.to_string(),
        email: email.unwrap_or_default(),
        role: "user".into(),
        realm: realm.name.clone(),
        enabled: true,
        roles: vec![],
        comment: Some(format!("created on first {} login", realm.realm_type)),
    };
    store::insert_user(pool, &user).await?;
    if let Some(g) = realm.default_group.as_deref().filter(|g| !g.is_empty()) {
        store::add_group_member(pool, g, &user.id).await?;
    }
    tracing::info!(
        "created account {} on first login via realm {}",
        username,
        realm.name
    );
    Ok(user)
}

#[derive(Deserialize)]
struct TfaBody {
    ticket: String,
    /// `totp`, `recovery` or `webauthn`; inferred when omitted.
    #[serde(default, alias = "type")]
    method: Option<String>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    credential: Option<webauthn::CredentialResponse>,
}

async fn login_tfa(
    State(state): St,
    ci: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
    Json(req): Json<TfaBody>,
) -> ApiResult<Response> {
    let ip = client_ip(&ci, &headers);
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok());
    let pool = state.database.pool();
    let Some((Some(user_id), mut data)) =
        store::peek_challenge(pool, &req.ticket, "tfa-login").await?
    else {
        return Err(ApiError::AuthenticationFailed);
    };
    let user = resolve_user(&state, &user_id).await?;
    let method = req.method.clone().unwrap_or_else(|| {
        if req.credential.is_some() {
            "webauthn".into()
        } else if req
            .code
            .as_deref()
            .map(|c| c.contains('-'))
            .unwrap_or(false)
        {
            "recovery".into()
        } else {
            "totp".into()
        }
    });
    let ok = match method.as_str() {
        "totp" => verify_any_totp(&state, &user.id, req.code.as_deref().unwrap_or("")).await?,
        "recovery" => {
            store::use_recovery_code(pool, &user.id, req.code.as_deref().unwrap_or("")).await?
        }
        "webauthn" => {
            let (Some(cred), Some(ch)) = (&req.credential, data["webauthn_challenge"].as_str())
            else {
                return Err(ApiError::BadRequest("webauthn needs `credential`".into()));
            };
            let rp = webauthn::RelyingParty {
                id: data["rp_id"].as_str().unwrap_or_default().to_string(),
                origins: serde_json::from_value(data["origins"].clone()).unwrap_or_default(),
            };
            verify_webauthn(&state, &user.id, cred, ch, &rp).await?
        }
        other => return Err(ApiError::BadRequest(format!("unknown method {}", other))),
    };
    if !ok {
        let failures = data["failures"].as_i64().unwrap_or(0) + 1;
        if failures >= 5 {
            store::take_challenge(pool, &req.ticket, "tfa-login").await?;
        } else {
            data["failures"] = json!(failures);
            store::update_challenge(pool, &req.ticket, &data).await?;
        }
        audit(
            &state,
            AuditEventType::LoginFailed,
            false,
            Some(&user.username),
            ip,
            "login-tfa",
            Some(format!("bad {} code", method)),
        )
        .await;
        return Err(ApiError::AuthenticationFailed);
    }
    // single use
    if store::take_challenge(pool, &req.ticket, "tfa-login")
        .await?
        .is_none()
    {
        return Err(ApiError::AuthenticationFailed);
    }
    if !user.enabled {
        return Err(ApiError::AuthenticationFailed);
    }
    issue_login(&state, &user, None, ip, ua).await
}

async fn verify_any_totp(state: &AppState, user_id: &str, code: &str) -> ApiResult<bool> {
    let pool = state.database.pool();
    let now = chrono::Utc::now().timestamp() as u64;
    for f in store::list_tfa(pool, user_id).await? {
        if f.kind != "totp" || !f.enabled {
            continue;
        }
        if let Some(step) = totp::verify(&f.data, code, now, f.sign_count) {
            store::set_totp_last_step(pool, &f.id, step).await?;
            return Ok(true);
        }
    }
    Ok(false)
}

async fn verify_webauthn(
    state: &AppState,
    user_id: &str,
    cred: &webauthn::CredentialResponse,
    challenge: &str,
    rp: &webauthn::RelyingParty,
) -> ApiResult<bool> {
    let pool = state.database.pool();
    let factors: Vec<_> = store::list_tfa(pool, user_id)
        .await?
        .into_iter()
        .filter(|f| f.kind == "webauthn" && f.enabled)
        .collect();
    let creds: Vec<(webauthn::StoredCredential, i64)> = factors
        .iter()
        .filter_map(|f| {
            serde_json::from_str(&f.data)
                .ok()
                .map(|c| (c, f.sign_count))
        })
        .collect();
    match webauthn::verify_assertion(cred, challenge, rp, &creds) {
        Ok((idx, count)) => {
            let id = &factors
                .iter()
                .find(|f| {
                    serde_json::from_str::<webauthn::StoredCredential>(&f.data)
                        .map(|c| c.credential_id == creds[idx].0.credential_id)
                        .unwrap_or(false)
                })
                .map(|f| f.id.clone())
                .unwrap_or_default();
            store::mark_tfa_used(pool, id, Some(count as i64)).await?;
            Ok(true)
        }
        Err(e) => {
            tracing::info!("webauthn assertion rejected: {}", e);
            Ok(false)
        }
    }
}

#[derive(Deserialize)]
struct RegisterBody {
    username: String,
    password: String,
    #[serde(default)]
    email: String,
}

/// Public self-registration. Off by default; when an operator turns it on,
/// the new account has no permissions until an administrator grants some.
async fn register(State(state): St, Json(req): Json<RegisterBody>) -> ApiResult<StatusCode> {
    if !crate::auth::registration_enabled(state.config.auth.allow_registration) {
        return Err(ApiError::Forbidden(
            "self-registration is disabled; ask an administrator for an account".into(),
        ));
    }
    validate_username(&req.username)?;
    validate_password(&req.password)?;
    if !req.email.is_empty() && !req.email.contains('@') {
        return Err(ApiError::ValidationError("invalid email address".into()));
    }
    let user = User {
        id: uuid::Uuid::new_v4().to_string(),
        username: req.username,
        password_hash: password::hash_password(&req.password)?,
        email: req.email,
        role: "user".into(),
        realm: "local".into(),
        enabled: true,
        roles: vec![],
        comment: Some("self-registered".into()),
    };
    store::insert_user(state.database.pool(), &user).await?;
    Ok(StatusCode::CREATED)
}

/// Realms offered on the login page.
async fn login_realms(State(state): St) -> ApiResult<Json<Value>> {
    let mut out = vec![json!({"name": "local", "type": "local", "comment": "Horcrux users"})];
    for r in store::list_realms(state.database.pool()).await? {
        if r.enabled {
            out.push(json!({"name": r.name, "type": r.realm_type, "comment": r.comment}));
        }
    }
    Ok(Json(json!({
        "realms": out,
        "registration_enabled": crate::auth::registration_enabled(state.config.auth.allow_registration),
        "initial_password_file": initial_password_file(&state.config)
            .exists()
            .then(|| initial_password_file(&state.config).display().to_string()),
    })))
}

// ===========================================================================
// OpenID Connect
// ===========================================================================

#[derive(Deserialize)]
struct OidcLoginQuery {
    #[serde(default)]
    redirect: Option<String>,
}

fn default_redirect_uri(host: &str) -> String {
    let scheme = if crate::tls::serving_tls() {
        "https"
    } else {
        "http"
    };
    format!("{}://{}/api/auth/oidc/callback", scheme, host)
}

async fn oidc_login(
    State(state): St,
    Host(host): Host,
    Path(realm): Path<String>,
    Query(q): Query<OidcLoginQuery>,
) -> ApiResult<Response> {
    let pool = state.database.pool();
    let r = store::get_realm(pool, &realm)
        .await?
        .filter(|r| r.enabled && r.realm_type == "openid")
        .ok_or_else(|| ApiError::NotFound(format!("no OpenID realm '{}'", realm)))?;
    let cfg: realms::OpenIdRealmConfig = serde_json::from_value(r.config.clone())
        .map_err(|e| ApiError::Internal(format!("realm config: {}", e)))?;
    let redirect_uri = cfg
        .redirect_uri
        .clone()
        .unwrap_or_else(|| default_redirect_uri(&host));
    let provider = realms::openid_provider(&cfg, &redirect_uri);
    let nonce = crate::auth::random_token(32);
    let verifier = crate::auth::random_token(64);
    // only same-site relative redirects after login
    let after = q
        .redirect
        .filter(|r| r.starts_with('/') && !r.starts_with("//"))
        .unwrap_or_else(|| "/".into());
    let state_id = store::put_challenge(
        pool,
        "oidc",
        None,
        &json!({"realm": realm, "nonce": nonce, "verifier": verifier,
                "redirect_uri": redirect_uri, "after": after}),
        600,
    )
    .await?;
    let url = provider
        .authorization_url_pkce(&state_id, &nonce, &realms::pkce_challenge(&verifier))
        .await
        .map_err(|e| ApiError::ServiceUnavailable(format!("OpenID provider: {}", e)))?;
    Ok(Redirect::to(&url).into_response())
}

#[derive(Deserialize)]
struct OidcCallbackQuery {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

fn login_error_redirect(msg: &str) -> Response {
    Redirect::to(&format!("/login#error={}", urlencoding::encode(msg))).into_response()
}

async fn oidc_callback(
    State(state): St,
    ci: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
    Query(q): Query<OidcCallbackQuery>,
) -> ApiResult<Response> {
    let ip = client_ip(&ci, &headers);
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok());
    if let Some(err) = q.error {
        return Ok(login_error_redirect(&format!(
            "OpenID provider refused the login: {} {}",
            err,
            q.error_description.unwrap_or_default()
        )));
    }
    let (Some(code), Some(state_id)) = (q.code, q.state) else {
        return Ok(login_error_redirect("OpenID callback without code/state"));
    };
    let pool = state.database.pool();
    let Some((_, data)) = store::take_challenge(pool, &state_id, "oidc").await? else {
        return Ok(login_error_redirect(
            "OpenID login expired or replayed - try again",
        ));
    };
    let realm_name = data["realm"].as_str().unwrap_or_default().to_string();
    let Some(r) = store::get_realm(pool, &realm_name)
        .await?
        .filter(|r| r.enabled)
    else {
        return Ok(login_error_redirect("realm no longer exists"));
    };
    let cfg: realms::OpenIdRealmConfig = serde_json::from_value(r.config.clone())
        .map_err(|e| ApiError::Internal(format!("realm config: {}", e)))?;
    let provider = realms::openid_provider(&cfg, data["redirect_uri"].as_str().unwrap_or_default());
    let tokens = match provider
        .exchange_code_pkce(&code, data["verifier"].as_str().unwrap_or_default())
        .await
    {
        Ok(t) => t,
        Err(e) => {
            return Ok(login_error_redirect(&format!(
                "token exchange failed: {}",
                e
            )))
        }
    };
    let Some(id_token) = tokens.id_token else {
        return Ok(login_error_redirect("provider returned no ID token"));
    };
    let claims = match provider
        .verify_id_token_with_nonce(&id_token, data["nonce"].as_str().unwrap_or_default())
        .await
    {
        Ok(c) => c,
        Err(e) => {
            audit(
                &state,
                AuditEventType::LoginFailed,
                false,
                None,
                ip,
                "login-oidc",
                Some(e.to_string()),
            )
            .await;
            return Ok(login_error_redirect(&format!("ID token rejected: {}", e)));
        }
    };
    let Some(name) = realms::openid_username(&cfg, &claims) else {
        return Ok(login_error_redirect(
            "no usable user name claim in the ID token",
        ));
    };
    let user = match external_user(&state, &r, &name, claims.email.clone()).await {
        Ok(u) => u,
        Err(ApiError::Forbidden(m)) => return Ok(login_error_redirect(&m)),
        Err(e) => return Err(e),
    };
    if !user.enabled {
        return Ok(login_error_redirect("account disabled"));
    }
    // Second factors for OpenID realms are the provider's job (as in PVE).
    let resp = issue_login(&state, &user, None, ip, ua).await?;
    let cookie = resp.headers().get(header::SET_COOKIE).cloned();
    let body = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let v: Value = serde_json::from_slice(&body)?;
    let after = data["after"].as_str().unwrap_or("/");
    // the token travels in the fragment: never sent to servers or logged
    let target = format!(
        "/login#ticket={}&username={}&redirect={}",
        v["ticket"].as_str().unwrap_or_default(),
        urlencoding::encode(v["username"].as_str().unwrap_or_default()),
        urlencoding::encode(after)
    );
    let mut out = Redirect::to(&target).into_response();
    if let Some(c) = cookie {
        out.headers_mut().insert(header::SET_COOKIE, c);
    }
    Ok(out)
}

// ===========================================================================
// own session / account
// ===========================================================================

async fn logout(
    State(state): St,
    u: Option<Extension<AuthUser>>,
    ci: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let user = me_user(u)?;
    if let Some(sid) = &user.session_id {
        crate::db::users::delete_session(state.database.pool(), sid).await?;
    }
    audit(
        &state,
        AuditEventType::Logout,
        true,
        Some(&user.username),
        client_ip(&ci, &headers),
        "logout",
        None,
    )
    .await;
    let mut resp = StatusCode::OK.into_response();
    resp.headers_mut()
        .insert(header::SET_COOKIE, session_cookie("", 0));
    Ok(resp)
}

async fn verify(u: Option<Extension<AuthUser>>) -> Json<Value> {
    let user = u.map(|Extension(u)| u);
    Json(json!({
        "valid": user.is_some(),
        "username": user.as_ref().map(|u| u.username.clone()),
        "scope": user.and_then(|u| u.scope),
    }))
}

/// Who am I and what may I do: drives which UI controls are shown.
async fn me(State(state): St, u: Option<Extension<AuthUser>>) -> ApiResult<Json<Value>> {
    let user = me_user(u)?;
    let pool = state.database.pool();
    let rec = store::get_user_record(pool, &user.user_id)
        .await?
        .ok_or(ApiError::AuthenticationFailed)?;
    let p = principal_of(&state, &user).await?;
    let mut paths: Vec<String> = p.acl.iter().map(|e| e.path.clone()).collect();
    paths.push("/".into());
    paths.sort();
    paths.dedup();
    let perms: serde_json::Map<String, Value> = paths
        .iter()
        .map(|path| {
            (
                path.clone(),
                json!(p
                    .privileges(path)
                    .iter()
                    .map(|x| x.name())
                    .collect::<Vec<_>>()),
            )
        })
        .collect();
    Ok(Json(json!({
        "user": rec,
        "is_admin": p.is_admin(),
        "permissions": perms,
        "scope": user.scope,
        "tfa_required": tfa_required(&state, &resolve_user(&state, &user.user_id).await?).await?,
    })))
}

async fn list_sessions(
    State(state): St,
    u: Option<Extension<AuthUser>>,
) -> ApiResult<Json<Vec<Value>>> {
    let user = me_user(u)?;
    let p = principal_of(&state, &user).await?;
    let scope = (!p.has(Privilege::UserModify, "/access/users")).then_some(user.user_id.as_str());
    Ok(Json(
        store::list_sessions_detailed(state.database.pool(), scope).await?,
    ))
}

async fn delete_session(
    State(state): St,
    u: Option<Extension<AuthUser>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let user = me_user(u)?;
    let p = principal_of(&state, &user).await?;
    let scope = (!p.has(Privilege::UserModify, "/access/users")).then_some(user.user_id.as_str());
    let sessions = store::list_sessions_detailed(state.database.pool(), scope).await?;
    if !sessions.iter().any(|s| s["id"] == id) {
        return Err(ApiError::NotFound(format!("Session {} not found", id)));
    }
    crate::db::users::delete_session(state.database.pool(), &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct OwnPasswordBody {
    /// legacy clients send their own user name; only that is accepted
    #[serde(default)]
    username: Option<String>,
    old_password: String,
    new_password: String,
}

async fn change_own_password(
    State(state): St,
    u: Option<Extension<AuthUser>>,
    ci: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
    Json(req): Json<OwnPasswordBody>,
) -> ApiResult<StatusCode> {
    let user = me_user(u)?;
    if let Some(n) = &req.username {
        if *n != user.username {
            return Err(ApiError::Forbidden(
                "use /api/users/:id/password to change someone else's password".into(),
            ));
        }
    }
    let target = resolve_user(&state, &user.user_id).await?;
    change_password_checked(&state, &target, Some(&req.old_password), &req.new_password).await?;
    audit(
        &state,
        AuditEventType::PasswordChanged,
        true,
        Some(&user.username),
        client_ip(&ci, &headers),
        "password",
        None,
    )
    .await;
    Ok(StatusCode::OK)
}

async fn change_password_checked(
    state: &AppState,
    target: &User,
    old: Option<&str>,
    new: &str,
) -> ApiResult<()> {
    if target.realm != "local" {
        return Err(ApiError::ValidationError(format!(
            "'{}' authenticates via realm '{}': change the password there",
            target.username, target.realm
        )));
    }
    if let Some(old) = old {
        if !password::verify_password(old, &target.password_hash).unwrap_or(false) {
            return Err(ApiError::AuthenticationFailed);
        }
    }
    validate_password(new)?;
    store::set_password_hash(
        state.database.pool(),
        &target.id,
        &password::hash_password(new)?,
    )
    .await?;
    if target.username == "admin" {
        let _ = std::fs::remove_file(initial_password_file(&state.config));
    }
    Ok(())
}

// ===========================================================================
// second factors (self-service)
// ===========================================================================

async fn mfa_status(State(state): St, u: Option<Extension<AuthUser>>) -> ApiResult<Json<Value>> {
    let user = me_user(u)?;
    let pool = state.database.pool();
    let factors = store::list_tfa(pool, &user.user_id).await?;
    let target = resolve_user(&state, &user.user_id).await?;
    let methods: Vec<Value> = factors
        .iter()
        .enumerate()
        .map(|(i, f)| {
            json!({
                "id": f.id,
                "method_type": f.kind,
                "name": f.name,
                "enabled": f.enabled,
                "registered_at": chrono::DateTime::from_timestamp(f.created_at, 0).map(|d| d.to_rfc3339()).unwrap_or_default(),
                "last_used": f.last_used.and_then(|t| chrono::DateTime::from_timestamp(t, 0)).map(|d| d.to_rfc3339()),
                "is_primary": i == 0,
            })
        })
        .collect();
    let last = factors.iter().filter_map(|f| f.last_used).max();
    Ok(Json(json!({
        "enabled": factors.iter().any(|f| f.enabled),
        "required": tfa_required(&state, &target).await?,
        "methods": methods,
        "backup_codes_remaining": store::recovery_codes_remaining(pool, &user.user_id).await?,
        "last_verified": last.and_then(|t| chrono::DateTime::from_timestamp(t, 0)).map(|d| d.to_rfc3339()),
        "trusted_devices": [],
    })))
}

async fn totp_setup(State(state): St, u: Option<Extension<AuthUser>>) -> ApiResult<Json<Value>> {
    let user = me_user(u)?;
    let secret = totp::generate_secret();
    let uri = totp::provisioning_uri("Horcrux", &user.username, &secret);
    let setup_id = store::put_challenge(
        state.database.pool(),
        "totp-setup",
        Some(&user.user_id),
        &json!({"secret": secret}),
        900,
    )
    .await?;
    Ok(Json(json!({
        "setup_id": setup_id,
        "secret": secret,
        "qr_code_url": uri,
        "otpauth_uri": uri,
        "backup_codes": [],
    })))
}

#[derive(Deserialize)]
struct TotpVerifyBody {
    code: String,
    #[serde(default)]
    setup_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

/// After enrolling the first factor: fresh recovery codes, and a restricted
/// "set up 2FA first" session becomes a full one.
async fn after_enrollment(state: &AppState, user: &AuthUser) -> ApiResult<Vec<String>> {
    let pool = state.database.pool();
    let mut codes = vec![];
    if store::recovery_codes_remaining(pool, &user.user_id).await? == 0 {
        codes = totp::generate_recovery_codes();
        store::replace_recovery_codes(pool, &user.user_id, &codes).await?;
    }
    if let Some(sid) = &user.session_id {
        sqlx::query(
            "UPDATE sessions SET scope = NULL, expires_at = ? WHERE id = ? AND scope IS NOT NULL",
        )
        .bind(store::now() + USER_TOKEN_TTL_SECS as i64)
        .bind(sid)
        .execute(pool)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    }
    audit(
        state,
        AuditEventType::TwoFactorEnabled,
        true,
        Some(&user.username),
        None,
        "tfa-add",
        None,
    )
    .await;
    Ok(codes)
}

async fn totp_verify(
    State(state): St,
    u: Option<Extension<AuthUser>>,
    Json(req): Json<TotpVerifyBody>,
) -> ApiResult<Json<Value>> {
    let user = me_user(u)?;
    let pool = state.database.pool();
    // the newest pending setup when the client doesn't name one
    let setup_id = match req.setup_id {
        Some(id) => id,
        None => {
            use sqlx::Row;
            sqlx::query(
                "SELECT id FROM auth_challenges WHERE kind = 'totp-setup' AND user_id = ? AND expires_at >= ? ORDER BY expires_at DESC LIMIT 1",
            )
            .bind(&user.user_id)
            .bind(store::now())
            .fetch_optional(pool)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?
            .map(|r| r.get::<String, _>("id"))
            .ok_or_else(|| ApiError::BadRequest("no TOTP setup in progress".into()))?
        }
    };
    let Some((Some(owner), data)) = store::peek_challenge(pool, &setup_id, "totp-setup").await?
    else {
        return Err(ApiError::BadRequest(
            "TOTP setup expired - start again".into(),
        ));
    };
    if owner != user.user_id {
        return Err(ApiError::Forbidden("not your setup".into()));
    }
    let secret = data["secret"].as_str().unwrap_or_default().to_string();
    let now = chrono::Utc::now().timestamp() as u64;
    let Some(step) = totp::verify(&secret, &req.code, now, 0) else {
        return Err(ApiError::ValidationError(
            "code doesn't match - check the authenticator app's clock".into(),
        ));
    };
    store::take_challenge(pool, &setup_id, "totp-setup").await?;
    let id = store::add_tfa(
        pool,
        &user.user_id,
        "totp",
        req.name.as_deref().unwrap_or("Authenticator app"),
        &secret,
    )
    .await?;
    store::set_totp_last_step(pool, &id, step).await?;
    let codes = after_enrollment(&state, &user).await?;
    Ok(Json(
        json!({ "id": id, "backup_codes": codes, "recovery_codes": codes }),
    ))
}

#[derive(Deserialize, Default)]
struct WebauthnStartBody {
    #[serde(default)]
    name: Option<String>,
}

async fn webauthn_register_start(
    State(state): St,
    Host(host): Host,
    u: Option<Extension<AuthUser>>,
    body: Option<Json<WebauthnStartBody>>,
) -> ApiResult<Json<Value>> {
    let user = me_user(u)?;
    let pool = state.database.pool();
    let rp = webauthn::RelyingParty::from_host(&host);
    let existing: Vec<webauthn::StoredCredential> = store::list_tfa(pool, &user.user_id)
        .await?
        .iter()
        .filter(|f| f.kind == "webauthn")
        .filter_map(|f| serde_json::from_str(&f.data).ok())
        .collect();
    let ch = webauthn::new_challenge();
    let options = webauthn::creation_options(&rp, &user.user_id, &user.username, &ch, &existing);
    let name = body
        .and_then(|Json(b)| b.name)
        .unwrap_or_else(|| "Security key".into());
    let challenge_id = store::put_challenge(
        pool,
        "webauthn-reg",
        Some(&user.user_id),
        &json!({"challenge": ch, "rp_id": rp.id, "origins": rp.origins, "name": name}),
        300,
    )
    .await?;
    Ok(Json(
        json!({ "challenge_id": challenge_id, "publicKey": options }),
    ))
}

#[derive(Deserialize)]
struct WebauthnFinishBody {
    challenge_id: String,
    credential: webauthn::CredentialResponse,
    #[serde(default)]
    name: Option<String>,
}

async fn webauthn_register_finish(
    State(state): St,
    u: Option<Extension<AuthUser>>,
    Json(req): Json<WebauthnFinishBody>,
) -> ApiResult<Json<Value>> {
    let user = me_user(u)?;
    let pool = state.database.pool();
    let Some((Some(owner), data)) =
        store::take_challenge(pool, &req.challenge_id, "webauthn-reg").await?
    else {
        return Err(ApiError::BadRequest(
            "registration expired - start again".into(),
        ));
    };
    if owner != user.user_id {
        return Err(ApiError::Forbidden("not your registration".into()));
    }
    let rp = webauthn::RelyingParty {
        id: data["rp_id"].as_str().unwrap_or_default().into(),
        origins: serde_json::from_value(data["origins"].clone()).unwrap_or_default(),
    };
    let cred = webauthn::verify_registration(
        &req.credential,
        data["challenge"].as_str().unwrap_or_default(),
        &rp,
    )
    .map_err(|e| ApiError::ValidationError(format!("security key registration rejected: {}", e)))?;
    let name = req
        .name
        .or_else(|| data["name"].as_str().map(str::to_string))
        .unwrap_or_else(|| "Security key".into());
    let id = store::add_tfa(
        pool,
        &user.user_id,
        "webauthn",
        &name,
        &serde_json::to_string(&cred)?,
    )
    .await?;
    let codes = after_enrollment(&state, &user).await?;
    Ok(Json(
        json!({ "id": id, "backup_codes": codes, "recovery_codes": codes }),
    ))
}

async fn regenerate_recovery_codes(
    State(state): St,
    u: Option<Extension<AuthUser>>,
) -> ApiResult<Json<Vec<String>>> {
    let user = me_user(u)?;
    let pool = state.database.pool();
    if store::list_tfa(pool, &user.user_id).await?.is_empty() {
        return Err(ApiError::BadRequest(
            "add an authenticator app or security key first".into(),
        ));
    }
    let codes = totp::generate_recovery_codes();
    store::replace_recovery_codes(pool, &user.user_id, &codes).await?;
    Ok(Json(codes))
}

#[derive(Deserialize, Default)]
struct RemoveFactorBody {
    /// a current TOTP / recovery code, or
    #[serde(default)]
    code: Option<String>,
    /// the account password (local users)
    #[serde(default)]
    password: Option<String>,
}

async fn remove_factor(
    state: &AppState,
    user: &AuthUser,
    id: &str,
    proof: RemoveFactorBody,
) -> ApiResult<StatusCode> {
    let pool = state.database.pool();
    let target = resolve_user(state, &user.user_id).await?;
    let proven = match (&proof.password, &proof.code) {
        (Some(pw), _) if target.realm == "local" => {
            password::verify_password(pw, &target.password_hash).unwrap_or(false)
        }
        (_, Some(code)) => {
            verify_any_totp(state, &user.user_id, code).await?
                || store::use_recovery_code(pool, &user.user_id, code).await?
        }
        _ => false,
    };
    if !proven {
        return Err(ApiError::Forbidden(
            "confirm with your password or a current code".into(),
        ));
    }
    if !store::delete_tfa(pool, &user.user_id, id).await? {
        return Err(ApiError::NotFound("no such factor".into()));
    }
    if store::list_tfa(pool, &user.user_id).await?.is_empty() {
        store::replace_recovery_codes(pool, &user.user_id, &[]).await?;
    }
    audit(
        state,
        AuditEventType::TwoFactorDisabled,
        true,
        Some(&user.username),
        None,
        "tfa-remove",
        None,
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_factor(
    State(state): St,
    u: Option<Extension<AuthUser>>,
    Path(id): Path<String>,
    body: Option<Json<RemoveFactorBody>>,
) -> ApiResult<StatusCode> {
    let user = me_user(u)?;
    remove_factor(
        &state,
        &user,
        &id,
        body.map(|Json(b)| b).unwrap_or_default(),
    )
    .await
}

async fn delete_factor_post(
    State(state): St,
    u: Option<Extension<AuthUser>>,
    Path(id): Path<String>,
    Json(body): Json<RemoveFactorBody>,
) -> ApiResult<StatusCode> {
    let user = me_user(u)?;
    remove_factor(&state, &user, &id, body).await
}

// ===========================================================================
// second-factor administration
// ===========================================================================

async fn get_tfa_policy(State(state): St) -> Json<Value> {
    let p = tfa_policy(&state).await;
    Json(json!({
        "global_enforcement": p.global_enforcement,
        "required_for_roles": p.required_for_roles,
        "allowed_methods": ["totp", "webauthn", "recovery"],
        "grace_period_days": 0,
        "require_backup_codes": true,
        "max_trusted_devices": 0,
        "trusted_device_expiry_days": 0,
    }))
}

async fn put_tfa_policy(State(state): St, Json(p): Json<Value>) -> ApiResult<StatusCode> {
    let policy = json!({
        "global_enforcement": p["global_enforcement"].as_bool().unwrap_or(false),
        "required_for_roles": p["required_for_roles"].as_array().cloned().unwrap_or_default(),
    });
    store::set_setting(state.database.pool(), "tfa_policy", &policy.to_string()).await?;
    Ok(StatusCode::OK)
}

async fn mfa_users(State(state): St) -> ApiResult<Json<Vec<Value>>> {
    let pool = state.database.pool();
    let mut out = vec![];
    for u in store::list_user_records(pool).await? {
        let factors = store::list_tfa(pool, &u.id).await?;
        let user = resolve_user(&state, &u.id).await?;
        let required = tfa_required(&state, &user).await?;
        out.push(json!({
            "user_id": u.id,
            "username": u.username,
            "email": u.email,
            "mfa_enabled": !factors.is_empty(),
            "methods_count": factors.len(),
            "last_mfa_activity": factors.iter().filter_map(|f| f.last_used).max()
                .and_then(|t| chrono::DateTime::from_timestamp(t, 0)).map(|d| d.to_rfc3339()),
            "enforcement_status": match (required, factors.is_empty()) {
                (true, true) => "required_not_configured",
                (true, false) => "enforced",
                (false, false) => "enabled",
                (false, true) => "optional",
            },
        }));
    }
    Ok(Json(out))
}

async fn mfa_reset_user(
    State(state): St,
    u: Option<Extension<AuthUser>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let caller = me_user(u)?;
    let p = principal_of(&state, &caller).await?;
    let target = resolve_user(&state, &id).await?;
    guard_admin_target(&state, &p, &target).await?;
    store::delete_all_tfa(state.database.pool(), &target.id).await?;
    audit(
        &state,
        AuditEventType::TwoFactorDisabled,
        true,
        Some(&caller.username),
        None,
        "tfa-reset",
        Some(format!("reset second factors of {}", target.username)),
    )
    .await;
    Ok(StatusCode::OK)
}

async fn mfa_enforce_user(
    State(state): St,
    u: Option<Extension<AuthUser>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let caller = me_user(u)?;
    let p = principal_of(&state, &caller).await?;
    let target = resolve_user(&state, &id).await?;
    guard_admin_target(&state, &p, &target).await?;
    store::update_user(
        state.database.pool(),
        &target.id,
        &UserUpdate {
            tfa_required: Some(true),
            ..Default::default()
        },
    )
    .await?;
    Ok(StatusCode::OK)
}

// ===========================================================================
// users
// ===========================================================================

async fn list_users(State(state): St) -> ApiResult<Json<Vec<store::UserRecord>>> {
    Ok(Json(store::list_user_records(state.database.pool()).await?))
}

async fn get_user(State(state): St, Path(id): Path<String>) -> ApiResult<Json<store::UserRecord>> {
    store::get_user_record(state.database.pool(), &id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("user '{}' not found", id)))
}

#[derive(Deserialize)]
struct CreateUserBody {
    username: String,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    email: String,
    #[serde(default)]
    realm: Option<String>,
    #[serde(default = "yes")]
    enabled: bool,
    #[serde(default)]
    comment: Option<String>,
    /// Legacy: "admin" grants Administrator on `/`.
    #[serde(default)]
    role: Option<String>,
    /// Roles to grant on `/`.
    #[serde(default)]
    roles: Vec<String>,
    #[serde(default)]
    groups: Vec<String>,
    #[serde(default)]
    tfa_required: bool,
}

fn yes() -> bool {
    true
}

async fn create_user(
    State(state): St,
    u: Option<Extension<AuthUser>>,
    Json(req): Json<CreateUserBody>,
) -> ApiResult<(StatusCode, Json<store::UserRecord>)> {
    let caller = me_user(u)?;
    let p = principal_of(&state, &caller).await?;
    let pool = state.database.pool();
    let realm = req
        .realm
        .clone()
        .filter(|r| !r.is_empty() && r != "pve")
        .unwrap_or_else(|| "local".into());
    let (username, hash) = if realm == "local" {
        validate_username(&req.username)?;
        let pw = req.password.clone().unwrap_or_default();
        validate_password(&pw)?;
        (req.username.clone(), password::hash_password(&pw)?)
    } else {
        // pre-create an external account (needed when the realm has autocreate off)
        if store::get_realm(pool, &realm).await?.is_none() {
            return Err(ApiError::ValidationError(format!(
                "unknown realm '{}'",
                realm
            )));
        }
        let base = req
            .username
            .trim_end_matches(&format!("@{}", realm))
            .to_string();
        validate_username(&base)?;
        (
            format!("{}@{}", base, realm),
            realms::EXTERNAL_PASSWORD.to_string(),
        )
    };
    let mut grant: Vec<String> = req.roles.clone();
    if matches!(req.role.as_deref(), Some("admin") | Some("Administrator")) {
        grant.push("Administrator".into());
    } else if let Some(r) = req
        .role
        .as_deref()
        .filter(|r| !r.is_empty() && *r != "user")
    {
        grant.push(r.to_string());
    }
    grant.sort();
    grant.dedup();
    if !grant.is_empty() && !p.has(Privilege::PermissionsModify, "/") {
        return Err(ApiError::Forbidden(
            "granting roles on / needs Permissions.Modify on /".into(),
        ));
    }
    let user = User {
        id: uuid::Uuid::new_v4().to_string(),
        username,
        password_hash: hash,
        email: req.email,
        role: if grant.iter().any(|g| g == "Administrator") {
            "admin".into()
        } else {
            "user".into()
        },
        realm,
        enabled: req.enabled,
        roles: vec![],
        comment: req.comment,
    };
    store::insert_user(pool, &user).await?;
    for r in &grant {
        if let Err(e) = store::add_acl(pool, "/", "user", &user.id, r, true).await {
            store::delete_user(pool, &user.id).await?;
            return Err(e.into());
        }
    }
    for g in &req.groups {
        if let Err(e) = require_group_grant_rights(&state, &caller, g).await {
            store::delete_user(pool, &user.id).await?;
            return Err(e);
        }
        store::add_group_member(pool, g, &user.id).await?;
    }
    if req.tfa_required {
        store::update_user(
            pool,
            &user.id,
            &UserUpdate {
                tfa_required: Some(true),
                ..Default::default()
            },
        )
        .await?;
    }
    audit(
        &state,
        AuditEventType::ConfigChanged,
        true,
        Some(&caller.username),
        None,
        "user-create",
        Some(format!("created {} roles={:?}", user.username, grant)),
    )
    .await;
    let rec = store::get_user_record(pool, &user.id)
        .await?
        .ok_or_else(|| ApiError::Internal("user vanished".into()))?;
    Ok((StatusCode::CREATED, Json(rec)))
}

#[derive(Deserialize)]
struct UpdateUserBody {
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    comment: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    tfa_required: Option<bool>,
}

async fn update_user(
    State(state): St,
    u: Option<Extension<AuthUser>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateUserBody>,
) -> ApiResult<Json<store::UserRecord>> {
    let caller = me_user(u)?;
    let p = principal_of(&state, &caller).await?;
    let target = resolve_user(&state, &id).await?;
    guard_admin_target(&state, &p, &target).await?;
    if req.enabled == Some(false) && target.id == caller.user_id {
        return Err(ApiError::ValidationError(
            "you can't disable your own account".into(),
        ));
    }
    if req.enabled == Some(false) {
        ensure_not_last_admin(&state, &target).await?;
    }
    store::update_user(
        state.database.pool(),
        &target.id,
        &UserUpdate {
            email: req.email,
            comment: req.comment,
            enabled: req.enabled,
            tfa_required: req.tfa_required,
        },
    )
    .await?;
    get_user(State(state), Path(target.id)).await
}

async fn ensure_not_last_admin(state: &AppState, target: &User) -> ApiResult<()> {
    let pool = state.database.pool();
    let tp = Principal::load(pool, &target.id).await?;
    if !tp.is_admin() {
        return Ok(());
    }
    for other in store::list_user_records(pool).await? {
        if other.id != target.id
            && other.enabled
            && Principal::load(pool, &other.id).await?.is_admin()
        {
            return Ok(());
        }
    }
    Err(ApiError::ValidationError(
        "that is the last enabled administrator".into(),
    ))
}

#[derive(Deserialize)]
struct ToggleBody {
    enabled: bool,
}

async fn toggle_user(
    State(state): St,
    u: Option<Extension<AuthUser>>,
    Path(id): Path<String>,
    Json(req): Json<ToggleBody>,
) -> ApiResult<Json<store::UserRecord>> {
    update_user(
        State(state),
        u,
        Path(id),
        Json(UpdateUserBody {
            email: None,
            comment: None,
            enabled: Some(req.enabled),
            tfa_required: None,
        }),
    )
    .await
}

async fn delete_user(
    State(state): St,
    u: Option<Extension<AuthUser>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let caller = me_user(u)?;
    let p = principal_of(&state, &caller).await?;
    let target = resolve_user(&state, &id).await?;
    if target.id == caller.user_id {
        return Err(ApiError::ValidationError(
            "you can't delete your own account".into(),
        ));
    }
    guard_admin_target(&state, &p, &target).await?;
    ensure_not_last_admin(&state, &target).await?;
    store::delete_user(state.database.pool(), &target.id).await?;
    audit(
        &state,
        AuditEventType::ConfigChanged,
        true,
        Some(&caller.username),
        None,
        "user-delete",
        Some(format!("deleted {}", target.username)),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct SetPasswordBody {
    #[serde(default)]
    old_password: Option<String>,
    new_password: String,
}

async fn set_user_password(
    State(state): St,
    u: Option<Extension<AuthUser>>,
    Path(id): Path<String>,
    Json(req): Json<SetPasswordBody>,
) -> ApiResult<StatusCode> {
    let caller = me_user(u)?;
    let target = resolve_user(&state, &id).await?;
    if target.id == caller.user_id {
        // your own password: prove you know the current one
        let old = req
            .old_password
            .as_deref()
            .ok_or_else(|| ApiError::ValidationError("old_password is required".into()))?;
        change_password_checked(&state, &target, Some(old), &req.new_password).await?;
    } else {
        let p = principal_of(&state, &caller).await?;
        guard_admin_target(&state, &p, &target).await?;
        change_password_checked(&state, &target, None, &req.new_password).await?;
        store::delete_user_sessions(state.database.pool(), &target.id).await?;
    }
    audit(
        &state,
        AuditEventType::PasswordChanged,
        true,
        Some(&caller.username),
        None,
        "password",
        Some(format!("for {}", target.username)),
    )
    .await;
    Ok(StatusCode::OK)
}

async fn delete_user_sessions(
    State(state): St,
    u: Option<Extension<AuthUser>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let caller = me_user(u)?;
    let target = resolve_user(&state, &id).await?;
    if target.id != caller.user_id {
        let p = principal_of(&state, &caller).await?;
        guard_admin_target(&state, &p, &target).await?;
    }
    let n = store::delete_user_sessions(state.database.pool(), &target.id).await?;
    Ok(Json(json!({ "deleted": n })))
}

#[derive(Deserialize)]
struct GroupsBody {
    groups: Vec<String>,
}

async fn set_user_groups(
    State(state): St,
    u: Option<Extension<AuthUser>>,
    Path(id): Path<String>,
    Json(req): Json<GroupsBody>,
) -> ApiResult<Json<store::UserRecord>> {
    let caller = me_user(u)?;
    let p = principal_of(&state, &caller).await?;
    let target = resolve_user(&state, &id).await?;
    guard_admin_target(&state, &p, &target).await?;
    let pool = state.database.pool();
    let current = store::user_groups(pool, &target.id).await?;
    for g in req.groups.iter().filter(|g| !current.contains(g)) {
        require_group_grant_rights(&state, &caller, g).await?;
    }
    sqlx::query("DELETE FROM auth_group_members WHERE user_id = ?")
        .bind(&target.id)
        .execute(pool)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    for g in &req.groups {
        store::add_group_member(pool, g, &target.id).await?;
    }
    get_user(State(state), Path(target.id)).await
}

/// Effective privileges per path the user has grants on.
async fn user_permissions(State(state): St, Path(id): Path<String>) -> ApiResult<Json<Vec<Value>>> {
    let target = resolve_user(&state, &id).await?;
    let p = Principal::load(state.database.pool(), &target.id).await?;
    let mut paths: Vec<String> = p.acl.iter().map(|e| e.path.clone()).collect();
    paths.push("/".into());
    paths.sort();
    paths.dedup();
    Ok(Json(
        paths
            .into_iter()
            .map(|path| {
                let grants: Vec<Value> = p
                    .acl
                    .iter()
                    .filter(|e| e.path == path)
                    .map(|e| json!({"role": e.role, "via": e.subject_type, "subject": e.subject, "propagate": e.propagate}))
                    .collect();
                json!({
                    "path": path,
                    "privileges": p.privileges(&path).iter().map(|x| x.name()).collect::<Vec<_>>(),
                    "grants": grants,
                })
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
struct UserPermissionBody {
    path: String,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    propagate: Option<bool>,
    /// UI legacy shape: privileges without a role are not supported.
    #[serde(default)]
    privileges: Option<Vec<String>>,
}

async fn require_acl_modify(state: &AppState, caller: &AuthUser, path: &str) -> ApiResult<()> {
    let p = principal_of(state, caller).await?;
    if !p.has(Privilege::PermissionsModify, path) {
        return Err(ApiError::Forbidden(format!(
            "Permission check failed: Permissions.Modify on {}",
            crate::auth::acl::normalize_path(path)
        )));
    }
    Ok(())
}

/// Putting someone into a group hands them the group's grants, so it takes
/// the right to make those grants (Permissions.Modify on each granted path).
async fn require_group_grant_rights(
    state: &AppState,
    caller: &AuthUser,
    group: &str,
) -> ApiResult<()> {
    let p = principal_of(state, caller).await?;
    for e in store::list_acl(state.database.pool()).await? {
        if e.subject_type == "group"
            && e.subject == group
            && !p.has(Privilege::PermissionsModify, &e.path)
        {
            return Err(ApiError::Forbidden(format!(
                "group '{}' holds {} on {}: changing its members needs Permissions.Modify there",
                group, e.role, e.path
            )));
        }
    }
    Ok(())
}

async fn add_user_permission(
    State(state): St,
    u: Option<Extension<AuthUser>>,
    Path(id): Path<String>,
    Json(req): Json<UserPermissionBody>,
) -> ApiResult<StatusCode> {
    let caller = me_user(u)?;
    require_acl_modify(&state, &caller, &req.path).await?;
    let target = resolve_user(&state, &id).await?;
    let role = match (req.role, req.privileges) {
        (Some(r), _) => r,
        (None, Some(_)) => {
            return Err(ApiError::ValidationError(
                "grant a role (create a custom role for a custom privilege set)".into(),
            ))
        }
        (None, None) => return Err(ApiError::ValidationError("role is required".into())),
    };
    store::add_acl(
        state.database.pool(),
        &req.path,
        "user",
        &target.id,
        &role,
        req.propagate.unwrap_or(true),
    )
    .await?;
    audit(
        &state,
        AuditEventType::RoleAssigned,
        true,
        Some(&caller.username),
        None,
        "acl-add",
        Some(format!("{} on {} to {}", role, req.path, target.username)),
    )
    .await;
    Ok(StatusCode::CREATED)
}

#[derive(Deserialize)]
struct RemovePermissionQuery {
    path: String,
    #[serde(default)]
    role: Option<String>,
}

async fn remove_user_permission(
    State(state): St,
    u: Option<Extension<AuthUser>>,
    Path(id): Path<String>,
    Query(q): Query<RemovePermissionQuery>,
) -> ApiResult<StatusCode> {
    let caller = me_user(u)?;
    require_acl_modify(&state, &caller, &q.path).await?;
    let target = resolve_user(&state, &id).await?;
    let n = store::delete_acl(
        state.database.pool(),
        &q.path,
        "user",
        &target.id,
        q.role.as_deref(),
    )
    .await?;
    if n == 0 {
        return Err(ApiError::NotFound("no such grant".into()));
    }
    audit(
        &state,
        AuditEventType::RoleRevoked,
        true,
        Some(&caller.username),
        None,
        "acl-remove",
        Some(format!(
            "{:?} on {} from {}",
            q.role, q.path, target.username
        )),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

// ----- API keys -----------------------------------------------------------

#[derive(Deserialize)]
struct CreateApiKeyBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    comment: Option<String>,
    #[serde(default)]
    expires_days: Option<i64>,
    /// absolute expiry (unix seconds), as the UI sends it
    #[serde(default)]
    expire: Option<i64>,
}

/// Keys act with the owner's full rights: minting one for an administrator
/// takes administrator rights.
async fn guard_key_owner(state: &AppState, caller: &AuthUser, target: &User) -> ApiResult<()> {
    if caller.user_id != target.id {
        let p = principal_of(state, caller).await?;
        guard_admin_target(state, &p, target).await?;
    }
    Ok(())
}

async fn create_api_key(
    State(state): St,
    u: Option<Extension<AuthUser>>,
    Path(id): Path<String>,
    Json(req): Json<CreateApiKeyBody>,
) -> ApiResult<Json<Value>> {
    let caller = me_user(u)?;
    let target = resolve_user(&state, &id).await?;
    guard_key_owner(&state, &caller, &target).await?;
    let key = password::generate_api_key();
    let hash = password::hash_password(&key)?;
    let expires_at = req
        .expire
        .filter(|e| *e > 0)
        .or_else(|| req.expires_days.map(|d| store::now() + d * 86400));
    let key_id = uuid::Uuid::new_v4().to_string();
    let name = req.name.or(req.comment).unwrap_or_else(|| "API key".into());
    sqlx::query(
        "INSERT INTO api_keys (id, user_id, key_hash, name, expires_at, enabled) VALUES (?, ?, ?, ?, ?, 1)",
    )
    .bind(&key_id)
    .bind(&target.id)
    .bind(&hash)
    .bind(&name)
    .bind(expires_at)
    .execute(state.database.pool())
    .await
    .map_err(|e| ApiError::Internal(format!("Failed to create API key: {}", e)))?;
    Ok(Json(json!({
        "id": key_id,
        "key": key,
        "name": name,
        "comment": name,
        "user": target.username,
        "enabled": true,
        "expires_at": expires_at,
        "expire": expires_at,
    })))
}

async fn list_api_keys(State(state): St, Path(id): Path<String>) -> ApiResult<Json<Vec<Value>>> {
    use sqlx::Row;
    let target = resolve_user(&state, &id).await?;
    let rows = sqlx::query(
        "SELECT id, name, enabled, expires_at, created_at, last_used_at FROM api_keys WHERE user_id = ? ORDER BY created_at DESC",
    )
    .bind(&target.id)
    .fetch_all(state.database.pool())
    .await
    .map_err(|e| ApiError::Internal(format!("Failed to list API keys: {}", e)))?;
    Ok(Json(
        rows.iter()
            .map(|r| {
                let name: String = r.get("name");
                json!({
                    "id": r.get::<String, _>("id"),
                    "name": name,
                    "comment": name,
                    "user": target.username,
                    "enabled": r.get::<bool, _>("enabled"),
                    "expires_at": r.get::<Option<i64>, _>("expires_at"),
                    "expire": r.get::<Option<i64>, _>("expires_at"),
                    "created_at": r.get::<Option<String>, _>("created_at"),
                    "last_used_at": r.get::<Option<String>, _>("last_used_at"),
                    "last_used": r.get::<Option<String>, _>("last_used_at"),
                })
            })
            .collect(),
    ))
}

async fn revoke_api_key(
    State(state): St,
    Path((id, key_id)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    let target = resolve_user(&state, &id).await?;
    let r = sqlx::query("DELETE FROM api_keys WHERE id = ? AND user_id = ?")
        .bind(&key_id)
        .bind(&target.id)
        .execute(state.database.pool())
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to revoke API key: {}", e)))?;
    if r.rows_affected() == 0 {
        return Err(ApiError::NotFound("API key not found".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn set_api_key_fields(
    state: &AppState,
    user: &str,
    key_id: &str,
    name: Option<String>,
    enabled: Option<bool>,
    expire: Option<i64>,
) -> ApiResult<()> {
    let target = resolve_user(state, user).await?;
    let r = sqlx::query(
        "UPDATE api_keys SET name = COALESCE(?, name), enabled = COALESCE(?, enabled), expires_at = COALESCE(?, expires_at) WHERE id = ? AND user_id = ?",
    )
    .bind(name)
    .bind(enabled)
    .bind(expire)
    .bind(key_id)
    .bind(&target.id)
    .execute(state.database.pool())
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;
    if r.rows_affected() == 0 {
        return Err(ApiError::NotFound("API key not found".into()));
    }
    Ok(())
}

async fn update_api_key(
    State(state): St,
    Path((id, key_id)): Path<(String, String)>,
    Json(req): Json<CreateApiKeyBody>,
) -> ApiResult<StatusCode> {
    set_api_key_fields(
        &state,
        &id,
        &key_id,
        req.name.or(req.comment),
        None,
        req.expire,
    )
    .await?;
    Ok(StatusCode::OK)
}

async fn toggle_api_key(
    State(state): St,
    Path((id, key_id)): Path<(String, String)>,
    Json(req): Json<ToggleBody>,
) -> ApiResult<StatusCode> {
    set_api_key_fields(&state, &id, &key_id, None, Some(req.enabled), None).await?;
    Ok(StatusCode::OK)
}

// ===========================================================================
// roles / privileges / groups / ACL
// ===========================================================================

fn role_json(r: &RoleRecord) -> Value {
    let mut v = serde_json::to_value(r).unwrap_or_default();
    v["permissions"] = serde_json::to_value(&r.to_role().permissions).unwrap_or_default();
    v
}

async fn list_roles(State(state): St) -> ApiResult<Json<Vec<Value>>> {
    Ok(Json(
        store::list_roles(state.database.pool())
            .await?
            .iter()
            .map(role_json)
            .collect(),
    ))
}

async fn get_role(State(state): St, Path(name): Path<String>) -> ApiResult<Json<Value>> {
    store::get_role(state.database.pool(), &name)
        .await?
        .map(|r| Json(role_json(&r)))
        .ok_or_else(|| ApiError::NotFound(format!("role '{}' not found", name)))
}

/// Accepts `{name, description, privileges: [...]}` or the UI's
/// `{name, description, permissions: [{path, privileges}]}`.
fn parse_role_body(v: &Value, name: Option<&str>) -> ApiResult<RoleRecord> {
    let name = name
        .map(str::to_string)
        .or_else(|| v["name"].as_str().map(str::to_string))
        .ok_or_else(|| ApiError::ValidationError("name is required".into()))?;
    let mut names: Vec<String> = v["privileges"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if let Some(perms) = v["permissions"].as_array() {
        for p in perms {
            if let Some(a) = p["privileges"].as_array() {
                names.extend(a.iter().filter_map(|x| x.as_str().map(str::to_string)));
            }
        }
    }
    let mut privileges = Vec::new();
    for n in names {
        let p = Privilege::parse(&n)
            .ok_or_else(|| ApiError::ValidationError(format!("unknown privilege '{}'", n)))?;
        if !privileges.contains(&p) {
            privileges.push(p);
        }
    }
    Ok(RoleRecord {
        name,
        description: v["description"].as_str().unwrap_or_default().to_string(),
        privileges,
        builtin: false,
    })
}

async fn create_role(
    State(state): St,
    Json(v): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let role = parse_role_body(&v, None)?;
    store::upsert_role(state.database.pool(), &role, true).await?;
    Ok((StatusCode::CREATED, Json(role_json(&role))))
}

async fn update_role(
    State(state): St,
    Path(name): Path<String>,
    Json(v): Json<Value>,
) -> ApiResult<Json<Value>> {
    let role = parse_role_body(&v, Some(&name))?;
    store::upsert_role(state.database.pool(), &role, false).await?;
    Ok(Json(role_json(&role)))
}

async fn delete_role(State(state): St, Path(name): Path<String>) -> ApiResult<StatusCode> {
    store::delete_role(state.database.pool(), &name).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_privileges() -> Json<Vec<&'static str>> {
    Json(Privilege::ALL.iter().map(|p| p.name()).collect())
}

async fn list_groups(State(state): St) -> ApiResult<Json<Vec<store::GroupRecord>>> {
    Ok(Json(store::list_groups(state.database.pool()).await?))
}

#[derive(Deserialize)]
struct GroupBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    comment: Option<String>,
    /// user names or ids
    #[serde(default)]
    members: Option<Vec<String>>,
}

async fn member_ids(state: &AppState, members: &[String]) -> ApiResult<Vec<String>> {
    let mut ids = vec![];
    for m in members {
        ids.push(resolve_user(state, m).await?.id);
    }
    Ok(ids)
}

async fn create_group(State(state): St, Json(req): Json<GroupBody>) -> ApiResult<StatusCode> {
    let name = req
        .name
        .ok_or_else(|| ApiError::ValidationError("name is required".into()))?;
    let pool = state.database.pool();
    store::create_group(pool, &name, req.comment.as_deref()).await?;
    if let Some(m) = req.members {
        let ids = member_ids(&state, &m).await?;
        store::set_group_members(pool, &name, &ids).await?;
    }
    Ok(StatusCode::CREATED)
}

async fn update_group(
    State(state): St,
    u: Option<Extension<AuthUser>>,
    Path(name): Path<String>,
    Json(req): Json<GroupBody>,
) -> ApiResult<StatusCode> {
    let caller = me_user(u)?;
    let pool = state.database.pool();
    if req.members.is_some() {
        require_group_grant_rights(&state, &caller, &name).await?;
    }
    if !store::list_groups(pool)
        .await?
        .iter()
        .any(|g| g.name == name)
    {
        return Err(ApiError::NotFound(format!("group '{}' not found", name)));
    }
    if let Some(c) = req.comment {
        sqlx::query("UPDATE auth_groups SET comment = ? WHERE name = ?")
            .bind(c)
            .bind(&name)
            .execute(pool)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
    }
    if let Some(m) = req.members {
        let ids = member_ids(&state, &m).await?;
        store::set_group_members(pool, &name, &ids).await?;
    }
    Ok(StatusCode::OK)
}

async fn delete_group(State(state): St, Path(name): Path<String>) -> ApiResult<StatusCode> {
    if !store::delete_group(state.database.pool(), &name).await? {
        return Err(ApiError::NotFound(format!("group '{}' not found", name)));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn list_acl(State(state): St) -> ApiResult<Json<Vec<Value>>> {
    let pool = state.database.pool();
    let users = store::list_user_records(pool).await?;
    Ok(Json(
        store::list_acl(pool)
            .await?
            .into_iter()
            .map(|e| {
                let display = if e.subject_type == "user" {
                    users
                        .iter()
                        .find(|u| u.id == e.subject)
                        .map(|u| u.username.clone())
                        .unwrap_or_else(|| e.subject.clone())
                } else {
                    e.subject.clone()
                };
                let mut v = serde_json::to_value(&e).unwrap_or_default();
                v["subject_name"] = json!(display);
                v
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
struct AclBody {
    path: String,
    subject_type: String,
    /// user name/id or group name
    subject: String,
    role: String,
    #[serde(default = "yes")]
    propagate: bool,
}

async fn acl_subject(state: &AppState, subject_type: &str, subject: &str) -> ApiResult<String> {
    match subject_type {
        "user" => Ok(resolve_user(state, subject).await?.id),
        "group" => {
            if store::list_groups(state.database.pool())
                .await?
                .iter()
                .any(|g| g.name == subject)
            {
                Ok(subject.to_string())
            } else {
                Err(ApiError::ValidationError(format!(
                    "unknown group '{}'",
                    subject
                )))
            }
        }
        _ => Err(ApiError::ValidationError(
            "subject_type must be user or group".into(),
        )),
    }
}

async fn add_acl(
    State(state): St,
    u: Option<Extension<AuthUser>>,
    Json(req): Json<AclBody>,
) -> ApiResult<StatusCode> {
    let caller = me_user(u)?;
    require_acl_modify(&state, &caller, &req.path).await?;
    let subject = acl_subject(&state, &req.subject_type, &req.subject).await?;
    store::add_acl(
        state.database.pool(),
        &req.path,
        &req.subject_type,
        &subject,
        &req.role,
        req.propagate,
    )
    .await?;
    audit(
        &state,
        AuditEventType::RoleAssigned,
        true,
        Some(&caller.username),
        None,
        "acl-add",
        Some(format!(
            "{} on {} to {} {}",
            req.role, req.path, req.subject_type, req.subject
        )),
    )
    .await;
    Ok(StatusCode::CREATED)
}

#[derive(Deserialize)]
struct AclDeleteQuery {
    path: String,
    subject_type: String,
    subject: String,
    #[serde(default)]
    role: Option<String>,
}

async fn remove_acl(
    State(state): St,
    u: Option<Extension<AuthUser>>,
    Query(q): Query<AclDeleteQuery>,
) -> ApiResult<StatusCode> {
    let caller = me_user(u)?;
    require_acl_modify(&state, &caller, &q.path).await?;
    let subject = acl_subject(&state, &q.subject_type, &q.subject).await?;
    let pool = state.database.pool();
    // never remove the last administrator grant
    if q.path == "/"
        && q.subject_type == "user"
        && q.role
            .as_deref()
            .map(|r| r == "Administrator")
            .unwrap_or(true)
    {
        let target = resolve_user(&state, &subject).await?;
        ensure_not_last_admin(&state, &target).await?;
    }
    let n = store::delete_acl(pool, &q.path, &q.subject_type, &subject, q.role.as_deref()).await?;
    if n == 0 {
        return Err(ApiError::NotFound("no such ACL entry".into()));
    }
    audit(
        &state,
        AuditEventType::RoleRevoked,
        true,
        Some(&caller.username),
        None,
        "acl-remove",
        Some(format!(
            "{:?} on {} from {} {}",
            q.role, q.path, q.subject_type, q.subject
        )),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

// ===========================================================================
// realms
// ===========================================================================

async fn list_realms(State(state): St) -> ApiResult<Json<Vec<RealmRecord>>> {
    Ok(Json(
        store::list_realms(state.database.pool())
            .await?
            .into_iter()
            .map(realms::redact)
            .collect(),
    ))
}

async fn get_realm(State(state): St, Path(name): Path<String>) -> ApiResult<Json<RealmRecord>> {
    store::get_realm(state.database.pool(), &name)
        .await?
        .map(|r| Json(realms::redact(r)))
        .ok_or_else(|| ApiError::NotFound(format!("realm '{}' not found", name)))
}

async fn create_realm(
    State(state): St,
    u: Option<Extension<AuthUser>>,
    Json(r): Json<RealmRecord>,
) -> ApiResult<(StatusCode, Json<RealmRecord>)> {
    let caller = me_user(u)?;
    realms::validate(&r)?;
    let pool = state.database.pool();
    // new realm users join the default group: same rights as adding members
    if let Some(g) = r.default_group.as_deref().filter(|g| !g.is_empty()) {
        require_group_grant_rights(&state, &caller, g).await?;
    }
    if store::get_realm(pool, &r.name).await?.is_some() {
        return Err(ApiError::Conflict(format!(
            "realm '{}' already exists",
            r.name
        )));
    }
    if let Some(g) = r.default_group.as_deref().filter(|g| !g.is_empty()) {
        if !store::list_groups(pool).await?.iter().any(|x| x.name == g) {
            return Err(ApiError::ValidationError(format!("unknown group '{}'", g)));
        }
    }
    store::upsert_realm(pool, &r).await?;
    Ok((StatusCode::CREATED, Json(realms::redact(r))))
}

async fn update_realm(
    State(state): St,
    u: Option<Extension<AuthUser>>,
    Path(name): Path<String>,
    Json(mut r): Json<RealmRecord>,
) -> ApiResult<Json<RealmRecord>> {
    let caller = me_user(u)?;
    if let Some(g) = r.default_group.as_deref().filter(|g| !g.is_empty()) {
        require_group_grant_rights(&state, &caller, g).await?;
    }
    let pool = state.database.pool();
    let old = store::get_realm(pool, &name)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("realm '{}' not found", name)))?;
    r.name = name;
    realms::merge_secrets(&mut r, &old);
    realms::validate(&r)?;
    store::upsert_realm(pool, &r).await?;
    Ok(Json(realms::redact(r)))
}

async fn delete_realm(State(state): St, Path(name): Path<String>) -> ApiResult<StatusCode> {
    if !store::delete_realm(state.database.pool(), &name).await? {
        return Err(ApiError::NotFound(format!("realm '{}' not found", name)));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, Default)]
struct RealmTestBody {
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
}

/// Connectivity (and optionally a credential) check for a realm.
async fn test_realm(
    State(state): St,
    Path(name): Path<String>,
    body: Option<Json<RealmTestBody>>,
) -> ApiResult<Json<Value>> {
    let r = store::get_realm(state.database.pool(), &name)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("realm '{}' not found", name)))?;
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let result = match r.realm_type.as_str() {
        "ldap" | "ad" => {
            let cfg: realms::LdapRealmConfig = serde_json::from_value(r.config.clone())
                .map_err(|e| ApiError::ValidationError(e.to_string()))?;
            match (&body.username, &body.password) {
                (Some(u), Some(p)) => {
                    match realms::ldap_authenticate(&cfg, r.realm_type == "ad", u, p).await {
                        Ok(Some(i)) => {
                            json!({"ok": true, "message": format!("{} authenticated", u), "email": i.email})
                        }
                        Ok(None) => json!({"ok": false, "message": "credentials rejected"}),
                        Err(e) => json!({"ok": false, "message": e.to_string()}),
                    }
                }
                _ => match realms::ldap_test(&cfg).await {
                    Ok(m) => json!({"ok": true, "message": m}),
                    Err(e) => json!({"ok": false, "message": e.to_string()}),
                },
            }
        }
        "openid" => {
            let cfg: realms::OpenIdRealmConfig = serde_json::from_value(r.config.clone())
                .map_err(|e| ApiError::ValidationError(e.to_string()))?;
            match realms::openid_provider(&cfg, "").load_discovery().await {
                Ok(d) => json!({"ok": true, "message": format!("discovered issuer {}", d.issuer)}),
                Err(e) => json!({"ok": false, "message": e.to_string()}),
            }
        }
        "pam" => match (&body.username, &body.password) {
            (Some(u), Some(p)) => match realms::pam_authenticate(u, p).await {
                Ok(Some(_)) => json!({"ok": true, "message": format!("{} authenticated", u)}),
                Ok(None) => json!({"ok": false, "message": "credentials rejected"}),
                Err(e) => json!({"ok": false, "message": e.to_string()}),
            },
            _ => json!({"ok": cfg!(feature = "pam"), "message": if cfg!(feature = "pam") {
                format!("PAM service '{}'", realms::PAM_SERVICE)
            } else {
                "this build has no PAM support".to_string()
            }}),
        },
        _ => json!({"ok": false, "message": "unknown realm type"}),
    };
    Ok(Json(result))
}

/// The route→privilege table as data (documentation / UI help).
#[allow(dead_code)]
pub fn describe(method: &str, path: &str) -> String {
    format!("{:?}", privileges::requirement(method, path))
}
