//! Passthrough devices and mediated devices (vGPU) over HTTP against a
//! freshly spawned horcrux-api. The CI runner has no IOMMU / vfio / mdev
//! hardware, so this proves the API contract (types, validation, refusal of
//! devices the host can't give, hostpci stored with the VM, the mdev routes
//! answering with an empty host); the QEMU arguments and the sysfs handling
//! are unit-tested against fixture trees (`vm::mdev`, `vm::qemu`).

use crate::support::{scratch, Server};
use serde_json::json;

#[tokio::test]
async fn hostpci_is_validated_refused_without_the_device_and_mdev_routes_answer() {
    let dir = scratch("mdev");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;

    let (s, v) = api
        .post(
            "/vms",
            json!({"id": "gpuvm", "name": "gpuvm", "hypervisor": "qemu", "architecture": "x86_64",
                   "cpus": 1, "memory": 256, "disk_size": 1, "status": "stopped"}),
        )
        .await;
    assert!(s == 200 || s == 201, "{v}");

    // malformed device strings never reach a QEMU command line
    for bad in ["", "0000:01:00.0,x-vga=on", "../../etc", "01:00.9"] {
        let (s, v) = api
            .put(
                "/vms/gpuvm/config",
                json!({"hardware": {"hostpci": [{"device": bad}]}}),
            )
            .await;
        assert!(s == 400 || s == 422, "{bad:?}: {s} {v}");
    }
    // duplicates / two primary GPUs
    let (s, _) = api
        .put(
            "/vms/gpuvm/config",
            json!({"hardware": {"hostpci": [{"device": "01:00.0"}, {"device": "0000:01:00.0"}]}}),
        )
        .await;
    assert!(s == 400 || s == 422, "duplicate device: {s}");

    // a well-formed device this host doesn't have (or hasn't bound to
    // vfio-pci) is refused with the reason, and nothing is stored
    let (s, v) = api
        .put(
            "/vms/gpuvm/config",
            json!({"hardware": {"hostpci": [{"device": "0000:ff:1f.7"}]}}),
        )
        .await;
    assert!(s == 400 || s == 422, "{s} {v}");
    assert!(v.to_string().contains("does not exist"), "{v}");
    let (s, v) = api
        .put(
            "/vms/gpuvm/config",
            json!({"hardware": {"hostpci": [{"device": "6b1f3a6e-5d0b-4b5f-9f61-0123456789ab"}]}}),
        )
        .await;
    assert!(s == 400 || s == 422, "{s} {v}");
    assert!(v.to_string().contains("mediated device"), "{v}");
    let (s, v) = api.get("/vms/gpuvm").await;
    assert_eq!(s, 200);
    assert!(
        v["hardware"]["hostpci"].is_null() || v["hardware"]["hostpci"] == json!([]),
        "{v}"
    );

    // mdev routes: an empty host has no types or devices
    let (s, v) = api.get("/gpu/mdev/types").await;
    assert_eq!(s, 200, "{v}");
    assert!(v.is_array());
    let (s, v) = api.get("/gpu/mdev/devices").await;
    assert_eq!(s, 200, "{v}");
    assert!(v.is_array());
    let (s, v) = api
        .post(
            "/gpu/mdev/devices",
            json!({"parent": "0000:00:02.0", "type": "i915-GVTg_V5_4"}),
        )
        .await;
    assert!(
        s == 404 || s == 409 || s == 500 || s == 400,
        "no such parent must be refused: {s} {v}"
    );
    let (s, _) = api
        .post("/gpu/mdev/devices", json!({"parent": "../x", "type": "y"}))
        .await;
    assert!(s == 400 || s == 422, "path-like parent: {s}");
    assert_eq!(
        api.delete("/gpu/mdev/devices/11111111-1111-4111-8111-111111111111")
            .await,
        404
    );
    assert!(matches!(
        api.delete("/gpu/mdev/devices/..%2Fetc").await,
        400 | 404 | 422
    ));
}
