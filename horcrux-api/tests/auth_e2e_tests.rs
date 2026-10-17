//! End-to-end access-control tests against the live server CI starts on
//! https://localhost:8006 (admin / ADMIN_PASSWORD=admin): RBAC on every
//! registered route, DB-backed user management, second factors (TOTP,
//! WebAuthn, recovery codes, enforcement), token revocation, privilege
//! escalation guards, collection filtering, HTTPS + redirect + certificate
//! hot swap.
//!
//! Override the server with HORCRUX_TEST_API (default https://localhost:8006).

use horcrux_api::auth::privileges::{requirement, Requirement};
use horcrux_api::auth::{totp, webauthn};
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};
use std::time::Duration;

fn base() -> String {
    std::env::var("HORCRUX_TEST_API").unwrap_or_else(|_| "https://localhost:8006".into())
}

fn api(path: &str) -> String {
    format!("{}/api{}", base(), path)
}

fn client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(60))
        .danger_accept_invalid_certs(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn unique(prefix: &str) -> String {
    format!(
        "{}-{}",
        prefix,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            % 100_000_000_000
    )
}

/// POST /auth/login (retrying on the auth rate limiter); raw JSON body.
async fn login_raw(c: &Client, username: &str, password: &str) -> (StatusCode, Value) {
    for _ in 0..60 {
        let r = c
            .post(api("/auth/login"))
            .json(&json!({"username": username, "password": password}))
            .send()
            .await
            .expect("horcrux-api must be running");
        if r.status() == StatusCode::TOO_MANY_REQUESTS {
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        }
        let s = r.status();
        return (s, r.json().await.unwrap_or(Value::Null));
    }
    panic!("login kept being rate limited");
}

async fn login(c: &Client, username: &str, password: &str) -> String {
    let (s, v) = login_raw(c, username, password).await;
    assert_eq!(s, 200, "login {username}: {v}");
    v["ticket"].as_str().expect("ticket").to_string()
}

async fn admin(c: &Client) -> String {
    let pw = std::env::var("ADMIN_PASSWORD").unwrap_or_else(|_| "admin".into());
    login(c, "admin", &pw).await
}

async fn call(
    c: &Client,
    tok: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let m = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
    let mut rb = c.request(m, api(path)).bearer_auth(tok);
    if let Some(b) = body {
        rb = rb.json(&b);
    }
    let r = rb.send().await.unwrap();
    let s = r.status();
    let text = r.text().await.unwrap_or_default();
    (
        s,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

/// Admin creates a local user, optionally granting `role` on `path`.
async fn make_user(
    c: &Client,
    adm: &str,
    name: &str,
    pw: &str,
    grant: Option<(&str, &str)>,
) -> String {
    let _ = call(c, adm, "DELETE", &format!("/users/{name}"), None).await;
    let (s, v) = call(
        c,
        adm,
        "POST",
        "/users",
        Some(json!({"username": name, "password": pw, "email": format!("{name}@example.com")})),
    )
    .await;
    assert_eq!(s, 201, "create {name}: {v}");
    if let Some((path, role)) = grant {
        let (s, v) = call(
            c,
            adm,
            "POST",
            "/acl",
            Some(json!({"path": path, "subject_type": "user", "subject": name, "role": role})),
        )
        .await;
        assert_eq!(s, 201, "grant {role} on {path} to {name}: {v}");
    }
    v["id"].as_str().unwrap().to_string()
}

/// Every `.route(...)` the server registers, with its methods.
fn registered_routes() -> Vec<(String, String)> {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = vec![src.join("main.rs")];
    for e in std::fs::read_dir(src.join("api")).unwrap().flatten() {
        files.push(e.path());
    }
    let route = regex::Regex::new(r#"\.route\(\s*"([^"]+)",\s*((?:[^()]|\([^()]*\))*)\)"#).unwrap();
    let method = regex::Regex::new(r"\b(get|post|put|delete|patch)\(").unwrap();
    let mut out = vec![];
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap();
        for cap in route.captures_iter(&text) {
            for m in method.captures_iter(&cap[2]) {
                out.push((m[1].to_uppercase(), cap[1].to_string()));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// The core regression: an authenticated account with no grants must get
/// 403 from every route that needs a privilege. This used to be 200 on all
/// of them (RBAC middleware never applied, and anyone could self-register).
#[tokio::test]
async fn every_route_enforces_rbac_for_an_unprivileged_user() {
    let c = client();
    let adm = admin(&c).await;
    let name = unique("nobody");
    make_user(&c, &adm, &name, "nobody-pass-1", None).await;
    let tok = login(&c, &name, "nobody-pass-1").await;

    let routes = registered_routes();
    assert!(routes.len() > 300, "only {} routes parsed", routes.len());
    let mut checked = 0;
    let mut leaks = vec![];
    for (method, pattern) in &routes {
        let path = pattern
            .split('/')
            .map(|s| if s.starts_with(':') { "rbac-probe" } else { s })
            .collect::<Vec<_>>()
            .join("/");
        let expect_denied = match requirement(method, &path) {
            Requirement::Public | Requirement::Authenticated => false,
            Requirement::SelfOr { user, .. } => user != name,
            Requirement::Privilege { .. } | Requirement::AnyBelow { .. } => true,
        };
        if !expect_denied {
            continue;
        }
        let url = format!("{}{}", base(), path);
        let m = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
        let r = c
            .request(m, &url)
            .bearer_auth(&tok)
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        checked += 1;
        if r.status() != StatusCode::FORBIDDEN {
            leaks.push(format!("{method} {pattern} -> {}", r.status()));
        }
    }
    let _ = call(&c, &adm, "DELETE", &format!("/users/{name}"), None).await;
    assert!(checked > 250, "only {checked} routes needed a privilege?");
    assert!(
        leaks.is_empty(),
        "routes reachable without permission:\n{}",
        leaks.join("\n")
    );
}

#[tokio::test]
async fn self_registration_is_off_and_users_live_in_the_database() {
    let c = client();
    let r = c
        .post(api("/auth/register"))
        .json(&json!({"username": unique("selfreg"), "password": "selfreg-123"}))
        .send()
        .await
        .unwrap();
    assert!(
        matches!(r.status().as_u16(), 403 | 429),
        "register: {}",
        r.status()
    );

    // a user created in the UI's API can log in (it used to land in an
    // in-memory list the login never read)
    let adm = admin(&c).await;
    let name = unique("dbuser");
    let id = make_user(&c, &adm, &name, "db-user-pass", None).await;
    let (_, users) = call(&c, &adm, "GET", "/users", None).await;
    assert!(users
        .as_array()
        .unwrap()
        .iter()
        .any(|u| u["id"] == id.as_str()));
    assert!(
        users
            .as_array()
            .unwrap()
            .iter()
            .all(|u| u.get("password_hash").is_none()),
        "hashes leaked"
    );
    let tok = login(&c, &name, "db-user-pass").await;
    let (s, me) = call(&c, &tok, "GET", "/auth/me", None).await;
    assert_eq!(s, 200);
    assert_eq!(me["user"]["username"], name.as_str());
    assert_eq!(me["is_admin"], false);

    // disabling the account kills its live token at once
    let (s, _) = call(
        &c,
        &adm,
        "POST",
        &format!("/users/{id}/toggle"),
        Some(json!({"enabled": false})),
    )
    .await;
    assert_eq!(s, 200);
    let (s, _) = call(&c, &tok, "GET", "/auth/me", None).await;
    assert_eq!(s, 401, "token of a disabled account");
    let (s, _) = login_raw(&c, &name, "db-user-pass").await;
    assert_eq!(s, 401, "disabled account can't log in");
    let (s, _) = call(&c, &adm, "DELETE", &format!("/users/{id}"), None).await;
    assert_eq!(s, 204);
}

#[tokio::test]
async fn logout_revokes_the_bearer_token() {
    let c = client();
    let adm = admin(&c).await;
    let name = unique("logout");
    make_user(&c, &adm, &name, "logout-pass-1", None).await;
    let tok = login(&c, &name, "logout-pass-1").await;
    assert_eq!(call(&c, &tok, "GET", "/auth/verify", None).await.0, 200);
    assert_eq!(call(&c, &tok, "POST", "/auth/logout", None).await.0, 200);
    assert_eq!(call(&c, &tok, "GET", "/auth/verify", None).await.0, 401);
    let _ = call(&c, &adm, "DELETE", &format!("/users/{name}"), None).await;
}

#[tokio::test]
async fn totp_enrollment_login_and_recovery_codes() {
    let c = client();
    let adm = admin(&c).await;
    let name = unique("totp");
    make_user(&c, &adm, &name, "totp-pass-1", Some(("/vms", "VMUser"))).await;
    let tok = login(&c, &name, "totp-pass-1").await;

    let (s, setup) = call(&c, &tok, "POST", "/auth/mfa/totp/setup", Some(json!({}))).await;
    assert_eq!(s, 200, "{setup}");
    let secret = setup["secret"].as_str().unwrap().to_string();
    assert!(setup["qr_code_url"]
        .as_str()
        .unwrap()
        .starts_with("otpauth://totp/Horcrux:"));
    // a wrong code doesn't enroll
    let (s, _) = call(
        &c,
        &tok,
        "POST",
        "/auth/mfa/totp/verify",
        Some(json!({"setup_id": setup["setup_id"], "code": "000000"})),
    )
    .await;
    assert_eq!(s, 422);
    let code = totp::code_at(&secret, now()).unwrap();
    let (s, v) = call(
        &c,
        &tok,
        "POST",
        "/auth/mfa/totp/verify",
        Some(json!({"setup_id": setup["setup_id"], "code": code, "name": "phone"})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let codes: Vec<String> = serde_json::from_value(v["recovery_codes"].clone()).unwrap();
    assert_eq!(codes.len(), 10);

    // password alone is no longer enough
    let (s, ch) = login_raw(&c, &name, "totp-pass-1").await;
    assert_eq!(s, 200);
    assert_eq!(ch["tfa_required"], true, "{ch}");
    assert!(
        ch.get("ticket")
            .and_then(|t| t.as_str())
            .map(|t| !t.contains('.'))
            .unwrap_or(false),
        "no JWT before the second factor"
    );
    let methods = ch["methods"].as_array().unwrap();
    assert!(methods.iter().any(|m| m == "totp") && methods.iter().any(|m| m == "recovery"));
    let ticket = ch["ticket"].as_str().unwrap().to_string();
    // wrong code
    let r = c
        .post(api("/auth/tfa"))
        .json(&json!({"ticket": ticket, "method": "totp", "code": "123456"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    // next step's code (the current one was consumed by enrollment: replay-protected)
    let code = totp::code_at(&secret, now() + 30).unwrap();
    let r = c
        .post(api("/auth/tfa"))
        .json(&json!({"ticket": ticket, "method": "totp", "code": code}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    let tok2 = v["ticket"].as_str().unwrap().to_string();
    assert_eq!(call(&c, &tok2, "GET", "/vms", None).await.0, 200);
    // the ticket is single use
    let r = c
        .post(api("/auth/tfa"))
        .json(&json!({"ticket": ticket, "method": "totp", "code": code}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);

    // a recovery code works exactly once
    let (_, ch) = login_raw(&c, &name, "totp-pass-1").await;
    let r = c
        .post(api("/auth/tfa"))
        .json(&json!({"ticket": ch["ticket"], "method": "recovery", "code": codes[0]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let (_, ch) = login_raw(&c, &name, "totp-pass-1").await;
    let r = c
        .post(api("/auth/tfa"))
        .json(&json!({"ticket": ch["ticket"], "method": "recovery", "code": codes[0]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401, "recovery code reused");
    let (_, st) = call(&c, &tok2, "GET", "/auth/mfa/status", None).await;
    assert_eq!(st["backup_codes_remaining"], 9);

    // removing the factor needs proof
    let id = st["methods"][0]["id"].as_str().unwrap().to_string();
    let (s, _) = call(
        &c,
        &tok2,
        "POST",
        &format!("/auth/mfa/methods/{id}/disable"),
        Some(json!({"code": "000000"})),
    )
    .await;
    assert_eq!(s, 403);
    let (s, _) = call(
        &c,
        &tok2,
        "POST",
        &format!("/auth/mfa/methods/{id}/disable"),
        Some(json!({"password": "totp-pass-1"})),
    )
    .await;
    assert_eq!(s, 204);
    let (s, v) = login_raw(&c, &name, "totp-pass-1").await;
    assert_eq!(s, 200);
    assert!(v.get("tfa_required").is_none(), "factor removed: {v}");
    let _ = call(&c, &adm, "DELETE", &format!("/users/{name}"), None).await;
}

#[tokio::test]
async fn webauthn_security_key_registration_and_login() {
    let c = client();
    let adm = admin(&c).await;
    let name = unique("webauthn");
    make_user(&c, &adm, &name, "webauthn-pass-1", Some(("/vms", "VMUser"))).await;
    let tok = login(&c, &name, "webauthn-pass-1").await;
    let host = base().trim_start_matches("https://").to_string();
    let origin = format!("https://{host}");

    let mut key = webauthn::soft::SoftAuthenticator::new();
    let (s, start) = call(
        &c,
        &tok,
        "POST",
        "/auth/mfa/webauthn/register",
        Some(json!({"name": "yubikey"})),
    )
    .await;
    assert_eq!(s, 200, "{start}");
    assert_eq!(
        start["publicKey"]["rp"]["id"],
        host.split(':').next().unwrap()
    );
    // a registration answered for the wrong origin (phishing proxy) fails
    let bad = key.register(&start["publicKey"], "https://evil.example");
    let (s, _) = call(
        &c,
        &tok,
        "POST",
        "/auth/mfa/webauthn/register/finish",
        Some(json!({"challenge_id": start["challenge_id"], "credential": bad})),
    )
    .await;
    assert_eq!(s, 422, "registration for a foreign origin is rejected");
    let (_, start) = call(
        &c,
        &tok,
        "POST",
        "/auth/mfa/webauthn/register",
        Some(json!({"name": "yubikey"})),
    )
    .await;
    let cred = key.register(&start["publicKey"], &origin);
    let (s, v) = call(
        &c,
        &tok,
        "POST",
        "/auth/mfa/webauthn/register/finish",
        Some(json!({"challenge_id": start["challenge_id"], "credential": cred})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["recovery_codes"].as_array().unwrap().len(), 10);

    let (_, ch) = login_raw(&c, &name, "webauthn-pass-1").await;
    assert_eq!(ch["tfa_required"], true);
    assert!(ch["methods"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m == "webauthn"));
    let assertion = key.assert(&ch["webauthn"], &origin);
    let r = c
        .post(api("/auth/tfa"))
        .json(&json!({"ticket": ch["ticket"], "method": "webauthn", "credential": assertion}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!(
        call(&c, v["ticket"].as_str().unwrap(), "GET", "/vms", None)
            .await
            .0,
        200
    );

    // a cloned key replaying an old signature counter is refused
    let (_, ch) = login_raw(&c, &name, "webauthn-pass-1").await;
    key.counter = 0;
    let replay = key.assert(&ch["webauthn"], &origin);
    let r = c
        .post(api("/auth/tfa"))
        .json(&json!({"ticket": ch["ticket"], "method": "webauthn", "credential": replay}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let _ = call(&c, &adm, "DELETE", &format!("/users/{name}"), None).await;
}

#[tokio::test]
async fn enforced_second_factor_forces_enrollment_first() {
    let c = client();
    let adm = admin(&c).await;
    let name = unique("enforced");
    let id = make_user(&c, &adm, &name, "enforced-pass-1", Some(("/vms", "VMUser"))).await;
    let (s, _) = call(
        &c,
        &adm,
        "POST",
        &format!("/auth/mfa/users/{id}/enforce"),
        Some(json!({})),
    )
    .await;
    assert_eq!(s, 200);
    let (s, v) = login_raw(&c, &name, "enforced-pass-1").await;
    assert_eq!(s, 200);
    assert_eq!(v["tfa_setup_required"], true, "{v}");
    let tok = v["ticket"].as_str().unwrap().to_string();
    let (s, body) = call(&c, &tok, "GET", "/vms", None).await;
    assert_eq!(s, 403, "restricted login can't use the API: {body}");
    assert_eq!(body["error"], "tfa_setup_required");
    let (_, setup) = call(&c, &tok, "POST", "/auth/mfa/totp/setup", Some(json!({}))).await;
    let code = totp::code_at(setup["secret"].as_str().unwrap(), now()).unwrap();
    let (s, _) = call(
        &c,
        &tok,
        "POST",
        "/auth/mfa/totp/verify",
        Some(json!({"setup_id": setup["setup_id"], "code": code})),
    )
    .await;
    assert_eq!(s, 200);
    // the same session is upgraded once a factor exists
    assert_eq!(call(&c, &tok, "GET", "/vms", None).await.0, 200);
    let (_, users) = call(&c, &adm, "GET", "/auth/mfa/users", None).await;
    let me = users
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["user_id"] == id.as_str())
        .unwrap()
        .clone();
    assert_eq!(me["enforcement_status"], "enforced");
    let _ = call(&c, &adm, "DELETE", &format!("/users/{id}"), None).await;
}

#[tokio::test]
async fn user_admins_cannot_escalate() {
    let c = client();
    let adm = admin(&c).await;
    let name = unique("useradmin");
    make_user(&c, &adm, &name, "useradmin-pass", Some(("/", "UserAdmin"))).await;
    let tok = login(&c, &name, "useradmin-pass").await;
    // may manage ordinary accounts
    let victim = unique("victim");
    let (s, v) = call(
        &c,
        &tok,
        "POST",
        "/users",
        Some(json!({"username": victim, "password": "victim-pass-1"})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    // ... but not grant roles, touch the admin, or mint keys as the admin
    let (s, _) = call(
        &c,
        &tok,
        "POST",
        "/users",
        Some(json!({"username": unique("x"), "password": "xxxxxxxx1", "role": "admin"})),
    )
    .await;
    assert_eq!(s, 403);
    let (s, _) = call(
        &c,
        &tok,
        "POST",
        "/acl",
        Some(
            json!({"path": "/", "subject_type": "user", "subject": name, "role": "Administrator"}),
        ),
    )
    .await;
    assert_eq!(s, 403);
    let (s, _) = call(
        &c,
        &tok,
        "POST",
        "/users/admin/password",
        Some(json!({"new_password": "pwned-admin-1"})),
    )
    .await;
    assert_eq!(s, 403);
    let (s, _) = call(
        &c,
        &tok,
        "POST",
        "/users/admin/api-keys",
        Some(json!({"name": "steal"})),
    )
    .await;
    assert_eq!(s, 403);
    let (s, _) = call(
        &c,
        &tok,
        "POST",
        "/users/admin/toggle",
        Some(json!({"enabled": false})),
    )
    .await;
    assert_eq!(s, 403);
    let (s, _) = call(&c, &tok, "DELETE", "/users/admin", None).await;
    assert_eq!(s, 403);
    let (s, _) = call(
        &c,
        &tok,
        "PUT",
        "/roles/Escalate",
        Some(json!({"privileges": ["Sys.Modify"]})),
    )
    .await;
    assert_eq!(s, 403);
    // ... nor slip anyone into a group that holds grants they can't make
    let group = unique("admins");
    let (s, _) = call(&c, &adm, "POST", "/groups", Some(json!({"name": group}))).await;
    assert_eq!(s, 201);
    let (s, _) = call(&c, &adm, "POST", "/acl", Some(json!({"path": "/", "subject_type": "group", "subject": group, "role": "Administrator"}))).await;
    assert_eq!(s, 201);
    let (s, _) = call(
        &c,
        &tok,
        "PUT",
        &format!("/users/{victim}/groups"),
        Some(json!({"groups": [group]})),
    )
    .await;
    assert_eq!(s, 403, "useradmin adds a user to an admin group");
    let (s, _) = call(
        &c,
        &tok,
        "PUT",
        &format!("/groups/{group}"),
        Some(json!({"members": [name]})),
    )
    .await;
    assert_eq!(s, 403, "useradmin adds themself to an admin group");
    let (s, _) = call(
        &c,
        &tok,
        "POST",
        "/users",
        Some(json!({"username": unique("y"), "password": "yyyyyyyy1", "groups": [group]})),
    )
    .await;
    assert_eq!(s, 403, "useradmin creates a user inside an admin group");
    let _ = call(&c, &adm, "DELETE", &format!("/groups/{group}"), None).await;
    // admin itself can't remove the last administrator
    let (s, _) = call(&c, &adm, "DELETE", "/users/admin", None).await;
    assert!(
        matches!(s.as_u16(), 422),
        "deleting yourself / last admin: {s}"
    );
    let _ = call(&c, &adm, "DELETE", &format!("/users/{victim}"), None).await;
    let _ = call(&c, &adm, "DELETE", &format!("/users/{name}"), None).await;
}

#[tokio::test]
async fn api_keys_act_with_their_owners_rights() {
    let c = client();
    let adm = admin(&c).await;
    let name = unique("keyowner");
    make_user(&c, &adm, &name, "keyowner-pass", Some(("/vms", "VMUser"))).await;
    let tok = login(&c, &name, "keyowner-pass").await;
    let (s, v) = call(
        &c,
        &tok,
        "POST",
        &format!("/users/{name}/api-keys"),
        Some(json!({"name": "ci"})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let key = v["key"].as_str().unwrap().to_string();
    let get = |p: &str| c.get(api(p)).header("X-API-Key", key.clone()).send();
    assert_eq!(get("/vms").await.unwrap().status(), 200);
    assert_eq!(get("/users").await.unwrap().status(), 403);
    assert_eq!(get("/storage/pools").await.unwrap().status(), 403);
    let _ = call(&c, &adm, "DELETE", &format!("/users/{name}"), None).await;
    assert_eq!(
        get("/vms").await.unwrap().status(),
        401,
        "key of a deleted user"
    );
}

#[tokio::test]
async fn groups_noaccess_and_collection_filtering() {
    let c = client();
    let adm = admin(&c).await;
    let group = unique("ops");
    let name = unique("grouped");
    let id = make_user(&c, &adm, &name, "grouped-pass-1", None).await;
    let (s, v) = call(
        &c,
        &adm,
        "POST",
        "/groups",
        Some(json!({"name": group, "members": [name]})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let (s, _) = call(&c, &adm, "POST", "/acl", Some(json!({"path": "/containers", "subject_type": "group", "subject": group, "role": "VMUser"}))).await;
    assert_eq!(s, 201);
    let tok = login(&c, &name, "grouped-pass-1").await;
    assert_eq!(
        call(&c, &tok, "GET", "/containers", None).await.0,
        200,
        "group grant applies"
    );
    assert_ne!(
        call(&c, &tok, "GET", "/containers/abc", None).await.0,
        403,
        "group grant reaches /containers/<id>"
    );
    // NoAccess for the user on one container beats the group's grant
    let (s, _) = call(&c, &adm, "POST", "/acl", Some(json!({"path": "/containers/secret", "subject_type": "user", "subject": name, "role": "NoAccess"}))).await;
    assert_eq!(s, 201);
    assert_eq!(
        call(&c, &tok, "GET", "/containers/secret", None).await.0,
        403
    );
    // a grant on a single VM: the list shows only that VM
    let (s, _) = call(&c, &adm, "POST", "/acl", Some(json!({"path": "/vms/e2e-only-this", "subject_type": "user", "subject": name, "role": "VMUser"}))).await;
    assert_eq!(s, 201);
    let (s, vms) = call(&c, &tok, "GET", "/vms", None).await;
    assert_eq!(s, 200, "partial grant may list: {vms}");
    assert!(
        vms.as_array()
            .unwrap()
            .iter()
            .all(|v| v["id"] == "e2e-only-this"),
        "{vms}"
    );
    let (_, all) = call(&c, &adm, "GET", "/vms", None).await;
    let _ = all;
    let _ = call(&c, &adm, "DELETE", &format!("/groups/{group}"), None).await;
    assert_eq!(
        call(&c, &tok, "GET", "/containers", None).await.0,
        403,
        "group deleted -> grant gone"
    );
    let _ = call(&c, &adm, "DELETE", &format!("/users/{id}"), None).await;
}

#[tokio::test]
async fn https_redirect_and_certificate_hot_swap() {
    let c = client();
    // plain HTTP on the HTTPS port is redirected, health still answers
    let http = base().replacen("https://", "http://", 1);
    let r = c.get(format!("{http}/vms?x=1")).send().await.unwrap();
    assert_eq!(r.status(), 308);
    assert!(
        r.headers()["location"]
            .to_str()
            .unwrap()
            .starts_with("https://"),
        "{:?}",
        r.headers()
    );
    assert!(r.headers()["location"]
        .to_str()
        .unwrap()
        .ends_with("/vms?x=1"));
    assert_eq!(
        c.get(format!("{http}/api/health"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );

    // the local CA is downloadable without logging in
    let r = c.get(api("/tls/ca.pem")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let ca = r.text().await.unwrap();
    assert!(ca.contains("BEGIN CERTIFICATE"));

    let adm = admin(&c).await;
    let (s, before) = call(&c, &adm, "GET", "/tls/status", None).await;
    assert_eq!(s, 200);
    let (s, issued) = call(
        &c,
        &adm,
        "POST",
        "/tls/certificate/generate",
        Some(json!({"san": ["e2e.horcrux.test"]})),
    )
    .await;
    assert_eq!(s, 200, "{issued}");
    assert_ne!(
        issued["fingerprint_sha256"],
        before["certificate"]["fingerprint_sha256"]
    );
    assert!(issued["san"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n == "e2e.horcrux.test"));

    // the listener serves the new certificate without a restart, and it
    // verifies against the local CA
    let mut roots = vec![];
    for c in reqwest::Certificate::from_pem_bundle(ca.as_bytes()).unwrap() {
        roots.push(c);
    }
    let mut b = Client::builder()
        .tls_info(true)
        .timeout(Duration::from_secs(20));
    for r in roots {
        b = b.add_root_certificate(r);
    }
    let strict = b.build().unwrap();
    let r = strict
        .get(api("/health"))
        .send()
        .await
        .expect("TLS verifies against the local CA");
    let info = r.extensions().get::<reqwest::tls::TlsInfo>().unwrap();
    use sha2::Digest;
    let fp = sha2::Sha256::digest(info.peer_certificate().unwrap())
        .iter()
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<_>>()
        .join(":");
    assert_eq!(fp, issued["fingerprint_sha256"].as_str().unwrap());

    // a key that doesn't match the certificate is refused
    let (cert, _) =
        horcrux_api::tls::generate_self_signed("a", "o", &["a.example".into()], 30).unwrap();
    let (_, key2) =
        horcrux_api::tls::generate_self_signed("b", "o", &["b.example".into()], 30).unwrap();
    let (s, v) = call(
        &c,
        &adm,
        "POST",
        "/tls/certificate/upload",
        Some(json!({"cert_pem": cert, "key_pem": key2})),
    )
    .await;
    assert_eq!(s, 422, "{v}");
}
