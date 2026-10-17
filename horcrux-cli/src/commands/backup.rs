use crate::api::ApiClient;
use crate::output::{self, format_bytes, format_relative_time, OutputFormat};
use crate::{BackupCommands, KeepArgs};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tabled::Tabled;

/// A backup as returned by `GET /api/backups`.
#[derive(Debug, Serialize, Deserialize)]
struct Backup {
    id: String,
    target_type: String,
    target_id: String,
    target_name: String,
    timestamp: i64,
    size: u64,
    mode: String,
    compression: String,
    path: String,
    #[serde(default)]
    storage: String,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    protected: bool,
    #[serde(default)]
    sha256: String,
    #[serde(default)]
    job_id: Option<String>,
    #[serde(default)]
    verify_state: Option<String>,
    #[serde(default)]
    verify_error: Option<String>,
}

#[derive(Tabled, Serialize)]
struct BackupRow {
    id: String,
    guest: String,
    mode: String,
    compression: String,
    size: String,
    storage: String,
    verified: String,
    created: String,
}

impl From<Backup> for BackupRow {
    fn from(b: Backup) -> Self {
        Self {
            id: b.id,
            guest: format!("{} {}", b.target_type, b.target_id),
            mode: b.mode,
            compression: b.compression,
            size: format_bytes(b.size),
            storage: b.storage,
            verified: b.verify_state.unwrap_or_else(|| "-".into()),
            created: format_relative_time(b.timestamp),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct BackupJob {
    id: String,
    schedule: String,
    targets: Vec<String>,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    storage: String,
    #[serde(default)]
    last_status: Option<String>,
    #[serde(default)]
    last_run: Option<i64>,
    #[serde(default)]
    next_run: Option<i64>,
    #[serde(default)]
    last_error: Option<String>,
}

#[derive(Tabled, Serialize)]
struct JobRow {
    id: String,
    schedule: String,
    targets: String,
    enabled: bool,
    last_run: String,
    status: String,
}

impl From<BackupJob> for JobRow {
    fn from(j: BackupJob) -> Self {
        Self {
            id: j.id,
            schedule: j.schedule,
            targets: j.targets.join(","),
            enabled: j.enabled,
            last_run: j
                .last_run
                .map(format_relative_time)
                .unwrap_or_else(|| "never".into()),
            status: match (j.last_status, j.last_error) {
                (Some(s), Some(e)) => format!("{s}: {e}"),
                (Some(s), None) => s,
                _ => "-".into(),
            },
        }
    }
}

fn keep_json(k: &KeepArgs) -> Value {
    json!({
        "keep_last": k.keep_last,
        "keep_hourly": k.keep_hourly,
        "keep_daily": k.keep_daily,
        "keep_weekly": k.keep_weekly,
        "keep_monthly": k.keep_monthly,
        "keep_yearly": k.keep_yearly,
    })
}

pub async fn handle_backup_command(
    command: BackupCommands,
    api: &ApiClient,
    output_format: &str,
) -> Result<()> {
    let format = OutputFormat::from_str(output_format);
    match command {
        BackupCommands::List { target } => {
            let path = match target {
                Some(t) => format!("/api/backups?target_id={t}"),
                None => "/api/backups".to_string(),
            };
            let backups: Vec<Backup> = api.get(&path).await?;
            let rows: Vec<BackupRow> = backups.into_iter().map(BackupRow::from).collect();
            output::print_output(rows, format)?;
        }
        BackupCommands::Show { id } => {
            let backup: Value = api.get(&format!("/api/backups/{}", id)).await?;
            output::print_single(&backup, format)?;
        }
        BackupCommands::Create {
            target_id,
            mode,
            compression,
            storage,
            notes,
            protected,
        } => {
            let backup: Backup = api
                .post(
                    "/api/backups",
                    &json!({
                        "target_id": target_id,
                        "mode": mode,
                        "compression": compression,
                        "storage": storage,
                        "notes": notes,
                        "protected": protected,
                    }),
                )
                .await?;
            output::print_created("Backup", &backup.path, &backup.id);
        }
        BackupCommands::Restore {
            id,
            target,
            name,
            force,
            start,
        } => {
            let r: Value = api
                .post(
                    &format!("/api/backups/{}/restore", id),
                    &json!({"target_id": target, "name": name, "force": force, "start": start}),
                )
                .await?;
            output::print_success(&format!(
                "Backup '{}' restored to {} {}",
                id,
                r["target_type"].as_str().unwrap_or(""),
                r["target_id"].as_str().unwrap_or("")
            ));
        }
        BackupCommands::Verify { id } => {
            let v: Value = api
                .post(&format!("/api/backups/{}/verify", id), &json!({}))
                .await?;
            if v["ok"].as_bool() == Some(true) {
                output::print_success(&format!("Backup '{}' verified OK", id));
            } else {
                anyhow::bail!(
                    "backup '{}' FAILED verification: {}",
                    id,
                    v["error"].as_str().unwrap_or("unknown error")
                );
            }
        }
        BackupCommands::Delete { id } => {
            api.delete(&format!("/api/backups/{}", id)).await?;
            output::print_deleted("Backup", &id);
        }
        BackupCommands::Prune {
            target_id,
            keep,
            storage,
            dry_run,
        } => {
            let mut body = keep_json(&keep);
            body["target_id"] = json!(target_id);
            body["storage"] = json!(storage);
            body["dry_run"] = json!(dry_run);
            let d: Vec<Value> = api.post("/api/backups/prune", &body).await?;
            for x in &d {
                println!(
                    "{}  {}",
                    x["mark"].as_str().unwrap_or(""),
                    x["backup_id"].as_str().unwrap_or("")
                );
            }
            let removed = d.iter().filter(|x| x["mark"] == "remove").count();
            output::print_success(&format!(
                "{} {} of {} backup(s)",
                if dry_run { "Would remove" } else { "Removed" },
                removed,
                d.len()
            ));
        }
        BackupCommands::Schedule {
            name,
            schedule,
            vms,
            storage,
            mode,
            compression,
            keep,
        } => {
            let targets: Vec<String> = vms
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            let job: BackupJob = api
                .post(
                    "/api/backup-jobs",
                    &json!({
                        "id": name, "schedule": schedule, "targets": targets,
                        "storage": storage, "mode": mode, "compression": compression,
                        "retention": keep_json(&keep),
                    }),
                )
                .await?;
            output::print_created("Backup job", &job.schedule, &job.id);
        }
        BackupCommands::Jobs => {
            let jobs: Vec<BackupJob> = api.get("/api/backup-jobs").await?;
            let rows: Vec<JobRow> = jobs.into_iter().map(JobRow::from).collect();
            output::print_output(rows, format)?;
        }
        BackupCommands::RunJob { id } => {
            api.post_empty(&format!("/api/backup-jobs/{}/run", id), &json!({}))
                .await?;
            output::print_started("Backup job", &id);
        }
        BackupCommands::DeleteJob { id } => {
            api.delete(&format!("/api/backup-jobs/{}", id)).await?;
            output::print_deleted("Backup job", &id);
        }
    }
    Ok(())
}
