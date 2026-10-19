//! VM hardware edits and power actions beyond start/stop: Proxmox `qm set`
//! (name, CPUs, memory, network cards, CD-ROM, boot order), live CD-ROM
//! media changes, and `qm shutdown` / `qm reboot` / `qm reset`.
use super::display::{qmp_command, DisplayRuntime};
use super::{qemu, QemuManager, VmManager};
use horcrux_common::{Error, Result, VmConfig, VmHardware, VmHypervisor, VmStatus};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// A random locally administered unicast MAC (`x2:..`, like Proxmox's
/// generated addresses: never collides with a vendor-assigned one).
pub fn generate_mac() -> String {
    use rand::Rng;
    let mut b = [0u8; 6];
    rand::thread_rng().fill(&mut b);
    b[0] = (b[0] & 0xfc) | 0x02;
    b.iter()
        .map(|x| format!("{x:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Validate a VM's hardware, normalize its MAC addresses and generate the
/// missing ones. Hardware settings exist for QEMU VMs only; an LXD/Incus VM
/// with any of them is refused rather than silently ignoring them.
pub fn prepare_hardware(config: &mut VmConfig) -> Result<()> {
    if config.hypervisor != VmHypervisor::Qemu && config.hardware != VmHardware::default() {
        return Err(Error::Validation(
            "Network cards, CD-ROM, boot order, CPU type, chipset, firmware and TPM are \
             QEMU VM settings; LXD/Incus VMs get them from the LXD/Incus profile"
                .into(),
        ));
    }
    config
        .hardware
        .validate_for(&config.architecture, config.cpus)
        .map_err(Error::Validation)?;
    let mut used: Vec<String> = Vec::new();
    for nic in &mut config.hardware.nets {
        if let Some(m) = &nic.mac {
            let m = horcrux_common::normalize_mac(m).map_err(Error::Validation)?;
            used.push(m.clone());
            nic.mac = Some(m);
        }
    }
    for nic in &mut config.hardware.nets {
        if nic.mac.is_none() {
            let mut m = generate_mac();
            while used.contains(&m) {
                m = generate_mac();
            }
            used.push(m.clone());
            nic.mac = Some(m);
        }
    }
    Ok(())
}

/// `PUT /api/vms/:id/config`: the fields to change (absent = keep).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VmConfigUpdate {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub cpus: Option<u32>,
    /// MB
    #[serde(default)]
    pub memory: Option<u64>,
    #[serde(default)]
    pub hardware: Option<VmHardware>,
}

impl VmManager {
    /// Change a VM's name, CPUs, memory and hardware (`qm set`). Only the
    /// name and the CD-ROM can change while the VM runs; everything else
    /// needs it stopped and applies at the next start.
    pub async fn update_config(&self, id: &str, upd: VmConfigUpdate) -> Result<VmConfig> {
        let cdrom_live = {
            let mut vms = self.vms.write().await;
            self.hydrate(&mut vms, id).await?;
            let vm = vms
                .get_mut(id)
                .ok_or_else(|| Error::VmNotFound(id.to_string()))?;
            let running = vm.hypervisor == VmHypervisor::Qemu
                && QemuManager::process_running(&vm.id)
                || vm.hypervisor != VmHypervisor::Qemu && vm.status == VmStatus::Running;

            let mut next = vm.to_config();
            if let Some(name) = &upd.name {
                let name = name.trim();
                if name.is_empty() || name.len() > 128 {
                    return Err(Error::Validation("name must be 1-128 characters".into()));
                }
                next.name = name.to_string();
            }
            if let Some(c) = upd.cpus {
                if !(1..=512).contains(&c) {
                    return Err(Error::Validation("cpus must be 1-512".into()));
                }
                next.cpus = c;
            }
            if let Some(m) = upd.memory {
                if m < 16 {
                    return Err(Error::Validation("memory must be at least 16 MB".into()));
                }
                next.memory = m;
            }
            if let Some(hw) = &upd.hardware {
                next.hardware = hw.clone();
            }
            prepare_hardware(&mut next)?;

            if vm.hypervisor != VmHypervisor::Qemu
                && (next.cpus != vm.cpus || next.memory != vm.memory)
            {
                return Err(Error::Validation(
                    "CPU and memory of LXD/Incus VMs are managed by LXD/Incus".into(),
                ));
            }
            let mut hw_without_cdrom = next.hardware.clone();
            hw_without_cdrom.cdrom = vm.hardware.cdrom.clone();
            let cold_change =
                next.cpus != vm.cpus || next.memory != vm.memory || hw_without_cdrom != vm.hardware;
            if running && cold_change {
                return Err(Error::Conflict(
                    "Stop the VM before changing CPUs, memory, network cards or boot order \
                     (the name and the CD-ROM can change while it runs)"
                        .into(),
                ));
            }
            let cdrom_changed = next.hardware.cdrom != vm.hardware.cdrom;
            if running && cdrom_changed {
                // Insert/eject in the running QEMU first; only record the
                // new medium once QEMU took it.
                Some((vm.disk_path.clone(), next.hardware.cdrom.clone()))
            } else {
                vm.name = next.name;
                vm.cpus = next.cpus;
                vm.memory = next.memory;
                vm.hardware = next.hardware;
                let config = vm.to_config();
                if let Some(db) = &self.db {
                    db.update_vm(&config).await?;
                }
                return Ok(config);
            }
        };
        let (disk, iso) = cdrom_live.expect("live CD-ROM change");
        change_medium(&disk, iso.as_deref()).await?;
        let mut vms = self.vms.write().await;
        let vm = vms
            .get_mut(id)
            .ok_or_else(|| Error::VmNotFound(id.to_string()))?;
        if let Some(name) = upd.name {
            vm.name = name.trim().to_string();
        }
        vm.hardware.cdrom = iso;
        let config = vm.to_config();
        if let Some(db) = &self.db {
            db.update_vm(&config).await?;
        }
        Ok(config)
    }

    /// Clone of a QEMU VM's runtime handle, for actions that must not hold
    /// the VM map's lock while they wait for the guest.
    async fn qemu_vm(&self, id: &str) -> Result<qemu::QemuVm> {
        let mut vms = self.vms.write().await;
        self.hydrate(&mut vms, id).await?;
        let vm = vms
            .get(id)
            .ok_or_else(|| Error::VmNotFound(id.to_string()))?;
        if vm.hypervisor != VmHypervisor::Qemu {
            return Err(Error::Validation(
                "This action is for QEMU VMs; use Stop/Start for LXD/Incus VMs".into(),
            ));
        }
        Ok(vm.clone())
    }

    async fn record_status(&self, id: &str, status: VmStatus) -> Result<VmConfig> {
        let mut vms = self.vms.write().await;
        let vm = vms
            .get_mut(id)
            .ok_or_else(|| Error::VmNotFound(id.to_string()))?;
        vm.status = status;
        let config = vm.to_config();
        if let Some(db) = &self.db {
            db.update_vm(&config).await?;
        }
        Ok(config)
    }

    /// Clean shutdown (`qm shutdown`): ACPI power button, then wait up to
    /// `timeout` for the guest to power off. If it doesn't, `force` stops
    /// it like Stop does; otherwise the VM keeps running and Conflict says
    /// so.
    pub async fn shutdown_vm(&self, id: &str, timeout: Duration, force: bool) -> Result<VmConfig> {
        let vm = self.qemu_vm(id).await?;
        if !QemuManager::process_running(&vm.id) {
            return Err(Error::Conflict(format!("VM {id} is not running")));
        }
        qmp_command(
            &DisplayRuntime::qmp_socket(&vm.disk_path),
            serde_json::json!({"execute": "system_powerdown"}),
        )
        .await?;
        let deadline = tokio::time::Instant::now() + timeout;
        while tokio::time::Instant::now() < deadline {
            if !QemuManager::process_running(&vm.id) {
                qemu::cleanup_after_exit(&vm).await;
                return self.record_status(id, VmStatus::Stopped).await;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        if !QemuManager::process_running(&vm.id) {
            qemu::cleanup_after_exit(&vm).await;
            return self.record_status(id, VmStatus::Stopped).await;
        }
        if force {
            tracing::warn!(
                "VM {id} did not power off within {}s, stopping it",
                timeout.as_secs()
            );
            return self.stop_vm(id).await;
        }
        Err(Error::Conflict(format!(
            "VM {id} did not shut down within {} s (the guest ignored the ACPI power button); \
             it is still running",
            timeout.as_secs()
        )))
    }

    /// Reboot (`qm reboot`): clean shutdown (stopped after `timeout` if the
    /// guest ignores it), then start again, so pending hardware changes
    /// take effect.
    pub async fn reboot_vm(&self, id: &str, timeout: Duration) -> Result<VmConfig> {
        self.shutdown_vm(id, timeout, true).await?;
        self.start_vm(id).await
    }

    /// Hard reset (`qm reset`): like pressing the reset button.
    pub async fn reset_vm(&self, id: &str) -> Result<VmConfig> {
        let vm = self.qemu_vm(id).await?;
        if !QemuManager::process_running(&vm.id) {
            return Err(Error::Conflict(format!("VM {id} is not running")));
        }
        qmp_command(
            &DisplayRuntime::qmp_socket(&vm.disk_path),
            serde_json::json!({"execute": "system_reset"}),
        )
        .await?;
        Ok(vm.to_config())
    }

    /// Add a new, empty disk to a stopped QEMU VM (`qm set --scsi1 local:32`):
    /// a qcow2 image of `size_gb` in the VM storage directory, attached on
    /// the given bus at the next start.
    pub async fn add_disk(&self, id: &str, spec: DiskSpec) -> Result<VmConfig> {
        let mut vms = self.vms.write().await;
        self.hydrate(&mut vms, id).await?;
        let vm = vms
            .get_mut(id)
            .ok_or_else(|| Error::VmNotFound(id.to_string()))?;
        require_stopped_qemu(vm, "adding a disk")?;
        let mut disks = vm.all_disks();
        if disks.len() >= horcrux_common::MAX_VM_DISKS {
            return Err(Error::Validation(format!(
                "a VM can have at most {} disks",
                horcrux_common::MAX_VM_DISKS
            )));
        }
        if spec.size_gb == 0 || spec.size_gb > 65536 {
            return Err(Error::Validation("size must be 1-65536 GB".into()));
        }
        // The first free <id>-disk<N>.qcow2 (a removed disk's number may
        // be reused once its file is gone).
        let mut n = disks.len();
        let path = loop {
            let p = self.qemu.disk_file(id, n);
            if !p.exists() && !disks.iter().any(|d| d.path == p.to_string_lossy()) {
                break p;
            }
            n += 1;
        };
        let disk = horcrux_common::VmDisk {
            path: path.to_string_lossy().to_string(),
            size_gb: spec.size_gb,
            disk_type: spec.bus.clone(),
            cache: spec.cache.clone(),
            discard: spec.discard,
            ssd: spec.ssd,
            iothread: spec.iothread,
        };
        disk.validate(disks.len()).map_err(Error::Validation)?;
        disks.push(disk);
        qemu::validate_disk_set(&disks).map_err(Error::Validation)?;
        QemuManager::create_disk_image(&path, spec.size_gb).await?;
        vm.disks = disks;
        let config = vm.to_config();
        if let Some(db) = &self.db {
            if let Err(e) = db.update_vm(&config).await {
                let _ = tokio::fs::remove_file(&path).await;
                vm.disks.pop();
                return Err(e);
            }
        }
        Ok(config)
    }

    /// Change a disk's bus/cache/options (stopped VM) and/or grow it
    /// (`qm resize`; also while the VM runs, through QMP `block_resize`).
    pub async fn update_disk(&self, id: &str, index: usize, upd: DiskUpdate) -> Result<VmConfig> {
        let (live_resize, new_size) = {
            let mut vms = self.vms.write().await;
            self.hydrate(&mut vms, id).await?;
            let vm = vms
                .get_mut(id)
                .ok_or_else(|| Error::VmNotFound(id.to_string()))?;
            if vm.hypervisor != VmHypervisor::Qemu {
                return Err(Error::Validation(
                    "disks of LXD/Incus VMs are managed by LXD/Incus".into(),
                ));
            }
            let running = QemuManager::process_running(&vm.id);
            let mut disks = vm.all_disks();
            let Some(cur) = disks.get(index).cloned() else {
                return Err(Error::NotFound(format!("VM {id} has no disk{index}")));
            };
            let mut next = cur.clone();
            if let Some(b) = &upd.bus {
                next.disk_type = b.clone();
            }
            if let Some(c) = &upd.cache {
                next.cache = c.clone();
            }
            if let Some(v) = upd.discard {
                next.discard = v;
            }
            if let Some(v) = upd.ssd {
                next.ssd = v;
            }
            if let Some(v) = upd.iothread {
                next.iothread = v;
            }
            next.validate(index).map_err(Error::Validation)?;
            let options_changed = {
                let mut a = next.clone();
                a.size_gb = cur.size_gb;
                a != cur
            };
            if running && options_changed {
                return Err(Error::Conflict(
                    "Stop the VM before changing a disk's bus, cache or options (a running \
                     disk can only be grown)"
                        .into(),
                ));
            }
            let grow = match upd.size_gb {
                Some(s) if s < cur.size_gb => {
                    return Err(Error::Validation(format!(
                    "disk{index} is {} GB: disks can only be grown (shrinking would cut off the \
                         guest's file systems)",
                    cur.size_gb
                )))
                }
                Some(s) if s > 65536 => {
                    return Err(Error::Validation("size must be at most 65536 GB".into()))
                }
                Some(s) if s > cur.size_gb => Some(s),
                _ => None,
            };
            disks[index] = next.clone();
            qemu::validate_disk_set(&disks).map_err(Error::Validation)?;
            if let Some(s) = grow {
                if !running {
                    resize_image(&next.path, s).await?;
                    disks[index].size_gb = s;
                }
            }
            if running && grow.is_some() {
                (Some(vm.disk_path.clone()), grow)
            } else {
                vm.disks = disks;
                if index == 0 {
                    vm.disk_size = vm.disks[0].size_gb;
                }
                let config = vm.to_config();
                if let Some(db) = &self.db {
                    db.update_vm(&config).await?;
                }
                return Ok(config);
            }
        };
        // Grow the disk inside the running QEMU (the guest sees the new size).
        let (Some(disk0), Some(size)) = (live_resize, new_size) else {
            unreachable!("live resize path");
        };
        qmp_command(
            &DisplayRuntime::qmp_socket(&disk0),
            serde_json::json!({
                "execute": "block_resize",
                "arguments": {"device": qemu::disk_drive_id(index), "size": size * 1024 * 1024 * 1024}
            }),
        )
        .await?;
        let mut vms = self.vms.write().await;
        let vm = vms
            .get_mut(id)
            .ok_or_else(|| Error::VmNotFound(id.to_string()))?;
        let mut disks = vm.all_disks();
        disks[index].size_gb = size;
        vm.disks = disks;
        if index == 0 {
            vm.disk_size = size;
        }
        let config = vm.to_config();
        if let Some(db) = &self.db {
            db.update_vm(&config).await?;
        }
        Ok(config)
    }

    /// Detach a disk from a stopped VM (not the first one: the VM's console
    /// sockets, EFI vars and TPM state live next to it). `delete_file`
    /// also deletes the image; otherwise it stays on disk.
    pub async fn remove_disk(&self, id: &str, index: usize, delete_file: bool) -> Result<VmConfig> {
        let mut vms = self.vms.write().await;
        self.hydrate(&mut vms, id).await?;
        let vm = vms
            .get_mut(id)
            .ok_or_else(|| Error::VmNotFound(id.to_string()))?;
        require_stopped_qemu(vm, "removing a disk")?;
        let mut disks = vm.all_disks();
        if index == 0 {
            return Err(Error::Validation(
                "disk0 is the VM's main disk and can't be removed (delete the VM instead)".into(),
            ));
        }
        if index >= disks.len() {
            return Err(Error::NotFound(format!("VM {id} has no disk{index}")));
        }
        let removed = disks.remove(index);
        vm.disks = disks;
        let config = vm.to_config();
        if let Some(db) = &self.db {
            db.update_vm(&config).await?;
        }
        if delete_file {
            if let Err(e) = tokio::fs::remove_file(&removed.path).await {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!("VM {id}: could not delete {}: {e}", removed.path);
                }
            }
        }
        Ok(config)
    }
}

fn require_stopped_qemu(vm: &qemu::QemuVm, what: &str) -> Result<()> {
    if vm.hypervisor != VmHypervisor::Qemu {
        return Err(Error::Validation(
            "disks of LXD/Incus VMs are managed by LXD/Incus".into(),
        ));
    }
    if QemuManager::process_running(&vm.id) {
        return Err(Error::Conflict(format!("Stop the VM before {what}")));
    }
    Ok(())
}

async fn resize_image(path: &str, size_gb: u64) -> Result<()> {
    let out = tokio::process::Command::new("qemu-img")
        .args(["resize", path, &format!("{size_gb}G")])
        .output()
        .await
        .map_err(|e| Error::System(format!("qemu-img: {e}")))?;
    if !out.status.success() {
        return Err(Error::System(format!(
            "qemu-img resize {path}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// `POST /api/vms/:id/disks`: a new disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskSpec {
    pub size_gb: u64,
    #[serde(default = "default_bus")]
    pub bus: String,
    #[serde(default)]
    pub cache: String,
    #[serde(default)]
    pub discard: bool,
    #[serde(default)]
    pub ssd: bool,
    #[serde(default)]
    pub iothread: bool,
}

fn default_bus() -> String {
    "virtio".into()
}

/// `PUT /api/vms/:id/disks/:index`: fields to change (absent = keep).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiskUpdate {
    #[serde(default)]
    pub size_gb: Option<u64>,
    #[serde(default)]
    pub bus: Option<String>,
    #[serde(default)]
    pub cache: Option<String>,
    #[serde(default)]
    pub discard: Option<bool>,
    #[serde(default)]
    pub ssd: Option<bool>,
    #[serde(default)]
    pub iothread: Option<bool>,
}

/// Insert (`Some`) or eject (`None`) the medium of a running VM's CD-ROM.
async fn change_medium(disk: &std::path::Path, iso: Option<&str>) -> Result<()> {
    let sock = DisplayRuntime::qmp_socket(disk);
    let cmd = match iso {
        Some(path) => serde_json::json!({
            "execute": "blockdev-change-medium",
            "arguments": {
                "id": qemu::CDROM_DEVICE_ID,
                "filename": path,
                "format": "raw",
                "read-only-mode": "read-only"
            }
        }),
        None => serde_json::json!({
            "execute": "eject",
            "arguments": {"id": qemu::CDROM_DEVICE_ID, "force": true}
        }),
    };
    qmp_command(&sock, cmd).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use horcrux_common::{NicModel, VmNic};

    fn cfg(hypervisor: VmHypervisor) -> VmConfig {
        VmConfig {
            id: "hw".into(),
            name: "hw".into(),
            hypervisor,
            memory: 512,
            cpus: 1,
            disk_size: 1,
            status: VmStatus::Stopped,
            architecture: Default::default(),
            disks: vec![],
            display: Default::default(),
            hardware: Default::default(),
        }
    }

    fn nic(mac: Option<&str>) -> VmNic {
        VmNic {
            model: NicModel::Virtio,
            bridge: "br0".into(),
            mac: mac.map(Into::into),
            vlan: None,
        }
    }

    #[test]
    fn generated_macs_are_local_unicast() {
        for _ in 0..200 {
            let m = generate_mac();
            let first = u8::from_str_radix(&m[..2], 16).unwrap();
            assert_eq!(first & 1, 0, "{m} multicast");
            assert_eq!(first & 2, 2, "{m} not locally administered");
            assert_eq!(horcrux_common::normalize_mac(&m).unwrap(), m);
        }
    }

    #[test]
    fn prepare_fills_missing_macs_and_keeps_given_ones() {
        let mut c = cfg(VmHypervisor::Qemu);
        c.hardware.nets = vec![nic(Some("02:AA:00:00:00:01")), nic(None), nic(None)];
        prepare_hardware(&mut c).unwrap();
        let macs: Vec<_> = c
            .hardware
            .nets
            .iter()
            .map(|n| n.mac.clone().unwrap())
            .collect();
        assert_eq!(macs[0], "02:aa:00:00:00:01");
        assert!(macs[1] != macs[2] && macs[1] != macs[0]);
    }

    #[test]
    fn hardware_on_lxd_or_incus_vms_is_refused() {
        let mut c = cfg(VmHypervisor::Incus);
        assert!(prepare_hardware(&mut c).is_ok());
        c.hardware.nets.push(nic(None));
        assert!(prepare_hardware(&mut c).is_err());
    }
}
