//! VM consoles: VNC (noVNC over the ticketed WebSocket proxy), SPICE
//! (WebSocket proxy for browser clients + `.vv` file for remote-viewer) and
//! the serial console (WebSocket bridge to QEMU's serial Unix socket).
//!
//! Every endpoint comes from what `vm::display` actually configured when the
//! VM was started (`<disk>.display.json`); nothing is allocated here. (This
//! module used to hand out VNC/SPICE ports and a serial socket path that
//! QEMU was never told about, so no console could ever connect.)

pub mod novnc;
pub mod serial_ws;

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
    pub created_at: i64,
    pub expires_at: i64,
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
/// get a pointer to their own console tooling instead.
pub fn runtime_disk(vm: &VmConfig) -> Result<PathBuf> {
    let tool = match vm.hypervisor {
        VmHypervisor::Qemu => None,
        VmHypervisor::Lxd => Some(("an LXD", "lxc")),
        VmHypervisor::Incus => Some(("an Incus", "incus")),
    };
    if let Some((kind, cli)) = tool {
        return Err(horcrux_common::Error::Validation(format!(
            "VM {} is {kind} VM: use its own console (`{cli} console {} --type=vga` for the \
             SPICE display, `--type=console` for the text console); Horcrux's VNC/SPICE/serial \
             consoles are for QEMU VMs",
            vm.id, vm.name
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
        Ok(ConsoleInfo {
            vm_id: vm.id.clone(),
            console_type: kind,
            host,
            port,
            ticket: ticket_id,
            ws_port: 0,
            ws_path,
            password,
        })
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
        // LXD/Incus VMs point at their own console tooling.
        let e = m
            .create_console(&vm(&disk, VmHypervisor::Incus), ConsoleType::Vnc)
            .await
            .unwrap_err();
        assert!(e.to_string().contains("incus console"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
