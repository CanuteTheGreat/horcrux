//! VM management commands
use crate::api::ApiClient;
use crate::output::{self, OutputFormat};
use crate::VmCommands;
use anyhow::Result;
use horcrux_common::VmConfig;
use serde::Serialize;
use tabled::Tabled;

#[derive(Tabled, Serialize)]
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
            // VmConfig.memory is already MB (it was divided by 1024*1024
            // again, so every VM showed 0).
            memory_mb: vm.memory,
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
            let format = OutputFormat::from_str(output_format);
            let rows: Vec<VmRow> = vms.into_iter().map(VmRow::from).collect();
            output::print_output(rows, format)?;
        }

        VmCommands::Show { id } => {
            let vm: VmConfig = api.get(&format!("/api/vms/{}", id)).await?;
            let format = OutputFormat::from_str(output_format);
            output::print_single(&vm, format)?;
        }

        VmCommands::Create {
            name,
            memory,
            cpus,
            disk,
        } => {
            use horcrux_common::{VmArchitecture, VmHypervisor, VmStatus};

            let vm_config = VmConfig {
                id: format!("vm-{}", chrono::Utc::now().timestamp()),
                name: name.clone(),
                hypervisor: VmHypervisor::Qemu,
                memory,
                cpus,
                disk_size: disk,
                architecture: VmArchitecture::X86_64,
                status: VmStatus::Stopped,
                disks: Vec::new(), // Empty disk list, will be populated by API
                display: Default::default(),
                hardware: Default::default(),
            };

            let vm: VmConfig = api.post("/api/vms", &vm_config).await?;
            output::print_created("VM", &vm.name, &vm.id);
        }

        VmCommands::Start { id } => {
            let vm: VmConfig = api
                .post(&format!("/api/vms/{}/start", id), &serde_json::json!({}))
                .await?;
            output::print_started("VM", &vm.name);
        }

        VmCommands::Stop { id } => {
            let vm: VmConfig = api
                .post(&format!("/api/vms/{}/stop", id), &serde_json::json!({}))
                .await?;
            output::print_stopped("VM", &vm.name);
        }

        VmCommands::Restart { id } => {
            // Stop then start
            api.post::<VmConfig, _>(&format!("/api/vms/{}/stop", id), &serde_json::json!({}))
                .await?;
            output::print_info(&format!("VM {} stopped, restarting...", id));

            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

            let vm: VmConfig = api
                .post(&format!("/api/vms/{}/start", id), &serde_json::json!({}))
                .await?;
            output::print_success(&format!("VM '{}' restarted", vm.name));
        }

        VmCommands::Reset { id } => {
            let vm: VmConfig = api
                .post(&format!("/api/vms/{}/reset", id), &serde_json::json!({}))
                .await?;
            output::print_success(&format!("VM '{}' reset", vm.name));
        }

        VmCommands::Exec {
            id,
            timeout,
            env,
            stdin,
            command,
        } => {
            let stdin = match stdin.as_deref() {
                None => None,
                Some("-") => {
                    let mut buf = Vec::new();
                    std::io::Read::read_to_end(&mut std::io::stdin(), &mut buf)?;
                    Some(buf)
                }
                Some(path) => Some(std::fs::read(path)?),
            };
            let body = exec_body(&command, &env, timeout, stdin.as_deref());
            let r: serde_json::Value = api
                .post(&format!("/api/vms/{}/guest-exec", id), &body)
                .await?;
            let code = exec_report(&r, output_format)?;
            if code != 0 {
                std::process::exit(code);
            }
        }

        VmCommands::Delete { id } => {
            use dialoguer::Confirm;

            let confirm = Confirm::new()
                .with_prompt(format!("Are you sure you want to delete VM {}?", id))
                .interact()?;

            if confirm {
                api.delete(&format!("/api/vms/{}", id)).await?;
                output::print_deleted("VM", &id);
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

/// The request body of `POST /api/vms/:id/guest-exec` for a CLI invocation
/// (output always comes back base64, so binary output survives).
pub fn exec_body(
    command: &[String],
    env: &[String],
    timeout: Option<u64>,
    stdin: Option<&[u8]>,
) -> serde_json::Value {
    use base64::Engine as _;
    let mut b = serde_json::json!({
        "path": command[0],
        "args": &command[1..],
        "env": env,
        "encoding": "base64",
    });
    if let Some(t) = timeout {
        b["timeout_secs"] = t.into();
    }
    if let Some(s) = stdin {
        b["stdin_base64"] = base64::engine::general_purpose::STANDARD.encode(s).into();
    }
    b
}

/// The process exit status that stands for a guest command's result.
pub fn exec_exit_code(r: &serde_json::Value) -> i32 {
    if r["timed_out"].as_bool() == Some(true) {
        124
    } else if let Some(sig) = r["signal"].as_i64() {
        128 + sig as i32
    } else {
        r["exit_code"].as_i64().unwrap_or(1) as i32
    }
}

/// Print a guest command's output (raw on stdout/stderr; the whole answer
/// for `-o json|yaml`) and return the status to exit with.
fn exec_report(r: &serde_json::Value, output_format: &str) -> Result<i32> {
    use base64::Engine as _;
    use std::io::Write as _;
    let code = exec_exit_code(r);
    if output_format != "table" {
        println!("{}", serde_json::to_string_pretty(r)?);
        return Ok(code);
    }
    let dec = |k: &str| {
        base64::engine::general_purpose::STANDARD
            .decode(r[k].as_str().unwrap_or(""))
            .unwrap_or_default()
    };
    std::io::stdout().write_all(&dec("stdout"))?;
    std::io::stderr().write_all(&dec("stderr"))?;
    if r["timed_out"].as_bool() == Some(true) {
        eprintln!("timed out waiting for the command (it may still be running in the guest)");
    }
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_body_carries_args_env_timeout_and_binary_stdin() {
        let cmd = vec!["/bin/sh".to_string(), "-c".into(), "cat".into()];
        let b = exec_body(&cmd, &["A=1".to_string()], Some(5), Some(&[0, 255]));
        assert_eq!(b["path"], "/bin/sh");
        assert_eq!(b["args"], serde_json::json!(["-c", "cat"]));
        assert_eq!(b["env"], serde_json::json!(["A=1"]));
        assert_eq!(b["timeout_secs"], 5);
        assert_eq!(b["encoding"], "base64");
        assert_eq!(b["stdin_base64"], "AP8=");
        let b = exec_body(&cmd[..1], &[], None, None);
        assert!(b.get("timeout_secs").is_none() && b.get("stdin_base64").is_none());
    }

    #[test]
    fn exec_exit_status_maps_exit_signal_and_timeout() {
        use serde_json::json;
        assert_eq!(exec_exit_code(&json!({"exit_code": 0})), 0);
        assert_eq!(exec_exit_code(&json!({"exit_code": 7})), 7);
        assert_eq!(
            exec_exit_code(&json!({"signal": 9, "exit_code": null})),
            137
        );
        assert_eq!(exec_exit_code(&json!({"timed_out": true})), 124);
        assert_eq!(exec_exit_code(&json!({})), 1);
    }

    #[test]
    fn list_row_shows_memory_in_mb() {
        let vm: VmConfig = serde_json::from_str(
            r#"{"id":"v","name":"v","hypervisor":"qemu","memory":128,"cpus":1,"disk_size":1,"status":"stopped"}"#,
        )
        .unwrap();
        assert_eq!(VmRow::from(vm).memory_mb, 128);
    }
}
