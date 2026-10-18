use crate::api::ApiClient;
use crate::output::{format_bytes, format_duration, OutputFormat};
use crate::MonitorCommands;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use tabled::Tabled;

// Wire types: the API's `monitoring::{NodeMetrics, ResourceMetrics,
// StorageMetrics}` (docs/API.md). The CLI used to expect an older flat shape
// ({vm_id, cpu_usage, ...}), so every `monitor` call failed to decode.

#[derive(Debug, Serialize, Deserialize)]
pub struct CpuMetrics {
    pub usage_percent: f64,
    #[serde(default)]
    pub cores: u32,
    #[serde(default)]
    pub load_average: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MemoryMetrics {
    pub total_bytes: u64,
    pub used_bytes: u64,
    #[serde(default)]
    pub free_bytes: u64,
    pub usage_percent: f64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct DiskMetrics {
    #[serde(default)]
    pub read_bytes_per_sec: u64,
    #[serde(default)]
    pub write_bytes_per_sec: u64,
    #[serde(default)]
    pub total_bytes: u64,
    #[serde(default)]
    pub used_bytes: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct NetworkMetrics {
    #[serde(default)]
    pub rx_bytes_per_sec: u64,
    #[serde(default)]
    pub tx_bytes_per_sec: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NodeMetrics {
    pub hostname: String,
    pub cpu: CpuMetrics,
    pub memory: MemoryMetrics,
    pub uptime_seconds: u64,
    #[serde(default)]
    pub load_average_1m: f64,
    #[serde(default)]
    pub load_average_5m: f64,
    #[serde(default)]
    pub load_average_15m: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ResourceMetrics {
    pub id: String,
    pub name: String,
    pub cpu: CpuMetrics,
    pub memory: MemoryMetrics,
    #[serde(default)]
    pub disk: DiskMetrics,
    #[serde(default)]
    pub network: NetworkMetrics,
}

#[derive(Tabled, Serialize)]
struct VmMetricsRow {
    vm_id: String,
    name: String,
    #[tabled(rename = "cpu %")]
    cpu_usage: String,
    memory: String,
    disk: String,
    #[tabled(rename = "disk r/w per s")]
    disk_rw: String,
    #[tabled(rename = "net rx/tx per s")]
    net_io: String,
}

impl From<ResourceMetrics> for VmMetricsRow {
    fn from(m: ResourceMetrics) -> Self {
        Self {
            vm_id: m.id,
            name: m.name,
            cpu_usage: format!("{:.1}%", m.cpu.usage_percent),
            memory: format!(
                "{} / {}",
                format_bytes(m.memory.used_bytes),
                format_bytes(m.memory.total_bytes)
            ),
            disk: format!(
                "{} / {}",
                format_bytes(m.disk.used_bytes),
                format_bytes(m.disk.total_bytes)
            ),
            disk_rw: format!(
                "{}/{}",
                format_bytes(m.disk.read_bytes_per_sec),
                format_bytes(m.disk.write_bytes_per_sec)
            ),
            net_io: format!(
                "{}/{}",
                format_bytes(m.network.rx_bytes_per_sec),
                format_bytes(m.network.tx_bytes_per_sec)
            ),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StorageMetrics {
    pub name: String,
    #[serde(default)]
    pub storage_type: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub usage_percent: f64,
}

#[derive(Tabled, Serialize)]
struct StorageMetricsRow {
    name: String,
    #[tabled(rename = "type")]
    storage_type: String,
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
            storage_type: m.storage_type,
            used: format_bytes(m.used_bytes),
            available: format_bytes(m.available_bytes),
            total: format_bytes(m.total_bytes),
            usage_percent: format!("{:.1}%", m.usage_percent),
        }
    }
}

/// A cluster node as `GET /api/cluster/nodes` returns it (the fields this
/// command shows; live usage is `null` when unknown).
#[derive(Debug, Serialize, Deserialize)]
pub struct ClusterNode {
    pub name: String,
    #[serde(default)]
    pub ip: String,
    pub status: serde_json::Value,
    #[serde(default)]
    pub cpu_cores: u32,
    #[serde(default)]
    pub memory_total: u64,
    #[serde(default)]
    pub cpu_usage: Option<f64>,
    #[serde(default)]
    pub memory_used: Option<u64>,
}

#[derive(Tabled, Serialize)]
struct ClusterNodeRow {
    name: String,
    ip: String,
    status: String,
    #[tabled(rename = "cpu %")]
    cpu: String,
    memory: String,
}

impl From<&ClusterNode> for ClusterNodeRow {
    fn from(n: &ClusterNode) -> Self {
        Self {
            name: n.name.clone(),
            ip: n.ip.clone(),
            status: match &n.status {
                serde_json::Value::String(s) => s.to_lowercase(),
                other => other.to_string(),
            },
            cpu: n
                .cpu_usage
                .map(|c| format!("{c:.1}% of {}", n.cpu_cores))
                .unwrap_or_else(|| "unknown".into()),
            memory: n
                .memory_used
                .map(|u| format!("{} / {}", format_bytes(u), format_bytes(n.memory_total)))
                .unwrap_or_else(|| format!("? / {}", format_bytes(n.memory_total))),
        }
    }
}

fn print_node(metrics: &NodeMetrics, title: &str) {
    println!("{title}:");
    println!("  Host:         {}", metrics.hostname);
    println!(
        "  CPU Usage:    {:.1}% ({} cores)",
        metrics.cpu.usage_percent, metrics.cpu.cores
    );
    println!(
        "  Memory:       {:.1}% ({} / {})",
        metrics.memory.usage_percent,
        format_bytes(metrics.memory.used_bytes),
        format_bytes(metrics.memory.total_bytes)
    );
    println!(
        "  Load:         {:.2} {:.2} {:.2}",
        metrics.load_average_1m, metrics.load_average_5m, metrics.load_average_15m
    );
    println!(
        "  Uptime:       {}",
        format_duration(metrics.uptime_seconds)
    );
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
                print_node(&metrics, "Node Metrics");
            } else {
                crate::output::print_single(&metrics, format)?;
            }
        }
        MonitorCommands::Vm { id } => {
            let metrics: Vec<ResourceMetrics> = if let Some(vm_id) = id {
                // `null` = the VM isn't running (no live metrics).
                let one: Option<ResourceMetrics> =
                    api.get(&format!("/api/monitoring/vms/{}", vm_id)).await?;
                match one {
                    Some(m) => vec![m],
                    None => anyhow::bail!("VM {vm_id} is not running: no live metrics"),
                }
            } else {
                api.get("/api/monitoring/vms").await?
            };
            let format = OutputFormat::from_str(output_format);
            let rows: Vec<VmMetricsRow> = metrics.into_iter().map(VmMetricsRow::from).collect();
            crate::output::print_output(rows, format)?;
        }
        MonitorCommands::Storage { name } => {
            let metrics: Vec<StorageMetrics> = if let Some(pool_name) = name {
                let one: Option<StorageMetrics> = api
                    .get(&format!("/api/monitoring/storage/{}", pool_name))
                    .await?;
                match one {
                    Some(m) => vec![m],
                    None => anyhow::bail!("No metrics for storage pool {pool_name}"),
                }
            } else {
                api.get("/api/monitoring/storage").await?
            };
            let format = OutputFormat::from_str(output_format);
            let rows: Vec<StorageMetricsRow> =
                metrics.into_iter().map(StorageMetricsRow::from).collect();
            crate::output::print_output(rows, format)?;
        }
        MonitorCommands::Cluster => {
            // Per-node live usage from the cluster heartbeat (there is no
            // /api/monitoring/cluster; the CLI used to call it and get 404).
            let nodes: Vec<ClusterNode> = api.get("/api/cluster/nodes").await?;
            let format = OutputFormat::from_str(output_format);
            if format == OutputFormat::Table {
                let rows: Vec<ClusterNodeRow> = nodes.iter().map(ClusterNodeRow::from).collect();
                crate::output::print_output(rows, format)?;
                let cores: u32 = nodes.iter().map(|n| n.cpu_cores).sum();
                let mem: u64 = nodes.iter().map(|n| n.memory_total).sum();
                let used: u64 = nodes.iter().filter_map(|n| n.memory_used).sum();
                println!(
                    "\n{} node(s), {} cores, memory {} / {}",
                    nodes.len(),
                    cores,
                    format_bytes(used),
                    format_bytes(mem)
                );
            } else {
                crate::output::print_single(&nodes, format)?;
            }
        }
        MonitorCommands::Watch { interval } => {
            use std::io::Write;
            use std::time::Duration;

            println!("Watching metrics (press Ctrl+C to stop)...\n");

            loop {
                // Clear screen (ANSI escape code)
                print!("\x1B[2J\x1B[1;1H");
                std::io::stdout().flush()?;

                let metrics: NodeMetrics = api.get("/api/monitoring/node").await?;
                print_node(
                    &metrics,
                    &format!("Node Metrics (refreshing every {}s)", interval),
                );

                tokio::time::sleep(Duration::from_secs(interval)).await;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bodies as the API serializes them (monitoring::ResourceMetrics etc.).
    #[test]
    fn api_metric_shapes_decode() {
        let vm: ResourceMetrics = serde_json::from_str(
            r#"{"id":"vm-1","name":"web","timestamp":1,
                "cpu":{"usage_percent":49.6,"cores":2,"load_average":0.0},
                "memory":{"total_bytes":1073741824,"used_bytes":324009984,"free_bytes":1,"usage_percent":30.2},
                "disk":{"read_bytes_per_sec":0,"write_bytes_per_sec":10,"read_iops":0,"write_iops":0,"total_bytes":2,"used_bytes":1},
                "network":{"rx_bytes_per_sec":5,"tx_bytes_per_sec":6,"rx_packets_per_sec":0,"tx_packets_per_sec":0}}"#,
        )
        .unwrap();
        let row = VmMetricsRow::from(vm);
        assert_eq!(row.vm_id, "vm-1");
        assert_eq!(row.cpu_usage, "49.6%");

        let none: Option<ResourceMetrics> = serde_json::from_str("null").unwrap();
        assert!(none.is_none());

        let node: NodeMetrics = serde_json::from_str(
            r#"{"hostname":"n1","timestamp":1,"cpu":{"usage_percent":3.0,"cores":8,"load_average":0.1},
                "memory":{"total_bytes":10,"used_bytes":5,"free_bytes":5,"usage_percent":50.0},
                "uptime_seconds":100,"load_average_1m":0.1,"load_average_5m":0.2,"load_average_15m":0.3}"#,
        )
        .unwrap();
        assert_eq!(node.cpu.cores, 8);

        let st: StorageMetrics = serde_json::from_str(
            r#"{"name":"local","storage_type":"directory","total_bytes":100,"used_bytes":40,"available_bytes":60,"usage_percent":40.0}"#,
        )
        .unwrap();
        assert_eq!(StorageMetricsRow::from(st).usage_percent, "40.0%");

        let nodes: Vec<ClusterNode> = serde_json::from_str(
            r#"[{"id":1,"name":"node1","ip":"10.0.0.1","status":"Online","priority":100,"is_local":true,
                 "architecture":"x86_64","cpu_cores":4,"memory_total":8000,"cpu_usage":12.5,"memory_used":4000},
                {"id":2,"name":"node2","ip":"10.0.0.2","status":"Offline","priority":100,"is_local":false,
                 "architecture":"x86_64","cpu_cores":4,"memory_total":8000,"cpu_usage":null}]"#,
        )
        .unwrap();
        let r: Vec<ClusterNodeRow> = nodes.iter().map(ClusterNodeRow::from).collect();
        assert_eq!(r[0].status, "online");
        assert_eq!(r[1].cpu, "unknown");
    }
}
