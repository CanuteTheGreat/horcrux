///! Background metrics collection task
///! Periodically collects system and VM metrics and broadcasts them via WebSocket

use std::sync::Arc;
use tokio::time::{interval, Duration};
use tracing::{debug, error, info};

use crate::websocket::WsState;
use crate::monitoring::MonitoringManager;
use crate::vm::VmManager;

/// Metrics collection intervals
const NODE_METRICS_INTERVAL_SECS: u64 = 5;  // Collect node metrics every 5 seconds
const VM_METRICS_INTERVAL_SECS: u64 = 10;   // Collect VM metrics every 10 seconds

/// Start the metrics collection background task
pub fn start_metrics_collector(
    ws_state: Arc<WsState>,
    monitoring_manager: Arc<MonitoringManager>,
    vm_manager: Arc<VmManager>,
) {
    // Spawn node metrics collection task
    let ws_state_clone = ws_state.clone();
    let monitoring_manager_clone = monitoring_manager.clone();
    tokio::spawn(async move {
        let mut interval = interval(Duration::from_secs(NODE_METRICS_INTERVAL_SECS));
        info!("Starting node metrics collector (interval: {}s)", NODE_METRICS_INTERVAL_SECS);

        loop {
            interval.tick().await;

            match collect_and_broadcast_node_metrics(&ws_state_clone, &monitoring_manager_clone).await {
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
    monitoring_manager: &Arc<MonitoringManager>,
) -> Result<(), String> {
    // Get node metrics from monitoring manager
    let metrics = match monitoring_manager.get_node_metrics().await {
        Some(m) => m,
        None => return Err("No node metrics available".to_string()),
    };

    // Get hostname
    let hostname = hostname::get()
        .map_err(|e| format!("Failed to get hostname: {}", e))?
        .to_string_lossy()
        .to_string();

    // Calculate disk usage percentage
    // For simplicity, we'll assume a fixed disk size and usage
    // In production, this should come from actual filesystem stats
    let disk_usage_percent = 65.0; // TODO: Calculate from actual disk usage

    // Broadcast metrics via WebSocket
    ws_state.broadcast_node_metrics(
        hostname,
        metrics.cpu.usage_percent,
        metrics.memory.usage_percent,
        disk_usage_percent,
        [
            metrics.load_average_1m,
            metrics.load_average_5m,
            metrics.load_average_15m,
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
async fn collect_vm_metrics(_vm_id: &str) -> Result<(f64, f64, u64, u64, u64, u64), String> {
    // In a real implementation, this would read from:
    // - /proc/<pid>/stat for CPU
    // - /proc/<pid>/status for memory
    // - /proc/<pid>/io for disk I/O
    // - /sys/class/net/<interface>/statistics/ for network

    // For now, we'll use simulated data
    // TODO: Replace with actual metric collection using libvirt or QEMU monitor

    use rand::Rng;
    let mut rng = rand::thread_rng();

    Ok((
        rng.gen_range(5.0..95.0),    // cpu_usage (%)
        rng.gen_range(20.0..80.0),   // memory_usage (%)
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
