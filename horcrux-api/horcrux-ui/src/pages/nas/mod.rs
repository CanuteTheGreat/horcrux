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
mod directory;
mod groups;
mod iscsi;
mod pools;
mod s3;
mod scheduler;
mod services;
mod shares;
mod users;

pub use dashboard::NasDashboard;
pub use directory::DirectoryPage;
pub use groups::GroupsPage;
pub use iscsi::IscsiPage;
pub use pools::PoolsPage;
pub use s3::S3Page;
pub use scheduler::SchedulerPage;
pub use services::ServicesPage;
pub use shares::SharesPage;
pub use users::UsersPage as NasUsersPage;
