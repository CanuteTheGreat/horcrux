//! Replication jobs over HTTP against a freshly spawned horcrux-api:
//! - unknown ids are 404 on get / status / delete / execute (never 500);
//! - "Execute Now" reports a missing precondition (no `zfs` on this node)
//!   as an error up front, and when it does start, a background failure is
//!   stored on the job (`last_status` failed + `last_error`).
//!
//! The failed-state persistence itself (including restart) is covered by
//! unit tests in crates/horcrux-vm/src/vm/replication.rs.

use crate::support::{scratch, Server};
use serde_json::json;

fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
        .unwrap_or(false)
}

#[tokio::test]
async fn replication_unknown_job_is_404_and_execute_failures_are_visible() {
    let dir = scratch("replication-api");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;

    for path in ["/replication/jobs/nope", "/replication/jobs/nope/status"] {
        let (s, v) = api.get(path).await;
        assert_eq!(s, 404, "GET {path}: {v}");
    }
    assert_eq!(api.delete("/replication/jobs/nope").await, 404);
    let (s, v) = api.post("/replication/jobs/nope/execute", json!({})).await;
    assert_eq!(s, 404, "execute unknown: {v}");

    // a real job to execute
    let vm = format!("repl{}", std::process::id());
    let (s, b) = api
        .post(
            "/vms",
            json!({"id": vm, "name": "repl source", "hypervisor": "qemu",
                   "architecture": "x86_64", "cpus": 1, "memory": 128, "disk_size": 1,
                   "status": "stopped", "display": {"protocol": "none"}}),
        )
        .await;
    assert!((200..300).contains(&s), "create vm: {s} {b}");
    let (s, job) = api
        .post(
            "/replication/jobs",
            json!({"source_vm_id": vm, "source_snapshot": "base",
                   "target_node": "node-b", "target_pool": "tank", "schedule": "manual"}),
        )
        .await;
    assert_eq!(s, 200, "create job: {job}");
    let id = job["id"].as_str().unwrap().to_string();

    let (s, got) = api.get(&format!("/replication/jobs/{id}")).await;
    assert_eq!(s, 200, "{got}");
    assert!(got["last_error"].is_null(), "{got}");

    let (s, v) = api
        .post(&format!("/replication/jobs/{id}/execute"), json!({}))
        .await;
    if !on_path("zfs") || !on_path("ssh") {
        // precondition missing: clear error now, nothing started, nothing stored
        assert_eq!(s, 422, "execute without zfs/ssh: {v}");
        assert!(
            v.to_string().contains("not installed on this node"),
            "message should name the missing command: {v}"
        );
        let (_, got) = api.get(&format!("/replication/jobs/{id}")).await;
        assert!(got["last_status"].is_null(), "{got}");
    } else {
        // zfs + ssh exist but the snapshot/target do not: the run starts and
        // then fails; the failure must show up on the job.
        assert_eq!(s, 202, "{v}");
        let mut failed = None;
        for _ in 0..120 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let (_, got) = api.get(&format!("/replication/jobs/{id}")).await;
            if got["last_status"] == "failed" {
                failed = Some(got);
                break;
            }
        }
        let got = failed.expect("job never reported failed");
        assert!(
            got["last_error"].as_str().is_some_and(|e| !e.is_empty()),
            "{got}"
        );
    }

    assert_eq!(api.delete(&format!("/replication/jobs/{id}")).await, 204);
    assert_eq!(api.delete(&format!("/replication/jobs/{id}")).await, 404);
}
