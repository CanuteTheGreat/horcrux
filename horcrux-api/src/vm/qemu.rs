//! QEMU/KVM integration
use horcrux_common::{Result, VmConfig, VmHypervisor, VmStatus};
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;
use tracing::{debug, error, info};

/// QEMU virtual machine instance
#[derive(Debug, Clone)]
pub struct QemuVm {
    pub id: String,
    pub name: String,
    pub memory: u64,
    pub cpus: u32,
    pub disk_path: PathBuf,
    pub disk_size: u64,
    pub status: VmStatus,
    pub _pid: Option<u32>, // Reserved for future process monitoring
    /// Which hypervisor backs this VM (QEMU, or an LXD/Incus VM).
    pub hypervisor: VmHypervisor,
    /// Guest CPU architecture (selects the qemu-system-* binary).
    pub architecture: horcrux_common::VmArchitecture,
    /// Display protocol, video card and sound card.
    pub display: horcrux_common::VmDisplay,
}

impl QemuVm {
    pub fn to_config(&self) -> VmConfig {
        VmConfig {
            id: self.id.clone(),
            name: self.name.clone(),
            hypervisor: self.hypervisor.clone(),
            memory: self.memory,
            cpus: self.cpus,
            disk_size: self.disk_size,
            status: self.status.clone(),
            architecture: self.architecture.clone(),
            disks: vec![horcrux_common::VmDisk {
                path: self.disk_path.to_string_lossy().to_string(),
                size_gb: self.disk_size,
                disk_type: "virtio".to_string(),
                cache: "writethrough".to_string(),
            }],
            display: self.display,
        }
    }
}

/// `qemu-system-*` binary and (for non-PC targets) the machine type.
pub fn qemu_binary_and_machine(
    arch: &horcrux_common::VmArchitecture,
) -> (&'static str, Option<&'static str>) {
    use horcrux_common::VmArchitecture::*;
    match arch {
        X86_64 => ("qemu-system-x86_64", None),
        Aarch64 => ("qemu-system-aarch64", Some("virt")),
        Riscv64 => ("qemu-system-riscv64", Some("virt")),
        Ppc64le => ("qemu-system-ppc64", Some("pseries")),
    }
}

/// Pidfile QEMU writes for a VM (`-pidfile`).
pub fn pidfile(vm_id: &str) -> String {
    format!("/var/run/horcrux-vm-{vm_id}.pid")
}

/// The complete QEMU command line for `vm`: binary plus every argument.
///
/// This is the ONE place QEMU arguments are assembled; `start_vm` only
/// runs what it returns. Add new device/feature arguments here (or in a
/// helper called from here, like `display::qemu_args` for the consoles),
/// and cover them in this module's tests.
pub fn qemu_command(
    vm: &QemuVm,
    runtime: &super::display::DisplayRuntime,
    kvm_available: bool,
) -> (&'static str, Vec<String>) {
    // Binary per guest architecture (this used to always be
    // qemu-system-x86_64, so aarch64/riscv64 VMs silently became x86).
    let (binary, machine) = qemu_binary_and_machine(&vm.architecture);
    let mut args: Vec<String> = Vec::new();

    // KVM when the host has it and the guest is the host's architecture,
    // otherwise software emulation (TCG) rather than failing. Uses
    // "-machine accel=..." since older QEMU lacks "-accel kvm:tcg".
    let native = vm.architecture == horcrux_common::VmArchitecture::default();
    let accel = if native && kvm_available {
        "kvm:tcg"
    } else {
        "tcg"
    };
    args.push("-machine".into());
    args.push(match machine {
        Some(m) => format!("{m},accel={accel}"),
        None => format!("accel={accel}"),
    });
    if machine.is_some() {
        args.extend(["-cpu".into(), "max".into()]);
    }

    // CPU, memory, disk.
    args.extend([
        "-m".into(),
        vm.memory.to_string(),
        "-smp".into(),
        vm.cpus.to_string(),
        "-drive".into(),
        format!("file={},format=qcow2", vm.disk_path.display()),
    ]);

    // Serial console, QMP, display protocol, video and sound.
    args.extend(super::display::qemu_args(
        &vm.display,
        &vm.architecture,
        runtime,
    ));

    // Run detached; the pidfile is how stop/status find the process.
    args.extend(["-daemonize".into(), "-pidfile".into(), pidfile(&vm.id)]);
    (binary, args)
}

/// QEMU manager for VM operations
pub struct QemuManager {
    storage_path: PathBuf,
}

impl Default for QemuManager {
    fn default() -> Self {
        Self::new()
    }
}

impl QemuManager {
    pub fn new() -> Self {
        Self {
            storage_path: PathBuf::from("/var/lib/horcrux/vms"),
        }
    }

    /// Create a QEMU manager that stores VM disk images under the given
    /// path instead of the hardcoded default. Lets deployments (and tests)
    /// honor `HORCRUX_DATA_DIR`/`HORCRUX_VM_STORAGE` instead of always
    /// requiring `/var/lib/horcrux/vms` to exist and be writable.
    pub fn with_storage_path(storage_path: PathBuf) -> Self {
        Self { storage_path }
    }

    /// Create a new QEMU virtual machine
    pub async fn create_vm(&self, config: &VmConfig) -> Result<QemuVm> {
        info!("Creating VM: {} (ID: {})", config.name, config.id);

        // Create storage directory if it doesn't exist
        tokio::fs::create_dir_all(&self.storage_path)
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to create storage directory: {}", e))
            })?;

        // Create disk image
        let disk_path = self.storage_path.join(format!("{}.qcow2", config.id));

        debug!(
            "Creating disk image at {:?} with size {}GB",
            disk_path, config.disk_size
        );

        let output = Command::new("qemu-img")
            .arg("create")
            .arg("-f")
            .arg("qcow2")
            .arg(&disk_path)
            .arg(format!("{}G", config.disk_size))
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to run qemu-img: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("qemu-img failed: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to create disk image: {}",
                stderr
            )));
        }

        info!("VM {} created successfully", config.id);

        Ok(QemuVm {
            id: config.id.clone(),
            name: config.name.clone(),
            memory: config.memory,
            cpus: config.cpus,
            disk_path,
            disk_size: config.disk_size,
            status: VmStatus::Stopped,
            _pid: None,
            hypervisor: VmHypervisor::Qemu,
            architecture: config.architecture.clone(),
            display: config.display,
        })
    }

    /// Rebuild the runtime handle for a VM that exists in the database but
    /// not in this process's memory (e.g. after an API restart). Nothing is
    /// created on disk; the status is taken from the live QEMU process.
    pub fn vm_from_config(&self, config: &VmConfig) -> QemuVm {
        let disk_path = config
            .disks
            .first()
            .map(|d| PathBuf::from(&d.path))
            .unwrap_or_else(|| self.storage_path.join(format!("{}.qcow2", config.id)));
        let status = if Self::process_running(&config.id) {
            VmStatus::Running
        } else {
            VmStatus::Stopped
        };
        QemuVm {
            id: config.id.clone(),
            name: config.name.clone(),
            memory: config.memory,
            cpus: config.cpus,
            disk_path,
            disk_size: config.disk_size,
            status,
            _pid: None,
            hypervisor: config.hypervisor.clone(),
            architecture: config.architecture.clone(),
            display: config.display,
        }
    }

    /// Whether the QEMU process for `vm_id` (per its pidfile) is alive.
    pub fn process_running(vm_id: &str) -> bool {
        std::fs::read_to_string(pidfile(vm_id))
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .map(|pid| std::path::Path::new(&format!("/proc/{pid}")).exists())
            .unwrap_or(false)
    }

    /// Start a QEMU virtual machine
    pub async fn start_vm(&self, vm: &QemuVm) -> Result<()> {
        info!("Starting VM: {} (ID: {})", vm.name, vm.id);

        if vm.status == VmStatus::Running {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "VM {} is already running",
                vm.id
            )));
        }

        // Consoles: serial Unix socket (+ <disk>.serial.log), QMP, and the
        // VM's display protocol / video / sound (see vm/display.rs).
        let runtime = super::display::DisplayRuntime::plan(&vm.disk_path, &vm.display)?;
        for stale in [&runtime.serial_socket, &runtime.qmp_socket] {
            let _ = tokio::fs::remove_file(stale).await;
        }
        let kvm = tokio::fs::metadata("/dev/kvm").await.is_ok();
        let (binary, args) = qemu_command(vm, &runtime, kvm);
        let mut cmd = Command::new(binary);
        cmd.args(&args).stdout(Stdio::null()).stderr(Stdio::piped());

        debug!("QEMU command: {:?}", cmd);

        let output = cmd
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to start VM: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("QEMU failed to start: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to start VM: {}",
                stderr
            )));
        }

        runtime.save(&vm.disk_path).await?;
        // Without a password nobody can attach to VNC/SPICE; set this run's.
        // If that fails, don't leave a VM running whose console nobody can
        // ever open: stop it and report the start as failed.
        if let Err(e) = super::display::apply_password(&runtime).await {
            error!(
                "VM {} started but console password could not be set: {}",
                vm.id, e
            );
            if let Some(pid) = std::fs::read_to_string(pidfile(&vm.id))
                .ok()
                .and_then(|s| s.trim().parse::<i32>().ok())
            {
                use nix::sys::signal::{kill, Signal};
                let _ = kill(nix::unistd::Pid::from_raw(pid), Signal::SIGTERM);
            }
            let _ = tokio::fs::remove_file(pidfile(&vm.id)).await;
            super::display::DisplayRuntime::remove(&vm.disk_path).await;
            return Err(e);
        }

        info!("VM {} started successfully", vm.id);
        Ok(())
    }

    /// Stop a QEMU virtual machine
    pub async fn stop_vm(&self, vm: &QemuVm) -> Result<()> {
        info!("Stopping VM: {} (ID: {})", vm.name, vm.id);

        if vm.status == VmStatus::Stopped {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "VM {} is already stopped",
                vm.id
            )));
        }

        // Read PID from pidfile
        let pidfile = pidfile(&vm.id);
        let pid_str = tokio::fs::read_to_string(&pidfile)
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to read pidfile: {}", e)))?;

        let pid: i32 = pid_str
            .trim()
            .parse()
            .map_err(|e| horcrux_common::Error::System(format!("Invalid PID in pidfile: {}", e)))?;

        // Send SIGTERM to gracefully shut down
        debug!("Sending SIGTERM to PID {}", pid);

        use nix::sys::signal::{kill, Signal};
        use nix::unistd::Pid;

        kill(Pid::from_raw(pid), Signal::SIGTERM)
            .map_err(|e| horcrux_common::Error::System(format!("Failed to stop VM: {}", e)))?;

        // Remove pidfile
        tokio::fs::remove_file(&pidfile).await.ok(); // Ignore errors if file doesn't exist
        super::display::DisplayRuntime::remove(&vm.disk_path).await;

        info!("VM {} stopped successfully", vm.id);
        Ok(())
    }

    /// Delete a QEMU virtual machine
    pub async fn delete_vm(&self, vm: &QemuVm) -> Result<()> {
        info!("Deleting VM: {} (ID: {})", vm.name, vm.id);

        // Ensure VM is stopped first
        if vm.status == VmStatus::Running {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Cannot delete running VM {}. Stop it first.",
                vm.id
            )));
        }

        // Delete disk image
        tokio::fs::remove_file(&vm.disk_path).await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to delete disk image: {}", e))
        })?;

        info!("VM {} deleted successfully", vm.id);
        Ok(())
    }

    /// Check if KVM is available
    pub fn check_kvm_available() -> bool {
        std::path::Path::new("/dev/kvm").exists()
    }

    /// Get QEMU version
    pub async fn get_qemu_version() -> Result<String> {
        let output = Command::new("qemu-system-x86_64")
            .arg("--version")
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run qemu-system-x86_64: {}", e))
            })?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(
                "QEMU not found or not working".to_string(),
            ));
        }

        let version = String::from_utf8_lossy(&output.stdout);
        Ok(version.lines().next().unwrap_or("Unknown").to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horcrux_common::{DisplayProtocol, SoundDevice, VideoDevice, VmArchitecture, VmDisplay};

    fn vm(arch: VmArchitecture, display: VmDisplay) -> QemuVm {
        QemuVm {
            id: "t1".into(),
            name: "t1".into(),
            memory: 256,
            cpus: 2,
            disk_path: "/vms/t1.qcow2".into(),
            disk_size: 1,
            status: VmStatus::Stopped,
            _pid: None,
            hypervisor: VmHypervisor::Qemu,
            architecture: arch,
            display,
        }
    }

    #[test]
    fn one_command_line_with_consoles_and_pidfile() {
        let d = VmDisplay {
            protocol: DisplayProtocol::Spice,
            video: VideoDevice::Qxl,
            sound: SoundDevice::IntelHda,
        };
        let v = vm(VmArchitecture::X86_64, d);
        let rt = super::super::display::DisplayRuntime::plan(&v.disk_path, &d).unwrap();
        let (bin, args) = qemu_command(&v, &rt, true);
        let a = args.join(" ");
        assert_eq!(bin, "qemu-system-x86_64");
        assert!(
            a.starts_with(
                "-machine accel=kvm:tcg -m 256 -smp 2 -drive file=/vms/t1.qcow2,format=qcow2"
            ),
            "{a}"
        );
        assert!(a.contains("-serial chardev:serial0") && a.contains("-qmp unix:/vms/t1.qmp.sock"));
        assert!(a.contains("-spice port=") && a.contains("-audiodev spice,id=snd0"));
        assert!(
            a.ends_with("-daemonize -pidfile /var/run/horcrux-vm-t1.pid"),
            "{a}"
        );
    }

    #[test]
    fn foreign_arch_uses_tcg_and_its_machine() {
        let v = vm(VmArchitecture::Aarch64, VmDisplay::default());
        let rt = super::super::display::DisplayRuntime::plan(&v.disk_path, &v.display).unwrap();
        let (bin, args) = qemu_command(&v, &rt, true);
        assert_eq!(bin, "qemu-system-aarch64");
        assert!(args
            .join(" ")
            .starts_with("-machine virt,accel=tcg -cpu max"));
    }
}
