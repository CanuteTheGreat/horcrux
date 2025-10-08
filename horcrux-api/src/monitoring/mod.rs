///! Monitoring and metrics collection
///! Provides real-time and historical resource metrics for VMs, containers, and system

use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Resource metrics for a VM or container
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceMetrics {
    pub id: String,
    pub name: String,
    pub timestamp: i64,
    pub cpu: CpuMetrics,
    pub memory: MemoryMetrics,
    pub disk: DiskMetrics,
    pub network: NetworkMetrics,
}

/// CPU metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuMetrics {
    pub usage_percent: f64,      // 0-100 per core (can exceed 100 for multi-core)
    pub cores: u32,
    pub load_average: f64,
}

/// Memory metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryMetrics {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
    pub usage_percent: f64,
}

/// Disk I/O metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskMetrics {
    pub read_bytes_per_sec: u64,
    pub write_bytes_per_sec: u64,
    pub read_iops: u64,
    pub write_iops: u64,
    pub total_bytes: u64,
    pub used_bytes: u64,
}

/// Network metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkMetrics {
    pub rx_bytes_per_sec: u64,
    pub tx_bytes_per_sec: u64,
    pub rx_packets_per_sec: u64,
    pub tx_packets_per_sec: u64,
}

/// Storage pool metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageMetrics {
    pub name: String,
    pub storage_type: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub usage_percent: f64,
}

/// Node system metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeMetrics {
    pub hostname: String,
    pub timestamp: i64,
    pub cpu: CpuMetrics,
    pub memory: MemoryMetrics,
    pub uptime_seconds: u64,
    pub load_average_1m: f64,
    pub load_average_5m: f64,
    pub load_average_15m: f64,
}

/// Time series data point
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeSeriesPoint {
    pub timestamp: i64,
    pub value: f64,
}

/// Monitoring manager
pub struct MonitoringManager {
    vm_metrics: Arc<RwLock<HashMap<String, ResourceMetrics>>>,
    container_metrics: Arc<RwLock<HashMap<String, ResourceMetrics>>>,
    storage_metrics: Arc<RwLock<HashMap<String, StorageMetrics>>>,
    node_metrics: Arc<RwLock<Option<NodeMetrics>>>,
    // Simple in-memory time series storage (would use proper TSDB in production)
    history: Arc<RwLock<HashMap<String, Vec<TimeSeriesPoint>>>>,
    max_history_points: usize,
}

impl MonitoringManager {
    pub fn new() -> Self {
        Self {
            vm_metrics: Arc::new(RwLock::new(HashMap::new())),
            container_metrics: Arc::new(RwLock::new(HashMap::new())),
            storage_metrics: Arc::new(RwLock::new(HashMap::new())),
            node_metrics: Arc::new(RwLock::new(None)),
            history: Arc::new(RwLock::new(HashMap::new())),
            max_history_points: 1440, // 24 hours at 1-minute intervals
        }
    }

    /// Start background metrics collection
    pub async fn start_collection(&self) {
        let vm_metrics = self.vm_metrics.clone();
        let container_metrics = self.container_metrics.clone();
        let storage_metrics = self.storage_metrics.clone();
        let node_metrics = self.node_metrics.clone();
        let history = self.history.clone();
        let max_points = self.max_history_points;

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));

            loop {
                interval.tick().await;

                // Collect node metrics
                if let Ok(metrics) = Self::collect_node_metrics().await {
                    let mut node = node_metrics.write().await;
                    *node = Some(metrics.clone());

                    // Store in history
                    let mut hist = history.write().await;
                    let key = "node.cpu.usage".to_string();
                    let points = hist.entry(key).or_insert_with(Vec::new);
                    points.push(TimeSeriesPoint {
                        timestamp: metrics.timestamp,
                        value: metrics.cpu.usage_percent,
                    });
                    if points.len() > max_points {
                        points.remove(0);
                    }
                }

                // Collect VM metrics
                if let Ok(vms) = Self::discover_vms().await {
                    for vm_id in vms {
                        if let Ok(metrics) = Self::collect_vm_metrics(&vm_id).await {
                            let mut vms = vm_metrics.write().await;
                            vms.insert(vm_id.clone(), metrics.clone());

                            // Store in history
                            let mut hist = history.write().await;
                            let key = format!("vm.{}.cpu.usage", vm_id);
                            let points = hist.entry(key).or_insert_with(Vec::new);
                            points.push(TimeSeriesPoint {
                                timestamp: metrics.timestamp,
                                value: metrics.cpu.usage_percent,
                            });
                            if points.len() > max_points {
                                points.remove(0);
                            }
                        }
                    }
                }

                // Collect container metrics
                if let Ok(containers) = Self::discover_containers().await {
                    for ct_id in containers {
                        if let Ok(metrics) = Self::collect_container_metrics(&ct_id).await {
                            let mut cts = container_metrics.write().await;
                            cts.insert(ct_id.clone(), metrics.clone());

                            // Store in history
                            let mut hist = history.write().await;
                            let key = format!("container.{}.cpu.usage", ct_id);
                            let points = hist.entry(key).or_insert_with(Vec::new);
                            points.push(TimeSeriesPoint {
                                timestamp: metrics.timestamp,
                                value: metrics.cpu.usage_percent,
                            });
                            if points.len() > max_points {
                                points.remove(0);
                            }
                        }
                    }
                }

                // Collect storage metrics
                if let Ok(pools) = Self::discover_storage_pools().await {
                    for pool_name in pools {
                        if let Ok(metrics) = Self::collect_storage_metrics(&pool_name).await {
                            let mut storage = storage_metrics.write().await;
                            storage.insert(pool_name, metrics);
                        }
                    }
                }
            }
        });
    }

    /// Get current VM metrics
    pub async fn get_vm_metrics(&self, vm_id: &str) -> Option<ResourceMetrics> {
        let metrics = self.vm_metrics.read().await;
        metrics.get(vm_id).cloned()
    }

    /// Get all VM metrics
    pub async fn list_vm_metrics(&self) -> Vec<ResourceMetrics> {
        let metrics = self.vm_metrics.read().await;
        metrics.values().cloned().collect()
    }

    /// Get current container metrics
    pub async fn get_container_metrics(&self, ct_id: &str) -> Option<ResourceMetrics> {
        let metrics = self.container_metrics.read().await;
        metrics.get(ct_id).cloned()
    }

    /// Get all container metrics
    pub async fn list_container_metrics(&self) -> Vec<ResourceMetrics> {
        let metrics = self.container_metrics.read().await;
        metrics.values().cloned().collect()
    }

    /// Get storage metrics
    pub async fn get_storage_metrics(&self, pool_name: &str) -> Option<StorageMetrics> {
        let metrics = self.storage_metrics.read().await;
        metrics.get(pool_name).cloned()
    }

    /// Get all storage metrics
    pub async fn list_storage_metrics(&self) -> Vec<StorageMetrics> {
        let metrics = self.storage_metrics.read().await;
        metrics.values().cloned().collect()
    }

    /// Get node metrics
    pub async fn get_node_metrics(&self) -> Option<NodeMetrics> {
        let metrics = self.node_metrics.read().await;
        metrics.clone()
    }

    /// Get historical data
    pub async fn get_history(&self, metric_key: &str, from: i64, to: i64) -> Vec<TimeSeriesPoint> {
        let history = self.history.read().await;
        if let Some(points) = history.get(metric_key) {
            points
                .iter()
                .filter(|p| p.timestamp >= from && p.timestamp <= to)
                .cloned()
                .collect()
        } else {
            Vec::new()
        }
    }

    // Private collection methods

    async fn collect_node_metrics() -> Result<NodeMetrics> {
        let timestamp = chrono::Utc::now().timestamp();

        // Read /proc/stat for CPU
        let cpu_usage = Self::read_cpu_usage().await?;

        // Read /proc/meminfo for memory
        let memory = Self::read_memory_info().await?;

        // Read /proc/uptime
        let uptime = Self::read_uptime().await?;

        // Read /proc/loadavg
        let (load1, load5, load15) = Self::read_load_average().await?;

        Ok(NodeMetrics {
            hostname: hostname::get()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            timestamp,
            cpu: CpuMetrics {
                usage_percent: cpu_usage,
                cores: num_cpus::get() as u32,
                load_average: load1,
            },
            memory,
            uptime_seconds: uptime,
            load_average_1m: load1,
            load_average_5m: load5,
            load_average_15m: load15,
        })
    }

    async fn collect_vm_metrics(vm_id: &str) -> Result<ResourceMetrics> {
        let timestamp = chrono::Utc::now().timestamp();

        // Use virsh or qemu monitor to get stats
        // For now, return placeholder data
        Ok(ResourceMetrics {
            id: vm_id.to_string(),
            name: format!("VM-{}", vm_id),
            timestamp,
            cpu: CpuMetrics {
                usage_percent: 0.0,
                cores: 2,
                load_average: 0.0,
            },
            memory: MemoryMetrics {
                total_bytes: 4 * 1024 * 1024 * 1024,
                used_bytes: 2 * 1024 * 1024 * 1024,
                free_bytes: 2 * 1024 * 1024 * 1024,
                usage_percent: 50.0,
            },
            disk: DiskMetrics {
                read_bytes_per_sec: 0,
                write_bytes_per_sec: 0,
                read_iops: 0,
                write_iops: 0,
                total_bytes: 100 * 1024 * 1024 * 1024,
                used_bytes: 50 * 1024 * 1024 * 1024,
            },
            network: NetworkMetrics {
                rx_bytes_per_sec: 0,
                tx_bytes_per_sec: 0,
                rx_packets_per_sec: 0,
                tx_packets_per_sec: 0,
            },
        })
    }

    async fn collect_container_metrics(ct_id: &str) -> Result<ResourceMetrics> {
        let timestamp = chrono::Utc::now().timestamp();

        // Read from cgroup v2 or docker stats
        // For now, return placeholder data
        Ok(ResourceMetrics {
            id: ct_id.to_string(),
            name: format!("CT-{}", ct_id),
            timestamp,
            cpu: CpuMetrics {
                usage_percent: 0.0,
                cores: 1,
                load_average: 0.0,
            },
            memory: MemoryMetrics {
                total_bytes: 1024 * 1024 * 1024,
                used_bytes: 512 * 1024 * 1024,
                free_bytes: 512 * 1024 * 1024,
                usage_percent: 50.0,
            },
            disk: DiskMetrics {
                read_bytes_per_sec: 0,
                write_bytes_per_sec: 0,
                read_iops: 0,
                write_iops: 0,
                total_bytes: 20 * 1024 * 1024 * 1024,
                used_bytes: 10 * 1024 * 1024 * 1024,
            },
            network: NetworkMetrics {
                rx_bytes_per_sec: 0,
                tx_bytes_per_sec: 0,
                rx_packets_per_sec: 0,
                tx_packets_per_sec: 0,
            },
        })
    }

    async fn collect_storage_metrics(pool_name: &str) -> Result<StorageMetrics> {
        // Parse zpool list, ceph df, vgs, or df output
        // For now, return placeholder data
        Ok(StorageMetrics {
            name: pool_name.to_string(),
            storage_type: "zfs".to_string(),
            total_bytes: 1024 * 1024 * 1024 * 1024,
            used_bytes: 512 * 1024 * 1024 * 1024,
            available_bytes: 512 * 1024 * 1024 * 1024,
            usage_percent: 50.0,
        })
    }

    async fn discover_vms() -> Result<Vec<String>> {
        // Query running VMs from libvirt/qemu
        Ok(vec![])
    }

    async fn discover_containers() -> Result<Vec<String>> {
        // Query running containers from docker/podman/lxc
        Ok(vec![])
    }

    async fn discover_storage_pools() -> Result<Vec<String>> {
        // Query storage pools
        Ok(vec![])
    }

    async fn read_cpu_usage() -> Result<f64> {
        // Read /proc/stat and calculate CPU usage
        // Simplified - return 0 for now
        Ok(0.0)
    }

    async fn read_memory_info() -> Result<MemoryMetrics> {
        let meminfo = tokio::fs::read_to_string("/proc/meminfo").await?;

        let mut total = 0u64;
        let mut available = 0u64;

        for line in meminfo.lines() {
            if line.starts_with("MemTotal:") {
                total = line.split_whitespace()
                    .nth(1)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0) * 1024; // Convert from KB to bytes
            } else if line.starts_with("MemAvailable:") {
                available = line.split_whitespace()
                    .nth(1)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0) * 1024;
            }
        }

        let used = total.saturating_sub(available);
        let usage_percent = if total > 0 {
            (used as f64 / total as f64) * 100.0
        } else {
            0.0
        };

        Ok(MemoryMetrics {
            total_bytes: total,
            used_bytes: used,
            free_bytes: available,
            usage_percent,
        })
    }

    async fn read_uptime() -> Result<u64> {
        let uptime_str = tokio::fs::read_to_string("/proc/uptime").await?;
        let uptime = uptime_str
            .split_whitespace()
            .next()
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        Ok(uptime as u64)
    }

    async fn read_load_average() -> Result<(f64, f64, f64)> {
        let loadavg = tokio::fs::read_to_string("/proc/loadavg").await?;
        let parts: Vec<&str> = loadavg.split_whitespace().collect();

        let load1 = parts.get(0).and_then(|s| s.parse().ok()).unwrap_or(0.0);
        let load5 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0.0);
        let load15 = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0.0);

        Ok((load1, load5, load15))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_monitoring_manager() {
        let manager = MonitoringManager::new();
        assert!(manager.get_node_metrics().await.is_none());
    }

    #[tokio::test]
    async fn test_read_memory_info() {
        let result = MonitoringManager::read_memory_info().await;
        assert!(result.is_ok());
        let mem = result.unwrap();
        assert!(mem.total_bytes > 0);
    }
}
