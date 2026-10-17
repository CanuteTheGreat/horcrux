//! VM consoles: VNC (noVNC over the ticketed WebSocket proxy), SPICE
//! (WebSocket proxy for browser clients + `.vv` file for remote-viewer) and
//! the serial console (WebSocket bridge to QEMU's serial Unix socket).
//!
//! LXD and Incus VMs get the same serial and SPICE consoles through the
//! daemon's instance console API (`console::lxd`, `console::spice_relay`).
//!
//! Every endpoint comes from what `vm::display` actually configured when the
//! VM was started (`<disk>.display.json`); nothing is allocated here. (This
//! module used to hand out VNC/SPICE ports and a serial socket path that
//! QEMU was never told about, so no console could ever connect.)

#[cfg(any(feature = "lxd", feature = "incus"))]
pub mod lxd;
pub mod novnc;
pub mod serial_ws;
#[cfg(any(feature = "lxd", feature = "incus"))]
pub mod spice_relay;

use crate::vm::display::DisplayRuntime;
use horcrux_common::{DisplayProtocol, Result, VmConfig, VmHypervisor};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ConsoleType {
    Vnc,
    Spice,
    Serial,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsoleInfo {
    pub vm_id: String,
    pub console_type: ConsoleType,
    /// Address QEMU listens on for this console (127.0.0.1 unless
    /// HORCRUX_SPICE_LISTEN says otherwise). Port 0 for serial.
    pub host: String,
    pub port: u16,
    pub ticket: String,
    /// Kept for API compatibility; the WebSocket proxy is served by the API
    /// itself at `ws_path`.
    pub ws_port: u16,
    /// Same-origin WebSocket path for this ticket.
    #[serde(default)]
    pub ws_path: String,
    /// VNC/SPICE password for this VM run (set over QMP at start).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ConsoleTicket {
    pub ticket_id: String,
    pub vm_id: String,
    pub console_type: ConsoleType,
    /// TCP target port of the WebSocket proxy (VNC or SPICE).
    pub vnc_port: u16,
    pub target_host: String,
    /// Unix socket of the serial console.
    pub serial_socket: Option<PathBuf>,
    /// LXD/Incus VM whose console (via the daemon's API) this ticket opens.
    #[cfg_attr(not(any(feature = "lxd", feature = "incus")), allow(dead_code))]
    pub instance: Option<InstanceTarget>,
    pub created_at: i64,
    pub expires_at: i64,
}

/// An LXD or Incus instance, addressed by name.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(any(feature = "lxd", feature = "incus")), allow(dead_code))]
pub struct InstanceTarget {
    pub hypervisor: VmHypervisor,
    pub name: String,
}

#[cfg(any(feature = "lxd", feature = "incus"))]
impl InstanceTarget {
    /// The daemon backend, or "not compiled in" for a USE flag that is off.
    pub fn backend(&self) -> Result<lxd::Backend> {
        match self.hypervisor {
            #[cfg(feature = "lxd")]
            VmHypervisor::Lxd => Ok(lxd::Backend::Lxd),
            #[cfg(feature = "incus")]
            VmHypervisor::Incus => Ok(lxd::Backend::Incus),
            ref h => Err(crate::features::hypervisor_not_compiled(h)),
        }
    }
}

pub struct ConsoleManager {
    active_tickets: Arc<RwLock<HashMap<String, ConsoleTicket>>>,
}

impl Default for ConsoleManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Disk of a QEMU VM (its console state lives next to it). LXD/Incus VMs
/// have no such state: their consoles come from the daemon.
pub fn runtime_disk(vm: &VmConfig) -> Result<PathBuf> {
    if vm.hypervisor != VmHypervisor::Qemu {
        return Err(horcrux_common::Error::Validation(format!(
            "VM {} is not a QEMU VM; its console state is kept by {:?}",
            vm.id, vm.hypervisor
        )));
    }
    vm.disks
        .first()
        .map(|d| PathBuf::from(&d.path))
        .ok_or_else(|| horcrux_common::Error::InvalidConfig(format!("VM {} has no disk", vm.id)))
}

impl ConsoleManager {
    pub fn new() -> Self {
        Self {
            active_tickets: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Issue a 5-minute ticket for one of the running VM's consoles.
    pub async fn create_console(&self, vm: &VmConfig, kind: ConsoleType) -> Result<ConsoleInfo> {
        match vm.hypervisor {
            VmHypervisor::Qemu => self.create_qemu_console(vm, kind).await,
            VmHypervisor::Lxd | VmHypervisor::Incus => self.create_instance_console(vm, kind).await,
        }
    }

    /// Serial and SPICE consoles of an LXD/Incus VM. The serial ticket is
    /// bridged to the daemon's `type=console` session; SPICE goes through
    /// a password-protected relay (`spice_relay`), so the `.vv` file and the
    /// SPICE WebSocket proxy work exactly as for QEMU VMs.
    #[cfg(any(feature = "lxd", feature = "incus"))]
    async fn create_instance_console(
        &self,
        vm: &VmConfig,
        kind: ConsoleType,
    ) -> Result<ConsoleInfo> {
        let target = InstanceTarget {
            hypervisor: vm.hypervisor.clone(),
            name: vm.name.clone(),
        };
        let backend = target.backend()?;
        let state = lxd::Client::new(backend).instance_state(&vm.name).await?;
        if !state.is_running() {
            return Err(horcrux_common::Error::Validation(format!(
                "VM {} is {} - start it to open its console",
                vm.id,
                state.status.to_lowercase()
            )));
        }
        match kind {
            ConsoleType::Serial => Ok(self
                .issue(vm, kind, "127.0.0.1".into(), 0, None, None, Some(target))
                .await),
            ConsoleType::Spice => {
                let relay = spice_relay::ensure(backend, &vm.name).await?;
                Ok(self
                    .issue(
                        vm,
                        kind,
                        relay.listen.clone(),
                        relay.port,
                        Some(relay.password),
                        None,
                        None,
                    )
                    .await)
            }
            ConsoleType::Vnc => Err(horcrux_common::Error::Validation(format!(
                "{} VMs have a SPICE graphical console and a serial console (no VNC): \
                 choose SPICE or Serial",
                backend.label()
            ))),
        }
    }

    #[cfg(not(any(feature = "lxd", feature = "incus")))]
    async fn create_instance_console(
        &self,
        vm: &VmConfig,
        _kind: ConsoleType,
    ) -> Result<ConsoleInfo> {
        Err(crate::features::hypervisor_not_compiled(&vm.hypervisor))
    }

    async fn create_qemu_console(&self, vm: &VmConfig, kind: ConsoleType) -> Result<ConsoleInfo> {
        let disk = runtime_disk(vm)?;
        let rt = DisplayRuntime::load(&disk).await?;
        let not_configured = |what: &str| {
            horcrux_common::Error::Validation(format!(
                "VM {} has no {} console (display protocol is {:?}); change the VM's display \
                 settings and restart it",
                vm.id, what, rt.protocol
            ))
        };
        let (port, host, password, serial_socket) = match kind {
            ConsoleType::Vnc => {
                if rt.protocol != DisplayProtocol::Vnc {
                    return Err(not_configured("VNC"));
                }
                let port = rt.vnc_port.ok_or_else(|| not_configured("VNC"))?;
                (port, "127.0.0.1".to_string(), rt.password.clone(), None)
            }
            ConsoleType::Spice => {
                if rt.protocol != DisplayProtocol::Spice {
                    return Err(not_configured("SPICE"));
                }
                let port = rt.spice_port.ok_or_else(|| not_configured("SPICE"))?;
                let listen = rt
                    .spice_listen
                    .clone()
                    .unwrap_or_else(|| "127.0.0.1".into());
                (port, listen, rt.password.clone(), None)
            }
            ConsoleType::Serial => (
                0,
                "127.0.0.1".to_string(),
                None,
                Some(rt.serial_socket.clone()),
            ),
        };

        Ok(self
            .issue(vm, kind, host, port, password, serial_socket, None)
            .await)
    }

    #[allow(clippy::too_many_arguments)]
    async fn issue(
        &self,
        vm: &VmConfig,
        kind: ConsoleType,
        host: String,
        port: u16,
        password: Option<String>,
        serial_socket: Option<PathBuf>,
        instance: Option<InstanceTarget>,
    ) -> ConsoleInfo {
        let ticket_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp();
        let target_host = match host.as_str() {
            "0.0.0.0" | "::" | "[::]" => "127.0.0.1".to_string(),
            h => h.to_string(),
        };
        let ticket = ConsoleTicket {
            ticket_id: ticket_id.clone(),
            vm_id: vm.id.clone(),
            console_type: kind,
            vnc_port: port,
            target_host,
            serial_socket,
            instance,
            created_at: now,
            expires_at: now + 300,
        };
        self.active_tickets
            .write()
            .await
            .insert(ticket_id.clone(), ticket);

        let ws_path = match kind {
            ConsoleType::Serial => format!("/api/console/ws/serial/{ticket_id}"),
            _ => format!("/api/console/ws/{ticket_id}"),
        };
        ConsoleInfo {
            vm_id: vm.id.clone(),
            console_type: kind,
            host,
            port,
            ticket: ticket_id,
            ws_port: 0,
            ws_path,
            password,
        }
    }

    pub async fn verify_ticket(&self, ticket_id: &str) -> Result<ConsoleTicket> {
        let tickets = self.active_tickets.read().await;
        let ticket = tickets
            .get(ticket_id)
            .ok_or_else(|| horcrux_common::Error::System("Invalid console ticket".to_string()))?;
        if chrono::Utc::now().timestamp() > ticket.expires_at {
            return Err(horcrux_common::Error::System(
                "Console ticket expired".to_string(),
            ));
        }
        Ok(ticket.clone())
    }

    pub async fn cleanup_expired_tickets(&self) {
        let now = chrono::Utc::now().timestamp();
        self.active_tickets
            .write()
            .await
            .retain(|_, ticket| ticket.expires_at > now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horcrux_common::{VmArchitecture, VmDisk, VmDisplay, VmStatus};

    fn vm(disk: &std::path::Path, hv: VmHypervisor) -> VmConfig {
        VmConfig {
            id: "c1".into(),
            name: "c1".into(),
            hypervisor: hv,
            memory: 64,
            cpus: 1,
            disk_size: 1,
            status: VmStatus::Running,
            architecture: VmArchitecture::X86_64,
            disks: vec![VmDisk {
                path: disk.to_string_lossy().into(),
                size_gb: 1,
                disk_type: "virtio".into(),
                cache: "none".into(),
            }],
            display: VmDisplay::default(),
        }
    }

    #[tokio::test]
    async fn tickets_come_from_the_running_vms_display_state() {
        let dir = std::env::temp_dir().join(format!("hx-console-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let disk = dir.join("c1.qcow2");
        let m = ConsoleManager::new();
        let qemu = vm(&disk, VmHypervisor::Qemu);
        // Not running -> clear error, no ticket.
        assert!(m.create_console(&qemu, ConsoleType::Vnc).await.is_err());

        let rt = DisplayRuntime::plan(&disk, &VmDisplay::default()).unwrap();
        rt.save(&disk).await.unwrap();
        let info = m.create_console(&qemu, ConsoleType::Vnc).await.unwrap();
        assert_eq!(Some(info.port), rt.vnc_port);
        assert_eq!(info.password, rt.password);
        assert!(info.ws_path.ends_with(&info.ticket));
        assert_eq!(
            m.verify_ticket(&info.ticket).await.unwrap().vnc_port,
            info.port
        );

        // SPICE isn't configured on a VNC VM.
        assert!(m.create_console(&qemu, ConsoleType::Spice).await.is_err());
        // Serial is always there on a running QEMU VM.
        let s = m.create_console(&qemu, ConsoleType::Serial).await.unwrap();
        let st = m.verify_ticket(&s.ticket).await.unwrap();
        assert_eq!(
            st.serial_socket.as_deref(),
            Some(rt.serial_socket.as_path())
        );
        // LXD/Incus VMs never point the user at a CLI: their consoles come
        // from the daemon (here: not compiled in, or no daemon/instance).
        for hv in [VmHypervisor::Lxd, VmHypervisor::Incus] {
            let e = m
                .create_console(&vm(&disk, hv), ConsoleType::Serial)
                .await
                .unwrap_err()
                .to_string();
            assert!(
                !e.contains("lxc console") && !e.contains("incus console"),
                "{e}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
