//! Storage Management Module
//!
//! This module provides comprehensive storage management including:
//! - Storage pool management (existing functionality)
//! - Disk management
//! - Volume management (per pool, in pool_management)
//! - SMART monitoring
//! - SAN (iSCSI / NVMe-oF / FC targets, path view, add-storage wizard)

mod disk_management;
mod plugin_form;
mod pool_management;
mod san;
mod smart_monitoring;

pub use disk_management::DiskManagementPage;
pub use pool_management::StorageManagement;
pub use san::{SanPage, SanWizard};
pub use smart_monitoring::SmartMonitoringPage;
