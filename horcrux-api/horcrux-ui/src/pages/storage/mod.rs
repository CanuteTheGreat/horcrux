//! Storage Management Module
//!
//! This module provides comprehensive storage management including:
//! - Storage pool management (existing functionality)
//! - Storage migration tools
//! - Disk management
//! - Volume management
//! - SMART monitoring

mod pool_management;
mod migration;
mod disk_management;
mod volume_management;
mod smart_monitoring;

pub use pool_management::StorageManagement;
