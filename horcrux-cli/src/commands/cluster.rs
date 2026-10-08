use crate::api::ApiClient;
use crate::output::{self, format_bytes, OutputFormat};
use crate::ClusterCommands;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use tabled::Tabled;

#[derive(Debug, Serialize, Deserialize)]
struct Node {
    name: String,
    address: String,
    architecture: String,
    total_memory: u64,
    total_cpus: u32,
    online: bool,
    /// Placement/HA weight (0-1000, default 100). Missing on older servers
    /// that predate this field -- default to 100 (neutral) rather than
    /// failing deserialization.
    #[serde(default = "default_priority")]
    priority: u32,
}

fn default_priority() -> u32 {
    100
}

#[derive(Tabled, Serialize)]
struct NodeRow {
    name: String,
    address: String,
    #[tabled(rename = "arch")]
    architecture: String,
    memory: String,
    cpus: u32,
    status: String,
    priority: u32,
}

impl From<Node> for NodeRow {
    fn from(node: Node) -> Self {
        Self {
            name: node.name,
            address: node.address,
            architecture: node.architecture,
            memory: format_bytes(node.total_memory * 1024 * 1024 * 1024),
            cpus: node.total_cpus,
            status: if node.online { "Online" } else { "Offline" }.to_string(),
            priority: node.priority,
        }
    }
}

#[derive(Tabled, Serialize)]
struct ArchRow {
    #[tabled(rename = "arch")]
    architecture: String,
    nodes: usize,
    vms: usize,
}

impl From<ArchInfo> for ArchRow {
    fn from(arch: ArchInfo) -> Self {
        Self {
            architecture: arch.architecture,
            nodes: arch.node_count,
            vms: arch.vm_count,
        }
    }
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
            let format = OutputFormat::from_str(output_format);
            let rows: Vec<NodeRow> = nodes.into_iter().map(NodeRow::from).collect();
            output::print_output(rows, format)?;
        }
        ClusterCommands::Status => {
            let status: ClusterStatus = api.get("/api/cluster/status").await?;
            let format = OutputFormat::from_str(output_format);
            output::print_single(&status, format)?;
        }
        ClusterCommands::Add { name, address } => {
            let request = AddNodeRequest {
                name: name.clone(),
                address: address.clone(),
            };

            api.post_empty(&format!("/api/cluster/nodes/{}", name), &request)
                .await?;
            output::print_created("Node", &name, &address);
        }
        ClusterCommands::Remove { name } => {
            api.delete(&format!("/api/cluster/nodes/{}", name)).await?;
            output::print_deleted("Node", &name);
        }
        ClusterCommands::SetPriority { name, priority } => {
            #[derive(Serialize)]
            struct SetPriorityRequest {
                priority: u32,
            }
            api.patch_empty(
                &format!("/api/cluster/nodes/{}/priority", name),
                &SetPriorityRequest { priority },
            )
            .await?;
            println!("Node '{}' placement weight set to {}", name, priority);
        }
        ClusterCommands::Architecture => {
            let summary: ArchitectureSummary = api.get("/api/cluster/architecture").await?;
            let format = OutputFormat::from_str(output_format);
            let rows: Vec<ArchRow> = summary
                .architectures
                .into_iter()
                .map(ArchRow::from)
                .collect();
            output::print_output(rows, format)?;
        }
    }
    Ok(())
}
