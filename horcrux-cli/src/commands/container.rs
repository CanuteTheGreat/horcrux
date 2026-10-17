//! Container management commands (`pct`/`pveam` equivalents for LXC).
use crate::api::ApiClient;
use crate::output::{self, truncate, OutputFormat};
use crate::{ContainerCommands, LxcCreateArgs, TemplateCommands};
use anyhow::{anyhow, Result};
use horcrux_common::{
    ContainerConfig, LxcCatalogEntry, LxcFeatures, LxcMount, LxcNet, LxcOptions, LxcSnapshot,
    LxcTemplate,
};
use serde::{Deserialize, Serialize};
use tabled::Tabled;

#[derive(Debug, Serialize, Deserialize)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub runtime: String,
    /// The API's ContainerConfig carries the image/template as `rootfs`.
    #[serde(alias = "rootfs")]
    pub image: String,
    pub status: String,
    pub created_at: Option<String>,
}

#[derive(Tabled, Serialize)]
struct ContainerRow {
    id: String,
    name: String,
    runtime: String,
    image: String,
    status: String,
}

impl From<Container> for ContainerRow {
    fn from(c: Container) -> Self {
        Self {
            id: c.id,
            name: truncate(&c.name, 30),
            runtime: c.runtime,
            image: truncate(&c.image, 40),
            status: c.status,
        }
    }
}

#[derive(Tabled, Serialize)]
struct TemplateRow {
    id: String,
    build: String,
    size: String,
    sha256: String,
}

#[derive(Tabled, Serialize)]
struct CatalogRow {
    distro: String,
    release: String,
    arch: String,
    variant: String,
    build: String,
}

#[derive(Tabled, Serialize)]
struct SnapshotRow {
    name: String,
    comment: String,
    created_at: String,
}

/// The LXC part of a create request from the CLI flags.
pub fn lxc_options(a: &LxcCreateArgs) -> Result<LxcOptions> {
    let o = LxcOptions {
        unprivileged: !a.privileged,
        disk_gb: a.disk,
        swap_mb: a.swap,
        cpu_limit: a.cpu_limit,
        hostname: a.hostname.clone(),
        nets: a
            .nets
            .iter()
            .map(|s| LxcNet::parse_spec(s))
            .collect::<std::result::Result<_, _>>()
            .map_err(|e| anyhow!(e))?,
        mounts: a
            .mounts
            .iter()
            .map(|s| LxcMount::parse_spec(s))
            .collect::<std::result::Result<_, _>>()
            .map_err(|e| anyhow!(e))?,
        features: match &a.features {
            Some(f) => LxcFeatures::parse_spec(f).map_err(|e| anyhow!(e))?,
            None => LxcFeatures::default(),
        },
        onboot: a.onboot,
        nameserver: a.nameserver.clone(),
        ssh_public_keys: a.ssh_keys.clone(),
        ..Default::default()
    };
    o.validate().map_err(|e| anyhow!(e))?;
    Ok(o)
}

/// The POST /api/containers body.
pub fn create_body(
    name: &str,
    runtime: &str,
    image: &str,
    memory: u64,
    cpus: u32,
    lxc: &LxcCreateArgs,
) -> Result<serde_json::Value> {
    let mut body = serde_json::json!({
        "id": name, "name": name, "runtime": runtime, "memory": memory, "cpus": cpus,
        "rootfs": image, "status": "stopped",
    });
    if runtime == "lxc" {
        body["lxc"] = serde_json::to_value(lxc_options(lxc)?)?;
    }
    Ok(body)
}

fn enc(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

pub async fn handle_container_command(
    command: ContainerCommands,
    api: &ApiClient,
    output_format: &str,
) -> Result<()> {
    let format = OutputFormat::from_str(output_format);
    match command {
        ContainerCommands::List => {
            let containers: Vec<Container> = api.get("/api/containers").await?;
            let rows: Vec<ContainerRow> = containers.into_iter().map(ContainerRow::from).collect();
            output::print_output(rows, format)?;
        }

        ContainerCommands::Show { id } => {
            let container: ContainerConfig =
                api.get(&format!("/api/containers/{}", enc(&id))).await?;
            output::print_single(&container, format)?;
        }

        ContainerCommands::Create {
            name,
            runtime,
            image,
            memory,
            cpus,
            lxc,
        } => {
            let body = create_body(&name, &runtime, &image, memory, cpus, &lxc)?;
            let container: Container = api.post("/api/containers", &body).await?;
            output::print_created("Container", &container.name, &container.id);
        }

        ContainerCommands::Start { id } => {
            let container: Container = api
                .post(
                    &format!("/api/containers/{}/start", enc(&id)),
                    &serde_json::json!({}),
                )
                .await?;
            output::print_started("Container", &container.name);
        }

        ContainerCommands::Stop { id } => {
            let container: Container = api
                .post(
                    &format!("/api/containers/{}/stop", enc(&id)),
                    &serde_json::json!({}),
                )
                .await?;
            output::print_stopped("Container", &container.name);
        }

        ContainerCommands::Shutdown { id, timeout, force } => {
            let container: Container = api
                .post(
                    &format!("/api/containers/{}/shutdown", enc(&id)),
                    &serde_json::json!({"timeout": timeout, "force": force}),
                )
                .await?;
            output::print_stopped("Container", &container.name);
        }

        ContainerCommands::Reboot { id } => {
            let container: Container = api
                .post(
                    &format!("/api/containers/{}/reboot", enc(&id)),
                    &serde_json::json!({}),
                )
                .await?;
            output::print_success(&format!("Container {} rebooted", container.name));
        }

        ContainerCommands::Delete { id } => {
            use dialoguer::Confirm;

            let confirm = Confirm::new()
                .with_prompt(format!("Are you sure you want to delete container {}?", id))
                .interact()?;

            if confirm {
                api.delete(&format!("/api/containers/{}", enc(&id))).await?;
                output::print_deleted("Container", &id);
            } else {
                output::print_info("Deletion cancelled");
            }
        }

        ContainerCommands::Exec { id, command } => {
            // POST /api/containers/:id/exec answers {"output": "..."} and
            // a non-2xx status (with the error) when the command fails.
            #[derive(Deserialize)]
            struct ExecResponse {
                output: String,
            }
            let response: Result<ExecResponse> = api
                .post(
                    &format!("/api/containers/{}/exec", enc(&id)),
                    &serde_json::json!({ "command": command }),
                )
                .await;
            match response {
                Ok(r) => print!("{}", r.output),
                Err(e) => {
                    output::print_error(&e.to_string());
                    anyhow::bail!("command failed in container {id}");
                }
            }
        }

        ContainerCommands::Config { id } => {
            let v: serde_json::Value = api
                .get(&format!("/api/containers/{}/lxc/config", enc(&id)))
                .await?;
            if matches!(format, OutputFormat::Table) {
                if let Some(p) = v["pending"].as_array().filter(|p| !p.is_empty()) {
                    output::print_warning(&format!(
                        "pending until reboot: {}",
                        p.iter()
                            .filter_map(|x| x.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                println!("{}", v["raw_config"].as_str().unwrap_or_default());
            } else {
                output::print_single(&v, format)?;
            }
        }

        ContainerCommands::Set {
            id,
            memory,
            cpus,
            swap,
            cpu_limit,
            hostname,
            nets,
            mounts,
            features,
            onboot,
        } => {
            let cur: serde_json::Value = api
                .get(&format!("/api/containers/{}/lxc/config", enc(&id)))
                .await?;
            let mut o: LxcOptions = serde_json::from_value(cur["lxc"].clone())?;
            if let Some(s) = swap {
                o.swap_mb = s;
            }
            if cpu_limit.is_some() {
                o.cpu_limit = cpu_limit;
            }
            if hostname.is_some() {
                o.hostname = hostname;
            }
            if !nets.is_empty() {
                o.nets = nets
                    .iter()
                    .map(|s| LxcNet::parse_spec(s))
                    .collect::<std::result::Result<_, _>>()
                    .map_err(|e| anyhow!(e))?;
            }
            if !mounts.is_empty() {
                o.mounts = mounts
                    .iter()
                    .map(|s| LxcMount::parse_spec(s))
                    .collect::<std::result::Result<_, _>>()
                    .map_err(|e| anyhow!(e))?;
            }
            if let Some(f) = features {
                o.features = LxcFeatures::parse_spec(&f).map_err(|e| anyhow!(e))?;
            }
            if let Some(b) = onboot {
                o.onboot = b;
            }
            let v: serde_json::Value = api
                .put(
                    &format!("/api/containers/{}/lxc/config", enc(&id)),
                    &serde_json::json!({"memory": memory, "cpus": cpus, "lxc": o}),
                )
                .await?;
            let pending: Vec<&str> = v["pending"]
                .as_array()
                .map(|p| p.iter().filter_map(|x| x.as_str()).collect())
                .unwrap_or_default();
            if pending.is_empty() {
                output::print_success(&format!("Container {id} updated"));
            } else {
                output::print_success(&format!(
                    "Container {id} updated; {} apply on the next reboot",
                    pending.join(", ")
                ));
            }
        }

        ContainerCommands::Resize { id, disk_gb } => {
            let _: serde_json::Value = api
                .post(
                    &format!("/api/containers/{}/resize", enc(&id)),
                    &serde_json::json!({ "disk_gb": disk_gb }),
                )
                .await?;
            output::print_success(&format!("Root disk of {id} grown to {disk_gb} GB"));
        }

        ContainerCommands::Snapshot { id, name, comment } => {
            let s: LxcSnapshot = api
                .post(
                    &format!("/api/containers/{}/snapshots", enc(&id)),
                    &serde_json::json!({"name": name, "comment": comment}),
                )
                .await?;
            output::print_created("Snapshot", &s.name, &id);
        }

        ContainerCommands::Snapshots { id } => {
            let list: Vec<LxcSnapshot> = api
                .get(&format!("/api/containers/{}/snapshots", enc(&id)))
                .await?;
            let rows: Vec<SnapshotRow> = list
                .into_iter()
                .map(|s| SnapshotRow {
                    name: s.name,
                    comment: s.comment.unwrap_or_default(),
                    created_at: s.created_at,
                })
                .collect();
            output::print_output(rows, format)?;
        }

        ContainerCommands::Rollback { id, name } => {
            let _: Container = api
                .post(
                    &format!(
                        "/api/containers/{}/snapshots/{}/rollback",
                        enc(&id),
                        enc(&name)
                    ),
                    &serde_json::json!({}),
                )
                .await?;
            output::print_success(&format!("Container {id} rolled back to {name}"));
        }

        ContainerCommands::DelSnapshot { id, name } => {
            api.delete(&format!(
                "/api/containers/{}/snapshots/{}",
                enc(&id),
                enc(&name)
            ))
            .await?;
            output::print_deleted("Snapshot", &name);
        }

        ContainerCommands::Clone { id, target } => {
            let c: Container = api
                .post(
                    &format!("/api/containers/{}/clone", enc(&id)),
                    &serde_json::json!({"target_id": target, "target_name": target, "snapshot": false}),
                )
                .await?;
            output::print_created("Container", &c.name, &c.id);
        }

        ContainerCommands::Template { command } => match command {
            TemplateCommands::List => {
                let list: Vec<LxcTemplate> = api.get("/api/lxc/templates").await?;
                let rows: Vec<TemplateRow> = list
                    .into_iter()
                    .map(|t| TemplateRow {
                        id: t.id,
                        build: t.build,
                        size: output::format_bytes(t.size_bytes),
                        sha256: t.sha256.chars().take(16).collect(),
                    })
                    .collect();
                output::print_output(rows, format)?;
            }
            TemplateCommands::Available { distro, arch } => {
                let mut path = "/api/lxc/templates/catalog?refresh=false".to_string();
                if let Some(a) = arch {
                    path.push_str(&format!("&arch={}", enc(&a)));
                }
                if let Some(d) = distro {
                    path.push_str(&format!("&distro={}", enc(&d)));
                }
                let list: Vec<LxcCatalogEntry> = api.get(&path).await?;
                let rows: Vec<CatalogRow> = list
                    .into_iter()
                    .map(|e| CatalogRow {
                        distro: e.distro,
                        release: e.release,
                        arch: e.arch,
                        variant: e.variant,
                        build: e.build,
                    })
                    .collect();
                output::print_output(rows, format)?;
            }
            TemplateCommands::Download { template } => {
                let parts: Vec<&str> = template.split('/').collect();
                let body = match parts.as_slice() {
                    [d, r] => serde_json::json!({"distro": d, "release": r}),
                    [d, r, v] => serde_json::json!({"distro": d, "release": r, "variant": v}),
                    [d, r, a, v] => {
                        serde_json::json!({"distro": d, "release": r, "arch": a, "variant": v})
                    }
                    _ => anyhow::bail!("template must be distro/release[/variant]"),
                };
                let t: LxcTemplate = api.post("/api/lxc/templates", &body).await?;
                output::print_success(&format!(
                    "Downloaded {} (build {}, {}, sha256 {})",
                    t.id,
                    t.build,
                    output::format_bytes(t.size_bytes),
                    t.sha256
                ));
            }
            TemplateCommands::Delete { id } => {
                let path = id.split('/').map(enc).collect::<Vec<_>>().join("/");
                api.delete(&format!("/api/lxc/templates/{path}")).await?;
                output::print_deleted("Template", &id);
            }
        },
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_body_matches_the_api() {
        let args = LxcCreateArgs {
            disk: Some(4),
            nets: vec!["bridge=vmbr0,tag=20,ip=10.0.0.5/24,gw=10.0.0.1".into()],
            mounts: vec!["mp=/srv,source=/tank,ro=1".into()],
            features: Some("nesting=1".into()),
            ..Default::default()
        };
        let body = create_body("web", "lxc", "alpine/3.22", 256, 2, &args).unwrap();
        let cfg: ContainerConfig = serde_json::from_value(body).unwrap();
        assert_eq!(cfg.id, "web");
        assert_eq!(cfg.rootfs, "alpine/3.22");
        let o = cfg.lxc.unwrap();
        assert!(o.unprivileged && o.features.nesting);
        assert_eq!(o.disk_gb, Some(4));
        assert_eq!(o.nets[0].vlan, Some(20));
        assert!(o.mounts[0].read_only);
        // Docker bodies carry no lxc object.
        let d = create_body("w", "docker", "nginx", 64, 1, &LxcCreateArgs::default()).unwrap();
        assert!(d.get("lxc").is_none());
        assert!(serde_json::from_value::<ContainerConfig>(d).is_ok());
        // Bad specs fail before any request.
        let bad = LxcCreateArgs {
            nets: vec!["tag=20".into()],
            ..Default::default()
        };
        assert!(create_body("w", "lxc", "alpine/3.22", 64, 1, &bad).is_err());
    }

    #[test]
    fn container_reads_the_api_shape() {
        let c: Container = serde_json::from_str(
            r#"{"id":"a","name":"a","runtime":"lxc","memory":1,"cpus":1,"rootfs":"alpine/3.22","status":"stopped"}"#,
        )
        .unwrap();
        assert_eq!(c.image, "alpine/3.22");
    }
}
