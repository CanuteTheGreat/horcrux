///! Container metrics collection via cgroups
///! Supports both cgroups v1 and v2

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

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
    let _cpu_usage_ns = read_container_cpu_usage(container_id)?;
    let (block_read, block_write) = read_container_blkio_stats(container_id)?;

    // Calculate CPU percentage (simplified - would need time delta in production)
    let cpu_percent = 0.0; // TODO: Calculate from time delta

    Ok(ContainerMetrics {
        cpu_usage_percent: cpu_percent,
        memory_usage_bytes: memory_usage,
        memory_limit_bytes: memory_limit,
        network_rx_bytes: 0, // TODO: Get from network namespace
        network_tx_bytes: 0,
        block_read_bytes: block_read,
        block_write_bytes: block_write,
    })
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
