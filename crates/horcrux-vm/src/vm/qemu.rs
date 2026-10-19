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
    /// Network cards, CD-ROM and boot order.
    pub hardware: horcrux_common::VmHardware,
    /// Every disk, `disks[0]` being the one at `disk_path` (the VM's
    /// console sockets, serial log, EFI vars and TPM state sit next to
    /// it). Empty = just `disk_path` on the legacy default bus.
    pub disks: Vec<horcrux_common::VmDisk>,
}

impl QemuVm {
    /// All disks, `disk_path` first (filled in for handles that only know
    /// the first disk).
    pub fn all_disks(&self) -> Vec<horcrux_common::VmDisk> {
        let mut disks = self.disks.clone();
        if disks.is_empty() {
            disks.push(horcrux_common::VmDisk {
                path: self.disk_path.to_string_lossy().to_string(),
                size_gb: self.disk_size,
                disk_type: "default".to_string(),
                ..Default::default()
            });
        } else {
            disks[0].path = self.disk_path.to_string_lossy().to_string();
        }
        disks
    }

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
            disks: self.all_disks(),
            display: self.display,
            hardware: self.hardware.clone(),
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

/// Whether `pid` is a live process. A zombie (exited, not yet reaped -
/// e.g. a daemonized QEMU whose parent is a container's non-reaping PID 1)
/// no longer runs the guest or writes its disks, so it counts as exited.
pub fn pid_alive(pid: u32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        // Format: "pid (comm) S ...": the state follows the last ')'.
        Ok(stat) => stat
            .rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .map(|state| state != "Z" && state != "X")
            .unwrap_or(true),
        Err(_) => false,
    }
}

/// Whether the pid in a VM's pidfile is a live QEMU process: not a zombie,
/// and still QEMU (a stale pidfile's pid may have been reused by anything).
fn qemu_running_from_pidfile(path: &std::path::Path) -> bool {
    let Some(pid) = std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
    else {
        return false;
    };
    if !pid_alive(pid) {
        return false;
    }
    std::fs::read(format!("/proc/{pid}/cmdline"))
        .map(|cmd| {
            let argv0 = cmd.split(|b| *b == 0).next().unwrap_or_default();
            String::from_utf8_lossy(argv0).contains("qemu")
        })
        .unwrap_or(false)
}

/// Where a VM's cloud-init seed ISO lives (next to its first disk).
pub fn cloudinit_seed(disk: &std::path::Path) -> PathBuf {
    disk.with_extension("cidata.iso")
}

/// Whether `vm` boots with UEFI (OVMF): only x86 guests set to OVMF (the
/// ARM/RISC-V `virt` machines bring their own firmware).
pub fn uses_ovmf(vm: &QemuVm) -> bool {
    vm.architecture == horcrux_common::VmArchitecture::X86_64
        && vm.hardware.firmware == horcrux_common::Firmware::Ovmf
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
    ovmf: Option<&super::firmware::Ovmf>,
) -> (&'static str, Vec<String>) {
    // Binary per guest architecture (this used to always be
    // qemu-system-x86_64, so aarch64/riscv64 VMs silently became x86).
    let (binary, machine) = qemu_binary_and_machine(&vm.architecture);
    let x86 = vm.architecture == horcrux_common::VmArchitecture::X86_64;
    // x86: the configured chipset (i440FX stays QEMU's plain default, so
    // existing VMs keep their exact command line).
    let machine = machine
        .or((x86 && !vm.hardware.machine.is_default()).then(|| vm.hardware.machine.qemu_name()));
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
    match &vm.hardware.cpu_type {
        Some(cpu) => args.extend(["-cpu".into(), cpu.clone()]),
        None if !x86 => args.extend(["-cpu".into(), "max".into()]),
        None => {}
    }

    // CPU topology and memory.
    args.extend(["-m".into(), vm.memory.to_string(), "-smp".into()]);
    args.push(match vm.hardware.sockets {
        Some(s) if s > 1 && vm.cpus.is_multiple_of(s) => {
            format!("{},sockets={s},cores={},threads=1", vm.cpus, vm.cpus / s)
        }
        _ => vm.cpus.to_string(),
    });

    // UEFI: read-only OVMF code + the VM's own variable store (qcow2).
    if uses_ovmf(vm) {
        if let Some(ovmf) = ovmf {
            args.extend([
                "-drive".into(),
                format!(
                    "if=pflash,unit=0,format=raw,readonly=on,file={}",
                    escape(&ovmf.code.to_string_lossy())
                ),
                "-drive".into(),
                format!(
                    "if=pflash,unit=1,id={},format=qcow2,file={}",
                    super::firmware::EFIVARS_DRIVE_ID,
                    escape(&super::firmware::efivars_path(&vm.disk_path).to_string_lossy())
                ),
            ]);
        }
    }

    // Disks (one -drive per disk; with a bus, plus its controller/device).
    let boot_index = BootIndex::new(vm);
    args.extend(disk_args(vm, &boot_index));

    // Cloud-init NoCloud seed (volume label "cidata"), generated by
    // POST /api/cloudinit/:vm_id next to the disk: attached as a read-only
    // CD-ROM so cloud images configure themselves on first boot.
    let seed = cloudinit_seed(&vm.disk_path);
    if seed.exists() {
        args.extend([
            "-drive".into(),
            format!("file={},format=raw,media=cdrom,readonly=on", seed.display()),
        ]);
    }

    // CD-ROM drive (installer ISOs) with a stable device id, so media can
    // be changed while the VM runs (QMP blockdev-change-medium / eject).
    args.extend(cdrom_args(vm, &boot_index));

    // Boot order: per-device bootindex (SeaBIOS and OVMF), or for VMs
    // with a legacy-bus disk (no -device to put it on) SeaBIOS letters.
    args.extend(boot_args(vm, &boot_index));

    // Networking. Without any -nic/-netdev QEMU silently adds a default
    // user-mode (SLIRP) NIC, which NATs the guest through the HOST's own
    // network stack: on a cloud host the guest could then read the host's
    // instance metadata service (169.254.169.254) and its IAM role
    // credentials. So a VM gets exactly the NICs it is configured with
    // (tap devices on host bridges, see `network_args`), and `-nic none`
    // when it has none. Only the implicit NIC is removed - no -nodefaults,
    // so QEMU's other default devices stay.
    args.extend(network_args(vm, &boot_index));

    // TPM 2.0: swtpm (started by `start_vm`) on its control socket.
    if vm.hardware.tpm {
        let device = if x86 { "tpm-tis" } else { "tpm-tis-device" };
        args.extend([
            "-chardev".into(),
            format!(
                "socket,id=chrtpm,path={}",
                escape(&super::firmware::tpm_socket(&vm.disk_path).to_string_lossy())
            ),
            "-tpmdev".into(),
            "emulator,id=tpm0,chardev=chrtpm".into(),
            "-device".into(),
            format!("{device},tpmdev=tpm0"),
        ]);
    }

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

/// The receiving QEMU of a live migration: `-S -incoming defer` before
/// `-daemonize`. -S keeps the guest paused after its state arrived, until
/// the source has completed its disk mirrors (the commit then resumes it).
pub fn with_incoming(mut args: Vec<String>) -> Vec<String> {
    let at = args
        .iter()
        .position(|a| a == "-daemonize")
        .unwrap_or(args.len());
    args.splice(
        at..at,
        [
            "-S".to_string(),
            "-incoming".to_string(),
            "defer".to_string(),
        ],
    );
    args
}

/// QMP/QOM id of a VM's CD-ROM drive (used by live media changes).
pub const CDROM_DEVICE_ID: &str = "cdrom0";

/// The host tap device of a VM's `index`-th NIC: `tap<8 hex>i<n>`, derived
/// from the VM id (Linux interface names are limited to 15 characters, VM
/// ids are longer) and stable across restarts, so firewall rules and
/// monitoring can find it.
pub fn tap_name(vm_id: &str, index: usize) -> String {
    horcrux_common::vm_tap_name(vm_id, index)
}

/// Network arguments for `vm`: one tap netdev + NIC device per configured
/// card (the taps are created and attached to their bridges by
/// `setup_taps` before QEMU starts), or `-nic none` when the VM has no
/// network (see the comment in `qemu_command`).
fn network_args(vm: &QemuVm, boot: &BootIndex) -> Vec<String> {
    if vm.hardware.nets.is_empty() {
        return vec!["-nic".into(), "none".into()];
    }
    let mut args = Vec::new();
    for (i, nic) in vm.hardware.nets.iter().enumerate() {
        args.push("-netdev".into());
        args.push(format!(
            "tap,id=net{i},ifname={},script=no,downscript=no",
            tap_name(&vm.id, i)
        ));
        let mut dev = format!("{},netdev=net{i},id=nic{i}", nic.model.qemu_device());
        if let Some(mac) = &nic.mac {
            dev.push_str(&format!(",mac={mac}"));
        }
        if let Some(n) = boot.nic(i) {
            dev.push_str(&format!(",bootindex={n}"));
        }
        args.push("-device".into());
        args.push(dev);
    }
    args
}

/// CD-ROM drive: IDE on the x86 PC machine, virtio-SCSI elsewhere (the
/// ARM/RISC-V `virt` machines have no IDE). Always present, empty when no
/// ISO is configured, so an ISO can be inserted later without a restart.
fn cdrom_args(vm: &QemuVm, boot: &BootIndex) -> Vec<String> {
    let mut drive = "if=none,id=cd0,media=cdrom,readonly=on".to_string();
    if let Some(iso) = &vm.hardware.cdrom {
        drive.push_str(&format!(",format=raw,file={}", escape(iso)));
    }
    let bootindex = boot
        .cdrom()
        .map(|n| format!(",bootindex={n}"))
        .unwrap_or_default();
    let mut args = vec!["-drive".into(), drive];
    if vm.architecture == horcrux_common::VmArchitecture::X86_64 {
        args.extend([
            "-device".into(),
            format!("ide-cd,drive=cd0,id={CDROM_DEVICE_ID}{bootindex}"),
        ]);
    } else {
        args.extend([
            "-device".into(),
            "virtio-scsi-pci,id=cdscsi".into(),
            "-device".into(),
            format!("scsi-cd,bus=cdscsi.0,drive=cd0,id={CDROM_DEVICE_ID}{bootindex}"),
        ]);
    }
    args
}

/// Escape a value for QEMU's option parser (a comma is written twice).
fn escape(v: &str) -> String {
    v.replace(',', ",,")
}

/// QEMU drive id (QMP `device`/block backend name) of a VM's `index`-th
/// disk. Stable, so snapshots, live migration mirrors and resizes can
/// address a disk by it.
pub fn disk_drive_id(index: usize) -> String {
    format!("drive-disk{index}")
}

/// Image format of a disk file, from its name (Horcrux creates qcow2).
pub fn disk_format(path: &str) -> &'static str {
    if path.ends_with(".qcow2") {
        "qcow2"
    } else {
        "raw"
    }
}

/// Boot priority of each bootable device (`bootindex=`), from the VM's
/// boot order. Only used when every disk has a bus; a VM with a legacy
/// default-bus disk keeps SeaBIOS `-boot order=` letters (that disk has
/// no -device to carry a bootindex).
struct BootIndex {
    disks: Vec<Option<u32>>,
    cdrom: Option<u32>,
    nics: Vec<Option<u32>>,
    legacy: bool,
}

impl BootIndex {
    fn new(vm: &QemuVm) -> Self {
        use horcrux_common::BootDevice;
        let disks = vm.all_disks();
        let legacy = disks.iter().any(|d| d.disk_type == "default");
        let order: Vec<BootDevice> = if vm.hardware.boot.is_empty() {
            vec![BootDevice::Disk, BootDevice::Cdrom]
        } else {
            vm.hardware.boot.clone()
        };
        let mut b = BootIndex {
            disks: vec![None; disks.len()],
            cdrom: None,
            nics: vec![None; vm.hardware.nets.len()],
            legacy,
        };
        if legacy {
            return b;
        }
        let mut next = 1u32;
        for dev in order {
            match dev {
                BootDevice::Disk => {
                    for d in b.disks.iter_mut() {
                        *d = Some(next);
                        next += 1;
                    }
                }
                BootDevice::Cdrom => {
                    b.cdrom = Some(next);
                    next += 1;
                }
                BootDevice::Net => {
                    for n in b.nics.iter_mut() {
                        *n = Some(next);
                        next += 1;
                    }
                }
            }
        }
        b
    }
    fn disk(&self, i: usize) -> Option<u32> {
        self.disks.get(i).copied().flatten()
    }
    fn cdrom(&self) -> Option<u32> {
        self.cdrom
    }
    fn nic(&self, i: usize) -> Option<u32> {
        self.nics.get(i).copied().flatten()
    }
}

/// `-drive` (+ controller and device) arguments of every disk.
fn disk_args(vm: &QemuVm, boot: &BootIndex) -> Vec<String> {
    let mut args = Vec::new();
    let mut sata_port = 0usize;
    for (i, d) in vm.all_disks().iter().enumerate() {
        let id = disk_drive_id(i);
        let fmt = disk_format(&d.path);
        if d.disk_type == "default" {
            // Exactly the drive every VM got before buses existed (QEMU's
            // default interface), plus a stable id.
            args.extend([
                "-drive".into(),
                format!("file={},format={fmt},id={id}", escape(&d.path)),
            ]);
            continue;
        }
        let mut drive = format!("if=none,id={id},file={},format={fmt}", escape(&d.path));
        if !d.cache.is_empty() {
            drive.push_str(&format!(",cache={}", d.cache));
        }
        if d.discard {
            drive.push_str(",discard=unmap");
        }
        args.extend(["-drive".into(), drive]);
        if d.iothread {
            args.extend(["-object".into(), format!("iothread,id=io{i}")]);
        }
        let bootindex = boot
            .disk(i)
            .map(|n| format!(",bootindex={n}"))
            .unwrap_or_default();
        let ssd = if d.ssd { ",rotation_rate=1" } else { "" };
        let iothread = if d.iothread {
            format!(",iothread=io{i}")
        } else {
            String::new()
        };
        match d.disk_type.as_str() {
            "virtio" => args.extend([
                "-device".into(),
                format!("virtio-blk-pci,drive={id},id=disk{i}{iothread}{bootindex}"),
            ]),
            "scsi" => args.extend([
                // One virtio-scsi controller per disk (like Proxmox's
                // virtio-scsi-single), so each can have its own I/O thread.
                "-device".into(),
                format!("virtio-scsi-pci,id=scsihw{i}{iothread}"),
                "-device".into(),
                format!("scsi-hd,bus=scsihw{i}.0,drive={id},id=disk{i}{ssd}{bootindex}"),
            ]),
            "sata" => {
                if sata_port == 0 {
                    args.extend(["-device".into(), "ahci,id=ahci0".into()]);
                }
                args.extend([
                    "-device".into(),
                    format!("ide-hd,bus=ahci0.{sata_port},drive={id},id=disk{i}{ssd}{bootindex}"),
                ]);
                sata_port += 1;
            }
            _ => args.extend([
                "-device".into(),
                format!("ide-hd,drive={id},id=disk{i}{ssd}{bootindex}"),
            ]),
        }
    }
    args
}

/// `-boot order=...` (SeaBIOS letters) for x86 guests that have a legacy
/// default-bus disk; everything else boots by bootindex. Default: disk,
/// then CD-ROM - a blank disk falls through to the installer ISO.
fn boot_args(vm: &QemuVm, boot: &BootIndex) -> Vec<String> {
    use horcrux_common::BootDevice;
    if vm.architecture != horcrux_common::VmArchitecture::X86_64 || !boot.legacy {
        return Vec::new();
    }
    let order: String = if vm.hardware.boot.is_empty() {
        "cd".into()
    } else {
        vm.hardware
            .boot
            .iter()
            .map(|d| match d {
                BootDevice::Disk => 'c',
                BootDevice::Cdrom => 'd',
                BootDevice::Net => 'n',
            })
            .collect()
    };
    vec!["-boot".into(), format!("order={order}")]
}

async fn run_ip(program: &str, args: &[&str]) -> Result<()> {
    let out = Command::new(program)
        .args(args)
        .output()
        .await
        .map_err(|e| horcrux_common::Error::System(format!("{program}: {e}")))?;
    if !out.status.success() {
        return Err(horcrux_common::Error::System(format!(
            "{program} {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// The host's network devices as Horcrux sees them: sysfs `class/net` to
/// look them up and the iproute2 programs (`ip`, `bridge`) that change
/// them. Production uses [`HostNet::system`]; tests point it at a fake
/// sysfs tree and stand-in programs, since real bridges and taps need
/// CAP_NET_ADMIN.
#[derive(Debug, Clone)]
pub struct HostNet {
    sys_class_net: PathBuf,
    /// argv prefix that runs `ip` / `bridge`.
    ip: Vec<String>,
    bridge: Vec<String>,
}

impl HostNet {
    /// This host: /sys/class/net, `ip` and `bridge` from PATH.
    pub fn system() -> Self {
        Self {
            sys_class_net: PathBuf::from("/sys/class/net"),
            ip: vec!["ip".into()],
            bridge: vec!["bridge".into()],
        }
    }

    async fn run(tool: &[String], args: &[&str]) -> Result<()> {
        let mut argv: Vec<&str> = tool[1..].iter().map(String::as_str).collect();
        argv.extend_from_slice(args);
        run_ip(&tool[0], &argv).await
    }

    fn exists(&self, ifname: &str) -> bool {
        self.sys_class_net.join(ifname).exists()
    }

    /// Whether the host has a Linux bridge called `name`, and if it
    /// filters VLANs (`vlan_filtering`).
    pub fn bridge_vlan_aware(&self, name: &str) -> Option<bool> {
        if !horcrux_common::valid_ifname(name) {
            return None;
        }
        let dir = self.sys_class_net.join(name);
        if !dir.join("bridge").is_dir() {
            return None;
        }
        Some(
            std::fs::read_to_string(dir.join("bridge/vlan_filtering"))
                .map(|s| s.trim() == "1")
                .unwrap_or(false),
        )
    }

    /// Create the tap device of each NIC and plug it into its bridge (with
    /// the VLAN tag as the port's PVID) before QEMU starts. On failure
    /// every tap made so far is removed again.
    pub async fn setup_taps(&self, vm: &QemuVm) -> Result<()> {
        for (i, nic) in vm.hardware.nets.iter().enumerate() {
            if let Err(e) = self.setup_tap(vm, i, nic).await {
                self.remove_taps(&vm.id).await;
                return Err(e);
            }
        }
        Ok(())
    }

    async fn setup_tap(&self, vm: &QemuVm, i: usize, nic: &horcrux_common::VmNic) -> Result<()> {
        let vlan_aware = self.bridge_vlan_aware(&nic.bridge).ok_or_else(|| {
            horcrux_common::Error::Validation(format!(
                "net{i}: bridge {} does not exist on this host",
                nic.bridge
            ))
        })?;
        if nic.vlan.is_some() && !vlan_aware {
            return Err(horcrux_common::Error::Validation(format!(
                "net{i}: VLAN tag set but bridge {} is not VLAN-aware (vlan_filtering is off)",
                nic.bridge
            )));
        }
        let tap = tap_name(&vm.id, i);
        // A tap left by a QEMU that died on its own: start from a clean one.
        if self.exists(&tap) {
            let _ = Self::run(&self.ip, &["link", "del", &tap]).await;
        }
        Self::run(&self.ip, &["tuntap", "add", "dev", &tap, "mode", "tap"]).await?;
        Self::run(&self.ip, &["link", "set", &tap, "master", &nic.bridge]).await?;
        if let Some(vid) = nic.vlan {
            let vid = vid.to_string();
            let _ = Self::run(&self.bridge, &["vlan", "del", "dev", &tap, "vid", "1"]).await;
            Self::run(
                &self.bridge,
                &["vlan", "add", "dev", &tap, "vid", &vid, "pvid", "untagged"],
            )
            .await?;
        }
        Self::run(&self.ip, &["link", "set", &tap, "up"]).await?;
        debug!("VM {}: net{i} = {tap} on {}", vm.id, nic.bridge);
        Ok(())
    }

    /// Remove a VM's tap devices (after stop, failed start or delete). Taps
    /// of NICs removed from the config since the last start are covered by
    /// scanning up to the maximum NIC count.
    pub async fn remove_taps(&self, vm_id: &str) {
        for i in 0..horcrux_common::MAX_VM_NICS {
            let tap = tap_name(vm_id, i);
            if self.exists(&tap) {
                if let Err(e) = Self::run(&self.ip, &["link", "del", &tap]).await {
                    tracing::warn!("VM {vm_id}: could not remove {tap}: {e}");
                }
            }
        }
    }
}

/// Clean up after a QEMU that exited by itself (guest powered off): its
/// pidfile, console sockets/state and tap devices.
pub async fn cleanup_after_exit(vm: &QemuVm) {
    let _ = tokio::fs::remove_file(pidfile(&vm.id)).await;
    super::display::DisplayRuntime::remove(&vm.disk_path).await;
    HostNet::system().remove_taps(&vm.id).await;
}

/// Checks across a VM's disks: no two share a file, and the buses with a
/// fixed number of ports aren't overfilled (6 SATA ports on the AHCI
/// controller; IDE has 4 units, one of them the CD-ROM).
pub fn validate_disk_set(disks: &[horcrux_common::VmDisk]) -> std::result::Result<(), String> {
    for (i, d) in disks.iter().enumerate() {
        if disks[..i]
            .iter()
            .any(|o| o.path == d.path && !d.path.is_empty())
        {
            return Err(format!(
                "disk{i}: {} is already another disk of this VM",
                d.path
            ));
        }
    }
    let sata = disks.iter().filter(|d| d.disk_type == "sata").count();
    if sata > 6 {
        return Err(format!(
            "{sata} SATA disks: the SATA controller has 6 ports"
        ));
    }
    let ide = disks
        .iter()
        .filter(|d| matches!(d.disk_type.as_str(), "ide" | "default"))
        .count();
    if ide > 3 {
        return Err(format!(
            "{ide} IDE disks: IDE has room for 3 disks next to the CD-ROM; use VirtIO, SCSI or SATA"
        ));
    }
    Ok(())
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

    /// Path of a VM's `index`-th disk in this manager's storage directory.
    pub fn disk_file(&self, vm_id: &str, index: usize) -> PathBuf {
        if index == 0 {
            self.storage_path.join(format!("{vm_id}.qcow2"))
        } else {
            self.storage_path.join(format!("{vm_id}-disk{index}.qcow2"))
        }
    }

    /// The VM storage directory.
    pub fn storage_path(&self) -> &std::path::Path {
        &self.storage_path
    }

    /// Create a qcow2 image of `size_gb` GB at `path`.
    pub async fn create_disk_image(path: &std::path::Path, size_gb: u64) -> Result<()> {
        debug!("Creating disk image at {:?} with size {}GB", path, size_gb);
        let output = Command::new("qemu-img")
            .arg("create")
            .arg("-f")
            .arg("qcow2")
            .arg(path)
            .arg(format!("{size_gb}G"))
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
        Ok(())
    }

    /// Create an empty qcow2 image of exactly `bytes` virtual size (the
    /// receiving end of a live migration's disk mirror).
    pub async fn create_disk_image_bytes(path: &std::path::Path, bytes: u64) -> Result<()> {
        let output = Command::new("qemu-img")
            .args(["create", "-f", "qcow2"])
            .arg(path)
            .arg(bytes.to_string())
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to run qemu-img: {}", e)))?;
        if !output.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "Failed to create disk image {}: {}",
                path.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(())
    }

    /// Virtual size in bytes of a disk image (`qemu-img info`).
    pub async fn virtual_size(path: &std::path::Path) -> Result<u64> {
        let out = Command::new("qemu-img")
            .args(["info", "--output=json", "-U"])
            .arg(path)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("qemu-img: {e}")))?;
        if !out.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "qemu-img info {}: {}",
                path.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        let v: serde_json::Value = serde_json::from_slice(&out.stdout)
            .map_err(|e| horcrux_common::Error::System(format!("qemu-img info: {e}")))?;
        v["virtual-size"].as_u64().ok_or_else(|| {
            horcrux_common::Error::System(format!(
                "qemu-img info {}: no virtual-size",
                path.display()
            ))
        })
    }

    /// The disks a new VM gets: `config.disks` describes them (bus, cache,
    /// options, size; a size of 0 on the first means `disk_size`) - their
    /// paths are always chosen here, never taken from the request - or,
    /// without any, one VirtIO disk of `disk_size` GB.
    pub fn planned_disks(&self, config: &VmConfig) -> Result<Vec<horcrux_common::VmDisk>> {
        let mut disks: Vec<horcrux_common::VmDisk> = if config.disks.is_empty() {
            vec![horcrux_common::VmDisk {
                size_gb: config.disk_size,
                disk_type: "virtio".into(),
                ..Default::default()
            }]
        } else {
            config.disks.clone()
        };
        if disks.len() > horcrux_common::MAX_VM_DISKS {
            return Err(horcrux_common::Error::Validation(format!(
                "a VM can have at most {} disks",
                horcrux_common::MAX_VM_DISKS
            )));
        }
        for (i, d) in disks.iter_mut().enumerate() {
            if d.disk_type.is_empty() {
                d.disk_type = "virtio".into();
            }
            if i == 0 && d.size_gb == 0 {
                d.size_gb = config.disk_size;
            }
            if d.size_gb == 0 {
                return Err(horcrux_common::Error::Validation(format!(
                    "disk{i}: size must be at least 1 GB"
                )));
            }
            d.validate(i).map_err(horcrux_common::Error::Validation)?;
            d.path = self.disk_file(&config.id, i).to_string_lossy().to_string();
        }
        validate_disk_set(&disks).map_err(horcrux_common::Error::Validation)?;
        Ok(disks)
    }

    /// Create a new QEMU virtual machine (and each of its disks).
    pub async fn create_vm(&self, config: &VmConfig) -> Result<QemuVm> {
        info!("Creating VM: {} (ID: {})", config.name, config.id);

        // Create storage directory if it doesn't exist
        tokio::fs::create_dir_all(&self.storage_path)
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to create storage directory: {}", e))
            })?;

        let disks = self.planned_disks(config)?;
        let mut created: Vec<PathBuf> = Vec::new();
        for d in &disks {
            let path = PathBuf::from(&d.path);
            if let Err(e) = Self::create_disk_image(&path, d.size_gb).await {
                for c in &created {
                    let _ = tokio::fs::remove_file(c).await;
                }
                return Err(e);
            }
            created.push(path);
        }
        let disk_path = created[0].clone();

        info!("VM {} created successfully", config.id);

        Ok(QemuVm {
            id: config.id.clone(),
            name: config.name.clone(),
            memory: config.memory,
            cpus: config.cpus,
            disk_path,
            disk_size: disks[0].size_gb,
            status: VmStatus::Stopped,
            _pid: None,
            hypervisor: VmHypervisor::Qemu,
            architecture: config.architecture.clone(),
            display: config.display,
            hardware: config.hardware.clone(),
            disks,
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
            hardware: config.hardware.clone(),
            disks: config.disks.clone(),
        }
    }

    /// Whether the QEMU process for `vm_id` (per its pidfile) is alive (see
    /// `qemu_running_from_pidfile`: not a zombie, and still QEMU).
    pub fn process_running(vm_id: &str) -> bool {
        qemu_running_from_pidfile(std::path::Path::new(&pidfile(vm_id)))
    }

    /// Start a QEMU virtual machine
    pub async fn start_vm(&self, vm: &QemuVm) -> Result<()> {
        self.start_vm_with(vm, false).await
    }

    /// Start QEMU for the receiving side of a live migration: everything
    /// as for a normal start, but with `-incoming defer` (the guest stays
    /// paused until `migrate-incoming` brings its state over QMP).
    pub async fn start_vm_incoming(&self, vm: &QemuVm) -> Result<()> {
        self.start_vm_with(vm, true).await
    }

    async fn start_vm_with(&self, vm: &QemuVm, incoming: bool) -> Result<()> {
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
        if let Some(iso) = &vm.hardware.cdrom {
            if !std::path::Path::new(iso).is_file() {
                return Err(horcrux_common::Error::Validation(format!(
                    "The ISO in the VM's CD-ROM drive is missing: {iso}"
                )));
            }
        }
        for (i, d) in vm.all_disks().iter().enumerate() {
            if !std::path::Path::new(&d.path).exists() {
                return Err(horcrux_common::Error::Validation(format!(
                    "disk{i} of VM {} is missing: {}",
                    vm.id, d.path
                )));
            }
        }
        // UEFI: OVMF must be installed; the VM gets its variable store.
        let ovmf = if uses_ovmf(vm) {
            let o = super::firmware::find_ovmf().ok_or_else(super::firmware::ovmf_missing)?;
            super::firmware::ensure_efivars(&vm.disk_path, &o).await?;
            Some(o)
        } else {
            None
        };
        // TPM: swtpm serves the socket QEMU connects to.
        if vm.hardware.tpm {
            super::firmware::start_swtpm(&vm.disk_path).await?;
        }
        // Tap devices on the configured bridges (before QEMU opens them).
        if let Err(e) = HostNet::system().setup_taps(vm).await {
            super::firmware::stop_swtpm(&vm.disk_path).await;
            return Err(e);
        }
        let kvm = tokio::fs::metadata("/dev/kvm").await.is_ok();
        let (binary, mut args) = qemu_command(vm, &runtime, kvm, ovmf.as_ref());
        if incoming {
            args = with_incoming(args);
        }
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
            // QEMU creates the serial/QMP sockets before it opens the disks;
            // a start that fails afterwards (e.g. a missing backing file)
            // would otherwise leave them behind.
            let _ = tokio::fs::remove_file(pidfile(&vm.id)).await;
            super::display::DisplayRuntime::remove(&vm.disk_path).await;
            HostNet::system().remove_taps(&vm.id).await;
            super::firmware::stop_swtpm(&vm.disk_path).await;
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
            HostNet::system().remove_taps(&vm.id).await;
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

        // Wait (bounded) for QEMU to exit so its tap devices are free.
        for _ in 0..50 {
            if !pid_alive(pid as u32) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        // Remove pidfile
        tokio::fs::remove_file(&pidfile).await.ok(); // Ignore errors if file doesn't exist
        super::display::DisplayRuntime::remove(&vm.disk_path).await;
        HostNet::system().remove_taps(&vm.id).await;

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

        // Files that belong to the disk: console state + serial/QMP
        // sockets (left behind by a QEMU that failed to start or was
        // killed), serial log, cloud-init seed, an interrupted operation's
        // hold marker.
        super::display::DisplayRuntime::remove(&vm.disk_path).await;
        for ext in ["cidata.iso", "serial.log", "display.json", "hold.json"] {
            let _ = tokio::fs::remove_file(vm.disk_path.with_extension(ext)).await;
        }
        let _ = tokio::fs::remove_file(pidfile(&vm.id)).await;
        HostNet::system().remove_taps(&vm.id).await;
        super::firmware::remove_state(&vm.disk_path).await;

        // Delete the disk images (already gone is fine: the rest is
        // cleaned). The first disk last, since the others' side files
        // are found through it.
        let mut disks: Vec<PathBuf> = vm
            .all_disks()
            .iter()
            .map(|d| PathBuf::from(&d.path))
            .collect();
        disks.rotate_left(1);
        for disk in disks {
            match tokio::fs::remove_file(&disk).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(horcrux_common::Error::System(format!(
                        "Failed to delete disk image {}: {}",
                        disk.display(),
                        e
                    )))
                }
            }
        }

        info!("VM {} deleted successfully", vm.id);
        Ok(())
    }

    /// The images `disk` sits on top of in its qcow2 backing chain,
    /// nearest first, as absolute paths. A backing file that no longer
    /// exists is still listed (it ends the chain). Empty for a standalone
    /// image or one qemu-img can't read.
    pub async fn backing_files(disk: &std::path::Path) -> Vec<PathBuf> {
        let mut chain = Vec::new();
        let mut current = disk.to_path_buf();
        // A chain longer than this is a loop, not something to follow.
        for _ in 0..32 {
            let out = match Command::new("qemu-img")
                .args(["info", "--output=json", "-U"])
                .arg(&current)
                .output()
                .await
            {
                Ok(o) if o.status.success() => o,
                _ => break,
            };
            let info: serde_json::Value = match serde_json::from_slice(&out.stdout) {
                Ok(v) => v,
                Err(_) => break,
            };
            let backing = info["full-backing-filename"]
                .as_str()
                .or_else(|| info["backing-filename"].as_str())
                .filter(|b| !b.is_empty());
            let Some(backing) = backing else { break };
            let mut next = PathBuf::from(backing);
            if next.is_relative() {
                if let Some(dir) = current.parent() {
                    next = dir.join(next);
                }
            }
            chain.push(next.clone());
            if !next.is_file() {
                break;
            }
            current = next;
        }
        chain
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
    #[test]
    fn exited_unreaped_qemu_is_not_running() {
        use std::os::unix::process::CommandExt;
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("vm.pid");
        // No pidfile / garbage pidfile.
        assert!(!qemu_running_from_pidfile(&pidfile));
        std::fs::write(&pidfile, "not-a-pid").unwrap();
        assert!(!qemu_running_from_pidfile(&pidfile));

        // A live process that is QEMU (argv[0] as QEMU sets it) is running.
        let mut live = std::process::Command::new("sleep")
            .arg0("qemu-system-x86_64")
            .arg("30")
            .spawn()
            .unwrap();
        // spawn() returns once the child's close-on-exec fds are closed,
        // which happens inside execve before the kernel has set up the new
        // argv; /proc/<pid>/cmdline reads empty for that instant. Wait for
        // the exec to finish (a real QEMU writes its pidfile long after).
        for _ in 0..250 {
            if std::fs::read(format!("/proc/{}/cmdline", live.id()))
                .map(|c| !c.is_empty())
                .unwrap_or(false)
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        std::fs::write(&pidfile, format!("{}\n", live.id())).unwrap();
        assert!(qemu_running_from_pidfile(&pidfile), "live QEMU is running");

        // The same QEMU after it exited but before anyone reaped it: a
        // zombie, which must count as stopped.
        live.kill().unwrap();
        let zpid = live.id();
        for _ in 0..250 {
            if !pid_alive(zpid) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            std::fs::read_to_string(format!("/proc/{zpid}/stat")).is_ok(),
            "still unreaped (a zombie) at this point"
        );
        assert!(!pid_alive(zpid), "zombie is not alive");
        assert!(
            !qemu_running_from_pidfile(&pidfile),
            "exited-but-unreaped QEMU is not running"
        );
        live.wait().unwrap();
        assert!(!qemu_running_from_pidfile(&pidfile), "reaped QEMU is gone");

        // A stale pidfile whose pid now belongs to something else.
        let mut other = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        std::fs::write(&pidfile, other.id().to_string()).unwrap();
        assert!(
            !qemu_running_from_pidfile(&pidfile),
            "a reused pid that is not QEMU is not the VM"
        );
        other.kill().unwrap();
        other.wait().unwrap();
    }

    use super::*;

    #[test]
    fn cloudinit_seed_is_attached_only_when_generated() {
        let d = std::env::temp_dir().join(format!("ci-seed-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let mut v = vm(VmArchitecture::X86_64, VmDisplay::default());
        v.disk_path = d.join("seed-vm.qcow2");
        let rt = crate::vm::display::DisplayRuntime::plan(&v.disk_path, &v.display).unwrap();
        let has_seed = |args: &[String]| args.iter().any(|a| a.contains("cidata.iso"));
        let (_, args) = qemu_command(&v, &rt, false, None);
        assert!(!has_seed(&args), "no seed generated: {args:?}");
        std::fs::write(cloudinit_seed(&v.disk_path), b"iso").unwrap();
        let (_, args) = qemu_command(&v, &rt, false, None);
        let seed = format!(
            "file={},format=raw,media=cdrom,readonly=on",
            d.join("seed-vm.cidata.iso").display()
        );
        assert!(args.contains(&seed), "{args:?}");
        std::fs::remove_dir_all(&d).unwrap();
    }

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
            hardware: Default::default(),
            disks: Vec::new(),
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
        let (bin, args) = qemu_command(&v, &rt, true, None);
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

    /// QEMU's implicit default NIC is user-mode networking through the
    /// host's own stack (a guest on a cloud host could read the host's
    /// instance metadata). Every generated command line must disable it,
    /// on every guest architecture, without -nodefaults (which would also
    /// drop QEMU's other default devices).
    #[test]
    fn no_implicit_user_mode_nic() {
        for arch in [
            VmArchitecture::X86_64,
            VmArchitecture::Aarch64,
            VmArchitecture::Riscv64,
        ] {
            let v = vm(arch.clone(), VmDisplay::default());
            let rt = super::super::display::DisplayRuntime::plan(&v.disk_path, &v.display).unwrap();
            let (_, args) = qemu_command(&v, &rt, true, None);
            let nic = args
                .windows(2)
                .filter(|w| w[0] == "-nic" || w[0] == "-net" || w[0] == "-netdev")
                .map(|w| format!("{} {}", w[0], w[1]))
                .collect::<Vec<_>>();
            assert_eq!(nic, vec!["-nic none".to_string()], "{arch:?}: {args:?}");
            assert!(
                !args
                    .iter()
                    .any(|a| a == "-nodefaults" || a.contains("user")),
                "{arch:?}: {args:?}"
            );
        }
    }

    #[test]
    fn configured_nics_are_tap_devices_on_their_bridges() {
        use horcrux_common::{NicModel, VmNic};
        let mut v = vm(VmArchitecture::X86_64, VmDisplay::default());
        v.hardware.nets = vec![
            VmNic {
                model: NicModel::Virtio,
                bridge: "vmbr0".into(),
                mac: Some("02:00:00:00:00:01".into()),
                vlan: Some(20),
            },
            VmNic {
                model: NicModel::E1000,
                bridge: "br1".into(),
                mac: Some("02:00:00:00:00:02".into()),
                vlan: None,
            },
        ];
        let rt = super::super::display::DisplayRuntime::plan(&v.disk_path, &v.display).unwrap();
        let (_, args) = qemu_command(&v, &rt, true, None);
        let a = args.join(" ");
        let (t0, t1) = (tap_name("t1", 0), tap_name("t1", 1));
        assert!(
            a.contains(&format!(
                "-netdev tap,id=net0,ifname={t0},script=no,downscript=no -device virtio-net-pci,netdev=net0,id=nic0,mac=02:00:00:00:00:01"
            )),
            "{a}"
        );
        assert!(
            a.contains(&format!(
                "-netdev tap,id=net1,ifname={t1},script=no,downscript=no -device e1000,netdev=net1,id=nic1,mac=02:00:00:00:00:02"
            )),
            "{a}"
        );
        // A VM with NICs has no "-nic none" and still no user-mode NIC.
        assert!(!a.contains("-nic none") && !a.contains("user"), "{a}");
    }

    #[test]
    fn tap_names_fit_linux_interface_names_and_are_stable() {
        let t = tap_name("vm-1792357611", 7);
        assert!(t.len() <= 15 && horcrux_common::valid_ifname(&t), "{t}");
        assert_eq!(t, tap_name("vm-1792357611", 7));
        assert_ne!(tap_name("vm-1", 0), tap_name("vm-2", 0));
        assert_ne!(tap_name("vm-1", 0), tap_name("vm-1", 1));
    }

    #[test]
    fn cdrom_and_boot_order() {
        use horcrux_common::BootDevice;
        let mut v = vm(VmArchitecture::X86_64, VmDisplay::default());
        let rt = super::super::display::DisplayRuntime::plan(&v.disk_path, &v.display).unwrap();
        // Empty drive, default order disk then CD-ROM.
        let a = qemu_command(&v, &rt, true, None).1.join(" ");
        assert!(
            a.contains(
                "-drive if=none,id=cd0,media=cdrom,readonly=on -device ide-cd,drive=cd0,id=cdrom0"
            ),
            "{a}"
        );
        assert!(a.contains("-boot order=cd"), "{a}");
        v.hardware.cdrom = Some("/isos/a,b.iso".into());
        v.hardware.boot = vec![BootDevice::Cdrom, BootDevice::Disk, BootDevice::Net];
        let a = qemu_command(&v, &rt, true, None).1.join(" ");
        // Commas in a path are escaped for QEMU's option parser.
        assert!(a.contains("format=raw,file=/isos/a,,b.iso"), "{a}");
        assert!(a.contains("-boot order=dcn"), "{a}");
        // ARM virt has no IDE: the CD-ROM sits on virtio-SCSI, no -boot.
        let mut v = vm(VmArchitecture::Aarch64, VmDisplay::default());
        v.hardware.cdrom = Some("/isos/x.iso".into());
        let a = qemu_command(&v, &rt, true, None).1.join(" ");
        assert!(
            a.contains("-device virtio-scsi-pci,id=cdscsi -device scsi-cd,bus=cdscsi.0,drive=cd0,id=cdrom0"),
            "{a}"
        );
        assert!(!a.contains("-boot"), "{a}");
    }

    fn disk(path: &str, bus: &str) -> horcrux_common::VmDisk {
        horcrux_common::VmDisk {
            path: path.into(),
            size_gb: 1,
            disk_type: bus.into(),
            ..Default::default()
        }
    }

    /// A VM recorded before buses existed keeps its exact old drive (QEMU's
    /// default interface) plus a stable id, and SeaBIOS boot letters.
    #[test]
    fn legacy_disks_keep_their_old_drive_and_boot_letters() {
        let mut v = vm(VmArchitecture::X86_64, VmDisplay::default());
        v.disks = vec![disk("/vms/t1.qcow2", "default")];
        let rt = super::super::display::DisplayRuntime::plan(&v.disk_path, &v.display).unwrap();
        let a = qemu_command(&v, &rt, true, None).1.join(" ");
        assert!(
            a.contains("-drive file=/vms/t1.qcow2,format=qcow2,id=drive-disk0 "),
            "{a}"
        );
        assert!(
            a.contains("-boot order=cd") && !a.contains("bootindex"),
            "{a}"
        );
    }

    #[test]
    fn every_disk_gets_its_bus_options_and_boot_index() {
        use horcrux_common::BootDevice;
        let mut v = vm(VmArchitecture::X86_64, VmDisplay::default());
        let mut d1 = disk("/vms/t1-disk1.qcow2", "scsi");
        d1.ssd = true;
        d1.discard = true;
        d1.iothread = true;
        d1.cache = "none".into();
        let d2 = disk("/vms/data.raw", "sata");
        let d3 = disk("/vms/t1-disk3.qcow2", "ide");
        v.disks = vec![disk("/vms/t1.qcow2", "virtio"), d1, d2, d3];
        v.hardware.boot = vec![BootDevice::Cdrom, BootDevice::Disk];
        let rt = super::super::display::DisplayRuntime::plan(&v.disk_path, &v.display).unwrap();
        let a = qemu_command(&v, &rt, true, None).1.join(" ");
        for want in [
            "-drive if=none,id=drive-disk0,file=/vms/t1.qcow2,format=qcow2 -device virtio-blk-pci,drive=drive-disk0,id=disk0,bootindex=2",
            "-drive if=none,id=drive-disk1,file=/vms/t1-disk1.qcow2,format=qcow2,cache=none,discard=unmap -object iothread,id=io1 -device virtio-scsi-pci,id=scsihw1,iothread=io1 -device scsi-hd,bus=scsihw1.0,drive=drive-disk1,id=disk1,rotation_rate=1,bootindex=3",
            "-drive if=none,id=drive-disk2,file=/vms/data.raw,format=raw -device ahci,id=ahci0 -device ide-hd,bus=ahci0.0,drive=drive-disk2,id=disk2,bootindex=4",
            "-device ide-hd,drive=drive-disk3,id=disk3,bootindex=5",
            "-device ide-cd,drive=cd0,id=cdrom0,bootindex=1",
        ] {
            assert!(a.contains(want), "missing {want}\nin {a}");
        }
        assert!(!a.contains("-boot "), "bootindex replaces -boot order: {a}");
    }

    #[test]
    fn legacy_vm_reports_its_one_disk_with_size_and_path() {
        let mut v = vm(VmArchitecture::X86_64, VmDisplay::default());
        v.disk_size = 20;
        let d = v.all_disks();
        assert_eq!(d.len(), 1);
        assert_eq!(
            (d[0].path.as_str(), d[0].size_gb, d[0].disk_type.as_str()),
            ("/vms/t1.qcow2", 20, "default")
        );
        assert_eq!(v.to_config().disks, d);
        // With a disk list, disk0's path is always the VM's disk_path.
        v.disks = vec![
            disk("/elsewhere.qcow2", "virtio"),
            disk("/vms/b.raw", "sata"),
        ];
        let d = v.all_disks();
        assert_eq!(
            d.iter().map(|d| d.path.as_str()).collect::<Vec<_>>(),
            ["/vms/t1.qcow2", "/vms/b.raw"]
        );
    }

    #[test]
    fn smp_topology_only_when_the_sockets_divide_the_cpus() {
        let smp = |cpus: u32, sockets: Option<u32>| {
            let mut v = vm(VmArchitecture::X86_64, VmDisplay::default());
            v.cpus = cpus;
            v.hardware.sockets = sockets;
            let rt = super::super::display::DisplayRuntime::plan(&v.disk_path, &v.display).unwrap();
            let a = qemu_command(&v, &rt, true, None).1;
            a[a.iter().position(|x| x == "-smp").unwrap() + 1].clone()
        };
        assert_eq!(smp(4, None), "4");
        assert_eq!(smp(4, Some(1)), "4", "one socket: plain -smp");
        assert_eq!(smp(3, Some(2)), "3", "3 CPUs can't split over 2 sockets");
        assert_eq!(smp(6, Some(3)), "6,sockets=3,cores=2,threads=1");
        assert_eq!(smp(2, Some(2)), "2,sockets=2,cores=1,threads=1");
    }

    #[test]
    fn network_boot_gets_bootindexes_in_the_configured_order() {
        use horcrux_common::BootDevice;
        let mut v = vm(VmArchitecture::X86_64, VmDisplay::default());
        v.disks = vec![disk("/vms/t1.qcow2", "virtio")];
        v.hardware.nets = vec![nic("vmbr0", None), nic("vmbr1", None)];
        v.hardware.boot = vec![BootDevice::Net, BootDevice::Disk];
        let rt = super::super::display::DisplayRuntime::plan(&v.disk_path, &v.display).unwrap();
        let a = qemu_command(&v, &rt, true, None).1.join(" ");
        for want in [
            "id=nic0,bootindex=1",
            "id=nic1,bootindex=2",
            "virtio-blk-pci,drive=drive-disk0,id=disk0,bootindex=3",
        ] {
            assert!(a.contains(want), "missing {want}\nin {a}");
        }
        assert!(
            !a.contains("cdrom0,bootindex"),
            "CD-ROM not in the order: {a}"
        );
        // Without Net in the boot order the NICs get no bootindex.
        v.hardware.boot = vec![BootDevice::Disk];
        let a = qemu_command(&v, &rt, true, None).1.join(" ");
        assert!(
            !a.contains("id=nic0,bootindex") && !a.contains("id=nic1,bootindex"),
            "{a}"
        );
        assert!(a.contains("id=disk0,bootindex=1"), "{a}");
    }

    #[test]
    fn sata_disks_share_one_ahci_controller_on_consecutive_ports() {
        let mut v = vm(VmArchitecture::X86_64, VmDisplay::default());
        v.disks = vec![
            disk("/vms/t1.qcow2", "sata"),
            disk("/vms/t1-disk1.qcow2", "virtio"),
            disk("/vms/t1-disk2.qcow2", "sata"),
            disk("/vms/t1-disk3.qcow2", "sata"),
        ];
        let rt = super::super::display::DisplayRuntime::plan(&v.disk_path, &v.display).unwrap();
        let a = qemu_command(&v, &rt, true, None).1.join(" ");
        assert_eq!(a.matches("ahci,id=ahci0").count(), 1, "{a}");
        for want in [
            "ide-hd,bus=ahci0.0,drive=drive-disk0,id=disk0",
            "ide-hd,bus=ahci0.1,drive=drive-disk2,id=disk2",
            "ide-hd,bus=ahci0.2,drive=drive-disk3,id=disk3",
        ] {
            assert!(a.contains(want), "missing {want}\nin {a}");
        }
    }

    #[test]
    fn cpu_type_topology_q35_ovmf_and_tpm() {
        use horcrux_common::{Firmware, MachineType};
        let mut v = vm(VmArchitecture::X86_64, VmDisplay::default());
        v.cpus = 4;
        v.hardware.cpu_type = Some("x86-64-v2-AES".into());
        v.hardware.sockets = Some(2);
        v.hardware.machine = MachineType::Q35;
        v.hardware.firmware = Firmware::Ovmf;
        v.hardware.tpm = true;
        let rt = super::super::display::DisplayRuntime::plan(&v.disk_path, &v.display).unwrap();
        let ovmf = crate::vm::firmware::Ovmf {
            code: "/usr/share/OVMF/OVMF_CODE_4M.fd".into(),
            vars_template: "/usr/share/OVMF/OVMF_VARS_4M.fd".into(),
        };
        let a = qemu_command(&v, &rt, true, Some(&ovmf)).1.join(" ");
        for want in [
            "-machine q35,accel=kvm:tcg -cpu x86-64-v2-AES -m 256 -smp 4,sockets=2,cores=2,threads=1",
            "-drive if=pflash,unit=0,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.fd",
            "-drive if=pflash,unit=1,id=drive-efivars,format=qcow2,file=/vms/t1.efivars.qcow2",
            "-chardev socket,id=chrtpm,path=/vms/t1.tpm.sock -tpmdev emulator,id=tpm0,chardev=chrtpm -device tpm-tis,tpmdev=tpm0",
        ] {
            assert!(a.contains(want), "missing {want}\nin {a}");
        }
        // Defaults: i440FX stays QEMU's plain default machine, no -cpu on
        // x86, plain -smp, no firmware/TPM arguments.
        let v = vm(VmArchitecture::X86_64, VmDisplay::default());
        let a = qemu_command(&v, &rt, true, Some(&ovmf)).1.join(" ");
        assert!(
            a.starts_with("-machine accel=kvm:tcg -m 256 -smp 2 "),
            "{a}"
        );
        assert!(
            !a.contains("pflash") && !a.contains("tpm") && !a.contains("-cpu"),
            "{a}"
        );
    }

    #[test]
    fn disk_sets_refuse_shared_files_and_full_buses() {
        let ok = vec![disk("/a", "virtio"), disk("/b", "sata")];
        assert!(validate_disk_set(&ok).is_ok());
        let dup = vec![disk("/a", "virtio"), disk("/a", "scsi")];
        assert!(validate_disk_set(&dup).unwrap_err().contains("already"));
        let sata: Vec<_> = (0..7).map(|i| disk(&format!("/s{i}"), "sata")).collect();
        assert!(validate_disk_set(&sata).is_err());
        let ide: Vec<_> = (0..4).map(|i| disk(&format!("/i{i}"), "ide")).collect();
        assert!(validate_disk_set(&ide).unwrap_err().contains("IDE"));
        // Exactly full buses are fine: 6 SATA ports, 3 IDE units (one of
        // them may be the legacy default-bus disk) next to the CD-ROM.
        let sata6: Vec<_> = (0..6).map(|i| disk(&format!("/s{i}"), "sata")).collect();
        assert_eq!(validate_disk_set(&sata6), Ok(()));
        let ide3 = vec![
            disk("/i0", "default"),
            disk("/i1", "ide"),
            disk("/i2", "ide"),
        ];
        assert_eq!(validate_disk_set(&ide3), Ok(()));
        let ide4 = [ide3.clone(), vec![disk("/i3", "default")]].concat();
        assert!(validate_disk_set(&ide4)
            .unwrap_err()
            .contains("4 IDE disks"));
    }

    #[tokio::test]
    async fn disk_image_of_an_exact_byte_size() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("mirror.qcow2");
        let bytes = 3 * 1024 * 1024 + 64 * 1024;
        QemuManager::create_disk_image_bytes(&p, bytes)
            .await
            .unwrap();
        assert_eq!(QemuManager::virtual_size(&p).await.unwrap(), bytes);
        let err = QemuManager::virtual_size(&dir.path().join("absent.qcow2"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("absent.qcow2"), "{err}");
        // qemu-img's failure is reported, naming the image.
        let bad = dir.path().join("no/such/dir/x.qcow2");
        let err = QemuManager::create_disk_image_bytes(&bad, bytes)
            .await
            .unwrap_err();
        assert!(
            matches!(&err, horcrux_common::Error::System(m) if m.contains(&*bad.to_string_lossy())),
            "{err}"
        );
        assert!(!bad.exists());
    }

    #[test]
    fn planned_disks_get_server_chosen_paths() {
        let m = QemuManager::with_storage_path("/srv/vms".into());
        let mut cfg = vm(VmArchitecture::X86_64, VmDisplay::default()).to_config();
        cfg.id = "vm-9".into();
        cfg.disk_size = 12;
        cfg.disks = vec![];
        let d = m.planned_disks(&cfg).unwrap();
        assert_eq!(d.len(), 1);
        assert_eq!(
            (d[0].path.as_str(), d[0].size_gb, d[0].disk_type.as_str()),
            ("/srv/vms/vm-9.qcow2", 12, "virtio")
        );
        let mut extra = disk("/etc/passwd", "scsi");
        extra.size_gb = 4;
        cfg.disks = vec![disk("", "sata"), extra];
        cfg.disks[0].size_gb = 0;
        let d = m.planned_disks(&cfg).unwrap();
        assert_eq!(d[0].path, "/srv/vms/vm-9.qcow2");
        assert_eq!(d[0].size_gb, 12, "size 0 on disk0 = disk_size");
        assert_eq!(
            d[1].path, "/srv/vms/vm-9-disk1.qcow2",
            "request paths are ignored"
        );
        cfg.disks[1].size_gb = 0;
        assert!(m.planned_disks(&cfg).is_err());
        // Up to MAX_VM_DISKS disks; one more is refused.
        let max = horcrux_common::MAX_VM_DISKS;
        cfg.disks = (0..max).map(|_| disk("", "virtio")).collect();
        assert_eq!(m.planned_disks(&cfg).unwrap().len(), max);
        cfg.disks.push(disk("", "virtio"));
        let err = m.planned_disks(&cfg).unwrap_err();
        assert!(
            err.to_string().contains(&format!("at most {max} disks")),
            "{err}"
        );
    }

    #[test]
    fn incoming_start_waits_paused_for_the_state() {
        let v = vm(VmArchitecture::X86_64, VmDisplay::default());
        let rt = super::super::display::DisplayRuntime::plan(&v.disk_path, &v.display).unwrap();
        let (_, args) = qemu_command(&v, &rt, true, None);
        let args = with_incoming(args);
        assert!(args
            .join(" ")
            .ends_with("-S -incoming defer -daemonize -pidfile /var/run/horcrux-vm-t1.pid"));
    }

    #[test]
    fn foreign_arch_uses_tcg_and_its_machine() {
        let v = vm(VmArchitecture::Aarch64, VmDisplay::default());
        let rt = super::super::display::DisplayRuntime::plan(&v.disk_path, &v.display).unwrap();
        let (bin, args) = qemu_command(&v, &rt, true, None);
        assert_eq!(bin, "qemu-system-aarch64");
        assert!(args
            .join(" ")
            .starts_with("-machine virt,accel=tcg -cpu max"));
    }

    // Host-side helpers and lifecycle guards: each test below targets
    // surviving mutants of the vm-runtime cargo-mutants group.

    fn temp_vm(dir: &std::path::Path, id: &str) -> QemuVm {
        let mut v = vm(VmArchitecture::X86_64, VmDisplay::default());
        v.id = format!("hxtest-{id}-{}", std::process::id());
        v.name = v.id.clone();
        v.disk_path = dir.join(format!("{id}.qcow2"));
        v
    }

    #[tokio::test]
    async fn run_ip_fails_on_a_failing_or_missing_program() {
        run_ip("true", &[]).await.expect("exit status 0 is success");
        let err = run_ip("false", &["link", "show"]).await.unwrap_err();
        assert!(err.to_string().contains("false link show"), "{err}");
        let err = run_ip("/nonexistent/hx-ip", &[]).await.unwrap_err();
        assert!(err.to_string().contains("/nonexistent/hx-ip"), "{err}");
    }

    /// A host with a fake `class/net` tree and stand-ins for `ip` and
    /// `bridge` that log each call to `<root>/calls` and make the device
    /// appear / disappear on `tuntap add` / `link del`, the way the kernel
    /// does. A call whose arguments equal the content of `<root>/fail`
    /// fails, like a real `ip` that hits an error.
    struct FakeNet {
        root: tempfile::TempDir,
        host: HostNet,
    }

    impl FakeNet {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let r = root.path();
            std::fs::create_dir(r.join("net")).unwrap();
            let script = r#"d=$(dirname "$0")
echo "$(basename "$0") $*" >> "$d/calls"
if [ -f "$d/fail" ] && [ "$*" = "$(cat "$d/fail")" ]; then echo "RTNETLINK answers: busy" >&2; exit 2; fi
case "$*" in
  "tuntap add dev "*" mode tap") mkdir "$d/net/$4" ;;
  "link del "*) rmdir "$d/net/$3" ;;
esac
"#;
            for tool in ["ip", "bridge"] {
                std::fs::write(r.join(tool), script).unwrap();
            }
            let sh = |tool: &str| vec!["sh".to_string(), r.join(tool).to_string_lossy().into()];
            let host = HostNet {
                sys_class_net: r.join("net"),
                ip: sh("ip"),
                bridge: sh("bridge"),
            };
            FakeNet { root, host }
        }

        fn net(&self) -> PathBuf {
            self.root.path().join("net")
        }

        fn add_bridge(&self, name: &str, vlan_filtering: Option<&str>) {
            std::fs::create_dir_all(self.net().join(name).join("bridge")).unwrap();
            if let Some(f) = vlan_filtering {
                std::fs::write(self.net().join(name).join("bridge/vlan_filtering"), f).unwrap();
            }
        }

        fn add_dev(&self, name: &str) {
            std::fs::create_dir_all(self.net().join(name)).unwrap();
        }

        fn has_dev(&self, name: &str) -> bool {
            self.net().join(name).exists()
        }

        fn fail_on(&self, args: &str) {
            std::fs::write(self.root.path().join("fail"), args).unwrap();
        }

        fn calls(&self) -> Vec<String> {
            std::fs::read_to_string(self.root.path().join("calls"))
                .unwrap_or_default()
                .lines()
                .map(str::to_string)
                .collect()
        }
    }

    fn nic(bridge: &str, vlan: Option<u16>) -> horcrux_common::VmNic {
        horcrux_common::VmNic {
            model: Default::default(),
            bridge: bridge.into(),
            mac: None,
            vlan,
        }
    }

    #[test]
    fn bridge_vlan_aware_reads_bridge_and_vlan_filtering_from_sysfs() {
        let f = FakeNet::new();
        f.add_bridge("vmbr0", Some("1\n"));
        f.add_bridge("vmbr1", Some("0\n"));
        f.add_bridge("vmbr2", None);
        f.add_dev("eth0");
        // A bridge one level up, reachable only through a path in the name.
        std::fs::create_dir_all(f.root.path().join("evil/bridge")).unwrap();
        std::fs::write(f.root.path().join("evil/bridge/vlan_filtering"), "1").unwrap();

        let h = &f.host;
        assert_eq!(h.bridge_vlan_aware("vmbr0"), Some(true));
        assert_eq!(h.bridge_vlan_aware("vmbr1"), Some(false));
        assert_eq!(h.bridge_vlan_aware("vmbr2"), Some(false));
        assert_eq!(h.bridge_vlan_aware("eth0"), None, "not a bridge");
        assert_eq!(h.bridge_vlan_aware("vmbr9"), None, "no such device");
        assert_eq!(h.bridge_vlan_aware("../evil"), None, "not an ifname");
        // The real sysfs: loopback exists in every netns and is no bridge.
        assert_eq!(HostNet::system().bridge_vlan_aware("lo"), None);
    }

    #[tokio::test]
    async fn setup_taps_refuses_a_bridge_missing_on_this_host() {
        let dir = tempfile::tempdir().unwrap();
        let mut v = temp_vm(dir.path(), "nobr");
        v.hardware.nets = vec![nic("hxnobr0", None)];
        // The real host (no such bridge in any CI netns) and a fake one.
        let f = FakeNet::new();
        for host in [HostNet::system(), f.host.clone()] {
            let err = host.setup_taps(&v).await.unwrap_err();
            assert!(
                matches!(&err, horcrux_common::Error::Validation(m)
                    if m.contains("net0") && m.contains("hxnobr0") && m.contains("does not exist")),
                "{err}"
            );
        }
        assert!(f.calls().is_empty(), "{:?}", f.calls());
    }

    #[tokio::test]
    async fn setup_taps_plugs_each_nic_into_its_bridge_with_its_vlan() {
        let dir = tempfile::tempdir().unwrap();
        let f = FakeNet::new();
        f.add_bridge("vmbr0", Some("0"));
        f.add_bridge("vmbr1", Some("1"));
        let mut v = temp_vm(dir.path(), "taps");
        // An untagged NIC on a plain bridge, a tagged one on a VLAN-aware bridge.
        v.hardware.nets = vec![nic("vmbr0", None), nic("vmbr1", Some(42))];
        let (t0, t1) = (tap_name(&v.id, 0), tap_name(&v.id, 1));
        // A tap left over from a QEMU that died: replaced by a fresh one.
        f.add_dev(&t1);

        f.host.setup_taps(&v).await.expect("both NICs set up");
        assert_eq!(
            f.calls(),
            [
                format!("ip tuntap add dev {t0} mode tap"),
                format!("ip link set {t0} master vmbr0"),
                format!("ip link set {t0} up"),
                format!("ip link del {t1}"),
                format!("ip tuntap add dev {t1} mode tap"),
                format!("ip link set {t1} master vmbr1"),
                format!("bridge vlan del dev {t1} vid 1"),
                format!("bridge vlan add dev {t1} vid 42 pvid untagged"),
                format!("ip link set {t1} up"),
            ]
        );
        assert!(f.has_dev(&t0) && f.has_dev(&t1));
    }

    #[tokio::test]
    async fn setup_taps_refuses_a_vlan_tag_on_a_bridge_without_vlan_filtering() {
        let dir = tempfile::tempdir().unwrap();
        let f = FakeNet::new();
        f.add_bridge("vmbr0", Some("1"));
        f.add_bridge("vmbr1", None);
        let mut v = temp_vm(dir.path(), "vlan");
        v.hardware.nets = vec![nic("vmbr0", None), nic("vmbr1", Some(7))];
        let err = f.host.setup_taps(&v).await.unwrap_err();
        assert!(
            matches!(&err, horcrux_common::Error::Validation(m)
                if m.contains("net1") && m.contains("vmbr1") && m.contains("not VLAN-aware")),
            "{err}"
        );
        // net0 had been made already: it is removed again.
        let t0 = tap_name(&v.id, 0);
        assert_eq!(f.calls().last(), Some(&format!("ip link del {t0}")));
        assert!(!f.has_dev(&t0));
    }

    #[tokio::test]
    async fn setup_taps_reports_a_failing_ip_and_removes_the_taps_made() {
        let dir = tempfile::tempdir().unwrap();
        let f = FakeNet::new();
        f.add_bridge("vmbr0", Some("1"));
        let mut v = temp_vm(dir.path(), "fail");
        v.hardware.nets = vec![nic("vmbr0", None), nic("vmbr0", Some(5))];
        let (t0, t1) = (tap_name(&v.id, 0), tap_name(&v.id, 1));
        f.fail_on(&format!("vlan add dev {t1} vid 5 pvid untagged"));
        let err = f.host.setup_taps(&v).await.unwrap_err();
        assert!(
            matches!(&err, horcrux_common::Error::System(m)
                if m.contains("vlan add") && m.contains("RTNETLINK answers: busy")),
            "{err}"
        );
        let calls = f.calls();
        assert!(
            !calls.contains(&format!("ip link set {t1} up")),
            "{calls:?}"
        );
        assert!(
            calls.ends_with(&[format!("ip link del {t0}"), format!("ip link del {t1}")]),
            "{calls:?}"
        );
        assert!(!f.has_dev(&t0) && !f.has_dev(&t1));
    }

    #[tokio::test]
    async fn remove_taps_deletes_every_tap_of_the_vm_and_nothing_else() {
        let f = FakeNet::new();
        let id = "hxtest-rm";
        // Taps of NICs since removed from the config are found too.
        f.add_dev(&tap_name(id, 0));
        f.add_dev(&tap_name(id, 5));
        f.add_dev(&tap_name(id, horcrux_common::MAX_VM_NICS - 1));
        f.add_dev(&tap_name("hxtest-other", 0));
        // A tap that can't be deleted doesn't stop the others' removal.
        f.fail_on(&format!("link del {}", tap_name(id, 0)));
        f.host.remove_taps(id).await;
        assert_eq!(
            f.calls(),
            [0, 5, horcrux_common::MAX_VM_NICS - 1]
                .map(|i| format!("ip link del {}", tap_name(id, i)))
        );
        assert!(f.has_dev(&tap_name(id, 0)), "the failed delete");
        assert!(!f.has_dev(&tap_name(id, 5)));
        assert!(!f.has_dev(&tap_name(id, horcrux_common::MAX_VM_NICS - 1)));
        assert!(f.has_dev(&tap_name("hxtest-other", 0)), "another VM's tap");
    }

    #[tokio::test]
    async fn cleanup_after_exit_removes_the_console_sockets_and_state() {
        let dir = tempfile::tempdir().unwrap();
        let v = temp_vm(dir.path(), "exit");
        let d = &v.disk_path;
        let files = [
            super::super::display::DisplayRuntime::state_path(d),
            super::super::display::DisplayRuntime::serial_socket(d),
            super::super::display::DisplayRuntime::qmp_socket(d),
        ];
        for p in &files {
            std::fs::write(p, b"x").unwrap();
        }
        std::fs::write(d, b"disk").unwrap();
        cleanup_after_exit(&v).await;
        for p in &files {
            assert!(!p.exists(), "{} left behind", p.display());
        }
        assert!(d.exists(), "the disk itself stays");
    }

    #[tokio::test]
    async fn start_vm_refuses_missing_media_before_launching_anything() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = QemuManager::new();
        let mut v = temp_vm(dir.path(), "start");
        let iso = dir.path().join("gone.iso").to_string_lossy().to_string();
        v.hardware.cdrom = Some(iso.clone());
        let err = mgr.start_vm(&v).await.unwrap_err();
        assert!(
            matches!(&err, horcrux_common::Error::Validation(m) if m.contains("ISO") && m.contains(&iso)),
            "{err}"
        );
        v.hardware.cdrom = None;
        let err = mgr.start_vm(&v).await.unwrap_err();
        assert!(
            matches!(&err, horcrux_common::Error::Validation(m) if m.contains("disk0") && m.contains("missing")),
            "{err}"
        );
        // The receiving side of a migration checks the same.
        let err = mgr.start_vm_incoming(&v).await.unwrap_err();
        assert!(
            matches!(&err, horcrux_common::Error::Validation(m) if m.contains("disk0") && m.contains("missing")),
            "{err}"
        );
        v.status = VmStatus::Running;
        let err = mgr.start_vm(&v).await.unwrap_err();
        assert!(err.to_string().contains("already running"), "{err}");
        let err = mgr.start_vm_incoming(&v).await.unwrap_err();
        assert!(err.to_string().contains("already running"), "{err}");
    }

    #[tokio::test]
    async fn stop_vm_without_a_pidfile_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = QemuManager::new();
        let mut v = temp_vm(dir.path(), "stop");
        let err = mgr.stop_vm(&v).await.unwrap_err();
        assert!(err.to_string().contains("already stopped"), "{err}");
        v.status = VmStatus::Running;
        assert!(!std::path::Path::new(&pidfile(&v.id)).exists());
        let err = mgr.stop_vm(&v).await.unwrap_err();
        assert!(
            matches!(&err, horcrux_common::Error::System(m) if m.contains("pidfile")),
            "{err}"
        );
    }

    #[test]
    fn only_x86_guests_set_to_ovmf_use_uefi() {
        use horcrux_common::Firmware;
        let mut v = vm(VmArchitecture::X86_64, VmDisplay::default());
        assert!(!uses_ovmf(&v), "BIOS by default");
        v.hardware.firmware = Firmware::Ovmf;
        assert!(uses_ovmf(&v));
        for arch in [VmArchitecture::Aarch64, VmArchitecture::Riscv64] {
            let mut a = vm(arch, VmDisplay::default());
            assert!(!uses_ovmf(&a));
            a.hardware.firmware = Firmware::Ovmf;
            assert!(!uses_ovmf(&a), "virt machines bring their own firmware");
        }
    }

    #[tokio::test]
    async fn stop_vm_waits_for_qemu_to_exit() {
        // A "QEMU" that takes a moment to shut down after SIGTERM.
        let mut child = std::process::Command::new("sh")
            .args([
                "-c",
                "trap 'sleep 0.4; exit 0' TERM; while true; do sleep 0.05; done",
            ])
            .spawn()
            .unwrap();
        let pid = child.id();
        // Let sh install its trap before the signal arrives.
        std::thread::sleep(std::time::Duration::from_millis(200));
        let dir = tempfile::tempdir().unwrap();
        let mut v = temp_vm(dir.path(), "wait");
        v.status = VmStatus::Running;
        let pf = pidfile(&v.id);
        if let Err(e) = std::fs::write(&pf, pid.to_string()) {
            // /var/run is root's; CI runs the tests as root.
            let _ = child.kill();
            let _ = child.wait();
            assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied, "{e}");
            eprintln!("skipped: {pf} not writable");
            return;
        }
        let t = std::time::Instant::now();
        let r = QemuManager::new().stop_vm(&v).await;
        let waited = t.elapsed();
        let gone = !pid_alive(pid);
        let _ = child.kill();
        let _ = child.wait();
        r.unwrap();
        assert!(gone, "stop_vm returned while QEMU was still running");
        assert!(
            waited < std::time::Duration::from_secs(4),
            "stop_vm kept waiting after QEMU exited: {waited:?}"
        );
        assert!(!std::path::Path::new(&pf).exists(), "pidfile removed");
    }

    #[tokio::test]
    async fn delete_vm_tolerates_a_gone_disk_but_not_an_undeletable_one() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = QemuManager::new();
        // Disk already gone: the side files are still cleaned, and it's Ok.
        let v = temp_vm(dir.path(), "gone");
        for ext in ["serial.log", "cidata.iso", "hold.json"] {
            std::fs::write(v.disk_path.with_extension(ext), b"x").unwrap();
        }
        mgr.delete_vm(&v).await.expect("a missing disk is fine");
        for ext in ["serial.log", "cidata.iso", "hold.json"] {
            assert!(
                !v.disk_path.with_extension(ext).exists(),
                "{ext} left behind"
            );
        }
        // A disk path that can't be unlinked (a directory) is reported.
        let v = temp_vm(dir.path(), "dir");
        std::fs::create_dir(&v.disk_path).unwrap();
        let err = mgr.delete_vm(&v).await.unwrap_err();
        assert!(
            err.to_string().contains("Failed to delete disk image"),
            "{err}"
        );
        // Running VMs are never deleted.
        let mut v = temp_vm(dir.path(), "run");
        std::fs::write(&v.disk_path, b"x").unwrap();
        v.status = VmStatus::Running;
        assert!(mgr.delete_vm(&v).await.is_err());
        assert!(v.disk_path.exists());
    }

    #[tokio::test]
    async fn backing_chain_is_followed_nearest_first() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let img = |args: &[&str]| {
            let out = std::process::Command::new("qemu-img")
                .args(args)
                .current_dir(d)
                .output()
                .expect("qemu-img (qemu-utils) is installed on the CI runners");
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        img(&["create", "-q", "-f", "qcow2", "base.qcow2", "1M"]);
        img(&[
            "create",
            "-q",
            "-f",
            "qcow2",
            "-b",
            "base.qcow2",
            "-F",
            "qcow2",
            "mid.qcow2",
        ]);
        img(&[
            "create",
            "-q",
            "-f",
            "qcow2",
            "-b",
            "mid.qcow2",
            "-F",
            "qcow2",
            "top.qcow2",
        ]);

        let chain = [d.join("mid.qcow2"), d.join("base.qcow2")];
        assert_eq!(
            QemuManager::backing_files(&d.join("top.qcow2")).await,
            chain
        );
        assert!(QemuManager::backing_files(&d.join("base.qcow2"))
            .await
            .is_empty());
        assert!(QemuManager::backing_files(&d.join("absent.qcow2"))
            .await
            .is_empty());
        // A missing backing file is still listed, and ends the chain.
        std::fs::remove_file(d.join("base.qcow2")).unwrap();
        assert_eq!(
            QemuManager::backing_files(&d.join("top.qcow2")).await,
            chain
        );
    }
}
