//! Migration Module Tests
//! Tests for VM live migration, offline migration, rollback, and health checks

use horcrux_api::migration::{
    MigrationManager, MigrationConfig, MigrationType, MigrationState,
    MigrationJob, MigrationStats,
};

#[tokio::test]
async fn test_migration_manager_creation() {
    let manager = MigrationManager::new();
    let jobs = manager.list_jobs().await;
    assert!(jobs.is_empty());
}

#[tokio::test]
async fn test_migration_job_states() {
    // Verify all migration states can be serialized/deserialized
    let states = vec![
        MigrationState::Pending,
        MigrationState::Preparing,
        MigrationState::Transferring,
        MigrationState::Syncing,
        MigrationState::Finalizing,
        MigrationState::Completed,
        MigrationState::Failed,
        MigrationState::Cancelled,
    ];

    for state in states {
        let json = serde_json::to_string(&state).unwrap();
        let _: MigrationState = serde_json::from_str(&json).unwrap();
    }
}

#[tokio::test]
async fn test_migration_types() {
    let types = vec![
        MigrationType::Live,
        MigrationType::Offline,
        MigrationType::Online,
    ];

    for mtype in types {
        let json = serde_json::to_string(&mtype).unwrap();
        let deserialized: MigrationType = serde_json::from_str(&json).unwrap();
        assert_eq!(mtype, deserialized);
    }
}

#[tokio::test]
async fn test_migration_config_serialization() {
    let config = MigrationConfig {
        vm_id: 100,
        target_node: "node2".to_string(),
        migration_type: MigrationType::Live,
        bandwidth_limit: Some(100),
        force: false,
        with_local_disks: false,
    };

    let json = serde_json::to_string(&config).unwrap();
    let deserialized: MigrationConfig = serde_json::from_str(&json).unwrap();

    assert_eq!(config.vm_id, deserialized.vm_id);
    assert_eq!(config.target_node, deserialized.target_node);
    assert_eq!(config.bandwidth_limit, deserialized.bandwidth_limit);
}

#[tokio::test]
async fn test_bandwidth_limit_setting() {
    let manager = MigrationManager::new();

    // Set bandwidth limit
    manager.set_bandwidth_limit(Some(200)).await;

    // Set to unlimited
    manager.set_bandwidth_limit(None).await;
}

#[tokio::test]
async fn test_max_concurrent_setting() {
    let manager = MigrationManager::new();

    // Set max concurrent migrations
    manager.set_max_concurrent(2).await;
    manager.set_max_concurrent(5).await;
}

#[tokio::test]
async fn test_auto_rollback_toggle() {
    let manager = MigrationManager::new();

    // Check default (should be enabled)
    assert!(manager.is_auto_rollback_enabled().await);

    // Disable
    manager.set_auto_rollback(false).await;
    assert!(!manager.is_auto_rollback_enabled().await);

    // Re-enable
    manager.set_auto_rollback(true).await;
    assert!(manager.is_auto_rollback_enabled().await);
}

#[tokio::test]
async fn test_health_check_toggle() {
    let manager = MigrationManager::new();

    // Check default (should be enabled)
    assert!(manager.is_health_check_enabled().await);

    // Disable
    manager.set_health_checks(false).await;
    assert!(!manager.is_health_check_enabled().await);

    // Re-enable
    manager.set_health_checks(true).await;
    assert!(manager.is_health_check_enabled().await);
}

#[tokio::test]
async fn test_list_active_migrations() {
    let manager = MigrationManager::new();

    // No active migrations initially
    let active = manager.list_active().await;
    assert!(active.is_empty());
}

#[tokio::test]
async fn test_list_rollbacks() {
    let manager = MigrationManager::new();

    // No rollbacks initially
    let rollbacks = manager.list_rollbacks().await;
    assert!(rollbacks.is_empty());
}

#[tokio::test]
async fn test_list_health_reports() {
    let manager = MigrationManager::new();

    // No health reports initially
    let reports = manager.list_health_reports().await;
    assert!(reports.is_empty());
}

#[tokio::test]
async fn test_get_nonexistent_job() {
    let manager = MigrationManager::new();

    let job = manager.get_job("nonexistent-job").await;
    assert!(job.is_none());
}

#[tokio::test]
async fn test_cancel_nonexistent_migration() {
    let manager = MigrationManager::new();

    let result = manager.cancel_migration("nonexistent-job").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_get_statistics_nonexistent() {
    let manager = MigrationManager::new();

    let result = manager.get_statistics("nonexistent-job").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_manual_rollback_nonexistent() {
    let manager = MigrationManager::new();

    let result = manager.manual_rollback("nonexistent-job").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_get_health_report_nonexistent() {
    let manager = MigrationManager::new();

    let report = manager.get_health_report("nonexistent-job").await;
    assert!(report.is_none());
}

#[tokio::test]
async fn test_get_health_summary_nonexistent() {
    let manager = MigrationManager::new();

    let summary = manager.get_health_summary("nonexistent-job").await;
    assert!(summary.is_none());
}

#[tokio::test]
async fn test_get_rollback_nonexistent() {
    let manager = MigrationManager::new();

    let rollback = manager.get_rollback("nonexistent-job").await;
    assert!(rollback.is_none());
}

#[tokio::test]
async fn test_migration_stats_serialization() {
    let stats = MigrationStats {
        duration_seconds: 120,
        downtime_ms: 50,
        transferred_gb: 10.5,
        average_speed_mbps: 100.0,
        memory_dirty_rate: 50.0,
    };

    let json = serde_json::to_string(&stats).unwrap();
    let deserialized: MigrationStats = serde_json::from_str(&json).unwrap();

    assert_eq!(stats.duration_seconds, deserialized.duration_seconds);
    assert_eq!(stats.downtime_ms, deserialized.downtime_ms);
}
