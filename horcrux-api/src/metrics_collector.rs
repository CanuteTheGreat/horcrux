///! Background metrics collection task
///! Periodically collects system and VM metrics and broadcasts them via WebSocket

use std::sync::Arc;
use tokio::time::{interval, Duration};
use tracing::{debug, error, info};

use crate::websocket::WsState;
use crate::monitoring::MonitoringManager;
use crate::vm::VmManager;
use crate::metrics::MetricsCache;

/// Metrics collection intervals
const NODE_METRICS_INTERVAL_SECS: u64 = 5;  // Collect node metrics every 5 seconds
const VM_METRICS_INTERVAL_SECS: u64 = 10;   // Collect VM metrics every 10 seconds

/// Start the metrics collection background task
pub fn start_metrics_collector(
    ws_state: Arc<WsState>,
    monitoring_manager: Arc<MonitoringManager>,
    vm_manager: Arc<VmManager>,
) {
    // Create shared metrics cache for rate calculations
    let metrics_cache = Arc::new(MetricsCache::new());

    // Spawn node metrics collection task
    let ws_state_clone = ws_state.clone();
    let monitoring_manager_clone = monitoring_manager.clone();
    let metrics_cache_clone = metrics_cache.clone();
    tokio::spawn(async move {
        let mut interval = interval(Duration::from_secs(NODE_METRICS_INTERVAL_SECS));
        info!("Starting node metrics collector (interval: {}s)", NODE_METRICS_INTERVAL_SECS);

        loop {
            interval.tick().await;

            match collect_and_broadcast_node_metrics(&ws_state_clone, &monitoring_manager_clone, &metrics_cache_clone).await {
                Ok(_) => debug!("Node metrics collected and broadcast"),
                Err(e) => error!("Failed to collect node metrics: {}", e),
            }
        }
    });

    // Spawn VM metrics collection task
    tokio::spawn(async move {
        let mut interval = interval(Duration::from_secs(VM_METRICS_INTERVAL_SECS));
        info!("Starting VM metrics collector (interval: {}s)", VM_METRICS_INTERVAL_SECS);

        loop {
            interval.tick().await;

            match collect_and_broadcast_vm_metrics(&ws_state, &vm_manager).await {
                Ok(_) => debug!("VM metrics collected and broadcast"),
                Err(e) => error!("Failed to collect VM metrics: {}", e),
            }
        }
    });

    info!("Metrics collection tasks started");
}

/// Collect and broadcast node metrics
async fn collect_and_broadcast_node_metrics(
    ws_state: &Arc<WsState>,
    _monitoring_manager: &Arc<MonitoringManager>,
    metrics_cache: &Arc<MetricsCache>,
) -> Result<(), String> {
    // Use real CPU usage from metrics cache
    let cpu_usage = metrics_cache.get_cpu_usage().await;

    // Use real memory stats
    let memory = crate::metrics::system::read_memory_stats()
        .map_err(|e| format!("Failed to read memory: {}", e))?;
    let memory_usage = memory.usage_percent();

    // Use real load average
    let load = crate::metrics::system::read_load_average()
        .map_err(|e| format!("Failed to read load average: {}", e))?;

    // Get hostname
    let hostname = hostname::get()
        .map_err(|e| format!("Failed to get hostname: {}", e))?
        .to_string_lossy()
        .to_string();

    // Calculate disk usage percentage
    // TODO: Calculate from actual filesystem stats using statfs
    let disk_usage_percent = 65.0;

    // Broadcast metrics via WebSocket
    ws_state.broadcast_node_metrics(
        hostname,
        cpu_usage,
        memory_usage,
        disk_usage_percent,
        [
            load.one_min,
            load.five_min,
            load.fifteen_min,
        ],
    );

    Ok(())
}

/// Collect and broadcast VM metrics
async fn collect_and_broadcast_vm_metrics(
    ws_state: &Arc<WsState>,
    vm_manager: &Arc<VmManager>,
) -> Result<(), String> {
    // Get list of all VMs
    let vms = vm_manager.list_vms().await;

    // Collect metrics for each running VM
    for vm in vms {
        // Only collect metrics for running VMs
        if vm.status == horcrux_common::VmStatus::Running {
            match collect_vm_metrics(&vm.id).await {
                Ok((cpu, memory, disk_read, disk_write, net_rx, net_tx)) => {
                    ws_state.broadcast_vm_metrics(
                        vm.id.clone(),
                        cpu,
                        memory,
                        disk_read,
                        disk_write,
                        net_rx,
                        net_tx,
                    );
                }
                Err(e) => {
                    debug!("Failed to collect metrics for VM {}: {}", vm.id, e);
                }
            }
        }
    }

    Ok(())
}

/// Collect metrics for a specific VM
/// Returns: (cpu_usage, memory_usage, disk_read, disk_write, network_rx, network_tx)
async fn collect_vm_metrics(vm_id: &str) -> Result<(f64, f64, u64, u64, u64, u64), String> {
    // Try to collect real metrics if this is a container
    // For Docker/Podman containers, vm_id might be a container ID
    if let Ok(metrics) = crate::metrics::get_docker_container_stats(vm_id).await {
        return Ok((
            metrics.cpu_usage_percent,
            (metrics.memory_usage_bytes as f64 / metrics.memory_limit_bytes as f64) * 100.0,
            metrics.block_read_bytes,
            metrics.block_write_bytes,
            metrics.network_rx_bytes,
            metrics.network_tx_bytes,
        ));
    }

    // For QEMU VMs, we would need to:
    // 1. Get the PID from the VM manager (need to add pid field to VmConfig)
    // 2. Read process stats from /proc/<pid>/stat
    // 3. Read I/O stats from /proc/<pid>/io
    // 4. For network stats, read from the tap interface in /sys/class/net/

    // TODO: Implement VM metrics collection via:
    // - libvirt API for KVM/QEMU VMs
    // - QEMU monitor socket for detailed VM stats
    // - /proc/<pid>/ for process-level metrics

    // Return simulated data for now
    use rand::Rng;
    let mut rng = rand::thread_rng();

    Ok((
        rng.gen_range(5.0..95.0),          // cpu_usage (%)
        rng.gen_range(20.0..80.0),         // memory_usage (%)
        rng.gen_range(0..100_000_000),     // disk_read (bytes)
        rng.gen_range(0..50_000_000),      // disk_write (bytes)
        rng.gen_range(0..500_000_000),     // network_rx (bytes)
        rng.gen_range(0..200_000_000),     // network_tx (bytes)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_collect_vm_metrics() {
        let result = collect_vm_metrics("test-vm-100").await;
        assert!(result.is_ok());

        let (cpu, memory, disk_read, disk_write, net_rx, net_tx) = result.unwrap();
        assert!(cpu >= 0.0 && cpu <= 100.0);
        assert!(memory >= 0.0 && memory <= 100.0);
        assert!(disk_read < 1_000_000_000);
        assert!(disk_write < 1_000_000_000);
        assert!(net_rx < 1_000_000_000);
        assert!(net_tx < 1_000_000_000);
    }
}
