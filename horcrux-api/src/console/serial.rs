//! Serial console management for VMs
//! Provides serial port access via PTY (pseudo-terminal) devices

#![allow(dead_code)]

use horcrux_common::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Serial console configuration
#[derive(Debug, Clone)]
pub struct SerialConfig {
    pub vm_id: String,
    pub pty_path: String,       // e.g., /dev/pts/5
    pub socket_path: String,    // e.g., /var/run/qemu-server/vm-100.serial
}

/// Serial console manager
pub struct SerialManager {
    serial_configs: Arc<RwLock<HashMap<String, SerialConfig>>>,
}

impl SerialManager {
    pub fn new() -> Self {
        Self {
            serial_configs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Ensure serial console is enabled for a VM, return the socket path
    pub async fn ensure_serial_enabled(&self, vm_id: &str) -> Result<String> {
        let configs = self.serial_configs.read().await;

        if let Some(config) = configs.get(vm_id) {
            return Ok(config.socket_path.clone());
        }

        drop(configs);

        // Serial console not configured, need to enable it
        self.enable_serial(vm_id).await
    }

    /// Enable serial console for a VM
    pub async fn enable_serial(&self, vm_id: &str) -> Result<String> {
        info!("Enabling serial console for VM {}", vm_id);

        // Check if VM is running
        let _pid = self.get_vm_pid(vm_id).await?;

        // Serial console socket path
        let socket_path = format!("/var/run/qemu-server/{}.serial", vm_id);

        // Try to find existing PTY from QEMU monitor
        let pty_path = self.get_serial_pty(vm_id).await
            .unwrap_or_else(|_| format!("/dev/pts/0")); // Fallback

        let config = SerialConfig {
            vm_id: vm_id.to_string(),
            pty_path,
            socket_path: socket_path.clone(),
        };

        let mut configs = self.serial_configs.write().await;
        configs.insert(vm_id.to_string(), config);

        info!("Serial console enabled for VM {} at {}", vm_id, socket_path);
        Ok(socket_path)
    }

    /// Get serial console socket path for a VM
    pub async fn get_serial_socket(&self, vm_id: &str) -> Result<String> {
        let configs = self.serial_configs.read().await;
        configs
            .get(vm_id)
            .map(|c| c.socket_path.clone())
            .ok_or_else(|| horcrux_common::Error::System(format!("Serial console not configured for VM {}", vm_id)))
    }

    /// Get serial console configuration for a VM
    pub async fn get_serial_config(&self, vm_id: &str) -> Result<SerialConfig> {
        let configs = self.serial_configs.read().await;
        configs
            .get(vm_id)
            .cloned()
            .ok_or_else(|| horcrux_common::Error::System(format!("Serial console not configured for VM {}", vm_id)))
    }

    /// Disable serial console for a VM
    pub async fn disable_serial(&self, vm_id: &str) -> Result<()> {
        let mut configs = self.serial_configs.write().await;
        configs.remove(vm_id);
        info!("Disabled serial console for VM {}", vm_id);
        Ok(())
    }

    /// Get VM PID from running processes
    async fn get_vm_pid(&self, vm_id: &str) -> Result<u32> {
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

    /// Get serial PTY path from QEMU monitor
    async fn get_serial_pty(&self, vm_id: &str) -> Result<String> {
        // Try to query QEMU monitor for serial port info
        let monitor_path = format!("/var/run/qemu-server/{}.mon", vm_id);

        if !std::path::Path::new(&monitor_path).exists() {
            return Err(horcrux_common::Error::System(
                format!("QEMU monitor not found for VM {}", vm_id)
            ));
        }

        // Use socat to query serial info from QEMU monitor
        // Command: info chardev
        let output = Command::new("sh")
            .arg("-c")
            .arg(format!("echo 'info chardev' | socat - UNIX-CONNECT:{}", monitor_path))
            .output()
            .await;

        if let Ok(output) = output {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                // Parse output to find serial0 PTY path
                for line in stdout.lines() {
                    if line.contains("serial0") && line.contains("/dev/pts/") {
                        // Extract PTY path from line like: "serial0: filename=/dev/pts/5"
                        if let Some(start) = line.find("/dev/pts/") {
                            let pty_path = &line[start..];
                            let pty_path = pty_path.split_whitespace().next().unwrap_or("");
                            if !pty_path.is_empty() {
                                info!("Found serial PTY for VM {}: {}", vm_id, pty_path);
                                return Ok(pty_path.to_string());
                            }
                        }
                    }
                }
            }
        }

        Err(horcrux_common::Error::System("Failed to get serial PTY path".to_string()))
    }

    /// Generate QEMU command line arguments for serial console
    pub fn generate_serial_args(socket_path: &str) -> Vec<String> {
        vec![
            "-serial".to_string(),
            format!("unix:{},server,nowait", socket_path),
        ]
    }

    /// Generate alternative serial args using PTY
    pub fn generate_serial_pty_args() -> Vec<String> {
        vec![
            "-serial".to_string(),
            "pty".to_string(),
        ]
    }

    /// Read serial console output (for testing/debugging)
    pub async fn read_serial_output(&self, vm_id: &str, lines: usize) -> Result<String> {
        let config = self.get_serial_config(vm_id).await?;

        // Try to read from socket
        let output = Command::new("sh")
            .arg("-c")
            .arg(format!("timeout 1 socat - UNIX-CONNECT:{} 2>/dev/null | tail -n {}",
                config.socket_path, lines))
            .output()
            .await;

        match output {
            Ok(output) if output.status.success() => {
                Ok(String::from_utf8_lossy(&output.stdout).to_string())
            }
            _ => {
                warn!("Failed to read serial output for VM {}", vm_id);
                Ok(String::new())
            }
        }
    }

    /// Write to serial console (send commands)
    pub async fn write_serial_input(&self, vm_id: &str, data: &str) -> Result<()> {
        let config = self.get_serial_config(vm_id).await?;

        let output = Command::new("sh")
            .arg("-c")
            .arg(format!("echo '{}' | socat - UNIX-CONNECT:{}", data, config.socket_path))
            .output()
            .await;

        match output {
            Ok(output) if output.status.success() => {
                info!("Sent data to serial console for VM {}", vm_id);
                Ok(())
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(horcrux_common::Error::System(
                    format!("Failed to write to serial console: {}", stderr)
                ))
            }
            Err(e) => {
                Err(horcrux_common::Error::System(
                    format!("Failed to write to serial console: {}", e)
                ))
            }
        }
    }

    /// Check if serial console is available for the system
    pub async fn check_serial_available() -> bool {
        // Check if socat is available (used for serial communication)
        Command::new("which")
            .arg("socat")
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_serial_args() {
        let args = SerialManager::generate_serial_args("/var/run/qemu-server/vm-100.serial");
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-serial");
        assert!(args[1].contains("unix:"));
        assert!(args[1].contains("server"));
    }

    #[test]
    fn test_generate_serial_pty_args() {
        let args = SerialManager::generate_serial_pty_args();
        assert_eq!(args, vec!["-serial", "pty"]);
    }

    #[tokio::test]
    async fn test_serial_manager() {
        let manager = SerialManager::new();
        // Test basic initialization
        assert!(manager.get_serial_socket("test-vm").await.is_err());
    }
}
