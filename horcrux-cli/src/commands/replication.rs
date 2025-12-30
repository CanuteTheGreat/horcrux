///! Replication management commands

use crate::api::ApiClient;
use crate::output::{self, OutputFormat, format_bytes};
use crate::ReplicationCommands;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use tabled::Tabled;

#[derive(Debug, Serialize, Deserialize)]
pub struct ReplicationJob {
    pub id: String,
    pub vm_id: String,
    pub source_node: String,
    pub target_node: String,
    pub schedule: String,
    pub enabled: bool,
    pub last_sync: Option<String>,
    pub next_sync: Option<String>,
    pub status: String,
    pub error: Option<String>,
}

#[derive(Tabled, Serialize)]
struct ReplicationJobRow {
    id: String,
    vm_id: String,
    target: String,
    schedule: String,
    enabled: String,
    status: String,
}

impl From<ReplicationJob> for ReplicationJobRow {
    fn from(job: ReplicationJob) -> Self {
        Self {
            id: job.id,
            vm_id: job.vm_id,
            target: job.target_node,
            schedule: job.schedule,
            enabled: if job.enabled { "Yes" } else { "No" }.to_string(),
            status: job.status,
        }
    }
}

pub async fn handle_replication_command(
    command: ReplicationCommands,
    api: &ApiClient,
    output_format: &str,
) -> Result<()> {
    match command {
        ReplicationCommands::List => {
            let jobs: Vec<ReplicationJob> = api.get("/api/replication/jobs").await?;
            let format = OutputFormat::from_str(output_format);
            let rows: Vec<ReplicationJobRow> = jobs.into_iter().map(ReplicationJobRow::from).collect();
            output::print_output(rows, format)?;
        }

        ReplicationCommands::Show { id } => {
            let job: ReplicationJob = api.get(&format!("/api/replication/jobs/{}", id)).await?;
            let format = OutputFormat::from_str(output_format);
            output::print_single(&job, format)?;
        }

        ReplicationCommands::Create {
            vm_id,
            target_node,
            schedule,
        } => {
            #[derive(Serialize)]
            struct CreateRequest {
                vm_id: String,
                target_node: String,
                schedule: String,
                enabled: bool,
            }

            let request = CreateRequest {
                vm_id: vm_id.clone(),
                target_node: target_node.clone(),
                schedule,
                enabled: true,
            };

            let job: ReplicationJob = api.post("/api/replication/jobs", &request).await?;
            output::print_created("Replication job", &vm_id, &job.id);
            output::print_info(&format!("VM {} will replicate to node {}", vm_id, target_node));
            if let Some(next_sync) = &job.next_sync {
                output::print_info(&format!("Next sync: {}", next_sync));
            }
        }

        ReplicationCommands::Execute { id } => {
            use indicatif::{ProgressBar, ProgressStyle};
            let spinner = ProgressBar::new_spinner();
            spinner.set_style(
                ProgressStyle::default_spinner()
                    .template("{spinner:.green} {msg}")
                    .unwrap()
            );
            spinner.set_message(format!("Executing replication job {}...", id));
            spinner.enable_steady_tick(std::time::Duration::from_millis(100));

            #[derive(Serialize, Deserialize)]
            struct ExecuteResponse {
                job_id: String,
                status: String,
                bytes_transferred: u64,
            }

            let response: ExecuteResponse = api
                .post(&format!("/api/replication/jobs/{}/execute", id), &serde_json::json!({}))
                .await?;

            spinner.finish_and_clear();
            output::print_success("Replication executed successfully");
            output::print_info(&format!("Status: {}", response.status));
            output::print_info(&format!("Transferred: {}", format_bytes(response.bytes_transferred)));
        }

        ReplicationCommands::Delete { id } => {
            use dialoguer::Confirm;

            let confirm = Confirm::new()
                .with_prompt(format!("Are you sure you want to delete replication job {}?", id))
                .interact()?;

            if confirm {
                api.delete(&format!("/api/replication/jobs/{}", id)).await?;
                output::print_deleted("Replication job", &id);
            } else {
                output::print_info("Deletion cancelled");
            }
        }

        ReplicationCommands::Status { id } => {
            #[derive(Serialize, Deserialize)]
            struct ReplicationStatus {
                job_id: String,
                status: String,
                last_sync_duration_secs: Option<u64>,
                bytes_transferred: Option<u64>,
                snapshots_synced: Option<u32>,
                error: Option<String>,
            }

            let status: ReplicationStatus = api
                .get(&format!("/api/replication/jobs/{}/status", id))
                .await?;
            let format = OutputFormat::from_str(output_format);
            output::print_single(&status, format)?;
        }
    }

    Ok(())
}
