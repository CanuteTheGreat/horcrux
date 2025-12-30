use crate::api::ApiClient;
use crate::output::{self, OutputFormat};
use crate::HaCommands;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use tabled::Tabled;

#[derive(Debug, Serialize, Deserialize)]
struct HaResource {
    vm_id: String,
    vm_name: String,
    group: String,
    priority: u32,
    state: String,
}

#[derive(Tabled, Serialize)]
struct HaResourceRow {
    vm_id: String,
    vm_name: String,
    group: String,
    priority: u32,
    state: String,
}

impl From<HaResource> for HaResourceRow {
    fn from(r: HaResource) -> Self {
        Self {
            vm_id: r.vm_id,
            vm_name: r.vm_name,
            group: r.group,
            priority: r.priority,
            state: r.state,
        }
    }
}

#[derive(Serialize)]
struct AddHaRequest {
    vm_id: u32,
    group: String,
    priority: u32,
}

#[derive(Serialize)]
struct CreateGroupRequest {
    name: String,
    nodes: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct HaStatus {
    total_resources: usize,
    running: usize,
    stopped: usize,
    migrating: usize,
}

pub async fn handle_ha_command(
    command: HaCommands,
    api: &ApiClient,
    output_format: &str,
) -> Result<()> {
    match command {
        HaCommands::List => {
            let resources: Vec<HaResource> = api.get("/api/ha/resources").await?;
            let format = OutputFormat::from_str(output_format);
            let rows: Vec<HaResourceRow> = resources.into_iter().map(HaResourceRow::from).collect();
            output::print_output(rows, format)?;
        }
        HaCommands::Add { vm_id, group, priority } => {
            let request = AddHaRequest { vm_id, group: group.clone(), priority };
            api.post_empty("/api/ha/resources", &request).await?;
            output::print_created("HA resource", &format!("VM {}", vm_id), &group);
        }
        HaCommands::Remove { vm_id } => {
            api.delete(&format!("/api/ha/resources/{}", vm_id)).await?;
            output::print_deleted("HA resource", &format!("{}", vm_id));
        }
        HaCommands::Status => {
            let status: HaStatus = api.get("/api/ha/status").await?;
            let format = OutputFormat::from_str(output_format);
            output::print_single(&status, format)?;
        }
        HaCommands::CreateGroup { name, nodes } => {
            let node_list: Vec<String> = nodes.split(',').map(|s| s.trim().to_string()).collect();
            let request = CreateGroupRequest { name: name.clone(), nodes: node_list.clone() };
            api.post_empty("/api/ha/groups", &request).await?;
            output::print_created("HA group", &name, &node_list.join(","));
        }
    }
    Ok(())
}
