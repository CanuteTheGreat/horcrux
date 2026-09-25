//! Kubernetes Workload Management Pages
//!
//! This module contains all Kubernetes workload-related pages for the Horcrux web UI.
//! Provides comprehensive workload management capabilities including pods, deployments,
//! services, and ingresses.

pub mod deployments;
pub mod ingresses;
pub mod pods;
pub mod services;

// Re-export components for easy access
pub use deployments::DeploymentsPage;
pub use ingresses::IngressesPage;
pub use pods::PodsPage;
pub use services::ServicesPage;
