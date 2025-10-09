///! VM management commands

use crate::api::ApiClient;
use crate::output;
use crate::VmCommands;
use anyhow::Result;
use horcrux_common::VmConfig;
use tabled::Tabled;

#[derive(Tabled)]
struct VmRow {
    id: String,
    name: String,
    status: String,
    cpus: u32,
    memory_mb: u64,
    architecture: String,
}

impl From<VmConfig> for VmRow {
    fn from(vm: VmConfig) -> Self {
        Self {
            id: vm.id,
            name: vm.name,
            status: format!("{:?}", vm.status),
            cpus: vm.cpus,
            memory_mb: vm.memory / 1024 / 1024,
            architecture: format!("{:?}", vm.architecture),
        }
    }
}

pub async fn handle_vm_command(
    command: VmCommands,
    api: &ApiClient,
    output_format: &str,
) -> Result<()> {
    match command {
        VmCommands::List => {
            let vms: Vec<VmConfig> = api.get("/api/vms").await?;

            if output_format == "json" {
                output::print_json(&vms)?;
            } else {
                let rows: Vec<VmRow> = vms.into_iter().map(VmRow::from).collect();
                output::print_table(rows);
            }
        }

        VmCommands::Show { id } => {
            let vm: VmConfig = api.get(&format!("/api/vms/{}", id)).await?;

            if output_format == "json" {
                output::print_json(&vm)?;
            } else {
                println!("VM Details:");
                println!("  ID: {}", vm.id);
                println!("  Name: {}", vm.name);
                println!("  Status: {:?}", vm.status);
                println!("  CPUs: {}", vm.cpus);
                println!("  Memory: {} MB", vm.memory / 1024 / 1024);
                println!("  Architecture: {:?}", vm.architecture);
                println!("  Hypervisor: {:?}", vm.hypervisor);
                println!("  Disk Size: {} GB", vm.disk_size);
            }
        }

        VmCommands::Create {
            name,
            memory,
            cpus,
            disk,
        } => {
            use horcrux_common::{VmStatus, VmArchitecture, VmHypervisor};

            let vm_config = VmConfig {
                id: format!("vm-{}", chrono::Utc::now().timestamp()),
                name: name.clone(),
                hypervisor: VmHypervisor::Qemu,
                memory,
                cpus,
                disk_size: disk,
                architecture: VmArchitecture::X86_64,
                status: VmStatus::Stopped,
            };

            let vm: VmConfig = api.post("/api/vms", &vm_config).await?;
            output::print_success(&format!("VM created: {} ({})", vm.name, vm.id));
        }

        VmCommands::Start { id } => {
            let vm: VmConfig = api
                .post(&format!("/api/vms/{}/start", id), &serde_json::json!({}))
                .await?;
            output::print_success(&format!("VM started: {}", vm.name));
        }

        VmCommands::Stop { id } => {
            let vm: VmConfig = api
                .post(&format!("/api/vms/{}/stop", id), &serde_json::json!({}))
                .await?;
            output::print_success(&format!("VM stopped: {}", vm.name));
        }

        VmCommands::Restart { id } => {
            // Stop then start
            api.post::<VmConfig, _>(
                &format!("/api/vms/{}/stop", id),
                &serde_json::json!({}),
            )
            .await?;
            output::print_info(&format!("VM {} stopped, restarting...", id));

            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

            let vm: VmConfig = api
                .post(&format!("/api/vms/{}/start", id), &serde_json::json!({}))
                .await?;
            output::print_success(&format!("VM restarted: {}", vm.name));
        }

        VmCommands::Delete { id } => {
            use dialoguer::Confirm;

            let confirm = Confirm::new()
                .with_prompt(format!("Are you sure you want to delete VM {}?", id))
                .interact()?;

            if confirm {
                api.delete(&format!("/api/vms/{}", id)).await?;
                output::print_success(&format!("VM deleted: {}", id));
            } else {
                output::print_info("Deletion cancelled");
            }
        }

        VmCommands::Clone { template_id, name } => {
            #[derive(serde::Serialize)]
            struct CloneRequest {
                name: String,
                target_storage: String,
            }

            let request = CloneRequest {
                name,
                target_storage: "local".to_string(),
            };

            #[derive(serde::Deserialize)]
            struct CloneResponse {
                new_vm_id: String,
            }

            let response: CloneResponse = api
                .post(&format!("/api/templates/{}/clone", template_id), &request)
                .await?;

            output::print_success(&format!(
                "VM cloned from template {}: {}",
                template_id, response.new_vm_id
            ));
        }
    }

    Ok(())
}
