//! "Adopt in place": take over the guests of a Proxmox VE installation that
//! lives on THIS host's ZFS pool (Horcrux booted from a new root dataset in
//! the same pool), without copying a byte of guest data.
//!
//! * The guest configs are read from the Proxmox root file system mounted
//!   read only (`pve_root`): Proxmox keeps them in its cluster database
//!   (`var/lib/pve-cluster/config.db`; `/etc/pve` is a FUSE view of it and
//!   is empty on disk), or from a copy of `/etc/pve` under `pve_root`.
//!   `storage.cfg` says which ZFS dataset backs each storage.
//! * Each referenced volume becomes `zfs clone <src>@<snapshot> <target>`
//!   under `target_parent_dataset` (O(1), no data moves). The Proxmox
//!   datasets themselves are never written: booting Proxmox again IS the
//!   rollback, and `zfs destroy` of the clones undoes the adoption.
//! * VM disks are the cloned zvols (`/dev/zvol/<clone>`, raw); the UEFI
//!   variables, the TPM state and the cloud-init seed are read from
//!   temporary clones into the VM's sidecar directory. A container's rootfs
//!   is the cloned subvolume mounted at `<lxc path>/<ct>/rootfs` (owners are
//!   already in the same 100000/65536 id map: nothing is chowned), managed
//!   mount points are cloned the same way, bind mounts keep their paths.
//! * Clones carry `horcrux:guest=<id>`: deleting the guest destroys them,
//!   never anything else. Without a snapshot (`allow_no_snapshot`) the
//!   Proxmox datasets are RENAMED into place instead: the guest then writes
//!   Proxmox's own data and booting Proxmox again no longer rolls back.
use super::conf::{GuestPlan, PveVol, Resolved, Row, VolKind, VolRole};
use super::source::{Guest, Kind};
use crate::error::ApiError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The ZFS property that names the guest a clone was made for.
pub const OWNER_PROP: &str = crate::vm::sidecar::OWNER_PROP;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdoptSource {
    /// The Proxmox root file system (mounted read only), or a directory
    /// with a copy of `/etc/pve` (`etc/pve/...` or `qemu-server/` + `lxc/`).
    pub pve_root: String,
    /// The snapshot every volume is cloned from (`zfs snapshot -r
    /// <pool>/data@<name>` taken after the guests stopped).
    #[serde(default)]
    pub snapshot: Option<String>,
    /// Dataset the clones are created under (same pool as the volumes;
    /// created with mountpoint=none if missing).
    pub target_parent_dataset: String,
    /// Proxmox storage id -> dataset prefix (default: storage.cfg's pool).
    #[serde(default)]
    pub storage_map: BTreeMap<String, String>,
    /// Use the Proxmox datasets directly (renamed into place) when no
    /// snapshot is given. Writes then reach Proxmox's data.
    #[serde(default)]
    pub allow_no_snapshot: bool,
    /// Saved `qm showcmd` output (a dump file with `===== VM <id>` headers
    /// or a directory as for the dir source), for the command-line diff.
    #[serde(default)]
    pub showcmd: Option<String>,
    /// Proxmox node whose guests are adopted (default: the root's
    /// etc/hostname, or the only node).
    #[serde(default)]
    pub node: Option<String>,
    /// Seconds to wait for a cloned zvol's device node (udev).
    #[serde(default)]
    pub device_wait_secs: Option<u64>,
}

/// Default for `device_wait_secs`.
pub const DEFAULT_DEVICE_WAIT_SECS: u64 = 30;

fn valid_dataset(ds: &str) -> bool {
    !ds.is_empty()
        && !ds.starts_with('/')
        && !ds.ends_with('/')
        && !ds.contains("..")
        && !ds.contains("//")
        && !ds.starts_with('-')
        && ds
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.' | ':'))
}

fn valid_snapshot(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 200
        && !s.starts_with('-')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
}

impl AdoptSource {
    pub fn validate(&self) -> Result<(), ApiError> {
        let p = super::source::safe_path(&self.pve_root)?;
        if !p.is_dir() {
            return Err(ApiError::BadRequest(format!(
                "{} is not a directory on this host (mount the Proxmox root file system read only without changing its properties, e.g. mkdir -p /mnt/pve-root && mount -t zfs -o ro,zfsutil <pool>/ROOT/pve-1 /mnt/pve-root)",
                self.pve_root
            )));
        }
        if !valid_dataset(&self.target_parent_dataset) {
            return Err(ApiError::BadRequest(format!(
                "target_parent_dataset {:?} is not a ZFS dataset name",
                self.target_parent_dataset
            )));
        }
        match &self.snapshot {
            Some(s) if !valid_snapshot(s) => {
                return Err(ApiError::BadRequest(format!(
                    "snapshot {s:?} is not a ZFS snapshot name (the part after @)"
                )))
            }
            None if !self.allow_no_snapshot => {
                return Err(ApiError::BadRequest(
                    "adopting in place needs a snapshot to clone from (snapshot: the name after @, taken with `zfs snapshot -r <pool>/data@<name>` once the guests are stopped); without one the Proxmox datasets themselves would be used and written (set allow_no_snapshot to accept that: booting Proxmox again is then no rollback)".into(),
                ))
            }
            _ => {}
        }
        for (id, ds) in &self.storage_map {
            if !valid_dataset(ds) {
                return Err(ApiError::BadRequest(format!(
                    "storage {id}: {ds:?} is not a ZFS dataset name"
                )));
            }
        }
        if let Some(n) = &self.node {
            if n.is_empty() || n.contains('/') || n.contains("..") {
                return Err(ApiError::BadRequest(format!("{n:?} is not a node name")));
            }
        }
        if let Some(s) = &self.showcmd {
            super::source::safe_path(s)?;
        }
        Ok(())
    }

    pub fn device_wait(&self) -> std::time::Duration {
        std::time::Duration::from_secs(
            self.device_wait_secs
                .unwrap_or(DEFAULT_DEVICE_WAIT_SECS)
                .clamp(1, 600),
        )
    }
}

// ------------------------------------------------------------------ configs

/// `/etc/pve` as path -> content (`nodes/<n>/qemu-server/100.conf`,
/// `storage.cfg`), and where it was read from.
#[derive(Debug, Clone, Default)]
pub struct PveTree {
    pub files: BTreeMap<String, Vec<u8>>,
    pub origin: String,
}

fn walk(base: &Path, rel: &str, depth: u32, out: &mut BTreeMap<String, Vec<u8>>) {
    let Ok(rd) = std::fs::read_dir(base.join(rel)) else {
        return;
    };
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        let r = if rel.is_empty() {
            n.clone()
        } else {
            format!("{rel}/{n}")
        };
        let Ok(m) = std::fs::metadata(e.path()) else {
            continue;
        };
        if m.is_dir() && depth > 0 {
            walk(base, &r, depth - 1, out);
        } else if m.is_file() && m.len() < (4 << 20) {
            if let Ok(b) = std::fs::read(e.path()) {
                out.insert(r, b);
            }
        }
    }
}

fn has_confs(files: &BTreeMap<String, Vec<u8>>) -> bool {
    files.keys().any(|k| conf_path(k).is_some())
}

/// `(node, kind, vmid)` of a guest config path in the tree.
pub fn conf_path(path: &str) -> Option<(Option<String>, Kind, u32)> {
    let parts: Vec<&str> = path.split('/').collect();
    let (node, dir, file) = match parts.as_slice() {
        ["nodes", n, d, f] => (Some(n.to_string()), *d, *f),
        [d, f] => (None, *d, *f),
        _ => return None,
    };
    let kind = match dir {
        "qemu-server" => Kind::Vm,
        "lxc" => Kind::Ct,
        _ => return None,
    };
    let vmid = file.strip_suffix(".conf")?.parse().ok()?;
    Some((node, kind, vmid))
}

/// Paths of pmxcfs's `tree` table rows (`inode, parent, type, name`):
/// type 4 = directory, 8 = file; the root directory is inode 0.
pub fn tree_paths(rows: &[(i64, i64, i64, String)]) -> BTreeMap<i64, String> {
    let by: BTreeMap<i64, (i64, i64, &str)> = rows
        .iter()
        .map(|(i, p, t, n)| (*i, (*p, *t, n.as_str())))
        .collect();
    let mut out = BTreeMap::new();
    for (i, (_, t, _)) in &by {
        if *t != 8 || *i == 0 {
            continue;
        }
        let mut parts: Vec<&str> = Vec::new();
        let mut cur = *i;
        let mut ok = true;
        for _ in 0..16 {
            if cur == 0 {
                break;
            }
            match by.get(&cur) {
                Some((p, _, n)) => {
                    parts.push(n);
                    cur = *p;
                }
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if ok && cur == 0 {
            parts.reverse();
            out.insert(*i, parts.join("/"));
        }
    }
    out
}

/// Read Proxmox's cluster database (a copy: the root is read only and the
/// write-ahead log must be replayed).
async fn read_config_db(db: &Path) -> Result<BTreeMap<String, Vec<u8>>, ApiError> {
    use sqlx::Row as _;
    let tmp = tempfile::tempdir().map_err(|e| ApiError::Internal(format!("temp dir: {e}")))?;
    let copy = tmp.path().join("config.db");
    std::fs::copy(db, &copy).map_err(|e| ApiError::BadRequest(format!("{}: {e}", db.display())))?;
    for ext in ["-wal", "-shm"] {
        let side = PathBuf::from(format!("{}{ext}", db.display()));
        if side.is_file() {
            let _ = std::fs::copy(&side, tmp.path().join(format!("config.db{ext}")));
        }
    }
    let opts = sqlx::sqlite::SqliteConnectOptions::new().filename(&copy);
    let mut conn = sqlx::ConnectOptions::connect(&opts)
        .await
        .map_err(|e| ApiError::BadRequest(format!("{} is not readable: {e}", db.display())))?;
    let rows = sqlx::query("SELECT inode, parent, type, name, data FROM tree")
        .fetch_all(&mut conn)
        .await
        .map_err(|e| {
            ApiError::BadRequest(format!(
                "{} is not a Proxmox cluster database: {e}",
                db.display()
            ))
        })?;
    let mut meta = Vec::new();
    let mut data: BTreeMap<i64, Vec<u8>> = BTreeMap::new();
    for r in rows {
        let inode: i64 = r.try_get("inode").unwrap_or(-1);
        let parent: i64 = r.try_get("parent").unwrap_or(-1);
        let t: i64 = r.try_get("type").unwrap_or(0);
        let name: String = r.try_get("name").unwrap_or_default();
        let d: Option<Vec<u8>> = r.try_get("data").ok();
        meta.push((inode, parent, t, name));
        if let Some(d) = d {
            data.insert(inode, d);
        }
    }
    let paths = tree_paths(&meta);
    Ok(paths
        .into_iter()
        .map(|(i, p)| (p, data.remove(&i).unwrap_or_default()))
        .collect())
}

pub async fn read_tree(root: &Path) -> Result<PveTree, ApiError> {
    let mut files = BTreeMap::new();
    walk(&root.join("etc/pve"), "", 4, &mut files);
    if has_confs(&files) {
        return Ok(PveTree {
            files,
            origin: format!("{}/etc/pve", root.display()),
        });
    }
    let db = root.join("var/lib/pve-cluster/config.db");
    if db.is_file() {
        let files = read_config_db(&db).await?;
        return Ok(PveTree {
            files,
            origin: db.display().to_string(),
        });
    }
    let mut files = BTreeMap::new();
    walk(root, "", 3, &mut files);
    if has_confs(&files) {
        return Ok(PveTree {
            files,
            origin: root.display().to_string(),
        });
    }
    Err(ApiError::BadRequest(format!(
        "no Proxmox guest configs under {}: expected the Proxmox root file system (var/lib/pve-cluster/config.db) or a copy of /etc/pve",
        root.display()
    )))
}

/// The node whose guests are adopted.
fn pick_node(a: &AdoptSource, tree: &PveTree) -> Result<Option<String>, ApiError> {
    let nodes: std::collections::BTreeSet<String> = tree
        .files
        .keys()
        .filter_map(|k| conf_path(k).and_then(|(n, _, _)| n))
        .collect();
    if nodes.is_empty() {
        return Ok(None);
    }
    if let Some(n) = &a.node {
        return if nodes.contains(n) {
            Ok(Some(n.clone()))
        } else {
            Err(ApiError::BadRequest(format!(
                "the Proxmox installation has no node {n} (nodes: {})",
                nodes.iter().cloned().collect::<Vec<_>>().join(", ")
            )))
        };
    }
    let host = std::fs::read_to_string(Path::new(&a.pve_root).join("etc/hostname"))
        .ok()
        .map(|h| h.trim().split('.').next().unwrap_or("").to_string())
        .filter(|h| !h.is_empty());
    if let Some(h) = host.filter(|h| nodes.contains(h)) {
        return Ok(Some(h));
    }
    if nodes.len() == 1 {
        return Ok(nodes.into_iter().next());
    }
    Err(ApiError::BadRequest(format!(
        "the Proxmox cluster database has several nodes ({}): say which one runs on this host (node)",
        nodes.into_iter().collect::<Vec<_>>().join(", ")
    )))
}

pub async fn list(a: &AdoptSource) -> Result<Vec<Guest>, ApiError> {
    let tree = read_tree(Path::new(&a.pve_root)).await?;
    let node = pick_node(a, &tree)?;
    let showcmd = match &a.showcmd {
        Some(s) if Path::new(s).is_dir() => super::source::read_showcmds(Path::new(s)),
        Some(s) => std::fs::read_to_string(s)
            .map(|t| super::cmdline::split_showcmd_dump(&t))
            .unwrap_or_default(),
        None => BTreeMap::new(),
    };
    let mut out: BTreeMap<u32, Guest> = BTreeMap::new();
    for (path, data) in &tree.files {
        let Some((n, kind, vmid)) = conf_path(path) else {
            continue;
        };
        if n.is_some() && n != node {
            continue;
        }
        out.entry(vmid).or_insert_with(|| Guest {
            vmid,
            kind,
            conf: String::from_utf8_lossy(data).to_string(),
            showcmd: showcmd.get(&vmid).cloned(),
        });
    }
    if out.is_empty() {
        return Err(ApiError::BadRequest(format!(
            "no guests in {} for node {}",
            tree.origin,
            node.unwrap_or_else(|| "-".into())
        )));
    }
    Ok(out.into_values().collect())
}

pub async fn storage_cfg(a: &AdoptSource) -> BTreeMap<String, (String, String)> {
    match read_tree(Path::new(&a.pve_root)).await {
        Ok(t) => t
            .files
            .get("storage.cfg")
            .map(|b| super::source::parse_storage_cfg(&String::from_utf8_lossy(b)))
            .unwrap_or_default(),
        Err(_) => BTreeMap::new(),
    }
}

// ------------------------------------------------------------------ zfs

fn zfs() -> std::process::Command {
    std::process::Command::new(std::env::var("HORCRUX_ZFS").unwrap_or_else(|_| "zfs".into()))
}

fn atokio_zfs() -> tokio::process::Command {
    tokio::process::Command::new(std::env::var("HORCRUX_ZFS").unwrap_or_else(|_| "zfs".into()))
}

/// `zfs get -Hp` of some properties (None: no such dataset).
pub fn zfs_props(ds: &str, props: &[&str]) -> Option<BTreeMap<String, String>> {
    let o = zfs()
        .args(["get", "-Hp", "-o", "property,value", &props.join(","), ds])
        .output()
        .ok()?;
    if !o.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter_map(|l| {
                let (k, v) = l.split_once('\t')?;
                Some((k.to_string(), v.to_string()))
            })
            .collect(),
    )
}

pub fn exists(ds: &str) -> bool {
    zfs()
        .args(["list", "-H", "-o", "name", ds])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Properties set locally on the source that a clone must get again
/// (clones inherit from their NEW parent): acltype/xattr of a container
/// subvolume, volblocksize is kept by ZFS itself. Size limits are set from
/// the Proxmox config; placement properties are Horcrux's.
pub fn local_props(ds: &str) -> Vec<(String, String)> {
    let Ok(o) = zfs()
        .args([
            "get",
            "-H",
            "-s",
            "local",
            "-o",
            "property,value",
            "all",
            ds,
        ])
        .output()
    else {
        return vec![];
    };
    String::from_utf8_lossy(&o.stdout)
        .lines()
        .filter_map(|l| {
            let (k, v) = l.split_once('\t')?;
            let skip = matches!(
                k,
                "mountpoint"
                    | "canmount"
                    | "refreservation"
                    | "reservation"
                    | "quota"
                    | "refquota"
                    | "volsize"
                    | "readonly"
                    | "sharenfs"
                    | "sharesmb"
                    | "zoned"
                    | "keylocation"
                    | "keyformat"
                    | "encryption"
            ) || k.contains(':');
            (!skip).then(|| (k.to_string(), v.to_string()))
        })
        .collect()
}

// ------------------------------------------------------------------ resolve

/// The dataset behind a Proxmox volume (`local-zfs:vm-117-disk-1`).
pub fn source_dataset(
    a: &AdoptSource,
    cfg: &BTreeMap<String, (String, String)>,
    v: &PveVol,
) -> Result<String, String> {
    let Some(sid) = &v.storage else {
        return Err(format!(
            "{} is a host path, not a storage volume: adopt in place takes over ZFS volumes only",
            v.volume
        ));
    };
    let prefix = match a.storage_map.get(sid) {
        Some(p) => p.clone(),
        None => match cfg.get(sid) {
            Some((t, pool)) if t == "zfspool" && !pool.is_empty() => pool.clone(),
            Some((t, _)) => {
                return Err(format!(
                    "storage {sid} is a {t} storage: adopt in place clones ZFS volumes only (import this guest with the directory or SSH source, which copies it)"
                ))
            }
            None => {
                return Err(format!(
                    "storage {sid} is not in storage.cfg: map it to its dataset in storage_map"
                ))
            }
        },
    };
    // linked clones: `base-100-disk-0/vm-101-disk-0` is <pool>/vm-101-disk-0
    let base = v.volume.rsplit('/').next().unwrap_or(&v.volume);
    let ds = format!("{}/{base}", prefix.trim_end_matches('/'));
    if !valid_dataset(&ds) {
        return Err(format!("{ds:?} is not a dataset name"));
    }
    Ok(ds)
}

/// Resolver for the plan: the SOURCE volume as it is now.
pub fn resolve(
    a: &AdoptSource,
    cfg: &BTreeMap<String, (String, String)>,
    v: &PveVol,
) -> Result<Resolved, String> {
    let ds = source_dataset(a, cfg, v)?;
    let p = zfs_props(&ds, &["type", "volsize", "mounted", "mountpoint", "used"])
        .ok_or_else(|| format!("dataset {ds} does not exist on this host"))?;
    match p.get("type").map(String::as_str) {
        Some("volume") => Ok(Resolved {
            path: format!("/dev/zvol/{ds}"),
            kind: VolKind::Block,
            format: "raw".into(),
            size: p.get("volsize").and_then(|s| s.parse().ok()),
        }),
        Some("filesystem") => Ok(Resolved {
            path: p
                .get("mountpoint")
                .filter(|m| m.starts_with('/'))
                .cloned()
                .unwrap_or_else(|| format!("zfs:{ds}")),
            kind: VolKind::Dir,
            format: "zfs".into(),
            size: p.get("used").and_then(|s| s.parse().ok()),
        }),
        other => Err(format!(
            "dataset {ds} is a {} (a volume or file system was expected)",
            other.unwrap_or("?")
        )),
    }
}

// ------------------------------------------------------------------ plan

/// One volume taken over.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdoptVol {
    /// Proxmox key (`scsi0`, `efidisk0`, `rootfs`, `mp0`).
    pub key: String,
    pub role: VolRole,
    pub source_dataset: String,
    /// `<source>@<snapshot>` cloned from (None: renamed, no snapshot).
    pub from_snapshot: Option<String>,
    /// The dataset the guest uses.
    pub dataset: String,
    /// `volume` or `filesystem`.
    pub zfs_type: String,
    /// Kept as the guest's volume (disks, rootfs, mount points); the
    /// firmware/TPM/cloud-init volumes are read into the sidecar dir and
    /// their temporary clone dropped.
    pub kept: bool,
    /// The Proxmox size limit (`size=`), set as refquota on file systems.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// Bytes the source changed since the snapshot (must be 0 to adopt).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub written_since: Option<u64>,
    /// The exact command apply runs.
    pub command: String,
}

/// `<parent>/<guest id>-<key>`; temporary ones end in `-tmp`.
pub fn clone_name(parent: &str, id: &str, key: &str, kept: bool) -> String {
    format!(
        "{}/{id}-{key}{}",
        parent.trim_end_matches('/'),
        if kept { "" } else { "-tmp" }
    )
}

fn pool_of(ds: &str) -> &str {
    ds.split('/').next().unwrap_or(ds)
}

/// The volumes of a planned guest as clones under `id`; problems go to
/// `blockers` (nothing is created here).
pub fn volumes(
    a: &AdoptSource,
    cfg: &BTreeMap<String, (String, String)>,
    p: &GuestPlan,
    id: &str,
    blockers: &mut Vec<String>,
) -> Vec<AdoptVol> {
    let mut out = Vec::new();
    for v in &p.volumes {
        let pv = PveVol::parse(&v.source);
        let ds = match source_dataset(a, cfg, &pv) {
            Ok(d) => d,
            Err(_) => continue, // already a "not found" blocker
        };
        let kept = matches!(v.role, VolRole::Disk | VolRole::Rootfs | VolRole::Mount);
        let target = clone_name(&a.target_parent_dataset, id, &v.key, kept);
        let props = zfs_props(&ds, &["type"]).unwrap_or_default();
        let zt = props.get("type").cloned().unwrap_or_default();
        if pool_of(&ds) != pool_of(&a.target_parent_dataset) {
            blockers.push(format!(
                "{}: {ds} is in pool {}, but the clones go under {} (pool {}): a clone must be in the same pool",
                v.key,
                pool_of(&ds),
                a.target_parent_dataset,
                pool_of(&a.target_parent_dataset)
            ));
        }
        if exists(&target) {
            blockers.push(format!(
                "{}: {target} already exists (adopted before? delete that guest first, or pick another id)",
                v.key
            ));
        }
        let (from, written, command) = match &a.snapshot {
            Some(s) => {
                let snap = format!("{ds}@{s}");
                let w = zfs_props(&ds, &[&format!("written@{s}")])
                    .and_then(|m| m.values().next().cloned())
                    .and_then(|x| x.parse::<u64>().ok());
                if !exists(&snap) {
                    blockers.push(format!(
                        "{}: snapshot {snap} does not exist: take it once the guest is stopped (zfs snapshot -r {}@{s})",
                        v.key,
                        ds.rsplit_once('/').map(|x| x.0).unwrap_or(&ds)
                    ));
                }
                let mut c = format!("zfs clone -o {OWNER_PROP}={id}");
                if zt == "filesystem" {
                    c.push_str(" -o mountpoint=none");
                    if let Some(sz) = v.size_bytes {
                        c.push_str(&format!(" -o refquota={sz}"));
                    }
                }
                for (k, val) in local_props(&ds) {
                    c.push_str(&format!(" -o {k}={val}"));
                }
                c.push_str(&format!(" {snap} {target}"));
                (Some(snap), w, c)
            }
            None => (None, None, format!("zfs rename {ds} {target}")),
        };
        out.push(AdoptVol {
            key: v.key.clone(),
            role: v.role,
            source_dataset: ds,
            from_snapshot: from,
            dataset: target,
            zfs_type: zt,
            kept,
            size_bytes: v.size_bytes,
            written_since: written,
            command,
        });
    }
    out
}

/// Turn a copy plan into an adopt plan: rows say what happens to the
/// volumes, a container's rootfs is the dataset (no fixed-size image), and
/// every adopt problem is a blocker.
pub fn finish_plan(a: &AdoptSource, cfg: &BTreeMap<String, (String, String)>, p: &mut GuestPlan) {
    let mut blockers = Vec::new();
    let id = p.target_id.clone();
    let vols = volumes(a, cfg, p, &id, &mut blockers);
    if let Some(ct) = p.ct.as_mut() {
        if let Some(o) = ct.lxc.as_mut() {
            o.disk_gb = None;
            if o.unprivileged
                && o.idmap_base
                    .unwrap_or(horcrux_common::lxc::LXC_DEFAULT_IDMAP_BASE)
                    != horcrux_common::lxc::LXC_DEFAULT_IDMAP_BASE
            {
                blockers.push("the container's id map differs from Proxmox's 100000/65536: its files would have the wrong owners in place (import it with the directory source, which shifts them)".into());
            }
        }
    }
    for v in &vols {
        let what = match v.role {
            VolRole::Disk => "VM disk = the clone's zvol, raw (nothing copied)",
            VolRole::Rootfs => "rootfs = the clone, mounted as the container's root (nothing copied, owners as they are)",
            VolRole::Mount => "mount point = the clone, mounted in place (nothing copied)",
            VolRole::Efivars => "read into the VM's UEFI variable store, temporary clone dropped",
            VolRole::Tpm => "read into the VM's TPM state, temporary clone dropped",
            VolRole::Cloudinit => "read into the VM's cloud-init seed, temporary clone dropped",
        };
        if let Some(r) = p.rows.iter_mut().find(|r| r.key == v.key) {
            r.carry = super::conf::Carry::Changed;
            r.horcrux = what.into();
            r.note = format!("{}; {}", v.command, r.note.trim_start_matches("; "));
        }
        if matches!(v.written_since, Some(n) if n > 0) {
            blockers.push(format!(
                "{}: {} changed after snapshot {} ({} bytes written since): the clone would miss those writes. Stop the guest and take the snapshot again",
                v.key,
                v.source_dataset,
                v.from_snapshot.clone().unwrap_or_default(),
                v.written_since.unwrap_or(0)
            ));
        }
    }
    if a.snapshot.is_none() {
        p.rows.push(Row {
            key: "adopt".into(),
            value: "no snapshot".into(),
            carry: super::conf::Carry::Changed,
            horcrux: "the Proxmox datasets are renamed into place".into(),
            note: "the guest writes Proxmox's own volumes: booting Proxmox again is NO rollback"
                .into(),
        });
    }
    p.blockers.extend(blockers);
    p.adopt = vols;
}

// ------------------------------------------------------------------ apply

/// Datasets created by an apply, destroyed again if the guest fails.
#[derive(Debug, Default)]
pub struct Made {
    pub created: Vec<String>,
    /// (renamed-to, original name): renamed back on failure.
    pub renamed: Vec<(String, String)>,
}

impl Made {
    pub async fn undo(&mut self) {
        for d in self.created.drain(..).rev() {
            let _ = atokio_zfs().args(["destroy", &d]).output().await;
        }
        for (now, orig) in self.renamed.drain(..).rev() {
            let _ = atokio_zfs().args(["rename", &now, &orig]).output().await;
        }
    }
}

async fn run_zfs(args: &[String]) -> Result<(), ApiError> {
    let o = atokio_zfs()
        .args(args)
        .output()
        .await
        .map_err(|e| ApiError::Internal(format!("zfs: {e}")))?;
    if !o.status.success() {
        return Err(ApiError::Internal(format!(
            "zfs {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&o.stderr).trim()
        )));
    }
    Ok(())
}

/// The parent of the clones (created with mountpoint=none when missing).
pub async fn ensure_parent(a: &AdoptSource) -> Result<(), ApiError> {
    if exists(&a.target_parent_dataset) {
        return Ok(());
    }
    run_zfs(&[
        "create".into(),
        "-p".into(),
        "-o".into(),
        "mountpoint=none".into(),
        a.target_parent_dataset.clone(),
    ])
    .await
}

/// Create one volume's dataset (clone or rename) and wait for a zvol's
/// device. Returns the device path (volumes) or the dataset (file systems).
pub async fn materialize(
    a: &AdoptSource,
    v: &AdoptVol,
    id: &str,
    made: &mut Made,
) -> Result<String, ApiError> {
    // the source must not have moved on since the plan
    if let (Some(s), Some(snap)) = (&a.snapshot, &v.from_snapshot) {
        let w = zfs_props(&v.source_dataset, &[&format!("written@{s}")])
            .and_then(|m| m.values().next().cloned())
            .and_then(|x| x.parse::<u64>().ok());
        if matches!(w, Some(n) if n > 0) {
            return Err(ApiError::Conflict(format!(
                "{}: {} changed after {snap}: stop the guest and take the snapshot again",
                v.key, v.source_dataset
            )));
        }
    }
    match &v.from_snapshot {
        Some(snap) => {
            let mut args: Vec<String> =
                vec!["clone".into(), "-o".into(), format!("{OWNER_PROP}={id}")];
            if v.zfs_type == "filesystem" {
                args.extend(["-o".into(), "mountpoint=none".into()]);
                if let Some(sz) = v.size_bytes {
                    args.extend(["-o".into(), format!("refquota={sz}")]);
                }
            }
            for (k, val) in local_props(&v.source_dataset) {
                args.extend(["-o".into(), format!("{k}={val}")]);
            }
            args.push(snap.clone());
            args.push(v.dataset.clone());
            run_zfs(&args).await?;
            made.created.push(v.dataset.clone());
        }
        None => {
            run_zfs(&["rename".into(), v.source_dataset.clone(), v.dataset.clone()]).await?;
            made.renamed
                .push((v.dataset.clone(), v.source_dataset.clone()));
        }
    }
    if v.zfs_type != "volume" {
        return Ok(v.dataset.clone());
    }
    let dev = PathBuf::from(format!("/dev/zvol/{}", v.dataset));
    let deadline = std::time::Instant::now() + a.device_wait();
    loop {
        if dev.exists() {
            return Ok(dev.to_string_lossy().to_string());
        }
        if std::time::Instant::now() > deadline {
            return Err(ApiError::Internal(format!(
                "{}: the device {} did not appear within {} s (udev zvol rules installed?)",
                v.key,
                dev.display(),
                a.device_wait().as_secs()
            )));
        }
        let _ = tokio::process::Command::new("udevadm")
            .args(["settle", "-t", "2"])
            .output()
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// Drop a temporary clone (firmware/TPM/cloud-init source).
pub async fn drop_temp(v: &AdoptVol, made: &mut Made) {
    if v.from_snapshot.is_some()
        && !v.kept
        && run_zfs(&["destroy".into(), v.dataset.clone()])
            .await
            .is_ok()
    {
        made.created.retain(|d| d != &v.dataset);
    }
}

/// `<data_dir>/proxmox-import/<id>.adopt.json`: what was taken over.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdoptRecord {
    pub id: String,
    pub vmid: u32,
    pub snapshot: Option<String>,
    pub volumes: Vec<AdoptVol>,
    /// Seconds the apply of this guest took (clones + definition).
    pub apply_secs: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src() -> AdoptSource {
        AdoptSource {
            pve_root: "/mnt/pve".into(),
            snapshot: Some("hx-cutover".into()),
            target_parent_dataset: "tank/horcrux".into(),
            storage_map: BTreeMap::new(),
            allow_no_snapshot: false,
            showcmd: None,
            node: None,
            device_wait_secs: None,
        }
    }

    #[test]
    fn config_paths_of_the_cluster_tree() {
        assert_eq!(
            conf_path("nodes/pve1/qemu-server/117.conf"),
            Some((Some("pve1".into()), Kind::Vm, 117))
        );
        assert_eq!(conf_path("lxc/110.conf"), Some((None, Kind::Ct, 110)));
        assert_eq!(conf_path("nodes/pve1/lxc/x.conf"), None);
        assert_eq!(conf_path("storage.cfg"), None);
        assert_eq!(conf_path("nodes/pve1/openvz/1.conf"), None);
    }

    #[test]
    fn pmxcfs_tree_rows_become_paths() {
        // root dir inode 0 (its own parent), nodes/<n>/{qemu-server,lxc}
        let rows = vec![
            (0, 0, 4, "".to_string()),
            (2, 0, 8, "storage.cfg".to_string()),
            (3, 0, 4, "nodes".to_string()),
            (4, 3, 4, "pve1".to_string()),
            (5, 4, 4, "qemu-server".to_string()),
            (6, 4, 4, "lxc".to_string()),
            (7, 5, 8, "117.conf".to_string()),
            (8, 6, 8, "110.conf".to_string()),
            (9, 42, 8, "orphan.conf".to_string()),
        ];
        let p = tree_paths(&rows);
        assert_eq!(p.get(&2).unwrap(), "storage.cfg");
        assert_eq!(p.get(&7).unwrap(), "nodes/pve1/qemu-server/117.conf");
        assert_eq!(p.get(&8).unwrap(), "nodes/pve1/lxc/110.conf");
        assert!(!p.contains_key(&9) && !p.contains_key(&4));
    }

    #[test]
    fn volumes_map_to_their_dataset_and_clone_name() {
        let a = src();
        let mut cfg = BTreeMap::new();
        cfg.insert("local-zfs".into(), ("zfspool".into(), "tank/data".into()));
        cfg.insert("local".into(), ("dir".into(), "/var/lib/vz".into()));
        let v = PveVol::parse("local-zfs:vm-117-disk-1,discard=on,size=8G");
        assert_eq!(
            source_dataset(&a, &cfg, &v).unwrap(),
            "tank/data/vm-117-disk-1"
        );
        let linked = PveVol::parse("local-zfs:base-100-disk-0/vm-101-disk-0");
        assert_eq!(
            source_dataset(&a, &cfg, &linked).unwrap(),
            "tank/data/vm-101-disk-0"
        );
        let e =
            source_dataset(&a, &cfg, &PveVol::parse("local:117/vm-117-disk-0.qcow2")).unwrap_err();
        assert!(e.contains("dir storage") && e.contains("copies"), "{e}");
        assert!(source_dataset(&a, &cfg, &PveVol::parse("/dev/sdb")).is_err());
        let mut m = a.clone();
        m.storage_map.insert("local-zfs".into(), "rpool/pve".into());
        assert_eq!(
            source_dataset(&m, &cfg, &v).unwrap(),
            "rpool/pve/vm-117-disk-1"
        );
        assert_eq!(
            clone_name("tank/horcrux/", "edge-router", "scsi0", true),
            "tank/horcrux/edge-router-scsi0"
        );
        assert_eq!(
            clone_name("tank/horcrux", "edge-router", "efidisk0", false),
            "tank/horcrux/edge-router-efidisk0-tmp"
        );
    }

    #[test]
    fn a_snapshot_is_required_unless_explicitly_waived() {
        let t = tempfile::tempdir().unwrap();
        let mut a = src();
        a.pve_root = t.path().display().to_string();
        assert!(a.validate().is_ok());
        a.snapshot = None;
        let e = format!("{:?}", a.validate().unwrap_err());
        assert!(e.contains("allow_no_snapshot"), "{e}");
        a.allow_no_snapshot = true;
        assert!(a.validate().is_ok());
        a.snapshot = Some("bad name".into());
        assert!(a.validate().is_err());
        a.snapshot = Some("ok".into());
        a.target_parent_dataset = "/tank".into();
        assert!(a.validate().is_err());
        a.target_parent_dataset = "tank/../x".into();
        assert!(a.validate().is_err());
    }

    #[tokio::test]
    async fn configs_are_read_from_the_cluster_database() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        std::fs::create_dir_all(root.join("var/lib/pve-cluster")).unwrap();
        std::fs::create_dir_all(root.join("etc/pve")).unwrap(); // empty FUSE mountpoint
        std::fs::write(root.join("etc/hostname"), "pve1\n").unwrap();
        let db = root.join("var/lib/pve-cluster/config.db");
        let opts = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&db)
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);
        let mut c = sqlx::ConnectOptions::connect(&opts).await.unwrap();
        sqlx::query("CREATE TABLE tree (inode INTEGER PRIMARY KEY NOT NULL, parent INTEGER NOT NULL, version INTEGER NOT NULL, writer INTEGER NOT NULL, mtime INTEGER NOT NULL, type INTEGER NOT NULL, name TEXT NOT NULL, data BLOB)")
            .execute(&mut c).await.unwrap();
        for (i, p, ty, n, d) in [
            (0i64, 0i64, 8i64, "__version__", None),
            (
                2,
                0,
                8,
                "storage.cfg",
                Some("zfspool: local-zfs\n\tpool tank/data\n"),
            ),
            (3, 0, 4, "nodes", None),
            (4, 3, 4, "pve1", None),
            (5, 4, 4, "qemu-server", None),
            (6, 4, 4, "lxc", None),
            (
                7,
                5,
                8,
                "117.conf",
                Some("name: edge-router\nscsi0: local-zfs:vm-117-disk-1,size=8G\n"),
            ),
            (
                8,
                6,
                8,
                "110.conf",
                Some("hostname: mirror\nrootfs: local-zfs:subvol-110-disk-0,size=8G\n"),
            ),
            (9, 3, 4, "pve2", None),
            (10, 9, 4, "qemu-server", None),
            (11, 10, 8, "200.conf", Some("name: other-node\n")),
        ] {
            sqlx::query("INSERT INTO tree VALUES (?, ?, 1, 0, 0, ?, ?, ?)")
                .bind(i)
                .bind(p)
                .bind(ty)
                .bind(n)
                .bind(d.map(|s: &str| s.as_bytes().to_vec()))
                .execute(&mut c)
                .await
                .unwrap();
        }
        // keep the connection open: the rows are only in the WAL
        let mut a = src();
        a.pve_root = root.display().to_string();
        let g = list(&a).await.unwrap();
        let ids: Vec<(u32, Kind)> = g.iter().map(|g| (g.vmid, g.kind)).collect();
        assert_eq!(ids, vec![(110, Kind::Ct), (117, Kind::Vm)]);
        assert!(g[1].conf.contains("edge-router"));
        let cfg = storage_cfg(&a).await;
        assert_eq!(cfg.get("local-zfs").unwrap().1, "tank/data");
        a.node = Some("pve2".into());
        assert_eq!(list(&a).await.unwrap()[0].vmid, 200);
        a.node = Some("nope".into());
        assert!(list(&a).await.is_err());
        drop(c);
    }
}
