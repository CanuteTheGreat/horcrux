///! Monitoring and metrics collection
///! Provides real-time and historical resource metrics for VMs, containers, and system

pub mod advanced_metrics;

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

        // Try to get VM PID
        let pid_result = tokio::process::Command::new("pgrep")
            .arg("-f")
            .arg(format!("qemu.*{}", vm_id))
            .output()
            .await;

        let (cpu_usage, memory_bytes) = if let Ok(pid_output) = pid_result {
            if pid_output.status.success() {
                let pid_str = String::from_utf8_lossy(&pid_output.stdout);
                if let Some(pid) = pid_str.trim().lines().next() {
                    // Read process stats from /proc/<pid>/stat
                    let stat_path = format!("/proc/{}/stat", pid);
                    let stat_result = tokio::fs::read_to_string(&stat_path).await;

                    let cpu = if let Ok(stat) = stat_result {
                        let parts: Vec<&str> = stat.split_whitespace().collect();
                        if parts.len() >= 14 {
                            let utime: u64 = parts[13].parse().unwrap_or(0);
                            let stime: u64 = parts[14].parse().unwrap_or(0);
                            let total_time = utime + stime;
                            // Convert to percentage (rough estimate)
                            (total_time as f64 / 100.0).min(100.0)
                        } else {
                            0.0
                        }
                    } else {
                        0.0
                    };

                    // Read memory from /proc/<pid>/status
                    let status_path = format!("/proc/{}/status", pid);
                    let mem = if let Ok(status) = tokio::fs::read_to_string(&status_path).await {
                        let mut mem_kb = 0u64;
                        for line in status.lines() {
                            if line.starts_with("VmRSS:") {
                                if let Some(kb_str) = line.split_whitespace().nth(1) {
                                    if let Ok(kb) = kb_str.parse::<u64>() {
                                        mem_kb = kb;
                                        break;
                                    }
                                }
                            }
                        }
                        mem_kb * 1024 // Convert KB to bytes
                    } else {
                        0u64
                    };

                    (cpu, mem)
                } else {
                    (0.0, 0u64)
                }
            } else {
                (0.0, 0u64)
            }
        } else {
            (0.0, 0u64)
        };

        // Estimate total memory (from VM config, would need to query QEMU)
        let total_memory = if memory_bytes > 0 {
            memory_bytes * 2 // Rough estimate
        } else {
            4 * 1024 * 1024 * 1024 // Default 4GB
        };

        let usage_percent = if total_memory > 0 {
            (memory_bytes as f64 / total_memory as f64) * 100.0
        } else {
            0.0
        };

        Ok(ResourceMetrics {
            id: vm_id.to_string(),
            name: format!("VM-{}", vm_id),
            timestamp,
            cpu: CpuMetrics {
                usage_percent: cpu_usage,
                cores: 2,
                load_average: cpu_usage / 100.0,
            },
            memory: MemoryMetrics {
                total_bytes: total_memory,
                used_bytes: memory_bytes,
                free_bytes: total_memory.saturating_sub(memory_bytes),
                usage_percent,
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

        // Try to read from cgroup v2
        let cgroup_path = format!("/sys/fs/cgroup/system.slice/{}", ct_id);

        // Read CPU usage from cgroup
        let cpu_usage = Self::read_cgroup_cpu(&cgroup_path).await.unwrap_or(0.0);

        // Read memory usage from cgroup
        let (memory_current, memory_max) = Self::read_cgroup_memory(&cgroup_path).await.unwrap_or((0, 1024 * 1024 * 1024));

        let usage_percent = if memory_max > 0 {
            (memory_current as f64 / memory_max as f64) * 100.0
        } else {
            0.0
        };

        Ok(ResourceMetrics {
            id: ct_id.to_string(),
            name: format!("CT-{}", ct_id),
            timestamp,
            cpu: CpuMetrics {
                usage_percent: cpu_usage,
                cores: 1,
                load_average: cpu_usage / 100.0,
            },
            memory: MemoryMetrics {
                total_bytes: memory_max,
                used_bytes: memory_current,
                free_bytes: memory_max.saturating_sub(memory_current),
                usage_percent,
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

    async fn read_cgroup_cpu(cgroup_path: &str) -> Result<f64> {
        // Read cpu.stat file
        let stat_path = format!("{}/cpu.stat", cgroup_path);
        if let Ok(content) = tokio::fs::read_to_string(&stat_path).await {
            for line in content.lines() {
                if line.starts_with("usage_usec ") {
                    if let Some(usec_str) = line.split_whitespace().nth(1) {
                        if let Ok(usec) = usec_str.parse::<u64>() {
                            // Convert microseconds to percentage (simplified)
                            return Ok((usec as f64 / 1_000_000.0).min(100.0));
                        }
                    }
                }
            }
        }
        Ok(0.0)
    }

    async fn read_cgroup_memory(cgroup_path: &str) -> Result<(u64, u64)> {
        // Read memory.current and memory.max
        let current_path = format!("{}/memory.current", cgroup_path);
        let max_path = format!("{}/memory.max", cgroup_path);

        let current = if let Ok(content) = tokio::fs::read_to_string(&current_path).await {
            content.trim().parse::<u64>().unwrap_or(0)
        } else {
            0
        };

        let max = if let Ok(content) = tokio::fs::read_to_string(&max_path).await {
            if content.trim() == "max" {
                // No limit set, use system memory as approximation
                8 * 1024 * 1024 * 1024 // 8GB default
            } else {
                content.trim().parse::<u64>().unwrap_or(1024 * 1024 * 1024)
            }
        } else {
            1024 * 1024 * 1024 // 1GB default
        };

        Ok((current, max))
    }

    async fn collect_storage_metrics(pool_name: &str) -> Result<StorageMetrics> {
        // Try ZFS first
        if let Ok(output) = tokio::process::Command::new("zpool")
            .arg("list")
            .arg("-Hp")
            .arg(pool_name)
            .output()
            .await
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let parts: Vec<&str> = stdout.split('\t').collect();
                if parts.len() >= 4 {
                    let total: u64 = parts[1].parse().unwrap_or(0);
                    let used: u64 = parts[2].parse().unwrap_or(0);
                    let available: u64 = parts[3].parse().unwrap_or(0);

                    let usage_percent = if total > 0 {
                        (used as f64 / total as f64) * 100.0
                    } else {
                        0.0
                    };

                    return Ok(StorageMetrics {
                        name: pool_name.to_string(),
                        storage_type: "zfs".to_string(),
                        total_bytes: total,
                        used_bytes: used,
                        available_bytes: available,
                        usage_percent,
                    });
                }
            }
        }

        // Try LVM
        if let Ok(output) = tokio::process::Command::new("vgs")
            .arg("--noheadings")
            .arg("--units")
            .arg("b")
            .arg("--nosuffix")
            .arg("-o")
            .arg("vg_name,vg_size,vg_free")
            .arg(pool_name)
            .output()
            .await
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let parts: Vec<&str> = stdout.split_whitespace().collect();
                if parts.len() >= 3 {
                    let total: u64 = parts[1].parse().unwrap_or(0);
                    let available: u64 = parts[2].parse().unwrap_or(0);
                    let used = total.saturating_sub(available);

                    let usage_percent = if total > 0 {
                        (used as f64 / total as f64) * 100.0
                    } else {
                        0.0
                    };

                    return Ok(StorageMetrics {
                        name: pool_name.to_string(),
                        storage_type: "lvm".to_string(),
                        total_bytes: total,
                        used_bytes: used,
                        available_bytes: available,
                        usage_percent,
                    });
                }
            }
        }

        // Fallback: assume directory storage, use df
        if let Ok(output) = tokio::process::Command::new("df")
            .arg("-B1")
            .arg(pool_name)
            .output()
            .await
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if let Some(line) = stdout.lines().nth(1) {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 4 {
                        let total: u64 = parts[1].parse().unwrap_or(0);
                        let used: u64 = parts[2].parse().unwrap_or(0);
                        let available: u64 = parts[3].parse().unwrap_or(0);

                        let usage_percent = if total > 0 {
                            (used as f64 / total as f64) * 100.0
                        } else {
                            0.0
                        };

                        return Ok(StorageMetrics {
                            name: pool_name.to_string(),
                            storage_type: "directory".to_string(),
                            total_bytes: total,
                            used_bytes: used,
                            available_bytes: available,
                            usage_percent,
                        });
                    }
                }
            }
        }

        // Fallback to placeholder
        Ok(StorageMetrics {
            name: pool_name.to_string(),
            storage_type: "unknown".to_string(),
            total_bytes: 0,
            used_bytes: 0,
            available_bytes: 0,
            usage_percent: 0.0,
        })
    }

    async fn discover_vms() -> Result<Vec<String>> {
        // Query running VMs from QEMU processes
        let output = tokio::process::Command::new("pgrep")
            .arg("-f")
            .arg("qemu-system")
            .output()
            .await?;

        if !output.status.success() {
            return Ok(vec![]);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let vm_ids: Vec<String> = stdout
            .lines()
            .filter_map(|pid| {
                // Try to extract VM ID from process command line
                // This is a simplified approach
                Some(format!("vm-{}", pid.trim()))
            })
            .collect();

        Ok(vm_ids)
    }

    async fn discover_containers() -> Result<Vec<String>> {
        // Try Docker first
        if let Ok(output) = tokio::process::Command::new("docker")
            .arg("ps")
            .arg("--format")
            .arg("{{.ID}}")
            .output()
            .await
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let ids: Vec<String> = stdout.lines().map(|s| s.trim().to_string()).collect();
                if !ids.is_empty() {
                    return Ok(ids);
                }
            }
        }

        // Try LXC
        if let Ok(output) = tokio::process::Command::new("lxc-ls")
            .output()
            .await
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let ids: Vec<String> = stdout.lines().map(|s| s.trim().to_string()).collect();
                if !ids.is_empty() {
                    return Ok(ids);
                }
            }
        }

        Ok(vec![])
    }

    async fn discover_storage_pools() -> Result<Vec<String>> {
        let mut pools = Vec::new();

        // Discover ZFS pools
        if let Ok(output) = tokio::process::Command::new("zpool")
            .arg("list")
            .arg("-H")
            .arg("-o")
            .arg("name")
            .output()
            .await
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                pools.extend(stdout.lines().map(|s| s.trim().to_string()));
            }
        }

        // Discover LVM volume groups
        if let Ok(output) = tokio::process::Command::new("vgs")
            .arg("--noheadings")
            .arg("-o")
            .arg("vg_name")
            .output()
            .await
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                pools.extend(stdout.lines().map(|s| s.trim().to_string()));
            }
        }

        Ok(pools)
    }

    async fn read_cpu_usage() -> Result<f64> {
        // Read /proc/stat and calculate CPU usage
        let stat = tokio::fs::read_to_string("/proc/stat").await?;

        // Parse first line which is aggregate CPU stats
        if let Some(line) = stat.lines().next() {
            if line.starts_with("cpu ") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 5 {
                    let user: u64 = parts[1].parse().unwrap_or(0);
                    let nice: u64 = parts[2].parse().unwrap_or(0);
                    let system: u64 = parts[3].parse().unwrap_or(0);
                    let idle: u64 = parts[4].parse().unwrap_or(0);

                    let total = user + nice + system + idle;
                    let active = user + nice + system;

                    if total > 0 {
                        return Ok((active as f64 / total as f64) * 100.0);
                    }
                }
            }
        }

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
