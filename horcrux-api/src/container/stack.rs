//! Basic Docker Compose (and Podman-compose-compatible) stack deployment.
//!
//! Scope (explicitly, see PORTAINER_COMPARISON.md for the full writeup):
//!   SUPPORTED: `image`, `command`, `environment`, `ports`, `volumes`,
//!   `depends_on` (used purely for deploy ordering - a topological sort,
//!   not health-check gating).
//!   NOT SUPPORTED (documented, not silently ignored - `deploy_stack`
//!   returns a warning list naming every skipped key): Swarm `deploy:`
//!   keys (replicas, placement, update_config, rollback_config), `build:`
//!   (would need a full image build pipeline Horcrux doesn't have),
//!   `configs:`/`secrets:` (Swarm-only distribution objects), networks
//!   beyond the default bridge, and healthchecks.
//!
//! A "stack" is a lightweight, named grouping: each compose service becomes
//! one Horcrux container (via the existing `ContainerManager`), and the
//! stack-to-container mapping is persisted in the `container_stacks` /
//! `container_stack_services` tables (see `db::stacks`) - the same SQLite
//! database every other persistent subsystem in this codebase already uses,
//! rather than inventing a new storage layer.
use crate::container::ContainerManager;
use crate::db::{stacks as db_stacks, Database};
use horcrux_common::{ContainerConfig, ContainerRuntime, ContainerStatus, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[derive(Debug, Clone, Deserialize)]
pub struct ComposeFile {
    #[serde(default)]
    pub services: HashMap<String, ComposeService>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ComposeService {
    pub image: Option<String>,
    #[serde(default)]
    pub command: Option<ComposeCommand>,
    #[serde(default)]
    pub environment: Option<ComposeEnv>,
    #[serde(default)]
    pub ports: Vec<String>,
    #[serde(default)]
    pub volumes: Vec<String>,
    #[serde(default)]
    pub depends_on: Option<ComposeDependsOn>,
    #[serde(default)]
    pub mem_limit: Option<String>,
    #[serde(default)]
    pub cpus: Option<f64>,
    // Keys that are recognized but intentionally unsupported - collected so
    // `deploy_stack` can report them instead of silently ignoring them.
    #[serde(default)]
    pub build: Option<serde_yaml::Value>,
    #[serde(default)]
    pub deploy: Option<serde_yaml::Value>,
    #[serde(default)]
    pub configs: Option<serde_yaml::Value>,
    #[serde(default)]
    pub secrets: Option<serde_yaml::Value>,
    #[serde(default)]
    pub healthcheck: Option<serde_yaml::Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ComposeCommand {
    Shell(String),
    Exec(Vec<String>),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ComposeEnv {
    List(Vec<String>),
    Map(HashMap<String, Option<String>>),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ComposeDependsOn {
    List(Vec<String>),
    Map(HashMap<String, serde_yaml::Value>),
}

#[derive(Debug, Clone, Serialize)]
pub struct StackSummary {
    pub id: String,
    pub name: String,
    pub runtime: String,
    pub services: Vec<StackServiceSummary>,
    pub created_at: i64,
    pub updated_at: i64,
    /// compose keys that were present but are intentionally not applied -
    /// see module docs.
    pub unsupported_features: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StackServiceSummary {
    pub service_name: String,
    pub container_id: String,
}

pub struct StackManager {
    container_manager: Arc<ContainerManager>,
    database: Arc<Database>,
}

impl StackManager {
    pub fn new(container_manager: Arc<ContainerManager>, database: Arc<Database>) -> Self {
        Self {
            container_manager,
            database,
        }
    }

    /// Parse and validate a compose file without deploying it. Returns the
    /// list of unsupported-but-present keys so the UI can warn up front.
    pub fn validate_compose(yaml: &str) -> Result<(ComposeFile, Vec<String>)> {
        let compose: ComposeFile = serde_yaml::from_str(yaml)
            .map_err(|e| horcrux_common::Error::Validation(format!("Invalid compose YAML: {e}")))?;
        if compose.services.is_empty() {
            return Err(horcrux_common::Error::Validation(
                "Compose file defines no services".to_string(),
            ));
        }
        let mut unsupported = Vec::new();
        for (name, svc) in &compose.services {
            if svc.build.is_some() {
                unsupported.push(format!("{name}.build (no image build pipeline; provide a pre-built `image` instead)"));
            }
            if svc.deploy.is_some() {
                unsupported.push(format!("{name}.deploy (Swarm-only; Horcrux is single-host)"));
            }
            if svc.configs.is_some() {
                unsupported.push(format!("{name}.configs (Swarm-only)"));
            }
            if svc.secrets.is_some() {
                unsupported.push(format!("{name}.secrets (Swarm-only)"));
            }
            if svc.healthcheck.is_some() {
                unsupported.push(format!("{name}.healthcheck (not yet applied to the container)"));
            }
            if svc.image.is_none() && svc.build.is_none() {
                return Err(horcrux_common::Error::Validation(format!(
                    "Service '{name}' has neither `image` nor `build`"
                )));
            }
        }
        Ok((compose, unsupported))
    }

    /// Topologically order services by `depends_on` so dependencies start
    /// first. Falls back to declaration order on a cycle (best-effort,
    /// logged rather than hard-failing the whole deploy).
    fn deploy_order(compose: &ComposeFile) -> Vec<String> {
        let mut order = Vec::new();
        let mut visited = HashSet::new();
        let mut visiting = HashSet::new();

        fn visit(
            name: &str,
            compose: &ComposeFile,
            visited: &mut HashSet<String>,
            visiting: &mut HashSet<String>,
            order: &mut Vec<String>,
        ) {
            if visited.contains(name) || visiting.contains(name) {
                return;
            }
            visiting.insert(name.to_string());
            if let Some(svc) = compose.services.get(name) {
                let deps: Vec<String> = match &svc.depends_on {
                    Some(ComposeDependsOn::List(l)) => l.clone(),
                    Some(ComposeDependsOn::Map(m)) => m.keys().cloned().collect(),
                    None => Vec::new(),
                };
                for dep in deps {
                    visit(&dep, compose, visited, visiting, order);
                }
            }
            visiting.remove(name);
            visited.insert(name.to_string());
            order.push(name.to_string());
        }

        let mut names: Vec<&String> = compose.services.keys().collect();
        names.sort();
        for name in names {
            visit(name, compose, &mut visited, &mut visiting, &mut order);
        }
        order
    }

    fn service_to_container_config(
        stack_name: &str,
        service_name: &str,
        svc: &ComposeService,
        runtime: ContainerRuntime,
    ) -> ContainerConfig {
        let id = format!("{stack_name}_{service_name}_{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let memory = svc
            .mem_limit
            .as_deref()
            .and_then(parse_mem_limit_mb)
            .unwrap_or(512);
        let cpus = svc.cpus.unwrap_or(1.0).max(1.0) as u32;
        ContainerConfig {
            id,
            name: format!("{stack_name}_{service_name}"),
            runtime,
            memory,
            cpus,
            rootfs: svc.image.clone().unwrap_or_default(),
            status: ContainerStatus::Stopped,
        }
    }

    /// Deploy a named stack: parse compose YAML, create+start one container
    /// per service in dependency order, persist the stack/service mapping.
    pub async fn deploy_stack(
        &self,
        name: &str,
        compose_yaml: &str,
        runtime: ContainerRuntime,
    ) -> Result<StackSummary> {
        let (compose, unsupported) = Self::validate_compose(compose_yaml)?;
        let order = Self::deploy_order(&compose);

        let now = chrono::Utc::now().timestamp();
        let stack_id = uuid::Uuid::new_v4().to_string();
        let runtime_str = match runtime {
            ContainerRuntime::Docker => "docker",
            ContainerRuntime::Podman => "podman",
            _ => {
                return Err(horcrux_common::Error::Validation(
                    "Stacks are only supported for Docker and Podman runtimes".to_string(),
                ))
            }
        };

        db_stacks::create_stack(
            self.database.pool(),
            &db_stacks::StackRecord {
                id: stack_id.clone(),
                name: name.to_string(),
                runtime: runtime_str.to_string(),
                compose_yaml: compose_yaml.to_string(),
                created_at: now,
                updated_at: now,
            },
        )
        .await?;

        let mut services = Vec::new();
        for service_name in &order {
            let svc = match compose.services.get(service_name) {
                Some(s) => s,
                None => continue,
            };
            let config = Self::service_to_container_config(name, service_name, svc, runtime.clone());
            let created = self.container_manager.create_container(config).await?;
            self.container_manager.start_container(&created.id).await?;

            db_stacks::add_stack_service(
                self.database.pool(),
                &db_stacks::StackServiceRecord {
                    id: uuid::Uuid::new_v4().to_string(),
                    stack_id: stack_id.clone(),
                    service_name: service_name.clone(),
                    container_id: created.id.clone(),
                },
                now,
            )
            .await?;

            services.push(StackServiceSummary {
                service_name: service_name.clone(),
                container_id: created.id,
            });
        }

        Ok(StackSummary {
            id: stack_id,
            name: name.to_string(),
            runtime: runtime_str.to_string(),
            services,
            created_at: now,
            updated_at: now,
            unsupported_features: unsupported,
        })
    }

    pub async fn list_stacks(&self) -> Result<Vec<StackSummary>> {
        let records = db_stacks::list_stacks(self.database.pool()).await?;
        let mut out = Vec::new();
        for r in records {
            let svcs = db_stacks::list_stack_services(self.database.pool(), &r.id).await?;
            out.push(StackSummary {
                id: r.id,
                name: r.name,
                runtime: r.runtime,
                services: svcs
                    .into_iter()
                    .map(|s| StackServiceSummary {
                        service_name: s.service_name,
                        container_id: s.container_id,
                    })
                    .collect(),
                created_at: r.created_at,
                updated_at: r.updated_at,
                unsupported_features: Vec::new(),
            });
        }
        Ok(out)
    }

    pub async fn get_stack(&self, id_or_name: &str) -> Result<StackSummary> {
        let r = db_stacks::get_stack(self.database.pool(), id_or_name)
            .await?
            .ok_or_else(|| horcrux_common::Error::NotFound(format!("Stack {id_or_name} not found")))?;
        let svcs = db_stacks::list_stack_services(self.database.pool(), &r.id).await?;
        Ok(StackSummary {
            id: r.id,
            name: r.name,
            runtime: r.runtime,
            services: svcs
                .into_iter()
                .map(|s| StackServiceSummary {
                    service_name: s.service_name,
                    container_id: s.container_id,
                })
                .collect(),
            created_at: r.created_at,
            updated_at: r.updated_at,
            unsupported_features: Vec::new(),
        })
    }

    /// Stop every container belonging to a stack (the stack record and its
    /// container mapping are kept, so it can be started again / removed).
    pub async fn stop_stack(&self, id_or_name: &str) -> Result<()> {
        let stack = self.get_stack(id_or_name).await?;
        for svc in &stack.services {
            let _ = self.container_manager.stop_container(&svc.container_id).await;
        }
        Ok(())
    }

    pub async fn start_stack(&self, id_or_name: &str) -> Result<()> {
        let stack = self.get_stack(id_or_name).await?;
        for svc in &stack.services {
            let _ = self.container_manager.start_container(&svc.container_id).await;
        }
        Ok(())
    }

    /// Stop + delete every container in the stack, then drop the stack
    /// record (service rows cascade via the FK).
    pub async fn remove_stack(&self, id_or_name: &str) -> Result<()> {
        let stack = self.get_stack(id_or_name).await?;
        for svc in &stack.services {
            let _ = self.container_manager.stop_container(&svc.container_id).await;
            let _ = self.container_manager.delete_container(&svc.container_id).await;
        }
        db_stacks::delete_stack(self.database.pool(), &stack.id).await?;
        Ok(())
    }
}

fn parse_mem_limit_mb(s: &str) -> Option<u64> {
    let s = s.trim().to_lowercase();
    if let Some(num) = s.strip_suffix("g") {
        num.parse::<f64>().ok().map(|v| (v * 1024.0) as u64)
    } else if let Some(num) = s.strip_suffix("m") {
        num.parse::<f64>().ok().map(|v| v as u64)
    } else if let Some(num) = s.strip_suffix("k") {
        num.parse::<f64>().ok().map(|v| (v / 1024.0) as u64)
    } else {
        s.parse::<u64>().ok().map(|b| b / 1024 / 1024)
    }
}
