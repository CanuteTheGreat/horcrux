//! Proxmox VE guest configuration (`/etc/pve/qemu-server/<id>.conf`,
//! `/etc/pve/lxc/<id>.conf`) parsed and mapped to Horcrux guests.
//!
//! Pure: no host access. The caller resolves volumes (`Resolver`) and tells
//! the planner what the target host has (`HostCtx`); every Proxmox key ends
//! up in the plan's table as carried, changed (carried with a different
//! form) or not carried, with the reason.
use horcrux_common::lxc::{LxcFeatures, LxcIoLimit, LxcMount, LxcNet, LxcOptions};
use horcrux_common::{
    BootDevice, ContainerConfig, ContainerRuntime, ContainerStatus, DiskThrottle, Firmware,
    MachineType, NicModel, TpmModel, UsbDevice, VideoDevice, VmAdvanced, VmArchitecture, VmConfig,
    VmDisk, VmDisplay, VmHardware, VmHostPci, VmHypervisor, VmNic, VmSerialPort, VmSmbios,
    VmStatus,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ------------------------------------------------------------------ parse

/// One guest config: the current section only (snapshots and pending
/// changes are listed, not applied).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PveConf {
    /// Keys in file order.
    pub keys: Vec<(String, String)>,
    /// The `#` comment lines of the current section (Proxmox's description).
    pub description: String,
    /// Snapshot section names (`[name]`).
    pub snapshots: Vec<String>,
    /// A `[PENDING]` section exists (changes not applied yet).
    pub pending: bool,
}

impl PveConf {
    pub fn get(&self, k: &str) -> Option<&str> {
        self.keys
            .iter()
            .find(|(a, _)| a == k)
            .map(|(_, v)| v.as_str())
    }
}

/// `%XX` decoding (Proxmox escapes `:` and `%` in descriptions and the
/// cloud-init `sshkeys` value).
pub fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && b[i + 1].is_ascii_hexdigit()
            && b[i + 2].is_ascii_hexdigit()
        {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

pub fn parse_conf(text: &str) -> PveConf {
    let mut c = PveConf::default();
    let mut desc: Vec<String> = Vec::new();
    let mut in_main = true;
    for line in text.lines() {
        let t = line.trim_end();
        if let Some(sec) = t.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            in_main = false;
            if sec == "PENDING" {
                c.pending = true;
            } else if !sec.starts_with("special:") {
                c.snapshots.push(sec.to_string());
            }
            continue;
        }
        if !in_main {
            continue;
        }
        if let Some(d) = t.strip_prefix('#') {
            desc.push(pct_decode(d));
            continue;
        }
        if t.trim().is_empty() {
            continue;
        }
        if let Some((k, v)) = t.split_once(':') {
            c.keys.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    c.description = desc.join("\n");
    c
}

/// `a=1,b=2` (the first part may be a bare value: returned under `""`).
pub fn kv(spec: &str) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for (i, p) in spec.split(',').enumerate() {
        let p = p.trim();
        if p.is_empty() {
            continue;
        }
        match p.split_once('=') {
            Some((k, v)) => {
                m.insert(k.trim().to_string(), v.trim().to_string());
            }
            None if i == 0 => {
                m.insert(String::new(), p.to_string());
            }
            None => {
                m.insert(p.to_string(), "1".into());
            }
        }
    }
    m
}

/// A Proxmox volume reference (`local-zfs:vm-100-disk-0`, `/dev/sdb`,
/// `none`, `cdrom`) plus its options.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PveVol {
    pub storage: Option<String>,
    pub volume: String,
    pub opts: BTreeMap<String, String>,
}

impl PveVol {
    pub fn parse(spec: &str) -> PveVol {
        let mut opts = kv(spec);
        let first = opts
            .remove("")
            .or_else(|| opts.remove("file"))
            .or_else(|| opts.remove("volume"))
            .unwrap_or_default();
        let (storage, volume) = if first.starts_with('/') {
            (None, first)
        } else {
            match first.split_once(':') {
                Some((s, v)) => (Some(s.to_string()), v.to_string()),
                None => (None, first),
            }
        };
        PveVol {
            storage,
            volume,
            opts,
        }
    }
    pub fn spec(&self) -> String {
        match &self.storage {
            Some(s) => format!("{s}:{}", self.volume),
            None => self.volume.clone(),
        }
    }
    /// `none`/`cdrom` (an empty drive) or a pass-through of the host drive.
    pub fn is_empty_drive(&self) -> bool {
        self.storage.is_none() && matches!(self.volume.as_str(), "none" | "cdrom" | "")
    }
    pub fn is_cdrom(&self) -> bool {
        self.opts.get("media").map(String::as_str) == Some("cdrom")
    }
    /// `<n>[KMGT]` -> bytes.
    pub fn size_bytes(&self) -> Option<u64> {
        parse_size(self.opts.get("size")?)
    }
}

pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num, mul) = match s.chars().last()? {
        'K' | 'k' => (&s[..s.len() - 1], 1u64 << 10),
        'M' | 'm' => (&s[..s.len() - 1], 1 << 20),
        'G' | 'g' => (&s[..s.len() - 1], 1 << 30),
        'T' | 't' => (&s[..s.len() - 1], 1 << 40),
        _ => (s, 1),
    };
    let v: f64 = num.parse().ok()?;
    Some((v * mul as f64) as u64)
}

// ------------------------------------------------------------------ plan types

/// How the source volume is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VolKind {
    /// A block device (ZFS zvol, LVM LV): raw.
    Block,
    /// An image file (raw/qcow2/vmdk).
    File,
    /// A directory (container subvolume).
    Dir,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resolved {
    pub path: String,
    pub kind: VolKind,
    /// `raw` / `qcow2` / `vmdk` for File; `raw` for Block.
    pub format: String,
    /// Bytes of the volume when the lookup told (SSH sources).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

pub type Resolver<'a> = &'a dyn Fn(&PveVol) -> Result<Resolved, String>;

/// What the target host has (filled by the caller).
#[derive(Debug, Clone, Default)]
pub struct HostCtx {
    /// Host bridges (names).
    pub bridges: Vec<String>,
    /// Proxmox bridge -> Horcrux bridge (missing = same name).
    pub bridge_map: BTreeMap<String, String>,
    /// PCI devices present: `0000:ad:00.0` -> IOMMU group (None = no IOMMU).
    pub pci: BTreeMap<String, Option<u32>>,
    /// Source block device numbers -> device path (`8:16` -> `/dev/sdb`),
    /// to translate raw `lxc.cgroup2.io.max` rules.
    pub io_devices: BTreeMap<String, String>,
    /// The host has the Secure Boot OVMF build.
    pub ovmf_secboot: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Carry {
    /// Same setting in Horcrux.
    Carried,
    /// Carried in another form (see `horcrux`/`note`).
    Changed,
    /// Not carried; `note` says why and what to do.
    NotCarried,
    /// Proxmox bookkeeping with no meaning on another host.
    Ignored,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub key: String,
    pub value: String,
    pub carry: Carry,
    /// The Horcrux setting it became.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub horcrux: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VolRole {
    Disk,
    Efivars,
    Tpm,
    Cloudinit,
    Rootfs,
    Mount,
}

/// A volume whose data moves into Horcrux storage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanVol {
    pub role: VolRole,
    /// Proxmox key (`scsi0`, `efidisk0`, `rootfs`, `mp1`).
    pub key: String,
    /// Proxmox volume (`local-zfs:vm-100-disk-0`).
    pub source: String,
    /// Where the data is read from on this host (None = not found).
    #[serde(default)]
    pub resolved: Option<Resolved>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
    /// Index of the Horcrux disk (Disk) / mount (Mount).
    #[serde(default)]
    pub index: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Autostart {
    pub onboot: bool,
    pub order: Option<i32>,
    pub up_delay_secs: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuestPlan {
    /// `vm` or `ct`.
    pub kind: String,
    pub vmid: u32,
    pub name: String,
    /// Proposed Horcrux id.
    pub target_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vm: Option<VmConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ct: Option<ContainerConfig>,
    pub autostart: Autostart,
    pub tags: Vec<String>,
    pub description: String,
    pub volumes: Vec<PlanVol>,
    pub rows: Vec<Row>,
    /// Reasons the import is refused (nothing is changed).
    pub blockers: Vec<String>,
    /// The guest's `protection` flag.
    pub protection: bool,
}

impl GuestPlan {
    fn row(&mut self, key: &str, value: &str, carry: Carry, horcrux: &str, note: &str) {
        self.rows.push(Row {
            key: key.into(),
            value: value.into(),
            carry,
            horcrux: horcrux.into(),
            note: note.into(),
        });
    }
}

/// A Horcrux id from a guest name (`dns-alpha-gentoo`), else `pve-<vmid>`.
pub fn target_id(name: &str, vmid: u32) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let s: String = s.trim_matches('-').chars().take(40).collect();
    let s = s.trim_end_matches('-').to_string();
    if s.is_empty() || !s.chars().next().is_some_and(|c| c.is_ascii_alphanumeric()) {
        format!("pve-{vmid}")
    } else {
        s
    }
}

fn on(v: &str) -> bool {
    matches!(v.trim(), "1" | "on" | "yes" | "true")
}

fn startup(v: &str) -> (Option<i32>, Option<u32>, Option<u32>) {
    let m = kv(v);
    let num = |k: &str| m.get(k).and_then(|x| x.parse::<i64>().ok());
    (
        num("order").map(|x| x as i32),
        num("up").map(|x| x.max(0) as u32),
        num("down").map(|x| x.max(0) as u32),
    )
}

fn tags(v: &str) -> Vec<String> {
    v.split([';', ',', ' '])
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(String::from)
        .collect()
}

/// Proxmox MB/s (`rate=`, `mbps_rd=`) to the Horcrux unit.
fn mb_s_to_bytes(v: &str) -> Option<u64> {
    let f: f64 = v.parse().ok()?;
    (f > 0.0).then_some((f * 1024.0 * 1024.0) as u64)
}

fn mb_s_to_mbit(v: &str) -> Option<u32> {
    let f: f64 = v.parse().ok()?;
    (f > 0.0).then_some((f * 8.0).ceil() as u32)
}

/// Full PCI address `0000:ad:00.0` from Proxmox's `ad:00.0` / `0000:ad:00.0`
/// (a bare `ad:00` means every function: only function 0 is mapped here).
pub fn pci_addr(v: &str) -> String {
    let v = v.trim().to_ascii_lowercase();
    let v = if v.matches(':').count() == 1 {
        format!("0000:{v}")
    } else {
        v
    };
    if v.contains('.') {
        v
    } else {
        format!("{v}.0")
    }
}

fn bookkeeping(k: &str) -> bool {
    matches!(
        k,
        "meta" | "digest" | "parent" | "snaptime" | "vmstate" | "runningmachine" | "runningcpu"
    )
}

fn drive_key(k: &str) -> Option<(&'static str, u32)> {
    for (p, bus) in [
        ("scsi", "scsi"),
        ("virtio", "virtio"),
        ("sata", "sata"),
        ("ide", "ide"),
    ] {
        if let Some(n) = k.strip_prefix(p).and_then(|n| n.parse::<u32>().ok()) {
            return Some((bus, n));
        }
    }
    None
}

// ------------------------------------------------------------------ VM

/// Map a qemu-server config to a Horcrux VM plan.
pub fn plan_vm(vmid: u32, conf: &PveConf, resolve: Resolver, host: &HostCtx) -> GuestPlan {
    let name = conf
        .get("name")
        .map(String::from)
        .unwrap_or_else(|| format!("vm{vmid}"));
    let mut p = GuestPlan {
        kind: "vm".into(),
        vmid,
        target_id: target_id(&name, vmid),
        name: name.clone(),
        vm: None,
        ct: None,
        autostart: Autostart::default(),
        tags: vec![],
        description: conf.description.clone(),
        volumes: vec![],
        rows: vec![],
        blockers: vec![],
        protection: false,
    };
    let mut hw = VmHardware::default();
    let mut adv = VmAdvanced::default();
    let mut display = VmDisplay::default();
    let mut cores = 1u32;
    let mut sockets = 1u32;
    let mut memory = 512u64;
    let mut balloon_seen = false;
    let mut tablet = true;
    let mut cpu_seen = false;
    let mut scsihw = "lsi".to_string();
    let mut bios_ovmf = false;
    let mut boot_order: Vec<String> = Vec::new();
    // (bus, index, key, vol) of every disk, in key order
    let mut disks: Vec<(String, u32, String, PveVol)> = Vec::new();
    let mut cdroms: Vec<(String, PveVol)> = Vec::new();
    let mut nets: Vec<(u32, String)> = Vec::new();
    let mut ostype = String::new();

    for (k, v) in &conf.keys {
        let k = k.as_str();
        match k {
            "name" => p.row(k, v, Carry::Carried, "name", ""),
            "memory" => {
                memory = v.parse().unwrap_or(512);
                p.row(k, v, Carry::Carried, "memory (MB)", "");
            }
            "balloon" => {
                balloon_seen = true;
                let b: u64 = v.parse().unwrap_or(0);
                hw.balloon = b > 0;
                if b > 0 {
                    p.row(
                        k,
                        v,
                        Carry::Changed,
                        "hardware.balloon",
                        "the balloon device is carried; Proxmox's minimum (auto-ballooning target) is not: Horcrux sets the balloon size on request (POST /api/vms/:id/balloon)",
                    );
                } else {
                    p.row(k, v, Carry::Carried, "hardware.balloon = false", "");
                }
            }
            "cores" => {
                cores = v.parse().unwrap_or(1).max(1);
                p.row(k, v, Carry::Carried, "cpus (sockets x cores)", "");
            }
            "sockets" => {
                sockets = v.parse().unwrap_or(1).max(1);
                p.row(k, v, Carry::Carried, "hardware.sockets", "");
            }
            "vcpus" => p.row(
                k,
                v,
                Carry::NotCarried,
                "",
                "hot-plugged vCPU count: the VM starts with all sockets x cores CPUs",
            ),
            "cpu" => {
                cpu_seen = true;
                let m = kv(v);
                let model = m.get("").or(m.get("cputype")).cloned().unwrap_or_default();
                if horcrux_common::valid_cpu_type(&model) {
                    hw.cpu_type = Some(model.clone());
                    let extra: Vec<&String> = m.keys().filter(|k| !k.is_empty() && *k != "cputype").collect();
                    if extra.is_empty() {
                        p.row(k, v, Carry::Carried, "hardware.cpu_type", "");
                    } else {
                        p.row(
                            k,
                            v,
                            Carry::Changed,
                            "hardware.cpu_type",
                            &format!("the CPU model is carried; {} not", extra.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")),
                        );
                    }
                } else {
                    p.row(k, v, Carry::NotCarried, "", "unknown CPU model: QEMU's default is used");
                }
            }
            "numa" => {
                adv.numa = on(v);
                p.row(k, v, Carry::Carried, "hardware.adv.numa", "");
            }
            "affinity" => {
                adv.cpu_pinning = Some(horcrux_common::VmCpuPinning {
                    vcpus: v.clone(),
                    emulator: None,
                });
                p.row(k, v, Carry::Changed, "hardware.adv.cpu_pinning", "Proxmox pins the whole QEMU process; Horcrux pins one vCPU per listed host CPU");
            }
            "hugepages" => {
                let kb = match v.as_str() {
                    "1024" => Some(1_048_576),
                    "2" | "any" => Some(2048),
                    _ => None,
                };
                if let Some(kb) = kb {
                    adv.hugepages = Some(horcrux_common::VmHugePages { size_kb: kb });
                    p.row(k, v, Carry::Carried, "hardware.adv.hugepages", "");
                } else {
                    p.row(k, v, Carry::NotCarried, "", "unknown huge page size");
                }
            }
            "machine" => {
                let m = kv(v);
                let t = m.get("").or(m.get("type")).cloned().unwrap_or_default();
                if t.contains("q35") {
                    hw.machine = MachineType::Q35;
                    p.row(k, v, Carry::Changed, "hardware.machine = q35", if t == "q35" { "" } else { "a pinned machine version is not kept: Horcrux runs the host QEMU's newest q35" });
                } else if t.is_empty() || t.contains("i440fx") || t == "pc" {
                    p.row(k, v, Carry::Carried, "hardware.machine = i440fx", "");
                } else {
                    p.row(k, v, Carry::NotCarried, "", "machine type not supported: i440FX is used");
                }
                if m.contains_key("viommu") {
                    p.row(k, v, Carry::NotCarried, "", "vIOMMU is not supported by Horcrux VMs");
                }
            }
            "bios" => {
                bios_ovmf = v == "ovmf";
                hw.firmware = if bios_ovmf { Firmware::Ovmf } else { Firmware::Seabios };
                p.row(k, v, Carry::Carried, "hardware.firmware", "");
            }
            "efidisk0" => {
                let vol = PveVol::parse(v);
                let pre = vol.opts.get("pre-enrolled-keys").is_some_and(|x| on(x));
                hw.secure_boot = pre;
                let note = if vol.opts.get("efitype").map(String::as_str) == Some("2m") {
                    "a 2 MB (old layout) variable store: Horcrux's OVMF uses the 4 MB layout, so a fresh store is created unless the layouts match (boot entries are re-discovered)"
                } else {
                    "the UEFI variable store (boot entries, Secure Boot keys) is copied into the VM's own store"
                };
                p.row(k, v, Carry::Carried, if pre { "UEFI variables + secure_boot" } else { "UEFI variables" }, note);
                p.volumes.push(PlanVol {
                    role: VolRole::Efivars,
                    key: k.into(),
                    source: vol.spec(),
                    resolved: resolve(&vol).ok(),
                    size_bytes: vol.size_bytes(),
                    index: None,
                });
            }
            "tpmstate0" => {
                let vol = PveVol::parse(v);
                let ver = vol.opts.get("version").cloned().unwrap_or_else(|| "v1.2".into());
                if ver != "v2.0" {
                    p.blockers.push(format!("tpmstate0 is TPM {ver}: Horcrux runs TPM 2.0 only, so the guest would lose its TPM keys (BitLocker, Windows Hello, device identity)"));
                    p.row(k, v, Carry::NotCarried, "", "TPM 1.2 cannot be carried");
                } else {
                    hw.tpm = true;
                    hw.tpm_model = Some(TpmModel::Tis);
                    hw.tpm_state_expected = true;
                    p.row(k, v, Carry::Carried, "hardware.tpm (tis) + state", "the swtpm state is converted from Proxmox's linear volume into the VM's TPM state (same keys)");
                    let r = resolve(&vol);
                    if let Err(e) = &r {
                        p.blockers.push(format!("the TPM state volume {} was not found ({e}): the guest would lose its TPM keys", vol.spec()));
                    }
                    p.volumes.push(PlanVol {
                        role: VolRole::Tpm,
                        key: k.into(),
                        source: vol.spec(),
                        resolved: r.ok(),
                        size_bytes: vol.size_bytes(),
                        index: None,
                    });
                }
            }
            "scsihw" => {
                scsihw = v.clone();
                match v.as_str() {
                    "virtio-scsi-single" => p.row(k, v, Carry::Carried, "one virtio-scsi controller per disk", ""),
                    "virtio-scsi-pci" => p.row(k, v, Carry::Changed, "one virtio-scsi controller per disk", "Horcrux gives every SCSI disk its own virtio-scsi controller (like virtio-scsi-single); the guest driver is the same"),
                    _ => p.row(k, v, Carry::Changed, "virtio-scsi", "this SCSI controller model is not emulated: disks are on virtio-scsi, so the guest needs the virtio-scsi driver"),
                }
            }
            "boot" => {
                let m = kv(v);
                if let Some(o) = m.get("order") {
                    boot_order = o.split(';').map(String::from).collect();
                    p.row(k, v, Carry::Changed, "hardware.boot + disk order", "the first boot disk becomes disk 0; Horcrux boots disks, then CD-ROM, then network in the listed class order");
                } else {
                    // legacy `boot: cdn` letters
                    for ch in m.get("").cloned().unwrap_or_default().chars() {
                        boot_order.push(match ch { 'c' => "disk".into(), 'd' => "cdrom".into(), 'n' => "net".into(), _ => continue });
                    }
                    p.row(k, v, Carry::Changed, "hardware.boot", "legacy boot letters");
                }
            }
            "bootdisk" => {
                if boot_order.is_empty() {
                    boot_order.push(v.clone());
                }
                p.row(k, v, Carry::Changed, "disk 0", "");
            }
            "agent" => {
                let m = kv(v);
                hw.guest_agent = m.get("").or(m.get("enabled")).is_some_and(|x| on(x));
                p.row(k, v, Carry::Carried, "hardware.guest_agent", "");
            }
            "tablet" => {
                tablet = on(v);
                p.row(k, v, Carry::Carried, if tablet { "absolute pointer (usb-tablet)" } else { "display.relative_mouse" }, "");
            }
            "vga" => {
                let m = kv(v);
                let t = m.get("").or(m.get("type")).cloned().unwrap_or_else(|| "std".into());
                let (vd, carry, note) = match t.as_str() {
                    "std" => (Some(VideoDevice::Std), Carry::Carried, ""),
                    "virtio" | "virtio-gl" => (Some(VideoDevice::Virtio), if t == "virtio" { Carry::Carried } else { Carry::Changed }, if t == "virtio" { "" } else { "3D (virgl) is not carried" }),
                    "qxl" | "qxl2" | "qxl3" | "qxl4" => (Some(VideoDevice::Qxl), Carry::Changed, "one QXL head"),
                    "none" => (Some(VideoDevice::None), Carry::Carried, ""),
                    "serial0" | "serial1" | "serial2" | "serial3" => (Some(VideoDevice::None), Carry::Changed, "no video card; use the serial console"),
                    _ => (Some(VideoDevice::Std), Carry::Changed, "this video model is not emulated: standard VGA"),
                };
                if let Some(vd) = vd {
                    display.video = vd;
                }
                p.row(k, v, carry, "display.video", note);
            }
            "smbios1" => {
                let m = kv(v);
                let b64 = m.get("base64").is_some_and(|x| on(x));
                let dec = |x: Option<&String>| -> Option<String> {
                    let x = x?;
                    if b64 {
                        use base64::Engine;
                        base64::engine::general_purpose::STANDARD
                            .decode(x)
                            .ok()
                            .map(|b| String::from_utf8_lossy(&b).to_string())
                    } else {
                        Some(x.clone())
                    }
                };
                adv.smbios = Some(VmSmbios {
                    manufacturer: dec(m.get("manufacturer")),
                    product: dec(m.get("product")),
                    version: dec(m.get("version")),
                    serial: dec(m.get("serial")),
                    family: dec(m.get("family")),
                    uuid: m.get("uuid").cloned(),
                });
                p.row(k, v, Carry::Carried, "hardware.adv.smbios (system UUID)", "");
            }
            "vmgenid" => {
                adv.vmgenid = Some(v.clone());
                p.row(k, v, Carry::Carried, "hardware.adv.vmgenid", "");
            }
            "protection" => {
                adv.protection = on(v);
                p.protection = on(v);
                p.row(k, v, Carry::Carried, "hardware.adv.protection", "");
            }
            "onboot" => {
                p.autostart.onboot = on(v);
                p.row(k, v, Carry::Changed, "guest power policy: start_action", "start on host boot (always) or stay stopped");
            }
            "startup" => {
                let (o, up, down) = startup(v);
                p.autostart.order = o;
                p.autostart.up_delay_secs = up;
                p.row(k, v, Carry::Changed, "guest power policy: start_order / start_delay_secs", if down.is_some() { "the shutdown timeout (down=) is not per guest in Horcrux: the node's stop budget applies" } else { "" });
            }
            "tags" => {
                p.tags = tags(v);
                p.row(k, v, Carry::Carried, "tags", "");
            }
            "ostype" => {
                ostype = v.clone();
                p.row(k, v, Carry::Ignored, "", "informational");
            }
            "localtime" => p.row(k, v, Carry::NotCarried, "", "Horcrux keeps the guest clock in UTC: a Windows guest needs RealTimeIsUniversal=1 (see the warning)"),
            "hookscript" => p.row(k, v, Carry::NotCarried, "", "a Proxmox snippet path: set hardware.adv.hookscript to a script on this host if still needed"),
            "args" => p.row(k, v, Carry::NotCarried, "", "raw QEMU arguments are never copied: add the equivalent setting by hand"),
            "hotplug" => p.row(k, v, Carry::Ignored, "", "Horcrux decides hot-plug per device"),
            "kvm" if !on(v) => p.row(k, v, Carry::NotCarried, "", "Horcrux always uses KVM when the host has it"),
            "lock" => {
                p.blockers.push(format!("the guest is locked in Proxmox ({v}): finish or clear that operation first"));
                p.row(k, v, Carry::NotCarried, "", "a Proxmox lock: blocks the import");
            }
            "template" if on(v) => p.row(k, v, Carry::NotCarried, "", "Proxmox template: imported as a normal VM (convert it to a Horcrux template afterwards)"),
            k if bookkeeping(k) => p.row(k, v, Carry::Ignored, "", "Proxmox bookkeeping"),
            k if k.starts_with("ipconfig") || matches!(k, "ciuser" | "cipassword" | "nameserver" | "searchdomain" | "sshkeys" | "citype" | "ciupgrade" | "cicustom") => {
                let shown = if k == "cipassword" { "(hidden)".to_string() } else if k == "sshkeys" { pct_decode(v) } else { v.clone() };
                p.row(k, shown.trim(), Carry::Changed, "cloud-init seed", "inside the carried cloud-init image (the guest sees the same NoCloud data); edit later through Horcrux cloud-init");
            }
            k if k.starts_with("net") && k[3..].parse::<u32>().is_ok() => nets.push((k[3..].parse().unwrap_or(0), v.clone())),
            k if drive_key(k).is_some() => {
                let (bus, n) = drive_key(k).unwrap_or(("ide", 0));
                let vol = PveVol::parse(v);
                if vol.is_cdrom() {
                    cdroms.push((k.to_string(), vol));
                } else {
                    disks.push((bus.to_string(), n, k.to_string(), vol));
                }
            }
            k if k.starts_with("hostpci") => {
                let m = kv(v);
                let dev = m.get("").or(m.get("host")).cloned().unwrap_or_default();
                if let Some(md) = m.get("mdev") {
                    p.row(k, v, Carry::NotCarried, "", &format!("mediated device type {md}: create the mdev on this host and add it under Hardware > PCI"));
                    continue;
                }
                if dev.contains(';') {
                    p.row(k, v, Carry::NotCarried, "", "several devices in one entry: add them one by one under Hardware > PCI");
                    continue;
                }
                if !dev.contains(':') {
                    p.blockers.push(format!("{k} uses a Proxmox resource mapping ({dev}): give the host PCI address instead"));
                    p.row(k, v, Carry::NotCarried, "", "resource mappings are Proxmox cluster objects: blocks the import");
                    continue;
                }
                let addr = pci_addr(&dev);
                match host.pci.get(&addr) {
                    None => {
                        p.blockers.push(format!("{k}: PCI device {addr} does not exist on this host: the guest would boot without it. Install the card, or remove {k} from the source config before importing"));
                        p.row(k, v, Carry::NotCarried, "", &format!("{addr} is not on this host: blocks the import"));
                    }
                    Some(None) => {
                        p.blockers.push(format!("{k}: PCI device {addr} has no IOMMU group on this host (IOMMU off: enable VT-d/AMD-Vi and intel_iommu=on / amd_iommu=on), so it cannot be passed through"));
                        p.row(k, v, Carry::NotCarried, "", &format!("{addr} has no IOMMU group here: blocks the import"));
                    }
                    Some(Some(g)) => {
                        hw.hostpci.push(VmHostPci {
                            device: addr.clone(),
                            primary_gpu: m.get("x-vga").is_some_and(|x| on(x)),
                            managed: true,
                            allow_display_loss: false,
                        });
                        let extra: Vec<&str> = m.keys().map(String::as_str).filter(|k| !matches!(*k, "" | "host" | "x-vga" | "pcie")).collect();
                        let mut note = format!("IOMMU group {g}; Horcrux binds it to vfio-pci when the VM starts and gives it back when it stops");
                        if !extra.is_empty() {
                            note.push_str(&format!("; not carried: {}", extra.join(", ")));
                        }
                        p.row(k, v, if extra.is_empty() { Carry::Carried } else { Carry::Changed }, "hardware.hostpci (managed)", &note);
                    }
                }
            }
            k if k.starts_with("usb") && k[3..].parse::<u32>().is_ok() => {
                let m = kv(v);
                let h = m.get("").or(m.get("host")).cloned().unwrap_or_default();
                if let Some((vid, pid)) = h.split_once(':').filter(|(a, b)| a.len() == 4 && b.len() == 4) {
                    hw.usb.push(UsbDevice { vendor_id: vid.to_ascii_lowercase(), product_id: pid.to_ascii_lowercase() });
                    hw.usb_controller = true;
                    p.row(k, v, Carry::Carried, "hardware.usb", "");
                } else {
                    p.row(k, v, Carry::NotCarried, "", "USB by host port (or SPICE redirection) is not carried: add the device by vendor:product under Hardware > USB");
                }
            }
            k if k.starts_with("serial") && k[6..].parse::<u32>().is_ok() => {
                if k == "serial0" && v == "socket" {
                    p.row(k, v, Carry::Carried, "the VM's serial console (serial0)", "every Horcrux VM has serial0 on a socket next to its disk (the console page's Serial tab, logged)");
                } else if v == "socket" {
                    adv.serial.push(VmSerialPort { device: None });
                    p.row(k, v, Carry::Changed, "hardware.adv.serial (socket)", "an extra port: a PCI serial device on a Unix socket next to the disk");
                } else if v.starts_with("/dev/tty") {
                    adv.serial.push(VmSerialPort { device: Some(v.clone()) });
                    p.row(k, v, Carry::Changed, "hardware.adv.serial (host tty)", "a PCI serial device on the host tty");
                } else {
                    p.row(k, v, Carry::NotCarried, "", "this serial device is not supported");
                }
            }
            k if k.starts_with("unused") => p.row(k, v, Carry::NotCarried, "", "a detached disk: not imported (attach it in Proxmox first to carry it)"),
            k if k.starts_with("virtiofs") => p.row(k, v, Carry::NotCarried, "", "Proxmox directory mapping: add a virtiofs share under Hardware with the host path"),
            "rng0" | "watchdog" | "audio0" | "spice_enhancements" | "keyboard" | "cpulimit" | "cpuunits" | "shares" | "acpi" | "freeze" | "reboot" | "migrate_downtime" | "migrate_speed" | "vmstatestorage" | "ivshmem" | "amd-sev" => {
                p.row(k, v, Carry::NotCarried, "", "no Horcrux equivalent carried by the importer: set it under Hardware if needed")
            }
            _ => p.row(k, v, Carry::NotCarried, "", "unknown key"),
        }
    }

    if !balloon_seen {
        hw.balloon = true; // Proxmox's default: a balloon device
    }
    if !cpu_seen {
        // Proxmox >= 8 default model
        hw.cpu_type = Some("x86-64-v2-AES".into());
        p.row(
            "cpu",
            "(default)",
            Carry::Changed,
            "hardware.cpu_type = x86-64-v2-AES",
            "Proxmox's default CPU model",
        );
    }
    if !bios_ovmf && p.volumes.iter().any(|v| v.role == VolRole::Efivars) {
        p.volumes.retain(|v| v.role != VolRole::Efivars);
    }
    if hw.secure_boot && !host.ovmf_secboot {
        p.blockers.push("the VM has Secure Boot keys enrolled but this host has no Secure Boot OVMF build (install sys-firmware/edk2 with Secure Boot)".into());
    }
    display.relative_mouse = !tablet;
    if ostype.starts_with("win") && conf.get("localtime").is_none_or(on) {
        p.row("localtime", "(windows default)", Carry::NotCarried, "", "Proxmox runs Windows guests on local time; Horcrux keeps UTC: set HKLM\\SYSTEM\\CurrentControlSet\\Control\\TimeZoneInformation RealTimeIsUniversal=1 in the guest before the first boot here, or its clock is off by the UTC offset");
    }
    let _ = &scsihw;

    // disks, ordered: the first boot disk first
    let first_boot = boot_order.iter().find(|b| drive_key(b).is_some()).cloned();
    disks.sort_by_key(|(bus, n, key, _)| {
        let rank = if Some(key) == first_boot.as_ref() {
            0
        } else {
            1
        };
        let bus_rank = ["scsi", "virtio", "sata", "ide"]
            .iter()
            .position(|b| b == bus)
            .unwrap_or(9);
        (rank, bus_rank, *n)
    });
    let mut hdisks: Vec<VmDisk> = Vec::new();
    for (bus, _n, key, vol) in &disks {
        let spec = vol.spec();
        if vol.is_empty_drive() {
            continue;
        }
        if vol.volume.ends_with("-cloudinit") || vol.volume.contains("cloudinit") {
            // a cloud-init drive attached as a disk (rare)
            p.volumes.push(PlanVol {
                role: VolRole::Cloudinit,
                key: key.clone(),
                source: spec.clone(),
                resolved: resolve(vol).ok(),
                size_bytes: vol.size_bytes(),
                index: None,
            });
            p.row(key, &spec, Carry::Changed, "cloud-init seed", "");
            continue;
        }
        let r = resolve(vol);
        if let Err(e) = &r {
            p.blockers
                .push(format!("{key}: volume {spec} not found: {e}"));
        }
        let mut d = VmDisk {
            path: String::new(),
            size_gb: vol.size_bytes().unwrap_or(0).div_ceil(1 << 30).max(1),
            disk_type: bus.clone(),
            // Proxmox's default for disks without cache=: no host cache.
            cache: vol
                .opts
                .get("cache")
                .cloned()
                .unwrap_or_else(|| "none".into()),
            discard: vol.opts.get("discard").map(String::as_str) == Some("on"),
            ssd: vol.opts.get("ssd").is_some_and(|x| on(x)) && bus != "virtio",
            iothread: vol.opts.get("iothread").is_some_and(|x| on(x))
                && matches!(bus.as_str(), "virtio" | "scsi"),
            storage: None,
            throttle: DiskThrottle::default(),
        };
        if !horcrux_common::DISK_CACHE_MODES.contains(&d.cache.as_str()) {
            d.cache = "none".into();
        }
        let both = |a: &str, b: &str| vol.opts.get(a).or(vol.opts.get(b)).cloned();
        d.throttle.bps_rd = both("mbps_rd", "mbps").and_then(|x| mb_s_to_bytes(&x));
        d.throttle.bps_wr = both("mbps_wr", "mbps").and_then(|x| mb_s_to_bytes(&x));
        d.throttle.iops_rd = both("iops_rd", "iops").and_then(|x| x.parse().ok());
        d.throttle.iops_wr = both("iops_wr", "iops").and_then(|x| x.parse().ok());
        let mut lost: Vec<String> = vol
            .opts
            .keys()
            .filter(|k| {
                !matches!(
                    k.as_str(),
                    "size"
                        | "discard"
                        | "ssd"
                        | "iothread"
                        | "cache"
                        | "mbps_rd"
                        | "mbps_wr"
                        | "mbps"
                        | "iops_rd"
                        | "iops_wr"
                        | "iops"
                        | "format"
                        | "aio"
                        | "backup"
                        | "replicate"
                        | "media"
                )
            })
            .cloned()
            .collect();
        if vol.opts.get("ssd").is_some_and(|x| on(x)) && bus == "virtio" {
            lost.push("ssd (virtio-blk has no rotation rate)".into());
        }
        let mut note = format!("disk {} on {bus}", hdisks.len());
        if vol.opts.get("backup").map(String::as_str) == Some("0") {
            note.push_str("; backup=0: leave it out of Horcrux backup jobs");
        }
        if !lost.is_empty() {
            note.push_str(&format!("; not carried: {}", lost.join(", ")));
        }
        p.row(
            key,
            &format!(
                "{spec},{}",
                vol.opts
                    .iter()
                    .map(|(a, b)| format!("{a}={b}"))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            if lost.is_empty() {
                Carry::Carried
            } else {
                Carry::Changed
            },
            "disk (data copied into Horcrux storage)",
            &note,
        );
        p.volumes.push(PlanVol {
            role: VolRole::Disk,
            key: key.clone(),
            source: spec,
            resolved: r.ok(),
            size_bytes: vol.size_bytes(),
            index: Some(hdisks.len()),
        });
        hdisks.push(d);
    }
    if hdisks.is_empty() {
        p.blockers.push("the VM has no disk to import".into());
    }

    // CD-ROMs
    let mut cdrom_in_boot = false;
    for (key, vol) in &cdroms {
        if boot_order.iter().any(|b| b == key) {
            cdrom_in_boot = true;
        }
        if vol.volume.contains("cloudinit") {
            p.volumes.push(PlanVol {
                role: VolRole::Cloudinit,
                key: key.clone(),
                source: vol.spec(),
                resolved: resolve(vol).ok(),
                size_bytes: vol.size_bytes(),
                index: None,
            });
            p.row(key, &vol.spec(), Carry::Changed, "cloud-init seed (<disk0>.cidata.iso)", "the NoCloud seed the guest booted with is copied byte for byte and attached read-only");
        } else if vol.is_empty_drive() {
            p.row(key, &vol.spec(), Carry::Carried, "empty CD-ROM drive", "");
        } else {
            p.row(
                key,
                &vol.spec(),
                Carry::NotCarried,
                "",
                "the ISO is not copied: upload it to the ISO library and insert it under Hardware",
            );
        }
    }

    // NICs
    nets.sort_by_key(|(n, _)| *n);
    for (n, v) in &nets {
        let key = format!("net{n}");
        let m = kv(v);
        let (model, mac) = [
            "virtio",
            "e1000",
            "e1000e",
            "rtl8139",
            "vmxnet3",
            "e1000-82540em",
            "e1000-82544gc",
            "e1000-82545em",
        ]
        .iter()
        .find_map(|mo| m.get(*mo).map(|mac| (*mo, mac.clone())))
        .unwrap_or(("virtio", String::new()));
        let nm = match model {
            "virtio" => NicModel::Virtio,
            "e1000e" => NicModel::E1000e,
            "rtl8139" => NicModel::Rtl8139,
            "vmxnet3" => NicModel::Vmxnet3,
            _ => NicModel::E1000,
        };
        let src_bridge = m.get("bridge").cloned().unwrap_or_default();
        let bridge = host
            .bridge_map
            .get(&src_bridge)
            .cloned()
            .unwrap_or_else(|| src_bridge.clone());
        if bridge.is_empty() {
            p.blockers.push(format!("{key} has no bridge"));
            p.row(
                &key,
                v,
                Carry::NotCarried,
                "",
                "no bridge: blocks the import",
            );
            continue;
        }
        if !host.bridges.contains(&bridge) {
            p.blockers.push(format!("{key}: bridge {bridge} does not exist on this host: create it (Network) or map {src_bridge} to an existing bridge"));
        }
        let mut lost = vec![];
        for k in m.keys() {
            if matches!(k.as_str(), "bridge" | "tag" | "rate") || k == model {
                continue;
            }
            lost.push(match k.as_str() {
                "firewall" => "firewall=1 (Proxmox firewall rules are not imported: add the VM's rules under Firewall)".to_string(),
                "link_down" => "link_down (the link is up in Horcrux)".to_string(),
                other => other.to_string(),
            });
        }
        if model.starts_with("e1000-") {
            lost.push(format!("{model} sub-model (plain e1000)"));
        }
        let nic = VmNic {
            model: nm,
            bridge: bridge.clone(),
            attach: Default::default(),
            mac: (!mac.is_empty()).then(|| mac.to_ascii_lowercase()),
            vlan: m.get("tag").and_then(|t| t.parse().ok()),
            rate_mbps: m.get("rate").and_then(|r| mb_s_to_mbit(r)),
        };
        let mut note = format!("MAC {} kept", nic.mac.clone().unwrap_or_default());
        if bridge != src_bridge {
            note.push_str(&format!("; bridge {src_bridge} -> {bridge}"));
        }
        if let Some(r) = nic.rate_mbps {
            note.push_str(&format!(
                "; rate {} MB/s = {r} Mbit/s",
                m.get("rate").cloned().unwrap_or_default()
            ));
        }
        if !lost.is_empty() {
            note.push_str(&format!("; not carried: {}", lost.join("; ")));
        }
        p.row(
            &key,
            v,
            if lost.is_empty() {
                Carry::Carried
            } else {
                Carry::Changed
            },
            "hardware.nets",
            &note,
        );
        hw.nets.push(nic);
    }

    // boot classes in Proxmox order
    let mut classes: Vec<BootDevice> = Vec::new();
    for b in &boot_order {
        let c = if b.starts_with("net") || b == "net" {
            BootDevice::Net
        } else if b == "cdrom" || cdroms.iter().any(|(k, _)| k == b) {
            BootDevice::Cdrom
        } else {
            BootDevice::Disk
        };
        if !classes.contains(&c) {
            classes.push(c);
        }
    }
    let _ = cdrom_in_boot;
    hw.boot = classes;
    hw.adv = adv;

    let cpus = (cores * sockets).max(1);
    if sockets > 1 {
        hw.sockets = Some(sockets);
    }
    let disk0 = hdisks.first().map(|d| d.size_gb).unwrap_or(1);
    p.vm = Some(VmConfig {
        id: p.target_id.clone(),
        name,
        hypervisor: VmHypervisor::Qemu,
        memory,
        cpus,
        disk_size: disk0,
        status: VmStatus::Stopped,
        architecture: VmArchitecture::X86_64,
        disks: hdisks,
        display,
        hardware: hw,
    });
    if !conf.snapshots.is_empty() {
        p.row("snapshots", &conf.snapshots.join(", "), Carry::NotCarried, "", "Proxmox snapshots are not imported: the current state is (take a Horcrux snapshot after the import)");
    }
    if conf.pending {
        p.row(
            "[PENDING]",
            "",
            Carry::NotCarried,
            "",
            "pending (not yet applied) Proxmox changes are ignored: the running config is imported",
        );
    }
    if !conf.description.is_empty() {
        p.row(
            "description",
            &conf.description,
            Carry::NotCarried,
            "",
            "Horcrux VMs have no description field: kept with the saved source config",
        );
    }
    p
}

// ------------------------------------------------------------------ CT

/// Map a pct config to a Horcrux LXC container plan.
pub fn plan_ct(vmid: u32, conf: &PveConf, resolve: Resolver, host: &HostCtx) -> GuestPlan {
    let name = conf
        .get("hostname")
        .map(String::from)
        .unwrap_or_else(|| format!("ct{vmid}"));
    let mut p = GuestPlan {
        kind: "ct".into(),
        vmid,
        target_id: target_id(&name, vmid),
        name: name.clone(),
        vm: None,
        ct: None,
        autostart: Autostart::default(),
        tags: vec![],
        description: conf.description.clone(),
        volumes: vec![],
        rows: vec![],
        blockers: vec![],
        protection: false,
    };
    let mut o = LxcOptions {
        // Proxmox's default when the key is absent is privileged.
        unprivileged: false,
        hostname: Some(name.clone()),
        ..Default::default()
    };
    let mut memory = 512u64;
    let mut cpus = 0u32;
    let mut rootfs_seen = false;
    let mut mounted_devices: Vec<String> = Vec::new();
    let mut allowed: Vec<String> = Vec::new();
    let mut raw_mounts: Vec<(String, String)> = Vec::new();
    let mut nets: Vec<(u32, String)> = Vec::new();
    let mut mps: Vec<(u32, String)> = Vec::new();

    for (k, v) in &conf.keys {
        let k = k.as_str();
        match k {
            "hostname" => p.row(k, v, Carry::Carried, "lxc.hostname", ""),
            "arch" => {
                o.arch = Some(v.clone());
                p.row(k, v, Carry::Carried, "lxc.arch", "");
            }
            "memory" => {
                memory = v.parse().unwrap_or(512);
                p.row(k, v, Carry::Carried, "memory (MB)", "");
            }
            "swap" => {
                o.swap_mb = v.parse().unwrap_or(0);
                p.row(k, v, Carry::Carried, "lxc.swap_mb", "");
            }
            "cores" => {
                cpus = v.parse().unwrap_or(0);
                p.row(k, v, Carry::Carried, "cpus", "");
            }
            "cpulimit" => {
                let c: f64 = v.parse().unwrap_or(0.0);
                if c > 0.0 {
                    o.cpu_limit = Some(c);
                }
                p.row(k, v, Carry::Carried, "lxc.cpu_limit", "");
            }
            "cpuunits" => {
                o.cpu_units = v.parse().ok().filter(|u: &u32| (1..=10000).contains(u));
                p.row(k, v, Carry::Carried, "lxc.cpu_units", "");
            }
            "unprivileged" => {
                o.unprivileged = on(v);
                p.row(k, v, Carry::Carried, "lxc.unprivileged (id map 100000/65536)", if on(v) { "Proxmox and Horcrux use the same id map, so file owners stay as the container sees them" } else { "" });
            }
            "features" => {
                let m = kv(v);
                let mut f = LxcFeatures::default();
                let mut lost = vec![];
                for (fk, fv) in &m {
                    match fk.as_str() {
                        "nesting" => f.nesting = on(fv),
                        "keyctl" => f.keyctl = on(fv),
                        "fuse" => f.fuse = on(fv),
                        other => lost.push(format!("{other}={fv}")),
                    }
                }
                o.features = f;
                p.row(
                    k,
                    v,
                    if lost.is_empty() {
                        Carry::Carried
                    } else {
                        Carry::Changed
                    },
                    "lxc.features",
                    &if lost.is_empty() {
                        String::new()
                    } else {
                        format!("not carried: {} (no Horcrux equivalent)", lost.join(", "))
                    },
                );
            }
            "onboot" => {
                o.onboot = on(v);
                p.autostart.onboot = on(v);
                p.row(k, v, Carry::Carried, "lxc.onboot", "");
            }
            "startup" => {
                let (ord, up, down) = startup(v);
                o.startup_order = ord.map(|x| x.max(0) as u32);
                o.startup_delay = up;
                p.autostart.order = ord;
                p.autostart.up_delay_secs = up;
                p.row(
                    k,
                    v,
                    Carry::Carried,
                    "lxc.startup_order / startup_delay",
                    if down.is_some() {
                        "down= (shutdown timeout) is not carried"
                    } else {
                        ""
                    },
                );
            }
            "nameserver" => {
                o.nameserver = Some(v.clone());
                p.row(k, v, Carry::Carried, "lxc.nameserver", "");
            }
            "searchdomain" => {
                o.searchdomain = Some(v.clone());
                p.row(k, v, Carry::Carried, "lxc.searchdomain", "");
            }
            "tags" => {
                p.tags = tags(v);
                p.row(k, v, Carry::Carried, "tags", "");
            }
            "protection" => {
                p.protection = on(v);
                p.row(
                    k,
                    v,
                    Carry::NotCarried,
                    "",
                    "Horcrux containers have no delete protection yet",
                );
            }
            "ostype" => p.row(
                k,
                v,
                Carry::Ignored,
                "",
                "Horcrux detects the distribution from the rootfs",
            ),
            "console" | "tty" | "cmode" => p.row(
                k,
                v,
                Carry::Ignored,
                "",
                "Horcrux sets up the console itself",
            ),
            "rootfs" => {
                rootfs_seen = true;
                let vol = PveVol::parse(v);
                let r = resolve(&vol);
                if let Err(e) = &r {
                    p.blockers
                        .push(format!("rootfs volume {} not found: {e}", vol.spec()));
                }
                if let Some(sz) = vol.size_bytes() {
                    o.disk_gb = Some(sz.div_ceil(1 << 30).max(1));
                }
                p.row(k, v, Carry::Changed, "rootfs (files copied into Horcrux's container storage)", &format!("size {} becomes a {} GB ext4 image (hard limit, can be grown); owners preserved", vol.opts.get("size").cloned().unwrap_or_default(), o.disk_gb.unwrap_or(0)));
                p.volumes.push(PlanVol {
                    role: VolRole::Rootfs,
                    key: k.into(),
                    source: vol.spec(),
                    resolved: r.ok(),
                    size_bytes: vol.size_bytes(),
                    index: None,
                });
            }
            k if k.starts_with("mp") && k[2..].parse::<u32>().is_ok() => {
                mps.push((k[2..].parse().unwrap_or(0), v.clone()))
            }
            k if k.starts_with("net") && k[3..].parse::<u32>().is_ok() => {
                nets.push((k[3..].parse().unwrap_or(0), v.clone()))
            }
            k if k.starts_with("dev") && k[3..].parse::<u32>().is_ok() => {
                let m = kv(v);
                let path = m.get("").or(m.get("path")).cloned().unwrap_or_default();
                if path.starts_with("/dev/") {
                    o.devices.push(path.clone());
                    p.row(
                        k,
                        v,
                        Carry::Carried,
                        "lxc.devices",
                        if m.len() > 1 {
                            "owner/mode options are not carried"
                        } else {
                            ""
                        },
                    );
                } else {
                    p.row(k, v, Carry::NotCarried, "", "not a /dev path");
                }
            }
            "lxc.cgroup2.io.max" | "lxc.cgroup.blkio.throttle.read_bps_device" => {
                // `MAJ:MIN rbps=.. wbps=.. riops=.. wiops=..`
                let mut it = v.split_whitespace();
                let dev = it.next().unwrap_or_default().to_string();
                let mut l = LxcIoLimit::default();
                for part in it {
                    if let Some((a, b)) = part.split_once('=') {
                        let n = b.parse::<u64>().ok().filter(|n| *n > 0);
                        match a {
                            "rbps" => l.rbps = n,
                            "wbps" => l.wbps = n,
                            "riops" => l.riops = n,
                            "wiops" => l.wiops = n,
                            _ => {}
                        }
                    }
                }
                match host.io_devices.get(&dev) {
                    Some(path) if !l.limits().is_empty() => {
                        l.device = path.clone();
                        p.row(k, v, Carry::Changed, &format!("lxc.io_limits on {path}"), &format!("device {dev} on the source is {path}: the limit follows the device path (renumbered devices after a reinstall keep it)"));
                        o.io_limits.push(l);
                    }
                    _ => p.row(k, v, Carry::NotCarried, "", &format!("block device {dev} is a source-host number: give its device path in io_devices (e.g. {{\"{dev}\": \"/dev/nvme0n1\"}}) to carry the limit")),
                }
            }
            "lxc.cgroup2.devices.allow" | "lxc.cgroup.devices.allow" => allowed.push(v.clone()),
            "lxc.mount.entry" => raw_mounts.push((k.to_string(), v.clone())),
            k if k.starts_with("lxc.") => p.row(
                k,
                v,
                Carry::NotCarried,
                "",
                "raw LXC key: not copied (add the equivalent setting if still needed)",
            ),
            k if k.starts_with("unused") => p.row(
                k,
                v,
                Carry::NotCarried,
                "",
                "a detached volume: not imported",
            ),
            k if bookkeeping(k) => p.row(k, v, Carry::Ignored, "", "Proxmox bookkeeping"),
            "lock" => p.blockers.push(format!(
                "the container is locked in Proxmox ({v}): finish or clear that operation first"
            )),
            _ => p.row(k, v, Carry::NotCarried, "", "unknown key"),
        }
    }
    // raw device bind mounts (`/dev/net/tun dev/net/tun none bind,create=file`)
    for (k, v) in raw_mounts {
        let src = v.split_whitespace().next().unwrap_or_default().to_string();
        if src.starts_with("/dev/") && v.contains("bind") {
            if !o.devices.contains(&src) {
                o.devices.push(src.clone());
            }
            mounted_devices.push(src.clone());
            p.row(
                &k,
                &v,
                Carry::Changed,
                "lxc.devices",
                &format!(
                    "{src} is passed into the container (device numbers looked up on this host)"
                ),
            );
        } else {
            p.row(
                &k,
                &v,
                Carry::NotCarried,
                "",
                "raw mount entry: add it as a mount point if still needed",
            );
        }
    }
    for a in allowed {
        if !mounted_devices.is_empty() {
            p.row("lxc.cgroup2.devices.allow", &a, Carry::Changed, "lxc.devices", "covered by the passed-in device (an unprivileged container has no device policy to extend)");
        } else {
            p.row(
                "lxc.cgroup2.devices.allow",
                &a,
                Carry::NotCarried,
                "",
                "device permission without a device: not carried",
            );
        }
    }
    if !rootfs_seen {
        p.blockers.push("the container has no rootfs".into());
    }
    if o.unprivileged {
        o.idmap_base = Some(horcrux_common::lxc::LXC_DEFAULT_IDMAP_BASE);
    }

    mps.sort_by_key(|(n, _)| *n);
    for (n, v) in mps {
        let key = format!("mp{n}");
        let vol = PveVol::parse(&v);
        let path = vol.opts.get("mp").cloned().unwrap_or_default();
        let ro = vol.opts.get("ro").is_some_and(|x| on(x));
        let mut notes: Vec<String> = vec![];
        if vol.opts.get("backup").map(String::as_str) == Some("0") {
            notes.push("backup=0".into());
        }
        for k in ["acl", "quota", "replicate", "shared", "mountoptions"] {
            if let Some(x) = vol.opts.get(k) {
                notes.push(format!("{k}={x} not carried"));
            }
        }
        if vol.storage.is_none() && vol.volume.starts_with('/') {
            // bind mount: the host path is kept
            o.mounts.push(LxcMount {
                path: path.clone(),
                source: Some(vol.volume.clone()),
                volume: None,
                read_only: ro,
            });
            p.row(
                &key,
                &v,
                Carry::Carried,
                "lxc.mounts (bind, host path kept)",
                &format!(
                    "{} must exist on this host{}",
                    vol.volume,
                    if notes.is_empty() {
                        String::new()
                    } else {
                        format!("; {}", notes.join("; "))
                    }
                ),
            );
        } else {
            let r = resolve(&vol);
            if let Err(e) = &r {
                p.blockers
                    .push(format!("{key}: volume {} not found: {e}", vol.spec()));
            }
            let idx = o.mounts.len();
            o.mounts.push(LxcMount {
                path: path.clone(),
                source: None,
                volume: Some(format!("mp{n}")),
                read_only: ro,
            });
            p.row(
                &key,
                &v,
                Carry::Changed,
                "lxc.mounts (managed volume, data copied)",
                &format!(
                    "a Horcrux volume directory; size limit {} not enforced{}",
                    vol.opts.get("size").cloned().unwrap_or_default(),
                    if notes.is_empty() {
                        String::new()
                    } else {
                        format!("; {}", notes.join("; "))
                    }
                ),
            );
            p.volumes.push(PlanVol {
                role: VolRole::Mount,
                key,
                source: vol.spec(),
                resolved: r.ok(),
                size_bytes: vol.size_bytes(),
                index: Some(idx),
            });
        }
    }

    nets.sort_by_key(|(n, _)| *n);
    for (n, v) in nets {
        let key = format!("net{n}");
        let m = kv(&v);
        let src_bridge = m.get("bridge").cloned().unwrap_or_default();
        let bridge = host
            .bridge_map
            .get(&src_bridge)
            .cloned()
            .unwrap_or_else(|| src_bridge.clone());
        if !host.bridges.contains(&bridge) {
            p.blockers.push(format!("{key}: bridge {bridge} does not exist on this host: create it (Network) or map {src_bridge} to an existing bridge"));
        }
        let ip = m.get("ip").cloned();
        let ip6 = m.get("ip6").cloned();
        let net = LxcNet {
            name: m.get("name").cloned(),
            bridge: bridge.clone(),
            hwaddr: m.get("hwaddr").cloned(),
            vlan: m.get("tag").and_then(|t| t.parse().ok()),
            ip: ip.clone(),
            gateway: m
                .get("gw")
                .cloned()
                .filter(|_| ip.as_deref().is_some_and(|i| i.contains('/'))),
            ip6: ip6.clone(),
            gateway6: m
                .get("gw6")
                .cloned()
                .filter(|_| ip6.as_deref().is_some_and(|i| i.contains('/'))),
            mtu: m.get("mtu").and_then(|t| t.parse().ok()),
            rate_mbps: m.get("rate").and_then(|r| mb_s_to_mbit(r)),
        };
        let lost: Vec<&str> = m
            .keys()
            .map(String::as_str)
            .filter(|k| {
                !matches!(
                    *k,
                    "name"
                        | "bridge"
                        | "hwaddr"
                        | "tag"
                        | "ip"
                        | "gw"
                        | "ip6"
                        | "gw6"
                        | "mtu"
                        | "rate"
                        | "type"
                )
            })
            .collect();
        let mut note = format!("MAC {} kept", net.hwaddr.clone().unwrap_or_default());
        if bridge != src_bridge {
            note.push_str(&format!("; bridge {src_bridge} -> {bridge}"));
        }
        if let Some(r) = net.rate_mbps {
            note.push_str(&format!(
                "; rate {} MB/s = {r} Mbit/s",
                m.get("rate").cloned().unwrap_or_default()
            ));
        }
        if !lost.is_empty() {
            note.push_str(&format!("; not carried: {}", lost.join(", ")));
            if lost.contains(&"firewall") {
                note.push_str(" (Proxmox firewall rules: add them under Firewall)");
            }
        }
        p.row(
            &key,
            &v,
            if lost.is_empty() {
                Carry::Carried
            } else {
                Carry::Changed
            },
            "lxc.nets",
            &note,
        );
        o.nets.push(net);
    }
    if let Err(e) = o.validate() {
        p.blockers
            .push(format!("the mapped settings are not valid: {e}"));
    }
    p.ct = Some(ContainerConfig {
        id: p.target_id.clone(),
        name: p.target_id.clone(),
        runtime: ContainerRuntime::Lxc,
        memory,
        cpus: if cpus == 0 { 1 } else { cpus },
        rootfs: format!("proxmox:{vmid}"),
        status: ContainerStatus::Stopped,
        options: Default::default(),
        created_at: None,
        lxc: Some(o),
    });
    if !conf.snapshots.is_empty() {
        p.row(
            "snapshots",
            &conf.snapshots.join(", "),
            Carry::NotCarried,
            "",
            "Proxmox snapshots are not imported: the current state is",
        );
    }
    if !conf.description.is_empty() {
        p.row(
            "description",
            &conf.description,
            Carry::NotCarried,
            "",
            "Horcrux containers have no description field: kept with the saved source config",
        );
    }
    if cpus == 0 {
        p.row(
            "cores",
            "(unset)",
            Carry::Changed,
            "cpus = 1 (no CPU cap)",
            "Proxmox gives a container without cores= every host CPU",
        );
    }
    p
}

#[cfg(test)]
mod tests;
