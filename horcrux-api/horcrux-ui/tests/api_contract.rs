//! UI <-> API contract test.
//!
//! The web UI (`horcrux-ui/src/api.rs`) has its own copies of every response
//! type. Those copies drifted from the API more than once and the UI then
//! showed raw serde errors instead of data (daily QA: containers page
//! "missing field `image`", dashboard "missing field `cpu_usage`"; the
//! snapshot, clone and replication pages had the same problem, unnoticed).
//!
//! This test talks to a REAL running horcrux-api (CI's Test job starts one
//! on :8006 before `cargo test --workspace`), seeds real objects through the
//! API (a Docker container, a VM, a VM snapshot, a replication job, a
//! compose stack), then fetches every list endpoint the UI reads and
//! deserializes the real response bodies into the UI's own types.
//!
//! Override the server with HORCRUX_CONTRACT_API (default
//! https://127.0.0.1:8006/api - the API serves HTTPS with a self-signed
//! certificate by default).

use horcrux_ui::api;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::time::Duration;

fn api_base() -> String {
    std::env::var("HORCRUX_CONTRACT_API").unwrap_or_else(|_| "https://127.0.0.1:8006/api".into())
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap()
}

async fn login(c: &reqwest::Client) -> String {
    for _ in 0..60 {
        let r = c
            .post(format!("{}/auth/login", api_base()))
            .json(&json!({"username": "admin", "password": "admin"}))
            .send()
            .await
            .expect("horcrux-api must be running for the UI contract test");
        if r.status() == 429 {
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        }
        assert!(r.status().is_success(), "login failed: {}", r.status());
        let v: Value = r.json().await.unwrap();
        return v["ticket"]
            .as_str()
            .or_else(|| v["token"].as_str())
            .expect("login response has a token")
            .to_string();
    }
    panic!("login kept being rate limited");
}

async fn get_raw(c: &reqwest::Client, token: &str, path: &str) -> (u16, String) {
    let r = c
        .get(format!("{}{}", api_base(), path))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    (r.status().as_u16(), r.text().await.unwrap_or_default())
}

/// Fetch `path` and deserialize into the UI type `T`.
async fn fetch<T: DeserializeOwned>(c: &reqwest::Client, token: &str, path: &str) -> T {
    let (status, body) = get_raw(c, token, path).await;
    assert_eq!(status, 200, "GET {path} -> {status}: {body}");
    serde_json::from_str(&body).unwrap_or_else(|e| {
        panic!(
            "UI type {} cannot read GET {path}: {e}\nbody: {}",
            std::any::type_name::<T>(),
            &body[..body.len().min(800)]
        )
    })
}

async fn post(c: &reqwest::Client, token: &str, path: &str, body: Value) -> (u16, Value) {
    let r = c
        .post(format!("{}{}", api_base(), path))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = r.status().as_u16();
    let text = r.text().await.unwrap_or_default();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

async fn delete(c: &reqwest::Client, token: &str, path: &str) {
    let _ = c
        .delete(format!("{}{}", api_base(), path))
        .bearer_auth(token)
        .send()
        .await;
}

fn docker_available() -> bool {
    std::process::Command::new("docker")
        .arg("info")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[tokio::test]
async fn ui_types_read_real_api_responses() {
    let c = client();
    let token = login(&c).await;
    let mut failures: Vec<String> = Vec::new();

    // ---- dashboard: node metrics (collector samples periodically) --------
    let mut node = None;
    for _ in 0..40 {
        node = fetch::<Option<api::NodeMetrics>>(&c, &token, "/monitoring/node").await;
        if node.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let node = node.expect("node metrics never became available");
    assert!(node.memory_total > 0, "node metrics: {node:?}");

    // ---- containers: create through the same body the UI form sends ----
    let ct = "contract-ct";
    delete(&c, &token, &format!("/containers/{ct}")).await;
    let _ = std::process::Command::new("docker")
        .args(["rm", "-f", ct])
        .output();
    if std::env::var("HORCRUX_TEST_REQUIRE_DOCKER").is_ok() || docker_available() {
        let req = api::CreateContainerRequest {
            id: ct.into(),
            name: ct.into(),
            runtime: "docker".into(),
            memory: 64,
            cpus: 1,
            rootfs: "busybox:latest".into(),
            status: "stopped".into(),
            env: vec!["CONTRACT=1".into()],
            command: vec!["sleep".into(), "600".into()],
            ..Default::default()
        };
        let (status, body) = post(
            &c,
            &token,
            "/containers",
            serde_json::to_value(&req).unwrap(),
        )
        .await;
        assert_eq!(status, 200, "create container: {body}");
        let created: api::Container =
            serde_json::from_value(body).expect("UI Container from create");
        assert_eq!(created.image, "busybox:latest");
        let list: Vec<api::Container> = fetch(&c, &token, "/containers").await;
        let mine = list
            .iter()
            .find(|x| x.id == ct)
            .expect("created container listed");
        assert!(
            mine.created_at.is_some(),
            "Created column must have a value: {mine:?}"
        );
    } else {
        failures.push("docker not reachable: container contract not exercised".into());
    }

    // ---- VM + snapshot + clone + replication ----------------------------
    let vm = "contract-vm";
    delete(&c, &token, &format!("/vms/{vm}")).await;
    let (status, body) = post(
        &c,
        &token,
        "/vms",
        json!({"id": vm, "name": "contract-vm", "hypervisor": "qemu", "architecture": "x86_64",
               "cpus": 1, "memory": 256, "disk_size": 1, "status": "stopped"}),
    )
    .await;
    assert!(status == 200 || status == 201, "create vm: {status} {body}");
    // VM create form "Boot disk from image": POST /vms/:id/import-disk.
    // The server's images dir is <data dir>/images (CI: /tmp/horcrux-ci-data).
    let images = std::path::PathBuf::from(
        std::env::var("HORCRUX_CONTRACT_DATA_DIR")
            .unwrap_or_else(|_| "/tmp/horcrux-ci-data".into()),
    )
    .join("images");
    let (status, body) = post(
        &c,
        &token,
        &format!("/vms/{vm}/import-disk"),
        json!({"source": "/etc/passwd"}),
    )
    .await;
    assert_eq!(
        status, 403,
        "import from outside the images dir refused: {body}"
    );
    if std::fs::create_dir_all(&images).is_ok() {
        let img = images.join("contract.qcow2");
        let made = std::process::Command::new("qemu-img")
            .args(["create", "-q", "-f", "qcow2"])
            .arg(&img)
            .arg("64M")
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(made, "qemu-img create {}", img.display());
        let (status, body) = post(
            &c,
            &token,
            &format!("/vms/{vm}/import-disk"),
            json!({"source": img.to_string_lossy()}),
        )
        .await;
        assert_eq!(status, 200, "import disk: {body}");
        let imported: horcrux_common::VmConfig =
            serde_json::from_value(body).expect("UI VmConfig from import-disk");
        assert_eq!(imported.id, vm);
        let _ = std::fs::remove_file(&img);
    }
    let (status, body) = post(
        &c,
        &token,
        &format!("/vms/{vm}/snapshots"),
        json!({"name": "contract-snap", "description": "contract", "include_memory": false}),
    )
    .await;
    if status == 200 {
        let snap: api::VmSnapshot =
            serde_json::from_value(body).expect("UI VmSnapshot from create");
        let snaps: Vec<api::VmSnapshot> = fetch(&c, &token, &format!("/vms/{vm}/snapshots")).await;
        assert!(snaps.iter().any(|s| s.id == snap.id), "snapshot listed");
    } else {
        failures.push(format!("VM snapshot create returned {status}: {body}"));
    }
    let (status, body) = post(
        &c,
        &token,
        "/replication/jobs",
        json!({"source_vm_id": vm, "source_snapshot": "contract-snap", "target_node": "node-b",
               "target_pool": "tank", "schedule": "daily"}),
    )
    .await;
    assert_eq!(status, 200, "create replication job: {body}");
    let rep_id = body["id"].as_str().unwrap_or_default().to_string();
    let reps: Vec<api::ReplicationJob> = fetch(&c, &token, "/replication/jobs").await;
    let rep = reps
        .iter()
        .find(|r| r.id == rep_id)
        .expect("replication job listed");
    assert_eq!(rep.vm_id, vm);
    assert!(rep.schedule.starts_with("daily"), "{rep:?}");

    // ---- backups: back up, verify, prune-preview, restore, job ------------
    use api::backup as bk;
    let (status, body) = post(
        &c,
        &token,
        "/backups",
        serde_json::to_value(bk::CreateBackupRequest {
            target_id: vm.into(),
            target_type: Some("vm".into()),
            storage: "local".into(),
            mode: "snapshot".into(),
            compression: "zstd".into(),
            notes: Some("contract".into()),
            protected: false,
        })
        .unwrap(),
    )
    .await;
    assert_eq!(status, 201, "create backup: {body}");
    let backup: bk::Backup = serde_json::from_value(body).expect("UI Backup from create");
    assert_eq!(backup.target_id, vm);
    assert!(backup.size > 0 && backup.sha256.len() == 64, "{backup:?}");
    let listed: Vec<bk::Backup> = fetch(&c, &token, "/backups").await;
    assert!(listed.iter().any(|b| b.id == backup.id), "backup listed");
    let (status, body) = post(
        &c,
        &token,
        &format!("/backups/{}/verify", backup.id),
        json!({}),
    )
    .await;
    assert_eq!(status, 200, "verify: {body}");
    let v: bk::VerifyResult = serde_json::from_value(body).expect("UI VerifyResult");
    assert!(v.ok, "{v:?}");
    let (status, body) = post(
        &c,
        &token,
        "/backups/prune",
        serde_json::to_value(bk::PruneRequest {
            target_id: vm.into(),
            storage: None,
            policy: bk::Retention {
                keep_last: Some(5),
                ..Default::default()
            },
            dry_run: true,
        })
        .unwrap(),
    )
    .await;
    assert_eq!(status, 200, "prune preview: {body}");
    let d: Vec<bk::PruneDecision> = serde_json::from_value(body).expect("UI PruneDecision");
    assert!(
        d.iter()
            .any(|x| x.backup_id == backup.id && x.mark == "keep"),
        "{d:?}"
    );
    let restored_vm = "contract-vm-restored";
    delete(&c, &token, &format!("/vms/{restored_vm}")).await;
    let (status, body) = post(
        &c,
        &token,
        &format!("/backups/{}/restore", backup.id),
        serde_json::to_value(bk::RestoreRequest {
            target_id: Some(restored_vm.into()),
            ..Default::default()
        })
        .unwrap(),
    )
    .await;
    assert_eq!(status, 200, "restore: {body}");
    let r: bk::RestoreResult = serde_json::from_value(body).expect("UI RestoreResult");
    assert_eq!(r.target_id, restored_vm);
    let job = bk::BackupJob {
        id: "contract-job".into(),
        enabled: false,
        schedule: "0 3 * * *".into(),
        targets: vec![vm.into()],
        mode: "stop".into(),
        compression: "gzip".into(),
        retention: bk::Retention {
            keep_daily: Some(7),
            ..Default::default()
        },
        ..Default::default()
    };
    delete(&c, &token, "/backup-jobs/contract-job").await;
    let (status, body) = post(
        &c,
        &token,
        "/backup-jobs",
        serde_json::to_value(&job).unwrap(),
    )
    .await;
    assert_eq!(status, 201, "create backup job: {body}");
    let saved: bk::BackupJob = serde_json::from_value(body).expect("UI BackupJob from create");
    assert_eq!(saved.retention.keep_daily, Some(7));
    let jobs: Vec<bk::BackupJob> = fetch(&c, &token, "/backup-jobs").await;
    assert!(jobs.iter().any(|j| j.id == "contract-job"), "job listed");
    let storages: Vec<bk::BackupStorage> = fetch(&c, &token, "/backup-storages").await;
    assert!(storages.iter().any(|s| s.id == "local"), "{storages:?}");
    let one: bk::Backup = fetch(&c, &token, &format!("/backups/{}", backup.id)).await;
    assert_eq!(one.verify_state.as_deref(), Some("ok"), "{one:?}");
    let r = c
        .put(format!("{}/backups/{}", api_base(), backup.id))
        .bearer_auth(&token)
        .json(&bk::UpdateBackupRequest {
            notes: Some("contract notes".into()),
            protected: None,
        })
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "update backup");
    let upd: bk::Backup = r.json().await.expect("UI Backup from update");
    assert_eq!(upd.notes.as_deref(), Some("contract notes"));
    let one_job: bk::BackupJob = fetch(&c, &token, "/backup-jobs/contract-job").await;
    let r = c
        .put(format!("{}/backup-jobs/contract-job", api_base()))
        .bearer_auth(&token)
        .json(&bk::BackupJob {
            comment: Some("edited".into()),
            ..one_job
        })
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "update backup job");
    let edited: bk::BackupJob = r.json().await.expect("UI BackupJob from update");
    assert_eq!(edited.comment.as_deref(), Some("edited"));
    let (status, body) = post(&c, &token, "/backup-jobs/contract-job/run", json!({})).await;
    assert_eq!(status, 202, "run job: {body}");
    let _: bk::RunStarted = serde_json::from_value(body).expect("UI RunStarted");
    let mut job_done = None;
    for _ in 0..120 {
        let j: bk::BackupJob = fetch(&c, &token, "/backup-jobs/contract-job").await;
        if j.last_status.is_some() {
            job_done = Some(j);
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let job_done = job_done.expect("backup job run finished");
    assert_eq!(job_done.last_status.as_deref(), Some("ok"), "{job_done:?}");
    let (status, body) = post(&c, &token, &format!("/backups/retention/{vm}"), json!({})).await;
    assert_eq!(status, 200, "retention: {body}");
    let kept: Vec<bk::PruneDecision> = serde_json::from_value(body).expect("UI PruneDecision");
    assert!(
        kept.iter().all(|d| d.mark == "keep"),
        "empty policy keeps all: {kept:?}"
    );

    // ---- VM display settings + consoles (Create VM form / console page) --
    let display = horcrux_common::VmDisplay {
        protocol: horcrux_common::DisplayProtocol::Spice,
        video: horcrux_common::VideoDevice::Qxl,
        sound: horcrux_common::SoundDevice::Ich9IntelHda,
    };
    let r = c
        .put(format!("{}/vms/{vm}/display", api_base()))
        .bearer_auth(&token)
        .json(&display)
        .send()
        .await
        .unwrap();
    let (status, body) = (r.status().as_u16(), r.text().await.unwrap_or_default());
    assert_eq!(status, 200, "PUT /vms/{vm}/display: {body}");
    let cfg: horcrux_common::VmConfig =
        serde_json::from_str(&body).expect("UI reads the PUT display response");
    assert_eq!(cfg.display, display);
    let cfg: horcrux_common::VmConfig = fetch(&c, &token, &format!("/vms/{vm}")).await;
    assert_eq!(cfg.display, display, "display setting persisted");
    let (status, body) = post(&c, &token, &format!("/vms/{vm}/start"), json!({})).await;
    assert!((200..300).contains(&status), "start {vm}: {status} {body}");
    let (status, body) = post(&c, &token, &format!("/console/{vm}/spice"), json!({})).await;
    assert_eq!(status, 200, "SPICE console: {body}");
    let info: api::ConsoleInfo = serde_json::from_value(body).expect("UI ConsoleInfo (SPICE)");
    assert!(info.port > 0 && info.password.is_some(), "{info:?}");
    let (status, body) = post(&c, &token, &format!("/console/{vm}/serial"), json!({})).await;
    assert_eq!(status, 200, "serial console: {body}");
    let info: api::ConsoleInfo = serde_json::from_value(body).expect("UI ConsoleInfo (serial)");
    assert!(info.ws_path.contains(&info.ticket), "{info:?}");
    let (status, body) = get_raw(&c, &token, &format!("/console/{vm}/spice.vv")).await;
    assert_eq!(status, 200, "spice.vv: {body}");
    let _ = post(&c, &token, &format!("/vms/{vm}/stop"), json!({})).await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    // ---- every list endpoint the UI reads --------------------------------
    macro_rules! check {
        ($path:expr, $t:ty) => {{
            let (status, body) = get_raw(&c, &token, $path).await;
            if status != 200 {
                failures.push(format!(
                    "GET {} -> {} (UI page has no working backend)",
                    $path, status
                ));
            } else if let Err(e) = serde_json::from_str::<$t>(&body) {
                failures.push(format!(
                    "GET {} : UI type {} can't read it: {e}",
                    $path,
                    stringify!($t)
                ));
            }
        }};
    }
    check!("/vms", Vec<horcrux_common::VmConfig>);
    check!("/vms", Vec<api::VirtualMachine>);
    check!("/containers", Vec<api::Container>);
    check!("/clone-jobs", Vec<api::CloneJob>);
    check!("/replication/jobs", Vec<api::ReplicationJob>);
    check!("/alerts/active", Vec<api::ActiveAlert>);
    check!("/alerts/rules", Vec<api::AlertRule>);
    check!("/alerts/history", Vec<api::ActiveAlert>);
    check!("/cluster/nodes", Vec<api::ClusterNode>);
    check!("/cluster/architecture", api::ClusterArchitecture);
    check!("/users", Vec<api::User>);
    check!("/roles", Vec<api::Role>);
    check!("/auth/sessions", Vec<api::UserSession>);
    check!("/backups", Vec<bk::Backup>);
    check!("/backup-jobs", Vec<bk::BackupJob>);
    check!("/backup-storages", Vec<bk::BackupStorage>);
    // access control (users/roles/groups/ACL/realms/2FA) and certificates
    check!("/auth/realms", api::LoginRealms);
    check!("/auth/me", api::Me);
    check!("/auth/mfa/status", api::MfaStatus);
    check!("/auth/mfa/policy", api::MfaEnforcementPolicy);
    check!("/auth/mfa/users", Vec<api::UserMfaStatus>);
    check!("/privileges", Vec<String>);
    check!("/groups", Vec<api::Group>);
    check!("/acl", Vec<api::AclEntry>);
    check!("/access/realms", Vec<api::Realm>);
    check!("/tls/status", api::TlsStatus);
    check!("/tls/certificates", Vec<api::CertificateInfo>);
    check!("/snapshot-schedules", Vec<api::SnapshotSchedule>);
    check!("/snapshot-quotas", Vec<api::SnapshotQuota>);
    check!("/snapshot-quotas/summary", api::QuotaSummary);
    check!("/templates", Vec<api::VmTemplate>);
    check!("/ha/resources", Vec<api::HaResource>);
    check!("/ha/status", api::HaStatus);
    check!("/ha/groups", Vec<api::HaGroup>);
    check!("/storage/disks", Vec<api::DiskInfo>);
    check!("/nodes/guest-power", api::PowerOverview);
    check!("/storage/smart/disks", Vec<api::SmartDiskInfo>);
    check!("/storage/smart/alerts", Vec<api::SmartAlert>);
    check!("/system/config", api::SystemConfiguration);
    check!("/system/network/dns", api::DnsConfiguration);
    check!("/system/time/ntp", api::NtpConfiguration);
    check!("/system/health", api::SystemHealth);
    check!("/system/diagnostics", api::SystemDiagnostics);
    check!("/tasks", Vec<api::TaskRecord>);
    check!("/audit/search", api::AuditPage);
    check!("/audit/filter-options", api::AuditFilterOptions);
    check!("/audit/security-events", Vec<api::AuditEvent>);
    check!("/sdn/zones", Vec<api::sdn_zones::Zone>);
    check!("/sdn/vnets", Vec<api::sdn_zones::VNet>);
    check!("/sdn/subnets", Vec<api::sdn_zones::Subnet>);
    check!("/sdn/status", Vec<api::sdn_zones::VNetStatus>);
    check!("/ha/events", Vec<api::HaEventView>);
    check!("/ha/fencing/devices", Vec<api::FencingDeviceView>);
    check!("/cluster/status", api::ClusterStatusView);
    check!("/webhooks", Vec<api::WebhookConfig>);
    check!("/storage/pools", Vec<api::StoragePoolInfo>);
    check!("/stacks", Vec<api::StackSummary>);
    check!("/volumes?runtime=docker", Vec<api::ContainerVolumeInfo>);
    check!(
        "/container-networks?runtime=docker",
        Vec<api::ContainerNetworkInfo>
    );

    // ---- storage: a directory pool reports real capacity and takes volumes
    let pool_dir = std::env::temp_dir().join("horcrux-contract-pool");
    let _ = std::fs::remove_dir_all(&pool_dir);
    std::fs::create_dir_all(&pool_dir).unwrap();
    let (status, body) = post(
        &c,
        &token,
        "/storage/pools",
        json!({"name": "contract-pool", "storage_type": "directory", "path": pool_dir.to_string_lossy()}),
    )
    .await;
    assert_eq!(status, 200, "create pool: {body}");
    let pool_id = body["id"].as_str().unwrap().to_string();
    let pools: Vec<api::StoragePoolInfo> = fetch(&c, &token, "/storage/pools").await;
    let p = pools.iter().find(|p| p.id == pool_id).expect("pool listed");
    assert_eq!(p.pool_type, "directory");
    assert!(
        p.available_bytes > 0 && p.total_bytes >= p.available_bytes,
        "{p:?}"
    );
    let (status, body) = post(
        &c,
        &token,
        &format!("/storage/pools/{pool_id}/volumes"),
        json!({"name": "contract-vol", "size": 1}),
    )
    .await;
    assert!(
        status == 200 || status == 201,
        "create volume: {status} {body}"
    );
    assert!(
        pool_dir.join("contract-vol.qcow2").is_file(),
        "volume file in pool dir"
    );
    // The Storage page's volume list, grow and delete (api::*_pool_volume
    // paths and the PoolVolume type against the real routes).
    let vols: Vec<api::PoolVolume> =
        fetch(&c, &token, &format!("/storage/pools/{pool_id}/volumes")).await;
    let v = vols
        .iter()
        .find(|v| v.name == "contract-vol")
        .expect("volume listed");
    assert_eq!(v.size_bytes, 1 << 30);
    assert_eq!(v.format, "qcow2");
    let (status, body) = post(
        &c,
        &token,
        &format!("/storage/pools/{pool_id}/volumes/contract-vol/resize"),
        json!({"size": 2}),
    )
    .await;
    assert_eq!(status, 200, "grow volume: {body}");
    let grown: api::PoolVolume = serde_json::from_value(body).unwrap();
    assert_eq!(grown.size_bytes, 2 << 30);
    let (status, body) = post(
        &c,
        &token,
        &format!("/storage/pools/{pool_id}/volumes"),
        json!({"name": "../../tmp/contract-escape", "size": 1}),
    )
    .await;
    assert_eq!(status, 400, "traversal refused: {body}");
    // Create VM / Add disk offer the pool for VM disks.
    let opts: api::VmHostOptions = fetch(&c, &token, "/vms/host-options").await;
    assert!(
        opts.storage_pools.iter().any(|p| p.id == pool_id),
        "{:?}",
        opts.storage_pools
    );
    delete(
        &c,
        &token,
        &format!("/storage/pools/{pool_id}/volumes/contract-vol"),
    )
    .await;
    assert!(!pool_dir.join("contract-vol.qcow2").exists());
    // The pool is a backup storage too: archives land in <pool>/dump.
    let storages: Vec<bk::BackupStorage> = fetch(&c, &token, "/backup-storages").await;
    assert!(storages.iter().any(|s| s.id == pool_id), "{storages:?}");
    let (status, body) = post(
        &c,
        &token,
        "/backups",
        json!({"target_id": vm, "storage": pool_id, "compression": "gzip"}),
    )
    .await;
    assert_eq!(status, 201, "backup to pool: {body}");
    let pb: bk::Backup = serde_json::from_value(body).unwrap();
    assert!(
        pb.path
            .starts_with(&*pool_dir.join("dump").to_string_lossy()),
        "{pb:?}"
    );
    delete(&c, &token, &format!("/backups/{}", pb.id)).await;
    delete(&c, &token, &format!("/storage/pools/{pool_id}")).await;
    let _ = std::fs::remove_dir_all(&pool_dir);

    // Lists that the checks above would pass even when empty: assert the
    // records the test itself created are really there in the UI's shape.
    let sessions: Vec<api::UserSession> = fetch(&c, &token, "/auth/sessions").await;
    assert!(
        sessions.iter().any(|s| s.username == "admin"),
        "login session listed: {sessions:?}"
    );
    let vms: Vec<api::VirtualMachine> = fetch(&c, &token, "/vms").await;
    let v = vms
        .iter()
        .find(|v| v.id == vm)
        .expect("contract VM listed for HA pages");
    assert_eq!(v.vmid, vm);
    assert!(!v.state.is_empty(), "{v:?}");

    // ---- access control: the bodies the UI pages send --------------------
    delete(&c, &token, "/groups/contract-group").await;
    delete(&c, &token, "/users/contract-user").await;
    delete(&c, &token, "/access/realms/contract-ldap").await;
    let (status, body) = post(
        &c,
        &token,
        "/users",
        serde_json::to_value(api::CreateUserRequest {
            username: "contract-user".into(),
            password: "contract-pass-1".into(),
            email: "contract@errant.solutions".into(),
            role: "user".into(),
            realm: "local".into(),
            enabled: true,
            comment: Some("api contract".into()),
        })
        .unwrap(),
    )
    .await;
    assert_eq!(status, 201, "create user: {body}");
    let created: api::User =
        serde_json::from_value(body).expect("UI User type reads create response");
    let (status, body) = post(
        &c,
        &token,
        "/groups",
        json!({"name": "contract-group", "members": ["contract-user"]}),
    )
    .await;
    assert_eq!(status, 201, "create group: {body}");
    let (status, body) = post(
        &c,
        &token,
        "/acl",
        json!({"path": "/vms", "subject_type": "group", "subject": "contract-group", "role": "VMUser", "propagate": true}),
    )
    .await;
    assert_eq!(status, 201, "grant: {body}");
    let acl: Vec<api::AclEntry> = fetch(&c, &token, "/acl").await;
    assert!(
        acl.iter()
            .any(|e| e.subject == "contract-group" && e.role == "VMUser"),
        "{acl:?}"
    );
    let groups: Vec<api::Group> = fetch(&c, &token, "/groups").await;
    assert!(groups
        .iter()
        .any(|g| g.name == "contract-group" && g.members == vec!["contract-user".to_string()]));
    let users: Vec<api::User> = fetch(&c, &token, "/users").await;
    let u = users
        .iter()
        .find(|u| u.id == created.id)
        .expect("created user listed (DB-backed)");
    assert_eq!(u.groups, vec!["contract-group".to_string()]);
    let (status, body) = post(
        &c,
        &token,
        &format!("/users/{}/toggle", created.id),
        json!({"enabled": false}),
    )
    .await;
    assert_eq!(status, 200, "toggle: {body}");
    // The Users page's enable switch (POST /users/:id/toggle) really
    // disables the account: listed as disabled and its login is refused.
    let users: Vec<api::User> = fetch(&c, &token, "/users").await;
    assert!(
        !users.iter().find(|u| u.id == created.id).unwrap().enabled,
        "toggle {{enabled:false}} disabled the user"
    );
    let r = c
        .post(format!("{}/auth/login", api_base()))
        .json(&json!({"username": "contract-user", "password": "contract-pass-1"}))
        .send()
        .await
        .unwrap();
    assert!(
        r.status().is_client_error(),
        "a disabled user can't log in: {}",
        r.status()
    );
    let (status, body) = post(
        &c,
        &token,
        &format!("/users/{}/toggle", created.id),
        json!({"enabled": true}),
    )
    .await;
    assert_eq!(status, 200, "re-enable: {body}");
    let users: Vec<api::User> = fetch(&c, &token, "/users").await;
    assert!(users.iter().find(|u| u.id == created.id).unwrap().enabled);
    let (status, body) = post(
        &c,
        &token,
        "/access/realms",
        serde_json::to_value(api::Realm {
            name: "contract-ldap".into(),
            realm_type: "ldap".into(),
            enabled: false,
            comment: None,
            config: json!({"server": "ldap.invalid", "base_dn": "dc=x", "bind_password": "s3cret"}),
            default_group: Some("contract-group".into()),
            tfa_required: false,
        })
        .unwrap(),
    )
    .await;
    assert_eq!(status, 201, "create realm: {body}");
    let realms: Vec<api::Realm> = fetch(&c, &token, "/access/realms").await;
    let r = realms
        .iter()
        .find(|r| r.name == "contract-ldap")
        .expect("realm listed");
    assert_eq!(
        r.config["bind_password"], "********",
        "secrets are never returned"
    );
    let (status, body) = post(&c, &token, "/auth/mfa/totp/setup", json!({})).await;
    assert_eq!(status, 200, "totp setup: {body}");
    let setup: api::TotpSetup =
        serde_json::from_value(body).expect("UI TotpSetup reads setup response");
    assert!(
        setup.qr_code_url.starts_with("otpauth://totp/"),
        "{setup:?}"
    );
    let roles: Vec<api::Role> = fetch(&c, &token, "/roles").await;
    for builtin in ["Administrator", "NoAccess", "Auditor", "VMAdmin", "VMUser"] {
        assert!(
            roles.iter().any(|r| r.name == builtin),
            "built-in role {builtin}"
        );
    }
    let r = c
        .put(format!("{}/roles/Administrator", api_base()))
        .bearer_auth(&token)
        .json(&json!({"name": "Administrator", "description": "x", "permissions": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 422, "built-in roles are read-only");
    // Custom role: create, then edit it the way the Roles page's edit
    // modal does (api::update_role -> POST /roles/:name), and read it back.
    delete(&c, &token, "/roles/contract-role").await;
    let role = |desc: &str, privs: &[&str]| api::Role {
        name: "contract-role".into(),
        description: desc.into(),
        permissions: vec![api::Permission {
            path: "/".into(),
            privileges: privs.iter().map(|p| p.to_string()).collect(),
        }],
    };
    let (status, body) = post(
        &c,
        &token,
        "/roles",
        serde_json::to_value(role("contract", &["VM.Audit"])).unwrap(),
    )
    .await;
    assert_eq!(status, 201, "create role: {body}");
    let (status, body) = post(
        &c,
        &token,
        "/roles/contract-role",
        serde_json::to_value(role("edited", &["VM.Audit", "VM.PowerMgmt"])).unwrap(),
    )
    .await;
    assert_eq!(status, 200, "edit role: {body}");
    let edited: api::Role = fetch(&c, &token, "/roles/contract-role").await;
    assert_eq!(edited.description, "edited");
    let privs: Vec<&str> = edited
        .permissions
        .iter()
        .flat_map(|p| p.privileges.iter().map(String::as_str))
        .collect();
    assert!(
        privs.contains(&"VM.Audit") && privs.contains(&"VM.PowerMgmt"),
        "edited privileges persisted: {privs:?}"
    );
    delete(&c, &token, "/roles/contract-role").await;
    let tls: api::TlsStatus = fetch(&c, &token, "/tls/status").await;
    assert!(!tls.certificate.fingerprint_sha256.is_empty(), "{tls:?}");
    delete(&c, &token, "/access/realms/contract-ldap").await;
    delete(&c, &token, "/groups/contract-group").await;
    delete(&c, &token, &format!("/users/{}", created.id)).await;

    // ---- cleanup ---------------------------------------------------------
    delete(&c, &token, &format!("/replication/jobs/{rep_id}")).await;
    delete(&c, &token, "/backup-jobs/contract-job").await;
    let mine: Vec<bk::Backup> = fetch(&c, &token, &format!("/backups?target_id={vm}")).await;
    for b in mine {
        delete(&c, &token, &format!("/backups/{}", b.id)).await;
    }
    delete(&c, &token, &format!("/vms/{restored_vm}")).await;
    delete(&c, &token, &format!("/vms/{vm}")).await;
    delete(&c, &token, &format!("/containers/{ct}")).await;

    assert!(
        failures.is_empty(),
        "UI/API contract failures:\n  {}",
        failures.join("\n  ")
    );
}

// ===========================================================================
// Lane M: Monitoring > Notifications, Monitoring > Metrics Explorer and
// Kubernetes > Helm. Every route those pages call is exercised here with the
// UI's own request/response types, and every action is checked for its real
// effect (a message arriving at a local receiver, a ConfigMap rendered by a
// Helm release in a real k3s cluster).
// ===========================================================================

async fn send_json(
    c: &reqwest::Client,
    token: &str,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> (u16, String) {
    let mut r = c
        .request(method, format!("{}{}", api_base(), path))
        .bearer_auth(token);
    if let Some(b) = body {
        r = r.json(&b);
    }
    let r = r.send().await.unwrap();
    (r.status().as_u16(), r.text().await.unwrap_or_default())
}

fn decode<T: DeserializeOwned>(what: &str, body: &str) -> T {
    serde_json::from_str(body).unwrap_or_else(|e| {
        panic!(
            "UI type {} cannot read {what}: {e}\nbody: {}",
            std::any::type_name::<T>(),
            &body[..body.len().min(800)]
        )
    })
}

/// One HTTP request as a local receiver saw it.
#[derive(Debug, Clone)]
struct Captured {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Captured {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

/// Minimal HTTP/1.1 receiver: records each request, answers `status`.
async fn http_receiver(
    bind: &str,
    status: u16,
) -> (
    std::net::SocketAddr,
    tokio::sync::mpsc::UnboundedReceiver<Captured>,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind(bind).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                let (head_end, content_len) = loop {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..i]).to_string();
                        let len = head
                            .lines()
                            .find_map(|l| {
                                let (k, v) = l.split_once(':')?;
                                k.trim()
                                    .eq_ignore_ascii_case("content-length")
                                    .then(|| v.trim().parse::<usize>().ok())?
                            })
                            .unwrap_or(0);
                        break (i + 4, len);
                    }
                };
                while buf.len() < head_end + content_len {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                }
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                let mut lines = head.lines();
                let mut first = lines.next().unwrap_or("").split_whitespace();
                let method = first.next().unwrap_or("").to_string();
                let path = first.next().unwrap_or("").to_string();
                let headers = lines
                    .filter_map(|l| l.split_once(':'))
                    .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                    .collect();
                let body = String::from_utf8_lossy(&buf[head_end..]).to_string();
                let _ = tx.send(Captured {
                    method,
                    path,
                    headers,
                    body,
                });
                let reply = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: text/plain\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
                );
                let _ = sock.write_all(reply.as_bytes()).await;
            });
        }
    });
    (addr, rx)
}

/// Minimal SMTP receiver (no TLS/AUTH): yields each message's DATA.
async fn smtp_receiver() -> (
    std::net::SocketAddr,
    tokio::sync::mpsc::UnboundedReceiver<String>,
) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            let tx = tx.clone();
            tokio::spawn(async move {
                let (r, mut w) = sock.into_split();
                let mut r = BufReader::new(r);
                let _ = w.write_all(b"220 contract-smtp ready\r\n").await;
                let mut line = String::new();
                let mut data: Option<String> = None;
                loop {
                    line.clear();
                    if r.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    if let Some(d) = data.as_mut() {
                        if line == ".\r\n" {
                            let _ = tx.send(data.take().unwrap());
                            let _ = w.write_all(b"250 queued\r\n").await;
                        } else {
                            d.push_str(&line);
                        }
                        continue;
                    }
                    let cmd = line.trim_end().to_uppercase();
                    let reply: &[u8] = if cmd.starts_with("EHLO") {
                        b"250 contract-smtp\r\n"
                    } else if cmd.starts_with("DATA") {
                        data = Some(String::new());
                        b"354 go\r\n"
                    } else if cmd.starts_with("QUIT") {
                        let _ = w.write_all(b"221 bye\r\n").await;
                        return;
                    } else {
                        b"250 ok\r\n"
                    };
                    let _ = w.write_all(reply).await;
                }
            });
        }
    });
    (addr, rx)
}

async fn next<T>(rx: &mut tokio::sync::mpsc::UnboundedReceiver<T>, what: &str) -> T {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .unwrap_or_else(|_| panic!("{what}: nothing arrived within 30s"))
        .unwrap()
}

fn channel(
    name: &str,
    kind: &str,
    config: api::NotificationChannelConfig,
) -> api::NotificationChannel {
    api::NotificationChannel {
        id: format!("channel-{}", chrono::Utc::now().timestamp()),
        name: name.into(),
        description: Some("contract".into()),
        channel_type: kind.into(),
        enabled: true,
        config,
        created_at: chrono::Utc::now(),
        last_used: None,
        success_count: 0,
        failure_count: 0,
    }
}

#[tokio::test]
async fn notification_channels_deliver_for_real() {
    use reqwest::Method;
    let _serial = CHANNEL_TESTS.lock().await;
    let c = client();
    let token = login(&c).await;
    let (hook_addr, mut hooks) = http_receiver("127.0.0.1:0", 200).await;
    let (bad_addr, mut bad_hooks) = http_receiver("127.0.0.1:0", 500).await;
    let (smtp_addr, mut mails) = smtp_receiver().await;
    let mut created: Vec<String> = Vec::new();

    // Create through the same request the UI's create form sends.
    async fn create(
        c: &reqwest::Client,
        token: &str,
        ch: &api::NotificationChannel,
    ) -> api::NotificationChannel {
        let (status, body) = send_json(
            c,
            token,
            reqwest::Method::POST,
            "/notifications/channels",
            Some(serde_json::to_value(ch).unwrap()),
        )
        .await;
        assert!(
            status == 200 || status == 201,
            "create channel {}: {status} {body}",
            ch.name
        );
        decode("POST /notifications/channels", &body)
    }
    async fn test(c: &reqwest::Client, token: &str, id: &str) -> (u16, String) {
        send_json(
            c,
            token,
            reqwest::Method::POST,
            &format!("/notifications/channels/{id}/test"),
            Some(Value::Null),
        )
        .await
    }

    // ---- webhook with bearer auth + custom header --------------------------
    let mut headers = std::collections::HashMap::new();
    headers.insert("X-Contract".to_string(), "lane-m".to_string());
    let wh = create(
        &c,
        &token,
        &channel(
            "contract-webhook",
            "webhook",
            api::NotificationChannelConfig::Webhook {
                url: format!("http://{hook_addr}/hook"),
                method: "POST".into(),
                headers,
                auth: Some(api::WebhookAuth::BearerToken {
                    token: "s3cret-token".into(),
                }),
            },
        ),
    )
    .await;
    created.push(wh.id.clone());
    assert_ne!(wh.id, "", "server assigns an id");
    // The stored secret is never echoed back.
    match &wh.config {
        api::NotificationChannelConfig::Webhook {
            auth: Some(api::WebhookAuth::BearerToken { token }),
            ..
        } => assert_ne!(token, "s3cret-token", "token must be masked"),
        other => panic!("webhook config came back as {other:?}"),
    }
    match &wh.config {
        api::NotificationChannelConfig::Webhook { url, .. } => {
            assert_eq!(
                url,
                &format!("http://{hook_addr}/********"),
                "url must be masked"
            )
        }
        other => panic!("webhook config came back as {other:?}"),
    }
    let (status, body) = test(&c, &token, &wh.id).await;
    assert_eq!(status, 204, "webhook test: {body}");
    let got = next(&mut hooks, "webhook test").await;
    assert_eq!(got.method, "POST");
    assert_eq!(got.path, "/hook");
    assert_eq!(got.header("authorization"), Some("Bearer s3cret-token"));
    assert_eq!(got.header("x-contract"), Some("lane-m"));
    assert_eq!(got.json()["event"], "test");

    // Edit like the UI does (it re-sends the masked token): the real token
    // must survive.
    let mut edited = wh.clone();
    edited.name = "contract-webhook-renamed".into();
    let (status, body) = send_json(
        &c,
        &token,
        Method::PUT,
        &format!("/notifications/channels/{}", wh.id),
        Some(serde_json::to_value(&edited).unwrap()),
    )
    .await;
    assert_eq!(status, 200, "update channel: {body}");
    let upd: api::NotificationChannel = decode("PUT /notifications/channels/:id", &body);
    assert_eq!(upd.name, "contract-webhook-renamed");
    assert_eq!(test(&c, &token, &wh.id).await.0, 204);
    let got = next(&mut hooks, "webhook test after edit").await;
    assert_eq!(
        got.path, "/hook",
        "the masked url re-sent by the edit kept the real one"
    );
    assert_eq!(got.header("authorization"), Some("Bearer s3cret-token"));

    // ---- Slack / Discord / Teams payload shapes ---------------------------
    for (name, kind, cfg, check) in [
        (
            "contract-slack",
            "slack",
            api::NotificationChannelConfig::Slack {
                webhook_url: format!("http://{hook_addr}/slack"),
                channel: Some("#ops".into()),
                username: Some("horcrux".into()),
                icon_emoji: None,
            },
            "text",
        ),
        (
            "contract-discord",
            "discord",
            api::NotificationChannelConfig::Discord {
                webhook_url: format!("http://{hook_addr}/discord"),
                username: None,
                avatar_url: None,
            },
            "content",
        ),
        (
            "contract-teams",
            "teams",
            api::NotificationChannelConfig::Teams {
                webhook_url: format!("http://{hook_addr}/teams"),
            },
            "text",
        ),
    ] {
        let ch = create(&c, &token, &channel(name, kind, cfg)).await;
        created.push(ch.id.clone());
        let (status, body) = test(&c, &token, &ch.id).await;
        assert_eq!(status, 204, "{kind} test: {body}");
        let got = next(&mut hooks, kind).await;
        assert!(
            got.json()[check]
                .as_str()
                .unwrap_or("")
                .contains("test notification"),
            "{kind} payload: {}",
            got.body
        );
    }

    // ---- PagerDuty Events v2 (CI points HORCRUX_PAGERDUTY_EVENTS_URL at
    // 127.0.0.1:18765) ---------------------------------------------------
    if let Ok(pd_url) = std::env::var("HORCRUX_PAGERDUTY_EVENTS_URL") {
        let bind = pd_url
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap()
            .to_string();
        let (_pd_addr, mut pd) = http_receiver(&bind, 202).await;
        let ch = create(
            &c,
            &token,
            &channel(
                "contract-pagerduty",
                "pagerduty",
                api::NotificationChannelConfig::PagerDuty {
                    integration_key: "R0UTINGKEY".into(),
                    routing_key: "R0UTINGKEY".into(),
                    severity: "warning".into(),
                },
            ),
        )
        .await;
        created.push(ch.id.clone());
        let (status, body) = test(&c, &token, &ch.id).await;
        assert_eq!(status, 204, "pagerduty test: {body}");
        let got = next(&mut pd, "pagerduty").await;
        let v = got.json();
        assert_eq!(v["routing_key"], "R0UTINGKEY");
        assert_eq!(v["event_action"], "trigger");
        assert_eq!(v["payload"]["severity"], "warning");
    }

    // ---- e-mail over SMTP ------------------------------------------------
    let mail = create(
        &c,
        &token,
        &channel(
            "contract-email",
            "email",
            api::NotificationChannelConfig::Email {
                address: "ops@example.org".into(),
                smtp_server: "127.0.0.1".into(),
                smtp_port: smtp_addr.port(),
                username: String::new(),
                password: String::new(),
                use_tls: false,
            },
        ),
    )
    .await;
    created.push(mail.id.clone());
    let (status, body) = test(&c, &token, &mail.id).await;
    assert_eq!(status, 204, "email test: {body}");
    let data = next(&mut mails, "email").await;
    assert!(data.contains("To: ops@example.org"), "{data}");
    assert!(data.contains("Horcrux test notification"), "{data}");

    // ---- a failing target: 502 with the target's answer, counted ---------
    let failing = create(
        &c,
        &token,
        &channel(
            "contract-failing",
            "webhook",
            api::NotificationChannelConfig::Webhook {
                url: format!("http://{bad_addr}/down"),
                method: "POST".into(),
                headers: Default::default(),
                auth: None,
            },
        ),
    )
    .await;
    created.push(failing.id.clone());
    let (status, body) = test(&c, &token, &failing.id).await;
    assert_eq!(
        status, 502,
        "failing target must report the failure: {body}"
    );
    assert!(body.contains("HTTP 500"), "{body}");
    next(&mut bad_hooks, "failing webhook").await;

    // ---- invalid configs are rejected, not stored -------------------------
    let (status, _) = send_json(
        &c,
        &token,
        Method::POST,
        "/notifications/channels",
        Some(
            serde_json::to_value(channel(
                "contract-bad",
                "webhook",
                api::NotificationChannelConfig::Webhook {
                    url: "not a url".into(),
                    method: "POST".into(),
                    headers: Default::default(),
                    auth: None,
                },
            ))
            .unwrap(),
        ),
    )
    .await;
    assert_eq!(status, 400, "invalid webhook URL accepted");

    // ---- the list the page renders reflects real delivery counters --------
    let list: Vec<api::NotificationChannel> = fetch(&c, &token, "/notifications/channels").await;
    let w = list.iter().find(|x| x.id == wh.id).expect("webhook listed");
    assert_eq!(w.success_count, 2, "{w:?}");
    assert!(w.last_used.is_some());
    let f = list
        .iter()
        .find(|x| x.id == failing.id)
        .expect("failing listed");
    assert_eq!((f.success_count, f.failure_count), (0, 1), "{f:?}");
    let one: api::NotificationChannel =
        fetch(&c, &token, &format!("/notifications/channels/{}", mail.id)).await;
    assert_eq!(one.success_count, 1);

    for id in &created {
        let (status, _) = send_json(
            &c,
            &token,
            Method::DELETE,
            &format!("/notifications/channels/{id}"),
            None,
        )
        .await;
        assert_eq!(status, 204, "delete channel {id}");
    }
    let (status, _) = get_raw(&c, &token, &format!("/notifications/channels/{}", wh.id)).await;
    assert_eq!(status, 404, "deleted channel still readable");
}

/// Serializes the tests that create notification channels or fire alerts:
/// a fired alert goes to EVERY enabled channel, so running them together
/// would put unexpected messages into each other's receivers / counters.
static CHANNEL_TESTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Monitoring > Alert Center against the real server: the UI's own
/// `AlertRule` / `ActiveAlert` types must read what the API returns for a
/// real rule (the old UI type expected `condition` to be a string, so the
/// page showed "invalid type: map, expected a string" for any rule), every
/// action the page offers must work, and a rule must really fire from the
/// node's metrics and be delivered to a notification channel.
#[tokio::test]
async fn alert_center_rules_fire_for_real() {
    use reqwest::Method;
    let _serial = CHANNEL_TESTS.lock().await;
    let c = client();
    let token = login(&c).await;
    let (hook_addr, mut hooks) = http_receiver("127.0.0.1:0", 200).await;

    let (status, body) = send_json(
        &c,
        &token,
        Method::POST,
        "/notifications/channels",
        Some(
            serde_json::to_value(channel(
                "contract-alert-hook",
                "webhook",
                api::NotificationChannelConfig::Webhook {
                    url: format!("http://{hook_addr}/alert"),
                    method: "POST".into(),
                    headers: Default::default(),
                    auth: None,
                },
            ))
            .unwrap(),
        ),
    )
    .await;
    assert!(
        status == 200 || status == 201,
        "create channel: {status} {body}"
    );
    let hook: api::NotificationChannel = decode("POST /notifications/channels", &body);

    // A rule the node always satisfies (load average > -1), firing at once.
    let rule = api::AlertRule {
        id: format!("contract-rule-{}", chrono::Utc::now().timestamp_millis()),
        name: "contract any load".into(),
        description: "contract test".into(),
        severity: "info".into(),
        enabled: true,
        condition: api::AlertCondition {
            metric_type: "node_load".into(),
            operator: "greaterthan".into(),
            threshold: -1.0,
            target_pattern: "*".into(),
            duration_seconds: 0,
        },
    };
    let rule_path = format!("/alerts/rules/{}", rule.id);
    let (status, body) = send_json(
        &c,
        &token,
        Method::POST,
        "/alerts/rules",
        Some(serde_json::to_value(&rule).unwrap()),
    )
    .await;
    assert_eq!(status, 201, "create rule: {body}");
    let rules: Vec<api::AlertRule> = fetch(&c, &token, "/alerts/rules").await;
    assert_eq!(
        rules.iter().find(|r| r.id == rule.id),
        Some(&rule),
        "listed rule round-trips through the UI type"
    );

    // Edit (the page's Save Changes).
    let mut edited = rule.clone();
    edited.name = "contract any load (edited)".into();
    edited.severity = "warning".into();
    let (status, body) = send_json(
        &c,
        &token,
        Method::PUT,
        &rule_path,
        Some(serde_json::to_value(&edited).unwrap()),
    )
    .await;
    assert_eq!(status, 200, "update rule: {body}");
    assert_eq!(
        decode::<api::AlertRule>("PUT /alerts/rules/:id", &body),
        edited
    );
    let rules: Vec<api::AlertRule> = fetch(&c, &token, "/alerts/rules").await;
    assert_eq!(rules.iter().find(|r| r.id == rule.id), Some(&edited));

    // Test button: a test alert reaches the channel.
    let (status, body) = send_json(
        &c,
        &token,
        Method::POST,
        &format!("{rule_path}/test"),
        Some(Value::Null),
    )
    .await;
    assert_eq!(status, 200, "test rule: {body}");
    let res: api::AlertRuleTestResult = decode("POST /alerts/rules/:id/test", &body);
    assert!(res.channels >= 1 && res.delivered >= 1, "{res:?}");

    // The evaluator (every 15 s) fires the rule from real node metrics.
    let mut fired: Option<api::ActiveAlert> = None;
    for _ in 0..60 {
        let active: Vec<api::ActiveAlert> = fetch(&c, &token, "/alerts/active").await;
        if let Some(a) = active.into_iter().find(|a| a.rule_id == rule.id) {
            fired = Some(a);
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let fired = fired.expect("rule never fired from the node's metrics");
    assert_eq!(fired.status, "firing");
    assert_eq!(fired.severity, "warning");
    assert_eq!(fired.rule_name, edited.name);
    assert!(
        !fired.target.is_empty() && fired.metric_value > -1.0,
        "{fired:?}"
    );

    // Both the test alert and the real one were delivered to the webhook.
    let mut events = std::collections::HashSet::new();
    while !(events.contains("alert_test") && events.contains("alert")) {
        let got = next(&mut hooks, "alert delivery").await;
        assert_eq!(got.path, "/alert");
        events.insert(got.json()["event"].as_str().unwrap_or("").to_string());
    }

    // Acknowledge (the page's button sends the logged-in user).
    let (status, body) = send_json(
        &c,
        &token,
        Method::POST,
        &format!("/alerts/{}/{}/acknowledge", rule.id, fired.target),
        Some(json!({"user": "admin"})),
    )
    .await;
    assert_eq!(status, 200, "acknowledge: {body}");
    let active: Vec<api::ActiveAlert> = fetch(&c, &token, "/alerts/active").await;
    let acked = active
        .iter()
        .find(|a| a.rule_id == rule.id)
        .expect("still active");
    assert_eq!(acked.status, "acknowledged");
    assert_eq!(acked.acknowledged_by.as_deref(), Some("admin"));

    // Disable: the alert resolves and lands in the history.
    let (status, body) = send_json(
        &c,
        &token,
        Method::PUT,
        &format!("{rule_path}/toggle"),
        Some(json!({"enabled": false})),
    )
    .await;
    assert_eq!(status, 200, "toggle: {body}");
    assert!(!decode::<api::AlertRule>("PUT /alerts/rules/:id/toggle", &body).enabled);
    let active: Vec<api::ActiveAlert> = fetch(&c, &token, "/alerts/active").await;
    assert!(
        active.iter().all(|a| a.rule_id != rule.id),
        "resolved on disable"
    );
    let history: Vec<api::ActiveAlert> = fetch(&c, &token, "/alerts/history?limit=200").await;
    assert!(
        history
            .iter()
            .any(|a| a.rule_id == rule.id && a.status == "resolved" && a.resolved_at.is_some()),
        "resolved alert in history"
    );

    let (status, _) = send_json(&c, &token, Method::DELETE, &rule_path, None).await;
    assert_eq!(status, 204, "delete rule");
    let (status, _) = send_json(&c, &token, Method::DELETE, &rule_path, None).await;
    assert_eq!(status, 404, "deleting a missing rule is a 404, not a 500");
    let rules: Vec<api::AlertRule> = fetch(&c, &token, "/alerts/rules").await;
    assert!(rules.iter().all(|r| r.id != rule.id));
    let (status, _) = send_json(
        &c,
        &token,
        Method::DELETE,
        &format!("/notifications/channels/{}", hook.id),
        None,
    )
    .await;
    assert_eq!(status, 204);
}

#[tokio::test]
async fn metrics_explorer_queries_real_samples() {
    let c = client();
    let token = login(&c).await;
    // The sampler records host series every 15 s (first one ~1 s after start).
    let mut defs: Vec<api::MetricDefinition> = Vec::new();
    for _ in 0..60 {
        defs = fetch(&c, &token, "/metrics/definitions").await;
        if defs
            .iter()
            .any(|d| d.name == "horcrux_node_memory_used_bytes" && d.cardinality > 0)
        {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let mem = defs
        .iter()
        .find(|d| d.name == "horcrux_node_memory_used_bytes")
        .expect("node memory metric defined");
    assert!(mem.cardinality >= 1, "no samples recorded: {mem:?}");
    assert!(mem.labels.contains(&"node".to_string()));
    assert!(!mem.last_scraped.is_empty());

    // Exactly what the explorer's builder sends (avg(...), 1 minute step).
    let end = chrono::Utc::now() + chrono::Duration::seconds(5);
    let q = api::MetricQuery {
        metric: "avg(horcrux_node_memory_used_bytes)".into(),
        labels: Default::default(),
        start_time: Some(end - chrono::Duration::hours(1)),
        end_time: Some(end),
        step: 60,
    };
    let (status, body) = post(
        &c,
        &token,
        "/metrics/query",
        serde_json::to_value(&q).unwrap(),
    )
    .await;
    assert_eq!(status, 200, "query: {body}");
    let r: api::MetricResult = serde_json::from_value(body).expect("UI MetricResult");
    assert!(!r.values.is_empty(), "{r:?}");
    let total = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|m| {
            m.lines()
                .find(|l| l.starts_with("MemTotal:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse::<f64>().ok())
        })
        .map(|kb| kb * 1024.0);
    for p in &r.values {
        assert!(p.value > 0.0, "{p:?}");
        if let Some(t) = total {
            assert!(p.value <= t, "used memory above MemTotal: {p:?}");
        }
    }

    // Raw selector with a label matcher returns labelled points.
    let node = r.values[0].labels.get("node").cloned();
    let q = api::MetricQuery {
        metric: "horcrux_node_load1{node=~\".+\"}".into(),
        labels: Default::default(),
        start_time: Some(end - chrono::Duration::minutes(15)),
        end_time: Some(end),
        step: 15,
    };
    let (status, body) = post(
        &c,
        &token,
        "/metrics/query",
        serde_json::to_value(&q).unwrap(),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let r: api::MetricResult = serde_json::from_value(body).unwrap();
    assert!(
        r.values.iter().all(|p| p.labels.contains_key("node")),
        "{r:?}"
    );
    let _ = node;

    // Unknown metrics / functions are a 400 with a reason, not an empty chart.
    for bad in ["no_such_metric", "rate(horcrux_node_load1[5m])"] {
        let q = api::MetricQuery {
            metric: bad.into(),
            labels: Default::default(),
            start_time: None,
            end_time: None,
            step: 60,
        };
        let (status, body) = post(
            &c,
            &token,
            "/metrics/query",
            serde_json::to_value(&q).unwrap(),
        )
        .await;
        assert_eq!(status, 400, "{bad}: {body}");
    }
}

/// Helm against a real cluster. CI's Test job starts k3s and a local chart
/// repository (horcrux-api/tests/k8s_helm_setup.sh) and exports
/// HORCRUX_CONTRACT_KUBECONFIG / HORCRUX_CONTRACT_HELM_REPO; with
/// HORCRUX_TEST_REQUIRE_K8S set, a missing cluster fails the test.
#[tokio::test]
async fn helm_pages_drive_a_real_cluster() {
    use reqwest::Method;
    let (Ok(kubeconfig), Ok(repo_url)) = (
        std::env::var("HORCRUX_CONTRACT_KUBECONFIG"),
        std::env::var("HORCRUX_CONTRACT_HELM_REPO"),
    ) else {
        if std::env::var("HORCRUX_TEST_REQUIRE_K8S").is_ok() {
            panic!("HORCRUX_TEST_REQUIRE_K8S is set but no k3s/chart repo was provided");
        }
        eprintln!("helm contract skipped: no HORCRUX_CONTRACT_KUBECONFIG");
        return;
    };
    let c = client();
    let token = login(&c).await;
    let kubeconfig = std::fs::read_to_string(&kubeconfig).expect("kubeconfig readable");

    // ---- cluster ------------------------------------------------------------
    let cluster_name = format!("contract-k3s-{}", std::process::id());
    let (status, body) = post(
        &c,
        &token,
        "/k8s/clusters",
        json!({"name": cluster_name, "kubeconfig": kubeconfig}),
    )
    .await;
    assert!(
        status == 200 || status == 201,
        "connect cluster: {status} {body}"
    );
    let cid = body["id"].as_str().expect("cluster id").to_string();

    // ---- repositories page --------------------------------------------------
    let repo = "horcrux-contract";
    let _ = send_json(
        &c,
        &token,
        Method::DELETE,
        &format!("/k8s/helm/repos/{repo}"),
        None,
    )
    .await;
    let add = api::AddHelmRepoRequest {
        name: repo.into(),
        url: repo_url.clone(),
        username: None,
        password: None,
        force_update: Some(true),
    };
    let (status, body) = post(
        &c,
        &token,
        "/k8s/helm/repos",
        serde_json::to_value(&add).unwrap(),
    )
    .await;
    assert!(status == 200 || status == 201, "add repo: {status} {body}");
    let r: api::HelmRepository = serde_json::from_value(body).expect("UI HelmRepository");
    assert_eq!(r.status, "ready");
    assert!(r.added_at.is_some() && r.last_update.is_some(), "{r:?}");
    let repos: Vec<api::HelmRepository> = fetch(&c, &token, "/k8s/helm/repos").await;
    assert!(repos
        .iter()
        .any(|x| x.name == repo && x.url.starts_with(&repo_url)));
    let (status, body) = send_json(
        &c,
        &token,
        Method::POST,
        &format!("/k8s/helm/repos/{repo}/update"),
        None,
    )
    .await;
    assert_eq!(status, 200, "update repo: {body}");

    // ---- charts page ----------------------------------------------------------
    let charts: Vec<api::HelmChart> = fetch(
        &c,
        &token,
        &format!("/k8s/helm/charts?query=horcrux-hello&repo={repo}"),
    )
    .await;
    let chart = charts
        .iter()
        .find(|x| x.name == "horcrux-hello")
        .expect("chart found by search");
    assert_eq!(chart.repository, repo);
    assert_eq!(chart.version, "0.2.0");
    assert!(
        chart.keywords.contains(&"contract".to_string()),
        "{chart:?}"
    );
    assert_eq!(chart.maintainers[0].name, "Horcrux CI");
    let one: api::HelmChart = fetch(
        &c,
        &token,
        &format!("/k8s/helm/charts/{repo}/horcrux-hello"),
    )
    .await;
    assert_eq!(one.version, "0.2.0");
    let versions: Vec<String> = fetch(
        &c,
        &token,
        &format!("/k8s/helm/charts/{repo}/horcrux-hello/versions"),
    )
    .await;
    assert_eq!(versions, vec!["0.2.0", "0.1.0"]);
    let defaults: Value = fetch(
        &c,
        &token,
        &format!("/k8s/helm/charts/{repo}/horcrux-hello/values?version=0.1.0"),
    )
    .await;
    assert_eq!(defaults["message"], "hello");

    // ---- install (charts page) ---------------------------------------------
    let ns = "contract-helm";
    let rel = "contract-web";
    let cm_path = format!("/k8s/clusters/{cid}/namespaces/{ns}/configmaps/{rel}-hello");
    let _ = send_json(
        &c,
        &token,
        Method::DELETE,
        &format!("/k8s/clusters/{cid}/namespaces/{ns}/helm/releases/{rel}"),
        None,
    )
    .await;
    let install = api::HelmInstallRequest {
        name: rel.into(),
        chart: format!("{}/{}", chart.repository, chart.name),
        version: Some("0.1.0".into()),
        values: Some(json!({"message": "installed"})),
        create_namespace: Some(true),
        wait: Some(true),
        timeout: Some("120s".into()),
    };
    let (status, body) = post(
        &c,
        &token,
        &format!("/k8s/clusters/{cid}/namespaces/{ns}/helm/install"),
        serde_json::to_value(&install).unwrap(),
    )
    .await;
    assert!(status == 200 || status == 201, "install: {status} {body}");
    let r: api::HelmRelease = serde_json::from_value(body).expect("UI HelmRelease");
    assert_eq!((r.revision, r.status.as_str()), (1, "deployed"), "{r:?}");
    let cm: Value = fetch(&c, &token, &cm_path).await;
    assert_eq!(cm["data"]["message"], "installed", "{cm}");
    assert_eq!(cm["data"]["chartVersion"], "0.1.0");

    // ---- releases page --------------------------------------------------------
    let list: Vec<api::HelmRelease> = fetch(
        &c,
        &token,
        &format!("/k8s/clusters/{cid}/helm/releases?namespace={ns}"),
    )
    .await;
    let mine = list.iter().find(|x| x.name == rel).expect("release listed");
    assert_eq!(mine.chart, "horcrux-hello");
    let all: Vec<api::HelmRelease> =
        fetch(&c, &token, &format!("/k8s/clusters/{cid}/helm/releases")).await;
    assert!(all.iter().any(|x| x.name == rel && x.namespace == ns));
    let detail: api::HelmRelease = fetch(
        &c,
        &token,
        &format!("/k8s/clusters/{cid}/helm/releases/{ns}/{rel}"),
    )
    .await;
    assert_eq!(detail.revision, 1);

    // Upgrade exactly as the releases page does: chart = the bare chart name
    // from the list, new version, edited values.
    let upgrade = api::HelmInstallRequest {
        name: rel.into(),
        chart: mine.chart.clone(),
        version: Some("0.2.0".into()),
        values: Some(json!({"message": "upgraded"})),
        create_namespace: None,
        wait: Some(true),
        timeout: Some("120s".into()),
    };
    let (status, body) = post(
        &c,
        &token,
        &format!("/k8s/clusters/{cid}/namespaces/{ns}/helm/releases/{rel}/upgrade"),
        serde_json::to_value(&upgrade).unwrap(),
    )
    .await;
    assert_eq!(status, 200, "upgrade: {body}");
    let r: api::HelmRelease = serde_json::from_value(body).unwrap();
    assert_eq!(r.revision, 2);
    let cm: Value = fetch(&c, &token, &cm_path).await;
    assert_eq!(cm["data"]["message"], "upgraded");
    assert_eq!(cm["data"]["chartVersion"], "0.2.0");

    let values: api::HelmValues = fetch(
        &c,
        &token,
        &format!("/k8s/clusters/{cid}/namespaces/{ns}/helm/releases/{rel}/values"),
    )
    .await;
    assert_eq!(values.user_supplied_values["message"], "upgraded");
    let history: Vec<api::HelmRelease> = fetch(
        &c,
        &token,
        &format!("/k8s/clusters/{cid}/namespaces/{ns}/helm/releases/{rel}/history"),
    )
    .await;
    assert_eq!(history.len(), 2, "{history:?}");
    assert_eq!(history[0].revision, 1);

    // Rollback to revision 1 (history modal)
    let (status, body) = post(
        &c,
        &token,
        &format!("/k8s/clusters/{cid}/namespaces/{ns}/helm/releases/{rel}/rollback"),
        json!({"revision": 1}),
    )
    .await;
    assert_eq!(status, 200, "rollback: {body}");
    let r: api::HelmRelease = serde_json::from_value(body).unwrap();
    assert_eq!(r.revision, 3);
    let cm: Value = fetch(&c, &token, &cm_path).await;
    assert_eq!(
        cm["data"]["message"], "installed",
        "rollback restored rev 1"
    );

    // Bad names never reach helm as flags.
    let (status, _) = post(
        &c,
        &token,
        &format!("/k8s/clusters/{cid}/namespaces/{ns}/helm/install"),
        json!({"name": "--post-renderer=/bin/sh", "chart": "x/y"}),
    )
    .await;
    assert_eq!(status, 400);

    // Uninstall: the release's ConfigMap is really gone.
    let (status, body) = send_json(
        &c,
        &token,
        Method::DELETE,
        &format!("/k8s/clusters/{cid}/namespaces/{ns}/helm/releases/{rel}"),
        None,
    )
    .await;
    assert_eq!(status, 204, "uninstall: {body}");
    let (status, _) = get_raw(&c, &token, &cm_path).await;
    assert_eq!(status, 404, "ConfigMap left behind after uninstall");
    let list: Vec<api::HelmRelease> = fetch(
        &c,
        &token,
        &format!("/k8s/clusters/{cid}/helm/releases?namespace={ns}"),
    )
    .await;
    assert!(list.iter().all(|x| x.name != rel));

    // ---- the other Kubernetes pages read this same real cluster -----------
    k8s_pages_contract(&c, &token, &cid, ns).await;

    let (status, _) = send_json(
        &c,
        &token,
        Method::DELETE,
        &format!("/k8s/helm/repos/{repo}"),
        None,
    )
    .await;
    assert_eq!(status, 204);
    let repos: Vec<api::HelmRepository> = fetch(&c, &token, "/k8s/helm/repos").await;
    assert!(repos.iter().all(|x| x.name != repo));
    let _ = send_json(
        &c,
        &token,
        Method::DELETE,
        &format!("/k8s/clusters/{cid}"),
        None,
    )
    .await;
}

/// Kubernetes cluster/workload/config pages against the real cluster: every
/// list decodes into the page's type, and creates/edits sent with the UI's
/// request types really change the cluster.
async fn k8s_pages_contract(c: &reqwest::Client, token: &str, cid: &str, ns: &str) {
    use horcrux_ui::pages::kubernetes::{cluster_dashboard, management};
    use reqwest::Method;
    let base = format!("/k8s/clusters/{cid}");

    let clusters: Vec<cluster_dashboard::K8sCluster> = fetch(c, token, "/k8s/clusters").await;
    assert!(clusters
        .iter()
        .any(|x| x.id == cid && x.status == "connected"));
    let _: Vec<management::K8sCluster> = fetch(c, token, "/k8s/clusters").await;
    let _: management::ClusterHealth = fetch(c, token, &format!("{base}/health")).await;
    let health: cluster_dashboard::ClusterHealth = fetch(c, token, &format!("{base}/health")).await;
    assert_eq!(health.node_status.total_nodes, 1, "{health:?}");
    assert_eq!(health.node_status.ready_nodes, 1, "{health:?}");
    let nodes: Vec<cluster_dashboard::K8sNode> = fetch(c, token, &format!("{base}/nodes")).await;
    let n = &nodes[0];
    assert!(
        !n.cpu_capacity.is_empty() && !n.pods_allocatable.is_empty(),
        "{n:?}"
    );
    assert!(n.version.starts_with('v'), "{n:?}");

    let pods: Vec<api::KubernetesPod> =
        fetch(c, token, &format!("{base}/namespaces/kube-system/pods")).await;
    let dns = pods
        .iter()
        .find(|p| p.name.starts_with("coredns"))
        .expect("coredns pod listed");
    assert_eq!(dns.status.phase, "Running", "{dns:?}");
    assert_eq!(dns.ready, "1/1");
    assert_ne!(dns.age, "-");
    let deps: Vec<api::KubernetesDeployment> = fetch(
        c,
        token,
        &format!("{base}/namespaces/kube-system/deployments"),
    )
    .await;
    let d = deps
        .iter()
        .find(|d| d.name == "coredns")
        .expect("coredns deployment");
    assert_eq!(d.replicas.desired, 1);
    let svcs: Vec<api::KubernetesService> =
        fetch(c, token, &format!("{base}/namespaces/default/services")).await;
    let k = svcs
        .iter()
        .find(|s| s.name == "kubernetes")
        .expect("kubernetes service");
    assert_eq!(k.ports[0].port, 443);

    // Service: create with the UI's request, list, delete.
    let svc = api::CreateServiceRequest {
        name: "contract-svc".into(),
        service_type: "ClusterIP".into(),
        ports: vec![api::ServicePort {
            name: Some("http".into()),
            protocol: "TCP".into(),
            port: 80,
            target_port: Some("8080".into()),
            node_port: None,
        }],
        selector: [("app".to_string(), "contract".to_string())].into(),
        labels: None,
    };
    let (status, body) = post(
        c,
        token,
        &format!("{base}/namespaces/{ns}/services"),
        serde_json::to_value(&svc).unwrap(),
    )
    .await;
    assert!(
        status == 200 || status == 201,
        "create service: {status} {body}"
    );
    let svcs: Vec<api::KubernetesService> =
        fetch(c, token, &format!("{base}/namespaces/{ns}/services")).await;
    assert!(svcs
        .iter()
        .any(|s| s.name == "contract-svc" && s.ports[0].port == 80));

    // Ingress pointing at it.
    let ing = api::CreateIngressRequest {
        name: "contract-ing".into(),
        ingress_class: None,
        rules: vec![api::IngressRule {
            host: Some("contract.example.org".into()),
            paths: vec![api::IngressPath {
                path: "/".into(),
                path_type: "Prefix".into(),
                service_name: "contract-svc".into(),
                service_port: 80,
            }],
        }],
        tls: None,
        labels: None,
        annotations: None,
    };
    let (status, body) = post(
        c,
        token,
        &format!("{base}/namespaces/{ns}/ingresses"),
        serde_json::to_value(&ing).unwrap(),
    )
    .await;
    assert!(
        status == 200 || status == 201,
        "create ingress: {status} {body}"
    );
    let ings: Vec<api::KubernetesIngress> =
        fetch(c, token, &format!("{base}/namespaces/{ns}/ingresses")).await;
    let i = ings
        .iter()
        .find(|i| i.name == "contract-ing")
        .expect("ingress listed");
    assert_eq!(i.hosts, vec!["contract.example.org"]);
    assert_eq!(i.rules[0].paths[0].service_name, "contract-svc");
    assert_eq!(i.rules[0].paths[0].service_port, 80);

    // ConfigMap: create, edit (drop a key), check, delete.
    let cm = api::CreateConfigMapRequest {
        name: "contract-cm".into(),
        data: [
            ("keep".to_string(), "1".to_string()),
            ("drop".to_string(), "2".to_string()),
        ]
        .into(),
        binary_data: None,
        labels: None,
        annotations: None,
    };
    let (status, body) = post(
        c,
        token,
        &format!("{base}/namespaces/{ns}/configmaps"),
        serde_json::to_value(&cm).unwrap(),
    )
    .await;
    assert!(
        status == 200 || status == 201,
        "create configmap: {status} {body}"
    );
    let mut edit = cm.clone();
    edit.data = [("keep".to_string(), "changed".to_string())].into();
    edit.labels = Some([("edited".to_string(), "yes".to_string())].into());
    let (status, body) = post(
        c,
        token,
        &format!("{base}/namespaces/{ns}/configmaps/contract-cm"),
        serde_json::to_value(&edit).unwrap(),
    )
    .await;
    assert_eq!(status, 200, "update configmap: {body}");
    let cms: Vec<api::KubernetesConfigMap> =
        fetch(c, token, &format!("{base}/namespaces/{ns}/configmaps")).await;
    let got = cms
        .iter()
        .find(|x| x.name == "contract-cm")
        .expect("configmap listed");
    assert_eq!(got.data.get("keep").map(String::as_str), Some("changed"));
    assert!(
        !got.data.contains_key("drop"),
        "removed key still there: {got:?}"
    );
    assert_eq!(got.labels.get("edited").map(String::as_str), Some("yes"));
    assert!(got.created_at.is_some() && got.age != "-");

    // Secret: create, then edit the way the dialog does (known key with an
    // empty value = keep, new key = set, missing key = remove).
    let secret = api::CreateSecretRequest {
        name: "contract-secret".into(),
        secret_type: "Opaque".into(),
        data: [
            ("user".to_string(), "admin".to_string()),
            ("old".to_string(), "x".to_string()),
        ]
        .into(),
        string_data: None,
        labels: None,
        annotations: None,
    };
    let (status, body) = post(
        c,
        token,
        &format!("{base}/namespaces/{ns}/secrets"),
        serde_json::to_value(&secret).unwrap(),
    )
    .await;
    assert!(
        status == 200 || status == 201,
        "create secret: {status} {body}"
    );
    let list: Vec<api::KubernetesSecret> =
        fetch(c, token, &format!("{base}/namespaces/{ns}/secrets")).await;
    let s = list
        .iter()
        .find(|x| x.name == "contract-secret")
        .expect("secret listed");
    assert!(s.data.contains_key("user") && s.data.values().all(String::is_empty));
    let mut edit = secret.clone();
    edit.data = [
        ("user".to_string(), String::new()),
        ("pw".to_string(), "hunter2".to_string()),
    ]
    .into();
    let (status, body) = post(
        c,
        token,
        &format!("{base}/namespaces/{ns}/secrets/contract-secret"),
        serde_json::to_value(&edit).unwrap(),
    )
    .await;
    assert_eq!(status, 200, "update secret: {body}");
    let list: Vec<api::KubernetesSecret> =
        fetch(c, token, &format!("{base}/namespaces/{ns}/secrets")).await;
    let s = list.iter().find(|x| x.name == "contract-secret").unwrap();
    let mut keys: Vec<&String> = s.data.keys().collect();
    keys.sort();
    assert_eq!(keys, vec!["pw", "user"]);
    // The kept value really is the old one (read straight from k3s).
    if let Ok(k3s) = std::env::var("HORCRUX_CI_K3S") {
        let out = std::process::Command::new("docker")
            .args([
                "exec",
                &k3s,
                "kubectl",
                "-n",
                ns,
                "get",
                "secret",
                "contract-secret",
                "-o",
                "jsonpath={.data.user} {.data.pw}",
            ])
            .output()
            .expect("docker exec kubectl");
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        // base64("admin") base64("hunter2")
        assert_eq!(
            text.trim(),
            "YWRtaW4= aHVudGVyMg==",
            "secret values after edit"
        );
    }

    for (kind, name) in [
        ("ingresses", "contract-ing"),
        ("services", "contract-svc"),
        ("configmaps", "contract-cm"),
        ("secrets", "contract-secret"),
    ] {
        let (status, body) = send_json(
            c,
            token,
            Method::DELETE,
            &format!("{base}/namespaces/{ns}/{kind}/{name}"),
            None,
        )
        .await;
        assert!(
            status == 200 || status == 204,
            "delete {kind}/{name}: {status} {body}"
        );
    }
}

/// `GET /api/features` (public, no token): the UI reads it to hide the
/// pages of USE flags the server was built without. CI's server is built
/// with --all-features, so every feature it knows about must be enabled
/// and the UI's FeatureSet must report each one.
#[tokio::test]
async fn features_endpoint_matches_ui_type() {
    let c = client();
    let r = c
        .get(format!("{}/features", api_base()))
        .send()
        .await
        .expect("horcrux-api must be running for the UI contract test");
    assert_eq!(r.status().as_u16(), 200, "/api/features must be public");
    let body = r.text().await.unwrap();
    let f: horcrux_ui::features::FeaturesResponse = serde_json::from_str(&body)
        .unwrap_or_else(|e| panic!("UI FeaturesResponse cannot read /api/features: {e}\n{body}"));
    assert!(!f.all.is_empty(), "{body}");
    let set = horcrux_ui::features::FeatureSet::from_names(f.features.clone());
    for name in &f.all {
        assert!(
            set.has(name),
            "--all-features server must report '{name}' enabled: {body}"
        );
    }
    for name in [
        "docker",
        "podman",
        "lxc",
        "backup",
        "cluster",
        "nas",
        "kubernetes",
    ] {
        assert!(set.has(name), "{name} missing from {body}");
    }
}

async fn lxc_send(
    c: &reqwest::Client,
    token: &str,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> (u16, String) {
    let mut r = c
        .request(method, format!("{}{}", api_base(), path))
        .bearer_auth(token);
    if let Some(b) = body {
        r = r.json(&b);
    }
    let r = r.send().await.unwrap();
    (r.status().as_u16(), r.text().await.unwrap_or_default())
}

fn lxc_decode<T: DeserializeOwned>(what: &str, status: u16, body: &str) -> T {
    assert!(
        (200..300).contains(&status),
        "{what} -> {status}: {}",
        &body[..body.len().min(800)]
    );
    serde_json::from_str(body).unwrap_or_else(|e| {
        panic!(
            "UI type {} cannot read {what}: {e}\nbody: {}",
            std::any::type_name::<T>(),
            &body[..body.len().min(800)]
        )
    })
}

/// LXC (lane J) routes: every one the UI calls, read into the UI's types.
/// In CI's Test job (no LXC there) the host/template routes and the error
/// paths are checked; lxc_e2e.sh runs this again inside a privileged
/// container with LXC (HORCRUX_CONTRACT_LXC=1) to exercise them all against
/// a real container.
#[tokio::test]
async fn lxc_ui_types_read_real_lxc_responses() {
    use reqwest::Method;
    let c = client();
    let token = login(&c).await;
    let host: api::LxcHostInfo = fetch(&c, &token, "/lxc/host").await;
    let _templates: Vec<api::LxcTemplate> = fetch(&c, &token, "/lxc/templates").await;
    // Routes exist (a missing route would be an empty 404 / 405).
    let (status, body) = get_raw(&c, &token, "/containers/no-such-ct/lxc/config").await;
    assert_eq!(status, 404, "{body}");
    assert!(body.contains("not found"), "{body}");
    let (status, _) = get_raw(&c, &token, "/containers/no-such-ct/snapshots").await;
    assert_eq!(status, 404);
    let (status, _) = lxc_send(&c, &token, Method::DELETE, "/lxc/templates/a/b/c/d", None).await;
    assert_eq!(status, 404, "template delete route");

    if std::env::var("HORCRUX_CONTRACT_LXC").ok().as_deref() != Some("1") {
        eprintln!("HORCRUX_CONTRACT_LXC!=1: LXC host here has available={}; full LXC contract runs in lxc_e2e.sh", host.available);
        return;
    }
    assert!(
        host.available,
        "LXC must be installed for the LXC contract run"
    );

    let catalog: Vec<api::LxcCatalogEntry> =
        fetch(&c, &token, &api::lxc_catalog_path(Some("amd64"), false)).await;
    let entry = catalog
        .iter()
        .filter(|e| e.distro == "alpine" && e.variant == "default" && e.release.contains('.'))
        .max_by_key(|e| {
            e.release
                .split('.')
                .map(|x| x.parse::<u32>().unwrap_or(0))
                .collect::<Vec<_>>()
        })
        .expect("alpine in catalog")
        .clone();
    let (st, b) = lxc_send(&c,
        &token,
        Method::POST,
        "/lxc/templates",
        Some(json!({"distro": entry.distro, "release": entry.release, "arch": entry.arch, "variant": entry.variant})),
    )
    .await;
    let tmpl: api::LxcTemplate = lxc_decode("POST /lxc/templates", st, &b);
    let listed: Vec<api::LxcTemplate> = fetch(&c, &token, "/lxc/templates").await;
    assert!(listed.iter().any(|t| t.id == tmpl.id));

    // Create through the exact body the UI form sends.
    let ct = "contract-lxc";
    let form = horcrux_common::LxcOptions {
        disk_gb: Some(1),
        nets: host
            .bridges
            .first()
            .map(|b| {
                vec![horcrux_common::LxcNet {
                    bridge: b.name.clone(),
                    ip: Some("dhcp".into()),
                    ..Default::default()
                }]
            })
            .unwrap_or_default(),
        mounts: vec![horcrux_common::LxcMount {
            path: "/data".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let req = api::CreateContainerRequest {
        id: ct.into(),
        name: ct.into(),
        runtime: "lxc".into(),
        memory: 128,
        cpus: 1,
        rootfs: format!("{}/{}", entry.distro, entry.release),
        status: "stopped".into(),
        lxc: Some(form),
        ..Default::default()
    };
    let (st, b) = lxc_send(
        &c,
        &token,
        Method::POST,
        "/containers",
        Some(serde_json::to_value(&req).unwrap()),
    )
    .await;
    let created: api::Container = lxc_decode("POST /containers (lxc)", st, &b);
    assert_eq!(created.runtime, "lxc");
    let list: Vec<api::Container> = fetch(&c, &token, "/containers").await;
    assert!(list.iter().any(|x| x.id == ct && x.runtime == "lxc"));
    let full: horcrux_common::ContainerConfig =
        fetch(&c, &token, &format!("/containers/{ct}")).await;
    assert!(full.lxc.is_some());

    let view: api::LxcConfigView = fetch(&c, &token, &format!("/containers/{ct}/lxc/config")).await;
    assert!(view.raw_config.contains("lxc.cgroup2.cpu.max"));
    let mut opts = view.lxc.clone();
    opts.swap_mb = 32;
    let (st, b) = lxc_send(
        &c,
        &token,
        Method::PUT,
        &format!("/containers/{ct}/lxc/config"),
        Some(json!({"memory": 160, "cpus": null, "lxc": opts})),
    )
    .await;
    let view: api::LxcConfigView = lxc_decode("PUT lxc/config", st, &b);
    assert_eq!((view.memory, view.lxc.swap_mb), (160, 32));

    for (path, what) in [("start", "start"), ("pause", "pause"), ("resume", "resume")] {
        let (st, b) = lxc_send(
            &c,
            &token,
            Method::POST,
            &format!("/containers/{ct}/{path}"),
            Some(json!({})),
        )
        .await;
        let _: horcrux_common::ContainerConfig = lxc_decode(what, st, &b);
    }
    let (st, b) = lxc_send(
        &c,
        &token,
        Method::POST,
        &format!("/containers/{ct}/snapshots"),
        Some(json!({"name": "c1", "comment": null})),
    )
    .await;
    let snap: api::LxcSnapshot = lxc_decode("POST snapshots", st, &b);
    let snaps: Vec<api::LxcSnapshot> =
        fetch(&c, &token, &format!("/containers/{ct}/snapshots")).await;
    assert!(snaps.iter().any(|s| s.name == snap.name));
    let (st, b) = lxc_send(
        &c,
        &token,
        Method::POST,
        &format!("/containers/{ct}/snapshots/c1/rollback"),
        Some(json!({})),
    )
    .await;
    let _: horcrux_common::ContainerConfig = lxc_decode("rollback", st, &b);
    let (st, b) = lxc_send(
        &c,
        &token,
        Method::DELETE,
        &format!("/containers/{ct}/snapshots/c1"),
        None,
    )
    .await;
    assert_eq!(st, 204, "{b}");
    let (st, b) = lxc_send(
        &c,
        &token,
        Method::POST,
        &format!("/containers/{ct}/reboot"),
        Some(json!({})),
    )
    .await;
    let _: horcrux_common::ContainerConfig = lxc_decode("reboot", st, &b);
    let (st, b) = lxc_send(
        &c,
        &token,
        Method::POST,
        &format!("/containers/{ct}/resize"),
        Some(json!({"disk_gb": 2})),
    )
    .await;
    let view: api::LxcConfigView = lxc_decode("resize", st, &b);
    assert_eq!(view.lxc.disk_gb, Some(2));
    let (st, b) = lxc_send(
        &c,
        &token,
        Method::POST,
        &format!("/containers/{ct}/shutdown"),
        Some(json!({"timeout": 30, "force": true})),
    )
    .await;
    let stopped: horcrux_common::ContainerConfig = lxc_decode("shutdown", st, &b);
    assert_eq!(stopped.status, horcrux_common::ContainerStatus::Stopped);
    let (st, b) = lxc_send(&c, &token, Method::POST, &format!("/containers/{ct}/clone"), Some(json!({"target_id": "contract-lxc2", "target_name": "contract-lxc2", "snapshot": false}))).await;
    let _: horcrux_common::ContainerConfig = lxc_decode("clone", st, &b);
    for id in [ct, "contract-lxc2"] {
        let (st, b) = lxc_send(
            &c,
            &token,
            Method::DELETE,
            &format!("/containers/{id}"),
            None,
        )
        .await;
        assert_eq!(st, 204, "delete {id}: {b}");
    }
    let (st, b) = lxc_send(
        &c,
        &token,
        Method::DELETE,
        &api::lxc_template_path(&tmpl.id),
        None,
    )
    .await;
    assert_eq!(st, 204, "delete template: {b}");
}

/// LXD/Incus/LXC container consoles: the console-ticket route the UI's
/// Console page calls (`api::get_container_console_ticket`), read into the
/// UI's ticket type. Docker/Podman have no system console (clear 422, never
/// a "use the CLI" hint). incus_e2e.sh runs this again on a server next to
/// a real Incus daemon; the console sessions themselves are driven there by
/// incus_e2e.py.
#[tokio::test]
async fn instance_console_ticket_route_reads_into_ui_type() {
    let c = client();
    let token = login(&c).await;
    let feats: Value = fetch(&c, &token, "/features").await;
    let on = |f: &str| {
        feats["features"]
            .as_array()
            .is_some_and(|a| a.iter().any(|v| v == f))
    };
    let ct = std::env::var("HORCRUX_CONTRACT_INCUS_CT").unwrap_or_else(|_| "no-such-ct".into());
    for rt in ["lxc", "lxd", "incus"] {
        let (status, body) = post(
            &c,
            &token,
            &format!("/containers/{ct}/console-ticket?runtime={rt}"),
            json!({}),
        )
        .await;
        if !on(rt) {
            assert_eq!(status, 422, "{rt} not compiled in: {body}");
            continue;
        }
        assert_eq!(status, 200, "{rt} console ticket: {body}");
        let t: api::WsTicketResponse = serde_json::from_value(body).unwrap();
        assert!(
            t.ws_path.starts_with("/api/containers/ws/console/"),
            "{}",
            t.ws_path
        );
        assert!(t.ws_path.ends_with(&t.ticket));
    }
    let (status, body) = post(
        &c,
        &token,
        &format!("/containers/{ct}/console-ticket?runtime=docker"),
        json!({}),
    )
    .await;
    assert_eq!(status, 422, "docker has no system console: {body}");
    let text = body.to_string();
    for hint in ["lxc console", "incus console", "docker attach"] {
        assert!(!text.contains(hint), "CLI hint in {text}");
    }
}

/// HA > Migration Center: the page lists, starts and cancels migrations
/// through /migration/jobs (+ /migration/bulk), which used to answer 404
/// ("Failed to load migration jobs: HTTP 404"). Starts a real migration job
/// for a real VM to a registered cluster node at a TEST-NET address that
/// never answers (the transfer itself fails or is cancelled - nothing
/// leaves the runner; real two-node migrations are tests/vm_migrate_e2e.sh)
/// and reads every response into the page's own types.
#[tokio::test]
async fn migration_center_routes_read_into_ui_types() {
    let c = client();
    let token = login(&c).await;
    let _: Vec<api::MigrationJob> = fetch(&c, &token, "/migration/jobs").await;

    let vm = "contract-migrate-vm";
    delete(&c, &token, &format!("/vms/{vm}")).await;
    // The target must be a node of the cluster (192.0.2.0/24 = TEST-NET-1).
    let _ = post(
        &c,
        &token,
        "/cluster/nodes/contract-target",
        json!({"ip": "192.0.2.10"}),
    )
    .await;
    let (status, body) = post(
        &c,
        &token,
        "/vms",
        json!({"id": vm, "name": vm, "hypervisor": "qemu", "architecture": "x86_64",
               "cpus": 1, "memory": 128, "disk_size": 1, "status": "stopped"}),
    )
    .await;
    assert!(status == 200 || status == 201, "create vm: {status} {body}");

    let job = |rtype: &str, rid: &str| api::MigrationJob {
        job_id: String::new(),
        resource_type: rtype.into(),
        resource_id: rid.into(),
        source_node: String::new(),
        target_node: "contract-target".into(),
        migration_type: "offline".into(),
        status: "pending".into(),
        progress: 0,
        start_time: chrono::Utc::now(),
        end_time: None,
        error: None,
        bandwidth_limit: None,
        timeout: 300,
        force: false,
        with_local_disks: false,
        estimated_duration: None,
        transferred_bytes: 0,
        remaining_bytes: None,
    };

    // containers aren't migratable: a clear 400, not a 404/500
    let (status, body) = post(
        &c,
        &token,
        "/migration/jobs",
        serde_json::to_value(job("container", "contract-ct")).unwrap(),
    )
    .await;
    assert_eq!(status, 400, "container migration: {body}");
    // unknown VM: 404
    let (status, body) = post(
        &c,
        &token,
        "/migration/jobs",
        serde_json::to_value(job("vm", "no-such-vm-for-migration")).unwrap(),
    )
    .await;
    assert_eq!(status, 404, "unknown vm: {body}");
    // a target that isn't a cluster node: 400 naming it
    let mut not_node = job("vm", vm);
    not_node.target_node = "no-such-node".into();
    let (status, body) = post(
        &c,
        &token,
        "/migration/jobs",
        serde_json::to_value(not_node).unwrap(),
    )
    .await;
    assert_eq!(status, 400, "unknown target: {body}");
    assert!(body.to_string().contains("no-such-node"), "{body}");
    // the peer API refuses user logins (only nodes' migration tokens)
    let (status, body) = post(&c, &token, "/migration/peer/probe", json!({"files": []})).await;
    assert_eq!(status, 403, "peer API with a user token: {body}");
    let (status, body) = post(&c, &token, "/migration/peer/j1/keepalive", json!({})).await;
    assert_eq!(status, 403, "peer keepalive with a user token: {body}");

    // start (one migration at a time: wait out a job another run left)
    let mut created = None;
    for _ in 0..30 {
        let (status, body) = post(
            &c,
            &token,
            "/migration/jobs",
            serde_json::to_value(job("vm", vm)).unwrap(),
        )
        .await;
        if status == 409 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            continue;
        }
        assert_eq!(status, 201, "start migration: {body}");
        created = Some(
            serde_json::from_value::<api::MigrationJob>(body).expect("UI MigrationJob from create"),
        );
        break;
    }
    let created = created.expect("migration slot never freed");
    assert_eq!(created.resource_id, vm);
    assert_eq!(created.resource_type, "vm");
    assert_eq!(created.target_node, "contract-target");
    assert!(!created.job_id.is_empty());

    // listed
    let jobs: Vec<api::MigrationJob> = fetch(&c, &token, "/migration/jobs").await;
    assert!(
        jobs.iter().any(|j| j.job_id == created.job_id),
        "new job not listed: {jobs:?}"
    );

    // cancel: 204 while it runs, 409 if the transfer already failed
    let (status, body) = send_json(
        &c,
        &token,
        reqwest::Method::DELETE,
        &format!("/migration/jobs/{}", created.job_id),
        None,
    )
    .await;
    assert!(status == 204 || status == 409, "cancel: {status} {body}");
    tokio::time::sleep(Duration::from_secs(2)).await;
    let jobs: Vec<api::MigrationJob> = fetch(&c, &token, "/migration/jobs").await;
    let mine = jobs
        .iter()
        .find(|j| j.job_id == created.job_id)
        .expect("job still listed");
    if status == 204 {
        assert_eq!(mine.status, "cancelled", "{mine:?}");
        assert!(mine.end_time.is_some());
    } else {
        assert_eq!(mine.status, "failed", "{mine:?}");
    }
    let (status, _) = send_json(
        &c,
        &token,
        reqwest::Method::DELETE,
        "/migration/jobs/no-such-job",
        None,
    )
    .await;
    assert_eq!(status, 404);

    // bulk: the page's BulkMigrationJob round-trips
    let mut bulk_ok = false;
    for _ in 0..30 {
        let bulk = api::BulkMigrationJob {
            job_id: String::new(),
            target_node: "contract-target".into(),
            resources: vec![("vm".into(), vm.into())],
            max_workers: 1,
            migration_type: "offline".into(),
            status: "pending".into(),
            start_time: chrono::Utc::now(),
            end_time: None,
            completed_count: 0,
            failed_count: 0,
            total_count: 1,
        };
        let (status, body) = post(
            &c,
            &token,
            "/migration/bulk",
            serde_json::to_value(&bulk).unwrap(),
        )
        .await;
        if (status == 400 || status == 409)
            && (body.to_string().contains("Maximum concurrent")
                || body.to_string().contains("already being migrated"))
        {
            tokio::time::sleep(Duration::from_secs(1)).await;
            continue;
        }
        assert_eq!(status, 201, "bulk: {body}");
        let b: api::BulkMigrationJob = serde_json::from_value(body).expect("UI BulkMigrationJob");
        assert_eq!(b.total_count, 1);
        assert_eq!(b.failed_count, 0);
        bulk_ok = true;
        break;
    }
    assert!(bulk_ok, "bulk migration never got a slot");
    let jobs: Vec<api::MigrationJob> = fetch(&c, &token, "/migration/jobs").await;
    for j in jobs.iter().filter(|j| j.resource_id == vm) {
        let _ = send_json(
            &c,
            &token,
            reqwest::Method::DELETE,
            &format!("/migration/jobs/{}", j.job_id),
            None,
        )
        .await;
    }
    delete(&c, &token, &format!("/vms/{vm}")).await;
    let _ = send_json(
        &c,
        &token,
        reqwest::Method::DELETE,
        "/cluster/nodes/contract-target",
        None,
    )
    .await;
}

async fn put(c: &reqwest::Client, token: &str, path: &str, body: Value) -> (u16, String) {
    let r = c
        .put(format!("{}{}", api_base(), path))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .unwrap();
    (r.status().as_u16(), r.text().await.unwrap_or_default())
}

/// This host's primary non-loopback address. The API refuses to download
/// from the node's own loopback, so the ISO "mirror" listens here. (A UDP
/// connect only picks the source address; nothing is sent.)
fn mirror_ip() -> std::net::IpAddr {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    s.connect("192.0.2.1:9")
        .expect("ISO mirror needs a non-loopback address (default route)");
    s.local_addr().unwrap().ip()
}

/// Serve `body` once per request on [`mirror_ip`] (an ISO "mirror").
async fn file_server(body: Vec<u8>) -> u16 {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind((mirror_ip(), 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                return;
            };
            let body = body.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf).await;
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = s.write_all(head.as_bytes()).await;
                let _ = s.write_all(&body).await;
            });
        }
    });
    port
}

/// VM hardware (NICs, CD-ROM, boot order), the ISO library and host bridges:
/// every route the Create VM form, the VM Hardware page, Infrastructure >
/// ISO Images and Host Bridges call, read into the UI's own types against
/// the real server. (Real tap devices, VLANs, PXE traffic and live CD-ROM
/// changes in a booted guest are covered by tests/vm_nic_e2e.sh.)
#[tokio::test]
async fn vm_hardware_iso_library_and_bridges_read_into_ui_types() {
    let c = client();
    let token = login(&c).await;
    let opts: api::VmHostOptions = fetch(&c, &token, "/vms/host-options").await;
    let _: Vec<api::BridgeInfo> = fetch(&c, &token, "/network/bridges").await;

    // ISO library: download from a URL -> listed as downloading, then ready.
    let iso_bytes = vec![0x5au8; 300_000];
    let port = file_server(iso_bytes.clone()).await;
    let mirror = mirror_ip();
    let name = format!("contract-{}.iso", std::process::id());
    delete(&c, &token, &format!("/storage/isos/{name}")).await;
    // Never from the node's own loopback (its local-only services).
    let (status, body) = post(
        &c,
        &token,
        "/storage/isos/download",
        json!({"url": format!("http://127.0.0.1:{port}/files/whatever.iso"), "filename": name}),
    )
    .await;
    assert_eq!(status, 403, "loopback download: {body}");
    assert!(body.to_string().contains("refusing"), "{body}");
    let (status, body) = post(
        &c,
        &token,
        "/storage/isos/download",
        json!({"url": format!("http://{mirror}:{port}/files/whatever.iso"), "filename": name}),
    )
    .await;
    assert_eq!(status, 202, "download: {body}");
    let started: api::IsoFile = serde_json::from_value(body).unwrap();
    assert!(started.downloading);
    let mut ready = None;
    for _ in 0..60 {
        let list: Vec<api::IsoFile> = fetch(&c, &token, "/storage/isos").await;
        if let Some(i) = list.iter().find(|i| i.name == name && !i.downloading) {
            assert!(i.error.is_none(), "download failed: {:?}", i.error);
            ready = Some(i.clone());
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let iso = ready.expect("ISO download never completed");
    assert_eq!(iso.size, iso_bytes.len() as u64);
    assert!(
        iso.path.starts_with(&opts.iso_dir),
        "{} not in {}",
        iso.path,
        opts.iso_dir
    );
    // a second download of the same name is refused
    let (status, _) = post(
        &c,
        &token,
        "/storage/isos/download",
        json!({"url": format!("http://{mirror}:{port}/x.iso"), "filename": name}),
    )
    .await;
    assert_eq!(status, 409);
    let (status, _) = post(
        &c,
        &token,
        "/storage/isos/download",
        json!({"url": "http://127.0.0.1:1/x.iso", "filename": "../etc.iso"}),
    )
    .await;
    assert_eq!(status, 400);

    // VM with the ISO in its CD-ROM and CD-ROM-first boot order.
    let vm = "contract-hw-vm";
    delete(&c, &token, &format!("/vms/{vm}")).await;
    let (status, body) = post(
        &c,
        &token,
        "/vms",
        json!({"id": vm, "name": vm, "hypervisor": "qemu", "architecture": "x86_64",
               "cpus": 1, "memory": 128, "disk_size": 1, "status": "stopped",
               "hardware": {"cdrom": iso.path, "boot": ["cdrom", "disk"]}}),
    )
    .await;
    assert!(status == 200 || status == 201, "create vm: {status} {body}");
    let created: horcrux_common::VmConfig = serde_json::from_value(body).unwrap();
    assert_eq!(created.hardware.cdrom.as_deref(), Some(iso.path.as_str()));

    // A NIC on a bridge this host doesn't have is refused with the reason.
    let (status, body) = post(
        &c,
        &token,
        "/vms",
        json!({"id": "contract-hw-bad", "name": "bad", "hypervisor": "qemu", "cpus": 1,
               "memory": 128, "disk_size": 1, "status": "stopped",
               "hardware": {"nets": [{"bridge": "nosuchbr0"}]}}),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert!(body.to_string().contains("nosuchbr0"), "{body}");
    // ...and so is an ISO outside the ISO library.
    let (status, _) = put(
        &c,
        &token,
        &format!("/vms/{vm}/config"),
        json!({"hardware": {"cdrom": "/etc/hostname"}}),
    )
    .await;
    assert!(status == 403 || status == 400, "foreign cdrom: {status}");

    // Edit through the UI's own request type.
    let mut hw = created.hardware.clone();
    if let Some(b) = opts.bridges.first() {
        hw.nets.push(horcrux_common::VmNic {
            model: horcrux_common::NicModel::E1000,
            bridge: b.name.clone(),
            mac: None,
            vlan: None,
        });
    }
    let upd = api::VmConfigUpdate {
        name: Some("contract-hw-renamed".into()),
        cpus: Some(2),
        memory: Some(256),
        hardware: Some(hw.clone()),
    };
    let (status, body) = put(
        &c,
        &token,
        &format!("/vms/{vm}/config"),
        serde_json::to_value(&upd).unwrap(),
    )
    .await;
    assert_eq!(status, 200, "update config: {body}");
    let updated: horcrux_common::VmConfig = decode("PUT /vms/:id/config", &body);
    assert_eq!((updated.cpus, updated.memory), (2, 256));
    assert_eq!(updated.name, "contract-hw-renamed");
    for n in &updated.hardware.nets {
        assert!(n.mac.is_some(), "MAC not generated: {:?}", n);
    }
    let got: horcrux_common::VmConfig = fetch(&c, &token, &format!("/vms/{vm}")).await;
    assert_eq!(got.hardware, updated.hardware);

    // VM firewall list (adding rules needs nftables + CAP_NET_ADMIN, which
    // this unprivileged job lacks: tests/vm_nic_e2e.sh covers it for real).
    let fw: Vec<api::GuestFirewallRule> =
        fetch(&c, &token, &format!("/vms/{vm}/firewall/rules")).await;
    assert!(fw.is_empty());
    let (status, _) = get_raw(&c, &token, "/vms/no-such-vm-fw/firewall/rules").await;
    assert_eq!(status, 404);

    // System settings round-trip (CPU type/sockets, q35, UEFI/TPM when the
    // host has OVMF/swtpm - the host options say so).
    let mut sys = updated.hardware.clone();
    sys.cpu_type = Some("qemu64".into());
    sys.sockets = Some(2);
    sys.machine = horcrux_common::MachineType::Q35;
    if opts.ovmf {
        sys.firmware = horcrux_common::Firmware::Ovmf;
    }
    let (status, body) = put(
        &c,
        &token,
        &format!("/vms/{vm}/config"),
        serde_json::to_value(&api::VmConfigUpdate {
            hardware: Some(sys.clone()),
            ..Default::default()
        })
        .unwrap(),
    )
    .await;
    assert_eq!(status, 200, "system settings: {body}");
    let got: horcrux_common::VmConfig = decode("PUT /vms/:id/config (system)", &body);
    assert_eq!(got.hardware.cpu_type.as_deref(), Some("qemu64"));
    assert_eq!(got.hardware.machine, horcrux_common::MachineType::Q35);
    // 2 CPUs over 3 sockets can't work.
    let mut bad = sys.clone();
    bad.sockets = Some(3);
    let (status, _) = put(
        &c,
        &token,
        &format!("/vms/{vm}/config"),
        json!({"hardware": serde_json::to_value(&bad).unwrap()}),
    )
    .await;
    assert!(status == 400 || status == 422, "bad sockets: {status}");
    if !opts.swtpm {
        let mut tpm = sys.clone();
        tpm.tpm = true;
        let (status, body) = put(
            &c,
            &token,
            &format!("/vms/{vm}/config"),
            json!({"hardware": serde_json::to_value(&tpm).unwrap()}),
        )
        .await;
        assert_eq!(status, 400, "TPM without swtpm: {body}");
    }

    // Disks: add one through the UI's request type, grow it, change its
    // bus, detach it (deleting the image).
    let spec = api::VmDiskSpec {
        size_gb: 1,
        bus: "scsi".into(),
        cache: "none".into(),
        discard: true,
        ssd: true,
        iothread: true,
        storage: None,
    };
    let (status, body) = post(
        &c,
        &token,
        &format!("/vms/{vm}/disks"),
        serde_json::to_value(&spec).unwrap(),
    )
    .await;
    assert_eq!(status, 201, "add disk: {body}");
    let with_disk: horcrux_common::VmConfig = serde_json::from_value(body).unwrap();
    assert_eq!(with_disk.disks.len(), 2, "{:?}", with_disk.disks);
    let d1 = with_disk.disks[1].clone();
    assert!(
        d1.path.ends_with(&format!("{vm}-disk1.qcow2")),
        "{}",
        d1.path
    );
    assert!(
        std::path::Path::new(&d1.path).exists(),
        "{} not created",
        d1.path
    );
    assert!((d1.discard, d1.ssd, d1.iothread) == (true, true, true));
    let (status, body) = put(
        &c,
        &token,
        &format!("/vms/{vm}/disks/1"),
        serde_json::to_value(&api::VmDiskUpdate {
            size_gb: Some(2),
            bus: Some("sata".into()),
            iothread: Some(false),
            ..Default::default()
        })
        .unwrap(),
    )
    .await;
    assert_eq!(status, 200, "update disk: {body}");
    let upd_disk: horcrux_common::VmConfig = decode("PUT /vms/:id/disks/1", &body);
    assert_eq!(
        (
            upd_disk.disks[1].size_gb,
            upd_disk.disks[1].disk_type.as_str()
        ),
        (2, "sata")
    );
    // shrinking is refused; disk0 can't be detached; bad bus refused
    let (status, _) = put(
        &c,
        &token,
        &format!("/vms/{vm}/disks/1"),
        json!({"size_gb": 1}),
    )
    .await;
    assert!(status == 400 || status == 422, "shrink: {status}");
    let (status, _) = post(
        &c,
        &token,
        &format!("/vms/{vm}/disks"),
        json!({"size_gb": 1, "bus": "floppy"}),
    )
    .await;
    assert!(status == 400 || status == 422, "bad bus: {status}");
    let r = c
        .delete(format!("{}/vms/{vm}/disks/0", api_base()))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert!(
        matches!(r.status().as_u16(), 400 | 422),
        "detach disk0: {}",
        r.status()
    );
    let r = c
        .delete(format!("{}/vms/{vm}/disks/1?delete_file=true", api_base()))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200);
    let after: horcrux_common::VmConfig = fetch(&c, &token, &format!("/vms/{vm}")).await;
    assert_eq!(after.disks.len(), 1);
    assert!(
        !std::path::Path::new(&d1.path).exists(),
        "image not deleted"
    );

    // Container firewall list for an unknown container: 404.
    let (status, _) = get_raw(&c, &token, "/containers/no-such-ct-fw/firewall/rules").await;
    assert_eq!(status, 404);

    // The ISO can't be deleted while a VM's CD-ROM holds it.
    let r = c
        .delete(format!("{}/storage/isos/{}", api_base(), name))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 409);

    // Power actions on a stopped VM answer 409, not 404/500.
    for action in ["shutdown", "reset"] {
        let (status, body) = post(&c, &token, &format!("/vms/{vm}/{action}"), json!({})).await;
        assert_eq!(status, 409, "{action} on a stopped VM: {body}");
    }

    // Eject, then the ISO deletes.
    hw.cdrom = None;
    let (status, body) = put(
        &c,
        &token,
        &format!("/vms/{vm}/config"),
        json!({"hardware": serde_json::to_value(&hw).unwrap()}),
    )
    .await;
    assert_eq!(status, 200, "eject: {body}");
    delete(&c, &token, &format!("/vms/{vm}")).await;
    let r = c
        .delete(format!("{}/storage/isos/{}", api_base(), name))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 204);
    let list: Vec<api::IsoFile> = fetch(&c, &token, "/storage/isos").await;
    assert!(!list.iter().any(|i| i.name == name));
}

// ---------------------------------------------------------------------------
// NAS > S3 Gateway against a real MinIO-compatible server
// ---------------------------------------------------------------------------

/// A free TCP port on 127.0.0.1.
fn free_tcp_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// An S3 request signed with SigV4 by curl (an S3 client independent of the
/// MinIO client the API uses): returns (HTTP status, body).
fn s3_curl(
    method: &str,
    url: &str,
    region: &str,
    access: &str,
    secret: &str,
    upload: Option<&std::path::Path>,
) -> (u16, Vec<u8>) {
    let out = std::env::temp_dir().join(format!("hx-s3-body-{}", std::process::id()));
    let mut cmd = std::process::Command::new("curl");
    cmd.args(["-s", "-o"])
        .arg(&out)
        .args(["-w", "%{http_code}", "-X", method])
        .args(["--aws-sigv4", &format!("aws:amz:{region}:s3")])
        .args(["--user", &format!("{access}:{secret}")]);
    if let Some(f) = upload {
        cmd.arg("-T").arg(f);
    }
    cmd.arg(url);
    let o = cmd.output().expect("curl is installed");
    let code = String::from_utf8_lossy(&o.stdout)
        .trim()
        .parse()
        .unwrap_or(0);
    let body = std::fs::read(&out).unwrap_or_default();
    let _ = std::fs::remove_file(&out);
    (code, body)
}

async fn send_bytes(
    c: &reqwest::Client,
    token: &str,
    method: reqwest::Method,
    path: &str,
    body: Option<Vec<u8>>,
) -> (u16, Vec<u8>) {
    let mut r = c
        .request(method, format!("{}{}", api_base(), path))
        .bearer_auth(token);
    if let Some(b) = body {
        r = r.body(b);
    }
    let r = r.send().await.unwrap();
    let status = r.status().as_u16();
    (status, r.bytes().await.unwrap_or_default().to_vec())
}

/// The S3 Gateway page end to end against the real server (MinIO or Silo +
/// the MinIO client installed on the CI runner): configure, start, bucket,
/// S3 user, access key, objects written and read with that key by an
/// independent SigV4 client, rotate (old secret refused), disable/enable,
/// upload/download/delete through the API, delete key/user/bucket, stop.
/// Every response is read into the page's own types.
#[tokio::test]
async fn s3_gateway_drives_a_real_minio() {
    let c = client();
    let token = login(&c).await;
    let (code, body) = get_raw(&c, &token, "/nas/s3/status").await;
    if code == 404 {
        assert!(
            std::env::var("HORCRUX_TEST_REQUIRE_S3").is_err(),
            "HORCRUX_TEST_REQUIRE_S3 is set but the server has no S3 gateway routes (built without USE=s3-gateway?)"
        );
        eprintln!("server built without s3-gateway: skipping");
        return;
    }
    let st: api::s3::S3GatewayStatus = decode("S3 status", &body);
    if !st.missing.is_empty() {
        assert!(
            std::env::var("HORCRUX_TEST_REQUIRE_S3").is_err(),
            "HORCRUX_TEST_REQUIRE_S3 is set but the server reports missing: {:?}",
            st.missing
        );
        eprintln!(
            "S3 server/client not installed ({:?}): skipping",
            st.missing
        );
        return;
    }

    // a clean, stopped gateway on private ports
    let (code, body) = post(&c, &token, "/nas/s3/stop", json!({})).await;
    assert_eq!(code, 200, "stop: {body}");
    let port = free_tcp_port();
    let cport = free_tcp_port();
    let (code, body) = put(
        &c,
        &token,
        "/nas/s3/config",
        json!({"address": "127.0.0.1", "port": port, "console_port": cport}),
    )
    .await;
    assert_eq!(code, 200, "config: {body}");
    let settings: api::s3::S3GatewaySettings = decode("S3 settings", &body);
    assert_eq!(settings.port, Some(port));
    let (code, _) = put(
        &c,
        &token,
        "/nas/s3/config",
        json!({"port": port, "console_port": port}),
    )
    .await;
    assert_eq!(code, 422, "same API and console port is refused");

    // start
    let (code, body) = post(&c, &token, "/nas/s3/start", json!({})).await;
    assert_eq!(code, 200, "start: {body}");
    let st: api::s3::S3GatewayStatus = serde_json::from_value(body).unwrap();
    assert!(st.running && st.enabled && st.pid.is_some(), "{st:?}");
    assert_eq!(st.port, port);
    let region = st.region.clone();
    let ep = format!("http://127.0.0.1:{port}");

    // bucket
    let bucket = format!("hx-contract-{}", std::process::id());
    let (code, body) = post(&c, &token, "/nas/s3/buckets", json!({"name": "Bad_Name"})).await;
    assert_eq!(code, 422, "invalid bucket name refused: {body}");
    let (code, body) = post(
        &c,
        &token,
        "/nas/s3/buckets",
        json!({"name": bucket, "versioning": true}),
    )
    .await;
    assert_eq!(code, 201, "create bucket: {body}");
    let created: api::s3::S3Bucket = serde_json::from_value(body).unwrap();
    assert_eq!(created.name, bucket);
    let (code, _) = post(&c, &token, "/nas/s3/buckets", json!({"name": bucket})).await;
    assert_eq!(code, 409, "duplicate bucket");
    let list: Vec<api::s3::S3Bucket> = fetch(&c, &token, "/nas/s3/buckets").await;
    let b = list
        .iter()
        .find(|b| b.name == bucket)
        .expect("bucket listed");
    assert!(b.versioning, "versioning enabled: {b:?}");

    // S3 user + an access key for it
    let user = format!("ct-user-{}", std::process::id());
    let (code, body) = post(
        &c,
        &token,
        "/nas/s3/users",
        json!({"username": user, "policy": "readwrite"}),
    )
    .await;
    assert_eq!(code, 201, "create user: {body}");
    let u: api::s3::S3User = serde_json::from_value(body).unwrap();
    assert!(u
        .secret_key
        .as_deref()
        .map(|s| s.len() >= 8)
        .unwrap_or(false));
    let users: Vec<api::s3::S3User> = fetch(&c, &token, "/nas/s3/users").await;
    assert!(users
        .iter()
        .any(|x| x.username == user && x.policies.contains(&"readwrite".to_string())));

    let (code, body) = post(
        &c,
        &token,
        "/nas/s3/access-keys",
        json!({"user_id": user, "description": "contract test"}),
    )
    .await;
    assert_eq!(code, 201, "create key: {body}");
    let key: api::s3::S3AccessKey = serde_json::from_value(body).unwrap();
    let ak = key.access_key.clone();
    let sk = key.secret_key.clone().expect("secret returned once");
    assert!(key.managed && key.enabled);
    let keys: Vec<api::s3::S3AccessKey> = fetch(&c, &token, "/nas/s3/access-keys").await;
    let listed = keys
        .iter()
        .find(|k| k.access_key == ak)
        .expect("key listed");
    assert_eq!(listed.user_id, user);
    assert!(listed.secret_key.is_none(), "list never returns secrets");
    let (code, _) = post(
        &c,
        &token,
        "/nas/s3/access-keys",
        json!({"user_id": "nobody-here"}),
    )
    .await;
    assert_eq!(code, 404, "key for an unknown user");

    // put + get an object with the key (curl SigV4, not the MinIO client)
    let payload = format!("horcrux s3 contract {}\n", std::process::id());
    let file = std::env::temp_dir().join(format!("hx-s3-up-{}", std::process::id()));
    std::fs::write(&file, &payload).unwrap();
    let url = format!("{ep}/{bucket}/dir/hello.txt");
    let (code, body) = s3_curl("PUT", &url, &region, &ak, &sk, Some(&file));
    assert_eq!(
        code,
        200,
        "PUT with the new key: {}",
        String::from_utf8_lossy(&body)
    );
    let (code, body) = s3_curl("GET", &url, &region, &ak, &sk, None);
    assert_eq!(
        (code, String::from_utf8_lossy(&body).to_string()),
        (200, payload.clone())
    );

    // the page sees it and downloads the same bytes
    let objs: Vec<api::s3::S3Object> =
        fetch(&c, &token, &format!("/nas/s3/buckets/{bucket}/objects")).await;
    let o = objs
        .iter()
        .find(|o| o.key == "dir/hello.txt")
        .expect("object listed");
    assert_eq!(o.size, payload.len() as u64);
    let (code, bytes) = send_bytes(
        &c,
        &token,
        reqwest::Method::GET,
        &format!("/nas/s3/buckets/{bucket}/objects/download?key=dir%2Fhello.txt"),
        None,
    )
    .await;
    assert_eq!((code, bytes), (200, payload.clone().into_bytes()));
    let st: api::s3::S3GatewayStatus = fetch(&c, &token, "/nas/s3/status").await;
    assert!(st.object_count >= 1 && st.bucket_count >= 1, "{st:?}");

    // rotate: the old secret stops working, the new one works
    let (code, body) = post(
        &c,
        &token,
        &format!("/nas/s3/access-keys/{ak}/rotate"),
        json!({}),
    )
    .await;
    assert_eq!(code, 200, "rotate: {body}");
    let rotated: api::s3::S3AccessKey = serde_json::from_value(body).unwrap();
    assert_eq!(rotated.access_key, ak);
    let sk2 = rotated.secret_key.expect("new secret");
    assert_ne!(sk2, sk);
    let (code, _) = s3_curl("GET", &url, &region, &ak, &sk, None);
    assert_eq!(code, 403, "old secret refused after rotate");
    let (code, _) = s3_curl("GET", &url, &region, &ak, &sk2, None);
    assert_eq!(code, 200, "new secret works");

    // disable / enable
    let (code, body) = put(
        &c,
        &token,
        &format!("/nas/s3/access-keys/{ak}"),
        json!({"enabled": false}),
    )
    .await;
    assert_eq!(code, 200, "disable: {body}");
    let k: api::s3::S3AccessKey = decode("disabled key", &body);
    assert!(!k.enabled);
    let (code, _) = s3_curl("GET", &url, &region, &ak, &sk2, None);
    assert_eq!(code, 403, "disabled key refused");
    let (code, body) = put(
        &c,
        &token,
        &format!("/nas/s3/access-keys/{ak}"),
        json!({"enabled": true}),
    )
    .await;
    assert_eq!(code, 200, "enable: {body}");
    let (code, _) = s3_curl("GET", &url, &region, &ak, &sk2, None);
    assert_eq!(code, 200, "re-enabled key works");

    // upload through the API (the page's file upload), read with the key
    let blob: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let (code, body) = send_bytes(
        &c,
        &token,
        reqwest::Method::PUT,
        &format!("/nas/s3/buckets/{bucket}/objects?key=api%2Fup.bin"),
        Some(blob.clone()),
    )
    .await;
    assert_eq!(code, 201, "API upload: {}", String::from_utf8_lossy(&body));
    let (code, got) = s3_curl(
        "GET",
        &format!("{ep}/{bucket}/api/up.bin"),
        &region,
        &ak,
        &sk2,
        None,
    );
    assert_eq!(code, 200);
    assert!(
        got == blob,
        "uploaded bytes round-trip ({} vs {})",
        got.len(),
        blob.len()
    );
    let (code, _) = send_bytes(
        &c,
        &token,
        reqwest::Method::DELETE,
        &format!("/nas/s3/buckets/{bucket}/objects?key=api%2Fup.bin"),
        None,
    )
    .await;
    assert_eq!(code, 204);
    let (code, _) = s3_curl(
        "GET",
        &format!("{ep}/{bucket}/api/up.bin"),
        &region,
        &ak,
        &sk2,
        None,
    );
    assert_eq!(code, 404, "deleted object is gone");

    // delete the key: refused at once, gone from the list
    let (code, _) = send_bytes(
        &c,
        &token,
        reqwest::Method::DELETE,
        &format!("/nas/s3/access-keys/{ak}"),
        None,
    )
    .await;
    assert_eq!(code, 204);
    let (code, _) = s3_curl("GET", &url, &region, &ak, &sk2, None);
    assert_eq!(code, 403, "deleted key refused");
    let keys: Vec<api::s3::S3AccessKey> = fetch(&c, &token, "/nas/s3/access-keys").await;
    assert!(!keys.iter().any(|k| k.access_key == ak));

    // a non-empty bucket needs force; then user, bucket
    let (code, _) = send_bytes(
        &c,
        &token,
        reqwest::Method::DELETE,
        &format!("/nas/s3/buckets/{bucket}"),
        None,
    )
    .await;
    assert_eq!(code, 409, "non-empty bucket needs force");
    let (code, _) = send_bytes(
        &c,
        &token,
        reqwest::Method::DELETE,
        &format!("/nas/s3/users/{user}"),
        None,
    )
    .await;
    assert_eq!(code, 204);
    let (code, _) = send_bytes(
        &c,
        &token,
        reqwest::Method::DELETE,
        &format!("/nas/s3/buckets/{bucket}?force=true"),
        None,
    )
    .await;
    assert_eq!(code, 204);
    let list: Vec<api::s3::S3Bucket> = fetch(&c, &token, "/nas/s3/buckets").await;
    assert!(!list.iter().any(|b| b.name == bucket));

    // stop: the server process and its port are gone
    let (code, body) = post(&c, &token, "/nas/s3/stop", json!({})).await;
    assert_eq!(code, 200, "stop: {body}");
    let st: api::s3::S3GatewayStatus = serde_json::from_value(body).unwrap();
    assert!(!st.running && !st.enabled && st.pid.is_none(), "{st:?}");
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", port)).is_err(),
        "S3 port still open after stop"
    );
    let (code, _) = post(&c, &token, "/nas/s3/buckets", json!({"name": "after-stop"})).await;
    assert_eq!(code, 409, "bucket ops need a running gateway");
    let _ = std::fs::remove_file(&file);
}

// ---------------------------------------------------------------------------
// Monitoring > Dashboards
// ---------------------------------------------------------------------------

/// User-defined dashboards: create (panels validated against the metrics
/// catalog), list/get/update/delete read into the page's types, and each
/// panel's query runs against the real metrics store.
#[tokio::test]
async fn dashboards_round_trip_and_panels_query_real_metrics() {
    let c = client();
    let token = login(&c).await;
    let (code, body) = post(
        &c,
        &token,
        "/monitoring/dashboards",
        json!({"name": "bad", "panels": [{"title": "x", "query": "no_such_metric", "chart": "line", "width": 1}]}),
    )
    .await;
    assert_eq!(code, 422, "unknown metric refused: {body}");

    let req = json!({
        "name": "Contract dashboard",
        "description": "made by api_contract",
        "time_range": "15m",
        "refresh_secs": 30,
        "panels": [
            {"title": "CPU", "query": "horcrux_node_cpu_usage_percent", "chart": "line", "width": 1},
            {"title": "Load", "query": "horcrux_node_load1", "chart": "stat", "width": 2}
        ]
    });
    let (code, body) = post(&c, &token, "/monitoring/dashboards", req).await;
    assert_eq!(code, 201, "create: {body}");
    let d: api::dashboards::Dashboard = serde_json::from_value(body).unwrap();
    assert_eq!(d.panels.len(), 2);
    assert!(d.panels.iter().all(|p| !p.id.is_empty()));
    assert_eq!(d.created_by, "admin");

    let list: Vec<api::dashboards::Dashboard> = fetch(&c, &token, "/monitoring/dashboards").await;
    assert!(list.iter().any(|x| x.id == d.id));
    let one: api::dashboards::Dashboard =
        fetch(&c, &token, &format!("/monitoring/dashboards/{}", d.id)).await;
    assert_eq!(one, d);

    // what the page does for each panel
    for p in &d.panels {
        let (code, body) = post(
            &c,
            &token,
            "/metrics/query",
            json!({"metric": p.query, "step": 15}),
        )
        .await;
        assert_eq!(code, 200, "panel query {}: {body}", p.query);
        let _: api::MetricResult = serde_json::from_value(body).unwrap();
    }

    let update = api::dashboards::DashboardRequest {
        name: "Contract dashboard v2".into(),
        description: String::new(),
        time_range: "6h".into(),
        refresh_secs: 0,
        panels: vec![d.panels[1].clone()],
    };
    let (code, body) = put(
        &c,
        &token,
        &format!("/monitoring/dashboards/{}", d.id),
        serde_json::to_value(&update).unwrap(),
    )
    .await;
    assert_eq!(code, 200, "update: {body}");
    let u: api::dashboards::Dashboard = decode("updated dashboard", &body);
    assert_eq!(
        (u.name.as_str(), u.time_range.as_str(), u.panels.len()),
        ("Contract dashboard v2", "6h", 1)
    );
    assert_eq!(u.panels[0].id, d.panels[1].id, "panel ids are kept");

    delete(&c, &token, &format!("/monitoring/dashboards/{}", d.id)).await;
    let (code, _) = get_raw(&c, &token, &format!("/monitoring/dashboards/{}", d.id)).await;
    assert_eq!(code, 404);
}

/// Networking > SDN: CNI networks, attachments (the UI's container picker
/// body), detach and the refusals the page shows. The CI Test job has no
/// CNI plugins, so the real attach runs in tests/sdn_cni_e2e.sh, which sets
/// HORCRUX_CONTRACT_SDN_NETNS to a real network namespace and runs this
/// test against its server.
#[tokio::test]
async fn sdn_cni_networks_and_attachments_read_into_ui_types() {
    use reqwest::Method;
    let c = client();
    let token = login(&c).await;
    let net = format!("contract-sdn-{}", std::process::id() % 100000);
    let _ = send_json(
        &c,
        &token,
        Method::DELETE,
        &format!("/cni/networks/{net}"),
        None,
    )
    .await;

    // The page's create form body; a bad subnet and a traversal name are 400s.
    let (code, body) = post(
        &c,
        &token,
        "/cni/networks",
        json!({"name": net, "plugin_type": "bridge", "bridge": "ctsdn0",
               "subnet": "10.252.9.0/24", "gateway": "10.252.9.1"}),
    )
    .await;
    assert_eq!(code, 200, "create: {body}");
    let created: api::sdn::CniNetwork = serde_json::from_value(body).expect("UI CniNetwork");
    assert_eq!(created.ipam.subnet.as_deref(), Some("10.252.9.0/24"));
    for bad in [
        json!({"name": "../../../tmp/contract-escape", "plugin_type": "bridge", "subnet": "10.252.8.0/24"}),
        json!({"name": "contract-bad", "plugin_type": "bridge", "subnet": "not-a-cidr"}),
        json!({"name": "contract-bad", "plugin_type": "bridge", "subnet": "10.252.8.0/24", "gateway": "10.9.9.9"}),
    ] {
        let (code, body) = post(&c, &token, "/cni/networks", bad.clone()).await;
        assert_eq!(code, 400, "{bad} -> {body}");
    }
    let (code, _) = post(
        &c,
        &token,
        "/cni/networks",
        json!({"name": net, "plugin_type": "bridge", "subnet": "10.252.9.0/24"}),
    )
    .await;
    assert_eq!(code, 409, "duplicate network name");

    let nets: Vec<api::sdn::CniNetwork> = fetch(&c, &token, "/cni/networks").await;
    assert!(nets.iter().any(|n| n.name == net));
    let _: Vec<api::sdn::CniAttachment> = fetch(&c, &token, "/cni/attachments").await;

    // Picker body for a container that doesn't exist: 404, nothing recorded.
    let req = api::sdn::AttachRequest {
        container_id: "contract-no-such-ct".into(),
        network_name: net.clone(),
        ..Default::default()
    };
    let (code, body) = post(
        &c,
        &token,
        "/cni/attach",
        serde_json::to_value(&req).unwrap(),
    )
    .await;
    assert_eq!(code, 404, "attach unknown container: {body}");

    if let Ok(netns) = std::env::var("HORCRUX_CONTRACT_SDN_NETNS") {
        // Real bridge + host-local plugins (sdn_cni_e2e.sh).
        let (code, body) = post(
            &c,
            &token,
            "/cni/attach",
            json!({"container_id": "contract-ns", "network_name": net, "netns_path": netns,
                   "labels": {"app": "contract"}}),
        )
        .await;
        assert_eq!(code, 200, "real attach: {body}");
        let a: api::sdn::CniAttachment = serde_json::from_value(body).expect("UI CniAttachment");
        assert!(a.ip_address.starts_with("10.252.9."), "{a:?}");
        assert_eq!(a.interface_name, "net0");
        let list: Vec<api::sdn::CniAttachment> = fetch(&c, &token, "/cni/attachments").await;
        assert!(list
            .iter()
            .any(|x| x.container_id == "contract-ns" && x.network_name == net));
        let (code, body) = send_json(
            &c,
            &token,
            Method::DELETE,
            &format!("/cni/networks/{net}"),
            None,
        )
        .await;
        assert_eq!(code, 409, "delete while attached: {body}");
        let det = api::sdn::DetachRequest {
            container_id: "contract-ns".into(),
            network_name: net.clone(),
        };
        let (code, body) = post(
            &c,
            &token,
            "/cni/detach",
            serde_json::to_value(&det).unwrap(),
        )
        .await;
        assert_eq!(code, 200, "detach: {body}");
    } else {
        eprintln!("HORCRUX_CONTRACT_SDN_NETNS unset: real CNI attach runs in sdn_cni_e2e.sh");
    }
    let det = api::sdn::DetachRequest {
        container_id: "contract-ns".into(),
        network_name: net.clone(),
    };
    let (code, _) = post(
        &c,
        &token,
        "/cni/detach",
        serde_json::to_value(&det).unwrap(),
    )
    .await;
    assert_eq!(code, 404, "detach of something not attached");

    let (code, body) = send_json(
        &c,
        &token,
        Method::DELETE,
        &format!("/cni/networks/{net}"),
        None,
    )
    .await;
    assert_eq!(code, 200, "delete: {body}");
    let (code, _) = send_json(
        &c,
        &token,
        Method::DELETE,
        "/cni/networks/..%2F..%2Ftmp%2Fx",
        None,
    )
    .await;
    assert!(code == 400 || code == 404, "traversal delete -> {code}");
}

/// Networking > SDN Policies: the create form's body, the enforcement
/// banner, update, the enforced-chain view and delete, in the UI's types.
/// (Real traffic being blocked is checked by tests/sdn_cni_e2e.sh.)
#[tokio::test]
async fn sdn_policies_read_into_ui_types() {
    use reqwest::Method;
    let c = client();
    let token = login(&c).await;
    let id = format!("contract-pol-{}", std::process::id() % 100000);
    let _ = send_json(
        &c,
        &token,
        Method::DELETE,
        &format!("/network-policies/{id}"),
        None,
    )
    .await;

    let body = json!({
        "id": id, "name": "contract deny", "namespace": "default",
        "pod_selector": {"match_labels": {"app": "contract-none"}, "match_expressions": []},
        "policy_types": ["Ingress"],
        "ingress": [{"from": [{"PodSelector": {"match_labels": {"app": "web"}, "match_expressions": []}}],
                     "ports": [{"protocol": "TCP", "port": 5432, "end_port": null}]}],
        "egress": [], "enabled": true
    });
    let (code, resp) = post(&c, &token, "/network-policies", body.clone()).await;
    assert_eq!(code, 200, "create: {resp}");
    let r: api::sdn::PolicyChangeResponse =
        serde_json::from_value(resp).expect("UI PolicyChangeResponse");
    assert_eq!(r.enforcement.table, "bridge horcrux_netpol");
    let (code, _) = post(&c, &token, "/network-policies", body.clone()).await;
    assert_eq!(code, 409, "duplicate policy id");
    let mut bad = body.clone();
    bad["id"] = json!("../x");
    let (code, _) = post(&c, &token, "/network-policies", bad).await;
    assert_eq!(code, 400, "bad policy id");

    let _: api::sdn::EnforcementStatus = fetch(&c, &token, "/network-policies/enforcement").await;
    let (code, text) = send_json(
        &c,
        &token,
        Method::POST,
        "/network-policies/enforcement",
        None,
    )
    .await;
    assert_eq!(code, 200, "re-apply: {text}");
    let _: api::sdn::EnforcementStatus = decode("re-apply", &text);

    let mut upd = body.clone();
    upd["name"] = json!("contract deny v2");
    let (code, text) = send_json(
        &c,
        &token,
        Method::PUT,
        &format!("/network-policies/{id}"),
        Some(upd),
    )
    .await;
    assert_eq!(code, 200, "update: {text}");
    let _: api::sdn::PolicyChangeResponse = decode("update", &text);
    let got: Value = fetch(&c, &token, &format!("/network-policies/{id}")).await;
    assert_eq!(got["name"], "contract deny v2");
    // Selects no attachment here: no chains.
    let view: Vec<String> = fetch(&c, &token, &format!("/network-policies/{id}/nftables")).await;
    assert!(view.is_empty(), "{view:?}");

    let (code, text) = send_json(
        &c,
        &token,
        Method::DELETE,
        &format!("/network-policies/{id}"),
        None,
    )
    .await;
    assert_eq!(code, 200, "delete: {text}");
    let _: api::sdn::PolicyChangeResponse = decode("delete", &text);
    let (code, _) = send_json(
        &c,
        &token,
        Method::DELETE,
        &format!("/network-policies/{id}"),
        None,
    )
    .await;
    assert_eq!(code, 404);
}

/// `X-Horcrux-Signature` of a received event, checked against `key`.
fn signed_with(got: &Captured, key: &str) -> bool {
    use hmac::{Hmac, Mac};
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(key.as_bytes()).unwrap();
    mac.update(got.body.as_bytes());
    let want = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
    got.header("X-Horcrux-Signature") == Some(want.as_str())
}

/// POST /webhooks/{id}/test and expect a successful delivery to it.
async fn fire_test_webhook(c: &reqwest::Client, token: &str, id: &str) {
    let (status, text) = send_json(
        c,
        token,
        reqwest::Method::POST,
        &format!("/webhooks/{id}/test"),
        None,
    )
    .await;
    assert_eq!(status, 200, "test webhook: {text}");
    let d: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(d["success"], true, "{text}");
    assert_eq!(d["status_code"], 200, "{text}");
    assert_eq!(d["webhook_id"], id, "{text}");
}

/// Event webhooks (`/api/webhooks`, the Notifications page's Webhooks tab).
/// The UI type used to fail on any real list (it expected fields the API
/// never sent), so the tab always said "No event webhooks configured". The
/// API returned the HMAC secret and header values to any Sys.Audit reader,
/// and Test broadcast to nobody. Now: the UI reads a real list, nothing
/// returned leaks a secret, Test reaches this webhook signed with the
/// stored key, and an edit that sends the masks back keeps both.
#[tokio::test]
async fn event_webhooks_mask_secrets_and_deliver_signed() {
    use reqwest::Method;
    let c = client();
    let token = login(&c).await;
    let (addr, mut rx) = http_receiver("127.0.0.1:0", 200).await;
    let id = format!("contract-hook-{}", std::process::id());
    let key = "contract-hmac-key";
    let bearer = "Bearer contract-token";
    let leaks = |text: &str| text.contains(key) || text.contains("contract-token");

    let body = json!({
        "id": id,
        "name": "contract hook",
        "url": format!("http://{addr}/evt"),
        "events": ["vm_started", {"custom": "contract"}],
        "secret": key,
        "headers": {"Authorization": bearer},
    });
    let (status, text) = send_json(&c, &token, Method::POST, "/webhooks", Some(body.clone())).await;
    assert_eq!(status, 200, "create webhook: {text}");
    assert!(!leaks(&text), "create echoed a secret: {text}");
    let (status, text) = send_json(&c, &token, Method::POST, "/webhooks", Some(body)).await;
    assert_eq!(status, 409, "a taken id is refused: {text}");
    let (status, text) = send_json(
        &c,
        &token,
        Method::POST,
        "/webhooks",
        Some(json!({"name": "bad", "url": "ftp://example.org/x"})),
    )
    .await;
    assert_eq!(status, 422, "a non-http url is refused: {text}");

    let (status, text) = get_raw(&c, &token, "/webhooks").await;
    assert_eq!(status, 200, "list: {text}");
    assert!(!leaks(&text), "list leaks a secret: {text}");
    let list: Vec<api::WebhookConfig> = decode("GET /webhooks", &text);
    let w = list
        .iter()
        .find(|w| w.id == id)
        .expect("created webhook is listed");
    assert_eq!(
        w.events.iter().map(|e| e.label()).collect::<Vec<_>>(),
        ["vm_started", "custom:contract"]
    );
    assert_eq!((w.success_count, w.failure_count), (0, 0));
    assert!(w.last_triggered.is_none());
    assert_eq!(w.secret.as_deref(), Some("********"));
    assert_eq!(w.headers["Authorization"], "********");
    // The URL's path can be the receiver's credential: host and port only.
    assert_eq!(w.url, format!("http://{addr}/********"));

    // Test reaches this webhook, signed with the stored secret.
    let received = |got: &Captured| {
        assert_eq!((got.method.as_str(), got.path.as_str()), ("POST", "/evt"));
        assert_eq!(got.header("Authorization"), Some(bearer));
        assert!(signed_with(got, key), "bad signature: {got:?}");
        let ev = got.json();
        assert_eq!(ev["event_type"], "test", "{ev}");
        assert_eq!(ev["data"]["webhook_id"], id.as_str(), "{ev}");
    };
    fire_test_webhook(&c, &token, &id).await;
    received(&next(&mut rx, "test event").await);

    let (status, text) = get_raw(&c, &token, &format!("/webhooks/{id}")).await;
    assert_eq!(status, 200, "get: {text}");
    assert!(!leaks(&text), "get leaks a secret: {text}");
    let w: api::WebhookConfig = decode("GET /webhooks/{id}", &text);
    assert_eq!((w.success_count, w.failure_count), (1, 0));
    assert!(w.last_triggered.is_some());

    // Edit what GET returned (masks included) under a stray body id.
    let mut edit: Value = serde_json::from_str(&text).unwrap();
    edit["name"] = json!("contract hook (edited)");
    edit["id"] = json!("contract-hook-stray");
    let (status, text) = send_json(
        &c,
        &token,
        Method::POST,
        &format!("/webhooks/{id}"),
        Some(edit),
    )
    .await;
    assert_eq!(status, 200, "update: {text}");
    fire_test_webhook(&c, &token, &id).await;
    received(&next(&mut rx, "test event after edit").await);
    let w: api::WebhookConfig = fetch(&c, &token, &format!("/webhooks/{id}")).await;
    assert_eq!(w.name, "contract hook (edited)");
    assert_eq!(w.success_count, 2);
    let (status, _) = get_raw(&c, &token, "/webhooks/contract-hook-stray").await;
    assert_eq!(status, 404, "the body id must not create a second webhook");

    let (status, text) = send_json(
        &c,
        &token,
        Method::POST,
        "/webhooks/contract-hook-missing/test",
        None,
    )
    .await;
    assert_eq!(status, 404, "test of an unknown webhook: {text}");
    let path = format!("/webhooks/{id}");
    let (status, text) = send_json(&c, &token, Method::DELETE, &path, None).await;
    assert_eq!(status, 200, "delete: {text}");
    let (status, _) = send_json(&c, &token, Method::DELETE, &path, None).await;
    assert_eq!(status, 404, "second delete");
    let (status, _) = get_raw(&c, &token, &path).await;
    assert_eq!(status, 404, "deleted webhook is gone");
}

/// Guest power (save state on host shutdown / resume on boot): the UI's
/// types read the real overview and per-guest routes; a stopped guest has
/// nothing to save or resume; the policy round-trips.
#[tokio::test]
async fn guest_power_routes_read_into_ui_types() {
    let c = client();
    let token = login(&c).await;
    let vm = "contract-power-vm";
    delete(&c, &token, &format!("/vms/{vm}")).await;
    let (status, body) = post(
        &c,
        &token,
        "/vms",
        json!({"id": vm, "name": vm, "hypervisor": "qemu", "architecture": "x86_64",
               "cpus": 1, "memory": 128, "disk_size": 1, "status": "stopped"}),
    )
    .await;
    assert!(status == 200 || status == 201, "create VM: {status} {body}");

    let ov: api::PowerOverview = fetch(&c, &token, "/nodes/guest-power").await;
    let g = ov
        .guests
        .iter()
        .find(|g| g.id == vm)
        .expect("VM in the power overview");
    assert_eq!(g.kind, "vm");
    assert!(g.saved.is_none() && g.can_save);
    assert!(["save", "shutdown", "poweroff"].contains(&ov.node.stop_action.as_str()));

    // Node default round-trips.
    let (st, body) = put(
        &c,
        &token,
        "/nodes/guest-power",
        serde_json::to_value(&ov.node).unwrap(),
    )
    .await;
    assert_eq!(st, 200, "{body}");
    let _: api::NodePower = serde_json::from_str(&body).expect("UI NodePower");
    let mut bad = serde_json::to_value(&ov.node).unwrap();
    bad["max_parallel"] = json!(0);
    assert_eq!(put(&c, &token, "/nodes/guest-power", bad).await.0, 422);

    // Per-guest policy.
    let path = format!("/vms/{vm}/power-policy");
    let (st, body) = put(
        &c,
        &token,
        &path,
        json!({"stop_action": "poweroff", "start_action": "always", "start_delay_secs": 7, "start_order": 3}),
    )
    .await;
    assert_eq!(st, 200, "{body}");
    let ov: api::PowerOverview = fetch(&c, &token, "/nodes/guest-power").await;
    let g = ov.guests.iter().find(|g| g.id == vm).unwrap();
    assert_eq!(g.policy.stop_action, "poweroff");
    assert_eq!(g.policy.start_action, "always");
    assert_eq!((g.policy.start_delay_secs, g.policy.start_order), (7, 3));
    assert_eq!(g.overrides.start_delay_secs, Some(7));
    assert_eq!(
        put(&c, &token, &path, json!({"stop_action": "explode"}))
            .await
            .0,
        422
    );

    // Nothing to save / resume / discard on a stopped guest.
    let (st, _) = post(&c, &token, &format!("/vms/{vm}/save-state"), json!({})).await;
    assert_eq!(st, 409, "saving a stopped VM");
    let (st, _) = post(&c, &token, &format!("/vms/{vm}/resume-state"), json!({})).await;
    assert_eq!(st, 404, "no saved state to resume");
    let (st, _) = get_raw(&c, &token, &format!("/vms/{vm}/saved-state")).await;
    assert_eq!(st, 404);
    let (st, _) = get_raw(&c, &token, "/vms/no-such-vm/power-policy").await;
    assert_eq!(st, 404);

    // Cluster maintenance / rebalance / placement (USE=cluster builds).
    let (st, _) = get_raw(&c, &token, "/nodes/maintenance").await;
    if st == 200 {
        let m: api::MaintenanceState = fetch(&c, &token, "/nodes/maintenance").await;
        assert!(
            !m.enabled && !m.clustered,
            "a single node is not in maintenance: {m:?}"
        );
        let _: api::MovePlan = fetch(&c, &token, "/nodes/maintenance/plan").await;
        let r: api::RebalanceState = fetch(&c, &token, "/cluster/rebalance").await;
        assert!(["off", "on_return", "auto"].contains(&r.settings.mode.as_str()));
        assert_eq!(r.nodes.len(), 1, "one node: {:?}", r.nodes);
        assert!(r.plan.moves.is_empty(), "nothing to rebalance on one node");
        let _: Vec<api::MoveHistoryEntry> = fetch(&c, &token, "/cluster/moves").await;
        // Entering maintenance needs somewhere to move guests to.
        let (st, _) = post(
            &c,
            &token,
            "/nodes/maintenance",
            json!({"enabled": true, "drain": true}),
        )
        .await;
        assert_eq!(st, 409, "maintenance on a single node");
        let mut bad = serde_json::to_value(&r.settings).unwrap();
        bad["mode"] = json!("sometimes");
        assert_eq!(put(&c, &token, "/cluster/rebalance", bad).await.0, 422);
        // Placement rules round-trip into the UI type.
        let pl = format!("/vms/{vm}/placement");
        let (st, body) = put(
            &c,
            &token,
            &pl,
            json!({"affinity_group": "web", "anti_affinity_group": "db-a", "no_migrate": true}),
        )
        .await;
        assert_eq!(st, 200, "{body}");
        let rules: api::PlacementRules = fetch(&c, &token, &pl).await;
        assert_eq!(rules.affinity_group.as_deref(), Some("web"));
        assert!(rules.no_migrate);
        assert_eq!(
            put(
                &c,
                &token,
                &pl,
                json!({"affinity_group": "g", "anti_affinity_group": "g"})
            )
            .await
            .0,
            422
        );
        let _ = put(&c, &token, &pl, json!({})).await;
        // Peer-only routes refuse a user token.
        assert_eq!(
            post(&c, &token, "/ha/peer/placement", json!({})).await.0,
            403
        );
    }

    // The host-agent hook accepts nothing without its token.
    let r = c
        .post(format!("{}/host-agent/stop-guests", api_base()))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 403);
    let r = c
        .post(format!("{}/host-agent/boot-guests", api_base()))
        .header("x-horcrux-host-token", "wrong")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 403);

    // Clean up: policy and VM.
    let _ = put(&c, &token, &path, json!({})).await;
    delete(&c, &token, &format!("/vms/{vm}")).await;
}
