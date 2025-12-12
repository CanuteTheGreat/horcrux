// Allow dead code for library functions that may be used by API consumers
#![allow(dead_code)]

///! Real metrics collection module
///! Provides actual system, VM, and container metrics

pub mod system;
pub mod container;
pub mod libvirt;

// Re-export commonly used functions
pub use container::get_docker_container_stats;
pub use libvirt::LibvirtManager;

use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;

/// Metrics cache to store previous samples for rate calculations
#[derive(Clone)]
pub struct MetricsCache {
    cpu_stats: Arc<RwLock<Option<system::CpuStats>>>,
    disk_stats: Arc<RwLock<HashMap<String, system::DiskStats>>>,
    network_stats: Arc<RwLock<HashMap<String, system::NetworkStats>>>,
}

impl MetricsCache {
    pub fn new() -> Self {
        Self {
            cpu_stats: Arc::new(RwLock::new(None)),
            disk_stats: Arc::new(RwLock::new(HashMap::new())),
            network_stats: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get CPU usage percentage
    pub async fn get_cpu_usage(&self) -> f64 {
        // Read current stats
        let current = match system::read_cpu_stats() {
            Ok(stats) => stats,
            Err(_) => return 0.0,
        };

        // Get previous stats
        let mut prev_lock = self.cpu_stats.write().await;
        let usage = if let Some(prev) = prev_lock.as_ref() {
            current.usage_percent(prev)
        } else {
            0.0
        };

        // Update cache
        *prev_lock = Some(current);

        usage
    }

    /// Get disk I/O rates
    pub async fn get_disk_io_rate(&self, device: &str) -> (u64, u64) {
        // Read current stats
        let current = match system::read_disk_stats(device) {
            Ok(stats) => stats,
            Err(_) => return (0, 0),
        };

        // Get previous stats
        let mut cache_lock = self.disk_stats.write().await;
        let rates = if let Some(prev) = cache_lock.get(device) {
            let read_rate = current.read_bytes.saturating_sub(prev.read_bytes);
            let write_rate = current.write_bytes.saturating_sub(prev.write_bytes);
            (read_rate, write_rate)
        } else {
            (0, 0)
        };

        // Update cache
        cache_lock.insert(device.to_string(), current);

        rates
    }

    /// Get network I/O rates
    pub async fn get_network_io_rate(&self, interface: &str) -> (u64, u64) {
        // Read current stats
        let current = match system::read_network_stats(interface) {
            Ok(stats) => stats,
            Err(_) => return (0, 0),
        };

        // Get previous stats
        let mut cache_lock = self.network_stats.write().await;
        let rates = if let Some(prev) = cache_lock.get(interface) {
            let rx_rate = current.rx_bytes.saturating_sub(prev.rx_bytes);
            let tx_rate = current.tx_bytes.saturating_sub(prev.tx_bytes);
            (rx_rate, tx_rate)
        } else {
            (0, 0)
        };

        // Update cache
        cache_lock.insert(interface.to_string(), current);

        rates
    }
}

impl Default for MetricsCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_metrics_cache() {
        let cache = MetricsCache::new();

        // First call should return 0 (no previous sample)
        let usage1 = cache.get_cpu_usage().await;
        assert_eq!(usage1, 0.0);

        // Sleep a bit
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Second call should return actual usage
        let usage2 = cache.get_cpu_usage().await;
        assert!(usage2 >= 0.0);
        assert!(usage2 <= 100.0);
    }
}
