//! Kubernetes configuration and storage resources
//!
//! Handles ConfigMaps, Secrets, PVCs, PVs, and StorageClasses.

pub mod configmaps;
pub mod secrets;
pub mod pvcs;
pub mod storageclasses;

/// Config and storage manager
pub struct ConfigStorageManager;

impl ConfigStorageManager {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ConfigStorageManager {
    fn default() -> Self {
        Self::new()
    }
}
