//! Common types and utilities shared between horcrux-api and horcrux-ui

pub mod auth;
pub mod console;
pub mod lxc;
pub mod remote_play;
pub mod security;

pub use lxc::{
    LxcCatalogEntry, LxcFeatures, LxcMount, LxcNet, LxcOptions, LxcSnapshot, LxcTemplate,
};

use serde::{Deserialize, Serialize};

/// Virtual machine hypervisor type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VmHypervisor {
    Qemu,  // QEMU/KVM
    Lxd,   // LXD (can do VMs)
    Incus, // Incus (LXD fork)
}

/// CPU architecture for VMs
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VmArchitecture {
    #[serde(rename = "x86_64")]
    X86_64, // amd64
    #[serde(rename = "aarch64")]
    Aarch64, // arm64
    #[serde(rename = "riscv64")]
    Riscv64, // risc-v 64-bit
    #[serde(rename = "ppc64le")]
    Ppc64le, // powerpc 64-bit little-endian
}

impl Default for VmArchitecture {
    fn default() -> Self {
        // Default to host architecture
        match std::env::consts::ARCH {
            "x86_64" => VmArchitecture::X86_64,
            "aarch64" => VmArchitecture::Aarch64,
            "riscv64" => VmArchitecture::Riscv64,
            "powerpc64" => VmArchitecture::Ppc64le,
            _ => VmArchitecture::X86_64,
        }
    }
}

/// Virtual machine disk configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct VmDisk {
    pub path: String,
    pub size_gb: u64,
    /// Bus the guest sees the disk on: "virtio" (virtio-blk), "scsi"
    /// (virtio-scsi), "sata" (AHCI), "ide", or "default" = QEMU's default
    /// interface for the machine (IDE on x86 PC): what every disk got
    /// before buses were configurable, kept for those VMs so their guests
    /// still find their disks.
    pub disk_type: String,
    /// Host cache mode: "none", "writethrough", "writeback", "directsync",
    /// "unsafe". Empty = QEMU's default (writeback).
    pub cache: String,
    /// Pass guest TRIM/UNMAP through to the image (frees space in qcow2).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub discard: bool,
    /// Report the disk to the guest as an SSD (non-rotational); not on virtio-blk.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ssd: bool,
    /// Give the disk its own I/O thread (virtio / virtio-scsi only).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub iothread: bool,
    /// Storage pool (id) the image lives in; `None` = the node's VM
    /// storage directory. Chosen at create / add-disk time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<String>,
    /// Image format QEMU opens the disk with ("qcow2" or "raw"). Set by
    /// the server when it places a disk on a block pool (an LVM volume is
    /// raw, a volume-chain LVM volume holds qcow2); never probed from the
    /// content. `None` = from the file name (`.qcow2` -> qcow2, else raw).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

impl VmDisk {
    /// Whether the disk is a block device a storage pool provisioned (an
    /// LVM volume under `/dev`), not an image file Horcrux created.
    pub fn is_block_volume(&self) -> bool {
        self.storage.is_some() && is_block_volume_path(&self.path)
    }
}

/// A path under `/dev/` (a pool's block volume such as `/dev/<vg>/<lv>`).
pub fn is_block_volume_path(path: &str) -> bool {
    path.starts_with("/dev/") && !path.contains("/../")
}

static VM_STATE_DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// Set once at start-up to the node's VM storage directory
/// (`paths.vm_storage`): side files of VMs whose first disk is a block
/// volume live under its `blk/` subdirectory.
pub fn set_vm_state_dir(dir: std::path::PathBuf) {
    let _ = VM_STATE_DIR.set(dir);
}

fn vm_state_dir() -> std::path::PathBuf {
    VM_STATE_DIR.get().cloned().unwrap_or_else(|| {
        std::env::var("HORCRUX_VM_STORAGE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                std::path::PathBuf::from(
                    std::env::var("HORCRUX_DATA_DIR").unwrap_or_else(|_| "/var/lib/horcrux".into()),
                )
                .join("vms")
            })
    })
}

/// The path a VM's side files hang off (`<stem>.qmp.sock`, `.serial.sock`,
/// `.display.json`, `.efivars.qcow2`, `.tpm`, ...): the first disk's own
/// path for an image file. A block volume (`/dev/<vg>/<lv>`) must never get
/// files next to it in /dev, so its stem is
/// `<VM storage dir>/blk/<vg>--<lv>.disk` on the node.
pub fn runtime_stem(disk: &std::path::Path) -> std::path::PathBuf {
    match disk.to_str() {
        Some(s) if is_block_volume_path(s) => vm_state_dir().join("blk").join(format!(
            "{}.disk",
            s.trim_start_matches("/dev/").replace('/', "--")
        )),
        _ => disk.to_path_buf(),
    }
}

#[cfg(test)]
mod runtime_stem_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn image_files_keep_their_path_block_volumes_map_into_the_vm_dir() {
        assert_eq!(
            runtime_stem(Path::new("/var/lib/horcrux/vms/100.qcow2")),
            Path::new("/var/lib/horcrux/vms/100.qcow2")
        );
        let s = runtime_stem(Path::new("/dev/vg_san/vm-100-disk-0"));
        assert!(s.ends_with("blk/vg_san--vm-100-disk-0.disk"), "{s:?}");
        assert!(!s.starts_with("/dev"));
        assert_eq!(
            s.with_extension("qmp.sock").file_name().unwrap(),
            "vg_san--vm-100-disk-0.qmp.sock"
        );
        assert!(!is_block_volume_path("/dev/../etc/x"));
        let d = VmDisk {
            path: "/dev/vg/lv".into(),
            storage: Some("p".into()),
            ..Default::default()
        };
        assert!(d.is_block_volume());
        let f = VmDisk {
            path: "/dev/vg/lv".into(),
            ..Default::default()
        };
        assert!(!f.is_block_volume());
    }
}

/// Disk buses a VM disk can use (`VmDisk::disk_type`).
pub const DISK_BUSES: [&str; 5] = ["virtio", "scsi", "sata", "ide", "default"];

/// Host cache modes a VM disk can use (`VmDisk::cache`, "" = default).
pub const DISK_CACHE_MODES: [&str; 6] = [
    "",
    "none",
    "writethrough",
    "writeback",
    "directsync",
    "unsafe",
];

/// Most disks one VM can have.
pub const MAX_VM_DISKS: usize = 16;

impl VmDisk {
    /// Check bus/cache/option combinations QEMU would refuse or ignore.
    pub fn validate(&self, index: usize) -> std::result::Result<(), String> {
        if !DISK_BUSES.contains(&self.disk_type.as_str()) {
            return Err(format!(
                "disk{index}: bus must be one of {}",
                DISK_BUSES.join(", ")
            ));
        }
        if !DISK_CACHE_MODES.contains(&self.cache.as_str()) {
            return Err(format!(
                "disk{index}: cache must be one of none, writethrough, writeback, directsync, unsafe"
            ));
        }
        if self.ssd && matches!(self.disk_type.as_str(), "virtio" | "default") {
            return Err(format!(
                "disk{index}: SSD emulation needs the scsi, sata or ide bus (virtio-blk has no rotation rate)"
            ));
        }
        if self.iothread && !matches!(self.disk_type.as_str(), "virtio" | "scsi") {
            return Err(format!(
                "disk{index}: an I/O thread needs the virtio or scsi bus"
            ));
        }
        if self.discard && self.disk_type == "default" {
            return Err(format!(
                "disk{index}: pick a bus to use discard (the legacy default bus has no options)"
            ));
        }
        Ok(())
    }
}

/// Virtual machine configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmConfig {
    pub id: String,
    pub name: String,
    pub hypervisor: VmHypervisor,
    pub memory: u64, // Memory in MB
    pub cpus: u32,
    pub disk_size: u64, // Disk size in GB (legacy, kept for compatibility)
    pub status: VmStatus,
    #[serde(default)]
    pub architecture: VmArchitecture, // CPU architecture
    #[serde(default)]
    pub disks: Vec<VmDisk>, // Disk configurations
    /// Graphical console, video card and sound (defaults: VNC, std VGA, no sound).
    #[serde(default)]
    pub display: VmDisplay,
    /// Network cards, CD-ROM (ISO) and boot order (QEMU VMs).
    #[serde(default)]
    pub hardware: VmHardware,
}

/// Emulated network card model.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum NicModel {
    /// Paravirtual (best performance; Linux guests, Windows with virtio drivers).
    #[default]
    Virtio,
    /// Intel e1000 (works without drivers in almost every OS installer).
    E1000,
    /// Intel 82574 e1000e (the model Windows 10/11 and virt-manager guests are installed with; keeps their drivers and licence/BitLocker hardware binding).
    E1000e,
    /// Realtek RTL8139 (very old guests).
    Rtl8139,
    /// VMware vmxnet3 (guests imported from ESXi).
    Vmxnet3,
}

impl NicModel {
    /// QEMU `-device` driver name.
    pub fn qemu_device(&self) -> &'static str {
        match self {
            NicModel::Virtio => "virtio-net-pci",
            NicModel::E1000 => "e1000",
            NicModel::E1000e => "e1000e",
            NicModel::Rtl8139 => "rtl8139",
            NicModel::Vmxnet3 => "vmxnet3",
        }
    }
}

/// How a VM network card reaches the network.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum NicAttach {
    /// A tap device plugged into a host Linux bridge (`VmNic::bridge`).
    #[default]
    Bridge,
    /// A macvtap device (bridge mode) directly on a host network interface
    /// (`VmNic::bridge` names that interface, e.g. the wired uplink). The
    /// guest gets its own MAC on that LAN without turning the host's uplink
    /// into a bridge, so the host's own address and connections are never
    /// touched. As with every macvtap setup, the host itself and the guest
    /// can't talk to each other directly (other machines and other macvtap
    /// guests on the same interface can). Wired interfaces only: Wi-Fi
    /// access points drop frames from a second MAC.
    Macvtap,
}

impl NicAttach {
    pub fn is_bridge(&self) -> bool {
        *self == NicAttach::Bridge
    }
}

/// One VM network card: a tap device on a host Linux bridge, or a macvtap
/// device directly on a host interface (see `attach`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VmNic {
    #[serde(default)]
    pub model: NicModel,
    /// Host bridge the NIC's tap device is attached to (e.g. vmbr0, br0);
    /// with `attach: macvtap` the host interface the macvtap device sits on
    /// (e.g. eth0).
    pub bridge: String,
    /// Bridge (default) or macvtap on a host interface.
    #[serde(default, skip_serializing_if = "NicAttach::is_bridge")]
    pub attach: NicAttach,
    /// MAC address; generated (locally administered) and stored when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mac: Option<String>,
    /// VLAN tag (the tap's PVID; the bridge must be VLAN-aware).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vlan: Option<u16>,
    /// Bandwidth cap in Mbit/s, applied to the tap in both directions
    /// (`tc`) when the VM starts; unset = unlimited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_mbps: Option<u32>,
}

/// Highest NIC rate limit accepted (Mbit/s).
pub const MAX_NIC_RATE_MBPS: u32 = 400_000;

/// A boot device class, in `VmHardware::boot` order.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BootDevice {
    Disk,
    Cdrom,
    Net,
}

/// Per-VM hardware beyond CPU/memory/disk/display.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct VmHardware {
    /// Network cards (net0, net1, ...). Empty = no network at all.
    #[serde(default)]
    pub nets: Vec<VmNic>,
    /// ISO image in the CD-ROM drive (a file from the ISO library); None =
    /// drive empty. Can be changed while the VM runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cdrom: Option<String>,
    /// More CD-ROM drives with ISOs from the ISO library (x86 only; e.g. the
    /// virtio-win driver ISO next to a Windows installer). Not bootable;
    /// changed with the VM stopped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_cdroms: Vec<String>,
    /// Boot order; empty = disk, then CD-ROM.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub boot: Vec<BootDevice>,
    /// QEMU CPU model (`host`, `max`, `qemu64`, `x86-64-v2-AES`,
    /// `EPYC-v4`, ...). None = QEMU's default for the machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_type: Option<String>,
    /// CPU sockets the VM's CPUs are split over (CPUs = sockets x cores;
    /// must divide the CPU count). None = 1 socket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sockets: Option<u32>,
    /// Chipset of x86 VMs.
    #[serde(default, skip_serializing_if = "MachineType::is_default")]
    pub machine: MachineType,
    /// Firmware of x86 VMs: SeaBIOS (legacy BIOS) or OVMF (UEFI, with a
    /// per-VM EFI variable store next to the first disk).
    #[serde(default, skip_serializing_if = "Firmware::is_default")]
    pub firmware: Firmware,
    /// UEFI Secure Boot: the Secure Boot build of OVMF with the vendor keys
    /// enrolled in the VM's variable store (needs `firmware: ovmf`). The
    /// guest's measured-boot state (BitLocker/PCR7) depends on it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub secure_boot: bool,
    /// Emulated TPM 2.0 chip (swtpm; its state lives next to the first disk).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tpm: bool,
    /// TPM interface model. None = a VM from before the setting existed
    /// (TIS, what it always ran). New VMs get CRB (what libvirt and
    /// virt-manager use for Windows 11); an import keeps the source's model.
    /// Changing it keeps the TPM state: only the device interface differs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tpm_model: Option<TpmModel>,
    /// The VM's TPM has been in use, so its state (keys, BitLocker/Windows
    /// Hello/Entra sealing) MUST exist when the VM starts. A start without
    /// the state is refused instead of silently creating an empty TPM
    /// (`POST /api/vms/:id/tpm/reset` is the explicit way to start over).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tpm_state_expected: bool,
    /// Memory ballooning (virtio-balloon): the host can take memory back
    /// from the running guest and give it back (`POST /api/vms/:id/balloon`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub balloon: bool,
    /// QEMU guest agent channel (virtio-serial socket on the host); the
    /// agent itself runs in the guest (`qemu-guest-agent`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub guest_agent: bool,
    /// An xHCI USB controller, so host USB devices can be passed through and
    /// hot-plugged. Implied by a non-empty `usb` list.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub usb_controller: bool,
    /// Host USB devices handed to the guest (by vendor:product id).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub usb: Vec<UsbDevice>,
    /// Host PCI devices (GPUs, NICs, NVMe, ...) and mediated devices
    /// (NVIDIA vGPU, Intel GVT-g) handed to the guest through vfio-pci.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hostpci: Vec<VmHostPci>,
    /// Serial ports, virtiofs shares, shared memory, SMBIOS, VM Generation
    /// ID, NUMA and CPU/memory hotplug limits, protection, lock, hookscript.
    #[serde(default, skip_serializing_if = "VmAdvanced::is_default")]
    pub adv: VmAdvanced,
}

/// Advanced per-VM settings (see `VmHardware::adv`). Every field is off by
/// default and absent from the stored config until used.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct VmAdvanced {
    /// Extra serial ports: a Unix socket on the host, or a host tty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub serial: Vec<VmSerialPort>,
    /// Host directories shared into the guest with virtiofs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub virtiofs: Vec<VmVirtiofs>,
    /// Inter-VM shared memory (ivshmem) backed by `/dev/shm`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ivshmem: Option<VmIvshmem>,
    /// SMBIOS type 1 (system) strings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smbios: Option<VmSmbios>,
    /// VM Generation ID GUID (`auto` generates one when saved).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vmgenid: Option<String>,
    /// One NUMA node per CPU socket, memory split evenly.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub numa: bool,
    /// Upper bound for CPU hotplug (`-smp maxcpus`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cpus: Option<u32>,
    /// Upper bound for memory hotplug in MB (`-m maxmem`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_memory_mb: Option<u64>,
    /// Refuse deleting the VM or removing its disks.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub protection: bool,
    /// `manual`: refuse start/stop/delete/config changes until unlocked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock: Option<String>,
    /// Executable run around the VM's lifecycle: `<vmid> pre-start|post-start|pre-stop|post-stop`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hookscript: Option<String>,
    /// Pin the vCPU threads (and QEMU's own threads) to host CPUs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_pinning: Option<VmCpuPinning>,
    /// Back guest memory with huge pages (reserved at start, given back at stop).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hugepages: Option<VmHugePages>,
    /// Hyper-V enlightenments for Windows guests (the list is the node
    /// setting `passthrough.hyperv_enlightenments`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hyperv: bool,
    /// Host keyboards/mice handed to the guest through evdev (`input-linux`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evdev: Vec<VmEvdev>,
    /// Expose the host CPU's virtualisation extensions (VT-x/AMD-V) to the
    /// guest, so it can run its own hypervisor: Windows VBS / Memory
    /// Integrity (HVCI), Hyper-V, WSL2. Needs nested KVM on the host.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub nested_virt: bool,
    /// Steam Remote Play / Moonlight readiness (network reach, discovery
    /// relay, port forwards, Wake-on-LAN, guest logon settings);
    /// docs/design/gaming-vm.md section 16. None = off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_play: Option<remote_play::VmRemotePlay>,
}

/// Most evdev input devices one VM can grab.
pub const MAX_VM_EVDEV: usize = 4;

/// CPU pinning of a VM (`VmAdvanced::cpu_pinning`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct VmCpuPinning {
    /// Host CPUs for the vCPU threads: a cpu list (`0-7`, `2,4,6-9`) used
    /// one host CPU per vCPU in order, or `auto` (on a hybrid CPU the
    /// performance cores, otherwise the highest-numbered CPUs; CPU 0 is left
    /// to the host when there are enough CPUs).
    pub vcpus: String,
    /// Host CPUs for QEMU's own threads (I/O, emulator); None or `auto` =
    /// the host CPUs not used by the vCPUs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emulator: Option<String>,
}

/// Huge-page backing (`VmAdvanced::hugepages`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VmHugePages {
    /// Page size in KiB: 2048 (2 MiB) or 1048576 (1 GiB).
    #[serde(default = "default_hugepage_kb")]
    pub size_kb: u64,
}

fn default_hugepage_kb() -> u64 {
    2048
}

impl Default for VmHugePages {
    fn default() -> Self {
        Self {
            size_kb: default_hugepage_kb(),
        }
    }
}

/// A host input device handed to the guest (`VmAdvanced::evdev`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct VmEvdev {
    /// `/dev/input/by-id/...-event-kbd` (stable across reboots) or `/dev/input/eventN`.
    pub path: String,
    /// Keyboards: also grab the other evdev devices of this VM, so the
    /// toggle (both Ctrl keys) switches every input between host and guest.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub grab_all: bool,
}

/// Parse a Linux cpu list (`0-3,8,10-11`) into sorted, unique CPU numbers.
pub fn parse_cpu_list(s: &str) -> std::result::Result<Vec<u32>, String> {
    let mut out: Vec<u32> = Vec::new();
    let s = s.trim();
    if s.is_empty() {
        return Err("empty cpu list".into());
    }
    for part in s.split(',') {
        let part = part.trim();
        let num = |x: &str| {
            x.trim()
                .parse::<u32>()
                .map_err(|_| format!("bad cpu list {s:?} (expected e.g. 0-7,10)"))
        };
        match part.split_once('-') {
            Some((a, b)) => {
                let (a, b) = (num(a)?, num(b)?);
                if a > b || b - a > 4096 {
                    return Err(format!("bad cpu range {part:?}"));
                }
                out.extend(a..=b);
            }
            None => out.push(num(part)?),
        }
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// Format CPU numbers as a compact cpu list (`0-3,8`).
pub fn format_cpu_list(cpus: &[u32]) -> String {
    let mut v: Vec<u32> = cpus.to_vec();
    v.sort_unstable();
    v.dedup();
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    while i < v.len() {
        let start = v[i];
        let mut end = start;
        while i + 1 < v.len() && v[i + 1] == end + 1 {
            i += 1;
            end = v[i];
        }
        parts.push(if start == end {
            start.to_string()
        } else {
            format!("{start}-{end}")
        });
        i += 1;
    }
    parts.join(",")
}

impl VmAdvanced {
    pub fn is_default(&self) -> bool {
        *self == VmAdvanced::default()
    }
}

/// An extra serial port of a VM.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct VmSerialPort {
    /// Host tty to pass through (`/dev/ttyUSB0`); None = a Unix socket
    /// `<disk>.serialN.sock` on the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
}

/// A host directory shared with the guest (`mount -t virtiofs <tag> /mnt`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VmVirtiofs {
    pub tag: String,
    pub path: String,
}

/// Shared memory device; VMs using the same `name` see the same memory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VmIvshmem {
    pub name: String,
    /// Power of two, 1-1024.
    pub size_mb: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct VmSmbios {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    /// System UUID (keeps a guest's identity when it is moved from another
    /// hypervisor: licences, management agents, machine-bound keys).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
}

pub const MAX_VM_SERIAL: usize = 4;
pub const MAX_VM_VIRTIOFS: usize = 4;

fn safe_path(what: &str, p: &str) -> std::result::Result<(), String> {
    let ok = p.starts_with('/')
        && !p.split('/').any(|seg| seg == "..")
        && p.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'.' | b'-' | b'+'));
    if ok {
        Ok(())
    } else {
        Err(format!(
            "{what} {p:?}: an absolute path of letters, digits and / _ . - + (no ..)"
        ))
    }
}

impl VmAdvanced {
    /// Everything that doesn't need the host. `cpus`/`sockets` are the VM's.
    pub fn validate(
        &self,
        arch: &VmArchitecture,
        cpus: u32,
        sockets: Option<u32>,
    ) -> std::result::Result<(), String> {
        let x86 = *arch == VmArchitecture::X86_64;
        if self.serial.len() > MAX_VM_SERIAL {
            return Err(format!("at most {MAX_VM_SERIAL} extra serial ports"));
        }
        let mut hosts: Vec<&String> = Vec::new();
        for p in &self.serial {
            if let Some(d) = &p.device {
                safe_path("serial device", d)?;
                if !d.starts_with("/dev/tty") {
                    return Err(format!("serial device {d:?} must be a /dev/tty* device"));
                }
                if hosts.contains(&d) {
                    return Err(format!("serial device {d} is listed twice"));
                }
                hosts.push(d);
            }
        }
        if self.virtiofs.len() > MAX_VM_VIRTIOFS {
            return Err(format!("at most {MAX_VM_VIRTIOFS} virtiofs shares"));
        }
        let mut tags: Vec<&String> = Vec::new();
        for v in &self.virtiofs {
            let ok = !v.tag.is_empty()
                && v.tag.len() <= 36
                && v.tag
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'));
            if !ok {
                return Err(format!(
                    "virtiofs tag {:?}: 1-36 letters, digits, _ and -",
                    v.tag
                ));
            }
            if tags.contains(&&v.tag) {
                return Err(format!("virtiofs tag {} is used twice", v.tag));
            }
            tags.push(&v.tag);
            safe_path("virtiofs path", &v.path)?;
            if v.path == "/" {
                return Err("sharing / with a guest is not allowed".into());
            }
        }
        if !self.virtiofs.is_empty() && !x86 {
            return Err("virtiofs is offered for x86_64 guests".into());
        }
        if let Some(p) = &self.cpu_pinning {
            let want = u64::from(self.max_cpus.unwrap_or(cpus).max(cpus));
            if p.vcpus.trim() != "auto" {
                let list =
                    parse_cpu_list(&p.vcpus).map_err(|e| format!("cpu_pinning.vcpus: {e}"))?;
                if (list.len() as u64) < want {
                    return Err(format!(
                        "cpu_pinning.vcpus lists {} host CPUs for {want} vCPUs: give one host CPU per vCPU",
                        list.len()
                    ));
                }
            }
            if let Some(e) = &p.emulator {
                if e.trim() != "auto" {
                    parse_cpu_list(e).map_err(|e| format!("cpu_pinning.emulator: {e}"))?;
                }
            }
        }
        if let Some(h) = &self.hugepages {
            if !matches!(h.size_kb, 2048 | 1048576) {
                return Err("hugepages.size_kb must be 2048 (2 MiB) or 1048576 (1 GiB)".into());
            }
            if !x86 {
                return Err("huge pages are offered for x86_64 guests".into());
            }
        }
        if self.hyperv && !x86 {
            return Err("Hyper-V enlightenments are for x86_64 guests".into());
        }
        if self.nested_virt && !x86 {
            return Err("nested virtualisation is offered for x86_64 guests".into());
        }
        if let Some(rp) = &self.remote_play {
            rp.validate()?;
        }
        if self.evdev.len() > MAX_VM_EVDEV {
            return Err(format!("at most {MAX_VM_EVDEV} evdev input devices"));
        }
        let mut evs: Vec<&String> = Vec::new();
        for e in &self.evdev {
            safe_path("evdev device", &e.path)?;
            if !e.path.starts_with("/dev/input/") {
                return Err(format!(
                    "evdev device {:?} must be under /dev/input/",
                    e.path
                ));
            }
            if evs.contains(&&e.path) {
                return Err(format!("evdev device {} is listed twice", e.path));
            }
            evs.push(&e.path);
        }
        if let Some(i) = &self.ivshmem {
            let ok = !i.name.is_empty()
                && i.name.len() <= 32
                && i.name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
            if !ok {
                return Err("ivshmem name: 1-32 lower-case letters, digits and -".into());
            }
            if !(1..=1024).contains(&i.size_mb) || !i.size_mb.is_power_of_two() {
                return Err("ivshmem size must be a power of two, 1-1024 MB".into());
            }
        }
        if let Some(sm) = &self.smbios {
            for (what, v) in [
                ("manufacturer", &sm.manufacturer),
                ("product", &sm.product),
                ("version", &sm.version),
                ("serial", &sm.serial),
                ("family", &sm.family),
            ] {
                if let Some(v) = v {
                    if v.len() > 64 || !v.bytes().all(|b| (0x20..0x7f).contains(&b)) {
                        return Err(format!(
                            "SMBIOS {what}: at most 64 printable ASCII characters"
                        ));
                    }
                }
            }
            if sm.uuid.as_deref().is_some_and(|u| !valid_guid(u)) {
                return Err("SMBIOS UUID must be a UUID (8-4-4-4-12 hex digits)".into());
            }
        }
        if let Some(g) = &self.vmgenid {
            if g != "auto" && !valid_guid(g) {
                return Err("VM Generation ID must be a GUID (or auto)".into());
            }
            if !x86 {
                return Err("VM Generation ID is offered for x86_64 guests".into());
            }
        }
        if self.numa && !x86 {
            return Err("NUMA is offered for x86_64 guests".into());
        }
        if let Some(m) = self.max_cpus {
            if !x86 {
                return Err("CPU hotplug is offered for x86_64 guests".into());
            }
            if m < cpus || m > 512 {
                return Err(format!("max CPUs must be {cpus}-512"));
            }
            if let Some(s) = sockets {
                if !m.is_multiple_of(s) {
                    return Err(format!(
                        "max CPUs ({m}) must divide evenly over {s} sockets"
                    ));
                }
            }
        }
        if self.max_memory_mb.is_some() && !x86 {
            return Err("memory hotplug is offered for x86_64 guests".into());
        }
        if let Some(l) = &self.lock {
            if l != "manual" {
                return Err("lock must be \"manual\"".into());
            }
        }
        if let Some(h) = &self.hookscript {
            safe_path("hookscript", h)?;
        }
        Ok(())
    }
}

/// `8-4-4-4-12` hex digits.
pub fn valid_guid(g: &str) -> bool {
    let parts: Vec<&str> = g.split('-').collect();
    parts.len() == 5
        && parts
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(p, n)| p.len() == n && p.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Most extra CD-ROM drives (`VmHardware::extra_cdroms`) one VM can have.
pub const MAX_VM_EXTRA_CDROMS: usize = 3;

/// Most host PCI / mediated devices one VM can get.
pub const MAX_VM_HOSTPCI: usize = 8;

/// A host PCI device or mediated device passed through to a VM.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct VmHostPci {
    /// A PCI address (`0000:01:00.0`, the domain may be left out) of a
    /// device bound to `vfio-pci`, or the UUID of a mediated device
    /// (`/api/gpu/mdev/devices`).
    pub device: String,
    /// The guest's primary VGA adapter (`x-vga=on`); at most one per VM.
    #[serde(default)]
    pub primary_gpu: bool,
    /// Horcrux moves the device (and the other functions of its IOMMU
    /// group, e.g. a GPU's HDMI audio) to `vfio-pci` when the VM starts and
    /// gives it back to its host driver when the VM stops. Off = the device
    /// must already be bound to `vfio-pci`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub managed: bool,
    /// Managed GPU only: allow the hand-off even though a monitor is
    /// connected to this GPU and no other GPU drives one (single-GPU
    /// passthrough: the host loses its local display while the VM runs).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub allow_display_loss: bool,
}

/// What a [`VmHostPci::device`] string names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostPciKind {
    /// `DDDD:BB:DD.F`, lower case.
    Pci(String),
    /// A mediated device UUID, lower case.
    Mdev(String),
}

impl VmHostPci {
    /// Parse the device string: a PCI address (domain defaults to 0000) or a
    /// UUID. Nothing else is accepted, so the value is safe to put into a
    /// QEMU `-device` argument and a sysfs path.
    pub fn kind(&self) -> std::result::Result<HostPciKind, String> {
        let d = self.device.trim().to_ascii_lowercase();
        let hex = |s: &str, n: usize| s.len() == n && s.chars().all(|c| c.is_ascii_hexdigit());
        // UUID 8-4-4-4-12
        let parts: Vec<&str> = d.split('-').collect();
        if parts.len() == 5
            && [8usize, 4, 4, 4, 12]
                .iter()
                .zip(&parts)
                .all(|(n, p)| hex(p, *n))
        {
            return Ok(HostPciKind::Mdev(d));
        }
        // [DDDD:]BB:DD.F
        let (domain, rest) = match d.matches(':').count() {
            2 => d.split_once(':').unwrap(),
            1 => ("0000", d.as_str()),
            _ => ("", ""),
        };
        if let Some((bus_dev, func)) = rest.rsplit_once('.') {
            if let Some((bus, dev)) = bus_dev.split_once(':') {
                if hex(domain, 4)
                    && hex(bus, 2)
                    && hex(dev, 2)
                    && func.len() == 1
                    && func.chars().all(|c| ('0'..='7').contains(&c))
                {
                    return Ok(HostPciKind::Pci(format!("{domain}:{bus}:{dev}.{func}")));
                }
            }
        }
        Err(format!(
            "hostpci device {:?} is neither a PCI address (0000:01:00.0) nor a mediated device UUID",
            self.device
        ))
    }
}

/// A host USB device passed through to a VM.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsbDevice {
    /// USB vendor id, 4 hex digits (`046d`)
    pub vendor_id: String,
    /// USB product id, 4 hex digits (`c52b`)
    pub product_id: String,
}

impl UsbDevice {
    /// Lower-case, 4-digit ids or an error naming the bad one.
    pub fn normalized(&self) -> std::result::Result<UsbDevice, String> {
        let norm = |what: &str, v: &str| -> std::result::Result<String, String> {
            let v = v.trim().trim_start_matches("0x").to_ascii_lowercase();
            if v.len() == 4 && v.chars().all(|c| c.is_ascii_hexdigit()) {
                Ok(v)
            } else {
                Err(format!(
                    "USB {what} id {v:?} must be 4 hex digits (e.g. 046d)"
                ))
            }
        };
        Ok(UsbDevice {
            vendor_id: norm("vendor", &self.vendor_id)?,
            product_id: norm("product", &self.product_id)?,
        })
    }
}

/// At most this many passed-through USB devices per VM.
pub const MAX_VM_USB: usize = 8;

/// x86 chipset (`-machine`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum MachineType {
    /// i440FX + PIIX (QEMU's `pc`): the default, what every OS knows.
    #[default]
    I440fx,
    /// Q35 + ICH9 (QEMU's `q35`): PCIe, AHCI; preferred for UEFI/Windows 11.
    Q35,
}

impl MachineType {
    pub fn is_default(&self) -> bool {
        *self == MachineType::default()
    }
    /// QEMU machine type name.
    pub fn qemu_name(&self) -> &'static str {
        match self {
            MachineType::I440fx => "pc",
            MachineType::Q35 => "q35",
        }
    }
}

/// Interface model of the emulated TPM.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum TpmModel {
    /// `tpm-tis` (the legacy default of older VMs).
    #[default]
    Tis,
    /// `tpm-crb` (libvirt/virt-manager's choice for Windows 11).
    Crb,
}

impl TpmModel {
    /// QEMU device name on x86 (aarch64 only has `tpm-tis-device`).
    pub fn qemu_device(self, x86: bool) -> &'static str {
        match (self, x86) {
            (_, false) => "tpm-tis-device",
            (TpmModel::Tis, true) => "tpm-tis",
            (TpmModel::Crb, true) => "tpm-crb",
        }
    }
}

impl VmHardware {
    /// The TPM interface this VM runs (legacy VMs: TIS).
    pub fn tpm_model(&self) -> TpmModel {
        self.tpm_model.unwrap_or_default()
    }
}

/// VM firmware.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Firmware {
    /// SeaBIOS: legacy BIOS boot.
    #[default]
    Seabios,
    /// OVMF (EDK2): UEFI boot.
    Ovmf,
}

impl Firmware {
    pub fn is_default(&self) -> bool {
        *self == Firmware::default()
    }
}

/// Whether `s` is a plausible QEMU CPU model name (`-cpu` value without
/// flags): letters, digits, `-`, `_`, `.`. QEMU itself checks the model exists.
pub fn valid_cpu_type(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Host tap device name prefix of a VM's network cards: `tap<8 hex>i`
/// (FNV-1a of the VM id; Linux interface names are limited to 15 characters
/// and VM ids are longer). Stable across restarts and builds, so the VM
/// firewall can match all of a VM's NICs with `iifname "<prefix>*"`.
pub fn vm_tap_prefix(vm_id: &str) -> String {
    let mut h: u32 = 0x811c9dc5;
    for b in vm_id.as_bytes() {
        h ^= *b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    format!("tap{h:08x}i")
}

fn fnv32(id: &str) -> u32 {
    let mut h: u32 = 0x811c9dc5;
    for b in id.as_bytes() {
        h ^= *b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    h
}

/// Host-side veth name prefix of a container's network interfaces:
/// `vc<8 hex>i` (FNV-1a of the container id, like the VM taps). Horcrux
/// gives every container NIC a stable host name (LXC `veth.pair`,
/// LXD/Incus `host_name`, Docker/Podman renamed at start), so the
/// container firewall can match exactly that container's traffic with
/// `iifname/oifname "<prefix>*"`.
pub fn ct_veth_prefix(ct_id: &str) -> String {
    format!("vc{:08x}i", fnv32(ct_id))
}

/// Host-side veth of a container's `index`-th interface (`vc<8 hex>i<n>`).
pub fn ct_veth_name(ct_id: &str, index: usize) -> String {
    format!("{}{index}", ct_veth_prefix(ct_id))
}

/// Host tap device of a VM's `index`-th network card (`tap<8 hex>i<n>`).
pub fn vm_tap_name(vm_id: &str, index: usize) -> String {
    format!("{}{index}", vm_tap_prefix(vm_id))
}

/// Most network cards one VM can have.
pub const MAX_VM_NICS: usize = 8;

/// Whether `name` is a valid Linux interface name (1-15 chars, no '/',
/// whitespace or ':').
pub fn valid_ifname(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 15
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Normalize a MAC address to lower-case `aa:bb:cc:dd:ee:ff`, refusing
/// malformed and multicast addresses (a guest NIC needs a unicast MAC).
pub fn normalize_mac(mac: &str) -> std::result::Result<String, String> {
    let parts: Vec<&str> = mac.trim().split([':', '-']).collect();
    if parts.len() != 6 || parts.iter().any(|p| p.len() != 2) {
        return Err(format!("invalid MAC address {mac:?}"));
    }
    let mut bytes = [0u8; 6];
    for (b, p) in bytes.iter_mut().zip(&parts) {
        *b = u8::from_str_radix(p, 16).map_err(|_| format!("invalid MAC address {mac:?}"))?;
    }
    if bytes[0] & 1 == 1 {
        return Err(format!("{mac} is a multicast address"));
    }
    if bytes == [0; 6] {
        return Err("the all-zero MAC address can't be used".into());
    }
    Ok(bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":"))
}

impl VmHardware {
    /// The hardware a clone gets: the same cards, ISO and boot order, but
    /// no MAC addresses (new ones are generated, two guests on one network
    /// with the same MAC break each other's connectivity).
    pub fn for_clone(&self) -> VmHardware {
        let mut hw = self.clone();
        for n in &mut hw.nets {
            n.mac = None;
        }
        // A physical device can only belong to one guest.
        hw.usb.clear();
        hw.hostpci.clear();
        // A host tty belongs to one guest; the clone is a different machine
        // (new Generation ID, not locked/protected, no shared memory ties).
        hw.adv.serial.retain(|p| p.device.is_none());
        if hw.adv.vmgenid.is_some() {
            hw.adv.vmgenid = Some("auto".into());
        }
        hw.adv.protection = false;
        hw.adv.lock = None;
        hw
    }

    /// Check everything that doesn't need the host (bridges and ISO files
    /// are checked by the server).
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.nets.len() > MAX_VM_NICS {
            return Err(format!("a VM can have at most {MAX_VM_NICS} network cards"));
        }
        let mut macs = Vec::new();
        for (i, n) in self.nets.iter().enumerate() {
            if !valid_ifname(&n.bridge) {
                return Err(format!("net{i}: invalid bridge name {:?}", n.bridge));
            }
            if let Some(v) = n.vlan {
                if !(1..=4094).contains(&v) {
                    return Err(format!("net{i}: VLAN tag must be 1-4094"));
                }
                if n.attach == NicAttach::Macvtap {
                    return Err(format!(
                        "net{i}: a VLAN tag needs a VLAN-aware bridge; a macvtap card takes \
                         the VLAN interface itself (e.g. eth0.20) as its host interface"
                    ));
                }
            }
            if let Some(m) = &n.mac {
                let m = normalize_mac(m).map_err(|e| format!("net{i}: {e}"))?;
                if macs.contains(&m) {
                    return Err(format!("net{i}: MAC {m} is used twice"));
                }
                macs.push(m);
            }
        }
        for (i, d) in self.boot.iter().enumerate() {
            if self.boot[..i].contains(d) {
                return Err(format!("boot device {d:?} is listed twice"));
            }
        }
        if let Some(iso) = &self.cdrom {
            if iso.trim().is_empty() {
                return Err("cdrom: empty ISO path (omit it to leave the drive empty)".into());
            }
        }
        if self.extra_cdroms.len() > MAX_VM_EXTRA_CDROMS {
            return Err(format!(
                "a VM can have at most {MAX_VM_EXTRA_CDROMS} extra CD-ROM drives"
            ));
        }
        for (i, iso) in self.extra_cdroms.iter().enumerate() {
            if iso.trim().is_empty() {
                return Err(format!("extra_cdroms[{i}]: empty ISO path"));
            }
            if self.extra_cdroms[..i].contains(iso) {
                return Err(format!("extra_cdroms: {iso} is listed twice"));
            }
        }
        if let Some(c) = &self.cpu_type {
            if !valid_cpu_type(c) {
                return Err(format!(
                    "cpu type {c:?}: letters, digits, - _ . only (e.g. host, max, x86-64-v2-AES)"
                ));
            }
        }
        if let Some(s) = self.sockets {
            if !(1..=16).contains(&s) {
                return Err("sockets must be 1-16".into());
            }
        }
        if self.usb.len() > MAX_VM_USB {
            return Err(format!("a VM can have at most {MAX_VM_USB} USB devices"));
        }
        let mut seen: Vec<UsbDevice> = Vec::new();
        for d in &self.usb {
            let n = d.normalized()?;
            if seen.contains(&n) {
                return Err(format!(
                    "USB device {}:{} is listed twice",
                    n.vendor_id, n.product_id
                ));
            }
            seen.push(n);
        }
        if self.hostpci.len() > MAX_VM_HOSTPCI {
            return Err(format!(
                "a VM can have at most {MAX_VM_HOSTPCI} host PCI devices"
            ));
        }
        let mut kinds: Vec<HostPciKind> = Vec::new();
        for d in &self.hostpci {
            let k = d.kind()?;
            if kinds.contains(&k) {
                return Err(format!("host PCI device {} is listed twice", d.device));
            }
            kinds.push(k);
        }
        if self.hostpci.iter().filter(|d| d.primary_gpu).count() > 1 {
            return Err("only one host PCI device can be the primary GPU".into());
        }
        for d in &self.hostpci {
            if (d.managed || d.allow_display_loss) && !matches!(d.kind()?, HostPciKind::Pci(_)) {
                return Err(format!(
                    "{}: managed hand-off is for PCI devices, not mediated devices",
                    d.device
                ));
            }
            if d.allow_display_loss && !d.managed {
                return Err(format!(
                    "{}: allow_display_loss needs managed: true",
                    d.device
                ));
            }
        }
        Ok(())
    }

    /// Checks that need the VM's architecture and CPU count.
    pub fn validate_for(
        &self,
        arch: &VmArchitecture,
        cpus: u32,
    ) -> std::result::Result<(), String> {
        self.validate()?;
        if self.secure_boot && self.firmware != Firmware::Ovmf {
            return Err("Secure Boot needs UEFI (OVMF) firmware".into());
        }
        if let Some(s) = self.sockets {
            // with CPU hotplug the topology covers the maximum
            let total = self.adv.max_cpus.unwrap_or(cpus);
            if !total.is_multiple_of(s) {
                return Err(format!(
                    "{total} CPUs can't be split evenly over {s} sockets"
                ));
            }
        }
        self.adv.validate(arch, cpus, self.sockets)?;
        if !self.extra_cdroms.is_empty() {
            if *arch != VmArchitecture::X86_64 {
                return Err("extra CD-ROM drives are offered for x86_64 VMs".into());
            }
            // i440FX has four IDE places: the CD-ROM, one extra drive and a
            // generated seed/answer-file disc; Q35's AHCI has six ports.
            if self.machine == MachineType::I440fx && self.extra_cdroms.len() > 1 {
                return Err(
                    "the i440FX chipset has room for one extra CD-ROM drive; use the q35 chipset for more"
                        .into(),
                );
            }
        }
        if *arch != VmArchitecture::X86_64 {
            if self.tpm_model() != TpmModel::Tis {
                return Err(
                    "the CRB TPM interface is for x86_64 VMs (this architecture only has TIS)"
                        .into(),
                );
            }
            if !self.machine.is_default() {
                return Err("the q35 chipset is for x86_64 VMs".into());
            }
            if !self.firmware.is_default() {
                return Err("OVMF (UEFI) firmware is offered for x86_64 VMs".into());
            }
        }
        Ok(())
    }
}

/// Remote display protocol a VM's graphical console is served over.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum DisplayProtocol {
    /// Headless: serial console only.
    None,
    /// VNC on 127.0.0.1, reached through the ticketed noVNC WebSocket proxy.
    #[default]
    Vnc,
    /// SPICE (password per VM start), with audio, clipboard agent and USB redirection.
    Spice,
}

/// Emulated video card.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum VideoDevice {
    #[default]
    Std,
    Virtio,
    /// Paravirtual card for SPICE (x86 only).
    Qxl,
    None,
}

/// Emulated sound card. With SPICE the audio travels over SPICE's playback
/// channels; with VNC or no display the card is still present in the guest
/// (audio is not carried by Horcrux's console, but RDP/SPICE/other remote
/// access inside the guest can use it).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SoundDevice {
    #[default]
    None,
    IntelHda,
    Ich9IntelHda,
    Ac97,
}

/// Per-VM display settings.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct VmDisplay {
    #[serde(default)]
    pub protocol: DisplayProtocol,
    #[serde(default)]
    pub video: VideoDevice,
    #[serde(default)]
    pub sound: SoundDevice,
    /// x86 VMs with a graphical console get a USB tablet (absolute pointer:
    /// the mouse in the web console is exactly where the guest draws it).
    /// `true` leaves only the relative PS/2 mouse, for old guests without
    /// USB HID support.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub relative_mouse: bool,
}

impl VmDisplay {
    /// Reject combinations QEMU can't do (or that would silently do nothing).
    pub fn validate(&self, arch: &VmArchitecture) -> std::result::Result<(), String> {
        if self.video == VideoDevice::Qxl && *arch != VmArchitecture::X86_64 {
            return Err("The qxl video card is only available on x86_64 guests".into());
        }
        if self.sound != SoundDevice::None && *arch != VmArchitecture::X86_64 {
            return Err("Emulated sound cards are only offered for x86_64 guests".into());
        }
        if self.protocol != DisplayProtocol::None && self.video == VideoDevice::None {
            return Err("A graphical console needs a video card (or set protocol to none)".into());
        }
        Ok(())
    }
}

/// Virtual machine status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VmStatus {
    Running,
    Stopped,
    Paused,
    Unknown,
}

impl std::fmt::Display for VmStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Running => write!(f, "running"),
            Self::Stopped => write!(f, "stopped"),
            Self::Paused => write!(f, "paused"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// Container runtime type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ContainerRuntime {
    Lxc,    // LXC
    Lxd,    // LXD (can do containers)
    Incus,  // Incus (LXD fork, can do containers)
    Docker, // Docker (secondary OCI runtime)
    #[default]
    Podman, // Podman (preferred OCI runtime; daemonless, rootful or rootless)
}

/// OCI-runtime (Docker/Podman) creation options: what a user fills in on
/// the "Create Container" form or what a compose service declares. Every
/// field is optional and omitted from JSON when empty, so older clients
/// that only send id/name/runtime/rootfs keep working unchanged.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContainerOptions {
    /// `KEY=value` environment entries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<String>,
    /// Port publications, `[host_ip:]host_port:container_port[/proto]`
    /// or a bare `container_port`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<String>,
    /// Bind mounts / named volumes, `source:target[:ro|rw]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volumes: Vec<String>,
    /// Command (argv) overriding the image's default CMD.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    /// Network to attach the container to (default: the runtime's bridge).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    /// DNS aliases on `network` (compose service names).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub network_aliases: Vec<String>,
}

impl ContainerOptions {
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    /// Reject malformed entries before they reach a runtime CLI, so a bad
    /// form value is a 400 with a clear message instead of a docker error.
    pub fn validate(&self) -> std::result::Result<(), String> {
        for e in &self.env {
            // `KEY=value`, or a bare `KEY` (value taken from the host, as
            // with `docker run -e KEY` and compose's `KEY:` with no value).
            let key = e.split_once('=').map(|(k, _)| k).unwrap_or(e);
            if key.is_empty() || key.contains(char::is_whitespace) {
                return Err(format!(
                    "invalid environment entry '{e}' (expected KEY=value)"
                ));
            }
        }
        for p in &self.ports {
            let (spec, proto) = match p.rsplit_once('/') {
                Some((s, pr)) => (s, Some(pr)),
                None => (p.as_str(), None),
            };
            if let Some(pr) = proto {
                if !matches!(pr, "tcp" | "udp" | "sctp") {
                    return Err(format!("invalid port protocol in '{p}'"));
                }
            }
            let parts: Vec<&str> = spec.split(':').collect();
            let numeric = |s: &str| s.parse::<u16>().map(|n| n > 0).unwrap_or(false);
            let ok = match parts.as_slice() {
                [c] => numeric(c),
                [h, c] => numeric(h) && numeric(c),
                [ip, h, c] => !ip.is_empty() && numeric(h) && numeric(c),
                _ => false,
            };
            if !ok {
                return Err(format!(
                    "invalid port mapping '{p}' (expected [ip:]host:container[/tcp|udp])"
                ));
            }
        }
        for v in &self.volumes {
            let parts: Vec<&str> = v.split(':').collect();
            let ok = match parts.as_slice() {
                [s, t] => !s.is_empty() && t.starts_with('/'),
                [s, t, m] => !s.is_empty() && t.starts_with('/') && matches!(*m, "ro" | "rw"),
                _ => false,
            };
            if !ok || v.starts_with('-') {
                return Err(format!(
                    "invalid volume '{v}' (expected source:/target[:ro|rw])"
                ));
            }
        }
        if let Some(n) = &self.network {
            if n.is_empty() || n.starts_with('-') {
                return Err(format!("invalid network name '{n}'"));
            }
        }
        Ok(())
    }
}

/// Container configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContainerConfig {
    pub id: String,
    pub name: String,
    pub runtime: ContainerRuntime,
    pub memory: u64, // Memory in MB
    pub cpus: u32,
    pub rootfs: String, // Path to rootfs or image name for Docker
    pub status: ContainerStatus,
    /// Docker/Podman creation options (env, ports, volumes, command, network).
    #[serde(flatten, default)]
    pub options: ContainerOptions,
    /// RFC 3339 creation time, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    /// LXC settings (network, mounts, idmap, features, limits); only valid
    /// with the `lxc` runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lxc: Option<LxcOptions>,
}

/// Container status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ContainerStatus {
    Running,
    Stopped,
    Paused,
    #[default]
    Unknown,
}

impl std::fmt::Display for ContainerStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Running => write!(f, "running"),
            Self::Stopped => write!(f, "stopped"),
            Self::Paused => write!(f, "paused"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// API error types
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Virtual machine not found: {0}")]
    VmNotFound(String),

    #[error("Container not found: {0}")]
    ContainerNotFound(String),

    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("System error: {0}")]
    System(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Authentication failed")]
    AuthenticationFailed,

    #[error("Authentication error: {0}")]
    Authentication(String),

    #[error("Conflict: {0}")]
    Conflict(String),

    #[error("Invalid session")]
    InvalidSession,

    // NAS-related errors
    #[error("Resource not found: {0}")]
    NotFound(String),

    #[error("Resource already exists: {0}")]
    AlreadyExists(String),

    #[error("Internal error: {0}")]
    Internal(String),

    /// The feature works, but this node lacks something it needs (a kernel
    /// facility, a delegated cgroup controller, a tool). The API answers 424
    /// with the stable `reason`, what is missing (`message`) and what to do
    /// (`hint`).
    #[error("{message} {hint}")]
    NodeLacks {
        reason: &'static str,
        message: String,
        hint: String,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

/// Cluster status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterStatus {
    pub name: String,
    pub quorum: bool,
    pub nodes: Vec<ClusterNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterNode {
    pub id: String,
    pub hostname: String,
    pub ip_address: String,
    pub status: NodeStatus,
    pub online: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NodeStatus {
    Online,
    Offline,
    Unknown,
}

/// Storage types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoragePool {
    pub id: String,
    pub name: String,
    pub pool_type: StorageType,
    pub path: String,
    pub size_gb: u64,
    pub used_gb: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StorageType {
    Lvm,
    Zfs,
    Directory,
    Nfs,
    Ceph,
}

/// Backup types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupInfo {
    pub id: String,
    pub vm_id: String,
    pub backup_type: BackupType,
    pub size_bytes: u64,
    pub created_at: i64,
    pub compression: CompressionType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BackupType {
    Full,
    Incremental,
    Differential,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompressionType {
    None,
    Gzip,
    Zstd,
    Lz4,
}

/// Monitoring types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeMetrics {
    pub cpu_usage: f64,
    pub memory_total: u64,
    pub memory_used: u64,
    pub disk_total: u64,
    pub disk_used: u64,
    pub network_rx_bytes: u64,
    pub network_tx_bytes: u64,
    pub load_average: f64,
    pub uptime_seconds: u64,
}

/// Alert types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertRule {
    pub name: String,
    pub metric: String,
    pub condition: AlertCondition,
    pub threshold: f64,
    pub severity: AlertSeverity,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AlertCondition {
    GreaterThan,
    LessThan,
    Equals,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AlertSeverity {
    Critical,
    Warning,
    Info,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub id: String,
    pub rule_name: String,
    pub target: String,
    pub severity: String,
    pub message: String,
    pub status: String,
    pub fired_at: i64,
}

/// Firewall types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirewallRule {
    pub name: String,
    pub action: FirewallAction,
    pub protocol: FirewallProtocol,
    pub source: String,
    pub destination: String,
    pub port: Option<u16>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FirewallAction {
    Accept,
    Drop,
    Reject,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FirewallProtocol {
    Tcp,
    Udp,
    Icmp,
    Any,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_pci_addresses_and_mdev_uuids_are_validated() {
        let h = |d: &str| VmHostPci {
            device: d.into(),
            primary_gpu: false,
            ..Default::default()
        };
        assert_eq!(
            h("01:00.0").kind(),
            Ok(HostPciKind::Pci("0000:01:00.0".into()))
        );
        assert_eq!(
            h(" 0000:0A:00.7 ").kind(),
            Ok(HostPciKind::Pci("0000:0a:00.7".into()))
        );
        assert_eq!(
            h("6B1F3A6E-5D0B-4B5F-9F61-0123456789AB").kind(),
            Ok(HostPciKind::Mdev(
                "6b1f3a6e-5d0b-4b5f-9f61-0123456789ab".into()
            ))
        );
        for bad in [
            "",
            "01:00.8",
            "1:00.0",
            "0000:01:00",
            "zz:00.0",
            "0000:01:00.0,x-vga=on",
            "../../etc",
            "6b1f3a6e-5d0b-4b5f-9f61-0123456789a",
        ] {
            assert!(h(bad).kind().is_err(), "{bad:?} must be refused");
        }
        let mut hw = VmHardware {
            hostpci: vec![h("01:00.0"), h("0000:02:00.0")],
            ..Default::default()
        };
        assert!(hw.validate().is_ok());
        hw.hostpci.push(h("0000:01:00.0"));
        assert!(hw.validate().unwrap_err().contains("twice"));
        hw.hostpci.pop();
        hw.hostpci.iter_mut().for_each(|d| d.primary_gpu = true);
        assert!(hw.validate().unwrap_err().contains("primary"));
        hw.hostpci = (0..9).map(|i| h(&format!("0{i}:00.0"))).collect();
        assert!(hw.validate().unwrap_err().contains("at most"));
        // a device belongs to one guest: clones don't get it
        hw.hostpci = vec![h("01:00.0")];
        assert!(hw.for_clone().hostpci.is_empty());
        // stored configs without the field still load, and it is omitted when empty
        let none: VmHardware = serde_json::from_str("{}").unwrap();
        assert!(none.hostpci.is_empty());
        assert!(!serde_json::to_string(&none).unwrap().contains("hostpci"));
    }

    #[test]
    fn container_options_validation() {
        let ok = ContainerOptions {
            env: vec!["A=1".into(), "HOME_ONLY".into()],
            ports: vec!["80".into(), "8080:80".into(), "127.0.0.1:53:53/udp".into()],
            volumes: vec!["data:/data".into(), "/srv:/srv:ro".into()],
            command: vec!["sleep".into(), "1".into()],
            network: Some("net".into()),
            network_aliases: vec!["web".into()],
        };
        assert!(ok.validate().is_ok());
        for bad in [
            ContainerOptions {
                env: vec!["=1".into()],
                ..Default::default()
            },
            ContainerOptions {
                ports: vec!["0:80".into()],
                ..Default::default()
            },
            ContainerOptions {
                ports: vec!["80:80/icmp".into()],
                ..Default::default()
            },
            ContainerOptions {
                ports: vec!["a:b:c:d".into()],
                ..Default::default()
            },
            ContainerOptions {
                volumes: vec!["data".into()],
                ..Default::default()
            },
            ContainerOptions {
                volumes: vec!["data:relative".into()],
                ..Default::default()
            },
            ContainerOptions {
                volumes: vec!["a:/b:rx".into()],
                ..Default::default()
            },
            ContainerOptions {
                network: Some("--privileged".into()),
                ..Default::default()
            },
        ] {
            assert!(bad.validate().is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn container_config_json_stays_backward_compatible() {
        // A minimal (pre-options) payload still deserializes...
        let c: ContainerConfig = serde_json::from_str(
            r#"{"id":"a","name":"a","runtime":"docker","memory":64,"cpus":1,
                "rootfs":"busybox","status":"stopped"}"#,
        )
        .unwrap();
        assert!(c.options.is_empty());
        assert!(c.created_at.is_none());
        // ...and empty options are not serialized, while set ones are flat.
        let json = serde_json::to_value(&c).unwrap();
        assert!(json.get("env").is_none() && json.get("options").is_none());
        let c2 = ContainerConfig {
            options: ContainerOptions {
                ports: vec!["80".into()],
                ..Default::default()
            },
            created_at: Some("2026-01-01T00:00:00+00:00".into()),
            ..c
        };
        let json = serde_json::to_value(&c2).unwrap();
        assert_eq!(json["ports"][0], "80");
        assert_eq!(json["created_at"], "2026-01-01T00:00:00+00:00");
        let back: ContainerConfig = serde_json::from_value(json).unwrap();
        assert_eq!(back.options.ports, vec!["80"]);
    }

    #[test]
    fn test_vm_config_serialization() {
        let vm = VmConfig {
            id: "test-vm".to_string(),
            name: "Test VM".to_string(),
            hypervisor: VmHypervisor::Qemu,
            architecture: VmArchitecture::X86_64,
            cpus: 2,
            memory: 2048,
            disk_size: 20,
            status: VmStatus::Running,
            disks: Vec::new(),
            display: Default::default(),
            hardware: Default::default(),
        };

        let json = serde_json::to_string(&vm).unwrap();
        let deserialized: VmConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, vm.id);
        assert_eq!(deserialized.name, vm.name);
    }

    #[test]
    fn test_vm_status_transitions() {
        let statuses = vec![
            VmStatus::Stopped,
            VmStatus::Running,
            VmStatus::Paused,
            VmStatus::Unknown,
        ];

        for status in statuses {
            let json = serde_json::to_string(&status).unwrap();
            let _: VmStatus = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn test_node_metrics_validation() {
        let metrics = NodeMetrics {
            cpu_usage: 45.5,
            memory_total: 16384,
            memory_used: 8192,
            disk_total: 500,
            disk_used: 250,
            network_rx_bytes: 1000000,
            network_tx_bytes: 500000,
            load_average: 2.5,
            uptime_seconds: 86400,
        };

        assert!(metrics.cpu_usage >= 0.0 && metrics.cpu_usage <= 100.0);
        assert!(metrics.memory_used <= metrics.memory_total);
        assert!(metrics.disk_used <= metrics.disk_total);
    }

    #[test]
    fn test_alert_rule_creation() {
        let rule = AlertRule {
            name: "high_cpu".to_string(),
            metric: "cpu_usage".to_string(),
            condition: AlertCondition::GreaterThan,
            threshold: 80.0,
            severity: AlertSeverity::Warning,
            enabled: true,
        };

        assert_eq!(rule.name, "high_cpu");
        assert_eq!(rule.threshold, 80.0);
        assert!(rule.enabled);
    }

    #[test]
    fn test_firewall_rule_validation() {
        let rule = FirewallRule {
            name: "allow_http".to_string(),
            action: FirewallAction::Accept,
            protocol: FirewallProtocol::Tcp,
            source: "0.0.0.0/0".to_string(),
            destination: "0.0.0.0/0".to_string(),
            port: Some(80),
            enabled: true,
        };

        assert_eq!(rule.port, Some(80));
        assert!(matches!(rule.action, FirewallAction::Accept));
        assert!(matches!(rule.protocol, FirewallProtocol::Tcp));
    }

    #[test]
    fn test_storage_pool_capacity() {
        let pool = StoragePool {
            id: "pool1".to_string(),
            name: "Main Pool".to_string(),
            pool_type: StorageType::Lvm,
            path: "/dev/vg0/pool1".to_string(),
            size_gb: 100,
            used_gb: 45,
        };

        let available = pool.size_gb - pool.used_gb;
        assert_eq!(available, 55);
        assert!(pool.used_gb <= pool.size_gb);
    }

    #[test]
    fn vm_hardware_validation() {
        let nic = |bridge: &str| VmNic {
            model: NicModel::Virtio,
            bridge: bridge.into(),
            attach: Default::default(),
            mac: None,
            vlan: None,
            rate_mbps: None,
        };
        let mut hw = VmHardware {
            nets: vec![nic("vmbr0")],
            cdrom: None,
            boot: vec![BootDevice::Cdrom, BootDevice::Disk],
            ..Default::default()
        };
        assert!(hw.validate().is_ok());
        hw.nets[0].vlan = Some(4095);
        assert!(hw.validate().unwrap_err().contains("VLAN"));
        hw.nets[0].vlan = Some(20);
        hw.nets.push(nic("bad/name"));
        assert!(hw.validate().unwrap_err().contains("bridge"));
        hw.nets[1] = nic("averyveryverylongbridge");
        assert!(hw.validate().is_err());
        hw.nets[1] = nic("br1");
        hw.nets[0].mac = Some("02:00:00:00:00:01".into());
        hw.nets[1].mac = Some("02-00-00-00-00-01".into());
        assert!(hw.validate().unwrap_err().contains("used twice"));
        hw.nets[1].mac = None;
        hw.boot.push(BootDevice::Disk);
        assert!(hw.validate().unwrap_err().contains("twice"));
        hw.boot.pop();
        hw.nets = (0..=MAX_VM_NICS).map(|_| nic("br0")).collect();
        assert!(hw.validate().is_err());
    }

    #[test]
    fn usb_devices_are_normalized_unique_and_bounded() {
        let dev = |v: &str, p: &str| UsbDevice {
            vendor_id: v.into(),
            product_id: p.into(),
        };
        assert_eq!(
            dev("0x046D", " C52B").normalized().unwrap(),
            dev("046d", "c52b")
        );
        assert!(dev("46d", "c52b").normalized().is_err());
        assert!(dev("046d", "zzzz")
            .normalized()
            .unwrap_err()
            .contains("product"));
        let mut hw = VmHardware {
            usb: vec![dev("046d", "c52b"), dev("0x046D", "C52B")],
            ..Default::default()
        };
        assert!(hw.validate().unwrap_err().contains("twice"));
        hw.usb = (0..=MAX_VM_USB)
            .map(|i| dev("1d6b", &format!("{i:04x}")))
            .collect();
        assert!(hw.validate().unwrap_err().contains("at most"));
        hw.usb = vec![dev("046d", "c52b")];
        assert!(hw.validate().is_ok());
        assert!(
            hw.for_clone().usb.is_empty(),
            "a clone doesn't get the physical device"
        );
        // new fields stay out of old configs
        let j = serde_json::to_value(VmHardware::default()).unwrap();
        assert!(
            j.get("balloon").is_none() && j.get("usb").is_none() && j.get("guest_agent").is_none()
        );
        let old: VmHardware = serde_json::from_str(r#"{"nets":[]}"#).unwrap();
        assert!(!old.balloon && !old.guest_agent && old.usb.is_empty());
    }

    #[test]
    fn tpm_model_defaults_keep_old_vms_on_tis_and_round_trip() {
        let old: VmHardware = serde_json::from_str(r#"{"nets":[],"tpm":true}"#).unwrap();
        assert_eq!(old.tpm_model(), TpmModel::Tis);
        assert!(!old.tpm_state_expected);
        let j = serde_json::to_value(&old).unwrap();
        assert!(j.get("tpm_model").is_none() && j.get("tpm_state_expected").is_none());
        let hw = VmHardware {
            tpm: true,
            tpm_model: Some(TpmModel::Crb),
            tpm_state_expected: true,
            ..Default::default()
        };
        let back: VmHardware = serde_json::from_str(&serde_json::to_string(&hw).unwrap()).unwrap();
        assert_eq!(back, hw);
        assert_eq!(TpmModel::Crb.qemu_device(true), "tpm-crb");
        assert_eq!(TpmModel::Tis.qemu_device(true), "tpm-tis");
        assert_eq!(TpmModel::Crb.qemu_device(false), "tpm-tis-device");
        // CRB is an x86 interface
        assert!(hw.validate_for(&VmArchitecture::X86_64, 2).is_ok());
        assert!(hw.validate_for(&VmArchitecture::Aarch64, 2).is_err());
    }

    #[test]
    fn cpu_machine_firmware_and_disk_validation() {
        let mut hw = VmHardware {
            cpu_type: Some("x86-64-v2-AES".into()),
            sockets: Some(2),
            machine: MachineType::Q35,
            firmware: Firmware::Ovmf,
            tpm: true,
            ..Default::default()
        };
        assert!(hw.validate_for(&VmArchitecture::X86_64, 4).is_ok());
        assert!(hw
            .validate_for(&VmArchitecture::X86_64, 3)
            .unwrap_err()
            .contains("sockets"));
        assert!(hw.validate_for(&VmArchitecture::Aarch64, 4).is_err());
        hw.cpu_type = Some("host,+vmx".into());
        assert!(hw.validate().is_err(), "flags are not a model name");
        hw.cpu_type = None;
        hw.sockets = Some(0);
        assert!(hw.validate().is_err());
        // Old rows/requests without the new keys parse to the defaults,
        // and the defaults aren't written back.
        let parsed: VmHardware = serde_json::from_str(r#"{"nets":[]}"#).unwrap();
        assert_eq!(parsed, VmHardware::default());
        assert_eq!(serde_json::to_string(&parsed).unwrap(), r#"{"nets":[]}"#);
        assert_eq!(
            serde_json::to_value(MachineType::Q35).unwrap(),
            serde_json::json!("q35")
        );

        let disk = |bus: &str| VmDisk {
            path: "/v/d.qcow2".into(),
            size_gb: 1,
            disk_type: bus.into(),
            ..Default::default()
        };
        assert!(disk("virtio").validate(0).is_ok());
        assert!(disk("floppy").validate(0).is_err());
        let mut d = disk("virtio");
        d.ssd = true;
        assert!(d.validate(1).unwrap_err().contains("disk1"));
        d.disk_type = "scsi".into();
        d.iothread = true;
        d.discard = true;
        d.cache = "none".into();
        assert!(d.validate(1).is_ok());
        d.disk_type = "sata".into();
        assert!(d.validate(1).is_err(), "iothread on sata");
        d.iothread = false;
        d.cache = "bogus".into();
        assert!(d.validate(1).is_err());
        // Disks stored before the new options parse with them off.
        let old: VmDisk = serde_json::from_str(
            r#"{"path":"/v/a.qcow2","size_gb":1,"disk_type":"default","cache":""}"#,
        )
        .unwrap();
        assert!(!old.discard && !old.ssd && !old.iothread);
    }

    #[test]
    fn mac_addresses_are_normalized_and_must_be_unicast() {
        assert_eq!(
            normalize_mac("02:AB:cd:00:11:22").unwrap(),
            "02:ab:cd:00:11:22"
        );
        assert_eq!(
            normalize_mac("52-54-00-12-34-56").unwrap(),
            "52:54:00:12:34:56"
        );
        assert!(normalize_mac("01:00:5e:00:00:01").is_err()); // multicast
        assert!(normalize_mac("00:00:00:00:00:00").is_err());
        assert!(normalize_mac("02:00:00:00:00").is_err());
        assert!(normalize_mac("zz:00:00:00:00:00").is_err());
    }

    #[test]
    fn vm_config_without_hardware_still_parses() {
        // VMs stored or sent before NIC support have no "hardware" key.
        let v: VmConfig = serde_json::from_str(
            r#"{"id":"a","name":"a","hypervisor":"qemu","memory":512,"cpus":1,"disk_size":1,"status":"stopped"}"#,
        )
        .unwrap();
        assert!(v.hardware.nets.is_empty());
        assert!(v.hardware.cdrom.is_none());
    }
}
