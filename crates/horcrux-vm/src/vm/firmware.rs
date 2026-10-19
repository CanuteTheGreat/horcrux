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

/// Find OVMF on this host. `HORCRUX_OVMF_CODE` + `HORCRUX_OVMF_VARS`
/// override the search (both must be set).
pub fn find_ovmf() -> Option<Ovmf> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
