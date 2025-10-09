use crate::api::ApiClient;
use crate::output;
use crate::ClusterCommands;
use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct Node {
    name: String,
    address: String,
    architecture: String,
    total_memory: u64,
    total_cpus: u32,
    online: bool,
}

#[derive(Serialize)]
struct AddNodeRequest {
    name: String,
    address: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ClusterStatus {
    total_nodes: usize,
    online_nodes: usize,
    total_vms: usize,
    total_memory: u64,
    total_cpus: u32,
}

#[derive(Debug, Serialize, Deserialize)]
struct ArchitectureSummary {
    architectures: Vec<ArchInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ArchInfo {
    architecture: String,
    node_count: usize,
    vm_count: usize,
}

pub async fn handle_cluster_command(
    command: ClusterCommands,
    api: &ApiClient,
    output_format: &str,
) -> Result<()> {
    match command {
        ClusterCommands::List => {
            let nodes: Vec<Node> = api.get("/api/cluster/nodes").await?;

            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&nodes)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&nodes)?);
            } else {
                println!("{:<20} {:<25} {:<12} {:<12} {:<8} {}",
                    "NAME", "ADDRESS", "ARCH", "MEMORY", "CPUS", "STATUS");
                println!("{}", "-".repeat(100));
                for node in nodes {
                    let status = if node.online { "Online" } else { "Offline" };
                    println!("{:<20} {:<25} {:<12} {:<12} {:<8} {}",
                        node.name, node.address, node.architecture,
                        format!("{} GB", node.total_memory),
                        node.total_cpus, status);
                }
            }
        }
        ClusterCommands::Status => {
            let status: ClusterStatus = api.get("/api/cluster/status").await?;

            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&status)?);
            } else {
                println!("Cluster Status:");
                println!("  Nodes:  {} online / {} total", status.online_nodes, status.total_nodes);
                println!("  VMs:    {}", status.total_vms);
                println!("  Memory: {} GB total", status.total_memory);
                println!("  CPUs:   {} total", status.total_cpus);
            }
        }
        ClusterCommands::Add { name, address } => {
            let request = AddNodeRequest {
                name: name.clone(),
                address: address.clone(),
            };

            api.post_empty(&format!("/api/cluster/nodes/{}", name), &request).await?;
            output::print_success(&format!("Node '{}' added to cluster", name));
        }
        ClusterCommands::Remove { name } => {
            api.delete(&format!("/api/cluster/nodes/{}", name)).await?;
            output::print_success(&format!("Node '{}' removed from cluster", name));
        }
        ClusterCommands::Architecture => {
            let summary: ArchitectureSummary = api.get("/api/cluster/architecture").await?;

            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&summary)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&summary)?);
            } else {
                println!("{:<12} {:<12} {:<12}",
                    "ARCH", "NODES", "VMS");
                println!("{}", "-".repeat(40));
                for arch in summary.architectures {
                    println!("{:<12} {:<12} {:<12}",
                        arch.architecture, arch.node_count, arch.vm_count);
                }
            }
        }
    }
    Ok(())
}
