///! GPU passthrough management
///! Supports PCI passthrough for GPUs (NVIDIA, AMD, Intel)

use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::RwLock;

/// GPU device information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuDevice {
    pub pci_address: String,
    pub vendor_id: String,
    pub device_id: String,
    pub vendor_name: String,
    pub device_name: String,
    pub driver: Option<String>,
    pub iommu_group: Option<String>,
    pub in_use: bool,
}

/// GPU passthrough configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuPassthroughConfig {
    pub pci_address: String,
    pub rom_file: Option<String>,
    pub multifunction: bool,
    pub primary_gpu: bool,
}

/// GPU Manager
pub struct GpuManager {
    devices: Arc<RwLock<HashMap<String, GpuDevice>>>,
}

impl GpuManager {
    pub fn new() -> Self {
        Self {
            devices: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Scan system for available GPU devices
    pub async fn scan_devices(&self) -> Result<Vec<GpuDevice>> {
        let mut devices = Vec::new();

        // Use lspci to find GPU devices
        let output = Command::new("lspci")
            .args(["-nn", "-D"])
            .output()
            .await?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(
                "Failed to execute lspci".to_string(),
            ));
        }

        let output_str = String::from_utf8_lossy(&output.stdout);

        for line in output_str.lines() {
            // Look for VGA, 3D, and Display controllers
            if line.contains("VGA compatible controller")
                || line.contains("3D controller")
                || line.contains("Display controller")
            {
                if let Some(device) = self.parse_lspci_line(line).await? {
                    devices.push(device);
                }
            }
        }

        // Update internal cache
        let mut dev_map = self.devices.write().await;
        for device in &devices {
            dev_map.insert(device.pci_address.clone(), device.clone());
        }

        Ok(devices)
    }

    /// Parse a single lspci line into GpuDevice
    async fn parse_lspci_line(&self, line: &str) -> Result<Option<GpuDevice>> {
        // Format: 0000:01:00.0 VGA compatible controller [0300]: NVIDIA Corporation ... [10de:1b80]
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.is_empty() {
            return Ok(None);
        }

        let pci_address = parts[0].to_string();

        // Extract vendor:device IDs from brackets
        let ids = if let Some(bracket_start) = line.rfind('[') {
            if let Some(bracket_end) = line.rfind(']') {
                &line[bracket_start + 1..bracket_end]
            } else {
                return Ok(None);
            }
        } else {
            return Ok(None);
        };

        let id_parts: Vec<&str> = ids.split(':').collect();
        if id_parts.len() != 2 {
            return Ok(None);
        }

        let vendor_id = id_parts[0].to_string();
        let device_id = id_parts[1].to_string();

        // Get vendor and device names
        let (vendor_name, device_name) = self.get_device_name(&pci_address).await?;

        // Get current driver
        let driver = self.get_device_driver(&pci_address).await?;

        // Get IOMMU group
        let iommu_group = self.get_iommu_group(&pci_address).await?;

        Ok(Some(GpuDevice {
            pci_address,
            vendor_id,
            device_id,
            vendor_name,
            device_name,
            driver,
            iommu_group,
            in_use: false,
        }))
    }

    /// Get device name using lspci
    async fn get_device_name(&self, pci_address: &str) -> Result<(String, String)> {
        let output = Command::new("lspci")
            .args(["-s", pci_address, "-v"])
            .output()
            .await?;

        let output_str = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = output_str.lines().collect();

        if let Some(first_line) = lines.first() {
            // Parse "0000:01:00.0 VGA compatible controller: NVIDIA Corporation GP104 [GeForce GTX 1080]"
            if let Some(colon_pos) = first_line.find(':') {
                let after_colon = &first_line[colon_pos + 1..];
                if let Some(vendor_end) = after_colon.find("Corporation") {
                    let vendor = after_colon[..vendor_end + 11].trim().to_string();
                    let device = after_colon[vendor_end + 11..].trim().to_string();
                    return Ok((vendor, device));
                }
                // Try other common patterns
                let parts: Vec<&str> = after_colon.split(':').skip(1).collect();
                if let Some(desc) = parts.first() {
                    let desc_parts: Vec<&str> = desc.split_whitespace().collect();
                    if desc_parts.len() >= 2 {
                        return Ok((desc_parts[0].to_string(), desc_parts[1..].join(" ")));
                    }
                }
            }
        }

        Ok(("Unknown".to_string(), "Unknown GPU".to_string()))
    }

    /// Get current driver for PCI device
    async fn get_device_driver(&self, pci_address: &str) -> Result<Option<String>> {
        let driver_path = format!("/sys/bus/pci/devices/{}/driver", pci_address);

        if let Ok(link) = tokio::fs::read_link(&driver_path).await {
            if let Some(driver_name) = link.file_name() {
                return Ok(Some(driver_name.to_string_lossy().to_string()));
            }
        }

        Ok(None)
    }

    /// Get IOMMU group for PCI device
    async fn get_iommu_group(&self, pci_address: &str) -> Result<Option<String>> {
        let iommu_path = format!("/sys/bus/pci/devices/{}/iommu_group", pci_address);

        if let Ok(link) = tokio::fs::read_link(&iommu_path).await {
            if let Some(group_name) = link.file_name() {
                return Ok(Some(group_name.to_string_lossy().to_string()));
            }
        }

        Ok(None)
    }

    /// List all available GPU devices
    pub async fn list_devices(&self) -> Vec<GpuDevice> {
        let devices = self.devices.read().await;
        devices.values().cloned().collect()
    }

    /// Get specific GPU device
    pub async fn get_device(&self, pci_address: &str) -> Result<GpuDevice> {
        let devices = self.devices.read().await;
        devices
            .get(pci_address)
            .cloned()
            .ok_or_else(|| horcrux_common::Error::System(format!("GPU device {} not found", pci_address)))
    }

    /// Bind GPU to vfio-pci driver for passthrough
    pub async fn bind_to_vfio(&self, pci_address: &str) -> Result<()> {
        let device = self.get_device(pci_address).await?;

        // Check if already bound to vfio-pci
        if let Some(ref driver) = device.driver {
            if driver == "vfio-pci" {
                return Ok(());
            }

            // Unbind from current driver
            let unbind_path = format!("/sys/bus/pci/drivers/{}/unbind", driver);
            tokio::fs::write(&unbind_path, pci_address).await?;
        }

        // Get vendor:device ID
        let vendor_device = format!("{} {}", device.vendor_id, device.device_id);

        // Add to vfio-pci new_id
        let new_id_path = "/sys/bus/pci/drivers/vfio-pci/new_id";
        tokio::fs::write(new_id_path, &vendor_device).await?;

        tracing::info!("Bound GPU {} to vfio-pci driver", pci_address);

        Ok(())
    }

    /// Unbind GPU from vfio-pci driver
    pub async fn unbind_from_vfio(&self, pci_address: &str) -> Result<()> {
        let unbind_path = "/sys/bus/pci/drivers/vfio-pci/unbind";
        tokio::fs::write(unbind_path, pci_address).await?;

        tracing::info!("Unbound GPU {} from vfio-pci driver", pci_address);

        Ok(())
    }

    /// Check if IOMMU is enabled
    pub async fn check_iommu_enabled(&self) -> bool {
        // Check for Intel IOMMU
        if Path::new("/sys/class/iommu/dmar0").exists() {
            return true;
        }

        // Check for AMD IOMMU
        if Path::new("/sys/class/iommu/ivhd0").exists() {
            return true;
        }

        // Check kernel command line
        if let Ok(cmdline) = tokio::fs::read_to_string("/proc/cmdline").await {
            if cmdline.contains("iommu=pt") || cmdline.contains("iommu=on") {
                return true;
            }
        }

        false
    }

    /// Generate QEMU arguments for GPU passthrough
    pub fn generate_qemu_args(&self, config: &GpuPassthroughConfig) -> Vec<String> {
        let mut args = Vec::new();

        // Basic PCI passthrough
        let mut device_arg = format!("vfio-pci,host={}", config.pci_address);

        // Add multifunction if needed
        if config.multifunction {
            device_arg.push_str(",multifunction=on");
        }

        // Mark as primary GPU
        if config.primary_gpu {
            device_arg.push_str(",x-vga=on");
        }

        // Add ROM file if specified
        if let Some(ref rom_file) = config.rom_file {
            device_arg.push_str(&format!(",romfile={}", rom_file));
        }

        args.push("-device".to_string());
        args.push(device_arg);

        // For primary GPU, add additional display arguments
        if config.primary_gpu {
            args.push("-vga".to_string());
            args.push("none".to_string());
            args.push("-nographic".to_string());
        }

        args
    }

    /// Validate GPU passthrough configuration
    pub async fn validate_config(&self, config: &GpuPassthroughConfig) -> Result<()> {
        // Check if IOMMU is enabled
        if !self.check_iommu_enabled().await {
            return Err(horcrux_common::Error::InvalidConfig(
                "IOMMU is not enabled. Add intel_iommu=on or amd_iommu=on to kernel parameters".to_string(),
            ));
        }

        // Check if device exists
        let device = self.get_device(&config.pci_address).await?;

        // Check if device has IOMMU group
        if device.iommu_group.is_none() {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "GPU {} is not in an IOMMU group",
                config.pci_address
            )));
        }

        // Check if ROM file exists
        if let Some(ref rom_file) = config.rom_file {
            if !Path::new(rom_file).exists() {
                return Err(horcrux_common::Error::InvalidConfig(format!(
                    "ROM file {} does not exist",
                    rom_file
                )));
            }
        }

        Ok(())
    }

    /// Get devices in same IOMMU group
    pub async fn get_iommu_group_devices(&self, pci_address: &str) -> Result<Vec<GpuDevice>> {
        let device = self.get_device(pci_address).await?;

        let Some(ref iommu_group) = device.iommu_group else {
            return Ok(vec![device]);
        };

        let devices = self.devices.read().await;
        let group_devices: Vec<GpuDevice> = devices
            .values()
            .filter(|d| {
                d.iommu_group
                    .as_ref()
                    .map(|g| g == iommu_group)
                    .unwrap_or(false)
            })
            .cloned()
            .collect();

        Ok(group_devices)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_gpu_manager_creation() {
        let manager = GpuManager::new();
        let devices = manager.list_devices().await;
        assert_eq!(devices.len(), 0);
    }

    #[tokio::test]
    async fn test_iommu_check() {
        let manager = GpuManager::new();
        let _iommu_enabled = manager.check_iommu_enabled().await;
        // Just verify it doesn't crash
    }

    #[test]
    fn test_qemu_args_generation() {
        let manager = GpuManager::new();
        let config = GpuPassthroughConfig {
            pci_address: "0000:01:00.0".to_string(),
            rom_file: Some("/path/to/vbios.rom".to_string()),
            multifunction: true,
            primary_gpu: true,
        };

        let args = manager.generate_qemu_args(&config);
        assert!(args.contains(&"-device".to_string()));
        assert!(args.iter().any(|a| a.contains("vfio-pci")));
        assert!(args.iter().any(|a| a.contains("multifunction=on")));
        assert!(args.iter().any(|a| a.contains("x-vga=on")));
        assert!(args.iter().any(|a| a.contains("romfile=")));
    }
}
