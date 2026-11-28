//! Where a VM's own files live: the UEFI variable store, the swtpm state,
//! the cloud-init seed, console sockets and state, hold/hand-off records.
//!
//! They are named after the VM's first disk. When that disk is a file
//! (`<vm storage>/<id>.qcow2`) they sit next to it, as they always have.
//! A disk that is a block device - a ZFS zvol (`/dev/zvol/<dataset>`), an LVM
//! volume, any `/dev/...` path - has no writable directory beside it, so its
//! VM's files go under the node's sidecar directory instead, at the device
//! path mirrored below it:
//! `/dev/zvol/tank/hx/router-disk0` -> `<sidecar dir>/zvol/tank/hx/router-disk0.disk`
//! (then `.efivars.qcow2`, `.tpm/`, `.cidata.iso`, `.qmp.sock`, ...).
//!
//! The sidecar directory is the typed config key `paths.vm_sidecars`
//! (env `HORCRUX_VM_SIDECARS`), default `<paths.vm_storage>/sidecars`.
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static ROOT: OnceLock<PathBuf> = OnceLock::new();

/// Set the node's sidecar directory (once, at startup).
pub fn set_root(dir: PathBuf) {
    let _ = ROOT.set(dir);
}

/// The node's sidecar directory.
pub fn root() -> PathBuf {
    if let Some(r) = ROOT.get() {
        return r.clone();
    }
    if let Some(r) = std::env::var_os("HORCRUX_VM_SIDECARS").filter(|v| !v.is_empty()) {
        return PathBuf::from(r);
    }
    crate::config::PathsConfig::default().effective_vm_sidecars()
}

/// Whether `disk0` is a device path (its VM's files go to the sidecar dir).
pub fn is_device(disk0: &Path) -> bool {
    disk0.starts_with("/dev")
}

/// The path the VM's files are named after: `disk0` itself for a file
/// disk, `<root>/<device path below /dev>.disk` for a device disk (its
/// parent directory is created on the way).
pub fn anchor(disk0: &Path) -> PathBuf {
    anchor_in(&root(), disk0)
}

pub fn anchor_in(root: &Path, disk0: &Path) -> PathBuf {
    let Ok(rel) = disk0.strip_prefix("/dev") else {
        return disk0.to_path_buf();
    };
    // `..` never leaves the sidecar dir (device paths never carry it)
    let rel: PathBuf = rel
        .components()
        .filter(|c| matches!(c, std::path::Component::Normal(_)))
        .collect();
    let mut s = root.join(rel).into_os_string();
    s.push(".disk");
    let p = PathBuf::from(s);
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    p
}

/// ZFS user property naming the Horcrux VM/container a dataset was made
/// for (adopted clones): only such datasets are destroyed with their guest.
pub const OWNER_PROP: &str = "horcrux:guest";

/// The dataset of a zvol device path (`/dev/zvol/<dataset>`).
pub fn zvol_dataset(disk: &Path) -> Option<String> {
    disk.strip_prefix("/dev/zvol")
        .ok()
        .map(|r| r.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
}

/// Destroy the zvol behind `disk` when it was made for guest `id`
/// (`horcrux:guest=<id>`); anything else is left alone.
pub async fn destroy_owned_zvol(disk: &Path, id: &str) -> horcrux_common::Result<()> {
    let Some(ds) = zvol_dataset(disk) else {
        return Ok(());
    };
    let Ok(o) = tokio::process::Command::new("zfs")
        .args(["get", "-H", "-o", "value", OWNER_PROP, &ds])
        .output()
        .await
    else {
        return Ok(());
    };
    if !o.status.success() || String::from_utf8_lossy(&o.stdout).trim() != id {
        tracing::info!(
            "VM {id}: disk {} is not a Horcrux-made zvol of this VM: left as it is",
            disk.display()
        );
        return Ok(());
    }
    let d = tokio::process::Command::new("zfs")
        .args(["destroy", &ds])
        .output()
        .await
        .map_err(|e| horcrux_common::Error::System(format!("zfs destroy {ds}: {e}")))?;
    if !d.status.success() {
        return Err(horcrux_common::Error::System(format!(
            "zfs destroy {ds}: {}",
            String::from_utf8_lossy(&d.stderr).trim()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_disks_keep_their_files_beside_them() {
        let d = Path::new("/var/lib/horcrux/vms/vm-1.qcow2");
        assert_eq!(anchor_in(Path::new("/sc"), d), d);
        assert_eq!(
            anchor_in(Path::new("/sc"), d).with_extension("efivars.qcow2"),
            PathBuf::from("/var/lib/horcrux/vms/vm-1.efivars.qcow2")
        );
    }

    #[test]
    fn device_disks_get_a_mirrored_place_in_the_sidecar_dir() {
        let t = tempfile::tempdir().unwrap();
        let a = anchor_in(t.path(), Path::new("/dev/zvol/tank/hx/edge.router-disk0"));
        assert_eq!(a, t.path().join("zvol/tank/hx/edge.router-disk0.disk"));
        // dots in dataset names survive with_extension
        assert_eq!(
            a.with_extension("efivars.qcow2"),
            t.path()
                .join("zvol/tank/hx/edge.router-disk0.efivars.qcow2")
        );
        assert_eq!(
            a.with_extension("tpm"),
            t.path().join("zvol/tank/hx/edge.router-disk0.tpm")
        );
        assert!(t.path().join("zvol/tank/hx").is_dir());
        // two zvols never share files
        assert_ne!(
            anchor_in(t.path(), Path::new("/dev/zvol/tank/a")),
            anchor_in(t.path(), Path::new("/dev/zvol/tank/b"))
        );
        // `..` cannot climb out
        let up = anchor_in(t.path(), Path::new("/dev/zvol/../../etc/x"));
        assert!(up.starts_with(t.path()), "{}", up.display());
        assert!(is_device(Path::new("/dev/mapper/vg-lv")));
        assert!(!is_device(Path::new("/devices/x.qcow2")));
    }
}
