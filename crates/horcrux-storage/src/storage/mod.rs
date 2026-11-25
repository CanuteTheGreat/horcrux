pub mod btrfs;
pub mod ceph;
pub mod cephfs;
pub mod cifs;
pub mod directory;
pub mod glusterfs;
pub mod iscsi;
pub mod lvm;
pub mod lvm_chain;
pub mod migration;
pub mod nfs;
pub mod plugin;
mod plugin_pools;
pub mod s3;
pub mod thin_provision;
/// Storage backend management
/// Supports ZFS, Ceph, LVM, iSCSI, directory-based, GlusterFS, BtrFS, and S3 storage
pub mod zfs;

pub use plugin_pools::VolumeSnapshot;

use crate::db::config_store::{self, kinds, ConfigStore};
use crate::db::Database;
use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Storage backend type. On the wire a lower-case name (`zfs`, `nfs`, ...)
/// or `plugin:<id>` for a third-party storage plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageType {
    Zfs,
    Ceph,
    /// A mounted CephFS (file-based, shared between the nodes).
    CephFs,
    Lvm,
    Iscsi,
    Directory,
    Cifs,
    Nfs,
    GlusterFs,
    BtrFs,
    S3,
    /// A pool served by the storage plugin with this id (docs/storage-plugins.md).
    Plugin(String),
}

impl StorageType {
    /// Wire name: `zfs`, `directory`, ..., `plugin:<id>`.
    pub fn name(&self) -> String {
        match self {
            StorageType::Zfs => "zfs".into(),
            StorageType::Ceph => "ceph".into(),
            StorageType::CephFs => "cephfs".into(),
            StorageType::Lvm => "lvm".into(),
            StorageType::Iscsi => "iscsi".into(),
            StorageType::Directory => "directory".into(),
            StorageType::Cifs => "cifs".into(),
            StorageType::Nfs => "nfs".into(),
            StorageType::GlusterFs => "glusterfs".into(),
            StorageType::BtrFs => "btrfs".into(),
            StorageType::S3 => "s3".into(),
            StorageType::Plugin(id) => format!("plugin:{id}"),
        }
    }

    /// Parse a wire name (built-in names case-insensitively, `dir` for
    /// `directory`; `plugin:<id>` with a valid plugin id).
    pub fn from_name(s: &str) -> Option<Self> {
        if let Some(id) = s.strip_prefix("plugin:") {
            return plugin::sdk::valid_plugin_id(id).then(|| StorageType::Plugin(id.to_string()));
        }
        Some(match s.to_lowercase().as_str() {
            "zfs" => StorageType::Zfs,
            "ceph" => StorageType::Ceph,
            "cephfs" => StorageType::CephFs,
            "lvm" => StorageType::Lvm,
            "iscsi" => StorageType::Iscsi,
            "directory" | "dir" => StorageType::Directory,
            "cifs" => StorageType::Cifs,
            "nfs" => StorageType::Nfs,
            "glusterfs" => StorageType::GlusterFs,
            "btrfs" => StorageType::BtrFs,
            "s3" => StorageType::S3,
            _ => return None,
        })
    }

    /// The plugin id of a plugin pool.
    pub fn plugin_id(&self) -> Option<&str> {
        match self {
            StorageType::Plugin(id) => Some(id),
            _ => None,
        }
    }
}

impl Serialize for StorageType {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.name())
    }
}

impl<'de> Deserialize<'de> for StorageType {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        StorageType::from_name(&s)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown storage type {s:?}")))
    }
}

/// Storage pool configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoragePool {
    pub id: String,
    pub name: String,
    pub storage_type: StorageType,
    pub path: String,
    pub available: u64, // Available space in GB
    pub total: u64,     // Total space in GB
    pub enabled: bool,
    /// Remote export a network file system pool was added with
    /// (`server:/export`). Such a pool's `path` is the local mount point
    /// the API keeps it mounted on, so volumes, capacity and listings use
    /// the same directory code as a directory pool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// LVM pools only: shared-VG locking and what happens to removed
    /// volumes (absent = defaults: local VG, plain `lvremove`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lvm: Option<lvm::LvmPoolOptions>,
    /// Plugin pools: the validated, non-secret config (secret fields are kept
    /// in the secrets store, never here).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_config: Option<std::collections::BTreeMap<String, serde_json::Value>>,
}

/// (total, available) GB of the filesystem holding `path`, via statvfs(2).
pub fn fs_capacity_gb(path: &str) -> Option<(u64, u64)> {
    let st = nix::sys::statvfs::statvfs(path).ok()?;
    let frag = st.fragment_size() as u64;
    let gb = 1024 * 1024 * 1024;
    Some((
        st.blocks() as u64 * frag / gb,
        st.blocks_available() as u64 * frag / gb,
    ))
}

/// (total, available) GB of a ZFS dataset (`zfs list -Hp -o used,avail`): a
/// ZFS pool's path is a dataset NAME (`tank`, `tank/vms`), not a filesystem
/// path, so statvfs on it fails and the pool used to report 0 GB (every zvol
/// create then failed the space check).
pub fn zfs_capacity_gb(dataset: &str) -> Option<(u64, u64)> {
    // HORCRUX_ZFS_BIN lets tests (and unusual installs) point at another zfs binary.
    let bin = std::env::var_os("HORCRUX_ZFS_BIN").unwrap_or_else(|| "zfs".into());
    let out = std::process::Command::new(bin)
        .args(["list", "-Hp", "-o", "used,avail", "--"])
        .arg(dataset)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_zfs_capacity(&String::from_utf8_lossy(&out.stdout))
}

/// Parse one `zfs list -Hp -o used,avail` line (bytes) into (total, available) GB.
pub fn parse_zfs_capacity(line: &str) -> Option<(u64, u64)> {
    let mut f = line.split_whitespace();
    let used: u64 = f.next()?.parse().ok()?;
    let avail: u64 = f.next()?.parse().ok()?;
    let gb = 1024 * 1024 * 1024;
    Some(((used + avail) / gb, avail / gb))
}

/// (total, available) GB of a Ceph pool, from `ceph df --format json`: a Ceph
/// pool's path is the RADOS pool NAME, so statvfs on it fails and the pool used
/// to report 0 GB (every `rbd create` was then refused by the space check).
/// `max_avail` is what the pool can still store (replication and the fullest
/// OSD already taken into account); total = stored raw bytes in the pool + that.
pub fn ceph_capacity_gb(pool: &str) -> Option<(u64, u64)> {
    // HORCRUX_CEPH_BIN lets tests (and unusual installs) point at another ceph binary.
    let bin = std::env::var_os("HORCRUX_CEPH_BIN").unwrap_or_else(|| "ceph".into());
    let out = std::process::Command::new(bin)
        .args(["--connect-timeout", "5", "df", "--format", "json"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_ceph_df(&String::from_utf8_lossy(&out.stdout), pool)
}

/// Parse `ceph df --format json` into (total, available) GB of `pool`.
pub fn parse_ceph_df(json: &str, pool: &str) -> Option<(u64, u64)> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let p = v
        .get("pools")?
        .as_array()?
        .iter()
        .find(|p| p.get("name").and_then(|n| n.as_str()) == Some(pool))?;
    let st = p.get("stats")?;
    let avail = st.get("max_avail")?.as_u64()?;
    let used = st.get("stored").and_then(|u| u.as_u64()).unwrap_or(0);
    let gb = 1024 * 1024 * 1024;
    Some(((used + avail) / gb, avail / gb))
}

/// (total, available) GB of an LVM volume group (`vgs --units b -o
/// vg_size,vg_free`): an LVM pool's path is the VG NAME, so statvfs on it
/// fails and the pool reported 0 GB (every `lvcreate` was then refused by the
/// space check, shared VGs on a SAN LUN included).
pub fn lvm_capacity_gb(vg: &str) -> Option<(u64, u64)> {
    // HORCRUX_VGS_BIN lets tests (and unusual installs) point at another vgs binary.
    let bin = std::env::var_os("HORCRUX_VGS_BIN").unwrap_or_else(|| "vgs".into());
    let out = std::process::Command::new(bin)
        .args([
            "--noheadings",
            "--units",
            "b",
            "--nosuffix",
            "-o",
            "vg_size,vg_free",
            "--",
        ])
        .arg(vg)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_vgs_capacity(&String::from_utf8_lossy(&out.stdout))
}

/// Parse one `vgs --units b --nosuffix -o vg_size,vg_free` line into (total, available) GB.
pub fn parse_vgs_capacity(text: &str) -> Option<(u64, u64)> {
    let line = text.lines().find(|l| !l.trim().is_empty())?;
    let mut f = line.split_whitespace();
    let size: u64 = f.next()?.parse().ok()?;
    let free: u64 = f.next()?.parse().ok()?;
    let gb = 1024 * 1024 * 1024;
    Some((size / gb, free / gb))
}

/// How the manager measures a pool's filesystem: `fs_capacity_gb`
/// (tests substitute a fake to pin or move the numbers).
pub type CapacityProbe = fn(&str) -> Option<(u64, u64)>;

/// A statvfs on a hung NFS/CIFS/GlusterFS mount can block for minutes:
/// past this the pool keeps its last known numbers.
const CAPACITY_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
const CEPH_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

/// Pools whose capacity is the filesystem's (statvfs on the pool path).
fn measured_by_statvfs(t: &StorageType) -> bool {
    matches!(
        t,
        StorageType::Directory
            | StorageType::Nfs
            | StorageType::Cifs
            | StorageType::GlusterFs
            | StorageType::CephFs
            | StorageType::BtrFs
    )
}

/// Pools measured by a probe: statvfs for filesystem pools, `zfs list` for ZFS,
/// `ceph df` for Ceph RBD, `vgs` for LVM.
fn measured_by_probe(t: &StorageType) -> bool {
    measured_by_statvfs(t) || matches!(t, StorageType::Zfs | StorageType::Ceph | StorageType::Lvm)
}

/// Defence in depth: every backend re-checks each name it joins onto a pool
/// path with the one shared validator (`validation::validate_volume_name`),
/// so a volume or snapshot can never be created/deleted outside its pool
/// even if a caller skipped the API-layer check.
pub(crate) fn check_names(names: &[&str]) -> Result<()> {
    for name in names {
        crate::validation::validate_volume_name(name)?;
    }
    Ok(())
}

/// A volume in a storage pool (`GET /api/storage/pools/:id/volumes`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeInfo {
    pub name: String,
    /// What a VM disk would point at: a file, `/dev/...` or `rbd:pool/image`.
    pub path: String,
    pub format: String,
    /// Virtual size in bytes.
    pub size_bytes: u64,
    /// Bytes actually allocated on the pool (0 when the backend can't tell).
    pub allocated_bytes: u64,
}

/// Virtual size from a qcow2 header (magic `QFI\xfb`, size at bytes 24..32).
fn qcow2_virtual_size(path: &Path) -> Option<u64> {
    use std::io::Read;
    let mut hdr = [0u8; 32];
    std::fs::File::open(path).ok()?.read_exact(&mut hdr).ok()?;
    if hdr[..4] != *b"QFI\xfb" {
        return None;
    }
    Some(u64::from_be_bytes(hdr[24..32].try_into().ok()?))
}

fn file_volume_info(name: &str, path: &Path) -> Option<VolumeInfo> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let (format, size) = match qcow2_virtual_size(path) {
        Some(size) => ("qcow2", size),
        None => ("raw", meta.len()),
    };
    Some(VolumeInfo {
        name: name.to_string(),
        path: path.to_string_lossy().to_string(),
        format: format.to_string(),
        size_bytes: size,
        allocated_bytes: meta.blocks() * 512,
    })
}

/// Volumes from `zfs list -H -p -t volume -o name,volsize,used` output
/// (tab-separated; lines with fewer than three fields are skipped).
fn parse_zfs_volumes(text: &str) -> Vec<VolumeInfo> {
    let mut out = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 3 {
            continue;
        }
        let name = f[0].rsplit('/').next().unwrap_or(f[0]);
        out.push(VolumeInfo {
            name: name.to_string(),
            path: format!("/dev/zvol/{}", f[0]),
            format: "zvol".into(),
            size_bytes: f[1].parse().unwrap_or(0),
            allocated_bytes: f[2].parse().unwrap_or(0),
        });
    }
    out
}

/// Volumes from `lvs --noheadings --units b --nosuffix --separator '|'
/// -o lv_name,lv_size <vg>` output (lines without a name and a size are
/// skipped).
fn parse_lvs_volumes(text: &str, vg: &str) -> Vec<VolumeInfo> {
    let mut out = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.trim().split('|').collect();
        if f.len() < 2 || f[0].is_empty() {
            continue;
        }
        let size = f[1].trim().parse().unwrap_or(0);
        out.push(VolumeInfo {
            name: f[0].to_string(),
            path: format!("/dev/{}/{}", vg, f[0]),
            format: "lv".into(),
            size_bytes: size,
            allocated_bytes: size,
        });
    }
    out
}

/// Volume-chain options of an LVM pool that keeps snapshots that way.
fn chain_opts(pool: &StoragePool) -> Option<&lvm::LvmPoolOptions> {
    pool.lvm
        .as_ref()
        .filter(|o| pool.storage_type == StorageType::Lvm && o.snapshot_as_volume_chain)
}

fn unsupported(op: &str, t: &StorageType) -> horcrux_common::Error {
    horcrux_common::Error::InvalidConfig(format!("{op} is not supported on {t:?} storage pools"))
}

async fn run_tool(program: &str, args: &[&str]) -> Result<String> {
    let out = tokio::process::Command::new(program)
        .args(args)
        .output()
        .await
        .map_err(|e| horcrux_common::Error::System(format!("Failed to run {program}: {e}")))?;
    if !out.status.success() {
        return Err(horcrux_common::Error::System(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Storage manager
pub struct StorageManager {
    pools: Arc<RwLock<HashMap<String, StoragePool>>>,
    zfs: zfs::ZfsManager,
    ceph: ceph::CephManager,
    lvm: lvm::LvmManager,
    directory: directory::DirectoryManager,
    iscsi: iscsi::IscsiManager,
    cifs: cifs::CifsManager,
    nfs: nfs::NfsManager,
    glusterfs: glusterfs::GlusterFsManager,
    cephfs: cephfs::CephFsManager,
    btrfs: btrfs::BtrFsManager,
    s3: s3::S3Manager,
    // Track next LUN ID for each iSCSI target
    iscsi_lun_counters: Arc<RwLock<HashMap<String, u32>>>,
    /// Durable copy of `pools`; `None` for a purely in-memory manager.
    store: Option<ConfigStore>,
    /// Measures filesystem-backed pools (statvfs).
    capacity_probe: CapacityProbe,
    /// Same for ZFS pools (dataset names, measured with `zfs list`).
    zfs_probe: CapacityProbe,
    ceph_probe: CapacityProbe,
    lvm_probe: CapacityProbe,
    /// Pools whose last measurement hasn't returned yet (hung mount): not
    /// probed again until it does, so blocked threads don't pile up.
    measuring: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    /// Third-party storage plugins (plugin pools).
    plugins: Arc<plugin::PluginHost>,
    /// Secret config fields of plugin pools, by pool id.
    plugin_secrets: Arc<RwLock<HashMap<String, std::collections::BTreeMap<String, String>>>>,
}

/// Combined size of an iSCSI target's LUNs in whole GB.
fn luns_total_gb(luns: &[iscsi::IscsiLun]) -> u64 {
    let bytes: u64 = luns.iter().map(|l| l.size_bytes).sum();
    bytes / (1024 * 1024 * 1024)
}

impl Default for StorageManager {
    fn default() -> Self {
        Self::new()
    }
}

impl StorageManager {
    pub fn new() -> Self {
        Self {
            pools: Arc::new(RwLock::new(HashMap::new())),
            zfs: zfs::ZfsManager::new(),
            ceph: ceph::CephManager::new(),
            lvm: lvm::LvmManager::new(),
            directory: directory::DirectoryManager::new(),
            iscsi: iscsi::IscsiManager::new(),
            cifs: cifs::CifsManager::new(),
            nfs: nfs::NfsManager::new(),
            glusterfs: glusterfs::GlusterFsManager::new(),
            cephfs: cephfs::CephFsManager::new(),
            btrfs: btrfs::BtrFsManager::new(),
            s3: s3::S3Manager::new(),
            iscsi_lun_counters: Arc::new(RwLock::new(HashMap::new())),
            store: None,
            capacity_probe: fs_capacity_gb,
            zfs_probe: zfs_capacity_gb,
            ceph_probe: ceph_capacity_gb,
            lvm_probe: lvm_capacity_gb,
            measuring: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            plugins: Arc::new(plugin::PluginHost::new()),
            plugin_secrets: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Use a plugin host with fixed settings (tests).
    pub fn with_plugin_host(mut self, host: plugin::PluginHost) -> Self {
        self.plugins = Arc::new(host);
        self
    }

    /// Storage manager backed by the database: loads the persisted pool
    /// definitions and writes every add/remove through, so pools survive a
    /// restart. Stored pools are not re-validated (a remote mount may be
    /// briefly unavailable at boot); filesystem capacity is re-measured.
    pub async fn with_database(db: Arc<Database>) -> Result<Self> {
        let store = ConfigStore::new(&db);
        let mut mgr = Self::new();
        {
            let mut pools = mgr.pools.write().await;
            for mut pool in store
                .load_values::<StoragePool>(kinds::STORAGE_POOL)
                .await?
            {
                if pool.storage_type.plugin_id().is_some() {
                    // activated below, once the secrets are loaded
                } else if let Some(src) = pool.source.clone() {
                    // Remount in the background (the server may be briefly
                    // down at boot); a pool that isn't mounted is never
                    // measured: statvfs of the empty mount point would
                    // report the local disk.
                    let dir = pool.path.clone();
                    tokio::spawn(async move {
                        if let Ok(cfg) = nfs::NfsConfig::parse(&src) {
                            let _ = tokio::time::timeout(
                                std::time::Duration::from_secs(60),
                                nfs::NfsManager::new().ensure_mounted(&cfg, &dir),
                            )
                            .await;
                        }
                    });
                } else {
                    if pool.storage_type == StorageType::Iscsi {
                        // Sessions don't survive a reboot: log in again.
                        if let Ok(t) = iscsi::IscsiTarget::parse(&pool.path) {
                            tokio::spawn(async move {
                                let _ = tokio::time::timeout(
                                    std::time::Duration::from_secs(60),
                                    iscsi::IscsiManager::new().login_target(&t),
                                )
                                .await;
                            });
                        }
                    }
                    Self::refresh_capacity(mgr.probe_for(&pool.storage_type), &mut pool);
                }
                pools.insert(pool.id.clone(), pool);
            }
            tracing::info!("Loaded {} storage pool(s) from database", pools.len());
        }
        mgr.store = Some(store);
        mgr.load_plugin_secrets().await?;
        mgr.spawn_plugin_activation().await;
        Ok(mgr)
    }

    /// Use `probe` instead of statvfs to measure pools (tests).
    pub fn with_capacity_probe(mut self, probe: CapacityProbe) -> Self {
        self.capacity_probe = probe;
        self.zfs_probe = probe;
        self.ceph_probe = probe;
        self.lvm_probe = probe;
        self
    }

    /// The probe that measures `t`'s capacity.
    fn probe_for(&self, t: &StorageType) -> CapacityProbe {
        if matches!(t, StorageType::Zfs) {
            self.zfs_probe
        } else if matches!(t, StorageType::Ceph) {
            self.ceph_probe
        } else if matches!(t, StorageType::Lvm) {
            self.lvm_probe
        } else {
            self.capacity_probe
        }
    }

    /// Measure real capacity for filesystem-backed pools.
    fn refresh_capacity(probe: CapacityProbe, pool: &mut StoragePool) {
        if measured_by_probe(&pool.storage_type) {
            if let Some((total, available)) = probe(&pool.path) {
                pool.total = total;
                pool.available = available;
            }
        }
    }

    /// Re-measure filesystem-backed pools (all, or just `only`) so lists
    /// and space checks see the space actually free now - capacity used to
    /// be measured only at pool add and API start, so a pool filled since
    /// still showed (and accepted volumes against) its old free space.
    /// statvfs runs off the async workers with a timeout: a pool whose
    /// mount doesn't answer keeps its last numbers.
    async fn refresh_pools(&self, only: Option<&str>) {
        let targets: Vec<(String, String, CapacityProbe, std::time::Duration)> = {
            let pools = self.pools.read().await;
            pools
                .values()
                .filter(|p| measured_by_probe(&p.storage_type))
                .filter(|p| only.is_none_or(|id| p.id == id))
                .filter(|p| p.source.is_none() || nfs::is_mounted(&p.path))
                .map(|p| {
                    (
                        p.id.clone(),
                        p.path.clone(),
                        self.probe_for(&p.storage_type),
                        // `ceph df` starts a client and asks the monitors: slower than statvfs
                        if p.storage_type == StorageType::Ceph {
                            CEPH_PROBE_TIMEOUT
                        } else {
                            CAPACITY_PROBE_TIMEOUT
                        },
                    )
                })
                .collect()
        };
        let wait = targets
            .iter()
            .map(|t| t.3)
            .max()
            .unwrap_or(CAPACITY_PROBE_TIMEOUT);
        let deadline = tokio::time::Instant::now() + wait;
        let mut running = Vec::new();
        for (id, path, probe, _) in targets {
            {
                let mut busy = self.measuring.lock().unwrap_or_else(|e| e.into_inner());
                if !busy.insert(id.clone()) {
                    continue; // the previous measurement is still stuck
                }
            }
            let measuring = self.measuring.clone();
            let key = id.clone();
            let task = tokio::task::spawn_blocking(move || {
                let r = probe(&path);
                measuring
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&key);
                r
            });
            running.push((id, task));
        }
        for (id, task) in running {
            match tokio::time::timeout_at(deadline, task).await {
                Ok(Ok(Some((total, available)))) => {
                    if let Some(p) = self.pools.write().await.get_mut(&id) {
                        p.total = total;
                        p.available = available;
                    }
                }
                Ok(_) => {}
                Err(_) => tracing::warn!(
                    "Storage pool {id}: capacity check timed out (mount not answering); \
                     keeping the last known numbers"
                ),
            }
        }
    }

    /// List all storage pools (filesystem pools freshly measured)
    pub async fn list_pools(&self) -> Vec<StoragePool> {
        self.refresh_pools(None).await;
        self.refresh_plugin_pools(None).await;
        let pools = self.pools.read().await;
        pools.values().cloned().collect()
    }

    /// Get a specific storage pool (freshly measured)
    pub async fn get_pool(&self, id: &str) -> Result<StoragePool> {
        self.refresh_pools(Some(id)).await;
        self.refresh_plugin_pools(Some(id)).await;
        let pools = self.pools.read().await;
        pools
            .get(id)
            .cloned()
            .ok_or_else(|| horcrux_common::Error::System(format!("Storage pool {} not found", id)))
    }

    /// Add a storage pool
    pub async fn add_pool(&self, pool: StoragePool) -> Result<StoragePool> {
        let mut pools = self.pools.write().await;

        if pools.contains_key(&pool.id) {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Storage pool {} already exists",
                pool.id
            )));
        }

        if let Some(o) = &pool.lvm {
            if pool.storage_type != StorageType::Lvm {
                return Err(horcrux_common::Error::InvalidConfig(
                    "lvm options only apply to LVM pools".into(),
                ));
            }
            o.validate()?;
            let tools = crate::san::tools::ToolStatus::probe();
            if o.wipe_removed || o.discard_removed {
                crate::san::tools::Need::Discard.check(&tools)?;
            }
            if o.shared {
                crate::san::tools::Need::SharedLvm.check(&tools)?;
                let t = std::time::Duration::from_secs(o.command_timeout_secs.into());
                match crate::san::lvm_shared::vg_lock_type(&pool.path, t).await {
                    Ok(Some(l)) if l == "sanlock" => {}
                    Ok(Some(l)) => {
                        return Err(horcrux_common::Error::InvalidConfig(format!(
                            "volume group {} is not shared (lock type {l}); create it through /api/san/lvm",
                            pool.path
                        )))
                    }
                    Ok(None) => {
                        return Err(horcrux_common::Error::NotFound(format!(
                            "volume group {} is not visible on this node",
                            pool.path
                        )))
                    }
                    Err(e) => return Err(horcrux_common::Error::System(e)),
                }
            }
        }

        // Validate the pool based on type
        match pool.storage_type {
            StorageType::Zfs => self.zfs.validate_pool(&pool).await?,
            StorageType::Ceph => self.ceph.validate_pool(&pool).await?,
            StorageType::Lvm => self.lvm.validate_pool(&pool).await?,
            StorageType::Directory => self.directory.validate_pool(&pool).await?,
            StorageType::Iscsi => self.iscsi.validate_pool(&pool).await?,
            StorageType::Cifs => self.cifs.validate_pool(&pool).await?,
            StorageType::Nfs => self.nfs.validate_pool(&pool).await?,
            StorageType::GlusterFs => self.glusterfs.validate_pool(&pool).await?,
            StorageType::CephFs => self.cephfs.validate_pool(&pool).await?,
            StorageType::BtrFs => self.btrfs.validate_pool(&pool).await?,
            StorageType::S3 => {
                // S3 validation: verify path contains valid bucket configuration
                // Format expected: "s3://bucket-name" or "s3://endpoint/bucket-name"
                if pool.path.is_empty() {
                    return Err(horcrux_common::Error::InvalidConfig(
                        "S3 path cannot be empty".to_string(),
                    ));
                }

                if !pool.path.starts_with("s3://") {
                    return Err(horcrux_common::Error::InvalidConfig(format!(
                        "S3 path must start with 's3://', got: {}",
                        pool.path
                    )));
                }

                let bucket_part = pool.path.strip_prefix("s3://").unwrap();
                if bucket_part.is_empty() {
                    return Err(horcrux_common::Error::InvalidConfig(
                        "S3 path must specify bucket name after 's3://'".to_string(),
                    ));
                }

                // Validate bucket name format (basic check)
                let bucket_name = bucket_part.split('/').next().unwrap_or("");
                if bucket_name.len() < 3 || bucket_name.len() > 63 {
                    return Err(horcrux_common::Error::InvalidConfig(
                        "S3 bucket name must be between 3 and 63 characters".to_string(),
                    ));
                }

                // Delegate detailed validation to S3 manager
                self.s3.validate_pool(&pool).await?;
            }
            StorageType::Plugin(_) => {
                return Err(horcrux_common::Error::InvalidConfig(
                    "plugin pools are added with their plugin config (add_plugin_pool)".into(),
                ))
            }
        }

        // Filesystem-backed pools: measure real capacity. The API used to
        // store available=0/total=0 ("will be updated by backend", never
        // was), so every volume create in a directory pool failed with
        // "Not enough space ... Available: 0GB".
        let mut pool = pool;
        if pool.storage_type == StorageType::Nfs && pool.source.is_none() {
            // `server:/export` is not a directory: keep the export mounted
            // under the data dir and let the pool's path be that mount.
            let cfg = nfs::NfsConfig::parse(&pool.path)?;
            let dir = nfs::mount_dir(&pool.id);
            self.nfs.ensure_mounted(&cfg, &dir).await?;
            pool.source = Some(pool.path.clone());
            pool.path = dir;
        }
        if pool.storage_type == StorageType::Iscsi {
            // Log in so the target's LUNs are block devices, and report
            // their real size (capacity was 0 and the pool had no disks).
            let target = iscsi::IscsiTarget::parse(&pool.path)?;
            let luns = self.iscsi.attach(&target).await?;
            pool.total = luns_total_gb(&luns);
            pool.available = pool.total;
        }
        Self::refresh_capacity(self.probe_for(&pool.storage_type), &mut pool);

        config_store::put_opt(&self.store, kinds::STORAGE_POOL, &pool.id, &pool).await?;
        let pool_clone = pool.clone();
        pools.insert(pool.id.clone(), pool);
        Ok(pool_clone)
    }

    /// Remove a storage pool
    pub async fn remove_pool(&self, id: &str) -> Result<()> {
        let mut pools = self.pools.write().await;

        if !pools.contains_key(id) {
            return Err(horcrux_common::Error::System(format!(
                "Storage pool {} not found",
                id
            )));
        }
        config_store::delete_opt(&self.store, kinds::STORAGE_POOL, id).await?;
        if let Some(p) = pools.remove(id) {
            if p.storage_type.plugin_id().is_some() {
                drop(pools);
                self.plugin_remove(&p).await;
                return Ok(());
            }
            if p.source.is_some() && p.storage_type == StorageType::Nfs {
                if let Err(e) = self.nfs.unmount_export(&p.path).await {
                    tracing::warn!("Storage pool {}: unmounting {}: {}", id, p.path, e);
                } else {
                    let _ = tokio::fs::remove_dir(&p.path).await;
                }
            }
            if p.storage_type == StorageType::Iscsi {
                if let Ok(t) = iscsi::IscsiTarget::parse(&p.path) {
                    if let Err(e) = self.iscsi.detach(&t).await {
                        tracing::warn!("Storage pool {}: iSCSI logout: {}", id, e);
                    }
                }
            }
        }

        Ok(())
    }

    /// Create a volume in a storage pool
    pub async fn create_volume(
        &self,
        pool_id: &str,
        volume_name: &str,
        size_gb: u64,
    ) -> Result<String> {
        check_names(&[volume_name])?;
        if let Some(p) = self.pools.read().await.get(pool_id).cloned() {
            if let Some(id) = p.storage_type.plugin_id() {
                self.refresh_plugin_pools(Some(pool_id)).await;
                let p = self.get_pool_or_404(pool_id).await?;
                return self
                    .plugin_create_volume(id, &p, volume_name, size_gb)
                    .await;
            }
        }
        // A network pool whose mount was lost (reboot, server restart)
        // is mounted again before anything is measured or written.
        let remote = {
            let pools = self.pools.read().await;
            pools
                .get(pool_id)
                .and_then(|p| p.source.clone().map(|s| (s, p.path.clone())))
        };
        if let Some((src, dir)) = remote {
            if !nfs::is_mounted(&dir) {
                let cfg = nfs::NfsConfig::parse(&src)?;
                self.nfs.ensure_mounted(&cfg, &dir).await?;
            }
        }
        // The space check below must see the space free now.
        self.refresh_pools(Some(pool_id)).await;
        let pools = self.pools.read().await;
        let pool = pools.get(pool_id).ok_or_else(|| {
            horcrux_common::Error::NotFound(format!("Storage pool {} not found", pool_id))
        })?;

        if !pool.enabled {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Storage pool {} is disabled",
                pool_id
            )));
        }

        if size_gb == 0 {
            return Err(horcrux_common::Error::InvalidConfig(
                "Volume size must be at least 1 GB".to_string(),
            ));
        }

        if size_gb > pool.available {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Not enough space in pool {}. Requested: {}GB, Available: {}GB",
                pool_id, size_gb, pool.available
            )));
        }

        // `qemu-img create` silently truncates an existing image: never let
        // a create overwrite a volume (possibly a VM's disk).
        if let Some(path) = Self::file_volume_path(pool, volume_name)? {
            if path.exists() {
                return Err(horcrux_common::Error::AlreadyExists(format!(
                    "Volume {} already exists in pool {}",
                    volume_name, pool_id
                )));
            }
        }

        let volume_path = match pool.storage_type {
            StorageType::Zfs => {
                self.zfs
                    .create_volume(&pool.path, volume_name, size_gb)
                    .await?
            }
            StorageType::Ceph => {
                self.ceph
                    .create_volume(&pool.path, volume_name, size_gb)
                    .await?
            }
            StorageType::Lvm => match chain_opts(pool) {
                Some(o) => {
                    self.lvm
                        .chain_create(&pool.path, volume_name, size_gb, o)
                        .await?
                }
                None => {
                    self.lvm
                        .create_volume(&pool.path, volume_name, size_gb)
                        .await?
                }
            },
            StorageType::Directory => {
                self.directory
                    .create_volume(&pool.path, volume_name, size_gb)
                    .await?
            }
            StorageType::Iscsi => {
                // For iSCSI, parse target and create volume
                let target = iscsi::IscsiTarget::parse(&pool.path)?;

                // Get and increment LUN ID for this target
                let target_key = format!("{}@{}", target.portal, target.iqn);
                let mut lun_counters = self.iscsi_lun_counters.write().await;
                let lun_id = lun_counters.entry(target_key).or_insert(0);
                let current_lun = *lun_id;
                *lun_id += 1;
                drop(lun_counters);

                self.iscsi
                    .create_volume(&target, current_lun, size_gb)
                    .await?
            }
            StorageType::Cifs => {
                // For CIFS, create file in mounted share
                self.cifs
                    .create_volume(&pool.path, volume_name, size_gb)
                    .await?
            }
            StorageType::Nfs => {
                // For NFS, create file in NFS mount
                self.nfs
                    .create_volume(&pool.path, volume_name, size_gb)
                    .await?
            }
            StorageType::GlusterFs => {
                // For GlusterFS, create file in gluster volume
                self.glusterfs
                    .create_volume(&pool.path, volume_name, size_gb)
                    .await?
            }
            StorageType::CephFs => {
                // For CephFS, create the file in the mounted file system
                self.cephfs
                    .create_volume(&pool.path, volume_name, size_gb)
                    .await?
            }
            StorageType::BtrFs => {
                // For BtrFS, create subvolume
                self.btrfs
                    .create_volume(&pool.path, volume_name, size_gb)
                    .await?
            }
            StorageType::S3 => {
                // S3 doesn't support block volumes, use for backup storage instead
                return Err(horcrux_common::Error::InvalidConfig(
                    "S3 storage is for backups/objects only, not for VM volumes".to_string(),
                ));
            }
            // dispatched to the plugin above
            ref t @ StorageType::Plugin(_) => return Err(unsupported("This operation", t)),
        };

        Ok(volume_path)
    }

    /// File backing `name` in a file-based pool (`None` for block/object
    /// pools). The name is validated first, so the result is always a
    /// direct child of the pool directory (btrfs: of its `vm-<name>`
    /// subvolume).
    pub fn file_volume_path(pool: &StoragePool, name: &str) -> Result<Option<PathBuf>> {
        check_names(&[name])?;
        let base = Path::new(&pool.path);
        Ok(match pool.storage_type {
            StorageType::Directory
            | StorageType::Nfs
            | StorageType::Cifs
            | StorageType::GlusterFs
            | StorageType::CephFs => Some(base.join(format!("{name}.qcow2"))),
            StorageType::BtrFs => Some(
                base.join(format!("vm-{name}"))
                    .join(format!("{name}.qcow2")),
            ),
            _ => None,
        })
    }

    /// What a VM disk on volume `name` of `pool` points at.
    pub fn volume_location(pool: &StoragePool, name: &str) -> Result<String> {
        if let Some(p) = Self::file_volume_path(pool, name)? {
            return Ok(p.to_string_lossy().to_string());
        }
        Ok(match pool.storage_type {
            StorageType::Zfs => format!("/dev/zvol/{}/{}", pool.path, name),
            StorageType::Lvm => format!("/dev/{}/{}", pool.path, name),
            StorageType::Ceph => format!("rbd:{}/{}", pool.path, name),
            StorageType::Iscsi => {
                let target = iscsi::IscsiTarget::parse(&pool.path)?;
                let n = iscsi::IscsiTarget::parse_volume_name(name).ok_or_else(|| {
                    horcrux_common::Error::NotFound(format!(
                        "No LUN named {name} (iSCSI volumes are lun0, lun1, ...)"
                    ))
                })?;
                target.lun_path(n)
            }
            ref t => return Err(unsupported("Named volumes", t)),
        })
    }

    /// Volumes currently in a pool.
    pub async fn list_volumes(&self, pool_id: &str) -> Result<Vec<VolumeInfo>> {
        let pool = self.get_pool_or_404(pool_id).await?;
        if let Some(id) = pool.storage_type.plugin_id() {
            return self.plugin_list_volumes(id, &pool).await;
        }
        let mut out = Vec::new();
        match pool.storage_type {
            StorageType::Directory
            | StorageType::Nfs
            | StorageType::Cifs
            | StorageType::GlusterFs
            | StorageType::CephFs => {
                let rd = std::fs::read_dir(&pool.path).map_err(|e| {
                    horcrux_common::Error::System(format!("Cannot read {}: {}", pool.path, e))
                })?;
                for e in rd.flatten() {
                    let file = e.file_name().to_string_lossy().to_string();
                    let Some(name) = file.strip_suffix(".qcow2") else {
                        continue;
                    };
                    if check_names(&[name]).is_err() {
                        continue;
                    }
                    if let Some(v) = file_volume_info(name, &e.path()) {
                        out.push(v);
                    }
                }
            }
            StorageType::BtrFs => {
                if let Ok(rd) = std::fs::read_dir(&pool.path) {
                    for e in rd.flatten() {
                        let dir = e.file_name().to_string_lossy().to_string();
                        let Some(name) = dir.strip_prefix("vm-") else {
                            continue;
                        };
                        if check_names(&[name]).is_err() {
                            continue;
                        }
                        if let Some(v) =
                            file_volume_info(name, &e.path().join(format!("{name}.qcow2")))
                        {
                            out.push(v);
                        }
                    }
                }
            }
            StorageType::Zfs => {
                let text = run_tool(
                    "zfs",
                    &[
                        "list",
                        "-H",
                        "-p",
                        "-t",
                        "volume",
                        "-d",
                        "1",
                        "-o",
                        "name,volsize,used",
                        &pool.path,
                    ],
                )
                .await?;
                out.extend(parse_zfs_volumes(&text));
            }
            StorageType::Lvm => {
                let text = run_tool(
                    "lvs",
                    &[
                        "--noheadings",
                        "--units",
                        "b",
                        "--nosuffix",
                        "--separator",
                        "|",
                        "-o",
                        "lv_name,lv_size",
                        &pool.path,
                    ],
                )
                .await?;
                let mut v = parse_lvs_volumes(&text, &pool.path);
                if chain_opts(&pool).is_some() {
                    // Snapshot LVs are part of their volume, not volumes.
                    v.retain(|x| !lvm_chain::is_snap_lv(&x.name));
                    for x in &mut v {
                        x.format = "qcow2".into();
                    }
                }
                out.extend(v);
            }
            StorageType::Ceph => {
                let text = run_tool("rbd", &["ls", "-l", "--format", "json", &pool.path]).await?;
                let items: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap_or_default();
                for it in items {
                    if it.get("snapshot").is_some() {
                        continue;
                    }
                    let Some(name) = it.get("image").and_then(|v| v.as_str()) else {
                        continue;
                    };
                    out.push(VolumeInfo {
                        name: name.to_string(),
                        path: format!("rbd:{}/{}", pool.path, name),
                        format: "rbd".into(),
                        size_bytes: it.get("size").and_then(|v| v.as_u64()).unwrap_or(0),
                        allocated_bytes: 0,
                    });
                }
            }
            StorageType::Iscsi => {
                let target = iscsi::IscsiTarget::parse(&pool.path)?;
                for l in iscsi::IscsiManager::list_luns(&target) {
                    out.push(VolumeInfo {
                        name: iscsi::IscsiTarget::volume_name(l.lun),
                        path: l.path,
                        format: "raw".into(),
                        size_bytes: l.size_bytes,
                        allocated_bytes: l.size_bytes,
                    });
                }
            }
            ref t => return Err(unsupported("Listing volumes", t)),
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    async fn get_pool_or_404(&self, pool_id: &str) -> Result<StoragePool> {
        self.pools
            .read()
            .await
            .get(pool_id)
            .cloned()
            .ok_or_else(|| {
                horcrux_common::Error::NotFound(format!("Storage pool {} not found", pool_id))
            })
    }

    /// Delete a volume from a storage pool
    pub async fn delete_volume(&self, pool_id: &str, volume_name: &str) -> Result<()> {
        check_names(&[volume_name])?;
        let pool = self.get_pool_or_404(pool_id).await?;
        if let Some(id) = pool.storage_type.plugin_id() {
            return self.plugin_delete_volume(id, &pool, volume_name).await;
        }

        if let Some(path) = Self::file_volume_path(&pool, volume_name)? {
            if !path.is_file() {
                return Err(horcrux_common::Error::NotFound(format!(
                    "Volume {} not found in pool {}",
                    volume_name, pool_id
                )));
            }
            let p = path.to_string_lossy().to_string();
            return match pool.storage_type {
                StorageType::Directory => {
                    self.directory.delete_volume(&pool.path, volume_name).await
                }
                StorageType::Nfs => self.nfs.delete_volume(&p).await,
                StorageType::Cifs => self.cifs.delete_volume(&p).await,
                StorageType::GlusterFs => self.glusterfs.delete_volume(&p).await,
                StorageType::CephFs => self.cephfs.delete_volume(&p).await,
                _ => self.btrfs.delete_volume(&p).await,
            };
        }

        match pool.storage_type {
            StorageType::Zfs => self.zfs.delete_volume(&pool.path, volume_name).await,
            StorageType::Ceph => self.ceph.delete_volume(&pool.path, volume_name).await,
            StorageType::Lvm => match &pool.lvm {
                Some(o) if o.snapshot_as_volume_chain => {
                    self.lvm
                        .chain_delete_volume(&pool.path, volume_name, o)
                        .await
                }
                Some(o) => {
                    self.lvm
                        .delete_volume_with(&pool.path, volume_name, o)
                        .await
                }
                None => self.lvm.delete_volume(&pool.path, volume_name).await,
            },
            ref t => Err(unsupported("Deleting volumes", t)),
        }
    }

    /// Grow a volume to `new_size_gb` (shrinking would destroy guest data
    /// and is refused).
    pub async fn resize_volume(
        &self,
        pool_id: &str,
        volume_name: &str,
        new_size_gb: u64,
    ) -> Result<()> {
        check_names(&[volume_name])?;
        // The grow check below must see the space free now.
        self.refresh_pools(Some(pool_id)).await;
        self.refresh_plugin_pools(Some(pool_id)).await;
        let pool = self.get_pool_or_404(pool_id).await?;
        if let Some(id) = pool.storage_type.plugin_id() {
            return self
                .plugin_resize_volume(id, &pool, volume_name, new_size_gb)
                .await;
        }
        let gb = 1024 * 1024 * 1024;
        let size_arg = format!("{new_size_gb}G");

        if let Some(path) = Self::file_volume_path(&pool, volume_name)? {
            let Some(cur) = file_volume_info(volume_name, &path) else {
                return Err(horcrux_common::Error::NotFound(format!(
                    "Volume {} not found in pool {}",
                    volume_name, pool_id
                )));
            };
            if new_size_gb.saturating_mul(gb) < cur.size_bytes {
                return Err(horcrux_common::Error::InvalidConfig(format!(
                    "Volume {} is {} GB; shrinking is not supported",
                    volume_name,
                    cur.size_bytes.div_ceil(gb)
                )));
            }
            let grow_gb = (new_size_gb * gb - cur.size_bytes).div_ceil(gb);
            if grow_gb > pool.available {
                return Err(horcrux_common::Error::InvalidConfig(format!(
                    "Not enough space in pool {}. Requested: +{}GB, Available: {}GB",
                    pool_id, grow_gb, pool.available
                )));
            }
            let p = path.to_string_lossy().to_string();
            run_tool("qemu-img", &["resize", "-f", &cur.format, &p, &size_arg]).await?;
            return Ok(());
        }

        let target = format!("{}/{}", pool.path, volume_name);
        match pool.storage_type {
            StorageType::Zfs => {
                self.zfs
                    .resize_volume(&pool.path, volume_name, new_size_gb)
                    .await
            }
            StorageType::Lvm => run_tool("lvextend", &["-L", &size_arg, &target])
                .await
                .map(|_| ()),
            StorageType::Ceph => {
                // rbd refuses a shrink without --allow-shrink, but with a 500:
                // say so up front as a bad request.
                let info = run_tool("rbd", &["info", "--format", "json", &target]).await?;
                let cur = serde_json::from_str::<serde_json::Value>(&info)
                    .ok()
                    .and_then(|v| v.get("size").and_then(|s| s.as_u64()));
                if cur.is_some_and(|c| new_size_gb.saturating_mul(gb) < c) {
                    return Err(horcrux_common::Error::InvalidConfig(format!(
                        "Volume {} is {} GB; shrinking is not supported",
                        volume_name,
                        cur.unwrap_or(0).div_ceil(gb)
                    )));
                }
                run_tool("rbd", &["resize", "--size", &size_arg, &target])
                    .await
                    .map(|_| ())
            }
            ref t => Err(unsupported("Resizing volumes", t)),
        }
    }

    /// Create a snapshot of a volume
    pub async fn create_snapshot(
        &self,
        pool_id: &str,
        volume_name: &str,
        snapshot_name: &str,
    ) -> Result<()> {
        check_names(&[volume_name, snapshot_name])?;
        let plugin_pool = self.pools.read().await.get(pool_id).cloned();
        if let Some(p) = plugin_pool.filter(|p| p.storage_type.plugin_id().is_some()) {
            let id = p.storage_type.plugin_id().unwrap_or_default();
            return self
                .plugin_snapshot(
                    id,
                    &p,
                    plugin::sdk::Op::CreateSnapshot,
                    volume_name,
                    snapshot_name,
                )
                .await;
        }
        let pools = self.pools.read().await;
        let pool = pools.get(pool_id).ok_or_else(|| {
            horcrux_common::Error::System(format!("Storage pool {} not found", pool_id))
        })?;

        match pool.storage_type {
            StorageType::Zfs => {
                self.zfs
                    .create_snapshot(&pool.path, volume_name, snapshot_name)
                    .await?
            }
            StorageType::Ceph => {
                self.ceph
                    .create_snapshot(&pool.path, volume_name, snapshot_name)
                    .await?
            }
            StorageType::Lvm => match chain_opts(pool) {
                Some(o) => {
                    self.lvm
                        .chain_snapshot(&pool.path, volume_name, snapshot_name, o)
                        .await?
                }
                None => {
                    self.lvm
                        .create_snapshot(&pool.path, volume_name, snapshot_name)
                        .await?
                }
            },
            StorageType::Directory => {
                return Err(horcrux_common::Error::InvalidConfig(
                    "Directory storage does not support snapshots".to_string(),
                ))
            }
            StorageType::Iscsi => {
                return Err(horcrux_common::Error::InvalidConfig(
                    "iSCSI storage does not support snapshots".to_string(),
                ))
            }
            StorageType::Cifs => {
                return Err(horcrux_common::Error::InvalidConfig(
                    "CIFS storage does not support snapshots".to_string(),
                ))
            }
            StorageType::Nfs => {
                return Err(horcrux_common::Error::InvalidConfig(
                    "NFS storage does not support snapshots".to_string(),
                ))
            }
            StorageType::CephFs => {
                return Err(horcrux_common::Error::InvalidConfig(
                    "CephFS storage does not support snapshots".to_string(),
                ))
            }
            StorageType::GlusterFs => {
                // GlusterFS snapshot takes volume_path and snapshot_name
                let volume_path = format!("{}/{}", pool.path, volume_name);
                self.glusterfs
                    .create_snapshot(&volume_path, snapshot_name)
                    .await?;
            }
            StorageType::BtrFs => {
                // BtrFS snapshot takes source_path, snapshot_name, and readonly flag
                let source_path = format!("{}/{}", pool.path, volume_name);
                self.btrfs
                    .create_snapshot(&source_path, snapshot_name, false)
                    .await?;
            }
            StorageType::S3 => {
                return Err(horcrux_common::Error::InvalidConfig(
                    "S3 does not support snapshots".to_string(),
                ))
            }
            ref t @ StorageType::Plugin(_) => return Err(unsupported("This operation", t)),
        }

        Ok(())
    }

    /// The pool and its volume-chain options, for the chain-only calls.
    async fn chain_pool(&self, pool_id: &str) -> Result<(String, lvm::LvmPoolOptions)> {
        let pools = self.pools.read().await;
        let pool = pools.get(pool_id).ok_or_else(|| {
            horcrux_common::Error::NotFound(format!("Storage pool {} not found", pool_id))
        })?;
        match chain_opts(pool) {
            Some(o) => Ok((pool.path.clone(), o.clone())),
            None => Err(horcrux_common::Error::InvalidConfig(format!(
                "Storage pool {pool_id} does not keep snapshots as a volume chain \
                 (an LVM pool created with snapshot_as_volume_chain)"
            ))),
        }
    }

    /// Whether `pool_id` is a plugin pool (false for an unknown pool).
    async fn is_plugin_pool(&self, pool_id: &str) -> bool {
        self.pools
            .read()
            .await
            .get(pool_id)
            .is_some_and(|p| p.storage_type.plugin_id().is_some())
    }

    /// A volume's snapshot chain (volume-chain LVM pools and plugin pools).
    pub async fn volume_snapshots(
        &self,
        pool_id: &str,
        volume_name: &str,
    ) -> Result<lvm_chain::ChainView> {
        check_names(&[volume_name])?;
        if self.is_plugin_pool(pool_id).await {
            let mut snaps = self.list_snapshots(pool_id, volume_name).await?;
            // Oldest first, like a volume chain (undated ones keep their order).
            snaps.sort_by_key(|s| s.created.unwrap_or(i64::MIN));
            return Ok(lvm_chain::ChainView {
                volume: volume_name.to_string(),
                snapshots: snaps.into_iter().map(|s| s.name).collect(),
                virtual_size_bytes: None,
            });
        }
        let (vg, o) = self.chain_pool(pool_id).await?;
        self.lvm.chain_info(&vg, volume_name, &o).await
    }

    /// Delete one snapshot of a volume (volume-chain LVM pools).
    pub async fn delete_volume_snapshot(
        &self,
        pool_id: &str,
        volume_name: &str,
        snapshot_name: &str,
    ) -> Result<()> {
        check_names(&[volume_name, snapshot_name])?;
        if self.is_plugin_pool(pool_id).await {
            return self
                .delete_snapshot(pool_id, volume_name, snapshot_name)
                .await;
        }
        let (vg, o) = self.chain_pool(pool_id).await?;
        self.lvm
            .chain_delete_snapshot(&vg, volume_name, snapshot_name, &o)
            .await
    }

    /// Restore a volume from a snapshot
    pub async fn restore_snapshot(
        &self,
        pool_id: &str,
        volume_name: &str,
        snapshot_name: &str,
    ) -> Result<()> {
        check_names(&[volume_name, snapshot_name])?;
        let plugin_pool = self.pools.read().await.get(pool_id).cloned();
        if let Some(p) = plugin_pool.filter(|p| p.storage_type.plugin_id().is_some()) {
            let id = p.storage_type.plugin_id().unwrap_or_default();
            return self
                .plugin_snapshot(
                    id,
                    &p,
                    plugin::sdk::Op::RollbackSnapshot,
                    volume_name,
                    snapshot_name,
                )
                .await;
        }
        let pools = self.pools.read().await;
        let pool = pools.get(pool_id).ok_or_else(|| {
            horcrux_common::Error::System(format!("Storage pool {} not found", pool_id))
        })?;

        match pool.storage_type {
            StorageType::Zfs => {
                self.zfs
                    .restore_snapshot(&pool.path, volume_name, snapshot_name)
                    .await?
            }
            StorageType::Ceph => {
                self.ceph
                    .restore_snapshot(&pool.path, volume_name, snapshot_name)
                    .await?
            }
            StorageType::Lvm => match chain_opts(pool) {
                Some(o) => {
                    self.lvm
                        .chain_rollback(&pool.path, volume_name, snapshot_name, o)
                        .await?
                }
                None => {
                    self.lvm
                        .restore_snapshot(&pool.path, volume_name, snapshot_name)
                        .await?
                }
            },
            StorageType::Directory => {
                return Err(horcrux_common::Error::InvalidConfig(
                    "Directory storage does not support snapshots".to_string(),
                ))
            }
            StorageType::Iscsi => {
                return Err(horcrux_common::Error::InvalidConfig(
                    "iSCSI storage does not support snapshots".to_string(),
                ))
            }
            StorageType::Cifs => {
                return Err(horcrux_common::Error::InvalidConfig(
                    "CIFS storage does not support snapshots".to_string(),
                ))
            }
            StorageType::Nfs => {
                return Err(horcrux_common::Error::InvalidConfig(
                    "NFS storage does not support snapshots".to_string(),
                ))
            }
            StorageType::CephFs => {
                return Err(horcrux_common::Error::InvalidConfig(
                    "CephFS storage does not support snapshots".to_string(),
                ))
            }
            StorageType::GlusterFs => {
                // GlusterFS restore takes volume_path and snapshot_name, mirroring create_snapshot
                let volume_path = format!("{}/{}", pool.path, volume_name);
                self.glusterfs
                    .restore_snapshot(&volume_path, snapshot_name)
                    .await?
            }
            StorageType::BtrFs => {
                // BtrFS restore_snapshot takes snapshot_path and target_path
                let snapshot_path = format!("{}/{}@{}", pool.path, volume_name, snapshot_name);
                let target_path = format!("{}/{}", pool.path, volume_name);
                self.btrfs
                    .restore_snapshot(&snapshot_path, &target_path)
                    .await?
            }
            StorageType::S3 => {
                return Err(horcrux_common::Error::InvalidConfig(
                    "S3 does not support snapshots".to_string(),
                ))
            }
            ref t @ StorageType::Plugin(_) => return Err(unsupported("This operation", t)),
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("hx-storage-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn pool(id: &str, t: StorageType, path: &str) -> StoragePool {
        StoragePool {
            id: id.into(),
            name: id.into(),
            storage_type: t,
            path: path.into(),
            available: 0,
            total: 0,
            enabled: true,
            source: None,
            lvm: None,
            plugin_config: None,
        }
    }

    /// (total, available) GB per `df` (1K blocks, floored to GB).
    fn df_gb(path: &str) -> (u64, u64) {
        let out = std::process::Command::new("df")
            .args(["-k", "--output=size,avail", path])
            .output()
            .expect("df");
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        let mut nums = text
            .lines()
            .nth(1)
            .expect("df row")
            .split_whitespace()
            .map(|n| n.parse::<u64>().unwrap() / (1024 * 1024));
        (nums.next().unwrap(), nums.next().unwrap())
    }

    /// A manager whose pools keep the capacity a test sets (no statvfs).
    fn pinned() -> StorageManager {
        StorageManager::new().with_capacity_probe(|_| None)
    }

    /// Fake filesystem for `moving_capacity`: (total, available) GB.
    static FAKE_TOTAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    static FAKE_AVAIL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    fn fake_fs(_: &str) -> Option<(u64, u64)> {
        use std::sync::atomic::Ordering::SeqCst;
        Some((FAKE_TOTAL.load(SeqCst), FAKE_AVAIL.load(SeqCst)))
    }

    /// Pool capacity is re-measured on every list/get and before each
    /// space check (it used to be measured only at pool add / API start,
    /// so a pool that filled up afterwards kept accepting volumes and the
    /// Storage page showed the old free space); GlusterFS pools are
    /// measured too; block pools keep their numbers.
    #[tokio::test]
    async fn moving_capacity() {
        use std::sync::atomic::Ordering::SeqCst;
        let d = scratch("moving-cap");
        let m = StorageManager::new().with_capacity_probe(fake_fs);
        FAKE_TOTAL.store(10, SeqCst);
        FAKE_AVAIL.store(5, SeqCst);
        let p = m
            .add_pool(pool("mc", StorageType::Directory, d.to_str().unwrap()))
            .await
            .unwrap();
        assert_eq!((p.total, p.available), (10, 5));

        // The filesystem fills up behind Horcrux's back.
        FAKE_AVAIL.store(1, SeqCst);
        assert_eq!(m.get_pool("mc").await.unwrap().available, 1);
        let listed = m.list_pools().await;
        assert_eq!((listed[0].total, listed[0].available), (10, 1));
        FAKE_AVAIL.store(2, SeqCst);
        // create_volume checks against the space free now: 2 GB fits, 3 not.
        assert!(matches!(
            m.create_volume("mc", "big", 3).await,
            Err(horcrux_common::Error::InvalidConfig(_))
        ));
        assert!(!d.join("big.qcow2").exists());
        m.create_volume("mc", "v1", 1).await.unwrap();
        // Grow: re-measured before the check too.
        FAKE_AVAIL.store(0, SeqCst);
        assert!(matches!(
            m.resize_volume("mc", "v1", 2).await,
            Err(horcrux_common::Error::InvalidConfig(_))
        ));
        FAKE_AVAIL.store(4, SeqCst);
        m.resize_volume("mc", "v1", 2).await.unwrap();

        // GlusterFS pools are filesystem pools: measured like the rest.
        let g = scratch("moving-cap-gluster");
        m.pools.write().await.insert(
            "g".into(),
            pool("g", StorageType::GlusterFs, g.to_str().unwrap()),
        );
        assert_eq!(m.get_pool("g").await.unwrap().available, 4);
        let mut gp = pool("g2", StorageType::GlusterFs, g.to_str().unwrap());
        StorageManager::refresh_capacity(fake_fs, &mut gp);
        assert_eq!((gp.total, gp.available), (10, 4));

        // LVM pools are measured (by `vgs`, the fake here) like the rest.
        let mut lp = pool("l", StorageType::Lvm, "vg_x");
        lp.available = 7;
        m.pools.write().await.insert("l".into(), lp);
        assert_eq!(m.get_pool("l").await.unwrap().available, 4);

        // iSCSI (sized at attach) and S3 aren't probed.
        for t in [StorageType::Iscsi, StorageType::S3] {
            let mut bp = pool("b", t.clone(), "tank/vms");
            bp.available = 7;
            m.pools.write().await.insert("b".into(), bp.clone());
            assert_eq!(m.get_pool("b").await.unwrap().available, 7, "{t:?}");
            StorageManager::refresh_capacity(fake_fs, &mut bp);
            assert_eq!(bp.available, 7, "{t:?}");
        }
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&g);
    }

    /// A ZFS pool's path is a dataset name: it is measured with the ZFS
    /// probe (it used to stay at 0 GB, so every zvol create was refused),
    /// and `zfs list -Hp` output parses to GB.
    #[tokio::test]
    async fn zfs_pool_capacity_is_measured() {
        use std::sync::atomic::Ordering::SeqCst;
        static ZT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        fn fs_none(_: &str) -> Option<(u64, u64)> {
            None
        }
        fn zfs_fake(ds: &str) -> Option<(u64, u64)> {
            (ds == "tank/vms").then(|| (ZT.load(SeqCst), ZT.load(SeqCst) - 1))
        }
        let mut m = StorageManager::new().with_capacity_probe(fs_none);
        m.zfs_probe = zfs_fake;
        ZT.store(3, SeqCst);
        m.pools
            .write()
            .await
            .insert("z".into(), pool("z", StorageType::Zfs, "tank/vms"));
        let p = m.get_pool("z").await.unwrap();
        assert_eq!((p.total, p.available), (3, 2));
        ZT.store(9, SeqCst);
        assert_eq!(m.list_pools().await[0].available, 8);
        assert_eq!(
            parse_zfs_capacity("1073741824\t2952790016\n"),
            Some((3, 2)) // 1 GiB used + 2.75 GiB free = 3.75 -> 3; 2.75 -> 2
        );
        assert_eq!(parse_zfs_capacity("-\t-\n"), None);
        assert_eq!(parse_zfs_capacity(""), None);
    }

    /// A Ceph pool's path is a RADOS pool name: it is measured from `ceph df`
    /// (it used to stay at 0 GB, so every rbd create was refused).
    #[test]
    fn parse_ceph_df_reads_the_named_pool() {
        let df = r#"{"stats":{"total_bytes":3221225472},"pools":[
            {"name":"hxpool","id":1,"stats":{"stored":1073741824,"max_avail":3031954944}},
            {"name":".mgr","id":2,"stats":{"stored":459280,"max_avail":99}}]}"#;
        // 1 GiB stored + 2.82 GiB free = 3.82 -> 3; 2.82 -> 2
        assert_eq!(parse_ceph_df(df, "hxpool"), Some((3, 2)));
        assert_eq!(parse_ceph_df(df, "nopool"), None);
        assert_eq!(parse_ceph_df("{}", "hxpool"), None);
        assert_eq!(parse_ceph_df("not json", "hxpool"), None);
    }

    #[tokio::test]
    async fn ceph_pool_capacity_is_measured() {
        fn fs_none(_: &str) -> Option<(u64, u64)> {
            None
        }
        fn ceph_fake(p: &str) -> Option<(u64, u64)> {
            (p == "rbdpool").then_some((8, 6))
        }
        let mut m = StorageManager::new().with_capacity_probe(fs_none);
        m.ceph_probe = ceph_fake;
        m.pools
            .write()
            .await
            .insert("c".into(), pool("c", StorageType::Ceph, "rbdpool"));
        let p = m.get_pool("c").await.unwrap();
        assert_eq!((p.total, p.available), (8, 6));
    }

    /// `zfs_capacity_gb` runs `zfs list` and reports (used + avail, avail) in GB;
    /// a failing `zfs` (exit != 0) is None even if it printed a valid line, and
    /// a missing binary is None. Fake `zfs` scripts pin each case (the only
    /// test that sets HORCRUX_ZFS_BIN: the other tests inject a probe).
    #[test]
    fn zfs_capacity_gb_runs_zfs_and_reports_its_numbers() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("zfs-bin");
        let fake = |name: &str, body: &str| {
            let f = dir.join(name);
            std::fs::write(&f, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
            f
        };
        // 1 GiB used + 3 GiB avail -> total 4, available 3; args are checked too.
        let ok = fake(
            "zfs-ok",
            "[ \"$1 $2 $3 $4 $5 $6\" = 'list -Hp -o used,avail -- tank/vms' ] || exit 9\n\
             printf '1073741824\\t3221225472\\n'",
        );
        let bad = fake("zfs-bad", "printf '1073741824\\t3221225472\\n'; exit 1");
        std::env::set_var("HORCRUX_ZFS_BIN", &ok);
        assert_eq!(zfs_capacity_gb("tank/vms"), Some((4, 3)));
        std::env::set_var("HORCRUX_ZFS_BIN", &bad);
        assert_eq!(zfs_capacity_gb("tank/vms"), None);
        std::env::set_var("HORCRUX_ZFS_BIN", dir.join("no-such-zfs"));
        assert_eq!(zfs_capacity_gb("tank/vms"), None);
        std::env::remove_var("HORCRUX_ZFS_BIN");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A pool whose mount doesn't answer keeps its last numbers, the call
    /// returns after the probe timeout, and the stuck pool is not probed
    /// again while its first measurement still hangs.
    #[tokio::test]
    async fn hung_mount_keeps_last_capacity() {
        static CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        fn hang(_: &str) -> Option<(u64, u64)> {
            CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_secs(4));
            Some((99, 99))
        }
        let m = StorageManager::new().with_capacity_probe(hang);
        let mut p = pool("h", StorageType::Nfs, "/mnt/hung");
        p.available = 3;
        m.pools.write().await.insert("h".into(), p);
        let t0 = std::time::Instant::now();
        assert_eq!(m.get_pool("h").await.unwrap().available, 3);
        assert!(
            t0.elapsed() < std::time::Duration::from_secs(4),
            "timed out, not blocked"
        );
        assert_eq!(m.list_pools().await[0].available, 3);
        assert_eq!(
            CALLS.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "not re-probed"
        );
    }

    #[test]
    fn capacity_matches_df() {
        // A filesystem with a few GB at least (the build tree), so the GB
        // arithmetic is visible.
        let path = env!("CARGO_MANIFEST_DIR");
        let (total, avail) = fs_capacity_gb(path).expect("statvfs");
        let (df_total, df_avail) = df_gb(path);
        assert_eq!(total, df_total);
        // Free space can move while other builds write to the disk.
        assert!(avail.abs_diff(df_avail) <= 1, "{avail} vs df {df_avail}");
        assert!(total > 0 && avail <= total);
        assert_eq!(fs_capacity_gb("/nonexistent/hx-storage-test"), None);
    }

    #[tokio::test]
    async fn directory_pool_lifecycle_with_real_capacity() {
        let d = scratch("dir");
        let m = StorageManager::new();
        let p = m
            .add_pool(pool("dirpool", StorageType::Directory, d.to_str().unwrap()))
            .await
            .unwrap();
        let (total, avail) = fs_capacity_gb(d.to_str().unwrap()).unwrap();
        assert_eq!(p.total, total, "capacity measured");
        assert!(
            p.available.abs_diff(avail) <= 1,
            "{} vs {avail}",
            p.available
        );
        let listed = m.list_pools().await;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "dirpool");
        assert_eq!(m.get_pool("dirpool").await.unwrap().total, total);
        assert!(m
            .add_pool(pool("dirpool", StorageType::Directory, d.to_str().unwrap()))
            .await
            .is_err());
        assert!(m.remove_pool("nope").await.is_err());
        m.remove_pool("dirpool").await.unwrap();
        assert!(m.list_pools().await.is_empty());
        assert!(m.get_pool("dirpool").await.is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn volumes_respect_available_space_and_pool_state() {
        let d = scratch("vol");
        let m = pinned();
        let mut p = pool("v", StorageType::Directory, d.to_str().unwrap());
        p.available = 2;
        m.pools.write().await.insert("v".into(), p);
        // Exactly the available space fits (qcow2 is sparse).
        let path = m.create_volume("v", "disk0", 2).await.unwrap();
        assert_eq!(path, format!("{}/disk0.qcow2", d.display()));
        assert!(std::path::Path::new(&path).is_file());
        // One GB more does not.
        assert!(m.create_volume("v", "disk1", 3).await.is_err());
        assert!(!d.join("disk1.qcow2").exists());
        // Disabled pools and unknown pools refuse.
        m.pools.write().await.get_mut("v").unwrap().enabled = false;
        assert!(m.create_volume("v", "disk2", 1).await.is_err());
        assert!(m.create_volume("missing", "disk3", 1).await.is_err());
        // S3 pools hold backups, not VM disks.
        let mut s3 = pool("s3", StorageType::S3, "s3://bucket");
        s3.available = 100;
        m.pools.write().await.insert("s3".into(), s3);
        assert!(m.create_volume("s3", "disk4", 1).await.is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    const BAD_NAMES: &[&str] = &[
        "../x",
        "../../tmp/x",
        "a/b",
        "/tmp/x",
        "/etc/passwd",
        "",
        ".",
        "..",
        "-rf",
        "x@y",
    ];

    fn is_validation<T>(r: &Result<T>) -> bool {
        matches!(r, Err(horcrux_common::Error::Validation(_)))
    }

    /// Every file under `root` (to prove nothing landed outside the pool).
    fn tree(root: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(root) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    out.extend(tree(&p));
                } else {
                    out.push(p);
                }
            }
        }
        out.sort();
        out
    }

    #[tokio::test]
    async fn volume_names_cannot_escape_the_pool() {
        // root/pool is the pool; root/victim.qcow2 and root/x.qcow2 sit
        // next to it, where "../victim" / "../x" would point.
        let root = scratch("traversal");
        let d = root.join("pool");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(root.join("victim.qcow2"), b"keep me").unwrap();
        let m = pinned();
        let mut p = pool("t", StorageType::Directory, d.to_str().unwrap());
        p.available = 100;
        m.pools.write().await.insert("t".into(), p);
        let before = tree(&root);

        for bad in BAD_NAMES.iter().chain(["../victim"].iter()) {
            assert!(
                is_validation(&m.create_volume("t", bad, 1).await),
                "create {bad:?}"
            );
            assert!(
                is_validation(&m.delete_volume("t", bad).await),
                "delete {bad:?}"
            );
            assert!(
                is_validation(&m.resize_volume("t", bad, 2).await),
                "resize {bad:?}"
            );
            assert!(
                is_validation(&m.create_snapshot("t", bad, "s1").await),
                "snapshot vol {bad:?}"
            );
            assert!(
                is_validation(&m.create_snapshot("t", "disk0", bad).await),
                "snapshot name {bad:?}"
            );
            assert!(
                is_validation(&m.restore_snapshot("t", bad, "s1").await),
                "restore {bad:?}"
            );
            let pool = m.get_pool("t").await.unwrap();
            assert!(StorageManager::file_volume_path(&pool, bad).is_err());
            assert!(StorageManager::volume_location(&pool, bad).is_err());
        }
        assert_eq!(tree(&root), before, "nothing created or deleted");
        assert_eq!(
            std::fs::read(root.join("victim.qcow2")).unwrap(),
            b"keep me"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Defence in depth: the backends themselves refuse bad names before
    /// building a path or running a tool, even when called directly.
    #[tokio::test]
    async fn every_backend_refuses_bad_names() {
        let root = scratch("backend");
        let dir = root.join("pool");
        std::fs::create_dir_all(&dir).unwrap();
        let ds = dir.to_str().unwrap();
        let before = tree(&root);
        for bad in BAD_NAMES {
            assert!(is_validation(
                &directory::DirectoryManager::new()
                    .create_volume(ds, bad, 1)
                    .await
            ));
            assert!(is_validation(
                &directory::DirectoryManager::new()
                    .delete_volume(ds, bad)
                    .await
            ));
            assert!(is_validation(
                &nfs::NfsManager::new().create_volume(ds, bad, 1).await
            ));
            assert!(is_validation(
                &nfs::NfsManager::new().create_snapshot(ds, bad).await
            ));
            assert!(is_validation(
                &cifs::CifsManager::new().create_volume(ds, bad, 1).await
            ));
            assert!(is_validation(
                &glusterfs::GlusterFsManager::new()
                    .create_volume(ds, bad, 1)
                    .await
            ));
            assert!(is_validation(
                &glusterfs::GlusterFsManager::new()
                    .create_snapshot(ds, bad)
                    .await
            ));
            assert!(is_validation(
                &glusterfs::GlusterFsManager::new()
                    .restore_snapshot(ds, bad)
                    .await
            ));
            assert!(is_validation(
                &btrfs::BtrFsManager::new().create_volume(ds, bad, 1).await
            ));
            assert!(is_validation(
                &btrfs::BtrFsManager::new()
                    .create_snapshot(ds, bad, true)
                    .await
            ));
            let z = zfs::ZfsManager::new();
            assert!(is_validation(&z.create_volume("tank", bad, 1).await));
            assert!(is_validation(&z.delete_volume("tank", bad).await));
            assert!(is_validation(&z.resize_volume("tank", bad, 2).await));
            assert!(is_validation(&z.create_snapshot("tank", bad, "s").await));
            assert!(is_validation(&z.create_snapshot("tank", "v", bad).await));
            assert!(is_validation(&z.restore_snapshot("tank", bad, "s").await));
            assert!(is_validation(
                &z.clone_snapshot("tank", "v", "s", bad).await
            ));
            assert!(is_validation(&z.delete_snapshot("tank", bad, "s").await));
            let l = lvm::LvmManager::new();
            assert!(is_validation(&l.create_volume("vg0", bad, 1).await));
            assert!(is_validation(&l.delete_volume("vg0", bad).await));
            assert!(is_validation(&l.create_snapshot("vg0", bad, "s").await));
            assert!(is_validation(&l.restore_snapshot("vg0", "v", bad).await));
            let c = ceph::CephManager::new();
            assert!(is_validation(&c.create_volume("rbd", bad, 1).await));
            assert!(is_validation(&c.delete_volume("rbd", bad).await));
            assert!(is_validation(&c.create_snapshot("rbd", bad, "s").await));
            assert!(is_validation(&c.restore_snapshot("rbd", "v", bad).await));
            assert!(is_validation(&c.clone_snapshot("rbd", "v", "s", bad).await));
            assert!(is_validation(&c.map_rbd("rbd", bad).await));
        }
        assert_eq!(tree(&root), before);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn directory_volumes_list_resize_and_delete() {
        let d = scratch("vols");
        let m = pinned();
        let mut p = pool("v", StorageType::Directory, d.to_str().unwrap());
        p.available = 10;
        m.pools.write().await.insert("v".into(), p);

        assert!(m.list_volumes("v").await.unwrap().is_empty());
        m.create_volume("v", "data-1", 1).await.unwrap();
        m.create_volume("v", "boot.disk", 2).await.unwrap();
        // Same name again would truncate the image: refused.
        assert!(matches!(
            m.create_volume("v", "data-1", 1).await,
            Err(horcrux_common::Error::AlreadyExists(_))
        ));
        assert!(m.create_volume("v", "zero", 0).await.is_err());
        // Files that aren't valid volume names are not listed.
        std::fs::write(d.join(".hidden.qcow2"), b"x").unwrap();
        std::fs::write(d.join("notes.txt"), b"x").unwrap();

        let gb = 1024 * 1024 * 1024;
        let vols = m.list_volumes("v").await.unwrap();
        let names: Vec<_> = vols.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, ["boot.disk", "data-1"]);
        assert_eq!(vols[0].size_bytes, 2 * gb);
        assert_eq!(vols[0].format, "qcow2");
        assert_eq!(vols[1].path, format!("{}/data-1.qcow2", d.display()));
        let pool = m.get_pool("v").await.unwrap();
        assert_eq!(
            StorageManager::volume_location(&pool, "data-1").unwrap(),
            vols[1].path
        );

        // Grow (real qemu-img resize), refuse shrink and over-allocation.
        m.resize_volume("v", "data-1", 3).await.unwrap();
        assert_eq!(qcow2_virtual_size(&d.join("data-1.qcow2")), Some(3 * gb));
        assert!(m.resize_volume("v", "data-1", 2).await.is_err());
        assert!(m.resize_volume("v", "data-1", 50).await.is_err());
        assert!(matches!(
            m.resize_volume("v", "missing", 5).await,
            Err(horcrux_common::Error::NotFound(_))
        ));

        m.delete_volume("v", "data-1").await.unwrap();
        assert!(!d.join("data-1.qcow2").exists());
        assert!(matches!(
            m.delete_volume("v", "data-1").await,
            Err(horcrux_common::Error::NotFound(_))
        ));
        assert!(matches!(
            m.list_volumes("nope").await,
            Err(horcrux_common::Error::NotFound(_))
        ));
        let names: Vec<_> = m
            .list_volumes("v")
            .await
            .unwrap()
            .into_iter()
            .map(|v| v.name)
            .collect();
        assert_eq!(names, ["boot.disk"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn s3_bucket_names_are_3_to_63_characters() {
        let m = StorageManager::new();
        for (i, (path, ok)) in [
            ("s3://ab".to_string(), false),
            ("s3://abc".to_string(), true),
            (format!("s3://{}", "b".repeat(63)), true),
            (format!("s3://{}", "b".repeat(64)), false),
            ("s3://endpoint.example/abc".to_string(), true),
            ("s3://".to_string(), false),
            ("http://abc".to_string(), false),
            (String::new(), false),
        ]
        .into_iter()
        .enumerate()
        {
            let r = m
                .add_pool(pool(&format!("s3-{i}"), StorageType::S3, &path))
                .await;
            assert_eq!(r.is_ok(), ok, "{path:?}: {r:?}");
        }
    }

    #[tokio::test]
    async fn pools_survive_a_restart_through_the_database() {
        let d = scratch("db");
        let db = Arc::new(Database::new("sqlite::memory:").await.unwrap());
        db.migrate().await.unwrap();
        let m = StorageManager::with_database(db.clone()).await.unwrap();
        m.add_pool(pool("kept", StorageType::Directory, d.to_str().unwrap()))
            .await
            .unwrap();
        m.add_pool(pool("gone", StorageType::Directory, d.to_str().unwrap()))
            .await
            .unwrap();
        m.remove_pool("gone").await.unwrap();
        drop(m);

        let again = StorageManager::with_database(db).await.unwrap();
        let pools = again.list_pools().await;
        assert_eq!(pools.len(), 1, "{pools:?}");
        assert_eq!(pools[0].id, "kept");
        // Capacity is re-measured on load.
        let (total, _) = fs_capacity_gb(d.to_str().unwrap()).unwrap();
        assert_eq!(pools[0].total, total);
        let _ = std::fs::remove_dir_all(&d);
    }

    // ---- tools faked on PATH (iscsiadm / mount / umount) -----------------
    // Every call is appended to one log; tests grep it for their own unique
    // pool path / IQN, so parallel tests don't interfere.
    fn fake_tool_log() -> &'static std::path::Path {
        static LOG: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
        LOG.get_or_init(|| {
            use std::os::unix::fs::PermissionsExt;
            let dir = scratch("faketools");
            let log = dir.join("calls.log");
            std::fs::write(&log, "").unwrap();
            for tool in ["iscsiadm", "mount", "umount", "rbd"] {
                let body = format!(
                    "#!/bin/sh\necho \"{tool} $*\" >> {log}\n\
                     if [ \"{tool}\" = iscsiadm ] && [ \"$2\" = session ]; then\n\
                     echo 'tcp: [1] 10.9.9.9:3260,1 iqn.hx-test:rm1 (non-flash)'\nfi\n\
                     if [ \"{tool}\" = rbd ] && [ \"$1\" = info ]; then\n\
                     echo '{{\"size\": 2147483648}}'\nfi\nexit 0\n",
                    log = log.display()
                );
                let f = dir.join(tool);
                std::fs::write(&f, body).unwrap();
                std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            let old = std::env::var("PATH").unwrap_or_default();
            std::env::set_var("PATH", format!("{}:{old}", dir.display()));
            log
        })
    }

    fn calls_matching(needle: &str) -> Vec<String> {
        std::fs::read_to_string(fake_tool_log())
            .unwrap()
            .lines()
            .filter(|l| l.contains(needle))
            .map(String::from)
            .collect()
    }

    #[test]
    fn iscsi_pool_total_is_the_luns_summed_in_whole_gb() {
        const GIB: u64 = 1024 * 1024 * 1024;
        let lun = |n, size_bytes| iscsi::IscsiLun {
            lun: n,
            path: format!("/dev/x{n}"),
            size_bytes,
        };
        assert_eq!(luns_total_gb(&[]), 0);
        assert_eq!(luns_total_gb(&[lun(0, 5 * GIB + 7), lun(1, 2 * GIB)]), 7);
        assert_eq!(luns_total_gb(&[lun(0, 3 * GIB - 1)]), 2);
        assert_eq!(luns_total_gb(&[lun(0, 40 * GIB)]), 40);
    }

    #[tokio::test]
    async fn remove_pool_unmounts_only_network_pools_with_a_source() {
        fake_tool_log();
        let m = pinned();
        let nfs_dir = scratch("rm-nfs-src");
        let bare_dir = scratch("rm-nfs-nosrc");
        let dir_dir = scratch("rm-dir-src");
        let mut with_src = pool("rm-a", StorageType::Nfs, nfs_dir.to_str().unwrap());
        with_src.source = Some("hx-srv:/rm-a".into());
        let bare = pool("rm-b", StorageType::Nfs, bare_dir.to_str().unwrap());
        let mut dir_src = pool("rm-c", StorageType::Directory, dir_dir.to_str().unwrap());
        dir_src.source = Some("hx-srv:/rm-c".into());
        for p in [&with_src, &bare, &dir_src] {
            m.pools.write().await.insert(p.id.clone(), p.clone());
        }

        m.remove_pool("rm-a").await.unwrap();
        assert_eq!(calls_matching(nfs_dir.to_str().unwrap()).len(), 1);
        assert!(!nfs_dir.exists(), "mount point removed after the unmount");

        m.remove_pool("rm-b").await.unwrap();
        m.remove_pool("rm-c").await.unwrap();
        assert!(calls_matching(bare_dir.to_str().unwrap()).is_empty());
        assert!(calls_matching(dir_dir.to_str().unwrap()).is_empty());
        assert!(bare_dir.exists() && dir_dir.exists());
        assert!(m.list_pools().await.is_empty());
        let _ = std::fs::remove_dir_all(&bare_dir);
        let _ = std::fs::remove_dir_all(&dir_dir);
    }

    #[tokio::test]
    async fn remove_pool_logs_out_of_an_iscsi_target() {
        fake_tool_log();
        let m = pinned();
        m.pools.write().await.insert(
            "rm-i".into(),
            pool(
                "rm-i",
                StorageType::Iscsi,
                "iscsi://10.9.9.9:3260/iqn.hx-test:rm1",
            ),
        );
        m.remove_pool("rm-i").await.unwrap();
        let logouts = calls_matching("--logout");
        assert!(
            logouts.iter().any(|l| l.contains("iqn.hx-test:rm1")),
            "{logouts:?}"
        );
    }

    #[tokio::test]
    async fn create_volume_remounts_a_lost_network_mount_only() {
        fake_tool_log();
        // Not mounted any more: the export is mounted again first.
        let lost = scratch("cv-lost");
        let mut p = pool("cv-lost", StorageType::Nfs, lost.to_str().unwrap());
        p.source = Some("hx-srv-lost:/exp".into());
        p.available = 10;
        let m = pinned();
        m.pools.write().await.insert(p.id.clone(), p);
        let _ = m.create_volume("cv-lost", "v1", 1).await;
        assert_eq!(calls_matching("hx-srv-lost:/exp").len(), 1);

        // Still mounted (/proc is in /proc/mounts): no second mount.
        let mut q = pool("cv-up", StorageType::Nfs, "/proc");
        q.source = Some("hx-srv-up:/exp".into());
        q.available = 10;
        m.pools.write().await.insert(q.id.clone(), q);
        let _ = m.create_volume("cv-up", "v1", 1).await;
        assert!(calls_matching("hx-srv-up:/exp").is_empty());
        let _ = std::fs::remove_dir_all(&lost);
    }

    #[tokio::test]
    async fn loading_an_iscsi_pool_logs_in_again_in_the_background() {
        fake_tool_log();
        let db = Arc::new(Database::new("sqlite::memory:").await.unwrap());
        db.migrate().await.unwrap();
        let store = Some(ConfigStore::new(&db));
        let iscsi_pool = pool(
            "wd-i",
            StorageType::Iscsi,
            "iscsi://10.9.9.9:3260/iqn.hx-test:wd",
        );
        config_store::put_opt(&store, kinds::STORAGE_POOL, "wd-i", &iscsi_pool)
            .await
            .unwrap();
        let _mgr = StorageManager::with_database(db).await.unwrap();
        let mut seen = false;
        for _ in 0..50 {
            if calls_matching("--login")
                .iter()
                .any(|l| l.contains("iqn.hx-test:wd"))
            {
                seen = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert!(seen, "no iscsiadm --login for the stored iSCSI pool");
    }

    /// A Ceph volume grows (also to its exact size) and is never shrunk:
    /// the size comes from `rbd info`, the grow goes through `rbd resize`.
    #[tokio::test]
    async fn ceph_resize_refuses_a_shrink_and_allows_same_or_larger() {
        fake_tool_log(); // fake rbd: every image is 2 GiB
        let m = pinned();
        m.pools
            .write()
            .await
            .insert("cr".into(), pool("cr", StorageType::Ceph, "hxrbd-resize"));
        let err = m.resize_volume("cr", "disk1", 1).await;
        assert!(
            matches!(err, Err(horcrux_common::Error::InvalidConfig(_))),
            "{err:?}"
        );
        assert!(calls_matching("resize --size 1G hxrbd-resize/disk1").is_empty());
        m.resize_volume("cr", "disk1", 2).await.unwrap();
        m.resize_volume("cr", "disk1", 3).await.unwrap();
        assert_eq!(
            calls_matching("resize --size 2G hxrbd-resize/disk1").len(),
            1
        );
        assert_eq!(
            calls_matching("resize --size 3G hxrbd-resize/disk1").len(),
            1
        );
    }

    /// `ceph_capacity_gb` runs `ceph df --format json`, reports the named
    /// pool in GB, and is None when ceph fails or is missing. (The only
    /// test that sets HORCRUX_CEPH_BIN.)
    #[test]
    fn ceph_capacity_gb_runs_ceph_and_reports_its_numbers() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("ceph-bin");
        let fake = |name: &str, body: &str| {
            let f = dir.join(name);
            std::fs::write(&f, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
            f
        };
        let df =
            r#"{"pools":[{"name":"hxpool","stats":{"stored":1073741824,"max_avail":3031954944}}]}"#;
        let ok = fake(
            "ceph-ok",
            &format!("[ \"$*\" = '--connect-timeout 5 df --format json' ] || exit 9\necho '{df}'"),
        );
        let bad = fake("ceph-bad", &format!("echo '{df}'; exit 1"));
        std::env::set_var("HORCRUX_CEPH_BIN", &ok);
        assert_eq!(ceph_capacity_gb("hxpool"), Some((3, 2)));
        assert_eq!(ceph_capacity_gb("nopool"), None);
        std::env::set_var("HORCRUX_CEPH_BIN", &bad);
        assert_eq!(ceph_capacity_gb("hxpool"), None);
        std::env::set_var("HORCRUX_CEPH_BIN", dir.join("no-such-ceph"));
        assert_eq!(ceph_capacity_gb("hxpool"), None);
        std::env::remove_var("HORCRUX_CEPH_BIN");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `lvm_capacity_gb` runs `vgs` in bytes and reports the VG in GB; None
    /// when vgs fails (VG not visible). (The only test that sets HORCRUX_VGS_BIN.)
    #[test]
    fn lvm_capacity_gb_runs_vgs_and_reports_its_numbers() {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            parse_vgs_capacity("\n  3217031168 2143289344\n"),
            Some((2, 1))
        );
        assert_eq!(parse_vgs_capacity("  garbage\n"), None);
        let dir = scratch("vgs-bin");
        let f = dir.join("vgs");
        std::fs::write(
            &f,
            "#!/bin/sh\n[ \"$*\" = '--noheadings --units b --nosuffix -o vg_size,vg_free -- vg_t' ] || exit 5\necho '  5368709120 4294967296'\n",
        )
        .unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var("HORCRUX_VGS_BIN", &f);
        assert_eq!(lvm_capacity_gb("vg_t"), Some((5, 4)));
        assert_eq!(lvm_capacity_gb("vg_other"), None);
        std::env::remove_var("HORCRUX_VGS_BIN");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_volume_info_reports_allocated_bytes_in_512_byte_blocks() {
        use std::io::Write;
        use std::os::unix::fs::MetadataExt;
        let d = scratch("alloc");
        let f = d.join("raw-1.qcow2");
        let mut fh = std::fs::File::create(&f).unwrap();
        fh.write_all(&vec![0xa5u8; 256 * 1024]).unwrap();
        fh.sync_all().unwrap();
        drop(fh);
        let blocks = std::fs::metadata(&f).unwrap().blocks();
        assert!(blocks > 0, "written file reports no blocks");
        let v = file_volume_info("raw-1", &f).unwrap();
        assert_eq!(v.format, "raw");
        assert_eq!(v.size_bytes, 256 * 1024);
        // st_blocks is always in 512-byte units, whatever the fs block size.
        assert_eq!(v.allocated_bytes, blocks * 512);
        assert!(v.allocated_bytes >= 256 * 1024);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn file_volume_paths_per_pool_type() {
        let p = |t| pool("p", t, "/srv/pool");
        for t in [
            StorageType::Directory,
            StorageType::Nfs,
            StorageType::Cifs,
            StorageType::GlusterFs,
            StorageType::CephFs,
        ] {
            assert_eq!(
                StorageManager::file_volume_path(&p(t), "d1").unwrap(),
                Some(PathBuf::from("/srv/pool/d1.qcow2"))
            );
        }
        // btrfs keeps each volume in its own `vm-<name>` subvolume.
        assert_eq!(
            StorageManager::file_volume_path(&p(StorageType::BtrFs), "d1").unwrap(),
            Some(PathBuf::from("/srv/pool/vm-d1/d1.qcow2"))
        );
        assert_eq!(
            StorageManager::volume_location(&p(StorageType::BtrFs), "d1").unwrap(),
            "/srv/pool/vm-d1/d1.qcow2"
        );
        for t in [StorageType::Zfs, StorageType::Lvm, StorageType::Ceph] {
            assert_eq!(StorageManager::file_volume_path(&p(t), "d1").unwrap(), None);
        }
    }

    #[tokio::test]
    async fn btrfs_pools_list_volumes_from_their_subvolume_dirs() {
        let d = scratch("btrfs-list");
        std::fs::create_dir_all(d.join("vm-a1")).unwrap();
        std::fs::write(d.join("vm-a1").join("a1.qcow2"), b"raw").unwrap();
        std::fs::create_dir_all(d.join("vm-empty")).unwrap();
        std::fs::write(d.join("stray.qcow2"), b"x").unwrap();
        let m = StorageManager::new();
        m.pools.write().await.insert(
            "b".into(),
            pool("b", StorageType::BtrFs, d.to_str().unwrap()),
        );
        let vols = m.list_volumes("b").await.unwrap();
        assert_eq!(vols.len(), 1);
        assert_eq!(vols[0].name, "a1");
        assert_eq!(vols[0].path, format!("{}/vm-a1/a1.qcow2", d.display()));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn zfs_volume_listing_parses_name_size_used() {
        let text = "tank/vms/disk-1\t10737418240\t4096\n\
                    garbage\n\
                    tank/vms/two\t5\n\
                    \n\
                    tank/vms/disk-2\t2147483648\t123\textra\n";
        let v = parse_zfs_volumes(text);
        assert_eq!(
            v,
            [
                VolumeInfo {
                    name: "disk-1".into(),
                    path: "/dev/zvol/tank/vms/disk-1".into(),
                    format: "zvol".into(),
                    size_bytes: 10737418240,
                    allocated_bytes: 4096,
                },
                VolumeInfo {
                    name: "disk-2".into(),
                    path: "/dev/zvol/tank/vms/disk-2".into(),
                    format: "zvol".into(),
                    size_bytes: 2147483648,
                    allocated_bytes: 123,
                },
            ]
        );
        assert!(parse_zfs_volumes("").is_empty());
    }

    #[test]
    fn lvs_volume_listing_parses_name_and_size() {
        let text = "  root|10737418240\n\
                    \n\
                    lonely\n\
                    |42\n\
                    data|  1048576 \n";
        let v = parse_lvs_volumes(text, "vg0");
        assert_eq!(
            v,
            [
                VolumeInfo {
                    name: "root".into(),
                    path: "/dev/vg0/root".into(),
                    format: "lv".into(),
                    size_bytes: 10737418240,
                    allocated_bytes: 10737418240,
                },
                VolumeInfo {
                    name: "data".into(),
                    path: "/dev/vg0/data".into(),
                    format: "lv".into(),
                    size_bytes: 1048576,
                    allocated_bytes: 1048576,
                },
            ]
        );
    }

    #[tokio::test]
    async fn network_fs_pools_delete_only_the_volume_file() {
        for (i, t) in [
            StorageType::Nfs,
            StorageType::Cifs,
            StorageType::GlusterFs,
            StorageType::CephFs,
        ]
        .into_iter()
        .enumerate()
        {
            let d = scratch(&format!("netdel-{i}"));
            std::fs::write(d.join("gone.qcow2"), b"x").unwrap();
            std::fs::write(d.join("keep.qcow2"), b"y").unwrap();
            let m = StorageManager::new();
            m.pools
                .write()
                .await
                .insert("n".into(), pool("n", t.clone(), d.to_str().unwrap()));
            m.delete_volume("n", "gone").await.unwrap_or_else(|e| {
                panic!("{t:?}: delete failed: {e}");
            });
            assert!(!d.join("gone.qcow2").exists(), "{t:?}");
            // The pool directory itself (and its other volumes) stays.
            assert!(d.join("keep.qcow2").is_file(), "{t:?}");
            let _ = std::fs::remove_dir_all(&d);
        }
    }

    #[tokio::test]
    async fn resize_to_same_size_or_exactly_the_free_space_is_allowed() {
        let gb = 1024 * 1024 * 1024;
        let d = scratch("resize-edge");
        let m = pinned();
        let mut p = pool("r", StorageType::Directory, d.to_str().unwrap());
        p.available = 10;
        m.pools.write().await.insert("r".into(), p);
        m.create_volume("r", "v1", 1).await.unwrap();
        // Only 1 GB left: growing 1 -> 2 GB needs exactly that.
        m.pools.write().await.get_mut("r").unwrap().available = 1;
        m.resize_volume("r", "v1", 2).await.unwrap();
        assert_eq!(qcow2_virtual_size(&d.join("v1.qcow2")), Some(2 * gb));
        // Same size with no free space left is a no-op grow, not a shrink.
        m.pools.write().await.get_mut("r").unwrap().available = 0;
        m.resize_volume("r", "v1", 2).await.unwrap();
        assert_eq!(qcow2_virtual_size(&d.join("v1.qcow2")), Some(2 * gb));
        // One more GB than is free is refused.
        assert!(matches!(
            m.resize_volume("r", "v1", 3).await,
            Err(horcrux_common::Error::InvalidConfig(_))
        ));
        let _ = std::fs::remove_dir_all(&d);
    }
}
