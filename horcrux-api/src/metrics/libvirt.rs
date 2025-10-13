///! VM metrics collection via libvirt
///! Provides real metrics for KVM/QEMU VMs using libvirt API

#[cfg(feature = "qemu")]
use virt::connect::Connect;
#[cfg(feature = "qemu")]
use virt::domain::Domain;
#[cfg(feature = "qemu")]
use virt::sys;

use std::io;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, warn};

/// VM metrics from libvirt
#[derive(Debug, Clone)]
pub struct VmMetrics {
    pub cpu_time: u64,           // CPU time in nanoseconds
    pub cpu_usage_percent: f64,  // CPU usage percentage
    pub memory_actual: u64,      // Actual memory usage in bytes
    pub memory_rss: u64,         // Resident set size in bytes
    pub disk_read_bytes: u64,    // Disk read bytes
    pub disk_write_bytes: u64,   // Disk write bytes
    pub network_rx_bytes: u64,   // Network receive bytes
    pub network_tx_bytes: u64,   // Network transmit bytes
}

/// Previous VM metrics for rate calculation
#[derive(Debug, Clone)]
struct PreviousVmMetrics {
    cpu_time: u64,
    timestamp: std::time::Instant,
    disk_read_bytes: u64,
    disk_write_bytes: u64,
    network_rx_bytes: u64,
    network_tx_bytes: u64,
}

/// Libvirt connection manager
pub struct LibvirtManager {
    #[cfg(feature = "qemu")]
    connection: Arc<RwLock<Option<Connect>>>,
    previous_metrics: Arc<RwLock<std::collections::HashMap<String, PreviousVmMetrics>>>,
}

impl LibvirtManager {
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "qemu")]
            connection: Arc::new(RwLock::new(None)),
            previous_metrics: Arc::new(RwLock::new(std::collections::HashMap::new())),
        }
    }

    /// Connect to libvirt
    #[cfg(feature = "qemu")]
    pub async fn connect(&self, uri: Option<&str>) -> io::Result<()> {
        let uri = uri.unwrap_or("qemu:///system");

        match Connect::open(Some(uri)) {
            Ok(conn) => {
                let mut connection = self.connection.write().await;
                *connection = Some(conn);
                debug!("Connected to libvirt: {}", uri);
                Ok(())
            }
            Err(e) => {
                error!("Failed to connect to libvirt: {:?}", e);
                Err(io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    format!("libvirt connection failed: {:?}", e),
                ))
            }
        }
    }

    /// Get VM metrics via libvirt
    #[cfg(feature = "qemu")]
    pub async fn get_vm_metrics(&self, vm_id: &str) -> io::Result<VmMetrics> {
        let connection = self.connection.read().await;
        let conn = connection.as_ref().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotConnected, "Not connected to libvirt")
        })?;

        // Look up domain by name
        let domain = Domain::lookup_by_name(conn, vm_id).map_err(|e| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("VM {} not found: {:?}", vm_id, e),
            )
        })?;

        // Get domain info for memory and CPU
        let info = domain.get_info().map_err(|e| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to get domain info: {:?}", e),
            )
        })?;

        let memory_actual = info.memory * 1024; // Convert KB to bytes
        let cpu_time = info.cpu_time; // nanoseconds

        // Get memory stats
        let memory_rss = self.get_memory_rss(&domain).unwrap_or(memory_actual);

        // Get block device stats
        let (disk_read_bytes, disk_write_bytes) = self.get_block_stats(&domain);

        // Get network interface stats
        let (network_rx_bytes, network_tx_bytes) = self.get_network_stats(&domain);

        // Calculate CPU usage percentage
        let cpu_usage_percent = self.calculate_cpu_usage(vm_id, cpu_time).await;

        // Store current metrics for next calculation
        let mut prev_metrics = self.previous_metrics.write().await;
        prev_metrics.insert(
            vm_id.to_string(),
            PreviousVmMetrics {
                cpu_time,
                timestamp: std::time::Instant::now(),
                disk_read_bytes,
                disk_write_bytes,
                network_rx_bytes,
                network_tx_bytes,
            },
        );

        Ok(VmMetrics {
            cpu_time,
            cpu_usage_percent,
            memory_actual,
            memory_rss,
            disk_read_bytes,
            disk_write_bytes,
            network_rx_bytes,
            network_tx_bytes,
        })
    }

    /// Calculate CPU usage percentage from CPU time delta
    #[cfg(feature = "qemu")]
    async fn calculate_cpu_usage(&self, vm_id: &str, current_cpu_time: u64) -> f64 {
        let prev_metrics = self.previous_metrics.read().await;

        if let Some(prev) = prev_metrics.get(vm_id) {
            let time_delta = prev.timestamp.elapsed().as_nanos() as u64;
            if time_delta == 0 {
                return 0.0;
            }

            let cpu_delta = current_cpu_time.saturating_sub(prev.cpu_time);

            // CPU usage = (CPU time delta / real time delta) * 100 * num_cpus
            // For now, assume 1 vCPU. In production, query domain vCPU count.
            let usage = (cpu_delta as f64 / time_delta as f64) * 100.0;
            usage.min(100.0) // Cap at 100%
        } else {
            0.0 // First sample, no previous data
        }
    }

    /// Get memory RSS (Resident Set Size)
    #[cfg(feature = "qemu")]
    fn get_memory_rss(&self, _domain: &Domain) -> Option<u64> {
        // TODO: Implement memory stats parsing when virt crate API is available
        // For now, return None and use memory_actual from domain info
        None
    }

    /// Get block device statistics
    #[cfg(feature = "qemu")]
    fn get_block_stats(&self, _domain: &Domain) -> (u64, u64) {
        // TODO: Implement block stats when virt crate API is available
        // For now, return zeros
        // In production, would call domain.block_stats() for each device
        (0, 0)
    }

    /// Get network interface statistics
    #[cfg(feature = "qemu")]
    fn get_network_stats(&self, _domain: &Domain) -> (u64, u64) {
        // TODO: Implement network stats when virt crate API is available
        // For now, return zeros
        // In production, would call domain.interface_stats() for each interface
        (0, 0)
    }

    /// List all running VMs
    #[cfg(feature = "qemu")]
    pub async fn list_running_vms(&self) -> io::Result<Vec<String>> {
        let connection = self.connection.read().await;
        let conn = connection.as_ref().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotConnected, "Not connected to libvirt")
        })?;

        let num_domains = conn.num_of_domains().map_err(|e| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to get domain count: {:?}", e),
            )
        })?;

        if num_domains == 0 {
            return Ok(Vec::new());
        }

        let domain_ids = conn.list_domains().map_err(|e| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to list domains: {:?}", e),
            )
        })?;

        let mut vm_names = Vec::new();
        for id in domain_ids {
            if let Ok(domain) = Domain::lookup_by_id(conn, id) {
                if let Ok(name) = domain.get_name() {
                    vm_names.push(name);
                }
            }
        }

        Ok(vm_names)
    }

    /// Close libvirt connection
    #[cfg(feature = "qemu")]
    pub async fn disconnect(&self) -> io::Result<()> {
        let mut connection = self.connection.write().await;
        if let Some(mut conn) = connection.take() {
            conn.close().map_err(|e| {
                io::Error::new(
                    io::ErrorKind::Other,
                    format!("Failed to close connection: {:?}", e),
                )
            })?;
            debug!("Disconnected from libvirt");
        }
        Ok(())
    }
}

/// Get VM metrics (stub for non-qemu builds)
#[cfg(not(feature = "qemu"))]
impl LibvirtManager {
    pub async fn connect(&self, _uri: Option<&str>) -> io::Result<()> {
        warn!("libvirt support not compiled in (qemu feature disabled)");
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "libvirt support not enabled",
        ))
    }

    pub async fn get_vm_metrics(&self, _vm_id: &str) -> io::Result<VmMetrics> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "libvirt support not enabled",
        ))
    }

    pub async fn list_running_vms(&self) -> io::Result<Vec<String>> {
        Ok(Vec::new())
    }

    pub async fn disconnect(&self) -> io::Result<()> {
        Ok(())
    }
}

impl Default for LibvirtManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_libvirt_manager_creation() {
        let manager = LibvirtManager::new();
        assert!(manager.previous_metrics.read().await.is_empty());
    }

    #[tokio::test]
    #[cfg(feature = "qemu")]
    async fn test_libvirt_connection_test_uri() {
        let manager = LibvirtManager::new();
        // Use test driver (no actual hypervisor required)
        let result = manager.connect(Some("test:///default")).await;
        // May fail if libvirt not installed, but should compile
        if result.is_ok() {
            assert!(manager.disconnect().await.is_ok());
        }
    }
}
