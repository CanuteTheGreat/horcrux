//! Tags, search and the resource tree over HTTP against a freshly spawned
//! horcrux-api: tags persist and are validated, search combines text/kind/
//! tag/status/pool filters, the tree groups by node and pool, and a user only
//! sees the resources they hold VM.Audit / Pool.Audit on.

use crate::support::{scratch, Api, Server};
use reqwest::Method;
use serde_json::{json, Value};

fn ids(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|r| {
            format!(
                "{}:{}",
                r["kind"].as_str().unwrap(),
                r["id"].as_str().unwrap()
            )
        })
        .collect()
}

#[tokio::test]
async fn tags_search_and_tree_follow_permissions() {
    let dir = scratch("inventory");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;

    for (id, name) in [("web1", "Web Frontend"), ("db1", "Database")] {
        let (s, v) = api
            .post(
                "/vms",
                json!({"id": id, "name": name, "hypervisor": "qemu", "architecture": "x86_64",
                       "cpus": 1, "memory": 256, "disk_size": 1, "status": "stopped"}),
            )
            .await;
        assert!(s == 200 || s == 201, "{v}");
    }

    // tags: validated, normalised, persisted
    let (s, _) = api
        .put("/vms/web1/tags", json!({"tags": ["bad tag"]}))
        .await;
    assert_eq!(s, 400);
    let (s, _) = api.put("/vms/nope/tags", json!({"tags": ["x"]})).await;
    assert_eq!(s, 404);
    let (s, v) = api
        .put("/vms/web1/tags", json!({"tags": ["Prod", "web", "prod"]}))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v, json!(["prod", "web"]));
    assert_eq!(
        api.put("/vms/db1/tags", json!({"tags": ["prod", "db"]}))
            .await
            .0,
        200
    );
    let (_, tags) = api.get("/tags").await;
    assert_eq!(
        tags,
        json!([{"tag": "db", "count": 1}, {"tag": "prod", "count": 2}, {"tag": "web", "count": 1}])
    );

    // pool membership shows up as a search filter
    let (s, v) = api
        .post("/pools", json!({"id": "team-a", "vms": ["web1"]}))
        .await;
    assert_eq!(s, 201, "{v}");

    let (_, r) = api.get("/search?q=web+front").await;
    assert_eq!(ids(&r), vec!["vm:web1"]);
    let (_, r) = api.get("/search?tag=prod&kind=vm").await;
    assert_eq!(ids(&r), vec!["vm:db1", "vm:web1"]);
    let (_, r) = api.get("/search?pool=team-a").await;
    assert_eq!(ids(&r), vec!["vm:web1"]);
    assert_eq!(r[0]["tags"], json!(["prod", "web"]));
    let (_, r) = api.get("/search?status=running").await;
    assert!(r.as_array().unwrap().is_empty());
    let (_, r) = api.get("/search?q=team-a&kind=pool").await;
    assert_eq!(ids(&r), vec!["pool:team-a"]);

    // tree: datacenter > node > VM group; pools group has team-a > web1
    let (s, t) = api.get("/tree").await;
    assert_eq!(s, 200);
    assert_eq!(t["kind"], "datacenter");
    let vms = &t["children"][0]["children"][0];
    assert_eq!(vms["name"], "Virtual machines (2)");
    let pools = &t["children"][1];
    assert_eq!(pools["children"][0]["id"], "team-a");
    assert_eq!(pools["children"][0]["children"][0]["id"], "web1");

    // a user granted VMUser on one VM sees only that VM
    let pw = "inventory-user-1";
    let (s, u) = api
        .post("/users", json!({"username": "invuser", "password": pw}))
        .await;
    assert_eq!(s, 201, "{u}");
    let uid = u["id"].as_str().unwrap();
    api.ok(
        Method::POST,
        "/acl",
        Some(json!({"path": "/vms/db1", "subject_type": "user", "subject": uid, "role": "VMUser"})),
    )
    .await;
    let user = Api::login_with(srv.client(), &srv.base(), "invuser", pw).await;
    let (_, r) = user.get("/search").await;
    assert_eq!(ids(&r), vec!["vm:db1"]);
    let (_, t) = user.get("/tree").await;
    assert_eq!(
        t["children"][0]["children"][0]["name"],
        "Virtual machines (1)"
    );
    assert_eq!(t["children"][1]["children"].as_array().unwrap().len(), 0);
    let (_, tags) = user.get("/tags").await;
    assert_eq!(
        tags,
        json!([{"tag": "db", "count": 1}, {"tag": "prod", "count": 1}])
    );
    // tagging needs VM.Config (VMUser lacks it)
    assert_eq!(
        user.put("/vms/db1/tags", json!({"tags": ["x"]})).await.0,
        403
    );
}
