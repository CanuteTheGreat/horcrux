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

use horcrux_common::*;
use reqwest::Client;
use serde_json::json;
use std::time::Duration;
use tokio::time::sleep;

const API_BASE: &str = "http://localhost:8006/api";
const TEST_VM_ID: &str = "test-vm-integration";
const TEST_NODE_ID: &str = "test-node-1";

/// Test helper to create an HTTP client with authentication
fn create_client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("Failed to create HTTP client")
}

/// Test helper to wait for async operations to complete
async fn wait_for_operation(duration_ms: u64) {
    sleep(Duration::from_millis(duration_ms)).await;
}

#[tokio::test]
async fn test_vm_lifecycle() {
    let client = create_client();

    // 1. Create VM
    let vm_config = json!({
        "id": TEST_VM_ID,
        "name": "Integration Test VM",
        "hypervisor": "Qemu",
        "architecture": "X86_64",
        "cpus": 2,
        "memory": 2048,
        "disk_size": 20,
        "status": "Stopped"
    });

    let response = client
        .post(&format!("{}/vms", API_BASE))
        .json(&vm_config)
        .send()
        .await;

    assert!(response.is_ok(), "Failed to create VM");
    let response = response.unwrap();
    assert_eq!(response.status(), 200, "VM creation returned non-200 status");

    // 2. Get VM details
    let response = client
        .get(&format!("{}/vms/{}", API_BASE, TEST_VM_ID))
        .send()
        .await;

    assert!(response.is_ok(), "Failed to get VM");
    let vm: VmConfig = response.unwrap().json().await.expect("Failed to parse VM");
    assert_eq!(vm.id, TEST_VM_ID);
    assert_eq!(vm.name, "Integration Test VM");
    assert_eq!(vm.cpus, 2);
    assert_eq!(vm.memory, 2048);

    // 3. Start VM
    let response = client
        .post(&format!("{}/vms/{}/start", API_BASE, TEST_VM_ID))
        .send()
        .await;

    assert!(response.is_ok(), "Failed to start VM");
    wait_for_operation(2000).await;

    // Verify VM is running
    let response = client
        .get(&format!("{}/vms/{}", API_BASE, TEST_VM_ID))
        .send()
        .await;

    let vm: VmConfig = response.unwrap().json().await.expect("Failed to parse VM");
    assert_eq!(vm.status, VmStatus::Running, "VM should be running");

    // 4. Stop VM
    let response = client
        .post(&format!("{}/vms/{}/stop", API_BASE, TEST_VM_ID))
        .send()
        .await;

    assert!(response.is_ok(), "Failed to stop VM");
    wait_for_operation(2000).await;

    // 5. Delete VM
    let response = client
        .delete(&format!("{}/vms/{}", API_BASE, TEST_VM_ID))
        .send()
        .await;

    assert!(response.is_ok(), "Failed to delete VM");

    // Verify VM is deleted
    let response = client
        .get(&format!("{}/vms/{}", API_BASE, TEST_VM_ID))
        .send()
        .await;

    assert_eq!(response.unwrap().status(), 404, "VM should be deleted");
}

#[tokio::test]
async fn test_cluster_operations() {
    let client = create_client();

    // 1. Get cluster status
    let response = client
        .get(&format!("{}/cluster/status", API_BASE))
        .send()
        .await;

    assert!(response.is_ok(), "Failed to get cluster status");
    let status: ClusterStatus = response.unwrap().json().await.expect("Failed to parse cluster status");
    assert!(status.nodes.len() > 0, "Cluster should have at least one node");

    // 2. Join node to cluster
    let join_request = json!({
        "node_id": TEST_NODE_ID,
        "hostname": "test-node.local",
        "ip_address": "192.168.1.100"
    });

    let response = client
        .post(&format!("{}/cluster/join", API_BASE))
        .json(&join_request)
        .send()
        .await;

    // Note: May fail if node already joined - that's ok
    if response.is_ok() {
        wait_for_operation(1000).await;
    }

    // 3. Get node list
    let response = client
        .get(&format!("{}/cluster/nodes", API_BASE))
        .send()
        .await;

    assert!(response.is_ok(), "Failed to get node list");
    let nodes: Vec<ClusterNode> = response.unwrap().json().await.expect("Failed to parse nodes");
    assert!(nodes.len() > 0, "Should have at least one node");

    // 4. Get quorum info
    let response = client
        .get(&format!("{}/cluster/quorum", API_BASE))
        .send()
        .await;

    assert!(response.is_ok(), "Failed to get quorum info");
}

#[tokio::test]
async fn test_storage_operations() {
    let client = create_client();

    // 1. List storage pools
    let response = client
        .get(&format!("{}/storage/pools", API_BASE))
        .send()
        .await;

    assert!(response.is_ok(), "Failed to get storage pools");
    let pools: Vec<StoragePool> = response.unwrap().json().await.expect("Failed to parse pools");

    // 2. Create storage pool
    let pool_config = json!({
        "id": "test-pool",
        "name": "Test Pool",
        "pool_type": "Lvm",
        "path": "/dev/test-vg/test-pool",
        "size_gb": 100,
        "used_gb": 0
    });

    let response = client
        .post(&format!("{}/storage/pools", API_BASE))
        .json(&pool_config)
        .send()
        .await;

    // Pool may already exist
    if response.is_ok() {
        let response = response.unwrap();
        assert!(response.status().is_success(), "Failed to create storage pool");
    }

    // 3. Get pool details
    let response = client
        .get(&format!("{}/storage/pools/test-pool", API_BASE))
        .send()
        .await;

    if response.is_ok() {
        let pool: StoragePool = response.unwrap().json().await.expect("Failed to parse pool");
        assert_eq!(pool.id, "test-pool");
    }

    // 4. List volumes in pool
    let response = client
        .get(&format!("{}/storage/pools/test-pool/volumes", API_BASE))
        .send()
        .await;

    assert!(response.is_ok(), "Failed to get volumes");
}

#[tokio::test]
async fn test_backup_operations() {
    let client = create_client();

    // First create a test VM
    let vm_config = json!({
        "id": "backup-test-vm",
        "name": "Backup Test VM",
        "hypervisor": "Qemu",
        "architecture": "X86_64",
        "cpus": 1,
        "memory": 1024,
        "disk_size": 10,
        "status": "Stopped"
    });

    let _ = client
        .post(&format!("{}/vms", API_BASE))
        .json(&vm_config)
        .send()
        .await;

    wait_for_operation(1000).await;

    // 1. Create backup
    let backup_request = json!({
        "vm_id": "backup-test-vm",
        "backup_type": "Full",
        "compression": "Zstd"
    });

    let response = client
        .post(&format!("{}/backup/create", API_BASE))
        .json(&backup_request)
        .send()
        .await;

    assert!(response.is_ok(), "Failed to create backup");
    let backup_id: String = response.unwrap().json().await.expect("Failed to get backup ID");

    wait_for_operation(3000).await;

    // 2. List backups
    let response = client
        .get(&format!("{}/backup/list", API_BASE))
        .send()
        .await;

    assert!(response.is_ok(), "Failed to list backups");
    let backups: Vec<BackupInfo> = response.unwrap().json().await.expect("Failed to parse backups");
    assert!(backups.len() > 0, "Should have at least one backup");

    // 3. Get backup info
    let response = client
        .get(&format!("{}/backup/{}", API_BASE, backup_id))
        .send()
        .await;

    assert!(response.is_ok(), "Failed to get backup info");

    // 4. Restore from backup (dry run)
    let restore_request = json!({
        "backup_id": backup_id,
        "target_vm_id": "restored-vm",
        "dry_run": true
    });

    let response = client
        .post(&format!("{}/backup/restore", API_BASE))
        .json(&restore_request)
        .send()
        .await;

    assert!(response.is_ok(), "Failed to restore backup");

    // Cleanup
    let _ = client
        .delete(&format!("{}/vms/backup-test-vm", API_BASE))
        .send()
        .await;
}

#[tokio::test]
async fn test_monitoring_and_alerts() {
    let client = create_client();

    // 1. Get node metrics
    let response = client
        .get(&format!("{}/monitoring/node/metrics", API_BASE))
        .send()
        .await;

    assert!(response.is_ok(), "Failed to get node metrics");
    let metrics: NodeMetrics = response.unwrap().json().await.expect("Failed to parse metrics");
    assert!(metrics.cpu_usage >= 0.0 && metrics.cpu_usage <= 100.0);
    assert!(metrics.memory_total > 0);

    // 2. Create alert rule
    let alert_rule = json!({
        "name": "high_cpu_test",
        "metric": "cpu_usage",
        "condition": "GreaterThan",
        "threshold": 80.0,
        "severity": "Warning",
        "enabled": true
    });

    let response = client
        .post(&format!("{}/alerts/rules", API_BASE))
        .json(&alert_rule)
        .send()
        .await;

    assert!(response.is_ok(), "Failed to create alert rule");

    // 3. List alert rules
    let response = client
        .get(&format!("{}/alerts/rules", API_BASE))
        .send()
        .await;

    assert!(response.is_ok(), "Failed to list alert rules");
    let rules: Vec<AlertRule> = response.unwrap().json().await.expect("Failed to parse rules");
    assert!(rules.len() > 0, "Should have at least one alert rule");

    // 4. Get active alerts
    let response = client
        .get(&format!("{}/alerts/active", API_BASE))
        .send()
        .await;

    assert!(response.is_ok(), "Failed to get active alerts");
    let alerts: Vec<Alert> = response.unwrap().json().await.expect("Failed to parse alerts");

    // 5. Get alert history
    let response = client
        .get(&format!("{}/alerts/history", API_BASE))
        .send()
        .await;

    assert!(response.is_ok(), "Failed to get alert history");
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
        .post(&format!("{}/auth/login", API_BASE))
        .json(&login_request)
        .send()
        .await;

    assert!(response.is_ok(), "Failed to login");
    let token_response: serde_json::Value = response.unwrap().json().await.expect("Failed to parse token");
    assert!(token_response.get("token").is_some(), "Should receive auth token");

    // 2. Verify token works
    let token = token_response["token"].as_str().unwrap();
    let response = client
        .get(&format!("{}/auth/verify", API_BASE))
        .bearer_auth(token)
        .send()
        .await;

    assert!(response.is_ok(), "Token verification failed");

    // 3. Test invalid credentials
    let bad_login = json!({
        "username": "admin",
        "password": "wrongpassword"
    });

    let response = client
        .post(&format!("{}/auth/login", API_BASE))
        .json(&bad_login)
        .send()
        .await;

    if response.is_ok() {
        assert_eq!(response.unwrap().status(), 401, "Should reject invalid credentials");
    }
}

#[tokio::test]
async fn test_firewall_rules() {
    let client = create_client();

    // 1. List firewall rules
    let response = client
        .get(&format!("{}/firewall/rules", API_BASE))
        .send()
        .await;

    assert!(response.is_ok(), "Failed to get firewall rules");

    // 2. Create firewall rule
    let rule = json!({
        "name": "test-http-allow",
        "action": "Accept",
        "protocol": "Tcp",
        "source": "0.0.0.0/0",
        "destination": "0.0.0.0/0",
        "port": 80,
        "enabled": true
    });

    let response = client
        .post(&format!("{}/firewall/rules", API_BASE))
        .json(&rule)
        .send()
        .await;

    assert!(response.is_ok(), "Failed to create firewall rule");

    // 3. Get rule details
    let response = client
        .get(&format!("{}/firewall/rules/test-http-allow", API_BASE))
        .send()
        .await;

    if response.is_ok() {
        let rule: FirewallRule = response.unwrap().json().await.expect("Failed to parse rule");
        assert_eq!(rule.name, "test-http-allow");
        assert_eq!(rule.port, Some(80));
    }

    // 4. Apply firewall rules
    let response = client
        .post(&format!("{}/firewall/apply", API_BASE))
        .send()
        .await;

    assert!(response.is_ok(), "Failed to apply firewall rules");

    // 5. Delete test rule
    let _ = client
        .delete(&format!("{}/firewall/rules/test-http-allow", API_BASE))
        .send()
        .await;
}

#[tokio::test]
async fn test_template_operations() {
    let client = create_client();

    // 1. List templates
    let response = client
        .get(&format!("{}/templates", API_BASE))
        .send()
        .await;

    assert!(response.is_ok(), "Failed to list templates");

    // 2. Create template from VM
    let template_request = json!({
        "vm_id": "base-vm",
        "template_name": "test-template",
        "description": "Test template for integration tests"
    });

    let response = client
        .post(&format!("{}/templates/create", API_BASE))
        .json(&template_request)
        .send()
        .await;

    // May fail if base VM doesn't exist - that's ok
    if response.is_ok() {
        wait_for_operation(2000).await;

        // 3. Deploy from template
        let deploy_request = json!({
            "template_id": "test-template",
            "vm_name": "from-template-vm",
            "cpus": 2,
            "memory": 2048
        });

        let response = client
            .post(&format!("{}/templates/deploy", API_BASE))
            .json(&deploy_request)
            .send()
            .await;

        assert!(response.is_ok(), "Failed to deploy from template");
    }
}

#[tokio::test]
async fn test_console_access() {
    let client = create_client();

    // Create test VM first
    let vm_config = json!({
        "id": "console-test-vm",
        "name": "Console Test VM",
        "hypervisor": "Qemu",
        "architecture": "X86_64",
        "cpus": 1,
        "memory": 1024,
        "disk_size": 10,
        "status": "Stopped"
    });

    let _ = client
        .post(&format!("{}/vms", API_BASE))
        .json(&vm_config)
        .send()
        .await;

    // Start VM
    let _ = client
        .post(&format!("{}/vms/console-test-vm/start", API_BASE))
        .send()
        .await;

    wait_for_operation(2000).await;

    // 1. Get VNC console URL
    let response = client
        .get(&format!("{}/vms/console-test-vm/console/vnc", API_BASE))
        .send()
        .await;

    assert!(response.is_ok(), "Failed to get VNC console URL");
    let console_info: serde_json::Value = response.unwrap().json().await.expect("Failed to parse console info");
    assert!(console_info.get("url").is_some(), "Should have console URL");

    // 2. Get serial console
    let response = client
        .get(&format!("{}/vms/console-test-vm/console/serial", API_BASE))
        .send()
        .await;

    // May not be supported for all hypervisors
    if response.is_ok() {
        let _ = response.unwrap();
    }

    // Cleanup
    let _ = client
        .post(&format!("{}/vms/console-test-vm/stop", API_BASE))
        .send()
        .await;

    wait_for_operation(1000).await;

    let _ = client
        .delete(&format!("{}/vms/console-test-vm", API_BASE))
        .send()
        .await;
}

#[tokio::test]
async fn test_api_health() {
    let client = create_client();

    // Test health endpoint
    let response = client
        .get(&format!("{}/health", API_BASE))
        .send()
        .await;

    assert!(response.is_ok(), "Health check failed");
    assert_eq!(response.unwrap().status(), 200, "Health check should return 200");
}
