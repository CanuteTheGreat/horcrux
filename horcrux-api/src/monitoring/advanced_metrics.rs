//! Advanced system metrics
//!
//! Provides Pressure Stall Information (PSI), ZFS ARC stats,
//! and other advanced performance metrics

use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::fs;
use tracing::{error, warn};

/// Pressure Stall Information (PSI) metrics
/// Available on Linux 4.20+ with CONFIG_PSI=y
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PsiMetrics {
    pub cpu: Option<PsiResource>,
    pub memory: Option<PsiResource>,
    pub io: Option<PsiResource>,
}

/// PSI resource pressure information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PsiResource {
    /// Some tasks are stalled (avg over 10s, 60s, 300s)
    pub some_avg10: f64,
    pub some_avg60: f64,
    pub some_avg300: f64,
    pub some_total: u64, // Total stall time in microseconds

    /// All tasks are stalled (not available for CPU)
    pub full_avg10: Option<f64>,
    pub full_avg60: Option<f64>,
    pub full_avg300: Option<f64>,
    pub full_total: Option<u64>,
}

/// ZFS ARC (Adaptive Replacement Cache) statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZfsArcStats {
    pub size: u64,           // Current ARC size in bytes
    pub target_size: u64,    // Target ARC size
    pub min_size: u64,       // Minimum ARC size
    pub max_size: u64,       // Maximum ARC size
    pub hits: u64,           // Cache hits
    pub misses: u64,         // Cache misses
    pub hit_ratio: f64,      // Hit ratio percentage
    pub mru_size: u64,       // MRU (Most Recently Used) size
    pub mfu_size: u64,       // MFU (Most Frequently Used) size
    pub metadata_size: u64,  // Metadata cache size
    pub data_size: u64,      // Data cache size
    pub evict_skip: u64,     // Eviction skips
    pub l2_size: u64,        // L2ARC size
    pub l2_hits: u64,        // L2ARC hits
    pub l2_misses: u64,      // L2ARC misses
}

/// NUMA (Non-Uniform Memory Access) statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NumaStats {
    pub nodes: Vec<NumaNode>,
}

/// NUMA node information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NumaNode {
    pub node_id: u32,
    pub total_memory_kb: u64,
    pub free_memory_kb: u64,
    pub used_memory_kb: u64,
    pub cpus: Vec<u32>,
}

/// Advanced metrics collector
pub struct AdvancedMetrics {}

impl AdvancedMetrics {
    pub fn new() -> Self {
        Self {}
    }

    /// Collect all advanced metrics
    pub async fn collect_all(&self) -> AdvancedMetricsSnapshot {
        AdvancedMetricsSnapshot {
            psi: self.collect_psi().await,
            zfs_arc: self.collect_zfs_arc().await,
            numa: self.collect_numa().await,
            timestamp: chrono::Utc::now(),
        }
    }

    /// Collect Pressure Stall Information (PSI)
    pub async fn collect_psi(&self) -> Option<PsiMetrics> {
        let cpu = self.read_psi_resource("/proc/pressure/cpu").await;
        let memory = self.read_psi_resource("/proc/pressure/memory").await;
        let io = self.read_psi_resource("/proc/pressure/io").await;

        // If any PSI metric is available, return the metrics
        if cpu.is_some() || memory.is_some() || io.is_some() {
            Some(PsiMetrics { cpu, memory, io })
        } else {
            None
        }
    }

    /// Read PSI metrics for a specific resource
    async fn read_psi_resource(&self, path: &str) -> Option<PsiResource> {
        let content = fs::read_to_string(path).await.ok()?;

        let mut some_avg10 = 0.0;
        let mut some_avg60 = 0.0;
        let mut some_avg300 = 0.0;
        let mut some_total = 0u64;
        let mut full_avg10 = None;
        let mut full_avg60 = None;
        let mut full_avg300 = None;
        let mut full_total = None;

        for line in content.lines() {
            if line.starts_with("some") {
                // Parse: some avg10=0.00 avg60=0.00 avg300=0.00 total=123456
                for part in line.split_whitespace().skip(1) {
                    if let Some(value) = part.strip_prefix("avg10=") {
                        some_avg10 = value.parse().unwrap_or(0.0);
                    } else if let Some(value) = part.strip_prefix("avg60=") {
                        some_avg60 = value.parse().unwrap_or(0.0);
                    } else if let Some(value) = part.strip_prefix("avg300=") {
                        some_avg300 = value.parse().unwrap_or(0.0);
                    } else if let Some(value) = part.strip_prefix("total=") {
                        some_total = value.parse().unwrap_or(0);
                    }
                }
            } else if line.starts_with("full") {
                // Parse full metrics (not available for CPU)
                let mut f_avg10 = 0.0;
                let mut f_avg60 = 0.0;
                let mut f_avg300 = 0.0;
                let mut f_total = 0u64;

                for part in line.split_whitespace().skip(1) {
                    if let Some(value) = part.strip_prefix("avg10=") {
                        f_avg10 = value.parse().unwrap_or(0.0);
                    } else if let Some(value) = part.strip_prefix("avg60=") {
                        f_avg60 = value.parse().unwrap_or(0.0);
                    } else if let Some(value) = part.strip_prefix("avg300=") {
                        f_avg300 = value.parse().unwrap_or(0.0);
                    } else if let Some(value) = part.strip_prefix("total=") {
                        f_total = value.parse().unwrap_or(0);
                    }
                }

                full_avg10 = Some(f_avg10);
                full_avg60 = Some(f_avg60);
                full_avg300 = Some(f_avg300);
                full_total = Some(f_total);
            }
        }

        Some(PsiResource {
            some_avg10,
            some_avg60,
            some_avg300,
            some_total,
            full_avg10,
            full_avg60,
            full_avg300,
            full_total,
        })
    }

    /// Collect ZFS ARC statistics
    pub async fn collect_zfs_arc(&self) -> Option<ZfsArcStats> {
        // Read /proc/spl/kstat/zfs/arcstats
        let content = fs::read_to_string("/proc/spl/kstat/zfs/arcstats")
            .await
            .ok()?;

        let mut stats = HashMap::new();

        for line in content.lines().skip(2) {
            // Skip header lines
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 {
                let name = parts[0];
                let value = parts[2].parse::<u64>().unwrap_or(0);
                stats.insert(name.to_string(), value);
            }
        }

        let hits = stats.get("hits").copied().unwrap_or(0);
        let misses = stats.get("misses").copied().unwrap_or(0);
        let total_accesses = hits + misses;
        let hit_ratio = if total_accesses > 0 {
            (hits as f64 / total_accesses as f64) * 100.0
        } else {
            0.0
        };

        Some(ZfsArcStats {
            size: stats.get("size").copied().unwrap_or(0),
            target_size: stats.get("c").copied().unwrap_or(0),
            min_size: stats.get("c_min").copied().unwrap_or(0),
            max_size: stats.get("c_max").copied().unwrap_or(0),
            hits,
            misses,
            hit_ratio,
            mru_size: stats.get("mru_size").copied().unwrap_or(0),
            mfu_size: stats.get("mfu_size").copied().unwrap_or(0),
            metadata_size: stats.get("metadata_size").copied().unwrap_or(0),
            data_size: stats.get("data_size").copied().unwrap_or(0),
            evict_skip: stats.get("evict_skip").copied().unwrap_or(0),
            l2_size: stats.get("l2_size").copied().unwrap_or(0),
            l2_hits: stats.get("l2_hits").copied().unwrap_or(0),
            l2_misses: stats.get("l2_misses").copied().unwrap_or(0),
        })
    }

    /// Collect NUMA statistics
    pub async fn collect_numa(&self) -> Option<NumaStats> {
        let mut nodes = Vec::new();

        // Check how many NUMA nodes exist
        let node_dirs = fs::read_dir("/sys/devices/system/node")
            .await
            .ok()?;

        let mut node_entries = Vec::new();
        let mut dir_stream = node_dirs;

        while let Some(entry) = dir_stream.next_entry().await.ok()? {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("node") && name_str.len() > 4 {
                if let Ok(node_id) = name_str[4..].parse::<u32>() {
                    node_entries.push(node_id);
                }
            }
        }

        for node_id in node_entries {
            if let Some(node) = self.read_numa_node(node_id).await {
                nodes.push(node);
            }
        }

        if nodes.is_empty() {
            None
        } else {
            Some(NumaStats { nodes })
        }
    }

    /// Read NUMA node information
    async fn read_numa_node(&self, node_id: u32) -> Option<NumaNode> {
        let base_path = format!("/sys/devices/system/node/node{}", node_id);

        // Read memory info
        let meminfo_path = format!("{}/meminfo", base_path);
        let meminfo = fs::read_to_string(&meminfo_path).await.ok()?;

        let mut total_memory_kb = 0u64;
        let mut free_memory_kb = 0u64;

        for line in meminfo.lines() {
            if line.contains("MemTotal:") {
                total_memory_kb = line
                    .split_whitespace()
                    .nth(3)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
            } else if line.contains("MemFree:") {
                free_memory_kb = line
                    .split_whitespace()
                    .nth(3)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
            }
        }

        let used_memory_kb = total_memory_kb.saturating_sub(free_memory_kb);

        // Read CPU list
        let cpulist_path = format!("{}/cpulist", base_path);
        let cpulist = fs::read_to_string(&cpulist_path).await.ok()?;
        let cpus = self.parse_cpu_list(&cpulist.trim());

        Some(NumaNode {
            node_id,
            total_memory_kb,
            free_memory_kb,
            used_memory_kb,
            cpus,
        })
    }

    /// Parse CPU list from NUMA node
    /// Format: "0-3,8-11" or "0,2,4,6"
    fn parse_cpu_list(&self, cpulist: &str) -> Vec<u32> {
        let mut cpus = Vec::new();

        for part in cpulist.split(',') {
            if part.contains('-') {
                // Range: "0-3"
                let range: Vec<&str> = part.split('-').collect();
                if range.len() == 2 {
                    if let (Ok(start), Ok(end)) = (range[0].parse::<u32>(), range[1].parse::<u32>()) {
                        for cpu in start..=end {
                            cpus.push(cpu);
                        }
                    }
                }
            } else {
                // Single CPU: "0"
                if let Ok(cpu) = part.parse::<u32>() {
                    cpus.push(cpu);
                }
            }
        }

        cpus
    }
}

/// Combined advanced metrics snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvancedMetricsSnapshot {
    pub psi: Option<PsiMetrics>,
    pub zfs_arc: Option<ZfsArcStats>,
    pub numa: Option<NumaStats>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl AdvancedMetricsSnapshot {
    /// Check if system is under pressure
    pub fn is_under_pressure(&self) -> bool {
        if let Some(ref psi) = self.psi {
            // Check if any resource has significant pressure (>10% avg60)
            if let Some(ref cpu) = psi.cpu {
                if cpu.some_avg60 > 10.0 {
                    return true;
                }
            }
            if let Some(ref memory) = psi.memory {
                if memory.some_avg60 > 10.0 {
                    return true;
                }
            }
            if let Some(ref io) = psi.io {
                if io.some_avg60 > 10.0 {
                    return true;
                }
            }
        }
        false
    }

    /// Get ZFS ARC efficiency rating
    pub fn arc_efficiency(&self) -> Option<String> {
        self.zfs_arc.as_ref().map(|arc| {
            if arc.hit_ratio > 95.0 {
                "Excellent".to_string()
            } else if arc.hit_ratio > 85.0 {
                "Good".to_string()
            } else if arc.hit_ratio > 70.0 {
                "Fair".to_string()
            } else {
                "Poor".to_string()
            }
        })
    }

    /// Check if NUMA imbalance exists
    pub fn has_numa_imbalance(&self) -> bool {
        if let Some(ref numa) = self.numa {
            if numa.nodes.len() < 2 {
                return false;
            }

            // Calculate average memory usage
            let total_used: u64 = numa.nodes.iter().map(|n| n.used_memory_kb).sum();
            let total_capacity: u64 = numa.nodes.iter().map(|n| n.total_memory_kb).sum();

            if total_capacity == 0 {
                return false;
            }

            let avg_usage_ratio = total_used as f64 / total_capacity as f64;

            // Check if any node deviates significantly from average
            for node in &numa.nodes {
                if node.total_memory_kb > 0 {
                    let node_usage_ratio = node.used_memory_kb as f64 / node.total_memory_kb as f64;
                    let deviation = (node_usage_ratio - avg_usage_ratio).abs();

                    // More than 20% deviation indicates imbalance
                    if deviation > 0.2 {
                        return true;
                    }
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_collect_psi() {
        let metrics = AdvancedMetrics::new();
        let psi = metrics.collect_psi().await;

        // PSI may not be available on all systems
        if psi.is_some() {
            println!("PSI metrics available");
        } else {
            println!("PSI metrics not available (kernel < 4.20 or CONFIG_PSI=n)");
        }
    }

    #[tokio::test]
    async fn test_collect_zfs_arc() {
        let metrics = AdvancedMetrics::new();
        let arc = metrics.collect_zfs_arc().await;

        // ZFS may not be installed
        if arc.is_some() {
            println!("ZFS ARC metrics available");
        } else {
            println!("ZFS not available");
        }
    }

    #[test]
    fn test_parse_cpu_list() {
        let metrics = AdvancedMetrics::new();

        let cpus = metrics.parse_cpu_list("0-3");
        assert_eq!(cpus, vec![0, 1, 2, 3]);

        let cpus = metrics.parse_cpu_list("0,2,4,6");
        assert_eq!(cpus, vec![0, 2, 4, 6]);

        let cpus = metrics.parse_cpu_list("0-1,4-5");
        assert_eq!(cpus, vec![0, 1, 4, 5]);
    }
}
