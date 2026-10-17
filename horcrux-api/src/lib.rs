//! Horcrux API Library
//!
//! This module exposes the core functionality of the Horcrux virtualization platform
//! for use by tests and external integrations.

// Allow dead code for library modules that may be used by API consumers
#![allow(dead_code)]

// Core modules
pub mod config;
pub mod error;
pub mod features;
pub mod validation;

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
pub mod storage;

// Networking
pub mod firewall;
pub mod sdn;

// Clustering
pub mod cluster;
#[cfg(feature = "cluster")]
pub mod ha;
pub mod migration;

// Monitoring
pub mod alerts;
pub mod metrics;
pub mod metrics_collector;
pub mod metrics_explorer;
pub mod monitoring;
pub mod notification_channels;
pub mod observability;

// Console access
pub mod console;

// Cloud-init
pub mod cloudinit;

// GPU passthrough
pub mod gpu;

// Kubernetes integration
#[cfg(feature = "kubernetes")]
pub mod kubernetes;

// NAS (Network Attached Storage)
#[cfg(feature = "nas")]
pub use horcrux_nas::nas;

// Database
pub mod db;

// Secrets management
pub mod secrets;

// TLS
pub mod tls;

// Encryption
pub mod encryption;

// Templates
pub mod template;

// Webhooks
pub mod webhooks;

// WebSocket
pub mod websocket;

// Prometheus metrics
pub mod prometheus;

// OpenAPI documentation
pub mod openapi;

// Logging configuration
pub mod logging;

// Health checks and readiness probes
pub mod health;

// Graceful shutdown handling
pub mod shutdown;
