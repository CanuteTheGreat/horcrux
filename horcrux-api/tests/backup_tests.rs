//! Backup/restore tests against real tools: qemu-img, tar, zstd/gzip, a
//! real SQLite catalog and (for containers) the real Docker daemon.
//!
//! The VM tests create a small qcow2 disk holding known data, back it up,
//! destroy the original (corrupt/delete), restore it and compare the
//! guest-visible disk contents byte for byte (SHA-256 of a raw dump).

use horcrux_api::backup::{
    BackupConfig, BackupJob, BackupManager, BackupMode, BackupRuntime, Compression, PruneRequest,
    RestoreRequest, RetentionPolicy, TargetType,
};
use horcrux_api::container::ContainerManager;
use horcrux_api::db::Database;
use horcrux_api::vm::VmManager;
use horcrux_common::{VmArchitecture, VmConfig, VmHypervisor, VmStatus};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    db: Arc<Database>,
    vms: Arc<VmManager>,
    containers: Arc<ContainerManager>,
}

impl Env {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let db = Arc::new(
            Database::new(&format!("sqlite://{}/horcrux.db", root.display()))
                .await
                .unwrap(),
        );
        db.migrate().await.unwrap();
        let vms = Arc::new(VmManager::with_database_and_storage_path(
            db.clone(),
            root.join("vms"),
        ));
        Self {
            _dir: dir,
            root,
            db,
            vms,
            containers: Arc::new(ContainerManager::new()),
        }
    }

    /// A BackupManager on this environment's database (call twice to
    /// simulate a server restart).
    async fn manager(&self) -> Arc<BackupManager> {
        let m = Arc::new(
            BackupManager::with_database(
                self.db.clone(),
                self.root.join("backups"),
                self.root.join("restore"),
            )
            .await
            .unwrap(),
        );
        m.attach_runtime(BackupRuntime {
            vm_manager: self.vms.clone(),
            container_manager: self.containers.clone(),
            storage_manager: None,
            vm_storage: self.root.join("vms"),
            lxc_path: self.root.join("lxc"),
            on_job_finished: None,
        })
        .unwrap();
        m
    }

    /// Create a VM whose disk holds 4 MiB of known, non-repeating data.
    async fn vm_with_data(&self, id: &str) -> PathBuf {
        let cfg = VmConfig {
            id: id.to_string(),
            name: format!("{id}-name"),
            hypervisor: VmHypervisor::Qemu,
            memory: 128,
            cpus: 1,
            disk_size: 1,
            status: VmStatus::Stopped,
            architecture: VmArchitecture::X86_64,
            disks: vec![],
        };
        let vm = self.vms.create_vm(cfg).await.unwrap();
        let disk = PathBuf::from(&vm.disks[0].path);
        let raw = self.root.join(format!("{id}.seed.raw"));
        let data: Vec<u8> = (0..4 * 1024 * 1024u32)
            .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8)
            .collect();
        std::fs::write(&raw, data).unwrap();
        sh(
            "qemu-img",
            &["convert", "-f", "raw", "-O", "qcow2"],
            &[&raw, &disk],
        );
        disk
    }
}

fn sh(prog: &str, args: &[&str], paths: &[&Path]) -> String {
    let out = Command::new(prog)
        .args(args)
        .args(paths)
        .output()
        .unwrap_or_else(|e| panic!("{prog} must be installed for backup tests: {e}"));
    assert!(
        out.status.success(),
        "{prog} {args:?} {paths:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// SHA-256 of the guest-visible contents of a disk image.
fn content_sha(disk: &Path, scratch: &Path) -> String {
    let raw = scratch.join("content-check.raw");
    let _ = std::fs::remove_file(&raw);
    sh("qemu-img", &["convert", "-O", "raw"], &[disk, &raw]);
    let s = sh("sha256sum", &[], &[&raw]);
    std::fs::remove_file(&raw).unwrap();
    s.split_whitespace().next().unwrap().to_string()
}

fn backup_req(target: &str, compression: Compression) -> BackupConfig {
    BackupConfig {
        target_id: target.to_string(),
        mode: BackupMode::Snapshot,
        compression,
        ..Default::default()
    }
}

fn have(prog: &str) -> bool {
    Command::new(prog).arg("--version").output().is_ok()
}

#[tokio::test]
async fn vm_backup_corrupt_restore_roundtrip() {
    let env = Env::new().await;
    let mgr = env.manager().await;
    let disk = env.vm_with_data("vm-bk1").await;
    let original = content_sha(&disk, &env.root);

    let b = mgr
        .create_backup(backup_req("vm-bk1", Compression::Zstd))
        .await
        .unwrap();
    assert_eq!(b.target_type, TargetType::Vm);
    assert!(b.path.exists(), "archive written");
    assert!(b.path.to_string_lossy().ends_with(".tar.zst"));
    assert_eq!(b.storage, "local");
    assert_eq!(b.sha256.len(), 64);
    // The archive really holds the disk: listing it shows manifest + disk.
    let listing = sh(
        "sh",
        &["-c", &format!("zstd -dc '{}' | tar -t", b.path.display())],
        &[],
    );
    assert!(listing.contains("horcrux-backup.json") && listing.contains("disk-0.qcow2"));

    // The catalog survives a "restart" (a fresh manager on the same DB).
    let mgr2 = env.manager().await;
    let listed = mgr2.list_backups(Some("vm-bk1".into())).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, b.id);
    assert_eq!(listed[0].sha256, b.sha256);

    let v = mgr2.verify_backup(&b.id).await.unwrap();
    assert!(v.ok, "verify failed: {:?}", v.error);
    assert_eq!(v.files_checked, 1);
    assert_eq!(
        mgr2.get_backup(&b.id)
            .await
            .unwrap()
            .verify_state
            .as_deref(),
        Some("ok")
    );

    // Corrupt the original disk, restore over it.
    std::fs::write(&disk, b"garbage, not a disk image").unwrap();
    let r = mgr2
        .restore_backup(&b.id, RestoreRequest::default())
        .await
        .unwrap();
    assert_eq!(r.target_id, "vm-bk1");
    assert!(r.replaced);
    let restored_disk = PathBuf::from(&env.vms.get_vm("vm-bk1").await.unwrap().disks[0].path);
    assert_eq!(content_sha(&restored_disk, &env.root), original);
    assert_eq!(env.vms.get_vm("vm-bk1").await.unwrap().name, "vm-bk1-name");

    // Delete the VM entirely; restore recreates it.
    env.vms.delete_vm("vm-bk1").await.unwrap();
    assert!(env.vms.get_vm("vm-bk1").await.is_err());
    let r = mgr2
        .restore_backup(&b.id, RestoreRequest::default())
        .await
        .unwrap();
    assert!(!r.replaced);
    let vm = env.vms.get_vm("vm-bk1").await.unwrap();
    assert_eq!(vm.memory, 128);
    assert_eq!(
        content_sha(Path::new(&vm.disks[0].path), &env.root),
        original
    );

    // Restore to a new VM id, next to the original.
    let r = mgr2
        .restore_backup(
            &b.id,
            RestoreRequest {
                target_id: Some("vm-bk1-copy".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(r.target_id, "vm-bk1-copy");
    assert_eq!(r.name, "vm-bk1-name-restored");
    let copy = env.vms.get_vm("vm-bk1-copy").await.unwrap();
    assert_ne!(copy.disks[0].path, vm.disks[0].path);
    assert_eq!(
        content_sha(Path::new(&copy.disks[0].path), &env.root),
        original
    );

    // Restoring over a different existing VM needs force.
    let err = mgr2
        .restore_backup(
            &b.id,
            RestoreRequest {
                target_id: Some("vm-bk1-copy".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("force"), "{err}");
    mgr2.restore_backup(
        &b.id,
        RestoreRequest {
            target_id: Some("vm-bk1-copy".into()),
            force: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // A corrupted archive fails verify and is never restored over the VM.
    let mut bytes = std::fs::read(&b.path).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xff;
    std::fs::write(&b.path, &bytes).unwrap();
    let v = mgr2.verify_backup(&b.id).await.unwrap();
    assert!(!v.ok);
    assert!(v.error.unwrap().contains("checksum mismatch"));
    assert_eq!(
        mgr2.get_backup(&b.id)
            .await
            .unwrap()
            .verify_state
            .as_deref(),
        Some("failed")
    );
    let before = content_sha(Path::new(&vm.disks[0].path), &env.root);
    assert!(mgr2
        .restore_backup(&b.id, RestoreRequest::default())
        .await
        .is_err());
    assert_eq!(content_sha(Path::new(&vm.disks[0].path), &env.root), before);

    // Delete removes catalog entry and archive.
    mgr2.delete_backup(&b.id).await.unwrap();
    assert!(!b.path.exists());
    assert!(mgr2.list_backups(None).await.unwrap().is_empty());
}

#[tokio::test]
async fn every_compression_roundtrips() {
    let env = Env::new().await;
    let mgr = env.manager().await;
    let disk = env.vm_with_data("vm-comp").await;
    let original = content_sha(&disk, &env.root);
    let mut comps = vec![Compression::None, Compression::Gzip, Compression::Zstd];
    if have("lzop") {
        comps.push(Compression::Lzo);
    }
    for c in comps {
        let b = mgr
            .create_backup(backup_req("vm-comp", c.clone()))
            .await
            .unwrap();
        assert!(mgr.verify_backup(&b.id).await.unwrap().ok, "{c:?}");
        std::fs::remove_file(&disk).unwrap();
        mgr.restore_backup(&b.id, RestoreRequest::default())
            .await
            .unwrap();
        let vm = env.vms.get_vm("vm-comp").await.unwrap();
        assert_eq!(
            content_sha(Path::new(&vm.disks[0].path), &env.root),
            original,
            "{c:?}"
        );
    }
}

#[tokio::test]
async fn protected_backups_cannot_be_deleted_or_pruned() {
    let env = Env::new().await;
    let mgr = env.manager().await;
    env.vm_with_data("vm-prot").await;
    let keep = mgr
        .create_backup(BackupConfig {
            protected: true,
            notes: Some("golden".into()),
            ..backup_req("vm-prot", Compression::Zstd)
        })
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let other = mgr
        .create_backup(backup_req("vm-prot", Compression::Zstd))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let newest = mgr
        .create_backup(backup_req("vm-prot", Compression::Zstd))
        .await
        .unwrap();
    assert!(mgr.delete_backup(&keep.id).await.is_err());

    let req = PruneRequest {
        target_id: "vm-prot".into(),
        storage: None,
        policy: RetentionPolicy {
            keep_last: Some(1),
            ..Default::default()
        },
        dry_run: true,
    };
    let preview = mgr.prune(&req).await.unwrap();
    assert_eq!(preview.len(), 3);
    assert_eq!(mgr.list_backups(None).await.unwrap().len(), 3, "dry run");
    mgr.prune(&PruneRequest {
        dry_run: false,
        ..req
    })
    .await
    .unwrap();
    let left: Vec<String> = mgr
        .list_backups(None)
        .await
        .unwrap()
        .into_iter()
        .map(|b| b.id)
        .collect();
    assert!(left.contains(&keep.id) && left.contains(&newest.id));
    assert!(!left.contains(&other.id));
    assert!(!other.path.exists());
}

fn job(id: &str, targets: &[&str], keep_last: u32) -> BackupJob {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "schedule": "* * * * *",
        "targets": targets,
        "mode": "stop",
        "compression": "zstd",
        "retention": {"keep_last": keep_last},
    }))
    .unwrap()
}

#[tokio::test]
async fn jobs_persist_run_and_apply_retention() {
    let env = Env::new().await;
    let mgr = env.manager().await;
    env.vm_with_data("vm-job").await;
    let created = mgr
        .create_job(job("nightly", &["vm-job"], 2))
        .await
        .unwrap();
    assert!(created.enabled);
    assert!(created.next_run.unwrap() > chrono::Utc::now().timestamp());
    assert!(mgr
        .create_job(job("nightly", &["vm-job"], 2))
        .await
        .is_err());

    for _ in 0..3 {
        let r = mgr.run_job("nightly").await.unwrap();
        assert!(r.ok, "{:?}", r.results);
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    }
    let backups = mgr.list_backups(Some("vm-job".into())).await.unwrap();
    assert_eq!(backups.len(), 2, "keep_last=2 pruned the oldest");
    assert!(backups
        .iter()
        .all(|b| b.job_id.as_deref() == Some("nightly")));

    // Job and its run history survive a restart.
    let mgr2 = env.manager().await;
    let j = mgr2.get_job("nightly").await.unwrap();
    assert_eq!(j.last_status.as_deref(), Some("ok"));
    assert!(j.last_run.is_some());

    // A failing target is reported on the job.
    let mut bad = job("broken", &["no-such-guest"], 0);
    bad.enabled = false;
    mgr2.create_job(bad).await.unwrap();
    let r = mgr2.run_job("broken").await.unwrap();
    assert!(!r.ok);
    let j = mgr2.get_job("broken").await.unwrap();
    assert_eq!(j.last_status.as_deref(), Some("failed"));
    assert!(j.last_error.unwrap().contains("no-such-guest"));
    assert!(j.next_run.is_none(), "disabled jobs are not scheduled");

    mgr2.delete_job("nightly").await.unwrap();
    assert!(mgr2.get_job("nightly").await.is_err());
    assert_eq!(
        mgr2.list_backups(None).await.unwrap().len(),
        2,
        "deleting a job keeps its backups"
    );
}

#[tokio::test]
async fn scheduler_fires_due_jobs() {
    let env = Env::new().await;
    let mgr = env.manager().await;
    env.vm_with_data("vm-sched").await;
    let j = mgr
        .create_job(job("every-minute", &["vm-sched"], 0))
        .await
        .unwrap();
    let next = j.next_run.unwrap();

    // Not due yet: nothing starts.
    assert!(mgr.run_due_jobs(chrono::Local::now()).await.is_empty());
    // At the scheduled minute the job starts and is rescheduled.
    let at = chrono::DateTime::from_timestamp(next, 0)
        .unwrap()
        .with_timezone(&chrono::Local);
    assert_eq!(mgr.run_due_jobs(at).await, vec!["every-minute".to_string()]);
    assert_eq!(
        mgr.get_job("every-minute").await.unwrap().next_run,
        Some(next + 60)
    );
    // The background run produces a backup.
    for _ in 0..120 {
        let b = mgr.list_backups(Some("vm-sched".into())).await.unwrap();
        if !b.is_empty() {
            assert_eq!(b[0].job_id.as_deref(), Some("every-minute"));
            let j = mgr.get_job("every-minute").await.unwrap();
            if j.last_status.is_some() {
                assert_eq!(j.last_status.as_deref(), Some("ok"));
                return;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    panic!("scheduled job never produced a backup");
}

#[tokio::test]
async fn storage_targets() {
    let env = Env::new().await;
    let mgr = env.manager().await;
    env.vm_with_data("vm-store").await;
    let dir = env.root.join("other-storage");
    let b = mgr
        .create_backup(BackupConfig {
            storage: dir.to_string_lossy().into(),
            ..backup_req("vm-store", Compression::Gzip)
        })
        .await
        .unwrap();
    assert!(b.path.starts_with(&dir));
    assert_eq!(b.storage, dir.to_string_lossy());
    for bad in ["nope-pool", "/tmp/../etc"] {
        assert!(mgr
            .create_backup(BackupConfig {
                storage: bad.into(),
                ..backup_req("vm-store", Compression::Zstd)
            })
            .await
            .is_err());
    }
    assert!(mgr
        .create_backup(backup_req("../etc", Compression::Zstd))
        .await
        .is_err());
}

fn docker_ok() -> bool {
    Command::new("docker")
        .args(["info", "--format", "{{.ServerVersion}}"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn docker(args: &[&str]) -> String {
    let out = Command::new("docker").args(args).output().unwrap();
    assert!(
        out.status.success(),
        "docker {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[tokio::test]
async fn docker_container_backup_delete_restore() {
    if !docker_ok() {
        assert!(
            std::env::var("HORCRUX_TEST_REQUIRE_DOCKER").is_err(),
            "HORCRUX_TEST_REQUIRE_DOCKER is set but Docker is unreachable"
        );
        eprintln!("skipping: no Docker daemon");
        return;
    }
    let env = Env::new().await;
    let mgr = env.manager().await;
    let id = format!("hx-bk-{}", std::process::id());
    let _ = Command::new("docker").args(["rm", "-f", &id]).output();
    let cfg: horcrux_common::ContainerConfig = serde_json::from_value(serde_json::json!({
        "id": id, "name": id, "runtime": "docker", "memory": 64, "cpus": 1,
        "rootfs": "busybox:latest", "status": "stopped",
        "command": ["sleep", "3600"], "env": ["GREETING=hello world"]
    }))
    .unwrap();
    env.containers.create_container(cfg).await.unwrap();
    env.containers.start_container(&id).await.unwrap();
    docker(&["exec", &id, "sh", "-c", "echo precious-data > /data.txt"]);

    // Running container, suspend mode: paused during export, running after.
    let b = mgr
        .create_backup(BackupConfig {
            target_id: id.clone(),
            mode: BackupMode::Suspend,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(b.target_type, TargetType::Container);
    assert_eq!(
        docker(&["inspect", "-f", "{{.State.Running}}", &id]),
        "true"
    );
    assert!(mgr.verify_backup(&b.id).await.unwrap().ok);

    // Destroy the container, restore it from the backup.
    env.containers.stop_container(&id).await.unwrap();
    env.containers.delete_container(&id).await.unwrap();
    let r = mgr
        .restore_backup(
            &b.id,
            RestoreRequest {
                start: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(r.target_id, id);
    assert_eq!(docker(&["exec", &id, "cat", "/data.txt"]), "precious-data");
    assert_eq!(
        docker(&["exec", &id, "sh", "-c", "echo $GREETING"]),
        "hello world"
    );

    let image = docker(&["inspect", "-f", "{{.Config.Image}}", &id]);
    let _ = Command::new("docker").args(["rm", "-f", &id]).output();
    let _ = Command::new("docker").args(["rmi", &image]).output();
}
