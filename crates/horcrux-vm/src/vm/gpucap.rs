//! Can a VM that owns VFIO devices be saved to a file (host save at shutdown)?
//!
//! QEMU can stream the state of a VFIO device into a migration stream only
//! when the device's kernel driver implements VFIO device migration v2
//! (`VFIO_DEVICE_FEATURE_MIGRATION`): SR-IOV virtual functions bound to a
//! vendor variant driver, vGPU/mediated devices whose parent driver offers
//! it. A plain `vfio-pci` device (a whole GPU) has no such interface, so a
//! VM owning one cannot be saved this way.
//!
//! The answer is read from the kernel, not guessed from vendor names: the
//! device's `vfio-dev` node (sysfs) names its character device, and the
//! migration feature is asked of it with the VFIO ioctls (iommufd device
//! interface, or the legacy group interface on older kernels). Everything
//! that touches the host goes through [`Env`] (sysfs root, `/dev` root and a
//! [`VfioFeature`] implementation), so the logic is tested on a fake tree.
//!
//! A device that a running QEMU already holds cannot be probed again (the
//! kernel answers EBUSY): the probe made just before the VM started is kept
//! next to the disk ([`cache_path`]) and used instead; when there is none
//! the verdict is `unverified` and QEMU itself decides at save time (it
//! refuses a save it cannot do without touching the guest).
//!
//! After a host reboot the devices a saved VM used (mediated devices, SR-IOV
//! virtual functions) no longer exist: [`recipes_of`] records how to make
//! them again at save time and [`restore_devices`] does it before the resume.
use horcrux_common::{Error, HostPciKind, Result, VmHostPci};
use serde::{Deserialize, Serialize};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};

/// Node setting: may a VM with VFIO devices be saved at all?
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DeviceSaveMode {
    /// Save when every device reports VFIO migration support (or cannot be
    /// asked: QEMU then decides); refuse when one reports none.
    #[default]
    Auto,
    /// Never save a VM that owns host devices.
    Never,
}

/// What the kernel says about one device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Migration {
    /// The device's driver implements VFIO migration v2.
    Supported {
        stop_copy: bool,
        p2p: bool,
        pre_copy: bool,
    },
    /// The device cannot be saved (`code` is stable, `reason` is for people).
    Unsupported { code: String, reason: String },
    /// The kernel could not be asked (device in use, no VFIO interface...).
    Unverified { code: String, reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceCap {
    /// The `hostpci` device string (PCI address or mediated device UUID).
    pub device: String,
    /// `pci` or `mdev`.
    pub kind: String,
    /// Kernel driver bound to the device (PCI) or to its parent (mediated).
    pub driver: Option<String>,
    /// `vfioN` name under the device's `vfio-dev` directory.
    pub vfio_name: Option<String>,
    pub migration: Migration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// The VM owns no host device: the usual rules apply.
    NotApplicable,
    /// Every device reports migration support.
    Possible,
    /// At least one device could not be asked; QEMU decides at save time.
    Unverified,
    /// At least one device cannot be saved, or the node setting forbids it.
    Impossible,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceSaveReport {
    pub verdict: Verdict,
    /// Stable machine-readable code of the blocking/unverified reason.
    pub code: Option<String>,
    /// Why the state cannot be saved (or could not be verified).
    pub reason: Option<String>,
    pub devices: Vec<DeviceCap>,
}

impl DeviceSaveReport {
    pub fn not_applicable() -> Self {
        Self {
            verdict: Verdict::NotApplicable,
            code: None,
            reason: None,
            devices: Vec::new(),
        }
    }
    /// May a save be attempted?
    pub fn may_save(&self) -> bool {
        self.verdict != Verdict::Impossible
    }
}

// ------------------------------------------------------------ kernel side

/// What identifies a device to the VFIO interfaces.
#[derive(Debug, Clone)]
pub struct DeviceNode {
    /// `vfioN`.
    pub vfio_name: String,
    /// `/dev/vfio/devices/vfioN` (iommufd device interface).
    pub cdev: PathBuf,
    /// IOMMU group number (legacy interface).
    pub group: Option<u32>,
    /// Name the group interface knows the device by (PCI address / UUID).
    pub dev_name: String,
    /// `/dev/iommu`.
    pub iommufd: PathBuf,
    /// `/dev/vfio/vfio` (legacy container).
    pub container: PathBuf,
    /// `/dev/vfio/<group>`.
    pub group_node: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeErr {
    /// The driver has no migration interface (ENOTTY / EINVAL).
    NotSupported,
    /// Another owner holds the device (a running VM): EBUSY.
    Busy,
    /// Neither VFIO interface exists / is readable here.
    NoInterface(String),
    Other(String),
}

/// Asks a device for its VFIO migration flags (`VFIO_MIGRATION_*`).
pub trait VfioFeature: Send + Sync {
    fn migration_flags(&self, node: &DeviceNode) -> std::result::Result<u64, ProbeErr>;
}

pub const MIG_STOP_COPY: u64 = 1 << 0;
pub const MIG_P2P: u64 = 1 << 1;
pub const MIG_PRE_COPY: u64 = 1 << 2;

// <linux/vfio.h>: VFIO_TYPE ';', VFIO_BASE 100.
const VFIO_TYPE: u32 = b';' as u32;
const fn io(nr: u32) -> u64 {
    // _IO(type, nr): no direction, no size.
    ((VFIO_TYPE << 8) | nr) as u64
}
const VFIO_SET_IOMMU: u64 = io(100 + 2);
const VFIO_GROUP_SET_CONTAINER: u64 = io(100 + 4);
const VFIO_GROUP_GET_DEVICE_FD: u64 = io(100 + 6);
const VFIO_DEVICE_FEATURE: u64 = io(100 + 17);
const VFIO_DEVICE_BIND_IOMMUFD: u64 = io(100 + 18);
const VFIO_TYPE1V2_IOMMU: u64 = 3;
const VFIO_DEVICE_FEATURE_GET: u32 = 1 << 16;
const VFIO_DEVICE_FEATURE_MIGRATION: u32 = 1;

/// `struct vfio_device_feature` + `struct vfio_device_feature_migration`.
#[repr(C)]
struct FeatureMigration {
    argsz: u32,
    flags: u32,
    mig_flags: u64,
}

/// `struct vfio_device_bind_iommufd`.
#[repr(C)]
struct BindIommufd {
    argsz: u32,
    flags: u32,
    iommufd: i32,
    out_devid: u32,
}

/// The real thing: ioctls on the device nodes.
pub struct KernelVfio;

fn errno(e: &std::io::Error) -> ProbeErr {
    match e.raw_os_error() {
        Some(nix::libc::ENOTTY) | Some(nix::libc::EINVAL) | Some(nix::libc::EOPNOTSUPP) => {
            ProbeErr::NotSupported
        }
        Some(nix::libc::EBUSY) => ProbeErr::Busy,
        Some(nix::libc::ENOENT) | Some(nix::libc::ENODEV) => ProbeErr::NoInterface(e.to_string()),
        _ => ProbeErr::Other(e.to_string()),
    }
}

fn open_rw(p: &Path) -> std::io::Result<OwnedFd> {
    let f = std::fs::OpenOptions::new().read(true).write(true).open(p)?;
    Ok(f.into())
}

fn ioctl_arg<T>(fd: &OwnedFd, req: u64, arg: &mut T) -> std::io::Result<i32> {
    // SAFETY: `arg` is a live, correctly sized #[repr(C)] struct for `req`.
    let r = unsafe { nix::libc::ioctl(fd.as_raw_fd(), req as _, arg as *mut T) };
    if r < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(r)
    }
}

fn ioctl_val(fd: &OwnedFd, req: u64, val: u64) -> std::io::Result<i32> {
    // SAFETY: plain integer argument.
    let r = unsafe { nix::libc::ioctl(fd.as_raw_fd(), req as _, val as nix::libc::c_ulong) };
    if r < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(r)
    }
}

fn ask_migration(dev: &OwnedFd) -> std::result::Result<u64, ProbeErr> {
    let mut f = FeatureMigration {
        argsz: std::mem::size_of::<FeatureMigration>() as u32,
        flags: VFIO_DEVICE_FEATURE_GET | VFIO_DEVICE_FEATURE_MIGRATION,
        mig_flags: 0,
    };
    ioctl_arg(dev, VFIO_DEVICE_FEATURE, &mut f).map_err(|e| errno(&e))?;
    Ok(f.mig_flags)
}

impl KernelVfio {
    fn via_iommufd(n: &DeviceNode) -> std::result::Result<u64, ProbeErr> {
        let iommufd = open_rw(&n.iommufd).map_err(|e| errno(&e))?;
        let dev = open_rw(&n.cdev).map_err(|e| errno(&e))?;
        let mut b = BindIommufd {
            argsz: std::mem::size_of::<BindIommufd>() as u32,
            flags: 0,
            iommufd: iommufd.as_raw_fd(),
            out_devid: 0,
        };
        ioctl_arg(&dev, VFIO_DEVICE_BIND_IOMMUFD, &mut b).map_err(|e| errno(&e))?;
        ask_migration(&dev)
    }

    fn via_group(n: &DeviceNode) -> std::result::Result<u64, ProbeErr> {
        let gnode = n
            .group_node
            .as_ref()
            .ok_or_else(|| ProbeErr::NoInterface("the device has no IOMMU group".into()))?;
        let container = open_rw(&n.container).map_err(|e| errno(&e))?;
        let group = open_rw(gnode).map_err(|e| errno(&e))?;
        ioctl_val(
            &group,
            VFIO_GROUP_SET_CONTAINER,
            container.as_raw_fd() as u64,
        )
        .map_err(|e| errno(&e))?;
        ioctl_val(&container, VFIO_SET_IOMMU, VFIO_TYPE1V2_IOMMU).map_err(|e| errno(&e))?;
        let name = std::ffi::CString::new(n.dev_name.clone())
            .map_err(|_| ProbeErr::Other("bad device name".into()))?;
        // SAFETY: `name` is a NUL-terminated string that outlives the call.
        let fd = unsafe {
            nix::libc::ioctl(
                group.as_raw_fd(),
                VFIO_GROUP_GET_DEVICE_FD as _,
                name.as_ptr(),
            )
        };
        if fd < 0 {
            return Err(errno(&std::io::Error::last_os_error()));
        }
        // SAFETY: the kernel just returned a new descriptor that we own.
        let dev = unsafe { OwnedFd::from_raw_fd(fd) };
        ask_migration(&dev)
    }
}

impl VfioFeature for KernelVfio {
    fn migration_flags(&self, n: &DeviceNode) -> std::result::Result<u64, ProbeErr> {
        if n.iommufd.exists() && n.cdev.exists() {
            match Self::via_iommufd(n) {
                // The device interface exists but this kernel cannot serve
                // the request: try the group interface before giving up.
                Err(ProbeErr::NoInterface(_)) => {}
                r => return r,
            }
        }
        if n.group_node.as_ref().is_some_and(|g| g.exists()) && n.container.exists() {
            return Self::via_group(n);
        }
        Err(ProbeErr::NoInterface(
            "this kernel exposes neither the VFIO device nodes (/dev/vfio/devices, /dev/iommu) \
             nor the group nodes (/dev/vfio) for the device"
                .into(),
        ))
    }
}

// ------------------------------------------------------------- sysfs side

/// Roots the detection reads (tests point them at a fake tree).
pub struct Env<'a> {
    pub sys: &'a Path,
    pub dev: &'a Path,
    pub probe: &'a dyn VfioFeature,
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn link_name(p: &Path) -> Option<String> {
    std::fs::read_link(p)
        .ok()
        .and_then(|l| l.file_name().map(|n| n.to_string_lossy().into_owned()))
}

fn vfio_dev_name(dir: &Path) -> Option<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir.join("vfio-dev"))
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("vfio"))
        .collect();
    v.sort();
    v.into_iter().next()
}

/// Resolve a hostpci entry to its sysfs directory, kind label and driver.
fn locate(
    env: &Env,
    d: &VmHostPci,
) -> std::result::Result<(PathBuf, &'static str, String, Option<String>), String> {
    match d.kind()? {
        HostPciKind::Pci(addr) => {
            let dir = env.sys.join("bus/pci/devices").join(&addr);
            let drv = link_name(&dir.join("driver"));
            Ok((dir, "pci", addr, drv))
        }
        HostPciKind::Mdev(uuid) => {
            let dir = env.sys.join("bus/mdev/devices").join(&uuid);
            // The vendor driver lives on the parent PCI device.
            let drv = std::fs::canonicalize(&dir)
                .ok()
                .and_then(|r| r.parent().map(|p| p.join("driver")))
                .and_then(|p| link_name(&p));
            Ok((dir, "mdev", uuid, drv))
        }
    }
}

fn unsupported_driver_text(kind: &str, driver: &Option<String>) -> String {
    match (kind, driver.as_deref()) {
        ("pci", Some(d)) => format!(
            "the device is bound to {d}, which has no VFIO device migration support (a whole \
             device handed to the guest keeps state in hardware that cannot be saved; \
             SR-IOV virtual functions on a migration-capable driver, or vGPU devices, can)"
        ),
        _ => "the device's VFIO driver has no device migration support".to_string(),
    }
}

/// Ask the kernel about one device.
pub fn probe_device(env: &Env, d: &VmHostPci, cached: Option<&DeviceCap>) -> DeviceCap {
    let (dir, kind, name, driver) = match locate(env, d) {
        Ok(v) => v,
        Err(e) => {
            return DeviceCap {
                device: d.device.clone(),
                kind: "pci".into(),
                driver: None,
                vfio_name: None,
                migration: Migration::Unsupported {
                    code: "bad_device".into(),
                    reason: e,
                },
            }
        }
    };
    let mk = |vfio_name: Option<String>, migration| DeviceCap {
        device: d.device.clone(),
        kind: kind.into(),
        driver: driver.clone(),
        vfio_name,
        migration,
    };
    if !dir.exists() {
        return mk(
            None,
            Migration::Unsupported {
                code: "device_missing".into(),
                reason: format!("the device {name} does not exist on this host"),
            },
        );
    }
    let Some(vfio) = vfio_dev_name(&dir) else {
        let (code, reason) = if kind == "pci" {
            (
                "not_vfio_bound",
                format!("PCI device {name} is not bound to a VFIO driver"),
            )
        } else {
            (
                "no_vfio_device",
                format!("mediated device {name} has no VFIO device node"),
            )
        };
        return mk(
            None,
            Migration::Unsupported {
                code: code.into(),
                reason,
            },
        );
    };
    let group = link_name(&dir.join("iommu_group")).and_then(|g| g.parse::<u32>().ok());
    let node = DeviceNode {
        cdev: env.dev.join("vfio/devices").join(&vfio),
        group,
        dev_name: name.clone(),
        iommufd: env.dev.join("iommu"),
        container: env.dev.join("vfio/vfio"),
        group_node: group.map(|g| env.dev.join("vfio").join(g.to_string())),
        vfio_name: vfio.clone(),
    };
    let migration = match env.probe.migration_flags(&node) {
        Ok(f) if f & MIG_STOP_COPY != 0 => Migration::Supported {
            stop_copy: true,
            p2p: f & MIG_P2P != 0,
            pre_copy: f & MIG_PRE_COPY != 0,
        },
        Ok(_) | Err(ProbeErr::NotSupported) => Migration::Unsupported {
            code: "no_migration_support".into(),
            reason: unsupported_driver_text(kind, &driver),
        },
        Err(ProbeErr::Busy) => match cached {
            Some(c) if c.vfio_name.as_deref() == Some(&vfio) && c.driver == driver => {
                c.migration.clone()
            }
            _ => Migration::Unverified {
                code: "in_use".into(),
                reason: format!(
                    "{name} is held by the running VM and was not probed before it started; \
                     QEMU decides when the save is attempted"
                ),
            },
        },
        Err(ProbeErr::NoInterface(why)) => Migration::Unverified {
            code: "no_vfio_interface".into(),
            reason: why,
        },
        Err(ProbeErr::Other(why)) => Migration::Unverified {
            code: "probe_failed".into(),
            reason: format!("the kernel could not be asked: {why}"),
        },
    };
    mk(Some(vfio), migration)
}

/// The verdict for a VM's host devices.
pub fn vm_device_save(
    env: &Env,
    devices: &[VmHostPci],
    mode: DeviceSaveMode,
    cache: &[DeviceCap],
) -> DeviceSaveReport {
    if devices.is_empty() {
        return DeviceSaveReport::not_applicable();
    }
    let caps: Vec<DeviceCap> = devices
        .iter()
        .map(|d| {
            let c = cache.iter().find(|c| c.device == d.device);
            probe_device(env, d, c)
        })
        .collect();
    if mode == DeviceSaveMode::Never {
        return DeviceSaveReport {
            verdict: Verdict::Impossible,
            code: Some("disabled_by_policy".into()),
            reason: Some(
                "the VM owns host devices and the node setting device_save is never".into(),
            ),
            devices: caps,
        };
    }
    for c in &caps {
        if let Migration::Unsupported { code, reason } = &c.migration {
            return DeviceSaveReport {
                verdict: Verdict::Impossible,
                code: Some(code.clone()),
                reason: Some(format!("{}: {reason}", c.device)),
                devices: caps.clone(),
            };
        }
    }
    for c in &caps {
        if let Migration::Unverified { code, reason } = &c.migration {
            return DeviceSaveReport {
                verdict: Verdict::Unverified,
                code: Some(code.clone()),
                reason: Some(format!("{}: {reason}", c.device)),
                devices: caps.clone(),
            };
        }
    }
    DeviceSaveReport {
        verdict: Verdict::Possible,
        code: None,
        reason: None,
        devices: caps,
    }
}

// ------------------------------------------------------------------ cache

/// Where the probe made just before a start is kept (next to the disk).
pub fn cache_path(disk: &Path) -> PathBuf {
    let mut s = disk.as_os_str().to_os_string();
    s.push(".vfio-caps.json");
    PathBuf::from(s)
}

pub fn load_cache(disk: &Path) -> Vec<DeviceCap> {
    std::fs::read(cache_path(disk))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Probe the devices (they are free right before QEMU opens them) and keep
/// the answers for as long as the VM runs.
pub fn record_probe(env: &Env, disk: &Path, devices: &[VmHostPci]) {
    let p = cache_path(disk);
    if devices.is_empty() {
        let _ = std::fs::remove_file(&p);
        return;
    }
    let caps: Vec<DeviceCap> = devices.iter().map(|d| probe_device(env, d, None)).collect();
    if let Ok(j) = serde_json::to_vec(&caps) {
        let _ = std::fs::write(&p, j);
    }
}

// ------------------------------------------------- recreate after a reboot

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRecipe {
    pub device: String,
    /// Mediated device: parent PCI address and type id.
    pub mdev: Option<(String, String)>,
    /// SR-IOV virtual function: physical function and the VF count it had.
    pub vf: Option<(String, u32)>,
    /// Driver the function was bound to (restored with `driver_override`).
    pub driver: Option<String>,
}

/// How to make the VM's devices again (called while they exist).
pub fn recipes_of(sys: &Path, devices: &[VmHostPci]) -> Vec<DeviceRecipe> {
    let mut out = Vec::new();
    for d in devices {
        let Ok(kind) = d.kind() else { continue };
        match kind {
            HostPciKind::Mdev(uuid) => {
                let dir = sys.join("bus/mdev/devices").join(&uuid);
                let real = std::fs::canonicalize(&dir).unwrap_or(dir.clone());
                let parent = real
                    .parent()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().into_owned());
                let ty = std::fs::canonicalize(dir.join("mdev_type"))
                    .ok()
                    .and_then(|t| t.file_name().map(|n| n.to_string_lossy().into_owned()));
                out.push(DeviceRecipe {
                    device: d.device.clone(),
                    mdev: parent.zip(ty),
                    vf: None,
                    driver: None,
                });
            }
            HostPciKind::Pci(addr) => {
                let dir = sys.join("bus/pci/devices").join(&addr);
                let pf = link_name(&dir.join("physfn"));
                let vf = pf.and_then(|pf| {
                    let n = read(&sys.join("bus/pci/devices").join(&pf).join("sriov_numvfs"))
                        .parse::<u32>()
                        .ok()?;
                    Some((pf, n))
                });
                out.push(DeviceRecipe {
                    device: d.device.clone(),
                    mdev: None,
                    vf,
                    driver: link_name(&dir.join("driver")),
                });
            }
        }
    }
    out
}

/// Make the recorded devices exist again. Returns what was done (for the
/// log). A device that is already there is left alone.
pub async fn restore_devices(sys: &Path, recipes: &[DeviceRecipe]) -> Result<Vec<String>> {
    let mut done = Vec::new();
    for r in recipes {
        let Ok(kind) = (VmHostPci {
            device: r.device.clone(),
            ..Default::default()
        })
        .kind()
        .map_err(Error::Validation) else {
            continue;
        };
        match kind {
            HostPciKind::Mdev(uuid) => {
                if sys.join("bus/mdev/devices").join(&uuid).exists() {
                    continue;
                }
                let (parent, ty) = r.mdev.clone().ok_or_else(|| {
                    Error::Conflict(format!(
                        "mediated device {uuid} is gone and its type was not recorded"
                    ))
                })?;
                super::mdev::MdevManager::with_sysfs(sys).create(&parent, &ty, Some(&uuid))?;
                done.push(format!("created mediated device {uuid} ({ty} on {parent})"));
            }
            HostPciKind::Pci(addr) => {
                let dir = sys.join("bus/pci/devices").join(&addr);
                if dir.exists() {
                    continue;
                }
                let Some((pf, want)) = r.vf.clone() else {
                    continue; // a plain device: check_device explains
                };
                let pfdir = sys.join("bus/pci/devices").join(&pf);
                let cur = read(&pfdir.join("sriov_numvfs"))
                    .parse::<u32>()
                    .unwrap_or(0);
                if cur == 0 {
                    std::fs::write(pfdir.join("sriov_numvfs"), want.to_string()).map_err(|e| {
                        Error::System(format!(
                            "cannot create {want} virtual functions on {pf}: {e}"
                        ))
                    })?;
                } else if cur < want {
                    return Err(Error::Conflict(format!(
                        "{pf} has {cur} virtual functions but the saved VM needs {want}; \
                         change the count only when no VM uses them"
                    )));
                }
                // The kernel creates the functions asynchronously.
                for _ in 0..50 {
                    if dir.exists() {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
                if !dir.exists() {
                    return Err(Error::Conflict(format!(
                        "virtual function {addr} did not appear on {pf}"
                    )));
                }
                if let Some(drv) = &r.driver {
                    if link_name(&dir.join("driver")).as_deref() != Some(drv) {
                        std::fs::write(dir.join("driver_override"), drv).map_err(|e| {
                            Error::System(format!("cannot choose the driver of {addr}: {e}"))
                        })?;
                        let _ = std::fs::write(sys.join("bus/pci/drivers_probe"), &addr);
                    }
                }
                done.push(format!("created virtual function {addr} on {pf}"));
            }
        }
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs;
    use std::os::unix::fs::symlink;

    struct Fake(HashMap<String, std::result::Result<u64, ProbeErr>>);
    impl VfioFeature for Fake {
        fn migration_flags(&self, n: &DeviceNode) -> std::result::Result<u64, ProbeErr> {
            self.0
                .get(&n.vfio_name)
                .cloned()
                .unwrap_or(Err(ProbeErr::NotSupported))
        }
    }

    fn hp(d: &str) -> VmHostPci {
        VmHostPci {
            device: d.into(),
            ..Default::default()
        }
    }

    fn pci(root: &Path, addr: &str, vfio: &str, driver: &str, group: u32) {
        let d = root.join("sys/bus/pci/devices").join(addr);
        fs::create_dir_all(d.join("vfio-dev").join(vfio)).unwrap();
        fs::create_dir_all(root.join("sys/bus/pci/drivers").join(driver)).unwrap();
        symlink(
            root.join("sys/bus/pci/drivers").join(driver),
            d.join("driver"),
        )
        .unwrap();
        fs::create_dir_all(root.join("sys/kernel/iommu_groups").join(group.to_string())).unwrap();
        symlink(
            root.join("sys/kernel/iommu_groups").join(group.to_string()),
            d.join("iommu_group"),
        )
        .unwrap();
    }

    fn env<'a>(root: &'a Path, p: &'a Fake, sys: &'a Path, dev: &'a Path) -> Env<'a> {
        let _ = root;
        Env { sys, dev, probe: p }
    }

    #[test]
    fn migration_capable_vf_and_incapable_whole_gpu() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        pci(r, "0000:03:00.1", "vfio0", "xyz_vfio_pci", 12); // a VF on a variant driver
        pci(r, "0000:01:00.0", "vfio1", "vfio-pci", 7); // a whole GPU
        let f = Fake(HashMap::from([
            ("vfio0".to_string(), Ok(MIG_STOP_COPY | MIG_P2P)),
            ("vfio1".to_string(), Err(ProbeErr::NotSupported)),
        ]));
        let sys = r.join("sys");
        let dev = r.join("dev");
        let e = env(r, &f, &sys, &dev);

        let ok = vm_device_save(&e, &[hp("0000:03:00.1")], DeviceSaveMode::Auto, &[]);
        assert_eq!(ok.verdict, Verdict::Possible);
        assert_eq!(
            ok.devices[0].migration,
            Migration::Supported {
                stop_copy: true,
                p2p: true,
                pre_copy: false
            }
        );
        assert_eq!(ok.devices[0].driver.as_deref(), Some("xyz_vfio_pci"));

        let no = vm_device_save(&e, &[hp("0000:01:00.0")], DeviceSaveMode::Auto, &[]);
        assert_eq!(no.verdict, Verdict::Impossible);
        assert_eq!(no.code.as_deref(), Some("no_migration_support"));
        assert!(no.reason.as_deref().unwrap().contains("vfio-pci"));
        assert!(!no.may_save());

        // One bad device (a GPU's own audio function, say) blocks the lot.
        let mix = vm_device_save(
            &e,
            &[hp("0000:03:00.1"), hp("0000:01:00.0")],
            DeviceSaveMode::Auto,
            &[],
        );
        assert_eq!(mix.verdict, Verdict::Impossible);
        assert_eq!(mix.devices.len(), 2);
    }

    #[test]
    fn no_devices_is_not_applicable_and_policy_never_blocks() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        pci(r, "0000:03:00.1", "vfio0", "xyz_vfio_pci", 12);
        let f = Fake(HashMap::from([("vfio0".to_string(), Ok(MIG_STOP_COPY))]));
        let (sys, dev) = (r.join("sys"), r.join("dev"));
        let e = env(r, &f, &sys, &dev);
        assert_eq!(
            vm_device_save(&e, &[], DeviceSaveMode::Auto, &[]).verdict,
            Verdict::NotApplicable
        );
        let n = vm_device_save(&e, &[hp("0000:03:00.1")], DeviceSaveMode::Never, &[]);
        assert_eq!(n.verdict, Verdict::Impossible);
        assert_eq!(n.code.as_deref(), Some("disabled_by_policy"));
    }

    #[test]
    fn flags_without_stop_copy_are_unsupported() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        pci(r, "0000:03:00.1", "vfio0", "v", 1);
        let f = Fake(HashMap::from([("vfio0".to_string(), Ok(0))]));
        let (sys, dev) = (r.join("sys"), r.join("dev"));
        let e = env(r, &f, &sys, &dev);
        let v = vm_device_save(&e, &[hp("0000:03:00.1")], DeviceSaveMode::Auto, &[]);
        assert_eq!(v.verdict, Verdict::Impossible);
    }

    #[test]
    fn missing_and_unbound_devices_are_named() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        fs::create_dir_all(r.join("sys/bus/pci/devices/0000:05:00.0")).unwrap();
        let f = Fake(HashMap::new());
        let (sys, dev) = (r.join("sys"), r.join("dev"));
        let e = env(r, &f, &sys, &dev);
        let a = vm_device_save(&e, &[hp("0000:09:00.0")], DeviceSaveMode::Auto, &[]);
        assert_eq!(a.code.as_deref(), Some("device_missing"));
        let b = vm_device_save(&e, &[hp("0000:05:00.0")], DeviceSaveMode::Auto, &[]);
        assert_eq!(b.code.as_deref(), Some("not_vfio_bound"));
    }

    #[test]
    fn busy_device_uses_the_probe_made_before_start_or_is_unverified() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        pci(r, "0000:03:00.1", "vfio0", "v", 1);
        let (sys, dev) = (r.join("sys"), r.join("dev"));
        let free = Fake(HashMap::from([("vfio0".to_string(), Ok(MIG_STOP_COPY))]));
        let disk = r.join("vm.qcow2");
        record_probe(&env(r, &free, &sys, &dev), &disk, &[hp("0000:03:00.1")]);
        let cache = load_cache(&disk);
        assert_eq!(cache.len(), 1);

        let busy = Fake(HashMap::from([("vfio0".to_string(), Err(ProbeErr::Busy))]));
        let e = env(r, &busy, &sys, &dev);
        let v = vm_device_save(&e, &[hp("0000:03:00.1")], DeviceSaveMode::Auto, &cache);
        assert_eq!(v.verdict, Verdict::Possible, "{v:?}");
        let u = vm_device_save(&e, &[hp("0000:03:00.1")], DeviceSaveMode::Auto, &[]);
        assert_eq!(u.verdict, Verdict::Unverified);
        assert_eq!(u.code.as_deref(), Some("in_use"));
        assert!(u.may_save());

        // A cache from another driver binding is not trusted.
        let mut stale = cache.clone();
        stale[0].driver = Some("other".into());
        let s = vm_device_save(&e, &[hp("0000:03:00.1")], DeviceSaveMode::Auto, &stale);
        assert_eq!(s.verdict, Verdict::Unverified);

        // No VFIO interface at all: unverified, not impossible.
        let none = Fake(HashMap::from([(
            "vfio0".to_string(),
            Err(ProbeErr::NoInterface("old kernel".into())),
        )]));
        let n = vm_device_save(
            &env(r, &none, &sys, &dev),
            &[hp("0000:03:00.1")],
            DeviceSaveMode::Auto,
            &[],
        );
        assert_eq!(n.verdict, Verdict::Unverified);
        assert_eq!(n.code.as_deref(), Some("no_vfio_interface"));
    }

    #[test]
    fn mediated_device_is_probed_through_its_vfio_dev() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        let uuid = "6b1f3a6e-5d0b-4b5f-9f61-0123456789ab";
        let parent = r.join("sys/devices/pci0000:00/0000:00:02.0");
        fs::create_dir_all(parent.join(uuid).join("vfio-dev/vfio3")).unwrap();
        fs::create_dir_all(parent.join("mdev_supported_types/t-1")).unwrap();
        symlink(
            parent.join("mdev_supported_types/t-1"),
            parent.join(uuid).join("mdev_type"),
        )
        .unwrap();
        fs::create_dir_all(r.join("sys/bus/pci/drivers/vgpu_drv")).unwrap();
        symlink(
            r.join("sys/bus/pci/drivers/vgpu_drv"),
            parent.join("driver"),
        )
        .unwrap();
        fs::create_dir_all(r.join("sys/bus/mdev/devices")).unwrap();
        symlink(parent.join(uuid), r.join("sys/bus/mdev/devices").join(uuid)).unwrap();
        let f = Fake(HashMap::from([(
            "vfio3".to_string(),
            Ok(MIG_STOP_COPY | MIG_PRE_COPY),
        )]));
        let (sys, dev) = (r.join("sys"), r.join("dev"));
        let v = vm_device_save(
            &env(r, &f, &sys, &dev),
            &[hp(uuid)],
            DeviceSaveMode::Auto,
            &[],
        );
        assert_eq!(v.verdict, Verdict::Possible, "{v:?}");
        assert_eq!(v.devices[0].kind, "mdev");
        assert_eq!(v.devices[0].driver.as_deref(), Some("vgpu_drv"));

        let rec = recipes_of(&sys, &[hp(uuid)]);
        assert_eq!(rec[0].mdev, Some(("0000:00:02.0".into(), "t-1".into())));
    }

    #[test]
    fn ioctl_numbers_and_struct_layouts_match_linux_vfio_h() {
        // From <linux/vfio.h>: _IO(';', 100 + n).
        assert_eq!(VFIO_SET_IOMMU, 0x3b66);
        assert_eq!(VFIO_GROUP_SET_CONTAINER, 0x3b68);
        assert_eq!(VFIO_GROUP_GET_DEVICE_FD, 0x3b6a);
        assert_eq!(VFIO_DEVICE_FEATURE, 0x3b75);
        assert_eq!(VFIO_DEVICE_BIND_IOMMUFD, 0x3b76);
        assert_eq!(std::mem::size_of::<FeatureMigration>(), 16);
        assert_eq!(std::mem::size_of::<BindIommufd>(), 16);
    }

    #[test]
    fn real_probe_without_any_node_says_no_interface() {
        let t = tempfile::tempdir().unwrap();
        let n = DeviceNode {
            vfio_name: "vfio0".into(),
            cdev: t.path().join("nope"),
            group: Some(1),
            dev_name: "0000:00:00.0".into(),
            iommufd: t.path().join("iommu"),
            container: t.path().join("vfio"),
            group_node: Some(t.path().join("1")),
        };
        assert!(matches!(
            KernelVfio.migration_flags(&n),
            Err(ProbeErr::NoInterface(_))
        ));
    }

    /// Real hardware only: `HX_TEST_VFIO_DEVICE=<pci addr or mdev uuid>
    /// cargo test -p horcrux-vm --lib real_device -- --ignored --nocapture`
    /// (as root, on a lab host, with the device unused). Prints the verdict.
    #[test]
    #[ignore]
    fn real_device_report() {
        let Ok(d) = std::env::var("HX_TEST_VFIO_DEVICE") else {
            return;
        };
        let env = Env {
            sys: Path::new("/sys"),
            dev: Path::new("/dev"),
            probe: &KernelVfio,
        };
        let r = vm_device_save(&env, &[hp(&d)], DeviceSaveMode::Auto, &[]);
        println!("{}", serde_json::to_string_pretty(&r).unwrap());
    }

    #[tokio::test]
    async fn restore_recreates_a_mediated_device_and_virtual_functions() {
        let t = tempfile::tempdir().unwrap();
        let sys = t.path().join("sys");
        let uuid = "6b1f3a6e-5d0b-4b5f-9f61-0123456789ab";
        // The mdev parent exists, the device is gone (host rebooted).
        let t1 = sys.join("class/mdev_bus/0000:00:02.0/mdev_supported_types/t-1");
        fs::create_dir_all(&t1).unwrap();
        fs::write(t1.join("create"), "").unwrap();
        fs::create_dir_all(sys.join("bus/mdev/devices")).unwrap();
        // A PF with no VFs.
        let pf = sys.join("bus/pci/devices/0000:03:00.0");
        fs::create_dir_all(&pf).unwrap();
        fs::write(pf.join("sriov_numvfs"), "0\n").unwrap();
        let vf = sys.join("bus/pci/devices/0000:03:00.1");
        let recipes = vec![
            DeviceRecipe {
                device: uuid.into(),
                mdev: Some(("0000:00:02.0".into(), "t-1".into())),
                vf: None,
                driver: None,
            },
            DeviceRecipe {
                device: "0000:03:00.1".into(),
                mdev: None,
                vf: Some(("0000:03:00.0".into(), 2)),
                driver: Some("xyz_vfio_pci".into()),
            },
        ];
        // The kernel makes the VF appear when sriov_numvfs is written: emulate it.
        let vf2 = vf.clone();
        let h = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            fs::create_dir_all(&vf2).unwrap();
        });
        let done = restore_devices(&sys, &recipes).await.unwrap();
        h.await.unwrap();
        assert_eq!(fs::read_to_string(pf.join("sriov_numvfs")).unwrap(), "2");
        assert_eq!(
            fs::read_to_string(vf.join("driver_override")).unwrap(),
            "xyz_vfio_pci"
        );
        assert_eq!(done.len(), 2, "{done:?}");
        assert!(done[0].contains("mediated"));

        // Too few VFs while some exist: refused, nothing is resized.
        fs::write(pf.join("sriov_numvfs"), "1\n").unwrap();
        fs::remove_dir_all(&vf).unwrap();
        let e = restore_devices(&sys, &recipes[1..])
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("needs 2"), "{e}");
    }
}
