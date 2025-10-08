//! Multi-Architecture Support Module
//!
//! Provides comprehensive CPU architecture detection, validation, and
//! compatibility checking for mixed-architecture clusters.
//!
//! Horcrux Unique Feature: Support for x86_64, aarch64, riscv64, ppc64le,
//! and dynamic registration of additional architectures.

use horcrux_common::VmArchitecture;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::process::Command;

/// Architecture manager for multi-arch cluster support
pub struct ArchitectureManager {
    /// Registered architectures with their capabilities
    architectures: HashMap<String, ArchitectureInfo>,
    /// Node architecture mappings
    node_architectures: HashMap<String, String>,
    /// Emulation support matrix
    emulation_matrix: HashMap<(String, String), EmulationSupport>,
}

impl ArchitectureManager {
    pub fn new() -> Self {
        let mut mgr = ArchitectureManager {
            architectures: HashMap::new(),
            node_architectures: HashMap::new(),
            emulation_matrix: HashMap::new(),
        };

        // Register built-in architectures
        mgr.register_builtin_architectures();
        mgr.build_emulation_matrix();

        mgr
    }

    /// Detect the current system architecture
    pub fn detect_host_architecture() -> Result<String, String> {
        let output = Command::new("uname")
            .arg("-m")
            .output()
            .map_err(|e| format!("Failed to detect architecture: {}", e))?;

        if !output.status.success() {
            return Err("Failed to detect architecture".to_string());
        }

        let arch = String::from_utf8_lossy(&output.stdout)
            .trim()
            .to_string();

        // Normalize architecture names
        Ok(Self::normalize_arch_name(&arch))
    }

    /// Register a node with its architecture
    pub fn register_node(&mut self, node_id: String, arch: String) -> Result<(), String> {
        if !self.architectures.contains_key(&arch) {
            return Err(format!("Unknown architecture: {}", arch));
        }

        self.node_architectures.insert(node_id, arch);
        Ok(())
    }

    /// Check if a VM can run on a node (native or emulated)
    pub fn can_run_on_node(
        &self,
        vm_arch: &str,
        node_id: &str,
    ) -> Result<PlacementCompatibility, String> {
        let node_arch = self.node_architectures.get(node_id)
            .ok_or_else(|| format!("Node {} not registered", node_id))?;

        // Check if architectures match (native)
        if vm_arch == node_arch {
            return Ok(PlacementCompatibility {
                compatible: true,
                native: true,
                emulation_required: false,
                performance_penalty: 0.0,
                emulation_type: None,
            });
        }

        // Check if emulation is available
        if let Some(emulation) = self.emulation_matrix.get(&(node_arch.clone(), vm_arch.to_string())) {
            return Ok(PlacementCompatibility {
                compatible: true,
                native: false,
                emulation_required: true,
                performance_penalty: emulation.performance_overhead,
                emulation_type: Some(emulation.emulation_type.clone()),
            });
        }

        Ok(PlacementCompatibility {
            compatible: false,
            native: false,
            emulation_required: true,
            performance_penalty: 100.0,
            emulation_type: None,
        })
    }

    /// Find best node for a VM based on architecture
    pub fn suggest_placement(
        &self,
        vm_arch: &str,
        available_nodes: &[String],
    ) -> Result<NodePlacement, String> {
        let mut native_nodes = Vec::new();
        let mut emulated_nodes = Vec::new();

        for node_id in available_nodes {
            match self.can_run_on_node(vm_arch, node_id)? {
                PlacementCompatibility { native: true, .. } => {
                    native_nodes.push(node_id.clone());
                }
                PlacementCompatibility { compatible: true, native: false, .. } => {
                    emulated_nodes.push(node_id.clone());
                }
                _ => {}
            }
        }

        if !native_nodes.is_empty() {
            return Ok(NodePlacement {
                node_id: native_nodes[0].clone(),
                native_execution: true,
                emulation_type: None,
                alternative_nodes: native_nodes[1..].to_vec(),
            });
        }

        if !emulated_nodes.is_empty() {
            let node_id = &emulated_nodes[0];
            let compat = self.can_run_on_node(vm_arch, node_id)?;

            return Ok(NodePlacement {
                node_id: node_id.clone(),
                native_execution: false,
                emulation_type: compat.emulation_type,
                alternative_nodes: emulated_nodes[1..].to_vec(),
            });
        }

        Err(format!("No compatible nodes found for architecture {}", vm_arch))
    }

    /// Validate VM migration compatibility
    pub fn validate_migration(
        &self,
        vm_arch: &str,
        source_node: &str,
        target_node: &str,
    ) -> Result<MigrationCompatibility, String> {
        let source_arch = self.node_architectures.get(source_node)
            .ok_or_else(|| format!("Source node {} not registered", source_node))?;

        let target_arch = self.node_architectures.get(target_node)
            .ok_or_else(|| format!("Target node {} not registered", target_node))?;

        let source_compat = self.can_run_on_node(vm_arch, source_node)?;
        let target_compat = self.can_run_on_node(vm_arch, target_node)?;

        if !target_compat.compatible {
            return Ok(MigrationCompatibility {
                compatible: false,
                reason: Some(format!(
                    "Target node does not support {} architecture",
                    vm_arch
                )),
                requires_shutdown: false,
                performance_change: 0.0,
            });
        }

        // Live migration only supported for same-arch nodes
        let requires_shutdown = source_arch != target_arch;

        let performance_change = if source_compat.native && !target_compat.native {
            -target_compat.performance_penalty
        } else if !source_compat.native && target_compat.native {
            source_compat.performance_penalty
        } else {
            0.0
        };

        Ok(MigrationCompatibility {
            compatible: true,
            reason: None,
            requires_shutdown,
            performance_change,
        })
    }

    /// Register a custom architecture dynamically
    pub fn register_architecture(&mut self, arch: ArchitectureInfo) -> Result<(), String> {
        if self.architectures.contains_key(&arch.name) {
            return Err(format!("Architecture {} already registered", arch.name));
        }

        self.architectures.insert(arch.name.clone(), arch);
        Ok(())
    }

    /// Get supported architectures
    pub fn list_architectures(&self) -> Vec<&ArchitectureInfo> {
        self.architectures.values().collect()
    }

    /// Get architecture info
    pub fn get_architecture(&self, name: &str) -> Option<&ArchitectureInfo> {
        self.architectures.get(name)
    }

    /// Get cluster architecture distribution
    pub fn get_cluster_stats(&self) -> ClusterArchStats {
        let mut arch_counts: HashMap<String, usize> = HashMap::new();

        for arch in self.node_architectures.values() {
            *arch_counts.entry(arch.clone()).or_insert(0) += 1;
        }

        let total_nodes = self.node_architectures.len();
        let unique_architectures = arch_counts.len();

        ClusterArchStats {
            total_nodes,
            unique_architectures,
            architecture_distribution: arch_counts,
            mixed_arch_cluster: unique_architectures > 1,
        }
    }

    // Helper functions

    fn register_builtin_architectures(&mut self) {
        // x86_64 / AMD64
        self.architectures.insert(
            "x86_64".to_string(),
            ArchitectureInfo {
                name: "x86_64".to_string(),
                description: "AMD64/Intel 64-bit (x86_64)".to_string(),
                aliases: vec!["amd64".to_string(), "x64".to_string()],
                word_size: 64,
                endianness: Endianness::Little,
                can_emulate: vec!["x86".to_string(), "i386".to_string()],
                features: vec![
                    "sse".to_string(),
                    "sse2".to_string(),
                    "avx".to_string(),
                    "kvm".to_string(),
                ],
            },
        );

        // aarch64 / ARM64
        self.architectures.insert(
            "aarch64".to_string(),
            ArchitectureInfo {
                name: "aarch64".to_string(),
                description: "ARM 64-bit (ARMv8)".to_string(),
                aliases: vec!["arm64".to_string(), "armv8".to_string()],
                word_size: 64,
                endianness: Endianness::Little,
                can_emulate: vec!["arm".to_string(), "armv7".to_string()],
                features: vec![
                    "neon".to_string(),
                    "crypto".to_string(),
                    "kvm".to_string(),
                ],
            },
        );

        // riscv64 / RISC-V 64-bit
        self.architectures.insert(
            "riscv64".to_string(),
            ArchitectureInfo {
                name: "riscv64".to_string(),
                description: "RISC-V 64-bit".to_string(),
                aliases: vec!["rv64".to_string()],
                word_size: 64,
                endianness: Endianness::Little,
                can_emulate: vec![],
                features: vec![
                    "rvc".to_string(), // Compressed instructions
                    "rvm".to_string(), // Integer multiply/divide
                    "rva".to_string(), // Atomic instructions
                    "rvf".to_string(), // Single-precision float
                    "rvd".to_string(), // Double-precision float
                ],
            },
        );

        // ppc64le / PowerPC 64-bit Little Endian
        self.architectures.insert(
            "ppc64le".to_string(),
            ArchitectureInfo {
                name: "ppc64le".to_string(),
                description: "PowerPC 64-bit Little Endian".to_string(),
                aliases: vec!["powerpc64le".to_string()],
                word_size: 64,
                endianness: Endianness::Little,
                can_emulate: vec![],
                features: vec![
                    "altivec".to_string(),
                    "vsx".to_string(),
                    "kvm".to_string(),
                ],
            },
        );

        // s390x / IBM Z
        self.architectures.insert(
            "s390x".to_string(),
            ArchitectureInfo {
                name: "s390x".to_string(),
                description: "IBM System z (s390x)".to_string(),
                aliases: vec![],
                word_size: 64,
                endianness: Endianness::Big,
                can_emulate: vec![],
                features: vec![
                    "kvm".to_string(),
                ],
            },
        );

        // mips64 / MIPS 64-bit
        self.architectures.insert(
            "mips64".to_string(),
            ArchitectureInfo {
                name: "mips64".to_string(),
                description: "MIPS 64-bit".to_string(),
                aliases: vec![],
                word_size: 64,
                endianness: Endianness::Big,
                can_emulate: vec!["mips".to_string()],
                features: vec![],
            },
        );
    }

    fn build_emulation_matrix(&mut self) {
        // x86_64 can emulate almost everything with QEMU
        for arch in ["aarch64", "riscv64", "ppc64le", "s390x", "mips64"] {
            self.emulation_matrix.insert(
                ("x86_64".to_string(), arch.to_string()),
                EmulationSupport {
                    emulation_type: EmulationType::Qemu,
                    performance_overhead: 50.0, // ~50% slowdown
                    supported_features: vec!["full".to_string()],
                },
            );
        }

        // aarch64 can emulate x86_64 and others
        self.emulation_matrix.insert(
            ("aarch64".to_string(), "x86_64".to_string()),
            EmulationSupport {
                emulation_type: EmulationType::Qemu,
                performance_overhead: 60.0, // Slightly slower
                supported_features: vec!["full".to_string()],
            },
        );

        for arch in ["riscv64", "ppc64le"] {
            self.emulation_matrix.insert(
                ("aarch64".to_string(), arch.to_string()),
                EmulationSupport {
                    emulation_type: EmulationType::Qemu,
                    performance_overhead: 50.0,
                    supported_features: vec!["full".to_string()],
                },
            );
        }

        // Other architectures can emulate with QEMU
        for host in ["riscv64", "ppc64le", "s390x"] {
            for guest in ["x86_64", "aarch64"] {
                self.emulation_matrix.insert(
                    (host.to_string(), guest.to_string()),
                    EmulationSupport {
                        emulation_type: EmulationType::Qemu,
                        performance_overhead: 70.0,
                        supported_features: vec!["basic".to_string()],
                    },
                );
            }
        }
    }

    fn normalize_arch_name(arch: &str) -> String {
        match arch {
            "amd64" | "x64" => "x86_64".to_string(),
            "arm64" | "armv8" => "aarch64".to_string(),
            "rv64" => "riscv64".to_string(),
            "powerpc64le" => "ppc64le".to_string(),
            _ => arch.to_string(),
        }
    }
}

/// Information about a CPU architecture
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchitectureInfo {
    pub name: String,
    pub description: String,
    pub aliases: Vec<String>,
    pub word_size: u8,          // 32 or 64
    pub endianness: Endianness,
    pub can_emulate: Vec<String>, // Other architectures this can natively emulate
    pub features: Vec<String>,   // CPU features (e.g., SSE, AVX, NEON)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Endianness {
    Little,
    Big,
}

/// VM placement compatibility result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlacementCompatibility {
    pub compatible: bool,
    pub native: bool,
    pub emulation_required: bool,
    pub performance_penalty: f64, // Percentage overhead
    pub emulation_type: Option<EmulationType>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EmulationType {
    Native,      // No emulation needed
    Qemu,        // QEMU TCG emulation
    Hvf,         // macOS Hypervisor Framework
    WhpX,        // Windows Hypervisor Platform
}

/// Emulation support information
#[derive(Debug, Clone)]
struct EmulationSupport {
    emulation_type: EmulationType,
    performance_overhead: f64,
    supported_features: Vec<String>,
}

/// Node placement recommendation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodePlacement {
    pub node_id: String,
    pub native_execution: bool,
    pub emulation_type: Option<EmulationType>,
    pub alternative_nodes: Vec<String>,
}

/// Migration compatibility result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationCompatibility {
    pub compatible: bool,
    pub reason: Option<String>,
    pub requires_shutdown: bool, // Live migration not possible
    pub performance_change: f64, // Positive = faster, negative = slower
}

/// Cluster architecture statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterArchStats {
    pub total_nodes: usize,
    pub unique_architectures: usize,
    pub architecture_distribution: HashMap<String, usize>,
    pub mixed_arch_cluster: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_architecture() {
        let arch = ArchitectureManager::detect_host_architecture();
        assert!(arch.is_ok());
    }

    #[test]
    fn test_builtin_architectures() {
        let mgr = ArchitectureManager::new();
        assert!(mgr.get_architecture("x86_64").is_some());
        assert!(mgr.get_architecture("aarch64").is_some());
        assert!(mgr.get_architecture("riscv64").is_some());
        assert!(mgr.get_architecture("ppc64le").is_some());
        assert!(mgr.get_architecture("s390x").is_some());
    }

    #[test]
    fn test_native_placement() {
        let mut mgr = ArchitectureManager::new();
        mgr.register_node("node1".to_string(), "x86_64".to_string()).unwrap();

        let compat = mgr.can_run_on_node("x86_64", "node1").unwrap();
        assert!(compat.compatible);
        assert!(compat.native);
        assert!(!compat.emulation_required);
    }

    #[test]
    fn test_emulated_placement() {
        let mut mgr = ArchitectureManager::new();
        mgr.register_node("node1".to_string(), "x86_64".to_string()).unwrap();

        let compat = mgr.can_run_on_node("aarch64", "node1").unwrap();
        assert!(compat.compatible);
        assert!(!compat.native);
        assert!(compat.emulation_required);
        assert!(compat.performance_penalty > 0.0);
    }

    #[test]
    fn test_migration_validation() {
        let mut mgr = ArchitectureManager::new();
        mgr.register_node("node1".to_string(), "x86_64".to_string()).unwrap();
        mgr.register_node("node2".to_string(), "aarch64".to_string()).unwrap();

        // Migration between different architectures requires shutdown
        let migration = mgr.validate_migration("x86_64", "node1", "node2").unwrap();
        assert!(!migration.compatible); // aarch64 can't run x86_64 natively without emulation

        // Same architecture migration
        mgr.register_node("node3".to_string(), "x86_64".to_string()).unwrap();
        let migration = mgr.validate_migration("x86_64", "node1", "node3").unwrap();
        assert!(migration.compatible);
        assert!(!migration.requires_shutdown);
    }

    #[test]
    fn test_cluster_stats() {
        let mut mgr = ArchitectureManager::new();
        mgr.register_node("node1".to_string(), "x86_64".to_string()).unwrap();
        mgr.register_node("node2".to_string(), "aarch64".to_string()).unwrap();
        mgr.register_node("node3".to_string(), "riscv64".to_string()).unwrap();

        let stats = mgr.get_cluster_stats();
        assert_eq!(stats.total_nodes, 3);
        assert_eq!(stats.unique_architectures, 3);
        assert!(stats.mixed_arch_cluster);
    }

    #[test]
    fn test_custom_architecture() {
        let mut mgr = ArchitectureManager::new();

        let custom_arch = ArchitectureInfo {
            name: "loongarch64".to_string(),
            description: "LoongArch 64-bit".to_string(),
            aliases: vec![],
            word_size: 64,
            endianness: Endianness::Little,
            can_emulate: vec![],
            features: vec![],
        };

        assert!(mgr.register_architecture(custom_arch).is_ok());
        assert!(mgr.get_architecture("loongarch64").is_some());
    }
}
