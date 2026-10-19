//! Authorization matrix: every built-in role x every registered route.
//!
//! The every-route test in auth_e2e_tests.rs proves an account with no
//! grants is refused everywhere. This suite proves the other half - each
//! role gets exactly what its privilege list says, no more and no less:
//!
//! - for every built-in role granted on `/`, every route whose required
//!   privilege is NOT in the role answers 403 (all methods; refused before
//!   any handler runs), and every GET route whose privilege IS in the role
//!   is not refused (reads only - allowed writes would really execute);
//! - an object-scoped grant (VMAdmin on one VM) reaches that VM's routes
//!   and nothing of another VM;
//! - GET /api/roles shows the privileges that are actually enforced.
//!
//! Runs against the live server the CI Test job starts (like
//! auth_e2e_tests.rs): HORCRUX_TEST_API (default https://localhost:8006),
//! HORCRUX_TEST_CA (PEM to trust; without it the self-signed node
//! certificate is accepted), ADMIN_PASSWORD (default admin).

use horcrux_api::auth::privileges::{builtin_roles, requirement, Requirement};
use horcrux_common::auth::Privilege;
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::time::Duration;

fn base() -> String {
    std::env::var("HORCRUX_TEST_API").unwrap_or_else(|_| "https://localhost:8006".into())
}

fn api(path: &str) -> String {
    format!("{}/api{}", base(), path)
}

/// URL of a full server path (`/api/...`, or a root route like `/metrics`).
fn url(full_path: &str) -> String {
    format!("{}{}", base(), full_path)
}

fn client() -> Client {
    let mut b = Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none());
    b = match std::env::var("HORCRUX_TEST_CA") {
        Ok(ca) => b.add_root_certificate(
            reqwest::Certificate::from_pem(&std::fs::read(&ca).expect("HORCRUX_TEST_CA"))
                .expect("CA PEM"),
        ),
        Err(_) => b.danger_accept_invalid_certs(true),
    };
    b.build().unwrap()
}

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            % 100_000_000_000
    )
}

async fn login(c: &Client, username: &str, password: &str) -> String {
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
        let v: Value = r.json().await.unwrap_or(Value::Null);
        assert_eq!(s, 200, "login {username}: {v}");
        return v["ticket"].as_str().expect("ticket").to_string();
    }
    panic!("login kept being rate limited");
}

async fn admin(c: &Client) -> String {
    let pw = std::env::var("ADMIN_PASSWORD").unwrap_or_else(|_| "admin".into());
    login(c, "admin", &pw).await
}

/// Status of one request to a full server path; a timeout or connection
/// reset counts as "not refused by RBAC" (it got past the access check).
async fn status(c: &Client, tok: &str, method: &str, full_path: &str) -> Option<StatusCode> {
    let m = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
    let mut rb = c.request(m, url(full_path)).bearer_auth(tok);
    if method != "GET" && method != "HEAD" {
        rb = rb.json(&json!({}));
    }
    rb.send().await.ok().map(|r| r.status())
}

async fn call(c: &Client, tok: &str, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let m = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
    let r = c
        .request(m, api(path))
        .bearer_auth(tok)
        .json(&body)
        .send()
        .await
        .unwrap();
    let s = r.status();
    let t = r.text().await.unwrap_or_default();
    (s, serde_json::from_str(&t).unwrap_or(Value::String(t)))
}

/// Admin creates a user and grants `role` on `path`.
async fn user_with(c: &Client, adm: &str, role: &str, path: &str) -> (String, String) {
    let name = unique(&format!("mx-{}", role.to_lowercase()));
    let pw = "matrix-pass-1";
    let (s, v) = call(
        c,
        adm,
        "POST",
        "/users",
        json!({"username": name, "password": pw, "email": format!("{name}@errant.solutions")}),
    )
    .await;
    assert_eq!(s, 201, "create {name}: {v}");
    let id = v["id"].as_str().unwrap().to_string();
    let (s, v) = call(
        c,
        adm,
        "POST",
        "/acl",
        json!({"path": path, "subject_type": "user", "subject": name, "role": role}),
    )
    .await;
    assert_eq!(s, 201, "grant {role} on {path}: {v}");
    (id, login(c, &name, pw).await)
}

/// Every `.route(...)` the server registers, with its methods (same source
/// scan as auth_e2e_tests.rs).
fn registered_routes() -> Vec<(String, String)> {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    // The router and handlers: src/app/ (formerly main.rs) and src/api/.
    let mut files = vec![];
    for dir in ["app", "api"] {
        for e in std::fs::read_dir(src.join(dir)).unwrap().flatten() {
            files.push(e.path());
        }
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

/// `/api/vms/:id/x` -> `/api/vms/<id>/x` (the path the middleware sees).
fn concrete(pattern: &str, id: &str) -> String {
    pattern
        .split('/')
        .map(|s| if s.starts_with(':') { id } else { s })
        .collect::<Vec<_>>()
        .join("/")
}

/// The privilege a request needs, for a grant on `/`.
fn needed(method: &str, full_path: &str) -> Option<Privilege> {
    match requirement(method, full_path) {
        Requirement::Privilege { privilege, .. } | Requirement::AnyBelow { privilege, .. } => {
            Some(privilege)
        }
        _ => None,
    }
}

/// WebSocket upgrade endpoints answer a plain GET with an upgrade error;
/// streaming endpoints may never finish - both are fine for "not 403".
fn read_probe_ok(s: Option<StatusCode>) -> bool {
    s != Some(StatusCode::FORBIDDEN) && s != Some(StatusCode::UNAUTHORIZED)
}

#[tokio::test]
async fn every_builtin_role_gets_exactly_its_privileges_on_every_route() {
    let c = client();
    let adm = admin(&c).await;
    let routes = registered_routes();
    assert!(routes.len() > 300, "only {} routes parsed", routes.len());

    // GET /roles shows what is enforced.
    let (s, listed) = call(&c, &adm, "GET", "/roles", Value::Null).await;
    assert_eq!(s, 200, "{listed}");
    let listed = listed.as_array().cloned().unwrap_or_default();

    let mut failures = vec![];
    let mut denied_checked = 0;
    let mut allowed_checked = 0;
    for (role, _, privs) in builtin_roles() {
        let enforced: BTreeSet<String> = privs
            .iter()
            .map(|p| {
                serde_json::to_value(p)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        let shown = listed
            .iter()
            .find(|r| r["name"] == role || r["id"] == role)
            .unwrap_or_else(|| panic!("built-in role {role} missing from GET /roles"));
        let shown: BTreeSet<String> = shown["privileges"]
            .as_array()
            .unwrap_or_else(|| panic!("{role}: no privileges in {shown}"))
            .iter()
            .filter_map(|p| p.as_str().map(str::to_string))
            .collect();
        assert_eq!(
            shown, enforced,
            "{role}: GET /roles differs from what is enforced"
        );

        let (uid, tok) = user_with(&c, &adm, role, "/").await;
        for (method, pattern) in &routes {
            let path = concrete(pattern, "rbac-probe");
            let Some(p) = needed(method, &path) else {
                continue;
            };
            let has = privs.contains(&p);
            if !has {
                let s = status(&c, &tok, method, &path).await;
                denied_checked += 1;
                if s != Some(StatusCode::FORBIDDEN) {
                    failures.push(format!(
                        "{role} lacks {p:?} but {method} {pattern} -> {s:?}"
                    ));
                }
            } else if method == "GET" {
                let s = status(&c, &tok, method, &path).await;
                allowed_checked += 1;
                if !read_probe_ok(s) {
                    failures.push(format!("{role} has {p:?} but GET {pattern} -> {s:?}"));
                }
            }
        }
        let _ = call(&c, &adm, "DELETE", &format!("/users/{uid}"), Value::Null).await;
    }
    assert!(
        denied_checked > 500,
        "only {denied_checked} denials checked"
    );
    assert!(
        allowed_checked > 200,
        "only {allowed_checked} allowed reads checked"
    );
    assert!(
        failures.is_empty(),
        "RBAC matrix mismatches:\n{}",
        failures.join("\n")
    );
}

#[tokio::test]
async fn a_grant_on_one_vm_reaches_that_vm_and_nothing_else() {
    let c = client();
    let adm = admin(&c).await;
    let own = "rbac-own-vm";
    let other = "rbac-other-vm";
    let (uid, tok) = user_with(&c, &adm, "VMAdmin", &format!("/vms/{own}")).await;
    let vmadmin = builtin_roles()
        .into_iter()
        .find(|(n, _, _)| *n == "VMAdmin")
        .expect("VMAdmin role")
        .2;

    let mut failures = vec![];
    let mut checked = 0;
    for (method, pattern) in registered_routes() {
        if !pattern.starts_with("/api/vms/:") {
            continue;
        }
        // Another VM: refused for every method (safe - refused before the handler).
        let theirs = concrete(&pattern, other);
        if needed(&method, &theirs).is_some() {
            let s = status(&c, &tok, &method, &theirs).await;
            checked += 1;
            if s != Some(StatusCode::FORBIDDEN) {
                failures.push(format!("other VM: {method} {pattern} -> {s:?}"));
            }
        }
        // Own VM: reads covered by the role are not refused.
        let mine = concrete(&pattern, own);
        if method == "GET" {
            if let Some(p) = needed(&method, &mine) {
                if vmadmin.contains(&p) {
                    let s = status(&c, &tok, &method, &mine).await;
                    checked += 1;
                    if !read_probe_ok(s) {
                        failures.push(format!("own VM: GET {pattern} -> {s:?}"));
                    }
                }
            }
        }
    }
    // Nothing outside the VM: a sample of other areas.
    for path in [
        "/containers",
        "/storage/pools",
        "/backups",
        "/users",
        "/cluster/nodes",
        "/firewall/rules",
    ] {
        let s = status(&c, &tok, "GET", &format!("/api{path}")).await;
        checked += 1;
        if s == Some(StatusCode::OK) {
            // Collections may answer 200 with only permitted items: must be empty.
            let (_, v) = call(&c, &tok, "GET", path, Value::Null).await;
            if v.as_array().map(|a| !a.is_empty()).unwrap_or(true) {
                failures.push(format!("GET {path} with a single-VM grant -> 200 {v}"));
            }
        }
    }
    let _ = call(&c, &adm, "DELETE", &format!("/users/{uid}"), Value::Null).await;
    assert!(checked > 20, "only {checked} checks");
    assert!(
        failures.is_empty(),
        "object-scoped grant leaks:\n{}",
        failures.join("\n")
    );
}

/// PUT /api/nas/s3/config is a /nas Sys.Modify route, but the API runs as
/// root: a NAS-scoped administrator must not choose what root executes or
/// point the gateway's data directory anywhere on the host.
#[tokio::test]
async fn a_nas_scoped_admin_cannot_choose_the_s3_gateway_programs_or_host_data_dirs() {
    let c = client();
    let adm = admin(&c).await;
    let (uid, tok) = user_with(&c, &adm, "SysAdmin", "/nas").await;
    let (s, orig) = call(&c, &tok, "GET", "/nas/s3/config", Value::Null).await;
    assert_eq!(s, 200, "NAS admin reads the gateway config: {orig}");
    let dirs: Vec<String> = serde_json::from_value(orig["data_dirs"].clone()).unwrap();
    let base = dirs.first().expect("a data dir").clone();
    // <node data dir>/nas/s3 by default
    let node_dir = std::path::Path::new(&base)
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/var/lib/horcrux".into());

    let mut failures: Vec<String> = vec![];
    fn expect(failures: &mut Vec<String>, what: &str, got: StatusCode, want: u16, body: &Value) {
        if got.as_u16() != want {
            failures.push(format!("{what}: {got} (want {want}) {body}"));
        }
    }
    // any program outside the allow-list -> 422, for every caller
    for (field, bin) in [
        ("server_bin", "/usr/bin/id"),
        ("server_bin", "id"),
        ("server_bin", "/tmp/silo"),
        ("server_bin", "/usr/bin/../../tmp/silo"),
        ("client_bin", "/bin/sh"),
        ("client_bin", "mcli; id"),
    ] {
        let (s, v) = call(&c, &tok, "PUT", "/nas/s3/config", json!({ field: bin })).await;
        expect(
            &mut failures,
            &format!("NAS admin sets {field}={bin}"),
            s,
            422,
            &v,
        );
    }
    let (s, v) = call(
        &c,
        &adm,
        "PUT",
        "/nas/s3/config",
        json!({"server_bin": "/usr/bin/id"}),
    )
    .await;
    expect(
        &mut failures,
        "root admin sets server_bin=/usr/bin/id",
        s,
        422,
        &v,
    );
    // system dirs and the node data dir -> 422 for everyone
    for d in [
        "/etc/hx-s3",
        "/usr/hx-s3",
        "/proc/hx-s3",
        "/srv/../etc/hx",
        node_dir.as_str(),
    ] {
        let (s, v) = call(&c, &tok, "PUT", "/nas/s3/config", json!({"data_dirs": [d]})).await;
        expect(
            &mut failures,
            &format!("NAS admin sets data_dirs=[{d}]"),
            s,
            422,
            &v,
        );
        let (s, v) = call(&c, &adm, "PUT", "/nas/s3/config", json!({"data_dirs": [d]})).await;
        expect(
            &mut failures,
            &format!("root admin sets data_dirs=[{d}]"),
            s,
            422,
            &v,
        );
    }
    // somewhere else on the host -> 403 for a NAS-scoped admin
    let (s, v) = call(
        &c,
        &tok,
        "PUT",
        "/nas/s3/config",
        json!({"data_dirs": ["/srv/hx-s3-rbac"]}),
    )
    .await;
    expect(
        &mut failures,
        "NAS admin sets data_dirs=[/srv/hx-s3-rbac]",
        s,
        403,
        &v,
    );
    let (s, after) = call(&c, &adm, "GET", "/nas/s3/config", Value::Null).await;
    assert_eq!(s, 200);
    if after["server_bin"] != orig["server_bin"]
        || after["client_bin"] != orig["client_bin"]
        || after["data_dirs"] != orig["data_dirs"]
    {
        failures.push(format!(
            "refused PUTs changed the config: {orig} -> {after}"
        ));
    }
    // allowed: packaged programs by name, and a dir below the NAS roots
    let below = format!("{base}-rbac");
    let (s, v) = call(
        &c,
        &tok,
        "PUT",
        "/nas/s3/config",
        json!({"server_bin": "silo", "client_bin": "/usr/local/bin/mcli", "data_dirs": [below]}),
    )
    .await;
    expect(
        &mut failures,
        "NAS admin sets silo + /usr/local/bin/mcli + a NAS data dir",
        s,
        200,
        &v,
    );
    // the unchanged (already accepted) settings stay savable
    let (s, v) = call(
        &c,
        &tok,
        "PUT",
        "/nas/s3/config",
        json!({"data_dirs": [below]}),
    )
    .await;
    expect(
        &mut failures,
        "NAS admin re-saves the same data dir",
        s,
        200,
        &v,
    );

    // restore
    let (s, v) = call(
        &c,
        &adm,
        "PUT",
        "/nas/s3/config",
        json!({"server_bin": orig["server_bin"], "client_bin": orig["client_bin"], "data_dirs": orig["data_dirs"]}),
    )
    .await;
    assert_eq!(s, 200, "restore the gateway config: {v}");
    let _ = call(&c, &adm, "DELETE", &format!("/users/{uid}"), Value::Null).await;
    assert!(
        failures.is_empty(),
        "gateway settings policy:\n{}",
        failures.join("\n")
    );
}
