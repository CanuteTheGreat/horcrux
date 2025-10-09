//! OpenTelemetry Observability Module
//!
//! Provides modern observability with:
//! - Metrics export via OTLP/HTTP
//! - Distributed tracing
//! - Integration with Prometheus, Grafana, Jaeger
//! - Native Prometheus metrics export
//!
//! Proxmox VE 9.0 feature parity

pub mod metrics;
pub mod prometheus;
pub mod tracing_export;
pub mod config;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// OpenTelemetry manager
pub struct OtelManager {
    config: Arc<RwLock<OtelConfig>>,
    metrics_exporter: Arc<RwLock<Option<MetricsExporter>>>,
    trace_exporter: Arc<RwLock<Option<TraceExporter>>>,
}

/// OpenTelemetry configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtelConfig {
    pub enabled: bool,
    pub endpoint: String,
    pub protocol: OtelProtocol,
    pub service_name: String,
    pub service_version: String,
    pub export_interval_secs: u64,
    pub headers: HashMap<String, String>,
    pub tls_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum OtelProtocol {
    #[serde(rename = "http")]
    Http,
    #[serde(rename = "grpc")]
    Grpc,
}

impl Default for OtelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: "http://localhost:4318".to_string(),
            protocol: OtelProtocol::Http,
            service_name: "horcrux".to_string(),
            service_version: env!("CARGO_PKG_VERSION").to_string(),
            export_interval_secs: 60,
            headers: HashMap::new(),
            tls_enabled: false,
        }
    }
}

impl OtelManager {
    pub fn new() -> Self {
        Self {
            config: Arc::new(RwLock::new(OtelConfig::default())),
            metrics_exporter: Arc::new(RwLock::new(None)),
            trace_exporter: Arc::new(RwLock::new(None)),
        }
    }

    /// Initialize OpenTelemetry with configuration
    pub async fn initialize(&self, config: OtelConfig) -> Result<(), String> {
        if !config.enabled {
            return Ok(());
        }

        // Store configuration
        *self.config.write().await = config.clone();

        // Initialize metrics exporter
        let metrics_exp = MetricsExporter::new(&config)?;
        *self.metrics_exporter.write().await = Some(metrics_exp);

        // Initialize trace exporter
        let trace_exp = TraceExporter::new(&config)?;
        *self.trace_exporter.write().await = Some(trace_exp);

        tracing::info!(
            "OpenTelemetry initialized: endpoint={}, protocol={:?}",
            config.endpoint,
            config.protocol
        );

        Ok(())
    }

    /// Export metrics to OTLP endpoint
    pub async fn export_metrics(&self, metrics: Vec<Metric>) -> Result<(), String> {
        let exporter_guard = self.metrics_exporter.read().await;

        if let Some(exporter) = exporter_guard.as_ref() {
            exporter.export(metrics).await?;
            Ok(())
        } else {
            Err("Metrics exporter not initialized".to_string())
        }
    }

    /// Export trace span to OTLP endpoint
    pub async fn export_trace(&self, span: TraceSpan) -> Result<(), String> {
        let exporter_guard = self.trace_exporter.read().await;

        if let Some(exporter) = exporter_guard.as_ref() {
            exporter.export(span).await?;
            Ok(())
        } else {
            Err("Trace exporter not initialized".to_string())
        }
    }

    /// Get current configuration
    pub async fn get_config(&self) -> OtelConfig {
        self.config.read().await.clone()
    }

    /// Update configuration
    pub async fn update_config(&self, config: OtelConfig) -> Result<(), String> {
        self.initialize(config).await
    }

    /// Shutdown and cleanup
    pub async fn shutdown(&self) -> Result<(), String> {
        *self.metrics_exporter.write().await = None;
        *self.trace_exporter.write().await = None;
        Ok(())
    }
}

/// Metrics exporter
pub struct MetricsExporter {
    endpoint: String,
    headers: HashMap<String, String>,
    client: reqwest::Client,
}

impl MetricsExporter {
    fn new(config: &OtelConfig) -> Result<Self, String> {
        let endpoint = match config.protocol {
            OtelProtocol::Http => format!("{}/v1/metrics", config.endpoint),
            OtelProtocol::Grpc => return Err("gRPC not yet supported".to_string()),
        };

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

        Ok(Self {
            endpoint,
            headers: config.headers.clone(),
            client,
        })
    }

    async fn export(&self, metrics: Vec<Metric>) -> Result<(), String> {
        let payload = MetricsPayload {
            resource_metrics: vec![ResourceMetrics {
                resource: Resource {
                    attributes: vec![
                        Attribute {
                            key: "service.name".to_string(),
                            value: AttributeValue::String("horcrux".to_string()),
                        },
                    ],
                },
                scope_metrics: vec![ScopeMetrics {
                    scope: InstrumentationScope {
                        name: "horcrux".to_string(),
                        version: env!("CARGO_PKG_VERSION").to_string(),
                    },
                    metrics,
                }],
            }],
        };

        let mut request = self.client.post(&self.endpoint).json(&payload);

        for (key, value) in &self.headers {
            request = request.header(key, value);
        }

        let response = request
            .send()
            .await
            .map_err(|e| format!("Failed to send metrics: {}", e))?;

        if !response.status().is_success() {
            return Err(format!(
                "Metrics export failed: HTTP {}",
                response.status()
            ));
        }

        Ok(())
    }
}

/// Trace exporter
pub struct TraceExporter {
    endpoint: String,
    headers: HashMap<String, String>,
    client: reqwest::Client,
}

impl TraceExporter {
    fn new(config: &OtelConfig) -> Result<Self, String> {
        let endpoint = match config.protocol {
            OtelProtocol::Http => format!("{}/v1/traces", config.endpoint),
            OtelProtocol::Grpc => return Err("gRPC not yet supported".to_string()),
        };

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

        Ok(Self {
            endpoint,
            headers: config.headers.clone(),
            client,
        })
    }

    async fn export(&self, span: TraceSpan) -> Result<(), String> {
        let payload = TracesPayload {
            resource_spans: vec![ResourceSpans {
                resource: Resource {
                    attributes: vec![
                        Attribute {
                            key: "service.name".to_string(),
                            value: AttributeValue::String("horcrux".to_string()),
                        },
                    ],
                },
                scope_spans: vec![ScopeSpans {
                    scope: InstrumentationScope {
                        name: "horcrux".to_string(),
                        version: env!("CARGO_PKG_VERSION").to_string(),
                    },
                    spans: vec![span],
                }],
            }],
        };

        let mut request = self.client.post(&self.endpoint).json(&payload);

        for (key, value) in &self.headers {
            request = request.header(key, value);
        }

        let response = request
            .send()
            .await
            .map_err(|e| format!("Failed to send trace: {}", e))?;

        if !response.status().is_success() {
            return Err(format!("Trace export failed: HTTP {}", response.status()));
        }

        Ok(())
    }
}

// OTLP data structures

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsPayload {
    #[serde(rename = "resourceMetrics")]
    resource_metrics: Vec<ResourceMetrics>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceMetrics {
    resource: Resource,
    #[serde(rename = "scopeMetrics")]
    scope_metrics: Vec<ScopeMetrics>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resource {
    attributes: Vec<Attribute>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attribute {
    key: String,
    value: AttributeValue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AttributeValue {
    String(String),
    Int(i64),
    Double(f64),
    Bool(bool),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeMetrics {
    scope: InstrumentationScope,
    metrics: Vec<Metric>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstrumentationScope {
    name: String,
    version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metric {
    pub name: String,
    pub description: Option<String>,
    pub unit: Option<String>,
    #[serde(flatten)]
    pub data: MetricData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MetricData {
    Gauge { data_points: Vec<DataPoint> },
    Sum { data_points: Vec<DataPoint>, aggregation_temporality: i32, is_monotonic: bool },
    Histogram { data_points: Vec<HistogramDataPoint>, aggregation_temporality: i32 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataPoint {
    pub attributes: Vec<Attribute>,
    pub start_time_unix_nano: u64,
    pub time_unix_nano: u64,
    #[serde(flatten)]
    pub value: DataPointValue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DataPointValue {
    AsInt(i64),
    AsDouble(f64),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistogramDataPoint {
    pub attributes: Vec<Attribute>,
    pub start_time_unix_nano: u64,
    pub time_unix_nano: u64,
    pub count: u64,
    pub sum: Option<f64>,
    pub bucket_counts: Vec<u64>,
    pub explicit_bounds: Vec<f64>,
}

// Tracing structures

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TracesPayload {
    #[serde(rename = "resourceSpans")]
    resource_spans: Vec<ResourceSpans>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceSpans {
    resource: Resource,
    #[serde(rename = "scopeSpans")]
    scope_spans: Vec<ScopeSpans>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeSpans {
    scope: InstrumentationScope,
    spans: Vec<TraceSpan>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceSpan {
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub name: String,
    pub kind: SpanKind,
    pub start_time_unix_nano: u64,
    pub end_time_unix_nano: u64,
    pub attributes: Vec<Attribute>,
    pub status: SpanStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SpanKind {
    #[serde(rename = "SPAN_KIND_INTERNAL")]
    Internal,
    #[serde(rename = "SPAN_KIND_SERVER")]
    Server,
    #[serde(rename = "SPAN_KIND_CLIENT")]
    Client,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpanStatus {
    pub code: StatusCode,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StatusCode {
    #[serde(rename = "STATUS_CODE_UNSET")]
    Unset,
    #[serde(rename = "STATUS_CODE_OK")]
    Ok,
    #[serde(rename = "STATUS_CODE_ERROR")]
    Error,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_otel_manager_creation() {
        let manager = OtelManager::new();
        let config = manager.get_config().await;
        assert!(!config.enabled);
    }

    #[test]
    fn test_default_config() {
        let config = OtelConfig::default();
        assert_eq!(config.service_name, "horcrux");
        assert_eq!(config.protocol, OtelProtocol::Http);
    }
}
