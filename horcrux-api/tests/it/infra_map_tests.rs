//! Infrastructure map over HTTP against a freshly spawned horcrux-api (docs/design/infra-map.md):
//! the graph lists what the node runs, a Patronus source (a fake router answering infra-map/v1)
//! is merged under its own prefix, a broken source is a warning node (never a broken map), UniFi
//! is refused as not built, and share keys gate the public live view / stats (401 without, 403
//! for a stats-only key on the graph, revoked = 401). The map component is served without login.

use crate::support::{scratch, Server};
use serde_json::{json, Value};
use std::io::{Read, Write};

/// A one-route HTTP server standing in for a Patronus router's /api/map/shared/graph.
fn fake_patronus() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { continue };
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase();
            let (code, body) = if req.contains("x-map-key: pmk_good") {
                (
                    "200 OK",
                    json!({"schema": "infra-map/v1", "source": {"product": "patronus", "name": "gw"},
                        "groups": [{"id": "seg:@lan", "label": "LAN", "sub": "192.168.50.0/24"}],
                        "nodes": [{"id": "router", "label": "gw", "kind": "router", "status": "up", "href": "/"},
                                  {"id": "dev:52:54:00:aa:00:01", "label": "web1", "group": "seg:@lan", "status": "up",
                                   "mac": "52:54:00:aa:00:01", "class": "computer"}],
                        "edges": [{"from": "router", "to": "seg:@lan", "label": "br0"}],
                        "findings": [{"id": "flat-network", "severity": "high", "title": "LAN is a flat network",
                                      "action": {"href": "/map#advisor"}}]})
                    .to_string(),
                )
            } else {
                ("401 Unauthorized", json!({"error": "key"}).to_string())
            };
            let _ = write!(
                s,
                "HTTP/1.1 {code}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    port
}

#[tokio::test]
async fn graph_sources_share_keys_and_public_view() {
    let dir = scratch("infra-map");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;
    let base = srv.base();
    let root = base.trim_end_matches("/api").to_string();
    let anon = srv.client();

    let (s, v) = api
        .post(
            "/vms",
            json!({"id": "web1", "name": "web1", "hypervisor": "qemu", "architecture": "x86_64",
                   "cpus": 1, "memory": 256, "disk_size": 1, "status": "stopped"}),
        )
        .await;
    assert!(s == 200 || s == 201, "{v}");

    // the local graph
    let (s, g) = api.get("/infra-map").await;
    assert_eq!(s, 200, "{g}");
    assert_eq!(g["schema"], "infra-map/v1");
    let nodes = g["nodes"].as_array().unwrap();
    assert!(
        nodes
            .iter()
            .any(|n| n["id"] == "host:local" && n["kind"] == "host"),
        "{g}"
    );
    assert!(
        nodes
            .iter()
            .any(|n| n["id"] == "vm:web1" && n["group"] == "g:vms" && n["status"] == "down"),
        "{g}"
    );
    assert_eq!(
        anon.get(format!("{base}/infra-map"))
            .send()
            .await
            .unwrap()
            .status(),
        401,
        "the full graph needs a login"
    );

    // sources: UniFi is specified but not built; a Patronus router is merged under its prefix
    let (s, v) = api
        .post(
            "/infra-map/sources",
            json!({"name": "uni", "kind": "unifi", "url": "https://192.0.2.1", "secret": "x"}),
        )
        .await;
    assert_eq!(s, 424, "{v}");
    let port = fake_patronus();
    let (s, v) = api
        .post(
            "/infra-map/sources",
            json!({"name": "lab", "kind": "patronus", "url": format!("http://127.0.0.1:{port}"), "secret": "pmk_good"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = api
        .post(
            "/infra-map/sources",
            json!({"name": "broken", "kind": "patronus", "url": format!("http://127.0.0.1:{port}"), "secret": "pmk_bad"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (_, srcs) = api.get("/infra-map/sources").await;
    assert!(
        srcs.as_array()
            .unwrap()
            .iter()
            .all(|s| s["secret"] == "********"),
        "secrets are never returned: {srcs}"
    );
    let (s, g) = api.get("/infra-map").await;
    assert_eq!(s, 200);
    let ids: Vec<&str> = g["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|n| n["id"].as_str())
        .collect();
    assert!(
        ids.contains(&"lab/router") && ids.contains(&"lab/dev:52:54:00:aa:00:01"),
        "{ids:?}"
    );
    assert!(
        g["groups"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["id"] == "lab/seg:@lan"),
        "{g}"
    );
    assert!(
        g["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["id"] == "broken/error" && n["status"] == "warn"),
        "a failing source is a warning node: {g}"
    );
    assert!(
        g["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["title"] == "lab: LAN is a flat network"),
        "{g}"
    );

    // share keys
    let (s, k) = api
        .post("/infra-map/keys", json!({"name": "wall", "scope": "view"}))
        .await;
    assert_eq!(s, 200, "{k}");
    let view = k["key"].as_str().unwrap().to_string();
    assert!(
        view.starts_with("hmk_")
            && k["live_path"]
                .as_str()
                .unwrap()
                .starts_with("/map/live#k=hmk_")
    );
    let (s, k) = api
        .post(
            "/infra-map/keys",
            json!({"name": "widget", "scope": "stats"}),
        )
        .await;
    assert_eq!(s, 200, "{k}");
    let stats = k["key"].as_str().unwrap().to_string();
    let (s, _) = api.post("/infra-map/keys", json!({"name": "wall"})).await;
    assert_eq!(s, 409, "duplicate key names are refused");
    let get = |url: String, key: Option<String>| {
        let c = anon.clone();
        async move {
            let mut r = c.get(url);
            if let Some(k) = key {
                r = r.header("X-Map-Key", k);
            }
            let r = r.send().await.unwrap();
            let st = r.status().as_u16();
            (st, r.text().await.unwrap_or_default())
        }
    };
    assert_eq!(
        get(format!("{base}/infra-map/shared/graph"), None).await.0,
        401
    );
    assert_eq!(
        get(
            format!("{base}/infra-map/shared/graph"),
            Some("hmk_wrong".into())
        )
        .await
        .0,
        401
    );
    assert_eq!(
        get(
            format!("{base}/infra-map/shared/graph"),
            Some(stats.clone())
        )
        .await
        .0,
        403
    );
    let (st, body) = get(format!("{base}/infra-map/shared/graph"), Some(view.clone())).await;
    assert_eq!(st, 200, "{body}");
    let sg: Value = serde_json::from_str(&body).unwrap();
    assert!(
        sg["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n.get("mac").is_none() && n.get("href").is_none()),
        "basic shares carry no MACs or admin links: {sg}"
    );
    let (st, body) = get(format!("{base}/infra-map/stats"), Some(stats.clone())).await;
    assert_eq!(st, 200, "{body}");
    let sv: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(sv["vms"], 1, "{sv}");
    assert_eq!(sv["sources"], 2, "{sv}");
    assert_eq!(get(format!("{base}/infra-map/stats"), None).await.0, 401);
    let (st, html) = get(format!("{root}/map/live"), None).await;
    assert_eq!(st, 200);
    assert!(
        html.contains("/ui-map/map.js") && html.contains("X-Map-Key"),
        "{html}"
    );
    let (st, js) = get(format!("{root}/ui-map/map.js"), None).await;
    assert_eq!(st, 200);
    assert!(js.contains("toMermaid"));
    let (s, list) = api.get("/infra-map/keys").await;
    assert_eq!(s, 200);
    assert!(
        list.as_array()
            .unwrap()
            .iter()
            .all(|k| k.get("hash").is_none()),
        "hashes stay on the server: {list}"
    );
    assert_eq!(api.delete("/infra-map/keys/wall").await, 200);
    assert_eq!(
        get(format!("{base}/infra-map/shared/graph"), Some(view))
            .await
            .0,
        401,
        "revoked"
    );
    assert_eq!(api.delete("/infra-map/sources/broken").await, 200);
}
