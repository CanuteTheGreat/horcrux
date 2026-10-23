//! Save a running QEMU VM's RAM and device state to a file ("suspend to
//! disk", Hyper-V's "Save state") and bring it back later with exactly the
//! same CPU, RAM and devices.
//!
//! Saving: the vCPUs are frozen (QMP `stop`), the whole machine state is
//! streamed to a file with `migrate` (`exec:cat > file`, works with KVM and
//! TCG and on every QEMU release), then QEMU exits. The file is written under
//! a temporary name and renamed once complete, so a half-written state is
//! never mistaken for a good one; if anything fails the guest is resumed
//! where it was and keeps running.
//!
//! Restoring: a fresh QEMU is started exactly like a normal start but with
//! `-S -incoming defer` (see `qemu::with_incoming`), the file is loaded with
//! `migrate-incoming` and the guest is continued. Whether the saved state
//! still matches the VM (same hardware, disks untouched since) is checked
//! first: loading RAM that no longer matches the disks would corrupt them.
//!
//! This module only does the QEMU part. Where the file lives, when to
//! delete it and the host-shutdown/boot orchestration are in horcrux-api's
//! `api/guest_state`.
use super::display::{qmp_command, DisplayRuntime};
use super::qemu::{self, QemuManager, QemuVm};
use super::{migrate, VmManager};
use horcrux_common::{Error, Result, VmConfig, VmHypervisor, VmStatus};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Identity of one file a saved state depends on: if it changed since the
/// state was saved, the state no longer matches the disks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileStamp {
    pub path: String,
    pub size: u64,
    /// Modification time, nanoseconds since the epoch.
    pub mtime_ns: u64,
}

impl FileStamp {
    pub fn of(path: &Path) -> Option<FileStamp> {
        let md = std::fs::metadata(path).ok()?;
        let mtime_ns = md
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos() as u64;
        Some(FileStamp {
            path: path.to_string_lossy().into_owned(),
            size: md.len(),
            mtime_ns,
        })
    }
}

/// What a successful save leaves behind (besides the state file).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmSaveOutcome {
    pub size_bytes: u64,
    /// The VM's configuration at save time (without status and name).
    pub config: serde_json::Value,
    /// Disks (and the UEFI variable store) as they were when QEMU exited.
    pub stamps: Vec<FileStamp>,
    /// `kvm` or `tcg`.
    pub accel: String,
}

/// Free space needed to save a guest with `memory_mb` of RAM: all of it
/// (zero pages are skipped by QEMU, but a busy guest has none) plus device
/// state.
pub fn space_needed(memory_mb: u64) -> u64 {
    memory_mb * 1024 * 1024 + 64 * 1024 * 1024
}

/// Free bytes on the file system holding `dir` (which must exist).
pub fn free_space(dir: &Path) -> Result<u64> {
    let st = nix::sys::statvfs::statvfs(dir)
        .map_err(|e| Error::System(format!("cannot check free space of {}: {e}", dir.display())))?;
    Ok(st.blocks_available() as u64 * st.fragment_size() as u64)
}

/// Refuse early, with the numbers, when `dir` cannot hold a saved state.
pub fn check_free_space(dir: &Path, memory_mb: u64) -> Result<()> {
    let need = space_needed(memory_mb);
    let have = free_space(dir)?;
    if have < need {
        return Err(Error::Validation(format!(
            "Not enough free space to save this guest's state on {}: about {} MB are needed \
             (the guest's {} MB of RAM plus device state) but only {} MB are free. \
             Free up space or choose another storage for saved states.",
            dir.display(),
            need / 1024 / 1024,
            memory_mb,
            have / 1024 / 1024
        )));
    }
    Ok(())
}

/// A path QEMU may put into an `exec:` migration URI (run by `sh -c`).
fn shell_safe(path: &Path) -> Result<String> {
    let s = path.to_string_lossy();
    if s.contains(['\'', '\n', '\0']) {
        return Err(Error::Validation(format!(
            "the saved-state path may not contain quotes or newlines: {s}"
        )));
    }
    Ok(s.into_owned())
}

/// The part of a VM's configuration a saved state depends on.
pub fn config_fingerprint(config: &VmConfig) -> serde_json::Value {
    let mut v = serde_json::to_value(config).unwrap_or_default();
    if let Some(o) = v.as_object_mut() {
        o.remove("status");
        o.remove("name");
    }
    v
}

/// Disks and UEFI variable store of `vm`, as stamps.
fn stamps_of(vm: &QemuVm) -> Vec<FileStamp> {
    let mut files: Vec<PathBuf> = vm
        .all_disks()
        .iter()
        .map(|d| PathBuf::from(&d.path))
        .collect();
    let vars = vm.disk_path.with_extension("efivars.fd");
    if vars.exists() {
        files.push(vars);
    }
    files.iter().filter_map(|f| FileStamp::of(f)).collect()
}

/// A failed save: continue the guest where it was, drop the partial file.
async fn undo(sock: &Path, tmp: &Path, was_running: bool, msg: String) -> Error {
    if was_running {
        let _ = qmp_command(sock, json!({"execute": "cont"})).await;
    }
    let _ = tokio::fs::remove_file(tmp).await;
    Error::System(msg)
}

/// A failed resume: no half-started QEMU may stay behind.
async fn abort_resume(vm: &QemuVm, msg: String) -> Error {
    migrate::kill_qemu(&vm.id).await;
    qemu::cleanup_after_exit(vm).await;
    super::firmware::stop_swtpm(&vm.disk_path).await;
    Error::System(msg)
}

async fn query_status(sock: &Path) -> Result<String> {
    let s = qmp_command(sock, json!({"execute": "query-status"})).await?;
    Ok(s["status"].as_str().unwrap_or("").to_string())
}

impl VmManager {
    /// Save the running QEMU VM `id` to `dest` and stop it (see the module
    /// docs). On any failure the guest is continued and keeps running.
    pub async fn save_state_to(
        &self,
        id: &str,
        dest: &Path,
        timeout: Duration,
    ) -> Result<VmSaveOutcome> {
        let vm = self.qemu_vm(id).await?;
        if vm.hypervisor != VmHypervisor::Qemu {
            return Err(Error::Validation(
                "only QEMU VMs can be saved to a file".into(),
            ));
        }
        if !QemuManager::process_running(&vm.id) {
            return Err(Error::Conflict(format!("VM {id} is not running")));
        }
        let dir = dest
            .parent()
            .ok_or_else(|| Error::Validation("invalid saved-state path".into()))?;
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| Error::System(format!("cannot create {}: {e}", dir.display())))?;
        check_free_space(dir, vm.memory)?;

        let sock = DisplayRuntime::qmp_socket(&vm.disk_path);
        let tmp = dest.with_extension("tmp");
        let uri = format!("exec:cat > '{}'", shell_safe(&tmp)?);
        let _ = tokio::fs::remove_file(&tmp).await;

        // Freeze the guest (stays frozen if it already was: a paused VM is
        // saved as paused-then-running like any other).
        let was_running = query_status(&sock).await? == "running";
        if was_running {
            qmp_command(&sock, json!({"execute": "stop"})).await?;
        }

        // A file is not a network: no bandwidth cap.
        let _ = qmp_command(
            &sock,
            json!({"execute": "migrate-set-parameters",
                   "arguments": {"max-bandwidth": 1u64 << 40}}),
        )
        .await;
        if let Err(e) = qmp_command(
            &sock,
            json!({"execute": "migrate", "arguments": {"uri": uri}}),
        )
        .await
        {
            return Err(undo(
                &sock,
                &tmp,
                was_running,
                format!("QEMU refused to save the state: {e}"),
            )
            .await);
        }
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let m = match qmp_command(&sock, json!({"execute": "query-migrate"})).await {
                Ok(m) => m,
                Err(e) => {
                    return Err(undo(
                        &sock,
                        &tmp,
                        was_running,
                        format!("lost QEMU while saving the state: {e}"),
                    )
                    .await)
                }
            };
            match m["status"].as_str() {
                Some("completed") => break,
                Some("failed") | Some("cancelled") => {
                    let why = m["error-desc"].as_str().unwrap_or("no reason given");
                    return Err(undo(
                        &sock,
                        &tmp,
                        was_running,
                        format!("saving the state failed: {why}"),
                    )
                    .await);
                }
                _ => {}
            }
            if tokio::time::Instant::now() >= deadline {
                let _ = qmp_command(&sock, json!({"execute": "migrate_cancel"})).await;
                return Err(undo(
                    &sock,
                    &tmp,
                    was_running,
                    format!(
                        "saving the state did not finish within {} s",
                        timeout.as_secs()
                    ),
                )
                .await);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        // The state is out. QEMU exits; the guest never ran after the
        // freeze, so the disks match the state exactly.
        let _ = qmp_command(&sock, json!({"execute": "quit"})).await;
        for _ in 0..100 {
            if !QemuManager::process_running(&vm.id) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if QemuManager::process_running(&vm.id) {
            migrate::kill_qemu(&vm.id).await;
        }
        qemu::cleanup_after_exit(&vm).await;
        super::firmware::stop_swtpm(&vm.disk_path).await;

        let size = tokio::fs::metadata(&tmp)
            .await
            .map(|m| m.len())
            .map_err(|e| Error::System(format!("saved state missing after save: {e}")))?;
        if size == 0 {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(Error::System("saved state is empty".into()));
        }
        // Durable before it counts as saved.
        if let Ok(f) = std::fs::File::open(&tmp) {
            let _ = f.sync_all();
        }
        tokio::fs::rename(&tmp, dest)
            .await
            .map_err(|e| Error::System(format!("cannot finalize {}: {e}", dest.display())))?;
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all();
        }
        let _ = self.record_status(id, VmStatus::Stopped).await;
        Ok(VmSaveOutcome {
            size_bytes: size,
            config: config_fingerprint(&vm.to_config()),
            stamps: stamps_of(&vm),
            accel: if tokio::fs::metadata("/dev/kvm").await.is_ok() {
                "kvm"
            } else {
                "tcg"
            }
            .into(),
        })
    }

    /// Why a saved state of `id` can no longer be resumed, if so (hardware
    /// changed, a disk was modified, file gone). `Ok` means it can be tried.
    pub async fn check_saved_state(
        &self,
        id: &str,
        file: &Path,
        saved: &VmSaveOutcome,
    ) -> Result<()> {
        let vm = self.qemu_vm(id).await?;
        if !file.is_file() {
            return Err(Error::Conflict(format!(
                "the saved state file {} is gone",
                file.display()
            )));
        }
        if config_fingerprint(&vm.to_config()) != saved.config {
            return Err(Error::Conflict(
                "the VM's hardware (CPU, memory, disks, network cards, firmware) changed since \
                 its state was saved, so the state cannot be loaded. Discard the saved state \
                 to start the VM normally."
                    .into(),
            ));
        }
        let now = stamps_of(&vm);
        for s in &saved.stamps {
            if !now.contains(s) {
                return Err(Error::Conflict(format!(
                    "{} was modified since the state was saved, so the state no longer matches \
                     the disks and resuming it would corrupt them. Discard the saved state to \
                     start the VM normally.",
                    s.path
                )));
            }
        }
        Ok(())
    }

    /// Start QEMU for `id` and load the state in `file` (checked first with
    /// [`Self::check_saved_state`]); the guest continues exactly where it
    /// was saved. The file is not touched: deleting it after success is the
    /// caller's job, and on failure it stays for another try.
    pub async fn resume_from_file(
        &self,
        id: &str,
        file: &Path,
        saved: &VmSaveOutcome,
        timeout: Duration,
    ) -> Result<VmConfig> {
        self.check_saved_state(id, file, saved).await?;
        let mut vm = self.qemu_vm(id).await?;
        if QemuManager::process_running(&vm.id) {
            return Err(Error::Conflict(format!("VM {id} is already running")));
        }
        vm.status = VmStatus::Stopped;
        self.qemu.start_vm_incoming(&vm).await?;
        let sock = DisplayRuntime::qmp_socket(&vm.disk_path);
        let uri = format!("exec:cat '{}'", shell_safe(file)?);
        if let Err(e) = qmp_command(
            &sock,
            json!({"execute": "migrate-incoming", "arguments": {"uri": uri}}),
        )
        .await
        {
            return Err(
                abort_resume(&vm, format!("QEMU refused to load the saved state: {e}")).await,
            );
        }
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if !QemuManager::process_running(&vm.id) {
                return Err(abort_resume(
                    &vm,
                    "QEMU exited while loading the saved state (it does not match this \
                     QEMU/host any more)"
                        .into(),
                )
                .await);
            }
            match query_status(&sock).await {
                Ok(s) if s == "running" => break,
                Ok(s) if matches!(s.as_str(), "paused" | "prelaunch" | "postmigrate") => {
                    // Loaded; `-S` keeps it frozen until now.
                    let m = qmp_command(&sock, json!({"execute": "query-migrate"}))
                        .await
                        .unwrap_or_default();
                    if m["status"].as_str() == Some("failed") {
                        let why = m["error-desc"].as_str().unwrap_or("no reason given");
                        return Err(abort_resume(
                            &vm,
                            format!("loading the saved state failed: {why}"),
                        )
                        .await);
                    }
                    let _ = qmp_command(&sock, json!({"execute": "cont"})).await;
                }
                _ => {}
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(abort_resume(
                    &vm,
                    format!(
                        "loading the saved state did not finish within {} s",
                        timeout.as_secs()
                    ),
                )
                .await);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        self.record_status(id, VmStatus::Running).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_needed_is_ram_plus_device_state() {
        assert_eq!(space_needed(256), (256 + 64) * 1024 * 1024);
    }

    #[test]
    fn free_space_check_reports_numbers() {
        let dir = tempfile::tempdir().unwrap();
        assert!(free_space(dir.path()).unwrap() > 0);
        // 100 TB of RAM never fits.
        let err = check_free_space(dir.path(), 100 * 1024 * 1024 * 1024)
            .unwrap_err()
            .to_string();
        assert!(err.contains("Not enough free space"), "{err}");
        assert!(err.contains("MB are free"), "{err}");
        check_free_space(dir.path(), 1).unwrap();
        assert!(free_space(&dir.path().join("absent")).is_err());
    }

    #[test]
    fn exec_uri_paths_are_checked() {
        assert!(shell_safe(Path::new("/var/lib/horcrux/a b/vmstate.bin")).is_ok());
        assert!(shell_safe(Path::new("/tmp/it's")).is_err());
        assert!(shell_safe(Path::new("/tmp/a\nb")).is_err());
    }

    #[test]
    fn file_stamp_tracks_size_and_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("d.qcow2");
        std::fs::write(&f, b"abc").unwrap();
        let a = FileStamp::of(&f).unwrap();
        assert_eq!(a.size, 3);
        assert_eq!(FileStamp::of(&f).unwrap(), a);
        std::fs::write(&f, b"abcd").unwrap();
        assert_ne!(FileStamp::of(&f).unwrap(), a);
        assert!(FileStamp::of(&dir.path().join("none")).is_none());
    }

    #[test]
    fn fingerprint_ignores_status_and_name_only() {
        let mut c = VmConfig {
            id: "v".into(),
            name: "a".into(),
            hypervisor: VmHypervisor::Qemu,
            memory: 256,
            cpus: 1,
            disk_size: 1,
            status: VmStatus::Running,
            architecture: Default::default(),
            disks: vec![],
            display: Default::default(),
            hardware: Default::default(),
        };
        let a = config_fingerprint(&c);
        c.name = "b".into();
        c.status = VmStatus::Stopped;
        assert_eq!(config_fingerprint(&c), a);
        c.memory = 512;
        assert_ne!(config_fingerprint(&c), a);
    }
}
