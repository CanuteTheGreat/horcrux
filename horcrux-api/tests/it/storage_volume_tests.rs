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

/// A pool volume that is the base image of a linked clone's disk (the
/// clone's overlay has it as backing file) can't be deleted or grown from
/// the Storage page, and Detach with "delete file" can't remove it from
/// the source VM either - both used to delete it and leave the clone
/// unbootable ("Could not open backing file"). Once the clone is gone the
/// volume can be deleted.
#[tokio::test]
async fn linked_clone_base_volumes_are_protected() {
    let dir = scratch("vol-linked-base");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;
    let pool_dir = dir.join("basepool");
    std::fs::create_dir_all(&pool_dir).unwrap();
    let pool = api
        .ok(
            reqwest::Method::POST,
            "/storage/pools",
            Some(json!({"name": "base", "storage_type": "directory",
                        "path": pool_dir.to_string_lossy()})),
        )
        .await;
    let pid = pool["id"].as_str().unwrap().to_string();

    // lbase: disk0 in the VM directory, disk1 on the pool.
    let (s, vm) = api
        .post(
            "/vms",
            json!({"id": "lbase", "name": "lbase", "hypervisor": "qemu",
            "architecture": "x86_64", "cpus": 1, "memory": 256, "disk_size": 1,
            "status": "stopped", "display": {"protocol": "none"}}),
        )
        .await;
    assert_eq!(s, 201, "create vm: {vm}");
    let (s, vm) = api
        .post(
            "/vms/lbase/disks",
            json!({"size_gb": 1, "bus": "virtio", "storage": pid}),
        )
        .await;
    assert_eq!(s, 201, "add disk: {vm}");
    let base = pool_dir.join("lbase-disk1.qcow2");
    assert_eq!(vm["disks"][1]["path"], base.to_string_lossy().as_ref());

    // Linked clone: its disk1 is an overlay on the pool volume.
    let (s, clone) = api
        .post(
            "/vms/lbase/clone",
            json!({"id": "lclone", "name": "lclone", "mode": "linked"}),
        )
        .await;
    assert_eq!(s, 200, "linked clone: {clone}");
    let (_, c) = api.get("/vms/lclone").await;
    let overlay = c["disks"][1]["path"].as_str().unwrap().to_string();
    let info = std::process::Command::new("qemu-img")
        .args(["info", "--output=json", "-U", &overlay])
        .output()
        .unwrap();
    let info: serde_json::Value = serde_json::from_slice(&info.stdout).unwrap();
    let canon = |p: &std::path::Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    assert_eq!(
        info["full-backing-filename"]
            .as_str()
            .or_else(|| info["backing-filename"].as_str())
            .map(|b| canon(std::path::Path::new(b))),
        Some(canon(&base)),
        "clone disk1 is backed by the pool volume: {info}"
    );

    // Detach from lbase deleting the file: refused, nothing changed.
    let (s, r) = api
        .call(
            reqwest::Method::DELETE,
            "/vms/lbase/disks/1?delete_file=true",
            None,
        )
        .await;
    assert_eq!(s, 409, "detach + delete a linked clone's base: {r}");
    assert!(r.to_string().contains("lclone"), "{r}");
    assert!(base.is_file());
    let (_, vm) = api.get("/vms/lbase").await;
    assert_eq!(vm["disks"].as_array().unwrap().len(), 2, "still attached");

    // Detach keeping the file (the QA's path to an unowned base volume).
    let (s, r) = api
        .call(reqwest::Method::DELETE, "/vms/lbase/disks/1", None)
        .await;
    assert_eq!(s, 200, "detach keeping the file: {r}");
    assert!(base.is_file());

    // Storage page Delete / Grow on it: 409 naming the clone.
    let (s, r) = api
        .call(
            reqwest::Method::DELETE,
            &format!("/storage/pools/{pid}/volumes/lbase-disk1"),
            None,
        )
        .await;
    assert_eq!(s, 409, "delete a linked clone's base volume: {r}");
    assert!(r.to_string().contains("lclone"), "{r}");
    assert!(base.is_file(), "base volume kept");
    let (s, r) = api
        .post(
            &format!("/storage/pools/{pid}/volumes/lbase-disk1/resize"),
            json!({"size": 2}),
        )
        .await;
    assert_eq!(s, 409, "grow a linked clone's base volume: {r}");
    let ok = std::process::Command::new("qemu-img")
        .args(["info", "-U", &overlay])
        .status()
        .unwrap();
    assert!(ok.success(), "the clone's overlay still opens");

    // Clone gone -> the volume is free to delete.
    assert!((200..300).contains(&api.delete("/vms/lclone").await));
    assert_eq!(
        api.delete(&format!("/storage/pools/{pid}/volumes/lbase-disk1"))
            .await,
        200
    );
    assert!(!base.exists());
    assert!((200..300).contains(&api.delete("/vms/lbase").await));
    let _ = std::fs::remove_dir_all(&dir);
}

/// CephFS pools: only an actual CephFS mount is accepted, an unmounted
/// directory is refused with the command that mounts it, and nothing is
/// stored for the refused pool. (CI has no Ceph cluster: the accepted
/// path is covered by the mount-table unit tests in `storage::cephfs`.)
#[tokio::test]
async fn cephfs_pool_needs_a_cephfs_mount() {
    let dir = scratch("storage-cephfs");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;
    let plain = dir.join("not-a-mount");
    std::fs::create_dir_all(&plain).unwrap();
    for path in [
        plain.to_string_lossy().to_string(),
        "relative/dir".into(),
        "/no/such/dir".into(),
    ] {
        let (s, v) = api
            .post(
                "/storage/pools",
                json!({"name": "cfs", "storage_type": "cephfs", "path": path}),
            )
            .await;
        assert!(s == 400 || s == 422 || s == 500, "{path}: {s} {v}");
        assert_ne!(s, 201, "{path}: {v}");
    }
    let (_, v) = api
        .post(
            "/storage/pools",
            json!({"name": "cfs", "storage_type": "cephfs", "path": plain.to_string_lossy()}),
        )
        .await;
    assert!(v.to_string().contains("mount point"), "{v}");
    let (_, pools) = api.get("/storage/pools").await;
    assert!(
        pools.as_array().unwrap().iter().all(|p| p["name"] != "cfs"),
        "refused pool was stored: {pools}"
    );
}
