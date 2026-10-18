//! Background metrics collection task
//! Periodically collects system and VM metrics and broadcasts them via WebSocket
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::time::{interval, Duration};
use tracing::{debug, error, info};

use crate::metrics::{LibvirtManager, MetricsCache};
use crate::monitoring::MonitoringManager;
use crate::vm::VmManager;
use crate::websocket::WsState;

/// Metrics collection intervals
const NODE_METRICS_INTERVAL_SECS: u64 = 5; // Collect node metrics every 5 seconds
const VM_METRICS_INTERVAL_SECS: u64 = 10; // Collect VM metrics every 10 seconds

/// Start the metrics collection background task
pub fn start_metrics_collector(
    ws_state: Arc<WsState>,
    monitoring_manager: Arc<MonitoringManager>,
    vm_manager: Arc<VmManager>,
    libvirt_manager: Option<Arc<LibvirtManager>>,
) {
    // Create shared metrics cache for rate calculations
    let metrics_cache = Arc::new(MetricsCache::new());

    // Spawn node metrics collection task
    let ws_state_clone = ws_state.clone();
    let monitoring_manager_clone = monitoring_manager.clone();
    let metrics_cache_clone = metrics_cache.clone();
    tokio::spawn(async move {
        let mut interval = interval(Duration::from_secs(NODE_METRICS_INTERVAL_SECS));
        info!(
            "Starting node metrics collector (interval: {}s)",
            NODE_METRICS_INTERVAL_SECS
        );

        loop {
            interval.tick().await;

            match collect_and_broadcast_node_metrics(
                &ws_state_clone,
                &monitoring_manager_clone,
                &metrics_cache_clone,
            )
            .await
            {
                Ok(_) => debug!("Node metrics collected and broadcast"),
                Err(e) => error!("Failed to collect node metrics: {}", e),
            }
        }
    });

    // Spawn VM metrics collection task
    let cpu_samples = CpuSamples::default();
    tokio::spawn(async move {
        let mut interval = interval(Duration::from_secs(VM_METRICS_INTERVAL_SECS));
        info!(
            "Starting VM metrics collector (interval: {}s)",
            VM_METRICS_INTERVAL_SECS
        );

        loop {
            interval.tick().await;

            match collect_and_broadcast_vm_metrics(
                &ws_state,
                &vm_manager,
                &libvirt_manager,
                &cpu_samples,
            )
            .await
            {
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

    // Calculate real disk usage percentage for horcrux's data directory
    // (VM storage, snapshots, etc. all live under here) via statvfs(2).
    // Falls back to HORCRUX_DATA_DIR if set (matches config.rs's own
    // override), otherwise the default /var/lib/horcrux install path.
    let data_dir =
        std::env::var("HORCRUX_DATA_DIR").unwrap_or_else(|_| "/var/lib/horcrux".to_string());
    let disk_usage_percent = crate::metrics::system::read_disk_usage_percent(&data_dir)
        .unwrap_or_else(|e| {
            debug!(
                "Failed to read disk usage for {}: {} (reporting 0.0)",
                data_dir, e
            );
            0.0
        });

    // Broadcast metrics via WebSocket
    ws_state.broadcast_node_metrics(
        hostname,
        cpu_usage,
        memory_usage,
        disk_usage_percent,
        [load.one_min, load.five_min, load.fifteen_min],
    );

    Ok(())
}

/// Collect and broadcast VM metrics
async fn collect_and_broadcast_vm_metrics(
    ws_state: &Arc<WsState>,
    vm_manager: &Arc<VmManager>,
    libvirt_manager: &Option<Arc<LibvirtManager>>,
    cpu_samples: &CpuSamples,
) -> Result<(), String> {
    // Get list of all VMs
    let vms = vm_manager.list_vms().await;

    // Forget CPU samples of VMs that stopped or were deleted, so a restarted
    // VM's fresh counters are never diffed against its old process.
    if let Ok(mut samples) = cpu_samples.lock() {
        samples.retain(|id, _| {
            vms.iter()
                .any(|vm| &vm.id == id && vm.status == horcrux_common::VmStatus::Running)
        });
    }

    // Collect metrics for each running VM
    for vm in vms {
        // Only collect metrics for running VMs
        if vm.status == horcrux_common::VmStatus::Running {
            match collect_vm_metrics(&vm, libvirt_manager, cpu_samples).await {
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
async fn collect_vm_metrics(
    vm: &horcrux_common::VmConfig,
    libvirt_manager: &Option<Arc<LibvirtManager>>,
    cpu_samples: &CpuSamples,
) -> Result<(f64, f64, u64, u64, u64, u64), String> {
    let vm_id = vm.id.as_str();
    // Try libvirt first (for KVM/QEMU VMs)
    if let Some(mgr) = libvirt_manager {
        if let Ok(metrics) = mgr.get_vm_metrics(vm_id).await {
            // Calculate memory usage percentage
            // Note: libvirt provides memory in bytes, need to get max memory for percentage
            // For now, use actual memory as-is and convert to MB for reasonable display
            let memory_mb = metrics.memory_actual / 1024 / 1024;
            let memory_percent = if memory_mb > 0 {
                (metrics.memory_rss as f64 / metrics.memory_actual as f64) * 100.0
            } else {
                0.0
            };

            debug!(
                "Collected libvirt metrics for VM {}: CPU={:.1}%, MEM={:.1}%",
                vm_id, metrics.cpu_usage_percent, memory_percent
            );

            return Ok((
                metrics.cpu_usage_percent,
                memory_percent,
                metrics.disk_read_bytes,
                metrics.disk_write_bytes,
                metrics.network_rx_bytes,
                metrics.network_tx_bytes,
            ));
        }
    }

    // Horcrux's own QEMU VMs (started directly, not through libvirt, so
    // libvirt above doesn't know them): read the process found via the
    // VM's pidfile.
    if let Some(pid) = qemu_pid(vm_id) {
        return qemu_process_metrics(vm, pid, cpu_samples);
    }

    // Try to collect real metrics if this is a container
    // For Docker/Podman containers, vm_id might be a container ID
    if let Ok(metrics) = crate::metrics::get_docker_container_stats(vm_id).await {
        debug!(
            "Collected container metrics for {}: CPU={:.1}%, MEM={:.1}%",
            vm_id,
            metrics.cpu_usage_percent,
            (metrics.memory_usage_bytes as f64 / metrics.memory_limit_bytes as f64) * 100.0
        );

        return Ok((
            metrics.cpu_usage_percent,
            (metrics.memory_usage_bytes as f64 / metrics.memory_limit_bytes as f64) * 100.0,
            metrics.block_read_bytes,
            metrics.block_write_bytes,
            metrics.network_rx_bytes,
            metrics.network_tx_bytes,
        ));
    }

    // No real source. Report nothing rather than made-up numbers: this
    // used to broadcast random CPU/memory/IO figures, which the monitoring
    // page showed as if they were the VM's real load.
    Err(format!("no metrics source for VM {vm_id}"))
}

/// Last cumulative CPU time (clock ticks) seen per VM, and when.
type CpuSamples = Arc<Mutex<HashMap<String, (Instant, u64)>>>;

/// Clock ticks per second in /proc/<pid>/stat. The kernel reports these in
/// USER_HZ, which is 100 on every architecture Horcrux targets.
const USER_HZ: f64 = 100.0;

/// The live QEMU process in `vm_id`'s pidfile, if any (a stale pidfile's
/// pid may be dead or reused by something else).
fn qemu_pid(vm_id: &str) -> Option<u32> {
    if !crate::vm::qemu::QemuManager::process_running(vm_id) {
        return None;
    }
    std::fs::read_to_string(crate::vm::qemu::pidfile(vm_id))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// CPU %, memory % and cumulative disk bytes of a QEMU process. CPU % needs
/// two samples, so the first call for a VM returns an error (nothing is
/// broadcast) instead of a fake 0%.
fn qemu_process_metrics(
    vm: &horcrux_common::VmConfig,
    pid: u32,
    cpu_samples: &CpuSamples,
) -> Result<(f64, f64, u64, u64, u64, u64), String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .map_err(|e| format!("VM {}: /proc/{pid}/stat: {e}", vm.id))?;
    let ticks = proc_stat_cpu_ticks(&stat)
        .ok_or_else(|| format!("VM {}: unparseable /proc/{pid}/stat", vm.id))?;
    let rss = std::fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()
        .as_deref()
        .and_then(proc_status_rss_bytes)
        .unwrap_or(0);
    // /proc/<pid>/io is root-only for other users' processes; 0 if unreadable.
    let (disk_read, disk_write) = crate::metrics::system::read_process_io_stats(pid)
        .map(|io| (io.read_bytes, io.write_bytes))
        .unwrap_or((0, 0));

    let now = Instant::now();
    let prev = cpu_samples
        .lock()
        .map_err(|_| "CPU sample cache poisoned".to_string())?
        .insert(vm.id.clone(), (now, ticks));
    let cpu = prev
        .and_then(|(at, prev_ticks)| {
            cpu_percent(prev_ticks, ticks, now.duration_since(at), vm.cpus)
        })
        .ok_or_else(|| format!("VM {}: first CPU sample, no rate yet", vm.id))?;

    // QEMU VMs have no NIC yet (see vm::qemu::network_args): no traffic.
    Ok((
        cpu,
        memory_percent(rss, vm.memory),
        disk_read,
        disk_write,
        0,
        0,
    ))
}

/// utime + stime (fields 14 and 15) from a /proc/<pid>/stat line. Fields are
/// counted after the last ')' because the command name may contain spaces.
fn proc_stat_cpu_ticks(stat: &str) -> Option<u64> {
    let (_, rest) = stat.rsplit_once(')')?;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // rest starts at field 3 (state), so field N is fields[N - 3].
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some(utime + stime)
}

/// VmRSS from /proc/<pid>/status, in bytes.
fn proc_status_rss_bytes(status: &str) -> Option<u64> {
    let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb.saturating_mul(1024))
}

/// Share of the VM's vCPUs used between two samples, 0-100. None when no
/// time has passed or the counter went backwards (a different process).
fn cpu_percent(prev_ticks: u64, ticks: u64, elapsed: Duration, vcpus: u32) -> Option<f64> {
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 || ticks < prev_ticks {
        return None;
    }
    let used = (ticks - prev_ticks) as f64 / USER_HZ;
    Some((used / secs / f64::from(vcpus.max(1)) * 100.0).clamp(0.0, 100.0))
}

/// Resident memory as a share of the VM's configured memory (MB), 0-100.
fn memory_percent(rss_bytes: u64, memory_mb: u64) -> f64 {
    if memory_mb == 0 {
        return 0.0;
    }
    (rss_bytes as f64 / (memory_mb as f64 * 1024.0 * 1024.0) * 100.0).clamp(0.0, 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_vm(id: &str) -> horcrux_common::VmConfig {
        horcrux_common::VmConfig {
            id: id.into(),
            name: id.into(),
            hypervisor: horcrux_common::VmHypervisor::Qemu,
            memory: 1024,
            cpus: 2,
            disk_size: 10,
            status: horcrux_common::VmStatus::Running,
            architecture: Default::default(),
            disks: Vec::new(),
            display: Default::default(),
        }
    }

    #[tokio::test]
    async fn vm_without_a_metrics_source_reports_nothing_not_made_up_numbers() {
        let libvirt_manager: Option<Arc<LibvirtManager>> = None;
        let samples = CpuSamples::default();
        let vm = test_vm("no-such-vm-metrics-test");
        assert!(collect_vm_metrics(&vm, &libvirt_manager, &samples)
            .await
            .is_err());
    }

    #[test]
    fn proc_stat_ticks_survive_spaces_and_parens_in_the_command_name() {
        let stat = "4242 (qemu (x) system) S 1 4242 4242 0 -1 4194560 100 0 0 0 \
                    750 250 0 0 20 0 3 0 12345 1000000 2500 18446744073709551615";
        assert_eq!(proc_stat_cpu_ticks(stat), Some(1000));
        assert_eq!(proc_stat_cpu_ticks("4242 (qemu) S 1"), None);
        assert_eq!(proc_stat_cpu_ticks("garbage"), None);
    }

    #[test]
    fn proc_status_rss_is_bytes() {
        let status = "Name:\tqemu-system-x86\nVmPeak:\t 9000 kB\nVmRSS:\t  524288 kB\n";
        assert_eq!(proc_status_rss_bytes(status), Some(512 * 1024 * 1024));
        assert_eq!(proc_status_rss_bytes("Name:\tqemu\n"), None);
    }

    #[test]
    fn cpu_percent_is_per_vcpu_and_needs_a_forward_moving_counter() {
        let ten_s = Duration::from_secs(10);
        // 10 s of one full core (1000 ticks) on a 2-vCPU VM = 50%.
        assert_eq!(cpu_percent(0, 1000, ten_s, 2), Some(50.0));
        assert_eq!(cpu_percent(500, 500, ten_s, 1), Some(0.0));
        // Never above 100%, even with a stray burst.
        assert_eq!(cpu_percent(0, 10_000, ten_s, 1), Some(100.0));
        // 0 vCPUs configured is treated as 1, not a division by zero.
        assert_eq!(cpu_percent(0, 1000, ten_s, 0), Some(100.0));
        // Counter went backwards (pid reused) or no time passed: no rate.
        assert_eq!(cpu_percent(2000, 1000, ten_s, 1), None);
        assert_eq!(cpu_percent(0, 1000, Duration::ZERO, 1), None);
    }

    #[test]
    fn memory_percent_is_rss_over_configured_memory() {
        assert_eq!(memory_percent(512 * 1024 * 1024, 1024), 50.0);
        assert_eq!(memory_percent(4096 * 1024 * 1024, 1024), 100.0);
        assert_eq!(memory_percent(1, 0), 0.0);
    }
}
