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
            "Network cards, CD-ROM and boot order are QEMU VM settings; LXD/Incus VMs \
             get their network from the LXD/Incus profile"
                .into(),
        ));
    }
    config.hardware.validate().map_err(Error::Validation)?;
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
