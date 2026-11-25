//! API access log end to end against a freshly spawned horcrux-api: requests
//! made with a password session and with an API token are queried back with
//! every filter and the statistics; privileges, trusted proxies, settings
//! and persistence across a restart.
use crate::support::{scratch, Server};
use reqwest::Client;
use serde_json::{json, Value};

async fn q(srv: &Server, tok: &str, path: &str) -> (u16, Value) {
    let r = srv
        .client()
        .get(format!("{}{}", srv.base(), path))
        .bearer_auth(tok)
        .send()
        .await
        .unwrap();
    let s = r.status().as_u16();
    let t = r.text().await.unwrap_or_default();
    (s, serde_json::from_str(&t).unwrap_or(Value::String(t)))
}

async fn hit(
    c: &Client,
    srv: &Server,
    path: &str,
    hdrs: &[(&str, &str)],
    bearer: Option<&str>,
) -> u16 {
    let mut r = c.get(format!("{}{}", srv.base(), path));
    for (k, v) in hdrs {
        r = r.header(*k, *v);
    }
    if let Some(t) = bearer {
        r = r.bearer_auth(t);
    }
    r.send().await.unwrap().status().as_u16()
}

fn entries(v: &Value) -> Vec<Value> {
    v["entries"].as_array().cloned().unwrap_or_default()
}

fn total(v: &Value) -> i64 {
    v["total"].as_i64().unwrap()
}

#[tokio::test]
async fn records_session_and_token_requests_and_answers_every_filter() {
    let dir = scratch("access-log");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;
    let c = srv.client();

    // an API token for admin
    let (s, v) = api
        .post("/users/admin/api-keys", json!({"name": "audit-ci"}))
        .await;
    assert_eq!(s, 200, "{v}");
    let key = v["key"].as_str().unwrap().to_string();
    let key_id = v["id"].as_str().map(str::to_string);

    // traffic: session, token, failures
    assert_eq!(hit(&c, &srv, "/vms", &[], Some(&api.token)).await, 200);
    assert_eq!(
        hit(&c, &srv, "/vms", &[("X-API-Key", &key)], None).await,
        200
    );
    assert_eq!(
        hit(&c, &srv, "/vms", &[("X-API-Key", &key)], None).await,
        200
    );
    assert_eq!(
        hit(&c, &srv, "/vms/no-such-vm", &[], Some(&api.token)).await,
        404
    );
    assert_eq!(hit(&c, &srv, "/users", &[], Some("not-a-jwt")).await, 401);
    assert_eq!(hit(&c, &srv, "/users", &[], None).await, 401);
    assert_eq!(hit(&c, &srv, "/health", &[], None).await, 200);
    let r = c
        .post(format!("{}/auth/login", srv.base()))
        .json(&json!({"username": "admin", "password": "wrong-password-xyz"}))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_client_error());
    // a path nobody serves is recorded too, under one group
    let _ = hit(&c, &srv, "/api/definitely/not/a/route/1234", &[], None).await;

    // everything, newest first
    let (s, all) = q(&srv, &api.token, "/access-log?limit=1000").await;
    assert_eq!(s, 200, "{all}");
    assert!(total(&all) >= 9, "{all}");
    let rows = entries(&all);
    let ts: Vec<i64> = rows.iter().map(|e| e["ts_ms"].as_i64().unwrap()).collect();
    assert!(ts.windows(2).all(|w| w[0] >= w[1]), "newest first");
    let text = all.to_string();
    assert!(
        !text.contains(&key),
        "the API token secret must never be logged"
    );
    assert_eq!(all["nodes"].as_array().unwrap().len(), 1);

    // user + realm + credential kind
    let (_, by_user) = q(&srv, &api.token, "/access-log?user=admin&limit=1000").await;
    assert!(total(&by_user) >= 5);
    assert!(entries(&by_user).iter().all(|e| e["user"] == "admin"));
    let jwt = entries(&by_user)
        .into_iter()
        .find(|e| e["auth_method"] == "jwt" && e["path"] == "/api/vms")
        .expect("session request");
    assert_eq!(jwt["method"], "GET");
    assert_eq!(jwt["path_group"], "/api/vms");
    assert_eq!(jwt["status"], 200);
    assert!(jwt["token_id"].is_null());
    assert_eq!(jwt["client"], "127.0.0.1");
    assert!(jwt["duration_ms"].as_f64().unwrap() >= 0.0);
    assert!(jwt["time"].as_str().unwrap().ends_with('Z'));
    assert!(!jwt["node"].as_str().unwrap().is_empty());

    // API token: by name and by id
    let (_, by_tok) = q(&srv, &api.token, "/access-log?token=audit-ci&limit=1000").await;
    assert_eq!(total(&by_tok), 2, "{by_tok}");
    for e in entries(&by_tok) {
        assert_eq!(e["auth_method"], "api_key");
        assert_eq!(e["token_name"], "audit-ci");
        assert_eq!(e["user"], "admin");
    }
    let tid = entries(&by_tok)[0]["token_id"]
        .as_str()
        .unwrap()
        .to_string();
    if let Some(k) = key_id {
        assert_eq!(k, tid);
    }
    let (_, by_id) = q(&srv, &api.token, &format!("/access-log?token={tid}")).await;
    assert_eq!(total(&by_id), 2);

    // statuses and classes; failed auth keeps the credential kind but no user
    let (_, f401) = q(&srv, &api.token, "/access-log?status=401&limit=1000").await;
    assert!(total(&f401) >= 3, "{f401}");
    assert!(entries(&f401)
        .iter()
        .any(|e| e["auth_method"] == "jwt" && e["user"].is_null()));
    assert!(entries(&f401).iter().any(|e| e["auth_method"].is_null()));
    let (_, c4) = q(&srv, &api.token, "/access-log?status=4xx&limit=1000").await;
    assert!(total(&c4) > total(&f401));
    let (_, c2) = q(&srv, &api.token, "/access-log?status=2xx&limit=1000").await;
    assert!(entries(&c2)
        .iter()
        .all(|e| (200..300).contains(&e["status"].as_i64().unwrap())));
    let (s, _) = q(&srv, &api.token, "/access-log?status=banana").await;
    assert_eq!(s, 400);

    // method, path substring (any case), route template, client
    let (_, post) = q(&srv, &api.token, "/access-log?method=post&limit=1000").await;
    assert!(entries(&post).iter().all(|e| e["method"] == "POST"));
    assert!(entries(&post)
        .iter()
        .any(|e| e["path_group"] == "/api/auth/login" && e["status"] != 200));
    let (_, sub) = q(&srv, &api.token, "/access-log?path=NO-SUCH&limit=1000").await;
    assert_eq!(total(&sub), 1);
    assert_eq!(entries(&sub)[0]["path"], "/api/vms/no-such-vm");
    let (_, g) = q(
        &srv,
        &api.token,
        "/access-log?group=%2Fapi%2Fvms%2F%3Aid&limit=1000",
    )
    .await;
    assert_eq!(total(&g), 1, "{g}");
    let (_, un) = q(&srv, &api.token, "/access-log?group=unmatched").await;
    assert!(total(&un) >= 1);
    let (_, cl) = q(&srv, &api.token, "/access-log?client=127.0.0.1").await;
    assert!(
        total(&cl) >= total(&all),
        "every request came from the loopback address"
    );
    let (_, none) = q(&srv, &api.token, "/access-log?client=203.0.113.99").await;
    assert_eq!(total(&none), 0);

    // exact time range with second precision
    let t0 = entries(&by_tok)[1]["ts_ms"].as_i64().unwrap();
    let sec = t0 / 1000;
    let (_, in_sec) = q(
        &srv,
        &api.token,
        &format!("/access-log?start={sec}&end={sec}&limit=1000"),
    )
    .await;
    assert!(entries(&in_sec)
        .iter()
        .any(|e| e["ts_ms"].as_i64().unwrap() == t0));
    assert!(entries(&in_sec)
        .iter()
        .all(|e| e["ts_ms"].as_i64().unwrap() / 1000 == sec));
    let iso = chrono::DateTime::from_timestamp(sec, 0)
        .unwrap()
        .format("%Y-%m-%dT%H:%M:%S")
        .to_string();
    let (_, in_iso) = q(
        &srv,
        &api.token,
        &format!("/access-log?start={iso}&end={iso}&limit=1000"),
    )
    .await;
    assert_eq!(total(&in_iso), total(&in_sec));
    let (_, future) = q(
        &srv,
        &api.token,
        &format!("/access-log?start={}", sec + 3600),
    )
    .await;
    assert_eq!(total(&future), 0);
    let (s, _) = q(
        &srv,
        &api.token,
        &format!("/access-log?start={}&end={}", sec + 5, sec),
    )
    .await;
    assert_eq!(s, 400, "start after end");

    // paging
    let (_, p1) = q(&srv, &api.token, "/access-log?limit=2&offset=0").await;
    let (_, p2) = q(&srv, &api.token, "/access-log?limit=2&offset=2").await;
    assert_eq!(entries(&p1).len(), 2);
    assert_ne!(entries(&p1)[0]["id"], entries(&p2)[0]["id"]);
    assert!(total(&p1) >= 4 && p1["limit"] == 2);

    // detail
    let id = jwt["id"].as_i64().unwrap();
    let (s, d) = q(&srv, &api.token, &format!("/access-log/{id}")).await;
    assert_eq!(s, 200);
    assert_eq!(d["path"], "/api/vms");
    let (s, _) = q(&srv, &api.token, "/access-log/99999999").await;
    assert_eq!(s, 404);

    // statistics follow the same filter
    let (_, again) = q(&srv, &api.token, "/access-log?user=admin&limit=1").await;
    let (s, st) = q(&srv, &api.token, "/access-log/stats?user=admin").await;
    assert_eq!(s, 200, "{st}");
    // the list request itself is in the stats (and the stats request is not yet)
    assert_eq!(st["total"].as_i64().unwrap(), total(&again) + 1, "{st}");
    let bsum: i64 = st["buckets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["total"].as_i64().unwrap())
        .sum();
    assert_eq!(bsum, st["total"].as_i64().unwrap());
    assert_eq!(st["top_users"][0]["value"], "admin");
    assert!(st["top_tokens"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["label"] == "audit-ci" && t["count"] == 2));
    assert!(st["top_groups"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["value"] == "/api/vms"));
    assert!(st["top_clients"][0]["value"] == "127.0.0.1");
    assert!(st["top_statuses"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["value"] == "200"));
    assert!(st["errors"].as_i64().unwrap() >= 1);
    assert_eq!(st["collector"]["enabled"], true);
    assert_eq!(st["collector"]["dropped"], 0);
    let (_, st4) = q(&srv, &api.token, "/access-log/stats?status=4xx").await;
    assert_eq!(st4["total"], st4["errors"]);
    let (_, empty) = q(&srv, &api.token, "/access-log/stats?user=nobody-at-all").await;
    assert_eq!(empty["total"], 0);
    let (s, _) = q(&srv, &api.token, "/access-log/stats?buckets=abc").await;
    assert_eq!(s, 400);

    // node selection on a single node
    let (_, all_nodes) = q(&srv, &api.token, "/access-log?node=all&limit=1").await;
    assert!(total(&all_nodes) >= 9);
    let (s, _) = q(&srv, &api.token, "/access-log?node=no-such-node").await;
    assert!(s == 404 || s == 503, "unknown node: {s}");
}

#[tokio::test]
async fn only_auditors_read_and_only_admins_change_settings() {
    let dir = scratch("access-log-rbac");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;
    for (name, role) in [("al-vmuser", "VMUser"), ("al-auditor", "Auditor")] {
        let (s, v) = api
            .post(
                "/users",
                json!({"username": name, "password": "al-test-pass-1", "email": format!("{name}@example.com")}),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        let (s, v) = api
            .post(
                "/acl",
                json!({"path": "/", "subject_type": "user", "subject": name, "role": role}),
            )
            .await;
        assert_eq!(s, 201, "{v}");
    }
    let c = srv.client();
    let login = |u: &'static str| {
        let (c, base) = (c.clone(), srv.base());
        async move {
            let v: Value = c
                .post(format!("{base}/auth/login"))
                .json(&json!({"username": u, "password": "al-test-pass-1"}))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            v["ticket"].as_str().unwrap().to_string()
        }
    };
    let vmuser = login("al-vmuser").await;
    let auditor = login("al-auditor").await;
    for p in [
        "/access-log",
        "/access-log/stats",
        "/access-log/1",
        "/access-log/settings",
    ] {
        assert_eq!(q(&srv, &vmuser, p).await.0, 403, "{p} for a VM user");
    }
    assert_eq!(q(&srv, &auditor, "/access-log").await.0, 200);
    assert_eq!(q(&srv, &auditor, "/access-log/stats").await.0, 200);
    assert_eq!(q(&srv, &auditor, "/access-log/settings").await.0, 200);
    let put = |tok: String, body: Value| {
        let (c, base) = (c.clone(), srv.base());
        async move {
            c.put(format!("{base}/access-log/settings"))
                .bearer_auth(tok)
                .json(&body)
                .send()
                .await
                .unwrap()
                .status()
                .as_u16()
        }
    };
    assert_eq!(
        put(auditor, json!({})).await,
        403,
        "an auditor reads, never changes"
    );
    // the peer routes refuse everyone but another cluster node
    let (s, _) = api
        .post(
            "/ha/peer/access-log/list",
            json!({"filter": {}, "limit": 1, "offset": 0}),
        )
        .await;
    assert!(s == 403 || s == 404, "peer route for a user: {s}");
}

#[tokio::test]
async fn trusted_proxies_settings_and_persistence() {
    let dir = scratch("access-log-settings");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;
    let c = srv.client();

    // defaults are documented values
    let (s, d) = api.get("/access-log/settings").await;
    assert_eq!(s, 200);
    assert_eq!(d["enabled"], true);
    assert_eq!(d["retention_days"], 30);
    assert_eq!(d["max_mb"], 256);
    assert_eq!(d["trusted_proxies"], json!([]));

    // X-Forwarded-For from an untrusted peer is ignored
    hit(
        &c,
        &srv,
        "/vms",
        &[("X-Forwarded-For", "198.51.100.7")],
        Some(&api.token),
    )
    .await;
    let (_, v) = api.get("/access-log?limit=1").await;
    assert_eq!(entries(&v)[0]["client"], "127.0.0.1");

    // bad settings are refused, nothing changes
    for bad in [
        json!({"retention_days": 0}),
        json!({"max_mb": 0}),
        json!({"trusted_proxies": ["not an address"]}),
        json!({"db_file": "../escape.db"}),
    ] {
        let (s, v) = api.put("/access-log/settings", bad.clone()).await;
        assert_eq!(s, 400, "{bad}: {v}");
    }
    assert_eq!(
        api.get("/access-log/settings").await.1["retention_days"],
        30
    );

    // a trusted proxy's header is believed (nearest untrusted hop)
    let (s, v) = api
        .put(
            "/access-log/settings",
            json!({"trusted_proxies": ["127.0.0.0/8"], "retention_days": 14, "exclude_path_prefixes": ["/api/health"]}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    hit(
        &c,
        &srv,
        "/vms",
        &[("X-Forwarded-For", "203.0.113.5, 198.51.100.7")],
        Some(&api.token),
    )
    .await;
    let (_, v) = api.get("/access-log?limit=1").await;
    assert_eq!(entries(&v)[0]["client"], "198.51.100.7");
    let (_, v) = api.get("/access-log?client=198.51.100.7").await;
    assert_eq!(total(&v), 1);

    // excluded paths are not recorded
    let before = total(&api.get("/access-log?path=/api/health").await.1);
    hit(&c, &srv, "/health", &[], None).await;
    assert_eq!(
        total(&api.get("/access-log?path=/api/health").await.1),
        before
    );

    // settings and entries survive a restart
    let marker = total(&api.get("/access-log").await.1);
    assert!(marker > 0);
    srv.restart_after(nix::sys::signal::Signal::SIGTERM).await;
    let api = srv.api().await;
    let (_, d) = api.get("/access-log/settings").await;
    assert_eq!(d["retention_days"], 14);
    assert_eq!(d["trusted_proxies"], json!(["127.0.0.0/8"]));
    assert!(total(&api.get("/access-log").await.1) >= marker);

    // switched off: requests after that are not recorded
    let (s, _) = api
        .put("/access-log/settings", json!({"enabled": false}))
        .await;
    assert_eq!(s, 200);
    let off = total(&api.get("/access-log").await.1);
    hit(&c, &srv, "/vms", &[], Some(&api.token)).await;
    hit(&c, &srv, "/vms", &[], Some(&api.token)).await;
    assert_eq!(total(&api.get("/access-log").await.1), off);
    let (_, st) = api.get("/access-log/stats").await;
    assert_eq!(st["collector"]["enabled"], false);

    // declarative export carries the section, and apply sets it
    let (s, ex) = api.get("/declarative/export").await;
    assert_eq!(s, 200, "{ex}");
    let spec = ex["spec"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| ex.to_string());
    assert!(spec.contains("access_log"), "{spec}");
    let (s, v) = api
        .post(
            "/declarative/apply",
            json!({"spec": "version: 1\naccess_log:\n  enabled: true\n  max_mb: 64\n"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (_, d) = api.get("/access-log/settings").await;
    assert_eq!(
        (d["enabled"].clone(), d["max_mb"].clone()),
        (json!(true), json!(64))
    );
}
