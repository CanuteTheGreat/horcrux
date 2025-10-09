use crate::api::ApiClient;
use crate::output;
use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct MigrateRequest {
    target_node: String,
    migration_type: Option<String>,
    online: Option<bool>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct MigrateResponse {
    // Job ID returned from migration API
}

pub async fn handle_migrate_command(
    vm_id: &str,
    target_node: &str,
    migration_type: &str,
    api: &ApiClient,
) -> Result<()> {
    output::print_info(&format!(
        "Starting migration of VM {} to {} (type: {})...",
        vm_id, target_node, migration_type
    ));

    // Determine if migration should be online/offline
    let online = migration_type == "live" || migration_type == "online";

    let request = MigrateRequest {
        target_node: target_node.to_string(),
        migration_type: Some(migration_type.to_string()),
        online: Some(online),
    };

    // Call migration API
    let response: String = api.post(&format!("/api/migrate/{}", vm_id), &request).await?;

    output::print_success(&format!(
        "Migration started successfully! Job ID: {}",
        response
    ));

    output::print_info(&format!(
        "Use 'horcrux monitor vm {}' to check migration status",
        vm_id
    ));

    Ok(())
}
