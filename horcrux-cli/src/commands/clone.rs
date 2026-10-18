//! VM cloning commands
use crate::api::ApiClient;
use crate::output::{self, OutputFormat};
use crate::CloneCommands;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use tabled::Tabled;

/// The API's clone job (`GET /api/clone-jobs[/:id]`,
/// horcrux-vm `vm::clone_progress::CloneJob`).
#[derive(Debug, Serialize, Deserialize)]
pub struct CloneJob {
    pub id: String,
    pub source_vm_id: String,
    pub target_vm_id: String,
    pub target_vm_name: String,
    /// queued, running, completed, failed or cancelled
    pub state: String,
    /// Current stage (e.g. "cloning_disk"), if started
    #[serde(default)]
    pub stage: Option<String>,
    /// 0-100
    pub progress: u8,
    pub created_at: String,
    #[serde(default)]
    pub completed_at: Option<String>,
    #[serde(default)]
    pub error_message: Option<String>,
}

#[derive(Tabled, Serialize)]
struct CloneJobRow {
    job_id: String,
    source: String,
    target: String,
    stage: String,
    status: String,
    progress: String,
}

impl From<CloneJob> for CloneJobRow {
    fn from(job: CloneJob) -> Self {
        Self {
            job_id: job.id,
            source: job.source_vm_id,
            target: format!("{} ({})", job.target_vm_name, job.target_vm_id),
            stage: job.stage.unwrap_or_else(|| "-".into()),
            status: job.state,
            progress: format!("{}%", job.progress),
        }
    }
}

/// API routes for clone jobs (the CLI used to call /api/vms/clone/jobs,
/// which doesn't exist).
fn jobs_path() -> &'static str {
    "/api/clone-jobs"
}
fn job_path(job_id: &str) -> String {
    format!("/api/clone-jobs/{job_id}")
}
fn cancel_path(job_id: &str) -> String {
    format!("/api/clone-jobs/{job_id}/cancel")
}

/// Body of POST /api/vms/:id/clone.
fn create_body(name: &str, linked: bool, start: bool) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "mode": if linked { "linked" } else { "full" },
        "start": start,
    })
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
            full: _,
            linked,
            start,
        } => {
            use indicatif::{ProgressBar, ProgressStyle};
            let spinner = ProgressBar::new_spinner();
            spinner.set_style(
                ProgressStyle::default_spinner()
                    .template("{spinner:.green} {msg}")
                    .unwrap(),
            );
            spinner.set_message(format!("Cloning VM {} as '{}'...", vm_id, name));
            spinner.enable_steady_tick(std::time::Duration::from_millis(100));

            // The API clones synchronously and answers with the new VM.
            let vm: serde_json::Value = api
                .post(
                    &format!("/api/vms/{}/clone", vm_id),
                    &create_body(&name, linked, start),
                )
                .await?;

            spinner.finish_and_clear();
            let new_id = vm["id"].as_str().unwrap_or("?");
            output::print_created("VM clone", &name, new_id);
            output::print_info(&format!(
                "Clone type: {}",
                if linked { "Linked" } else { "Full" }
            ));
            output::print_info("Clone jobs: horcrux clone list");
        }

        CloneCommands::List => {
            let jobs: Vec<CloneJob> = api.get(jobs_path()).await?;
            let format = OutputFormat::from_str(output_format);
            let rows: Vec<CloneJobRow> = jobs.into_iter().map(CloneJobRow::from).collect();
            output::print_output(rows, format)?;
        }

        CloneCommands::Status { job_id } => {
            let job: CloneJob = api.get(&job_path(&job_id)).await?;
            let format = OutputFormat::from_str(output_format);

            if format == OutputFormat::Table && (job.state == "running" || job.state == "queued") {
                use indicatif::{ProgressBar, ProgressStyle};
                let pb = ProgressBar::new(100);
                pb.set_style(
                    ProgressStyle::default_bar()
                        .template("{msg} [{bar:40.cyan/blue}] {pos}%")
                        .unwrap()
                        .progress_chars("=> "),
                );
                pb.set_message("Progress");
                pb.set_position(u64::from(job.progress.min(100)));
                pb.finish();
            }
            output::print_single(&job, format)?;
        }

        CloneCommands::Cancel { job_id } => {
            use dialoguer::Confirm;

            let confirm = Confirm::new()
                .with_prompt(format!(
                    "Are you sure you want to cancel clone job {}?",
                    job_id
                ))
                .interact()?;

            if confirm {
                api.post::<serde_json::Value, _>(&cancel_path(&job_id), &serde_json::json!({}))
                    .await?;
                output::print_success(&format!("Clone job '{}' cancellation requested", job_id));
            } else {
                output::print_info("Cancellation aborted");
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_are_the_api_clone_job_routes() {
        assert_eq!(jobs_path(), "/api/clone-jobs");
        assert_eq!(job_path("j1"), "/api/clone-jobs/j1");
        assert_eq!(cancel_path("j1"), "/api/clone-jobs/j1/cancel");
    }

    #[test]
    fn create_body_sends_the_mode() {
        assert_eq!(create_body("c", false, false)["mode"], "full");
        assert_eq!(create_body("c", true, true)["mode"], "linked");
        assert_eq!(create_body("c", true, true)["start"], true);
    }

    /// What GET /api/clone-jobs returns (serde of the API's CloneJob).
    #[test]
    fn parses_the_api_clone_job() {
        let j: CloneJob = serde_json::from_str(
            r#"{"id":"5b1c","source_vm_id":"vm-1","target_vm_id":"vm-2","target_vm_name":"web-clone",
                "state":"completed","stage":"finalizing","progress":100,"total_size_bytes":null,
                "copied_bytes":null,"created_at":"2026-10-18T12:00:00Z","started_at":"2026-10-18T12:00:00Z",
                "completed_at":"2026-10-18T12:00:02Z","error_message":null,"cancellation_requested":false}"#,
        )
        .unwrap();
        let row = CloneJobRow::from(j);
        assert_eq!(row.target, "web-clone (vm-2)");
        assert_eq!(row.stage, "finalizing");
        assert_eq!(row.progress, "100%");
        let q: CloneJob = serde_json::from_str(
            r#"{"id":"x","source_vm_id":"a","target_vm_id":"b","target_vm_name":"b","state":"queued",
                "stage":null,"progress":0,"created_at":"2026-10-18T12:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!(CloneJobRow::from(q).stage, "-");
    }
}
