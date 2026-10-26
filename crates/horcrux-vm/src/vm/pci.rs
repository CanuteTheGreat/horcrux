//! Host PCI device inventory and vfio-pci binding over sysfs (any device, not
//! only GPUs: NICs, HBAs, NVMe, USB controllers, SR-IOV virtual functions).
//!
//! * [`PciManager::list`] - every PCI device with its class, driver, IOMMU
//!   group and the other members of that group; the group is only passable
//!   when every non-bridge member is bound to `vfio-pci`.
//! * [`PciManager::bind_vfio`] / [`PciManager::unbind_vfio`] - move one device
//!   to `vfio-pci` with `driver_override` (no `new_id`, so identical sibling
//!   devices keep their host driver) and back.
//!
//! Everything takes a sysfs root so it is tested against a fixture tree.
use horcrux_common::{Error, HostPciKind, Result, VmHostPci};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// One PCI device.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PciDevice {
    /// `0000:01:00.0`
    pub address: String,
    pub vendor_id: String,
    pub device_id: String,
    pub vendor_name: String,
    /// Class code (`0x030000`).
    pub class: String,
    /// `gpu`, `network`, `storage`, `usb`, `bridge`, ...
    pub class_name: String,
    /// Bound driver (`nvidia`, `vfio-pci`, ...), none if unbound.
    pub driver: Option<String>,
    pub iommu_group: Option<String>,
    /// Other devices in the same IOMMU group.
    pub group_members: Vec<String>,
    /// Non-bridge group members still bound to a host driver: the group can
    /// not be passed through until they are on `vfio-pci` too.
    pub group_blockers: Vec<String>,
    pub vfio: bool,
    /// SR-IOV: virtual functions enabled / the PF this device belongs to.
    pub sriov_vfs: u32,
    pub sriov_max_vfs: u32,
    pub physfn: Option<String>,
    pub numa_node: Option<i32>,
}

/// Sysfs access for PCI devices.
#[derive(Debug, Clone)]
pub struct PciManager {
    sys: PathBuf,
}

impl Default for PciManager {
    fn default() -> Self {
        Self::new()
    }
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn base(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn sorted_dir(p: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(p)
        .map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

fn class_name(class: &str) -> &'static str {
    let hex = class.trim_start_matches("0x");
    match hex.get(0..2).unwrap_or("") {
        "01" => "storage",
        "02" => "network",
        "03" => "gpu",
        "04" => "multimedia",
        "05" => "memory",
        "06" => "bridge",
        "07" => "communication",
        "08" => "system",
        "0b" => "processor",
        "0c" if hex.get(2..4) == Some("03") => "usb",
        "0c" => "serial bus",
        "12" => "accelerator",
        _ => "other",
    }
}

fn vendor_name(id: &str) -> &'static str {
    match id.trim_start_matches("0x") {
        "8086" => "Intel",
        "10de" => "NVIDIA",
        "1002" | "1022" => "AMD",
        "15b3" => "NVIDIA Mellanox",
        "14e4" => "Broadcom",
        "1000" => "Broadcom/LSI",
        "1b4b" => "Marvell",
        "144d" => "Samsung",
        "1344" => "Micron",
        "1912" => "Renesas",
        "1af4" => "Red Hat (virtio)",
        _ => "",
    }
}

impl PciManager {
    pub fn new() -> Self {
        Self::with_sysfs("/sys")
    }

    pub fn with_sysfs(root: impl Into<PathBuf>) -> Self {
        Self { sys: root.into() }
    }

    fn devs(&self) -> PathBuf {
        self.sys.join("bus/pci/devices")
    }

    /// Is the IOMMU active (any IOMMU group exists)?
    pub fn iommu_groups(&self) -> usize {
        sorted_dir(&self.sys.join("kernel/iommu_groups")).len()
    }

    fn one(&self, d: &Path) -> PciDevice {
        let address = base(d);
        let driver = std::fs::read_link(d.join("driver")).ok().map(|p| base(&p));
        let group = std::fs::read_link(d.join("iommu_group"))
            .ok()
            .map(|p| base(&p))
            .or_else(|| {
                d.join("iommu_group").is_dir().then(|| {
                    base(&std::fs::canonicalize(d.join("iommu_group")).unwrap_or_default())
                })
            });
        let class = read(&d.join("class"));
        let vendor_id = read(&d.join("vendor")).trim_start_matches("0x").to_string();
        let device_id = read(&d.join("device")).trim_start_matches("0x").to_string();
        let group_members: Vec<String> = group
            .as_ref()
            .map(|g| {
                sorted_dir(&self.sys.join("kernel/iommu_groups").join(g).join("devices"))
                    .iter()
                    .map(|p| base(p))
                    .filter(|m| *m != address)
                    .collect()
            })
            .unwrap_or_default();
        let group_blockers = group_members
            .iter()
            .filter(|m| {
                let md = self.devs().join(m);
                if class_name(&read(&md.join("class"))) == "bridge" {
                    return false;
                }
                !matches!(
                    std::fs::read_link(md.join("driver"))
                        .ok()
                        .map(|p| base(&p))
                        .as_deref(),
                    Some("vfio-pci") | Some("pci-stub")
                )
            })
            .cloned()
            .collect();
        PciDevice {
            vendor_name: vendor_name(&vendor_id).to_string(),
            vfio: driver.as_deref() == Some("vfio-pci"),
            class_name: class_name(&class).to_string(),
            sriov_vfs: read(&d.join("sriov_numvfs")).parse().unwrap_or(0),
            sriov_max_vfs: read(&d.join("sriov_totalvfs")).parse().unwrap_or(0),
            physfn: std::fs::read_link(d.join("physfn")).ok().map(|p| base(&p)),
            numa_node: read(&d.join("numa_node"))
                .parse::<i32>()
                .ok()
                .filter(|n| *n >= 0),
            address,
            vendor_id,
            device_id,
            class,
            driver,
            iommu_group: group,
            group_members,
            group_blockers,
        }
    }

    /// Every PCI device, sorted by address.
    pub fn list(&self) -> Vec<PciDevice> {
        sorted_dir(&self.devs())
            .iter()
            .map(|d| self.one(d))
            .collect()
    }

    fn address(addr: &str) -> Result<String> {
        match (VmHostPci {
            device: addr.to_string(),
            primary_gpu: false,
        })
        .kind()
        {
            Ok(HostPciKind::Pci(a)) => Ok(a),
            _ => Err(Error::Validation(format!(
                "{addr:?} is not a PCI address (0000:01:00.0)"
            ))),
        }
    }

    fn existing(&self, addr: &str) -> Result<(String, PathBuf)> {
        let a = Self::address(addr)?;
        let d = self.devs().join(&a);
        if !d.exists() {
            return Err(Error::NotFound(format!("PCI device {a} does not exist")));
        }
        Ok((a, d))
    }

    /// Bind one device to `vfio-pci`.
    pub fn bind_vfio(&self, addr: &str) -> Result<PciDevice> {
        let (a, d) = self.existing(addr)?;
        if class_name(&read(&d.join("class"))) == "bridge" {
            return Err(Error::Validation(format!(
                "{a} is a PCI bridge: bridges are never passed through"
            )));
        }
        if !d.join("iommu_group").exists() {
            return Err(Error::Validation(format!(
                "{a} is in no IOMMU group: enable the IOMMU (intel_iommu=on / amd_iommu=on) and reboot"
            )));
        }
        if !self.sys.join("bus/pci/drivers/vfio-pci").exists() {
            return Err(Error::Validation(
                "the vfio-pci driver is not loaded (modprobe vfio-pci)".into(),
            ));
        }
        let w = |p: PathBuf, v: &str| {
            std::fs::write(&p, v)
                .map_err(|e| Error::System(format!("writing {}: {e}", p.display())))
        };
        let current = std::fs::read_link(d.join("driver")).ok().map(|p| base(&p));
        if current.as_deref() != Some("vfio-pci") {
            w(d.join("driver_override"), "vfio-pci")?;
            if let Some(cur) = current {
                w(
                    self.sys.join("bus/pci/drivers").join(cur).join("unbind"),
                    &a,
                )?;
            }
            w(self.sys.join("bus/pci/drivers_probe"), &a)?;
        }
        Ok(self.one(&d))
    }

    /// Give a device back to its host driver.
    pub fn unbind_vfio(&self, addr: &str) -> Result<PciDevice> {
        let (a, d) = self.existing(addr)?;
        let w = |p: PathBuf, v: &str| {
            std::fs::write(&p, v)
                .map_err(|e| Error::System(format!("writing {}: {e}", p.display())))
        };
        let current = std::fs::read_link(d.join("driver")).ok().map(|p| base(&p));
        if current.as_deref() == Some("vfio-pci") {
            w(self.sys.join("bus/pci/drivers/vfio-pci/unbind"), &a)?;
        }
        // an empty override lets the kernel pick the normal driver again
        w(d.join("driver_override"), "\n")?;
        w(self.sys.join("bus/pci/drivers_probe"), &a)?;
        Ok(self.one(&d))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    fn dev(root: &Path, addr: &str, class: &str, vendor: &str, driver: Option<&str>, group: &str) {
        let d = root.join("bus/pci/devices").join(addr);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("class"), format!("{class}\n")).unwrap();
        fs::write(d.join("vendor"), format!("{vendor}\n")).unwrap();
        fs::write(d.join("device"), "0x1234\n").unwrap();
        let g = root.join("kernel/iommu_groups").join(group);
        fs::create_dir_all(g.join("devices")).unwrap();
        symlink(&g, d.join("iommu_group")).unwrap();
        symlink(&d, g.join("devices").join(addr)).unwrap();
        if let Some(drv) = driver {
            let dd = root.join("bus/pci/drivers").join(drv);
            fs::create_dir_all(&dd).unwrap();
            symlink(&dd, d.join("driver")).unwrap();
        }
    }

    fn fixture() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        fs::create_dir_all(r.join("bus/pci/drivers/vfio-pci")).unwrap();
        // GPU + its audio function in group 12, a NIC alone in group 20,
        // a bridge in group 1
        dev(
            r,
            "0000:01:00.0",
            "0x030000",
            "0x10de",
            Some("vfio-pci"),
            "12",
        );
        dev(
            r,
            "0000:01:00.1",
            "0x040300",
            "0x10de",
            Some("snd_hda_intel"),
            "12",
        );
        dev(
            r,
            "0000:41:00.0",
            "0x020000",
            "0x15b3",
            Some("mlx5_core"),
            "20",
        );
        dev(r, "0000:00:01.0", "0x060400", "0x8086", None, "1");
        fs::write(r.join("bus/pci/devices/0000:41:00.0/sriov_numvfs"), "4\n").unwrap();
        fs::write(r.join("bus/pci/devices/0000:41:00.0/sriov_totalvfs"), "8\n").unwrap();
        t
    }

    #[test]
    fn lists_class_driver_group_and_blockers() {
        let t = fixture();
        let l = PciManager::with_sysfs(t.path()).list();
        assert_eq!(l.len(), 4);
        let gpu = l.iter().find(|d| d.address == "0000:01:00.0").unwrap();
        assert_eq!(gpu.class_name, "gpu");
        assert_eq!(gpu.vendor_name, "NVIDIA");
        assert!(gpu.vfio);
        assert_eq!(gpu.iommu_group.as_deref(), Some("12"));
        assert_eq!(gpu.group_members, vec!["0000:01:00.1"]);
        // the audio function is still on the host driver
        assert_eq!(gpu.group_blockers, vec!["0000:01:00.1"]);
        let nic = l.iter().find(|d| d.address == "0000:41:00.0").unwrap();
        assert_eq!(
            (nic.class_name.as_str(), nic.vendor_name.as_str()),
            ("network", "NVIDIA Mellanox")
        );
        assert_eq!((nic.sriov_vfs, nic.sriov_max_vfs), (4, 8));
        assert!(nic.group_blockers.is_empty() && !nic.vfio);
        assert_eq!(
            l.iter()
                .find(|d| d.address == "0000:00:01.0")
                .unwrap()
                .class_name,
            "bridge"
        );
        assert_eq!(PciManager::with_sysfs(t.path()).iommu_groups(), 3);
    }

    #[test]
    fn bind_uses_driver_override_and_unbinds_the_old_driver() {
        let t = fixture();
        let m = PciManager::with_sysfs(t.path());
        m.bind_vfio("41:00.0").unwrap();
        let d = t.path().join("bus/pci/devices/0000:41:00.0");
        assert_eq!(read(&d.join("driver_override")), "vfio-pci");
        assert_eq!(
            read(&t.path().join("bus/pci/drivers/mlx5_core/unbind")),
            "0000:41:00.0"
        );
        assert_eq!(
            read(&t.path().join("bus/pci/drivers_probe")),
            "0000:41:00.0"
        );
        // already on vfio: nothing is written
        let before = read(&t.path().join("bus/pci/drivers_probe"));
        fs::write(t.path().join("bus/pci/drivers_probe"), "").unwrap();
        m.bind_vfio("0000:01:00.0").unwrap();
        assert_eq!(read(&t.path().join("bus/pci/drivers_probe")), "");
        assert!(!before.is_empty());
        m.unbind_vfio("0000:01:00.0").unwrap();
        assert_eq!(
            read(&t.path().join("bus/pci/drivers/vfio-pci/unbind")),
            "0000:01:00.0"
        );
        assert_eq!(
            fs::read_to_string(
                t.path()
                    .join("bus/pci/devices/0000:01:00.0/driver_override")
            )
            .unwrap(),
            "\n"
        );
    }

    #[test]
    fn bind_refuses_bridges_bad_addresses_missing_devices_and_no_iommu() {
        let t = fixture();
        let m = PciManager::with_sysfs(t.path());
        for (a, what) in [
            ("0000:00:01.0", "bridge"),
            ("../x", "not a PCI address"),
            ("0000:aa:00.0", "does not exist"),
        ] {
            let e = m.bind_vfio(a).unwrap_err().to_string();
            assert!(e.contains(what), "{a}: {e}");
        }
        // no IOMMU group
        fs::remove_file(t.path().join("bus/pci/devices/0000:41:00.0/iommu_group")).unwrap();
        assert!(m
            .bind_vfio("0000:41:00.0")
            .unwrap_err()
            .to_string()
            .contains("IOMMU"));
        // no vfio-pci driver
        fs::remove_dir_all(t.path().join("bus/pci/drivers/vfio-pci")).unwrap();
        assert!(m
            .bind_vfio("0000:01:00.1")
            .unwrap_err()
            .to_string()
            .contains("modprobe vfio-pci"));
    }
}
