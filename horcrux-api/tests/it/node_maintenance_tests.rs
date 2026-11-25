//! Node maintenance of any member (`/api/nodes/<node>/maintenance`, its
//! `/plan`, the cluster-wide `/api/cluster/maintenance` settings) against a
//! freshly spawned single node: the routes for the local member, the
//! refusals a lone node gives, the typed return-policy setting and the
//! per-node RBAC path (`/nodes/<node>`). Forwarding to other members is
//! exercised by the multi-node vmtest scenario `cluster_maintenance.py`.

use crate::support::{scratch, Api, Server};
use serde_json::json;

#[tokio::test]
async fn per_node_maintenance_routes_settings_and_rbac() {
    let dir = scratch("node-maint");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;

    // the local member, by name
    let (s, v) = api.get("/nodes/maintenance").await;
    assert_eq!(s, 200, "{v}");
    let me = v["node"].as_str().unwrap().to_string();
    assert!(!me.is_empty(), "{v}");
    let (s, v) = api.get(&format!("/nodes/{me}/maintenance")).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["node"], me.as_str());
    assert_eq!(v["enabled"], false);
    assert_eq!(v["effective_return_policy"], "origin", "documented default");
    assert_eq!(v["origins"], json!([]));

    // a name that is no member
    let (s, v) = api.get("/nodes/no-such-member/maintenance").await;
    assert_eq!(s, 404, "{v}");
    let (s, v) = api
        .post("/nodes/no-such-member/maintenance", json!({"enabled": true}))
        .await;
    assert_eq!(s, 404, "{v}");

    // a lone node has nowhere to move guests to
    let (s, v) = api
        .post(&format!("/nodes/{me}/maintenance"), json!({"enabled": true}))
        .await;
    assert_eq!(s, 409, "{v}");
    assert!(v.to_string().contains("not in a cluster"), "{v}");

    // plans: enter (default), exit with a policy, a bad action
    let (s, v) = api.get(&format!("/nodes/{me}/maintenance/plan")).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["moves"], json!([]));
    let (s, v) = api
        .get(&format!("/nodes/{me}/maintenance/plan?action=exit&return=origin"))
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, _) = api
        .get(&format!("/nodes/{me}/maintenance/plan?action=sideways"))
        .await;
    assert_eq!(s, 422);

    // the cluster-wide typed setting: default origin, validated, kept
    let (s, v) = api.get("/cluster/maintenance").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["settings"]["return_policy"], "origin");
    let rows = v["nodes"].as_array().unwrap();
    assert!(
        rows.iter().any(|r| r["node"] == me.as_str() && r["local"] == true),
        "{v}"
    );
    let (s, v) = api
        .put("/cluster/maintenance", json!({"return_policy": "sideways"}))
        .await;
    assert!(s == 400 || s == 422, "{s} {v}");
    let (s, v) = api
        .put("/cluster/maintenance", json!({"return_policy": "none"}))
        .await;
    assert_eq!(s, 200, "{v}");
    let (_, v) = api.get(&format!("/nodes/{me}/maintenance")).await;
    assert_eq!(v["effective_return_policy"], "none");

    // leaving (not in maintenance) is harmless and records the exit
    let (s, v) = api
        .post(&format!("/nodes/{me}/maintenance"), json!({"enabled": false}))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["enabled"], false);
    assert_eq!(v["exit"]["kind"], "return (none)");
    // origin on a lone node: done at once, the next call is not blocked
    for _ in 0..2 {
        let (s, v) = api
            .post(
                &format!("/nodes/{me}/maintenance"),
                json!({"enabled": false, "return": "origin"}),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["exit"]["status"], "done", "{v}");
    }
    let (s, v) = api
        .post(
            &format!("/nodes/{me}/maintenance"),
            json!({"enabled": false, "return": "backwards"}),
        )
        .await;
    assert!(s == 400 || s == 422, "{s} {v}");

    // RBAC: the ACL path is /nodes/<node>
    let mk = |name: &'static str| {
        let api = &api;
        async move {
            let pw = "node-maint-pass-1";
            let (s, u) = api
                .post("/users", json!({"username": name, "password": pw}))
                .await;
            assert_eq!(s, 201, "{u}");
            u["id"].as_str().unwrap().to_string()
        }
    };
    let grant = |uid: String, path: String, role: &'static str| {
        let api = &api;
        async move {
            let (s, v) = api
                .post(
                    "/acl",
                    json!({"path": path, "subject_type": "user", "subject": uid, "role": role}),
                )
                .await;
            assert_eq!(s, 201, "{v}");
        }
    };
    let aud = mk("nm-auditor").await;
    grant(aud, format!("/nodes/{me}"), "Auditor").await;
    let other = mk("nm-other").await;
    grant(other, "/nodes/somewhere-else".into(), "SysAdmin").await;
    let ops = mk("nm-ops").await;
    grant(ops, format!("/nodes/{me}"), "SysAdmin").await;
    let base = srv.base();
    let pw = "node-maint-pass-1";
    let auditor = Api::login_with(srv.client(), &base, "nm-auditor", pw).await;
    let outsider = Api::login_with(srv.client(), &base, "nm-other", pw).await;
    let operator = Api::login_with(srv.client(), &base, "nm-ops", pw).await;
    let path = format!("/nodes/{me}/maintenance");
    assert_eq!(auditor.get(&path).await.0, 200, "Sys.Audit on the node reads it");
    assert_eq!(
        auditor.get(&format!("{path}/plan")).await.0,
        200,
        "and its plan"
    );
    assert_eq!(
        auditor.post(&path, json!({"enabled": true})).await.0,
        403,
        "but cannot change it"
    );
    assert_eq!(outsider.get(&path).await.0, 403, "a grant on another node does not reach this one");
    assert_eq!(outsider.post(&path, json!({"enabled": false})).await.0, 403);
    // Sys.Modify on this node passes the gate (and gets the lone-node refusal)
    assert_eq!(operator.post(&path, json!({"enabled": true})).await.0, 409);
    assert_eq!(operator.post(&path, json!({"enabled": false})).await.0, 200);
    // the cluster-wide setting stays a /cluster matter
    assert_eq!(
        operator
            .put("/cluster/maintenance", json!({"return_policy": "origin"}))
            .await
            .0,
        403
    );
}
