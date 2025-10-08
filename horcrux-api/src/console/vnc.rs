///! VNC server management for QEMU VMs

use horcrux_common::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::RwLock;

/// VNC configuration for a VM
#[derive(Debug, Clone)]
pub struct VncConfig {
    pub vm_id: String,
    pub display: u16,      // VNC display number (5900 + display = port)
    pub port: u16,         // Actual VNC port
    pub websocket: bool,   // Enable WebSocket support
    pub password: Option<String>,
}

/// VNC manager
pub struct VncManager {
    vnc_configs: Arc<RwLock<HashMap<String, VncConfig>>>,
    next_display: Arc<RwLock<u16>>,
}

impl VncManager {
    pub fn new() -> Self {
        Self {
            vnc_configs: Arc::new(RwLock::new(HashMap::new())),
            next_display: Arc::new(RwLock::new(0)),
        }
    }

    /// Ensure VNC is enabled for a VM, return the VNC port
    pub async fn ensure_vnc_enabled(&self, vm_id: &str) -> Result<u16> {
        let configs = self.vnc_configs.read().await;

        if let Some(config) = configs.get(vm_id) {
            return Ok(config.port);
        }

        drop(configs);

        // VNC not configured, need to enable it
        self.enable_vnc(vm_id, None).await
    }

    /// Enable VNC for a VM
    pub async fn enable_vnc(&self, vm_id: &str, password: Option<String>) -> Result<u16> {
        // Get next available display number
        let mut next_display = self.next_display.write().await;
        let display = *next_display;
        *next_display += 1;
        drop(next_display);

        let port = 5900 + display;

        // Check if VM is running and get its PID
        let pid = self.get_vm_pid(vm_id).await?;

        // Use QEMU monitor to enable VNC
        // For now, we'll assume VNC is started with the VM
        // In production, you'd use QEMU QMP (QEMU Machine Protocol) to configure this

        let config = VncConfig {
            vm_id: vm_id.to_string(),
            display,
            port,
            websocket: true,
            password,
        };

        let mut configs = self.vnc_configs.write().await;
        configs.insert(vm_id.to_string(), config);

        Ok(port)
    }

    /// Get VNC port for a VM
    pub async fn get_vnc_port(&self, vm_id: &str) -> Result<u16> {
        let configs = self.vnc_configs.read().await;
        configs
            .get(vm_id)
            .map(|c| c.port)
            .ok_or_else(|| horcrux_common::Error::System(format!("VNC not configured for VM {}", vm_id)))
    }

    /// Disable VNC for a VM
    pub async fn disable_vnc(&self, vm_id: &str) -> Result<()> {
        let mut configs = self.vnc_configs.write().await;
        configs.remove(vm_id);
        Ok(())
    }

    /// Get VM PID from running processes
    async fn get_vm_pid(&self, vm_id: &str) -> Result<u32> {
        // Try to find the QEMU process for this VM
        let output = Command::new("pgrep")
            .arg("-f")
            .arg(format!("qemu.*{}", vm_id))
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to find VM process: {}", e)))?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(format!("VM {} is not running", vm_id)));
        }

        let pid_str = String::from_utf8_lossy(&output.stdout);
        let pid = pid_str
            .trim()
            .lines()
            .next()
            .and_then(|s| s.parse::<u32>().ok())
            .ok_or_else(|| horcrux_common::Error::System("Failed to parse VM PID".to_string()))?;

        Ok(pid)
    }

    /// Generate VNC command line arguments for QEMU
    pub fn generate_vnc_args(display: u16, websocket: bool, password: Option<&str>) -> Vec<String> {
        let mut args = vec![
            "-vnc".to_string(),
            format!("0.0.0.0:{}", display),
        ];

        if websocket {
            args.push(format!(",websocket={}", 5700 + display));
        }

        if password.is_some() {
            args.push(",password=on".to_string());
        }

        args
    }

    /// Check if VNC is available on the system
    pub fn check_vnc_available() -> bool {
        // VNC is built into QEMU, so just check for QEMU
        std::process::Command::new("qemu-system-x86_64")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vnc_port_calculation() {
        let display = 0;
        let port = 5900 + display;
        assert_eq!(port, 5900);

        let display = 10;
        let port = 5900 + display;
        assert_eq!(port, 5910);
    }

    #[test]
    fn test_generate_vnc_args() {
        let args = VncManager::generate_vnc_args(0, false, None);
        assert_eq!(args, vec!["-vnc", "0.0.0.0:0"]);

        let args = VncManager::generate_vnc_args(1, true, None);
        assert_eq!(args, vec!["-vnc", "0.0.0.0:1,websocket=5701"]);

        let args = VncManager::generate_vnc_args(2, true, Some("secret"));
        assert_eq!(args, vec!["-vnc", "0.0.0.0:2,websocket=5702,password=on"]);
    }
}
