//! Kubernetes networking resources
//!
//! Handles Services, Ingress, and NetworkPolicies.

pub mod services;
pub mod ingress;
pub mod network_policies;

/// Networking manager aggregating all networking operations
pub struct NetworkingManager;

impl NetworkingManager {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NetworkingManager {
    fn default() -> Self {
        Self::new()
    }
}
