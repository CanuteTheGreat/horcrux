//! NAS Management Module
//!
//! This module provides comprehensive NAS management including:
//! - NAS Dashboard with service overview
//! - Share management (SMB, NFS, AFP, WebDAV, FTP)
//! - User and group management
//! - Storage pool and dataset management
//! - iSCSI target management
//! - S3 gateway management
//! - Directory services (LDAP, Kerberos, Active Directory)
//! - Replication and snapshot policies
//! - Job scheduling for automated tasks

mod dashboard;
mod shares;
mod users;
mod groups;
mod services;
mod pools;
mod iscsi;
mod s3;
mod directory;
mod scheduler;

pub use dashboard::NasDashboard;
pub use shares::SharesPage;
pub use users::UsersPage as NasUsersPage;
pub use groups::GroupsPage;
pub use services::ServicesPage;
pub use pools::PoolsPage;
pub use iscsi::IscsiPage;
pub use s3::S3Page;
pub use directory::DirectoryPage;
pub use scheduler::SchedulerPage;
