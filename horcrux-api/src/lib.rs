//! Horcrux API Library
//!
//! This module exposes the core functionality of the Horcrux virtualization platform
//! for use by tests and external integrations.

// Allow dead code for library modules that may be used by API consumers
#![allow(dead_code)]

// Core modules
pub mod config;
pub mod error;
pub mod validation;

// Application state
pub mod state;
pub use state::AppState;

// Authentication & Authorization
pub mod auth;
pub mod middleware;
pub mod audit;

// Virtualization
pub mod vm;
pub mod container;

// Storage
pub mod storage;
pub mod backup;

// Networking
pub mod sdn;
pub mod firewall;

// Clustering
pub mod cluster;
pub mod migration;
pub mod ha;

// Monitoring
pub mod monitoring;
pub mod alerts;
pub mod metrics;
pub mod metrics_collector;
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
