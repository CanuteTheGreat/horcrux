//! Kubernetes Workload Management Pages
//!
//! This module contains all Kubernetes workload-related pages for the Horcrux web UI.
//! Provides comprehensive workload management capabilities including pods, deployments,
//! services, and ingresses.

pub mod pods;
pub mod deployments;
pub mod services;
pub mod ingresses;

// Re-export components for easy access
pub use pods::PodsPage;
pub use deployments::DeploymentsPage;
pub use services::ServicesPage;
pub use ingresses::IngressesPage;