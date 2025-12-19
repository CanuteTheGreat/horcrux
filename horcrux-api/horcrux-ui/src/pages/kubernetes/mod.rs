//! Kubernetes Management Pages
//!
//! This module contains all Kubernetes-related pages for the Horcrux web UI.
//! Provides comprehensive Kubernetes management capabilities including cluster
//! management, workload operations, and application lifecycle management.

pub mod management;
pub mod cluster_dashboard;
pub mod workloads;
pub mod helm;
pub mod config;

// Re-export components for easy access
pub use management::KubernetesManagement;
pub use cluster_dashboard::ClusterDashboard;
pub use workloads::{PodsPage, DeploymentsPage, ServicesPage, IngressesPage};
pub use helm::{HelmRepositoriesPage, HelmChartsPage, HelmReleasesPage};
pub use config::{ConfigMapsPage, SecretsPage};