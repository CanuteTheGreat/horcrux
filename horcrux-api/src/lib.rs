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
pub use horcrux_auth::audit;
pub use horcrux_auth::auth;
pub use horcrux_auth::middleware;

// Virtualization
pub use horcrux_container::container;
pub use horcrux_vm::vm;

// Storage
#[cfg(feature = "backup")]
pub use horcrux_backup::backup;
pub use horcrux_storage::storage;

// Networking
pub use horcrux_sdn::firewall;
pub use horcrux_sdn::sdn;

// Clustering
pub use horcrux_cluster::cluster;
#[cfg(feature = "cluster")]
pub use horcrux_cluster::ha;
pub use horcrux_cluster::migration;

// Monitoring
pub use horcrux_monitoring::alerts;
pub use horcrux_monitoring::metrics;
pub mod metrics_collector;
pub use horcrux_monitoring::metrics_explorer;
pub use horcrux_monitoring::monitoring;
pub use horcrux_monitoring::notification_channels;
pub use horcrux_monitoring::observability;

// Console access
pub use horcrux_vm::console;

// Cloud-init
pub use horcrux_storage::cloudinit;

// GPU passthrough
pub use horcrux_vm::gpu;

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
pub use horcrux_auth::tls;

// Encryption
pub use horcrux_core::encryption;

// Templates
pub use horcrux_storage::template;

// Webhooks
pub use horcrux_monitoring::webhooks;

// WebSocket
pub use horcrux_auth::websocket;

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
