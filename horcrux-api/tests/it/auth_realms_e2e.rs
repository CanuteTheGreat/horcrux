//! Authentication realms and ACME against real servers: OpenLDAP (slapd),
//! Dex (OpenID Connect), Linux-PAM and Pebble (ACME). The servers run in
//! containers started by `tests/auth_realms_e2e.sh`, which also runs this
//! binary next to a live horcrux-api; under a plain `cargo test` these are
//! ignored.

use reqwest::{Client, StatusCode};
use serde_json::{json, Value};
use std::time::Duration;

fn base() -> String {
    std::env::var("HORCRUX_TEST_API").unwrap_or_else(|_| "https://localhost:8006".into())
}
fn api(p: &str) -> String {
    format!("{}/api{}", base(), p)
}
fn env(k: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| panic!("{k} must be set (run tests/auth_realms_e2e.sh)"))
}

fn client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(60))
        .danger_accept_invalid_certs(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

async fn login_raw(c: &Client, body: Value) -> (StatusCode, Value) {
    let r = c.post(api("/auth/login")).json(&body).send().await.unwrap();
    let s = r.status();
    (s, r.json().await.unwrap_or(Value::Null))
}

async fn admin(c: &Client) -> String {
    let (s, v) = login_raw(c, json!({"username": "admin", "password": "admin"})).await;
    assert_eq!(s, 200, "{v}");
    v["ticket"].as_str().unwrap().to_string()
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
    let t = r.text().await.unwrap_or_default();
    (s, serde_json::from_str(&t).unwrap_or(Value::String(t)))
}

/// Group whose members may use VMs; realm users land in it on first login.
async fn realm_group(c: &Client, adm: &str, name: &str) {
    let _ = call(c, adm, "DELETE", &format!("/groups/{name}"), None).await;
    let (s, v) = call(c, adm, "POST", "/groups", Some(json!({"name": name}))).await;
    assert_eq!(s, 201, "{v}");
    let (s, v) = call(
        c,
        adm,
        "POST",
        "/acl",
        Some(json!({"path": "/vms", "subject_type": "group", "subject": name, "role": "VMUser"})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
}

#[tokio::test]
#[ignore = "needs OpenLDAP: run tests/auth_realms_e2e.sh"]
async fn ldap_realm_search_then_bind_login() {
    let c = client();
    let adm = admin(&c).await;
    realm_group(&c, &adm, "ldap-users").await;
    let _ = call(&c, &adm, "DELETE", "/access/realms/corp", None).await;
    let realm = json!({
        "name": "corp", "realm_type": "ldap", "enabled": true, "comment": "Corporate LDAP",
        "default_group": "ldap-users",
        "config": {
            "server": env("LDAP_HOST"), "port": 389, "mode": "ldap",
            "base_dn": "ou=people,dc=horcrux,dc=test", "user_attr": "uid",
            "bind_dn": "cn=admin,dc=horcrux,dc=test", "bind_password": env("LDAP_ADMIN_PASSWORD")
        }
    });
    let (s, v) = call(&c, &adm, "POST", "/access/realms", Some(realm)).await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["config"]["bind_password"], "********");
    let (s, v) = call(
        &c,
        &adm,
        "POST",
        "/access/realms/corp/test",
        Some(json!({})),
    )
    .await;
    assert_eq!((s, v["ok"].clone()), (StatusCode::OK, json!(true)), "{v}");
    let (_, v) = call(
        &c,
        &adm,
        "POST",
        "/access/realms/corp/test",
        Some(json!({"username": "alice", "password": "alice-ldap-pass"})),
    )
    .await;
    assert_eq!(v["ok"], true, "{v}");

    // the login page lists the realm
    let r: Value = c
        .get(api("/auth/realms"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(r["realms"]
        .as_array()
        .unwrap()
        .iter()
        .any(|x| x["name"] == "corp" && x["type"] == "ldap"));

    for bad in [
        json!({"username": "alice@corp", "password": "wrong"}),
        json!({"username": "alice@corp", "password": ""}),
        json!({"username": "*", "password": "x", "realm": "corp"}),
        json!({"username": "nobody@corp", "password": "alice-ldap-pass"}),
    ] {
        let (s, _) = login_raw(&c, bad.clone()).await;
        assert!(matches!(s.as_u16(), 401 | 422), "{bad} -> {s}");
    }
    let (s, v) = login_raw(
        &c,
        json!({"username": "alice@corp", "password": "alice-ldap-pass"}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let tok = v["ticket"].as_str().unwrap().to_string();
    let (_, me) = call(&c, &tok, "GET", "/auth/me", None).await;
    assert_eq!(me["user"]["username"], "alice@corp");
    assert_eq!(me["user"]["realm"], "corp");
    assert_eq!(
        me["user"]["email"], "alice@horcrux.test",
        "mail attribute copied"
    );
    assert_eq!(me["user"]["groups"], json!(["ldap-users"]));
    assert_eq!(
        call(&c, &tok, "GET", "/vms", None).await.0,
        200,
        "default group's grant applies"
    );
    assert_eq!(call(&c, &tok, "GET", "/users", None).await.0, 403);
    // the password lives in LDAP, not here
    let (s, _) = call(
        &c,
        &tok,
        "POST",
        "/auth/password",
        Some(json!({"old_password": "alice-ldap-pass", "new_password": "something-else"})),
    )
    .await;
    assert_eq!(s, 422);
    // same with an explicit realm field
    let (s, _) = login_raw(
        &c,
        json!({"username": "alice", "password": "alice-ldap-pass", "realm": "corp"}),
    )
    .await;
    assert_eq!(s, 200);
    // a disabled realm no longer authenticates
    let (s, _) = call(&c, &adm, "PUT", "/access/realms/corp", Some(json!({"name": "corp", "realm_type": "ldap", "enabled": false,
        "config": {"server": env("LDAP_HOST"), "base_dn": "ou=people,dc=horcrux,dc=test", "bind_dn": "cn=admin,dc=horcrux,dc=test", "bind_password": "********"}}))).await;
    assert_eq!(s, 200);
    let (s, _) = login_raw(
        &c,
        json!({"username": "alice@corp", "password": "alice-ldap-pass"}),
    )
    .await;
    assert_eq!(s, 401);
}

/// Follow redirects, but stop at Horcrux's /login (the token is in the
/// fragment of that redirect).
fn browser() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(60))
        .danger_accept_invalid_certs(true)
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::custom(|a| {
            if a.url().path() == "/login" || a.previous().len() > 15 {
                a.stop()
            } else {
                a.follow()
            }
        }))
        .build()
        .unwrap()
}

#[tokio::test]
#[ignore = "needs Dex: run tests/auth_realms_e2e.sh"]
async fn openid_connect_login_through_dex() {
    let c = client();
    let adm = admin(&c).await;
    realm_group(&c, &adm, "sso-users").await;
    let _ = call(&c, &adm, "DELETE", "/access/realms/dex", None).await;
    let (s, v) = call(&c, &adm, "POST", "/access/realms", Some(json!({
        "name": "dex", "realm_type": "openid", "enabled": true, "default_group": "sso-users",
        "config": {"issuer_url": env("DEX_ISSUER"), "client_id": "horcrux", "client_secret": "horcrux-ci-secret",
                   "username_claim": "email"}
    }))).await;
    assert_eq!(s, 201, "{v}");
    let (_, v) = call(&c, &adm, "POST", "/access/realms/dex/test", Some(json!({}))).await;
    assert_eq!(v["ok"], true, "{v}");
    // a password login can't be used for an OpenID realm
    let (s, _) = login_raw(
        &c,
        json!({"username": "carol", "password": "password", "realm": "dex"}),
    )
    .await;
    assert_eq!(s, 400);

    let b = browser();
    // 1. Horcrux -> Dex (authorization request with state, nonce, PKCE)
    let r = b
        .get(api("/auth/oidc/dex/login?redirect=/vms"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "Dex login page: {}", r.url());
    let page_url = r.url().clone();
    let html = r.text().await.unwrap();
    let action = regex::Regex::new(r#"<form[^>]*action="([^"]+)""#)
        .unwrap()
        .captures(&html)
        .map(|c| c[1].replace("&amp;", "&"))
        .unwrap_or_else(|| panic!("no login form in Dex page: {html}"));
    let action = page_url.join(&action).unwrap();
    // 2. log in at Dex -> code -> Horcrux callback -> /login#ticket=...
    let r = b
        .post(action)
        .form(&[("login", "carol@horcrux.test"), ("password", "password")])
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        303,
        "callback should redirect to /login, got {} at {}",
        r.status(),
        r.url()
    );
    let loc = r.headers()["location"].to_str().unwrap().to_string();
    assert!(loc.starts_with("/login#ticket="), "{loc}");
    let frag: std::collections::HashMap<String, String> = loc
        .split_once('#')
        .unwrap()
        .1
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), urlencoding::decode(v).unwrap().into_owned()))
        .collect();
    assert_eq!(frag["redirect"], "/vms");
    let tok = &frag["ticket"];
    let (s, me) = call(&c, tok, "GET", "/auth/me", None).await;
    assert_eq!(s, 200, "{me}");
    assert_eq!(me["user"]["username"], "carol@horcrux.test@dex");
    assert_eq!(me["user"]["realm"], "dex");
    assert_eq!(call(&c, tok, "GET", "/vms", None).await.0, 200);
    assert_eq!(call(&c, tok, "GET", "/users", None).await.0, 403);

    // state is single use: replaying the callback fails cleanly
    let replay = b
        .get(api("/auth/oidc/callback?code=x&state=not-a-real-state"))
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 303);
    assert!(replay.headers()["location"]
        .to_str()
        .unwrap()
        .starts_with("/login#error="));
}

#[tokio::test]
#[ignore = "needs PAM user: run tests/auth_realms_e2e.sh"]
async fn pam_realm_login_with_a_host_account() {
    let c = client();
    let adm = admin(&c).await;
    let _ = call(&c, &adm, "DELETE", "/access/realms/host", None).await;
    let (s, v) = call(
        &c,
        &adm,
        "POST",
        "/access/realms",
        Some(json!({"name": "host", "realm_type": "pam", "config": {}})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let user = env("PAM_USER");
    let (s, _) = login_raw(
        &c,
        json!({"username": format!("{user}@host"), "password": "wrong-password"}),
    )
    .await;
    assert_eq!(s, 401);
    let (s, v) = login_raw(
        &c,
        json!({"username": format!("{user}@host"), "password": env("PAM_PASSWORD")}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let (_, me) = call(&c, v["ticket"].as_str().unwrap(), "GET", "/auth/me", None).await;
    assert_eq!(me["user"]["username"], format!("{user}@host"));
    // no grants yet: logged in but can't do anything
    assert_eq!(
        call(&c, v["ticket"].as_str().unwrap(), "GET", "/vms", None)
            .await
            .0,
        403
    );
}

#[tokio::test]
#[ignore = "needs Pebble: run tests/auth_realms_e2e.sh"]
async fn acme_certificate_from_pebble_is_served() {
    let c = client();
    let adm = admin(&c).await;
    let domain = env("ACME_DOMAIN");
    let (s, v) = call(
        &c,
        &adm,
        "PUT",
        "/tls/acme",
        Some(json!({
            "directory_url": env("ACME_DIR"), "domains": [domain], "ca_cert": env("ACME_CA"),
            "contact_email": "hostmaster@horcrux.test", "accept_tos": true
        })),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let (s, info) = call(&c, &adm, "POST", "/tls/acme/order", Some(json!({}))).await;
    assert_eq!(s, 200, "ACME order: {info}");
    assert_eq!(info["source"], "acme");
    assert!(
        info["san"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n == domain.as_str()),
        "{info}"
    );
    assert!(
        info["issuer"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("pebble"),
        "{info}"
    );
    // the listener now presents it
    let probe = Client::builder()
        .danger_accept_invalid_certs(true)
        .tls_info(true)
        .build()
        .unwrap();
    let r = probe.get(api("/health")).send().await.unwrap();
    let der = r
        .extensions()
        .get::<reqwest::tls::TlsInfo>()
        .unwrap()
        .peer_certificate()
        .unwrap()
        .to_vec();
    use sha2::Digest;
    let fp = sha2::Sha256::digest(&der)
        .iter()
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<_>>()
        .join(":");
    assert_eq!(fp, info["fingerprint_sha256"].as_str().unwrap());
    let (_, st) = call(&c, &adm, "GET", "/tls/status", None).await;
    assert_eq!(st["acme_last_result"]["ok"], true, "{st}");
}
