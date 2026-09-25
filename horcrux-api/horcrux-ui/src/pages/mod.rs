mod alerts;
pub mod audit;
pub mod auth;
pub mod backup;
mod clone_list;
mod console;
mod container_list;
pub mod dashboard;
mod gpu;
pub mod ha;
mod kubernetes;
mod login;
pub mod metrics;
mod monitoring;
pub mod nas;
mod network;
mod replication_list;
mod snapshot_list;
pub mod storage;
pub mod system;
mod vm_create;
mod vm_list;

pub use alerts::Alerts;
pub use clone_list::CloneList;
pub use container_list::ContainerList;
pub use dashboard::Dashboard;
pub use gpu::GpuManagement;
pub use login::Login;
pub use monitoring::Monitoring;
pub use network::NetworkManagement;
pub use replication_list::ReplicationList;
pub use snapshot_list::SnapshotList;
pub use storage::StorageManagement;
pub use vm_create::VmCreate;
pub use vm_list::VmList;
// pub use console::ConsolePage;

// Re-export auth components
pub use auth::{ApiKeysPage, RolesPage, SessionsPage, UsersPage};

// Re-export kubernetes components
pub use kubernetes::{
    ClusterDashboard, ConfigMapsPage, DeploymentsPage, HelmChartsPage, HelmReleasesPage,
    HelmRepositoriesPage, IngressesPage, KubernetesManagement, PodsPage, SecretsPage, ServicesPage,
};

// Re-export backup components
pub use backup::{
    BackupDashboard, BackupJobsPage, RetentionPoliciesPage, SnapshotManagerPage,
    TemplateManagerPage,
};

// Re-export HA components
pub use ha::{ClusterManagementPage, HaDashboard, HaGroupsPage, MigrationCenterPage};

// Re-export monitoring components
pub use monitoring::{
    AlertCenterPage, DashboardsPage, MetricsExplorerPage, NotificationsPage, ObservabilityPage,
};

// Re-export system components

// Re-export dashboard components

// Re-export audit components

// Re-export metrics components

// Re-export storage components

// Re-export NAS components
#[allow(unused_imports)]
pub use nas::{
    DirectoryPage, GroupsPage as NasGroupsPage, IscsiPage, NasDashboard, NasUsersPage, PoolsPage,
    S3Page, SchedulerPage as NasSchedulerPage, ServicesPage as NasServicesPage,
    SharesPage as NasSharesPage,
};
