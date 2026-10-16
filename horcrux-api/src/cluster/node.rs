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
    /// Live CPU utilisation in percent (0-100, averaged over the last
    /// heartbeat interval), as last reported by the node itself via
    /// `GET /api/cluster/local-usage` (see `cluster::usage`). `None` means
    /// genuinely unknown -- the node has never reported, is unreachable, or
    /// answered without usage data -- and must be shown as unknown, never
    /// as 0%: a 0 here would make an unreachable node look like the idlest
    /// migration target in the cluster.
    #[serde(default)]
    pub cpu_usage: Option<f64>,
    /// Live memory in use, in bytes (MemTotal - MemAvailable). `None` means
    /// unknown, same rules as `cpu_usage`.
    #[serde(default)]
    pub memory_used: Option<u64>,
    /// Unix timestamp (seconds) of the last successful usage report from
    /// this node; `None` if it has never reported.
    #[serde(default)]
    pub last_seen: Option<i64>,
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
            cpu_usage: None,
            memory_used: None,
            last_seen: None,
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
            cpu_usage: None,
            memory_used: None,
            last_seen: None,
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

    /// Base URL of this node's Horcrux API. horcrux-api always listens on
    /// plain HTTP (TLS, if any, is terminated in front of it), and every
    /// node in a cluster runs on the same configured port. IPv6 literals
    /// are bracketed.
    pub fn api_url(&self, port: u16) -> String {
        if self.ip.contains(':') && !self.ip.starts_with('[') {
            format!("http://[{}]:{}", self.ip, port)
        } else {
            format!("http://{}:{}", self.ip, port)
        }
    }

    /// Fraction (0.0-1.0) of this node's RAM in use, when both the live
    /// figure and the total are known.
    pub fn memory_used_fraction(&self) -> Option<f64> {
        match self.memory_used {
            Some(used) if self.memory_total > 0 => Some(used as f64 / self.memory_total as f64),
            _ => None,
        }
    }

    /// RAM currently free for new VMs: total minus the live in-use figure
    /// when known, otherwise the total (the pre-usage-reporting behaviour).
    pub fn memory_available(&self) -> u64 {
        self.memory_total
            .saturating_sub(self.memory_used.unwrap_or(0))
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

fn default_node_priority() -> u32 {
    100
}

/// Body of `POST /api/cluster/nodes/:name`.
///
/// Every client of this endpoint (the web UI's "Add Node" form, the CLI's
/// `cluster add`, the Python example client) sends only what an operator
/// actually knows when joining a node -- its address and, optionally, its
/// architecture -- while the handler used to demand a fully-populated
/// [`Node`] (id, status, is_local, cpu_cores, memory_total, ...), so every
/// one of those clients got a 422. Only `ip` (or its `address` alias) is
/// required; the rest default to "not yet known" values and a full `Node`
/// body is still accepted unchanged.
#[derive(Debug, Clone, Deserialize)]
pub struct AddNodeRequest {
    /// Node id; assigned by the server when omitted.
    #[serde(default)]
    pub id: Option<u32>,
    #[serde(alias = "address")]
    pub ip: String,
    /// Unknown until the node reports in, unless the operator says so.
    #[serde(default)]
    pub architecture: Option<Architecture>,
    #[serde(default)]
    pub status: Option<NodeStatus>,
    #[serde(default = "default_node_priority")]
    pub priority: u32,
    #[serde(default)]
    pub is_local: bool,
    #[serde(default)]
    pub cpu_cores: u32,
    /// Total RAM in bytes.
    #[serde(default)]
    pub memory_total: u64,
}

impl AddNodeRequest {
    /// Build the [`Node`] to register. `name` comes from the URL path;
    /// `next_id` is used when the request didn't pin an id.
    pub fn into_node(self, name: String, next_id: u32) -> Node {
        Node {
            id: self.id.unwrap_or(next_id),
            name,
            ip: self.ip,
            status: self.status.unwrap_or(NodeStatus::Unknown),
            priority: self.priority,
            is_local: self.is_local,
            architecture: self.architecture.unwrap_or(Architecture::Unknown),
            cpu_cores: self.cpu_cores,
            memory_total: self.memory_total,
            // Live usage is never accepted from the client: it is only ever
            // filled in by the usage monitor from the node's own report.
            cpu_usage: None,
            memory_used: None,
            last_seen: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_node_request_accepts_minimal_ui_and_cli_body() {
        // What the web UI's Add Node form and `horcrux cluster add` send.
        let req: AddNodeRequest = serde_json::from_str(
            r#"{"name":"node-02","address":"192.168.1.100","ssh_key":null,"architecture":"aarch64"}"#,
        )
        .expect("minimal add-node body must deserialize");
        let node = req.into_node("node-02".into(), 7);
        assert_eq!(node.id, 7);
        assert_eq!(node.name, "node-02");
        assert_eq!(node.ip, "192.168.1.100");
        assert_eq!(node.architecture, Architecture::Aarch64);
        assert_eq!(node.status, NodeStatus::Unknown);
        assert_eq!(node.priority, 100);
        assert!(!node.is_local);

        let req: AddNodeRequest =
            serde_json::from_str(r#"{"name":"n3","address":"10.0.0.3"}"#).unwrap();
        assert_eq!(
            req.into_node("n3".into(), 1).architecture,
            Architecture::Unknown
        );
    }

    #[test]
    fn add_node_request_still_accepts_full_node_body() {
        let req: AddNodeRequest = serde_json::from_str(
            r#"{"id":1,"name":"n","ip":"192.168.1.100","status":"online","priority":300,
                "is_local":false,"architecture":"x86_64","cpu_cores":4,"memory_total":8589934592}"#,
        )
        .unwrap();
        let node = req.into_node("n".into(), 99);
        assert_eq!(node.id, 1);
        assert_eq!(node.status, NodeStatus::Online);
        assert_eq!(node.priority, 300);
        assert_eq!(node.cpu_cores, 4);
        assert_eq!(node.memory_total, 8_589_934_592);
        assert_eq!(node.architecture, Architecture::X86_64);
    }

    #[test]
    fn node_serializes_unknown_usage_as_null_not_zero() {
        let req: AddNodeRequest =
            serde_json::from_str(r#"{"address":"10.0.0.2","cpu_cores":4,"memory_total":1024}"#)
                .unwrap();
        let node = req.into_node("n2".into(), 2);
        let json = serde_json::to_value(&node).unwrap();
        assert!(json["cpu_usage"].is_null());
        assert!(json["memory_used"].is_null());
        assert!(json["last_seen"].is_null());
        assert_eq!(node.memory_used_fraction(), None);
        assert_eq!(node.memory_available(), 1024);
    }

    #[test]
    fn node_serializes_live_usage() {
        let req: AddNodeRequest =
            serde_json::from_str(r#"{"address":"10.0.0.2","cpu_cores":4,"memory_total":1000}"#)
                .unwrap();
        let mut node = req.into_node("n2".into(), 2);
        node.cpu_usage = Some(42.5);
        node.memory_used = Some(250);
        node.last_seen = Some(1_700_000_000);
        let json = serde_json::to_value(&node).unwrap();
        assert_eq!(json["cpu_usage"], serde_json::json!(42.5));
        assert_eq!(json["memory_used"], serde_json::json!(250));
        assert_eq!(json["last_seen"], serde_json::json!(1_700_000_000));
        assert_eq!(node.memory_used_fraction(), Some(0.25));
        assert_eq!(node.memory_available(), 750);
    }

    #[test]
    fn node_without_usage_fields_still_deserializes() {
        // Shape sent by servers that predate live usage reporting.
        let node: Node = serde_json::from_str(
            r#"{"id":1,"name":"n","ip":"10.0.0.1","status":"online","priority":100,
                "is_local":false,"architecture":"x86_64","cpu_cores":4,"memory_total":1024}"#,
        )
        .unwrap();
        assert_eq!(node.cpu_usage, None);
        assert_eq!(node.memory_used, None);
        assert_eq!(node.last_seen, None);
    }

    #[test]
    fn add_node_request_ignores_client_supplied_usage() {
        let req: AddNodeRequest = serde_json::from_str(
            r#"{"address":"10.0.0.3","cpu_usage":0.0,"memory_used":0,"last_seen":1}"#,
        )
        .unwrap();
        let node = req.into_node("n3".into(), 3);
        assert_eq!(node.cpu_usage, None);
        assert_eq!(node.memory_used, None);
        assert_eq!(node.last_seen, None);
    }

    #[test]
    fn api_url_is_plain_http_on_configured_port() {
        let mut node = Node::new(1, "n".into(), "10.0.0.1".into());
        assert_eq!(node.api_url(8006), "http://10.0.0.1:8006");
        node.ip = "fd00::1".into();
        assert_eq!(node.api_url(9000), "http://[fd00::1]:9000");
    }
}
