use crate::api::ApiClient;
use crate::output;
use crate::BackupCommands;
use anyhow::Result;
use serde::{Deserialize, Serialize};

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

            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&backups)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&backups)?);
            } else {
                println!("{:<36} {:<20} {:<12} {:<12} {:<10} {}",
                    "ID", "VM", "MODE", "COMPRESSION", "SIZE", "CREATED");
                println!("{}", "-".repeat(110));
                for backup in backups {
                    let size_gb = backup.size as f64 / 1024.0 / 1024.0 / 1024.0;
                    let created = chrono::DateTime::from_timestamp(backup.created_at, 0)
                        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_else(|| "Unknown".to_string());
                    println!("{:<36} {:<20} {:<12} {:<12} {:<10.2} {}",
                        backup.id, backup.vm_name, backup.mode, backup.compression,
                        size_gb, created);
                }
            }
        }
        BackupCommands::Show { id } => {
            let backup: Backup = api.get(&format!("/api/backups/{}", id)).await?;

            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&backup)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&backup)?);
            } else {
                let size_gb = backup.size as f64 / 1024.0 / 1024.0 / 1024.0;
                let created = chrono::DateTime::from_timestamp(backup.created_at, 0)
                    .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                    .unwrap_or_else(|| "Unknown".to_string());
                println!("Backup: {}", backup.id);
                println!("  VM:          {} ({})", backup.vm_name, backup.vm_id);
                println!("  Path:        {}", backup.path);
                println!("  Size:        {:.2} GB", size_gb);
                println!("  Mode:        {}", backup.mode);
                println!("  Compression: {}", backup.compression);
                println!("  Created:     {}", created);
            }
        }
        BackupCommands::Create { vm_id, mode, compression } => {
            let request = CreateBackupRequest {
                vm_id: vm_id.clone(),
                mode: mode.clone(),
                compression: compression.clone(),
            };

            let backup: Backup = api.post("/api/backups", &request).await?;
            output::print_success(&format!("Backup created successfully (ID: {})", backup.id));
        }
        BackupCommands::Restore { id, target } => {
            let request = RestoreBackupRequest {
                target_vm_id: target,
            };

            api.post_empty(&format!("/api/backups/{}/restore", id), &request).await?;
            output::print_success(&format!("Backup {} restored successfully", id));
        }
        BackupCommands::Delete { id } => {
            api.delete(&format!("/api/backups/{}", id)).await?;
            output::print_success(&format!("Backup {} deleted successfully", id));
        }
        BackupCommands::Schedule { name, schedule, vms } => {
            let vm_ids: Vec<String> = vms.split(',').map(|s| s.trim().to_string()).collect();
            let request = CreateBackupJobRequest {
                name: name.clone(),
                schedule: schedule.clone(),
                vm_ids,
            };

            let job: BackupJob = api.post("/api/backup-jobs", &request).await?;
            output::print_success(&format!("Backup job '{}' scheduled successfully (ID: {})", name, job.id));
        }
    }
    Ok(())
}
