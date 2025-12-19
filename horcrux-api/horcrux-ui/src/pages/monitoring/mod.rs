mod basic_monitoring;
pub mod alert_center;
pub mod dashboards;
pub mod metrics_explorer;
pub mod notifications;
pub mod observability;

pub use basic_monitoring::Monitoring;
pub use alert_center::AlertCenterPage;
pub use dashboards::DashboardsPage;
pub use metrics_explorer::MetricsExplorerPage;
pub use notifications::NotificationsPage;
pub use observability::ObservabilityPage;