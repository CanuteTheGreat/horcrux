#![allow(dead_code)]

use std::collections::HashMap;
use std::fmt::Write as FmtWrite;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Metric type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricType {
    Counter,
    Gauge,
    Histogram,
    Summary,
}

impl MetricType {
    fn as_str(&self) -> &'static str {
        match self {
            MetricType::Counter => "counter",
            MetricType::Gauge => "gauge",
            MetricType::Histogram => "histogram",
            MetricType::Summary => "summary",
        }
    }
}

/// Single metric value
#[derive(Debug, Clone)]
pub struct Metric {
    pub name: String,
    pub metric_type: MetricType,
    pub help: String,
    pub values: Vec<(HashMap<String, String>, f64)>,
}

impl Metric {
    pub fn new(name: String, metric_type: MetricType, help: String) -> Self {
        Self {
            name,
            metric_type,
            help,
            values: Vec::new(),
        }
    }

    pub fn add_value(&mut self, labels: HashMap<String, String>, value: f64) {
        self.values.push((labels, value));
    }

    pub fn set_value(&mut self, labels: HashMap<String, String>, value: f64) {
        // Replace existing value or add new
        if let Some(pos) = self.values.iter().position(|(l, _)| l == &labels) {
            self.values[pos].1 = value;
        } else {
            self.values.push((labels, value));
        }
    }

    /// Format metric in Prometheus exposition format
    pub fn format_prometheus(&self) -> String {
        let mut output = String::new();

        // Write HELP line
        writeln!(&mut output, "# HELP {} {}", self.name, self.help).unwrap();

        // Write TYPE line
        writeln!(&mut output, "# TYPE {} {}", self.name, self.metric_type.as_str()).unwrap();

        // Write metric values
        for (labels, value) in &self.values {
            if labels.is_empty() {
                writeln!(&mut output, "{} {}", self.name, value).unwrap();
            } else {
                let label_str = labels
                    .iter()
                    .map(|(k, v)| format!("{}=\"{}\"", k, v))
                    .collect::<Vec<_>>()
                    .join(",");

                writeln!(&mut output, "{}{{{}}} {}", self.name, label_str, value).unwrap();
            }
        }

        output
    }
}

/// Prometheus metrics registry
pub struct PrometheusRegistry {
    metrics: Arc<RwLock<HashMap<String, Metric>>>,
}

impl PrometheusRegistry {
    pub fn new() -> Self {
        Self {
            metrics: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a new metric
    pub async fn register(&self, metric: Metric) {
        let mut metrics = self.metrics.write().await;
        metrics.insert(metric.name.clone(), metric);
    }

    /// Update counter metric
    pub async fn inc_counter(&self, name: &str, labels: HashMap<String, String>) {
        let mut metrics = self.metrics.write().await;
        if let Some(metric) = metrics.get_mut(name) {
            if let Some((_, value)) = metric.values.iter_mut().find(|(l, _)| l == &labels) {
                *value += 1.0;
            } else {
                metric.values.push((labels, 1.0));
            }
        }
    }

    /// Set gauge metric value
    pub async fn set_gauge(&self, name: &str, labels: HashMap<String, String>, value: f64) {
        let mut metrics = self.metrics.write().await;
        if let Some(metric) = metrics.get_mut(name) {
            metric.set_value(labels, value);
        }
    }

    /// Export all metrics in Prometheus format
    pub async fn export(&self) -> String {
        let metrics = self.metrics.read().await;
        let mut output = String::new();

        for metric in metrics.values() {
            output.push_str(&metric.format_prometheus());
            output.push('\n');
        }

        output
    }

    /// Clear all metrics
    pub async fn clear(&self) {
        let mut metrics = self.metrics.write().await;
        metrics.clear();
    }
}

/// Prometheus metrics manager
pub struct PrometheusManager {
    registry: PrometheusRegistry,
}

impl PrometheusManager {
    pub fn new() -> Self {
        let registry = PrometheusRegistry::new();
        Self { registry }
    }

    /// Initialize standard Horcrux metrics
    pub async fn init_default_metrics(&self) {
        // VM metrics
        let vm_count = Metric::new(
            "horcrux_vms_total".to_string(),
            MetricType::Gauge,
            "Total number of VMs".to_string(),
        );
        self.registry.register(vm_count.clone()).await;

        let vm_running = Metric::new(
            "horcrux_vms_running".to_string(),
            MetricType::Gauge,
            "Number of running VMs".to_string(),
        );
        self.registry.register(vm_running.clone()).await;

        let vm_cpu_usage = Metric::new(
            "horcrux_vm_cpu_usage_percent".to_string(),
            MetricType::Gauge,
            "VM CPU usage percentage".to_string(),
        );
        self.registry.register(vm_cpu_usage.clone()).await;

        let vm_memory_usage = Metric::new(
            "horcrux_vm_memory_usage_bytes".to_string(),
            MetricType::Gauge,
            "VM memory usage in bytes".to_string(),
        );
        self.registry.register(vm_memory_usage.clone()).await;

        // Node metrics
        let node_cpu = Metric::new(
            "horcrux_node_cpu_usage_percent".to_string(),
            MetricType::Gauge,
            "Node CPU usage percentage".to_string(),
        );
        self.registry.register(node_cpu.clone()).await;

        let node_memory = Metric::new(
            "horcrux_node_memory_usage_bytes".to_string(),
            MetricType::Gauge,
            "Node memory usage in bytes".to_string(),
        );
        self.registry.register(node_memory.clone()).await;

        let node_uptime = Metric::new(
            "horcrux_node_uptime_seconds".to_string(),
            MetricType::Counter,
            "Node uptime in seconds".to_string(),
        );
        self.registry.register(node_uptime.clone()).await;

        // Storage metrics
        let storage_total = Metric::new(
            "horcrux_storage_total_bytes".to_string(),
            MetricType::Gauge,
            "Total storage capacity in bytes".to_string(),
        );
        self.registry.register(storage_total.clone()).await;

        let storage_used = Metric::new(
            "horcrux_storage_used_bytes".to_string(),
            MetricType::Gauge,
            "Used storage in bytes".to_string(),
        );
        self.registry.register(storage_used.clone()).await;

        // API metrics
        let http_requests = Metric::new(
            "horcrux_http_requests_total".to_string(),
            MetricType::Counter,
            "Total HTTP requests".to_string(),
        );
        self.registry.register(http_requests.clone()).await;

        let http_request_duration = Metric::new(
            "horcrux_http_request_duration_seconds".to_string(),
            MetricType::Histogram,
            "HTTP request duration in seconds".to_string(),
        );
        self.registry.register(http_request_duration.clone()).await;

        // Backup metrics
        let backup_count = Metric::new(
            "horcrux_backups_total".to_string(),
            MetricType::Gauge,
            "Total number of backups".to_string(),
        );
        self.registry.register(backup_count.clone()).await;

        let backup_size = Metric::new(
            "horcrux_backup_size_bytes".to_string(),
            MetricType::Gauge,
            "Backup size in bytes".to_string(),
        );
        self.registry.register(backup_size.clone()).await;

        // Cluster metrics
        let cluster_nodes = Metric::new(
            "horcrux_cluster_nodes_total".to_string(),
            MetricType::Gauge,
            "Total number of cluster nodes".to_string(),
        );
        self.registry.register(cluster_nodes.clone()).await;

        let cluster_nodes_online = Metric::new(
            "horcrux_cluster_nodes_online".to_string(),
            MetricType::Gauge,
            "Number of online cluster nodes".to_string(),
        );
        self.registry.register(cluster_nodes_online.clone()).await;

        tracing::info!("Initialized Prometheus metrics");
    }

    /// Update VM metrics
    pub async fn update_vm_metrics(&self, vm_id: &str, vm_name: &str, cpu_usage: f64, memory_usage: u64, _status: &str) {
        let mut labels = HashMap::new();
        labels.insert("vm_id".to_string(), vm_id.to_string());
        labels.insert("vm_name".to_string(), vm_name.to_string());

        self.registry.set_gauge("horcrux_vm_cpu_usage_percent", labels.clone(), cpu_usage).await;
        self.registry.set_gauge("horcrux_vm_memory_usage_bytes", labels.clone(), memory_usage as f64).await;
    }

    /// Update node metrics
    pub async fn update_node_metrics(&self, node_name: &str, cpu_usage: f64, memory_usage: u64, uptime: u64) {
        let mut labels = HashMap::new();
        labels.insert("node".to_string(), node_name.to_string());

        self.registry.set_gauge("horcrux_node_cpu_usage_percent", labels.clone(), cpu_usage).await;
        self.registry.set_gauge("horcrux_node_memory_usage_bytes", labels.clone(), memory_usage as f64).await;
        self.registry.set_gauge("horcrux_node_uptime_seconds", labels.clone(), uptime as f64).await;
    }

    /// Update storage metrics
    pub async fn update_storage_metrics(&self, pool_id: &str, pool_name: &str, total: u64, used: u64) {
        let mut labels = HashMap::new();
        labels.insert("pool_id".to_string(), pool_id.to_string());
        labels.insert("pool_name".to_string(), pool_name.to_string());

        self.registry.set_gauge("horcrux_storage_total_bytes", labels.clone(), (total * 1024 * 1024 * 1024) as f64).await;
        self.registry.set_gauge("horcrux_storage_used_bytes", labels.clone(), ((total - used) * 1024 * 1024 * 1024) as f64).await;
    }

    /// Increment HTTP request counter
    pub async fn inc_http_requests(&self, method: &str, path: &str, status_code: u16) {
        let mut labels = HashMap::new();
        labels.insert("method".to_string(), method.to_string());
        labels.insert("path".to_string(), path.to_string());
        labels.insert("status".to_string(), status_code.to_string());

        self.registry.inc_counter("horcrux_http_requests_total", labels).await;
    }

    /// Update cluster metrics
    pub async fn update_cluster_metrics(&self, total_nodes: usize, online_nodes: usize) {
        self.registry.set_gauge("horcrux_cluster_nodes_total", HashMap::new(), total_nodes as f64).await;
        self.registry.set_gauge("horcrux_cluster_nodes_online", HashMap::new(), online_nodes as f64).await;
    }

    /// Export metrics in Prometheus format
    pub async fn export_metrics(&self) -> String {
        self.registry.export().await
    }

    /// Get the registry reference
    pub fn registry(&self) -> &PrometheusRegistry {
        &self.registry
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metric_formatting() {
        let mut metric = Metric::new(
            "test_metric".to_string(),
            MetricType::Gauge,
            "A test metric".to_string(),
        );

        let mut labels = HashMap::new();
        labels.insert("instance".to_string(), "localhost".to_string());
        metric.add_value(labels, 42.0);

        let output = metric.format_prometheus();
        assert!(output.contains("# HELP test_metric A test metric"));
        assert!(output.contains("# TYPE test_metric gauge"));
        assert!(output.contains("test_metric{instance=\"localhost\"} 42"));
    }

    #[tokio::test]
    async fn test_registry() {
        let registry = PrometheusRegistry::new();

        let metric = Metric::new(
            "test_counter".to_string(),
            MetricType::Counter,
            "Test counter".to_string(),
        );

        registry.register(metric).await;

        let output = registry.export().await;
        assert!(output.contains("test_counter"));
    }

    #[tokio::test]
    async fn test_manager_init() {
        let manager = PrometheusManager::new();
        manager.init_default_metrics().await;

        let output = manager.export_metrics().await;
        assert!(output.contains("horcrux_vms_total"));
        assert!(output.contains("horcrux_node_cpu_usage_percent"));
    }
}
