//! Host PCI passthrough and mediated devices (vGPU) for QEMU VMs.
//!
//! Everything works on sysfs, so it can be tested against a fixture tree:
//!
//! * [`check_device`] - is a [`VmHostPci`] usable right now? A PCI device
//!   must exist, sit in an IOMMU group and be bound to `vfio-pci`; a mediated
//!   device must exist under `/sys/bus/mdev/devices`.
//! * [`MdevManager`] - the mediated devices the host can create
//!   (`/sys/class/mdev_bus/<parent>/mdev_supported_types`: NVIDIA vGPU
//!   profiles, Intel GVT-g, ...), the ones that exist, create and remove.
//!
//! The VM side is `-device vfio-pci,host=<addr>` / `sysfsdev=<mdev>` (see
//! `qemu::device_args`); `POST /api/gpu/devices/:addr/bind-vfio` (gpu.rs)
//! prepares a whole device.
use horcrux_common::{Error, HostPciKind, Result, VmHostPci};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// A mediated device type a parent device offers (a vGPU profile).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MdevType {
    /// Parent PCI address (`0000:00:02.0`).
    pub parent: String,
    /// Type id (`i915-GVTg_V5_4`, `nvidia-256`).
    pub id: String,
    pub name: String,
    pub description: String,
    pub available_instances: u32,
    /// `vfio-pci`, `vfio-ccw`, ...
    pub device_api: String,
}

/// An existing mediated device.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MdevDevice {
    pub uuid: String,
    pub parent: String,
    pub mdev_type: String,
}

/// Sysfs access for mediated devices.
#[derive(Debug, Clone)]
pub struct MdevManager {
    sys: PathBuf,
}

impl Default for MdevManager {
    fn default() -> Self {
        Self::new()
    }
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn sorted_dir(p: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(p)
        .map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

impl MdevManager {
    pub fn new() -> Self {
        Self::with_sysfs("/sys")
    }

    /// Use another sysfs root (tests).
    pub fn with_sysfs(root: impl Into<PathBuf>) -> Self {
        Self { sys: root.into() }
    }

    /// Every mediated device type every parent offers.
    pub fn list_types(&self) -> Vec<MdevType> {
        let mut out = Vec::new();
        for parent in sorted_dir(&self.sys.join("class/mdev_bus")) {
            let parent_name = file_name(&parent);
            for t in sorted_dir(&parent.join("mdev_supported_types")) {
                out.push(MdevType {
                    parent: parent_name.clone(),
                    id: file_name(&t),
                    name: read(&t.join("name")),
                    description: read(&t.join("description")).replace('\n', "; "),
                    available_instances: read(&t.join("available_instances")).parse().unwrap_or(0),
                    device_api: read(&t.join("device_api")),
                });
            }
        }
        out
    }

    /// The mediated devices that exist.
    pub fn list_devices(&self) -> Vec<MdevDevice> {
        sorted_dir(&self.sys.join("bus/mdev/devices"))
            .into_iter()
            .map(|d| {
                // <parent>/<uuid> is the device, its `mdev_type` link points
                // at <parent>/mdev_supported_types/<type>.
                let real = std::fs::canonicalize(&d).unwrap_or_else(|_| d.clone());
                let parent = real
                    .parent()
                    .map(file_name)
                    .filter(|p| !p.is_empty())
                    .unwrap_or_default();
                let mdev_type = std::fs::canonicalize(d.join("mdev_type"))
                    .map(|t| file_name(&t))
                    .unwrap_or_else(|_| file_name(&d.join("mdev_type")));
                MdevDevice {
                    uuid: file_name(&d),
                    parent,
                    mdev_type,
                }
            })
            .collect()
    }

    /// Create a mediated device of `mdev_type` on `parent` (a fresh UUID
    /// unless one is given). The kernel does the real checking; the
    /// instance count is verified first so the error says why.
    pub fn create(&self, parent: &str, mdev_type: &str, uuid: Option<&str>) -> Result<MdevDevice> {
        safe_component(parent, "parent")?;
        safe_component(mdev_type, "type")?;
        let uuid = match uuid {
            Some(u) => {
                let u = u.trim().to_ascii_lowercase();
                match (VmHostPci {
                    device: u.clone(),
                    primary_gpu: false,
                })
                .kind()
                {
                    Ok(HostPciKind::Mdev(u)) => u,
                    _ => return Err(Error::Validation(format!("{u:?} is not a UUID"))),
                }
            }
            None => uuid_v4(),
        };
        let tdir = self
            .sys
            .join("class/mdev_bus")
            .join(parent)
            .join("mdev_supported_types")
            .join(mdev_type);
        if !tdir.is_dir() {
            return Err(Error::NotFound(format!(
                "{parent} offers no mediated device type {mdev_type}"
            )));
        }
        if self.sys.join("bus/mdev/devices").join(&uuid).exists() {
            return Err(Error::Conflict(format!(
                "mediated device {uuid} already exists"
            )));
        }
        if read(&tdir.join("available_instances")).parse::<u32>() == Ok(0) {
            return Err(Error::Conflict(format!(
                "no instance of {mdev_type} is available on {parent}"
            )));
        }
        std::fs::write(tdir.join("create"), &uuid)
            .map_err(|e| Error::System(format!("creating mediated device {uuid}: {e}")))?;
        Ok(MdevDevice {
            uuid,
            parent: parent.to_string(),
            mdev_type: mdev_type.to_string(),
        })
    }

    /// Remove a mediated device (the kernel refuses while a guest holds it).
    pub fn remove(&self, uuid: &str) -> Result<()> {
        let u = uuid.trim().to_ascii_lowercase();
        if !matches!(
            VmHostPci {
                device: u.clone(),
                primary_gpu: false
            }
            .kind(),
            Ok(HostPciKind::Mdev(_))
        ) {
            return Err(Error::Validation(format!("{uuid:?} is not a UUID")));
        }
        let dev = self.sys.join("bus/mdev/devices").join(&u);
        if !dev.exists() {
            return Err(Error::NotFound(format!("mediated device {u} not found")));
        }
        std::fs::write(dev.join("remove"), "1").map_err(|e| {
            Error::Conflict(format!(
                "removing mediated device {u} (is a running VM using it?): {e}"
            ))
        })
    }
}

fn safe_component(v: &str, what: &str) -> Result<()> {
    if v.is_empty()
        || v.len() > 64
        || !v
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '.' | '_' | '-'))
        || v.starts_with('.')
    {
        return Err(Error::Validation(format!("invalid mdev {what} {v:?}")));
    }
    Ok(())
}

fn uuid_v4() -> String {
    let mut b: [u8; 16] = rand::random();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// Is `dev` usable by a VM right now (sysfs root `sys`)?
pub fn check_device(sys: &Path, dev: &VmHostPci) -> Result<()> {
    match dev.kind().map_err(Error::Validation)? {
        HostPciKind::Mdev(uuid) => {
            if !sys.join("bus/mdev/devices").join(&uuid).exists() {
                return Err(Error::Validation(format!(
                    "mediated device {uuid} does not exist on this host (create one with POST /api/gpu/mdev/devices)"
                )));
            }
        }
        HostPciKind::Pci(addr) => {
            let d = sys.join("bus/pci/devices").join(&addr);
            if !d.exists() {
                return Err(Error::Validation(format!(
                    "PCI device {addr} does not exist on this host"
                )));
            }
            let driver = std::fs::read_link(d.join("driver"))
                .map(|p| file_name(&p))
                .unwrap_or_default();
            if driver != "vfio-pci" {
                return Err(Error::Validation(format!(
                    "PCI device {addr} is bound to {} - bind it to vfio-pci first (POST /api/gpu/devices/{addr}/bind-vfio)",
                    if driver.is_empty() { "no driver" } else { &driver }
                )));
            }
            if !d.join("iommu_group").exists() {
                return Err(Error::Validation(format!(
                    "PCI device {addr} is in no IOMMU group: enable the IOMMU (intel_iommu=on / amd_iommu=on) and reboot"
                )));
            }
        }
    }
    Ok(())
}

/// The QEMU arguments of one passed-through device.
pub fn device_args(i: usize, dev: &VmHostPci) -> Vec<String> {
    let Ok(kind) = dev.kind() else {
        return Vec::new();
    };
    let target = match kind {
        HostPciKind::Pci(a) => format!("host={a}"),
        HostPciKind::Mdev(u) => format!("sysfsdev=/sys/bus/mdev/devices/{u}"),
    };
    let vga = if dev.primary_gpu { ",x-vga=on" } else { "" };
    vec![
        "-device".into(),
        format!("vfio-pci,{target},id=hostpci{i}{vga}"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const UUID: &str = "6b1f3a6e-5d0b-4b5f-9f61-0123456789ab";

    fn hp(d: &str) -> VmHostPci {
        VmHostPci {
            device: d.into(),
            primary_gpu: false,
        }
    }

    /// A sysfs tree with one mdev parent (GVT-g style, 2 types) and a
    /// vfio-bound + a nvidia-bound PCI device.
    fn fixture() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        let s = t.path();
        let ty = s.join("class/mdev_bus/0000:00:02.0/mdev_supported_types");
        for (id, name, avail) in [
            ("i915-GVTg_V5_4", "GVTg_V5_4", "2"),
            ("i915-GVTg_V5_8", "GVTg_V5_8", "0"),
        ] {
            fs::create_dir_all(ty.join(id)).unwrap();
            fs::write(ty.join(id).join("name"), format!("{name}\n")).unwrap();
            fs::write(
                ty.join(id).join("description"),
                "low_gm_size: 128MB\nhigh_gm_size: 512MB\n",
            )
            .unwrap();
            fs::write(
                ty.join(id).join("available_instances"),
                format!("{avail}\n"),
            )
            .unwrap();
            fs::write(ty.join(id).join("device_api"), "vfio-pci\n").unwrap();
        }
        fs::create_dir_all(s.join("bus/mdev/devices")).unwrap();
        for (addr, drv) in [
            ("0000:01:00.0", Some("vfio-pci")),
            ("0000:02:00.0", Some("nvidia")),
            ("0000:03:00.0", None),
        ] {
            let d = s.join("bus/pci/devices").join(addr);
            fs::create_dir_all(&d).unwrap();
            if let Some(drv) = drv {
                let dd = s.join("bus/pci/drivers").join(drv);
                fs::create_dir_all(&dd).unwrap();
                std::os::unix::fs::symlink(&dd, d.join("driver")).unwrap();
            }
        }
        fs::create_dir_all(s.join("bus/pci/devices/0000:01:00.0/iommu_group")).unwrap();
        fs::create_dir_all(s.join("bus/pci/devices/0000:02:00.0/iommu_group")).unwrap();
        t
    }

    #[test]
    fn types_list_with_instances() {
        let t = fixture();
        let m = MdevManager::with_sysfs(t.path());
        let ty = m.list_types();
        assert_eq!(ty.len(), 2);
        assert_eq!(ty[0].id, "i915-GVTg_V5_4");
        assert_eq!(ty[0].parent, "0000:00:02.0");
        assert_eq!(ty[0].name, "GVTg_V5_4");
        assert_eq!(ty[0].available_instances, 2);
        assert_eq!(ty[0].description, "low_gm_size: 128MB; high_gm_size: 512MB");
        assert_eq!(ty[1].available_instances, 0);
        assert_eq!(ty[0].device_api, "vfio-pci");
        assert!(MdevManager::with_sysfs("/nonexistent-sysfs")
            .list_types()
            .is_empty());
    }

    #[test]
    fn create_writes_the_uuid_to_the_type_and_checks_first() {
        let t = fixture();
        let m = MdevManager::with_sysfs(t.path());
        let d = m
            .create("0000:00:02.0", "i915-GVTg_V5_4", Some(UUID))
            .unwrap();
        assert_eq!(d.uuid, UUID);
        let create = t
            .path()
            .join("class/mdev_bus/0000:00:02.0/mdev_supported_types/i915-GVTg_V5_4/create");
        assert_eq!(fs::read_to_string(create).unwrap(), UUID);
        // a generated UUID is a valid v4 UUID
        let g = m.create("0000:00:02.0", "i915-GVTg_V5_4", None).unwrap();
        assert!(matches!(hp(&g.uuid).kind(), Ok(HostPciKind::Mdev(_))));
        assert_eq!(&g.uuid[14..15], "4");
        // no instance left, unknown type, bad names
        assert!(matches!(
            m.create("0000:00:02.0", "i915-GVTg_V5_8", None),
            Err(Error::Conflict(_))
        ));
        assert!(matches!(
            m.create("0000:00:02.0", "nope", None),
            Err(Error::NotFound(_))
        ));
        assert!(matches!(
            m.create("../x", "nope", None),
            Err(Error::Validation(_))
        ));
        assert!(matches!(
            m.create("0000:00:02.0", "../../x", None),
            Err(Error::Validation(_))
        ));
        assert!(matches!(
            m.create("0000:00:02.0", "i915-GVTg_V5_4", Some("not-a-uuid")),
            Err(Error::Validation(_))
        ));
    }

    #[test]
    fn existing_devices_are_listed_with_parent_and_type_and_removed() {
        let t = fixture();
        let s = t.path();
        // what the kernel makes: <parent>/<uuid>, bus link, mdev_type link
        let parent = s.join("devices/pci0000:00/0000:00:02.0");
        let dev = parent.join(UUID);
        fs::create_dir_all(&dev).unwrap();
        std::os::unix::fs::symlink(
            s.join("class/mdev_bus/0000:00:02.0/mdev_supported_types/i915-GVTg_V5_4"),
            dev.join("mdev_type"),
        )
        .unwrap();
        std::os::unix::fs::symlink(&dev, s.join("bus/mdev/devices").join(UUID)).unwrap();
        let m = MdevManager::with_sysfs(s);
        let l = m.list_devices();
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].uuid, UUID);
        assert_eq!(l[0].parent, "0000:00:02.0");
        assert_eq!(l[0].mdev_type, "i915-GVTg_V5_4");
        assert!(matches!(
            m.create("0000:00:02.0", "i915-GVTg_V5_4", Some(UUID)),
            Err(Error::Conflict(_))
        ));
        m.remove(UUID).unwrap();
        assert_eq!(fs::read_to_string(dev.join("remove")).unwrap(), "1");
        assert!(matches!(
            m.remove("11111111-1111-4111-8111-111111111111"),
            Err(Error::NotFound(_))
        ));
        assert!(matches!(m.remove("../etc"), Err(Error::Validation(_))));
    }

    #[test]
    fn check_device_demands_vfio_and_an_iommu_group() {
        let t = fixture();
        let s = t.path();
        assert!(check_device(s, &hp("0000:01:00.0")).is_ok());
        assert!(check_device(s, &hp("01:00.0")).is_ok(), "domain optional");
        let e = check_device(s, &hp("0000:02:00.0"))
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("bound to nvidia") && e.contains("bind-vfio"),
            "{e}"
        );
        let e = check_device(s, &hp("0000:03:00.0"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("no driver"), "{e}");
        assert!(check_device(s, &hp("0000:09:00.0"))
            .unwrap_err()
            .to_string()
            .contains("does not exist"));
        fs::remove_dir_all(s.join("bus/pci/devices/0000:01:00.0/iommu_group")).unwrap();
        assert!(check_device(s, &hp("0000:01:00.0"))
            .unwrap_err()
            .to_string()
            .contains("IOMMU group"));
        assert!(check_device(s, &hp(UUID))
            .unwrap_err()
            .to_string()
            .contains("mediated device"));
        fs::create_dir_all(s.join("bus/mdev/devices").join(UUID)).unwrap();
        assert!(check_device(s, &hp(UUID)).is_ok());
        assert!(check_device(s, &hp("garbage; -drive")).is_err());
    }

    #[test]
    fn qemu_arguments() {
        assert_eq!(
            device_args(0, &hp("01:00.0")),
            ["-device", "vfio-pci,host=0000:01:00.0,id=hostpci0"]
        );
        let g = VmHostPci {
            device: "0000:01:00.0".into(),
            primary_gpu: true,
        };
        assert_eq!(
            device_args(1, &g)[1],
            "vfio-pci,host=0000:01:00.0,id=hostpci1,x-vga=on"
        );
        assert_eq!(
            device_args(2, &hp(&UUID.to_uppercase()))[1],
            format!("vfio-pci,sysfsdev=/sys/bus/mdev/devices/{UUID},id=hostpci2")
        );
        assert!(device_args(0, &hp("x,y")).is_empty());
    }
}
