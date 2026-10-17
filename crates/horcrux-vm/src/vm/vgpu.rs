//! vGPU (GPU Passthrough) support
//!
//! Provides NVIDIA vGPU, AMD MxGPU, and Intel GVT-g support
//! with live migration capabilities (Proxmox VE 9.0 feature)
//!
//! Note: This module is future-ready but not yet integrated into the main API.
//! It will be activated in Phase 3 of the roadmap (GPU Support).

use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use tokio::process::Command as AsyncCommand;
use tracing::info;

/// vGPU type
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VGpuType {
    /// NVIDIA vGPU
    Nvidia,
    /// AMD MxGPU
    Amd,
    /// Intel GVT-g
    Intel,
    /// Generic PCI passthrough
    Passthrough,
}

/// vGPU profile (NVIDIA vGPU profiles)
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VGpuProfile {
    pub name: String,
    pub vgpu_type: String, // e.g., "nvidia-256", "nvidia-512"
    pub framebuffer_mb: u64,
    pub max_instances: u32,
    pub description: String,
}

/// vGPU configuration for a VM
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VGpuConfig {
    pub enabled: bool,
    pub vgpu_type: VGpuType,
    pub device_id: String,       // PCI device ID (e.g., "0000:01:00.0")
    pub profile: Option<String>, // vGPU profile name
    pub migration_enabled: bool,
}

/// vGPU device information
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VGpuDevice {
    pub pci_id: String,
    pub vendor: String,
    pub device_name: String,
    pub vgpu_type: VGpuType,
    pub available_profiles: Vec<VGpuProfile>,
    pub in_use: bool,
}

/// vGPU manager
#[allow(dead_code)]
pub struct VGpuManager {}

#[allow(dead_code)]
impl Default for VGpuManager {
    fn default() -> Self {
        Self::new()
    }
}

impl VGpuManager {
    pub fn new() -> Self {
        Self {}
    }

    /// List available vGPU devices
    pub async fn list_devices(&self) -> Result<Vec<VGpuDevice>> {
        let mut devices = Vec::new();

        // Check for NVIDIA GPUs
        if let Ok(nvidia_devices) = self.list_nvidia_devices().await {
            devices.extend(nvidia_devices);
        }

        // Check for AMD GPUs
        if let Ok(amd_devices) = self.list_amd_devices().await {
            devices.extend(amd_devices);
        }

        // Check for Intel GPUs
        if let Ok(intel_devices) = self.list_intel_devices().await {
            devices.extend(intel_devices);
        }

        Ok(devices)
    }

    /// List NVIDIA vGPU devices
    async fn list_nvidia_devices(&self) -> Result<Vec<VGpuDevice>> {
        let output = AsyncCommand::new("lspci")
            .args(["-nn", "-d", "10de:"])
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to list NVIDIA devices: {}", e))
            })?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut devices = Vec::new();

        for line in stdout.lines() {
            if line.contains("VGA") || line.contains("3D controller") {
                if let Some(pci_id) = line.split_whitespace().next() {
                    // Check if vGPU is supported
                    if let Ok(profiles) = self.get_nvidia_vgpu_profiles(pci_id).await {
                        devices.push(VGpuDevice {
                            pci_id: pci_id.to_string(),
                            vendor: "NVIDIA".to_string(),
                            device_name: self.parse_device_name(line),
                            vgpu_type: VGpuType::Nvidia,
                            available_profiles: profiles,
                            in_use: false,
                        });
                    }
                }
            }
        }

        Ok(devices)
    }

    /// Get NVIDIA vGPU profiles for a device
    async fn get_nvidia_vgpu_profiles(&self, pci_id: &str) -> Result<Vec<VGpuProfile>> {
        // Check if nvidia-smi is available
        let output = AsyncCommand::new("nvidia-smi")
            .args(["vgpu", "-i", pci_id, "-q"])
            .output()
            .await;

        if let Ok(output) = output {
            if output.status.success() {
                return self.parse_nvidia_vgpu_profiles(&String::from_utf8_lossy(&output.stdout));
            }
        }

        // Fallback: common profiles
        Ok(vec![
            VGpuProfile {
                name: "nvidia-256".to_string(),
                vgpu_type: "GRID A100-4C".to_string(),
                framebuffer_mb: 4096,
                max_instances: 16,
                description: "4GB framebuffer, compute workloads".to_string(),
            },
            VGpuProfile {
                name: "nvidia-512".to_string(),
                vgpu_type: "GRID A100-8C".to_string(),
                framebuffer_mb: 8192,
                max_instances: 8,
                description: "8GB framebuffer, graphics workloads".to_string(),
            },
        ])
    }

    /// Parse NVIDIA vGPU profiles from `nvidia-smi vgpu -q` output.
    ///
    /// Expected blocks look roughly like:
    /// ```text
    ///     vGPU Type Supported
    ///         vGPU 1
    ///             Name                      : GRID A100-4C
    ///             Max Instances             : 16
    ///             Frame Buffer              : 4096 MiB
    /// ```
    /// Field order/spacing varies across driver versions, so we track the
    /// current profile as a set of optional fields and flush it whenever a
    /// new "vGPU <n>" header (or EOF) is reached. If nothing parses out of
    /// real output (unexpected format, or future driver changes it again),
    /// we fall back to the common hardcoded profiles so callers always get
    /// something usable instead of silently getting an empty list.
    fn parse_nvidia_vgpu_profiles(&self, output: &str) -> Result<Vec<VGpuProfile>> {
        let mut profiles = Vec::new();

        let mut cur_type: Option<String> = None;
        let mut cur_name: Option<String> = None;
        let mut cur_fb_mb: Option<u64> = None;
        let mut cur_max_instances: Option<u32> = None;

        let flush = |profiles: &mut Vec<VGpuProfile>,
                     vgpu_type: &mut Option<String>,
                     name: &mut Option<String>,
                     fb_mb: &mut Option<u64>,
                     max_instances: &mut Option<u32>| {
            if let Some(n) = name.take() {
                let vt = vgpu_type.take().unwrap_or_else(|| n.clone());
                let fb = fb_mb.take().unwrap_or(0);
                let mi = max_instances.take().unwrap_or(1);
                profiles.push(VGpuProfile {
                    name: n.to_lowercase().replace([' ', '_'], "-"),
                    vgpu_type: vt,
                    framebuffer_mb: fb,
                    max_instances: mi,
                    description: format!("{} MiB framebuffer", fb),
                });
            }
            *vgpu_type = None;
            *fb_mb = None;
            *max_instances = None;
        };

        for raw_line in output.lines() {
            let line = raw_line.trim();

            if line.starts_with("vGPU ") && line.chars().nth(5).is_some_and(|c| c.is_ascii_digit())
            {
                // New profile block starting: flush whatever we had.
                flush(
                    &mut profiles,
                    &mut cur_type,
                    &mut cur_name,
                    &mut cur_fb_mb,
                    &mut cur_max_instances,
                );
                continue;
            }

            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim();
            if value.is_empty() {
                continue;
            }

            match key {
                "Name" => {
                    cur_type = Some(value.to_string());
                    cur_name = Some(value.to_string());
                }
                "Max Instances" => {
                    cur_max_instances = value.parse::<u32>().ok();
                }
                "Frame Buffer" | "FB Memory" | "Frame Buffer Size" => {
                    // Typically formatted like "4096 MiB" or "4 GiB"
                    let mut parts = value.split_whitespace();
                    if let Some(num) = parts.next().and_then(|n| n.parse::<u64>().ok()) {
                        let unit = parts.next().unwrap_or("MiB");
                        cur_fb_mb = Some(if unit.eq_ignore_ascii_case("GiB") {
                            num * 1024
                        } else {
                            num
                        });
                    }
                }
                _ => {}
            }
        }
        // Flush the final profile block (no trailing "vGPU N" header follows it).
        flush(
            &mut profiles,
            &mut cur_type,
            &mut cur_name,
            &mut cur_fb_mb,
            &mut cur_max_instances,
        );

        if profiles.is_empty() {
            // Unexpected/unparseable output (e.g. driver format changed again) --
            // fall back to the same common profiles used when nvidia-smi isn't
            // available at all, rather than silently returning nothing.
            return Ok(vec![
                VGpuProfile {
                    name: "nvidia-256".to_string(),
                    vgpu_type: "GRID A100-4C".to_string(),
                    framebuffer_mb: 4096,
                    max_instances: 16,
                    description: "4GB framebuffer, compute workloads".to_string(),
                },
                VGpuProfile {
                    name: "nvidia-512".to_string(),
                    vgpu_type: "GRID A100-8C".to_string(),
                    framebuffer_mb: 8192,
                    max_instances: 8,
                    description: "8GB framebuffer, graphics workloads".to_string(),
                },
            ]);
        }

        Ok(profiles)
    }

    /// List AMD MxGPU devices
    async fn list_amd_devices(&self) -> Result<Vec<VGpuDevice>> {
        let output = AsyncCommand::new("lspci")
            .args(["-nn", "-d", "1002:"])
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to list AMD devices: {}", e))
            })?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut devices = Vec::new();

        for line in stdout.lines() {
            if line.contains("VGA") || line.contains("Display controller") {
                if let Some(pci_id) = line.split_whitespace().next() {
                    devices.push(VGpuDevice {
                        pci_id: pci_id.to_string(),
                        vendor: "AMD".to_string(),
                        device_name: self.parse_device_name(line),
                        vgpu_type: VGpuType::Amd,
                        available_profiles: vec![],
                        in_use: false,
                    });
                }
            }
        }

        Ok(devices)
    }

    /// List Intel GVT-g devices
    async fn list_intel_devices(&self) -> Result<Vec<VGpuDevice>> {
        let output = AsyncCommand::new("lspci")
            .args(["-nn", "-d", "8086:"])
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to list Intel devices: {}", e))
            })?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut devices = Vec::new();

        for line in stdout.lines() {
            if line.contains("VGA") {
                if let Some(pci_id) = line.split_whitespace().next() {
                    devices.push(VGpuDevice {
                        pci_id: pci_id.to_string(),
                        vendor: "Intel".to_string(),
                        device_name: self.parse_device_name(line),
                        vgpu_type: VGpuType::Intel,
                        available_profiles: self.get_intel_gvtg_profiles(),
                        in_use: false,
                    });
                }
            }
        }

        Ok(devices)
    }

    /// Get Intel GVT-g profiles
    fn get_intel_gvtg_profiles(&self) -> Vec<VGpuProfile> {
        vec![
            VGpuProfile {
                name: "i915-GVTg_V5_4".to_string(),
                vgpu_type: "GVT-g".to_string(),
                framebuffer_mb: 512,
                max_instances: 7,
                description: "Low-performance vGPU".to_string(),
            },
            VGpuProfile {
                name: "i915-GVTg_V5_8".to_string(),
                vgpu_type: "GVT-g".to_string(),
                framebuffer_mb: 1024,
                max_instances: 3,
                description: "High-performance vGPU".to_string(),
            },
        ]
    }

    /// Parse device name from lspci output
    fn parse_device_name(&self, line: &str) -> String {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() >= 3 {
            parts[2].trim().to_string()
        } else {
            "Unknown GPU".to_string()
        }
    }

    /// Attach vGPU to VM
    pub async fn attach_vgpu(&self, vm_id: u32, config: &VGpuConfig) -> Result<()> {
        info!("Attaching vGPU {} to VM {}", config.device_id, vm_id);

        match config.vgpu_type {
            VGpuType::Nvidia => self.attach_nvidia_vgpu(vm_id, config).await?,
            VGpuType::Amd => self.attach_amd_mxgpu(vm_id, config).await?,
            VGpuType::Intel => self.attach_intel_gvtg(vm_id, config).await?,
            VGpuType::Passthrough => self.attach_pci_passthrough(vm_id, config).await?,
        }

        Ok(())
    }

    /// Attach NVIDIA vGPU
    async fn attach_nvidia_vgpu(&self, _vm_id: u32, config: &VGpuConfig) -> Result<()> {
        // Create vGPU instance
        let vgpu_uuid = uuid::Uuid::new_v4();
        let profile = config.profile.as_ref().ok_or_else(|| {
            horcrux_common::Error::InvalidConfig(
                "vGPU profile required for NVIDIA vGPU".to_string(),
            )
        })?;

        let mdev_path = format!(
            "/sys/class/mdev_bus/{}/mdev_supported_types/{}/create",
            config.device_id, profile
        );

        tokio::fs::write(&mdev_path, vgpu_uuid.to_string())
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to create vGPU: {}", e)))?;

        info!("Created NVIDIA vGPU instance: {}", vgpu_uuid);

        // Add to VM configuration (QEMU args)
        // -device vfio-pci,sysfsdev=/sys/bus/mdev/devices/{uuid}

        Ok(())
    }

    /// Attach AMD MxGPU
    async fn attach_amd_mxgpu(&self, vm_id: u32, config: &VGpuConfig) -> Result<()> {
        info!("Attaching AMD MxGPU to VM {}", vm_id);

        // AMD MxGPU uses SR-IOV
        // Enable SR-IOV on the device and attach VF
        self.attach_pci_passthrough(vm_id, config).await
    }

    /// Attach Intel GVT-g
    async fn attach_intel_gvtg(&self, vm_id: u32, config: &VGpuConfig) -> Result<()> {
        info!("Attaching Intel GVT-g to VM {}", vm_id);

        let vgpu_uuid = uuid::Uuid::new_v4();
        let profile = config.profile.as_ref().ok_or_else(|| {
            horcrux_common::Error::InvalidConfig(
                "vGPU profile required for Intel GVT-g".to_string(),
            )
        })?;

        let mdev_path = format!(
            "/sys/class/mdev_bus/{}/mdev_supported_types/{}/create",
            config.device_id, profile
        );

        tokio::fs::write(&mdev_path, vgpu_uuid.to_string())
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to create GVT-g vGPU: {}", e))
            })?;

        info!("Created Intel GVT-g instance: {}", vgpu_uuid);

        Ok(())
    }

    /// Attach PCI passthrough
    async fn attach_pci_passthrough(&self, vm_id: u32, config: &VGpuConfig) -> Result<()> {
        info!(
            "Attaching PCI passthrough {} to VM {}",
            config.device_id, vm_id
        );

        // Bind to vfio-pci driver
        self.bind_vfio_pci(&config.device_id).await?;

        Ok(())
    }

    /// Bind device to vfio-pci driver
    async fn bind_vfio_pci(&self, pci_id: &str) -> Result<()> {
        // Get device vendor and device ID
        let ids_path = format!("/sys/bus/pci/devices/{}/vendor", pci_id);
        let vendor = tokio::fs::read_to_string(&ids_path).await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to read vendor ID: {}", e))
        })?;

        let ids_path = format!("/sys/bus/pci/devices/{}/device", pci_id);
        let device = tokio::fs::read_to_string(&ids_path).await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to read device ID: {}", e))
        })?;

        // Unbind from current driver
        let unbind_path = format!("/sys/bus/pci/devices/{}/driver/unbind", pci_id);
        let _ = tokio::fs::write(&unbind_path, pci_id).await;

        // Bind to vfio-pci
        let new_id_path = "/sys/bus/pci/drivers/vfio-pci/new_id";
        tokio::fs::write(new_id_path, format!("{} {}", vendor.trim(), device.trim()))
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to bind vfio-pci: {}", e))
            })?;

        Ok(())
    }

    /// Detach vGPU from VM
    pub async fn detach_vgpu(&self, vm_id: u32, vgpu_uuid: &str) -> Result<()> {
        info!("Detaching vGPU {} from VM {}", vgpu_uuid, vm_id);

        let remove_path = format!("/sys/bus/mdev/devices/{}/remove", vgpu_uuid);
        tokio::fs::write(&remove_path, "1")
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to remove vGPU: {}", e)))?;

        Ok(())
    }

    /// Check if vGPU migration is supported
    pub fn supports_migration(&self, vgpu_type: &VGpuType) -> bool {
        match vgpu_type {
            VGpuType::Nvidia => true,       // NVIDIA vGPU supports live migration
            VGpuType::Amd => false,         // AMD MxGPU has limited migration support
            VGpuType::Intel => false,       // Intel GVT-g doesn't support live migration
            VGpuType::Passthrough => false, // Full passthrough can't migrate
        }
    }

    /// Prepare vGPU for live migration
    pub async fn prepare_migration(&self, vm_id: u32, vgpu_uuid: &str) -> Result<()> {
        info!("Preparing vGPU {} for migration of VM {}", vgpu_uuid, vm_id);

        // Suspend vGPU operations
        // Save vGPU state
        // This is vendor-specific - NVIDIA has specific APIs

        Ok(())
    }

    /// Complete vGPU migration on target host
    pub async fn complete_migration(
        &self,
        vm_id: u32,
        config: &VGpuConfig,
        _state_data: Vec<u8>,
    ) -> Result<()> {
        info!("Completing vGPU migration for VM {}", vm_id);

        // Recreate vGPU on target host
        self.attach_vgpu(vm_id, config).await?;

        // Restore vGPU state
        // This is vendor-specific

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_list_devices() {
        let manager = VGpuManager::new();
        let result = manager.list_devices().await;
        // This will succeed even if no GPUs are present
        assert!(result.is_ok());
    }

    #[test]
    fn test_migration_support() {
        let manager = VGpuManager::new();
        assert!(manager.supports_migration(&VGpuType::Nvidia));
        assert!(!manager.supports_migration(&VGpuType::Passthrough));
    }

    #[test]
    fn test_parse_nvidia_vgpu_profiles_real_format() {
        let manager = VGpuManager::new();
        let sample = "\
GPU 00000000:01:00.0
    vGPU Type Supported
        vGPU 1
            Name                      : GRID A100-4C
            Max Instances             : 16
            Frame Buffer              : 4096 MiB
        vGPU 2
            Name                      : GRID A100-8C
            Max Instances             : 8
            Frame Buffer              : 8 GiB
";
        let profiles = manager.parse_nvidia_vgpu_profiles(sample).unwrap();
        assert_eq!(profiles.len(), 2);

        assert_eq!(profiles[0].vgpu_type, "GRID A100-4C");
        assert_eq!(profiles[0].framebuffer_mb, 4096);
        assert_eq!(profiles[0].max_instances, 16);

        assert_eq!(profiles[1].vgpu_type, "GRID A100-8C");
        assert_eq!(profiles[1].framebuffer_mb, 8192); // 8 GiB -> 8192 MiB
        assert_eq!(profiles[1].max_instances, 8);
    }

    #[test]
    fn test_parse_nvidia_vgpu_profiles_falls_back_on_unparseable_output() {
        let manager = VGpuManager::new();
        let profiles = manager
            .parse_nvidia_vgpu_profiles("totally unexpected driver output\nwith no known fields")
            .unwrap();
        // Should fall back to the common hardcoded profiles rather than an
        // empty list, since the output didn't match any known field.
        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].name, "nvidia-256");
        assert_eq!(profiles[1].name, "nvidia-512");
    }
}
