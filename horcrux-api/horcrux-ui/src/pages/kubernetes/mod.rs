//! Kubernetes Management Pages
//!
//! This module contains all Kubernetes-related pages for the Horcrux web UI.
//! Provides comprehensive Kubernetes management capabilities including cluster
//! management, workload operations, and application lifecycle management.

pub mod cluster_dashboard;
pub mod config;
pub mod helm;
pub mod management;
pub mod workloads;

// Re-export components for easy access
pub use cluster_dashboard::ClusterDashboard;
pub use config::{ConfigMapsPage, SecretsPage};
pub use helm::{HelmChartsPage, HelmReleasesPage, HelmRepositoriesPage};
pub use management::KubernetesManagement;
pub use workloads::{DeploymentsPage, IngressesPage, PodsPage, ServicesPage};
