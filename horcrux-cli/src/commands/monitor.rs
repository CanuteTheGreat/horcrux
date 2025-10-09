use crate::api::ApiClient;
use crate::MonitorCommands;
use anyhow::Result;
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Serialize, Deserialize)]
struct StorageMetrics {
    name: String,
    used: u64,
    available: u64,
    total: u64,
    usage_percent: f64,
}

pub async fn handle_monitor_command(
    command: MonitorCommands,
    api: &ApiClient,
    output_format: &str,
) -> Result<()> {
    match command {
        MonitorCommands::Node => {
            let metrics: NodeMetrics = api.get("/api/monitoring/node").await?;

            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&metrics)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&metrics)?);
            } else {
                println!("Node Metrics:");
                println!("  CPU Usage:    {:.1}%", metrics.cpu_usage);
                println!("  Memory Usage: {:.1}%", metrics.memory_usage);
                println!("  Memory Total: {} GB", metrics.memory_total);
                println!("  Disk Usage:   {:.1}%", metrics.disk_usage);
                println!("  Uptime:       {} seconds", metrics.uptime);
            }
        }
        MonitorCommands::Vm { id } => {
            let metrics: Vec<VmMetrics> = if let Some(vm_id) = id {
                vec![api.get(&format!("/api/monitoring/vms/{}", vm_id)).await?]
            } else {
                api.get("/api/monitoring/vms").await?
            };

            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&metrics)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&metrics)?);
            } else {
                println!("{:<12} {:<20} {:<10} {:<12} {:<12} {:<12}",
                    "VM ID", "NAME", "CPU %", "MEMORY", "DISK R/W", "NET I/O");
                println!("{}", "-".repeat(90));
                for vm in metrics {
                    println!("{:<12} {:<20} {:<10.1} {:<12} {:<12} {:<12}",
                        vm.vm_id, vm.vm_name, vm.cpu_usage,
                        format!("{} MB", vm.memory_usage / 1024 / 1024),
                        format!("{}/{}", vm.disk_read, vm.disk_write),
                        format!("{}/{}", vm.net_in, vm.net_out));
                }
            }
        }
        MonitorCommands::Storage { name } => {
            let metrics: Vec<StorageMetrics> = if let Some(pool_name) = name {
                vec![api.get(&format!("/api/monitoring/storage/{}", pool_name)).await?]
            } else {
                api.get("/api/monitoring/storage").await?
            };

            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&metrics)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&metrics)?);
            } else {
                println!("{:<20} {:<12} {:<12} {:<12} {}",
                    "NAME", "USED", "AVAILABLE", "TOTAL", "USAGE %");
                println!("{}", "-".repeat(70));
                for pool in metrics {
                    println!("{:<20} {:<12} {:<12} {:<12} {:.1}%",
                        pool.name,
                        format!("{} GB", pool.used),
                        format!("{} GB", pool.available),
                        format!("{} GB", pool.total),
                        pool.usage_percent);
                }
            }
        }
        MonitorCommands::Cluster => {
            let metrics: NodeMetrics = api.get("/api/monitoring/cluster").await?;

            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&metrics)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&metrics)?);
            } else {
                println!("Cluster Metrics:");
                println!("  CPU Usage:    {:.1}%", metrics.cpu_usage);
                println!("  Memory Usage: {:.1}%", metrics.memory_usage);
                println!("  Memory Total: {} GB", metrics.memory_total);
                println!("  Disk Usage:   {:.1}%", metrics.disk_usage);
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
