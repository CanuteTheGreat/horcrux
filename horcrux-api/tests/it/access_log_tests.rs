//! API access log end to end: the real binary records who called what, a
//! failed call is filterable, statistics add up, and a restart keeps the log.

use crate::support::*;

#[tokio::test]
async fn requests_are_logged_filtered_counted_and_kept_over_a_restart() {
    let dir = scratch("access-log");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let a = srv.api().await;

    let (s, _) = a.get("/vms").await;
    assert_eq!(s, 200);
    let (s, _) = a.get("/no-such-area-xyz").await;
    assert!(s == 404 || s == 403, "{s}");

    let (s, v) = a.get("/access-log?group=vms").await;
    assert_eq!(s, 200, "{v}");
    let first = &v["entries"][0];
    assert_eq!(first["path"], "/api/vms", "{v}");
    assert!(first["user"].as_str().is_some_and(|u| !u.is_empty()), "{v}");
    assert_eq!(first["status"], 200, "{v}");
    assert!(
        first["client"].as_str().is_some_and(|c| !c.is_empty()),
        "{v}"
    );
    assert!(
        !v.to_string().contains(&a.token),
        "token must never be logged"
    );

    let (_, v) = a.get("/access-log?status=4xx").await;
    assert!(v["total"].as_u64().unwrap() >= 1, "{v}");
    assert!(v["entries"]
        .as_array()
        .unwrap()
        .iter()
        .all(|e| e["status"].as_u64().unwrap() / 100 == 4));

    let (s, st) = a.get("/access-log/stats?bucket_secs=60").await;
    assert_eq!(s, 200, "{st}");
    assert!(st["total"].as_u64().unwrap() >= 2, "{st}");
    assert!(st["failed"].as_u64().unwrap() >= 1, "{st}");
    assert!(st["by_group"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["key"] == "vms"));
    assert!(!st["over_time"].as_array().unwrap().is_empty());

    srv.restart_after(nix::sys::signal::Signal::SIGTERM).await;
    let a = srv.api().await;
    let (_, v) = a.get("/access-log?group=vms").await;
    assert!(
        v["total"].as_u64().unwrap() >= 1,
        "kept over a restart: {v}"
    );
}
