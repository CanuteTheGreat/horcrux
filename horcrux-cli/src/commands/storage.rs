use crate::api::ApiClient;
use crate::output;
use crate::StorageCommands;
use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct StoragePool {
    id: String,
    name: String,
    storage_type: String,
    path: String,
    available: u64,
    total: u64,
    enabled: bool,
}

#[derive(Serialize)]
struct AddPoolRequest {
    name: String,
    storage_type: String,
    path: String,
}

#[derive(Serialize)]
struct CreateVolumeRequest {
    name: String,
    size: u64,
}

pub async fn handle_storage_command(
    command: StorageCommands,
    api: &ApiClient,
    output_format: &str,
) -> Result<()> {
    match command {
        StorageCommands::List => {
            let pools: Vec<StoragePool> = api.get("/api/storage/pools").await?;

            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&pools)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&pools)?);
            } else {
                // Table format
                println!("{:<36} {:<20} {:<12} {:<12} {:<12} {}",
                    "ID", "NAME", "TYPE", "AVAILABLE", "TOTAL", "PATH");
                println!("{}", "-".repeat(120));
                for pool in pools {
                    let avail = if pool.available > 0 {
                        format!("{} GB", pool.available)
                    } else {
                        "N/A".to_string()
                    };
                    let total = if pool.total > 0 {
                        format!("{} GB", pool.total)
                    } else {
                        "N/A".to_string()
                    };
                    println!("{:<36} {:<20} {:<12} {:<12} {:<12} {}",
                        pool.id, pool.name, pool.storage_type, avail, total, pool.path);
                }
            }
        }
        StorageCommands::Show { id } => {
            let pool: StoragePool = api.get(&format!("/api/storage/pools/{}", id)).await?;

            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&pool)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&pool)?);
            } else {
                println!("Storage Pool: {}", pool.name);
                println!("  ID:        {}", pool.id);
                println!("  Type:      {}", pool.storage_type);
                println!("  Path:      {}", pool.path);
                println!("  Available: {} GB", pool.available);
                println!("  Total:     {} GB", pool.total);
                println!("  Enabled:   {}", pool.enabled);
            }
        }
        StorageCommands::Add { name, storage_type, path } => {
            let request = AddPoolRequest {
                name: name.clone(),
                storage_type: storage_type.clone(),
                path: path.clone(),
            };

            let pool: StoragePool = api.post("/api/storage/pools", &request).await?;
            output::print_success(&format!("Storage pool '{}' added successfully (ID: {})", name, pool.id));
        }
        StorageCommands::Remove { id } => {
            api.delete(&format!("/api/storage/pools/{}", id)).await?;
            output::print_success(&format!("Storage pool {} removed successfully", id));
        }
        StorageCommands::CreateVolume { pool_id, name, size } => {
            let request = CreateVolumeRequest {
                name: name.clone(),
                size,
            };

            api.post_empty(&format!("/api/storage/pools/{}/volumes", pool_id), &request).await?;
            output::print_success(&format!("Volume '{}' created successfully ({} GB)", name, size));
        }
    }
    Ok(())
}
