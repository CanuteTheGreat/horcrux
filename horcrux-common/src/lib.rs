//! Common types and utilities shared between horcrux-api and horcrux-ui

pub mod auth;

use serde::{Deserialize, Serialize};

/// Virtual machine hypervisor type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VmHypervisor {
    Qemu,      // QEMU/KVM
    Lxd,       // LXD (can do VMs)
    Incus,     // Incus (LXD fork)
}

/// CPU architecture for VMs
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VmArchitecture {
    #[serde(rename = "x86_64")]
    X86_64,      // amd64
    #[serde(rename = "aarch64")]
    Aarch64,     // arm64
    #[serde(rename = "riscv64")]
    Riscv64,     // risc-v 64-bit
    #[serde(rename = "ppc64le")]
    Ppc64le,     // powerpc 64-bit little-endian
}

impl Default for VmArchitecture {
    fn default() -> Self {
        // Default to host architecture
        match std::env::consts::ARCH {
            "x86_64" => VmArchitecture::X86_64,
            "aarch64" => VmArchitecture::Aarch64,
            "riscv64" => VmArchitecture::Riscv64,
            "powerpc64" => VmArchitecture::Ppc64le,
            _ => VmArchitecture::X86_64,
        }
    }
}

/// Virtual machine disk configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmDisk {
    pub path: String,
    pub size_gb: u64,
    pub disk_type: String, // "virtio", "scsi", "ide", "sata"
    pub cache: String,     // "none", "writethrough", "writeback"
}

/// Virtual machine configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmConfig {
    pub id: String,
    pub name: String,
    pub hypervisor: VmHypervisor,
    pub memory: u64,      // Memory in MB
    pub cpus: u32,
    pub disk_size: u64,   // Disk size in GB (legacy, kept for compatibility)
    pub status: VmStatus,
    #[serde(default)]
    pub architecture: VmArchitecture, // CPU architecture
    #[serde(default)]
    pub disks: Vec<VmDisk>, // Disk configurations
}

/// Virtual machine status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VmStatus {
    Running,
    Stopped,
    Paused,
    Unknown,
}

/// Container runtime type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ContainerRuntime {
    Lxc,       // LXC
    Lxd,       // LXD (can do containers)
    Incus,     // Incus (LXD fork, can do containers)
    Docker,    // Docker
    Podman,    // Podman (daemonless Docker alternative)
}

/// Container configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerConfig {
    pub id: String,
    pub name: String,
    pub runtime: ContainerRuntime,
    pub memory: u64,      // Memory in MB
    pub cpus: u32,
    pub rootfs: String,   // Path to rootfs or image name for Docker
    pub status: ContainerStatus,
}

/// Container status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ContainerStatus {
    Running,
    Stopped,
    Paused,
    #[default]
    Unknown,
}

impl std::fmt::Display for ContainerStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Running => write!(f, "running"),
            Self::Stopped => write!(f, "stopped"),
            Self::Paused => write!(f, "paused"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// API error types
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Virtual machine not found: {0}")]
    VmNotFound(String),

    #[error("Container not found: {0}")]
    ContainerNotFound(String),

    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("System error: {0}")]
    System(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Authentication failed")]
    AuthenticationFailed,

    #[error("Invalid session")]
    InvalidSession,
}

pub type Result<T> = std::result::Result<T, Error>;

/// Cluster status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterStatus {
    pub name: String,
    pub quorum: bool,
    pub nodes: Vec<ClusterNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterNode {
    pub id: String,
    pub hostname: String,
    pub ip_address: String,
    pub status: NodeStatus,
    pub online: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NodeStatus {
    Online,
    Offline,
    Unknown,
}

/// Storage types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoragePool {
    pub id: String,
    pub name: String,
    pub pool_type: StorageType,
    pub path: String,
    pub size_gb: u64,
    pub used_gb: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StorageType {
    Lvm,
    Zfs,
    Directory,
    Nfs,
    Ceph,
}

/// Backup types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupInfo {
    pub id: String,
    pub vm_id: String,
    pub backup_type: BackupType,
    pub size_bytes: u64,
    pub created_at: i64,
    pub compression: CompressionType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BackupType {
    Full,
    Incremental,
    Differential,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompressionType {
    None,
    Gzip,
    Zstd,
    Lz4,
}

/// Monitoring types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeMetrics {
    pub cpu_usage: f64,
    pub memory_total: u64,
    pub memory_used: u64,
    pub disk_total: u64,
    pub disk_used: u64,
    pub network_rx_bytes: u64,
    pub network_tx_bytes: u64,
    pub load_average: f64,
    pub uptime_seconds: u64,
}

/// Alert types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertRule {
    pub name: String,
    pub metric: String,
    pub condition: AlertCondition,
    pub threshold: f64,
    pub severity: AlertSeverity,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AlertCondition {
    GreaterThan,
    LessThan,
    Equals,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AlertSeverity {
    Critical,
    Warning,
    Info,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub id: String,
    pub rule_name: String,
    pub target: String,
    pub severity: String,
    pub message: String,
    pub status: String,
    pub fired_at: i64,
}

/// Firewall types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirewallRule {
    pub name: String,
    pub action: FirewallAction,
    pub protocol: FirewallProtocol,
    pub source: String,
    pub destination: String,
    pub port: Option<u16>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FirewallAction {
    Accept,
    Drop,
    Reject,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FirewallProtocol {
    Tcp,
    Udp,
    Icmp,
    Any,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vm_config_serialization() {
        let vm = VmConfig {
            id: "test-vm".to_string(),
            name: "Test VM".to_string(),
            hypervisor: VmHypervisor::Qemu,
            architecture: VmArchitecture::X86_64,
            cpus: 2,
            memory: 2048,
            disk_size: 20,
            status: VmStatus::Running,
        };

        let json = serde_json::to_string(&vm).unwrap();
        let deserialized: VmConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, vm.id);
        assert_eq!(deserialized.name, vm.name);
    }

    #[test]
    fn test_vm_status_transitions() {
        let statuses = vec![
            VmStatus::Stopped,
            VmStatus::Running,
            VmStatus::Paused,
            VmStatus::Unknown,
        ];

        for status in statuses {
            let json = serde_json::to_string(&status).unwrap();
            let _: VmStatus = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn test_node_metrics_validation() {
        let metrics = NodeMetrics {
            cpu_usage: 45.5,
            memory_total: 16384,
            memory_used: 8192,
            disk_total: 500,
            disk_used: 250,
            network_rx_bytes: 1000000,
            network_tx_bytes: 500000,
            load_average: 2.5,
            uptime_seconds: 86400,
        };

        assert!(metrics.cpu_usage >= 0.0 && metrics.cpu_usage <= 100.0);
        assert!(metrics.memory_used <= metrics.memory_total);
        assert!(metrics.disk_used <= metrics.disk_total);
    }

    #[test]
    fn test_alert_rule_creation() {
        let rule = AlertRule {
            name: "high_cpu".to_string(),
            metric: "cpu_usage".to_string(),
            condition: AlertCondition::GreaterThan,
            threshold: 80.0,
            severity: AlertSeverity::Warning,
            enabled: true,
        };

        assert_eq!(rule.name, "high_cpu");
        assert_eq!(rule.threshold, 80.0);
        assert!(rule.enabled);
    }

    #[test]
    fn test_firewall_rule_validation() {
        let rule = FirewallRule {
            name: "allow_http".to_string(),
            action: FirewallAction::Accept,
            protocol: FirewallProtocol::Tcp,
            source: "0.0.0.0/0".to_string(),
            destination: "0.0.0.0/0".to_string(),
            port: Some(80),
            enabled: true,
        };

        assert_eq!(rule.port, Some(80));
        assert!(matches!(rule.action, FirewallAction::Accept));
        assert!(matches!(rule.protocol, FirewallProtocol::Tcp));
    }

    #[test]
    fn test_storage_pool_capacity() {
        let pool = StoragePool {
            id: "pool1".to_string(),
            name: "Main Pool".to_string(),
            pool_type: StorageType::Lvm,
            path: "/dev/vg0/pool1".to_string(),
            size_gb: 100,
            used_gb: 45,
        };

        let available = pool.size_gb - pool.used_gb;
        assert_eq!(available, 55);
        assert!(pool.used_gb <= pool.size_gb);
    }
}
