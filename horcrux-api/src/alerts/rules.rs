///! Alert rule definitions and evaluation

use serde::{Deserialize, Serialize};
use super::AlertSeverity;

/// Type of metric to monitor
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricType {
    CpuUsage,
    MemoryUsage,
    DiskUsage,
    DiskIo,
    NetworkIo,
    NodeLoad,
    VmCount,
}

impl MetricType {
    pub fn to_string(&self) -> &str {
        match self {
            MetricType::CpuUsage => "CPU usage",
            MetricType::MemoryUsage => "Memory usage",
            MetricType::DiskUsage => "Disk usage",
            MetricType::DiskIo => "Disk I/O",
            MetricType::NetworkIo => "Network I/O",
            MetricType::NodeLoad => "Node load",
            MetricType::VmCount => "VM count",
        }
    }
}

/// Comparison operator for thresholds
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComparisonOperator {
    GreaterThan,
    LessThan,
    Equal,
    NotEqual,
}

impl ComparisonOperator {
    pub fn evaluate(&self, value: f64, threshold: f64) -> bool {
        match self {
            ComparisonOperator::GreaterThan => value > threshold,
            ComparisonOperator::LessThan => value < threshold,
            ComparisonOperator::Equal => (value - threshold).abs() < f64::EPSILON,
            ComparisonOperator::NotEqual => (value - threshold).abs() >= f64::EPSILON,
        }
    }

    pub fn to_string(&self) -> &str {
        match self {
            ComparisonOperator::GreaterThan => ">",
            ComparisonOperator::LessThan => "<",
            ComparisonOperator::Equal => "==",
            ComparisonOperator::NotEqual => "!=",
        }
    }
}

/// Alert condition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertCondition {
    pub metric_type: MetricType,
    pub operator: ComparisonOperator,
    pub threshold: f64,
    pub target_pattern: String, // e.g., "vm-*", "node-*", "*", "vm-100"
    pub duration_seconds: u64,   // How long condition must be true before firing
}

impl AlertCondition {
    /// Evaluate if the condition is met
    pub fn evaluate(&self, value: f64) -> bool {
        self.operator.evaluate(value, self.threshold)
    }
}

/// Alert rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertRule {
    pub id: String,
    pub name: String,
    pub description: String,
    pub severity: AlertSeverity,
    pub enabled: bool,
    pub condition: AlertCondition,
}

impl AlertRule {
    /// Create a new alert rule
    pub fn new(
        name: String,
        description: String,
        severity: AlertSeverity,
        condition: AlertCondition,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            description,
            severity,
            enabled: true,
            condition,
        }
    }

    /// Create a predefined high CPU usage alert
    pub fn high_cpu_usage(target: &str, threshold: f64) -> Self {
        Self::new(
            "High CPU Usage".to_string(),
            format!("CPU usage exceeds {}%", threshold),
            AlertSeverity::Warning,
            AlertCondition {
                metric_type: MetricType::CpuUsage,
                operator: ComparisonOperator::GreaterThan,
                threshold,
                target_pattern: target.to_string(),
                duration_seconds: 60,
            },
        )
    }

    /// Create a predefined high memory usage alert
    pub fn high_memory_usage(target: &str, threshold: f64) -> Self {
        Self::new(
            "High Memory Usage".to_string(),
            format!("Memory usage exceeds {}%", threshold),
            AlertSeverity::Warning,
            AlertCondition {
                metric_type: MetricType::MemoryUsage,
                operator: ComparisonOperator::GreaterThan,
                threshold,
                target_pattern: target.to_string(),
                duration_seconds: 60,
            },
        )
    }

    /// Create a predefined disk full alert
    pub fn disk_full(target: &str, threshold: f64) -> Self {
        Self::new(
            "Disk Almost Full".to_string(),
            format!("Disk usage exceeds {}%", threshold),
            AlertSeverity::Critical,
            AlertCondition {
                metric_type: MetricType::DiskUsage,
                operator: ComparisonOperator::GreaterThan,
                threshold,
                target_pattern: target.to_string(),
                duration_seconds: 300,
            },
        )
    }

    /// Create a predefined high node load alert
    pub fn high_node_load(target: &str, threshold: f64) -> Self {
        Self::new(
            "High Node Load".to_string(),
            format!("Node load average exceeds {}", threshold),
            AlertSeverity::Warning,
            AlertCondition {
                metric_type: MetricType::NodeLoad,
                operator: ComparisonOperator::GreaterThan,
                threshold,
                target_pattern: target.to_string(),
                duration_seconds: 180,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_comparison_operators() {
        assert!(ComparisonOperator::GreaterThan.evaluate(90.0, 80.0));
        assert!(!ComparisonOperator::GreaterThan.evaluate(70.0, 80.0));

        assert!(ComparisonOperator::LessThan.evaluate(70.0, 80.0));
        assert!(!ComparisonOperator::LessThan.evaluate(90.0, 80.0));
    }

    #[test]
    fn test_alert_condition() {
        let condition = AlertCondition {
            metric_type: MetricType::CpuUsage,
            operator: ComparisonOperator::GreaterThan,
            threshold: 80.0,
            target_pattern: "*".to_string(),
            duration_seconds: 60,
        };

        assert!(condition.evaluate(90.0));
        assert!(!condition.evaluate(70.0));
    }

    #[test]
    fn test_predefined_rules() {
        let rule = AlertRule::high_cpu_usage("vm-*", 80.0);
        assert_eq!(rule.severity, AlertSeverity::Warning);
        assert_eq!(rule.condition.threshold, 80.0);

        let rule = AlertRule::disk_full("*", 90.0);
        assert_eq!(rule.severity, AlertSeverity::Critical);
    }
}
