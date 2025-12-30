//! S3 Gateway module
//!
//! Manages MinIO for S3-compatible object storage API.
//! Provides full control over buckets, policies, users, and lifecycle.

use horcrux_common::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tokio::process::Command;
use tokio::io::AsyncWriteExt;

/// S3 Gateway configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3GatewayConfig {
    /// Data directory (or directories for erasure coding)
    pub data_dirs: Vec<String>,
    /// Listen address
    pub address: String,
    /// Listen port
    pub port: u16,
    /// Console port (web UI)
    pub console_port: u16,
    /// Root user (access key)
    pub root_user: String,
    /// Root password (secret key)
    pub root_password: String,
    /// Enable browser/console
    pub browser: bool,
    /// Region
    pub region: String,
    /// TLS configuration
    pub tls: Option<S3TlsConfig>,
    /// Erasure coding enabled
    pub erasure_coding: bool,
    /// Cache configuration
    pub cache: Option<S3CacheConfig>,
    /// Metrics enabled
    pub metrics: bool,
    /// Prometheus auth token (if metrics enabled)
    pub metrics_auth_token: Option<String>,
    /// Audit logging
    pub audit_log: bool,
    /// Audit log webhook
    pub audit_webhook: Option<String>,
    /// Domain for virtual host style access
    pub domain: Option<String>,
    /// Max object size (bytes, default 5TiB)
    pub max_object_size: u64,
}

impl Default for S3GatewayConfig {
    fn default() -> Self {
        Self {
            data_dirs: vec!["/mnt/nas/s3".to_string()],
            address: "0.0.0.0".to_string(),
            port: 9000,
            console_port: 9001,
            root_user: "minioadmin".to_string(),
            root_password: "minioadmin".to_string(),
            browser: true,
            region: "us-east-1".to_string(),
            tls: None,
            erasure_coding: false,
            cache: None,
            metrics: true,
            metrics_auth_token: None,
            audit_log: false,
            audit_webhook: None,
            domain: None,
            max_object_size: 5 * 1024 * 1024 * 1024 * 1024, // 5TiB
        }
    }
}

/// TLS configuration for S3 gateway
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3TlsConfig {
    /// Enable TLS
    pub enabled: bool,
    /// Certificate file path
    pub certificate: String,
    /// Private key file path
    pub private_key: String,
    /// CA certificate (for client cert auth)
    pub ca_certificate: Option<String>,
    /// Require client certificates
    pub client_auth: bool,
    /// Minimum TLS version
    pub min_version: String,
    /// Cipher suites (empty = default)
    pub cipher_suites: Vec<String>,
}

impl Default for S3TlsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            certificate: "/etc/minio/certs/public.crt".to_string(),
            private_key: "/etc/minio/certs/private.key".to_string(),
            ca_certificate: None,
            client_auth: false,
            min_version: "1.2".to_string(),
            cipher_suites: vec![],
        }
    }
}

/// Cache configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3CacheConfig {
    /// Cache drives
    pub drives: Vec<String>,
    /// Expiry in days
    pub expiry: u32,
    /// Quota percentage (0-100)
    pub quota: u8,
    /// Exclude patterns
    pub exclude: Vec<String>,
    /// After (cache objects accessed after this duration)
    pub after: u32,
    /// Watermark low percentage
    pub watermark_low: u8,
    /// Watermark high percentage
    pub watermark_high: u8,
}

impl Default for S3CacheConfig {
    fn default() -> Self {
        Self {
            drives: vec!["/mnt/cache/s3".to_string()],
            expiry: 90,
            quota: 80,
            exclude: vec!["*.pdf".to_string(), "*.mp4".to_string()],
            after: 0,
            watermark_low: 70,
            watermark_high: 90,
        }
    }
}

/// S3 Bucket
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3Bucket {
    /// Bucket name
    pub name: String,
    /// Physical path
    pub path: String,
    /// Versioning enabled
    pub versioning: bool,
    /// Object locking enabled
    pub object_locking: bool,
    /// Quota in bytes (None = unlimited)
    pub quota_bytes: Option<u64>,
    /// Creation timestamp
    pub created_at: i64,
    /// Tags
    pub tags: HashMap<String, String>,
    /// Encryption configuration
    pub encryption: Option<BucketEncryption>,
    /// Lifecycle rules
    pub lifecycle_rules: Vec<LifecycleRule>,
    /// Replication configuration
    pub replication: Option<BucketReplication>,
    /// Event notifications
    pub notifications: Vec<BucketNotification>,
}

/// Bucket encryption configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BucketEncryption {
    /// Encryption type (SSE-S3, SSE-KMS)
    pub encryption_type: EncryptionType,
    /// KMS key ID (for SSE-KMS)
    pub kms_key_id: Option<String>,
}

/// Encryption type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING-KEBAB-CASE")]
pub enum EncryptionType {
    SseS3,
    SseKms,
}

/// Lifecycle rule for bucket
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LifecycleRule {
    /// Rule ID
    pub id: String,
    /// Prefix filter
    pub prefix: String,
    /// Status (Enabled/Disabled)
    pub enabled: bool,
    /// Expiration days
    pub expiration_days: Option<u32>,
    /// Transition to tier after days
    pub transition_days: Option<u32>,
    /// Transition storage class
    pub transition_storage_class: Option<String>,
    /// Delete expired delete markers
    pub delete_markers: bool,
    /// Noncurrent version expiration
    pub noncurrent_expiration_days: Option<u32>,
    /// Tags filter
    pub tags: HashMap<String, String>,
}

/// Bucket replication configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BucketReplication {
    /// Replication rule ID
    pub id: String,
    /// Target bucket ARN
    pub target_arn: String,
    /// Target endpoint
    pub target_endpoint: String,
    /// Target access key
    pub target_access_key: String,
    /// Target secret key
    pub target_secret_key: String,
    /// Sync mode (async/sync)
    pub sync: bool,
    /// Bandwidth limit in bytes/sec
    pub bandwidth_limit: Option<u64>,
    /// Replicate delete markers
    pub replicate_deletes: bool,
    /// Replicate metadata
    pub replicate_metadata: bool,
    /// Priority (lower = higher priority)
    pub priority: u32,
}

/// Bucket notification configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BucketNotification {
    /// Notification ID
    pub id: String,
    /// Event type
    pub events: Vec<S3Event>,
    /// Prefix filter
    pub prefix: Option<String>,
    /// Suffix filter
    pub suffix: Option<String>,
    /// Target type
    pub target: NotificationTarget,
}

/// S3 event types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum S3Event {
    ObjectCreated,
    ObjectCreatedPut,
    ObjectCreatedPost,
    ObjectCreatedCopy,
    ObjectCreatedMultipartUpload,
    ObjectRemoved,
    ObjectRemovedDelete,
    ObjectRemovedDeleteMarkerCreated,
    ObjectRestore,
    Replication,
    IlmTransition,
    IlmExpiry,
    ScannerFinding,
}

/// Notification target
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum NotificationTarget {
    Webhook { url: String, auth_token: Option<String> },
    Amqp { url: String, exchange: String, routing_key: String },
    Kafka { brokers: Vec<String>, topic: String },
    Nats { address: String, subject: String },
    Redis { address: String, key: String },
    Postgres { connection_string: String, table: String },
    Mysql { dsn: String, table: String },
    Elasticsearch { url: String, index: String },
}

/// S3 Access Key
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3AccessKey {
    /// Access key ID
    pub access_key: String,
    /// Secret key (only returned on creation)
    #[serde(skip_serializing_if = "String::is_empty")]
    pub secret_key: String,
    /// Associated user
    pub user_id: String,
    /// Description
    pub description: Option<String>,
    /// Enabled
    pub enabled: bool,
    /// Expiry timestamp (None = never)
    pub expires_at: Option<i64>,
    /// Creation timestamp
    pub created_at: i64,
    /// Last used timestamp
    pub last_used: Option<i64>,
    /// Policy ARNs attached
    pub policies: Vec<String>,
}

/// S3 User
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3User {
    /// Username
    pub username: String,
    /// Status (enabled/disabled)
    pub enabled: bool,
    /// Attached policy ARNs
    pub policies: Vec<String>,
    /// Member of groups
    pub groups: Vec<String>,
    /// Creation timestamp
    pub created_at: i64,
}

/// S3 Policy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3Policy {
    /// Policy name
    pub name: String,
    /// Policy document (JSON)
    pub policy: String,
    /// Built-in or custom
    pub builtin: bool,
}

/// S3 Group
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3Group {
    /// Group name
    pub name: String,
    /// Member users
    pub members: Vec<String>,
    /// Attached policies
    pub policies: Vec<String>,
    /// Status
    pub enabled: bool,
}

/// Bucket quota configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BucketQuota {
    /// Quota type (hard/fifo)
    pub quota_type: QuotaType,
    /// Quota size in bytes
    pub quota_bytes: u64,
}

/// Quota type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuotaType {
    /// Hard quota - deny writes when exceeded
    Hard,
    /// FIFO quota - delete oldest objects when exceeded
    Fifo,
}

/// S3 Gateway Status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3GatewayStatus {
    /// Running
    pub running: bool,
    /// Version
    pub version: Option<String>,
    /// Address
    pub address: String,
    /// Console address
    pub console_address: String,
    /// Bucket count
    pub bucket_count: u32,
    /// Object count
    pub object_count: u64,
    /// Total storage used
    pub storage_used_bytes: u64,
    /// Total storage available
    pub storage_available_bytes: u64,
    /// Region
    pub region: String,
    /// TLS enabled
    pub tls_enabled: bool,
    /// Uptime in seconds
    pub uptime_secs: Option<u64>,
    /// Connected servers (for distributed mode)
    pub servers: Vec<ServerInfo>,
    /// Drives info
    pub drives: Vec<DriveInfo>,
}

/// Server info for distributed mode
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    /// Endpoint
    pub endpoint: String,
    /// State (online/offline)
    pub state: String,
    /// Uptime
    pub uptime_secs: u64,
}

/// Drive information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveInfo {
    /// Path
    pub path: String,
    /// State
    pub state: String,
    /// Total bytes
    pub total_bytes: u64,
    /// Used bytes
    pub used_bytes: u64,
    /// Available bytes
    pub available_bytes: u64,
}

/// Object info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3Object {
    /// Object key
    pub key: String,
    /// Size in bytes
    pub size: u64,
    /// Last modified
    pub last_modified: i64,
    /// ETag
    pub etag: String,
    /// Content type
    pub content_type: Option<String>,
    /// Storage class
    pub storage_class: String,
    /// Version ID
    pub version_id: Option<String>,
    /// Is delete marker
    pub is_delete_marker: bool,
    /// User metadata
    pub metadata: HashMap<String, String>,
    /// Tags
    pub tags: HashMap<String, String>,
}

/// Presigned URL request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresignedUrlRequest {
    /// Bucket name
    pub bucket: String,
    /// Object key
    pub key: String,
    /// Expiry in seconds
    pub expires_secs: u64,
    /// HTTP method (GET/PUT)
    pub method: String,
    /// Content type (for PUT)
    pub content_type: Option<String>,
}

/// S3 Gateway Manager
pub struct S3GatewayManager {
    config: S3GatewayConfig,
    mc_alias: String,
}

impl S3GatewayManager {
    /// Create a new S3 gateway manager
    pub fn new() -> Self {
        Self {
            config: S3GatewayConfig::default(),
            mc_alias: "local".to_string(),
        }
    }

    /// Create with specific configuration
    pub fn with_config(config: S3GatewayConfig) -> Self {
        Self {
            config,
            mc_alias: "local".to_string(),
        }
    }

    /// Set configuration
    pub fn set_config(&mut self, config: S3GatewayConfig) {
        self.config = config;
    }

    /// Get current configuration
    pub fn get_config(&self) -> &S3GatewayConfig {
        &self.config
    }

    /// Setup mc alias for API access
    async fn setup_mc_alias(&self) -> Result<()> {
        let protocol = if self.config.tls.as_ref().map(|t| t.enabled).unwrap_or(false) {
            "https"
        } else {
            "http"
        };

        let url = format!(
            "{}://{}:{}",
            protocol,
            if self.config.address == "0.0.0.0" { "127.0.0.1" } else { &self.config.address },
            self.config.port
        );

        let output = Command::new("mc")
            .args([
                "alias", "set", &self.mc_alias,
                &url,
                &self.config.root_user,
                &self.config.root_password,
            ])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to setup mc alias: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("mc alias setup failed: {}", stderr)));
        }

        Ok(())
    }

    /// Start the S3 gateway
    pub async fn start(&self) -> Result<()> {
        // Ensure data directories exist
        for dir in &self.config.data_dirs {
            tokio::fs::create_dir_all(dir).await.map_err(|e| {
                Error::Internal(format!("Failed to create S3 data dir {}: {}", dir, e))
            })?;
        }

        // Write configuration
        self.write_config().await?;

        // Start MinIO via systemd/OpenRC
        crate::nas::services::manage_service(
            &crate::nas::services::NasService::Minio,
            crate::nas::services::ServiceAction::Start,
        ).await?;

        // Wait for service to be ready
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        // Setup mc alias
        let _ = self.setup_mc_alias().await;

        Ok(())
    }

    /// Stop the S3 gateway
    pub async fn stop(&self) -> Result<()> {
        crate::nas::services::manage_service(
            &crate::nas::services::NasService::Minio,
            crate::nas::services::ServiceAction::Stop,
        ).await
    }

    /// Restart the S3 gateway
    pub async fn restart(&self) -> Result<()> {
        self.write_config().await?;
        crate::nas::services::manage_service(
            &crate::nas::services::NasService::Minio,
            crate::nas::services::ServiceAction::Restart,
        ).await?;

        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        let _ = self.setup_mc_alias().await;

        Ok(())
    }

    /// Check if S3 gateway is running
    pub async fn is_running(&self) -> bool {
        let output = Command::new("systemctl")
            .args(["is-active", "--quiet", "minio"])
            .status()
            .await;

        if let Ok(status) = output {
            if status.success() {
                return true;
            }
        }

        // Try OpenRC
        let output = Command::new("rc-service")
            .args(["minio", "status"])
            .output()
            .await;

        if let Ok(out) = output {
            let stdout = String::from_utf8_lossy(&out.stdout);
            return stdout.contains("started") || stdout.contains("running");
        }

        false
    }

    /// Get S3 gateway status
    pub async fn get_status(&self) -> Result<S3GatewayStatus> {
        let running = self.is_running().await;
        let buckets = if running {
            self.list_buckets().await.unwrap_or_default()
        } else {
            Vec::new()
        };

        // Get version
        let version = self.get_version().await.ok();

        // Get drive info
        let drives = if running {
            self.get_drive_info().await.unwrap_or_default()
        } else {
            Vec::new()
        };

        let (storage_used, storage_available) = drives.iter().fold((0u64, 0u64), |acc, d| {
            (acc.0 + d.used_bytes, acc.1 + d.available_bytes)
        });

        // Get uptime
        let uptime = if running {
            self.get_uptime().await.ok()
        } else {
            None
        };

        // Get object count (approximate)
        let object_count = if running {
            self.get_total_object_count().await.unwrap_or(0)
        } else {
            0
        };

        Ok(S3GatewayStatus {
            running,
            version,
            address: format!("{}:{}", self.config.address, self.config.port),
            console_address: format!("{}:{}", self.config.address, self.config.console_port),
            bucket_count: buckets.len() as u32,
            object_count,
            storage_used_bytes: storage_used,
            storage_available_bytes: storage_available,
            region: self.config.region.clone(),
            tls_enabled: self.config.tls.as_ref().map(|t| t.enabled).unwrap_or(false),
            uptime_secs: uptime,
            servers: vec![],
            drives,
        })
    }

    /// Get MinIO version
    async fn get_version(&self) -> Result<String> {
        let output = Command::new("minio")
            .args(["--version"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get minio version: {}", e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        // Parse "minio version RELEASE.2024-01-01T00-00-00Z"
        if let Some(version) = stdout.split_whitespace().nth(2) {
            return Ok(version.to_string());
        }

        Err(Error::Internal("Failed to parse minio version".to_string()))
    }

    /// Get drive information
    async fn get_drive_info(&self) -> Result<Vec<DriveInfo>> {
        let output = Command::new("mc")
            .args(["admin", "info", &self.mc_alias, "--json"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc admin info failed: {}", e)))?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Parse JSON response
        let mut drives = Vec::new();
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&stdout) {
            if let Some(disks) = json.get("info").and_then(|i| i.get("backend")).and_then(|b| b.get("onlineDisks")) {
                if let Some(disk_list) = disks.as_array() {
                    for disk in disk_list {
                        drives.push(DriveInfo {
                            path: disk.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                            state: disk.get("state").and_then(|v| v.as_str()).unwrap_or("online").to_string(),
                            total_bytes: disk.get("totalSpace").and_then(|v| v.as_u64()).unwrap_or(0),
                            used_bytes: disk.get("usedSpace").and_then(|v| v.as_u64()).unwrap_or(0),
                            available_bytes: disk.get("availableSpace").and_then(|v| v.as_u64()).unwrap_or(0),
                        });
                    }
                }
            }
        }

        // Fallback: use df for data directories
        if drives.is_empty() {
            for dir in &self.config.data_dirs {
                if let Ok((total, used, available)) = get_disk_usage(dir).await {
                    drives.push(DriveInfo {
                        path: dir.clone(),
                        state: "online".to_string(),
                        total_bytes: total,
                        used_bytes: used,
                        available_bytes: available,
                    });
                }
            }
        }

        Ok(drives)
    }

    /// Get uptime in seconds
    async fn get_uptime(&self) -> Result<u64> {
        let output = Command::new("mc")
            .args(["admin", "info", &self.mc_alias, "--json"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc admin info failed: {}", e)))?;

        if !output.status.success() {
            return Err(Error::Internal("mc admin info failed".to_string()));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&stdout) {
            if let Some(uptime) = json.get("info").and_then(|i| i.get("uptime")).and_then(|u| u.as_u64()) {
                return Ok(uptime);
            }
        }

        Err(Error::Internal("Failed to parse uptime".to_string()))
    }

    /// Get total object count
    async fn get_total_object_count(&self) -> Result<u64> {
        let buckets = self.list_buckets().await?;
        let mut total = 0u64;

        for bucket in buckets {
            if let Ok(count) = self.get_bucket_object_count(&bucket.name).await {
                total += count;
            }
        }

        Ok(total)
    }

    /// Get object count in bucket
    async fn get_bucket_object_count(&self, bucket: &str) -> Result<u64> {
        let output = Command::new("mc")
            .args(["stat", &format!("{}/{}", self.mc_alias, bucket), "--json"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc stat failed: {}", e)))?;

        if !output.status.success() {
            return Ok(0);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&stdout) {
            if let Some(count) = json.get("objectsCount").and_then(|v| v.as_u64()) {
                return Ok(count);
            }
        }

        Ok(0)
    }

    // ========== Bucket Operations ==========

    /// Create a bucket
    pub async fn create_bucket(&self, name: &str, options: Option<CreateBucketOptions>) -> Result<S3Bucket> {
        let opts = options.unwrap_or_default();

        // Create bucket using mc
        let mut args = vec!["mb".to_string()];

        if opts.object_locking {
            args.push("--with-lock".to_string());
        }

        args.push(format!("{}/{}", self.mc_alias, name));

        let output = Command::new("mc")
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to create bucket: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Check if bucket already exists
            if stderr.contains("already exists") || stderr.contains("already owned") {
                return Err(Error::Conflict(format!("Bucket '{}' already exists", name)));
            }
            return Err(Error::Internal(format!("Failed to create bucket: {}", stderr)));
        }

        // Set versioning if requested
        if opts.versioning {
            let _ = self.set_bucket_versioning(name, true).await;
        }

        // Set quota if specified
        if let Some(quota) = opts.quota {
            let _ = self.set_bucket_quota(name, &quota).await;
        }

        // Set encryption if specified
        if let Some(encryption) = opts.encryption {
            let _ = self.set_bucket_encryption(name, &encryption).await;
        }

        // Set tags if specified
        if !opts.tags.is_empty() {
            let _ = self.set_bucket_tags(name, &opts.tags).await;
        }

        Ok(S3Bucket {
            name: name.to_string(),
            path: format!("{}/{}", self.config.data_dirs.first().unwrap_or(&"/mnt/nas/s3".to_string()), name),
            versioning: opts.versioning,
            object_locking: opts.object_locking,
            quota_bytes: opts.quota.map(|q| q.quota_bytes),
            created_at: chrono::Utc::now().timestamp(),
            tags: opts.tags,
            encryption: opts.encryption,
            lifecycle_rules: vec![],
            replication: None,
            notifications: vec![],
        })
    }

    /// Delete a bucket
    pub async fn delete_bucket(&self, name: &str, force: bool) -> Result<()> {
        let mut args = vec!["rb".to_string()];

        if force {
            args.push("--force".to_string());
        }

        args.push(format!("{}/{}", self.mc_alias, name));

        let output = Command::new("mc")
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to delete bucket: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("not empty") {
                return Err(Error::Conflict("Bucket is not empty".to_string()));
            }
            return Err(Error::Internal(format!("Failed to delete bucket: {}", stderr)));
        }

        Ok(())
    }

    /// List buckets
    pub async fn list_buckets(&self) -> Result<Vec<S3Bucket>> {
        let output = Command::new("mc")
            .args(["ls", &self.mc_alias, "--json"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc ls failed: {}", e)))?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut buckets = Vec::new();

        for line in stdout.lines() {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(line) {
                if json.get("status").and_then(|s| s.as_str()) == Some("success") {
                    if let Some(key) = json.get("key").and_then(|k| k.as_str()) {
                        let name = key.trim_end_matches('/');
                        buckets.push(S3Bucket {
                            name: name.to_string(),
                            path: format!("{}/{}", self.config.data_dirs.first().unwrap_or(&"/mnt/nas/s3".to_string()), name),
                            versioning: false,
                            object_locking: false,
                            quota_bytes: None,
                            created_at: json.get("lastModified").and_then(|v| v.as_str())
                                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                                .map(|d| d.timestamp())
                                .unwrap_or(0),
                            tags: HashMap::new(),
                            encryption: None,
                            lifecycle_rules: vec![],
                            replication: None,
                            notifications: vec![],
                        });
                    }
                }
            }
        }

        Ok(buckets)
    }

    /// Get bucket info
    pub async fn get_bucket(&self, name: &str) -> Result<S3Bucket> {
        let output = Command::new("mc")
            .args(["stat", &format!("{}/{}", self.mc_alias, name), "--json"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc stat failed: {}", e)))?;

        if !output.status.success() {
            return Err(Error::NotFound(format!("Bucket '{}' not found", name)));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Get versioning status
        let versioning = self.get_bucket_versioning(name).await.unwrap_or(false);

        // Get quota
        let quota = self.get_bucket_quota(name).await.ok().flatten();

        // Get tags
        let tags = self.get_bucket_tags(name).await.unwrap_or_default();

        // Get lifecycle rules
        let lifecycle_rules = self.get_bucket_lifecycle(name).await.unwrap_or_default();

        Ok(S3Bucket {
            name: name.to_string(),
            path: format!("{}/{}", self.config.data_dirs.first().unwrap_or(&"/mnt/nas/s3".to_string()), name),
            versioning,
            object_locking: false, // Would need to parse from mc output
            quota_bytes: quota.map(|q| q.quota_bytes),
            created_at: 0,
            tags,
            encryption: None,
            lifecycle_rules,
            replication: None,
            notifications: vec![],
        })
    }

    /// Set bucket versioning
    pub async fn set_bucket_versioning(&self, bucket: &str, enabled: bool) -> Result<()> {
        let status = if enabled { "enable" } else { "suspend" };

        let output = Command::new("mc")
            .args(["version", status, &format!("{}/{}", self.mc_alias, bucket)])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc version failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to set versioning: {}", stderr)));
        }

        Ok(())
    }

    /// Get bucket versioning status
    pub async fn get_bucket_versioning(&self, bucket: &str) -> Result<bool> {
        let output = Command::new("mc")
            .args(["version", "info", &format!("{}/{}", self.mc_alias, bucket), "--json"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc version failed: {}", e)))?;

        if !output.status.success() {
            return Ok(false);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.contains("\"versioning\":\"Enabled\"") || stdout.contains("\"versioning\": \"Enabled\""))
    }

    /// Set bucket quota
    pub async fn set_bucket_quota(&self, bucket: &str, quota: &BucketQuota) -> Result<()> {
        let quota_type = match quota.quota_type {
            QuotaType::Hard => "hard",
            QuotaType::Fifo => "fifo",
        };

        let output = Command::new("mc")
            .args([
                "quota", "set",
                &format!("{}/{}", self.mc_alias, bucket),
                "--size", &format!("{}B", quota.quota_bytes),
                "--type", quota_type,
            ])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc quota failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to set quota: {}", stderr)));
        }

        Ok(())
    }

    /// Get bucket quota
    pub async fn get_bucket_quota(&self, bucket: &str) -> Result<Option<BucketQuota>> {
        let output = Command::new("mc")
            .args(["quota", "info", &format!("{}/{}", self.mc_alias, bucket), "--json"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc quota failed: {}", e)))?;

        if !output.status.success() {
            return Ok(None);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&stdout) {
            if let (Some(quota), Some(quota_type)) = (
                json.get("quota").and_then(|q| q.as_u64()),
                json.get("quotatype").and_then(|t| t.as_str()),
            ) {
                if quota > 0 {
                    return Ok(Some(BucketQuota {
                        quota_type: if quota_type == "fifo" { QuotaType::Fifo } else { QuotaType::Hard },
                        quota_bytes: quota,
                    }));
                }
            }
        }

        Ok(None)
    }

    /// Clear bucket quota
    pub async fn clear_bucket_quota(&self, bucket: &str) -> Result<()> {
        let output = Command::new("mc")
            .args(["quota", "clear", &format!("{}/{}", self.mc_alias, bucket)])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc quota failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to clear quota: {}", stderr)));
        }

        Ok(())
    }

    /// Set bucket encryption
    pub async fn set_bucket_encryption(&self, bucket: &str, encryption: &BucketEncryption) -> Result<()> {
        let enc_type = match encryption.encryption_type {
            EncryptionType::SseS3 => "sse-s3",
            EncryptionType::SseKms => "sse-kms",
        };

        let mut args = vec!["encrypt", "set", enc_type];

        if let Some(ref key_id) = encryption.kms_key_id {
            args.push(key_id);
        }

        args.push(&format!("{}/{}", self.mc_alias, bucket));

        let output = Command::new("mc")
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc encrypt failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to set encryption: {}", stderr)));
        }

        Ok(())
    }

    /// Set bucket tags
    pub async fn set_bucket_tags(&self, bucket: &str, tags: &HashMap<String, String>) -> Result<()> {
        let tags_str: Vec<String> = tags.iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();

        let output = Command::new("mc")
            .args([
                "tag", "set",
                &format!("{}/{}", self.mc_alias, bucket),
                &tags_str.join("&"),
            ])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc tag failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to set tags: {}", stderr)));
        }

        Ok(())
    }

    /// Get bucket tags
    pub async fn get_bucket_tags(&self, bucket: &str) -> Result<HashMap<String, String>> {
        let output = Command::new("mc")
            .args(["tag", "list", &format!("{}/{}", self.mc_alias, bucket), "--json"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc tag failed: {}", e)))?;

        if !output.status.success() {
            return Ok(HashMap::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut tags = HashMap::new();

        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&stdout) {
            if let Some(tagset) = json.get("tagset").and_then(|t| t.as_object()) {
                for (key, value) in tagset {
                    if let Some(v) = value.as_str() {
                        tags.insert(key.clone(), v.to_string());
                    }
                }
            }
        }

        Ok(tags)
    }

    /// Set bucket lifecycle rules
    pub async fn set_bucket_lifecycle(&self, bucket: &str, rules: &[LifecycleRule]) -> Result<()> {
        // Generate lifecycle XML
        let xml = generate_lifecycle_xml(rules);

        // Write to temp file
        let temp_file = format!("/tmp/lifecycle_{}.xml", bucket);
        tokio::fs::write(&temp_file, &xml).await.map_err(|e| {
            Error::Internal(format!("Failed to write lifecycle config: {}", e))
        })?;

        let output = Command::new("mc")
            .args([
                "ilm", "import",
                &format!("{}/{}", self.mc_alias, bucket),
            ])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| Error::Internal(format!("mc ilm failed: {}", e)))?;

        // Write XML to stdin
        if let Some(mut stdin) = output.stdin {
            stdin.write_all(xml.as_bytes()).await.ok();
        }

        // Clean up
        let _ = tokio::fs::remove_file(&temp_file).await;

        Ok(())
    }

    /// Get bucket lifecycle rules
    pub async fn get_bucket_lifecycle(&self, bucket: &str) -> Result<Vec<LifecycleRule>> {
        let output = Command::new("mc")
            .args(["ilm", "ls", &format!("{}/{}", self.mc_alias, bucket), "--json"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc ilm failed: {}", e)))?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut rules = Vec::new();

        for line in stdout.lines() {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(line) {
                if let Some(id) = json.get("id").and_then(|i| i.as_str()) {
                    rules.push(LifecycleRule {
                        id: id.to_string(),
                        prefix: json.get("prefix").and_then(|p| p.as_str()).unwrap_or("").to_string(),
                        enabled: json.get("status").and_then(|s| s.as_str()) == Some("Enabled"),
                        expiration_days: json.get("expiration").and_then(|e| e.get("days")).and_then(|d| d.as_u64()).map(|d| d as u32),
                        transition_days: None,
                        transition_storage_class: None,
                        delete_markers: false,
                        noncurrent_expiration_days: None,
                        tags: HashMap::new(),
                    });
                }
            }
        }

        Ok(rules)
    }

    // ========== User Operations ==========

    /// Create S3 user
    pub async fn create_user(&self, username: &str, password: &str) -> Result<S3User> {
        let output = Command::new("mc")
            .args([
                "admin", "user", "add",
                &self.mc_alias,
                username,
                password,
            ])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc admin user add failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to create user: {}", stderr)));
        }

        Ok(S3User {
            username: username.to_string(),
            enabled: true,
            policies: vec![],
            groups: vec![],
            created_at: chrono::Utc::now().timestamp(),
        })
    }

    /// Delete S3 user
    pub async fn delete_user(&self, username: &str) -> Result<()> {
        let output = Command::new("mc")
            .args(["admin", "user", "remove", &self.mc_alias, username])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc admin user remove failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to delete user: {}", stderr)));
        }

        Ok(())
    }

    /// List S3 users
    pub async fn list_users(&self) -> Result<Vec<S3User>> {
        let output = Command::new("mc")
            .args(["admin", "user", "list", &self.mc_alias, "--json"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc admin user list failed: {}", e)))?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut users = Vec::new();

        for line in stdout.lines() {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(line) {
                if let Some(access_key) = json.get("accessKey").and_then(|a| a.as_str()) {
                    users.push(S3User {
                        username: access_key.to_string(),
                        enabled: json.get("userStatus").and_then(|s| s.as_str()) == Some("enabled"),
                        policies: json.get("policyName").and_then(|p| p.as_str())
                            .map(|s| vec![s.to_string()]).unwrap_or_default(),
                        groups: vec![],
                        created_at: 0,
                    });
                }
            }
        }

        Ok(users)
    }

    /// Set user policy
    pub async fn set_user_policy(&self, username: &str, policy: &str) -> Result<()> {
        let output = Command::new("mc")
            .args([
                "admin", "policy", "attach",
                &self.mc_alias,
                policy,
                "--user", username,
            ])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc admin policy attach failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to set policy: {}", stderr)));
        }

        Ok(())
    }

    /// Enable/disable user
    pub async fn set_user_status(&self, username: &str, enabled: bool) -> Result<()> {
        let status = if enabled { "enable" } else { "disable" };

        let output = Command::new("mc")
            .args(["admin", "user", status, &self.mc_alias, username])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc admin user {} failed: {}", status, e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to {} user: {}", status, stderr)));
        }

        Ok(())
    }

    // ========== Access Key Operations ==========

    /// Create access key
    pub async fn create_access_key(&self, user_id: &str, description: Option<&str>) -> Result<S3AccessKey> {
        // Generate random access key and secret
        use rand::Rng;
        let mut rng = rand::thread_rng();

        let access_key: String = (0..20)
            .map(|_| rng.sample(rand::distributions::Alphanumeric) as char)
            .collect();
        let secret_key: String = (0..40)
            .map(|_| rng.sample(rand::distributions::Alphanumeric) as char)
            .collect();

        let mut args = vec![
            "admin".to_string(), "user".to_string(), "svcacct".to_string(), "add".to_string(),
            "--access-key".to_string(), access_key.clone(),
            "--secret-key".to_string(), secret_key.clone(),
        ];

        if let Some(desc) = description {
            args.push("--description".to_string());
            args.push(desc.to_string());
        }

        args.push(self.mc_alias.clone());
        args.push(user_id.to_string());

        let output = Command::new("mc")
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc svcacct add failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to create access key: {}", stderr)));
        }

        Ok(S3AccessKey {
            access_key,
            secret_key,
            user_id: user_id.to_string(),
            description: description.map(|s| s.to_string()),
            enabled: true,
            expires_at: None,
            created_at: chrono::Utc::now().timestamp(),
            last_used: None,
            policies: vec![],
        })
    }

    /// Delete access key
    pub async fn delete_access_key(&self, access_key: &str) -> Result<()> {
        let output = Command::new("mc")
            .args(["admin", "user", "svcacct", "rm", &self.mc_alias, access_key])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc svcacct rm failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to delete access key: {}", stderr)));
        }

        Ok(())
    }

    /// List access keys for user
    pub async fn list_access_keys(&self, user_id: &str) -> Result<Vec<S3AccessKey>> {
        let output = Command::new("mc")
            .args(["admin", "user", "svcacct", "list", &self.mc_alias, user_id, "--json"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc svcacct list failed: {}", e)))?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut keys = Vec::new();

        for line in stdout.lines() {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(line) {
                if let Some(access_key) = json.get("accessKey").and_then(|a| a.as_str()) {
                    keys.push(S3AccessKey {
                        access_key: access_key.to_string(),
                        secret_key: String::new(),
                        user_id: user_id.to_string(),
                        description: json.get("description").and_then(|d| d.as_str()).map(|s| s.to_string()),
                        enabled: json.get("accountStatus").and_then(|s| s.as_str()) != Some("off"),
                        expires_at: None,
                        created_at: 0,
                        last_used: None,
                        policies: vec![],
                    });
                }
            }
        }

        Ok(keys)
    }

    // ========== Policy Operations ==========

    /// Create custom policy
    pub async fn create_policy(&self, name: &str, policy_json: &str) -> Result<S3Policy> {
        // Validate JSON
        if serde_json::from_str::<serde_json::Value>(policy_json).is_err() {
            return Err(Error::Validation("Invalid policy JSON".to_string()));
        }

        // Write to temp file
        let temp_file = format!("/tmp/policy_{}.json", name);
        tokio::fs::write(&temp_file, policy_json).await.map_err(|e| {
            Error::Internal(format!("Failed to write policy file: {}", e))
        })?;

        let output = Command::new("mc")
            .args(["admin", "policy", "create", &self.mc_alias, name, &temp_file])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc policy create failed: {}", e)))?;

        // Clean up
        let _ = tokio::fs::remove_file(&temp_file).await;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to create policy: {}", stderr)));
        }

        Ok(S3Policy {
            name: name.to_string(),
            policy: policy_json.to_string(),
            builtin: false,
        })
    }

    /// Delete policy
    pub async fn delete_policy(&self, name: &str) -> Result<()> {
        let output = Command::new("mc")
            .args(["admin", "policy", "remove", &self.mc_alias, name])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc policy remove failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to delete policy: {}", stderr)));
        }

        Ok(())
    }

    /// List policies
    pub async fn list_policies(&self) -> Result<Vec<S3Policy>> {
        let output = Command::new("mc")
            .args(["admin", "policy", "list", &self.mc_alias, "--json"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc policy list failed: {}", e)))?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut policies = Vec::new();

        for line in stdout.lines() {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(line) {
                if let Some(policy) = json.get("policy").and_then(|p| p.as_str()) {
                    let builtin = ["readonly", "readwrite", "writeonly", "diagnostics", "consoleAdmin"]
                        .contains(&policy);
                    policies.push(S3Policy {
                        name: policy.to_string(),
                        policy: String::new(),
                        builtin,
                    });
                }
            }
        }

        Ok(policies)
    }

    // ========== Object Operations ==========

    /// List objects in bucket
    pub async fn list_objects(&self, bucket: &str, prefix: Option<&str>, recursive: bool) -> Result<Vec<S3Object>> {
        let mut args = vec!["ls".to_string()];

        if recursive {
            args.push("--recursive".to_string());
        }

        args.push("--json".to_string());

        let path = if let Some(p) = prefix {
            format!("{}/{}/{}", self.mc_alias, bucket, p)
        } else {
            format!("{}/{}", self.mc_alias, bucket)
        };
        args.push(path);

        let output = Command::new("mc")
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc ls failed: {}", e)))?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut objects = Vec::new();

        for line in stdout.lines() {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(line) {
                if json.get("status").and_then(|s| s.as_str()) == Some("success") {
                    if let Some(key) = json.get("key").and_then(|k| k.as_str()) {
                        objects.push(S3Object {
                            key: key.to_string(),
                            size: json.get("size").and_then(|s| s.as_u64()).unwrap_or(0),
                            last_modified: json.get("lastModified").and_then(|l| l.as_str())
                                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                                .map(|d| d.timestamp())
                                .unwrap_or(0),
                            etag: json.get("etag").and_then(|e| e.as_str()).unwrap_or("").to_string(),
                            content_type: json.get("contentType").and_then(|c| c.as_str()).map(|s| s.to_string()),
                            storage_class: json.get("storageClass").and_then(|s| s.as_str()).unwrap_or("STANDARD").to_string(),
                            version_id: json.get("versionId").and_then(|v| v.as_str()).map(|s| s.to_string()),
                            is_delete_marker: false,
                            metadata: HashMap::new(),
                            tags: HashMap::new(),
                        });
                    }
                }
            }
        }

        Ok(objects)
    }

    /// Delete object
    pub async fn delete_object(&self, bucket: &str, key: &str) -> Result<()> {
        let output = Command::new("mc")
            .args(["rm", &format!("{}/{}/{}", self.mc_alias, bucket, key)])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc rm failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to delete object: {}", stderr)));
        }

        Ok(())
    }

    /// Generate presigned URL
    pub async fn generate_presigned_url(&self, request: &PresignedUrlRequest) -> Result<String> {
        let mut args = vec!["share".to_string()];

        match request.method.to_uppercase().as_str() {
            "GET" => args.push("download".to_string()),
            "PUT" => args.push("upload".to_string()),
            _ => return Err(Error::Validation("Invalid method, use GET or PUT".to_string())),
        }

        args.push("--expire".to_string());
        args.push(format!("{}s", request.expires_secs));
        args.push(format!("{}/{}/{}", self.mc_alias, request.bucket, request.key));

        let output = Command::new("mc")
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc share failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to generate presigned URL: {}", stderr)));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        // Extract URL from output
        for line in stdout.lines() {
            if line.starts_with("http://") || line.starts_with("https://") {
                return Ok(line.trim().to_string());
            }
            if line.contains("URL: ") {
                if let Some(url) = line.split("URL: ").nth(1) {
                    return Ok(url.trim().to_string());
                }
            }
        }

        Err(Error::Internal("Failed to parse presigned URL".to_string()))
    }

    // ========== Configuration ==========

    /// Write MinIO configuration files
    pub async fn write_config(&self) -> Result<()> {
        let env_content = self.generate_env_file();

        // Create config directory
        tokio::fs::create_dir_all("/etc/minio").await.ok();
        tokio::fs::create_dir_all("/etc/minio/certs").await.ok();

        // Write environment file
        let env_path = "/etc/default/minio";
        tokio::fs::write(env_path, &env_content).await.map_err(|e| {
            Error::Internal(format!("Failed to write MinIO config: {}", e))
        })?;

        // Write systemd service file if needed
        if crate::nas::services::detect_init_system() == crate::nas::services::InitSystem::Systemd {
            self.write_systemd_service().await?;
        } else {
            self.write_openrc_service().await?;
        }

        Ok(())
    }

    /// Generate MinIO environment file
    pub fn generate_env_file(&self) -> String {
        let mut content = format!(
            "# MinIO Configuration - Generated by Horcrux\n\
             MINIO_ROOT_USER={}\n\
             MINIO_ROOT_PASSWORD={}\n\
             MINIO_VOLUMES={}\n",
            self.config.root_user,
            self.config.root_password,
            self.config.data_dirs.join(" "),
        );

        // Server address
        content.push_str(&format!(
            "MINIO_OPTS=\"--address {}:{} --console-address {}:{}\"\n",
            self.config.address,
            self.config.port,
            self.config.address,
            self.config.console_port,
        ));

        // Region
        content.push_str(&format!("MINIO_REGION={}\n", self.config.region));

        // Browser
        if !self.config.browser {
            content.push_str("MINIO_BROWSER=off\n");
        }

        // TLS
        if let Some(ref tls) = self.config.tls {
            if tls.enabled {
                content.push_str(&format!(
                    "MINIO_CERTS_DIR=/etc/minio/certs\n"
                ));
            }
        }

        // Metrics
        if self.config.metrics {
            content.push_str("MINIO_PROMETHEUS_AUTH_TYPE=public\n");
            if let Some(ref token) = self.config.metrics_auth_token {
                content.push_str(&format!("MINIO_PROMETHEUS_AUTH_TYPE=jwt\n"));
                content.push_str(&format!("MINIO_PROMETHEUS_JWT_SECRET={}\n", token));
            }
        }

        // Domain
        if let Some(ref domain) = self.config.domain {
            content.push_str(&format!("MINIO_DOMAIN={}\n", domain));
        }

        // Audit
        if self.config.audit_log {
            if let Some(ref webhook) = self.config.audit_webhook {
                content.push_str(&format!("MINIO_AUDIT_WEBHOOK_ENABLE_target1=on\n"));
                content.push_str(&format!("MINIO_AUDIT_WEBHOOK_ENDPOINT_target1={}\n", webhook));
            }
        }

        // Cache
        if let Some(ref cache) = self.config.cache {
            content.push_str(&format!("MINIO_CACHE=on\n"));
            content.push_str(&format!("MINIO_CACHE_DRIVES={}\n", cache.drives.join(",")));
            content.push_str(&format!("MINIO_CACHE_EXPIRY={}\n", cache.expiry));
            content.push_str(&format!("MINIO_CACHE_QUOTA={}\n", cache.quota));
            if !cache.exclude.is_empty() {
                content.push_str(&format!("MINIO_CACHE_EXCLUDE={}\n", cache.exclude.join(";")));
            }
        }

        content
    }

    /// Write systemd service file
    async fn write_systemd_service(&self) -> Result<()> {
        let service = r#"[Unit]
Description=MinIO S3 Gateway
Documentation=https://docs.min.io
After=network-online.target
Wants=network-online.target

[Service]
Type=notify
EnvironmentFile=/etc/default/minio
ExecStart=/usr/bin/minio server $MINIO_VOLUMES $MINIO_OPTS
Restart=always
RestartSec=5
LimitNOFILE=65536
TasksMax=infinity
TimeoutStopSec=0

[Install]
WantedBy=multi-user.target
"#;

        tokio::fs::write("/etc/systemd/system/minio.service", service).await.map_err(|e| {
            Error::Internal(format!("Failed to write systemd service: {}", e))
        })?;

        // Reload systemd
        let _ = Command::new("systemctl")
            .args(["daemon-reload"])
            .output()
            .await;

        Ok(())
    }

    /// Write OpenRC service file
    async fn write_openrc_service(&self) -> Result<()> {
        let service = r#"#!/sbin/openrc-run

name="minio"
description="MinIO S3 Gateway"

depend() {
    need net
    after firewall
}

start_pre() {
    . /etc/default/minio
}

start() {
    . /etc/default/minio
    ebegin "Starting MinIO"
    start-stop-daemon --start --background \
        --make-pidfile --pidfile /var/run/minio.pid \
        --exec /usr/bin/minio -- server $MINIO_VOLUMES $MINIO_OPTS
    eend $?
}

stop() {
    ebegin "Stopping MinIO"
    start-stop-daemon --stop --pidfile /var/run/minio.pid
    eend $?
}
"#;

        tokio::fs::write("/etc/init.d/minio", service).await.map_err(|e| {
            Error::Internal(format!("Failed to write OpenRC service: {}", e))
        })?;

        // Make executable
        let _ = Command::new("chmod")
            .args(["+x", "/etc/init.d/minio"])
            .output()
            .await;

        Ok(())
    }
}

impl Default for S3GatewayManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Options for creating a bucket
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateBucketOptions {
    /// Enable versioning
    pub versioning: bool,
    /// Enable object locking
    pub object_locking: bool,
    /// Quota
    pub quota: Option<BucketQuota>,
    /// Encryption
    pub encryption: Option<BucketEncryption>,
    /// Tags
    pub tags: HashMap<String, String>,
}

/// Generate lifecycle XML from rules
fn generate_lifecycle_xml(rules: &[LifecycleRule]) -> String {
    let mut xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<LifecycleConfiguration xmlns="http://s3.amazonaws.com/doc/2006-03-01/">"#.to_string();

    for rule in rules {
        xml.push_str(&format!(r#"
  <Rule>
    <ID>{}</ID>
    <Status>{}</Status>
    <Filter>
      <Prefix>{}</Prefix>
    </Filter>"#,
            rule.id,
            if rule.enabled { "Enabled" } else { "Disabled" },
            rule.prefix,
        ));

        if let Some(days) = rule.expiration_days {
            xml.push_str(&format!(r#"
    <Expiration>
      <Days>{}</Days>
    </Expiration>"#, days));
        }

        if let Some(days) = rule.noncurrent_expiration_days {
            xml.push_str(&format!(r#"
    <NoncurrentVersionExpiration>
      <NoncurrentDays>{}</NoncurrentDays>
    </NoncurrentVersionExpiration>"#, days));
        }

        xml.push_str("\n  </Rule>");
    }

    xml.push_str("\n</LifecycleConfiguration>");
    xml
}

/// Get disk usage for a path
async fn get_disk_usage(path: &str) -> Result<(u64, u64, u64)> {
    let output = Command::new("df")
        .args(["-B1", path])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("df failed: {}", e)))?;

    if !output.status.success() {
        return Err(Error::Internal("df failed".to_string()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 4 {
            let total = parts[1].parse::<u64>().unwrap_or(0);
            let used = parts[2].parse::<u64>().unwrap_or(0);
            let available = parts[3].parse::<u64>().unwrap_or(0);
            return Ok((total, used, available));
        }
    }

    Err(Error::Internal("Failed to parse df output".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = S3GatewayConfig::default();
        assert_eq!(config.port, 9000);
        assert_eq!(config.console_port, 9001);
        assert_eq!(config.root_user, "minioadmin");
    }

    #[test]
    fn test_env_generation() {
        let manager = S3GatewayManager::new();
        let env = manager.generate_env_file();
        assert!(env.contains("MINIO_ROOT_USER=minioadmin"));
        assert!(env.contains("MINIO_VOLUMES=/mnt/nas/s3"));
    }

    #[test]
    fn test_lifecycle_xml() {
        let rules = vec![
            LifecycleRule {
                id: "delete-old".to_string(),
                prefix: "logs/".to_string(),
                enabled: true,
                expiration_days: Some(30),
                transition_days: None,
                transition_storage_class: None,
                delete_markers: false,
                noncurrent_expiration_days: Some(7),
                tags: HashMap::new(),
            },
        ];

        let xml = generate_lifecycle_xml(&rules);
        assert!(xml.contains("<ID>delete-old</ID>"));
        assert!(xml.contains("<Days>30</Days>"));
        assert!(xml.contains("<NoncurrentDays>7</NoncurrentDays>"));
    }

    #[test]
    fn test_create_bucket_options() {
        let opts = CreateBucketOptions {
            versioning: true,
            object_locking: true,
            quota: Some(BucketQuota {
                quota_type: QuotaType::Hard,
                quota_bytes: 1024 * 1024 * 1024, // 1GB
            }),
            ..Default::default()
        };

        assert!(opts.versioning);
        assert!(opts.object_locking);
        assert!(opts.quota.is_some());
    }
}
