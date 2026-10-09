use crate::api::ApiClient;
use crate::output::{self, format_bytes, OutputFormat};
use crate::ClusterCommands;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use tabled::Tabled;

// Field names and the `status` type mirror horcrux-api's actual JSON
// response shape (horcrux-api/src/cluster/node.rs::Node) exactly --
// this struct previously used different field names entirely
// (address/total_memory/total_cpus/online) which caused every
// `horcrux cluster list` call to hard-fail deserialization (confirmed
// broken both before and after commit 335852a by horcrux-agent-driven-qa,
// 2026-10-08). `total_memory` was also being treated as gigabytes and
// multiplied by 1024^3 below -- the API's `memory_total` is already in
// bytes (see its own doc comment), so that was a second, independent bug
// stacked on top of the field-name mismatch.
#[derive(Debug, Serialize, Deserialize)]
struct Node {
    name: String,
    ip: String,
    architecture: String,
    memory_total: u64,
    cpu_cores: u32,
    status: NodeStatus,
    /// Placement/HA weight (0-1000, default 100). Missing on older servers
    /// that predate this field -- default to 100 (neutral) rather than
    /// failing deserialization.
    #[serde(default = "default_priority")]
    priority: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum NodeStatus {
    Online,
    Offline,
    Unknown,
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
            address: node.ip,
            architecture: node.architecture,
            memory: format_bytes(node.memory_total),
            cpus: node.cpu_cores,
            status: match node.status {
                NodeStatus::Online => "Online",
                NodeStatus::Offline => "Offline",
                NodeStatus::Unknown => "Unknown",
            }
            .to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;

    // Regression test for the Node field-name/type mismatch that broke
    // every `horcrux cluster list` call: this struct previously used
    // address/total_memory/total_cpus/online instead of the API's real
    // ip/memory_total/cpu_cores/status fields, so deserialization always
    // failed. This is a real sample of horcrux-api's actual response
    // shape (see horcrux-api/src/cluster/node.rs::Node's Serialize impl).
    #[test]
    fn node_deserializes_from_real_api_shape() {
        let json = r#"{
            "id": 1,
            "name": "node-primary",
            "ip": "10.0.0.1",
            "status": "online",
            "priority": 300,
            "is_local": true,
            "architecture": "X86_64",
            "cpu_cores": 8,
            "memory_total": 17179869184
        }"#;
        let node: Node = serde_json::from_str(json).expect("must deserialize");
        assert_eq!(node.name, "node-primary");
        assert_eq!(node.ip, "10.0.0.1");
        assert_eq!(node.status, NodeStatus::Online);
        assert_eq!(node.priority, 300);
        assert_eq!(node.cpu_cores, 8);
        assert_eq!(node.memory_total, 17179869184);
    }

    #[test]
    fn node_priority_defaults_when_missing() {
        let json = r#"{
            "id": 1, "name": "n", "ip": "10.0.0.1", "status": "offline",
            "architecture": "X86_64", "cpu_cores": 4, "memory_total": 1024
        }"#;
        let node: Node = serde_json::from_str(json).expect("must deserialize");
        assert_eq!(node.priority, 100);
        assert_eq!(node.status, NodeStatus::Offline);
    }

    #[test]
    fn node_row_formats_memory_as_bytes_not_gigabytes() {
        // memory_total is already in bytes (per horcrux-api's own doc
        // comment) -- this used to be multiplied by 1024^3 again here,
        // inflating every displayed memory value by ~1 billion times.
        let node = Node {
            name: "n".into(),
            ip: "10.0.0.1".into(),
            architecture: "X86_64".into(),
            memory_total: 8 * 1024 * 1024 * 1024, // 8 GiB, in bytes
            cpu_cores: 4,
            status: NodeStatus::Online,
            priority: 100,
        };
        let row = NodeRow::from(node);
        assert_eq!(row.memory, format_bytes(8 * 1024 * 1024 * 1024));
        assert_eq!(row.address, "10.0.0.1");
        assert_eq!(row.status, "Online");
    }
}
