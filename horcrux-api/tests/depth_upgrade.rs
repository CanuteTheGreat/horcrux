//! Upgrade: data written by the previous release survives the HEAD
//! binary's migrations and stays usable.
//!
//! Runs the previous release's `horcrux-api` (HORCRUX_DEPTH_OLD_BIN, built
//! from the last release tag by the nightly workflow) on a fresh data dir,
//! creates a VM, a user and an API key through ITS API, stops it, then
//! starts HEAD on the same database: HEAD must start (migrations apply),
//! everything must still be there and work (log in as both users, the API
//! key authenticates, the VM's disk is intact and the VM boots), and a
//! second HEAD start must be a no-op for the migrations.

mod support;

use serde_json::json;
use std::path::PathBuf;
use support::*;

#[tokio::test]
#[ignore = "nightly depth suite: needs HORCRUX_DEPTH_OLD_BIN (previous release binary)"]
async fn previous_release_data_survives_upgrade_to_head() {
    let old_bin = PathBuf::from(
        std::env::var("HORCRUX_DEPTH_OLD_BIN")
            .expect("HORCRUX_DEPTH_OLD_BIN must point at the previous release's horcrux-api"),
    );
    assert!(old_bin.is_file(), "{} missing", old_bin.display());
    let dir = scratch("upgrade");
    let tag = std::process::id();
    let vm = format!("up{tag}");
    let user = format!("upgrader{tag}");
    let pass = "Upgrade-Pass-2026";

    // ---- previous release ------------------------------------------------
    let mut old = Server::with_bin(&dir, old_bin);
    old.start();
    old.wait_healthy().await;
    let a = Api::login_with(old.client(), &old.base(), "admin", &admin_password()).await;
    let (s, created) = a
        .post(
            "/vms",
            json!({"id": vm, "name": "upgrade vm", "hypervisor": "qemu", "architecture": "x86_64",
                   "cpus": 2, "memory": 192, "disk_size": 1, "status": "stopped"}),
        )
        .await;
    assert!(s == 200 || s == 201, "old: create vm: {s} {created}");
    let (s, b) = a
        .post(
            "/auth/register",
            json!({"username": user, "password": pass, "email": format!("{user}@errant.solutions")}),
        )
        .await;
    assert!((200..300).contains(&s), "old: register user: {s} {b}");
    let (s, key) = a
        .post("/users/admin/api-keys", json!({"name": "upgrade-key"}))
        .await;
    assert!((200..300).contains(&s), "old: api key: {s} {key}");
    let api_key = key["key"].as_str().expect("api key").to_string();
    let (_, vm_old) = a.get(&format!("/vms/{vm}")).await;
    // v0.1.0 recorded no disks in the database (added by migration 021)
    // and always put the image at /var/lib/horcrux/vms/<id>.qcow2.
    let disk = PathBuf::from(
        vm_old["disks"][0]["path"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| format!("/var/lib/horcrux/vms/{vm}.qcow2")),
    );
    assert!(disk.is_file(), "old release created {}", disk.display());
    let vm_storage = disk.parent().unwrap().to_string_lossy().to_string();
    // Known content on the disk.
    let seed = dir.join("seed.raw");
    std::fs::write(&seed, vec![0x5au8; 4 << 20]).unwrap();
    run(
        "qemu-img",
        &[
            "convert",
            "-f",
            "raw",
            "-O",
            "qcow2",
            &seed.to_string_lossy(),
            &disk.to_string_lossy(),
        ],
    );
    let sha = content_sha(&disk);
    old.term();

    // ---- HEAD on the same database --------------------------------------
    // HEAD with the same VM storage directory as the old install.
    let mut head = Server::new(&dir).env("HORCRUX_VM_STORAGE", &vm_storage);
    head.port = old.port;
    for round in 1..=2 {
        head.start();
        head.wait_healthy().await;
        let log = head.log_text();
        assert!(!log.contains("panicked"), "HEAD start {round}:\n{log}");
        let a = head.api().await;

        let (s, v) = a.get(&format!("/vms/{vm}")).await;
        assert_eq!(s, 200, "round {round}: VM after upgrade: {v}");
        assert_eq!(v["name"], "upgrade vm");
        assert_eq!(v["cpus"], 2);
        assert_eq!(v["memory"], 192);
        assert_eq!(v["status"], "stopped");
        assert_eq!(
            v["disks"][0]["path"].as_str(),
            Some(disk.to_string_lossy().as_ref()),
            "round {round}: the upgraded VM has its disk: {v}"
        );
        assert_eq!(
            content_sha(&disk),
            sha,
            "round {round}: disk content intact"
        );

        let u = Api::try_login(head.client(), &head.base(), &user, pass).await;
        assert!(
            u.is_ok(),
            "round {round}: user from the old release logs in: {:?}",
            u.err()
        );

        let r = head
            .client()
            .get(format!("{}/vms", head.base()))
            .header("x-api-key", &api_key)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "round {round}: old API key still works");

        if round == 1 {
            // The upgraded VM is fully usable.
            let (s, b) = a.post(&format!("/vms/{vm}/start"), json!({})).await;
            assert_eq!(s, 200, "start upgraded VM: {b}");
            wait_until("QEMU of upgraded VM", 30, || qemu_pid(&vm).is_some()).await;
            let (s, b) = a.post(&format!("/vms/{vm}/stop"), json!({})).await;
            assert_eq!(s, 200, "stop upgraded VM: {b}");
            wait_until("QEMU stopped", 30, || qemu_pid(&vm).is_none()).await;
            let (s, b) = a
                .post(
                    &format!("/vms/{vm}/snapshots"),
                    json!({"name": "post-upgrade"}),
                )
                .await;
            assert_eq!(s, 200, "snapshot upgraded VM: {b}");
        }
        head.term();
    }
    let mut head = Server::new(&dir).env("HORCRUX_VM_STORAGE", &vm_storage);
    head.start();
    head.wait_healthy().await;
    let a = head.api().await;
    assert!((200..300).contains(&a.delete(&format!("/vms/{vm}")).await));
    assert!(!disk.exists(), "deleting the upgraded VM removes its disk");
    head.term();
    let _ = std::fs::remove_dir_all(&dir);
}
