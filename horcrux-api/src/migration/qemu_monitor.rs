///! QEMU Monitor Protocol (QMP) Integration
///!
///! Provides real-time monitoring and control of QEMU VMs via QMP
///! Used for tracking migration progress, memory transfer, and VM state

use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tracing::{debug, info, warn};

/// QMP command request
#[derive(Debug, Serialize)]
struct QmpCommand {
    execute: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    arguments: Option<serde_json::Value>,
}

/// QMP response
#[derive(Debug, Deserialize)]
struct QmpResponse {
    #[serde(rename = "return")]
    return_value: Option<serde_json::Value>,
    error: Option<QmpError>,
}

/// QMP error
#[derive(Debug, Deserialize)]
struct QmpError {
    class: String,
    desc: String,
}

/// Migration status from QMP
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationStatus {
    pub status: MigrationState,
    pub total_time_ms: u64,
    pub downtime_ms: u64,
    pub setup_time_ms: u64,
    pub ram_transferred_mb: u64,
    pub ram_remaining_mb: u64,
    pub ram_total_mb: u64,
    pub ram_duplicate_mb: u64,
    pub ram_normal_mb: u64,
    pub ram_normal_bytes: u64,
    pub dirty_pages_rate: u64,
    pub mbps: f64,
    pub dirty_sync_count: u64,
    pub page_size_kb: u64,
    pub expected_downtime_ms: u64,
}

/// Migration state from QMP
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MigrationState {
    None,
    Setup,
    Active,
    PreSwitchover,
    DeviceTransfer,
    PostCopy,
    Completed,
    Failed,
    Cancelling,
    Cancelled,
    Wait,
}

impl std::fmt::Display for MigrationState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MigrationState::None => write!(f, "none"),
            MigrationState::Setup => write!(f, "setup"),
            MigrationState::Active => write!(f, "active"),
            MigrationState::PreSwitchover => write!(f, "pre-switchover"),
            MigrationState::DeviceTransfer => write!(f, "device"),
            MigrationState::PostCopy => write!(f, "postcopy-active"),
            MigrationState::Completed => write!(f, "completed"),
            MigrationState::Failed => write!(f, "failed"),
            MigrationState::Cancelling => write!(f, "cancelling"),
            MigrationState::Cancelled => write!(f, "cancelled"),
            MigrationState::Wait => write!(f, "wait-unplug"),
        }
    }
}

/// VM information from QMP
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmInfo {
    pub name: String,
    pub running: bool,
    pub status: String,
    pub singlestep: bool,
}

/// QEMU Monitor client
pub struct QemuMonitor {
    socket_path: PathBuf,
}

impl QemuMonitor {
    /// Create a new QEMU monitor client
    pub fn new(socket_path: PathBuf) -> Self {
        Self { socket_path }
    }

    /// Connect to QEMU monitor socket
    async fn connect(&self) -> Result<UnixStream> {
        debug!("Connecting to QEMU monitor at {:?}", self.socket_path);

        let stream = UnixStream::connect(&self.socket_path).await
            .map_err(|e| horcrux_common::Error::System(
                format!("Failed to connect to QEMU monitor: {}", e)
            ))?;

        Ok(stream)
    }

    /// Execute a QMP command
    async fn execute_command(
        &self,
        command: &str,
        arguments: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let stream = self.connect().await?;

        // Split stream into read and write halves
        let (read_half, mut write_half) = stream.into_split();
        let mut reader = BufReader::new(read_half);

        // Read and discard the initial QMP greeting
        let mut greeting = String::new();
        reader.read_line(&mut greeting).await
            .map_err(|e| horcrux_common::Error::System(
                format!("Failed to read QMP greeting: {}", e)
            ))?;

        debug!("QMP greeting: {}", greeting.trim());

        // Enter command mode
        let qmp_cmd = QmpCommand {
            execute: "qmp_capabilities".to_string(),
            arguments: None,
        };

        let cmd_json = serde_json::to_string(&qmp_cmd)
            .map_err(|e| horcrux_common::Error::System(
                format!("Failed to serialize QMP command: {}", e)
            ))?;

        write_half.write_all(cmd_json.as_bytes()).await
            .map_err(|e| horcrux_common::Error::System(
                format!("Failed to write QMP command: {}", e)
            ))?;
        write_half.write_all(b"\n").await
            .map_err(|e| horcrux_common::Error::System(
                format!("Failed to write newline: {}", e)
            ))?;

        // Read response
        let mut response = String::new();
        reader.read_line(&mut response).await
            .map_err(|e| horcrux_common::Error::System(
                format!("Failed to read QMP response: {}", e)
            ))?;

        debug!("QMP capabilities response: {}", response.trim());

        // Now execute the actual command
        let actual_cmd = QmpCommand {
            execute: command.to_string(),
            arguments,
        };

        let cmd_json = serde_json::to_string(&actual_cmd)
            .map_err(|e| horcrux_common::Error::System(
                format!("Failed to serialize QMP command: {}", e)
            ))?;

        write_half.write_all(cmd_json.as_bytes()).await
            .map_err(|e| horcrux_common::Error::System(
                format!("Failed to write QMP command: {}", e)
            ))?;
        write_half.write_all(b"\n").await
            .map_err(|e| horcrux_common::Error::System(
                format!("Failed to write newline: {}", e)
            ))?;

        // Read command response
        let mut response = String::new();
        reader.read_line(&mut response).await
            .map_err(|e| horcrux_common::Error::System(
                format!("Failed to read QMP response: {}", e)
            ))?;

        debug!("QMP command response: {}", response.trim());

        // Parse response
        let qmp_response: QmpResponse = serde_json::from_str(&response)
            .map_err(|e| horcrux_common::Error::System(
                format!("Failed to parse QMP response: {}", e)
            ))?;

        if let Some(error) = qmp_response.error {
            return Err(horcrux_common::Error::System(
                format!("QMP error: {} - {}", error.class, error.desc)
            ));
        }

        Ok(qmp_response.return_value.unwrap_or(serde_json::Value::Null))
    }

    /// Query migration status
    pub async fn query_migrate(&self) -> Result<MigrationStatus> {
        debug!("Querying migration status");

        let response = self.execute_command("query-migrate", None).await?;

        // Parse the migration status from the response
        self.parse_migration_status(response)
    }

    /// Parse migration status from QMP response
    fn parse_migration_status(&self, value: serde_json::Value) -> Result<MigrationStatus> {
        // Extract status string
        let status_str = value.get("status")
            .and_then(|v| v.as_str())
            .ok_or_else(|| horcrux_common::Error::System(
                "Missing migration status".to_string()
            ))?;

        let status = match status_str {
            "none" => MigrationState::None,
            "setup" => MigrationState::Setup,
            "active" => MigrationState::Active,
            "pre-switchover" => MigrationState::PreSwitchover,
            "device" => MigrationState::DeviceTransfer,
            "postcopy-active" => MigrationState::PostCopy,
            "completed" => MigrationState::Completed,
            "failed" => MigrationState::Failed,
            "cancelling" => MigrationState::Cancelling,
            "cancelled" => MigrationState::Cancelled,
            "wait-unplug" => MigrationState::Wait,
            _ => {
                warn!("Unknown migration status: {}", status_str);
                MigrationState::None
            }
        };

        // Extract RAM statistics
        let ram = value.get("ram").unwrap_or(&serde_json::Value::Null);

        let ram_transferred = ram.get("transferred")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) / (1024 * 1024); // Convert to MB

        let ram_remaining = ram.get("remaining")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) / (1024 * 1024);

        let ram_total = ram.get("total")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) / (1024 * 1024);

        let ram_duplicate = ram.get("duplicate")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) / (1024 * 1024);

        let ram_normal = ram.get("normal")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) / (1024 * 1024);

        let ram_normal_bytes = ram.get("normal-bytes")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let dirty_pages_rate = ram.get("dirty-pages-rate")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let mbps = ram.get("mbps")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        let dirty_sync_count = ram.get("dirty-sync-count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let page_size = ram.get("page-size")
            .and_then(|v| v.as_u64())
            .unwrap_or(4096) / 1024; // Convert to KB

        // Extract timing information
        let total_time = value.get("total-time")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let downtime = value.get("downtime")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let setup_time = value.get("setup-time")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let expected_downtime = value.get("expected-downtime")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        Ok(MigrationStatus {
            status,
            total_time_ms: total_time,
            downtime_ms: downtime,
            setup_time_ms: setup_time,
            ram_transferred_mb: ram_transferred,
            ram_remaining_mb: ram_remaining,
            ram_total_mb: ram_total,
            ram_duplicate_mb: ram_duplicate,
            ram_normal_mb: ram_normal,
            ram_normal_bytes,
            dirty_pages_rate,
            mbps,
            dirty_sync_count,
            page_size_kb: page_size,
            expected_downtime_ms: expected_downtime,
        })
    }

    /// Query VM status
    pub async fn query_status(&self) -> Result<VmInfo> {
        debug!("Querying VM status");

        let response = self.execute_command("query-status", None).await?;

        let status = response.get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let running = response.get("running")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let singlestep = response.get("singlestep")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        Ok(VmInfo {
            name: "unknown".to_string(), // Would need query-name command
            running,
            status,
            singlestep,
        })
    }

    /// Start migration via QMP
    pub async fn migrate(&self, uri: &str, incremental: bool, blk: bool, detach: bool) -> Result<()> {
        info!("Starting migration to {}", uri);

        let mut args = serde_json::Map::new();
        args.insert("uri".to_string(), serde_json::Value::String(uri.to_string()));

        if incremental {
            args.insert("inc".to_string(), serde_json::Value::Bool(true));
        }

        if blk {
            args.insert("blk".to_string(), serde_json::Value::Bool(true));
        }

        if detach {
            args.insert("detach".to_string(), serde_json::Value::Bool(true));
        }

        self.execute_command("migrate", Some(serde_json::Value::Object(args))).await?;

        info!("Migration command sent successfully");
        Ok(())
    }

    /// Cancel ongoing migration
    pub async fn migrate_cancel(&self) -> Result<()> {
        info!("Cancelling migration");

        self.execute_command("migrate_cancel", None).await?;

        info!("Migration cancellation requested");
        Ok(())
    }

    /// Set migration downtime limit (in seconds)
    pub async fn migrate_set_downtime(&self, downtime_secs: f64) -> Result<()> {
        info!("Setting migration downtime limit to {} seconds", downtime_secs);

        let mut args = serde_json::Map::new();
        args.insert("value".to_string(), serde_json::Value::from(downtime_secs));

        self.execute_command("migrate-set-parameters", Some(serde_json::Value::Object(args))).await?;

        Ok(())
    }

    /// Set migration speed limit (in bytes/sec)
    pub async fn migrate_set_speed(&self, speed_bytes_per_sec: u64) -> Result<()> {
        info!("Setting migration speed limit to {} bytes/sec", speed_bytes_per_sec);

        let mut args = serde_json::Map::new();
        args.insert("value".to_string(), serde_json::Value::from(speed_bytes_per_sec));

        self.execute_command("migrate_set_speed", Some(serde_json::Value::Object(args))).await?;

        Ok(())
    }

    /// Calculate migration progress percentage
    pub fn calculate_progress(status: &MigrationStatus) -> f32 {
        if status.ram_total_mb == 0 {
            return 0.0;
        }

        let progress = (status.ram_transferred_mb as f64 / status.ram_total_mb as f64) * 100.0;
        progress.min(100.0) as f32
    }

    /// Check if migration is complete
    pub fn is_migration_complete(status: &MigrationStatus) -> bool {
        matches!(
            status.status,
            MigrationState::Completed | MigrationState::Failed | MigrationState::Cancelled
        )
    }

    /// Check if migration failed
    pub fn is_migration_failed(status: &MigrationStatus) -> bool {
        status.status == MigrationState::Failed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_migration_state_display() {
        assert_eq!(MigrationState::Active.to_string(), "active");
        assert_eq!(MigrationState::Completed.to_string(), "completed");
        assert_eq!(MigrationState::Failed.to_string(), "failed");
    }

    #[test]
    fn test_calculate_progress() {
        let status = MigrationStatus {
            status: MigrationState::Active,
            total_time_ms: 10000,
            downtime_ms: 0,
            setup_time_ms: 100,
            ram_transferred_mb: 500,
            ram_remaining_mb: 500,
            ram_total_mb: 1000,
            ram_duplicate_mb: 0,
            ram_normal_mb: 500,
            ram_normal_bytes: 500 * 1024 * 1024,
            dirty_pages_rate: 100,
            mbps: 50.0,
            dirty_sync_count: 5,
            page_size_kb: 4,
            expected_downtime_ms: 100,
        };

        let progress = QemuMonitor::calculate_progress(&status);
        assert_eq!(progress, 50.0);
    }

    #[test]
    fn test_is_migration_complete() {
        let completed_status = MigrationStatus {
            status: MigrationState::Completed,
            total_time_ms: 10000,
            downtime_ms: 100,
            setup_time_ms: 100,
            ram_transferred_mb: 1000,
            ram_remaining_mb: 0,
            ram_total_mb: 1000,
            ram_duplicate_mb: 0,
            ram_normal_mb: 1000,
            ram_normal_bytes: 1000 * 1024 * 1024,
            dirty_pages_rate: 0,
            mbps: 0.0,
            dirty_sync_count: 10,
            page_size_kb: 4,
            expected_downtime_ms: 0,
        };

        assert!(QemuMonitor::is_migration_complete(&completed_status));
        assert!(!QemuMonitor::is_migration_failed(&completed_status));
    }

    #[test]
    fn test_migration_status_parse() {
        let monitor = QemuMonitor::new(PathBuf::from("/tmp/test.sock"));

        let json_response = serde_json::json!({
            "status": "active",
            "total-time": 5000,
            "setup-time": 100,
            "expected-downtime": 50,
            "ram": {
                "transferred": 524288000,
                "remaining": 524288000,
                "total": 1048576000,
                "duplicate": 0,
                "normal": 500,
                "normal-bytes": 524288000,
                "dirty-pages-rate": 100,
                "mbps": 50.5,
                "dirty-sync-count": 3,
                "page-size": 4096
            }
        });

        let status = monitor.parse_migration_status(json_response).unwrap();

        assert_eq!(status.status, MigrationState::Active);
        assert_eq!(status.total_time_ms, 5000);
        assert_eq!(status.ram_transferred_mb, 500);
        assert_eq!(status.ram_total_mb, 1000);
        assert_eq!(status.mbps, 50.5);
    }
}
