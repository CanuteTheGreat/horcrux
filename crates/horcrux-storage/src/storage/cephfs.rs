//! CephFS storage (Proxmox "CephFS" storage type): a POSIX file system on a
//! Ceph cluster, used like a directory pool (VM disks as qcow2 files, ISOs,
//! container templates, backups) that every node mounts, so it is shared
//! storage for migration and HA.
//!
//! The pool's `path` is the mount point on this node. Mounting is done by the
//! administrator (`mount -t ceph mon1,mon2:/ /mnt/cephfs -o name=<user>,secretfile=...`
//! or an fstab / systemd mount unit; `ceph-fuse` works too): adding the pool
//! checks that the path really is a CephFS mount, so a pool pointed at an
//! unmounted directory cannot silently fill the root file system.
use super::StoragePool;
use horcrux_common::{Error, Result};
use tokio::process::Command as AsyncCommand;

pub struct CephFsManager {}

impl Default for CephFsManager {
    fn default() -> Self {
        Self::new()
    }
}

/// File system type of the mount that holds exactly `path`, from the text of
/// `/proc/mounts` (`None` = `path` is not a mount point).
pub fn mount_fstype(mounts: &str, path: &str) -> Option<String> {
    let want = path.trim_end_matches('/');
    let want = if want.is_empty() { "/" } else { want };
    mounts
        .lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let (_src, mp, ty) = (f.next()?, f.next()?, f.next()?);
            // /proc/mounts escapes spaces as \040
            (mp.replace("\\040", " ") == want).then(|| ty.to_string())
        })
        .next_back()
}

impl CephFsManager {
    pub fn new() -> Self {
        Self {}
    }

    /// The pool path must be a mounted CephFS (kernel client `ceph` or `fuse.ceph-fuse`).
    pub fn check_mounted(mounts: &str, path: &str) -> Result<()> {
        match mount_fstype(mounts, path).as_deref() {
            Some("ceph") | Some("fuse.ceph-fuse") | Some("fuse.ceph") => Ok(()),
            Some(other) => Err(Error::InvalidConfig(format!(
                "{path} is mounted as {other}, not as CephFS"
            ))),
            None => Err(Error::InvalidConfig(format!(
                "{path} is not a mount point: mount the CephFS first, e.g. \
                 `mount -t ceph <mon1>,<mon2>:/ {path} -o name=<user>,secretfile=/etc/ceph/<user>.secret` \
                 (or `ceph-fuse {path}`), then add the pool"
            ))),
        }
    }

    pub async fn validate_pool(&self, pool: &StoragePool) -> Result<()> {
        if !pool.path.starts_with('/') {
            return Err(Error::InvalidConfig(
                "a CephFS pool's path is the absolute mount point on this node".into(),
            ));
        }
        if !std::path::Path::new(&pool.path).is_dir() {
            return Err(Error::InvalidConfig(format!(
                "{} is not a directory",
                pool.path
            )));
        }
        let mounts = tokio::fs::read_to_string("/proc/mounts")
            .await
            .map_err(|e| Error::System(format!("reading /proc/mounts: {e}")))?;
        Self::check_mounted(&mounts, &pool.path)
    }

    /// A qcow2 volume in the CephFS directory.
    pub async fn create_volume(
        &self,
        mount_point: &str,
        volume_name: &str,
        size_gb: u64,
    ) -> Result<String> {
        super::check_names(&[volume_name])?;
        let volume_path = format!("{}/{}.qcow2", mount_point, volume_name);
        let output = AsyncCommand::new("qemu-img")
            .args([
                "create",
                "-f",
                "qcow2",
                &volume_path,
                &format!("{size_gb}G"),
            ])
            .output()
            .await
            .map_err(|e| Error::System(format!("Failed to create volume: {e}")))?;
        if !output.status.success() {
            return Err(Error::System(format!(
                "Volume creation failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(volume_path)
    }

    pub async fn delete_volume(&self, volume_path: &str) -> Result<()> {
        tokio::fs::remove_file(volume_path)
            .await
            .map_err(|e| Error::System(format!("Failed to delete volume: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOUNTS: &str = "\
/dev/sda1 / ext4 rw,relatime 0 0
10.0.0.1:6789,10.0.0.2:6789:/ /mnt/cephfs ceph rw,relatime,name=hx,acl 0 0
ceph-fuse /mnt/fuse\\040fs fuse.ceph-fuse rw,nosuid 0 0
10.9.9.9:/export /mnt/nfs nfs4 rw 0 0
tmpfs /mnt/cephfs/sub tmpfs rw 0 0
";

    #[test]
    fn finds_the_mount_that_holds_exactly_the_path() {
        assert_eq!(mount_fstype(MOUNTS, "/mnt/cephfs").as_deref(), Some("ceph"));
        assert_eq!(
            mount_fstype(MOUNTS, "/mnt/cephfs/").as_deref(),
            Some("ceph")
        );
        assert_eq!(
            mount_fstype(MOUNTS, "/mnt/fuse fs").as_deref(),
            Some("fuse.ceph-fuse")
        );
        assert_eq!(
            mount_fstype(MOUNTS, "/mnt/cephfs/sub").as_deref(),
            Some("tmpfs")
        );
        assert_eq!(mount_fstype(MOUNTS, "/mnt/other"), None);
        assert_eq!(mount_fstype(MOUNTS, "/").as_deref(), Some("ext4"));
    }

    #[test]
    fn only_a_mounted_cephfs_is_accepted_with_a_message_that_says_how() {
        assert!(CephFsManager::check_mounted(MOUNTS, "/mnt/cephfs").is_ok());
        assert!(CephFsManager::check_mounted(MOUNTS, "/mnt/fuse fs").is_ok());
        let e = CephFsManager::check_mounted(MOUNTS, "/mnt/nfs")
            .unwrap_err()
            .to_string();
        assert!(e.contains("nfs4") && e.contains("not as CephFS"), "{e}");
        let e = CephFsManager::check_mounted(MOUNTS, "/mnt/cephfs/sub")
            .unwrap_err()
            .to_string();
        assert!(e.contains("tmpfs"), "{e}");
        let e = CephFsManager::check_mounted(MOUNTS, "/srv/nothing")
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("mount -t ceph") && e.contains("/srv/nothing"),
            "{e}"
        );
    }

    #[tokio::test]
    async fn pools_must_be_absolute_existing_directories() {
        let mk = |p: &str| StoragePool {
            id: "c".into(),
            name: "c".into(),
            storage_type: super::super::StorageType::CephFs,
            path: p.into(),
            available: 0,
            total: 0,
            enabled: true,
        };
        let m = CephFsManager::new();
        assert!(m.validate_pool(&mk("rel/path")).await.is_err());
        assert!(m
            .validate_pool(&mk("/no/such/dir/for/cephfs"))
            .await
            .is_err());
        // an existing directory that is not a CephFS mount is refused
        let e = m.validate_pool(&mk("/tmp")).await.unwrap_err().to_string();
        assert!(
            e.contains("not as CephFS") || e.contains("not a mount point"),
            "{e}"
        );
    }

    #[tokio::test]
    async fn volumes_are_qcow2_files_with_checked_names() {
        let d = std::env::temp_dir().join(format!("cephfs-vol-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let m = CephFsManager::new();
        let p = m
            .create_volume(d.to_str().unwrap(), "vm1-disk0", 1)
            .await
            .unwrap();
        assert!(p.ends_with("/vm1-disk0.qcow2") && std::path::Path::new(&p).is_file());
        assert!(m
            .create_volume(d.to_str().unwrap(), "../evil", 1)
            .await
            .is_err());
        m.delete_volume(&p).await.unwrap();
        assert!(!std::path::Path::new(&p).exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
