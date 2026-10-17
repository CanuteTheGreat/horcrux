//! Metrics collection for OpenTelemetry export

use super::{Attribute, AttributeValue, DataPoint, DataPointValue, Metric, MetricData};
use std::time::{SystemTime, UNIX_EPOCH};

/// Metrics collector for Horcrux
pub struct MetricsCollector;

impl MetricsCollector {
    /// Collect system metrics for export
    pub fn collect_system_metrics() -> Vec<Metric> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;

        let mut metrics = Vec::new();

        // CPU metrics
        if let Ok(cpu_usage) = Self::get_cpu_usage() {
            metrics.push(Metric {
                name: "system.cpu.utilization".to_string(),
                description: Some("CPU utilization percentage".to_string()),
                unit: Some("percent".to_string()),
                data: MetricData::Gauge {
                    data_points: vec![DataPoint {
                        attributes: vec![],
                        start_time_unix_nano: now,
                        time_unix_nano: now,
                        value: DataPointValue::AsDouble(cpu_usage),
                    }],
                },
            });
        }

        // Memory metrics
        if let Ok((total, used)) = Self::get_memory_usage() {
            metrics.push(Metric {
                name: "system.memory.usage".to_string(),
                description: Some("Memory usage in bytes".to_string()),
                unit: Some("bytes".to_string()),
                data: MetricData::Gauge {
                    data_points: vec![DataPoint {
                        attributes: vec![Attribute {
                            key: "state".to_string(),
                            value: AttributeValue::String("used".to_string()),
                        }],
                        start_time_unix_nano: now,
                        time_unix_nano: now,
                        value: DataPointValue::AsInt(used as i64),
                    }],
                },
            });

            metrics.push(Metric {
                name: "system.memory.utilization".to_string(),
                description: Some("Memory utilization percentage".to_string()),
                unit: Some("percent".to_string()),
                data: MetricData::Gauge {
                    data_points: vec![DataPoint {
                        attributes: vec![],
                        start_time_unix_nano: now,
                        time_unix_nano: now,
                        value: DataPointValue::AsDouble((used as f64 / total as f64) * 100.0),
                    }],
                },
            });
        }

        // Disk I/O metrics
        if let Ok((read_bytes, write_bytes)) = Self::get_disk_io() {
            metrics.push(Metric {
                name: "system.disk.io".to_string(),
                description: Some("Disk I/O bytes".to_string()),
                unit: Some("bytes".to_string()),
                data: MetricData::Sum {
                    data_points: vec![
                        DataPoint {
                            attributes: vec![Attribute {
                                key: "direction".to_string(),
                                value: AttributeValue::String("read".to_string()),
                            }],
                            start_time_unix_nano: now,
                            time_unix_nano: now,
                            value: DataPointValue::AsInt(read_bytes as i64),
                        },
                        DataPoint {
                            attributes: vec![Attribute {
                                key: "direction".to_string(),
                                value: AttributeValue::String("write".to_string()),
                            }],
                            start_time_unix_nano: now,
                            time_unix_nano: now,
                            value: DataPointValue::AsInt(write_bytes as i64),
                        },
                    ],
                    aggregation_temporality: 2, // CUMULATIVE
                    is_monotonic: true,
                },
            });
        }

        metrics
    }

    /// Collect VM metrics
    pub fn collect_vm_metrics(vm_id: &str, cpu: f64, memory: u64) -> Vec<Metric> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;

        vec![
            Metric {
                name: "vm.cpu.utilization".to_string(),
                description: Some("VM CPU utilization".to_string()),
                unit: Some("percent".to_string()),
                data: MetricData::Gauge {
                    data_points: vec![DataPoint {
                        attributes: vec![Attribute {
                            key: "vm.id".to_string(),
                            value: AttributeValue::String(vm_id.to_string()),
                        }],
                        start_time_unix_nano: now,
                        time_unix_nano: now,
                        value: DataPointValue::AsDouble(cpu),
                    }],
                },
            },
            Metric {
                name: "vm.memory.usage".to_string(),
                description: Some("VM memory usage".to_string()),
                unit: Some("bytes".to_string()),
                data: MetricData::Gauge {
                    data_points: vec![DataPoint {
                        attributes: vec![Attribute {
                            key: "vm.id".to_string(),
                            value: AttributeValue::String(vm_id.to_string()),
                        }],
                        start_time_unix_nano: now,
                        time_unix_nano: now,
                        value: DataPointValue::AsInt(memory as i64),
                    }],
                },
            },
        ]
    }

    // Helper functions to get system metrics

    fn get_cpu_usage() -> Result<f64, String> {
        // Simplified CPU usage - in production would read /proc/stat
        Ok(25.5)
    }

    fn get_memory_usage() -> Result<(u64, u64), String> {
        // Simplified memory usage - in production would read /proc/meminfo
        Ok((16_000_000_000, 8_000_000_000)) // 16GB total, 8GB used
    }

    fn get_disk_io() -> Result<(u64, u64), String> {
        // Simplified disk I/O - in production would read /proc/diskstats
        Ok((1_000_000_000, 500_000_000)) // 1GB read, 500MB written
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collect_system_metrics() {
        let metrics = MetricsCollector::collect_system_metrics();
        assert!(!metrics.is_empty());
    }

    #[test]
    fn test_collect_vm_metrics() {
        let metrics = MetricsCollector::collect_vm_metrics("vm-100", 50.0, 4_000_000_000);
        assert_eq!(metrics.len(), 2);
    }
}
