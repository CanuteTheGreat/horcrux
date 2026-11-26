//! VM disks on Ceph RBD pools.
//!
//! A disk on a `ceph` storage pool is stored in `VmDisk::path` as
//! `rbd:<pool>/<image>` (the same string the storage crate's Ceph backend
//! returns). QEMU opens it through librbd with
//! `rbd:<pool>/<image>:id=<id>:conf=<ceph.conf>`; every management
//! operation goes through the `rbd` CLI. A VM whose first disk is an RBD
//! image keeps its console sockets, serial log, EFI vars, TPM state and
//! cloud-init seed next to a local *anchor* path
//! (`<vm storage dir>/<id>.qcow2`, never created), see [`anchor_path`].

use horcrux_common::{Error, Result};
use std::path::{Path, PathBuf};
use tokio::process::Command;

/// Prefix of a VM disk path that lives on RBD.
pub const PREFIX: &str = "rbd:";

/// Path of the Ceph client configuration (`HORCRUX_CEPH_CONF`).
pub fn conf_path() -> String {
    std::env::var("HORCRUX_CEPH_CONF")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "/etc/ceph/ceph.conf".to_string())
}

/// Ceph client id (`HORCRUX_CEPH_ID`, the part after `client.`); `None` =
/// librbd's default (`admin`).
pub fn client_id() -> Option<String> {
    std::env::var("HORCRUX_CEPH_ID")
        .ok()
        .map(|s| s.trim().trim_start_matches("client.").to_string())
        .filter(|s| !s.is_empty())
}

/// Whether a disk path is an RBD image (`rbd:pool/image`).
pub fn is_rbd(path: &str) -> bool {
    parse(path).is_some()
}

/// `(pool, image)` of an `rbd:pool/image` path (a trailing `@snapshot` is
/// not part of the image and is refused).
pub fn parse(path: &str) -> Option<(String, String)> {
    let rest = path.strip_prefix(PREFIX)?;
    let (pool, image) = rest.split_once('/')?;
    let ok = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    (ok(pool) && ok(image)).then(|| (pool.to_string(), image.to_string()))
}

/// `rbd:<pool>/<image>`.
pub fn path_of(pool: &str, image: &str) -> String {
    format!("{PREFIX}{pool}/{image}")
}

/// `<pool>/<image>`, the image spec the `rbd` CLI takes.
pub fn cli_spec(path: &str) -> Option<String> {
    parse(path).map(|(p, i)| format!("{p}/{i}"))
}

/// The RBD image name of a VM's `index`-th disk.
pub fn image_name(vm_id: &str, index: usize) -> String {
    if index == 0 {
        format!("vm-{vm_id}-disk-0")
    } else {
        format!("vm-{vm_id}-disk-{index}")
    }
}

/// QEMU `file=` value for an RBD disk path.
pub fn qemu_file(path: &str) -> Option<String> {
    let (pool, image) = parse(path)?;
    let mut s = format!("rbd:{pool}/{image}");
    if let Some(id) = client_id() {
        s.push_str(&format!(":id={id}"));
    }
    s.push_str(&format!(":conf={}", conf_path()));
    Some(s)
}

/// qemu-img/QEMU `json:` free spelling is not needed: this is the
/// `qemu-img` source/target spelling of an RBD path (same as `file=`).
pub fn qemu_img_target(path: &str) -> Option<String> {
    qemu_file(path)
}

static ANCHOR_DIR: std::sync::RwLock<Option<PathBuf>> = std::sync::RwLock::new(None);

/// Remember the node's VM storage directory (set when the QEMU manager is
/// created) so a VM config alone can name its anchor.
pub fn set_anchor_dir(dir: &Path) {
    if let Ok(mut g) = ANCHOR_DIR.write() {
        *g = Some(dir.to_path_buf());
    }
}

fn anchor_dir() -> PathBuf {
    ANCHOR_DIR
        .read()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_else(|| PathBuf::from("/var/lib/horcrux/vms"))
}

/// The path the VM's side files (QMP/serial/guest-agent sockets, EFI vars,
/// TPM state, cloud-init seed, hold marker) sit next to, from its first
/// disk path: that path itself for a file disk, the local anchor for an
/// RBD disk.
pub fn anchor_of(disk0: &str, vm_id: &str) -> PathBuf {
    if is_rbd(disk0) {
        anchor_path(&anchor_dir(), vm_id)
    } else {
        PathBuf::from(disk0)
    }
}

/// [`anchor_of`] for a VM config (`None` when it has no disk).
pub fn vm_anchor(vm: &horcrux_common::VmConfig) -> Option<PathBuf> {
    vm.disks.first().map(|d| anchor_of(&d.path, &vm.id))
}

/// The local anchor path a VM whose first disk is on RBD keeps its side
/// files next to (never created itself).
pub fn anchor_path(storage_dir: &Path, vm_id: &str) -> PathBuf {
    storage_dir.join(format!("{vm_id}.qcow2"))
}

fn lacks(reason: &'static str, message: String, hint: &str) -> Error {
    Error::NodeLacks {
        reason,
        message,
        hint: hint.to_string(),
    }
}

/// The node must have the `rbd` client, a Ceph config and a keyring for
/// the client id; else a [`Error::NodeLacks`] (answered as 424).
pub fn check_client() -> Result<()> {
    if !on_path("rbd") {
        return Err(lacks(
            "ceph-client-missing",
            "This node has no Ceph client (`rbd`), so it can't use VM disks on a Ceph pool.".into(),
            "Install the Ceph client packages (ceph-common) and copy ceph.conf and a keyring from the cluster.",
        ));
    }
    let conf = conf_path();
    if !Path::new(&conf).is_file() {
        return Err(lacks(
            "ceph-conf-missing",
            format!("The Ceph configuration {conf} is missing on this node."),
            "Copy ceph.conf from a cluster node, or set HORCRUX_CEPH_CONF.",
        ));
    }
    if !keyring_present(&conf) {
        return Err(lacks(
            "ceph-keyring-missing",
            "This node has no Ceph keyring for its client id.".into(),
            "Copy the client keyring (ceph.client.<id>.keyring) into /etc/ceph, or set `keyring` in ceph.conf.",
        ));
    }
    Ok(())
}

fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
        .unwrap_or(false)
}

fn keyring_present(conf: &str) -> bool {
    let id = client_id().unwrap_or_else(|| "admin".into());
    let dir = Path::new(conf)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("/etc/ceph"));
    if let Ok(text) = std::fs::read_to_string(conf) {
        for line in text.lines() {
            let l = line.trim();
            // cephx switched off: the cluster needs no key.
            let compact: String = l
                .chars()
                .filter(|c| !c.is_whitespace() && *c != '_')
                .collect();
            if compact.starts_with("authclientrequired=none")
                || compact.starts_with("authsupported=none")
            {
                return true;
            }
            if let Some(v) = l.strip_prefix("keyring") {
                let v = v.trim_start().trim_start_matches('=').trim();
                if !v.is_empty() && v.split(',').any(|f| Path::new(f.trim()).is_file()) {
                    return true;
                }
            }
            if l.starts_with("key") && !l.starts_with("keyring") && l.contains('=') {
                return true;
            }
        }
    }
    dir.join(format!("ceph.client.{id}.keyring")).is_file() || dir.join("keyring").is_file()
}

/// Arguments every `rbd` call carries to reach the right cluster as the
/// right client.
pub fn base_args() -> Vec<String> {
    let mut a = vec!["--conf".to_string(), conf_path()];
    if let Some(id) = client_id() {
        a.push("--id".into());
        a.push(id);
    }
    a
}

async fn rbd(args: &[&str]) -> Result<std::process::Output> {
    check_client()?;
    Command::new("rbd")
        .args(base_args())
        .args(args)
        .output()
        .await
        .map_err(|e| Error::System(format!("Failed to run rbd: {e}")))
}

fn fail(what: &str, out: &std::process::Output) -> Error {
    Error::System(format!(
        "{what}: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    ))
}

/// Whether the image exists.
pub async fn exists(path: &str) -> Result<bool> {
    let spec =
        cli_spec(path).ok_or_else(|| Error::Validation(format!("{path} is not an RBD path")))?;
    let out = rbd(&["info", &spec]).await?;
    Ok(out.status.success())
}

/// Create an image of `size_gb` GB (409 when it already exists).
pub async fn create(path: &str, size_gb: u64) -> Result<()> {
    create_bytes(path, size_gb.saturating_mul(1 << 30)).await
}

/// Create an image of exactly `bytes` bytes.
pub async fn create_bytes(path: &str, bytes: u64) -> Result<()> {
    let spec =
        cli_spec(path).ok_or_else(|| Error::Validation(format!("{path} is not an RBD path")))?;
    if exists(path).await? {
        return Err(Error::AlreadyExists(format!(
            "{spec} already exists in the Ceph pool"
        )));
    }
    let mib = bytes.div_ceil(1 << 20).max(1);
    let out = rbd(&["create", "--size", &format!("{mib}M"), &spec]).await?;
    if !out.status.success() {
        return Err(fail("Failed to create the RBD image", &out));
    }
    Ok(())
}

/// Remove an image together with its snapshots. A snapshot that still has
/// linked clones (children) keeps the image: that is a 409 naming them.
pub async fn remove(path: &str) -> Result<()> {
    let spec =
        cli_spec(path).ok_or_else(|| Error::Validation(format!("{path} is not an RBD path")))?;
    if !exists(path).await? {
        return Ok(());
    }
    for snap in snapshots(path).await? {
        let s = format!("{spec}@{snap}");
        let _ = rbd(&["snap", "unprotect", &s]).await?;
        let out = rbd(&["snap", "rm", &s]).await?;
        if !out.status.success() {
            let kids = children(path).await.unwrap_or_default();
            return Err(Error::Conflict(format!(
                "{spec} is the base of linked clone(s) {}: delete those VMs or flatten them first",
                if kids.is_empty() {
                    "(see `rbd children`)".to_string()
                } else {
                    kids.join(", ")
                }
            )));
        }
    }
    let out = rbd(&["rm", &spec]).await?;
    if !out.status.success() {
        return Err(fail("Failed to remove the RBD image", &out));
    }
    Ok(())
}

/// Names of the image's snapshots.
pub async fn snapshots(path: &str) -> Result<Vec<String>> {
    let spec =
        cli_spec(path).ok_or_else(|| Error::Validation(format!("{path} is not an RBD path")))?;
    let out = rbd(&["snap", "ls", "--format", "json", &spec]).await?;
    if !out.status.success() {
        return Err(fail("Failed to list RBD snapshots", &out));
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| Error::System(format!("rbd snap ls answered no JSON: {e}")))?;
    Ok(v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.get("name").and_then(|n| n.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default())
}

/// `pool/image` specs of the clones that depend on any snapshot of `path`.
pub async fn children(path: &str) -> Result<Vec<String>> {
    let spec =
        cli_spec(path).ok_or_else(|| Error::Validation(format!("{path} is not an RBD path")))?;
    let out = rbd(&["children", "--all", "--format", "json", &spec]).await?;
    if !out.status.success() {
        return Ok(Vec::new());
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
    Ok(v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    let p = x.get("pool")?.as_str()?;
                    let i = x.get("image")?.as_str()?;
                    Some(format!("{p}/{i}"))
                })
                .collect()
        })
        .unwrap_or_default())
}

/// Create a snapshot `snap` of the image (atomic: a crash-consistent
/// point in time even while a guest runs on it).
pub async fn snap_create(path: &str, snap: &str) -> Result<()> {
    let spec = snap_spec(path, snap)?;
    let out = rbd(&["snap", "create", &spec]).await?;
    if !out.status.success() {
        return Err(fail("Failed to create the RBD snapshot", &out));
    }
    Ok(())
}

/// Remove a snapshot (unprotecting it first).
pub async fn snap_remove(path: &str, snap: &str) -> Result<()> {
    let spec = snap_spec(path, snap)?;
    let _ = rbd(&["snap", "unprotect", &spec]).await?;
    let out = rbd(&["snap", "rm", &spec]).await?;
    if !out.status.success() {
        return Err(fail("Failed to remove the RBD snapshot", &out));
    }
    Ok(())
}

/// Roll the image back to a snapshot (the VM must be stopped).
pub async fn snap_rollback(path: &str, snap: &str) -> Result<()> {
    let spec = snap_spec(path, snap)?;
    let out = rbd(&["snap", "rollback", &spec]).await?;
    if !out.status.success() {
        return Err(fail("Failed to roll back the RBD image", &out));
    }
    Ok(())
}

fn snap_spec(path: &str, snap: &str) -> Result<String> {
    let spec =
        cli_spec(path).ok_or_else(|| Error::Validation(format!("{path} is not an RBD path")))?;
    if snap.is_empty()
        || !snap
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(Error::Validation(format!("bad snapshot name {snap:?}")));
    }
    Ok(format!("{spec}@{snap}"))
}

/// Copy-on-write clone `dst` of snapshot `snap` of `src` (the snapshot is
/// protected and stays until the clone is flattened or deleted).
pub async fn clone_snapshot(src: &str, snap: &str, dst: &str) -> Result<()> {
    let s = snap_spec(src, snap)?;
    let d = cli_spec(dst).ok_or_else(|| Error::Validation(format!("{dst} is not an RBD path")))?;
    if exists(dst).await? {
        return Err(Error::AlreadyExists(format!("{d} already exists")));
    }
    let out = rbd(&["snap", "protect", &s]).await?;
    if !out.status.success() && !String::from_utf8_lossy(&out.stderr).contains("already protected")
    {
        return Err(fail("Failed to protect the RBD snapshot", &out));
    }
    let out = rbd(&["clone", &s, &d]).await?;
    if !out.status.success() {
        return Err(fail("Failed to clone the RBD snapshot", &out));
    }
    Ok(())
}

/// Make a clone independent of its parent snapshot.
pub async fn flatten(path: &str) -> Result<()> {
    let spec =
        cli_spec(path).ok_or_else(|| Error::Validation(format!("{path} is not an RBD path")))?;
    let out = rbd(&["flatten", &spec]).await?;
    if !out.status.success() {
        return Err(fail("Failed to flatten the RBD image", &out));
    }
    Ok(())
}

/// Full, independent copy of the image as it is right now (a temporary
/// snapshot makes it a consistent point in time even under a running
/// guest); `dst` is created, 409 if it exists.
pub async fn copy_consistent(src: &str, dst: &str) -> Result<()> {
    let tmp = format!("hx-copy-{}", uuid::Uuid::new_v4().simple());
    snap_create(src, &tmp).await?;
    let res = copy_snapshot(src, &tmp, dst).await;
    let _ = snap_remove(src, &tmp).await;
    res
}

/// `rbd cp <src>@<snap> <dst>`.
pub async fn copy_snapshot(src: &str, snap: &str, dst: &str) -> Result<()> {
    let s = snap_spec(src, snap)?;
    let d = cli_spec(dst).ok_or_else(|| Error::Validation(format!("{dst} is not an RBD path")))?;
    if exists(dst).await? {
        return Err(Error::AlreadyExists(format!("{d} already exists")));
    }
    let out = rbd(&["cp", &s, &d]).await?;
    if !out.status.success() {
        return Err(fail("Failed to copy the RBD image", &out));
    }
    Ok(())
}

/// Grow (or, with `allow_shrink`, resize) an image to `size_gb` GB.
pub async fn resize(path: &str, size_gb: u64, allow_shrink: bool) -> Result<()> {
    let spec =
        cli_spec(path).ok_or_else(|| Error::Validation(format!("{path} is not an RBD path")))?;
    let size = format!("{size_gb}G");
    let mut args = vec!["resize", "--size", size.as_str()];
    if allow_shrink {
        args.push("--allow-shrink");
    }
    args.push(&spec);
    let out = rbd(&args).await?;
    if !out.status.success() {
        return Err(fail("Failed to resize the RBD image", &out));
    }
    Ok(())
}

/// Virtual size in bytes of an image.
pub async fn size_bytes(path: &str) -> Result<u64> {
    let spec =
        cli_spec(path).ok_or_else(|| Error::Validation(format!("{path} is not an RBD path")))?;
    let out = rbd(&["info", "--format", "json", &spec]).await?;
    if !out.status.success() {
        return Err(fail("Failed to read the RBD image", &out));
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| Error::System(format!("rbd info answered no JSON: {e}")))?;
    v.get("size")
        .and_then(|s| s.as_u64())
        .ok_or_else(|| Error::System("rbd info has no size".into()))
}

/// Full copy of `src` into the new image `dst` (`rbd cp`, flattened).
pub async fn copy(src: &str, dst: &str) -> Result<()> {
    let s = cli_spec(src).ok_or_else(|| Error::Validation(format!("{src} is not an RBD path")))?;
    let d = cli_spec(dst).ok_or_else(|| Error::Validation(format!("{dst} is not an RBD path")))?;
    if exists(dst).await? {
        return Err(Error::AlreadyExists(format!("{d} already exists")));
    }
    let out = rbd(&["cp", &s, &d]).await?;
    if !out.status.success() {
        return Err(fail("Failed to copy the RBD image", &out));
    }
    Ok(())
}

/// Rename an image within its pool.
pub async fn rename(src: &str, dst: &str) -> Result<()> {
    let s = cli_spec(src).ok_or_else(|| Error::Validation(format!("{src} is not an RBD path")))?;
    let d = cli_spec(dst).ok_or_else(|| Error::Validation(format!("{dst} is not an RBD path")))?;
    let out = rbd(&["rename", &s, &d]).await?;
    if !out.status.success() {
        return Err(fail("Failed to rename the RBD image", &out));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyring_not_needed_when_cephx_is_off() {
        let d = std::env::temp_dir().join(format!("hx-rbd-auth-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let conf = d.join("ceph.conf");
        std::fs::write(&conf, "[global]\nmon_host = 1.2.3.4\n").unwrap();
        assert!(!keyring_present(conf.to_str().unwrap()));
        std::fs::write(&conf, "[global]\nauth_client_required = none\n").unwrap();
        assert!(keyring_present(conf.to_str().unwrap()));
        std::fs::write(&conf, "[global]\nauth supported = none\n").unwrap();
        assert!(keyring_present(conf.to_str().unwrap()));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn parses_and_formats() {
        assert_eq!(
            parse("rbd:vms/vm-1-disk-0"),
            Some(("vms".into(), "vm-1-disk-0".into()))
        );
        assert!(parse("vms/vm-1").is_none());
        assert!(parse("rbd:vms").is_none());
        assert!(parse("rbd:vms/a@snap").is_none());
        assert!(parse("rbd:vms/a:conf=/etc/passwd").is_none());
        assert_eq!(path_of("p", "i"), "rbd:p/i");
        assert_eq!(cli_spec("rbd:p/i").as_deref(), Some("p/i"));
        assert_eq!(image_name("7", 0), "vm-7-disk-0");
        assert_eq!(image_name("7", 2), "vm-7-disk-2");
    }

    #[test]
    fn qemu_file_carries_conf() {
        let f = qemu_file("rbd:p/i").unwrap();
        assert!(f.starts_with("rbd:p/i"));
        assert!(f.contains(":conf="));
    }

    #[test]
    fn anchor_is_local() {
        assert_eq!(
            anchor_path(Path::new("/var/lib/horcrux/vms"), "9"),
            PathBuf::from("/var/lib/horcrux/vms/9.qcow2")
        );
    }
}
