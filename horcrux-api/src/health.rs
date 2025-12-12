//! Health check and readiness probes for production deployment
//!
//! Provides comprehensive health checking for:
//! - Database connectivity
//! - Storage backends
//! - Monitoring subsystem
//! - Cluster connectivity
//! - External service dependencies

use serde::{Deserialize, Serialize};
use std::time::Instant;

/// Overall system health status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    /// All components healthy
    Healthy,
    /// Some components degraded but functional
    Degraded,
    /// System is unhealthy
    Unhealthy,
}

/// Individual component health
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentHealth {
    pub name: String,
    pub status: HealthStatus,
    pub message: Option<String>,
    pub latency_ms: Option<u64>,
}

/// Comprehensive health check response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: HealthStatus,
    pub version: String,
    pub uptime_seconds: u64,
    pub timestamp: i64,
    pub components: Vec<ComponentHealth>,
}

/// Liveness probe response (for k8s/container orchestration)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LivenessResponse {
    pub alive: bool,
    pub timestamp: i64,
}

/// Readiness probe response (for k8s/container orchestration)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadinessResponse {
    pub ready: bool,
    pub reason: Option<String>,
    pub timestamp: i64,
}

/// Health checker for system components
pub struct HealthChecker {
    start_time: Instant,
    version: String,
}

impl HealthChecker {
    pub fn new(version: &str) -> Self {
        Self {
            start_time: Instant::now(),
            version: version.to_string(),
        }
    }

    /// Get uptime in seconds
    pub fn uptime_seconds(&self) -> u64 {
        self.start_time.elapsed().as_secs()
    }

    /// Simple liveness check - is the service running?
    pub fn liveness(&self) -> LivenessResponse {
        LivenessResponse {
            alive: true,
            timestamp: chrono::Utc::now().timestamp(),
        }
    }

    /// Check if database is reachable
    pub async fn check_database(&self, db: &crate::db::Database) -> ComponentHealth {
        let start = Instant::now();

        match db.health_check().await {
            Ok(_) => ComponentHealth {
                name: "database".to_string(),
                status: HealthStatus::Healthy,
                message: Some("Connected".to_string()),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Err(e) => ComponentHealth {
                name: "database".to_string(),
                status: HealthStatus::Unhealthy,
                message: Some(format!("Connection failed: {}", e)),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
        }
    }

    /// Check monitoring subsystem
    pub async fn check_monitoring(&self, monitoring: &crate::monitoring::MonitoringManager) -> ComponentHealth {
        let start = Instant::now();

        match monitoring.collect_resource_metrics().await {
            Ok(_) => ComponentHealth {
                name: "monitoring".to_string(),
                status: HealthStatus::Healthy,
                message: Some("Metrics collection working".to_string()),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Err(e) => ComponentHealth {
                name: "monitoring".to_string(),
                status: HealthStatus::Degraded,
                message: Some(format!("Metrics collection failed: {}", e)),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
        }
    }

    /// Check storage manager
    pub async fn check_storage(&self, storage: &crate::storage::StorageManager) -> ComponentHealth {
        let start = Instant::now();

        // Just verify storage manager is accessible
        let pools = storage.list_pools().await;

        ComponentHealth {
            name: "storage".to_string(),
            status: HealthStatus::Healthy,
            message: Some(format!("{} pool(s) configured", pools.len())),
            latency_ms: Some(start.elapsed().as_millis() as u64),
        }
    }

    /// Check VM manager
    pub async fn check_vm_manager(&self, vm_manager: &crate::vm::VmManager) -> ComponentHealth {
        let start = Instant::now();

        // Verify VM manager is responsive
        let vms = vm_manager.list_vms().await;

        ComponentHealth {
            name: "vm_manager".to_string(),
            status: HealthStatus::Healthy,
            message: Some(format!("{} VM(s) registered", vms.len())),
            latency_ms: Some(start.elapsed().as_millis() as u64),
        }
    }

    /// Check cluster connectivity
    pub async fn check_cluster(&self, cluster: &crate::cluster::ClusterManager) -> ComponentHealth {
        let start = Instant::now();

        let node_count = cluster.list_nodes().await.len();
        let status_result = cluster.get_cluster_status().await;

        let (health_status, has_quorum) = match status_result {
            Ok(status) => {
                let h = if status.has_quorum {
                    HealthStatus::Healthy
                } else if node_count > 0 {
                    HealthStatus::Degraded
                } else {
                    HealthStatus::Healthy // Single-node mode is OK
                };
                (h, status.has_quorum)
            }
            Err(_) => (HealthStatus::Healthy, true), // Single-node fallback
        };

        ComponentHealth {
            name: "cluster".to_string(),
            status: health_status,
            message: Some(format!(
                "{} node(s), quorum: {}",
                node_count,
                if has_quorum { "yes" } else { "no" }
            )),
            latency_ms: Some(start.elapsed().as_millis() as u64),
        }
    }

    /// Check WebSocket state
    pub fn check_websocket(&self, ws_state: &crate::websocket::WsState) -> ComponentHealth {
        let connection_count = ws_state.connection_count();

        ComponentHealth {
            name: "websocket".to_string(),
            status: HealthStatus::Healthy,
            message: Some(format!("{} active connection(s)", connection_count)),
            latency_ms: Some(0),
        }
    }

    /// Aggregate component health into overall status
    fn aggregate_status(components: &[ComponentHealth]) -> HealthStatus {
        let mut has_unhealthy = false;
        let mut has_degraded = false;

        for component in components {
            match component.status {
                HealthStatus::Unhealthy => has_unhealthy = true,
                HealthStatus::Degraded => has_degraded = true,
                HealthStatus::Healthy => {}
            }
        }

        if has_unhealthy {
            HealthStatus::Unhealthy
        } else if has_degraded {
            HealthStatus::Degraded
        } else {
            HealthStatus::Healthy
        }
    }

    /// Build complete health response
    pub fn build_response(&self, components: Vec<ComponentHealth>) -> HealthResponse {
        let status = Self::aggregate_status(&components);

        HealthResponse {
            status,
            version: self.version.clone(),
            uptime_seconds: self.uptime_seconds(),
            timestamp: chrono::Utc::now().timestamp(),
            components,
        }
    }

    /// Check readiness (can the service accept traffic?)
    pub fn readiness(&self, components: &[ComponentHealth]) -> ReadinessResponse {
        // We're ready if database is healthy
        let db_healthy = components.iter()
            .find(|c| c.name == "database")
            .map(|c| c.status == HealthStatus::Healthy)
            .unwrap_or(false);

        if db_healthy {
            ReadinessResponse {
                ready: true,
                reason: None,
                timestamp: chrono::Utc::now().timestamp(),
            }
        } else {
            ReadinessResponse {
                ready: false,
                reason: Some("Database not ready".to_string()),
                timestamp: chrono::Utc::now().timestamp(),
            }
        }
    }
}

/// Startup health checker for initialization phase
pub struct StartupChecker {
    checks_passed: Vec<String>,
    checks_failed: Vec<(String, String)>,
}

impl StartupChecker {
    pub fn new() -> Self {
        Self {
            checks_passed: Vec::new(),
            checks_failed: Vec::new(),
        }
    }

    pub fn pass(&mut self, check_name: &str) {
        self.checks_passed.push(check_name.to_string());
    }

    pub fn fail(&mut self, check_name: &str, reason: &str) {
        self.checks_failed.push((check_name.to_string(), reason.to_string()));
    }

    pub fn is_healthy(&self) -> bool {
        self.checks_failed.is_empty()
    }

    pub fn summary(&self) -> String {
        let mut msg = format!("Startup checks: {} passed", self.checks_passed.len());
        if !self.checks_failed.is_empty() {
            msg.push_str(&format!(", {} failed", self.checks_failed.len()));
            for (name, reason) in &self.checks_failed {
                msg.push_str(&format!("\n  - {}: {}", name, reason));
            }
        }
        msg
    }
}

impl Default for StartupChecker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_status_aggregation() {
        let healthy_components = vec![
            ComponentHealth {
                name: "db".to_string(),
                status: HealthStatus::Healthy,
                message: None,
                latency_ms: Some(5),
            },
            ComponentHealth {
                name: "storage".to_string(),
                status: HealthStatus::Healthy,
                message: None,
                latency_ms: Some(10),
            },
        ];

        assert_eq!(
            HealthChecker::aggregate_status(&healthy_components),
            HealthStatus::Healthy
        );

        let degraded_components = vec![
            ComponentHealth {
                name: "db".to_string(),
                status: HealthStatus::Healthy,
                message: None,
                latency_ms: Some(5),
            },
            ComponentHealth {
                name: "monitoring".to_string(),
                status: HealthStatus::Degraded,
                message: None,
                latency_ms: Some(10),
            },
        ];

        assert_eq!(
            HealthChecker::aggregate_status(&degraded_components),
            HealthStatus::Degraded
        );

        let unhealthy_components = vec![
            ComponentHealth {
                name: "db".to_string(),
                status: HealthStatus::Unhealthy,
                message: None,
                latency_ms: Some(100),
            },
            ComponentHealth {
                name: "storage".to_string(),
                status: HealthStatus::Healthy,
                message: None,
                latency_ms: Some(10),
            },
        ];

        assert_eq!(
            HealthChecker::aggregate_status(&unhealthy_components),
            HealthStatus::Unhealthy
        );
    }

    #[test]
    fn test_startup_checker() {
        let mut checker = StartupChecker::new();

        checker.pass("database");
        checker.pass("storage");

        assert!(checker.is_healthy());

        checker.fail("cluster", "No nodes found");

        assert!(!checker.is_healthy());
        assert!(checker.summary().contains("1 failed"));
    }

    #[test]
    fn test_liveness() {
        let checker = HealthChecker::new("0.1.0");
        let response = checker.liveness();

        assert!(response.alive);
        assert!(response.timestamp > 0);
    }
}
