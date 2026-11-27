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
    /// A virtual machine (QEMU). A stopped VM is scanned from a read-only
    /// export of its disk (`dir:` of the mounted guest file system); a
    /// running VM is scanned from the package inventory its guest agent
    /// reports (`sbom:`), never by touching its live disk.
    Vm,
}

impl ScanTargetKind {
    pub const ALL: [ScanTargetKind; 5] = [
        ScanTargetKind::Podman,
        ScanTargetKind::Docker,
        ScanTargetKind::Lxc,
        ScanTargetKind::Incus,
        ScanTargetKind::Vm,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ScanTargetKind::Podman => "podman",
            ScanTargetKind::Docker => "docker",
            ScanTargetKind::Lxc => "lxc",
            ScanTargetKind::Incus => "incus",
            ScanTargetKind::Vm => "vm",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ScanTargetKind::Podman => "Podman image",
            ScanTargetKind::Docker => "Docker image",
            ScanTargetKind::Lxc => "LXC container",
            ScanTargetKind::Incus => "Incus container",
            ScanTargetKind::Vm => "virtual machine",
        }
    }

    /// Image kinds name an image reference; the others name a container or VM.
    pub fn is_image(self) -> bool {
        matches!(self, ScanTargetKind::Podman | ScanTargetKind::Docker)
    }

    /// A VM is scanned either from a mounted disk export (stopped) or from
    /// the guest agent's inventory (running), not through a container runtime.
    pub fn is_vm(self) -> bool {
        matches!(self, ScanTargetKind::Vm)
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
    /// Scan stopped VMs from a read-only export of their disk (the disk is
    /// never written; encrypted guest disks are skipped with a reason).
    pub vm_disk_scan: bool,
    /// Scan running VMs from the package inventory their Horcrux guest agent
    /// reports (no host access to the live disk).
    pub vm_agent_scan: bool,
    /// A VM disk scan whose mount/export preparation runs longer than this is
    /// given up (separate from the scanner's own `timeout_secs`).
    pub vm_mount_timeout_secs: u32,
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
            vm_disk_scan: true,
            vm_agent_scan: true,
            vm_mount_timeout_secs: 120,
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
        if !(10..=3600).contains(&self.vm_mount_timeout_secs) {
            return Err("vuln_scan.vm_mount_timeout_secs: 10-3600".into());
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

/// Node security settings (`GET/PUT /api/security/settings`, declarative
/// `security:`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct SecuritySettings {
    pub vuln_scan: VulnScanSettings,
    pub hardening: HardeningSettings,
    pub microseg: MicrosegSettings,
}

impl SecuritySettings {
    pub fn validate(&self) -> Result<(), String> {
        self.vuln_scan.validate()?;
        self.hardening.validate()?;
        self.microseg.validate()
    }
}

// ------------------------------------------------------ microsegmentation

/// Ready-made per-guest firewall profile (docs/design/security-suite.md,
/// "Guest microsegmentation").
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum MsegTemplate {
    /// Only what `allow_from` / `allow_to` and the defaults say.
    #[default]
    Custom,
    /// Web ports in from anywhere; DNS, NTP and HTTPS out.
    WebServer,
    /// Database ports in from the listed groups only; DNS and NTP out.
    Database,
    /// Talks to its lab network and nothing else, both ways.
    IsolatedLab,
    /// Outbound only to its lab network; lab hosts may call back; the
    /// listed groups may manage it.
    PentestBox,
}

impl MsegTemplate {
    pub const ALL: [MsegTemplate; 5] = [
        MsegTemplate::Custom,
        MsegTemplate::WebServer,
        MsegTemplate::Database,
        MsegTemplate::IsolatedLab,
        MsegTemplate::PentestBox,
    ];
    pub fn id(&self) -> &'static str {
        match self {
            MsegTemplate::Custom => "custom",
            MsegTemplate::WebServer => "web_server",
            MsegTemplate::Database => "database",
            MsegTemplate::IsolatedLab => "isolated_lab",
            MsegTemplate::PentestBox => "pentest_box",
        }
    }
    pub fn title(&self) -> &'static str {
        match self {
            MsegTemplate::Custom => "Custom",
            MsegTemplate::WebServer => "Web server",
            MsegTemplate::Database => "Database",
            MsegTemplate::IsolatedLab => "Isolated lab",
            MsegTemplate::PentestBox => "Pentest box",
        }
    }
    pub fn description(&self) -> &'static str {
        match self {
            MsegTemplate::Custom => {
                "Default deny in and out; only the groups you allow below are reachable."
            }
            MsegTemplate::WebServer => {
                "HTTP and HTTPS in from anywhere; DNS, NTP and HTTPS out. Everything else is dropped."
            }
            MsegTemplate::Database => {
                "Database ports in from the allowed groups only; DNS and NTP out. Nothing else in or out."
            }
            MsegTemplate::IsolatedLab => {
                "Talks to the lab network in both directions and nothing else."
            }
            MsegTemplate::PentestBox => {
                "Outbound only to the lab network, so a scan can never leave it; lab hosts may call back; the allowed groups may manage it."
            }
        }
    }
    /// Needs `lab_network` set.
    pub fn needs_lab_network(&self) -> bool {
        matches!(self, MsegTemplate::IsolatedLab | MsegTemplate::PentestBox)
    }
    /// Default ports of the template (`ports` overrides), "" = none.
    pub fn default_ports(&self) -> &'static str {
        match self {
            MsegTemplate::WebServer => "80,443",
            MsegTemplate::Database => "3306,5432",
            MsegTemplate::PentestBox => "",
            _ => "",
        }
    }
}

/// Default action of one direction when no rule matched.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum MsegDefault {
    Allow,
    #[default]
    Deny,
}

/// "Allow from / to group X": `group` is an address, a CIDR, an alias or an
/// IP set written `+name` (the firewall objects).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct MsegPeer {
    pub group: String,
    /// `any` (default), `tcp`, `udp` or `icmp`.
    pub protocol: String,
    /// Port, range (`8000-8100`) or list (`80,443`); empty = any port.
    pub dport: String,
    pub comment: String,
}

impl Default for MsegPeer {
    fn default() -> Self {
        Self {
            group: String::new(),
            protocol: "any".into(),
            dport: String::new(),
            comment: String::new(),
        }
    }
}

/// The microsegmentation profile of one guest (`vm:<id>` or
/// `container:<id>`). It is installed per NIC of that guest: on every tap
/// or veth of a bridge or SDN vnet, and on the macvtap devices listed in
/// `macvtap_devices`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct GuestProfile {
    pub enabled: bool,
    pub template: MsegTemplate,
    /// Inbound / outbound action when nothing matched (default deny).
    pub default_in: MsegDefault,
    pub default_out: MsegDefault,
    pub allow_from: Vec<MsegPeer>,
    pub allow_to: Vec<MsegPeer>,
    /// Lab network of `isolated_lab` / `pentest_box`.
    pub lab_network: String,
    /// Template port list override (`web_server`: the served ports,
    /// `database`: the database ports, `pentest_box`: callback ports,
    /// empty = any).
    pub ports: String,
    /// Host-side macvtap devices of the guest's macvtap NICs (they are not
    /// bridge ports, so they are filtered on the device itself).
    pub macvtap_devices: Vec<String>,
    pub comment: String,
}

impl Default for GuestProfile {
    fn default() -> Self {
        Self {
            enabled: true,
            template: MsegTemplate::Custom,
            default_in: MsegDefault::Deny,
            default_out: MsegDefault::Deny,
            allow_from: Vec::new(),
            allow_to: Vec::new(),
            lab_network: String::new(),
            ports: String::new(),
            macvtap_devices: Vec::new(),
            comment: String::new(),
        }
    }
}

/// Microsegmentation settings (`security.microseg`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct MicrosegSettings {
    /// Master switch: off removes every profile from the kernel (the
    /// stored profiles stay).
    pub enabled: bool,
    /// Rate-limited kernel log line for every dropped flow (feeds the
    /// flow view's denied list).
    pub log_denied: bool,
    /// Per-guest default-deny still lets ARP, IPv6 neighbour discovery and
    /// DHCP through; turning it off needs static addressing.
    pub allow_infrastructure: bool,
    /// Most flows the flow view returns.
    pub flow_limit: u32,
    /// Logged drops per second per guest chain (nft `limit rate`).
    pub log_rate_per_second: u32,
    /// Profiles by guest key (`vm:100`, `container:web1`).
    pub profiles: BTreeMap<String, GuestProfile>,
}

impl Default for MicrosegSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            log_denied: true,
            allow_infrastructure: true,
            flow_limit: 500,
            log_rate_per_second: 10,
            profiles: BTreeMap::new(),
        }
    }
}

/// Prefix of the kernel log line written for a dropped flow.
pub const MSEG_LOG_PREFIX: &str = "hx-mseg: ";

fn valid_group(g: &str) -> bool {
    let g = g.trim();
    !g.is_empty()
        && g.len() <= 64
        && g.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '/' | '+' | '_' | '-'))
}

fn valid_ports(p: &str) -> bool {
    let p = p.trim();
    if p.is_empty() {
        return true;
    }
    p.split(',').all(|part| {
        let part = part.trim();
        let mut it = part.splitn(2, '-');
        let ok = |s: &str| s.parse::<u16>().map(|n| n > 0).unwrap_or(false);
        match (it.next(), it.next()) {
            (Some(a), None) => ok(a),
            (Some(a), Some(b)) => {
                ok(a) && ok(b) && a.parse::<u16>().unwrap_or(0) <= b.parse::<u16>().unwrap_or(0)
            }
            _ => false,
        }
    })
}

/// A guest key is `vm:<id>` or `container:<id>`.
pub fn valid_guest_key(k: &str) -> bool {
    match k.split_once(':') {
        Some((kind, id)) => {
            matches!(kind, "vm" | "container")
                && !id.is_empty()
                && id.len() <= 64
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        }
        None => false,
    }
}

impl MsegPeer {
    pub fn validate(&self, what: &str) -> Result<(), String> {
        if !valid_group(&self.group) {
            return Err(format!(
                "{what}.group: an address, CIDR, alias or +ipset (letters, digits, . : / + _ -)"
            ));
        }
        if !matches!(self.protocol.as_str(), "any" | "tcp" | "udp" | "icmp") {
            return Err(format!("{what}.protocol: any, tcp, udp or icmp"));
        }
        if !valid_ports(&self.dport) {
            return Err(format!(
                "{what}.dport: a port, a range 1-2 or a list 80,443"
            ));
        }
        if !self.dport.trim().is_empty() && !matches!(self.protocol.as_str(), "tcp" | "udp") {
            return Err(format!("{what}.dport: ports need protocol tcp or udp"));
        }
        if self.comment.len() > 120 || self.comment.contains(['"', '\n', '\r']) {
            return Err(format!("{what}.comment: at most 120 characters, no quotes"));
        }
        Ok(())
    }
}

impl GuestProfile {
    pub fn validate(&self, key: &str) -> Result<(), String> {
        let w = format!("microseg.profiles.{key}");
        if self.allow_from.len() > 64 || self.allow_to.len() > 64 {
            return Err(format!(
                "{w}: at most 64 allow_from and 64 allow_to entries"
            ));
        }
        for (i, p) in self.allow_from.iter().enumerate() {
            p.validate(&format!("{w}.allow_from[{i}]"))?;
        }
        for (i, p) in self.allow_to.iter().enumerate() {
            p.validate(&format!("{w}.allow_to[{i}]"))?;
        }
        if self.template.needs_lab_network() && !valid_group(&self.lab_network) {
            return Err(format!(
                "{w}.lab_network: required for the {} template (CIDR, alias or +ipset)",
                self.template.id()
            ));
        }
        if !self.lab_network.is_empty() && !valid_group(&self.lab_network) {
            return Err(format!("{w}.lab_network: a CIDR, alias or +ipset"));
        }
        if !valid_ports(&self.ports) {
            return Err(format!("{w}.ports: a port, a range 1-2 or a list 80,443"));
        }
        if self.template == MsegTemplate::PentestBox && self.default_out == MsegDefault::Allow {
            return Err(format!(
                "{w}.default_out: a pentest box never defaults to allow outbound (it must not leave its lab)"
            ));
        }
        if self.template == MsegTemplate::IsolatedLab
            && (self.default_out == MsegDefault::Allow || self.default_in == MsegDefault::Allow)
        {
            return Err(format!(
                "{w}: an isolated lab denies by default in both directions"
            ));
        }
        if self.macvtap_devices.len() > 8 {
            return Err(format!("{w}.macvtap_devices: at most 8"));
        }
        for d in &self.macvtap_devices {
            if d.is_empty()
                || d.len() > 15
                || !d
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
            {
                return Err(format!(
                    "{w}.macvtap_devices: interface names (max 15 characters)"
                ));
            }
        }
        if self.comment.len() > 200 || self.comment.contains(['\n', '\r']) {
            return Err(format!("{w}.comment: one line, at most 200 characters"));
        }
        Ok(())
    }
}

impl MicrosegSettings {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=10_000).contains(&self.flow_limit) {
            return Err("microseg.flow_limit: 1 to 10000".into());
        }
        if !(1..=1000).contains(&self.log_rate_per_second) {
            return Err("microseg.log_rate_per_second: 1 to 1000".into());
        }
        if self.profiles.len() > 2000 {
            return Err("microseg.profiles: at most 2000".into());
        }
        for (k, p) in &self.profiles {
            if !valid_guest_key(k) {
                return Err(format!(
                    "microseg.profiles.{k}: the key is vm:<id> or container:<id>"
                ));
            }
            p.validate(k)?;
        }
        Ok(())
    }
}

/// One template for the UI picker.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MsegTemplateInfo {
    pub id: String,
    pub title: String,
    pub description: String,
    pub needs_lab_network: bool,
    pub default_ports: String,
    /// What the template allows, as readable lines.
    pub allows: Vec<String>,
}

/// Hit counter of one rule of a guest's chain.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MsegRuleHit {
    pub guest: String,
    /// `in` (to the guest) or `out` (from the guest).
    pub direction: String,
    pub rule_id: String,
    pub action: String,
    /// The rule as nft shows it.
    pub rule: String,
    pub packets: u64,
    pub bytes: u64,
}

/// One tracked flow (conntrack) in the flow view.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MsegFlow {
    pub proto: String,
    pub src: String,
    pub dst: String,
    pub sport: String,
    pub dport: String,
    pub state: String,
    pub packets: u64,
    pub bytes: u64,
}

/// One dropped flow from the kernel log.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MsegDenied {
    pub time: String,
    pub iface_in: String,
    pub iface_out: String,
    pub src: String,
    pub dst: String,
    pub proto: String,
    pub sport: String,
    pub dport: String,
}

/// Answer of `GET /api/security/microseg/guests/:key/flows`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct MsegFlowView {
    pub flows: Vec<MsegFlow>,
    pub denied: Vec<MsegDenied>,
    /// Why a part is empty (no conntrack, no kernel log access).
    pub notes: Vec<String>,
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

// --------------------------------------------------- VM guest inventory

/// One installed package reported by a guest's software inventory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct GuestPackage {
    pub name: String,
    pub version: String,
    /// Scanner package type: `deb`, `rpm`, `apk`, `portage` or `windows`.
    #[serde(default, rename = "type")]
    pub pkg_type: String,
    /// Architecture, when the package manager reports one.
    #[serde(default)]
    pub arch: String,
}

/// A guest's software inventory: the output of `horcrux-guest-agent
/// inventory`, read from a running VM through its guest agent. Turned into an
/// SBOM (`to_sbom`) that the vulnerability scanner matches against its
/// database, so the live guest disk is never touched.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct GuestInventory {
    /// `linux` or `windows`.
    #[serde(default)]
    pub os: String,
    /// os-release `ID` on Linux (`debian`, `ubuntu`, `alpine`, `gentoo`,
    /// `rhel`, ...); empty on Windows.
    #[serde(default)]
    pub distro_id: String,
    /// os-release `VERSION_ID`.
    #[serde(default)]
    pub distro_version: String,
    /// os-release `ID_LIKE` entries (helps the scanner's distro matching).
    #[serde(default)]
    pub distro_like: Vec<String>,
    pub packages: Vec<GuestPackage>,
}

impl GuestInventory {
    /// A package Universal URL (PURL) for a package of this inventory, e.g.
    /// `pkg:deb/ubuntu/bash@5.1?arch=amd64&distro=ubuntu-22.04`. This is the
    /// key the scanner matches OS-package advisories on.
    fn purl(&self, p: &GuestPackage) -> String {
        let name = purl_encode(&p.name);
        let ver = purl_encode(&p.version);
        let mut qs: Vec<String> = Vec::new();
        if !p.arch.is_empty() {
            qs.push(format!("arch={}", purl_encode(&p.arch)));
        }
        let distro = self.distro_qualifier();
        match p.pkg_type.as_str() {
            "deb" | "rpm" | "apk" => {
                let ns = if self.distro_id.is_empty() {
                    p.pkg_type.clone()
                } else {
                    purl_encode(&self.distro_id)
                };
                if let Some(d) = &distro {
                    qs.push(format!("distro={d}"));
                }
                with_qs(&format!("pkg:{}/{ns}/{name}@{ver}", p.pkg_type), &qs)
            }
            "portage" => with_qs(&format!("pkg:portage/{name}@{ver}"), &qs),
            // Windows and anything else: a generic PURL (best-effort, matched
            // by CPE only; there is no OS advisory feed for it).
            _ => with_qs(&format!("pkg:generic/{name}@{ver}"), &qs),
        }
    }

    /// `ubuntu-22.04`-style distro qualifier, or `None` without a distro.
    fn distro_qualifier(&self) -> Option<String> {
        if self.distro_id.is_empty() {
            return None;
        }
        Some(if self.distro_version.is_empty() {
            purl_encode(&self.distro_id)
        } else {
            format!(
                "{}-{}",
                purl_encode(&self.distro_id),
                purl_encode(&self.distro_version)
            )
        })
    }

    /// A CycloneDX 1.5 JSON SBOM (the format syft emits and Grype/Trivy read
    /// from `sbom:`). The OS is a `operating-system` metadata component with
    /// `syft:distro:*` properties so the scanner applies the right distro
    /// feed; every package carries its PURL and `syft:package:type`.
    pub fn to_sbom(&self) -> String {
        let mut distro_props = Vec::new();
        if !self.distro_id.is_empty() {
            distro_props.push(serde_json::json!({"name":"syft:distro:id","value":self.distro_id}));
        }
        if !self.distro_version.is_empty() {
            distro_props.push(
                serde_json::json!({"name":"syft:distro:versionID","value":self.distro_version}),
            );
        }
        for (i, l) in self.distro_like.iter().enumerate() {
            distro_props
                .push(serde_json::json!({"name":format!("syft:distro:idLike:{i}"),"value":l}));
        }
        let components: Vec<serde_json::Value> = self
            .packages
            .iter()
            .map(|p| {
                let mut props =
                    vec![serde_json::json!({"name":"syft:package:type","value":p.pkg_type})];
                if !p.arch.is_empty() {
                    props.push(
                        serde_json::json!({"name":"syft:package:metadataType","value":p.pkg_type}),
                    );
                }
                serde_json::json!({
                    "type": "library",
                    "name": p.name,
                    "version": p.version,
                    "purl": self.purl(p),
                    "properties": props,
                })
            })
            .collect();
        let os_name = if self.distro_id.is_empty() {
            self.os.clone()
        } else {
            self.distro_id.clone()
        };
        let doc = serde_json::json!({
            "bomFormat": "CycloneDX",
            "specVersion": "1.5",
            "version": 1,
            "metadata": {
                "component": {
                    "type": "operating-system",
                    "name": os_name,
                    "version": self.distro_version,
                    "properties": distro_props,
                }
            },
            "components": components,
        });
        serde_json::to_string(&doc).unwrap_or_default()
    }
}

fn with_qs(base: &str, qs: &[String]) -> String {
    if qs.is_empty() {
        base.to_string()
    } else {
        format!("{base}?{}", qs.join("&"))
    }
}

/// Percent-encode the characters a PURL must not carry literally.
fn purl_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'/' | b'+') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Identify an encrypted guest volume from the first bytes of a block device
/// or partition, so a VM disk scan skips it with a clear reason instead of
/// trying (and failing) to mount it. Only positive identifications are
/// reported; anything unknown returns `None` (treated as not encrypted).
pub fn disk_encryption(header: &[u8]) -> Option<&'static str> {
    // LUKS1 and LUKS2 both start with the magic "LUKS\xba\xbe".
    if header.len() >= 6 && &header[0..6] == b"LUKS\xba\xbe" {
        return Some("LUKS");
    }
    // BitLocker: the volume boot record's OEM id (offset 3, 8 bytes) is
    // "-FVE-FS-" (full-volume encryption) or "MSWIN4.1" for BitLocker-To-Go
    // with the FVE GUID following; match the FVE marker.
    if header.len() >= 11 && &header[3..11] == b"-FVE-FS-" {
        return Some("BitLocker");
    }
    None
}

/// Filesystem types `blkid`/`lsblk` report for an encrypted volume, mapped to
/// a human reason; used when the VM disk scan inspects a nbd export.
pub fn encrypted_fstype(fstype: &str) -> Option<&'static str> {
    match fstype.trim() {
        "crypto_LUKS" => Some("LUKS"),
        "BitLocker" => Some("BitLocker"),
        _ => None,
    }
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
    fn partial_json_fills_defaults() {
        let s: SecuritySettings =
            serde_json::from_str(r#"{"vuln_scan":{"scanner":"trivy"}}"#).unwrap();
        assert_eq!(s.vuln_scan.scanner, "trivy");
        assert_eq!(s.vuln_scan.timeout_secs, 1800);
        assert_eq!(s.hardening, HardeningSettings::default());
    }

    #[test]
    fn vm_kind_and_settings() {
        assert_eq!(ScanTargetKind::Vm.as_str(), "vm");
        assert!(ScanTargetKind::Vm.is_vm() && !ScanTargetKind::Vm.is_image());
        assert_eq!(ScanTargetKind::ALL.len(), 5);
        assert!(ScanTargetKind::ALL.contains(&ScanTargetKind::Vm));
        // round-trips through the wire format
        let j = serde_json::to_string(&ScanTargetKind::Vm).unwrap();
        assert_eq!(j, "\"vm\"");
        assert_eq!(
            serde_json::from_str::<ScanTargetKind>("\"vm\"").unwrap(),
            ScanTargetKind::Vm
        );
        // VM targets are scanned by default and validated.
        let s = VulnScanSettings::default();
        assert!(
            s.vm_disk_scan && s.vm_agent_scan && s.schedule_kinds.contains(&ScanTargetKind::Vm)
        );
        let mut s = SecuritySettings::default();
        s.vuln_scan.vm_mount_timeout_secs = 5;
        assert!(s.validate().is_err());
    }

    #[test]
    fn inventory_round_trips_and_builds_an_sbom() {
        let raw = r#"{
            "os":"linux","distro_id":"ubuntu","distro_version":"22.04",
            "distro_like":["debian"],
            "packages":[
                {"name":"bash","version":"5.1-6ubuntu1","type":"deb","arch":"amd64"},
                {"name":"app-admin/sudo","version":"1.9.15","type":"portage"}
            ]}"#;
        let inv: GuestInventory = serde_json::from_str(raw).unwrap();
        assert_eq!(inv.packages.len(), 2);
        // PURLs carry the distro and package type.
        assert_eq!(
            inv.purl(&inv.packages[0]),
            "pkg:deb/ubuntu/bash@5.1-6ubuntu1?arch=amd64&distro=ubuntu-22.04"
        );
        assert_eq!(
            inv.purl(&inv.packages[1]),
            "pkg:portage/app-admin/sudo@1.9.15"
        );
        let sbom = inv.to_sbom();
        let v: serde_json::Value = serde_json::from_str(&sbom).unwrap();
        assert_eq!(v["bomFormat"], "CycloneDX");
        assert_eq!(v["metadata"]["component"]["type"], "operating-system");
        assert_eq!(v["metadata"]["component"]["name"], "ubuntu");
        assert_eq!(v["components"].as_array().unwrap().len(), 2);
        assert_eq!(
            v["components"][0]["purl"],
            "pkg:deb/ubuntu/bash@5.1-6ubuntu1?arch=amd64&distro=ubuntu-22.04"
        );
        // distro id/version/idLike are carried as syft properties.
        let props = v["metadata"]["component"]["properties"].as_array().unwrap();
        assert!(props
            .iter()
            .any(|p| p["name"] == "syft:distro:id" && p["value"] == "ubuntu"));
        assert!(props
            .iter()
            .any(|p| p["name"] == "syft:distro:idLike:0" && p["value"] == "debian"));
    }

    #[test]
    fn windows_inventory_uses_generic_purls() {
        let inv = GuestInventory {
            os: "windows".into(),
            packages: vec![GuestPackage {
                name: "7-Zip 23.01".into(),
                version: "23.01".into(),
                pkg_type: "windows".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        // spaces are percent-encoded; no distro feed, so a generic PURL.
        assert_eq!(
            inv.purl(&inv.packages[0]),
            "pkg:generic/7-Zip%2023.01@23.01"
        );
        let v: serde_json::Value = serde_json::from_str(&inv.to_sbom()).unwrap();
        assert_eq!(v["metadata"]["component"]["name"], "windows");
    }

    #[test]
    fn encrypted_disks_are_identified() {
        let mut luks = b"LUKS\xba\xbe".to_vec();
        luks.extend_from_slice(&[0u8; 100]);
        assert_eq!(disk_encryption(&luks), Some("LUKS"));
        let mut bl = vec![0xEBu8, 0x58, 0x90];
        bl.extend_from_slice(b"-FVE-FS-");
        bl.extend_from_slice(&[0u8; 50]);
        assert_eq!(disk_encryption(&bl), Some("BitLocker"));
        // a plain ext4/ntfs superblock is not flagged
        assert_eq!(disk_encryption(b"\xEB\x52\x90NTFS    "), None);
        assert_eq!(disk_encryption(&[0u8; 4]), None);
        assert_eq!(encrypted_fstype("crypto_LUKS"), Some("LUKS"));
        assert_eq!(encrypted_fstype("ext4"), None);
    }
}

/// One row of the guest list (`GET /api/security/microseg/guests`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MsegGuestRow {
    /// `vm:<id>` or `container:<id>`.
    pub key: String,
    pub kind: String,
    pub id: String,
    pub name: String,
    pub running: bool,
    /// Template id of its profile, `None` = no profile (unmanaged).
    pub template: Option<String>,
    pub profile_enabled: bool,
    /// The kernel enforces it (profile on, feature on).
    pub enforced: bool,
    /// Packets dropped by the default-deny rules so far.
    pub dropped_packets: u64,
    /// Host devices of its macvtap NICs.
    pub macvtap_devices: Vec<String>,
}

/// Answer of `PUT /api/security/microseg/guests/:key`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MsegPutAnswer {
    pub profile: GuestProfile,
    /// What the kernel could not take (no nft, a macvtap device that does
    /// not exist yet).
    pub warnings: Vec<String>,
}
