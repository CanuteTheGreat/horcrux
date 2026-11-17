//! Configuration management for Horcrux API
//!
//! This module provides a centralized configuration system that loads settings from:
//! 1. Environment variables (highest priority)
//! 2. Configuration file (TOML format)
//! 3. Default values (lowest priority)

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Main configuration struct for Horcrux
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HorcruxConfig {
    /// Server configuration
    pub server: ServerConfig,
    /// Storage paths configuration
    pub paths: PathsConfig,
    /// Database configuration
    pub database: DatabaseConfig,
    /// TLS/SSL configuration
    pub tls: TlsConfig,
    /// QEMU configuration
    pub qemu: QemuConfig,
    /// Logging configuration
    pub logging: LoggingConfig,
    /// CNI (Container Networking Interface) configuration
    pub cni: CniConfig,
    /// Container runtimes (image catalogs)
    #[serde(default)]
    pub containers: ContainersConfig,
    /// High availability timings
    #[serde(default)]
    pub ha: HaConfig,
    /// Cluster ports
    #[serde(default)]
    pub cluster: ClusterConfig,
    /// Authentication settings
    #[serde(default)]
    pub auth: AuthConfig,
}

/// Authentication settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// Allow anyone to create an account at `/api/auth/register` (off by
    /// default; such accounts start without any permissions). Env:
    /// `HORCRUX_ALLOW_REGISTRATION`.
    #[serde(default)]
    pub allow_registration: bool,
    /// System groups whose members are full administrators of the web UI/API
    /// (users sign in with their host account through PAM).
    /// Env: `HORCRUX_ADMIN_GROUPS` (comma separated).
    #[serde(default = "default_admin_groups")]
    pub admin_groups: Vec<String>,
    /// System groups mapped to the `VMAdmin` role (manage guests).
    #[serde(default)]
    pub operator_groups: Vec<String>,
    /// System groups mapped to the read-only `Auditor` role.
    #[serde(default)]
    pub readonly_groups: Vec<String>,
    /// Let root (uid 0) sign in over the web. Off by default.
    /// Env: `HORCRUX_ALLOW_ROOT_LOGIN`.
    #[serde(default)]
    pub allow_root: bool,
    /// Keep accepting the old Horcrux-only accounts (the pre-PAM user store)
    /// for this release so nobody is locked out during the upgrade. The
    /// legacy `admin` account is disabled automatically once a PAM
    /// administrator has signed in. Set false to refuse them now.
    #[serde(default = "yes_true")]
    pub legacy_local_login: bool,
    /// Lifetime of a web/API login, in seconds (300 .. 30 days; the login
    /// token's `exp` and the stored session agree). Env:
    /// `HORCRUX_SESSION_TTL_SECS`.
    #[serde(default = "default_session_ttl_secs")]
    pub session_ttl_secs: u64,
    /// Lifetime of the restricted "set up two-factor first" login, in
    /// seconds (60 .. 3600). Env: `HORCRUX_TFA_SETUP_TTL_SECS`.
    #[serde(default = "default_tfa_setup_ttl_secs")]
    pub tfa_setup_ttl_secs: u64,
}

/// Bounds of [`AuthConfig::session_ttl_secs`].
pub const SESSION_TTL_RANGE: std::ops::RangeInclusive<u64> = 300..=30 * 86_400;
/// Bounds of [`AuthConfig::tfa_setup_ttl_secs`].
pub const TFA_SETUP_TTL_RANGE: std::ops::RangeInclusive<u64> = 60..=3_600;

fn default_session_ttl_secs() -> u64 {
    86_400
}

fn default_tfa_setup_ttl_secs() -> u64 {
    900
}

impl AuthConfig {
    /// Refuse out-of-range values (a typo must not give week-long or
    /// instantly-expiring logins silently).
    pub fn validate(&self) -> Result<(), String> {
        if !SESSION_TTL_RANGE.contains(&self.session_ttl_secs) {
            return Err(format!(
                "[auth] session_ttl_secs = {} is outside {}..={} seconds",
                self.session_ttl_secs,
                SESSION_TTL_RANGE.start(),
                SESSION_TTL_RANGE.end()
            ));
        }
        if !TFA_SETUP_TTL_RANGE.contains(&self.tfa_setup_ttl_secs) {
            return Err(format!(
                "[auth] tfa_setup_ttl_secs = {} is outside {}..={} seconds",
                self.tfa_setup_ttl_secs,
                TFA_SETUP_TTL_RANGE.start(),
                TFA_SETUP_TTL_RANGE.end()
            ));
        }
        Ok(())
    }
}

fn default_admin_groups() -> Vec<String> {
    vec!["horcrux-admin".into(), "wheel".into()]
}

fn yes_true() -> bool {
    true
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            allow_registration: false,
            admin_groups: default_admin_groups(),
            operator_groups: vec![],
            readonly_groups: vec![],
            allow_root: false,
            legacy_local_login: true,
            session_ttl_secs: default_session_ttl_secs(),
            tfa_setup_ttl_secs: default_tfa_setup_ttl_secs(),
        }
    }
}

/// Server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Host address to bind to
    pub host: String,
    /// Port to listen on
    pub port: u16,
    /// Serve HTTPS (default). Turn off only behind a TLS-terminating proxy.
    pub tls_enabled: bool,
    /// Extra plain-HTTP port that redirects to HTTPS and answers ACME
    /// HTTP-01 challenges (80 for Let's Encrypt). Env: `HORCRUX_HTTP_REDIRECT_PORT`.
    #[serde(default)]
    pub http_redirect_port: Option<u16>,
}

/// Storage paths configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathsConfig {
    /// Base directory for Horcrux data
    pub data_dir: PathBuf,
    /// Directory for VM disk images
    pub vm_storage: PathBuf,
    /// Directory for snapshots
    pub snapshots: PathBuf,
    /// Directory for templates
    pub templates: PathBuf,
    /// Directory for cloud-init data
    pub cloudinit: PathBuf,
    /// Directory for restore operations
    pub restore: PathBuf,
    /// Directory for backups
    pub backups: PathBuf,
}

/// Database configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    /// Database URL (e.g., "sqlite:///var/lib/horcrux/horcrux.db")
    pub url: String,
    /// Maximum number of connections in the pool
    pub max_connections: u32,
}

/// TLS/SSL configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsConfig {
    /// Path to SSL certificate file
    pub cert_path: PathBuf,
    /// Path to SSL private key file
    pub key_path: PathBuf,
    /// SSL directory for storing certificates
    pub ssl_dir: PathBuf,
    /// ACME directory a new certificate order uses unless its configuration names another
    /// (https). Env: `HORCRUX_ACME_DIRECTORY`.
    #[serde(default = "default_acme_directory")]
    pub acme_directory: String,
    /// Certificate authorities offered on the TLS page (any ACME CA works; the list only
    /// feeds the picker). Replace it to offer an internal CA.
    #[serde(default = "default_acme_directories")]
    pub acme_directories: Vec<AcmeDirectory>,
}

/// One selectable ACME certificate authority.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AcmeDirectory {
    pub name: String,
    /// ACME directory URL (https).
    pub url: String,
}

fn default_acme_directory() -> String {
    default_acme_directories()
        .into_iter()
        .next()
        .map(|d| d.url)
        .unwrap_or_default()
}

fn default_acme_directories() -> Vec<AcmeDirectory> {
    [
        (
            "Let's Encrypt",
            "https://acme-v02.api.letsencrypt.org/directory",
        ),
        (
            "Let's Encrypt (staging, untrusted test certificates)",
            "https://acme-staging-v02.api.letsencrypt.org/directory",
        ),
        ("ZeroSSL", "https://acme.zerossl.com/v2/DV90"),
        (
            "Google Trust Services",
            "https://dv.acme-v02.api.pki.goog/directory",
        ),
    ]
    .into_iter()
    .map(|(n, u)| AcmeDirectory {
        name: n.into(),
        url: u.into(),
    })
    .collect()
}

impl TlsConfig {
    pub fn validate(&self) -> Result<(), String> {
        let https = |u: &str| u.starts_with("https://") && !u.chars().any(|c| c.is_whitespace());
        if !https(&self.acme_directory) {
            return Err(format!(
                "[tls] acme_directory = {:?} is not an https URL",
                self.acme_directory
            ));
        }
        for d in &self.acme_directories {
            if d.name.trim().is_empty() || !https(&d.url) {
                return Err(format!(
                    "[tls] acme_directories entry {:?} needs a name and an https URL",
                    d.name
                ));
            }
        }
        Ok(())
    }
}

/// QEMU configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QemuConfig {
    /// Pattern for QMP socket path (use {vm_id} as placeholder)
    pub qmp_socket_pattern: String,
    /// Pattern for monitor socket path (use {vm_id} as placeholder)
    pub monitor_socket_pattern: String,
    /// Pattern for serial socket path (use {vm_id} as placeholder)
    pub serial_socket_pattern: String,
    /// Directory for QEMU runtime files
    pub run_dir: PathBuf,
}

/// Logging configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// Log level (trace, debug, info, warn, error)
    pub level: String,
    /// Directory for log files
    pub log_dir: PathBuf,
    /// Path to audit log file
    pub audit_log: PathBuf,
    /// Enable file logging
    pub file_logging_enabled: bool,
}

/// CNI (Container Networking Interface) configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CniConfig {
    /// Directory containing CNI binaries
    pub bin_dir: PathBuf,
    /// Directory containing CNI configuration files
    pub conf_dir: PathBuf,
    /// Enable CNI features
    pub enabled: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 8006,
            tls_enabled: true,
            http_redirect_port: None,
        }
    }
}

impl Default for PathsConfig {
    fn default() -> Self {
        let data_dir = PathBuf::from("/var/lib/horcrux");
        Self {
            vm_storage: data_dir.join("vms"),
            snapshots: data_dir.join("snapshots"),
            templates: data_dir.join("templates"),
            cloudinit: data_dir.join("cloudinit"),
            restore: data_dir.join("restore"),
            backups: data_dir.join("backups"),
            data_dir,
        }
    }
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: "sqlite:///var/lib/horcrux/horcrux.db".to_string(),
            max_connections: 10,
        }
    }
}

impl Default for TlsConfig {
    fn default() -> Self {
        let ssl_dir = PathBuf::from("/etc/horcrux/ssl");
        Self {
            cert_path: ssl_dir.join("cert.pem"),
            key_path: ssl_dir.join("key.pem"),
            ssl_dir,
            acme_directory: default_acme_directory(),
            acme_directories: default_acme_directories(),
        }
    }
}

impl Default for QemuConfig {
    fn default() -> Self {
        Self {
            qmp_socket_pattern: "/var/run/qemu/{vm_id}.qmp".to_string(),
            monitor_socket_pattern: "/var/run/qemu-server/{vm_id}.mon".to_string(),
            serial_socket_pattern: "/var/run/qemu-server/{vm_id}.serial".to_string(),
            run_dir: PathBuf::from("/var/run/qemu-server"),
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            log_dir: PathBuf::from("/var/log/horcrux"),
            audit_log: PathBuf::from("/var/log/horcrux/audit.log"),
            file_logging_enabled: true,
        }
    }
}

impl Default for CniConfig {
    fn default() -> Self {
        Self {
            bin_dir: PathBuf::from("/opt/cni/bin"),
            conf_dir: PathBuf::from("/etc/cni/net.d"),
            enabled: true,
        }
    }
}

// ---------------------------------------------------------------------------
// [containers]
// ---------------------------------------------------------------------------

/// Default LXC image server (the images.linuxcontainers.org simplestreams layout).
pub const DEFAULT_LXC_IMAGES_URL: &str = "https://images.linuxcontainers.org";
/// Bounds of [`ContainersConfig::lxc_catalog_ttl_secs`].
pub const LXC_CATALOG_TTL_RANGE: std::ops::RangeInclusive<u64> = 60..=86_400;

/// Container settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainersConfig {
    /// LXC image server the template catalog and downloads come from (any server with
    /// the same `meta/1.0/index-system` layout works: a mirror, an internal cache).
    /// Env: `HORCRUX_LXC_IMAGES_URL`.
    #[serde(default = "default_lxc_images_url")]
    pub lxc_images_url: String,
    /// How long the fetched image catalog is reused before it is fetched again, in
    /// seconds (60 .. 86400). Env: `HORCRUX_LXC_CATALOG_TTL_SECS`.
    #[serde(default = "default_lxc_catalog_ttl_secs")]
    pub lxc_catalog_ttl_secs: u64,
}

fn default_lxc_images_url() -> String {
    DEFAULT_LXC_IMAGES_URL.to_string()
}

fn default_lxc_catalog_ttl_secs() -> u64 {
    1_800
}

impl Default for ContainersConfig {
    fn default() -> Self {
        Self {
            lxc_images_url: default_lxc_images_url(),
            lxc_catalog_ttl_secs: default_lxc_catalog_ttl_secs(),
        }
    }
}

impl ContainersConfig {
    pub fn validate(&self) -> Result<(), String> {
        let u = self.lxc_images_url.trim();
        if !(u.starts_with("https://") || u.starts_with("http://"))
            || u.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(format!(
                "[containers] lxc_images_url = {u:?} is not an http(s) URL"
            ));
        }
        if !LXC_CATALOG_TTL_RANGE.contains(&self.lxc_catalog_ttl_secs) {
            return Err(format!(
                "[containers] lxc_catalog_ttl_secs = {} is outside {}..={} seconds",
                self.lxc_catalog_ttl_secs,
                LXC_CATALOG_TTL_RANGE.start(),
                LXC_CATALOG_TTL_RANGE.end()
            ));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// [cluster]
// ---------------------------------------------------------------------------

/// Ports the cluster code connects to on other nodes. The management (API) port is the
/// node's own `[server] port`: every node of a cluster uses the same one.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClusterConfig {
    /// SSH port of the nodes (migration reachability check, cross-node clone).
    /// Env: `HORCRUX_CLUSTER_SSH_PORT`.
    #[serde(default = "default_ssh_port")]
    pub ssh_port: u16,
    /// Incus/LXD remote API port of the nodes (container moves between hosts).
    /// Env: `HORCRUX_CLUSTER_INCUS_PORT`.
    #[serde(default = "default_incus_port")]
    pub incus_port: u16,
}

fn default_ssh_port() -> u16 {
    22
}

fn default_incus_port() -> u16 {
    8443
}

impl Default for ClusterConfig {
    fn default() -> Self {
        Self {
            ssh_port: default_ssh_port(),
            incus_port: default_incus_port(),
        }
    }
}

impl ClusterConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.ssh_port == 0 || self.incus_port == 0 {
            return Err("[cluster] ssh_port and incus_port must not be 0".into());
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// [ha]
// // ---------------------------------------------------------------------------

/// High-availability timings. Every value is a number of seconds unless noted.
/// The values depend on each other (a node must have killed its guests before the
/// survivors restart them), so [`HaConfig::validate`] checks them together, and the
/// fence agent (a separate process) reads the same section.
///
/// Env overrides keep their historic names (`HORCRUX_HA_TICK_SECS`, ...).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HaConfig {
    /// Period of the HA loop (leader decisions, recovery). Env `HORCRUX_HA_TICK_SECS`.
    #[serde(default = "ha_tick")]
    pub tick_secs: u64,
    /// A node without a majority kills its HA guests after this long without having seen
    /// one. Env `HORCRUX_HA_SELF_FENCE_SECS`.
    #[serde(default = "ha_self_fence")]
    pub self_fence_secs: u64,
    /// Wait before fencing a node that has gone quiet. Env `HORCRUX_HA_FENCE_AFTER_SECS`.
    #[serde(default = "ha_fence_after")]
    pub fence_after_secs: u64,
    /// The fence agent kills the guests when the manager's heartbeat is older than this
    /// (3 or more). Env `HORCRUX_HA_AGENT_FENCE_SECS`.
    #[serde(default = "ha_agent_fence")]
    pub agent_fence_secs: u64,
    /// Time a manager that is enabled but not running is given to start before the node
    /// is declared manager-less. Env `HORCRUX_HA_AGENT_BOOT_SETTLE_SECS`.
    #[serde(default = "ha_agent_boot_settle")]
    pub agent_boot_settle_secs: u64,
    /// TCP port of the fence agent's attestation endpoint. Env `HORCRUX_HA_FENCE_PORT`.
    #[serde(default = "ha_fence_port")]
    pub fence_port: u16,
    /// Timeout of one peer-to-peer HA call. Env `HORCRUX_HA_SYNC_TIMEOUT_SECS`.
    #[serde(default = "ha_sync_timeout")]
    pub sync_timeout_secs: u64,
    /// Extra time the leader waits, beyond the self-fence time, before it restarts the
    /// guests of an unfenced node. Unset = the proven floor for the timings in effect.
    /// Env `HORCRUX_HA_RECOVERY_MARGIN_SECS` (a lab shortcut: below the floor it is only
    /// warned about).
    #[serde(default)]
    pub recovery_margin_secs: Option<u64>,
    /// Period of the self-fence watchdog task.
    #[serde(default = "ha_watchdog")]
    pub watchdog_secs: u64,
    /// Clock-rate drift allowance between two nodes over one fencing window.
    #[serde(default = "ha_drift")]
    pub drift_secs: u64,
    /// Longest wait for SIGKILLed QEMUs to be gone before the node is reset.
    #[serde(default = "ha_kill_wait")]
    pub fence_kill_wait_secs: u64,
    /// Time a node reset (sysrq `b`) needs to stop every guest.
    #[serde(default = "ha_reset")]
    pub fence_reset_secs: u64,
    /// The manager must have been silent this long before an attestation is acted on.
    #[serde(default = "ha_attest_min_silent")]
    pub attest_min_silent_secs: u64,
    /// A node whose agent keeps saying "not yet" this long is held like any unproven node.
    #[serde(default = "ha_attest_not_yet_max")]
    pub attest_not_yet_max_secs: u64,
    /// Set by the env override of `recovery_margin_secs` (not a config key).
    #[serde(skip)]
    pub margin_from_env: bool,
}

fn ha_tick() -> u64 {
    3
}
fn ha_self_fence() -> u64 {
    10
}
fn ha_fence_after() -> u64 {
    10
}
fn ha_agent_fence() -> u64 {
    15
}
fn ha_agent_boot_settle() -> u64 {
    20
}
fn ha_fence_port() -> u16 {
    8026
}
fn ha_sync_timeout() -> u64 {
    5
}
fn ha_watchdog() -> u64 {
    1
}
fn ha_drift() -> u64 {
    2
}
fn ha_kill_wait() -> u64 {
    2
}
fn ha_reset() -> u64 {
    1
}
fn ha_attest_min_silent() -> u64 {
    3
}
fn ha_attest_not_yet_max() -> u64 {
    180
}

impl Default for HaConfig {
    fn default() -> Self {
        Self {
            tick_secs: ha_tick(),
            self_fence_secs: ha_self_fence(),
            fence_after_secs: ha_fence_after(),
            agent_fence_secs: ha_agent_fence(),
            agent_boot_settle_secs: ha_agent_boot_settle(),
            fence_port: ha_fence_port(),
            sync_timeout_secs: ha_sync_timeout(),
            recovery_margin_secs: None,
            watchdog_secs: ha_watchdog(),
            drift_secs: ha_drift(),
            fence_kill_wait_secs: ha_kill_wait(),
            fence_reset_secs: ha_reset(),
            attest_min_silent_secs: ha_attest_min_silent(),
            attest_not_yet_max_secs: ha_attest_not_yet_max(),
            margin_from_env: false,
        }
    }
}

impl HaConfig {
    /// The HA section in effect now (config file + environment). Read at each use, so a
    /// tick always sees the current file; the file is tiny.
    pub fn current() -> Self {
        let ha = HorcruxConfig::load().ha;
        match ha.validate() {
            Ok(()) => ha,
            Err(e) => {
                // an inconsistent section must never weaken fencing: use the proven defaults
                static WARNED: std::sync::Once = std::sync::Once::new();
                WARNED.call_once(|| eprintln!("horcrux: {e}; using the default HA timings"));
                HaConfig::default()
            }
        }
    }

    /// Hard upper bound from "the fence fires" to "no guest of this node can run":
    /// SIGKILL wait + node reset. The recovery margin and the agent settle time derive
    /// from this one number.
    pub fn kill_budget_secs(&self) -> u64 {
        self.fence_kill_wait_secs + self.fence_reset_secs
    }

    /// Longest time between the manager's last answer and the guests being gone, beyond
    /// `agent_fence_secs`: one heartbeat period, one agent poll period, the kill budget
    /// and clock drift.
    pub fn agent_settle_secs(&self) -> u64 {
        1 + 1 + self.kill_budget_secs() + self.drift_secs
    }

    /// The part of the smallest safe recovery margin that depends only on this section
    /// (the cluster monitor's poll and fetch times are added by the HA loop).
    pub fn margin_floor_part(&self) -> u64 {
        2 * self.watchdog_secs + self.kill_budget_secs() + self.drift_secs
    }

    /// Check the values and their relations.
    pub fn validate(&self) -> Result<(), String> {
        let rng = |name: &str, v: u64, lo: u64, hi: u64| -> Result<(), String> {
            if (lo..=hi).contains(&v) {
                Ok(())
            } else {
                Err(format!("[ha] {name} = {v} is outside {lo}..={hi} seconds"))
            }
        };
        rng("tick_secs", self.tick_secs, 1, 300)?;
        rng("self_fence_secs", self.self_fence_secs, 3, 600)?;
        rng("fence_after_secs", self.fence_after_secs, 1, 600)?;
        rng("agent_fence_secs", self.agent_fence_secs, 3, 3_600)?;
        rng(
            "agent_boot_settle_secs",
            self.agent_boot_settle_secs,
            0,
            600,
        )?;
        rng("sync_timeout_secs", self.sync_timeout_secs, 1, 120)?;
        rng("watchdog_secs", self.watchdog_secs, 1, 10)?;
        rng("drift_secs", self.drift_secs, 0, 30)?;
        rng("fence_kill_wait_secs", self.fence_kill_wait_secs, 1, 30)?;
        rng("fence_reset_secs", self.fence_reset_secs, 1, 10)?;
        rng("attest_min_silent_secs", self.attest_min_silent_secs, 1, 60)?;
        rng(
            "attest_not_yet_max_secs",
            self.attest_not_yet_max_secs,
            10,
            86_400,
        )?;
        if self.fence_port == 0 {
            return Err("[ha] fence_port must not be 0".into());
        }
        // relations
        if self.watchdog_secs >= self.self_fence_secs {
            return Err(format!(
                "[ha] watchdog_secs ({}) must be shorter than self_fence_secs ({}): the \
                 watchdog could not notice the expiry in time",
                self.watchdog_secs, self.self_fence_secs
            ));
        }
        if self.kill_budget_secs() >= self.self_fence_secs {
            return Err(format!(
                "[ha] the kill budget fence_kill_wait_secs + fence_reset_secs ({}) must be \
                 shorter than self_fence_secs ({})",
                self.kill_budget_secs(),
                self.self_fence_secs
            ));
        }
        if self.attest_min_silent_secs >= self.attest_not_yet_max_secs {
            return Err(format!(
                "[ha] attest_min_silent_secs ({}) must be shorter than attest_not_yet_max_secs ({})",
                self.attest_min_silent_secs, self.attest_not_yet_max_secs
            ));
        }
        if self.tick_secs > self.self_fence_secs {
            return Err(format!(
                "[ha] tick_secs ({}) must not exceed self_fence_secs ({})",
                self.tick_secs, self.self_fence_secs
            ));
        }
        // an explicit margin below what this section alone needs would let the survivors
        // restart a guest the isolated node has not killed yet (split brain); a value from
        // the environment is the lab shortcut and is only warned about by the HA loop
        if let Some(m) = self.recovery_margin_secs {
            if !self.margin_from_env && m < self.margin_floor_part() {
                return Err(format!(
                    "[ha] recovery_margin_secs = {m} is below {} (2 x watchdog_secs + kill budget \
                     + drift_secs): the survivors could restart a guest before the isolated node \
                     has killed its copy",
                    self.margin_floor_part()
                ));
            }
        }
        Ok(())
    }
}

impl HorcruxConfig {
    /// Load configuration from environment variables and optional config file
    pub fn load() -> Self {
        let mut config = Self::default();

        // Try to load from config file first
        if let Some(config_path) = Self::find_config_file() {
            if let Ok(file_config) = Self::load_from_file(&config_path) {
                config = file_config;
            }
        }

        // Override with environment variables
        config.apply_env_overrides();

        config
    }

    /// Load configuration from a specific file path
    pub fn load_from_file(path: &PathBuf) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| ConfigError::FileRead(path.clone(), e.to_string()))?;

        toml::from_str(&content).map_err(|e| ConfigError::Parse(e.to_string()))
    }

    /// Find configuration file in standard locations
    fn find_config_file() -> Option<PathBuf> {
        let paths = [
            // Environment variable override
            std::env::var("HORCRUX_CONFIG").ok().map(PathBuf::from),
            // Standard locations
            Some(PathBuf::from("/etc/horcrux/config.toml")),
            Some(PathBuf::from("./config.toml")),
            Some(PathBuf::from("./horcrux.toml")),
        ];

        paths.into_iter().flatten().find(|p| p.exists())
    }

    /// Apply environment variable overrides
    fn apply_env_overrides(&mut self) {
        // Server
        if let Ok(host) = std::env::var("HORCRUX_HOST") {
            self.server.host = host;
        }
        if let Ok(port) = std::env::var("HORCRUX_PORT") {
            if let Ok(port) = port.parse() {
                self.server.port = port;
            }
        }
        if let Ok(tls) = std::env::var("HORCRUX_TLS_ENABLED") {
            self.server.tls_enabled = !matches!(
                tls.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            );
        }
        if let Ok(port) = std::env::var("HORCRUX_HTTP_REDIRECT_PORT") {
            self.server.http_redirect_port = port.trim().parse().ok().filter(|p| *p != 0);
        }
        if let Ok(v) = std::env::var("HORCRUX_ADMIN_GROUPS") {
            self.auth.admin_groups = v
                .split(',')
                .map(|g| g.trim().to_string())
                .filter(|g| !g.is_empty())
                .collect();
        }
        if let Ok(v) = std::env::var("HORCRUX_ALLOW_ROOT_LOGIN") {
            self.auth.allow_root = matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            );
        }
        if let Ok(v) = std::env::var("HORCRUX_SESSION_TTL_SECS") {
            if let Ok(n) = v.trim().parse() {
                self.auth.session_ttl_secs = n;
            }
        }
        if let Ok(v) = std::env::var("HORCRUX_TFA_SETUP_TTL_SECS") {
            if let Ok(n) = v.trim().parse() {
                self.auth.tfa_setup_ttl_secs = n;
            }
        }
        if let Ok(v) = std::env::var("HORCRUX_ALLOW_REGISTRATION") {
            self.auth.allow_registration = matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            );
        }

        if let Ok(v) = std::env::var("HORCRUX_ACME_DIRECTORY") {
            self.tls.acme_directory = v;
        }

        // Containers
        if let Ok(v) = std::env::var("HORCRUX_LXC_IMAGES_URL") {
            self.containers.lxc_images_url = v;
        }
        if let Ok(v) = std::env::var("HORCRUX_LXC_CATALOG_TTL_SECS") {
            if let Ok(n) = v.trim().parse() {
                self.containers.lxc_catalog_ttl_secs = n;
            }
        }

        for (name, slot) in [
            ("HORCRUX_CLUSTER_SSH_PORT", &mut self.cluster.ssh_port),
            ("HORCRUX_CLUSTER_INCUS_PORT", &mut self.cluster.incus_port),
        ] {
            if let Some(p) = std::env::var(name)
                .ok()
                .and_then(|v| v.trim().parse::<u16>().ok())
                .filter(|p| *p != 0)
            {
                *slot = p;
            }
        }

        // High availability (historic env names; an unusable value is ignored)
        let env_u64 = |name: &str, min: u64| {
            std::env::var(name)
                .ok()
                .and_then(|v| v.trim().parse::<u64>().ok())
                .filter(|v| *v >= min)
        };
        if let Some(v) = env_u64("HORCRUX_HA_TICK_SECS", 1) {
            self.ha.tick_secs = v;
        }
        if let Some(v) = env_u64("HORCRUX_HA_SELF_FENCE_SECS", 1) {
            self.ha.self_fence_secs = v;
        }
        if let Some(v) = env_u64("HORCRUX_HA_FENCE_AFTER_SECS", 1) {
            self.ha.fence_after_secs = v;
        }
        if let Some(v) = env_u64("HORCRUX_HA_AGENT_FENCE_SECS", 3) {
            self.ha.agent_fence_secs = v;
        }
        if let Some(v) = env_u64("HORCRUX_HA_AGENT_BOOT_SETTLE_SECS", 0) {
            self.ha.agent_boot_settle_secs = v;
        }
        if let Some(v) = env_u64("HORCRUX_HA_SYNC_TIMEOUT_SECS", 1) {
            self.ha.sync_timeout_secs = v;
        }
        if let Some(v) = env_u64("HORCRUX_HA_FENCE_PORT", 1).and_then(|p| u16::try_from(p).ok()) {
            self.ha.fence_port = v;
        }
        if let Some(v) = env_u64("HORCRUX_HA_RECOVERY_MARGIN_SECS", 1) {
            self.ha.recovery_margin_secs = Some(v);
            self.ha.margin_from_env = true;
        }

        // Paths
        if let Ok(data_dir) = std::env::var("HORCRUX_DATA_DIR") {
            let data_dir = PathBuf::from(data_dir);
            self.paths.vm_storage = data_dir.join("vms");
            self.paths.snapshots = data_dir.join("snapshots");
            self.paths.templates = data_dir.join("templates");
            self.paths.cloudinit = data_dir.join("cloudinit");
            self.paths.restore = data_dir.join("restore");
            self.paths.backups = data_dir.join("backups");
            self.paths.data_dir = data_dir;
        }
        if let Ok(path) = std::env::var("HORCRUX_VM_STORAGE") {
            self.paths.vm_storage = PathBuf::from(path);
        }
        if let Ok(path) = std::env::var("HORCRUX_SNAPSHOTS_DIR") {
            self.paths.snapshots = PathBuf::from(path);
        }
        if let Ok(path) = std::env::var("HORCRUX_TEMPLATES_DIR") {
            self.paths.templates = PathBuf::from(path);
        }
        if let Ok(path) = std::env::var("HORCRUX_CLOUDINIT_DIR") {
            self.paths.cloudinit = PathBuf::from(path);
        }

        // Database
        if let Ok(url) = std::env::var("HORCRUX_DATABASE_URL") {
            self.database.url = url;
        }
        if let Ok(max) = std::env::var("HORCRUX_DATABASE_MAX_CONNECTIONS") {
            if let Ok(max) = max.parse() {
                self.database.max_connections = max;
            }
        }

        // TLS
        if let Ok(path) = std::env::var("HORCRUX_TLS_CERT") {
            self.tls.cert_path = PathBuf::from(path);
        }
        if let Ok(path) = std::env::var("HORCRUX_TLS_KEY") {
            self.tls.key_path = PathBuf::from(path);
        }
        if let Ok(path) = std::env::var("HORCRUX_SSL_DIR") {
            self.tls.ssl_dir = PathBuf::from(path);
            if std::env::var("HORCRUX_TLS_CERT").is_err() {
                self.tls.cert_path = self.tls.ssl_dir.join("cert.pem");
            }
            if std::env::var("HORCRUX_TLS_KEY").is_err() {
                self.tls.key_path = self.tls.ssl_dir.join("key.pem");
            }
        }

        // QEMU
        if let Ok(pattern) = std::env::var("HORCRUX_QMP_SOCKET_PATTERN") {
            self.qemu.qmp_socket_pattern = pattern;
        }
        if let Ok(pattern) = std::env::var("HORCRUX_MONITOR_SOCKET_PATTERN") {
            self.qemu.monitor_socket_pattern = pattern;
        }
        if let Ok(pattern) = std::env::var("HORCRUX_SERIAL_SOCKET_PATTERN") {
            self.qemu.serial_socket_pattern = pattern;
        }
        if let Ok(path) = std::env::var("HORCRUX_QEMU_RUN_DIR") {
            self.qemu.run_dir = PathBuf::from(path);
        }

        // Logging
        if let Ok(level) = std::env::var("HORCRUX_LOG_LEVEL") {
            self.logging.level = level;
        }
        if let Ok(path) = std::env::var("HORCRUX_LOG_DIR") {
            let log_dir = PathBuf::from(path);
            self.logging.audit_log = log_dir.join("audit.log");
            self.logging.log_dir = log_dir;
        }
        if let Ok(path) = std::env::var("HORCRUX_AUDIT_LOG") {
            self.logging.audit_log = PathBuf::from(path);
        }
        if let Ok(enabled) = std::env::var("HORCRUX_FILE_LOGGING") {
            self.logging.file_logging_enabled = enabled.parse().unwrap_or(true);
        }

        // CNI
        if let Ok(path) = std::env::var("HORCRUX_CNI_BIN_DIR") {
            self.cni.bin_dir = PathBuf::from(path);
        }
        if let Ok(path) = std::env::var("HORCRUX_CNI_CONF_DIR") {
            self.cni.conf_dir = PathBuf::from(path);
        }
        if let Ok(enabled) = std::env::var("HORCRUX_CNI_ENABLED") {
            self.cni.enabled = enabled.parse().unwrap_or(true);
        }
    }

    /// Generate a sample configuration file
    pub fn generate_sample() -> String {
        let config = Self::default();
        toml::to_string_pretty(&config).unwrap_or_default()
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<(), ConfigError> {
        // Validate port range
        if self.server.port == 0 {
            return Err(ConfigError::Validation("Port cannot be 0".to_string()));
        }

        // Validate database URL
        if self.database.url.is_empty() {
            return Err(ConfigError::Validation(
                "Database URL cannot be empty".to_string(),
            ));
        }

        self.auth.validate().map_err(ConfigError::Validation)?;
        self.containers
            .validate()
            .map_err(ConfigError::Validation)?;
        self.tls.validate().map_err(ConfigError::Validation)?;
        self.ha.validate().map_err(ConfigError::Validation)?;
        self.cluster.validate().map_err(ConfigError::Validation)?;

        // Validate socket patterns contain placeholder
        if !self.qemu.qmp_socket_pattern.contains("{vm_id}") {
            return Err(ConfigError::Validation(
                "QMP socket pattern must contain {vm_id} placeholder".to_string(),
            ));
        }

        Ok(())
    }
}

impl QemuConfig {
    /// Get the QMP socket path for a specific VM
    pub fn get_qmp_socket(&self, vm_id: &str) -> PathBuf {
        PathBuf::from(self.qmp_socket_pattern.replace("{vm_id}", vm_id))
    }

    /// Get the monitor socket path for a specific VM
    pub fn get_monitor_socket(&self, vm_id: &str) -> PathBuf {
        PathBuf::from(self.monitor_socket_pattern.replace("{vm_id}", vm_id))
    }

    /// Get the serial socket path for a specific VM
    pub fn get_serial_socket(&self, vm_id: &str) -> PathBuf {
        PathBuf::from(self.serial_socket_pattern.replace("{vm_id}", vm_id))
    }
}

/// Configuration errors
#[derive(Debug, Clone)]
pub enum ConfigError {
    /// Failed to read configuration file
    FileRead(PathBuf, String),
    /// Failed to parse configuration
    Parse(String),
    /// Configuration validation failed
    Validation(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::FileRead(path, err) => {
                write!(f, "Failed to read config file {:?}: {}", path, err)
            }
            ConfigError::Parse(err) => write!(f, "Failed to parse config: {}", err),
            ConfigError::Validation(err) => write!(f, "Config validation failed: {}", err),
        }
    }
}

impl std::error::Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every key of the node configuration survives a write -> read round
    /// trip (configurable-everything: nothing is lost or silently reset).
    #[test]
    fn every_config_key_round_trips_through_toml() {
        let mut c = HorcruxConfig::default();
        c.auth.session_ttl_secs = 3600;
        c.auth.tfa_setup_ttl_secs = 120;
        c.auth.allow_root = true;
        c.server.port = 9443;
        c.containers.lxc_images_url = "https://mirror.test/images".into();
        c.containers.lxc_catalog_ttl_secs = 600;
        c.tls.acme_directory = "https://ca.test/acme".into();
        c.tls.acme_directories.push(AcmeDirectory {
            name: "Internal CA".into(),
            url: "https://ca.test/acme".into(),
        });
        c.cluster.ssh_port = 2222;
        c.cluster.incus_port = 18443;
        c.ha = HaConfig {
            tick_secs: 2,
            self_fence_secs: 12,
            fence_after_secs: 11,
            agent_fence_secs: 20,
            agent_boot_settle_secs: 30,
            fence_port: 18026,
            sync_timeout_secs: 4,
            recovery_margin_secs: Some(15),
            watchdog_secs: 2,
            drift_secs: 3,
            fence_kill_wait_secs: 3,
            fence_reset_secs: 2,
            attest_min_silent_secs: 4,
            attest_not_yet_max_secs: 200,
            margin_from_env: false,
        };
        c.validate().unwrap();
        let text = toml::to_string_pretty(&c).unwrap();
        let back: HorcruxConfig = toml::from_str(&text).unwrap();
        assert_eq!(
            serde_json::to_value(&c).unwrap(),
            serde_json::to_value(&back).unwrap()
        );
        // and a file that leaves keys out gets the documented defaults
        let without: String = toml::to_string_pretty(&HorcruxConfig::default())
            .unwrap()
            .lines()
            .filter(|l| !l.contains("ttl_secs"))
            .map(|l| format!("{l}\n"))
            .collect();
        let partial: HorcruxConfig = toml::from_str(&without).unwrap();
        assert_eq!(partial.auth.session_ttl_secs, 86_400);
        assert_eq!(partial.auth.tfa_setup_ttl_secs, 900);
    }

    #[test]
    fn ha_defaults_are_consistent_and_relations_are_enforced() {
        HaConfig::default().validate().unwrap();
        let bad = |f: &dyn Fn(&mut HaConfig), needle: &str| {
            let mut h = HaConfig::default();
            f(&mut h);
            let e = h.validate().unwrap_err();
            assert!(e.contains(needle), "{e}");
        };
        // the guests must be dead before the node's own fence time is over
        bad(&|h| h.fence_kill_wait_secs = 12, "kill budget");
        bad(&|h| h.watchdog_secs = 10, "watchdog_secs");
        bad(&|h| h.tick_secs = 11, "tick_secs");
        bad(
            &|h| h.attest_min_silent_secs = 200,
            "attest_min_silent_secs",
        );
        bad(&|h| h.agent_fence_secs = 2, "agent_fence_secs");
        // an explicit margin below the part this section guarantees is refused ...
        bad(
            &|h| h.recovery_margin_secs = Some(3),
            "recovery_margin_secs",
        );
        // ... unless it is the environment's lab shortcut
        let mut h = HaConfig {
            recovery_margin_secs: Some(3),
            margin_from_env: true,
            ..HaConfig::default()
        };
        h.validate().unwrap();
        h.margin_from_env = false;
        h.recovery_margin_secs = Some(h.margin_floor_part());
        h.validate().unwrap();
        assert_eq!(HaConfig::default().kill_budget_secs(), 3);
        assert_eq!(HaConfig::default().agent_settle_secs(), 1 + 1 + 3 + 2);
    }

    #[test]
    fn containers_cluster_and_acme_values_are_validated() {
        let mut c = HorcruxConfig::default();
        c.validate().unwrap();
        c.containers.lxc_images_url = "ftp://x".into();
        assert!(c
            .validate()
            .unwrap_err()
            .to_string()
            .contains("lxc_images_url"));
        c.containers = ContainersConfig::default();
        c.containers.lxc_catalog_ttl_secs = 5;
        assert!(c
            .validate()
            .unwrap_err()
            .to_string()
            .contains("lxc_catalog_ttl_secs"));
        c.containers = ContainersConfig::default();
        c.tls.acme_directory = "http://insecure/dir".into();
        assert!(c
            .validate()
            .unwrap_err()
            .to_string()
            .contains("acme_directory"));
        c.tls = TlsConfig::default();
        c.cluster.ssh_port = 0;
        assert!(c.validate().unwrap_err().to_string().contains("ssh_port"));
        // a config file without the new sections gets today's values
        let old: HorcruxConfig = toml::from_str(
            &toml::to_string_pretty(&HorcruxConfig::default())
                .unwrap()
                .lines()
                .take_while(|l| !l.starts_with("[containers]"))
                .map(|l| format!("{l}\n"))
                .collect::<String>(),
        )
        .unwrap();
        assert_eq!(old.ha, HaConfig::default());
        assert_eq!(old.cluster, ClusterConfig::default());
        assert_eq!(old.containers.lxc_catalog_ttl_secs, 1_800);
    }

    #[test]
    fn shipped_example_config_parses_and_validates() {
        let text = include_str!("../../../horcrux-api/config.example.toml");
        let c: HorcruxConfig = toml::from_str(text).unwrap();
        c.validate().unwrap();
        assert_eq!(
            c.auth.session_ttl_secs,
            AuthConfig::default().session_ttl_secs
        );
    }

    #[test]
    fn login_lifetimes_out_of_range_are_refused() {
        let mut c = HorcruxConfig::default();
        c.auth.session_ttl_secs = 10;
        assert!(c
            .validate()
            .unwrap_err()
            .to_string()
            .contains("session_ttl_secs"));
        c.auth.session_ttl_secs = 3600;
        c.auth.tfa_setup_ttl_secs = 99_999;
        assert!(c
            .validate()
            .unwrap_err()
            .to_string()
            .contains("tfa_setup_ttl_secs"));
    }

    #[test]
    fn test_default_config() {
        let config = HorcruxConfig::default();
        assert_eq!(config.server.port, 8006);
        assert_eq!(config.paths.data_dir, PathBuf::from("/var/lib/horcrux"));
        assert_eq!(config.database.url, "sqlite:///var/lib/horcrux/horcrux.db");
    }

    #[test]
    fn test_qemu_socket_paths() {
        let config = QemuConfig::default();
        assert_eq!(
            config.get_qmp_socket("100"),
            PathBuf::from("/var/run/qemu/100.qmp")
        );
        assert_eq!(
            config.get_monitor_socket("100"),
            PathBuf::from("/var/run/qemu-server/100.mon")
        );
        assert_eq!(
            config.get_serial_socket("100"),
            PathBuf::from("/var/run/qemu-server/100.serial")
        );
    }

    #[test]
    fn test_config_validation() {
        let config = HorcruxConfig::default();
        assert!(config.validate().is_ok());

        let mut invalid_config = HorcruxConfig::default();
        invalid_config.server.port = 0;
        assert!(invalid_config.validate().is_err());

        let mut invalid_qemu = HorcruxConfig::default();
        invalid_qemu.qemu.qmp_socket_pattern = "/no/placeholder.sock".to_string();
        assert!(invalid_qemu.validate().is_err());
    }

    #[test]
    fn test_generate_sample_config() {
        let sample = HorcruxConfig::generate_sample();
        assert!(sample.contains("[server]"));
        assert!(sample.contains("[paths]"));
        assert!(sample.contains("[database]"));
        assert!(sample.contains("[tls]"));
        assert!(sample.contains("[qemu]"));
        assert!(sample.contains("[logging]"));
        assert!(sample.contains("[cni]"));
    }

    #[test]
    fn test_paths_config_default() {
        let paths = PathsConfig::default();
        assert_eq!(paths.vm_storage, PathBuf::from("/var/lib/horcrux/vms"));
        assert_eq!(paths.snapshots, PathBuf::from("/var/lib/horcrux/snapshots"));
        assert_eq!(paths.templates, PathBuf::from("/var/lib/horcrux/templates"));
        assert_eq!(paths.cloudinit, PathBuf::from("/var/lib/horcrux/cloudinit"));
    }

    #[test]
    fn test_env_override_data_dir() {
        // This test would need to set env vars, which could affect other tests
        // In a real scenario, you'd use a test framework that isolates env vars
        let config = HorcruxConfig::default();
        assert_eq!(config.paths.data_dir, PathBuf::from("/var/lib/horcrux"));
    }
}
