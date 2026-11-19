//! Per-GPU capability model: which ways a GPU can be given to VMs, and why
//! the others are not available.
//!
//! * passthrough: the whole device through `vfio-pci` (needs an IOMMU);
//! * SR-IOV: the driver of the physical function (PF) exposes
//!   `sriov_totalvfs`; virtual functions (VFs) are assigned like any PCI
//!   device. For Intel `xe` PFs the per-VF resources (VRAM quota, execution
//!   quantum, preemption timeout) are read from the driver's `sriov_admin`
//!   sysfs tree;
//! * vendor vGPU: mediated-device (mdev) types the parent offers.
//!
//! VFIO migration support is **read from the device**
//! (`VFIO_DEVICE_FEATURE_MIGRATION` through its VFIO device node), never
//! inferred from a model or vendor list: a VF or mdev either answers with the
//! migration flags of its VFIO driver or it does not. A device that is not
//! bound to a VFIO driver, or that a VM currently holds open, is reported as
//! `unknown` with the reason, and the last successful answer is remembered.
//!
//! Everything works on a sysfs/devfs root (see [`HostPaths`]) so it is tested
//! against fixture trees.
use super::mdev::{MdevManager, MdevType};
use super::pci::{PciDevice, PciManager};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Where sysfs and devfs live. Defaults `/sys` and `/dev`; the environment
/// (`HORCRUX_SYSFS_ROOT`, `HORCRUX_DEVFS_ROOT`) points them at a fixture tree.
#[derive(Debug, Clone)]
pub struct HostPaths {
    pub sys: PathBuf,
    pub dev: PathBuf,
}

impl HostPaths {
    pub fn from_env() -> Self {
        let get = |k: &str, d: &str| {
            std::env::var(k)
                .ok()
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| d.to_string())
        };
        Self {
            sys: get("HORCRUX_SYSFS_ROOT", "/sys").into(),
            dev: get("HORCRUX_DEVFS_ROOT", "/dev").into(),
        }
    }

    pub fn with_roots(sys: impl Into<PathBuf>, dev: impl Into<PathBuf>) -> Self {
        Self {
            sys: sys.into(),
            dev: dev.into(),
        }
    }

    pub fn pci_dev(&self, addr: &str) -> PathBuf {
        self.sys.join("bus/pci/devices").join(addr)
    }
}

pub(crate) fn read(p: &Path) -> String {
    std::fs::read_to_string(p)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn base(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn link_name(p: &Path) -> Option<String> {
    std::fs::read_link(p).ok().map(|l| base(&l))
}

fn sorted_dir(p: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(p)
        .map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

// ------------------------------------------------------------- migration

/// VFIO migration flags (`VFIO_MIGRATION_*` from `linux/vfio.h`).
pub const VFIO_MIGRATION_STOP_COPY: u64 = 1;
pub const VFIO_MIGRATION_P2P: u64 = 1 << 1;
pub const VFIO_MIGRATION_PRE_COPY: u64 = 1 << 2;

/// Whether a VF or mediated device can be saved/migrated by the host
/// (QEMU migrate-to-file with the device state), as the device itself says.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MigrationInfo {
    /// `supported`, `unsupported` or `unknown`.
    pub state: String,
    /// Why it is unsupported or unknown (empty when supported).
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub stop_copy: bool,
    #[serde(default)]
    pub p2p: bool,
    #[serde(default)]
    pub pre_copy: bool,
}

impl MigrationInfo {
    pub fn supported(flags: u64) -> Self {
        Self {
            state: "supported".into(),
            reason: String::new(),
            stop_copy: flags & VFIO_MIGRATION_STOP_COPY != 0,
            p2p: flags & VFIO_MIGRATION_P2P != 0,
            pre_copy: flags & VFIO_MIGRATION_PRE_COPY != 0,
        }
    }
    pub fn unsupported(reason: impl Into<String>) -> Self {
        Self {
            state: "unsupported".into(),
            reason: reason.into(),
            stop_copy: false,
            p2p: false,
            pre_copy: false,
        }
    }
    pub fn unknown(reason: impl Into<String>) -> Self {
        Self {
            state: "unknown".into(),
            ..Self::unsupported(reason)
        }
    }
    /// The device supports at least stop-and-copy (the minimum every
    /// migration-capable VFIO driver must offer).
    pub fn can_save(&self) -> bool {
        self.state == "supported" && self.stop_copy
    }
}

/// Why a probe produced no flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeError {
    /// Another process (a VM) holds the VFIO device open.
    Busy,
    /// The driver does not implement the migration feature.
    NoMigration,
    /// Anything else (no iommufd, permission, ...).
    Other(String),
}

/// Reads the VFIO migration flags of the device behind a VFIO device node.
pub trait MigrationProbe: Send + Sync {
    fn flags(&self, iommufd: &Path, vfio_cdev: &Path) -> Result<u64, ProbeError>;
}

/// The real probe: bind the VFIO device node to an iommufd and issue
/// `VFIO_DEVICE_FEATURE` (`MIGRATION`, GET). Needs Linux 6.6+ (VFIO device
/// cdev) and root. It opens the device, so a device a VM holds is `Busy`.
pub struct IoctlProbe;

#[repr(C)]
struct BindIommufd {
    argsz: u32,
    flags: u32,
    iommufd: i32,
    out_devid: u32,
}

#[repr(C)]
struct FeatureMigration {
    argsz: u32,
    flags: u32,
    mig_flags: u64,
}

// _IO(VFIO_TYPE=';', VFIO_BASE(100) + n)
const VFIO_DEVICE_FEATURE: u64 = 0x3b00 | (100 + 17);
const VFIO_DEVICE_BIND_IOMMUFD: u64 = 0x3b00 | (100 + 18);
const VFIO_DEVICE_FEATURE_GET: u32 = 1 << 16;
const VFIO_DEVICE_FEATURE_MIGRATION: u32 = 1;

impl MigrationProbe for IoctlProbe {
    fn flags(&self, iommufd: &Path, cdev: &Path) -> Result<u64, ProbeError> {
        use nix::libc;
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let open = |p: &Path| -> Result<i32, ProbeError> {
            let c = CString::new(p.as_os_str().as_bytes())
                .map_err(|_| ProbeError::Other("bad path".into()))?;
            // SAFETY: plain open(2) of a NUL-terminated path.
            let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
            if fd < 0 {
                let e = std::io::Error::last_os_error();
                return Err(match e.raw_os_error() {
                    Some(libc::EBUSY) => ProbeError::Busy,
                    _ => ProbeError::Other(format!("{}: {e}", p.display())),
                });
            }
            Ok(fd)
        };
        let io = open(iommufd)?;
        let dev = match open(cdev) {
            Ok(d) => d,
            Err(e) => {
                // SAFETY: `io` is a valid fd we own.
                unsafe { libc::close(io) };
                return Err(e);
            }
        };
        let result = (|| {
            let mut bind = BindIommufd {
                argsz: std::mem::size_of::<BindIommufd>() as u32,
                flags: 0,
                iommufd: io,
                out_devid: 0,
            };
            // SAFETY: `bind` is a valid, correctly sized repr(C) argument.
            let r = unsafe { libc::ioctl(dev, VFIO_DEVICE_BIND_IOMMUFD as _, &mut bind) };
            if r < 0 {
                let e = std::io::Error::last_os_error();
                return Err(match e.raw_os_error() {
                    Some(libc::EBUSY) => ProbeError::Busy,
                    _ => ProbeError::Other(format!("binding the device to iommufd: {e}")),
                });
            }
            let mut f = FeatureMigration {
                argsz: std::mem::size_of::<FeatureMigration>() as u32,
                flags: VFIO_DEVICE_FEATURE_GET | VFIO_DEVICE_FEATURE_MIGRATION,
                mig_flags: 0,
            };
            // SAFETY: `f` is a valid vfio_device_feature + 8 byte payload.
            let r = unsafe { libc::ioctl(dev, VFIO_DEVICE_FEATURE as _, &mut f) };
            if r < 0 {
                let e = std::io::Error::last_os_error();
                return Err(match e.raw_os_error() {
                    Some(libc::ENOTTY) | Some(libc::EINVAL) | Some(libc::EOPNOTSUPP) => {
                        ProbeError::NoMigration
                    }
                    Some(libc::EBUSY) => ProbeError::Busy,
                    _ => ProbeError::Other(format!("VFIO_DEVICE_FEATURE: {e}")),
                });
            }
            Ok(f.mig_flags)
        })();
        // SAFETY: both fds are valid and owned here.
        unsafe {
            libc::close(dev);
            libc::close(io);
        }
        result
    }
}

/// Last successful answer per device (`<pci address or uuid>`), so a device a
/// running VM holds open still reports what it was found to support.
fn cache() -> &'static Mutex<HashMap<String, MigrationInfo>> {
    static C: std::sync::OnceLock<Mutex<HashMap<String, MigrationInfo>>> =
        std::sync::OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Forget remembered answers (a VF was removed or re-created).
pub fn forget_migration(paths: &HostPaths, key: &str) {
    if let Ok(mut c) = cache().lock() {
        c.remove(&ckey(paths, key));
    }
}

fn ckey(paths: &HostPaths, key: &str) -> String {
    format!("{}|{key}", paths.sys.display())
}

/// The VFIO device node of a sysfs device (`<dev>/vfio-dev/vfioN`).
fn vfio_cdev(paths: &HostPaths, devdir: &Path) -> Option<PathBuf> {
    let name = sorted_dir(&devdir.join("vfio-dev"))
        .into_iter()
        .map(|p| base(&p))
        .find(|n| n.starts_with("vfio"))?;
    Some(paths.dev.join("vfio/devices").join(name))
}

/// VFIO migration capability of a PCI function (a VF) or mediated device
/// (`key` = PCI address or mdev UUID, lower case).
pub fn migration_of(paths: &HostPaths, probe: &dyn MigrationProbe, key: &str) -> MigrationInfo {
    let is_mdev = !key.contains(':');
    let devdir = if is_mdev {
        paths.sys.join("bus/mdev/devices").join(key)
    } else {
        paths.pci_dev(key)
    };
    if !devdir.exists() {
        return MigrationInfo::unknown("device does not exist");
    }
    if !is_mdev {
        match link_name(&devdir.join("driver")) {
            None => return remembered(paths, key).unwrap_or_else(|| {
                MigrationInfo::unknown(
                    "not bound to a VFIO driver: bind it to vfio-pci (or the vendor VFIO driver, \
                         e.g. xe-vfio-pci) so its migration support can be read",
                )
            }),
            Some(d) if !d.contains("vfio") => {
                return remembered(paths, key).unwrap_or_else(|| {
                    MigrationInfo::unknown(format!(
                        "bound to the host driver {d}: migration support is read once it is bound \
                         to a VFIO driver"
                    ))
                })
            }
            Some(_) => {}
        }
    }
    let Some(cdev) = vfio_cdev(paths, &devdir) else {
        return remembered(paths, key).unwrap_or_else(|| {
            MigrationInfo::unknown(
                "the kernel exposes no VFIO device node for it (needs Linux 6.6+ with \
                 CONFIG_VFIO_DEVICE_CDEV)",
            )
        });
    };
    let info = match probe.flags(&paths.dev.join("iommu"), &cdev) {
        Ok(flags) if flags & VFIO_MIGRATION_STOP_COPY != 0 => MigrationInfo::supported(flags),
        Ok(_) => MigrationInfo::unsupported("the driver reports no stop-and-copy migration"),
        Err(ProbeError::NoMigration) => {
            let drv = if is_mdev {
                "its vendor driver".to_string()
            } else {
                link_name(&devdir.join("driver")).unwrap_or_default()
            };
            MigrationInfo::unsupported(format!(
                "{drv} does not implement VFIO device migration (a VM using it cannot be saved on \
                 the host)"
            ))
        }
        Err(ProbeError::Busy) => {
            return remembered(paths, key).unwrap_or_else(|| {
                MigrationInfo::unknown(
                    "a running VM holds the device; migration support is read while it is idle",
                )
            })
        }
        Err(ProbeError::Other(e)) => {
            return remembered(paths, key)
                .unwrap_or_else(|| MigrationInfo::unknown(format!("cannot read it: {e}")))
        }
    };
    if let Ok(mut c) = cache().lock() {
        c.insert(ckey(paths, key), info.clone());
    }
    info
}

fn remembered(paths: &HostPaths, key: &str) -> Option<MigrationInfo> {
    cache()
        .lock()
        .ok()
        .and_then(|c| c.get(&ckey(paths, key)).cloned())
}

// ------------------------------------------------------------ capabilities

/// One way (or not) to use a GPU.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModeStatus {
    pub available: bool,
    /// Why not (empty when available).
    #[serde(default)]
    pub reason: String,
}

impl ModeStatus {
    fn yes() -> Self {
        Self {
            available: true,
            reason: String::new(),
        }
    }
    fn no(reason: impl Into<String>) -> Self {
        Self {
            available: false,
            reason: reason.into(),
        }
    }
}

/// A virtual function of an SR-IOV GPU.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VfInfo {
    /// 1-based, as in the driver's `sriov_admin/vfN`.
    pub vf: u32,
    pub address: String,
    pub driver: Option<String>,
    pub iommu_group: Option<String>,
    /// VRAM given to this VF, MiB, where the driver exposes it.
    pub vram_mib: Option<u64>,
    pub exec_quantum_ms: Option<u64>,
    pub preempt_timeout_us: Option<u64>,
    pub migration: MigrationInfo,
    /// VM that lists this VF in its hardware (filled by the API).
    #[serde(default)]
    pub vm: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SriovCap {
    /// PF driver (`xe`, `nvidia`, `amdgpu`, ...).
    pub driver: String,
    pub total_vfs: u32,
    pub current_vfs: u32,
    /// `sriov_drivers_autoprobe`: VFs get a host driver when created.
    pub autoprobe: Option<bool>,
    /// The PF driver exposes per-VF resources (`sriov_admin`): Horcrux sets
    /// VRAM quota and scheduling per VF.
    pub per_vf_resources: bool,
    /// VFIO drivers a VF can be bound to here.
    pub vf_drivers: Vec<String>,
    pub vfs: Vec<VfInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MdevInstanceInfo {
    pub uuid: String,
    pub mdev_type: String,
    pub migration: MigrationInfo,
    #[serde(default)]
    pub vm: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VgpuCap {
    pub types: Vec<MdevTypeInfo>,
    pub devices: Vec<MdevInstanceInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MdevTypeInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub available_instances: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GpuCapability {
    pub address: String,
    pub vendor_id: String,
    pub vendor_name: String,
    pub device_id: String,
    pub driver: Option<String>,
    pub iommu_group: Option<String>,
    pub passthrough: ModeStatus,
    pub sriov: Option<SriovCap>,
    pub vendor_vgpu: Option<VgpuCap>,
    /// One line per mode that is not available, with the reason.
    pub why_not: Vec<String>,
}

/// Reason a PF offers no SR-IOV (vendor-specific wording, no model lists).
pub fn sriov_refusal(vendor_id: &str, driver: Option<&str>) -> String {
    match (vendor_id, driver) {
        ("8086", Some("xe")) => "Intel GPU on the xe driver exposes no SR-IOV: this platform or \
            device is not SR-IOV capable for xe, or SR-IOV is disabled in the firmware (needs \
            Linux 6.17+)"
            .into(),
        ("8086", Some("i915")) => "Intel GPU on the i915 driver: SR-IOV PF support lives in xe \
            (Linux 6.17+) on supported platforms; bind the device to xe to use it"
            .into(),
        ("8086", _) => "Intel GPU without a bound xe driver: SR-IOV needs the xe driver \
            (Linux 6.17+)"
            .into(),
        ("10de", _) => "this NVIDIA GPU exposes no SR-IOV virtual functions (vGPU is offered on \
            licensed datacenter and professional cards); passthrough only"
            .into(),
        ("1002", _) | ("1022", _) => "this AMD GPU exposes no SR-IOV virtual functions \
            (MxGPU is offered on Instinct and Radeon PRO V-series cards); passthrough only"
            .into(),
        _ => "the device exposes no SR-IOV capability".into(),
    }
}

/// Collects capabilities for every GPU on the host.
pub struct GpuCaps<'a> {
    pub paths: HostPaths,
    pub probe: &'a dyn MigrationProbe,
}

fn vfio_drivers(paths: &HostPaths) -> Vec<String> {
    let dir = paths.sys.join("bus/pci/drivers");
    let mut v = Vec::new();
    for want in ["vfio-pci", "xe-vfio-pci", "xe_vfio_pci"] {
        if dir.join(want).exists() {
            v.push(want.replace('_', "-"));
        }
    }
    v.dedup();
    v
}

impl<'a> GpuCaps<'a> {
    pub fn new(paths: HostPaths, probe: &'a dyn MigrationProbe) -> Self {
        Self { paths, probe }
    }

    fn pci(&self) -> PciManager {
        PciManager::with_sysfs(&self.paths.sys)
    }

    /// Every GPU (PCI display class, excluding VFs, which are listed under
    /// their PF).
    pub fn collect(&self) -> Vec<GpuCapability> {
        let pci = self.pci();
        let devices = pci.list();
        let iommu = pci.iommu_groups() > 0;
        let mdev = MdevManager::with_sysfs(&self.paths.sys);
        let types = mdev.list_types();
        let instances = mdev.list_devices();
        devices
            .iter()
            .filter(|d| d.class_name == "gpu" && d.physfn.is_none())
            .map(|d| self.one(d, iommu, &types, &instances))
            .collect()
    }

    fn one(
        &self,
        d: &PciDevice,
        iommu: bool,
        types: &[MdevType],
        instances: &[super::mdev::MdevDevice],
    ) -> GpuCapability {
        let mut why_not = Vec::new();
        let passthrough = if d.iommu_group.is_some() && iommu {
            ModeStatus::yes()
        } else {
            let r = "no IOMMU group: enable the IOMMU in firmware and the kernel \
                     (intel_iommu=on / amd_iommu=on)";
            ModeStatus::no(r)
        };
        if !passthrough.available {
            why_not.push(format!("passthrough: {}", passthrough.reason));
        }

        let sriov = if d.sriov_max_vfs > 0 {
            Some(self.sriov(d))
        } else {
            why_not.push(format!(
                "SR-IOV: {}",
                sriov_refusal(&d.vendor_id, d.driver.as_deref())
            ));
            None
        };

        let mine: Vec<&MdevType> = types.iter().filter(|t| t.parent == d.address).collect();
        let vendor_vgpu = if mine.is_empty() {
            let r = if d.sriov_max_vfs > 0 {
                "vendor vGPU is provided through the SR-IOV virtual functions of this device, not \
                 mediated devices"
            } else {
                "the device offers no mediated-device (vGPU) types: the host driver has no vGPU \
                 support loaded for it"
            };
            why_not.push(format!("vendor vGPU: {r}"));
            None
        } else {
            Some(VgpuCap {
                types: mine
                    .iter()
                    .map(|t| MdevTypeInfo {
                        id: t.id.clone(),
                        name: t.name.clone(),
                        description: t.description.clone(),
                        available_instances: t.available_instances,
                    })
                    .collect(),
                devices: instances
                    .iter()
                    .filter(|i| i.parent == d.address)
                    .map(|i| MdevInstanceInfo {
                        uuid: i.uuid.clone(),
                        mdev_type: i.mdev_type.clone(),
                        migration: migration_of(&self.paths, self.probe, &i.uuid),
                        vm: None,
                    })
                    .collect(),
            })
        };

        GpuCapability {
            address: d.address.clone(),
            vendor_id: d.vendor_id.clone(),
            vendor_name: d.vendor_name.clone(),
            device_id: d.device_id.clone(),
            driver: d.driver.clone(),
            iommu_group: d.iommu_group.clone(),
            passthrough,
            sriov,
            vendor_vgpu,
            why_not,
        }
    }

    fn sriov(&self, pf: &PciDevice) -> SriovCap {
        let dir = self.paths.pci_dev(&pf.address);
        let admin = dir.join("sriov_admin");
        let autoprobe = match read(&dir.join("sriov_drivers_autoprobe")).as_str() {
            "1" => Some(true),
            "0" => Some(false),
            _ => None,
        };
        // virtfnN links -> VF PCI addresses
        let mut links: Vec<(u32, PathBuf)> = sorted_dir(&dir)
            .into_iter()
            .filter_map(|p| {
                let n = base(&p).strip_prefix("virtfn")?.parse::<u32>().ok()?;
                Some((n, p))
            })
            .collect();
        links.sort();
        let pci = self.pci();
        let mut vfs = Vec::new();
        for (n, link) in links {
            let addr = std::fs::read_link(&link)
                .ok()
                .map(|l| base(&l))
                .unwrap_or_default();
            if addr.is_empty() {
                continue;
            }
            let vf = n + 1;
            let prof = admin.join(format!("vf{vf}/profile"));
            let num = |f: &str| read(&prof.join(f)).parse::<u64>().ok();
            let dev = pci.list().into_iter().find(|x| x.address == addr);
            vfs.push(VfInfo {
                vf,
                driver: dev.as_ref().and_then(|x| x.driver.clone()),
                iommu_group: dev.as_ref().and_then(|x| x.iommu_group.clone()),
                vram_mib: num("vram_quota").map(|b| b / (1024 * 1024)),
                exec_quantum_ms: num("exec_quantum_ms"),
                preempt_timeout_us: num("preempt_timeout_us"),
                migration: migration_of(&self.paths, self.probe, &addr),
                address: addr,
                vm: None,
            });
        }
        SriovCap {
            driver: pf.driver.clone().unwrap_or_default(),
            total_vfs: pf.sriov_max_vfs,
            current_vfs: pf.sriov_vfs,
            autoprobe,
            per_vf_resources: admin.is_dir(),
            vf_drivers: vfio_drivers(&self.paths),
            vfs,
        }
    }
}

/// What one VM's passthrough devices mean for saving it on the host.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceSaveInfo {
    pub device: String,
    /// `vf`, `mdev` or `pci` (whole device).
    pub kind: String,
    pub migration: MigrationInfo,
}

/// The save verdict for a VM's host devices: `host save` only when every
/// device is a VF/mdev that reports VFIO migration; otherwise the reason.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VmDeviceSave {
    pub devices: Vec<DeviceSaveInfo>,
    /// True when no device blocks a host-side save.
    pub host_save: bool,
    /// Human text for the VM page: `host save (VFIO migration supported)` or
    /// `not saveable: <reason>`.
    pub summary: String,
}

/// Evaluate a VM's `hostpci` list. `devices` are the raw device strings.
pub fn vm_device_save(
    paths: &HostPaths,
    probe: &dyn MigrationProbe,
    devices: &[String],
) -> VmDeviceSave {
    let mut out = Vec::new();
    for raw in devices {
        let key = raw.trim().to_ascii_lowercase();
        let key = if !key.contains(':') || key.matches(':').count() == 2 {
            key
        } else {
            format!("0000:{key}")
        };
        let is_mdev = !key.contains(':');
        let dir = if is_mdev {
            paths.sys.join("bus/mdev/devices").join(&key)
        } else {
            paths.pci_dev(&key)
        };
        let is_vf = !is_mdev && dir.join("physfn").exists();
        if is_mdev || is_vf {
            out.push(DeviceSaveInfo {
                kind: if is_mdev { "mdev" } else { "vf" }.into(),
                migration: migration_of(paths, probe, &key),
                device: key,
            });
        } else {
            out.push(DeviceSaveInfo {
                kind: "pci".into(),
                migration: MigrationInfo::unsupported(
                    "whole-device passthrough has no host-side state capture (only SR-IOV VFs and \
                     mediated devices with VFIO migration can be saved on the host)",
                ),
                device: key,
            });
        }
    }
    let blocker = out.iter().find(|d| !d.migration.can_save());
    let (host_save, summary) = match blocker {
        None if out.is_empty() => (true, "no host devices".to_string()),
        None => (true, "host save (VFIO migration supported)".to_string()),
        Some(b) => (
            false,
            format!("not saveable: {}: {}", b.device, b.migration.reason),
        ),
    };
    VmDeviceSave {
        devices: out,
        host_save,
        summary,
    }
}

/// Capabilities keyed by PF address (convenience for tests and the API).
pub fn by_address(caps: Vec<GpuCapability>) -> BTreeMap<String, GpuCapability> {
    caps.into_iter().map(|c| (c.address.clone(), c)).collect()
}

#[cfg(test)]
pub(crate) mod fixture {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    /// A probe answering from a table keyed by the VFIO node name.
    pub struct FakeProbe(pub std::collections::HashMap<String, Result<u64, ProbeError>>);
    impl MigrationProbe for FakeProbe {
        fn flags(&self, _iommufd: &Path, cdev: &Path) -> Result<u64, ProbeError> {
            self.0
                .get(&base(cdev))
                .cloned()
                .unwrap_or(Err(ProbeError::NoMigration))
        }
    }

    pub fn w(p: &Path, v: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, v).unwrap();
    }

    /// A PCI function with class/vendor/device/driver/IOMMU group.
    pub fn dev(r: &Path, addr: &str, vendor: &str, class: &str, driver: Option<&str>, group: &str) {
        let d = r.join("bus/pci/devices").join(addr);
        fs::create_dir_all(&d).unwrap();
        w(&d.join("vendor"), vendor);
        w(&d.join("device"), "0xe20b");
        w(&d.join("class"), class);
        let g = r.join("kernel/iommu_groups").join(group);
        fs::create_dir_all(g.join("devices")).unwrap();
        symlink(&g, d.join("iommu_group")).unwrap();
        symlink(&d, g.join("devices").join(addr)).unwrap();
        if let Some(drv) = driver {
            let dd = r.join("bus/pci/drivers").join(drv);
            fs::create_dir_all(&dd).unwrap();
            symlink(&dd, d.join("driver")).unwrap();
        }
    }

    /// Give a VFIO device node (`vfio0`) to a sysfs device dir + devfs.
    pub fn vfio_node(sys_dev: &Path, dev_root: &Path, name: &str) {
        fs::create_dir_all(sys_dev.join("vfio-dev").join(name)).unwrap();
        w(&dev_root.join("vfio/devices").join(name), "");
    }

    /// An Intel xe PF with `n` VFs: virtfn links, sriov_admin profiles, VFs
    /// bound to `vf_driver` with VFIO nodes `vfio1..`.
    pub fn xe_pf(r: &Path, devroot: &Path, pf: &str, total: u32, n: u32, vf_driver: &str) {
        dev(r, pf, "0x8086", "0x030000", Some("xe"), "10");
        let d = r.join("bus/pci/devices").join(pf);
        w(&d.join("sriov_totalvfs"), &format!("{total}\n"));
        w(&d.join("sriov_numvfs"), &format!("{n}\n"));
        w(&d.join("sriov_drivers_autoprobe"), "0\n");
        for i in 0..total {
            w(
                &d.join(format!("sriov_admin/vf{}/profile/vram_quota", i + 1)),
                "0\n",
            );
        }
        for i in 0..n {
            let a = format!("0000:03:00.{}", i + 1);
            dev(
                r,
                &a,
                "0x8086",
                "0x030000",
                Some(vf_driver),
                &format!("2{i}"),
            );
            let vd = r.join("bus/pci/devices").join(&a);
            symlink(&d, vd.join("physfn")).unwrap();
            symlink(&vd, d.join(format!("virtfn{i}"))).unwrap();
            w(
                &d.join(format!("sriov_admin/vf{}/profile/vram_quota", i + 1)),
                "4294967296\n",
            );
            w(
                &d.join(format!("sriov_admin/vf{}/profile/exec_quantum_ms", i + 1)),
                "16\n",
            );
            vfio_node(&vd, devroot, &format!("vfio{}", i + 1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::*;
    use super::*;
    use std::collections::HashMap;
    use tempfile::tempdir;

    fn paths(t: &tempfile::TempDir) -> HostPaths {
        HostPaths::with_roots(t.path().join("sys"), t.path().join("dev"))
    }

    #[test]
    fn xe_pf_reports_vfs_vram_and_migration_read_from_the_device() {
        let t = tempdir().unwrap();
        let p = paths(&t);
        xe_pf(&p.sys, &p.dev, "0000:03:00.0", 7, 2, "xe-vfio-pci");
        std::fs::create_dir_all(p.sys.join("bus/pci/drivers/xe-vfio-pci")).unwrap();
        let probe = FakeProbe(HashMap::from([
            (
                "vfio1".to_string(),
                Ok(VFIO_MIGRATION_STOP_COPY | VFIO_MIGRATION_P2P),
            ),
            ("vfio2".to_string(), Err(ProbeError::NoMigration)),
        ]));
        let caps = GpuCaps::new(p, &probe).collect();
        assert_eq!(caps.len(), 1, "VFs are not listed as GPUs of their own");
        let g = &caps[0];
        assert!(g.passthrough.available);
        let s = g.sriov.as_ref().unwrap();
        assert_eq!((s.total_vfs, s.current_vfs), (7, 2));
        assert!(s.per_vf_resources);
        assert_eq!(s.autoprobe, Some(false));
        assert!(s.vf_drivers.contains(&"xe-vfio-pci".to_string()));
        assert_eq!(s.vfs.len(), 2);
        assert_eq!(s.vfs[0].vf, 1);
        assert_eq!(s.vfs[0].address, "0000:03:00.1");
        assert_eq!(s.vfs[0].vram_mib, Some(4096));
        assert_eq!(s.vfs[0].exec_quantum_ms, Some(16));
        let m0 = &s.vfs[0].migration;
        assert!(m0.can_save() && m0.p2p && !m0.pre_copy, "{m0:?}");
        assert_eq!(s.vfs[1].migration.state, "unsupported");
        assert!(s.vfs[1].migration.reason.contains("does not implement"));
        // no mdev types: the reason is listed
        assert!(g.why_not.iter().any(|w| w.starts_with("vendor vGPU:")));
        assert!(!g.why_not.iter().any(|w| w.starts_with("SR-IOV")));
    }

    #[test]
    fn consumer_nvidia_explains_why_only_passthrough() {
        let t = tempdir().unwrap();
        let p = paths(&t);
        dev(
            &p.sys,
            "0000:01:00.0",
            "0x10de",
            "0x030000",
            Some("nvidia"),
            "1",
        );
        let probe = FakeProbe(HashMap::new());
        let g = GpuCaps::new(p, &probe).collect().remove(0);
        assert!(g.passthrough.available);
        assert!(g.sriov.is_none() && g.vendor_vgpu.is_none());
        assert!(g
            .why_not
            .iter()
            .any(|w| w.contains("SR-IOV") && w.contains("passthrough only")));
    }

    #[test]
    fn intel_on_i915_points_at_xe_and_no_iommu_blocks_passthrough() {
        let t = tempdir().unwrap();
        let p = paths(&t);
        let d = p.sys.join("bus/pci/devices/0000:00:02.0");
        std::fs::create_dir_all(&d).unwrap();
        w(&d.join("vendor"), "0x8086");
        w(&d.join("device"), "0x46a6");
        w(&d.join("class"), "0x030000");
        let probe = FakeProbe(HashMap::new());
        let g = GpuCaps::new(p, &probe).collect().remove(0);
        assert!(!g.passthrough.available);
        assert!(g.why_not[0].starts_with("passthrough: no IOMMU group"));
        assert!(g.why_not.iter().any(|w| w.contains("Intel GPU without")));
    }

    #[test]
    fn mdev_types_and_instances_with_migration() {
        let t = tempdir().unwrap();
        let p = paths(&t);
        dev(
            &p.sys,
            "0000:41:00.0",
            "0x10de",
            "0x030200",
            Some("nvidia"),
            "5",
        );
        let par = p
            .sys
            .join("class/mdev_bus/0000:41:00.0/mdev_supported_types/nvidia-256");
        w(&par.join("name"), "GRID A100-4C");
        w(&par.join("description"), "num_heads=1");
        w(&par.join("available_instances"), "3");
        w(&par.join("device_api"), "vfio-pci");
        let probe = FakeProbe(HashMap::new());
        let g = GpuCaps::new(p, &probe).collect().remove(0);
        let v = g.vendor_vgpu.unwrap();
        assert_eq!(v.types[0].name, "GRID A100-4C");
        assert_eq!(v.types[0].available_instances, 3);
        assert!(g.why_not.iter().any(|w| w.starts_with("SR-IOV")));
    }

    #[test]
    fn unbound_vf_and_busy_device_say_why_and_busy_uses_the_last_answer() {
        let t = tempdir().unwrap();
        let p = paths(&t);
        xe_pf(&p.sys, &p.dev, "0000:03:00.0", 2, 1, "xe-vfio-pci");
        // unbound VF
        std::fs::remove_file(p.sys.join("bus/pci/devices/0000:03:00.1/driver")).unwrap();
        let none = FakeProbe(HashMap::new());
        let m = migration_of(&p, &none, "0000:03:00.1");
        assert_eq!(m.state, "unknown");
        assert!(m.reason.contains("not bound to a VFIO driver"));
        // bound to a host driver
        let dd = p.sys.join("bus/pci/drivers/xe");
        std::fs::remove_file(p.sys.join("bus/pci/devices/0000:03:00.1/driver")).ok();
        std::os::unix::fs::symlink(&dd, p.sys.join("bus/pci/devices/0000:03:00.1/driver")).unwrap();
        let m = migration_of(&p, &none, "0000:03:00.1");
        assert!(m.reason.contains("host driver xe"), "{m:?}");
        // vfio-bound: supported, then busy -> remembered
        std::fs::remove_file(p.sys.join("bus/pci/devices/0000:03:00.1/driver")).unwrap();
        let vd = p.sys.join("bus/pci/drivers/xe-vfio-pci");
        std::fs::create_dir_all(&vd).unwrap();
        std::os::unix::fs::symlink(&vd, p.sys.join("bus/pci/devices/0000:03:00.1/driver")).unwrap();
        forget_migration(&p, "0000:03:00.1");
        let ok = FakeProbe(HashMap::from([(
            "vfio1".to_string(),
            Ok(VFIO_MIGRATION_STOP_COPY),
        )]));
        assert!(migration_of(&p, &ok, "0000:03:00.1").can_save());
        let busy = FakeProbe(HashMap::from([(
            "vfio1".to_string(),
            Err(ProbeError::Busy),
        )]));
        assert!(migration_of(&p, &busy, "0000:03:00.1").can_save());
        forget_migration(&p, "0000:03:00.1");
        let m = migration_of(&p, &busy, "0000:03:00.1");
        assert_eq!(m.state, "unknown");
        assert!(m.reason.contains("running VM holds"));
    }

    #[test]
    fn vm_save_verdict_names_the_blocking_device() {
        let t = tempdir().unwrap();
        let p = paths(&t);
        xe_pf(&p.sys, &p.dev, "0000:03:00.0", 2, 1, "xe-vfio-pci");
        forget_migration(&p, "0000:03:00.1");
        let ok = FakeProbe(HashMap::from([(
            "vfio1".to_string(),
            Ok(VFIO_MIGRATION_STOP_COPY),
        )]));
        let v = vm_device_save(&p, &ok, &["03:00.1".to_string()]);
        assert!(v.host_save, "{v:?}");
        assert_eq!(v.summary, "host save (VFIO migration supported)");
        assert_eq!(v.devices[0].kind, "vf");
        // the PF itself is a whole device
        let v = vm_device_save(&p, &ok, &["0000:03:00.1".into(), "0000:03:00.0".into()]);
        assert!(!v.host_save);
        assert!(
            v.summary.starts_with("not saveable: 0000:03:00.0"),
            "{}",
            v.summary
        );
        assert_eq!(vm_device_save(&p, &ok, &[]).summary, "no host devices");
    }
}
