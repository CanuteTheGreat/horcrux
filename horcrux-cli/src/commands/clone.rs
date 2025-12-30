///! VM cloning commands

use crate::api::ApiClient;
use crate::output::{self, OutputFormat};
use crate::CloneCommands;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use tabled::Tabled;

#[derive(Debug, Serialize, Deserialize)]
pub struct CloneJob {
    pub job_id: String,
    pub source_vm_id: String,
    pub target_vm_id: Option<String>,
    pub target_vm_name: String,
    pub clone_type: String,
    pub status: String,
    pub progress: f64,
    pub created_at: String,
    pub completed_at: Option<String>,
    pub error: Option<String>,
}

#[derive(Tabled, Serialize)]
struct CloneJobRow {
    job_id: String,
    source: String,
    target: String,
    clone_type: String,
    status: String,
    progress: String,
}

impl From<CloneJob> for CloneJobRow {
    fn from(job: CloneJob) -> Self {
        Self {
            job_id: job.job_id,
            source: job.source_vm_id,
            target: job.target_vm_name,
            clone_type: job.clone_type,
            status: job.status,
            progress: format!("{:.1}%", job.progress * 100.0),
        }
    }
}

pub async fn handle_clone_command(
    command: CloneCommands,
    api: &ApiClient,
    output_format: &str,
) -> Result<()> {
    match command {
        CloneCommands::Create {
            vm_id,
            name,
            full,
            start,
        } => {
            #[derive(Serialize)]
            struct CloneRequest {
                name: String,
                full_clone: bool,
                start_after_clone: bool,
                target_storage: String,
            }

            let request = CloneRequest {
                name: name.clone(),
                full_clone: full,
                start_after_clone: start,
                target_storage: "local".to_string(),
            };

            use indicatif::{ProgressBar, ProgressStyle};
            let spinner = ProgressBar::new_spinner();
            spinner.set_style(
                ProgressStyle::default_spinner()
                    .template("{spinner:.green} {msg}")
                    .unwrap()
            );
            spinner.set_message(format!("Cloning VM {} as '{}'...", vm_id, name));
            spinner.enable_steady_tick(std::time::Duration::from_millis(100));

            let job: CloneJob = api
                .post(&format!("/api/vms/{}/clone", vm_id), &request)
                .await?;

            spinner.finish_and_clear();
            output::print_created("Clone job", &name, &job.job_id);
            output::print_info(&format!("Clone type: {}", if full { "Full" } else { "Linked" }));
            output::print_info(&format!("Track progress: horcrux clone status {}", job.job_id));
        }

        CloneCommands::List => {
            let jobs: Vec<CloneJob> = api.get("/api/vms/clone/jobs").await?;
            let format = OutputFormat::from_str(output_format);
            let rows: Vec<CloneJobRow> = jobs.into_iter().map(CloneJobRow::from).collect();
            output::print_output(rows, format)?;
        }

        CloneCommands::Status { job_id } => {
            let job: CloneJob = api.get(&format!("/api/vms/clone/jobs/{}", job_id)).await?;
            let format = OutputFormat::from_str(output_format);

            if format == OutputFormat::Table {
                // Show progress bar if still in progress
                if job.status == "running" || job.status == "pending" {
                    use indicatif::{ProgressBar, ProgressStyle};
                    let pb = ProgressBar::new(100);
                    pb.set_style(
                        ProgressStyle::default_bar()
                            .template("{msg} [{bar:40.cyan/blue}] {pos}%")
                            .unwrap()
                            .progress_chars("=> ")
                    );
                    pb.set_message("Progress");
                    pb.set_position((job.progress * 100.0) as u64);
                    pb.finish();
                }
            }
            output::print_single(&job, format)?;
        }

        CloneCommands::Cancel { job_id } => {
            use dialoguer::Confirm;

            let confirm = Confirm::new()
                .with_prompt(format!("Are you sure you want to cancel clone job {}?", job_id))
                .interact()?;

            if confirm {
                api.post::<serde_json::Value, _>(
                    &format!("/api/vms/clone/jobs/{}/cancel", job_id),
                    &serde_json::json!({}),
                )
                .await?;
                output::print_success(&format!("Clone job '{}' cancelled", job_id));
            } else {
                output::print_info("Cancellation aborted");
            }
        }
    }

    Ok(())
}
