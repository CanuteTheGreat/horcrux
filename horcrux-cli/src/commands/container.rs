///! Container management commands

use crate::api::ApiClient;
use crate::output;
use crate::ContainerCommands;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use tabled::Tabled;

#[derive(Debug, Serialize, Deserialize)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub runtime: String,
    pub image: String,
    pub status: String,
    pub created_at: Option<String>,
}

#[derive(Tabled)]
struct ContainerRow {
    id: String,
    name: String,
    runtime: String,
    image: String,
    status: String,
}

impl From<Container> for ContainerRow {
    fn from(c: Container) -> Self {
        Self {
            id: c.id,
            name: c.name,
            runtime: c.runtime,
            image: c.image,
            status: c.status,
        }
    }
}

pub async fn handle_container_command(
    command: ContainerCommands,
    api: &ApiClient,
    output_format: &str,
) -> Result<()> {
    match command {
        ContainerCommands::List => {
            let containers: Vec<Container> = api.get("/api/containers").await?;

            if output_format == "json" {
                output::print_json(&containers)?;
            } else {
                let rows: Vec<ContainerRow> = containers.into_iter().map(ContainerRow::from).collect();
                output::print_table(rows);
            }
        }

        ContainerCommands::Show { id } => {
            let container: Container = api.get(&format!("/api/containers/{}", id)).await?;

            if output_format == "json" {
                output::print_json(&container)?;
            } else {
                println!("Container Details:");
                println!("  ID: {}", container.id);
                println!("  Name: {}", container.name);
                println!("  Runtime: {}", container.runtime);
                println!("  Image: {}", container.image);
                println!("  Status: {}", container.status);
                if let Some(created) = container.created_at {
                    println!("  Created: {}", created);
                }
            }
        }

        ContainerCommands::Create {
            name,
            runtime,
            image,
            memory,
            cpus,
        } => {
            #[derive(Serialize)]
            struct CreateRequest {
                name: String,
                runtime: String,
                image: String,
                memory: Option<u64>,
                cpus: Option<u32>,
            }

            let request = CreateRequest {
                name: name.clone(),
                runtime,
                image,
                memory,
                cpus,
            };

            let container: Container = api.post("/api/containers", &request).await?;
            output::print_success(&format!("Container created: {} ({})", container.name, container.id));
        }

        ContainerCommands::Start { id } => {
            let container: Container = api
                .post(&format!("/api/containers/{}/start", id), &serde_json::json!({}))
                .await?;
            output::print_success(&format!("Container started: {}", container.name));
        }

        ContainerCommands::Stop { id } => {
            let container: Container = api
                .post(&format!("/api/containers/{}/stop", id), &serde_json::json!({}))
                .await?;
            output::print_success(&format!("Container stopped: {}", container.name));
        }

        ContainerCommands::Delete { id } => {
            use dialoguer::Confirm;

            let confirm = Confirm::new()
                .with_prompt(format!("Are you sure you want to delete container {}?", id))
                .interact()?;

            if confirm {
                api.delete(&format!("/api/containers/{}", id)).await?;
                output::print_success(&format!("Container deleted: {}", id));
            } else {
                output::print_info("Deletion cancelled");
            }
        }

        ContainerCommands::Exec { id, command } => {
            #[derive(Serialize)]
            struct ExecRequest {
                command: Vec<String>,
            }

            #[derive(Deserialize)]
            struct ExecResponse {
                exit_code: i32,
                stdout: String,
                stderr: String,
            }

            let request = ExecRequest { command };
            let response: ExecResponse = api
                .post(&format!("/api/containers/{}/exec", id), &request)
                .await?;

            if !response.stdout.is_empty() {
                println!("{}", response.stdout);
            }
            if !response.stderr.is_empty() {
                eprintln!("{}", response.stderr);
            }

            if response.exit_code != 0 {
                anyhow::bail!("Command exited with code {}", response.exit_code);
            }
        }
    }

    Ok(())
}
