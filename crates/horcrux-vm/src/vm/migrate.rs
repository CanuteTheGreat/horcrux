//! The VM-manager side of moving a QEMU VM between nodes (the migration
//! engine itself lives in horcrux-api: `api/vm_migration.rs` on the source,
//! `api/migration_peer.rs` on the target).
use super::{qemu, QemuManager, VmManager};
use horcrux_common::{Error, Result, VmConfig, VmHypervisor};
use std::path::{Path, PathBuf};

impl VmManager {
    /// Directory new VM disks are created in on this node.
    pub fn storage_dir(&self) -> PathBuf {
        self.qemu.storage_path().to_path_buf()
    }

    /// Whether a VM with this id exists on this node (memory or database).
    pub async fn vm_exists(&self, id: &str) -> bool {
        if self.vms.read().await.contains_key(id) {
            return true;
        }
        match &self.db {
            Some(db) => db.get_vm(id).await.is_ok(),
            None => false,
        }
    }

    /// Receiving side of a live migration: start QEMU for `config` (whose
    /// disks are already in place on this node) waiting for the incoming
    /// state (`-incoming defer`). Not recorded as a VM until
    /// [`Self::import_vm`] commits it.
    pub async fn start_incoming(&self, config: &VmConfig) -> Result<()> {
        if config.hypervisor != VmHypervisor::Qemu {
            return Err(Error::Validation("only QEMU VMs can be migrated".into()));
        }
        if self.vm_exists(&config.id).await {
            return Err(Error::Conflict(format!(
                "VM {} already exists on this node",
                config.id
            )));
        }
        let vm = self.qemu.vm_from_config(config);
        if QemuManager::process_running(&vm.id) {
            return Err(Error::Conflict(format!(
                "a QEMU process for VM {} is already running on this node",
                config.id
            )));
        }
        self.qemu.start_vm_incoming(&vm).await
    }

    /// Stop the QEMU of an aborted incoming migration and clean up its
    /// sockets, taps and swtpm (its disks are the caller's to remove).
    pub async fn stop_incoming(&self, config: &VmConfig) {
        let vm = self.qemu.vm_from_config(config);
        kill_qemu(&vm.id).await;
        qemu::cleanup_after_exit(&vm).await;
        super::firmware::stop_swtpm(&vm.disk_path).await;
    }

    /// Source side after a successful migration: the VM now lives on the
    /// other node. Drop it from this node's records and clean up what is
    /// this node's: its QEMU process, pidfile and taps; `delete` lists the
    /// files to remove (disks that were copied, not shared ones). With
    /// `shared_disk0` the files next to the first disk (console sockets and
    /// state, serial log, EFI vars, TPM state, cloud-init seed) are the
    /// target's now and stay; only the source's renamed runtime files
    /// (`*.mig-src`) go.
    pub async fn forget_migrated(
        &self,
        id: &str,
        delete: &[PathBuf],
        shared_disk0: bool,
    ) -> Result<()> {
        let mut vms = self.vms.write().await;
        let _ = self.hydrate(&mut vms, id).await;
        let vm = vms
            .remove(id)
            .ok_or_else(|| Error::VmNotFound(id.to_string()))?;
        if QemuManager::process_running(id) {
            kill_qemu(id).await;
        }
        let _ = tokio::fs::remove_file(qemu::pidfile(id)).await;
        qemu::remove_taps(id, vm.hardware.nets.len()).await;
        if shared_disk0 {
            for f in renamed_runtime_files(&vm.disk_path) {
                let _ = tokio::fs::remove_file(f).await;
            }
        } else {
            super::display::DisplayRuntime::remove(&vm.disk_path).await;
            for ext in ["serial.log", "display.json", "hold.json", "cidata.iso"] {
                let _ = tokio::fs::remove_file(vm.disk_path.with_extension(ext)).await;
            }
            super::firmware::remove_state(&vm.disk_path).await;
        }
        for f in delete {
            match tokio::fs::remove_file(f).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => tracing::warn!("VM {id} migrated, but {} stayed: {e}", f.display()),
            }
        }
        if let Some(db) = &self.db {
            db.delete_vm(id).await?;
        }
        Ok(())
    }
}

/// Runtime files a running QEMU keeps next to its first disk: console
/// sockets and state, and the swtpm socket/pidfile.
pub const RUNTIME_EXTS: [&str; 5] = [
    "qmp.sock",
    "serial.sock",
    "display.json",
    "tpm.sock",
    "tpm.pid",
];

/// Suffix the source gives its runtime files during a live migration on
/// shared storage, where the target's QEMU creates the same paths.
pub const MIG_SRC_SUFFIX: &str = "mig-src";

/// `<disk0>.<ext>.mig-src` of every runtime file.
pub fn renamed_runtime_files(disk0: &Path) -> Vec<PathBuf> {
    RUNTIME_EXTS
        .iter()
        .map(|e| disk0.with_extension(format!("{e}.{MIG_SRC_SUFFIX}")))
        .collect()
}

/// Live migration on shared storage: move the source QEMU's runtime files
/// aside (a listening socket keeps working when renamed) so the target's
/// QEMU can create its own at the standard paths; `restore` puts them back
/// after a rollback.
pub async fn set_runtime_files_aside(disk0: &Path, restore: bool) {
    for e in RUNTIME_EXTS {
        let std_path = disk0.with_extension(e);
        let aside = disk0.with_extension(format!("{e}.{MIG_SRC_SUFFIX}"));
        let (from, to) = if restore {
            (aside, std_path)
        } else {
            (std_path, aside)
        };
        if from.exists() {
            if restore {
                let _ = tokio::fs::remove_file(&to).await;
            }
            if let Err(err) = tokio::fs::rename(&from, &to).await {
                tracing::warn!("{} -> {}: {err}", from.display(), to.display());
            }
        }
    }
}

/// SIGTERM (then SIGKILL) the QEMU of `vm_id` and wait for it to exit.
pub async fn kill_qemu(vm_id: &str) {
    let Some(pid) = tokio::fs::read_to_string(qemu::pidfile(vm_id))
        .await
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok())
    else {
        return;
    };
    if !QemuManager::process_running(vm_id) {
        return;
    }
    use nix::sys::signal::{kill, Signal};
    let p = nix::unistd::Pid::from_raw(pid);
    let _ = kill(p, Signal::SIGTERM);
    for i in 0..100 {
        if !qemu::pid_alive(pid as u32) {
            return;
        }
        if i == 50 {
            let _ = kill(p, Signal::SIGKILL);
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// The per-VM files next to a VM's first disk that belong to it besides
/// its disks: EFI variable store and the cloud-init seed.
pub fn side_files(disk0: &Path) -> Vec<(&'static str, PathBuf)> {
    vec![
        ("efivars", super::firmware::efivars_path(disk0)),
        ("cidata", qemu::cloudinit_seed(disk0)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A listening socket moved aside keeps answering at its new path, the
    /// standard path is free for the target's QEMU, and restore puts it back.
    #[tokio::test]
    async fn runtime_files_move_aside_and_back() {
        let dir = tempfile::tempdir().unwrap();
        let disk0 = dir.path().join("vm-9.qcow2");
        let qmp = disk0.with_extension("qmp.sock");
        let listener = std::os::unix::net::UnixListener::bind(&qmp).unwrap();
        std::fs::write(disk0.with_extension("display.json"), b"{}").unwrap();
        set_runtime_files_aside(&disk0, false).await;
        assert!(!qmp.exists() && !disk0.with_extension("display.json").exists());
        let aside = disk0.with_extension("qmp.sock.mig-src");
        assert!(
            std::os::unix::net::UnixStream::connect(&aside).is_ok(),
            "renamed socket answers"
        );
        // The target creates its own file at the standard path...
        std::fs::write(&qmp, b"target").unwrap();
        // ...which the restore replaces with ours.
        set_runtime_files_aside(&disk0, true).await;
        assert!(std::os::unix::net::UnixStream::connect(&qmp).is_ok());
        assert!(disk0.with_extension("display.json").exists());
        assert!(renamed_runtime_files(&disk0).iter().all(|f| !f.exists()));
        drop(listener);
    }
}
