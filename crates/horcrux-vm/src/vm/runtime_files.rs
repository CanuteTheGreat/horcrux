//! Where a VM's runtime sockets live.
//!
//! A running QEMU keeps its sockets (QMP, serial, guest agent, guest tools,
//! swtpm, extra serial ports, virtiofsd) next to its first disk:
//! `<disk0>.<ext>`. Some file systems can't hold Unix sockets: FUSE mounts
//! (SSHFS, the reference storage plugin) and SMB/CIFS answer `bind()` with
//! EPERM/EOPNOTSUPP, so a VM whose first disk is on such a pool would never
//! start. For those disks the sockets go to a node-local directory instead,
//! `<qemu run_dir>/vm-sockets/<disk file stem>-<hash of the disk's directory>.<ext>`
//! (the hash keeps two pools' `vm-1.qcow2` apart and is stable across
//! releases, so an upgraded manager finds a running VM's sockets).
use std::path::{Path, PathBuf};

/// statfs(2) magic numbers of file systems without Unix socket support.
const NO_SOCKET_FS: [u32; 3] = [
    0x6573_5546, // FUSE_SUPER_MAGIC (sshfs and other FUSE mounts)
    0xFF53_4D42, // CIFS_SUPER_MAGIC
    0xFE53_4D42, // SMB2_SUPER_MAGIC
];

/// Whether Unix sockets can be created in `dir` (true when unknown).
pub fn holds_sockets(dir: &Path) -> bool {
    match nix::sys::statfs::statfs(dir) {
        // the field is a signed or unsigned word depending on the target;
        // the magic is always the low 32 bits
        #[allow(clippy::unnecessary_cast)]
        Ok(st) => !NO_SOCKET_FS.contains(&((st.filesystem_type().0 as u64 & 0xffff_ffff) as u32)),
        Err(_) => true,
    }
}

/// FNV-1a 64 (stable: the name must not change between releases).
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Node-local directory for sockets of disks on socket-less file systems.
pub fn local_socket_dir() -> PathBuf {
    horcrux_core::config::HorcruxConfig::load()
        .qemu
        .run_dir
        .join("vm-sockets")
}

/// Path of socket `ext` (e.g. `qmp.sock`) of the VM whose first disk is `disk`.
pub fn socket_path(disk: &Path, ext: &str) -> PathBuf {
    socket_path_in(disk, ext, &local_socket_dir())
}

fn socket_path_in(disk: &Path, ext: &str, local: &Path) -> PathBuf {
    let next_to_disk = disk.with_extension(ext);
    match disk.parent() {
        Some(dir) if !dir.as_os_str().is_empty() && !holds_sockets(dir) => {
            let stem = disk
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "vm".into());
            let h = fnv1a(dir.as_os_str().as_encoded_bytes());
            let _ = std::fs::create_dir_all(local);
            local.join(format!("{stem}-{h:016x}.{ext}"))
        }
        _ => next_to_disk,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_disks_keep_their_sockets_next_to_them() {
        let d = tempfile::tempdir().unwrap();
        let disk = d.path().join("vm-1.qcow2");
        assert!(holds_sockets(d.path()));
        assert_eq!(
            socket_path_in(&disk, "qmp.sock", Path::new("/run/x")),
            d.path().join("vm-1.qmp.sock")
        );
    }

    #[test]
    fn hash_is_stable() {
        // pinned: a running VM's sockets must be found after an upgrade
        assert_ne!(fnv1a(b"/mnt/a"), fnv1a(b"/mnt/b"));
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
