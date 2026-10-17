//! Restart survival and crash consistency, against the real binary.
//!
//! - Running guests are untouched by an API restart (SIGTERM and SIGKILL):
//!   same QEMU PID, still reported running, consoles/snapshots/backups and
//!   config still there.
//! - A QEMU that crashes is reported stopped and can be started again.
//! - The API killed in the middle of a suspend-mode backup or a live
//!   snapshot: after restart the guest is running again (not left paused),
//!   no half-written archive or staging directory is left behind, nothing
//!   half-done is in the catalog, and the next backup works.
//! - Backing up to a storage pool that runs out of space fails cleanly:
//!   an error, no catalog entry, no partial file, the VM keeps running.
//!
//! Nightly (`--ignored`): needs qemu-system-x86_64 + qemu-img; the
//! disk-full test mounts a small tmpfs (root/CAP_SYS_ADMIN, as in the
//! privileged kvm-x86_64 CI job).

use crate::support::*;
use nix::sys::signal::Signal;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

async fn create_vm(a: &Api, id: &str, disk_gb: u64) -> PathBuf {
    let (s, b) = a
        .post(
            "/vms",
            json!({"id": id, "name": id, "hypervisor": "qemu", "architecture": "x86_64",
                   "cpus": 1, "memory": 128, "disk_size": disk_gb, "status": "stopped",
                   "display": {"protocol": "none"}}),
        )
        .await;
    assert_eq!(s, 201, "create {id}: {b}");
    PathBuf::from(b["disks"][0]["path"].as_str().unwrap())
}

async fn start(a: &Api, id: &str) -> u32 {
    let (s, b) = a.post(&format!("/vms/{id}/start"), json!({})).await;
    assert_eq!(s, 200, "start {id}: {b}");
    let mut pid = None;
    wait_until("QEMU up", 30, || {
        pid = qemu_pid(id);
        pid.is_some()
    })
    .await;
    pid.unwrap()
}

async fn status(a: &Api, id: &str) -> String {
    let (s, b) = a.get(&format!("/vms/{id}")).await;
    assert_eq!(s, 200, "{b}");
    b["status"].as_str().unwrap_or_default().to_string()
}

fn run_state(disk: &Path) -> String {
    qmp(
        &disk.with_extension("qmp.sock"),
        json!({"execute": "query-status"}),
    )["return"]["status"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Fill a VM disk with `mb` MiB of incompressible data (slow to back up).
fn fill_disk(disk: &Path, mb: usize) {
    let raw = disk.with_extension("fill.raw");
    run(
        "dd",
        &[
            "if=/dev/urandom",
            &format!("of={}", raw.display()),
            "bs=1M",
            &format!("count={mb}"),
            "status=none",
        ],
    );
    run(
        "qemu-img",
        &[
            "convert",
            "-f",
            "raw",
            "-O",
            "qcow2",
            &raw.to_string_lossy(),
            &disk.to_string_lossy(),
        ],
    );
    std::fs::remove_file(&raw).unwrap();
}

fn leftovers(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| n.starts_with(".tmp-") || n.ends_with(".partial"))
                .collect()
        })
        .unwrap_or_default()
}

fn backups(v: &Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}

#[tokio::test]
#[ignore = "nightly depth suite: real QEMU"]
async fn running_guests_survive_api_restarts_and_qemu_crashes_are_detected() {
    let dir = scratch("restart");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let a = srv.api().await;
    let id = format!("rs{}", std::process::id());
    let disk = create_vm(&a, &id, 1).await;
    let pid = start(&a, &id).await;
    let (s, snap) = a
        .post(
            &format!("/vms/{id}/snapshots"),
            json!({"name": "before-restart"}),
        )
        .await;
    assert_eq!(s, 200, "live snapshot: {snap}");
    let (s, bk) = a
        .post(
            "/backups",
            json!({"target_id": id, "mode": "suspend", "compression": "gzip"}),
        )
        .await;
    assert_eq!(s, 201, "backup: {bk}");
    assert_eq!(qemu_pid(&id), Some(pid));
    assert_eq!(run_state(&disk), "running");

    for sig in [Signal::SIGTERM, Signal::SIGKILL] {
        srv.restart_after(sig).await;
        let a = srv.api().await;
        assert_eq!(qemu_pid(&id), Some(pid), "{sig:?}: QEMU untouched");
        assert_eq!(run_state(&disk), "running", "{sig:?}: guest still running");
        assert_eq!(status(&a, &id).await, "running", "{sig:?}");
        let (_, snaps) = a.get(&format!("/vms/{id}/snapshots")).await;
        assert!(
            backups(&snaps).iter().any(|s| s["id"] == snap["id"]),
            "{sig:?}: snapshot still listed: {snaps}"
        );
        let (_, list) = a.get(&format!("/backups?target_id={id}")).await;
        assert!(
            backups(&list).iter().any(|b| b["id"] == bk["id"]),
            "{sig:?}: backup still listed: {list}"
        );
        // The console of the running guest is still reachable.
        let (s, c) = a.post(&format!("/console/{id}/serial"), json!({})).await;
        assert!(s == 200 || s == 201, "{sig:?}: serial console: {s} {c}");
        // ... and it can still be managed: stop works after a restart.
    }
    let a = srv.api().await;

    // QEMU crash: reported stopped, and startable again.
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), Signal::SIGKILL).unwrap();
    wait_until("QEMU gone", 10, || qemu_pid(&id).is_none()).await;
    assert_eq!(
        status(&a, &id).await,
        "stopped",
        "crashed VM reported stopped"
    );
    let (_, list) = a.get("/vms").await;
    let row = backups(&list)
        .into_iter()
        .find(|v| v["id"] == id.as_str())
        .unwrap();
    assert_eq!(row["status"], "stopped", "list agrees: {row}");
    let pid2 = start(&a, &id).await;
    assert_ne!(pid2, pid);
    assert_eq!(status(&a, &id).await, "running");

    // Crash again, then a stop request: no error about a stale pidfile.
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid2 as i32), Signal::SIGKILL).unwrap();
    wait_until("QEMU gone", 10, || qemu_pid(&id).is_none()).await;
    let (s, b) = a.post(&format!("/vms/{id}/start"), json!({})).await;
    assert_eq!(s, 200, "start after second crash: {b}");
    let (s, b) = a.post(&format!("/vms/{id}/stop"), json!({})).await;
    assert_eq!(s, 200, "stop: {b}");
    wait_until("QEMU stopped", 30, || qemu_pid(&id).is_none()).await;
    assert_eq!(status(&a, &id).await, "stopped");
    a.delete(&format!("/backups/{}", bk["id"].as_str().unwrap()))
        .await;
    a.delete(&format!("/vms/{id}")).await;
    srv.term();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
#[ignore = "nightly depth suite: real QEMU"]
async fn api_killed_mid_backup_or_live_snapshot_leaves_consistent_state() {
    let dir = scratch("crash");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let a = srv.api().await;
    let id = format!("cr{}", std::process::id());
    let disk = create_vm(&a, &id, 2).await;
    fill_disk(&disk, 1536);
    let pid = start(&a, &id).await;
    let backup_dir = srv.data().join("backups");

    // Kill the API while a suspend-mode backup has the guest paused.
    let a2 = a.clone();
    let vm = id.clone();
    let req = tokio::spawn(async move {
        a2.post(
            "/backups",
            json!({"target_id": vm, "mode": "suspend", "compression": "zstd"}),
        )
        .await
    });
    wait_until("guest paused by the backup", 60, || {
        run_state(&disk) == "paused"
    })
    .await;
    // Let the copy get going so staging data exists.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    srv.kill9();
    let (s, _) = req.await.unwrap();
    assert_eq!(s, 0, "request died with the server");
    assert_eq!(
        run_state(&disk),
        "paused",
        "precondition: guest left paused by the kill"
    );
    assert!(
        !leftovers(&backup_dir).is_empty(),
        "precondition: the kill left staging data in {}",
        backup_dir.display()
    );

    srv.start();
    srv.wait_healthy().await;
    let a = srv.api().await;
    wait_until("guest resumed after API restart", 30, || {
        run_state(&disk) == "running"
    })
    .await;
    assert_eq!(qemu_pid(&id), Some(pid), "same QEMU, resumed not restarted");
    wait_until("staging leftovers swept", 30, || {
        leftovers(&backup_dir).is_empty()
    })
    .await;
    let (_, list) = a.get(&format!("/backups?target_id={id}")).await;
    assert!(
        backups(&list).is_empty(),
        "nothing half-done in the catalog: {list}"
    );
    assert!(
        !disk.with_extension("hold.json").exists(),
        "hold marker cleared"
    );

    // Kill the API in the middle of a stop-mode backup: the VM comes back.
    let a2 = a.clone();
    let vm = id.clone();
    let req = tokio::spawn(async move {
        a2.post(
            "/backups",
            json!({"target_id": vm, "mode": "stop", "compression": "zstd"}),
        )
        .await
    });
    wait_until("VM stopped by the backup", 60, || qemu_pid(&id).is_none()).await;
    tokio::time::sleep(Duration::from_millis(1000)).await;
    srv.kill9();
    let _ = req.await;
    assert!(
        qemu_pid(&id).is_none(),
        "precondition: VM down after the kill"
    );
    srv.start();
    srv.wait_healthy().await;
    let a = srv.api().await;
    wait_until("VM restarted after API restart", 60, || {
        qemu_pid(&id).is_some()
    })
    .await;
    assert_eq!(status(&a, &id).await, "running");
    wait_until("staging leftovers swept", 30, || {
        leftovers(&backup_dir).is_empty()
    })
    .await;

    // The next backup works and verifies.
    let (s, bk) = a
        .post(
            "/backups",
            json!({"target_id": id, "mode": "suspend", "compression": "gzip"}),
        )
        .await;
    assert_eq!(s, 201, "backup after recovery: {bk}");
    let (_, v) = a
        .post(
            &format!("/backups/{}/verify", bk["id"].as_str().unwrap()),
            json!({}),
        )
        .await;
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(run_state(&disk), "running");

    // Kill the API during a live disk-only snapshot: guest not left paused.
    // (The snapshot pause is short, so stop the guest the way the snapshot
    // does and leave the hold marker, then kill: same on-disk state as a
    // kill between QMP stop and cont.)
    let pid = qemu_pid(&id).unwrap();
    std::fs::write(
        disk.with_extension("hold.json"),
        json!({"action": "resume", "reason": "live snapshot", "since": 0}).to_string(),
    )
    .unwrap();
    qmp(&disk.with_extension("qmp.sock"), json!({"execute": "stop"}));
    srv.kill9();
    srv.start();
    srv.wait_healthy().await;
    wait_until("guest resumed", 30, || run_state(&disk) == "running").await;
    assert_eq!(qemu_pid(&id), Some(pid));

    let a = srv.api().await;
    let (s, _) = a.post(&format!("/vms/{id}/stop"), json!({})).await;
    assert_eq!(s, 200);
    let (_, list) = a.get(&format!("/backups?target_id={id}")).await;
    for b in backups(&list) {
        a.delete(&format!("/backups/{}", b["id"].as_str().unwrap()))
            .await;
    }
    a.delete(&format!("/vms/{id}")).await;
    srv.term();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
#[ignore = "nightly depth suite: real QEMU + tmpfs mount"]
async fn backup_to_a_full_storage_pool_fails_cleanly() {
    let dir = scratch("diskfull");
    let pool_dir = dir.join("small-pool");
    std::fs::create_dir_all(&pool_dir).unwrap();
    let mounted = std::process::Command::new("mount")
        .args(["-t", "tmpfs", "-o", "size=48m", "tmpfs"])
        .arg(&pool_dir)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    assert!(mounted, "mounting a 48 MB tmpfs needs root/CAP_SYS_ADMIN");
    struct Unmount(PathBuf);
    impl Drop for Unmount {
        fn drop(&mut self) {
            let _ = std::process::Command::new("umount").arg(&self.0).status();
        }
    }
    let _um = Unmount(pool_dir.clone());

    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let a = srv.api().await;
    let (s, p) = a
        .post(
            "/storage/pools",
            json!({"name": "small", "storage_type": "directory",
                   "path": pool_dir.to_string_lossy()}),
        )
        .await;
    assert!((200..300).contains(&s), "create pool: {s} {p}");

    let id = format!("df{}", std::process::id());
    let disk = create_vm(&a, &id, 1).await;
    fill_disk(&disk, 200);
    let pid = start(&a, &id).await;

    let (s, b) = a
        .post(
            "/backups",
            json!({"target_id": id, "mode": "suspend", "compression": "none", "storage": "small"}),
        )
        .await;
    assert!(
        s >= 400 && s != 404,
        "backup to a full pool must fail: {s} {b}"
    );
    let msg = b.to_string().to_lowercase();
    assert!(
        msg.contains("space") || msg.contains("no space") || msg.contains("enospc"),
        "error says why: {b}"
    );
    let (_, list) = a.get(&format!("/backups?target_id={id}")).await;
    assert!(backups(&list).is_empty(), "no catalog entry: {list}");
    let dump = pool_dir.join("dump");
    assert!(
        leftovers(&dump).is_empty(),
        "no partial files: {:?}",
        leftovers(&dump)
    );
    let used: u64 = std::fs::read_dir(&dump)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| e.metadata().ok())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0);
    assert_eq!(used, 0, "the full pool was cleaned up");
    assert_eq!(qemu_pid(&id), Some(pid), "VM untouched");
    assert_eq!(
        run_state(&disk),
        "running",
        "VM resumed after the failed backup"
    );
    assert!(!disk.with_extension("hold.json").exists());

    // A small backup to the same pool still works once it fits.
    let (s, _) = a.post(&format!("/vms/{id}/stop"), json!({})).await;
    assert_eq!(s, 200);
    wait_until("stopped", 30, || qemu_pid(&id).is_none()).await;
    run(
        "qemu-img",
        &["create", "-q", "-f", "qcow2", &disk.to_string_lossy(), "1G"],
    );
    let (s, b) = a
        .post(
            "/backups",
            json!({"target_id": id, "mode": "stop", "compression": "zstd", "storage": "small"}),
        )
        .await;
    assert_eq!(s, 201, "small backup fits: {b}");
    a.delete(&format!("/backups/{}", b["id"].as_str().unwrap()))
        .await;
    a.delete(&format!("/vms/{id}")).await;
    srv.term();
    drop(_um);
    let _ = std::fs::remove_dir_all(&dir);
}
