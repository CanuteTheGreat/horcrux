//! S3 Gateway module
//!
//! Manages MinIO for S3-compatible object storage API.
//! Provides full control over buckets, policies, users, and lifecycle.

use horcrux_common::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

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
    /// Started by Horcrux and to be started again when the API starts
    #[serde(default)]
    pub enabled: bool,
    /// Pid file, server log and client state live here
    #[serde(default = "default_run_dir")]
    pub run_dir: String,
    /// Server binary (`minio`, `silo` or a path); empty = auto-detect
    #[serde(default)]
    pub server_bin: Option<String>,
    /// MinIO client binary (`mcli`, `mc` or a path); empty = auto-detect
    #[serde(default)]
    pub client_bin: Option<String>,
}

fn default_run_dir() -> String {
    "/var/lib/horcrux/nas/s3-gateway".to_string()
}

impl Default for S3GatewayConfig {
    fn default() -> Self {
        Self {
            data_dirs: vec!["/var/lib/horcrux/nas/s3".to_string()],
            address: "0.0.0.0".to_string(),
            port: 9000,
            console_port: 9001,
            // never a well-known default password: a fresh random one
            root_user: "horcrux-admin".to_string(),
            root_password: random_alnum(40),
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
            enabled: false,
            run_dir: default_run_dir(),
            server_bin: None,
            client_bin: None,
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
    /// Bytes stored
    #[serde(default)]
    pub size_bytes: u64,
    /// Objects stored
    #[serde(default)]
    pub object_count: u64,
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
    Webhook {
        url: String,
        auth_token: Option<String>,
    },
    Amqp {
        url: String,
        exchange: String,
        routing_key: String,
    },
    Kafka {
        brokers: Vec<String>,
        topic: String,
    },
    Nats {
        address: String,
        subject: String,
    },
    Redis {
        address: String,
        key: String,
    },
    Postgres {
        connection_string: String,
        table: String,
    },
    Mysql {
        dsn: String,
        table: String,
    },
    Elasticsearch {
        url: String,
        index: String,
    },
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
    /// Running (process alive and answering its health check)
    pub running: bool,
    /// Started from Horcrux; started again when the API starts
    pub enabled: bool,
    /// Server process id
    pub pid: Option<u32>,
    /// S3 API port
    pub port: u16,
    /// Web console port
    pub console_port: u16,
    /// http or https
    pub scheme: String,
    /// Server binary in use (None = not installed)
    pub server_binary: Option<String>,
    /// Client binary in use (None = not installed)
    pub client_binary: Option<String>,
    /// What has to be installed before the gateway can start
    pub missing: Vec<String>,
    /// Root (admin) user name; its password is never returned
    pub root_user: String,
    /// Data directories
    pub data_dirs: Vec<String>,
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
///
/// Horcrux supervises the S3 server process itself (like it does QEMU): the
/// server is started detached in its own process group with a pid file, so
/// it keeps running across API restarts, and the API starts it again at boot
/// when the gateway is enabled. Administration goes through the MinIO client
/// (`mcli`/`mc`) with the root credentials passed in `MC_HOST_<alias>`, so no
/// client configuration is left in anyone's home directory.
///
/// Any MinIO-compatible server works: `minio` or `silo` (the community fork
/// that still ships binaries) - see `S3GatewayConfig::server_bin`.
pub struct S3GatewayManager {
    config: S3GatewayConfig,
    mc_alias: String,
}

/// Seconds to wait for a freshly started server to answer its health check.
const START_TIMEOUT_SECS: u64 = 45;

/// A random alphanumeric string (credentials, generated keys).
pub fn random_alnum(len: usize) -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| rng.sample(rand::distributions::Alphanumeric) as char)
        .collect()
}

/// Find `name` in `$PATH` (plus the usual sbin dirs a service PATH may lack).
fn find_in_path(name: &str) -> Option<String> {
    let mut dirs: Vec<std::path::PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    for extra in ["/usr/local/bin", "/usr/bin", "/opt/horcrux/bin"] {
        dirs.push(extra.into());
    }
    dirs.into_iter()
        .map(|d| d.join(name))
        .find(|p| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(p)
                .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        })
        .map(|p| p.to_string_lossy().into_owned())
}

/// Is `bin` a MinIO-compatible client (and not GNU Midnight Commander, which
/// is also installed as `mc` on many hosts)?
fn is_minio_client(bin: &str) -> bool {
    std::process::Command::new(bin)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .output()
        .map(|o| {
            let out = String::from_utf8_lossy(&o.stdout).to_lowercase();
            o.status.success() && out.contains("release.") && !out.contains("midnight")
        })
        .unwrap_or(false)
}

/// pid of a live (not zombie) process whose command line contains `needle`.
fn live_pid(pid: i32, needle: &str) -> bool {
    let stat = match std::fs::read_to_string(format!("/proc/{}/stat", pid)) {
        Ok(s) => s,
        Err(_) => return false,
    };
    // state is the first field after the parenthesised comm
    let state = stat
        .rsplit_once(')')
        .and_then(|(_, rest)| rest.split_whitespace().next())
        .unwrap_or("X");
    if state == "Z" || state == "X" {
        return false;
    }
    std::fs::read(format!("/proc/{}/cmdline", pid))
        .map(|c| String::from_utf8_lossy(&c).contains(needle))
        .unwrap_or(false)
}

impl S3GatewayManager {
    /// Create a new S3 gateway manager with the default configuration
    pub fn new() -> Self {
        Self::with_config(S3GatewayConfig::default())
    }

    /// Create with specific configuration
    pub fn with_config(config: S3GatewayConfig) -> Self {
        Self {
            config,
            mc_alias: "horcrux".to_string(),
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

    /// The S3 server binary: the configured one, else `minio`, else `silo`.
    pub fn server_binary(&self) -> Option<String> {
        if let Some(b) = self.config.server_bin.as_deref().filter(|b| !b.is_empty()) {
            return if b.contains('/') {
                std::path::Path::new(b).exists().then(|| b.to_string())
            } else {
                find_in_path(b)
            };
        }
        find_in_path("minio").or_else(|| find_in_path("silo"))
    }

    /// The MinIO client binary: the configured one, else `mcli`, else an
    /// `mc` that really is the MinIO client.
    pub fn client_binary(&self) -> Option<String> {
        if let Some(b) = self.config.client_bin.as_deref().filter(|b| !b.is_empty()) {
            return if b.contains('/') {
                std::path::Path::new(b).exists().then(|| b.to_string())
            } else {
                find_in_path(b)
            };
        }
        find_in_path("mcli").or_else(|| find_in_path("mc").filter(|b| is_minio_client(b)))
    }

    /// Host:port the API (and the client) reaches the server on.
    fn local_endpoint(&self) -> String {
        let host = match self.config.address.as_str() {
            "" | "0.0.0.0" | "::" | "[::]" => "127.0.0.1",
            a => a,
        };
        format!("{}:{}", host, self.config.port)
    }

    fn scheme(&self) -> &'static str {
        if self.config.tls.as_ref().map(|t| t.enabled).unwrap_or(false) {
            "https"
        } else {
            "http"
        }
    }

    fn pid_file(&self) -> std::path::PathBuf {
        std::path::Path::new(&self.config.run_dir).join("server.pid")
    }

    /// Server log (stdout + stderr of the server process).
    pub fn log_file(&self) -> std::path::PathBuf {
        std::path::Path::new(&self.config.run_dir).join("server.log")
    }

    /// A client command bound to this gateway's root credentials.
    fn mc(&self) -> Command {
        let bin = self.client_binary().unwrap_or_else(|| "mcli".to_string());
        let mut cmd = Command::new(bin);
        cmd.env(
            format!("MC_HOST_{}", self.mc_alias),
            format!(
                "{}://{}:{}@{}",
                self.scheme(),
                self.config.root_user,
                self.config.root_password,
                self.local_endpoint()
            ),
        )
        .env(
            "MC_CONFIG_DIR",
            std::path::Path::new(&self.config.run_dir).join("mc"),
        )
        .env("MC_NO_COLOR", "1")
        .env("MC_DISABLE_PAGER", "1")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
        if self.scheme() == "https" {
            cmd.env("MC_INSECURE", "1");
        }
        cmd
    }

    /// Run a client command and return stdout, or the client's own error
    /// message (JSON `error.cause.message` / `error.message`, else stderr).
    async fn mc_run(&self, args: &[&str]) -> Result<String> {
        if self.client_binary().is_none() {
            return Err(Error::Internal(
                "MinIO client not installed (need `mcli` or the MinIO `mc`)".to_string(),
            ));
        }
        let output = self
            .mc()
            .args(args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to run the MinIO client: {}", e)))?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        if output.status.success() {
            return Ok(stdout);
        }
        Err(Error::Internal(mc_error_message(
            &stdout,
            &String::from_utf8_lossy(&output.stderr),
        )))
    }

    /// pid of the running server, if any.
    pub fn running_pid(&self) -> Option<i32> {
        let content = std::fs::read_to_string(self.pid_file()).ok()?;
        let pid: i32 = content.lines().next()?.trim().parse().ok()?;
        let needle = self
            .server_binary()
            .and_then(|b| {
                std::path::Path::new(&b)
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "server".to_string());
        live_pid(pid, &needle).then_some(pid)
    }

    /// Start time (unix seconds) recorded with the pid.
    fn started_at(&self) -> Option<i64> {
        let content = std::fs::read_to_string(self.pid_file()).ok()?;
        content.lines().nth(1)?.trim().parse().ok()
    }

    /// Does the server answer its liveness endpoint?
    pub async fn healthy(&self) -> bool {
        use tokio::io::AsyncReadExt;
        let ep = self.local_endpoint();
        let fut = async {
            let mut s = tokio::net::TcpStream::connect(&ep).await.ok()?;
            s.write_all(
                format!("GET /minio/health/live HTTP/1.0\r\nHost: {}\r\n\r\n", ep).as_bytes(),
            )
            .await
            .ok()?;
            let mut buf = vec![0u8; 64];
            let n = s.read(&mut buf).await.ok()?;
            Some(String::from_utf8_lossy(&buf[..n]).contains(" 200"))
        };
        if self.scheme() == "https" {
            // health over TLS: fall back to "the port accepts connections"
            return tokio::time::timeout(
                std::time::Duration::from_secs(3),
                tokio::net::TcpStream::connect(&ep),
            )
            .await
            .map(|r| r.is_ok())
            .unwrap_or(false);
        }
        tokio::time::timeout(std::time::Duration::from_secs(3), fut)
            .await
            .ok()
            .flatten()
            .unwrap_or(false)
    }

    /// Start the S3 gateway (no-op if it is already running).
    pub async fn start(&self) -> Result<()> {
        if self.running_pid().is_some() {
            return Ok(());
        }
        let server = self.server_binary().ok_or_else(|| {
            Error::Internal(
                "S3 server not installed: install MinIO (`minio`) or Silo (`silo`), or set the server binary in the gateway settings".to_string(),
            )
        })?;
        if self.client_binary().is_none() {
            return Err(Error::Internal(
                "MinIO client not installed: install `mcli` (or the MinIO `mc`) - the gateway is administered through it".to_string(),
            ));
        }
        if self.config.data_dirs.is_empty() {
            return Err(Error::Validation(
                "No data directory configured".to_string(),
            ));
        }
        for dir in &self.config.data_dirs {
            tokio::fs::create_dir_all(dir).await.map_err(|e| {
                Error::Internal(format!("Failed to create S3 data dir {}: {}", dir, e))
            })?;
        }
        tokio::fs::create_dir_all(&self.config.run_dir)
            .await
            .map_err(|e| {
                Error::Internal(format!(
                    "Failed to create S3 run dir {}: {}",
                    self.config.run_dir, e
                ))
            })?;
        // Something else already on the port would make the health check
        // pass against the wrong server.
        if self.healthy().await {
            return Err(Error::Conflict(format!(
                "Port {} already serves an S3 endpoint that Horcrux did not start",
                self.config.port
            )));
        }

        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.log_file())
            .map_err(|e| Error::Internal(format!("Failed to open S3 server log: {}", e)))?;
        let log_err = log
            .try_clone()
            .map_err(|e| Error::Internal(format!("Failed to open S3 server log: {}", e)))?;

        let mut cmd = Command::new(&server);
        cmd.arg("server");
        for dir in &self.config.data_dirs {
            cmd.arg(dir);
        }
        cmd.arg("--address")
            .arg(format!("{}:{}", self.config.address, self.config.port))
            .arg("--console-address")
            .arg(format!(
                "{}:{}",
                self.config.address, self.config.console_port
            ));
        if self.config.tls.as_ref().map(|t| t.enabled).unwrap_or(false) {
            cmd.arg("--certs-dir")
                .arg(std::path::Path::new(&self.config.run_dir).join("certs"));
        }
        for (k, v) in self.server_env() {
            cmd.env(k, v);
        }
        cmd.stdin(std::process::Stdio::null())
            .stdout(log)
            .stderr(log_err)
            // own process group: not killed with the API's group, survives
            // an API restart
            .process_group(0);
        let mut child = cmd
            .spawn()
            .map_err(|e| Error::Internal(format!("Failed to start {}: {}", server, e)))?;
        let pid = child.id().unwrap_or(0) as i32;
        std::fs::write(
            self.pid_file(),
            format!("{}\n{}\n", pid, chrono::Utc::now().timestamp()),
        )
        .map_err(|e| Error::Internal(format!("Failed to write S3 pid file: {}", e)))?;
        // Reap the child when it exits (no zombie while the API runs).
        tokio::spawn(async move {
            let _ = child.wait().await;
        });

        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(START_TIMEOUT_SECS);
        loop {
            if self.healthy().await {
                tracing::info!(
                    "S3 gateway started (pid {}, {}://{})",
                    pid,
                    self.scheme(),
                    self.local_endpoint()
                );
                return Ok(());
            }
            if self.running_pid().is_none() || std::time::Instant::now() > deadline {
                let _ = self.stop().await;
                return Err(Error::Internal(format!(
                    "S3 server did not come up: {}",
                    self.log_tail(15)
                )));
            }
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }
    }

    /// Environment of the server process.
    pub fn server_env(&self) -> Vec<(String, String)> {
        let mut env = vec![
            ("MINIO_ROOT_USER".to_string(), self.config.root_user.clone()),
            (
                "MINIO_ROOT_PASSWORD".to_string(),
                self.config.root_password.clone(),
            ),
            ("MINIO_SITE_REGION".to_string(), self.config.region.clone()),
            // nothing phones home / offers updates from a managed install
            ("MINIO_UPDATE".to_string(), "off".to_string()),
        ];
        if !self.config.browser {
            env.push(("MINIO_BROWSER".to_string(), "off".to_string()));
        }
        if self.config.metrics {
            env.push((
                "MINIO_PROMETHEUS_AUTH_TYPE".to_string(),
                if self.config.metrics_auth_token.is_some() {
                    "jwt"
                } else {
                    "public"
                }
                .to_string(),
            ));
        }
        if let Some(ref domain) = self.config.domain {
            env.push(("MINIO_DOMAIN".to_string(), domain.clone()));
        }
        if self.config.audit_log {
            if let Some(ref webhook) = self.config.audit_webhook {
                env.push(("MINIO_AUDIT_WEBHOOK_ENABLE_target1".into(), "on".into()));
                env.push((
                    "MINIO_AUDIT_WEBHOOK_ENDPOINT_target1".into(),
                    webhook.clone(),
                ));
            }
        }
        env
    }

    /// Last `n` lines of the server log (for error messages).
    pub fn log_tail(&self, n: usize) -> String {
        let text = std::fs::read_to_string(self.log_file()).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        let tail = lines[lines.len().saturating_sub(n)..].join("\n");
        if tail.trim().is_empty() {
            "(no server output)".to_string()
        } else {
            tail
        }
    }

    /// Stop the S3 gateway (SIGTERM, then SIGKILL after 15 s).
    pub async fn stop(&self) -> Result<()> {
        if let Some(pid) = self.running_pid() {
            use nix::sys::signal::{kill, Signal};
            use nix::unistd::Pid;
            let _ = kill(Pid::from_raw(pid), Signal::SIGTERM);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
            while self.running_pid().is_some() && std::time::Instant::now() < deadline {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
            if self.running_pid().is_some() {
                let _ = kill(Pid::from_raw(pid), Signal::SIGKILL);
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            if self.running_pid().is_some() {
                return Err(Error::Internal(format!(
                    "S3 server (pid {}) did not stop",
                    pid
                )));
            }
            tracing::info!("S3 gateway stopped (pid {})", pid);
        }
        let _ = std::fs::remove_file(self.pid_file());
        Ok(())
    }

    /// Restart the S3 gateway
    pub async fn restart(&self) -> Result<()> {
        self.stop().await?;
        self.start().await
    }

    /// Check if S3 gateway is running
    pub async fn is_running(&self) -> bool {
        self.running_pid().is_some()
    }

    /// Get S3 gateway status
    pub async fn get_status(&self) -> Result<S3GatewayStatus> {
        let pid = self.running_pid();
        let running = pid.is_some() && self.healthy().await;
        let server_binary = self.server_binary();
        let client_binary = self.client_binary();
        let mut missing = Vec::new();
        if server_binary.is_none() {
            missing.push("S3 server (minio or silo)".to_string());
        }
        if client_binary.is_none() {
            missing.push("MinIO client (mcli or mc)".to_string());
        }

        let buckets = if running {
            self.list_buckets().await.unwrap_or_default()
        } else {
            Vec::new()
        };
        let version = match &server_binary {
            Some(b) => get_version(b).await.ok(),
            None => None,
        };
        let drives = if running {
            self.get_drive_info().await.unwrap_or_default()
        } else {
            Vec::new()
        };
        let (storage_used, storage_available) = drives.iter().fold((0u64, 0u64), |acc, d| {
            (acc.0 + d.used_bytes, acc.1 + d.available_bytes)
        });
        let uptime = if running {
            self.started_at()
                .map(|t| (chrono::Utc::now().timestamp() - t).max(0) as u64)
        } else {
            None
        };
        let object_count = buckets.iter().map(|b| b.object_count).sum();

        Ok(S3GatewayStatus {
            running,
            enabled: self.config.enabled,
            pid: pid.filter(|_| running).map(|p| p as u32),
            version,
            address: format!("{}:{}", self.config.address, self.config.port),
            console_address: format!("{}:{}", self.config.address, self.config.console_port),
            port: self.config.port,
            console_port: self.config.console_port,
            scheme: self.scheme().to_string(),
            bucket_count: buckets.len() as u32,
            object_count,
            storage_used_bytes: storage_used,
            storage_available_bytes: storage_available,
            region: self.config.region.clone(),
            tls_enabled: self.config.tls.as_ref().map(|t| t.enabled).unwrap_or(false),
            uptime_secs: uptime,
            servers: vec![],
            drives,
            server_binary,
            client_binary,
            missing,
            root_user: self.config.root_user.clone(),
            data_dirs: self.config.data_dirs.clone(),
        })
    }

    /// Get drive information
    async fn get_drive_info(&self) -> Result<Vec<DriveInfo>> {
        let mut drives = Vec::new();
        if let Ok(stdout) = self
            .mc_run(&["admin", "info", &self.mc_alias, "--json"])
            .await
        {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&stdout) {
                let servers = json
                    .pointer("/info/servers")
                    .and_then(|s| s.as_array())
                    .cloned()
                    .unwrap_or_default();
                for server in servers {
                    for disk in server
                        .get("drives")
                        .and_then(|d| d.as_array())
                        .cloned()
                        .unwrap_or_default()
                    {
                        let n = |k: &str| disk.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
                        drives.push(DriveInfo {
                            path: disk
                                .get("path")
                                .or_else(|| disk.get("endpoint"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            state: disk
                                .get("state")
                                .and_then(|v| v.as_str())
                                .unwrap_or("ok")
                                .to_string(),
                            total_bytes: n("totalspace"),
                            used_bytes: n("usedspace"),
                            available_bytes: n("availspace"),
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
                        state: "ok".to_string(),
                        total_bytes: total,
                        used_bytes: used,
                        available_bytes: available,
                    });
                }
            }
        }

        Ok(drives)
    }

    /// Size and object count of a bucket (`du`).
    async fn bucket_usage(&self, bucket: &str) -> (u64, u64) {
        let target = format!("{}/{}", self.mc_alias, bucket);
        match self.mc_run(&["du", &target, "--json"]).await {
            Ok(stdout) => stdout
                .lines()
                .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                .find(|j| j.get("status").and_then(|s| s.as_str()) == Some("success"))
                .map(|j| {
                    (
                        j.get("size").and_then(|v| v.as_u64()).unwrap_or(0),
                        j.get("objects").and_then(|v| v.as_u64()).unwrap_or(0),
                    )
                })
                .unwrap_or((0, 0)),
            Err(_) => (0, 0),
        }
    }

    // ========== Bucket Operations ==========

    /// Create a bucket
    pub async fn create_bucket(
        &self,
        name: &str,
        options: Option<CreateBucketOptions>,
    ) -> Result<S3Bucket> {
        validate_bucket_name(name)?;
        let opts = options.unwrap_or_default();

        // Create bucket using mc
        let mut args = vec!["mb".to_string()];

        if opts.object_locking {
            args.push("--with-lock".to_string());
        }

        args.push(format!("{}/{}", self.mc_alias, name));

        let output = self
            .mc()
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to create bucket: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Check if bucket already exists
            if stderr.contains("already exists") || stderr.contains("already own") {
                return Err(Error::Conflict(format!("Bucket '{}' already exists", name)));
            }
            return Err(Error::Internal(format!(
                "Failed to create bucket: {}",
                stderr
            )));
        }

        // Set versioning if requested
        if opts.versioning {
            self.set_bucket_versioning(name, true).await?;
        }

        // Set quota if specified
        if let Some(ref quota) = opts.quota {
            let _ = self.set_bucket_quota(name, quota).await;
        }

        // Set encryption if specified
        if let Some(ref encryption) = opts.encryption {
            let _ = self.set_bucket_encryption(name, encryption).await;
        }

        // Set tags if specified
        if !opts.tags.is_empty() {
            let _ = self.set_bucket_tags(name, &opts.tags).await;
        }

        Ok(S3Bucket {
            name: name.to_string(),
            path: format!(
                "{}/{}",
                self.config
                    .data_dirs
                    .first()
                    .unwrap_or(&"/var/lib/horcrux/nas/s3".to_string()),
                name
            ),
            versioning: opts.versioning,
            object_locking: opts.object_locking,
            quota_bytes: opts.quota.map(|q| q.quota_bytes),
            created_at: chrono::Utc::now().timestamp(),
            tags: opts.tags,
            encryption: opts.encryption,
            lifecycle_rules: vec![],
            replication: None,
            notifications: vec![],
            size_bytes: 0,
            object_count: 0,
        })
    }

    /// Delete a bucket
    pub async fn delete_bucket(&self, name: &str, force: bool) -> Result<()> {
        let mut args = vec!["rb".to_string()];

        if force {
            args.push("--force".to_string());
        }

        args.push(format!("{}/{}", self.mc_alias, name));

        let output = self
            .mc()
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to delete bucket: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("not empty") {
                return Err(Error::Conflict("Bucket is not empty".to_string()));
            }
            return Err(Error::Internal(format!(
                "Failed to delete bucket: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// List buckets
    pub async fn list_buckets(&self) -> Result<Vec<S3Bucket>> {
        let output = self
            .mc()
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
                            path: format!(
                                "{}/{}",
                                self.config
                                    .data_dirs
                                    .first()
                                    .unwrap_or(&"/var/lib/horcrux/nas/s3".to_string()),
                                name
                            ),
                            versioning: false,
                            object_locking: false,
                            quota_bytes: None,
                            created_at: json
                                .get("lastModified")
                                .and_then(|v| v.as_str())
                                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                                .map(|d| d.timestamp())
                                .unwrap_or(0),
                            tags: HashMap::new(),
                            encryption: None,
                            lifecycle_rules: vec![],
                            replication: None,
                            notifications: vec![],
                            size_bytes: 0,
                            object_count: 0,
                        });
                    }
                }
            }
        }

        for b in buckets.iter_mut() {
            let (size, objects) = self.bucket_usage(&b.name).await;
            b.size_bytes = size;
            b.object_count = objects;
            b.versioning = self.get_bucket_versioning(&b.name).await.unwrap_or(false);
        }

        Ok(buckets)
    }

    /// Get bucket info
    pub async fn get_bucket(&self, name: &str) -> Result<S3Bucket> {
        let output = self
            .mc()
            .args(["stat", &format!("{}/{}", self.mc_alias, name), "--json"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc stat failed: {}", e)))?;

        if !output.status.success() {
            return Err(Error::NotFound(format!("Bucket '{}' not found", name)));
        }

        // Get versioning status
        let versioning = self.get_bucket_versioning(name).await.unwrap_or(false);

        // Get quota
        let quota = self.get_bucket_quota(name).await.ok().flatten();

        // Get tags
        let tags = self.get_bucket_tags(name).await.unwrap_or_default();

        // Get lifecycle rules
        let lifecycle_rules = self.get_bucket_lifecycle(name).await.unwrap_or_default();
        let (size_bytes, object_count) = self.bucket_usage(name).await;

        Ok(S3Bucket {
            name: name.to_string(),
            path: format!(
                "{}/{}",
                self.config
                    .data_dirs
                    .first()
                    .unwrap_or(&"/var/lib/horcrux/nas/s3".to_string()),
                name
            ),
            versioning,
            object_locking: false, // Would need to parse from mc output
            quota_bytes: quota.map(|q| q.quota_bytes),
            created_at: 0,
            tags,
            encryption: None,
            lifecycle_rules,
            replication: None,
            notifications: vec![],
            size_bytes,
            object_count,
        })
    }

    /// Set bucket versioning
    pub async fn set_bucket_versioning(&self, bucket: &str, enabled: bool) -> Result<()> {
        let status = if enabled { "enable" } else { "suspend" };

        let output = self
            .mc()
            .args(["version", status, &format!("{}/{}", self.mc_alias, bucket)])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc version failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "Failed to set versioning: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Get bucket versioning status
    pub async fn get_bucket_versioning(&self, bucket: &str) -> Result<bool> {
        let output = self
            .mc()
            .args([
                "version",
                "info",
                &format!("{}/{}", self.mc_alias, bucket),
                "--json",
            ])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc version failed: {}", e)))?;

        if !output.status.success() {
            return Ok(false);
        }

        // {"status":"success","versioning":{"status":"Enabled","MFADelete":""}}
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .any(|j| {
                let v = j.get("versioning");
                v.and_then(|v| v.get("status")).and_then(|s| s.as_str()) == Some("Enabled")
                    || v.and_then(|v| v.as_str()) == Some("Enabled")
            }))
    }

    /// Set bucket quota
    pub async fn set_bucket_quota(&self, bucket: &str, quota: &BucketQuota) -> Result<()> {
        let quota_type = match quota.quota_type {
            QuotaType::Hard => "hard",
            QuotaType::Fifo => "fifo",
        };

        let output = self
            .mc()
            .args([
                "quota",
                "set",
                &format!("{}/{}", self.mc_alias, bucket),
                "--size",
                &format!("{}B", quota.quota_bytes),
                "--type",
                quota_type,
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
        let output = self
            .mc()
            .args([
                "quota",
                "info",
                &format!("{}/{}", self.mc_alias, bucket),
                "--json",
            ])
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
                        quota_type: if quota_type == "fifo" {
                            QuotaType::Fifo
                        } else {
                            QuotaType::Hard
                        },
                        quota_bytes: quota,
                    }));
                }
            }
        }

        Ok(None)
    }

    /// Clear bucket quota
    pub async fn clear_bucket_quota(&self, bucket: &str) -> Result<()> {
        let output = self
            .mc()
            .args(["quota", "clear", &format!("{}/{}", self.mc_alias, bucket)])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc quota failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "Failed to clear quota: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Set bucket encryption
    pub async fn set_bucket_encryption(
        &self,
        bucket: &str,
        encryption: &BucketEncryption,
    ) -> Result<()> {
        let enc_type = match encryption.encryption_type {
            EncryptionType::SseS3 => "sse-s3",
            EncryptionType::SseKms => "sse-kms",
        };

        let mut args = vec!["encrypt", "set", enc_type];

        if let Some(ref key_id) = encryption.kms_key_id {
            args.push(key_id);
        }

        let target = format!("{}/{}", self.mc_alias, bucket);
        args.push(&target);

        let output = self
            .mc()
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc encrypt failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "Failed to set encryption: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Set bucket tags
    pub async fn set_bucket_tags(
        &self,
        bucket: &str,
        tags: &HashMap<String, String>,
    ) -> Result<()> {
        let tags_str: Vec<String> = tags.iter().map(|(k, v)| format!("{}={}", k, v)).collect();

        let output = self
            .mc()
            .args([
                "tag",
                "set",
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
        let output = self
            .mc()
            .args([
                "tag",
                "list",
                &format!("{}/{}", self.mc_alias, bucket),
                "--json",
            ])
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
        tokio::fs::write(&temp_file, &xml)
            .await
            .map_err(|e| Error::Internal(format!("Failed to write lifecycle config: {}", e)))?;

        let output = self
            .mc()
            .args(["ilm", "import", &format!("{}/{}", self.mc_alias, bucket)])
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
        let output = self
            .mc()
            .args([
                "ilm",
                "ls",
                &format!("{}/{}", self.mc_alias, bucket),
                "--json",
            ])
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
                        prefix: json
                            .get("prefix")
                            .and_then(|p| p.as_str())
                            .unwrap_or("")
                            .to_string(),
                        enabled: json.get("status").and_then(|s| s.as_str()) == Some("Enabled"),
                        expiration_days: json
                            .get("expiration")
                            .and_then(|e| e.get("days"))
                            .and_then(|d| d.as_u64())
                            .map(|d| d as u32),
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
        let output = self
            .mc()
            .args(["admin", "user", "add", &self.mc_alias, username, password])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc admin user add failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "Failed to create user: {}",
                stderr
            )));
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
        let output = self
            .mc()
            .args(["admin", "user", "remove", &self.mc_alias, username])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc admin user remove failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "Failed to delete user: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// List S3 users
    pub async fn list_users(&self) -> Result<Vec<S3User>> {
        let output = self
            .mc()
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
                        policies: json
                            .get("policyName")
                            .and_then(|p| p.as_str())
                            .map(|s| vec![s.to_string()])
                            .unwrap_or_default(),
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
        let output = self
            .mc()
            .args([
                "admin",
                "policy",
                "attach",
                &self.mc_alias,
                policy,
                "--user",
                username,
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

        let output = self
            .mc()
            .args(["admin", "user", status, &self.mc_alias, username])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc admin user {} failed: {}", status, e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "Failed to {} user: {}",
                status, stderr
            )));
        }

        Ok(())
    }

    // ========== Access Key Operations ==========

    /// Create access key
    pub async fn create_access_key(
        &self,
        user_id: &str,
        description: Option<&str>,
    ) -> Result<S3AccessKey> {
        // Generate random access key and secret
        let (access_key, secret_key) = {
            use rand::Rng;
            let mut rng = rand::thread_rng();
            let access_key: String = (0..20)
                .map(|_| rng.sample(rand::distributions::Alphanumeric) as char)
                .collect();
            let secret_key: String = (0..40)
                .map(|_| rng.sample(rand::distributions::Alphanumeric) as char)
                .collect();
            (access_key, secret_key)
        };

        let mut args = vec![
            "admin".to_string(),
            "user".to_string(),
            "svcacct".to_string(),
            "add".to_string(),
            "--access-key".to_string(),
            access_key.clone(),
            "--secret-key".to_string(),
            secret_key.clone(),
        ];

        if let Some(desc) = description {
            args.push("--description".to_string());
            args.push(desc.to_string());
        }

        args.push(self.mc_alias.clone());
        args.push(user_id.to_string());

        let output = self
            .mc()
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc svcacct add failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "Failed to create access key: {}",
                stderr
            )));
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
        let output = self
            .mc()
            .args(["admin", "user", "svcacct", "rm", &self.mc_alias, access_key])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc svcacct rm failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "Failed to delete access key: {}",
                stderr
            )));
        }

        Ok(())
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
        tokio::fs::write(&temp_file, policy_json)
            .await
            .map_err(|e| Error::Internal(format!("Failed to write policy file: {}", e)))?;

        let output = self
            .mc()
            .args([
                "admin",
                "policy",
                "create",
                &self.mc_alias,
                name,
                &temp_file,
            ])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc policy create failed: {}", e)))?;

        // Clean up
        let _ = tokio::fs::remove_file(&temp_file).await;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "Failed to create policy: {}",
                stderr
            )));
        }

        Ok(S3Policy {
            name: name.to_string(),
            policy: policy_json.to_string(),
            builtin: false,
        })
    }

    /// Delete policy
    pub async fn delete_policy(&self, name: &str) -> Result<()> {
        let output = self
            .mc()
            .args(["admin", "policy", "remove", &self.mc_alias, name])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc policy remove failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "Failed to delete policy: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// List policies
    pub async fn list_policies(&self) -> Result<Vec<S3Policy>> {
        let output = self
            .mc()
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
                    let builtin = [
                        "readonly",
                        "readwrite",
                        "writeonly",
                        "diagnostics",
                        "consoleAdmin",
                    ]
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
    pub async fn list_objects(
        &self,
        bucket: &str,
        prefix: Option<&str>,
        recursive: bool,
    ) -> Result<Vec<S3Object>> {
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

        let output = self
            .mc()
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
                            last_modified: json
                                .get("lastModified")
                                .and_then(|l| l.as_str())
                                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                                .map(|d| d.timestamp())
                                .unwrap_or(0),
                            etag: json
                                .get("etag")
                                .and_then(|e| e.as_str())
                                .unwrap_or("")
                                .to_string(),
                            content_type: json
                                .get("contentType")
                                .and_then(|c| c.as_str())
                                .map(|s| s.to_string()),
                            storage_class: json
                                .get("storageClass")
                                .and_then(|s| s.as_str())
                                .unwrap_or("STANDARD")
                                .to_string(),
                            version_id: json
                                .get("versionId")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
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
        let output = self
            .mc()
            .args(["rm", &format!("{}/{}/{}", self.mc_alias, bucket, key)])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc rm failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "Failed to delete object: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Generate presigned URL
    pub async fn generate_presigned_url(&self, request: &PresignedUrlRequest) -> Result<String> {
        let mut args = vec!["share".to_string()];

        match request.method.to_uppercase().as_str() {
            "GET" => args.push("download".to_string()),
            "PUT" => args.push("upload".to_string()),
            _ => {
                return Err(Error::Validation(
                    "Invalid method, use GET or PUT".to_string(),
                ))
            }
        }

        args.push("--expire".to_string());
        args.push(format!("{}s", request.expires_secs));
        args.push(format!(
            "{}/{}/{}",
            self.mc_alias, request.bucket, request.key
        ));

        let output = self
            .mc()
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("mc share failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "Failed to generate presigned URL: {}",
                stderr
            )));
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

    /// Every access key on the server, as `(parent user, masked key)`.
    ///
    /// The client masks access keys in listings (`AKIATE...0001`); callers
    /// match them against the keys Horcrux created with `mask_access_key`.
    pub async fn list_server_access_keys(&self) -> Result<Vec<(String, String)>> {
        let stdout = self
            .mc_run(&[
                "admin",
                "accesskey",
                "ls",
                &self.mc_alias,
                "--all",
                "--json",
            ])
            .await?;
        let mut out = Vec::new();
        for line in stdout.lines() {
            let Ok(json) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let user = json.get("user").and_then(|u| u.as_str()).unwrap_or("");
            for key in json
                .get("svcaccs")
                .and_then(|s| s.as_array())
                .cloned()
                .unwrap_or_default()
            {
                if let Some(k) = key.get("accessKey").and_then(|k| k.as_str()) {
                    out.push((user.to_string(), k.to_string()));
                }
            }
        }
        Ok(out)
    }

    /// Details of one access key, or `NotFound` if the server lacks it.
    pub async fn access_key_info(&self, access_key: &str) -> Result<S3AccessKey> {
        let stdout = self
            .mc_run(&[
                "admin",
                "user",
                "svcacct",
                "info",
                &self.mc_alias,
                access_key,
                "--json",
            ])
            .await
            .map_err(|e| {
                let msg = e.to_string();
                if msg.contains("does not exist") || msg.contains("NoSuch") {
                    Error::NotFound(format!("Access key {} not found", access_key))
                } else {
                    e
                }
            })?;
        let json: serde_json::Value = stdout
            .lines()
            .find_map(|l| serde_json::from_str(l).ok())
            .ok_or_else(|| Error::Internal("Unreadable access key info".to_string()))?;
        let s = |k: &str| json.get(k).and_then(|v| v.as_str()).map(|v| v.to_string());
        Ok(S3AccessKey {
            access_key: access_key.to_string(),
            secret_key: String::new(),
            user_id: s("parentUser").unwrap_or_default(),
            description: s("description").filter(|d| !d.is_empty()),
            enabled: s("accountStatus").as_deref() != Some("off"),
            expires_at: None,
            created_at: 0,
            last_used: None,
            policies: vec![],
        })
    }

    /// Give an access key a new secret (the access key id stays). Returns
    /// the new secret; the old one stops working at once.
    pub async fn rotate_access_key(&self, access_key: &str) -> Result<String> {
        let secret = random_alnum(40);
        self.mc_run(&[
            "admin",
            "user",
            "svcacct",
            "edit",
            "--secret-key",
            &secret,
            &self.mc_alias,
            access_key,
            "--json",
        ])
        .await?;
        Ok(secret)
    }

    /// Enable or disable an access key.
    pub async fn set_access_key_enabled(&self, access_key: &str, enabled: bool) -> Result<()> {
        let action = if enabled { "enable" } else { "disable" };
        self.mc_run(&[
            "admin",
            "user",
            "svcacct",
            action,
            &self.mc_alias,
            access_key,
            "--json",
        ])
        .await
        .map(|_| ())
    }

    /// Upload an object: `data` is streamed to the client's stdin.
    pub async fn put_object<R>(&self, bucket: &str, key: &str, data: &mut R) -> Result<u64>
    where
        R: tokio::io::AsyncRead + Unpin + ?Sized,
    {
        validate_object_key(key)?;
        let target = format!("{}/{}/{}", self.mc_alias, bucket, key);
        let mut child = self
            .mc()
            .args(["pipe", &target, "--json"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| Error::Internal(format!("Failed to run the MinIO client: {}", e)))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::Internal("client stdin unavailable".to_string()))?;
        let copied = tokio::io::copy(data, &mut stdin).await;
        drop(stdin);
        let output = child
            .wait_with_output()
            .await
            .map_err(|e| Error::Internal(format!("Upload failed: {}", e)))?;
        let total = copied.map_err(|e| Error::Internal(format!("Upload failed: {}", e)))?;
        if !output.status.success() {
            return Err(Error::Internal(mc_error_message(
                &String::from_utf8_lossy(&output.stdout),
                &String::from_utf8_lossy(&output.stderr),
            )));
        }
        Ok(total)
    }

    /// Read an object (whole body).
    pub async fn get_object(&self, bucket: &str, key: &str) -> Result<Vec<u8>> {
        validate_object_key(key)?;
        let target = format!("{}/{}/{}", self.mc_alias, bucket, key);
        let output = self
            .mc()
            .args(["cat", &target])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to run the MinIO client: {}", e)))?;
        if !output.status.success() {
            let msg = mc_error_message(
                &String::from_utf8_lossy(&output.stdout),
                &String::from_utf8_lossy(&output.stderr),
            );
            if msg.contains("does not exist") {
                return Err(Error::NotFound(format!("Object {} not found", key)));
            }
            return Err(Error::Internal(msg));
        }
        Ok(output.stdout)
    }
}

/// Masked form the client prints for an access key: first 6 + "..." + last 4.
pub fn mask_access_key(key: &str) -> String {
    if key.len() <= 10 {
        return key.to_string();
    }
    format!("{}...{}", &key[..6], &key[key.len() - 4..])
}

/// S3 bucket names: 3-63 chars of a-z, 0-9, '.', '-', starting and ending
/// with a letter or digit.
pub fn validate_bucket_name(name: &str) -> Result<()> {
    let ok = (3..=63).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
        && name
            .chars()
            .next()
            .map(|c| c.is_ascii_alphanumeric())
            .unwrap_or(false)
        && name
            .chars()
            .last()
            .map(|c| c.is_ascii_alphanumeric())
            .unwrap_or(false)
        && !name.contains("..");
    if ok {
        Ok(())
    } else {
        Err(Error::Validation(format!(
            "Invalid bucket name '{}': use 3-63 lowercase letters, digits, '-' or '.', starting and ending with a letter or digit",
            name
        )))
    }
}

/// Object keys: non-empty, no leading '/', no '..' path segments.
pub fn validate_object_key(key: &str) -> Result<()> {
    if key.is_empty()
        || key.len() > 1024
        || key.starts_with('/')
        || key.split('/').any(|s| s == ".." || s == ".")
        || key.contains('\0')
    {
        return Err(Error::Validation(format!("Invalid object key '{}'", key)));
    }
    Ok(())
}

/// S3 user names (MinIO access keys of users): 3-128 safe characters.
pub fn validate_user_name(name: &str) -> Result<()> {
    if (3..=128).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.@+=,".contains(c))
    {
        Ok(())
    } else {
        Err(Error::Validation(format!(
            "Invalid user name '{}': 3-128 letters, digits or -_.@+=,",
            name
        )))
    }
}

/// The client's own error text from its JSON (or plain) output.
fn mc_error_message(stdout: &str, stderr: &str) -> String {
    for line in stdout.lines().chain(stderr.lines()) {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(line) {
            let msg = json.pointer("/error/message").and_then(|m| m.as_str());
            let cause = json
                .pointer("/error/cause/message")
                .and_then(|m| m.as_str())
                .filter(|m| !m.is_empty());
            if let Some(m) = msg {
                return match cause {
                    Some(c) => format!("{} {}", m, c),
                    None => m.to_string(),
                };
            }
        }
    }
    let text = stderr.trim();
    if text.is_empty() {
        stdout.trim().to_string()
    } else {
        text.to_string()
    }
}

/// `<server> --version` -> "RELEASE...."
async fn get_version(bin: &str) -> Result<String> {
    let output = Command::new(bin)
        .arg("--version")
        .output()
        .await
        .map_err(|e| Error::Internal(format!("Failed to get server version: {}", e)))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .split_whitespace()
        .find(|w| w.starts_with("RELEASE."))
        .map(|v| v.to_string())
        .ok_or_else(|| Error::Internal("Failed to parse server version".to_string()))
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
<LifecycleConfiguration xmlns="http://s3.amazonaws.com/doc/2006-03-01/">"#
        .to_string();

    for rule in rules {
        xml.push_str(&format!(
            r#"
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
            xml.push_str(&format!(
                r#"
    <Expiration>
      <Days>{}</Days>
    </Expiration>"#,
                days
            ));
        }

        if let Some(days) = rule.noncurrent_expiration_days {
            xml.push_str(&format!(
                r#"
    <NoncurrentVersionExpiration>
      <NoncurrentDays>{}</NoncurrentDays>
    </NoncurrentVersionExpiration>"#,
                days
            ));
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
        // no well-known default credentials
        assert_ne!(config.root_password, "minioadmin");
        assert_eq!(config.root_password.len(), 40);
        assert_ne!(
            S3GatewayConfig::default().root_password,
            config.root_password
        );
        assert!(!config.enabled);
    }

    #[test]
    fn test_server_env() {
        let manager = S3GatewayManager::new();
        let env = manager.server_env();
        let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(get("MINIO_ROOT_USER").as_deref(), Some("horcrux-admin"));
        assert_eq!(
            get("MINIO_ROOT_PASSWORD"),
            Some(manager.get_config().root_password.clone())
        );
        assert_eq!(get("MINIO_SITE_REGION").as_deref(), Some("us-east-1"));
    }

    #[test]
    fn test_mask_and_validation() {
        assert_eq!(mask_access_key("AKIATEST000000000001"), "AKIATE...0001");
        assert!(validate_bucket_name("my-bucket.1").is_ok());
        for bad in ["ab", "My-Bucket", "-abc", "abc-", "a..b", "a_b_c"] {
            assert!(validate_bucket_name(bad).is_err(), "{bad}");
        }
        assert!(validate_object_key("dir/file.txt").is_ok());
        for bad in ["", "/abs", "a/../b", "./x"] {
            assert!(validate_object_key(bad).is_err(), "{bad}");
        }
        assert!(validate_user_name("alice").is_ok());
        assert!(validate_user_name("a b").is_err());
    }

    #[test]
    fn test_mc_error_message() {
        let out = r#"{"status":"error","error":{"message":"Unable to make bucket `hx/b1`.","cause":{"message":"Bucket name cannot be shorter than 3 characters","error":{}},"type":"error"}}"#;
        assert_eq!(
            mc_error_message(out, ""),
            "Unable to make bucket `hx/b1`. Bucket name cannot be shorter than 3 characters"
        );
        assert_eq!(mc_error_message("", "boom\n"), "boom");
    }

    #[test]
    fn test_lifecycle_xml() {
        let rules = vec![LifecycleRule {
            id: "delete-old".to_string(),
            prefix: "logs/".to_string(),
            enabled: true,
            expiration_days: Some(30),
            transition_days: None,
            transition_storage_class: None,
            delete_markers: false,
            noncurrent_expiration_days: Some(7),
            tags: HashMap::new(),
        }];

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
