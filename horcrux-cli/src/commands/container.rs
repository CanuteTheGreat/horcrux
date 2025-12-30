///! Container management commands

use crate::api::ApiClient;
use crate::output::{self, OutputFormat, truncate};
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

#[derive(Tabled, Serialize)]
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
            name: truncate(&c.name, 30),
            runtime: c.runtime,
            image: truncate(&c.image, 40),
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
            let format = OutputFormat::from_str(output_format);
            let rows: Vec<ContainerRow> = containers.into_iter().map(ContainerRow::from).collect();
            output::print_output(rows, format)?;
        }

        ContainerCommands::Show { id } => {
            let container: Container = api.get(&format!("/api/containers/{}", id)).await?;
            let format = OutputFormat::from_str(output_format);
            output::print_single(&container, format)?;
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
            output::print_created("Container", &container.name, &container.id);
        }

        ContainerCommands::Start { id } => {
            let container: Container = api
                .post(&format!("/api/containers/{}/start", id), &serde_json::json!({}))
                .await?;
            output::print_started("Container", &container.name);
        }

        ContainerCommands::Stop { id } => {
            let container: Container = api
                .post(&format!("/api/containers/{}/stop", id), &serde_json::json!({}))
                .await?;
            output::print_stopped("Container", &container.name);
        }

        ContainerCommands::Delete { id } => {
            use dialoguer::Confirm;

            let confirm = Confirm::new()
                .with_prompt(format!("Are you sure you want to delete container {}?", id))
                .interact()?;

            if confirm {
                api.delete(&format!("/api/containers/{}", id)).await?;
                output::print_deleted("Container", &id);
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
                output::print_error(&response.stderr);
            }

            if response.exit_code != 0 {
                anyhow::bail!("Command exited with code {}", response.exit_code);
            }
        }
    }

    Ok(())
}
