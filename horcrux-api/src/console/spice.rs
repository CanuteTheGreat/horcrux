///! SPICE server management for QEMU VMs
///! SPICE (Simple Protocol for Independent Computing Environments)
///! provides enhanced remote desktop capabilities including USB redirection,
///! audio, and better performance than VNC.

use horcrux_common::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::RwLock;

/// SPICE configuration for a VM
#[derive(Debug, Clone)]
pub struct SpiceConfig {
    pub vm_id: String,
    pub port: u16,         // SPICE port
    pub tls_port: Option<u16>, // Optional TLS port
    pub password: Option<String>,
    pub addr: String,      // Listen address
    pub disable_ticketing: bool,
}

/// SPICE manager
pub struct SpiceManager {
    spice_configs: Arc<RwLock<HashMap<String, SpiceConfig>>>,
    next_port: Arc<RwLock<u16>>,
}

impl SpiceManager {
    pub fn new() -> Self {
        Self {
            spice_configs: Arc::new(RwLock::new(HashMap::new())),
            next_port: Arc::new(RwLock::new(5930)), // Start at 5930 to avoid VNC range
        }
    }

    /// Ensure SPICE is enabled for a VM, return the SPICE port
    pub async fn ensure_spice_enabled(&self, vm_id: &str) -> Result<u16> {
        let configs = self.spice_configs.read().await;

        if let Some(config) = configs.get(vm_id) {
            return Ok(config.port);
        }

        drop(configs);

        // SPICE not configured, need to enable it
        self.enable_spice(vm_id, None).await
    }

    /// Enable SPICE for a VM
    pub async fn enable_spice(&self, vm_id: &str, password: Option<String>) -> Result<u16> {
        // Get next available port number
        let mut next_port = self.next_port.write().await;
        let port = *next_port;
        *next_port += 1;
        drop(next_port);

        // Check if VM is running and get its PID
        let _pid = self.get_vm_pid(vm_id).await?;

        // Use QEMU monitor to enable SPICE
        // For now, we'll assume SPICE is started with the VM
        // In production, you'd use QEMU QMP (QEMU Machine Protocol) to configure this

        let config = SpiceConfig {
            vm_id: vm_id.to_string(),
            port,
            tls_port: None,
            password,
            addr: "127.0.0.1".to_string(),
            disable_ticketing: false,
        };

        let mut configs = self.spice_configs.write().await;
        configs.insert(vm_id.to_string(), config);

        Ok(port)
    }

    /// Get SPICE port for a VM
    pub async fn get_spice_port(&self, vm_id: &str) -> Result<u16> {
        let configs = self.spice_configs.read().await;
        configs
            .get(vm_id)
            .map(|c| c.port)
            .ok_or_else(|| horcrux_common::Error::System(format!("SPICE not configured for VM {}", vm_id)))
    }

    /// Get SPICE configuration for a VM
    pub async fn get_spice_config(&self, vm_id: &str) -> Result<SpiceConfig> {
        let configs = self.spice_configs.read().await;
        configs
            .get(vm_id)
            .cloned()
            .ok_or_else(|| horcrux_common::Error::System(format!("SPICE not configured for VM {}", vm_id)))
    }

    /// Disable SPICE for a VM
    pub async fn disable_spice(&self, vm_id: &str) -> Result<()> {
        let mut configs = self.spice_configs.write().await;
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

    /// Generate SPICE command line arguments for QEMU
    pub fn generate_spice_args(
        port: u16,
        addr: &str,
        password: Option<&str>,
        tls_port: Option<u16>,
    ) -> Vec<String> {
        let mut spice_spec = format!("port={},addr={}", port, addr);

        // Add password if provided
        if let Some(pwd) = password {
            spice_spec.push_str(&format!(",password={}", pwd));
        } else {
            spice_spec.push_str(",disable-ticketing=on");
        }

        // Add TLS port if provided
        if let Some(tls) = tls_port {
            spice_spec.push_str(&format!(",tls-port={}", tls));
        }

        // Common SPICE options for better compatibility
        spice_spec.push_str(",disable-copy-paste=off");
        spice_spec.push_str(",seamless-migration=on");

        vec!["-spice".to_string(), spice_spec]
    }

    /// Generate SPICE device arguments for QEMU (QXL graphics, spice-vdagent channel, etc.)
    pub fn generate_spice_device_args() -> Vec<String> {
        vec![
            // QXL graphics device (SPICE-optimized)
            "-vga".to_string(),
            "qxl".to_string(),
            // SPICE agent for clipboard, display resolution, etc.
            "-device".to_string(),
            "virtio-serial-pci".to_string(),
            "-device".to_string(),
            "virtserialport,chardev=spicechannel0,name=com.redhat.spice.0".to_string(),
            "-chardev".to_string(),
            "spicevmc,id=spicechannel0,name=vdagent".to_string(),
        ]
    }

    /// Check if SPICE is available on the system
    pub async fn check_spice_available() -> bool {
        // Check if QEMU supports SPICE
        let output = Command::new("qemu-system-x86_64")
            .arg("-spice")
            .arg("help")
            .output()
            .await;

        match output {
            Ok(out) => out.status.success() || String::from_utf8_lossy(&out.stderr).contains("spice"),
            Err(_) => false,
        }
    }

    /// Set SPICE password via QEMU monitor
    pub async fn set_spice_password(&self, vm_id: &str, password: &str) -> Result<()> {
        let monitor_path = format!("/var/run/qemu-server/{}.mon", vm_id);

        if !std::path::Path::new(&monitor_path).exists() {
            return Err(horcrux_common::Error::System(
                format!("QEMU monitor not found for VM {}", vm_id)
            ));
        }

        // Use socat to send command to QEMU monitor
        // Command: set_password spice <password> keep
        let command = format!("set_password spice {} keep", password);

        let output = Command::new("socat")
            .arg("-")
            .arg(format!("UNIX-CONNECT:{}", monitor_path))
            .arg("<<<")
            .arg(&command)
            .output()
            .await;

        match output {
            Ok(out) if out.status.success() => {
                tracing::info!("SPICE password set for VM {}", vm_id);

                // Update config
                let mut configs = self.spice_configs.write().await;
                if let Some(config) = configs.get_mut(vm_id) {
                    config.password = Some(password.to_string());
                }

                Ok(())
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                Err(horcrux_common::Error::System(
                    format!("Failed to set SPICE password: {}", stderr)
                ))
            }
            Err(e) => {
                Err(horcrux_common::Error::System(
                    format!("Failed to communicate with QEMU monitor: {}", e)
                ))
            }
        }
    }

    /// Generate SPICE connection URI
    pub fn generate_spice_uri(host: &str, port: u16, password: Option<&str>) -> String {
        let mut uri = format!("spice://{}:{}", host, port);

        if let Some(pwd) = password {
            uri.push_str(&format!("?password={}", pwd));
        }

        uri
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_spice_args() {
        let args = SpiceManager::generate_spice_args(5930, "127.0.0.1", None, None);
        assert_eq!(args[0], "-spice");
        assert!(args[1].contains("port=5930"));
        assert!(args[1].contains("addr=127.0.0.1"));
        assert!(args[1].contains("disable-ticketing=on"));

        let args = SpiceManager::generate_spice_args(5931, "0.0.0.0", Some("secret"), Some(5932));
        assert_eq!(args[0], "-spice");
        assert!(args[1].contains("port=5931"));
        assert!(args[1].contains("password=secret"));
        assert!(args[1].contains("tls-port=5932"));
    }

    #[test]
    fn test_generate_spice_device_args() {
        let args = SpiceManager::generate_spice_device_args();
        assert!(args.contains(&"-vga".to_string()));
        assert!(args.contains(&"qxl".to_string()));
        assert!(args.contains(&"-device".to_string()));
        assert!(args.iter().any(|s| s.contains("virtio-serial")));
    }

    #[test]
    fn test_generate_spice_uri() {
        let uri = SpiceManager::generate_spice_uri("192.168.1.100", 5930, None);
        assert_eq!(uri, "spice://192.168.1.100:5930");

        let uri = SpiceManager::generate_spice_uri("192.168.1.100", 5930, Some("secret"));
        assert_eq!(uri, "spice://192.168.1.100:5930?password=secret");
    }
}
