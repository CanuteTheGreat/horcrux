///! Replication management commands

use crate::api::ApiClient;
use crate::output;
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

#[derive(Tabled)]
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

            if output_format == "json" {
                output::print_json(&jobs)?;
            } else {
                let rows: Vec<ReplicationJobRow> = jobs.into_iter().map(ReplicationJobRow::from).collect();
                output::print_table(rows);
            }
        }

        ReplicationCommands::Show { id } => {
            let job: ReplicationJob = api.get(&format!("/api/replication/jobs/{}", id)).await?;

            if output_format == "json" {
                output::print_json(&job)?;
            } else {
                println!("Replication Job Details:");
                println!("  ID: {}", job.id);
                println!("  VM ID: {}", job.vm_id);
                println!("  Source Node: {}", job.source_node);
                println!("  Target Node: {}", job.target_node);
                println!("  Schedule: {}", job.schedule);
                println!("  Enabled: {}", if job.enabled { "Yes" } else { "No" });
                println!("  Status: {}", job.status);
                if let Some(last_sync) = job.last_sync {
                    println!("  Last Sync: {}", last_sync);
                }
                if let Some(next_sync) = job.next_sync {
                    println!("  Next Sync: {}", next_sync);
                }
                if let Some(error) = job.error {
                    println!("  Error: {}", error);
                }
            }
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

            if output_format == "json" {
                output::print_json(&job)?;
            } else {
                output::print_success(&format!("Replication job created: {}", job.id));
                println!("VM {} will replicate to node {}", vm_id, target_node);
                if let Some(next_sync) = job.next_sync {
                    println!("Next sync scheduled for: {}", next_sync);
                }
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

            spinner.finish_with_message("Replication completed");

            if output_format == "json" {
                output::print_json(&response)?;
            } else {
                output::print_success("Replication executed successfully");
                println!("Status: {}", response.status);
                println!(
                    "Transferred: {} MB",
                    response.bytes_transferred / 1024 / 1024
                );
            }
        }

        ReplicationCommands::Delete { id } => {
            use dialoguer::Confirm;

            let confirm = Confirm::new()
                .with_prompt(format!("Are you sure you want to delete replication job {}?", id))
                .interact()?;

            if confirm {
                api.delete(&format!("/api/replication/jobs/{}", id)).await?;
                output::print_success(&format!("Replication job deleted: {}", id));
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

            if output_format == "json" {
                output::print_json(&status)?;
            } else {
                println!("Replication Status:");
                println!("  Job ID: {}", status.job_id);
                println!("  Status: {}", status.status);
                if let Some(duration) = status.last_sync_duration_secs {
                    println!("  Last Sync Duration: {}s", duration);
                }
                if let Some(bytes) = status.bytes_transferred {
                    println!("  Bytes Transferred: {} MB", bytes / 1024 / 1024);
                }
                if let Some(snapshots) = status.snapshots_synced {
                    println!("  Snapshots Synced: {}", snapshots);
                }
                if let Some(error) = status.error {
                    println!("  Error: {}", error);
                }
            }
        }
    }

    Ok(())
}
