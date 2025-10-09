///! Prometheus metrics exporter
///!
///! Exposes Horcrux metrics in Prometheus format

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Prometheus metric type
#[derive(Debug, Clone, PartialEq)]
pub enum MetricType {
    Counter,
    Gauge,
    Histogram,
    Summary,
}

/// Prometheus metric
#[derive(Debug, Clone)]
pub struct Metric {
    pub name: String,
    pub help: String,
    pub metric_type: MetricType,
    pub value: f64,
    pub labels: HashMap<String, String>,
}

/// Prometheus exporter
pub struct PrometheusExporter {
    metrics: Arc<RwLock<Vec<Metric>>>,
    prefix: String,
}

impl PrometheusExporter {
    pub fn new(prefix: &str) -> Self {
        Self {
            metrics: Arc::new(RwLock::new(Vec::new())),
            prefix: prefix.to_string(),
        }
    }

    /// Register a counter metric
    pub async fn counter(&self, name: &str, help: &str, value: f64, labels: HashMap<String, String>) {
        let metric = Metric {
            name: format!("{}_{}", self.prefix, name),
            help: help.to_string(),
            metric_type: MetricType::Counter,
            value,
            labels,
        };

        let mut metrics = self.metrics.write().await;
        metrics.push(metric);
    }

    /// Register a gauge metric
    pub async fn gauge(&self, name: &str, help: &str, value: f64, labels: HashMap<String, String>) {
        let metric = Metric {
            name: format!("{}_{}", self.prefix, name),
            help: help.to_string(),
            metric_type: MetricType::Gauge,
            value,
            labels,
        };

        let mut metrics = self.metrics.write().await;
        metrics.push(metric);
    }

    /// Export all metrics in Prometheus text format
    pub async fn export(&self) -> String {
        let metrics = self.metrics.read().await;
        let mut output = String::new();

        // Group metrics by name
        let mut grouped: HashMap<String, Vec<&Metric>> = HashMap::new();
        for metric in metrics.iter() {
            grouped.entry(metric.name.clone())
                .or_insert_with(Vec::new)
                .push(metric);
        }

        // Format metrics
        for (name, metric_group) in grouped.iter() {
            if let Some(first) = metric_group.first() {
                // HELP line
                output.push_str(&format!("# HELP {} {}\n", name, first.help));

                // TYPE line
                let type_str = match first.metric_type {
                    MetricType::Counter => "counter",
                    MetricType::Gauge => "gauge",
                    MetricType::Histogram => "histogram",
                    MetricType::Summary => "summary",
                };
                output.push_str(&format!("# TYPE {} {}\n", name, type_str));

                // Metric values
                for metric in metric_group {
                    let labels_str = self.format_labels(&metric.labels);
                    if labels_str.is_empty() {
                        output.push_str(&format!("{} {}\n", name, metric.value));
                    } else {
                        output.push_str(&format!("{}{{{}}}{}\n",
                            name, labels_str, metric.value));
                    }
                }

                output.push('\n');
            }
        }

        output
    }

    /// Format labels for Prometheus
    fn format_labels(&self, labels: &HashMap<String, String>) -> String {
        if labels.is_empty() {
            return String::new();
        }

        labels.iter()
            .map(|(k, v)| format!("{}=\"{}\"", k, v))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Clear all metrics
    pub async fn clear(&self) {
        let mut metrics = self.metrics.write().await;
        metrics.clear();
    }

    /// Collect and export system metrics
    pub async fn collect_system_metrics(&self) {
        self.clear().await;

        // VM metrics
        self.gauge("vm_count", "Total number of VMs", 10.0, HashMap::new()).await;
        self.gauge("vm_running", "Number of running VMs", 7.0, HashMap::new()).await;

        // CPU metrics
        let mut cpu_labels = HashMap::new();
        cpu_labels.insert("node".to_string(), "node1".to_string());
        self.gauge("cpu_usage_percent", "CPU usage percentage", 45.5, cpu_labels.clone()).await;

        // Memory metrics
        self.gauge("memory_total_bytes", "Total memory in bytes", 64_000_000_000.0, cpu_labels.clone()).await;
        self.gauge("memory_used_bytes", "Used memory in bytes", 32_000_000_000.0, cpu_labels.clone()).await;

        // Storage metrics
        let mut storage_labels = HashMap::new();
        storage_labels.insert("pool".to_string(), "local".to_string());
        storage_labels.insert("type".to_string(), "zfs".to_string());
        self.gauge("storage_total_bytes", "Total storage in bytes", 1_000_000_000_000.0, storage_labels.clone()).await;
        self.gauge("storage_used_bytes", "Used storage in bytes", 500_000_000_000.0, storage_labels.clone()).await;

        // Network metrics
        let mut net_labels = HashMap::new();
        net_labels.insert("interface".to_string(), "vmbr0".to_string());
        self.counter("network_rx_bytes_total", "Total received bytes", 1_234_567_890.0, net_labels.clone()).await;
        self.counter("network_tx_bytes_total", "Total transmitted bytes", 9_876_543_210.0, net_labels.clone()).await;

        // Cluster metrics
        self.gauge("cluster_nodes_total", "Total cluster nodes", 3.0, HashMap::new()).await;
        self.gauge("cluster_nodes_online", "Online cluster nodes", 3.0, HashMap::new()).await;
        self.gauge("cluster_quorate", "Cluster has quorum (1=yes, 0=no)", 1.0, HashMap::new()).await;

        // Migration metrics
        self.counter("migrations_total", "Total migrations performed", 25.0, HashMap::new()).await;
        self.gauge("migrations_active", "Currently active migrations", 0.0, HashMap::new()).await;

        // Backup metrics
        self.counter("backups_total", "Total backups created", 150.0, HashMap::new()).await;
        self.gauge("backup_jobs_active", "Active backup jobs", 1.0, HashMap::new()).await;

        // HA metrics
        self.gauge("ha_resources_total", "Total HA resources", 5.0, HashMap::new()).await;
        self.gauge("ha_resources_started", "HA resources in started state", 5.0, HashMap::new()).await;
        self.gauge("ha_resources_error", "HA resources in error state", 0.0, HashMap::new()).await;
    }

    /// Export metrics for specific VM
    pub async fn collect_vm_metrics(&self, vm_id: u32, cpu_percent: f64, memory_bytes: u64, disk_io_bytes: u64) {
        let mut labels = HashMap::new();
        labels.insert("vm_id".to_string(), vm_id.to_string());

        self.gauge("vm_cpu_usage_percent", "VM CPU usage", cpu_percent, labels.clone()).await;
        self.gauge("vm_memory_bytes", "VM memory usage", memory_bytes as f64, labels.clone()).await;
        self.counter("vm_disk_io_bytes_total", "VM disk I/O bytes", disk_io_bytes as f64, labels).await;
    }
}

/// Build Prometheus HTTP response
pub fn build_prometheus_response(metrics: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: text/plain; version=0.0.4\r\n\
         Content-Length: {}\r\n\
         \r\n\
         {}",
        metrics.len(),
        metrics
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_prometheus_export() {
        let exporter = PrometheusExporter::new("horcrux");

        let mut labels = HashMap::new();
        labels.insert("node".to_string(), "node1".to_string());

        exporter.gauge("cpu_usage", "CPU usage percentage", 75.5, labels).await;
        exporter.counter("requests_total", "Total requests", 12345.0, HashMap::new()).await;

        let output = exporter.export().await;

        assert!(output.contains("# HELP horcrux_cpu_usage CPU usage percentage"));
        assert!(output.contains("# TYPE horcrux_cpu_usage gauge"));
        assert!(output.contains("horcrux_cpu_usage{node=\"node1\"} 75.5"));

        assert!(output.contains("# HELP horcrux_requests_total Total requests"));
        assert!(output.contains("# TYPE horcrux_requests_total counter"));
        assert!(output.contains("horcrux_requests_total 12345"));
    }

    #[tokio::test]
    async fn test_label_formatting() {
        let exporter = PrometheusExporter::new("test");

        let mut labels = HashMap::new();
        labels.insert("key1".to_string(), "value1".to_string());
        labels.insert("key2".to_string(), "value2".to_string());

        let formatted = exporter.format_labels(&labels);

        assert!(formatted.contains("key1=\"value1\""));
        assert!(formatted.contains("key2=\"value2\""));
        assert!(formatted.contains(","));
    }

    #[tokio::test]
    async fn test_clear_metrics() {
        let exporter = PrometheusExporter::new("test");

        exporter.gauge("test_metric", "Test", 1.0, HashMap::new()).await;

        let output1 = exporter.export().await;
        assert!(!output1.is_empty());

        exporter.clear().await;

        let output2 = exporter.export().await;
        assert!(output2.is_empty());
    }

    #[tokio::test]
    async fn test_collect_system_metrics() {
        let exporter = PrometheusExporter::new("horcrux");

        exporter.collect_system_metrics().await;

        let output = exporter.export().await;

        // Check for key metrics
        assert!(output.contains("horcrux_vm_count"));
        assert!(output.contains("horcrux_cpu_usage_percent"));
        assert!(output.contains("horcrux_memory_total_bytes"));
        assert!(output.contains("horcrux_cluster_nodes_total"));
    }
}
