///! VM snapshot management commands

use crate::api::ApiClient;
use crate::output;
use crate::SnapshotCommands;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use tabled::Tabled;

#[derive(Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: String,
    pub vm_id: String,
    pub name: String,
    pub description: Option<String>,
    pub created_at: String,
    pub size_bytes: u64,
    pub include_memory: bool,
    pub parent_snapshot: Option<String>,
}

#[derive(Tabled)]
struct SnapshotRow {
    id: String,
    name: String,
    created_at: String,
    size_mb: u64,
    memory: String,
}

impl From<Snapshot> for SnapshotRow {
    fn from(s: Snapshot) -> Self {
        Self {
            id: s.id,
            name: s.name,
            created_at: s.created_at,
            size_mb: s.size_bytes / 1024 / 1024,
            memory: if s.include_memory { "Yes" } else { "No" }.to_string(),
        }
    }
}

pub async fn handle_snapshot_command(
    command: SnapshotCommands,
    api: &ApiClient,
    output_format: &str,
) -> Result<()> {
    match command {
        SnapshotCommands::List { vm_id } => {
            let snapshots: Vec<Snapshot> = api.get(&format!("/api/vms/{}/snapshots", vm_id)).await?;

            if output_format == "json" {
                output::print_json(&snapshots)?;
            } else {
                let rows: Vec<SnapshotRow> = snapshots.into_iter().map(SnapshotRow::from).collect();
                output::print_table(rows);
            }
        }

        SnapshotCommands::Show { vm_id, snapshot_id } => {
            let snapshot: Snapshot = api
                .get(&format!("/api/vms/{}/snapshots/{}", vm_id, snapshot_id))
                .await?;

            if output_format == "json" {
                output::print_json(&snapshot)?;
            } else {
                println!("Snapshot Details:");
                println!("  ID: {}", snapshot.id);
                println!("  VM ID: {}", snapshot.vm_id);
                println!("  Name: {}", snapshot.name);
                if let Some(desc) = snapshot.description {
                    println!("  Description: {}", desc);
                }
                println!("  Created: {}", snapshot.created_at);
                println!("  Size: {} MB", snapshot.size_bytes / 1024 / 1024);
                println!("  Include Memory: {}", snapshot.include_memory);
                if let Some(parent) = snapshot.parent_snapshot {
                    println!("  Parent: {}", parent);
                }
            }
        }

        SnapshotCommands::Create {
            vm_id,
            name,
            description,
            include_memory,
        } => {
            #[derive(Serialize)]
            struct CreateRequest {
                name: String,
                description: Option<String>,
                include_memory: bool,
            }

            let request = CreateRequest {
                name: name.clone(),
                description,
                include_memory,
            };

            use indicatif::{ProgressBar, ProgressStyle};
            let spinner = ProgressBar::new_spinner();
            spinner.set_style(
                ProgressStyle::default_spinner()
                    .template("{spinner:.green} {msg}")
                    .unwrap()
            );
            spinner.set_message(format!("Creating snapshot '{}'...", name));
            spinner.enable_steady_tick(std::time::Duration::from_millis(100));

            let snapshot: Snapshot = api
                .post(&format!("/api/vms/{}/snapshots", vm_id), &request)
                .await?;

            spinner.finish_with_message(format!("Snapshot created: {}", snapshot.name));
            output::print_success(&format!("Snapshot ID: {}", snapshot.id));
        }

        SnapshotCommands::Restore { vm_id, snapshot_id } => {
            use dialoguer::Confirm;

            let confirm = Confirm::new()
                .with_prompt(format!(
                    "Are you sure you want to restore VM {} to snapshot {}? Current state will be lost.",
                    vm_id, snapshot_id
                ))
                .interact()?;

            if !confirm {
                output::print_info("Restore cancelled");
                return Ok(());
            }

            use indicatif::{ProgressBar, ProgressStyle};
            let spinner = ProgressBar::new_spinner();
            spinner.set_style(
                ProgressStyle::default_spinner()
                    .template("{spinner:.green} {msg}")
                    .unwrap()
            );
            spinner.set_message("Restoring snapshot...");
            spinner.enable_steady_tick(std::time::Duration::from_millis(100));

            api.post::<serde_json::Value, _>(
                &format!("/api/vms/{}/snapshots/{}/restore", vm_id, snapshot_id),
                &serde_json::json!({}),
            )
            .await?;

            spinner.finish_with_message("Snapshot restored successfully");
        }

        SnapshotCommands::Delete { vm_id, snapshot_id } => {
            use dialoguer::Confirm;

            let confirm = Confirm::new()
                .with_prompt(format!(
                    "Are you sure you want to delete snapshot {}?",
                    snapshot_id
                ))
                .interact()?;

            if confirm {
                api.delete(&format!("/api/vms/{}/snapshots/{}", vm_id, snapshot_id))
                    .await?;
                output::print_success(&format!("Snapshot deleted: {}", snapshot_id));
            } else {
                output::print_info("Deletion cancelled");
            }
        }

        SnapshotCommands::Tree { vm_id } => {
            let snapshots: Vec<Snapshot> = api.get(&format!("/api/vms/{}/snapshots", vm_id)).await?;

            if snapshots.is_empty() {
                output::print_info("No snapshots found for this VM");
                return Ok(());
            }

            // Build a simple tree representation
            println!("Snapshot Tree for VM {}:", vm_id);
            println!();

            // Find root snapshots (no parent)
            let roots: Vec<&Snapshot> = snapshots
                .iter()
                .filter(|s| s.parent_snapshot.is_none())
                .collect();

            fn print_snapshot_tree(
                snapshot: &Snapshot,
                all_snapshots: &[Snapshot],
                prefix: &str,
                is_last: bool,
            ) {
                let connector = if is_last { "└── " } else { "├── " };
                println!(
                    "{}{}{} ({}) - {} MB",
                    prefix,
                    connector,
                    snapshot.name,
                    snapshot.id,
                    snapshot.size_bytes / 1024 / 1024
                );

                // Find children
                let children: Vec<&Snapshot> = all_snapshots
                    .iter()
                    .filter(|s| s.parent_snapshot.as_ref() == Some(&snapshot.id))
                    .collect();

                let new_prefix = format!("{}{}   ", prefix, if is_last { " " } else { "│" });
                for (i, child) in children.iter().enumerate() {
                    let is_last_child = i == children.len() - 1;
                    print_snapshot_tree(child, all_snapshots, &new_prefix, is_last_child);
                }
            }

            for (i, root) in roots.iter().enumerate() {
                let is_last = i == roots.len() - 1;
                print_snapshot_tree(root, &snapshots, "", is_last);
            }
        }
    }

    Ok(())
}
