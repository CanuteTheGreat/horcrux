pub mod dashboard;
mod vm_list;
mod vm_create;
mod alerts;
mod login;
mod container_list;
mod snapshot_list;
mod clone_list;
mod replication_list;
mod monitoring;
mod gpu;
mod kubernetes;
pub mod storage;
mod network;
mod console;
pub mod auth;
pub mod backup;
pub mod ha;
pub mod system;
pub mod audit;
pub mod metrics;
pub mod nas;

pub use dashboard::Dashboard;
pub use vm_list::VmList;
pub use vm_create::VmCreate;
pub use alerts::Alerts;
pub use login::Login;
pub use container_list::ContainerList;
pub use snapshot_list::SnapshotList;
pub use clone_list::CloneList;
pub use replication_list::ReplicationList;
pub use monitoring::Monitoring;
pub use gpu::GpuManagement;
pub use storage::StorageManagement;
pub use network::NetworkManagement;
// pub use console::ConsolePage;

// Re-export auth components
pub use auth::{UsersPage, RolesPage, SessionsPage, ApiKeysPage};

// Re-export kubernetes components
pub use kubernetes::{KubernetesManagement, PodsPage, DeploymentsPage, ServicesPage, IngressesPage, ClusterDashboard, HelmRepositoriesPage, HelmChartsPage, HelmReleasesPage, ConfigMapsPage, SecretsPage};

// Re-export backup components
pub use backup::{BackupDashboard, BackupJobsPage, RetentionPoliciesPage, SnapshotManagerPage, TemplateManagerPage};

// Re-export HA components
pub use ha::{HaDashboard, ClusterManagementPage, HaGroupsPage, MigrationCenterPage};

// Re-export monitoring components
pub use monitoring::{AlertCenterPage, DashboardsPage, MetricsExplorerPage, NotificationsPage, ObservabilityPage};

// Re-export system components

// Re-export dashboard components

// Re-export audit components

// Re-export metrics components

// Re-export storage components

// Re-export NAS components
#[allow(unused_imports)]
pub use nas::{NasDashboard, SharesPage as NasSharesPage, NasUsersPage, GroupsPage as NasGroupsPage, ServicesPage as NasServicesPage, PoolsPage, IscsiPage, S3Page, DirectoryPage, SchedulerPage as NasSchedulerPage};
