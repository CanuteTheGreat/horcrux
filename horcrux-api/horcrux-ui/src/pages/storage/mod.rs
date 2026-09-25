//! Storage Management Module
//!
//! This module provides comprehensive storage management including:
//! - Storage pool management (existing functionality)
//! - Storage migration tools
//! - Disk management
//! - Volume management
//! - SMART monitoring

mod disk_management;
mod migration;
mod pool_management;
mod smart_monitoring;
mod volume_management;

pub use pool_management::StorageManagement;
