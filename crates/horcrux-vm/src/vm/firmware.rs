//! UEFI firmware (OVMF) and the emulated TPM 2.0 (swtpm) of QEMU VMs.
//!
//! Both keep per-VM state next to the VM's first disk, like the console
//! sockets and the serial log do:
//!
//! - `<disk0>.efivars.qcow2`: the VM's own copy of the OVMF variable store
//!   (boot entries, Secure Boot keys), created from the host's vars
//!   template on first start. qcow2, so internal snapshots (`savevm`) of a
//!   UEFI VM still work and live migration can mirror it like a disk.
//! - `<disk0>.tpm/`: swtpm's state directory, `<disk0>.tpm.sock` its control
//!   socket, `<disk0>.tpm.log` its log.
use horcrux_common::{Error, Result};
use std::path::{Path, PathBuf};
use tokio::process::Command;

/// QMP/QEMU drive id of a VM's EFI variable store (mirrored by live migration).
pub const EFIVARS_DRIVE_ID: &str = "drive-efivars";

/// The host's OVMF code + variable-template pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ovmf {
    pub code: PathBuf,
    pub vars_template: PathBuf,
}

/// Known OVMF locations, best first: (code, vars template). Ubuntu/Debian
/// `ovmf`, Gentoo `sys-firmware/edk2-bin` and the older `edk2-ovmf(-bin)`,
/// Fedora/Arch `edk2-ovmf`, and QEMU's own bundled EDK2 build.
const OVMF_CANDIDATES: &[(&str, &str)] = &[
    (
        "/usr/share/OVMF/OVMF_CODE_4M.fd",
        "/usr/share/OVMF/OVMF_VARS_4M.fd",
    ),
    (
        "/usr/share/OVMF/OVMF_CODE.fd",
        "/usr/share/OVMF/OVMF_VARS.fd",
    ),
    (
        "/usr/share/edk2/OvmfX64/OVMF_CODE.fd",
        "/usr/share/edk2/OvmfX64/OVMF_VARS.fd",
    ),
    (
        "/usr/share/edk2-ovmf/OVMF_CODE.fd",
        "/usr/share/edk2-ovmf/OVMF_VARS.fd",
    ),
    (
        "/usr/share/edk2-ovmf/x64/OVMF_CODE.fd",
        "/usr/share/edk2-ovmf/x64/OVMF_VARS.fd",
    ),
    (
        "/usr/share/edk2/x64/OVMF_CODE.4m.fd",
        "/usr/share/edk2/x64/OVMF_VARS.4m.fd",
    ),
    (
        "/usr/share/edk2/ovmf/OVMF_CODE.fd",
        "/usr/share/edk2/ovmf/OVMF_VARS.fd",
    ),
    (
        "/usr/share/qemu/edk2-x86_64-code.fd",
        "/usr/share/qemu/edk2-i386-vars.fd",
    ),
];

/// Secure Boot builds (vendor keys enrolled in the vars template).
const OVMF_SECBOOT_CANDIDATES: &[(&str, &str)] = &[
    (
        "/usr/share/OVMF/OVMF_CODE_4M.ms.fd",
        "/usr/share/OVMF/OVMF_VARS_4M.ms.fd",
    ),
    (
        "/usr/share/OVMF/OVMF_CODE.secboot.fd",
        "/usr/share/OVMF/OVMF_VARS.secboot.fd",
    ),
    (
        "/usr/share/edk2/OvmfX64/OVMF_CODE.secboot.fd",
        "/usr/share/edk2/OvmfX64/OVMF_VARS.secboot.fd",
    ),
    (
        "/usr/share/edk2/ovmf/OVMF_CODE.secboot.fd",
        "/usr/share/edk2/ovmf/OVMF_VARS.secboot.fd",
    ),
];

/// Find OVMF on this host. `HORCRUX_OVMF_CODE` + `HORCRUX_OVMF_VARS`
/// override the search (both must be set).
pub fn find_ovmf() -> Option<Ovmf> {
    find_ovmf_for(false)
}

/// The Secure Boot build of OVMF (`HORCRUX_OVMF_SECBOOT_CODE` +
/// `HORCRUX_OVMF_SECBOOT_VARS` override the search).
pub fn find_ovmf_for(secure_boot: bool) -> Option<Ovmf> {
    if secure_boot {
        if let (Ok(code), Ok(vars)) = (
            std::env::var("HORCRUX_OVMF_SECBOOT_CODE"),
            std::env::var("HORCRUX_OVMF_SECBOOT_VARS"),
        ) {
            let (code, vars) = (PathBuf::from(code), PathBuf::from(vars));
            return (code.is_file() && vars.is_file()).then_some(Ovmf {
                code,
                vars_template: vars,
            });
        }
        return OVMF_SECBOOT_CANDIDATES.iter().find_map(|(c, v)| {
            let (c, v) = (Path::new(c), Path::new(v));
            (c.is_file() && v.is_file()).then(|| Ovmf {
                code: c.to_path_buf(),
                vars_template: v.to_path_buf(),
            })
        });
    }
    if let (Ok(code), Ok(vars)) = (
        std::env::var("HORCRUX_OVMF_CODE"),
        std::env::var("HORCRUX_OVMF_VARS"),
    ) {
        let (code, vars) = (PathBuf::from(code), PathBuf::from(vars));
        return (code.is_file() && vars.is_file()).then_some(Ovmf {
            code,
            vars_template: vars,
        });
    }
    OVMF_CANDIDATES.iter().find_map(|(c, v)| {
        let (c, v) = (Path::new(c), Path::new(v));
        (c.is_file() && v.is_file()).then(|| Ovmf {
            code: c.to_path_buf(),
            vars_template: v.to_path_buf(),
        })
    })
}

/// Why a Secure Boot VM can't start here.
pub fn ovmf_secboot_missing() -> Error {
    Error::Validation(
        "the Secure Boot build of OVMF is not installed on this host: install \
         sys-firmware/edk2-bin (Debian/Ubuntu: ovmf), or set HORCRUX_OVMF_SECBOOT_CODE and \
         HORCRUX_OVMF_SECBOOT_VARS"
            .into(),
    )
}

/// Why a UEFI VM can't start here (no OVMF installed).
pub fn ovmf_missing() -> Error {
    Error::Validation(
        "UEFI (OVMF) firmware is not installed on this host: install sys-firmware/edk2-bin \
         (Debian/Ubuntu: ovmf), or set HORCRUX_OVMF_CODE and HORCRUX_OVMF_VARS"
            .into(),
    )
}

/// The VM's EFI variable store.
pub fn efivars_path(disk0: &Path) -> PathBuf {
    disk0.with_extension("efivars.qcow2")
}

/// swtpm's state directory of the VM.
pub fn tpm_state_dir(disk0: &Path) -> PathBuf {
    disk0.with_extension("tpm")
}

/// swtpm's control socket (QEMU's `-chardev socket`).
pub fn tpm_socket(disk0: &Path) -> PathBuf {
    disk0.with_extension("tpm.sock")
}

fn tpm_pidfile(disk0: &Path) -> PathBuf {
    disk0.with_extension("tpm.pid")
}

fn tpm_log(disk0: &Path) -> PathBuf {
    disk0.with_extension("tpm.log")
}

/// Every per-VM firmware/TPM file next to `disk0` (removed with the VM,
/// moved by migration).
pub fn state_files(disk0: &Path) -> Vec<PathBuf> {
    vec![
        efivars_path(disk0),
        tpm_socket(disk0),
        tpm_pidfile(disk0),
        tpm_log(disk0),
    ]
}

/// Create the VM's EFI variable store from the host template if it has
/// none yet (first start of a UEFI VM).
pub async fn ensure_efivars(disk0: &Path, ovmf: &Ovmf) -> Result<PathBuf> {
    let vars = efivars_path(disk0);
    if vars.is_file() {
        return Ok(vars);
    }
    let out = Command::new("qemu-img")
        .args(["convert", "-f", "raw", "-O", "qcow2"])
        .arg(&ovmf.vars_template)
        .arg(&vars)
        .output()
        .await
        .map_err(|e| Error::System(format!("qemu-img: {e}")))?;
    if !out.status.success() {
        let _ = tokio::fs::remove_file(&vars).await;
        return Err(Error::System(format!(
            "could not create the EFI variable store {}: {}",
            vars.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(vars)
}

/// Start the VM's swtpm (TPM 2.0) before QEMU. It serves the control
/// socket QEMU connects to and exits by itself when QEMU disconnects
/// (`--terminate`), so stop/crash need no extra cleanup.
pub async fn start_swtpm(disk0: &Path) -> Result<()> {
    let dir = tpm_state_dir(disk0);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| Error::System(format!("TPM state dir {}: {e}", dir.display())))?;
    let sock = tpm_socket(disk0);
    stop_swtpm(disk0).await;
    let _ = tokio::fs::remove_file(&sock).await;
    let out = Command::new("swtpm")
        .arg("socket")
        .arg("--tpm2")
        .arg("--tpmstate")
        .arg(format!("dir={},mode=0600", dir.display()))
        .arg("--ctrl")
        .arg(format!("type=unixio,path={},mode=0600", sock.display()))
        .arg("--log")
        .arg(format!("file={}", tpm_log(disk0).display()))
        .arg("--pid")
        .arg(format!("file={}", tpm_pidfile(disk0).display()))
        .arg("--terminate")
        .arg("--daemon")
        .output()
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::Validation(
                    "The VM has a TPM, but swtpm is not installed on this host \
                     (install app-crypt/swtpm; Debian/Ubuntu: swtpm)"
                        .into(),
                )
            } else {
                Error::System(format!("swtpm: {e}"))
            }
        })?;
    if !out.status.success() {
        return Err(Error::System(format!(
            "swtpm failed to start: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    for _ in 0..50 {
        if sock.exists() {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    Err(Error::System(format!(
        "swtpm did not create its socket {}",
        sock.display()
    )))
}

/// Stop a VM's swtpm if one is still running (QEMU failed to start, or
/// the VM is being deleted).
pub async fn stop_swtpm(disk0: &Path) {
    let pidfile = tpm_pidfile(disk0);
    if let Some(pid) = tokio::fs::read_to_string(&pidfile)
        .await
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok())
    {
        let is_swtpm = std::fs::read(format!("/proc/{pid}/cmdline"))
            .map(|c| String::from_utf8_lossy(&c).contains("swtpm"))
            .unwrap_or(false);
        if is_swtpm {
            use nix::sys::signal::{kill, Signal};
            let _ = kill(nix::unistd::Pid::from_raw(pid), Signal::SIGTERM);
            for _ in 0..20 {
                if !super::qemu::pid_alive(pid as u32) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }
    }
    let _ = tokio::fs::remove_file(&pidfile).await;
}

/// Remove all firmware/TPM state of a deleted VM.
pub async fn remove_state(disk0: &Path) {
    stop_swtpm(disk0).await;
    for f in state_files(disk0) {
        let _ = tokio::fs::remove_file(f).await;
    }
    let _ = tokio::fs::remove_dir_all(tpm_state_dir(disk0)).await;
}

// ------------------------------------------------------------ TPM state
//
// The TPM's state holds the guest's keys: BitLocker sealing, Windows Hello,
// the Microsoft Entra / Intune device identity, LUKS-TPM2 enrolments. An
// empty TPM in place of the old one breaks all of them (Windows: "Device TPM
// problem" 0xc0090016 NTE_BAD_KEYSET), so the state is never replaced
// silently: moves carry it, a missing state refuses the start, and only an
// explicit reset creates a new one.

/// swtpm's permanent-state file of a TPM 2.0 in a state directory.
pub const TPM_PERMALL: &str = "tpm2-00.permall";

/// Whether the VM's TPM state exists (a non-empty permanent-state file).
pub fn tpm_state_present(disk0: &Path) -> bool {
    std::fs::metadata(tpm_state_dir(disk0).join(TPM_PERMALL))
        .map(|m| m.is_file() && m.len() > 0)
        .unwrap_or(false)
}

/// The refusal for a start without the TPM state the VM had.
pub fn tpm_state_missing_error(vm_id: &str, disk0: &Path) -> Error {
    Error::Conflict(format!(
        "The TPM state of VM {vm_id} is missing ({}): this VM had a TPM, and starting it with an empty \
         one would break what the guest keeps in it (BitLocker keys, Windows Hello, Microsoft Entra/Intune \
         device identity, TPM-bound disk encryption; Windows shows 'Device TPM problem' 0xc0090016). \
         Restore the TPM state (from a backup, snapshot, or the old host's swtpm directory) or, if you \
         accept that loss and have your BitLocker recovery key, create a new TPM explicitly: \
         POST /api/vms/{vm_id}/tpm/reset {{\"confirm_data_loss\": true}}",
        tpm_state_dir(disk0).join(TPM_PERMALL).display()
    ))
}

/// Refuse to start a VM whose TPM was in use but whose state is gone.
pub fn check_tpm_state_for_start(
    vm_id: &str,
    disk0: &Path,
    hw: &horcrux_common::VmHardware,
) -> Result<()> {
    if hw.tpm && hw.tpm_state_expected && !tpm_state_present(disk0) {
        return Err(tpm_state_missing_error(vm_id, disk0));
    }
    Ok(())
}

/// Throw the TPM state away (the explicit "create new TPM" action).
pub async fn reset_tpm_state(disk0: &Path) -> Result<()> {
    stop_swtpm(disk0).await;
    let dir = tpm_state_dir(disk0);
    match tokio::fs::remove_dir_all(&dir).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::System(format!("removing {}: {e}", dir.display()))),
    }
}

/// A swtpm state blob starts with a header whose total length (big endian,
/// bytes 6..10) is the blob's own size: a truncated or foreign file fails.
pub fn tpm_blob_is_complete(b: &[u8]) -> bool {
    b.len() >= 10 && b[0] >= 2 && u32::from_be_bytes([b[6], b[7], b[8], b[9]]) as usize == b.len()
}

/// Copy a swtpm state directory (`tpm2-00.permall`, never the lock file) to
/// `to` and read every copy back: the result is the same bytes, or an error.
/// Returns the number of files copied. A directory without a TPM 2.0
/// permanent state is an error (a TPM 1.2 state, `tpm-00.permall`, cannot be
/// run by Horcrux's swtpm --tpm2).
pub fn copy_tpm_state_verified(from: &Path, to: &Path) -> Result<usize> {
    let src = from.join(TPM_PERMALL);
    let first = std::fs::read(&src).map_err(|e| {
        if from.join("tpm-00.permall").is_file() {
            Error::Validation(format!(
                "{} holds a TPM 1.2 state: only TPM 2.0 is supported",
                from.display()
            ))
        } else {
            Error::System(format!("reading {}: {e}", src.display()))
        }
    })?;
    if !tpm_blob_is_complete(&first) {
        return Err(Error::Validation(format!(
            "{} is not a complete swtpm TPM 2.0 state",
            src.display()
        )));
    }
    std::fs::create_dir_all(to)
        .map_err(|e| Error::System(format!("creating {}: {e}", to.display())))?;
    let mut n = 0;
    let rd = std::fs::read_dir(from)
        .map_err(|e| Error::System(format!("reading {}: {e}", from.display())))?;
    for e in rd {
        let e = e.map_err(|e| Error::System(e.to_string()))?;
        let name = e.file_name();
        let nm = name.to_string_lossy();
        if !e.file_type().map(|t| t.is_file()).unwrap_or(false) || nm.starts_with('.') {
            continue;
        }
        let dst = to.join(&name);
        let data = std::fs::read(e.path())
            .map_err(|er| Error::System(format!("reading {}: {er}", e.path().display())))?;
        write_private(&dst, &data)?;
        let back = std::fs::read(&dst)
            .map_err(|er| Error::System(format!("reading back {}: {er}", dst.display())))?;
        if back != data {
            return Err(Error::System(format!(
                "the copy {} differs from its source",
                dst.display()
            )));
        }
        n += 1;
    }
    Ok(n)
}

/// Write `data` to `path` (mode 0600, flushed to disk).
pub fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| Error::System(format!("writing {}: {e}", path.display())))?;
    f.write_all(data)
        .and_then(|_| f.sync_all())
        .map_err(|e| Error::System(format!("writing {}: {e}", path.display())))
}

/// The permanent state inside a swtpm *linear file* (`--tpmstate
/// backend-uri=file://...`, what Proxmox keeps in a VM's `tpmstate0`
/// volume): magic `nilmptws`, a header whose u32 at 0x0c is the data offset
/// and u32 at 0x10 its length, then the same state blob a state directory
/// holds as `tpm2-00.permall`. Anything that does not check out is refused.
pub fn tpm_state_from_linear(raw: &[u8]) -> Result<Vec<u8>> {
    let bad = |why: &str| Error::Validation(format!("not a swtpm TPM 2.0 state volume: {why}"));
    if raw.len() < 0x18 || &raw[..8] != b"nilmptws" {
        return Err(bad("no swtpm linear-file header (an empty or never-started TPM volume has no state to carry)"));
    }
    let u32le =
        |o: usize| u32::from_le_bytes([raw[o], raw[o + 1], raw[o + 2], raw[o + 3]]) as usize;
    let (off, len) = (u32le(0x0c), u32le(0x10));
    let end = off.checked_add(len).filter(|e| *e <= raw.len());
    let Some(end) = end.filter(|_| len > 10) else {
        return Err(bad("the header's data range is outside the volume"));
    };
    let blob = &raw[off..end];
    if !tpm_blob_is_complete(blob) {
        return Err(bad("the state blob is incomplete"));
    }
    Ok(blob.to_vec())
}

/// Install a Proxmox `tpmstate0` volume (or any swtpm linear file) as the
/// state directory `to`.
pub fn import_linear_tpm_volume(volume: &Path, to: &Path) -> Result<()> {
    let raw = std::fs::read(volume)
        .map_err(|e| Error::System(format!("reading {}: {e}", volume.display())))?;
    let blob = tpm_state_from_linear(&raw)?;
    std::fs::create_dir_all(to)
        .map_err(|e| Error::System(format!("creating {}: {e}", to.display())))?;
    let dst = to.join(TPM_PERMALL);
    write_private(&dst, &blob)?;
    if std::fs::read(&dst).map_err(|e| Error::System(e.to_string()))? != blob {
        return Err(Error::System(format!("the copy {} differs", dst.display())));
    }
    Ok(())
}

/// The per-VM firmware/TPM files a backup (or any export) must carry:
/// `(archive name, kind, path)`. The EFI variable store (boot entries, Secure
/// Boot keys) and every TPM state file; sockets, pidfiles and logs are not
/// state.
pub fn carried_state(disk0: &Path) -> Vec<(String, String, PathBuf)> {
    let mut v = Vec::new();
    let efi = efivars_path(disk0);
    if efi.is_file() {
        v.push(("efivars.qcow2".to_string(), "efivars".to_string(), efi));
    }
    if let Ok(rd) = std::fs::read_dir(tpm_state_dir(disk0)) {
        let mut files: Vec<_> = rd
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .collect();
        files.sort_by_key(|e| e.file_name());
        for e in files {
            v.push((
                format!("tpm-{}", e.file_name().to_string_lossy()),
                "tpm".to_string(),
                e.path(),
            ));
        }
    }
    v
}

/// Copy a TPM state file that a running swtpm may rewrite: retried until the
/// bytes read are a complete state blob (swtpm replaces the file as a whole,
/// so a torn read shows as a length mismatch).
pub fn copy_live_tpm_file(src: &Path, dst: &Path) -> Result<()> {
    let mut last = String::new();
    for _ in 0..10 {
        match std::fs::read(src) {
            Ok(b) if !src.ends_with(TPM_PERMALL) || tpm_blob_is_complete(&b) => {
                return write_private(dst, &b);
            }
            Ok(_) => last = "incomplete state blob".into(),
            Err(e) => last = e.to_string(),
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Err(Error::System(format!(
        "could not copy a consistent TPM state from {}: {last}",
        src.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A state blob with swtpm's header: version, min version, header size,
    /// flags, total length (big endian) at bytes 6..10, then payload.
    fn blob(total: usize) -> Vec<u8> {
        let mut b = vec![0u8; total];
        b[0] = 2;
        b[1] = 1;
        b[3] = 10;
        b[6..10].copy_from_slice(&(total as u32).to_be_bytes());
        for (i, x) in b.iter_mut().enumerate().skip(10) {
            *x = (i % 251) as u8;
        }
        b
    }

    fn linear(blob: &[u8], off: usize) -> Vec<u8> {
        let mut v = vec![0u8; off + blob.len() + 100];
        v[..8].copy_from_slice(b"nilmptws");
        v[8] = 1;
        v[0x0a] = off as u8;
        v[0x0c..0x10].copy_from_slice(&(off as u32).to_le_bytes());
        v[0x10..0x14].copy_from_slice(&(blob.len() as u32).to_le_bytes());
        v[off..off + blob.len()].copy_from_slice(blob);
        v
    }

    #[test]
    fn missing_tpm_state_refuses_the_start_only_when_the_vm_had_one() {
        let d = tempfile::tempdir().unwrap();
        let disk = d.path().join("vm.qcow2");
        let mut hw = horcrux_common::VmHardware {
            tpm: true,
            ..Default::default()
        };
        // never started: a fresh TPM is fine
        assert!(check_tpm_state_for_start("vm", &disk, &hw).is_ok());
        // it had state, now the directory is gone: refused, never replaced
        hw.tpm_state_expected = true;
        let e = check_tpm_state_for_start("vm", &disk, &hw).unwrap_err();
        assert!(matches!(e, Error::Conflict(_)));
        let m = e.to_string();
        for k in [
            "TPM state of VM vm is missing",
            "BitLocker",
            "tpm/reset",
            "confirm_data_loss",
        ] {
            assert!(m.contains(k), "{k} in {m}");
        }
        // an empty file is not state
        std::fs::create_dir_all(tpm_state_dir(&disk)).unwrap();
        std::fs::write(tpm_state_dir(&disk).join(TPM_PERMALL), b"").unwrap();
        assert!(check_tpm_state_for_start("vm", &disk, &hw).is_err());
        std::fs::write(tpm_state_dir(&disk).join(TPM_PERMALL), blob(64)).unwrap();
        assert!(tpm_state_present(&disk));
        assert!(check_tpm_state_for_start("vm", &disk, &hw).is_ok());
        // no TPM at all: nothing to check
        hw.tpm = false;
        std::fs::remove_dir_all(tpm_state_dir(&disk)).unwrap();
        assert!(check_tpm_state_for_start("vm", &disk, &hw).is_ok());
    }

    #[tokio::test]
    async fn reset_deletes_the_state_and_is_idempotent() {
        let d = tempfile::tempdir().unwrap();
        let disk = d.path().join("vm.qcow2");
        std::fs::create_dir_all(tpm_state_dir(&disk)).unwrap();
        std::fs::write(tpm_state_dir(&disk).join(TPM_PERMALL), blob(64)).unwrap();
        reset_tpm_state(&disk).await.unwrap();
        assert!(!tpm_state_dir(&disk).exists());
        reset_tpm_state(&disk).await.unwrap();
    }

    #[test]
    fn verified_copy_is_byte_identical_skips_the_lock_and_refuses_junk() {
        let d = tempfile::tempdir().unwrap();
        let (src, dst) = (d.path().join("src"), d.path().join("dst"));
        std::fs::create_dir_all(&src).unwrap();
        let b = blob(3055);
        std::fs::write(src.join(TPM_PERMALL), &b).unwrap();
        std::fs::write(src.join(".lock"), b"").unwrap();
        assert_eq!(copy_tpm_state_verified(&src, &dst).unwrap(), 1);
        assert_eq!(std::fs::read(dst.join(TPM_PERMALL)).unwrap(), b);
        assert!(!dst.join(".lock").exists());
        // truncated state: refused, nothing created
        std::fs::write(src.join(TPM_PERMALL), &b[..100]).unwrap();
        let other = d.path().join("other");
        assert!(copy_tpm_state_verified(&src, &other).is_err());
        assert!(!other.exists());
        // a TPM 1.2 state is named as such
        let v12 = d.path().join("v12");
        std::fs::create_dir_all(&v12).unwrap();
        std::fs::write(v12.join("tpm-00.permall"), b"x").unwrap();
        let e = copy_tpm_state_verified(&v12, &other)
            .unwrap_err()
            .to_string();
        assert!(e.contains("TPM 1.2"), "{e}");
    }

    #[test]
    fn proxmox_style_linear_volume_yields_the_permanent_state() {
        let b = blob(3126);
        let raw = linear(&b, 192);
        assert_eq!(tpm_state_from_linear(&raw).unwrap(), b);
        // not a swtpm volume / empty volume / lying header / cut-off blob
        assert!(tpm_state_from_linear(&vec![0u8; 4 << 20]).is_err());
        assert!(tpm_state_from_linear(b"nilmptws").is_err());
        let mut lie = raw.clone();
        lie[0x10..0x14].copy_from_slice(&(1u32 << 30).to_le_bytes());
        assert!(tpm_state_from_linear(&lie).is_err());
        let mut wrong_len = raw.clone();
        wrong_len[0x10..0x14].copy_from_slice(&3000u32.to_le_bytes());
        assert!(tpm_state_from_linear(&wrong_len).is_err());
        // installed as a state directory
        let d = tempfile::tempdir().unwrap();
        let vol = d.path().join("tpmstate0.raw");
        std::fs::write(&vol, &raw).unwrap();
        let to = d.path().join("vm.tpm");
        import_linear_tpm_volume(&vol, &to).unwrap();
        assert_eq!(std::fs::read(to.join(TPM_PERMALL)).unwrap(), b);
    }

    #[test]
    fn carried_state_lists_efivars_and_tpm_files_only() {
        let d = tempfile::tempdir().unwrap();
        let disk = d.path().join("vm.qcow2");
        assert!(carried_state(&disk).is_empty());
        std::fs::write(efivars_path(&disk), b"nv").unwrap();
        std::fs::create_dir_all(tpm_state_dir(&disk)).unwrap();
        std::fs::write(tpm_state_dir(&disk).join(TPM_PERMALL), blob(64)).unwrap();
        std::fs::write(tpm_state_dir(&disk).join(".lock"), b"").unwrap();
        std::fs::write(tpm_socket(&disk), b"").unwrap();
        let names: Vec<String> = carried_state(&disk).into_iter().map(|c| c.0).collect();
        assert_eq!(names, ["efivars.qcow2", "tpm-tpm2-00.permall"]);
    }

    #[test]
    fn state_lives_next_to_the_first_disk() {
        let d = Path::new("/vms/vm-1.qcow2");
        assert_eq!(efivars_path(d), PathBuf::from("/vms/vm-1.efivars.qcow2"));
        assert_eq!(tpm_state_dir(d), PathBuf::from("/vms/vm-1.tpm"));
        assert_eq!(tpm_socket(d), PathBuf::from("/vms/vm-1.tpm.sock"));
    }

    #[test]
    fn ovmf_override_needs_both_files() {
        let dir = tempfile::tempdir().unwrap();
        let code = dir.path().join("code.fd");
        let vars = dir.path().join("vars.fd");
        std::fs::write(&code, b"c").unwrap();
        std::env::set_var("HORCRUX_OVMF_CODE", &code);
        std::env::set_var("HORCRUX_OVMF_VARS", &vars);
        assert_eq!(find_ovmf(), None, "vars template missing");
        std::fs::write(&vars, b"v").unwrap();
        assert_eq!(
            find_ovmf(),
            Some(Ovmf {
                code: code.clone(),
                vars_template: vars.clone()
            })
        );
        std::env::remove_var("HORCRUX_OVMF_CODE");
        std::env::remove_var("HORCRUX_OVMF_VARS");
    }
}
