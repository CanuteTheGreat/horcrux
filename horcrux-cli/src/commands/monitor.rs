use crate::api::ApiClient;
use crate::output::{OutputFormat, format_bytes, format_duration};
use crate::MonitorCommands;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use tabled::Tabled;

#[derive(Debug, Serialize, Deserialize)]
struct NodeMetrics {
    cpu_usage: f64,
    memory_usage: f64,
    memory_total: u64,
    disk_usage: f64,
    uptime: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct VmMetrics {
    vm_id: String,
    vm_name: String,
    cpu_usage: f64,
    memory_usage: u64,
    disk_read: u64,
    disk_write: u64,
    net_in: u64,
    net_out: u64,
}

#[derive(Tabled, Serialize)]
struct VmMetricsRow {
    vm_id: String,
    name: String,
    #[tabled(rename = "cpu %")]
    cpu_usage: String,
    memory: String,
    #[tabled(rename = "disk r/w")]
    disk_rw: String,
    #[tabled(rename = "net i/o")]
    net_io: String,
}

impl From<VmMetrics> for VmMetricsRow {
    fn from(m: VmMetrics) -> Self {
        Self {
            vm_id: m.vm_id,
            name: m.vm_name,
            cpu_usage: format!("{:.1}%", m.cpu_usage),
            memory: format_bytes(m.memory_usage),
            disk_rw: format!("{}/{}", format_bytes(m.disk_read), format_bytes(m.disk_write)),
            net_io: format!("{}/{}", format_bytes(m.net_in), format_bytes(m.net_out)),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct StorageMetrics {
    name: String,
    used: u64,
    available: u64,
    total: u64,
    usage_percent: f64,
}

#[derive(Tabled, Serialize)]
struct StorageMetricsRow {
    name: String,
    used: String,
    available: String,
    total: String,
    #[tabled(rename = "usage %")]
    usage_percent: String,
}

impl From<StorageMetrics> for StorageMetricsRow {
    fn from(m: StorageMetrics) -> Self {
        Self {
            name: m.name,
            used: format_bytes(m.used * 1024 * 1024 * 1024),
            available: format_bytes(m.available * 1024 * 1024 * 1024),
            total: format_bytes(m.total * 1024 * 1024 * 1024),
            usage_percent: format!("{:.1}%", m.usage_percent),
        }
    }
}

pub async fn handle_monitor_command(
    command: MonitorCommands,
    api: &ApiClient,
    output_format: &str,
) -> Result<()> {
    match command {
        MonitorCommands::Node => {
            let metrics: NodeMetrics = api.get("/api/monitoring/node").await?;
            let format = OutputFormat::from_str(output_format);

            if format == OutputFormat::Table {
                println!("Node Metrics:");
                println!("  CPU Usage:    {:.1}%", metrics.cpu_usage);
                println!("  Memory Usage: {:.1}%", metrics.memory_usage);
                println!("  Memory Total: {}", format_bytes(metrics.memory_total * 1024 * 1024 * 1024));
                println!("  Disk Usage:   {:.1}%", metrics.disk_usage);
                println!("  Uptime:       {}", format_duration(metrics.uptime));
            } else {
                crate::output::print_single(&metrics, format)?;
            }
        }
        MonitorCommands::Vm { id } => {
            let metrics: Vec<VmMetrics> = if let Some(vm_id) = id {
                vec![api.get(&format!("/api/monitoring/vms/{}", vm_id)).await?]
            } else {
                api.get("/api/monitoring/vms").await?
            };
            let format = OutputFormat::from_str(output_format);
            let rows: Vec<VmMetricsRow> = metrics.into_iter().map(VmMetricsRow::from).collect();
            crate::output::print_output(rows, format)?;
        }
        MonitorCommands::Storage { name } => {
            let metrics: Vec<StorageMetrics> = if let Some(pool_name) = name {
                vec![api.get(&format!("/api/monitoring/storage/{}", pool_name)).await?]
            } else {
                api.get("/api/monitoring/storage").await?
            };
            let format = OutputFormat::from_str(output_format);
            let rows: Vec<StorageMetricsRow> = metrics.into_iter().map(StorageMetricsRow::from).collect();
            crate::output::print_output(rows, format)?;
        }
        MonitorCommands::Cluster => {
            let metrics: NodeMetrics = api.get("/api/monitoring/cluster").await?;
            let format = OutputFormat::from_str(output_format);

            if format == OutputFormat::Table {
                println!("Cluster Metrics:");
                println!("  CPU Usage:    {:.1}%", metrics.cpu_usage);
                println!("  Memory Usage: {:.1}%", metrics.memory_usage);
                println!("  Memory Total: {}", format_bytes(metrics.memory_total * 1024 * 1024 * 1024));
                println!("  Disk Usage:   {:.1}%", metrics.disk_usage);
            } else {
                crate::output::print_single(&metrics, format)?;
            }
        }
        MonitorCommands::Watch { interval } => {
            use std::time::Duration;
            use std::io::Write;

            println!("Watching metrics (press Ctrl+C to stop)...\n");

            loop {
                // Clear screen (ANSI escape code)
                print!("\x1B[2J\x1B[1;1H");
                std::io::stdout().flush()?;

                // Fetch and display node metrics
                let metrics: NodeMetrics = api.get("/api/monitoring/node").await?;
                println!("Node Metrics (refreshing every {}s):", interval);
                println!("  CPU Usage:    {:.1}%", metrics.cpu_usage);
                println!("  Memory Usage: {:.1}%", metrics.memory_usage);
                println!("  Disk Usage:   {:.1}%", metrics.disk_usage);

                tokio::time::sleep(Duration::from_secs(interval)).await;
            }
        }
    }
    Ok(())
}
