//! Storage Management Module
//!
//! This module provides comprehensive storage management including:
//! - Storage pool management (existing functionality)
//! - Storage migration tools
//! - Disk management
//! - Volume management (per pool, in pool_management)
//! - SMART monitoring

mod disk_management;
mod migration;
mod pool_management;
mod smart_monitoring;

pub use disk_management::DiskManagementPage;
pub use pool_management::StorageManagement;
pub use smart_monitoring::SmartMonitoringPage;
