//! Cluster node representation
use serde::{Deserialize, Serialize};

/// CPU architecture
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum Architecture {
    #[serde(rename = "x86_64")]
    #[default]
    X86_64, // amd64
    #[serde(rename = "aarch64")]
    Aarch64, // arm64
    #[serde(rename = "riscv64")]
    Riscv64, // risc-v 64-bit
    #[serde(rename = "ppc64le")]
    Ppc64le, // powerpc 64-bit little-endian
    Unknown,
}

impl From<&horcrux_common::VmArchitecture> for Architecture {
    fn from(vm_arch: &horcrux_common::VmArchitecture) -> Self {
        match vm_arch {
            horcrux_common::VmArchitecture::X86_64 => Architecture::X86_64,
            horcrux_common::VmArchitecture::Aarch64 => Architecture::Aarch64,
            horcrux_common::VmArchitecture::Riscv64 => Architecture::Riscv64,
            horcrux_common::VmArchitecture::Ppc64le => Architecture::Ppc64le,
        }
    }
}

impl From<horcrux_common::VmArchitecture> for Architecture {
    fn from(vm_arch: horcrux_common::VmArchitecture) -> Self {
        Architecture::from(&vm_arch)
    }
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
            // riscv64 guests have no QEMU emulation path modeled here: they
            // must land on a native riscv64 host, never on x86_64/aarch64.
            (_, Architecture::Riscv64) => false,
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

/// Identifies a passthrough-capable PCI device (GPU, etc.) by vendor:device
/// ID rather than by PCI address. PCI addresses (e.g. `0000:01:00.0`) are
/// specific to the physical slot layout of a single host and are near-certain
/// to differ on the failover target -- vendor_id:device_id identifies the
/// same *model* of hardware regardless of which physical node it is plugged
/// into or which slot it occupies there, so it is the right key for matching
/// "does this other node have an equivalent device" across a cluster.
///
/// `device_name` is an optional human-readable label (e.g. "NVIDIA RTX A6000")
/// carried for diagnostics/UI only -- it is NOT used for matching since the
/// same vendor:device pair is authoritative and names can vary slightly
/// between lspci outputs/driver versions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PassthroughDeviceRequirement {
    pub vendor_id: String,
    pub device_id: String,
    #[serde(default)]
    pub device_name: String,
}

impl PassthroughDeviceRequirement {
    /// Compare only the vendor:device identity (case-insensitive -- lspci ID
    /// casing is not guaranteed consistent across tools/hosts), ignoring the
    /// cosmetic device_name label.
    pub fn same_model(&self, other: &PassthroughDeviceRequirement) -> bool {
        self.vendor_id.eq_ignore_ascii_case(&other.vendor_id)
            && self.device_id.eq_ignore_ascii_case(&other.device_id)
    }
}

/// Check whether `available` (a list of currently-unused passthrough device
/// units on some node, one entry per physical unit) contains, as a multiset,
/// at least one available unit for every entry in `required`. This correctly
/// handles a VM requiring two devices of the *same* model (needs two distinct
/// available units, not one unit satisfying both requirements) as well as
/// nodes that have the right model but every unit already in_use (callers
/// should not include in_use devices in `available`).
pub fn passthrough_devices_satisfied(
    available: &[PassthroughDeviceRequirement],
    required: &[PassthroughDeviceRequirement],
) -> bool {
    if required.is_empty() {
        return true;
    }

    let mut remaining: Vec<&PassthroughDeviceRequirement> = available.iter().collect();

    for req in required {
        match remaining.iter().position(|dev| dev.same_model(req)) {
            Some(idx) => {
                remaining.remove(idx);
            }
            None => return false,
        }
    }

    true
}

/// Cluster node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: u32,
    pub name: String,
    pub ip: String,
    pub status: NodeStatus,
    /// Placement/HA weight (0-1000, default 100). Used two ways:
    ///   1. HA failover ordering for a given resource (higher first).
    ///   2. General VM placement preference in `ClusterManager::find_best_node`
    ///      and `ClusterBalancer` -- in a mixed-age cluster, give older/
    ///      slower nodes a lower weight so they're only chosen when nothing
    ///      better-weighted has room, and newer/faster nodes a higher weight
    ///      so they're preferred all else being equal. A VM/HA group can
    ///      also require a minimum weight (`min_priority`) to pin it to only
    ///      the higher (or, with a low cutoff, effectively any) tier.
    pub priority: u32,
    pub is_local: bool,             // Is this the local node?
    pub architecture: Architecture, // CPU architecture
    pub cpu_cores: u32,             // Total CPU cores
    pub memory_total: u64,          // Total RAM in bytes
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

    /// Set this node's placement/HA weight (builder style). Not bounds-
    /// checked here -- callers that expose this over the API/CLI should
    /// validate the 0-1000 convention themselves and return a proper error
    /// rather than silently clamping.
    pub fn with_priority(mut self, priority: u32) -> Self {
        self.priority = priority;
        self
    }
}
