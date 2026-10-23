mod alerts;
pub mod audit;
pub mod auth;
pub mod backup;
pub mod ceph;
mod clone_list;
mod console;
mod container_console;
pub mod container_create;
mod container_events;
mod container_firewall;
mod container_images;
mod container_list;
mod container_networks;
mod container_stacks;
mod container_volumes;
pub mod dashboard;
mod firewall_objects;
mod gpu;
pub mod ha;
mod host_bridges;
mod iso_library;
pub mod kubernetes;
mod login;
pub mod lxc;
mod monitoring;
pub mod nas;
mod network;
mod power;
mod replication_list;
mod resource_pools;
pub mod sdn_zones;
mod snapshot_list;
pub mod storage;
mod storage_content;
pub mod system;
mod vm_create;
pub mod vm_devices;
mod vm_display;
mod vm_hardware;
mod vm_list;

pub use alerts::Alerts;
pub use clone_list::CloneList;
pub use console::VmConsole;
pub use container_console::{ContainerExecPage, ContainerLogsPage, ContainerSystemConsolePage};
pub use container_create::ContainerCreate;
pub use container_events::ContainerEventsPage;
pub use container_firewall::ContainerFirewallPage;
pub use container_images::ImagesPage;
pub use container_list::ContainerList;
pub use container_networks::ContainerNetworksPage;
pub use container_stacks::StacksPage;
pub use container_volumes::VolumesPage;
pub use dashboard::Dashboard;
pub use firewall_objects::FirewallObjectsPage;
pub use gpu::GpuManagement;
pub use host_bridges::HostBridgesPage;
pub use iso_library::IsoLibraryPage;
pub use login::Login;
pub use lxc::{LxcContainerPage, LxcTemplatesPage};
pub use monitoring::Monitoring;
pub use network::NetworkManagement;
pub use power::{human_size, saved_label, GuestPowerPage};
pub use replication_list::ReplicationList;
pub use resource_pools::ResourcePoolsPage;
pub use snapshot_list::SnapshotList;
pub use storage::{DiskManagementPage, SmartMonitoringPage, StorageManagement};
pub use storage_content::StorageContentPage;
pub use vm_create::VmCreate;
pub use vm_hardware::VmHardwarePage;
pub use vm_list::VmList;

// Re-export auth components
pub use auth::{
    ApiKeysPage, CertificatesPage, MfaPage, PermissionsPage, RealmsPage, RolesPage, SessionsPage,
    UserMenu, UsersPage,
};

// Re-export kubernetes components
pub use kubernetes::{
    ClusterDashboard, ConfigMapsPage, DeploymentsPage, HelmChartsPage, HelmReleasesPage,
    HelmRepositoriesPage, IngressesPage, KubernetesManagement, PodsPage, SecretsPage, ServicesPage,
};

// Re-export backup components
pub use backup::{
    BackupDashboard, BackupJobsPage, BackupVerifyPage, RetentionPoliciesPage, SnapshotManagerPage,
    TemplateManagerPage,
};

// Re-export HA components
pub use ha::{
    ClusterManagementPage, HaDashboard, HaFailoverPage, HaGroupsPage, MaintenancePage,
    MigrationCenterPage,
};

// Re-export monitoring components
pub use monitoring::{
    AlertCenterPage, DashboardsPage, MetricsExplorerPage, NotificationsPage, ObservabilityPage,
};

// Re-export system components

// Re-export dashboard components

// Re-export audit components
pub use audit::AuditLogPage;

// Re-export storage components

// Re-export NAS components
#[allow(unused_imports)]
pub use nas::{
    DirectoryPage, GroupsPage as NasGroupsPage, IscsiPage, NasDashboard, NasUsersPage, PoolsPage,
    S3Page, SchedulerPage as NasSchedulerPage, ServicesPage as NasServicesPage,
    SharesPage as NasSharesPage,
};
