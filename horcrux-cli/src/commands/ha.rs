use crate::api::ApiClient;
use crate::output;
use crate::HaCommands;
use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct HaResource {
    vm_id: String,
    vm_name: String,
    group: String,
    priority: u32,
    state: String,
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

            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&resources)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&resources)?);
            } else {
                println!("{:<12} {:<20} {:<15} {:<10} {}",
                    "VM ID", "VM NAME", "GROUP", "PRIORITY", "STATE");
                println!("{}", "-".repeat(80));
                for resource in resources {
                    println!("{:<12} {:<20} {:<15} {:<10} {}",
                        resource.vm_id, resource.vm_name, resource.group,
                        resource.priority, resource.state);
                }
            }
        }
        HaCommands::Add { vm_id, group, priority } => {
            let request = AddHaRequest { vm_id, group: group.clone(), priority };
            api.post_empty("/api/ha/resources", &request).await?;
            output::print_success(&format!("VM {} added to HA group '{}'", vm_id, group));
        }
        HaCommands::Remove { vm_id } => {
            api.delete(&format!("/api/ha/resources/{}", vm_id)).await?;
            output::print_success(&format!("VM {} removed from HA", vm_id));
        }
        HaCommands::Status => {
            let status: HaStatus = api.get("/api/ha/status").await?;

            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&status)?);
            } else {
                println!("HA Status:");
                println!("  Total Resources: {}", status.total_resources);
                println!("  Running:         {}", status.running);
                println!("  Stopped:         {}", status.stopped);
                println!("  Migrating:       {}", status.migrating);
            }
        }
        HaCommands::CreateGroup { name, nodes } => {
            let node_list: Vec<String> = nodes.split(',').map(|s| s.trim().to_string()).collect();
            let request = CreateGroupRequest { name: name.clone(), nodes: node_list };
            api.post_empty("/api/ha/groups", &request).await?;
            output::print_success(&format!("HA group '{}' created", name));
        }
    }
    Ok(())
}
