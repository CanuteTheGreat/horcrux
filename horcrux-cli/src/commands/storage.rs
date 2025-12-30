use crate::api::ApiClient;
use crate::output::{self, OutputFormat, format_bytes};
use crate::StorageCommands;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use tabled::Tabled;

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

#[derive(Tabled, Serialize)]
struct StoragePoolRow {
    id: String,
    name: String,
    #[tabled(rename = "type")]
    storage_type: String,
    available: String,
    total: String,
    path: String,
}

impl From<StoragePool> for StoragePoolRow {
    fn from(pool: StoragePool) -> Self {
        Self {
            id: pool.id,
            name: pool.name,
            storage_type: pool.storage_type,
            available: if pool.available > 0 {
                format_bytes(pool.available * 1024 * 1024 * 1024)
            } else {
                "N/A".to_string()
            },
            total: if pool.total > 0 {
                format_bytes(pool.total * 1024 * 1024 * 1024)
            } else {
                "N/A".to_string()
            },
            path: pool.path,
        }
    }
}

#[derive(Serialize)]
struct CreatePoolRequest {
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
            let format = OutputFormat::from_str(output_format);
            let rows: Vec<StoragePoolRow> = pools.into_iter().map(StoragePoolRow::from).collect();
            output::print_output(rows, format)?;
        }
        StorageCommands::Show { id } => {
            let pool: StoragePool = api.get(&format!("/api/storage/pools/{}", id)).await?;
            let format = OutputFormat::from_str(output_format);
            output::print_single(&pool, format)?;
        }
        StorageCommands::Create { name, storage_type, path } => {
            let request = CreatePoolRequest {
                name: name.clone(),
                storage_type: storage_type.clone(),
                path: path.clone(),
            };

            let pool: StoragePool = api.post("/api/storage/pools", &request).await?;
            output::print_created("Storage pool", &name, &pool.id);
        }
        StorageCommands::Delete { id } => {
            api.delete(&format!("/api/storage/pools/{}", id)).await?;
            output::print_deleted("Storage pool", &id);
        }
        StorageCommands::CreateVolume { pool_id, name, size } => {
            let request = CreateVolumeRequest {
                name: name.clone(),
                size,
            };

            api.post_empty(&format!("/api/storage/pools/{}/volumes", pool_id), &request).await?;
            output::print_created("Volume", &name, &format!("{} GB", size));
        }
    }
    Ok(())
}
