//! Protection flag over HTTP against a freshly spawned horcrux-api: a
//! protected VM refuses DELETE (409) until the flag is cleared, the flag
//! survives a read-back, and unknown guests are 404.

use crate::support::{scratch, Server};
use serde_json::json;

#[tokio::test]
async fn protected_vm_cannot_be_deleted() {
    let dir = scratch("protection");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;

    let (s, v) = api
        .post(
            "/vms",
            json!({"id": "keep1", "name": "Keep", "hypervisor": "qemu", "architecture": "x86_64",
                   "cpus": 1, "memory": 256, "disk_size": 1, "status": "stopped"}),
        )
        .await;
    assert!(s == 200 || s == 201, "{v}");

    assert_eq!(api.get("/vms/nope/protection").await.0, 404);
    let (s, v) = api.get("/vms/keep1/protection").await;
    assert_eq!((s, v), (200, json!({"protected": false})));

    let (s, v) = api
        .put("/vms/keep1/protection", json!({"protected": true}))
        .await;
    assert_eq!((s, v), (200, json!({"protected": true})));
    assert_eq!(
        api.get("/vms/keep1/protection").await.1,
        json!({"protected": true})
    );

    assert_eq!(api.delete("/vms/keep1").await, 409);
    assert_eq!(api.get("/vms/keep1").await.0, 200, "VM must still exist");

    let (s, _) = api
        .put("/vms/keep1/protection", json!({"protected": false}))
        .await;
    assert_eq!(s, 200);
    let st = api.delete("/vms/keep1").await;
    assert!(st == 200 || st == 204, "delete after unprotect: {st}");
    assert_eq!(api.get("/vms/keep1").await.0, 404);
}

/// The Hardware > Advanced "Protection" checkbox (`hardware.adv.protection`)
/// used to be stored but never enforced.
#[tokio::test]
async fn advanced_protection_setting_refuses_delete() {
    let dir = scratch("protection-adv");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;

    let (s, v) = api
        .post(
            "/vms",
            json!({"id": "keep2", "name": "Keep2", "hypervisor": "qemu", "architecture": "x86_64",
                   "cpus": 1, "memory": 256, "disk_size": 1, "status": "stopped",
                   "hardware": {"adv": {"protection": true}}}),
        )
        .await;
    assert!(s == 200 || s == 201, "{v}");
    let (st, body) = api.call(reqwest::Method::DELETE, "/vms/keep2", None).await;
    assert_eq!(st, 409, "{body}");
    // the refusal names the flag (vm_advanced_e2e.py and the UI match on "protection")
    assert!(body.to_string().contains("protection flag"), "{body}");
    assert_eq!(api.get("/vms/keep2").await.0, 200, "VM must still exist");
}

/// `hardware.adv.lock = "manual"` refuses start, config changes and delete
/// until a config update clears the lock.
#[tokio::test]
async fn manual_lock_refuses_lifecycle_until_cleared() {
    let dir = scratch("lock-adv");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;

    let (s, v) = api
        .post(
            "/vms",
            json!({"id": "lock1", "name": "Lock1", "hypervisor": "qemu", "architecture": "x86_64",
                   "cpus": 1, "memory": 256, "disk_size": 1, "status": "stopped",
                   "hardware": {"adv": {"lock": "manual"}}}),
        )
        .await;
    assert!(s == 200 || s == 201, "{v}");
    let (st, body) = api.call(reqwest::Method::POST, "/vms/lock1/start", None).await;
    assert_eq!(st, 409, "{body}");
    assert!(body.to_string().contains("locked"), "{body}");
    let (st, body) = api
        .call(reqwest::Method::PUT, "/vms/lock1/config", Some(json!({"name": "Renamed"})))
        .await;
    assert_eq!(st, 409, "{body}");
    assert_eq!(api.delete("/vms/lock1").await, 409);

    let (st, body) = api
        .call(
            reqwest::Method::PUT,
            "/vms/lock1/config",
            Some(json!({"hardware": {"adv": {}}})),
        )
        .await;
    assert_eq!(st, 200, "unlock must be accepted: {body}");
    let st = api.delete("/vms/lock1").await;
    assert!(st == 200 || st == 204, "delete after unlock: {st}");
}
