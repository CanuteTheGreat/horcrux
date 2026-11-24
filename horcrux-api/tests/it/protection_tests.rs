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
    assert_eq!(api.delete("/vms/keep2").await, 409);
    assert_eq!(api.get("/vms/keep2").await.0, 200, "VM must still exist");
}
