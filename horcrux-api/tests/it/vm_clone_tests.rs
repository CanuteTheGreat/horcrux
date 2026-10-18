//! VM clones through the real API binary with real qemu-img disks
//! (per-push suite; needs qemu-img, and qemu-system-x86_64 for the failed
//! start check):
//!
//! * a linked clone's disk is a qcow2 overlay on the source disk, and a
//!   linked clone of that clone chains on top of it;
//! * deleting a VM whose disk backs linked clones is refused with 409
//!   naming every dependent (direct and further down the chain), and the
//!   disk stays; once the clones are gone the delete works (it used to
//!   delete the disk and leave the clones unbootable);
//! * every clone is recorded as a clone job (GET /api/clone-jobs used to
//!   stay empty), and a refused clone is not;
//! * a clone without an explicit id gets Create VM's `vm-<unix time>` id,
//!   and an id that's taken is refused before any disk is written;
//! * a QEMU start that fails (missing backing file) and a delete leave no
//!   `.qmp.sock` / `.serial.sock` behind.

use crate::support::*;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

async fn disk_of(a: &Api, id: &str) -> PathBuf {
    let (s, vm) = a.get(&format!("/vms/{id}")).await;
    assert_eq!(s, 200, "{vm}");
    PathBuf::from(vm["disks"][0]["path"].as_str().expect("disk path"))
}

fn backing_of(disk: &Path) -> Option<PathBuf> {
    let out = Command::new("qemu-img")
        .args(["info", "--output=json", "-U"])
        .arg(disk)
        .output()
        .unwrap();
    assert!(out.status.success(), "qemu-img info {}", disk.display());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    v["full-backing-filename"]
        .as_str()
        .or_else(|| v["backing-filename"].as_str())
        .map(PathBuf::from)
}

/// Files in the VM storage directory that belong to VM disk `disk`
/// (`<id>.qcow2`, `<id>.qmp.sock`, ...).
fn files_of(disk: &Path) -> Vec<String> {
    let stem = disk.file_stem().unwrap().to_string_lossy().to_string();
    let mut v: Vec<String> = std::fs::read_dir(disk.parent().unwrap())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with(&format!("{stem}.")))
        .collect();
    v.sort();
    v
}

fn is_create_vm_id(id: &str) -> bool {
    id.strip_prefix("vm-")
        .is_some_and(|t| !t.is_empty() && t.chars().all(|c| c.is_ascii_digit()))
}

async fn clone(a: &Api, src: &str, body: Value) -> (u16, Value) {
    a.post(&format!("/vms/{src}/clone"), body).await
}

#[tokio::test]
async fn linked_clones_protect_their_base_and_are_recorded_as_jobs() {
    let dir = scratch("vm-clone");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let a = srv.api().await;

    let src = format!("clsrc{}", std::process::id());
    let (s, b) = a
        .post(
            "/vms",
            json!({"id": src, "name": "clone source", "hypervisor": "qemu",
                   "architecture": "x86_64", "cpus": 1, "memory": 128, "disk_size": 1,
                   "status": "stopped", "display": {"protocol": "none"}}),
        )
        .await;
    assert!((200..300).contains(&s), "create source: {s} {b}");
    let src_disk = disk_of(&a, &src).await;
    assert!(src_disk.is_file());

    // Linked clone, no id: Create VM's id scheme, overlay on the source.
    let (s, linked) = clone(&a, &src, json!({"name": "cl linked", "mode": "linked"})).await;
    assert_eq!(s, 200, "linked clone: {linked}");
    let linked_id = linked["id"].as_str().unwrap().to_string();
    assert!(is_create_vm_id(&linked_id), "clone id {linked_id}");
    let linked_disk = disk_of(&a, &linked_id).await;
    assert_eq!(
        backing_of(&linked_disk).map(|p| std::fs::canonicalize(p).unwrap()),
        Some(std::fs::canonicalize(&src_disk).unwrap()),
        "linked clone disk is an overlay on the source"
    );

    // A linked clone of the linked clone chains on top of it.
    let (s, chained) = clone(
        &a,
        &linked_id,
        json!({"name": "cl chained", "mode": "linked"}),
    )
    .await;
    assert_eq!(s, 200, "chained clone: {chained}");
    let chained_id = chained["id"].as_str().unwrap().to_string();
    assert!(
        is_create_vm_id(&chained_id) && chained_id != linked_id,
        "{chained_id}"
    );
    let chained_disk = disk_of(&a, &chained_id).await;

    // Full clone: standalone disk.
    let (s, full) = clone(&a, &src, json!({"name": "cl full", "mode": "full"})).await;
    assert_eq!(s, 200, "full clone: {full}");
    let full_id = full["id"].as_str().unwrap().to_string();
    let full_disk = disk_of(&a, &full_id).await;
    assert_eq!(
        backing_of(&full_disk),
        None,
        "full clone has no backing file"
    );

    // An id that's taken is refused and the existing disk untouched.
    let before = std::fs::metadata(&linked_disk).unwrap().len();
    let (s, b) = clone(
        &a,
        &src,
        json!({"name": "dup", "id": linked_id, "mode": "full"}),
    )
    .await;
    assert_eq!(s, 409, "clone onto an existing id: {b}");
    assert!(backing_of(&linked_disk).is_some());
    assert_eq!(std::fs::metadata(&linked_disk).unwrap().len(), before);
    let (s, b) = clone(&a, &src, json!({"name": "x", "mode": "sideways"})).await;
    assert_eq!(s, 400, "bad mode: {b}");

    // Every clone is a completed clone job; the refused ones aren't jobs.
    let (s, jobs) = a.get("/clone-jobs").await;
    assert_eq!(s, 200, "{jobs}");
    let jobs = jobs.as_array().unwrap().clone();
    for (name, id, from) in [
        ("cl linked", &linked_id, &src),
        ("cl chained", &chained_id, &linked_id),
        ("cl full", &full_id, &src),
    ] {
        let j = jobs
            .iter()
            .find(|j| j["target_vm_name"] == name)
            .unwrap_or_else(|| panic!("no clone job for {name}: {jobs:?}"));
        assert_eq!(j["state"], "completed", "{j}");
        assert_eq!(j["progress"], 100, "{j}");
        assert_eq!(&j["target_vm_id"], id.as_str(), "{j}");
        assert_eq!(&j["source_vm_id"], from.as_str(), "{j}");
        let jid = j["id"].as_str().unwrap();
        let (s, one) = a.get(&format!("/clone-jobs/{jid}")).await;
        assert_eq!((s, &one["id"]), (200, &j["id"]), "{one}");
    }
    assert_eq!(jobs.len(), 3, "refused clones are not recorded: {jobs:?}");

    // The base of linked clones can't be deleted: 409 naming every
    // dependent (direct and chained); the disk stays.
    let (s, b) = a
        .call(reqwest::Method::DELETE, &format!("/vms/{src}"), None)
        .await;
    assert_eq!(s, 409, "delete base of linked clones: {b}");
    let msg = b["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains(&linked_id) && msg.contains(&chained_id),
        "{msg}"
    );
    assert!(
        !msg.contains(&full_id),
        "a full clone is not a dependent: {msg}"
    );
    assert!(src_disk.is_file(), "base disk kept");
    let (s, _) = a.get(&format!("/vms/{src}")).await;
    assert_eq!(s, 200, "base VM still listed");
    // The middle of the chain is protected too.
    let (s, b) = a
        .call(reqwest::Method::DELETE, &format!("/vms/{linked_id}"), None)
        .await;
    assert_eq!(s, 409, "delete middle of a chain: {b}");

    // A start that fails after QEMU made its sockets (missing backing
    // file: the base disk is moved away behind the API's back) leaves no
    // sockets behind.
    if Command::new("qemu-system-x86_64")
        .arg("--version")
        .output()
        .is_ok()
    {
        let aside = src_disk.with_extension("aside");
        std::fs::rename(&src_disk, &aside).unwrap();
        let (s, b) = a.post(&format!("/vms/{linked_id}/start"), json!({})).await;
        assert!(
            s >= 400,
            "start without its backing file must fail: {s} {b}"
        );
        let left: Vec<String> = files_of(&linked_disk)
            .into_iter()
            .filter(|n| n.ends_with(".sock"))
            .collect();
        assert!(
            left.is_empty(),
            "sockets left after a failed start: {left:?}"
        );
        std::fs::rename(&aside, &src_disk).unwrap();
    }

    // A template's disk is protected the same way while a VM linked-cloned
    // from the template exists.
    let (s, tpl) = a
        .post(
            "/templates",
            json!({"vm_id": full_id, "name": "cl template"}),
        )
        .await;
    assert_eq!(s, 201, "create template: {tpl}");
    let tpl_id = tpl["id"].as_str().unwrap().to_string();
    let tplc = format!("{src}-tplc");
    let (s, b) = a
        .post(
            &format!("/templates/{tpl_id}/clone"),
            json!({"new_vm_id": tplc, "new_vm_name": tplc, "clone_type": "linked",
                   "storage_pool": null, "cloudinit_config": null}),
        )
        .await;
    assert!((200..300).contains(&s), "linked template clone: {s} {b}");
    let tplc_disk = disk_of(&a, &tplc).await;
    assert!(backing_of(&tplc_disk).is_some(), "template clone is linked");
    let (s, b) = a
        .call(
            reqwest::Method::DELETE,
            &format!("/templates/{tpl_id}"),
            None,
        )
        .await;
    assert_eq!(s, 409, "delete template under a linked clone: {b}");
    assert!(
        b["message"].as_str().unwrap_or_default().contains(&tplc),
        "{b}"
    );
    assert_eq!(a.delete(&format!("/vms/{tplc}")).await, 204);
    let (s, b) = a
        .call(
            reqwest::Method::DELETE,
            &format!("/templates/{tpl_id}"),
            None,
        )
        .await;
    assert!((200..300).contains(&s), "delete template: {s} {b}");

    // Stale sockets from a killed QEMU are removed with the VM.
    for ext in ["qmp.sock", "serial.sock"] {
        std::fs::write(chained_disk.with_extension(ext), b"").unwrap();
    }

    // Leaves first: then the base can go, and nothing of theirs is left.
    for (id, disk) in [
        (&chained_id, &chained_disk),
        (&linked_id, &linked_disk),
        (&full_id, &full_disk),
        (&src, &src_disk),
    ] {
        let (s, b) = a
            .call(reqwest::Method::DELETE, &format!("/vms/{id}"), None)
            .await;
        assert!((200..300).contains(&s), "delete {id}: {s} {b}");
        assert_eq!(files_of(disk), Vec::<String>::new(), "files left of {id}");
    }
}
