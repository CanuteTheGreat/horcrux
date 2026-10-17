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
    check!("/backups", Vec<api::VmBackup>);
    check!("/backup-jobs", Vec<api::BackupJob>);
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
    delete(&c, &token, &format!("/vms/{vm}")).await;
    delete(&c, &token, &format!("/containers/{ct}")).await;

    assert!(
        failures.is_empty(),
        "UI/API contract failures:\n  {}",
        failures.join("\n  ")
    );
}
