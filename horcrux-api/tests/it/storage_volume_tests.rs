//! Storage pool volumes over HTTP against a freshly spawned horcrux-api
//! (support::Server): names that would leave the pool (`../x`,
//! `../../tmp/x`, `a/b`, absolute paths, `.`, `..`, empty, option-like)
//! are refused with 400 on create, delete and resize, and nothing appears
//! or disappears outside the pool directory; valid names create, list,
//! grow and delete real qcow2 files.

use crate::support::{scratch, Server};
use serde_json::json;
use std::path::{Path, PathBuf};

fn tree(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(tree(&p));
            } else {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

#[tokio::test]
async fn volume_names_cannot_escape_the_pool_over_http() {
    let dir = scratch("storage-volumes");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;

    // <outside>/pool is the pool; <outside>/victim.qcow2 is where
    // "../victim" would point.
    let outside = dir.join("outside");
    let pool_dir = outside.join("pool");
    std::fs::create_dir_all(&pool_dir).unwrap();
    std::fs::write(outside.join("victim.qcow2"), b"keep me").unwrap();
    let tmp_marker = std::env::temp_dir().join(format!("hx-trav-{}", std::process::id()));
    let tmp_name = tmp_marker
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();

    let pool = api
        .ok(
            reqwest::Method::POST,
            "/storage/pools",
            Some(json!({"name": "trav", "storage_type": "directory",
                        "path": pool_dir.to_string_lossy()})),
        )
        .await;
    let pid = pool["id"].as_str().unwrap().to_string();
    let before = tree(&outside);

    let tmp_rel = format!("../../../../../../../..{}", tmp_marker.display());
    let bad = [
        "../x".to_string(),
        "../victim".to_string(),
        "../../tmp/x".to_string(),
        tmp_rel.clone(),
        "a/b".to_string(),
        tmp_marker.to_string_lossy().to_string(),
        "/etc/passwd".to_string(),
        String::new(),
        ".".to_string(),
        "..".to_string(),
        "-rf".to_string(),
        "x@y".to_string(),
    ];
    for name in &bad {
        let (s, v) = api
            .post(
                &format!("/storage/pools/{pid}/volumes"),
                json!({"name": name, "size": 1}),
            )
            .await;
        assert_eq!(s, 400, "create {name:?}: {v}");

        // In a URL path segment: percent-encode everything so `/` and dot
        // segments reach the handler as part of the name.
        let enc: String = name.bytes().map(|b| format!("%{b:02X}")).collect();
        if enc.is_empty() || name == "." || name == ".." {
            continue; // not expressible as a path segment (URL dot segments)
        }
        let s = api
            .delete(&format!("/storage/pools/{pid}/volumes/{enc}"))
            .await;
        assert_eq!(s, 400, "delete {name:?}");
        let (s, v) = api
            .post(
                &format!("/storage/pools/{pid}/volumes/{enc}/resize"),
                json!({"size": 2}),
            )
            .await;
        assert_eq!(s, 400, "resize {name:?}: {v}");
    }
    assert_eq!(tree(&outside), before, "nothing changed outside the pool");
    assert_eq!(
        std::fs::read(outside.join("victim.qcow2")).unwrap(),
        b"keep me"
    );
    assert!(!tmp_marker.with_extension("qcow2").exists(), "{tmp_name}");

    // Valid names: real files in the pool, listed, grown, deleted.
    let gb: u64 = 1024 * 1024 * 1024;
    let v = api
        .ok(
            reqwest::Method::POST,
            &format!("/storage/pools/{pid}/volumes"),
            Some(json!({"name": "data-1", "size": 1})),
        )
        .await;
    assert_eq!(v["name"], "data-1");
    assert_eq!(v["size_bytes"], gb);
    assert!(pool_dir.join("data-1.qcow2").is_file());
    let (s, _) = api
        .post(
            &format!("/storage/pools/{pid}/volumes"),
            json!({"name": "data-1", "size": 1}),
        )
        .await;
    assert_eq!(s, 409, "same name again must not truncate the image");

    let (s, list) = api.get(&format!("/storage/pools/{pid}/volumes")).await;
    assert_eq!(s, 200);
    let names: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["data-1"]);

    let v = api
        .ok(
            reqwest::Method::POST,
            &format!("/storage/pools/{pid}/volumes/data-1/resize"),
            Some(json!({"size": 2})),
        )
        .await;
    assert_eq!(v["size_bytes"], 2 * gb);
    let (s, _) = api
        .post(
            &format!("/storage/pools/{pid}/volumes/data-1/resize"),
            json!({"size": 1}),
        )
        .await;
    assert_eq!(s, 400, "shrink refused");

    assert_eq!(
        api.delete(&format!("/storage/pools/{pid}/volumes/data-1"))
            .await,
        200
    );
    assert!(!pool_dir.join("data-1.qcow2").exists());
    assert_eq!(
        api.delete(&format!("/storage/pools/{pid}/volumes/data-1"))
            .await,
        404
    );
    let (s, _) = api.get("/storage/pools/no-such-pool/volumes").await;
    assert_eq!(s, 404);
    assert_eq!(tree(&outside), before);
    let _ = std::fs::remove_dir_all(&dir);
}

/// VM disks placed on a storage pool (Create VM and Add disk pool
/// selector): the images are created in the pool directory, the pool's
/// volume list shows them, a volume in use can't be deleted, and pools
/// that can't hold VM disks are refused.
#[tokio::test]
async fn vm_disks_can_live_on_a_storage_pool() {
    let dir = scratch("vm-disk-pool");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;
    let pool_dir = dir.join("fastpool");
    std::fs::create_dir_all(&pool_dir).unwrap();
    let pool = api
        .ok(
            reqwest::Method::POST,
            "/storage/pools",
            Some(json!({"name": "fast", "storage_type": "directory",
                        "path": pool_dir.to_string_lossy()})),
        )
        .await;
    let pid = pool["id"].as_str().unwrap().to_string();

    // The Create VM / Add disk selector offers the pool.
    let (s, opts) = api.get("/vms/host-options").await;
    assert_eq!(s, 200);
    assert!(
        opts["storage_pools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == pid.as_str()),
        "{opts}"
    );

    // disk0 on the pool, disk1 in the default VM directory; a request
    // path is never trusted.
    let (s, vm) = api
        .post(
            "/vms",
            json!({"id": "pooled", "name": "pooled", "hypervisor": "qemu",
            "architecture": "x86_64", "cpus": 1, "memory": 256, "disk_size": 1,
            "status": "stopped", "display": {"protocol": "none"},
            "disks": [
                {"path": "/etc/evil", "size_gb": 1, "disk_type": "virtio", "cache": "",
                 "storage": pid},
                {"path": "/etc/evil2", "size_gb": 1, "disk_type": "virtio", "cache": ""}
            ]}),
        )
        .await;
    assert_eq!(s, 201, "create vm: {vm}");
    let d0 = pool_dir.join("pooled.qcow2");
    assert_eq!(vm["disks"][0]["path"], d0.to_string_lossy().as_ref());
    assert_eq!(vm["disks"][0]["storage"], pid.as_str());
    assert!(d0.is_file());
    let d1 = vm["disks"][1]["path"].as_str().unwrap().to_string();
    assert!(!d1.starts_with(&*pool_dir.to_string_lossy()) && d1.ends_with("pooled-disk1.qcow2"));
    assert!(vm["disks"][1].get("storage").is_none());
    assert!(!std::path::Path::new("/etc/evil").exists());

    // Add disk on the pool.
    let (s, vm) = api
        .post(
            "/vms/pooled/disks",
            json!({"size_gb": 2, "bus": "virtio", "storage": pid}),
        )
        .await;
    assert_eq!(s, 201, "add disk: {vm}");
    let d2 = pool_dir.join("pooled-disk2.qcow2");
    assert_eq!(vm["disks"][2]["path"], d2.to_string_lossy().as_ref());
    assert!(d2.is_file());

    // The pool's volume list shows both; in-use volumes can't be deleted.
    let (_, list) = api.get(&format!("/storage/pools/{pid}/volumes")).await;
    let names: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["pooled", "pooled-disk2"]);
    assert_eq!(
        api.delete(&format!("/storage/pools/{pid}/volumes/pooled"))
            .await,
        409
    );
    assert!(d0.is_file());
    let (s, _) = api
        .post(
            &format!("/storage/pools/{pid}/volumes/pooled/resize"),
            json!({"size": 5}),
        )
        .await;
    assert_eq!(s, 409, "a VM's disk is grown from the VM, not the pool");

    // Unknown pool -> 404; no space -> 400; nothing created.
    let (s, _) = api
        .post(
            "/vms/pooled/disks",
            json!({"size_gb": 1, "bus": "virtio", "storage": "no-such-pool"}),
        )
        .await;
    assert_eq!(s, 404);
    let (s, _) = api
        .post(
            "/vms/pooled/disks",
            json!({"size_gb": 65536, "bus": "virtio", "storage": pid}),
        )
        .await;
    assert_eq!(s, 400);
    assert!(!pool_dir.join("pooled-disk3.qcow2").exists());

    // Deleting the VM removes its pool disks too.
    assert!((200..300).contains(&api.delete("/vms/pooled").await));
    assert!(!d0.exists() && !d2.exists());
    let (_, list) = api.get(&format!("/storage/pools/{pid}/volumes")).await;
    assert_eq!(list.as_array().unwrap().len(), 0, "{list}");
    let _ = std::fs::remove_dir_all(&dir);
}
