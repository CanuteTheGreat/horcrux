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
