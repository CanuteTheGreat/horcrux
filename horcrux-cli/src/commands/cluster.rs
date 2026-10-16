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
    /// Live CPU % / RAM in use (bytes) from the node heartbeat. `null` (or
    /// absent on older servers) means unknown -- shown as "unknown", never 0.
    #[serde(default)]
    cpu_usage: Option<f64>,
    #[serde(default)]
    memory_used: Option<u64>,
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
    #[tabled(rename = "cpu")]
    cpu_usage: String,
    #[tabled(rename = "mem used")]
    memory_used: String,
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
            cpu_usage: node
                .cpu_usage
                .map(|c| format!("{:.1}%", c))
                .unwrap_or_else(|| "unknown".to_string()),
            memory_used: node
                .memory_used
                .map(format_bytes)
                .unwrap_or_else(|| "unknown".to_string()),
            priority: node.priority,
        }
    }
}

#[derive(Tabled, Serialize)]
struct ArchRow {
    #[tabled(rename = "arch")]
    architecture: String,
    nodes: usize,
}

impl ArchitectureSummary {
    fn rows(&self) -> Vec<ArchRow> {
        vec![
            ArchRow {
                architecture: "x86_64".to_string(),
                nodes: self.x86_64_nodes,
            },
            ArchRow {
                architecture: "aarch64".to_string(),
                nodes: self.aarch64_nodes,
            },
            ArchRow {
                architecture: "other".to_string(),
                nodes: self.other_nodes,
            },
        ]
    }
}

#[derive(Serialize)]
struct AddNodeRequest {
    name: String,
    address: String,
}

/// Mirrors horcrux-api's `cluster::ClusterStatusReport`
/// (`GET /api/cluster/status`). That route didn't exist until this struct's
/// shape was pinned to it, so `horcrux cluster status` always 404'd.
#[derive(Debug, Serialize, Deserialize)]
struct ClusterStatus {
    /// Corosync cluster name; `null` when no Corosync cluster is set up.
    #[serde(default)]
    name: Option<String>,
    total_nodes: usize,
    online_nodes: usize,
    /// `null` when quorum can't be determined.
    #[serde(default)]
    has_quorum: Option<bool>,
    total_vms: usize,
    total_cpus: u32,
    /// Bytes.
    total_memory: u64,
    /// Bytes in use across the nodes reporting usage; `null` if none do.
    #[serde(default)]
    memory_used: Option<u64>,
    #[serde(default)]
    nodes_reporting_usage: usize,
}

/// Path of the remove-node endpoint (`DELETE`), shared with the tests so the
/// CLI and the server router can't silently drift apart again.
fn node_path(name: &str) -> String {
    format!("/api/cluster/nodes/{}", name)
}

const CLUSTER_STATUS_PATH: &str = "/api/cluster/status";

// Mirrors horcrux-api's `cluster::ArchitectureSummary` (the documented
// /api/cluster/architecture contract since the first commit). This used to
// expect an `architectures: [{architecture, node_count, vm_count}]` list the
// API has never sent, so `horcrux cluster architecture` always failed to
// deserialize.
#[derive(Debug, Serialize, Deserialize)]
struct ArchitectureSummary {
    x86_64_nodes: usize,
    aarch64_nodes: usize,
    other_nodes: usize,
    is_mixed: bool,
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
            let status: ClusterStatus = api.get(CLUSTER_STATUS_PATH).await?;
            let format = OutputFormat::from_str(output_format);
            output::print_single(&status, format)?;
        }
        ClusterCommands::Add { name, address } => {
            let request = AddNodeRequest {
                name: name.clone(),
                address: address.clone(),
            };

            api.post_empty(&node_path(&name), &request).await?;
            output::print_created("Node", &name, &address);
        }
        ClusterCommands::Remove { name } => {
            api.delete(&node_path(&name)).await?;
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
            output::print_output(summary.rows(), format)?;
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
    fn architecture_summary_deserializes_from_real_api_shape() {
        // Real horcrux-api /api/cluster/architecture response shape.
        let json = r#"{"x86_64_nodes":2,"aarch64_nodes":1,"other_nodes":0,"is_mixed":true}"#;
        let summary: ArchitectureSummary = serde_json::from_str(json).expect("must deserialize");
        assert!(summary.is_mixed);
        let rows = summary.rows();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].architecture, "x86_64");
        assert_eq!(rows[0].nodes, 2);
        assert_eq!(rows[1].nodes, 1);
        assert_eq!(rows[2].nodes, 0);
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
            cpu_usage: None,
            memory_used: None,
        };
        let row = NodeRow::from(node);
        assert_eq!(row.memory, format_bytes(8 * 1024 * 1024 * 1024));
        assert_eq!(row.address, "10.0.0.1");
        assert_eq!(row.status, "Online");
        assert_eq!(
            row.cpu_usage, "unknown",
            "unknown usage must not print as 0"
        );
        assert_eq!(row.memory_used, "unknown");
    }

    #[test]
    fn node_live_usage_deserializes_and_renders() {
        let json = r#"{
            "id": 2, "name": "n2", "ip": "10.0.0.2", "status": "online",
            "priority": 100, "is_local": false, "architecture": "aarch64",
            "cpu_cores": 4, "memory_total": 8589934592,
            "cpu_usage": 37.5, "memory_used": 2147483648, "last_seen": 1760000000
        }"#;
        let node: Node = serde_json::from_str(json).expect("must deserialize");
        assert_eq!(node.cpu_usage, Some(37.5));
        assert_eq!(node.memory_used, Some(2_147_483_648));
        let row = NodeRow::from(node);
        assert_eq!(row.cpu_usage, "37.5%");
        assert_eq!(row.memory_used, format_bytes(2_147_483_648));

        let unknown = r#"{
            "id": 3, "name": "n3", "ip": "10.0.0.3", "status": "offline",
            "architecture": "x86_64", "cpu_cores": 4, "memory_total": 1024,
            "cpu_usage": null, "memory_used": null, "last_seen": null
        }"#;
        let node: Node = serde_json::from_str(unknown).unwrap();
        assert_eq!(node.cpu_usage, None);
        assert_eq!(NodeRow::from(node).cpu_usage, "unknown");
    }

    #[test]
    fn cluster_status_deserializes_from_real_api_shape() {
        // Real horcrux-api /api/cluster/status (ClusterStatusReport) body.
        let json = r#"{
            "name": null, "total_nodes": 2, "online_nodes": 1, "has_quorum": null,
            "total_vms": 3, "total_cpus": 12, "total_memory": 25769803776,
            "memory_used": 4294967296, "nodes_reporting_usage": 1
        }"#;
        let status: ClusterStatus = serde_json::from_str(json).expect("must deserialize");
        assert_eq!(status.name, None);
        assert_eq!(status.total_nodes, 2);
        assert_eq!(status.online_nodes, 1);
        assert_eq!(status.has_quorum, None);
        assert_eq!(status.total_vms, 3);
        assert_eq!(status.total_cpus, 12);
        assert_eq!(status.total_memory, 25_769_803_776);
        assert_eq!(status.memory_used, Some(4_294_967_296));
        assert_eq!(status.nodes_reporting_usage, 1);
    }

    #[test]
    fn cluster_paths_match_server_routes() {
        // horcrux-api registers GET /api/cluster/status and
        // POST|DELETE /api/cluster/nodes/:name (cluster_routes in main.rs).
        assert_eq!(CLUSTER_STATUS_PATH, "/api/cluster/status");
        assert_eq!(node_path("node-02"), "/api/cluster/nodes/node-02");
    }
}
