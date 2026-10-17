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
//! http://127.0.0.1:8006/api).

use horcrux_ui::api;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::time::Duration;

fn api_base() -> String {
    std::env::var("HORCRUX_CONTRACT_API").unwrap_or_else(|_| "http://127.0.0.1:8006/api".into())
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
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
    check!("/cluster/nodes", Vec<api::ClusterNode>);
    check!("/cluster/architecture", api::ClusterArchitecture);
    check!("/users", Vec<api::User>);
    check!("/roles", Vec<api::Role>);
    check!("/auth/sessions", Vec<api::UserSession>);
    check!("/backups", Vec<bk::Backup>);
    check!("/backup-jobs", Vec<bk::BackupJob>);
    check!("/backup-storages", Vec<bk::BackupStorage>);
    check!("/snapshot-schedules", Vec<api::SnapshotSchedule>);
    check!("/snapshot-quotas", Vec<api::SnapshotQuota>);
    check!("/snapshot-quotas/summary", api::QuotaSummary);
    check!("/templates", Vec<api::VmTemplate>);
    check!("/ha/resources", Vec<api::HaResource>);
    check!("/ha/status", api::HaStatus);
    check!("/ha/groups", Vec<api::HaGroup>);
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
