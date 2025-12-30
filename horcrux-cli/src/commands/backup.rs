use crate::api::ApiClient;
use crate::output::{self, OutputFormat, format_bytes, format_relative_time};
use crate::BackupCommands;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use tabled::Tabled;

#[derive(Debug, Serialize, Deserialize)]
struct Backup {
    id: String,
    vm_id: String,
    vm_name: String,
    path: String,
    size: u64,
    mode: String,
    compression: String,
    created_at: i64,
}

#[derive(Tabled, Serialize)]
struct BackupRow {
    id: String,
    vm_name: String,
    mode: String,
    compression: String,
    size: String,
    created: String,
}

impl From<Backup> for BackupRow {
    fn from(b: Backup) -> Self {
        Self {
            id: b.id,
            vm_name: b.vm_name,
            mode: b.mode,
            compression: b.compression,
            size: format_bytes(b.size),
            created: format_relative_time(b.created_at),
        }
    }
}

#[derive(Serialize)]
struct CreateBackupRequest {
    vm_id: String,
    mode: String,
    compression: String,
}

#[derive(Serialize)]
struct RestoreBackupRequest {
    target_vm_id: Option<String>,
}

#[derive(Serialize)]
struct CreateBackupJobRequest {
    name: String,
    schedule: String,
    vm_ids: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct BackupJob {
    id: String,
    name: String,
    schedule: String,
    vm_ids: Vec<String>,
}

pub async fn handle_backup_command(
    command: BackupCommands,
    api: &ApiClient,
    output_format: &str,
) -> Result<()> {
    match command {
        BackupCommands::List => {
            let backups: Vec<Backup> = api.get("/api/backups").await?;
            let format = OutputFormat::from_str(output_format);
            let rows: Vec<BackupRow> = backups.into_iter().map(BackupRow::from).collect();
            output::print_output(rows, format)?;
        }
        BackupCommands::Show { id } => {
            let backup: Backup = api.get(&format!("/api/backups/{}", id)).await?;
            let format = OutputFormat::from_str(output_format);
            output::print_single(&backup, format)?;
        }
        BackupCommands::Create { vm_id, mode, compression } => {
            let request = CreateBackupRequest {
                vm_id: vm_id.clone(),
                mode: mode.clone(),
                compression: compression.clone(),
            };

            let backup: Backup = api.post("/api/backups", &request).await?;
            output::print_created("Backup", &backup.vm_name, &backup.id);
        }
        BackupCommands::Restore { id, target } => {
            let request = RestoreBackupRequest {
                target_vm_id: target,
            };

            api.post_empty(&format!("/api/backups/{}/restore", id), &request).await?;
            output::print_success(&format!("Backup '{}' restored successfully", id));
        }
        BackupCommands::Delete { id } => {
            api.delete(&format!("/api/backups/{}", id)).await?;
            output::print_deleted("Backup", &id);
        }
        BackupCommands::Schedule { name, schedule, vms } => {
            let vm_ids: Vec<String> = vms.split(',').map(|s| s.trim().to_string()).collect();
            let request = CreateBackupJobRequest {
                name: name.clone(),
                schedule: schedule.clone(),
                vm_ids,
            };

            let job: BackupJob = api.post("/api/backup-jobs", &request).await?;
            output::print_created("Backup job", &name, &job.id);
        }
    }
    Ok(())
}
