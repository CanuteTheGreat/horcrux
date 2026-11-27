//! Built-in security tooling (docs/design/security-suite.md): vulnerability
//! scanning of container images and container root file systems, and the
//! host hardening baseline with its compliance report.
//!
//! Types here are shared by the API and the web UI. Every operational value
//! is a setting with a documented default (`[security]` node settings,
//! `PUT /api/security/settings`, declarative `security:` section).
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Severity scale used by both scanners (Grype and Trivy map onto it).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Unknown,
    Negligible,
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    pub const ALL: [Severity; 6] = [
        Severity::Critical,
        Severity::High,
        Severity::Medium,
        Severity::Low,
        Severity::Negligible,
        Severity::Unknown,
    ];

    /// Parse a scanner's severity word (case-insensitive).
    pub fn parse(s: &str) -> Severity {
        match s.trim().to_ascii_lowercase().as_str() {
            "critical" => Severity::Critical,
            "high" => Severity::High,
            "medium" | "moderate" => Severity::Medium,
            "low" => Severity::Low,
            "negligible" | "none" => Severity::Negligible,
            _ => Severity::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Critical => "critical",
            Severity::High => "high",
            Severity::Medium => "medium",
            Severity::Low => "low",
            Severity::Negligible => "negligible",
            Severity::Unknown => "unknown",
        }
    }
}

/// Counts per severity.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SeverityCounts {
    pub critical: u32,
    pub high: u32,
    pub medium: u32,
    pub low: u32,
    pub negligible: u32,
    pub unknown: u32,
    /// Findings for which the scanner knows a fixed version.
    pub fixable: u32,
}

impl SeverityCounts {
    pub fn add(&mut self, s: Severity, fixable: bool) {
        match s {
            Severity::Critical => self.critical += 1,
            Severity::High => self.high += 1,
            Severity::Medium => self.medium += 1,
            Severity::Low => self.low += 1,
            Severity::Negligible => self.negligible += 1,
            Severity::Unknown => self.unknown += 1,
        }
        if fixable {
            self.fixable += 1;
        }
    }

    pub fn get(&self, s: Severity) -> u32 {
        match s {
            Severity::Critical => self.critical,
            Severity::High => self.high,
            Severity::Medium => self.medium,
            Severity::Low => self.low,
            Severity::Negligible => self.negligible,
            Severity::Unknown => self.unknown,
        }
    }

    pub fn total(&self) -> u32 {
        Severity::ALL.iter().map(|s| self.get(*s)).sum()
    }

    /// Findings at or above `min`.
    pub fn at_least(&self, min: Severity) -> u32 {
        Severity::ALL
            .iter()
            .filter(|s| **s >= min)
            .map(|s| self.get(*s))
            .sum()
    }
}

/// What a scan looks at.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum ScanTargetKind {
    /// A Podman image (`podman:<ref>`).
    Podman,
    /// A Docker image (`docker:<ref>`).
    Docker,
    /// An LXC container's root file system.
    Lxc,
    /// An Incus system container's root file system.
    Incus,
}

impl ScanTargetKind {
    pub const ALL: [ScanTargetKind; 4] = [
        ScanTargetKind::Podman,
        ScanTargetKind::Docker,
        ScanTargetKind::Lxc,
        ScanTargetKind::Incus,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ScanTargetKind::Podman => "podman",
            ScanTargetKind::Docker => "docker",
            ScanTargetKind::Lxc => "lxc",
            ScanTargetKind::Incus => "incus",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ScanTargetKind::Podman => "Podman image",
            ScanTargetKind::Docker => "Docker image",
            ScanTargetKind::Lxc => "LXC container",
            ScanTargetKind::Incus => "Incus container",
        }
    }

    /// Image kinds name an image reference; the others name a container.
    pub fn is_image(self) -> bool {
        matches!(self, ScanTargetKind::Podman | ScanTargetKind::Docker)
    }
}

/// Something on this node that can be scanned.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScanTarget {
    pub kind: ScanTargetKind,
    /// Image reference (`docker.io/library/nginx:1.27`) or container name.
    pub name: String,
    /// Image id or container state, for display.
    #[serde(default)]
    pub detail: String,
    /// Newest finished scan of this target, if any.
    #[serde(default)]
    pub last_scan: Option<ScanSummary>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ScanStatus {
    #[default]
    Queued,
    Running,
    Done,
    Failed,
}

/// One vulnerability in one package.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VulnFinding {
    /// CVE-2024-1234, GHSA-..., ALPINE-...
    pub id: String,
    pub severity: Severity,
    pub package: String,
    pub installed: String,
    /// Versions that fix it; empty = no fix known.
    #[serde(default)]
    pub fixed_in: Vec<String>,
    /// Package type (apk, deb, rpm, python, go-module, ...).
    #[serde(default)]
    pub pkg_type: String,
    #[serde(default)]
    pub title: String,
    /// Advisory link.
    #[serde(default)]
    pub url: String,
}

/// A scan without its findings (lists).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ScanSummary {
    pub id: String,
    pub kind: Option<ScanTargetKind>,
    pub target: String,
    pub scanner: String,
    #[serde(default)]
    pub scanner_version: String,
    pub status: ScanStatus,
    /// Unix seconds.
    pub created: i64,
    #[serde(default)]
    pub finished: Option<i64>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub counts: SeverityCounts,
    /// Who started it (`scheduler` for the periodic scan).
    #[serde(default)]
    pub requested_by: String,
}

/// A scan with its findings (detail view).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ScanResult {
    #[serde(flatten)]
    pub summary: ScanSummary,
    #[serde(default)]
    pub findings: Vec<VulnFinding>,
}

/// `POST /api/security/scans`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScanRequest {
    pub kind: ScanTargetKind,
    pub target: String,
}

/// Vulnerability scanner settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct VulnScanSettings {
    /// Scanning on/off for this node (API and schedule).
    pub enabled: bool,
    /// `grype` (Anchore, Apache-2.0) or `trivy` (Aqua, Apache-2.0).
    pub scanner: String,
    /// Scanner program; a bare name is looked up in PATH.
    pub scanner_path: String,
    /// Vulnerability database location for air-gapped nodes (Grype:
    /// `GRYPE_DB_UPDATE_URL` listing URL, Trivy: `--db-repository`). Empty =
    /// the scanner's own default.
    pub db_url: String,
    /// Scanner database/cache directory. Empty = `<data_dir>/security/scanner-db`.
    pub db_dir: String,
    /// Refresh the database before a scan (off for nodes without internet
    /// whose database is copied in).
    pub update_db: bool,
    /// Hours between scheduled scans of every target in `schedule_kinds`;
    /// 0 = only on demand.
    pub schedule_hours: u32,
    /// Target kinds the schedule scans.
    pub schedule_kinds: Vec<ScanTargetKind>,
    /// Scans that may run at the same time (each is CPU and IO heavy).
    pub max_concurrent: u32,
    /// CPU niceness of the scanner process (0-19); IO runs idle-class.
    pub nice: u8,
    /// A scan that runs longer is killed.
    pub timeout_secs: u32,
    /// Finished scans kept (oldest removed first).
    pub keep_scans: u32,
    /// Leave out findings without a known fix.
    pub only_fixed: bool,
    /// A finished scan with findings at or above this severity sends a
    /// `security` notification; `none` = never.
    pub notify_min_severity: String,
    /// LXC container directory (`lxc.lxcpath`).
    pub lxc_path: String,
    /// Incus data directory.
    pub incus_path: String,
}

impl Default for VulnScanSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            scanner: "grype".into(),
            scanner_path: String::new(),
            db_url: String::new(),
            db_dir: String::new(),
            update_db: true,
            schedule_hours: 24,
            schedule_kinds: ScanTargetKind::ALL.to_vec(),
            max_concurrent: 1,
            nice: 15,
            timeout_secs: 1800,
            keep_scans: 200,
            only_fixed: false,
            notify_min_severity: "critical".into(),
            lxc_path: "/var/lib/lxc".into(),
            incus_path: "/var/lib/incus".into(),
        }
    }
}

pub const SCANNERS: &[&str] = &["grype", "trivy"];

impl VulnScanSettings {
    /// Program to run: `scanner_path`, else the scanner's own name.
    pub fn program(&self) -> String {
        if self.scanner_path.trim().is_empty() {
            self.scanner.clone()
        } else {
            self.scanner_path.trim().to_string()
        }
    }

    /// `None` when notifications are off.
    pub fn notify_threshold(&self) -> Option<Severity> {
        match self
            .notify_min_severity
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "" | "none" | "off" => None,
            s => Some(Severity::parse(s)),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if !SCANNERS.contains(&self.scanner.as_str()) {
            return Err(format!(
                "vuln_scan.scanner: {:?} is not one of {}",
                self.scanner,
                SCANNERS.join(", ")
            ));
        }
        if self.max_concurrent == 0 || self.max_concurrent > 16 {
            return Err("vuln_scan.max_concurrent: 1-16".into());
        }
        if self.nice > 19 {
            return Err("vuln_scan.nice: 0-19".into());
        }
        if !(30..=86_400).contains(&self.timeout_secs) {
            return Err("vuln_scan.timeout_secs: 30-86400".into());
        }
        if self.keep_scans == 0 || self.keep_scans > 100_000 {
            return Err("vuln_scan.keep_scans: 1-100000".into());
        }
        if self.schedule_hours > 24 * 90 {
            return Err("vuln_scan.schedule_hours: 0 (off) to 2160".into());
        }
        let n = self.notify_min_severity.trim().to_ascii_lowercase();
        if !matches!(n.as_str(), "" | "none" | "off")
            && Severity::parse(&n) == Severity::Unknown
            && n != "unknown"
        {
            return Err(
                "vuln_scan.notify_min_severity: none, critical, high, medium, low, negligible or unknown"
                    .into(),
            );
        }
        for (what, p) in [
            ("vuln_scan.lxc_path", &self.lxc_path),
            ("vuln_scan.incus_path", &self.incus_path),
        ] {
            if !p.starts_with('/') {
                return Err(format!("{what}: an absolute path"));
            }
        }
        if !self.db_dir.is_empty() && !self.db_dir.starts_with('/') {
            return Err("vuln_scan.db_dir: empty or an absolute path".into());
        }
        if self.scanner_path.contains(char::is_whitespace) {
            return Err("vuln_scan.scanner_path: one program path, no arguments".into());
        }
        Ok(())
    }
}

/// Host hardening baseline settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct HardeningSettings {
    /// Rule ids that are not evaluated (shown as `excluded` with this reason
    /// list's entry) and never applied.
    pub exclude: Vec<String>,
    /// Per-rule wanted value overriding the baseline (sysctl rules only),
    /// e.g. `kernel.yama.ptrace_scope: "2"`.
    pub overrides: BTreeMap<String, String>,
    /// sysctl drop-in that holds the applied kernel settings (read at boot
    /// by OpenRC's sysctl service and by systemd-sysctl alike).
    pub sysctl_file: String,
    /// Hours between scheduled baseline checks; 0 = only on demand.
    pub schedule_hours: u32,
    /// A scheduled check that finds a rule newly failing sends a
    /// `security` notification.
    pub notify_on_regression: bool,
}

impl Default for HardeningSettings {
    fn default() -> Self {
        Self {
            exclude: Vec::new(),
            overrides: BTreeMap::new(),
            sysctl_file: "/etc/sysctl.d/60-horcrux-hardening.conf".into(),
            schedule_hours: 24,
            notify_on_regression: true,
        }
    }
}

impl HardeningSettings {
    pub fn validate(&self) -> Result<(), String> {
        if !self.sysctl_file.starts_with("/etc/sysctl.d/") || !self.sysctl_file.ends_with(".conf") {
            return Err("hardening.sysctl_file: a /etc/sysctl.d/*.conf path".into());
        }
        if self.sysctl_file.contains("..") {
            return Err("hardening.sysctl_file: no '..'".into());
        }
        if self.schedule_hours > 24 * 90 {
            return Err("hardening.schedule_hours: 0 (off) to 2160".into());
        }
        for (k, v) in &self.overrides {
            if v.is_empty() || v.len() > 64 || v.contains(['\n', '\r', '=']) {
                return Err(format!(
                    "hardening.overrides.{k}: a short single-line value"
                ));
            }
        }
        Ok(())
    }
}

/// Linux audit (auditd) settings: Horcrux's rule set and event collection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct AuditSettings {
    /// Load the Horcrux rule set, enable and start auditd, collect events.
    pub enabled: bool,
    /// Where the rule set is written (read by augenrules at boot on OpenRC
    /// and systemd alike).
    pub rules_file: String,
    /// Extra files or directories watched for writes and attribute changes
    /// (key `hx-watch`), on top of the built-in list.
    pub extra_watch: Vec<String>,
    /// Record kernel module loads and unloads.
    pub watch_modules: bool,
    /// Record changes of the system clock.
    pub watch_time: bool,
    /// Make the rule set immutable until the next boot (`-e 2`). Off by
    /// default: turning the rules off again then needs a reboot.
    pub immutable: bool,
    /// Kernel audit backlog (`-b`).
    pub backlog: u32,
    /// Seconds between event collections (ausearch).
    pub collect_secs: u32,
    /// Send a `security` notification for these event kinds (see
    /// `AUDIT_KINDS`); empty = none.
    pub notify_kinds: Vec<String>,
    /// Manage the auditd service (enable + start). Off when another tool
    /// owns auditd; the rule file is still written and loaded.
    pub manage_service: bool,
}

/// Event kinds of the audit rule set (the audit key without `hx-`).
pub const AUDIT_KINDS: &[&str] = &[
    "login", "auth", "sudo", "identity", "sshd", "horcrux", "binaries", "modules", "time", "watch",
];

impl Default for AuditSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            rules_file: "/etc/audit/rules.d/50-horcrux.rules".into(),
            extra_watch: Vec::new(),
            watch_modules: true,
            watch_time: true,
            immutable: false,
            backlog: 8192,
            collect_secs: 60,
            notify_kinds: vec![
                "identity".into(),
                "sshd".into(),
                "binaries".into(),
                "modules".into(),
            ],
            manage_service: true,
        }
    }
}

impl AuditSettings {
    pub fn validate(&self) -> Result<(), String> {
        if !self.rules_file.starts_with("/etc/audit/rules.d/")
            || !self.rules_file.ends_with(".rules")
            || self.rules_file.contains("..")
        {
            return Err("audit.rules_file: a /etc/audit/rules.d/*.rules path".into());
        }
        for p in &self.extra_watch {
            if !p.starts_with('/') || p.contains(char::is_whitespace) || p.contains("..") {
                return Err(format!(
                    "audit.extra_watch: {p:?} is not an absolute path without spaces"
                ));
            }
        }
        if !(64..=1_048_576).contains(&self.backlog) {
            return Err("audit.backlog: 64-1048576".into());
        }
        if !(10..=86_400).contains(&self.collect_secs) {
            return Err("audit.collect_secs: 10-86400".into());
        }
        for k in &self.notify_kinds {
            if !AUDIT_KINDS.contains(&k.as_str()) {
                return Err(format!(
                    "audit.notify_kinds: {k:?} is not one of {}",
                    AUDIT_KINDS.join(", ")
                ));
            }
        }
        Ok(())
    }
}

/// File integrity (AIDE) settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct IntegritySettings {
    /// Scheduled checks and the Initialize action.
    pub enabled: bool,
    /// AIDE program; empty = `aide` from PATH.
    pub aide_path: String,
    /// Directories whose files are recorded (content hash, owner, mode, ...).
    pub paths: Vec<String>,
    /// Paths left out (regular expressions are escaped; a path excludes
    /// everything below it). VM disks, container and data dirs belong here.
    pub exclude: Vec<String>,
    /// Database directory. Empty = `<data_dir>/security/aide`.
    pub db_dir: String,
    /// Hours between scheduled checks; 0 = only on demand.
    pub schedule_hours: u32,
    /// CPU niceness of AIDE (0-19); IO runs idle-class.
    pub nice: u8,
    /// A run that takes longer is stopped.
    pub timeout_secs: u32,
    /// A check with differences sends a `security` notification.
    pub notify: bool,
    /// Per-file events recorded per check (the rest are counted in the
    /// summary event).
    pub max_file_events: u32,
}

impl Default for IntegritySettings {
    fn default() -> Self {
        Self {
            enabled: false,
            aide_path: String::new(),
            paths: vec!["/usr".into(), "/etc".into(), "/boot".into()],
            exclude: vec![
                "/usr/src".into(),
                "/usr/portage".into(),
                "/usr/share/man".into(),
                "/usr/share/doc".into(),
                "/etc/mtab".into(),
                "/etc/adjtime".into(),
                "/etc/resolv.conf".into(),
                "/etc/ld.so.cache".into(),
                "/etc/horcrux/state".into(),
                "/var/lib/horcrux".into(),
                "/var/lib/libvirt".into(),
                "/var/lib/lxc".into(),
                "/var/lib/incus".into(),
                "/var/lib/containers".into(),
                "/var/lib/docker".into(),
            ],
            db_dir: String::new(),
            schedule_hours: 24,
            nice: 19,
            timeout_secs: 7200,
            notify: true,
            max_file_events: 200,
        }
    }
}

impl IntegritySettings {
    pub fn program(&self) -> String {
        if self.aide_path.trim().is_empty() {
            "aide".into()
        } else {
            self.aide_path.trim().to_string()
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.paths.is_empty() {
            return Err("integrity.paths: at least one directory".into());
        }
        for (what, list) in [
            ("integrity.paths", &self.paths),
            ("integrity.exclude", &self.exclude),
        ] {
            for p in list {
                if !p.starts_with('/') || p.contains(char::is_whitespace) || p.contains("..") {
                    return Err(format!(
                        "{what}: {p:?} is not an absolute path without spaces"
                    ));
                }
            }
        }
        if !self.db_dir.is_empty() && !self.db_dir.starts_with('/') {
            return Err("integrity.db_dir: empty or an absolute path".into());
        }
        if self.aide_path.contains(char::is_whitespace) {
            return Err("integrity.aide_path: one program path, no arguments".into());
        }
        if self.schedule_hours > 24 * 90 {
            return Err("integrity.schedule_hours: 0 (off) to 2160".into());
        }
        if self.nice > 19 {
            return Err("integrity.nice: 0-19".into());
        }
        if !(60..=86_400).contains(&self.timeout_secs) {
            return Err("integrity.timeout_secs: 60-86400".into());
        }
        if self.max_file_events > 10_000 {
            return Err("integrity.max_file_events: 0-10000".into());
        }
        Ok(())
    }
}

/// Security event store settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct EventSettings {
    /// Events kept (oldest removed first).
    pub keep: u32,
}

impl Default for EventSettings {
    fn default() -> Self {
        Self { keep: 20_000 }
    }
}

/// Node security settings (`GET/PUT /api/security/settings`, declarative
/// `security:`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct SecuritySettings {
    pub vuln_scan: VulnScanSettings,
    pub hardening: HardeningSettings,
    pub audit: AuditSettings,
    pub integrity: IntegritySettings,
    pub events: EventSettings,
}

impl SecuritySettings {
    pub fn validate(&self) -> Result<(), String> {
        self.vuln_scan.validate()?;
        self.hardening.validate()?;
        self.audit.validate()?;
        self.integrity.validate()?;
        if !(100..=10_000_000).contains(&self.events.keep) {
            return Err("events.keep: 100-10000000".into());
        }
        Ok(())
    }
}

/// Where a security event came from.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum EventSource {
    /// Linux audit (auditd / ausearch).
    Audit,
    /// File integrity check (AIDE).
    Integrity,
}

impl EventSource {
    pub fn as_str(self) -> &'static str {
        match self {
            EventSource::Audit => "audit",
            EventSource::Integrity => "integrity",
        }
    }
}

/// One intrusion-detection or integrity finding (`GET /api/security/events`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct SecurityEvent {
    pub id: String,
    /// Unix seconds.
    pub at: i64,
    pub source: Option<EventSource>,
    /// Audit: the rule kind (`identity`, `sudo`, ...) or the record type for
    /// PAM events (`login`, `auth`); integrity: `added`, `removed`,
    /// `changed` or `summary`.
    pub kind: String,
    pub severity: Option<Severity>,
    /// One line for lists.
    pub summary: String,
    /// The account that acted (audit: login uid, interpreted).
    pub user: String,
    /// File or object involved.
    pub path: String,
    /// Program that acted.
    pub exe: String,
    /// Remote address (logins).
    pub addr: String,
    /// `success` / `failed` (audit), empty otherwise.
    pub result: String,
    /// Scanner detail: audit serial / record type, AIDE attribute diff.
    pub detail: String,
}

/// `GET /api/security/ids`: state of audit and integrity on this node.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct IdsStatus {
    pub audit_enabled: bool,
    /// auditctl/auditd/ausearch are installed.
    pub audit_installed: bool,
    pub auditd_running: bool,
    /// Rules with a `hx-` key are loaded in the kernel.
    pub audit_rules_loaded: u32,
    #[serde(default)]
    pub audit_last_collect: Option<i64>,
    #[serde(default)]
    pub audit_error: Option<String>,
    pub integrity_enabled: bool,
    pub aide_installed: bool,
    #[serde(default)]
    pub aide_version: String,
    /// A baseline database exists.
    pub integrity_initialized: bool,
    #[serde(default)]
    pub integrity_initialized_at: Option<i64>,
    /// `init`, `check` or `update` while one runs.
    #[serde(default)]
    pub integrity_running: Option<String>,
    #[serde(default)]
    pub integrity_last_check: Option<i64>,
    /// Differences found by the last check (added + removed + changed).
    #[serde(default)]
    pub integrity_last_differences: Option<u32>,
    #[serde(default)]
    pub integrity_error: Option<String>,
    pub events_total: u32,
    pub events_24h: u32,
}

/// Result of one hardening rule.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum RuleStatus {
    Pass,
    Fail,
    /// Needs a person (lockout risk, or an install decision): what to do is
    /// in `remediation`.
    Manual,
    /// Does not apply to this node (feature absent).
    Na,
    /// Excluded by `hardening.exclude`.
    Excluded,
}

/// How a rule is remediated.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FixKind {
    /// Horcrux applies and reverts it (kernel parameter, file mode).
    Auto,
    /// Horcrux tells you what to do; it never changes this itself.
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HardeningRuleResult {
    pub id: String,
    pub category: String,
    pub title: String,
    pub severity: Severity,
    pub status: RuleStatus,
    pub fix: FixKind,
    /// What the node has now.
    pub current: String,
    /// What the baseline wants.
    pub expected: String,
    /// Why it matters on a hypervisor.
    pub rationale: String,
    pub remediation: String,
    /// CIS Distribution Independent Linux / kernel docs reference.
    #[serde(default)]
    pub reference: String,
    /// Horcrux applied this rule (and can revert it).
    #[serde(default)]
    pub applied: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct HardeningReport {
    pub node: String,
    /// Unix seconds.
    pub checked_at: i64,
    pub baseline: String,
    /// Weighted share of passing rules (0-100) over pass + fail + manual.
    pub score: f64,
    pub passed: u32,
    pub failed: u32,
    pub manual: u32,
    pub not_applicable: u32,
    pub excluded: u32,
    pub rules: Vec<HardeningRuleResult>,
}

/// `POST /api/security/hardening/apply` and `/revert`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct HardeningApplyRequest {
    /// Rule ids; empty = every failing auto-fixable rule (apply) or every
    /// applied rule (revert).
    pub rules: Vec<String>,
    /// Show what would change without changing it.
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HardeningChange {
    pub rule: String,
    /// `set kernel.kptr_restrict 0 -> 1`, `chmod /etc/shadow 0644 -> 0640`.
    pub change: String,
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct HardeningApplyResult {
    pub dry_run: bool,
    pub changes: Vec<HardeningChange>,
    /// Rules asked for that cannot be applied automatically, with the reason.
    pub skipped: Vec<HardeningChange>,
}

/// `GET /api/security/overview`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SecurityOverview {
    /// The scanner program was found.
    pub scanner_available: bool,
    pub scanner: String,
    #[serde(default)]
    pub scanner_version: String,
    pub scanning_enabled: bool,
    /// Targets on this node (images and containers).
    pub targets: u32,
    /// Targets whose newest scan is done.
    pub targets_scanned: u32,
    /// Sum over the newest scan of each target.
    pub counts: SeverityCounts,
    pub running_scans: u32,
    #[serde(default)]
    pub last_scan_at: Option<i64>,
    pub hardening_score: f64,
    pub hardening_failed: u32,
    pub hardening_manual: u32,
    #[serde(default)]
    pub hardening_checked_at: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate() {
        SecuritySettings::default().validate().unwrap();
    }

    #[test]
    fn severity_parse_and_order() {
        assert_eq!(Severity::parse("Critical"), Severity::Critical);
        assert_eq!(Severity::parse("MODERATE"), Severity::Medium);
        assert_eq!(Severity::parse("??"), Severity::Unknown);
        assert!(Severity::Critical > Severity::High && Severity::Low > Severity::Unknown);
        let mut c = SeverityCounts::default();
        c.add(Severity::Critical, true);
        c.add(Severity::Low, false);
        assert_eq!(
            (c.total(), c.at_least(Severity::High), c.fixable),
            (2, 1, 1)
        );
    }

    #[test]
    fn bad_settings_are_refused() {
        let mut s = SecuritySettings::default();
        s.vuln_scan.scanner = "clamav".into();
        assert!(s.validate().is_err());
        let mut s = SecuritySettings::default();
        s.hardening.sysctl_file = "/etc/passwd".into();
        assert!(s.validate().is_err());
        let mut s = SecuritySettings::default();
        s.vuln_scan.notify_min_severity = "loud".into();
        assert!(s.validate().is_err());
        let mut s = SecuritySettings::default();
        s.vuln_scan.notify_min_severity = "none".into();
        assert!(s.validate().is_ok());
        assert!(s.vuln_scan.notify_threshold().is_none());
    }

    #[test]
    fn ids_settings_defaults_and_refusals() {
        let s = SecuritySettings::default();
        assert!(!s.audit.enabled && !s.integrity.enabled);
        assert_eq!(s.integrity.paths, ["/usr", "/etc", "/boot"]);
        assert!(s.integrity.exclude.iter().any(|p| p == "/var/lib/horcrux"));
        assert_eq!(s.integrity.program(), "aide");
        let mut b = s.clone();
        b.audit.rules_file = "/etc/passwd".into();
        assert!(b.validate().is_err());
        let mut b = s.clone();
        b.audit.notify_kinds = vec!["everything".into()];
        assert!(b.validate().is_err());
        let mut b = s.clone();
        b.audit.extra_watch = vec!["relative/path".into()];
        assert!(b.validate().is_err());
        let mut b = s.clone();
        b.integrity.paths = vec![];
        assert!(b.validate().is_err());
        let mut b = s.clone();
        b.integrity.exclude = vec!["/a b".into()];
        assert!(b.validate().is_err());
        let mut b = s.clone();
        b.events.keep = 1;
        assert!(b.validate().is_err());
        // Old settings without the new sections still load.
        let old: SecuritySettings = serde_json::from_str(r#"{"hardening":{}}"#).unwrap();
        assert_eq!(old.audit, AuditSettings::default());
        assert_eq!(old.integrity.max_file_events, 200);
    }

    #[test]
    fn partial_json_fills_defaults() {
        let s: SecuritySettings =
            serde_json::from_str(r#"{"vuln_scan":{"scanner":"trivy"}}"#).unwrap();
        assert_eq!(s.vuln_scan.scanner, "trivy");
        assert_eq!(s.vuln_scan.timeout_secs, 1800);
        assert_eq!(s.hardening, HardeningSettings::default());
    }
}
