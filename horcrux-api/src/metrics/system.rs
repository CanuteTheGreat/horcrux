///! Real system metrics collection
///! Reads from /proc filesystem for accurate node metrics

use std::fs;
use std::io;

/// CPU statistics from /proc/stat
#[derive(Debug, Clone)]
pub struct CpuStats {
    pub user: u64,
    pub nice: u64,
    pub system: u64,
    pub idle: u64,
    pub iowait: u64,
    pub irq: u64,
    pub softirq: u64,
    pub steal: u64,
}

impl CpuStats {
    /// Calculate total CPU time
    pub fn total(&self) -> u64 {
        self.user + self.nice + self.system + self.idle + self.iowait + self.irq + self.softirq + self.steal
    }

    /// Calculate idle time
    pub fn idle_time(&self) -> u64 {
        self.idle + self.iowait
    }

    /// Calculate usage percentage between two samples
    pub fn usage_percent(&self, prev: &CpuStats) -> f64 {
        let total_delta = self.total().saturating_sub(prev.total());
        let idle_delta = self.idle_time().saturating_sub(prev.idle_time());

        if total_delta == 0 {
            return 0.0;
        }

        let used_delta = total_delta - idle_delta;
        (used_delta as f64 / total_delta as f64) * 100.0
    }
}

/// Read CPU stats from /proc/stat
pub fn read_cpu_stats() -> io::Result<CpuStats> {
    let content = fs::read_to_string("/proc/stat")?;

    for line in content.lines() {
        if line.starts_with("cpu ") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 9 {
                return Ok(CpuStats {
                    user: parts[1].parse().unwrap_or(0),
                    nice: parts[2].parse().unwrap_or(0),
                    system: parts[3].parse().unwrap_or(0),
                    idle: parts[4].parse().unwrap_or(0),
                    iowait: parts[5].parse().unwrap_or(0),
                    irq: parts[6].parse().unwrap_or(0),
                    softirq: parts[7].parse().unwrap_or(0),
                    steal: parts[8].parse().unwrap_or(0),
                });
            }
        }
    }

    Err(io::Error::new(io::ErrorKind::NotFound, "CPU stats not found"))
}

/// Memory statistics from /proc/meminfo
#[derive(Debug, Clone)]
pub struct MemoryStats {
    pub total: u64,
    pub free: u64,
    pub available: u64,
    pub buffers: u64,
    pub cached: u64,
}

impl MemoryStats {
    /// Calculate used memory
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.free + self.buffers + self.cached)
    }

    /// Calculate usage percentage
    pub fn usage_percent(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        (self.used() as f64 / self.total as f64) * 100.0
    }
}

/// Read memory stats from /proc/meminfo
pub fn read_memory_stats() -> io::Result<MemoryStats> {
    let content = fs::read_to_string("/proc/meminfo")?;

    let mut total = 0;
    let mut free = 0;
    let mut available = 0;
    let mut buffers = 0;
    let mut cached = 0;

    for line in content.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            let value: u64 = parts[1].parse().unwrap_or(0) * 1024; // Convert KB to bytes

            match parts[0] {
                "MemTotal:" => total = value,
                "MemFree:" => free = value,
                "MemAvailable:" => available = value,
                "Buffers:" => buffers = value,
                "Cached:" => cached = value,
                _ => {}
            }
        }
    }

    Ok(MemoryStats {
        total,
        free,
        available,
        buffers,
        cached,
    })
}

/// Load average from /proc/loadavg
#[derive(Debug, Clone)]
pub struct LoadAverage {
    pub one_min: f64,
    pub five_min: f64,
    pub fifteen_min: f64,
}

/// Read load average from /proc/loadavg
pub fn read_load_average() -> io::Result<LoadAverage> {
    let content = fs::read_to_string("/proc/loadavg")?;
    let parts: Vec<&str> = content.split_whitespace().collect();

    if parts.len() >= 3 {
        Ok(LoadAverage {
            one_min: parts[0].parse().unwrap_or(0.0),
            five_min: parts[1].parse().unwrap_or(0.0),
            fifteen_min: parts[2].parse().unwrap_or(0.0),
        })
    } else {
        Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid loadavg format"))
    }
}

/// Get number of CPU cores
pub fn get_cpu_count() -> usize {
    num_cpus::get()
}

/// Uptime from /proc/uptime
pub fn read_uptime() -> io::Result<u64> {
    let content = fs::read_to_string("/proc/uptime")?;
    let parts: Vec<&str> = content.split_whitespace().collect();

    if !parts.is_empty() {
        let uptime: f64 = parts[0].parse().unwrap_or(0.0);
        Ok(uptime as u64)
    } else {
        Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid uptime format"))
    }
}

/// Disk statistics from /proc/diskstats
#[derive(Debug, Clone)]
pub struct DiskStats {
    pub read_bytes: u64,
    pub write_bytes: u64,
}

/// Read disk stats for a device from /proc/diskstats
pub fn read_disk_stats(device: &str) -> io::Result<DiskStats> {
    let content = fs::read_to_string("/proc/diskstats")?;

    for line in content.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 14 && parts[2] == device {
            // Sectors read/written (multiply by 512 to get bytes)
            let read_sectors: u64 = parts[5].parse().unwrap_or(0);
            let write_sectors: u64 = parts[9].parse().unwrap_or(0);

            return Ok(DiskStats {
                read_bytes: read_sectors * 512,
                write_bytes: write_sectors * 512,
            });
        }
    }

    Err(io::Error::new(io::ErrorKind::NotFound, format!("Device {} not found", device)))
}

/// Network statistics from /proc/net/dev
#[derive(Debug, Clone)]
pub struct NetworkStats {
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

/// Read network stats for an interface from /proc/net/dev
pub fn read_network_stats(interface: &str) -> io::Result<NetworkStats> {
    let content = fs::read_to_string("/proc/net/dev")?;

    for line in content.lines() {
        if line.contains(interface) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 10 {
                return Ok(NetworkStats {
                    rx_bytes: parts[1].parse().unwrap_or(0),
                    tx_bytes: parts[9].parse().unwrap_or(0),
                });
            }
        }
    }

    Err(io::Error::new(io::ErrorKind::NotFound, format!("Interface {} not found", interface)))
}

/// Process statistics from /proc/[pid]/stat
#[derive(Debug, Clone)]
pub struct ProcessStats {
    pub pid: u32,
    pub utime: u64,  // User mode time
    pub stime: u64,  // Kernel mode time
    pub rss: u64,    // Resident Set Size (pages)
}

/// Read process stats from /proc/[pid]/stat
pub fn read_process_stats(pid: u32) -> io::Result<ProcessStats> {
    let path = format!("/proc/{}/stat", pid);
    let content = fs::read_to_string(&path)?;

    // Parse stat file (format is complex due to process name with spaces)
    let parts: Vec<&str> = content.split_whitespace().collect();

    if parts.len() >= 24 {
        Ok(ProcessStats {
            pid,
            utime: parts[13].parse().unwrap_or(0),
            stime: parts[14].parse().unwrap_or(0),
            rss: parts[23].parse().unwrap_or(0),
        })
    } else {
        Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid stat format"))
    }
}

/// Process I/O stats from /proc/[pid]/io
#[derive(Debug, Clone)]
pub struct ProcessIoStats {
    pub read_bytes: u64,
    pub write_bytes: u64,
}

/// Read process I/O stats from /proc/[pid]/io
pub fn read_process_io_stats(pid: u32) -> io::Result<ProcessIoStats> {
    let path = format!("/proc/{}/io", pid);
    let content = fs::read_to_string(&path)?;

    let mut read_bytes = 0;
    let mut write_bytes = 0;

    for line in content.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            let value: u64 = parts[1].parse().unwrap_or(0);
            match parts[0] {
                "read_bytes:" => read_bytes = value,
                "write_bytes:" => write_bytes = value,
                _ => {}
            }
        }
    }

    Ok(ProcessIoStats {
        read_bytes,
        write_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_cpu_stats() {
        let stats = read_cpu_stats();
        assert!(stats.is_ok());
        let stats = stats.unwrap();
        assert!(stats.total() > 0);
    }

    #[test]
    fn test_read_memory_stats() {
        let stats = read_memory_stats();
        assert!(stats.is_ok());
        let stats = stats.unwrap();
        assert!(stats.total > 0);
        assert!(stats.usage_percent() >= 0.0);
        assert!(stats.usage_percent() <= 100.0);
    }

    #[test]
    fn test_read_load_average() {
        let load = read_load_average();
        assert!(load.is_ok());
        let load = load.unwrap();
        assert!(load.one_min >= 0.0);
    }

    #[test]
    fn test_get_cpu_count() {
        let count = get_cpu_count();
        assert!(count > 0);
    }

    #[test]
    fn test_read_uptime() {
        let uptime = read_uptime();
        assert!(uptime.is_ok());
        assert!(uptime.unwrap() > 0);
    }

    #[test]
    fn test_cpu_usage_calculation() {
        let prev = CpuStats {
            user: 1000,
            nice: 0,
            system: 500,
            idle: 5000,
            iowait: 100,
            irq: 0,
            softirq: 0,
            steal: 0,
        };

        let curr = CpuStats {
            user: 1500,
            nice: 0,
            system: 700,
            idle: 5500,
            iowait: 150,
            irq: 0,
            softirq: 0,
            steal: 0,
        };

        let usage = curr.usage_percent(&prev);
        assert!(usage > 0.0);
        assert!(usage <= 100.0);
    }
}
