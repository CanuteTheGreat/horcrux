pub mod cluster_management;
pub mod dashboard;
pub mod failover;
pub mod ha_groups;
pub mod migration_center;

pub use cluster_management::ClusterManagementPage;
pub use dashboard::HaDashboard;
pub use failover::HaFailoverPage;
pub use ha_groups::HaGroupsPage;
pub use migration_center::MigrationCenterPage;
