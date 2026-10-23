//! Common types and utilities shared between horcrux-api and horcrux-ui

pub mod auth;
pub mod lxc;

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
    /// Realtek RTL8139 (very old guests).
    Rtl8139,
}

impl NicModel {
    /// QEMU `-device` driver name.
    pub fn qemu_device(&self) -> &'static str {
        match self {
            NicModel::Virtio => "virtio-net-pci",
            NicModel::E1000 => "e1000",
            NicModel::Rtl8139 => "rtl8139",
        }
    }
}

/// One VM network card: a tap device on a host Linux bridge.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VmNic {
    #[serde(default)]
    pub model: NicModel,
    /// Host bridge the NIC's tap device is attached to (e.g. vmbr0, br0).
    pub bridge: String,
    /// MAC address; generated (locally administered) and stored when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mac: Option<String>,
    /// VLAN tag (the tap's PVID; the bridge must be VLAN-aware).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vlan: Option<u16>,
}

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
    /// Emulated TPM 2.0 chip (swtpm; its state lives next to the first disk).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tpm: bool,
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
        Ok(())
    }

    /// Checks that need the VM's architecture and CPU count.
    pub fn validate_for(
        &self,
        arch: &VmArchitecture,
        cpus: u32,
    ) -> std::result::Result<(), String> {
        self.validate()?;
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
        if *arch != VmArchitecture::X86_64 {
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

/// Emulated sound card; audio is carried over SPICE, so it needs `protocol: spice`.
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
}

impl VmDisplay {
    /// Reject combinations QEMU can't do (or that would silently do nothing).
    pub fn validate(&self, arch: &VmArchitecture) -> std::result::Result<(), String> {
        if self.sound != SoundDevice::None && self.protocol != DisplayProtocol::Spice {
            return Err("Sound is carried over SPICE: set the display protocol to spice".into());
        }
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
    Lxc,   // LXC
    Lxd,   // LXD (can do containers)
    Incus, // Incus (LXD fork, can do containers)
    #[default]
    Docker, // Docker
    Podman, // Podman (daemonless Docker alternative)
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
            mac: None,
            vlan: None,
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
