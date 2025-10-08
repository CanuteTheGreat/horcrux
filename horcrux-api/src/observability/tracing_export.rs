//! Distributed tracing for OpenTelemetry export

use super::{Attribute, AttributeValue, SpanKind, SpanStatus, StatusCode, TraceSpan};
use std::time::{SystemTime, UNIX_EPOCH};

/// Trace helper for creating spans
pub struct TraceHelper;

impl TraceHelper {
    /// Create a new trace span
    pub fn create_span(
        name: &str,
        kind: SpanKind,
        parent_span_id: Option<String>,
    ) -> SpanBuilder {
        SpanBuilder::new(name, kind, parent_span_id)
    }

    /// Generate a random trace ID (128-bit hex string)
    pub fn generate_trace_id() -> String {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        format!("{:032x}", rng.gen::<u128>())
    }

    /// Generate a random span ID (64-bit hex string)
    pub fn generate_span_id() -> String {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        format!("{:016x}", rng.gen::<u64>())
    }
}

/// Builder for creating trace spans
pub struct SpanBuilder {
    trace_id: String,
    span_id: String,
    parent_span_id: Option<String>,
    name: String,
    kind: SpanKind,
    start_time: u64,
    attributes: Vec<Attribute>,
}

impl SpanBuilder {
    pub fn new(name: &str, kind: SpanKind, parent_span_id: Option<String>) -> Self {
        let start_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;

        Self {
            trace_id: TraceHelper::generate_trace_id(),
            span_id: TraceHelper::generate_span_id(),
            parent_span_id,
            name: name.to_string(),
            kind,
            start_time,
            attributes: Vec::new(),
        }
    }

    /// Add an attribute to the span
    pub fn with_attribute(mut self, key: &str, value: AttributeValue) -> Self {
        self.attributes.push(Attribute {
            key: key.to_string(),
            value,
        });
        self
    }

    /// Add a string attribute
    pub fn with_string(self, key: &str, value: &str) -> Self {
        self.with_attribute(key, AttributeValue::String(value.to_string()))
    }

    /// Add an integer attribute
    pub fn with_int(self, key: &str, value: i64) -> Self {
        self.with_attribute(key, AttributeValue::Int(value))
    }

    /// Add a boolean attribute
    pub fn with_bool(self, key: &str, value: bool) -> Self {
        self.with_attribute(key, AttributeValue::Bool(value))
    }

    /// Finish the span with a status
    pub fn finish(self, status: StatusCode, message: Option<String>) -> TraceSpan {
        let end_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;

        TraceSpan {
            trace_id: self.trace_id,
            span_id: self.span_id,
            parent_span_id: self.parent_span_id,
            name: self.name,
            kind: self.kind,
            start_time_unix_nano: self.start_time,
            end_time_unix_nano: end_time,
            attributes: self.attributes,
            status: SpanStatus {
                code: status,
                message,
            },
        }
    }

    /// Finish the span successfully
    pub fn finish_ok(self) -> TraceSpan {
        self.finish(StatusCode::Ok, None)
    }

    /// Finish the span with an error
    pub fn finish_error(self, message: &str) -> TraceSpan {
        self.finish(StatusCode::Error, Some(message.to_string()))
    }
}

/// Common span operations
pub struct SpanOperations;

impl SpanOperations {
    /// Create a span for VM creation
    pub fn vm_create(vm_id: &str) -> SpanBuilder {
        TraceHelper::create_span("vm.create", SpanKind::Internal, None)
            .with_string("vm.id", vm_id)
            .with_string("operation", "create")
    }

    /// Create a span for VM start
    pub fn vm_start(vm_id: &str) -> SpanBuilder {
        TraceHelper::create_span("vm.start", SpanKind::Internal, None)
            .with_string("vm.id", vm_id)
            .with_string("operation", "start")
    }

    /// Create a span for VM stop
    pub fn vm_stop(vm_id: &str) -> SpanBuilder {
        TraceHelper::create_span("vm.stop", SpanKind::Internal, None)
            .with_string("vm.id", vm_id)
            .with_string("operation", "stop")
    }

    /// Create a span for VM migration
    pub fn vm_migrate(vm_id: &str, source_node: &str, target_node: &str) -> SpanBuilder {
        TraceHelper::create_span("vm.migrate", SpanKind::Internal, None)
            .with_string("vm.id", vm_id)
            .with_string("source.node", source_node)
            .with_string("target.node", target_node)
            .with_string("operation", "migrate")
    }

    /// Create a span for API request
    pub fn http_request(method: &str, path: &str, status_code: u16) -> SpanBuilder {
        TraceHelper::create_span("http.request", SpanKind::Server, None)
            .with_string("http.method", method)
            .with_string("http.path", path)
            .with_int("http.status_code", status_code as i64)
    }

    /// Create a span for storage operation
    pub fn storage_operation(operation: &str, pool: &str, volume: &str) -> SpanBuilder {
        TraceHelper::create_span("storage.operation", SpanKind::Internal, None)
            .with_string("storage.operation", operation)
            .with_string("storage.pool", pool)
            .with_string("storage.volume", volume)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_span_builder() {
        let span = TraceHelper::create_span("test.span", SpanKind::Internal, None)
            .with_string("test.key", "test.value")
            .finish_ok();

        assert_eq!(span.name, "test.span");
        assert_eq!(span.attributes.len(), 1);
    }

    #[test]
    fn test_vm_create_span() {
        let span = SpanOperations::vm_create("vm-100").finish_ok();
        assert_eq!(span.name, "vm.create");
    }

    #[test]
    fn test_trace_id_generation() {
        let id1 = TraceHelper::generate_trace_id();
        let id2 = TraceHelper::generate_trace_id();
        assert_ne!(id1, id2);
        assert_eq!(id1.len(), 32); // 128 bits = 32 hex chars
    }
}
