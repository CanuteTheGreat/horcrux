# Horcrux Real-Time Features Documentation

## Overview

This document describes the real-time monitoring and WebSocket features added to Horcrux, enabling live updates for VM status, system metrics, and operational events.

## 🔄 WebSocket Architecture

### Server-Side Implementation

**Location**: `horcrux-api/src/websocket.rs` (643 lines)

The WebSocket server provides real-time event broadcasting to connected clients using Tokio's async broadcast channels.

**Features:**
- Topic-based subscription model
- Automatic heartbeat/ping every 30 seconds
- Per-user authentication and session management
- Connection timeout protection (10 seconds)
- Graceful disconnect handling

**Event Types Supported:**
- VM status changes (running, stopped, paused)
- VM metrics (CPU, memory, disk I/O, network)
- Node metrics (CPU, memory, disk, load average)
- VM lifecycle events (created, deleted)
- Backup completion events
- Migration progress and completion
- Alert triggers and resolutions
- Container status changes
- Generic notifications

### Client-Side Implementation

**Location**: `horcrux-api/horcrux-ui/src/websocket.rs` (276 lines)

The WebSocket client is built with WASM/Leptos and provides a reactive hook for components.

**Usage Example:**
```rust
use crate::websocket;

#[component]
pub fn MyComponent() -> impl IntoView {
    // Subscribe to VM and node metrics
    let (ws_event, ws_connected) = websocket::use_websocket(vec![
        websocket::TOPIC_VM_METRICS.to_string(),
        websocket::TOPIC_NODE_METRICS.to_string(),
    ]);

    // React to events
    create_effect(move |_| {
        if let Some(event) = ws_event.get() {
            match event {
                websocket::WsEvent::VmMetrics { vm_id, cpu_usage, .. } => {
                    logging::log!("VM {} CPU: {}%", vm_id, cpu_usage);
                }
                _ => {}
            }
        }
    });

    view! {
        <div>
            {move || if ws_connected.get() {
                "Connected ●"
            } else {
                "Disconnected ○"
            }}
        </div>
    }
}
```

### Subscription Topics

| Topic | Description | Data |
|-------|-------------|------|
| `vm:status` | VM status changes | vm_id, old_status, new_status |
| `vm:metrics` | VM performance metrics | cpu_usage, memory_usage, disk_read/write, network_rx/tx |
| `node:metrics` | Node/host metrics | cpu_usage, memory_usage, disk_usage, load_average |
| `vm:events` | VM lifecycle events | vm_id, name, user (created/deleted) |
| `backups` | Backup completion | vm_id, backup_id, size_bytes, duration |
| `migrations` | Migration progress | vm_id, progress, source_node, target_node |
| `alerts` | Alert triggers/resolutions | alert_id, severity, message |
| `notifications` | Generic notifications | level, title, message |

### Connection Flow

1. Client opens WebSocket connection to `ws://localhost:8006/api/ws`
2. Server authenticates user via auth middleware
3. Server sends welcome notification
4. Client sends subscription request with topics list
5. Server confirms subscription
6. Server broadcasts events to subscribed topics
7. Heartbeat pings sent every 30 seconds
8. Client/server can close connection gracefully

**Subscription Request Format:**
```json
{
  "topics": ["vm:status", "vm:metrics", "node:metrics"]
}
```

**Event Format:**
```json
{
  "type": "VmStatusChanged",
  "data": {
    "vm_id": "vm-100",
    "old_status": "stopped",
    "new_status": "running",
    "timestamp": "2025-10-12T10:30:45Z"
  }
}
```

## 📊 Metrics Collection System

### Background Collection Task

**Location**: `horcrux-api/src/metrics_collector.rs` (172 lines)

Automatically collects and broadcasts metrics at configurable intervals:
- **Node metrics**: Every 5 seconds
- **VM metrics**: Every 10 seconds

**Features:**
- Async background tasks using Tokio
- Automatic error recovery
- Only collects metrics for running VMs
- Simulated data (TODO: integrate with libvirt/QEMU monitor)

**Integration Points:**
- Uses `MonitoringManager` for node metrics
- Uses `VmManager` to list VMs
- Broadcasts via `WsState` to all subscribed clients

**Starting the Collector:**
```rust
// In main.rs
metrics_collector::start_metrics_collector(
    state.ws_state.clone(),
    state.monitoring_manager.clone(),
    state.vm_manager.clone(),
);
```

### Node Metrics

**Structure:**
```rust
pub struct NodeMetrics {
    pub hostname: String,
    pub timestamp: i64,
    pub cpu: CpuMetrics,      // usage_percent, cores, load_average
    pub memory: MemoryMetrics, // total_bytes, used_bytes, usage_percent
    pub uptime_seconds: u64,
    pub load_average_1m: f64,
    pub load_average_5m: f64,
    pub load_average_15m: f64,
}
```

**Collection Method:**
- Reads from `/proc/stat` for CPU
- Reads from `/proc/meminfo` for memory
- Reads from `/proc/loadavg` for load averages
- Uses `sysinfo` crate for cross-platform support

### VM Metrics

**Metrics Collected Per VM:**
- CPU usage percentage (0-100)
- Memory usage percentage (0-100)
- Disk read bytes
- Disk write bytes
- Network RX bytes
- Network TX bytes

**Future Enhancement:**
Current implementation uses simulated data. Production implementation should:
- Use libvirt `virDomainGetCPUStats()` for CPU
- Use `/proc/<pid>/status` for memory
- Use `/proc/<pid>/io` for disk I/O
- Use `/sys/class/net/<interface>/statistics/` for network

## 🖥️ Monitoring Dashboard UI

### Dashboard Page

**Location**: `horcrux-api/horcrux-ui/src/pages/monitoring.rs` (381 lines)

Full-featured monitoring dashboard with real-time updates via WebSocket.

**Features:**
- Live connection status indicator
- Real-time node metrics with visual indicators
- Per-VM metrics cards with progress bars
- Color-coded status (Good/Moderate/Warning/Critical)
- Load average display (1min, 5min, 15min)
- Byte formatting (B/KB/MB/GB)
- Placeholder for future performance trend graphs

**UI Components:**

1. **MetricCard** - Large metric display with progress bar
   - Color-coded based on thresholds (75% warning, 90% critical)
   - Icon representation
   - Percentage and status text

2. **LoadAverageCard** - Load average visualization
   - Three time intervals (1min, 5min, 15min)
   - Formatted to 2 decimal places

3. **VmMetricsCard** - Individual VM metrics
   - VM ID and timestamp
   - 6 metrics in grid layout
   - Mini progress bars for CPU/Memory
   - Byte-formatted disk and network stats

**Navigation:**
- Added to main navbar: `/monitoring`
- Icon: 📊 Monitoring
- Real-time connection badge

### Metrics Thresholds

| Metric | Good | Moderate | Warning | Critical |
|--------|------|----------|---------|----------|
| CPU    | 0-50% | 50-75% | 75-90% | >90% |
| Memory | 0-50% | 50-75% | 75-90% | >90% |
| Disk   | 0-50% | 50-75% | 75-90% | >90% |

### Future Enhancements

**Performance Trends Section (Placeholder):**
- Real-time line charts for CPU, Memory, Disk I/O, Network
- Historical data storage (last 24 hours)
- Zoom and pan controls
- Export to CSV/JSON

**Recommended Libraries:**
- **Plotly.js** - Interactive charts with zoom/pan
- **Chart.js** - Lightweight charting
- **Apache ECharts** - Advanced visualizations

## 🔐 Authentication & RBAC

### JWT Authentication

**Already Implemented:**
- `horcrux-api/src/auth/mod.rs` - Complete auth manager
- JWT token generation with `jsonwebtoken` crate
- Argon2 password hashing
- Session management with CSRF tokens

**Features:**
- PAM authentication support
- LDAP authentication support
- OIDC authentication support
- API token generation
- Session expiry and renewal

### Role-Based Access Control (RBAC)

**Location**: `horcrux-api/src/middleware/rbac.rs` (218 lines)

Complete RBAC implementation with predefined roles.

**Built-in Roles:**

1. **Administrator** - Full system access
   - All VM privileges (allocate, config, power, migrate, snapshot, backup, audit)
   - All datastore privileges
   - System modification and auditing
   - User management
   - Pool allocation

2. **VmAdmin** - VM management only
   - VM allocation, configuration, power management
   - Snapshot and backup operations
   - VM auditing
   - Restricted to `/api/vms/**` paths

3. **VmUser** - Basic VM operations
   - VM power management (start/stop/restart)
   - VM console access
   - View-only privileges

4. **StorageAdmin** - Storage management
   - Datastore allocation and auditing
   - Pool management
   - Restricted to `/api/storage/**` paths

5. **Auditor** - Read-only access
   - VM auditing
   - Datastore auditing
   - System auditing
   - Full API read access (`/api/**`)

**Privilege Types:**
```rust
pub enum Privilege {
    VmAllocate,        // Create/destroy VMs
    VmConfig,          // Modify VM configuration
    VmPowerMgmt,       // Start/stop/restart VMs
    VmMigrate,         // Live migration
    VmSnapshot,        // Snapshot operations
    VmBackup,          // Backup operations
    VmConsole,         // Console access
    VmAudit,           // View VM details
    DatastoreAllocate, // Manage datastores
    DatastoreAudit,    // View datastore info
    SysModify,         // Modify system settings
    SysAudit,          // View system info
    UserModify,        // User management
    PermissionsModify, // RBAC management
    PoolAllocate,      // Resource pool management
}
```

**Usage in API Handlers:**
```rust
use crate::middleware::auth::AuthUser;
use horcrux_common::auth::Privilege;

async fn create_vm(
    Extension(auth_user): Extension<AuthUser>,
    State(state): State<Arc<AppState>>,
    Json(vm_config): Json<VmConfig>,
) -> Result<Json<VmConfig>, ApiError> {
    // Check RBAC permission
    require_privilege!(state, auth_user, "/api/vms", Privilege::VmAllocate)?;

    // ... create VM logic
}
```

### User Management APIs

**Endpoints Already Implemented:**
```
GET    /api/users                    - List all users
POST   /api/users                    - Create user
DELETE /api/users/:id                - Delete user
GET    /api/users/:username/api-keys - List API keys
POST   /api/users/:username/api-keys - Create API key
DELETE /api/users/:username/api-keys/:key_id - Revoke API key
GET    /api/roles                    - List all roles
```

**User Creation Example:**
```bash
curl -X POST http://localhost:8006/api/users \
  -H "Content-Type: application/json" \
  -d '{
    "username": "alice",
    "password": "secure_password",
    "email": "alice@example.com",
    "realm": "pam",
    "roles": ["VmUser"],
    "enabled": true
  }'
```

## 📈 Statistics

### Code Additions

| Component | Lines | Description |
|-----------|-------|-------------|
| **WebSocket Server** | 643 | Complete event broadcasting system |
| **WebSocket Client** | 276 | WASM client with reactive hooks |
| **Metrics Collector** | 172 | Background task for metrics collection |
| **Monitoring Dashboard** | 381 | Real-time metrics UI page |
| **RBAC Middleware** | 218 | Already implemented |
| **Auth System** | 358 | Already implemented |
| **Total New Code** | **2,048** | Real-time features and monitoring |

### Features Summary

✅ **Real-Time Features:**
- WebSocket server with topic-based subscriptions
- WebSocket client with Leptos integration
- Automatic heartbeat and connection management
- Event broadcasting for all major operations

✅ **Monitoring:**
- Background metrics collection (5s/10s intervals)
- Node metrics (CPU, memory, disk, load)
- VM metrics (CPU, memory, disk I/O, network)
- Real-time dashboard with live updates

✅ **Security:**
- JWT authentication with multiple realms (PAM, LDAP, OIDC)
- Argon2 password hashing
- RBAC with 5 predefined roles
- API token support
- Session management with CSRF protection

✅ **User Management:**
- User CRUD operations
- Role assignment
- API key generation and revocation
- Permission management

## 🚀 Getting Started

### 1. Start the Server

```bash
cd horcrux/horcrux-api
cargo run --release
```

The server will:
- Start WebSocket endpoint at `ws://localhost:8006/api/ws`
- Launch metrics collection background tasks
- Serve UI at `http://localhost:8006`

### 2. Access the Monitoring Dashboard

Open browser and navigate to:
```
http://localhost:8006/monitoring
```

You should see:
- Real-time connection status indicator
- Live node metrics updating every 5 seconds
- VM metrics for all running VMs (every 10 seconds)

### 3. Test WebSocket Connection

Using `wscat` or browser console:
```bash
wscat -c ws://localhost:8006/api/ws

# Send subscription
> {"topics": ["vm:metrics", "node:metrics"]}

# Receive events
< {"type":"Subscribed","data":{"topics":["vm:metrics","node:metrics"],"timestamp":"2025-10-12T10:30:00Z"}}
< {"type":"NodeMetrics","data":{"hostname":"node1","cpu_usage":45.5,"memory_usage":60.2,...}}
< {"type":"VmMetrics","data":{"vm_id":"vm-100","cpu_usage":25.3,"memory_usage":45.8,...}}
```

### 4. Create Users and Assign Roles

```bash
# Create admin user
curl -X POST http://localhost:8006/api/users \
  -H "Content-Type: application/json" \
  -d '{
    "username": "admin",
    "password": "admin123",
    "email": "admin@horcrux.local",
    "realm": "pam",
    "roles": ["Administrator"],
    "enabled": true
  }'

# Create VM user
curl -X POST http://localhost:8006/api/users \
  -H "Content-Type: application/json" \
  -d '{
    "username": "vmuser",
    "password": "user123",
    "email": "user@horcrux.local",
    "realm": "pam",
    "roles": ["VmUser"],
    "enabled": true
  }'
```

## 🔧 Configuration

### Metrics Collection Intervals

Edit `horcrux-api/src/metrics_collector.rs`:
```rust
const NODE_METRICS_INTERVAL_SECS: u64 = 5;  // Node metrics every 5 seconds
const VM_METRICS_INTERVAL_SECS: u64 = 10;   // VM metrics every 10 seconds
```

### WebSocket Heartbeat

Edit `horcrux-api/src/websocket.rs`:
```rust
// Heartbeat interval (30 seconds)
let mut heartbeat_interval = tokio::time::interval(
    tokio::time::Duration::from_secs(30)
);
```

### Subscription Timeout

Edit `horcrux-api/src/websocket.rs`:
```rust
_ = tokio::time::sleep(tokio::time::Duration::from_secs(10)) => {
    error!("Timeout waiting for subscription");
}
```

## 📝 Next Steps

### Recommended Enhancements

1. **Real-Time Graphs**
   - Integrate charting library (Plotly.js or Chart.js)
   - Store historical metrics in database
   - Add zoom, pan, and time range selectors
   - Export functionality

2. **Advanced Alerting**
   - Threshold-based alerts
   - Email/SMS notifications
   - Alert history and acknowledgment
   - Custom alert rules

3. **Metric Storage**
   - Time-series database integration (InfluxDB or TimescaleDB)
   - Retention policies
   - Metric aggregation
   - Historical data queries

4. **WebSocket Enhancements**
   - Message compression
   - Binary protocol support (MessagePack)
   - Reconnection with exponential backoff
   - Offline message queue

5. **User Management UI**
   - User list page
   - User creation/edit forms
   - Role assignment interface
   - API key management

6. **Real VM Metrics**
   - Integrate with libvirt API
   - QEMU monitor protocol
   - Direct `/proc` filesystem reads
   - Container metrics via cgroups

## 🎯 Testing

### Unit Tests

```bash
# Test metrics collector
cargo test -p horcrux-api metrics_collector::tests

# Test WebSocket events
cargo test -p horcrux-api websocket::tests

# Test RBAC
cargo test -p horcrux-api rbac::tests
```

### Integration Testing

1. Start server
2. Open browser console
3. Connect to WebSocket
4. Monitor real-time events
5. Verify authentication and RBAC

### Load Testing

Use `websocat` or custom tool to simulate multiple connections:
```bash
# Simulate 100 concurrent WebSocket connections
for i in {1..100}; do
  websocat ws://localhost:8006/api/ws &
done
```

## 📚 References

- **WebSocket RFC**: https://tools.ietf.org/html/rfc6455
- **JWT Specification**: https://jwt.io/introduction
- **Argon2**: https://github.com/P-H-C/phc-winner-argon2
- **Tokio**: https://tokio.rs/
- **Leptos**: https://leptos.dev/
- **RBAC Design Patterns**: https://en.wikipedia.org/wiki/Role-based_access_control

---

**Made with ❤️ for the Gentoo community**

Last Updated: 2025-10-12
