//! Horcrux API Library
//!
//! This module exposes the core functionality of the Horcrux virtualization platform
//! for use by tests and external integrations.

// Allow dead code for library modules that may be used by API consumers
#![allow(dead_code)]

// Core modules
pub use horcrux_core::config;
pub use horcrux_core::error;
pub use horcrux_core::features;
pub use horcrux_core::validation;

// Application state
pub mod state;
pub use state::AppState;

// The API server (startup, router, HTTP handlers) and route modules
pub mod api;
pub mod app;
#[cfg(feature = "lxc")]
#[path = "api/lxc.rs"]
pub mod lxc_api;
pub mod persistence;
pub mod static_ui;
pub mod vm_console_api;

// Authentication & Authorization
pub mod audit;
pub mod auth;
pub mod middleware;

// Virtualization
pub mod container;
pub mod vm;

// Storage
#[cfg(feature = "backup")]
pub mod backup;
pub use horcrux_storage::storage;

// Networking
pub use horcrux_sdn::firewall;
pub use horcrux_sdn::sdn;

// Clustering
pub mod cluster;
#[cfg(feature = "cluster")]
pub mod ha;
pub mod migration;

// Monitoring
pub use horcrux_monitoring::alerts;
pub use horcrux_monitoring::metrics;
pub mod metrics_collector;
pub use horcrux_monitoring::metrics_explorer;
pub use horcrux_monitoring::monitoring;
pub use horcrux_monitoring::notification_channels;
pub use horcrux_monitoring::observability;

// Console access
pub mod console;

// Cloud-init
pub use horcrux_storage::cloudinit;

// GPU passthrough
pub mod gpu;

// Kubernetes integration
#[cfg(feature = "kubernetes")]
pub use horcrux_k8s::kubernetes;

// NAS (Network Attached Storage)
#[cfg(feature = "nas")]
pub use horcrux_nas::nas;

// Database
pub use horcrux_core::db;

// Secrets management
pub use horcrux_core::secrets;

// TLS
pub mod tls;

// Encryption
pub use horcrux_core::encryption;

// Templates
pub use horcrux_storage::template;

// Webhooks
pub use horcrux_monitoring::webhooks;

// WebSocket
pub mod websocket;

// Prometheus metrics
pub use horcrux_monitoring::prometheus;

// OpenAPI documentation
pub mod openapi;

// Logging configuration
pub use horcrux_core::logging;

// Health checks and readiness probes
pub mod health;

// Graceful shutdown handling
pub mod shutdown;
