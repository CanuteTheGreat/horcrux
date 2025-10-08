///! Cluster node representation

use serde::{Deserialize, Serialize};

/// CPU architecture
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Architecture {
    #[serde(rename = "x86_64")]
    X86_64,  // amd64
    #[serde(rename = "aarch64")]
    Aarch64, // arm64
    #[serde(rename = "riscv64")]
    Riscv64, // risc-v 64-bit
    #[serde(rename = "ppc64le")]
    Ppc64le, // powerpc 64-bit little-endian
    Unknown,
}

impl Architecture {
    /// Detect the current system architecture
    pub fn detect() -> Self {
        match std::env::consts::ARCH {
            "x86_64" => Architecture::X86_64,
            "aarch64" => Architecture::Aarch64,
            "riscv64" => Architecture::Riscv64,
            "powerpc64" => Architecture::Ppc64le,
            _ => Architecture::Unknown,
        }
    }

    /// Check if this architecture can run VMs of the target architecture
    pub fn can_run(&self, target: &Architecture) -> bool {
        match (self, target) {
            // Same architecture always works
            (a, b) if a == b => true,
            // x86_64 can emulate other architectures via QEMU (slower)
            (Architecture::X86_64, _) => true,
            // aarch64 can emulate other architectures via QEMU (slower)
            (Architecture::Aarch64, _) => true,
            // Other combinations would require QEMU emulation
            _ => false,
        }
    }

    /// Check if this is native (non-emulated) execution
    pub fn is_native(&self, target: &Architecture) -> bool {
        self == target
    }

    /// Get QEMU system binary name for this architecture
    pub fn qemu_system_binary(&self) -> &'static str {
        match self {
            Architecture::X86_64 => "qemu-system-x86_64",
            Architecture::Aarch64 => "qemu-system-aarch64",
            Architecture::Riscv64 => "qemu-system-riscv64",
            Architecture::Ppc64le => "qemu-system-ppc64",
            Architecture::Unknown => "qemu-system-x86_64",
        }
    }
}

/// Cluster node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: u32,
    pub name: String,
    pub ip: String,
    pub status: NodeStatus,
    pub priority: u32,     // For HA failover priority
    pub is_local: bool,    // Is this the local node?
    pub architecture: Architecture, // CPU architecture
    pub cpu_cores: u32,    // Total CPU cores
    pub memory_total: u64, // Total RAM in bytes
}

/// Node status
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeStatus {
    Online,
    Offline,
    Unknown,
}

impl Node {
    pub fn new(id: u32, name: String, ip: String) -> Self {
        Self {
            id,
            name,
            ip,
            status: NodeStatus::Unknown,
            priority: 100,
            is_local: false,
            architecture: Architecture::detect(),
            cpu_cores: num_cpus::get() as u32,
            memory_total: Self::detect_memory(),
        }
    }

    /// Create a local node with detected system information
    pub fn new_local(id: u32, name: String, ip: String) -> Self {
        Self {
            id,
            name,
            ip,
            status: NodeStatus::Online,
            priority: 100,
            is_local: true,
            architecture: Architecture::detect(),
            cpu_cores: num_cpus::get() as u32,
            memory_total: Self::detect_memory(),
        }
    }

    /// Detect total system memory
    fn detect_memory() -> u64 {
        // Read from /proc/meminfo
        if let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") {
            for line in meminfo.lines() {
                if line.starts_with("MemTotal:") {
                    if let Some(kb_str) = line.split_whitespace().nth(1) {
                        if let Ok(kb) = kb_str.parse::<u64>() {
                            return kb * 1024; // Convert KB to bytes
                        }
                    }
                }
            }
        }
        0
    }

    /// Check if node is online
    pub fn is_online(&self) -> bool {
        self.status == NodeStatus::Online
    }

    /// Get node address for API communication
    pub fn api_url(&self) -> String {
        format!("https://{}:8006", self.ip)
    }

    /// Check if this node can run a VM with the target architecture
    pub fn can_run_architecture(&self, target: &Architecture) -> bool {
        self.architecture.can_run(target)
    }

    /// Check if VM would run natively (not emulated) on this node
    pub fn is_native_for(&self, target: &Architecture) -> bool {
        self.architecture.is_native(target)
    }
}
