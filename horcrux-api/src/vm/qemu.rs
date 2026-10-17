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
        }
    }

    /// Whether the QEMU process for `vm_id` (per its pidfile) is alive.
    pub fn process_running(vm_id: &str) -> bool {
        std::fs::read_to_string(format!("/var/run/horcrux-vm-{}.pid", vm_id))
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

        // Build QEMU command for the guest's architecture (this used to
        // always run qemu-system-x86_64, so aarch64/riscv64 VMs silently
        // became x86 guests).
        let (binary, machine) = qemu_binary_and_machine(&vm.architecture);
        let mut cmd = Command::new(binary);

        // Prefer hardware acceleration (KVM) when the host supports it and
        // the guest is the host's own architecture; otherwise fall back to
        // software emulation (TCG) rather than failing.
        // Note: use "-machine accel=..." (not "-accel") since older QEMU
        // builds don't support the standalone "-accel kvm:tcg" fallback list.
        let native = vm.architecture == horcrux_common::VmArchitecture::default();
        let accel = if native && tokio::fs::metadata("/dev/kvm").await.is_ok() {
            "kvm:tcg"
        } else {
            "tcg"
        };
        match machine {
            Some(m) => cmd.arg("-machine").arg(format!("{m},accel={accel}")),
            None => cmd.arg("-machine").arg(format!("accel={accel}")),
        };
        if machine.is_some() {
            cmd.arg("-cpu").arg("max");
        }

        // Redirect the guest serial console to a log file next to the disk
        // image. This is the only way to observe genuine guest boot
        // activity (kernel/init output, login prompts) since we run
        // "-display none -daemonize" and never attach an interactive
        // console; without it there is no way to tell a VM that is truly
        // booting an OS apart from one that silently sits at "no bootable
        // device". Harmless in production (just an extra log file) and
        // used by the nightly realistic-test workflow to assert on real
        // guest boot output.
        let serial_log_path = vm.disk_path.with_extension("serial.log");

        cmd.arg("-m")
            .arg(vm.memory.to_string())
            .arg("-smp")
            .arg(vm.cpus.to_string())
            .arg("-drive")
            .arg(format!("file={},format=qcow2", vm.disk_path.display()))
            .arg("-serial")
            .arg(format!("file:{}", serial_log_path.display()))
            .arg("-display")
            .arg("none")
            .arg("-daemonize")
            .arg("-pidfile")
            .arg(format!("/var/run/horcrux-vm-{}.pid", vm.id))
            .stdout(Stdio::null())
            .stderr(Stdio::null());

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
        let pidfile = format!("/var/run/horcrux-vm-{}.pid", vm.id);
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
