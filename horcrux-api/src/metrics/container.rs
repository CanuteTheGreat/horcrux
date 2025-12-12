///! Container metrics collection via cgroups
///! Supports both cgroups v1 and v2

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// Container metrics
#[derive(Debug, Clone)]
pub struct ContainerMetrics {
    pub cpu_usage_percent: f64,
    pub memory_usage_bytes: u64,
    pub memory_limit_bytes: u64,
    pub network_rx_bytes: u64,
    pub network_tx_bytes: u64,
    pub block_read_bytes: u64,
    pub block_write_bytes: u64,
}

/// Previous CPU measurement for delta calculation
#[derive(Debug, Clone)]
struct CpuSample {
    cpu_usage_ns: u64,
    timestamp_ns: u64,
}

/// Cache of previous CPU samples for calculating CPU percentage
static CPU_SAMPLES: OnceLock<Mutex<HashMap<String, CpuSample>>> = OnceLock::new();

fn get_cpu_samples() -> &'static Mutex<HashMap<String, CpuSample>> {
    CPU_SAMPLES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Get current timestamp in nanoseconds
fn current_timestamp_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// Calculate CPU percentage using previous sample
fn calculate_cpu_percent(container_id: &str, current_cpu_ns: u64) -> f64 {
    let current_time_ns = current_timestamp_ns();

    let mut samples = get_cpu_samples().lock().unwrap();

    let cpu_percent = if let Some(prev) = samples.get(container_id) {
        let cpu_delta = current_cpu_ns.saturating_sub(prev.cpu_usage_ns) as f64;
        let time_delta = current_time_ns.saturating_sub(prev.timestamp_ns) as f64;

        if time_delta > 0.0 {
            // CPU percentage = (CPU time used / elapsed time) * 100
            // CPU time is in nanoseconds, elapsed time is also in nanoseconds
            (cpu_delta / time_delta) * 100.0
        } else {
            0.0
        }
    } else {
        0.0 // First sample, no delta available
    };

    // Store current sample for next calculation
    samples.insert(
        container_id.to_string(),
        CpuSample {
            cpu_usage_ns: current_cpu_ns,
            timestamp_ns: current_time_ns,
        },
    );

    cpu_percent
}

/// Detect cgroups version
pub fn detect_cgroups_version() -> io::Result<u8> {
    if Path::new("/sys/fs/cgroup/cgroup.controllers").exists() {
        Ok(2) // cgroups v2
    } else if Path::new("/sys/fs/cgroup/cpu").exists() {
        Ok(1) // cgroups v1
    } else {
        Err(io::Error::new(io::ErrorKind::NotFound, "cgroups not found"))
    }
}

/// Get cgroup path for a container (Docker/Podman)
pub fn get_container_cgroup_path(container_id: &str, subsystem: &str) -> io::Result<PathBuf> {
    let version = detect_cgroups_version()?;

    if version == 2 {
        // cgroups v2: unified hierarchy
        let paths = vec![
            format!("/sys/fs/cgroup/system.slice/docker-{}.scope", container_id),
            format!("/sys/fs/cgroup/machine.slice/libpod-{}.scope", container_id),
            format!("/sys/fs/cgroup/docker/{}", container_id),
        ];

        for path in paths {
            if Path::new(&path).exists() {
                return Ok(PathBuf::from(path));
            }
        }
    } else {
        // cgroups v1: separate hierarchies per subsystem
        let paths = vec![
            format!("/sys/fs/cgroup/{}/system.slice/docker-{}.scope", subsystem, container_id),
            format!("/sys/fs/cgroup/{}/docker/{}", subsystem, container_id),
            format!("/sys/fs/cgroup/{}/machine.slice/libpod-{}.scope", subsystem, container_id),
        ];

        for path in paths {
            if Path::new(&path).exists() {
                return Ok(PathBuf::from(path));
            }
        }
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("cgroup path not found for container {}", container_id),
    ))
}

/// Read CPU usage from cgroups
pub fn read_container_cpu_usage(container_id: &str) -> io::Result<u64> {
    let version = detect_cgroups_version()?;

    if version == 2 {
        // cgroups v2
        let cgroup_path = get_container_cgroup_path(container_id, "")?;
        let stat_path = cgroup_path.join("cpu.stat");
        let content = fs::read_to_string(stat_path)?;

        for line in content.lines() {
            if line.starts_with("usage_usec ") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    let usage_usec: u64 = parts[1].parse().unwrap_or(0);
                    return Ok(usage_usec * 1000); // Convert to nanoseconds
                }
            }
        }
    } else {
        // cgroups v1
        let cgroup_path = get_container_cgroup_path(container_id, "cpuacct")?;
        let usage_path = cgroup_path.join("cpuacct.usage");
        let content = fs::read_to_string(usage_path)?;
        return Ok(content.trim().parse().unwrap_or(0));
    }

    Err(io::Error::new(io::ErrorKind::NotFound, "CPU usage not found"))
}

/// Read memory usage from cgroups
pub fn read_container_memory_usage(container_id: &str) -> io::Result<(u64, u64)> {
    let version = detect_cgroups_version()?;

    if version == 2 {
        // cgroups v2
        let cgroup_path = get_container_cgroup_path(container_id, "")?;

        // Read current memory usage
        let current_path = cgroup_path.join("memory.current");
        let usage = fs::read_to_string(current_path)?
            .trim()
            .parse()
            .unwrap_or(0);

        // Read memory limit
        let max_path = cgroup_path.join("memory.max");
        let limit = fs::read_to_string(max_path)?
            .trim()
            .parse()
            .unwrap_or(u64::MAX);

        Ok((usage, limit))
    } else {
        // cgroups v1
        let cgroup_path = get_container_cgroup_path(container_id, "memory")?;

        // Read current memory usage
        let usage_path = cgroup_path.join("memory.usage_in_bytes");
        let usage = fs::read_to_string(usage_path)?
            .trim()
            .parse()
            .unwrap_or(0);

        // Read memory limit
        let limit_path = cgroup_path.join("memory.limit_in_bytes");
        let limit = fs::read_to_string(limit_path)?
            .trim()
            .parse()
            .unwrap_or(u64::MAX);

        Ok((usage, limit))
    }
}

/// Read block I/O stats from cgroups
pub fn read_container_blkio_stats(container_id: &str) -> io::Result<(u64, u64)> {
    let version = detect_cgroups_version()?;

    if version == 2 {
        // cgroups v2
        let cgroup_path = get_container_cgroup_path(container_id, "")?;
        let io_stat_path = cgroup_path.join("io.stat");
        let content = fs::read_to_string(io_stat_path)?;

        let mut read_bytes = 0;
        let mut write_bytes = 0;

        for line in content.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            for part in parts.iter().skip(1) {
                if part.starts_with("rbytes=") {
                    read_bytes += part[7..].parse::<u64>().unwrap_or(0);
                } else if part.starts_with("wbytes=") {
                    write_bytes += part[7..].parse::<u64>().unwrap_or(0);
                }
            }
        }

        Ok((read_bytes, write_bytes))
    } else {
        // cgroups v1
        let cgroup_path = get_container_cgroup_path(container_id, "blkio")?;
        let io_stat_path = cgroup_path.join("blkio.throttle.io_service_bytes");
        let content = fs::read_to_string(io_stat_path)?;

        let mut read_bytes = 0;
        let mut write_bytes = 0;

        for line in content.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 {
                let value: u64 = parts[2].parse().unwrap_or(0);
                match parts[1] {
                    "Read" => read_bytes += value,
                    "Write" => write_bytes += value,
                    _ => {}
                }
            }
        }

        Ok((read_bytes, write_bytes))
    }
}

/// Get container metrics via Docker API (alternative method)
pub async fn get_docker_container_stats(container_id: &str) -> io::Result<ContainerMetrics> {
    // Try Docker API first (requires bollard and Docker daemon)
    if let Ok(stats) = get_docker_container_stats_via_api(container_id).await {
        return Ok(stats);
    }

    // Fall back to cgroups if Docker API unavailable
    let (memory_usage, memory_limit) = read_container_memory_usage(container_id)?;
    let cpu_usage_ns = read_container_cpu_usage(container_id)?;
    let (block_read, block_write) = read_container_blkio_stats(container_id)?;

    // Calculate CPU percentage using time delta from previous sample
    let cpu_percent = calculate_cpu_percent(container_id, cpu_usage_ns);

    // Try to get network stats
    let (network_rx, network_tx) = read_container_network_stats(container_id).unwrap_or((0, 0));

    Ok(ContainerMetrics {
        cpu_usage_percent: cpu_percent,
        memory_usage_bytes: memory_usage,
        memory_limit_bytes: memory_limit,
        network_rx_bytes: network_rx,
        network_tx_bytes: network_tx,
        block_read_bytes: block_read,
        block_write_bytes: block_write,
    })
}

/// Read network stats for a container from /proc/net/dev in its network namespace
fn read_container_network_stats(container_id: &str) -> io::Result<(u64, u64)> {
    // Try to get PID of the container's init process
    let pid = get_container_init_pid(container_id)?;

    // Read network stats from container's network namespace
    let net_dev_path = format!("/proc/{}/net/dev", pid);
    let content = fs::read_to_string(&net_dev_path)?;

    let mut rx_bytes = 0u64;
    let mut tx_bytes = 0u64;

    for line in content.lines().skip(2) {
        // Skip header lines
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Parse interface name and stats
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() != 2 {
            continue;
        }

        let interface = parts[0].trim();
        // Skip loopback interface
        if interface == "lo" {
            continue;
        }

        let stats: Vec<&str> = parts[1].split_whitespace().collect();
        if stats.len() >= 10 {
            // Fields: rx_bytes, rx_packets, rx_errs, rx_drop, rx_fifo, rx_frame, rx_compressed, rx_multicast
            //         tx_bytes, tx_packets, tx_errs, tx_drop, tx_fifo, tx_colls, tx_carrier, tx_compressed
            if let Ok(rx) = stats[0].parse::<u64>() {
                rx_bytes += rx;
            }
            if let Ok(tx) = stats[8].parse::<u64>() {
                tx_bytes += tx;
            }
        }
    }

    Ok((rx_bytes, tx_bytes))
}

/// Get the init process PID for a container
fn get_container_init_pid(container_id: &str) -> io::Result<u32> {
    // Docker stores PID in a file
    let docker_pid_file = format!("/var/run/docker/containerd/daemon/io.containerd.runtime.v2.task/moby/{}/init.pid", container_id);
    if let Ok(content) = fs::read_to_string(&docker_pid_file) {
        if let Ok(pid) = content.trim().parse::<u32>() {
            return Ok(pid);
        }
    }

    // Try alternative location for older Docker versions
    let alt_pid_file = format!("/var/run/docker/libcontainerd/{}/init.pid", container_id);
    if let Ok(content) = fs::read_to_string(&alt_pid_file) {
        if let Ok(pid) = content.trim().parse::<u32>() {
            return Ok(pid);
        }
    }

    // Try reading from cgroup
    let cgroup_path = get_container_cgroup_path(container_id, "")?;
    let cgroup_procs = cgroup_path.join("cgroup.procs");
    if let Ok(content) = fs::read_to_string(&cgroup_procs) {
        // First PID in cgroup.procs is typically the init process
        for line in content.lines() {
            if let Ok(pid) = line.trim().parse::<u32>() {
                return Ok(pid);
            }
        }
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("Cannot find PID for container {}", container_id),
    ))
}

/// Get container stats via Docker API using bollard
async fn get_docker_container_stats_via_api(container_id: &str) -> io::Result<ContainerMetrics> {
    use bollard::Docker;
    use bollard::container::StatsOptions;
    use futures::StreamExt;

    // Connect to Docker API
    let docker = Docker::connect_with_local_defaults()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("Docker API unavailable: {}", e)))?;

    let stats_options = StatsOptions {
        stream: false,
        one_shot: true,
    };

    let mut stats_stream = docker.stats(container_id, Some(stats_options));

    if let Some(stats_result) = stats_stream.next().await {
        let stats = stats_result
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("Failed to get stats: {}", e)))?;

        // Parse CPU stats
        let cpu_delta = stats.cpu_stats.cpu_usage.total_usage
            - stats.precpu_stats.cpu_usage.total_usage;
        let system_delta = stats.cpu_stats.system_cpu_usage.unwrap_or(0)
            - stats.precpu_stats.system_cpu_usage.unwrap_or(0);
        let num_cpus = stats.cpu_stats.online_cpus.unwrap_or(1) as f64;

        let cpu_percent = if system_delta > 0 {
            (cpu_delta as f64 / system_delta as f64) * num_cpus * 100.0
        } else {
            0.0
        };

        // Parse memory stats
        let memory_usage = stats.memory_stats.usage.unwrap_or(0);
        let memory_limit = stats.memory_stats.limit.unwrap_or(0);

        // Parse network stats
        let mut network_rx_bytes = 0u64;
        let mut network_tx_bytes = 0u64;

        if let Some(networks) = stats.networks {
            for (_interface, net_stats) in networks {
                network_rx_bytes += net_stats.rx_bytes;
                network_tx_bytes += net_stats.tx_bytes;
            }
        }

        // Parse block I/O stats
        let mut block_read_bytes = 0u64;
        let mut block_write_bytes = 0u64;

        if let Some(blkio_stats) = stats.blkio_stats.io_service_bytes_recursive {
            for entry in blkio_stats {
                match entry.op.as_str() {
                    "Read" => block_read_bytes += entry.value,
                    "Write" => block_write_bytes += entry.value,
                    _ => {}
                }
            }
        }

        return Ok(ContainerMetrics {
            cpu_usage_percent: cpu_percent,
            memory_usage_bytes: memory_usage,
            memory_limit_bytes: memory_limit,
            network_rx_bytes,
            network_tx_bytes,
            block_read_bytes,
            block_write_bytes,
        });
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("No stats available for container {}", container_id),
    ))
}

/// Get all containers from Docker
pub fn list_running_containers() -> io::Result<Vec<String>> {
    // Try Docker API first
    if let Ok(containers) = tokio::runtime::Handle::try_current() {
        if let Ok(result) = containers.block_on(list_containers_via_docker_api()) {
            return Ok(result);
        }
    }

    // Fall back to scanning cgroups
    let mut containers = Vec::new();

    if let Ok(entries) = fs::read_dir("/sys/fs/cgroup/system.slice") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("docker-") && name.ends_with(".scope") {
                // Extract container ID
                let id = name
                    .strip_prefix("docker-")
                    .and_then(|s| s.strip_suffix(".scope"))
                    .map(|s| s.to_string());
                if let Some(id) = id {
                    containers.push(id);
                }
            }
        }
    }

    Ok(containers)
}

/// List containers via Docker API
async fn list_containers_via_docker_api() -> io::Result<Vec<String>> {
    use bollard::Docker;
    use bollard::container::ListContainersOptions;

    let docker = Docker::connect_with_local_defaults()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("Docker API unavailable: {}", e)))?;

    let options = Some(ListContainersOptions::<String> {
        all: false, // Only running containers
        ..Default::default()
    });

    let containers = docker
        .list_containers(options)
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("Failed to list containers: {}", e)))?;

    let ids: Vec<String> = containers
        .into_iter()
        .filter_map(|c| c.id)
        .collect();

    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_cgroups_version() {
        // This test will pass on any Linux system with cgroups
        let result = detect_cgroups_version();
        if result.is_ok() {
            let version = result.unwrap();
            assert!(version == 1 || version == 2);
        }
    }

    #[test]
    fn test_list_running_containers() {
        // This test won't fail even if no containers are running
        let result = list_running_containers();
        assert!(result.is_ok());
    }
}
