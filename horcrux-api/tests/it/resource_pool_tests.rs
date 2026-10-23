//! Resource pools over HTTP against a freshly spawned horcrux-api: CRUD,
//! one-pool-per-member, and the real effect - a grant on /pool/<id> gives a
//! user the role on exactly that pool's storages (and nothing else), and goes
//! away when the member leaves the pool.

use crate::support::{scratch, Api, Server};
use reqwest::Method;
use serde_json::json;

#[tokio::test]
async fn a_grant_on_a_pool_applies_to_its_members_only() {
    let dir = scratch("resource-pools");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;

    let mut ids = Vec::new();
    for n in ["a", "b"] {
        let d = dir.join(format!("pool-{n}"));
        std::fs::create_dir_all(&d).unwrap();
        let p = api
            .ok(
                Method::POST,
                "/storage/pools",
                Some(json!({"name": n, "storage_type": "directory", "path": d.to_string_lossy()})),
            )
            .await;
        ids.push(p["id"].as_str().unwrap().to_string());
    }
    let (sa, sb) = (ids[0].clone(), ids[1].clone());

    // validation
    let (s, _) = api.post("/pools", json!({"id": "../x"})).await;
    assert_eq!(s, 400);
    let (s, v) = api
        .post("/pools", json!({"id": "p1", "storages": ["nope"]}))
        .await;
    assert_eq!(s, 400, "{v}");
    let (s, v) = api
        .post(
            "/pools",
            json!({"id": "p1", "comment": "team a", "storages": [sa, sa]}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["storages"].as_array().unwrap().len(), 1, "deduplicated");
    assert_eq!(api.post("/pools", json!({"id": "p1"})).await.0, 409);
    // a storage is in one pool only
    let (s, v) = api
        .post("/pools", json!({"id": "p2", "storages": [sa]}))
        .await;
    assert_eq!(s, 409, "{v}");

    // a user with no grants cannot see either storage
    let pw = "pool-user-pass-1";
    let (s, u) = api
        .post("/users", json!({"username": "pooluser", "password": pw}))
        .await;
    assert_eq!(s, 201, "{u}");
    let uid = u["id"].as_str().unwrap().to_string();
    let user = Api::login_with(srv.client(), &srv.base(), "pooluser", pw).await;
    assert_eq!(user.get(&format!("/storage/pools/{sa}")).await.0, 403);
    assert_eq!(user.get("/pools").await.0, 403, "no Pool.Audit yet");

    // grant DatastoreUser on the pool: member storage opens up, the other stays closed
    let (s, v) = api
        .post(
            "/acl",
            json!({"path": "/pool/p1", "subject_type": "user", "subject": uid, "role": "DatastoreAdmin"}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(user.get(&format!("/storage/pools/{sa}")).await.0, 200);
    assert_eq!(user.get(&format!("/storage/pools/{sb}")).await.0, 403);

    // move storage b into the pool: now it opens up; drop a: it closes again
    let (s, v) = api
        .put("/pools/p1", json!({"storages": [sb.clone()]}))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(user.get(&format!("/storage/pools/{sb}")).await.0, 200);
    assert_eq!(user.get(&format!("/storage/pools/{sa}")).await.0, 403);

    // the user cannot change the pool (no Pool.Allocate), admin deletes it
    assert_eq!(user.put("/pools/p1", json!({"comment": "x"})).await.0, 403);
    assert_eq!(api.get("/pools/p1").await.1["comment"], "team a");
    assert_eq!(api.delete("/pools/p1").await, 204);
    assert_eq!(api.get("/pools/p1").await.0, 404);
    assert_eq!(user.get(&format!("/storage/pools/{sb}")).await.0, 403);
    let (_, acl) = api.get("/acl").await;
    assert!(
        !acl.to_string().contains("/pool/p1"),
        "grants on the pool are removed with it: {acl}"
    );
}
