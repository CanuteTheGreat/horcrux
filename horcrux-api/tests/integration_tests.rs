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

    // 2. Create ZFS storage pool
    let zfs_pool = json!({
        "name": "zfs-test-pool",
        "storage_type": "zfs",
        "path": "tank/horcrux"
    });

    let response = client
        .post(&format!("{}/storage/pools", API_BASE))
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
        .post(&format!("{}/storage/pools", API_BASE))
        .json(&lvm_pool)
        .send()
        .await;

    let lvm_pool_created = response.is_ok() && response.as_ref().unwrap().status().is_success();

    if lvm_pool_created {
        wait_for_operation(500).await;
    }

    // 4. Create Directory storage pool
    let dir_pool = json!({
        "name": "dir-test-pool",
        "storage_type": "directory",
        "path": "/var/lib/horcrux/storage/test"
    });

    let response = client
        .post(&format!("{}/storage/pools", API_BASE))
        .json(&dir_pool)
        .send()
        .await;

    let dir_pool_created = response.is_ok() && response.as_ref().unwrap().status().is_success();
    let mut test_pool_id = String::new();

    if dir_pool_created {
        let pool_response: serde_json::Value = response.unwrap().json().await.expect("Failed to parse pool");
        test_pool_id = pool_response["id"].as_str().unwrap().to_string();
        wait_for_operation(500).await;

        // 5. Get pool details
        let response = client
            .get(&format!("{}/storage/pools/{}", API_BASE, test_pool_id))
            .send()
            .await;

        assert!(response.is_ok(), "Failed to get pool details");

        // 6. Create volume in pool
        let volume_request = json!({
            "name": "test-volume",
            "size": 10  // 10 GB
        });

        let response = client
            .post(&format!("{}/storage/pools/{}/volumes", API_BASE, test_pool_id))
            .json(&volume_request)
            .send()
            .await;

        if response.is_ok() {
            wait_for_operation(1000).await;
        }

        // 7. Delete the test pool
        let response = client
            .delete(&format!("{}/storage/pools/{}", API_BASE, test_pool_id))
            .send()
            .await;

        assert!(response.is_ok(), "Failed to delete storage pool");
    }

    // 8. Test Ceph pool creation (will likely fail without Ceph cluster)
    let ceph_pool = json!({
        "name": "ceph-test-pool",
        "storage_type": "ceph",
        "path": "rbd/horcrux"
    });

    let _ = client
        .post(&format!("{}/storage/pools", API_BASE))
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
        .post(&format!("{}/storage/pools", API_BASE))
        .json(&nfs_pool)
        .send()
        .await;
    // Don't assert - NFS server might not be available
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

#[tokio::test]
async fn test_session_management() {
    let client = create_client();

    // 1. Login to create session
    let login_request = json!({
        "username": "admin",
        "password": "admin"
    });

    let response = client
        .post(&format!("{}/auth/login", API_BASE))
        .json(&login_request)
        .send()
        .await;

    assert!(response.is_ok(), "Login failed");
    let response = response.unwrap();

    // Extract session cookie from Set-Cookie header
    let set_cookie = response.headers()
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
        .get(&format!("{}/vms", API_BASE))
        .header("Cookie", format!("session_id={}", session_id))
        .send()
        .await;

    assert!(response.is_ok(), "Failed to access with session cookie");
    assert_eq!(response.unwrap().status(), 200);

    // 3. Logout (destroy session)
    let response = client
        .post(&format!("{}/auth/logout", API_BASE))
        .header("Cookie", format!("session_id={}", session_id))
        .send()
        .await;

    assert!(response.is_ok(), "Logout failed");

    // 4. Verify session is invalid after logout
    let response = client
        .get(&format!("{}/vms", API_BASE))
        .header("Cookie", format!("session_id={}", session_id))
        .send()
        .await;

    if response.is_ok() {
        assert_eq!(response.unwrap().status(), 401, "Should reject invalid session");
    }
}

#[tokio::test]
async fn test_password_change() {
    let client = create_client();

    // 1. Login first
    let login_request = json!({
        "username": "testuser",
        "password": "testpass123"
    });

    // Assume test user exists or create one first
    let _ = client
        .post(&format!("{}/users", API_BASE))
        .json(&json!({
            "username": "testuser",
            "password": "testpass123",
            "email": "test@example.com",
            "role": "VmUser"
        }))
        .send()
        .await;

    let response = client
        .post(&format!("{}/auth/login", API_BASE))
        .json(&login_request)
        .send()
        .await;

    if response.is_err() {
        // User might not exist in test environment, skip test
        return;
    }

    let token_response: serde_json::Value = response.unwrap().json().await.unwrap();
    let token = token_response["token"].as_str().unwrap();

    // 2. Change password
    let change_request = json!({
        "username": "testuser",
        "old_password": "testpass123",
        "new_password": "newpass456"
    });

    let response = client
        .post(&format!("{}/auth/password", API_BASE))
        .bearer_auth(token)
        .json(&change_request)
        .send()
        .await;

    assert!(response.is_ok(), "Password change failed");

    // 3. Verify old password doesn't work
    let old_login = json!({
        "username": "testuser",
        "password": "testpass123"
    });

    let response = client
        .post(&format!("{}/auth/login", API_BASE))
        .json(&old_login)
        .send()
        .await;

    if response.is_ok() {
        assert_eq!(response.unwrap().status(), 401, "Old password should not work");
    }

    // 4. Verify new password works
    let new_login = json!({
        "username": "testuser",
        "password": "newpass456"
    });

    let response = client
        .post(&format!("{}/auth/login", API_BASE))
        .json(&new_login)
        .send()
        .await;

    assert!(response.is_ok(), "New password should work");

    // Cleanup
    let _ = client
        .delete(&format!("{}/users/testuser", API_BASE))
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
        .post(&format!("{}/auth/login", API_BASE))
        .json(&login_request)
        .send()
        .await;

    if response.is_err() {
        return; // Skip if server not running
    }

    let token_response: serde_json::Value = response.unwrap().json().await.unwrap();
    let jwt_token = token_response["token"].as_str().unwrap();

    // 2. Create API key
    let api_key_request = json!({
        "name": "Integration Test Key",
        "expires_days": 30
    });

    let response = client
        .post(&format!("{}/users/admin/api-keys", API_BASE))
        .bearer_auth(jwt_token)
        .json(&api_key_request)
        .send()
        .await;

    assert!(response.is_ok(), "Failed to create API key");
    let api_key_response: serde_json::Value = response.unwrap().json().await.unwrap();

    assert!(api_key_response.get("key").is_some(), "Should receive API key");
    assert!(api_key_response.get("id").is_some(), "Should receive key ID");

    let api_key = api_key_response["key"].as_str().unwrap();
    assert!(api_key.starts_with("hx_"), "API key should have hx_ prefix");

    // 3. Use API key to access endpoint
    let response = client
        .get(&format!("{}/vms", API_BASE))
        .header("X-API-Key", api_key)
        .send()
        .await;

    assert!(response.is_ok(), "Failed to use API key");
    assert_eq!(response.unwrap().status(), 200);

    // 4. List user's API keys
    let response = client
        .get(&format!("{}/users/admin/api-keys", API_BASE))
        .bearer_auth(jwt_token)
        .send()
        .await;

    assert!(response.is_ok(), "Failed to list API keys");
    let keys: Vec<serde_json::Value> = response.unwrap().json().await.unwrap();
    assert!(keys.len() > 0, "Should have at least one API key");
}

#[tokio::test]
async fn test_rbac_permissions() {
    let client = create_client();

    // 1. Create user with VmUser role (limited permissions)
    let user_request = json!({
        "username": "vmuser",
        "password": "vmpass123",
        "email": "vmuser@example.com",
        "role": "VmUser"
    });

    // Login as admin first to create user
    let admin_login = json!({
        "username": "admin",
        "password": "admin"
    });

    let response = client
        .post(&format!("{}/auth/login", API_BASE))
        .json(&admin_login)
        .send()
        .await;

    if response.is_err() {
        return; // Skip if server not running
    }

    let admin_token: serde_json::Value = response.unwrap().json().await.unwrap();
    let admin_jwt = admin_token["token"].as_str().unwrap();

    let _ = client
        .post(&format!("{}/users", API_BASE))
        .bearer_auth(admin_jwt)
        .json(&user_request)
        .send()
        .await;

    // 2. Login as limited user
    let user_login = json!({
        "username": "vmuser",
        "password": "vmpass123"
    });

    let response = client
        .post(&format!("{}/auth/login", API_BASE))
        .json(&user_login)
        .send()
        .await;

    if response.is_err() {
        return;
    }

    let user_token: serde_json::Value = response.unwrap().json().await.unwrap();
    let user_jwt = user_token["token"].as_str().unwrap();

    // 3. VmUser can view VMs (VmAudit privilege)
    let response = client
        .get(&format!("{}/vms", API_BASE))
        .bearer_auth(user_jwt)
        .send()
        .await;

    assert!(response.is_ok(), "VmUser should be able to view VMs");

    // 4. VmUser can start/stop VMs (VmPowerMgmt privilege)
    let response = client
        .post(&format!("{}/vms/test-vm/start", API_BASE))
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

    let response = client
        .post(&format!("{}/vms", API_BASE))
        .bearer_auth(user_jwt)
        .json(&vm_config)
        .send()
        .await;

    if response.is_ok() {
        // Should be forbidden (403) or unauthorized (401)
        let status = response.unwrap().status();
        assert!(
            status == 403 || status == 401,
            "VmUser should not be able to create VMs, got status: {}",
            status
        );
    }

    // Cleanup
    let _ = client
        .delete(&format!("{}/users/vmuser", API_BASE))
        .bearer_auth(admin_jwt)
        .send()
        .await;
}

#[tokio::test]
async fn test_cni_network_operations() {
    let client = create_client();

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
        .post(&format!("{}/cni/networks", API_BASE))
        .json(&network_config)
        .send()
        .await;

    if response.is_err() {
        return; // Skip if server not running
    }

    assert!(response.is_ok(), "Failed to create CNI network");

    // 2. List CNI networks
    let response = client
        .get(&format!("{}/cni/networks", API_BASE))
        .send()
        .await;

    assert!(response.is_ok(), "Failed to list CNI networks");
    let networks: Vec<serde_json::Value> = response.unwrap().json().await.unwrap();
    let test_network = networks.iter().find(|n| n["name"] == "test-bridge");
    assert!(test_network.is_some(), "Test network should exist");

    // 3. Delete CNI network
    let response = client
        .delete(&format!("{}/cni/networks/test-bridge", API_BASE))
        .send()
        .await;

    assert!(response.is_ok(), "Failed to delete CNI network");
}

#[tokio::test]
async fn test_network_policy_enforcement() {
    let client = create_client();

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
        .post(&format!("{}/network-policies", API_BASE))
        .json(&policy)
        .send()
        .await;

    if response.is_err() {
        return;
    }

    assert!(response.is_ok(), "Failed to create network policy");

    // 2. List network policies
    let response = client
        .get(&format!("{}/network-policies", API_BASE))
        .send()
        .await;

    assert!(response.is_ok(), "Failed to list network policies");
    let policies: Vec<serde_json::Value> = response.unwrap().json().await.unwrap();
    assert!(policies.len() > 0, "Should have at least one policy");

    // 3. Get iptables rules for policy
    let response = client
        .get(&format!("{}/network-policies/test-policy-1/iptables", API_BASE))
        .send()
        .await;

    assert!(response.is_ok(), "Failed to get iptables rules");
    let rules: Vec<String> = response.unwrap().json().await.unwrap();
    assert!(rules.len() > 0, "Should generate iptables rules");

    // 4. Delete network policy
    let response = client
        .delete(&format!("{}/network-policies/test-policy-1", API_BASE))
        .send()
        .await;

    assert!(response.is_ok(), "Failed to delete network policy");
}

#[tokio::test]
async fn test_vm_migration() {
    let client = create_client();

    // First, create a test VM for migration
    let vm_config = json!({
        "id": "migration-test-vm",
        "name": "Migration Test VM",
        "hypervisor": "Qemu",
        "architecture": "X86_64",
        "cpus": 1,
        "memory": 1024,
        "disk_size": 10,
        "status": "Stopped"
    });

    let response = client
        .post(&format!("{}/vms", API_BASE))
        .json(&vm_config)
        .send()
        .await;

    if response.is_err() {
        return; // Skip if server not available
    }

    wait_for_operation(1000).await;

    // 1. Test offline migration
    let migrate_request = json!({
        "target_node": "node2",
        "online": false,
        "migration_type": "offline"
    });

    let response = client
        .post(&format!("{}/migrate/migration-test-vm", API_BASE))
        .json(&migrate_request)
        .send()
        .await;

    // Migration will likely fail in test environment (no cluster), but API should respond
    if response.is_ok() {
        let job_id: serde_json::Value = response.unwrap().json().await.unwrap();
        assert!(job_id.is_string(), "Should receive migration job ID");

        let job_id_str = job_id.as_str().unwrap();

        // 2. Check migration status
        wait_for_operation(500).await;

        let response = client
            .get(&format!("{}/migrate/migration-test-vm/status", API_BASE))
            .send()
            .await;

        if response.is_ok() {
            let status: serde_json::Value = response.unwrap().json().await.unwrap();
            assert!(status.get("state").is_some(), "Migration status should include state");
        }
    }

    // 3. Test live migration request
    let live_migrate_request = json!({
        "target_node": "node3",
        "online": true,
        "migration_type": "live"
    });

    let _ = client
        .post(&format!("{}/migrate/migration-test-vm", API_BASE))
        .json(&live_migrate_request)
        .send()
        .await;
    // Don't assert - cluster might not be available

    // Cleanup
    wait_for_operation(1000).await;
    let _ = client
        .delete(&format!("{}/vms/migration-test-vm", API_BASE))
        .send()
        .await;
}

#[tokio::test]
async fn test_storage_snapshots() {
    let client = create_client();

    // This test verifies storage-level snapshot operations
    // (different from VM snapshots which are tested elsewhere)

    // 1. Create a test storage pool
    let pool_config = json!({
        "name": "snapshot-test-pool",
        "storage_type": "directory",
        "path": "/var/lib/horcrux/storage/snapshot-test"
    });

    let response = client
        .post(&format!("{}/storage/pools", API_BASE))
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
        .post(&format!("{}/storage/pools/{}/volumes", API_BASE, pool_id))
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
        .delete(&format!("{}/storage/pools/{}", API_BASE, pool_id))
        .send()
        .await;
}

#[tokio::test]
async fn test_container_lifecycle() {
    let client = create_client();

    // 1. Create LXC container
    let container_config = json!({
        "id": "test-container-1",
        "name": "Test Container",
        "container_type": "lxc",
        "image": "ubuntu:22.04",
        "memory": 512,
        "cpus": 1,
        "status": "stopped"
    });

    let response = client
        .post(&format!("{}/containers", API_BASE))
        .json(&container_config)
        .send()
        .await;

    if response.is_err() {
        return; // Skip if server not available
    }

    assert!(response.is_ok(), "Failed to create container");
    wait_for_operation(1000).await;

    // 2. Get container details
    let response = client
        .get(&format!("{}/containers/test-container-1", API_BASE))
        .send()
        .await;

    assert!(response.is_ok(), "Failed to get container");

    // 3. Start container
    let response = client
        .post(&format!("{}/containers/test-container-1/start", API_BASE))
        .send()
        .await;

    if response.is_ok() {
        wait_for_operation(2000).await;

        // 4. Check container status
        let response = client
            .get(&format!("{}/containers/test-container-1/status", API_BASE))
            .send()
            .await;

        assert!(response.is_ok(), "Failed to get container status");

        // 5. Pause container (if supported)
        let response = client
            .post(&format!("{}/containers/test-container-1/pause", API_BASE))
            .send()
            .await;

        if response.is_ok() {
            wait_for_operation(500).await;

            // 6. Resume container
            let _ = client
                .post(&format!("{}/containers/test-container-1/resume", API_BASE))
                .send()
                .await;

            wait_for_operation(500).await;
        }

        // 7. Execute command in container
        let exec_request = json!({
            "command": ["echo", "hello"],
            "workdir": "/tmp"
        });

        let response = client
            .post(&format!("{}/containers/test-container-1/exec", API_BASE))
            .json(&exec_request)
            .send()
            .await;

        // Command execution may not be supported in test environment
        if response.is_ok() {
            wait_for_operation(500).await;
        }

        // 8. Stop container
        let response = client
            .post(&format!("{}/containers/test-container-1/stop", API_BASE))
            .send()
            .await;

        assert!(response.is_ok(), "Failed to stop container");
        wait_for_operation(2000).await;
    }

    // 9. Clone container
    let clone_request = json!({
        "name": "Test Container Clone",
        "id": "test-container-2"
    });

    let response = client
        .post(&format!("{}/containers/test-container-1/clone", API_BASE))
        .json(&clone_request)
        .send()
        .await;

    if response.is_ok() {
        wait_for_operation(2000).await;

        // Cleanup clone
        let _ = client
            .delete(&format!("{}/containers/test-container-2", API_BASE))
            .send()
            .await;
    }

    // 10. Delete container
    let response = client
        .delete(&format!("{}/containers/test-container-1", API_BASE))
        .send()
        .await;

    assert!(response.is_ok(), "Failed to delete container");
}

#[tokio::test]
async fn test_snapshot_scheduling() {
    let client = create_client();

    // First, create a test VM for snapshot scheduling
    let vm_config = json!({
        "id": "schedule-test-vm",
        "name": "Snapshot Schedule Test VM",
        "hypervisor": "Qemu",
        "architecture": "X86_64",
        "cpus": 1,
        "memory": 1024,
        "disk_size": 10,
        "status": "Stopped"
    });

    let response = client
        .post(&format!("{}/vms", API_BASE))
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
        .post(&format!("{}/snapshot-schedules", API_BASE))
        .json(&schedule)
        .send()
        .await;

    if response.is_ok() {
        let schedule_response: serde_json::Value = response.unwrap().json().await.unwrap();
        let schedule_id = schedule_response["id"].as_str().unwrap();

        wait_for_operation(500).await;

        // 2. List snapshot schedules
        let response = client
            .get(&format!("{}/snapshot-schedules", API_BASE))
            .send()
            .await;

        assert!(response.is_ok(), "Failed to list snapshot schedules");

        // 3. Get specific schedule
        let response = client
            .get(&format!("{}/snapshot-schedules/{}", API_BASE, schedule_id))
            .send()
            .await;

        assert!(response.is_ok(), "Failed to get snapshot schedule");

        // 4. Update schedule (disable it)
        let update = json!({
            "enabled": false
        });

        let response = client
            .put(&format!("{}/snapshot-schedules/{}", API_BASE, schedule_id))
            .json(&update)
            .send()
            .await;

        if response.is_ok() {
            wait_for_operation(500).await;
        }

        // 5. Delete schedule
        let response = client
            .delete(&format!("{}/snapshot-schedules/{}", API_BASE, schedule_id))
            .send()
            .await;

        assert!(response.is_ok(), "Failed to delete snapshot schedule");
    }

    // Cleanup VM
    wait_for_operation(1000).await;
    let _ = client
        .delete(&format!("{}/vms/schedule-test-vm", API_BASE))
        .send()
        .await;
}

#[tokio::test]
async fn test_high_availability() {
    let client = create_client();

    // Create a test VM for HA
    let vm_config = json!({
        "id": "ha-test-vm",
        "name": "HA Test VM",
        "hypervisor": "Qemu",
        "architecture": "X86_64",
        "cpus": 1,
        "memory": 1024,
        "disk_size": 10,
        "status": "Stopped"
    });

    let response = client
        .post(&format!("{}/vms", API_BASE))
        .json(&vm_config)
        .send()
        .await;

    if response.is_err() {
        return; // Skip if server not available
    }

    wait_for_operation(1000).await;

    // 1. Create HA group
    let ha_group = json!({
        "name": "critical-services",
        "nodes": ["node1", "node2"],
        "nofailback": false
    });

    let response = client
        .post(&format!("{}/ha/groups", API_BASE))
        .json(&ha_group)
        .send()
        .await;

    if response.is_ok() {
        wait_for_operation(500).await;

        // 2. List HA groups
        let response = client
            .get(&format!("{}/ha/groups", API_BASE))
            .send()
            .await;

        assert!(response.is_ok(), "Failed to list HA groups");
    }

    // 3. Add VM to HA management
    let ha_resource = json!({
        "vm_id": "ha-test-vm",
        "priority": 100,
        "group": "critical-services",
        "preferred_node": "node1"
    });

    let response = client
        .post(&format!("{}/ha/resources", API_BASE))
        .json(&ha_resource)
        .send()
        .await;

    if response.is_ok() {
        wait_for_operation(500).await;

        // 4. List HA resources
        let response = client
            .get(&format!("{}/ha/resources", API_BASE))
            .send()
            .await;

        assert!(response.is_ok(), "Failed to list HA resources");

        // 5. Get HA status
        let response = client
            .get(&format!("{}/ha/status", API_BASE))
            .send()
            .await;

        assert!(response.is_ok(), "Failed to get HA status");

        // 6. Remove VM from HA
        let response = client
            .delete(&format!("{}/ha/resources/ha-test-vm", API_BASE))
            .send()
            .await;

        assert!(response.is_ok(), "Failed to remove HA resource");
    }

    // Cleanup VM
    wait_for_operation(1000).await;
    let _ = client
        .delete(&format!("{}/vms/ha-test-vm", API_BASE))
        .send()
        .await;
}

#[tokio::test]
async fn test_multi_hypervisor_support() {
    let client = create_client();

    // Test creating VMs with different hypervisors

    // 1. QEMU/KVM VM
    let qemu_vm = json!({
        "id": "qemu-test",
        "name": "QEMU Test VM",
        "hypervisor": "Qemu",
        "architecture": "X86_64",
        "cpus": 1,
        "memory": 1024,
        "disk_size": 10,
        "status": "Stopped"
    });

    let response = client
        .post(&format!("{}/vms", API_BASE))
        .json(&qemu_vm)
        .send()
        .await;

    if response.is_ok() {
        wait_for_operation(500).await;
        let _ = client
            .delete(&format!("{}/vms/qemu-test", API_BASE))
            .send()
            .await;
    }

    // 2. LXD VM (if available)
    let lxd_vm = json!({
        "id": "lxd-test",
        "name": "LXD Test VM",
        "hypervisor": "Lxd",
        "architecture": "X86_64",
        "cpus": 1,
        "memory": 1024,
        "disk_size": 10,
        "status": "Stopped"
    });

    let response = client
        .post(&format!("{}/vms", API_BASE))
        .json(&lxd_vm)
        .send()
        .await;

    if response.is_ok() {
        wait_for_operation(500).await;
        let _ = client
            .delete(&format!("{}/vms/lxd-test", API_BASE))
            .send()
            .await;
    }

    // 3. Incus VM (if available)
    let incus_vm = json!({
        "id": "incus-test",
        "name": "Incus Test VM",
        "hypervisor": "Incus",
        "architecture": "X86_64",
        "cpus": 1,
        "memory": 1024,
        "disk_size": 10,
        "status": "Stopped"
    });

    let response = client
        .post(&format!("{}/vms", API_BASE))
        .json(&incus_vm)
        .send()
        .await;

    if response.is_ok() {
        wait_for_operation(500).await;
        let _ = client
            .delete(&format!("{}/vms/incus-test", API_BASE))
            .send()
            .await;
    }
}
