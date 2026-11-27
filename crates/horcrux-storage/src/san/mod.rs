//! SAN block storage shared by the cluster: iSCSI targets logged in on
//! every advertised portal, dm-multipath on top, and shared LVM on the
//! multipath LUNs with lvmlockd + sanlock node locking.
//!
//! This crate holds the typed configuration, the pure renderers/parsers
//! (unit-tested with fixtures) and thin wrappers around the host tools
//! (`iscsiadm`, `multipathd`, `vgcreate --shared`, `blkdiscard`). The HTTP
//! layer, the cluster replication and the per-node reconcile loop live in
//! `horcrux-api` (`api/san.rs`).
//!
//! Design notes (docs/SAN.md):
//! * Every operational value is a typed key with a documented default
//!   ([`MultipathSettings`], [`IscsiTargetSpec`], [`SharedLvmSpec`]).
//! * Horcrux owns one multipath drop-in ([`MultipathSettings::config_path`],
//!   default `/etc/multipath/conf.d/horcrux.conf`); `/etc/multipath.conf`
//!   is never edited.
//! * CHAP secrets never sit in the replicated config rows: they live in
//!   the node's secrets directory (`<data_dir>/secrets/san/<target>.chap`,
//!   mode 0600) and travel to the other nodes only over the authenticated
//!   peer channel.

pub mod fc;
pub mod iscsi;
pub mod lvm_shared;
pub mod multipath;
pub mod nvme;
pub mod paths;
pub mod tools;

use serde::{Deserialize, Serialize};
use std::fmt;

/// Config-store kinds (rows replicated to every cluster member).
pub mod kinds {
    pub const TARGET: &str = "san_iscsi_target";
    pub const SETTINGS: &str = "san_settings";
    pub const SHARED_LVM: &str = "san_shared_lvm";
    pub const HOST_ID: &str = "san_host_id";
    pub const NVME_TARGET: &str = "san_nvme_target";
}

pub use nvme::NvmeTargetSpec;

/// The id of the single cluster-wide settings row.
pub const SETTINGS_ID: &str = "cluster";

fn yes() -> bool {
    true
}

/// Valid object id: what storage ids accept (`[A-Za-z0-9_-]`, 1..=64).
pub fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && !s.starts_with('-')
}

/// An iSCSI qualified name (`iqn.YYYY-MM.reverse.domain[:id]`) or an EUI
/// (`eui.` + 16 hex digits). RFC 3720 3.2.6.
pub fn valid_iqn(s: &str) -> bool {
    if s.len() > 223 || s.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    if let Some(hex) = s.strip_prefix("eui.") {
        return hex.len() == 16 && hex.chars().all(|c| c.is_ascii_hexdigit());
    }
    let Some(rest) = s.strip_prefix("iqn.") else {
        return false;
    };
    // date part: YYYY-MM
    let b = rest.as_bytes();
    if b.len() < 9 || b[4] != b'-' || b[7] != b'.' {
        return false;
    }
    b[..4].iter().all(u8::is_ascii_digit)
        && b[5..7].iter().all(u8::is_ascii_digit)
        && rest[8..]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'))
}

/// One iSCSI target the cluster logs in to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IscsiTargetSpec {
    pub id: String,
    /// Target IQN.
    pub iqn: String,
    /// Portals to run SendTargets discovery against (`host`, `host:port`,
    /// `[v6]:port`; default port 3260). At least one.
    pub portals: Vec<String>,
    /// Log in on every portal the target advertises in discovery, not only
    /// the listed ones (default true: that is what gives multipath its
    /// paths). When false, only the listed portals are used.
    #[serde(default = "yes")]
    pub use_advertised_portals: bool,
    /// One-way CHAP user name. The secret is write-only (see
    /// [`TargetWrite::chap_secret`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chap_user: Option<String>,
    /// `node.session.timeo.replacement_timeout`: seconds a failed session
    /// holds I/O before failing it up to multipath (default 5, RHEL's
    /// recommendation with dm-multipath).
    #[serde(default = "default_replacement_timeout")]
    pub replacement_timeout_secs: u32,
    /// `node.conn[0].timeo.login_timeout` (default 15).
    #[serde(default = "default_login_timeout")]
    pub login_timeout_secs: u32,
    /// Log in at boot (`node.startup automatic`, default true).
    #[serde(default = "yes")]
    pub startup_automatic: bool,
    /// Limit the target to some nodes (empty = every node).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Change counter (unix ms) for the cluster merge; newest wins.
    #[serde(default)]
    pub rev: u64,
    /// Tombstone, so a delete reaches nodes that were offline.
    #[serde(default)]
    pub deleted: bool,
}

fn default_replacement_timeout() -> u32 {
    5
}
fn default_login_timeout() -> u32 {
    15
}

/// The API's create/update body: the target plus the write-only secret
/// (`None` keeps the stored one, `Some("")` clears it).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetWrite {
    #[serde(flatten)]
    pub target: IscsiTargetSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chap_secret: Option<String>,
}

impl IscsiTargetSpec {
    /// Does this target apply to `node`?
    pub fn applies_to(&self, node: &str) -> bool {
        self.enabled
            && !self.deleted
            && (self.nodes.is_empty() || self.nodes.iter().any(|n| n == node))
    }

    pub fn validate(&mut self) -> Result<(), String> {
        self.id = self.id.trim().to_string();
        self.iqn = self.iqn.trim().to_string();
        if !valid_id(&self.id) {
            return Err(format!(
                "id {:?}: use letters, digits, '-' and '_' (max 64)",
                self.id
            ));
        }
        if !valid_iqn(&self.iqn) {
            return Err(format!(
                "iqn {:?} is not an iSCSI name (iqn.YYYY-MM.domain[:name] or eui.<16 hex>)",
                self.iqn
            ));
        }
        if self.portals.is_empty() {
            return Err("at least one portal is needed".into());
        }
        let mut out = Vec::new();
        for p in &self.portals {
            let p = iscsi::Portal::parse(p)?.to_string();
            if !out.contains(&p) {
                out.push(p);
            }
        }
        self.portals = out;
        if let Some(u) = &self.chap_user {
            let u = u.trim();
            if u.is_empty() {
                self.chap_user = None;
            } else if u.len() > 256 || u.chars().any(|c| c.is_whitespace() || c.is_control()) {
                return Err("chap_user must be 1..256 characters without spaces".into());
            } else {
                self.chap_user = Some(u.to_string());
            }
        }
        if !(1..=600).contains(&self.replacement_timeout_secs) {
            return Err("replacement_timeout_secs must be 1..600".into());
        }
        if !(1..=600).contains(&self.login_timeout_secs) {
            return Err("login_timeout_secs must be 1..600".into());
        }
        for n in &self.nodes {
            if n.trim().is_empty() {
                return Err("nodes must not contain empty names".into());
            }
        }
        Ok(())
    }
}

/// A CHAP secret: RFC 3720 wants 12..=16 bytes for interoperability; open-iscsi
/// accepts up to 256. Refuse whitespace (iscsiadm would split it).
pub fn validate_chap_secret(s: &str) -> Result<(), String> {
    if s.len() < 12 || s.len() > 256 {
        return Err("chap_secret must be 12..256 characters".into());
    }
    if s.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("chap_secret must not contain spaces or control characters".into());
    }
    Ok(())
}

// ----------------------------------------------------------- multipath

/// `path_grouping_policy`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PathGrouping {
    /// All paths in one group, I/O spread over all of them (active/active).
    #[default]
    Multibus,
    /// One path per group (active/passive).
    Failover,
    /// Group by ALUA/priority (arrays with preferred controllers).
    GroupByPrio,
    GroupBySerial,
    GroupByNodeName,
}

impl PathGrouping {
    pub fn as_conf(&self) -> &'static str {
        match self {
            Self::Multibus => "multibus",
            Self::Failover => "failover",
            Self::GroupByPrio => "group_by_prio",
            Self::GroupBySerial => "group_by_serial",
            Self::GroupByNodeName => "group_by_node_name",
        }
    }
}

/// `path_selector`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PathSelector {
    #[default]
    ServiceTime,
    RoundRobin,
    QueueLength,
}

impl PathSelector {
    pub fn as_conf(&self) -> &'static str {
        match self {
            Self::ServiceTime => "service-time 0",
            Self::RoundRobin => "round-robin 0",
            Self::QueueLength => "queue-length 0",
        }
    }
}

/// `find_multipaths`. Horcrux defaults to `strict` and registers the WWIDs
/// of its own SAN LUNs (`multipath -a`), so local disks are never grabbed.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FindMultipaths {
    #[default]
    Strict,
    Yes,
    Smart,
    Greedy,
    No,
}

impl FindMultipaths {
    pub fn as_conf(&self) -> &'static str {
        match self {
            Self::Strict => "strict",
            Self::Yes => "yes",
            Self::Smart => "smart",
            Self::Greedy => "greedy",
            Self::No => "no",
        }
    }
}

/// `no_path_retry`: what a map does when every path is down.
/// Serialized as `"queue"`, `"fail"` or a number of checker intervals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoPathRetry {
    /// Queue I/O forever (a dead SAN hangs the guests until it returns).
    Queue,
    /// Fail I/O at once.
    Fail,
    /// Queue for N polling intervals, then fail.
    Retries(u32),
}

impl Default for NoPathRetry {
    fn default() -> Self {
        // 12 x 5 s = a minute of queueing: rides out a controller failover,
        // does not hang guests forever.
        Self::Retries(12)
    }
}

impl fmt::Display for NoPathRetry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Queue => f.write_str("queue"),
            Self::Fail => f.write_str("fail"),
            Self::Retries(n) => write!(f, "{n}"),
        }
    }
}

impl std::str::FromStr for NoPathRetry {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s.trim() {
            "queue" => Ok(Self::Queue),
            "fail" => Ok(Self::Fail),
            n => match n.parse::<u32>() {
                Ok(v) if (1..=10_000).contains(&v) => Ok(Self::Retries(v)),
                _ => Err(format!(
                    "no_path_retry {s:?}: use \"queue\", \"fail\" or a count 1..10000"
                )),
            },
        }
    }
}

/// `failback`: when to go back to a recovered higher-priority group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Failback {
    #[default]
    Immediate,
    Manual,
    Followover,
    /// Wait N seconds after the path recovers.
    Seconds(u32),
}

impl fmt::Display for Failback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Immediate => f.write_str("immediate"),
            Self::Manual => f.write_str("manual"),
            Self::Followover => f.write_str("followover"),
            Self::Seconds(n) => write!(f, "{n}"),
        }
    }
}

impl std::str::FromStr for Failback {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s.trim() {
            "immediate" => Ok(Self::Immediate),
            "manual" => Ok(Self::Manual),
            "followover" => Ok(Self::Followover),
            n => match n.parse::<u32>() {
                Ok(v) if (1..=3600).contains(&v) => Ok(Self::Seconds(v)),
                _ => Err(format!(
                    "failback {s:?}: use \"immediate\", \"manual\", \"followover\" or seconds 1..3600"
                )),
            },
        }
    }
}

macro_rules! string_serde {
    ($t:ty) => {
        impl Serialize for $t {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&self.to_string())
            }
        }
        impl<'de> Deserialize<'de> for $t {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                // Accept "12" and 12 alike.
                let v = serde_json::Value::deserialize(d)?;
                let s = match v {
                    serde_json::Value::String(s) => s,
                    serde_json::Value::Number(n) => n.to_string(),
                    other => {
                        return Err(serde::de::Error::custom(format!(
                            "expected a string or number, got {other}"
                        )))
                    }
                };
                s.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}
string_serde!(NoPathRetry);
string_serde!(Failback);

/// A blacklist / exception rule (multipath.conf `blacklist { ... }`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "match")]
pub enum DeviceMatch {
    /// A WWID (exact, or a regex as multipath reads it).
    Wwid { wwid: String },
    /// A kernel device name regex (`^sd[a-b]$`).
    Devnode { devnode: String },
    /// Vendor + product regexes (array model).
    Device { vendor: String, product: String },
}

impl DeviceMatch {
    fn validate(&self) -> Result<(), String> {
        let ok = |s: &str| {
            !s.trim().is_empty()
                && s.len() <= 256
                && !s
                    .chars()
                    .any(|c| c == '"' || c == '\n' || c == '\r' || c == '{' || c == '}')
        };
        let fine = match self {
            Self::Wwid { wwid } => ok(wwid),
            Self::Devnode { devnode } => ok(devnode),
            Self::Device { vendor, product } => ok(vendor) && ok(product),
        };
        if fine {
            Ok(())
        } else {
            Err(format!(
                "blacklist rule {self:?}: values must be non-empty, without quotes, braces or newlines"
            ))
        }
    }
}

/// Cluster-wide SAN settings (one row, id [`SETTINGS_ID`]).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MultipathSettings {
    /// Manage dm-multipath at all (default true). Off: LUNs are used
    /// through their single block devices and no drop-in is written.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// The drop-in Horcrux owns (multipathd reads `config_dir`,
    /// default `/etc/multipath/conf.d`).
    #[serde(default = "default_config_path")]
    pub config_path: String,
    #[serde(default)]
    pub path_grouping_policy: PathGrouping,
    #[serde(default)]
    pub path_selector: PathSelector,
    #[serde(default)]
    pub no_path_retry: NoPathRetry,
    #[serde(default)]
    pub failback: Failback,
    /// Path checker interval (default 5 s).
    #[serde(default = "default_polling")]
    pub polling_interval_secs: u32,
    #[serde(default)]
    pub find_multipaths: FindMultipaths,
    /// `user_friendly_names` (default false: maps are named by WWID, the
    /// same on every node, which shared storage needs).
    #[serde(default)]
    pub user_friendly_names: bool,
    /// FC/SAS: `fast_io_fail_tmo` (default 5).
    #[serde(default = "default_fast_io_fail")]
    pub fast_io_fail_tmo_secs: u32,
    /// FC/SAS: `dev_loss_tmo` (default 600).
    #[serde(default = "default_dev_loss")]
    pub dev_loss_tmo_secs: u32,
    #[serde(default)]
    pub blacklist: Vec<DeviceMatch>,
    #[serde(default)]
    pub blacklist_exceptions: Vec<DeviceMatch>,
    /// How often each node re-applies the SAN config (log in on missing
    /// paths, rewrite the drop-in when it drifted). Default 30 s.
    #[serde(default = "default_reconcile")]
    pub reconcile_interval_secs: u32,
    /// sanlock uses the host watchdog through wdmd (default true). Only a
    /// test bed without any watchdog device turns this off.
    #[serde(default = "yes")]
    pub sanlock_watchdog: bool,
    /// Fibre Channel: register the WWIDs of the LUNs behind every online FC
    /// target port on each reconcile, so they get multipath maps (default
    /// true). FC needs no login: fabric zoning decides what a node sees.
    #[serde(default = "yes")]
    pub fc_multipath: bool,
    /// What an FC rescan does unless the request says otherwise (default
    /// `scan`: non-disruptive SCSI host scan; `lip` also re-logs the HBA in
    /// to the fabric).
    #[serde(default)]
    pub fc_rescan_mode: fc::FcRescanMode,
    #[serde(default)]
    pub rev: u64,
}

fn default_config_path() -> String {
    "/etc/multipath/conf.d/horcrux.conf".into()
}
fn default_polling() -> u32 {
    5
}
fn default_fast_io_fail() -> u32 {
    5
}
fn default_dev_loss() -> u32 {
    600
}
fn default_reconcile() -> u32 {
    30
}

impl Default for MultipathSettings {
    fn default() -> Self {
        serde_json::from_str("{}").expect("defaults")
    }
}

impl MultipathSettings {
    pub fn validate(&self) -> Result<(), String> {
        let p = std::path::Path::new(&self.config_path);
        if !p.is_absolute()
            || !self.config_path.ends_with(".conf")
            || self.config_path.contains("..")
            || self.config_path == "/etc/multipath.conf"
        {
            return Err(format!(
                "config_path {:?}: an absolute *.conf drop-in path (not /etc/multipath.conf)",
                self.config_path
            ));
        }
        if !(1..=300).contains(&self.polling_interval_secs) {
            return Err("polling_interval_secs must be 1..300".into());
        }
        if !(1..=3600).contains(&self.fast_io_fail_tmo_secs) {
            return Err("fast_io_fail_tmo_secs must be 1..3600".into());
        }
        if self.dev_loss_tmo_secs < self.fast_io_fail_tmo_secs || self.dev_loss_tmo_secs > 2_147_483
        {
            return Err("dev_loss_tmo_secs must be >= fast_io_fail_tmo_secs".into());
        }
        if !(5..=3600).contains(&self.reconcile_interval_secs) {
            return Err("reconcile_interval_secs must be 5..3600".into());
        }
        for r in self.blacklist.iter().chain(&self.blacklist_exceptions) {
            r.validate()?;
        }
        Ok(())
    }

    /// Same settings, ignoring the merge counter.
    pub fn same_as(&self, o: &Self) -> bool {
        let mut a = self.clone();
        a.rev = o.rev;
        a == *o
    }
}

// ----------------------------------------------------------- shared LVM

/// How the nodes coordinate LVM metadata and LV activation on a shared VG.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum LockType {
    /// lvmlockd with sanlock: leases on the shared LUN itself, no extra
    /// cluster stack. The only lock manager Gentoo packages for lvmlockd
    /// (`sys-fs/lvm2[sanlock]` + `sys-cluster/sanlock`; dlm is not in
    /// the tree), and it works the same on OpenRC and systemd.
    #[default]
    Sanlock,
}

/// A volume group on a SAN LUN, used by every node as one storage pool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SharedLvmSpec {
    /// Storage pool name on every node.
    pub id: String,
    /// Volume group name.
    pub vg_name: String,
    /// WWID of the multipath LUN the VG lives on (`/dev/mapper/<wwid>`), or
    /// for a single-path LUN its `/dev/disk/by-id/...` path.
    pub device: String,
    #[serde(default)]
    pub lock_type: LockType,
    /// Zero a volume before it is removed (`blkdiscard --zeroout`), so the
    /// next tenant of those extents never reads old data. Default false.
    #[serde(default)]
    pub wipe_removed: bool,
    /// Discard (UNMAP/TRIM) a volume's extents on removal, giving the space
    /// back to a thin-provisioned array. Default false.
    #[serde(default)]
    pub discard_removed: bool,
    /// Snapshots as a qcow2 volume chain on the thick LVs (default false;
    /// see `storage::lvm_chain`). Fixed once the storage holds volumes.
    #[serde(default)]
    pub snapshot_as_volume_chain: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<String>,
    #[serde(default)]
    pub rev: u64,
    #[serde(default)]
    pub deleted: bool,
}

/// A valid LVM VG name (lvm(8): `[A-Za-z0-9+_.-]`, not starting with `-`,
/// not `.`/`..`, max 127).
pub fn valid_vg_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 127
        && s != "."
        && s != ".."
        && !s.starts_with('-')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '_' | '.' | '-'))
}

impl SharedLvmSpec {
    pub fn applies_to(&self, node: &str) -> bool {
        !self.deleted && (self.nodes.is_empty() || self.nodes.iter().any(|n| n == node))
    }

    pub fn validate(&mut self) -> Result<(), String> {
        self.id = self.id.trim().to_string();
        self.device = self.device.trim().to_string();
        if !valid_id(&self.id) {
            return Err(format!("id {:?}: letters, digits, '-' and '_'", self.id));
        }
        if !valid_vg_name(&self.vg_name) {
            return Err(format!(
                "vg_name {:?} is not a valid LVM name",
                self.vg_name
            ));
        }
        self.device_path()?;
        Ok(())
    }

    /// The block device the VG sits on.
    pub fn device_path(&self) -> Result<String, String> {
        let d = &self.device;
        if d.starts_with("/dev/") && !d.contains("..") && !d.contains(char::is_whitespace) {
            return Ok(d.clone());
        }
        if !d.is_empty()
            && d.len() <= 128
            && d.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
        {
            return Ok(format!("/dev/mapper/{d}"));
        }
        Err(format!(
            "device {d:?}: a multipath WWID or an absolute /dev/... path"
        ))
    }
}

/// A node's sanlock host id (1..=2000), one row per node.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HostId {
    pub node: String,
    pub host_id: u16,
    #[serde(default)]
    pub rev: u64,
}

pub const MAX_HOST_ID: u16 = 2000;

/// The host id `node` should use given every node's claim: its own claim if
/// it is unique (ties go to the node whose name sorts first), else the
/// lowest free id.
pub fn pick_host_id(node: &str, claims: &[HostId]) -> u16 {
    let mine = claims.iter().find(|c| c.node == node).map(|c| c.host_id);
    if let Some(id) = mine {
        let clash = claims
            .iter()
            .any(|c| c.node != node && c.host_id == id && c.node.as_str() < node);
        if !clash && (1..=MAX_HOST_ID).contains(&id) {
            return id;
        }
    }
    let used: std::collections::BTreeSet<u16> = claims
        .iter()
        .filter(|c| c.node != node)
        .map(|c| c.host_id)
        .collect();
    (1..=MAX_HOST_ID)
        .find(|i| !used.contains(i))
        .unwrap_or(MAX_HOST_ID)
}

/// The `san:` section of the declarative spec (cluster-wide, like the API).
/// CHAP secrets are not part of it: set them through the API once; an
/// apply keeps the stored secret.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SanSpec {
    #[serde(default)]
    pub settings: MultipathSettings,
    #[serde(default)]
    pub targets: Vec<IscsiTargetSpec>,
    #[serde(default)]
    pub shared_lvm: Vec<SharedLvmSpec>,
    /// NVMe over Fabrics subsystems.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nvme_targets: Vec<NvmeTargetSpec>,
}

impl SanSpec {
    /// Without merge counters/tombstones, sorted by id: what a diff compares.
    pub fn normalized(&self) -> Self {
        let mut s = self.clone();
        s.settings.rev = 0;
        s.targets.retain(|t| !t.deleted);
        s.shared_lvm.retain(|l| !l.deleted);
        s.nvme_targets.retain(|t| !t.deleted);
        for t in &mut s.nvme_targets {
            t.rev = 0;
        }
        s.nvme_targets.sort_by(|a, b| a.id.cmp(&b.id));
        for t in &mut s.targets {
            t.rev = 0;
        }
        for l in &mut s.shared_lvm {
            l.rev = 0;
        }
        s.targets.sort_by(|a, b| a.id.cmp(&b.id));
        s.shared_lvm.sort_by(|a, b| a.id.cmp(&b.id));
        s
    }

    pub fn validate(&mut self) -> Result<(), String> {
        self.settings
            .validate()
            .map_err(|e| format!("settings: {e}"))?;
        let mut ids = std::collections::BTreeSet::new();
        for t in &mut self.targets {
            t.validate().map_err(|e| format!("targets.{}: {e}", t.id))?;
            if !ids.insert(t.id.clone()) {
                return Err(format!("targets: {} twice", t.id));
            }
        }
        let mut ids = std::collections::BTreeSet::new();
        for l in &mut self.shared_lvm {
            l.validate()
                .map_err(|e| format!("shared_lvm.{}: {e}", l.id))?;
            if !ids.insert(l.id.clone()) {
                return Err(format!("shared_lvm: {} twice", l.id));
            }
        }
        let mut ids = std::collections::BTreeSet::new();
        for t in &mut self.nvme_targets {
            t.validate()
                .map_err(|e| format!("nvme_targets.{}: {e}", t.id))?;
            if !ids.insert(t.id.clone()) {
                return Err(format!("nvme_targets: {} twice", t.id));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn san_spec_normalizes_for_diffs() {
        let mut a: SanSpec = serde_json::from_value(serde_json::json!({
            "targets": [
                {"id": "b", "iqn": "iqn.2003-01.org.x:b", "portals": ["10.0.0.2"], "rev": 9},
                {"id": "a", "iqn": "iqn.2003-01.org.x:a", "portals": ["10.0.0.1"]},
                {"id": "z", "iqn": "iqn.2003-01.org.x:z", "portals": ["10.0.0.3"], "deleted": true}
            ]
        }))
        .unwrap();
        a.validate().unwrap();
        let n = a.normalized();
        assert_eq!(n.targets.len(), 2);
        assert_eq!(n.targets[0].id, "a");
        assert_eq!(n.targets[1].rev, 0);
        let dup: Result<(), String> = SanSpec {
            targets: vec![a.targets[0].clone(), a.targets[0].clone()],
            ..Default::default()
        }
        .validate();
        assert!(dup.is_err());
    }

    #[test]
    fn iqn_validation() {
        assert!(valid_iqn("iqn.2003-01.org.linux-iscsi.target:sn.123"));
        assert!(valid_iqn("iqn.2024-11.example.com:lun-store"));
        assert!(valid_iqn("eui.0123456789ABCDEF"));
        assert!(!valid_iqn("iqn.2003.org.x"));
        assert!(!valid_iqn("iqn.2003-01.org x"));
        assert!(!valid_iqn("nqn.2014-08.org.nvmexpress:x"));
        assert!(!valid_iqn("eui.123"));
    }

    #[test]
    fn target_validation_normalises_portals() {
        let mut t: IscsiTargetSpec = serde_json::from_value(serde_json::json!({
            "id": "san1",
            "iqn": "iqn.2003-01.org.linux-iscsi.t:sn.1",
            "portals": ["10.0.0.1", "10.0.0.1:3260", "[fd00::1]:3261"]
        }))
        .unwrap();
        t.validate().unwrap();
        assert_eq!(t.portals, vec!["10.0.0.1:3260", "[fd00::1]:3261"]);
        assert!(t.use_advertised_portals && t.enabled && t.startup_automatic);
        assert_eq!(t.replacement_timeout_secs, 5);
        t.portals.clear();
        assert!(t.validate().is_err());
    }

    #[test]
    fn no_path_retry_and_failback_round_trip() {
        let s: MultipathSettings = serde_json::from_value(serde_json::json!({
            "no_path_retry": "queue", "failback": 30
        }))
        .unwrap();
        assert_eq!(s.no_path_retry, NoPathRetry::Queue);
        assert_eq!(s.failback, Failback::Seconds(30));
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["no_path_retry"], "queue");
        assert_eq!(v["failback"], "30");
        let d = MultipathSettings::default();
        assert_eq!(d.no_path_retry, NoPathRetry::Retries(12));
        assert_eq!(d.config_path, "/etc/multipath/conf.d/horcrux.conf");
        assert!(d.validate().is_ok());
        assert!(serde_json::from_value::<MultipathSettings>(
            serde_json::json!({"no_path_retry": "sometimes"})
        )
        .is_err());
    }

    #[test]
    fn settings_refuse_the_main_config_file() {
        let s = MultipathSettings {
            config_path: "/etc/multipath.conf".into(),
            ..Default::default()
        };
        assert!(s.validate().is_err());
        let s = MultipathSettings {
            config_path: "relative.conf".into(),
            ..Default::default()
        };
        assert!(s.validate().is_err());
    }

    #[test]
    fn blacklist_rules_cannot_inject_config() {
        let s = MultipathSettings {
            blacklist: vec![DeviceMatch::Devnode {
                devnode: "^sda\"\n}\ndefaults {".into(),
            }],
            ..Default::default()
        };
        assert!(s.validate().is_err());
    }

    #[test]
    fn shared_lvm_device_paths() {
        let mut s = SharedLvmSpec {
            id: "san-lvm".into(),
            vg_name: "vg_san".into(),
            device: "36001405abcdef".into(),
            lock_type: LockType::Sanlock,
            wipe_removed: false,
            discard_removed: true,
            snapshot_as_volume_chain: false,
            nodes: vec![],
            rev: 0,
            deleted: false,
        };
        s.validate().unwrap();
        assert_eq!(s.device_path().unwrap(), "/dev/mapper/36001405abcdef");
        s.device = "/dev/disk/by-id/scsi-36001405abcdef".into();
        assert_eq!(
            s.device_path().unwrap(),
            "/dev/disk/by-id/scsi-36001405abcdef"
        );
        s.device = "../etc".into();
        assert!(s.validate().is_err());
        s.device = "x".into();
        s.vg_name = "-bad".into();
        assert!(s.validate().is_err());
    }

    #[test]
    fn host_ids_are_unique_and_stable() {
        let c = |n: &str, id: u16| HostId {
            node: n.into(),
            host_id: id,
            rev: 0,
        };
        assert_eq!(pick_host_id("a", &[]), 1);
        assert_eq!(pick_host_id("b", &[c("a", 1)]), 2);
        // keeps its own unique claim
        assert_eq!(pick_host_id("b", &[c("a", 1), c("b", 7)]), 7);
        // a clash: the name that sorts first keeps it
        assert_eq!(pick_host_id("a", &[c("a", 3), c("b", 3)]), 3);
        assert_eq!(pick_host_id("b", &[c("a", 3), c("b", 3)]), 1);
    }
}
