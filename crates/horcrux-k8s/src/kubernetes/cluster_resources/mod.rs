//! Kubernetes cluster-scoped resources
//!
//! Handles Namespaces, Nodes, ResourceQuotas, LimitRanges, and RBAC.

pub mod namespaces;
pub mod nodes;
pub mod quotas;
pub mod rbac;

/// Cluster resources manager
pub struct ClusterResourcesManager;

impl ClusterResourcesManager {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ClusterResourcesManager {
    fn default() -> Self {
        Self::new()
    }
}
