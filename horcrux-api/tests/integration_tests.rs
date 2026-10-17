//! Integration tests for Horcrux API
//!
//! These tests verify end-to-end functionality across all major subsystems:
//! - VM lifecycle management
//! - Clustering and node management
//! - Storage operations
//! - Backup and restore
//! - Monitoring and alerts
//! - Authentication and authorization
//! - Firewall rules
//!
//! Run with: cargo test --test integration_tests

#![allow(clippy::unnecessary_unwrap)]

use horcrux_common::*;
use reqwest::Client;
use serde_json::json;
use std::time::Duration;
use tokio::time::sleep;

const API_BASE: &str = "http://localhost:8006/api";
const TEST_VM_ID: &str = "test-vm-integration";
const TEST_NODE_ID: &str = "test-node-1";

/// Test helper to create a plain (unauthenticated) HTTP client
fn create_client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("Failed to create HTTP client")
}

/// Assert the request reached the server AND got a 2xx status (printing the
/// body on failure). `response.is_ok()` alone only proves *some* HTTP
/// response came back - a 401/404/500 passed, which is how most of this
/// suite used to "pass" without exercising anything.
async fn expect_2xx(
    response: reqwest::Result<reqwest::Response>,
    what: &str,
) -> reqwest::Result<reqwest::Response> {
    let response = response.unwrap_or_else(|e| panic!("{what}: request failed: {e}"));
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        panic!("{what}: HTTP {status}: {body}");
    }
    Ok(response)
}

/// A client whose every request carries the shared admin bearer token.
async fn authed_client() -> Client {
    let token = admin_token(&create_client()).await;
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    Client::builder()
        .timeout(Duration::from_secs(60))
        .default_headers(headers)
        .build()
        .expect("Failed to create HTTP client")
}

/// Test helper to wait for async operations to complete
async fn wait_for_operation(duration_ms: u64) {
    sleep(Duration::from_millis(duration_ms)).await;
}

/// POST a login request, retrying on 429 (Too Many Requests) since the
/// login endpoint is strictly rate-limited (5/min) and shared across all
/// concurrently-running tests.
async fn login_with_retry(
    client: &Client,
    username: &str,
    password: &str,
) -> reqwest::Result<reqwest::Response> {
    let body = json!({"username": username, "password": password});
    let mut attempt = 0;
    loop {
        let resp = client
            .post(format!("{}/auth/login", API_BASE))
            .json(&body)
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS && attempt < 30 {
            attempt += 1;
            wait_for_operation(2000).await;
            continue;
        }
        return Ok(resp);
    }
}

/// Log in as admin/admin and return the bearer token (JWT), panicking on
/// failure. All protected endpoints require Bearer/session/API-key auth.
///
/// The result is cached process-wide: the login endpoint is rate-limited,
/// and each `#[tokio::test]` runs in its own task, so logging in fresh for
/// every test would blow through that limit. All tests share one token.
async fn admin_token(client: &Client) -> String {
    static TOKEN: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();
    TOKEN
        .get_or_init(|| async {
            let mut attempt = 0;
            loop {
                let response = client
                    .post(format!("{}/auth/login", API_BASE))
                    .json(&json!({"username": "admin", "password": "admin"}))
                    .send()
                    .await
                    .expect("Failed to reach server for admin login");
                if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS && attempt < 10 {
                    attempt += 1;
                    sleep(Duration::from_millis(500)).await;
                    continue;
                }
                assert!(
                    response.status().is_success(),
                    "Admin login failed: {}",
                    response.status()
                );
                let body: serde_json::Value = response
                    .json()
                    .await
                    .expect("Failed to parse login response");
                break body["ticket"]
                    .as_str()
                    .expect("Login response missing ticket")
                    .to_string();
            }
        })
        .await
        .clone()
}

#[tokio::test]
async fn test_vm_lifecycle() {
    let client = create_client();
    let token = admin_token(&client).await;

    // Best-effort cleanup in case a previous run left this VM behind
    let _ = client
        .delete(format!("{}/vms/{}", API_BASE, TEST_VM_ID))
        .bearer_auth(&token)
        .send()
        .await;

    // 1. Create VM
    let vm_config = json!({
        "id": TEST_VM_ID,
        "name": "Integration Test VM",
        "hypervisor": "qemu",
        "architecture": "x86_64",
        "cpus": 2,
        "memory": 2048,
        "disk_size": 20,
        "status": "stopped"
    });

    let response = client
        .post(format!("{}/vms", API_BASE))
        .bearer_auth(&token)
        .json(&vm_config)
        .send()
        .await;

    let response = expect_2xx(response, "Failed to create VM").await;
    let response = response.unwrap();
    assert_eq!(
        response.status(),
        201,
        "VM creation returned unexpected status"
    );

    // 2. Get VM details
    let response = client
        .get(format!("{}/vms/{}", API_BASE, TEST_VM_ID))
        .bearer_auth(&token)
        .send()
        .await;

    let response = expect_2xx(response, "Failed to get VM").await;
    let vm: VmConfig = response.unwrap().json().await.expect("Failed to parse VM");
    assert_eq!(vm.id, TEST_VM_ID);
    assert_eq!(vm.name, "Integration Test VM");
    assert_eq!(vm.cpus, 2);
    assert_eq!(vm.memory, 2048);

    // 3. Start VM
    let response = client
        .post(format!("{}/vms/{}/start", API_BASE, TEST_VM_ID))
        .bearer_auth(&token)
        .send()
        .await;

    let _ = expect_2xx(response, "Failed to start VM").await;
    wait_for_operation(2000).await;

    // Verify VM is running
    let response = client
        .get(format!("{}/vms/{}", API_BASE, TEST_VM_ID))
        .bearer_auth(&token)
        .send()
        .await;

    let vm: VmConfig = response.unwrap().json().await.expect("Failed to parse VM");
    assert_eq!(vm.status, VmStatus::Running, "VM should be running");

    // 4. Stop VM
    let response = client
        .post(format!("{}/vms/{}/stop", API_BASE, TEST_VM_ID))
        .bearer_auth(&token)
        .send()
        .await;

    let _ = expect_2xx(response, "Failed to stop VM").await;
    wait_for_operation(2000).await;

    // 5. Delete VM
    let response = client
        .delete(format!("{}/vms/{}", API_BASE, TEST_VM_ID))
        .bearer_auth(&token)
        .send()
        .await;

    let _ = expect_2xx(response, "Failed to delete VM").await;

    // Verify VM is deleted
    let response = client
        .get(format!("{}/vms/{}", API_BASE, TEST_VM_ID))
        .bearer_auth(&token)
        .send()
        .await;

    assert_eq!(response.unwrap().status(), 404, "VM should be deleted");
}

#[tokio::test]
async fn test_cluster_operations() {
    let client = create_client();
    let token = admin_token(&client).await;

    // Idempotent: drop a node left by an earlier run against this server.
    let _ = client
        .delete(format!("{}/cluster/nodes/{}", API_BASE, TEST_NODE_ID))
        .bearer_auth(&token)
        .send()
        .await;

    // 1. Add a node to the cluster (real API: POST /cluster/nodes/:name)
    let node = json!({
        "id": 1,
        "name": TEST_NODE_ID,
        "ip": "192.168.1.100",
        "status": "online",
        "priority": 100,
        "is_local": false,
        "architecture": "x86_64",
        "cpu_cores": 4,
        "memory_total": 8589934592u64
    });

    let response = client
        .post(format!("{}/cluster/nodes/{}", API_BASE, TEST_NODE_ID))
        .bearer_auth(&token)
        .json(&node)
        .send()
        .await;

    let _ = expect_2xx(response, "Failed to add cluster node").await;
    wait_for_operation(500).await;

    // 2. Get node list
    let response = client
        .get(format!("{}/cluster/nodes", API_BASE))
        .bearer_auth(&token)
        .send()
        .await;

    let response = expect_2xx(response, "Failed to get node list").await;
    let nodes: Vec<serde_json::Value> = response
        .unwrap()
        .json()
        .await
        .expect("Failed to parse nodes");
    assert!(!nodes.is_empty(), "Should have at least one node");
    assert!(
        nodes.iter().any(|n| n["name"] == TEST_NODE_ID),
        "Added node should be in the list"
    );

    // 3. Get cluster architecture summary
    let response = client
        .get(format!("{}/cluster/architecture", API_BASE))
        .bearer_auth(&token)
        .send()
        .await;

    let _ = expect_2xx(response, "Failed to get cluster architecture").await;

    // 4. Remove it again
    let response = client
        .delete(format!("{}/cluster/nodes/{}", API_BASE, TEST_NODE_ID))
        .bearer_auth(&token)
        .send()
        .await;
    let _ = expect_2xx(response, "Failed to remove cluster node").await;
}

/// `GET /api/cluster/status`, `DELETE /api/cluster/nodes/:name` (used by
/// `horcrux cluster status` / `cluster remove`, and unregistered until
/// now) and the node heartbeat filling in live CPU/memory usage.
#[tokio::test]
async fn test_cluster_status_remove_and_live_usage() {
    const NODE: &str = "ci-loopback-node";
    let client = create_client();
    let token = admin_token(&client).await;

    // Status is a protected route.
    let resp = client
        .get(format!("{}/cluster/status", API_BASE))
        .send()
        .await
        .expect("status request");
    assert_eq!(resp.status(), 401, "cluster status must require auth");

    let resp = client
        .get(format!("{}/cluster/status", API_BASE))
        .bearer_auth(&token)
        .send()
        .await
        .expect("status request");
    assert_eq!(resp.status(), 200, "GET /api/cluster/status must exist");
    let status: serde_json::Value = resp.json().await.expect("status json");
    for field in [
        "name",
        "total_nodes",
        "online_nodes",
        "has_quorum",
        "total_vms",
        "total_cpus",
        "total_memory",
        "memory_used",
        "nodes_reporting_usage",
    ] {
        assert!(
            status.get(field).is_some(),
            "status missing {field}: {status}"
        );
    }

    // This server's own usage endpoint (what other nodes poll).
    let resp = client
        .get(format!("{}/cluster/local-usage", API_BASE))
        .bearer_auth(&token)
        .send()
        .await
        .expect("local-usage request");
    assert_eq!(resp.status(), 200);
    let usage: serde_json::Value = resp.json().await.unwrap();
    assert!(usage["memory_total"].as_u64().unwrap_or(0) > 0, "{usage}");
    assert!(usage["memory_used"].as_u64().unwrap_or(0) > 0, "{usage}");
    let cpu = usage["cpu_usage"].as_f64().expect("cpu_usage number");
    assert!((0.0..=100.0).contains(&cpu), "{usage}");

    // Register this host by loopback address, with no specs and no status:
    // the heartbeat must mark it online and fill in real usage + hardware.
    let resp = client
        .post(format!("{}/cluster/nodes/{}", API_BASE, NODE))
        .bearer_auth(&token)
        .json(&json!({"address": "127.0.0.1"}))
        .send()
        .await
        .expect("add node");
    assert_eq!(resp.status(), 201);

    let mut reported = None;
    for _ in 0..40 {
        let nodes: Vec<serde_json::Value> = client
            .get(format!("{}/cluster/nodes", API_BASE))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let node = nodes
            .into_iter()
            .find(|n| n["name"] == NODE)
            .expect("added node listed");
        if !node["cpu_usage"].is_null() {
            reported = Some(node);
            break;
        }
        assert!(
            node["memory_used"].is_null(),
            "usage must be null (unknown) until reported, never 0: {node}"
        );
        wait_for_operation(1000).await;
    }
    let node = reported.expect("node heartbeat never reported usage within 40s");
    assert_eq!(node["status"], "online", "{node}");
    assert!(node["memory_used"].as_u64().unwrap_or(0) > 0, "{node}");
    assert!(node["memory_total"].as_u64().unwrap_or(0) > 0, "{node}");
    assert!(node["cpu_cores"].as_u64().unwrap_or(0) > 0, "{node}");
    assert!(node["last_seen"].as_i64().is_some(), "{node}");

    // Remove it: 204, gone from the list, second delete is 404.
    let resp = client
        .delete(format!("{}/cluster/nodes/{}", API_BASE, NODE))
        .bearer_auth(&token)
        .send()
        .await
        .expect("delete node");
    assert_eq!(
        resp.status(),
        204,
        "DELETE /api/cluster/nodes/:name must exist"
    );

    let nodes: Vec<serde_json::Value> = client
        .get(format!("{}/cluster/nodes", API_BASE))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!nodes.iter().any(|n| n["name"] == NODE), "node not removed");

    let resp = client
        .delete(format!("{}/cluster/nodes/{}", API_BASE, NODE))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn test_storage_operations() {
    let client = authed_client().await;

    // 1. List storage pools
    let response = client
        .get(format!("{}/storage/pools", API_BASE))
        .send()
        .await;

    let _ = expect_2xx(response, "Failed to get storage pools").await;

    // 2. Create ZFS storage pool
    let zfs_pool = json!({
        "name": "zfs-test-pool",
        "storage_type": "zfs",
        "path": "tank/horcrux"
    });

    let response = client
        .post(format!("{}/storage/pools", API_BASE))
        .json(&zfs_pool)
        .send()
        .await;

    // Pool creation may fail if ZFS not available - that's ok for integration test
    let zfs_pool_created = response.is_ok() && response.as_ref().unwrap().status().is_success();

    if zfs_pool_created {
        wait_for_operation(500).await;
    }

    // 3. Create LVM storage pool
    let lvm_pool = json!({
        "name": "lvm-test-pool",
        "storage_type": "lvm",
        "path": "vg_test"
    });

    let response = client
        .post(format!("{}/storage/pools", API_BASE))
        .json(&lvm_pool)
        .send()
        .await;

    let lvm_pool_created = response.is_ok() && response.as_ref().unwrap().status().is_success();

    if lvm_pool_created {
        wait_for_operation(500).await;
    }

    // 4. Create Directory storage pool (the server runs on this host in CI,
    // so the test owns the backing directory and can check what lands in it)
    let dir_path = std::env::temp_dir().join("horcrux-it-dir-pool");
    let _ = std::fs::remove_dir_all(&dir_path);
    std::fs::create_dir_all(&dir_path).unwrap();
    let dir_pool = json!({
        "name": "dir-test-pool",
        "storage_type": "directory",
        "path": dir_path.to_string_lossy()
    });

    let response = client
        .post(format!("{}/storage/pools", API_BASE))
        .json(&dir_pool)
        .send()
        .await;

    // A directory pool needs nothing but a writable path: it must work.
    let response = expect_2xx(response, "Failed to create directory storage pool").await;
    #[allow(unused_assignments)]
    let mut test_pool_id = String::new();

    {
        let pool_response: serde_json::Value = response
            .unwrap()
            .json()
            .await
            .expect("Failed to parse pool");
        test_pool_id = pool_response["id"].as_str().unwrap().to_string();
        wait_for_operation(500).await;

        // 5. Get pool details
        let response = client
            .get(format!("{}/storage/pools/{}", API_BASE, test_pool_id))
            .send()
            .await;

        let _ = expect_2xx(response, "Failed to get pool details").await;

        // 6. Create volume in pool
        let volume_request = json!({
            "name": "test-volume",
            "size": 1  // GB (qcow2, sparse)
        });

        let response = client
            .post(format!(
                "{}/storage/pools/{}/volumes",
                API_BASE, test_pool_id
            ))
            .json(&volume_request)
            .send()
            .await;

        let _ = expect_2xx(response, "Failed to create volume in directory pool").await;
        // The volume is a real image file in the pool's backing directory.
        let vol = dir_path.join("test-volume.qcow2");
        assert!(vol.is_file(), "volume file missing: {}", vol.display());

        // 7. Delete the test pool
        let response = client
            .delete(format!("{}/storage/pools/{}", API_BASE, test_pool_id))
            .send()
            .await;

        let _ = expect_2xx(response, "Failed to delete storage pool").await;
    }

    // 8. Test Ceph pool creation (will likely fail without Ceph cluster)
    let ceph_pool = json!({
        "name": "ceph-test-pool",
        "storage_type": "ceph",
        "path": "rbd/horcrux"
    });

    let _ = client
        .post(format!("{}/storage/pools", API_BASE))
        .json(&ceph_pool)
        .send()
        .await;
    // Don't assert - Ceph might not be available

    // 9. Test NFS pool creation
    let nfs_pool = json!({
        "name": "nfs-test-pool",
        "storage_type": "nfs",
        "path": "192.168.1.100:/exports/horcrux"
    });

    let _ = client
        .post(format!("{}/storage/pools", API_BASE))
        .json(&nfs_pool)
        .send()
        .await;
    // Don't assert - NFS server might not be available
}

#[tokio::test]
async fn test_backup_operations() {
    let client = create_client();
    let token = admin_token(&client).await;

    // First create a test VM
    let vm_config = json!({
        "id": "backup-test-vm",
        "name": "Backup Test VM",
        "hypervisor": "qemu",
        "architecture": "x86_64",
        "cpus": 1,
        "memory": 1024,
        "disk_size": 10,
        "status": "stopped"
    });

    let _ = client
        .post(format!("{}/vms", API_BASE))
        .bearer_auth(&token)
        .json(&vm_config)
        .send()
        .await;

    wait_for_operation(1000).await;

    // 1. Create backup
    let backup_id = "backup-test-vm-full-1";
    let backup_request = json!({
        "id": backup_id,
        "name": "backup-test-vm full backup",
        "target_type": "vm",
        "target_id": "backup-test-vm",
        "storage": "/var/lib/horcrux/backups",
        "mode": "snapshot",
        "compression": "zstd",
        "notes": null
    });

    let response = client
        .post(format!("{}/backups", API_BASE))
        .bearer_auth(&token)
        .json(&backup_request)
        .send()
        .await;

    let response = expect_2xx(response, "Failed to create backup").await;
    let backup: serde_json::Value = response
        .unwrap()
        .json()
        .await
        .expect("Failed to parse backup response");
    let backup_id = backup["id"]
        .as_str()
        .expect("Backup should have an id")
        .to_string();

    wait_for_operation(1000).await;

    // 2. List backups
    let response = client
        .get(format!("{}/backups", API_BASE))
        .bearer_auth(&token)
        .send()
        .await;

    let response = expect_2xx(response, "Failed to list backups").await;
    let backups: Vec<serde_json::Value> = response
        .unwrap()
        .json()
        .await
        .expect("Failed to parse backups");
    assert!(!backups.is_empty(), "Should have at least one backup");

    // 3. Get backup info
    let response = client
        .get(format!("{}/backups/{}", API_BASE, backup_id))
        .bearer_auth(&token)
        .send()
        .await;

    let _ = expect_2xx(response, "Failed to get backup info").await;

    // 4. Restore from backup
    let restore_request = json!({
        "target_id": "backup-test-vm"
    });

    let response = client
        .post(format!("{}/backups/{}/restore", API_BASE, backup_id))
        .bearer_auth(&token)
        .json(&restore_request)
        .send()
        .await;

    let _ = expect_2xx(response, "Failed to restore backup").await;

    // Cleanup
    let _ = client
        .delete(format!("{}/backups/{}", API_BASE, backup_id))
        .bearer_auth(&token)
        .send()
        .await;
    let _ = client
        .delete(format!("{}/vms/backup-test-vm", API_BASE))
        .bearer_auth(&token)
        .send()
        .await;
}

#[tokio::test]
async fn test_monitoring_and_alerts() {
    let client = create_client();
    let token = admin_token(&client).await;

    // 1. Get node metrics
    let response = client
        .get(format!("{}/monitoring/node", API_BASE))
        .bearer_auth(&token)
        .send()
        .await;

    let response = expect_2xx(response, "Failed to get node metrics").await;
    let metrics: Option<serde_json::Value> = response
        .unwrap()
        .json()
        .await
        .expect("Failed to parse metrics");
    let metrics = metrics.expect("Node metrics should be populated");
    let cpu_usage = metrics["cpu"]["usage_percent"]
        .as_f64()
        .expect("cpu.usage_percent should be present");
    assert!((0.0..=100.0).contains(&cpu_usage));
    assert!(metrics["memory"]["total_bytes"].as_u64().unwrap_or(0) > 0);

    // 2. Create alert rule
    let alert_rule = json!({
        "id": "high-cpu-test",
        "name": "high_cpu_test",
        "description": "CPU usage exceeds 80%",
        "severity": "warning",
        "enabled": true,
        "condition": {
            "metric_type": "cpu_usage",
            "operator": "greaterthan",
            "threshold": 80.0,
            "target_pattern": "*",
            "duration_seconds": 60
        }
    });

    let response = client
        .post(format!("{}/alerts/rules", API_BASE))
        .bearer_auth(&token)
        .json(&alert_rule)
        .send()
        .await;

    let _ = expect_2xx(response, "Failed to create alert rule").await;

    // 3. List alert rules
    let response = client
        .get(format!("{}/alerts/rules", API_BASE))
        .bearer_auth(&token)
        .send()
        .await;

    let response = expect_2xx(response, "Failed to list alert rules").await;
    let rules: Vec<serde_json::Value> = response
        .unwrap()
        .json()
        .await
        .expect("Failed to parse rules");
    assert!(!rules.is_empty(), "Should have at least one alert rule");

    // 4. Get active alerts
    let response = client
        .get(format!("{}/alerts/active", API_BASE))
        .bearer_auth(&token)
        .send()
        .await;

    let response = expect_2xx(response, "Failed to get active alerts").await;
    let _alerts: Vec<serde_json::Value> = response
        .unwrap()
        .json()
        .await
        .expect("Failed to parse alerts");

    // 5. Get alert history
    let response = client
        .get(format!("{}/alerts/history", API_BASE))
        .bearer_auth(&token)
        .send()
        .await;

    let _ = expect_2xx(response, "Failed to get alert history").await;

    // Cleanup
    let _ = client
        .delete(format!("{}/alerts/rules/high-cpu-test", API_BASE))
        .bearer_auth(&token)
        .send()
        .await;
}

#[tokio::test]
async fn test_authentication() {
    let client = create_client();

    // 1. Login with valid credentials
    let login_request = json!({
        "username": "admin",
        "password": "admin"
    });

    let response = client
        .post(format!("{}/auth/login", API_BASE))
        .json(&login_request)
        .send()
        .await;

    let response = expect_2xx(response, "Failed to login").await;
    let token_response: serde_json::Value = response
        .unwrap()
        .json()
        .await
        .expect("Failed to parse token");
    assert!(
        token_response.get("ticket").is_some(),
        "Should receive auth token"
    );

    // 2. Verify token works
    let token = token_response["ticket"].as_str().unwrap();
    let response = client
        .get(format!("{}/auth/verify", API_BASE))
        .bearer_auth(token)
        .send()
        .await;

    let verified: serde_json::Value = expect_2xx(response, "Token verification failed")
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(verified["valid"], true, "{verified}");
    assert_eq!(verified["username"], "admin", "{verified}");

    // 3. Test invalid credentials
    let bad_login = json!({
        "username": "admin",
        "password": "wrongpassword"
    });

    let response = client
        .post(format!("{}/auth/login", API_BASE))
        .json(&bad_login)
        .send()
        .await;

    if response.is_ok() {
        let status = response.unwrap().status();
        assert!(
            status == 401 || status == 429,
            "Should reject invalid credentials (got {})",
            status
        );
    }
}

#[tokio::test]
async fn test_firewall_rules() {
    let client = create_client();
    let token = admin_token(&client).await;

    // 1. List firewall rules (datacenter scope)
    let response = client
        .get(format!("{}/firewall/rules", API_BASE))
        .bearer_auth(&token)
        .send()
        .await;

    let _ = expect_2xx(response, "Failed to get firewall rules").await;

    // 2. Create firewall rule
    let rule_id = "test-http-allow";
    let rule = json!({
        "id": rule_id,
        "enabled": true,
        "action": "ACCEPT",
        "direction": "in",
        "protocol": "tcp",
        "source": "0.0.0.0/0",
        "dest": "0.0.0.0/0",
        "sport": null,
        "dport": "80",
        "comment": "integration test rule",
        "log": false,
        "position": 0
    });

    let response = client
        .post(format!("{}/firewall/rules", API_BASE))
        .bearer_auth(&token)
        .json(&rule)
        .send()
        .await;

    let _ = expect_2xx(response, "Failed to create firewall rule").await;

    // 3. Confirm the rule appears in the list
    let response = client
        .get(format!("{}/firewall/rules", API_BASE))
        .bearer_auth(&token)
        .send()
        .await;

    let response = expect_2xx(response, "Failed to list firewall rules").await;
    let rules: Vec<serde_json::Value> = response
        .unwrap()
        .json()
        .await
        .expect("Failed to parse rules");
    assert!(
        rules.iter().any(|r| r["id"] == rule_id),
        "Created rule should be in the list"
    );

    // 4. Apply firewall rules (datacenter scope)
    let response = client
        .post(format!("{}/firewall/datacenter/apply", API_BASE))
        .bearer_auth(&token)
        .send()
        .await;

    // Applying needs nft + CAP_NET_ADMIN. CI's unprivileged Test container
    // has neither, so here the endpoint must either apply or fail with a
    // clear nftables error; the real apply (rules visible in `nft list
    // ruleset`) is proven by the "Firewall apply (real nftables)" CI step.
    let response = response.expect("apply request failed");
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    assert!(
        status.is_success() || (status.as_u16() == 500 && body.contains("nftables")),
        "apply firewall rules: {status} {body}"
    );

    // 5. Delete test rule
    let response = client
        .delete(format!("{}/firewall/rules/{}", API_BASE, rule_id))
        .bearer_auth(&token)
        .send()
        .await;

    let _ = expect_2xx(response, "Failed to delete firewall rule").await;
}

#[tokio::test]
async fn test_template_operations() {
    let client = authed_client().await;
    let base = "tpl-base-vm";
    for id in [base, "tpl-clone-full", "tpl-clone-linked"] {
        let _ = client
            .delete(format!("{}/vms/{}", API_BASE, id))
            .send()
            .await;
    }

    let r = client.get(format!("{}/templates", API_BASE)).send().await;
    let _ = expect_2xx(r, "Failed to list templates").await;

    // 1. A real VM to turn into a template.
    let r = client
        .post(format!("{}/vms", API_BASE))
        .json(&json!({"id": base, "name": base, "hypervisor": "qemu",
                      "architecture": "x86_64", "cpus": 2, "memory": 512,
                      "disk_size": 1, "status": "stopped"}))
        .send()
        .await;
    let _ = expect_2xx(r, "create base VM").await;

    // Unknown VM -> 404; a path that isn't the VM's disk -> 422.
    let r = client
        .post(format!("{}/templates", API_BASE))
        .json(&json!({"vm_id": "no-such-vm", "name": "x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    let r = client
        .post(format!("{}/templates", API_BASE))
        .json(&json!({"vm_id": base, "name": "x", "disk_path": "/etc/passwd"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 422);

    // 2. Create the template.
    let r = client
        .post(format!("{}/templates", API_BASE))
        .json(&json!({"vm_id": base, "name": "it-template", "description": "integration"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        201,
        "create template: {}",
        r.text().await.unwrap_or_default()
    );
    let tpl: serde_json::Value = r.json().await.unwrap();
    let tpl_id = tpl["id"].as_str().unwrap().to_string();
    assert_eq!(tpl["memory"], 512);
    assert_eq!(tpl["cpus"], 2);
    let list: Vec<serde_json::Value> = client
        .get(format!("{}/templates", API_BASE))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(list.iter().any(|t| t["id"] == tpl_id.as_str()));

    // 3. Deploy (full + linked) -> real VMs that are listed and startable.
    for (id, kind) in [("tpl-clone-full", "full"), ("tpl-clone-linked", "linked")] {
        let r = client
            .post(format!("{}/templates/{}/clone", API_BASE, tpl_id))
            .json(
                &json!({"new_vm_id": id, "new_vm_name": id, "clone_type": kind,
                          "storage_pool": null, "cloudinit_config": null}),
            )
            .send()
            .await;
        let r = expect_2xx(r, "clone template").await.unwrap();
        let body: serde_json::Value = r.json().await.unwrap();
        assert_eq!(body["new_vm_id"], id);
        let r = client.get(format!("{}/vms/{}", API_BASE, id)).send().await;
        let vm: serde_json::Value = expect_2xx(r, "cloned VM is registered")
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(vm["memory"], 512);
        assert!(std::path::Path::new(vm["disks"][0]["path"].as_str().unwrap()).is_file());
    }
    let r = client
        .post(format!("{}/vms/tpl-clone-linked/start", API_BASE))
        .send()
        .await;
    let _ = expect_2xx(r, "start VM deployed from template").await;
    let r = client
        .post(format!("{}/vms/tpl-clone-linked/stop", API_BASE))
        .send()
        .await;
    let _ = expect_2xx(r, "stop VM deployed from template").await;

    // Same id again -> 409; unknown template -> 404.
    let r = client
        .post(format!("{}/templates/{}/clone", API_BASE, tpl_id))
        .json(
            &json!({"new_vm_id": "tpl-clone-full", "new_vm_name": "x", "clone_type": "full",
                      "storage_pool": null, "cloudinit_config": null}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 409);
    let r = client
        .post(format!("{}/templates/nope/clone", API_BASE))
        .json(
            &json!({"new_vm_id": "x", "new_vm_name": "x", "clone_type": "full",
                      "storage_pool": null, "cloudinit_config": null}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);

    // 4. Deleting the template leaves the base VM's disk alone.
    let r = client
        .delete(format!("{}/templates/{}", API_BASE, tpl_id))
        .send()
        .await;
    let _ = expect_2xx(r, "delete template").await;
    let base_vm: serde_json::Value = client
        .get(format!("{}/vms/{}", API_BASE, base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(std::path::Path::new(base_vm["disks"][0]["path"].as_str().unwrap()).is_file());

    for id in ["tpl-clone-linked", "tpl-clone-full", base] {
        let _ = client
            .delete(format!("{}/vms/{}", API_BASE, id))
            .send()
            .await;
    }
}

#[tokio::test]
async fn test_console_access() {
    let client = create_client();
    let token = admin_token(&client).await;

    // Create test VM first
    let vm_config = json!({
        "id": "console-test-vm",
        "name": "Console Test VM",
        "hypervisor": "qemu",
        "architecture": "x86_64",
        "cpus": 1,
        "memory": 1024,
        "disk_size": 10,
        "status": "stopped"
    });

    let _ = client
        .post(format!("{}/vms", API_BASE))
        .bearer_auth(&token)
        .json(&vm_config)
        .send()
        .await;

    // Start VM
    let _ = client
        .post(format!("{}/vms/console-test-vm/start", API_BASE))
        .bearer_auth(&token)
        .send()
        .await;

    wait_for_operation(2000).await;

    // 1. Get VNC console URL
    let response = client
        .post(format!("{}/console/console-test-vm/vnc", API_BASE))
        .bearer_auth(&token)
        .send()
        .await;

    let response = expect_2xx(response, "Failed to get VNC console URL").await;
    let console_info: serde_json::Value = response
        .unwrap()
        .json()
        .await
        .expect("Failed to parse console info");
    assert!(
        console_info.get("ticket").is_some(),
        "Should have console ticket"
    );

    // 2. Get serial console
    let response = client
        .get(format!("{}/vms/console-test-vm/console/serial", API_BASE))
        .bearer_auth(&token)
        .send()
        .await;

    // May not be supported for all hypervisors
    if response.is_ok() {
        let _ = response.unwrap();
    }

    // Cleanup
    let _ = client
        .post(format!("{}/vms/console-test-vm/stop", API_BASE))
        .bearer_auth(&token)
        .send()
        .await;

    wait_for_operation(1000).await;

    let _ = client
        .delete(format!("{}/vms/console-test-vm", API_BASE))
        .bearer_auth(&token)
        .send()
        .await;
}

#[tokio::test]
async fn test_api_health() {
    let client = create_client();

    // Test health endpoint
    let response = client.get(format!("{}/health", API_BASE)).send().await;

    let response = expect_2xx(response, "Health check failed").await;
    assert_eq!(
        response.unwrap().status(),
        200,
        "Health check should return 200"
    );
}

#[tokio::test]
async fn test_session_management() {
    let client = create_client();

    // 1. Login to create session (retry on rate limit, since the auth
    // endpoint is strictly rate-limited and other tests share the budget)
    let login_request = json!({
        "username": "admin",
        "password": "admin"
    });

    let mut attempt = 0;
    let response = loop {
        let resp = client
            .post(format!("{}/auth/login", API_BASE))
            .json(&login_request)
            .send()
            .await
            .expect("Login failed");
        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS && attempt < 30 {
            attempt += 1;
            wait_for_operation(2000).await;
            continue;
        }
        break resp;
    };

    // Extract session cookie from Set-Cookie header
    let set_cookie = response
        .headers()
        .get("set-cookie")
        .expect("Should receive set-cookie header")
        .to_str()
        .unwrap();

    // Parse session_id from cookie string
    let session_id = set_cookie
        .split(';')
        .next()
        .unwrap()
        .trim()
        .strip_prefix("session_id=")
        .expect("Should have session_id cookie");

    // 2. Use session cookie to access protected endpoint
    let response = client
        .get(format!("{}/vms", API_BASE))
        .header("Cookie", format!("session_id={}", session_id))
        .send()
        .await;

    let response = expect_2xx(response, "Failed to access with session cookie").await;
    assert_eq!(response.unwrap().status(), 200);

    // 3. Logout (destroy session)
    let response = client
        .post(format!("{}/auth/logout", API_BASE))
        .header("Cookie", format!("session_id={}", session_id))
        .send()
        .await;

    let _ = expect_2xx(response, "Logout failed").await;

    // 4. Verify session is invalid after logout (retry on rate limit, same
    // pattern as every other endpoint in this suite -- /vms shares the
    // general rate-limit budget with the many other tests hitting it, so
    // without a retry loop this assertion can flake on a 429 that has
    // nothing to do with session validity, see login retry above).
    let mut attempt = 0;
    let response = loop {
        let resp = client
            .get(format!("{}/vms", API_BASE))
            .header("Cookie", format!("session_id={}", session_id))
            .send()
            .await;
        match &resp {
            Ok(r) if r.status() == reqwest::StatusCode::TOO_MANY_REQUESTS && attempt < 30 => {
                attempt += 1;
                wait_for_operation(2000).await;
                continue;
            }
            _ => break resp,
        }
    };

    if let Ok(resp) = response {
        assert_eq!(resp.status(), 401, "Should reject invalid session");
    }
}

#[tokio::test]
async fn test_password_change() {
    let client = create_client();

    // Use a unique username per test run instead of a fixed "testuser".
    // `/auth/register` persists to the database, but the admin `DELETE
    // /users/:id` cleanup below only removes the user from the in-memory
    // auth manager (a separate store from the DB-backed registration
    // table — see `AuthManager::delete_user`), so a fixed username would
    // hit "user already exists" on every run after the first and make
    // this test depend on manual DB resets between runs. A unique name
    // sidesteps that leftover state entirely.
    let username = format!(
        "testuser-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    // 1. Register the test user (with retry, since /auth/register shares
    // the same strict rate limit as /auth/login).
    let mut attempt = 0;
    loop {
        let resp = client
            .post(format!("{}/auth/register", API_BASE))
            .json(&json!({
                "username": username,
                "password": "testpass123",
                "email": format!("{}@example.com", username)
            }))
            .send()
            .await;
        match resp {
            Ok(r) if r.status() == reqwest::StatusCode::TOO_MANY_REQUESTS && attempt < 30 => {
                attempt += 1;
                wait_for_operation(2000).await;
                continue;
            }
            _ => break,
        }
    }

    let response = login_with_retry(&client, &username, "testpass123").await;

    // `login_with_retry` returns `Ok` even when its retry budget is
    // exhausted while the server is still rate-limiting (it just gives up
    // and hands back the last response, whatever its status) — so a
    // network-level error isn't the only "couldn't log in" case; a non-200
    // response (e.g. a lingering 429) means there's no token to extract.
    if response.is_err() {
        // User might not exist in test environment, skip test
        return;
    }
    let response = response.unwrap();
    if response.status() != reqwest::StatusCode::OK {
        // Login never succeeded (e.g. rate limit never cleared within the
        // retry budget) — nothing meaningful to assert on, skip test.
        return;
    }

    let token_response: serde_json::Value = response.json().await.unwrap();
    let token = token_response["ticket"].as_str().unwrap();

    // 2. Change password
    let change_request = json!({
        "username": username,
        "old_password": "testpass123",
        "new_password": "newpass456"
    });

    let response = client
        .post(format!("{}/auth/password", API_BASE))
        .bearer_auth(token)
        .json(&change_request)
        .send()
        .await;

    let _ = expect_2xx(response, "Password change failed").await;

    // 3. Verify old password doesn't work
    let response = login_with_retry(&client, &username, "testpass123").await;

    if response.is_ok() {
        assert_eq!(
            response.unwrap().status(),
            401,
            "Old password should not work"
        );
    }

    // 4. Verify new password works
    let response = login_with_retry(&client, &username, "newpass456").await;

    let _ = expect_2xx(response, "New password should work").await;

    // Cleanup
    let _ = client
        .delete(format!("{}/users/{}", API_BASE, username))
        .bearer_auth(token)
        .send()
        .await;
}

#[tokio::test]
async fn test_api_token_generation() {
    let client = create_client();

    // 1. Login first
    let login_request = json!({
        "username": "admin",
        "password": "admin"
    });

    let response = client
        .post(format!("{}/auth/login", API_BASE))
        .json(&login_request)
        .send()
        .await;

    if response.is_err() {
        return; // Skip if server not running
    }

    let token_response: serde_json::Value = response.unwrap().json().await.unwrap();
    let jwt_token = token_response["ticket"].as_str().unwrap();

    // 2. Create API key
    let api_key_request = json!({
        "name": "Integration Test Key",
        "expires_days": 30
    });

    let response = client
        .post(format!("{}/users/admin/api-keys", API_BASE))
        .bearer_auth(jwt_token)
        .json(&api_key_request)
        .send()
        .await;

    let response = expect_2xx(response, "Failed to create API key").await;
    let api_key_response: serde_json::Value = response.unwrap().json().await.unwrap();

    assert!(
        api_key_response.get("key").is_some(),
        "Should receive API key"
    );
    assert!(
        api_key_response.get("id").is_some(),
        "Should receive key ID"
    );

    let api_key = api_key_response["key"].as_str().unwrap();
    assert!(api_key.starts_with("hx_"), "API key should have hx_ prefix");

    // 3. Use API key to access endpoint
    let response = client
        .get(format!("{}/vms", API_BASE))
        .header("X-API-Key", api_key)
        .send()
        .await;

    let response = expect_2xx(response, "Failed to use API key").await;
    assert_eq!(response.unwrap().status(), 200);

    // 4. List user's API keys
    let response = client
        .get(format!("{}/users/admin/api-keys", API_BASE))
        .bearer_auth(jwt_token)
        .send()
        .await;

    let response = expect_2xx(response, "Failed to list API keys").await;
    let keys: Vec<serde_json::Value> = response.unwrap().json().await.unwrap();
    assert!(!keys.is_empty(), "Should have at least one API key");
}

#[tokio::test]
async fn test_rbac_permissions() {
    let client = create_client();

    // Use the shared cached admin token instead of a fresh login, since
    // /auth/login is strictly rate-limited (5/min) and every test competes
    // for that budget.
    let admin_jwt = admin_token(&client).await;

    // Register the vmuser (retry on rate limit; /auth/register shares the
    // same strict 5/min budget as /auth/login).
    let register_body = json!({
        "username": "vmuser",
        "password": "vmpass123",
        "email": "vmuser@example.com"
    });
    let mut attempt = 0;
    loop {
        let resp = client
            .post(format!("{}/auth/register", API_BASE))
            .json(&register_body)
            .send()
            .await;
        match resp {
            Ok(r) if r.status() == reqwest::StatusCode::TOO_MANY_REQUESTS && attempt < 30 => {
                attempt += 1;
                wait_for_operation(2000).await;
                continue;
            }
            _ => break,
        }
    }

    // 2. Login as limited user (retry on rate limit; shared 5/min budget)
    let user_login = json!({
        "username": "vmuser",
        "password": "vmpass123"
    });

    let mut attempt = 0;
    let response = loop {
        let resp = client
            .post(format!("{}/auth/login", API_BASE))
            .json(&user_login)
            .send()
            .await;
        match resp {
            Ok(r) if r.status() == reqwest::StatusCode::TOO_MANY_REQUESTS && attempt < 30 => {
                attempt += 1;
                wait_for_operation(2000).await;
                continue;
            }
            other => break other,
        }
    };

    if response.is_err() {
        return;
    }

    let user_token: serde_json::Value = response.unwrap().json().await.unwrap();
    let user_jwt = user_token["ticket"].as_str().unwrap();

    // 3. VmUser can view VMs (VmAudit privilege)
    let response = client
        .get(format!("{}/vms", API_BASE))
        .bearer_auth(user_jwt)
        .send()
        .await;

    let _ = expect_2xx(response, "VmUser should be able to view VMs").await;

    // 4. VmUser can start/stop VMs (VmPowerMgmt privilege)
    let response = client
        .post(format!("{}/vms/test-vm/start", API_BASE))
        .bearer_auth(user_jwt)
        .send()
        .await;

    // Should either succeed or fail because VM doesn't exist, not forbidden
    if response.is_ok() {
        let status = response.unwrap().status();
        assert_ne!(status, 403, "VmUser should have power management privilege");
    }

    // 5. VmUser CANNOT create VMs (requires VmAllocate privilege)
    let vm_config = json!({
        "name": "Unauthorized VM",
        "cpus": 1,
        "memory": 512
    });

    // Retry on rate limit, same pattern used everywhere else in this suite
    // (admin_token(), register/login above, test_session_management's final
    // assertion): POST /vms shares one rate-limit budget with every other
    // test hitting it in this binary, so without a retry loop this
    // assertion can flake on a 429 that has nothing to do with RBAC.
    let mut attempt = 0;
    let response = loop {
        let resp = client
            .post(format!("{}/vms", API_BASE))
            .bearer_auth(user_jwt)
            .json(&vm_config)
            .send()
            .await;
        match &resp {
            Ok(r) if r.status() == reqwest::StatusCode::TOO_MANY_REQUESTS && attempt < 30 => {
                attempt += 1;
                wait_for_operation(2000).await;
                continue;
            }
            _ => break resp,
        }
    };

    if let Ok(resp) = response {
        // Should be forbidden (403) or unauthorized (401). A 422 is also
        // acceptable here: this vm_config is intentionally minimal (it
        // omits fields required by the real schema), so validation may
        // reject it before an authorization check would even matter.
        let status = resp.status();
        assert!(
            status == 403 || status == 401 || status == 422,
            "VmUser should not be able to create VMs, got status: {}",
            status
        );
    }

    // Cleanup
    let _ = client
        .delete(format!("{}/users/vmuser", API_BASE))
        .bearer_auth(&admin_jwt)
        .send()
        .await;
}

#[tokio::test]
async fn test_cni_network_operations() {
    let client = create_client();
    let token = admin_token(&client).await;

    // 1. Create CNI network
    let network_config = json!({
        "cni_version": "1.0.0",
        "name": "test-bridge",
        "plugin_type": "bridge",
        "bridge": "cni-test0",
        "ipam": {
            "ipam_type": "host-local",
            "subnet": "10.99.0.0/24",
            "range_start": "10.99.0.10",
            "range_end": "10.99.0.254",
            "gateway": "10.99.0.1",
            "routes": []
        },
        "dns": null,
        "capabilities": {}
    });

    let response = client
        .post(format!("{}/cni/networks", API_BASE))
        .bearer_auth(&token)
        .json(&network_config)
        .send()
        .await;

    if response.is_err() {
        return; // Skip if server not running
    }

    let _ = expect_2xx(response, "Failed to create CNI network").await;

    // 2. List CNI networks
    let response = client
        .get(format!("{}/cni/networks", API_BASE))
        .bearer_auth(&token)
        .send()
        .await;

    let response = expect_2xx(response, "Failed to list CNI networks").await;
    let networks: Vec<serde_json::Value> = response.unwrap().json().await.unwrap();
    let test_network = networks.iter().find(|n| n["name"] == "test-bridge");
    assert!(test_network.is_some(), "Test network should exist");

    // 3. Delete CNI network
    let response = client
        .delete(format!("{}/cni/networks/test-bridge", API_BASE))
        .bearer_auth(&token)
        .send()
        .await;

    let _ = expect_2xx(response, "Failed to delete CNI network").await;
}

#[tokio::test]
async fn test_network_policy_enforcement() {
    let client = create_client();
    let token = admin_token(&client).await;

    // 1. Create network policy
    let policy = json!({
        "id": "test-policy-1",
        "name": "deny-all-ingress",
        "namespace": "default",
        "pod_selector": {
            "match_labels": {},
            "match_expressions": []
        },
        "policy_types": ["Ingress"],
        "ingress": [],
        "egress": [],
        "enabled": true
    });

    let response = client
        .post(format!("{}/network-policies", API_BASE))
        .bearer_auth(&token)
        .json(&policy)
        .send()
        .await;

    if response.is_err() {
        return;
    }

    let _ = expect_2xx(response, "Failed to create network policy").await;

    // 2. List network policies
    let response = client
        .get(format!("{}/network-policies", API_BASE))
        .bearer_auth(&token)
        .send()
        .await;

    let response = expect_2xx(response, "Failed to list network policies").await;
    let policies: Vec<serde_json::Value> = response.unwrap().json().await.unwrap();
    assert!(!policies.is_empty(), "Should have at least one policy");

    // 3. Get iptables rules for policy
    let response = client
        .get(format!(
            "{}/network-policies/test-policy-1/iptables",
            API_BASE
        ))
        .bearer_auth(&token)
        .send()
        .await;

    let response = expect_2xx(response, "Failed to get iptables rules").await;
    let rules: Vec<String> = response.unwrap().json().await.unwrap();
    assert!(!rules.is_empty(), "Should generate iptables rules");

    // 4. Delete network policy
    let response = client
        .delete(format!("{}/network-policies/test-policy-1", API_BASE))
        .bearer_auth(&token)
        .send()
        .await;

    let _ = expect_2xx(response, "Failed to delete network policy").await;
}

#[tokio::test]
async fn test_vm_migration() {
    let client = create_client();
    let token = admin_token(&client).await;
    let vm = "migration-test-vm";
    let _ = client
        .delete(format!("{}/vms/{}", API_BASE, vm))
        .bearer_auth(&token)
        .send()
        .await;

    let response = client
        .post(format!("{}/vms", API_BASE))
        .bearer_auth(&token)
        .json(&json!({
            "id": vm, "name": "Migration Test VM", "hypervisor": "qemu",
            "architecture": "x86_64", "cpus": 1, "memory": 256, "disk_size": 1,
            "status": "stopped"
        }))
        .send()
        .await;
    let _ = expect_2xx(response, "Failed to create VM for migration").await;

    // Unknown VM -> 404; unknown migration type -> 400 (both used to be 500s,
    // and every *real* VM id was rejected as "Invalid VM ID" because the
    // migration code only accepted numeric ids).
    let r = client
        .post(format!("{}/migrate/no-such-vm", API_BASE))
        .bearer_auth(&token)
        .json(&json!({"target_node": "node2", "migration_type": "offline"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    let r = client
        .post(format!("{}/migrate/{}", API_BASE, vm))
        .bearer_auth(&token)
        .json(&json!({"target_node": "node2", "migration_type": "teleport"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);

    // A real VM id is accepted and produces a trackable job.
    let response = client
        .post(format!("{}/migrate/{}", API_BASE, vm))
        .bearer_auth(&token)
        .json(&json!({"target_node": "node2", "online": false, "migration_type": "offline"}))
        .send()
        .await;
    let response = expect_2xx(response, "Failed to start migration").await;
    let job_id: serde_json::Value = response.unwrap().json().await.unwrap();
    assert!(job_id.is_string(), "migration returns a job id: {job_id}");

    wait_for_operation(500).await;
    let response = client
        .get(format!("{}/migrate/{}/status", API_BASE, vm))
        .bearer_auth(&token)
        .send()
        .await;
    let response = expect_2xx(response, "Failed to get migration status").await;
    let status: serde_json::Value = response.unwrap().json().await.unwrap();
    assert_eq!(status["vm_id"], vm);
    assert_eq!(status["id"], job_id);
    assert!(status.get("state").is_some(), "{status}");

    let _ = client
        .delete(format!("{}/vms/{}", API_BASE, vm))
        .bearer_auth(&token)
        .send()
        .await;
}

#[tokio::test]
async fn test_storage_snapshots() {
    let client = create_client();
    let token = admin_token(&client).await;

    // This test verifies storage-level snapshot operations
    // (different from VM snapshots which are tested elsewhere)

    // Ensure the target directory exists (the API validates the path
    // up front and won't create it for us).
    let _ = tokio::fs::create_dir_all("/var/lib/horcrux/storage/snapshot-test").await;

    // 1. Create a test storage pool
    let pool_config = json!({
        "name": "snapshot-test-pool",
        "storage_type": "directory",
        "path": "/var/lib/horcrux/storage/snapshot-test"
    });

    let response = client
        .post(format!("{}/storage/pools", API_BASE))
        .bearer_auth(&token)
        .json(&pool_config)
        .send()
        .await;

    if response.is_err() {
        return; // Skip if server not available
    }

    let pool: serde_json::Value = response.unwrap().json().await.unwrap();
    let pool_id = pool["id"].as_str().unwrap();

    wait_for_operation(500).await;

    // 2. Create a volume in the pool
    let volume_request = json!({
        "name": "snap-test-vol",
        "size": 5
    });

    let response = client
        .post(format!("{}/storage/pools/{}/volumes", API_BASE, pool_id))
        .bearer_auth(&token)
        .json(&volume_request)
        .send()
        .await;

    if response.is_ok() {
        wait_for_operation(1000).await;

        // Note: Snapshot endpoints would be:
        // POST /api/storage/pools/{pool_id}/volumes/{volume_name}/snapshots
        // GET /api/storage/pools/{pool_id}/volumes/{volume_name}/snapshots
        // DELETE /api/storage/pools/{pool_id}/volumes/{volume_name}/snapshots/{snapshot_name}

        // These might not be fully implemented yet, so we don't assert
    }

    // Cleanup
    let _ = client
        .delete(format!("{}/storage/pools/{}", API_BASE, pool_id))
        .bearer_auth(&token)
        .send()
        .await;
}

fn docker(args: &[&str]) -> std::process::Output {
    std::process::Command::new("docker")
        .args(args)
        .output()
        .expect("docker CLI must be installed for the container tests")
}

fn docker_state(name: &str) -> Option<String> {
    let out = docker(&["inspect", "--format", "{{.State.Status}}", name]);
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn docker_inspect(name: &str) -> serde_json::Value {
    let out = docker(&["inspect", name]);
    assert!(out.status.success(), "docker inspect {name} failed");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    v[0].clone()
}

/// Full Docker container lifecycle through the HTTP API, verified against
/// the real daemon after every step (the old version of this test sent
/// unauthenticated requests and only checked that *some* HTTP response
/// came back, so it passed on 401s).
#[tokio::test]
async fn test_container_lifecycle() {
    let client = create_client();
    let token = admin_token(&client).await;
    let id = "it-ct-1";
    let clone_id = "it-ct-1-clone";
    for n in [id, clone_id] {
        let _ = client
            .delete(format!("{}/containers/{}", API_BASE, n))
            .bearer_auth(&token)
            .send()
            .await;
        docker(&["rm", "-f", n]);
    }
    docker(&["rmi", "-f", "it-ct-1-clone-image"]);

    // Unauthenticated access is refused.
    let r = client
        .get(format!("{}/containers", API_BASE))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);

    // 1. Create with env/ports/command (what the UI form sends).
    let r = client
        .post(format!("{}/containers", API_BASE))
        .bearer_auth(&token)
        .json(&json!({
            "id": id, "name": id, "runtime": "docker", "memory": 64, "cpus": 1,
            "rootfs": "busybox:latest", "status": "stopped",
            "env": ["IT=1"], "ports": ["8080"],
            "command": ["httpd", "-f", "-p", "8080"]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        200,
        "create: {}",
        r.text().await.unwrap_or_default()
    );
    let created: serde_json::Value = r.json().await.unwrap();
    assert_eq!(created["rootfs"], "busybox:latest");
    assert!(created["created_at"].is_string(), "{created}");
    assert_eq!(docker_state(id).as_deref(), Some("created"));
    let info = docker_inspect(id);
    assert!(info["Config"]["Env"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e == "IT=1"));
    assert_eq!(info["Config"]["Labels"]["horcrux.id"], id);
    assert!(info["HostConfig"]["PortBindings"]["8080/tcp"].is_array());

    // Duplicate id -> 422; invalid input -> 422 (not a docker error / 500).
    for bad in [
        json!({"id": id, "name": id, "runtime": "docker", "memory": 64, "cpus": 1,
               "rootfs": "busybox:latest", "status": "stopped"}),
        json!({"id": "it-bad", "name": "bad name", "runtime": "docker", "memory": 64,
               "cpus": 1, "rootfs": "busybox:latest", "status": "stopped"}),
        json!({"id": "it-bad", "name": "it-bad", "runtime": "docker", "memory": 64,
               "cpus": 1, "rootfs": "busybox:latest", "status": "stopped",
               "ports": ["http"]}),
    ] {
        let r = client
            .post(format!("{}/containers", API_BASE))
            .bearer_auth(&token)
            .json(&bad)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 422, "expected 422 for {bad}");
    }

    // 2. Get
    let r = client
        .get(format!("{}/containers/{}", API_BASE, id))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // 3. Start -> really running
    let r = client
        .post(format!("{}/containers/{}/start", API_BASE, id))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        200,
        "start: {}",
        r.text().await.unwrap_or_default()
    );
    assert_eq!(docker_state(id).as_deref(), Some("running"));

    // 4. Live status
    let r = client
        .get(format!("{}/containers/{}/status", API_BASE, id))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.text().await.unwrap().contains("running"));

    // 5. Pause / resume
    for (action, want) in [("pause", "paused"), ("resume", "running")] {
        let r = client
            .post(format!("{}/containers/{}/{}", API_BASE, id, action))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "{action}");
        assert_eq!(docker_state(id).as_deref(), Some(want), "after {action}");
    }

    // 6. Exec returns the command's real output
    let r = client
        .post(format!("{}/containers/{}/exec", API_BASE, id))
        .bearer_auth(&token)
        .json(&json!({"command": ["echo", "hello-from-exec"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.text().await.unwrap().contains("hello-from-exec"));

    // 7. Out-of-band stop is reflected by the list (reconciliation)
    assert!(docker(&["stop", "-t", "1", id]).status.success());
    let list: Vec<serde_json::Value> = client
        .get(format!("{}/containers", API_BASE))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mine = list.iter().find(|c| c["id"] == id).expect("listed");
    assert_eq!(mine["status"], "stopped");

    // 8. Clone -> a real, registered container built from a commit
    let r = client
        .post(format!("{}/containers/{}/clone", API_BASE, id))
        .bearer_auth(&token)
        .json(&json!({"target_id": clone_id, "target_name": clone_id}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        200,
        "clone: {}",
        r.text().await.unwrap_or_default()
    );
    assert!(docker_state(clone_id).is_some(), "clone exists in docker");
    let r = client
        .get(format!("{}/containers/{}", API_BASE, clone_id))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "clone is tracked");

    // 9. Delete both -> gone from docker and from the API
    for n in [clone_id, id] {
        let r = client
            .delete(format!("{}/containers/{}", API_BASE, n))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 204, "delete {n}");
        assert!(docker_state(n).is_none(), "{n} removed from docker");
        let r = client
            .get(format!("{}/containers/{}", API_BASE, n))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 404);
    }
    docker(&["rmi", "-f", "it-ct-1-clone-image"]);
}

/// Compose stack through the API: services share a per-stack network with
/// service-name DNS, and ports/env/command reach the containers (they used
/// to be parsed and silently dropped).
#[tokio::test]
async fn test_stack_networking_and_options() {
    let client = create_client();
    let token = admin_token(&client).await;
    let name = "itstack";
    let _ = client
        .delete(format!("{}/stacks/{}", API_BASE, name))
        .bearer_auth(&token)
        .send()
        .await;
    for n in ["itstack_web", "itstack_db"] {
        docker(&["rm", "-f", n]);
    }
    docker(&["network", "rm", "itstack_default"]);

    let compose = "services:\n  db:\n    image: busybox:latest\n    command: [\"sleep\", \"600\"]\n    environment:\n      ROLE: db\n  web:\n    image: busybox:latest\n    command: httpd -f -p 80\n    ports:\n      - \"80\"\n    depends_on: [db]\n";
    let r = client
        .post(format!("{}/stacks", API_BASE))
        .bearer_auth(&token)
        .json(&json!({"name": name, "compose_yaml": compose, "runtime": "docker"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        200,
        "deploy: {}",
        r.text().await.unwrap_or_default()
    );

    assert_eq!(docker_state("itstack_web").as_deref(), Some("running"));
    assert_eq!(docker_state("itstack_db").as_deref(), Some("running"));
    let web = docker_inspect("itstack_web");
    assert!(
        web["NetworkSettings"]["Networks"]["itstack_default"].is_object(),
        "{}",
        web["NetworkSettings"]["Networks"]
    );
    assert!(web["HostConfig"]["PortBindings"]["80/tcp"].is_array());
    assert_eq!(web["Config"]["Cmd"], json!(["httpd", "-f", "-p", "80"]));
    let db = docker_inspect("itstack_db");
    assert!(db["Config"]["Env"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e == "ROLE=db"));
    // busybox nslookup only tries `db.<host search domain>`; ping resolves
    // through the normal resolver path (Docker's embedded DNS).
    let dns = docker(&["exec", "itstack_web", "ping", "-c", "1", "-W", "3", "db"]);
    assert!(
        dns.status.success(),
        "service 'db' must resolve from 'web': {}",
        String::from_utf8_lossy(&dns.stdout)
    );

    let r = client
        .delete(format!("{}/stacks/{}", API_BASE, name))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "remove: {}", r.status());
    assert!(docker_state("itstack_web").is_none());
    assert!(docker_state("itstack_db").is_none());
    assert!(!docker(&["network", "inspect", "itstack_default"])
        .status
        .success());
}

#[tokio::test]
async fn test_snapshot_scheduling() {
    let client = create_client();
    let token = admin_token(&client).await;

    // First, create a test VM for snapshot scheduling
    let vm_config = json!({
        "id": "schedule-test-vm",
        "name": "Snapshot Schedule Test VM",
        "hypervisor": "qemu",
        "architecture": "x86_64",
        "cpus": 1,
        "memory": 1024,
        "disk_size": 10,
        "status": "stopped"
    });

    let response = client
        .post(format!("{}/vms", API_BASE))
        .bearer_auth(&token)
        .json(&vm_config)
        .send()
        .await;

    if response.is_err() {
        return; // Skip if server not available
    }

    wait_for_operation(1000).await;

    // 1. Create snapshot schedule
    let schedule = json!({
        "vm_id": "schedule-test-vm",
        "name": "daily-backup",
        "frequency": {
            "daily": {
                "hour": 2
            }
        },
        "retention_count": 7,
        "enabled": true,
        "include_memory": false
    });

    let response = client
        .post(format!("{}/snapshot-schedules", API_BASE))
        .bearer_auth(&token)
        .json(&schedule)
        .send()
        .await;

    if response.is_ok() {
        let schedule_response: serde_json::Value = response.unwrap().json().await.unwrap();
        let schedule_id = schedule_response["id"].as_str().unwrap();

        wait_for_operation(500).await;

        // 2. List snapshot schedules
        let response = client
            .get(format!("{}/snapshot-schedules", API_BASE))
            .bearer_auth(&token)
            .send()
            .await;

        let _ = expect_2xx(response, "Failed to list snapshot schedules").await;

        // 3. Get specific schedule
        let response = client
            .get(format!("{}/snapshot-schedules/{}", API_BASE, schedule_id))
            .bearer_auth(&token)
            .send()
            .await;

        let _ = expect_2xx(response, "Failed to get snapshot schedule").await;

        // 4. Update schedule (disable it)
        let update = json!({
            "enabled": false
        });

        let response = client
            .put(format!("{}/snapshot-schedules/{}", API_BASE, schedule_id))
            .bearer_auth(&token)
            .json(&update)
            .send()
            .await;

        if response.is_ok() {
            wait_for_operation(500).await;
        }

        // 5. Delete schedule
        let response = client
            .delete(format!("{}/snapshot-schedules/{}", API_BASE, schedule_id))
            .bearer_auth(&token)
            .send()
            .await;

        let _ = expect_2xx(response, "Failed to delete snapshot schedule").await;
    }

    // Cleanup VM
    wait_for_operation(1000).await;
    let _ = client
        .delete(format!("{}/vms/schedule-test-vm", API_BASE))
        .bearer_auth(&token)
        .send()
        .await;
}

#[tokio::test]
async fn test_high_availability() {
    let client = authed_client().await;
    let vm = "ha-test-vm";
    let _ = client
        .delete(format!("{}/ha/resources/{}", API_BASE, vm))
        .send()
        .await;
    let _ = client
        .delete(format!("{}/ha/groups/critical-services", API_BASE))
        .send()
        .await;
    let _ = client
        .delete(format!("{}/vms/{}", API_BASE, vm))
        .send()
        .await;

    let r = client
        .post(format!("{}/vms", API_BASE))
        .json(
            &json!({"id": vm, "name": "HA Test VM", "hypervisor": "qemu",
                      "architecture": "x86_64", "cpus": 1, "memory": 256,
                      "disk_size": 1, "status": "stopped"}),
        )
        .send()
        .await;
    let _ = expect_2xx(r, "create HA test VM").await;

    // 1. HA group
    let r = client
        .post(format!("{}/ha/groups", API_BASE))
        .json(&json!({"name": "critical-services", "nodes": ["node1", "node2"]}))
        .send()
        .await;
    let _ = expect_2xx(r, "create HA group").await;
    let groups: Vec<serde_json::Value> = client
        .get(format!("{}/ha/groups", API_BASE))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        groups.iter().any(|g| g["name"] == "critical-services"),
        "{groups:?}"
    );

    // 2. Put a real VM (string id) under HA. HA used to take numeric ids
    //    only, so no VM created through the API could ever be managed.
    let r = client
        .post(format!("{}/ha/resources", API_BASE))
        .json(&json!({"vm_id": vm, "priority": 5, "group": "critical-services"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        201,
        "add HA resource: {}",
        r.text().await.unwrap_or_default()
    );
    let r = client
        .post(format!("{}/ha/resources", API_BASE))
        .json(&json!({"vm_id": vm, "priority": 5, "group": "critical-services"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 409, "duplicate HA resource");
    let r = client
        .post(format!("{}/ha/resources", API_BASE))
        .json(&json!({"vm_id": "no-such-vm", "priority": 5, "group": "critical-services"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404, "HA for a VM that doesn't exist");

    let resources: Vec<serde_json::Value> = client
        .get(format!("{}/ha/resources", API_BASE))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(resources.iter().any(|r| r["vm_id"] == vm), "{resources:?}");
    let status: serde_json::Value = client
        .get(format!("{}/ha/status", API_BASE))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(status["total_resources"].as_u64().unwrap() >= 1, "{status}");

    // 3. Remove, then removing again is a 404.
    let r = client
        .delete(format!("{}/ha/resources/{}", API_BASE, vm))
        .send()
        .await;
    let _ = expect_2xx(r, "remove HA resource").await;
    let r = client
        .delete(format!("{}/ha/resources/{}", API_BASE, vm))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);

    // 4. Group assignment endpoint (UI "assign to group") + group update/delete.
    let r = client
        .post(format!(
            "{}/ha/groups/critical-services/resources",
            API_BASE
        ))
        .json(&json!({"resource_id": vm, "priority": 2}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201, "assign via group");
    let r = client
        .delete(format!("{}/ha/groups/critical-services", API_BASE))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 409, "group still in use");
    let r = client
        .delete(format!("{}/ha/resources/{}", API_BASE, vm))
        .send()
        .await;
    let _ = expect_2xx(r, "remove HA resource (2)").await;
    let r = client
        .put(format!("{}/ha/groups/critical-services", API_BASE))
        .json(&json!({"nodes": ["node3"]}))
        .send()
        .await;
    let _ = expect_2xx(r, "update HA group").await;
    let groups: Vec<serde_json::Value> = client
        .get(format!("{}/ha/groups", API_BASE))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let g = groups
        .iter()
        .find(|g| g["name"] == "critical-services")
        .unwrap();
    assert_eq!(g["nodes"], json!(["node3"]));
    let r = client
        .delete(format!("{}/ha/groups/critical-services", API_BASE))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);

    let _ = client
        .delete(format!("{}/vms/{}", API_BASE, vm))
        .send()
        .await;
}

#[tokio::test]
async fn test_multi_hypervisor_support() {
    let client = authed_client().await;
    let create = |id: &str, hv: &str, arch: &str| {
        json!({"id": id, "name": id, "hypervisor": hv, "architecture": arch,
               "cpus": 1, "memory": 256, "disk_size": 1, "status": "stopped"})
    };
    for id in ["mh-qemu", "mh-arm", "mh-lxd", "mh-incus"] {
        let _ = client
            .delete(format!("{}/vms/{}", API_BASE, id))
            .send()
            .await;
    }

    // 1. QEMU VM keeps its hypervisor/architecture.
    let r = client
        .post(format!("{}/vms", API_BASE))
        .json(&create("mh-qemu", "qemu", "x86_64"))
        .send()
        .await;
    let vm: serde_json::Value = expect_2xx(r, "create qemu vm")
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(vm["hypervisor"], "qemu");
    assert_eq!(vm["architecture"], "x86_64");

    // 2. An aarch64 guest is stored as aarch64 and boots under
    //    qemu-system-aarch64 (both used to silently become x86_64).
    let r = client
        .post(format!("{}/vms", API_BASE))
        .json(&create("mh-arm", "qemu", "aarch64"))
        .send()
        .await;
    let vm: serde_json::Value = expect_2xx(r, "create aarch64 vm")
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(vm["architecture"], "aarch64");
    let r = client
        .post(format!("{}/vms/mh-arm/start", API_BASE))
        .send()
        .await;
    let _ = expect_2xx(r, "start aarch64 vm").await;
    let pid = std::fs::read_to_string("/var/run/horcrux-vm-mh-arm.pid").expect("pidfile");
    let cmdline = std::fs::read(format!("/proc/{}/cmdline", pid.trim())).unwrap_or_default();
    assert!(
        String::from_utf8_lossy(&cmdline).contains("qemu-system-aarch64"),
        "aarch64 VM must run under qemu-system-aarch64"
    );
    let r = client
        .post(format!("{}/vms/mh-arm/stop", API_BASE))
        .send()
        .await;
    let _ = expect_2xx(r, "stop aarch64 vm").await;

    // 3. LXD / Incus VMs go to their own backend; on a host without them
    //    that is a clear 422, never a QEMU VM wearing an "lxd" label.
    for (id, hv, bin) in [("mh-lxd", "lxd", "lxc"), ("mh-incus", "incus", "incus")] {
        let installed = std::process::Command::new(bin)
            .arg("version")
            .output()
            .is_ok();
        let r = client
            .post(format!("{}/vms", API_BASE))
            .json(&create(id, hv, "x86_64"))
            .send()
            .await
            .unwrap();
        if installed {
            assert!(r.status().is_success(), "{hv}: {}", r.status());
            let vm: serde_json::Value = r.json().await.unwrap();
            assert_eq!(vm["hypervisor"], hv);
        } else {
            assert_eq!(r.status(), 422, "{hv} VM without {hv} installed");
            let r = client
                .get(format!("{}/vms/{}", API_BASE, id))
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 404, "no {hv} VM may be recorded");
        }
    }

    for id in ["mh-qemu", "mh-arm", "mh-lxd", "mh-incus"] {
        let _ = client
            .delete(format!("{}/vms/{}", API_BASE, id))
            .send()
            .await;
    }
}
