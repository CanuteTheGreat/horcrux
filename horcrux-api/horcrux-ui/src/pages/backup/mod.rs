mod common;
pub mod dashboard;
pub mod jobs;
pub mod retention_policies;
pub mod template_manager;
pub mod validation;

pub use dashboard::BackupDashboard;
pub use jobs::BackupJobsPage;
pub use retention_policies::RetentionPoliciesPage;
pub use template_manager::TemplateManagerPage;
pub use validation::BackupVerifyPage;
