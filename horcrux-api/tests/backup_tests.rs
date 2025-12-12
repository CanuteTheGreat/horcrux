//! Backup Module Tests
//! Tests for backup creation, restoration, scheduling, and retention

use horcrux_api::backup::{
    BackupManager, BackupConfig, BackupMode, Compression, TargetType,
    BackupJob, RetentionPolicy, Backup,
};

#[tokio::test]
async fn test_backup_manager_creation() {
    let manager = BackupManager::new();
    let backups = manager.list_backups(None).await;
    assert!(backups.is_empty());
}

#[tokio::test]
async fn test_backup_modes() {
    let modes = vec![
        BackupMode::Snapshot,
        BackupMode::Suspend,
        BackupMode::Stop,
    ];

    for mode in modes {
        let json = serde_json::to_string(&mode).unwrap();
        let deserialized: BackupMode = serde_json::from_str(&json).unwrap();
        assert_eq!(mode, deserialized);
    }
}

#[tokio::test]
async fn test_compression_types() {
    let compressions = vec![
        Compression::None,
        Compression::Gzip,
        Compression::Lzo,
        Compression::Zstd,
    ];

    for compression in compressions {
        let json = serde_json::to_string(&compression).unwrap();
        let deserialized: Compression = serde_json::from_str(&json).unwrap();
        assert_eq!(compression, deserialized);
    }
}

#[tokio::test]
async fn test_target_types() {
    let targets = vec![
        TargetType::Vm,
        TargetType::Container,
    ];

    for target in targets {
        let json = serde_json::to_string(&target).unwrap();
        let deserialized: TargetType = serde_json::from_str(&json).unwrap();
        assert_eq!(target, deserialized);
    }
}

#[tokio::test]
async fn test_backup_config_serialization() {
    let config = BackupConfig {
        id: "backup-1".to_string(),
        name: "Test Backup".to_string(),
        target_type: TargetType::Vm,
        target_id: "vm-100".to_string(),
        storage: "/var/lib/horcrux/backups".to_string(),
        mode: BackupMode::Snapshot,
        compression: Compression::Zstd,
        notes: Some("Test backup".to_string()),
    };

    let json = serde_json::to_string(&config).unwrap();
    let deserialized: BackupConfig = serde_json::from_str(&json).unwrap();

    assert_eq!(config.id, deserialized.id);
    assert_eq!(config.name, deserialized.name);
    assert_eq!(config.target_type, deserialized.target_type);
}

#[tokio::test]
async fn test_retention_policy_serialization() {
    let policy = RetentionPolicy {
        keep_hourly: Some(24),
        keep_daily: Some(7),
        keep_weekly: Some(4),
        keep_monthly: Some(12),
        keep_yearly: Some(2),
    };

    let json = serde_json::to_string(&policy).unwrap();
    let deserialized: RetentionPolicy = serde_json::from_str(&json).unwrap();

    assert_eq!(policy.keep_hourly, deserialized.keep_hourly);
    assert_eq!(policy.keep_daily, deserialized.keep_daily);
    assert_eq!(policy.keep_weekly, deserialized.keep_weekly);
    assert_eq!(policy.keep_monthly, deserialized.keep_monthly);
    assert_eq!(policy.keep_yearly, deserialized.keep_yearly);
}

#[tokio::test]
async fn test_backup_job_serialization() {
    let job = BackupJob {
        id: "job-1".to_string(),
        enabled: true,
        schedule: "0 2 * * *".to_string(),
        targets: vec!["vm-100".to_string(), "vm-101".to_string()],
        storage: "/var/lib/horcrux/backups".to_string(),
        mode: BackupMode::Snapshot,
        compression: Compression::Zstd,
        retention: RetentionPolicy {
            keep_hourly: None,
            keep_daily: Some(7),
            keep_weekly: Some(4),
            keep_monthly: Some(6),
            keep_yearly: None,
        },
        notify: Some("admin@example.com".to_string()),
    };

    let json = serde_json::to_string(&job).unwrap();
    let deserialized: BackupJob = serde_json::from_str(&json).unwrap();

    assert_eq!(job.id, deserialized.id);
    assert_eq!(job.enabled, deserialized.enabled);
    assert_eq!(job.schedule, deserialized.schedule);
    assert_eq!(job.targets.len(), deserialized.targets.len());
}

#[tokio::test]
async fn test_list_backups_empty() {
    let manager = BackupManager::new();

    // No target filter
    let backups = manager.list_backups(None).await;
    assert!(backups.is_empty());

    // With target filter
    let backups = manager.list_backups(Some("vm-100".to_string())).await;
    assert!(backups.is_empty());
}

#[tokio::test]
async fn test_list_jobs_empty() {
    let manager = BackupManager::new();
    let jobs = manager.list_jobs().await;
    assert!(jobs.is_empty());
}

#[tokio::test]
async fn test_delete_nonexistent_backup() {
    let manager = BackupManager::new();
    let result = manager.delete_backup("nonexistent").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_restore_nonexistent_backup() {
    let manager = BackupManager::new();
    let result = manager.restore_backup("nonexistent", None).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_backup_metadata_serialization() {
    use std::path::PathBuf;

    let backup = Backup {
        id: "backup-123".to_string(),
        target_type: TargetType::Vm,
        target_id: "vm-100".to_string(),
        target_name: "Web Server".to_string(),
        timestamp: 1700000000,
        size: 1024 * 1024 * 1024, // 1 GB
        mode: BackupMode::Snapshot,
        compression: Compression::Zstd,
        path: PathBuf::from("/var/lib/horcrux/backups/vzdump-qemu-100.tar.zst"),
        config_included: true,
        notes: Some("Weekly backup".to_string()),
    };

    let json = serde_json::to_string(&backup).unwrap();
    let deserialized: Backup = serde_json::from_str(&json).unwrap();

    assert_eq!(backup.id, deserialized.id);
    assert_eq!(backup.size, deserialized.size);
    assert_eq!(backup.config_included, deserialized.config_included);
}

#[tokio::test]
async fn test_create_backup_job() {
    let manager = BackupManager::new();

    let job = BackupJob {
        id: "job-test".to_string(),
        enabled: false, // Disabled to avoid actually scheduling
        schedule: "0 3 * * *".to_string(),
        targets: vec!["vm-100".to_string()],
        storage: "/tmp/backups".to_string(),
        mode: BackupMode::Stop,
        compression: Compression::Gzip,
        retention: RetentionPolicy {
            keep_hourly: None,
            keep_daily: Some(7),
            keep_weekly: None,
            keep_monthly: None,
            keep_yearly: None,
        },
        notify: None,
    };

    let result = manager.create_job(job.clone()).await;
    assert!(result.is_ok());

    let jobs = manager.list_jobs().await;
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].id, "job-test");
}

#[tokio::test]
async fn test_duplicate_backup_job() {
    let manager = BackupManager::new();

    let job = BackupJob {
        id: "job-duplicate".to_string(),
        enabled: false,
        schedule: "0 3 * * *".to_string(),
        targets: vec!["vm-100".to_string()],
        storage: "/tmp/backups".to_string(),
        mode: BackupMode::Stop,
        compression: Compression::None,
        retention: RetentionPolicy {
            keep_hourly: None,
            keep_daily: Some(3),
            keep_weekly: None,
            keep_monthly: None,
            keep_yearly: None,
        },
        notify: None,
    };

    // First creation should succeed
    let result = manager.create_job(job.clone()).await;
    assert!(result.is_ok());

    // Second creation with same ID should fail
    let result = manager.create_job(job).await;
    assert!(result.is_err());
}
