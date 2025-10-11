# Horcrux Logging Guide

## Overview

Horcrux uses structured logging with `tracing` for comprehensive observability. Logs are output to both console and files with automatic rotation.

## Log Levels

| Level | Description | Use Case |
|-------|-------------|----------|
| `TRACE` | Very detailed information | Deep debugging, request/response bodies |
| `DEBUG` | Detailed information | Development debugging, SQL queries |
| `INFO` | General information | Normal operations, lifecycle events |
| `WARN` | Warning messages | Deprecated features, recoverable errors |
| `ERROR` | Error messages | Operation failures, exceptions |

## Configuration

### Environment Variables

```bash
# Set log level (default: info)
export RUST_LOG=debug

# Set log file path (default: /var/log/horcrux)
export HORCRUX_LOG_PATH=/var/log/horcrux

# Enable specific modules
export RUST_LOG=horcrux_api=debug,sqlx=warn

# Multiple targets
export RUST_LOG=horcrux_api::vm=trace,horcrux_api::db=debug,info
```

### Programmatic Configuration

```rust
use horcrux_api::logging::{LoggingConfig, LogRotation};

let config = LoggingConfig {
    level: "debug".to_string(),
    file_path: Some("/var/log/horcrux".to_string()),
    rotation: LogRotation::Daily,
    json_format: false,
    include_targets: vec![],
};

config.init()?;
```

## Log Outputs

### Console Output (STDOUT)

- **Format**: Human-readable with ANSI colors
- **Target**: Shown (e.g., `horcrux_api::vm`)
- **Level**: Shown with color
- **Thread IDs**: Hidden

**Example**:
```
2025-10-09T10:30:45.123Z  INFO horcrux_api::vm: VM operation operation="start" vm_id="vm-100"
2025-10-09T10:30:46.456Z  WARN horcrux_api::db: Slow query duration_ms=1234 query="SELECT * FROM vms"
2025-10-09T10:30:47.789Z ERROR horcrux_api::vm: Failed to start VM vm_id="vm-100" error="Connection refused"
```

### File Output (JSON)

- **Format**: Structured JSON (one log per line)
- **Location**: `/var/log/horcrux/horcrux.log`
- **Rotation**: Daily by default
- **Thread IDs**: Included

**Example**:
```json
{"timestamp":"2025-10-09T10:30:45.123Z","level":"INFO","target":"horcrux_api::vm","fields":{"operation":"start","vm_id":"vm-100"},"message":"VM operation"}
{"timestamp":"2025-10-09T10:30:46.456Z","level":"WARN","target":"horcrux_api::db","fields":{"duration_ms":1234,"query":"SELECT * FROM vms"},"message":"Slow query"}
{"timestamp":"2025-10-09T10:30:47.789Z","level":"ERROR","target":"horcrux_api::vm","fields":{"vm_id":"vm-100","error":"Connection refused"},"message":"Failed to start VM"}
```

## Logging Macros

### Standard Logging

```rust
use tracing::{trace, debug, info, warn, error};

// Simple message
info!("Server started");

// With structured fields
info!(port = 8006, "Server listening");

// Multiple fields
info!(
    vm_id = "vm-100",
    status = "running",
    memory_mb = 2048,
    "VM started successfully"
);

// Error with context
error!(
    error = %err,
    vm_id = "vm-100",
    "Failed to start VM"
);
```

### Custom Logging Macros

#### VM Operations

```rust
use horcrux_api::log_vm_operation;

// Simple operation
log_vm_operation!("start", "vm-100");

// With additional context
log_vm_operation!(
    "start",
    "vm-100",
    memory_mb = 2048,
    cpus = 2
);
```

**Output**:
```
INFO horcrux_api: VM operation operation="start" vm_id="vm-100" memory_mb=2048 cpus=2
```

#### API Requests

```rust
use horcrux_api::log_api_request;

// Basic request
log_api_request!("GET", "/api/vms");

// With user context
log_api_request!("POST", "/api/vms", "admin@localhost");
```

**Output**:
```
DEBUG horcrux_api: API request method="GET" path="/api/vms" user="admin@localhost"
```

#### Database Operations

```rust
use horcrux_api::log_db_operation;

// Simple operation
log_db_operation!("SELECT", "vms");

// With record ID
log_db_operation!("UPDATE", "vms", "vm-100");
```

**Output**:
```
DEBUG horcrux_api: Database operation operation="UPDATE" table="vms" record_id="vm-100"
```

#### Performance Metrics

```rust
use horcrux_api::log_performance;
use std::time::Instant;

let start = Instant::now();
// ... operation ...
log_performance!("vm_start", start.elapsed().as_millis());
```

**Output**:
```
INFO horcrux_api: Performance metric operation="vm_start" duration_ms=1234
```

## Structured Logging Best Practices

### 1. Use Spans for Context

```rust
use tracing::instrument;

#[instrument(skip(db))]
async fn create_vm(db: &Database, config: VmConfig) -> Result<()> {
    info!("Creating VM");
    // All logs within this function will include vm_id automatically
    Ok(())
}

// Spans can be manual too
let span = info_span!("vm_operation", vm_id = "vm-100");
let _enter = span.enter();
info!("Starting VM"); // Includes vm_id="vm-100"
```

### 2. Log at Appropriate Levels

```rust
// ERROR - Operation failed
error!(error = %err, "Failed to connect to database");

// WARN - Something unexpected but recoverable
warn!(vm_id = "vm-100", "VM memory usage above 90%");

// INFO - Normal operation lifecycle
info!(vm_id = "vm-100", "VM started successfully");

// DEBUG - Detailed information for debugging
debug!(query = "SELECT * FROM vms", "Executing database query");

// TRACE - Very detailed information
trace!(headers = ?req.headers(), "Received HTTP request");
```

### 3. Include Relevant Context

```rust
// Bad - Missing context
error!("Operation failed");

// Good - Includes what failed and why
error!(
    operation = "start_vm",
    vm_id = "vm-100",
    error = %err,
    "Failed to start VM"
);
```

### 4. Use Display vs Debug Formatting

```rust
// Display formatting (%)
error!(error = %err, "Failed");

// Debug formatting (?)
debug!(config = ?vm_config, "Using configuration");

// Both
info!(
    message = %err,  // User-friendly
    details = ?err,  // Technical details
    "Error occurred"
);
```

### 5. Avoid Logging Sensitive Information

```rust
// Bad - Logs password
debug!(password = config.password, "User config");

// Good - Redact sensitive data
debug!(
    username = config.username,
    password = "[REDACTED]",
    "User config"
);
```

## Common Logging Patterns

### API Handler Logging

```rust
async fn start_vm_handler(
    Path(vm_id): Path<String>,
) -> Result<Json<VmConfig>, ApiError> {
    info!(vm_id = %vm_id, "Received request to start VM");

    match vm_manager.start_vm(&vm_id).await {
        Ok(vm) => {
            info!(
                vm_id = %vm_id,
                status = ?vm.status,
                "Successfully started VM"
            );
            Ok(Json(vm))
        }
        Err(e) => {
            error!(
                vm_id = %vm_id,
                error = %e,
                "Failed to start VM"
            );
            Err(e.into())
        }
    }
}
```

### Database Operation Logging

```rust
pub async fn create_vm(pool: &SqlitePool, vm: &VmConfig) -> Result<()> {
    debug!(
        vm_id = %vm.id,
        vm_name = %vm.name,
        "Inserting VM into database"
    );

    let start = Instant::now();

    sqlx::query("INSERT INTO vms (...) VALUES (...)")
        .bind(&vm.id)
        .execute(pool)
        .await
        .map_err(|e| {
            error!(
                vm_id = %vm.id,
                error = %e,
                "Failed to insert VM"
            );
            Error::System(e.to_string())
        })?;

    let duration = start.elapsed();
    if duration.as_millis() > 100 {
        warn!(
            vm_id = %vm.id,
            duration_ms = duration.as_millis(),
            "Slow database insert"
        );
    }

    Ok(())
}
```

### Long-Running Operation Logging

```rust
async fn migrate_vm(vm_id: &str, target: &str) -> Result<()> {
    let _span = info_span!(
        "vm_migration",
        vm_id = vm_id,
        target_node = target
    ).entered();

    info!("Starting VM migration");

    // Pre-migration checks
    debug!("Running pre-migration checks");
    run_checks().await?;

    // Start migration
    info!(phase = "start", "Migration started");
    start_migration().await?;

    // Transfer memory
    info!(phase = "memory_transfer", "Transferring memory");
    transfer_memory().await?;

    // Finalize
    info!(phase = "finalize", "Finalizing migration");
    finalize().await?;

    info!("Migration completed successfully");
    Ok(())
}
```

## Log Analysis

### Filtering Logs

```bash
# Show only ERROR logs
RUST_LOG=error cargo run

# Show INFO and above for specific module
RUST_LOG=horcrux_api::vm=info cargo run

# Multiple modules with different levels
RUST_LOG="horcrux_api::vm=debug,horcrux_api::db=trace,info" cargo run
```

### Parsing JSON Logs

```bash
# Pretty print JSON logs
cat /var/log/horcrux/horcrux.log | jq '.'

# Filter by level
cat /var/log/horcrux/horcrux.log | jq 'select(.level == "ERROR")'

# Filter by field
cat /var/log/horcrux/horcrux.log | jq 'select(.fields.vm_id == "vm-100")'

# Show only errors for specific VM
cat /var/log/horcrux/horcrux.log | \
  jq 'select(.level == "ERROR" and .fields.vm_id == "vm-100")'

# Count errors by type
cat /var/log/horcrux/horcrux.log | \
  jq -r 'select(.level == "ERROR") | .fields.error' | \
  sort | uniq -c | sort -rn
```

### Log Aggregation

For production deployments, consider:

- **Loki**: Lightweight log aggregation (Grafana stack)
- **ELK Stack**: Elasticsearch, Logstash, Kibana
- **Fluentd**: Log collection and forwarding
- **Vector**: High-performance observability pipeline

**Example Loki query**:
```logql
{job="horcrux"} | json | level="ERROR" | vm_id="vm-100"
```

## Integration with Monitoring

### Prometheus Metrics from Logs

```rust
// Track errors by VM
static ERROR_COUNTER: Lazy<IntCounterVec> = Lazy::new(|| {
    IntCounterVec::new(
        Opts::new("vm_errors_total", "Total VM errors"),
        &["vm_id", "operation"]
    ).unwrap()
});

error!(
    vm_id = %vm_id,
    operation = "start",
    "Failed to start VM"
);

ERROR_COUNTER.with_label_values(&[&vm_id, "start"]).inc();
```

### Tracing Integration

```rust
// OpenTelemetry integration for distributed tracing
use tracing_subscriber::layer::SubscriberExt;
use tracing_opentelemetry::OpenTelemetryLayer;

let tracer = opentelemetry_jaeger::new_pipeline()
    .with_service_name("horcrux-api")
    .install_batch(opentelemetry::runtime::Tokio)?;

tracing_subscriber::registry()
    .with(OpenTelemetryLayer::new(tracer))
    .with(fmt::layer())
    .init();
```

## Troubleshooting

### No Logs Appearing

```bash
# Check log level
echo $RUST_LOG

# Try explicit level
RUST_LOG=debug cargo run

# Check if logs are going to file
tail -f /var/log/horcrux/horcrux.log
```

### Too Many Logs

```bash
# Reduce verbosity
RUST_LOG=warn cargo run

# Filter specific modules
RUST_LOG="horcrux_api=warn,sqlx=error" cargo run
```

### Performance Impact

- **Console logging**: Minimal impact (buffered)
- **File logging**: Low impact (async, non-blocking)
- **JSON formatting**: ~10-20% overhead vs plain text
- **Recommendation**: Use INFO level in production

### Log Rotation Not Working

```bash
# Check directory permissions
ls -la /var/log/horcrux

# Ensure directory exists
sudo mkdir -p /var/log/horcrux
sudo chown $USER:$USER /var/log/horcrux

# Check disk space
df -h /var/log
```

## Example Log Outputs

### Successful VM Start

```
2025-10-09T10:30:45.123Z  INFO horcrux_api::main: API request method="POST" path="/api/vms/100/start" user="admin"
2025-10-09T10:30:45.145Z  INFO horcrux_api::vm: VM operation operation="start" vm_id="vm-100"
2025-10-09T10:30:45.167Z DEBUG horcrux_api::db: Database operation operation="SELECT" table="vms" record_id="vm-100"
2025-10-09T10:30:45.189Z DEBUG horcrux_api::vm::qemu: Starting QEMU process vm_id="vm-100" command="qemu-system-x86_64 ..."
2025-10-09T10:30:46.234Z  INFO horcrux_api::vm: VM started successfully vm_id="vm-100" pid=12345 uptime_ms=1045
```

### Failed VM Start

```
2025-10-09T10:31:15.123Z  INFO horcrux_api::main: API request method="POST" path="/api/vms/100/start" user="admin"
2025-10-09T10:31:15.145Z  INFO horcrux_api::vm: VM operation operation="start" vm_id="vm-100"
2025-10-09T10:31:15.167Z DEBUG horcrux_api::db: Database operation operation="SELECT" table="vms" record_id="vm-100"
2025-10-09T10:31:15.189Z DEBUG horcrux_api::vm::qemu: Starting QEMU process vm_id="vm-100"
2025-10-09T10:31:15.234Z ERROR horcrux_api::vm::qemu: QEMU process failed vm_id="vm-100" error="Connection refused" stderr="Could not connect to socket"
2025-10-09T10:31:15.256Z ERROR horcrux_api::vm: Failed to start VM vm_id="vm-100" error="QEMU start failed"
2025-10-09T10:31:15.278Z WARN horcrux_api::main: API request failed method="POST" path="/api/vms/100/start" status=500 error="Failed to start VM"
```

---

**Last Updated**: 2025-10-09
**Version**: 1.0
