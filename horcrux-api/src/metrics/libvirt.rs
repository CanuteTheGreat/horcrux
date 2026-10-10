//! VM metrics collection via libvirt
//!
//! Provides real metrics for KVM/QEMU VMs using libvirt API

#[cfg(feature = "qemu")]
use virt::connect::Connect;
#[cfg(feature = "qemu")]
use virt::domain::Domain;

use std::io;
use std::sync::Arc;
use tokio::sync::RwLock;

#[cfg(not(feature = "qemu"))]
use tracing::warn;
#[cfg(feature = "qemu")]
use tracing::{debug, error};

/// VM metrics from libvirt
#[derive(Debug, Clone)]
pub struct VmMetrics {
    pub cpu_time: u64,          // CPU time in nanoseconds
    pub cpu_usage_percent: f64, // CPU usage percentage
    pub memory_actual: u64,     // Actual memory usage in bytes
    pub memory_rss: u64,        // Resident set size in bytes
    pub disk_read_bytes: u64,   // Disk read bytes
    pub disk_write_bytes: u64,  // Disk write bytes
    pub network_rx_bytes: u64,  // Network receive bytes
    pub network_tx_bytes: u64,  // Network transmit bytes
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
        let info = domain
            .get_info()
            .map_err(|e| io::Error::other(format!("Failed to get domain info: {:?}", e)))?;

        let memory_actual = info.memory * 1024; // Convert KB to bytes
        let cpu_time = info.cpu_time; // nanoseconds

        // Get memory stats
        let memory_rss = self.get_memory_rss(&domain).unwrap_or(memory_actual);

        // Get block device stats
        let (disk_read_bytes, disk_write_bytes) = self.get_block_stats(&domain);

        // Get network interface stats
        let (network_rx_bytes, network_tx_bytes) = self.get_network_stats(&domain);

        // Calculate CPU usage percentage (normalized across all vCPUs)
        let num_vcpus = info.nr_virt_cpu.max(1);
        let cpu_usage_percent = self
            .calculate_cpu_usage(vm_id, cpu_time, num_vcpus)
            .await;

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

    /// Calculate CPU usage percentage from CPU time delta, normalized to the
    /// domain's vCPU count so usage is reported as a percentage of the VM's
    /// total allotted CPU capacity (matching libvirt/virsh/top semantics),
    /// not percentage of a single core.
    #[cfg(feature = "qemu")]
    async fn calculate_cpu_usage(&self, vm_id: &str, current_cpu_time: u64, num_vcpus: u32) -> f64 {
        let prev_metrics = self.previous_metrics.read().await;

        if let Some(prev) = prev_metrics.get(vm_id) {
            let time_delta = prev.timestamp.elapsed().as_nanos() as u64;
            if time_delta == 0 {
                return 0.0;
            }

            let cpu_delta = current_cpu_time.saturating_sub(prev.cpu_time);

            // cpu_time accumulates across all vCPUs, so a fully-busy 4-vCPU
            // domain reports ~4x the wall-clock time delta. Divide by vCPU
            // count to get usage as a percentage of the domain's total
            // allotted capacity (0-100%), rather than percentage of one core
            // (which could read e.g. 400% for a 4-vCPU domain, or wrongly
            // report 100% for a domain pegging only 1 of 4 vCPUs).
            let usage =
                (cpu_delta as f64 / time_delta as f64) * 100.0 / num_vcpus.max(1) as f64;
            usage.clamp(0.0, 100.0)
        } else {
            0.0 // First sample, no previous data
        }
    }

    /// Get memory RSS (Resident Set Size) via libvirt's memory_stats API.
    /// libvirt reports this as VIR_DOMAIN_MEMORY_STAT_RSS, in kilobytes.
    #[cfg(feature = "qemu")]
    fn get_memory_rss(&self, domain: &Domain) -> Option<u64> {
        // Tag value from libvirt's virDomainMemoryStatTags enum (VIR_DOMAIN_MEMORY_STAT_RSS).
        const VIR_DOMAIN_MEMORY_STAT_RSS: u32 = 7;

        match domain.memory_stats(0) {
            Ok(stats) => stats
                .into_iter()
                .find(|s| s.tag == VIR_DOMAIN_MEMORY_STAT_RSS)
                .map(|s| s.val * 1024), // KB -> bytes
            Err(e) => {
                debug!("memory_stats unavailable for domain: {:?}", e);
                None
            }
        }
    }

    /// Enumerate a domain's disk/interface device names from its XML description.
    /// The virt crate has no structured device-list API, so we parse the
    /// `<target dev="...">` attributes out of `<disk>`/`<interface>` elements.
    #[cfg(feature = "qemu")]
    fn list_device_targets(domain: &Domain, element: &str) -> Vec<String> {
        let xml = match domain.get_xml_desc(0) {
            Ok(xml) => xml,
            Err(e) => {
                debug!(
                    "get_xml_desc failed, cannot enumerate {} devices: {:?}",
                    element, e
                );
                return Vec::new();
            }
        };

        let mut targets = Vec::new();
        let open_tag = format!("<{}", element);
        let mut search_from = 0;
        while let Some(rel_start) = xml[search_from..].find(&open_tag) {
            let elem_start = search_from + rel_start;
            let Some(rel_end) = xml[elem_start..].find('>') else {
                break;
            };
            let elem_close = elem_start + rel_end;
            // Find the nested <target dev="..."/> within this element's opening
            // span (covers everything up to the matching close, which is
            // sufficient since target is always a direct child near the top).
            let scan_end = (elem_close + 400).min(xml.len());
            if let Some(target_rel) = xml[elem_close..scan_end].find("<target") {
                let target_start = elem_close + target_rel;
                let dev_attr = "dev=\"";
                if let Some(dev_rel) = xml[target_start..scan_end].find(dev_attr) {
                    let dev_start = target_start + dev_rel + dev_attr.len();
                    if let Some(end_rel) = xml[dev_start..scan_end].find('"') {
                        targets.push(xml[dev_start..dev_start + end_rel].to_string());
                    }
                }
            }
            search_from = elem_close + 1;
        }
        targets
    }

    /// Get block device statistics, summed across all attached disks.
    #[cfg(feature = "qemu")]
    fn get_block_stats(&self, domain: &Domain) -> (u64, u64) {
        let disks = Self::list_device_targets(domain, "disk");
        let mut read_bytes: u64 = 0;
        let mut write_bytes: u64 = 0;

        for disk in &disks {
            match domain.get_block_stats(disk) {
                Ok(stats) => {
                    read_bytes = read_bytes.saturating_add(stats.rd_bytes.max(0) as u64);
                    write_bytes = write_bytes.saturating_add(stats.wr_bytes.max(0) as u64);
                }
                Err(e) => {
                    debug!("block_stats failed for disk {}: {:?}", disk, e);
                }
            }
        }

        (read_bytes, write_bytes)
    }

    /// Get network interface statistics, summed across all attached interfaces.
    #[cfg(feature = "qemu")]
    fn get_network_stats(&self, domain: &Domain) -> (u64, u64) {
        let ifaces = Self::list_device_targets(domain, "interface");
        let mut rx_bytes: u64 = 0;
        let mut tx_bytes: u64 = 0;

        for iface in &ifaces {
            match domain.interface_stats(iface) {
                Ok(stats) => {
                    rx_bytes = rx_bytes.saturating_add(stats.rx_bytes.max(0) as u64);
                    tx_bytes = tx_bytes.saturating_add(stats.tx_bytes.max(0) as u64);
                }
                Err(e) => {
                    debug!("interface_stats failed for iface {}: {:?}", iface, e);
                }
            }
        }

        (rx_bytes, tx_bytes)
    }

    /// List all running VMs
    #[cfg(feature = "qemu")]
    pub async fn list_running_vms(&self) -> io::Result<Vec<String>> {
        let connection = self.connection.read().await;
        let conn = connection.as_ref().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotConnected, "Not connected to libvirt")
        })?;

        let num_domains = conn
            .num_of_domains()
            .map_err(|e| io::Error::other(format!("Failed to get domain count: {:?}", e)))?;

        if num_domains == 0 {
            return Ok(Vec::new());
        }

        let domain_ids = conn
            .list_domains()
            .map_err(|e| io::Error::other(format!("Failed to list domains: {:?}", e)))?;

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
            conn.close()
                .map_err(|e| io::Error::other(format!("Failed to close connection: {:?}", e)))?;
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
