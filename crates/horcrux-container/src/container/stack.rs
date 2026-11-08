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
//!   `configs:`/`secrets:` (Swarm-only distribution objects), and
//!   healthchecks. Every service joins one per-stack network
//!   (`<stack>_default`, like compose) with its service name as a DNS
//!   alias; custom `networks:` beyond that are not applied.
//!
//! A "stack" is a lightweight, named grouping: each compose service becomes
//! one Horcrux container (via the existing `ContainerManager`), and the
//! stack-to-container mapping is persisted in the `container_stacks` /
//! `container_stack_services` tables (see `db::stacks`) - the same SQLite
//! database every other persistent subsystem in this codebase already uses,
//! rather than inventing a new storage layer.
use crate::container::ContainerManager;
use crate::db::{stacks as db_stacks, Database};
use horcrux_common::{
    ContainerConfig, ContainerOptions, ContainerRuntime, ContainerStatus, Result,
};
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

/// What `update_stack` did, per service.
#[derive(Debug, Clone, Default, Serialize)]
pub struct StackUpdateReport {
    pub stack: Option<StackSummary>,
    /// services recreated with the new configuration
    pub recreated: Vec<String>,
    /// services whose configuration did not change (left running)
    pub unchanged: Vec<String>,
    /// services new in the compose file
    pub added: Vec<String>,
    /// services dropped from the compose file (containers removed)
    pub removed: Vec<String>,
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
    /// The compose file the stack runs (only on `get_stack`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compose_yaml: Option<String>,
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
                unsupported.push(format!(
                    "{name}.build (no image build pipeline; provide a pre-built `image` instead)"
                ));
            }
            if svc.deploy.is_some() {
                unsupported.push(format!(
                    "{name}.deploy (Swarm-only; Horcrux is single-host)"
                ));
            }
            if svc.configs.is_some() {
                unsupported.push(format!("{name}.configs (Swarm-only)"));
            }
            if svc.secrets.is_some() {
                unsupported.push(format!("{name}.secrets (Swarm-only)"));
            }
            if svc.healthcheck.is_some() {
                unsupported.push(format!(
                    "{name}.healthcheck (not yet applied to the container)"
                ));
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

    /// Name of the per-stack network every service joins (compose's
    /// `<project>_default`), giving services name-based DNS to each other.
    fn stack_network_name(stack_name: &str) -> String {
        format!("{stack_name}_default")
    }

    fn service_to_container_config(
        stack_name: &str,
        service_name: &str,
        svc: &ComposeService,
        runtime: ContainerRuntime,
    ) -> ContainerConfig {
        let id = format!(
            "{stack_name}_{service_name}_{}",
            &uuid::Uuid::new_v4().to_string()[..8]
        );
        let memory = svc
            .mem_limit
            .as_deref()
            .and_then(parse_mem_limit_mb)
            .unwrap_or(512);
        let cpus = svc.cpus.unwrap_or(1.0).max(1.0) as u32;
        let env = match &svc.environment {
            Some(ComposeEnv::List(l)) => l.clone(),
            Some(ComposeEnv::Map(m)) => {
                let mut v: Vec<String> = m
                    .iter()
                    .map(|(k, val)| match val {
                        Some(val) => format!("{k}={val}"),
                        None => k.clone(),
                    })
                    .collect();
                v.sort();
                v
            }
            None => Vec::new(),
        };
        let command = match &svc.command {
            Some(ComposeCommand::Exec(argv)) => argv.clone(),
            Some(ComposeCommand::Shell(s)) => s.split_whitespace().map(str::to_string).collect(),
            None => Vec::new(),
        };
        ContainerConfig {
            id,
            name: format!("{stack_name}_{service_name}"),
            runtime,
            memory,
            cpus,
            rootfs: svc.image.clone().unwrap_or_default(),
            status: ContainerStatus::Stopped,
            options: ContainerOptions {
                env,
                ports: svc.ports.clone(),
                volumes: svc.volumes.clone(),
                command,
                network: Some(Self::stack_network_name(stack_name)),
                network_aliases: vec![service_name.to_string()],
            },
            created_at: None,
            lxc: None,
        }
    }

    /// Create the stack network (idempotent: an existing one is reused).
    /// Returns whether this call created it, so a failed deploy only
    /// removes a network it made itself.
    async fn ensure_stack_network(runtime: &str, network: &str, stack_name: &str) -> Result<bool> {
        let exists = tokio::process::Command::new(runtime)
            .args(["network", "inspect", network])
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);
        if exists {
            return Ok(false);
        }
        let out = tokio::process::Command::new(runtime)
            .args([
                "network",
                "create",
                "--label",
                &format!("horcrux.stack={stack_name}"),
                network,
            ])
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!(
                    "Failed to run {runtime} network create: {e}"
                ))
            })?;
        if !out.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "Failed to create stack network {network}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(true)
    }

    async fn remove_stack_network(runtime: &str, network: &str) {
        let _ = tokio::process::Command::new(runtime)
            .args(["network", "rm", network])
            .output()
            .await;
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
        if !crate::container::is_valid_oci_name(name) {
            return Err(horcrux_common::Error::Validation(format!(
                "Invalid stack name '{name}': use letters, digits, '_', '.' or '-'"
            )));
        }
        for service_name in compose.services.keys() {
            if !crate::container::is_valid_oci_name(service_name) {
                return Err(horcrux_common::Error::Validation(format!(
                    "Invalid service name '{service_name}'"
                )));
            }
        }
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

        // A second deploy under an existing name would share (and, if it
        // failed, tear down) the first stack's network: refuse it up front.
        if db_stacks::get_stack(self.database.pool(), name)
            .await?
            .is_some()
        {
            return Err(horcrux_common::Error::AlreadyExists(format!(
                "Stack '{name}' already exists"
            )));
        }

        let network = Self::stack_network_name(name);
        let created_network = Self::ensure_stack_network(runtime_str, &network, name).await?;

        if let Err(e) = db_stacks::create_stack(
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
        .await
        {
            if created_network {
                Self::remove_stack_network(runtime_str, &network).await;
            }
            return Err(e);
        }

        let mut services = Vec::new();
        if let Err(e) = self
            .deploy_services(
                &stack_id,
                name,
                &order,
                &compose,
                runtime,
                now,
                &mut services,
            )
            .await
        {
            // All-or-nothing. A half-deployed stack used to keep its row with
            // only the services recorded before the failure, so the container
            // whose start failed (and the network) were never part of the
            // stack and `DELETE /api/stacks/<name>` left them behind.
            tracing::warn!("Deploying stack {name} failed, rolling back: {e}");
            self.teardown_services(&services).await;
            if created_network {
                Self::remove_stack_network(runtime_str, &network).await;
            }
            if let Err(del) = db_stacks::delete_stack(self.database.pool(), &stack_id).await {
                tracing::error!("Rolling back stack {name}: deleting its row failed: {del}");
            }
            return Err(e);
        }

        Ok(StackSummary {
            id: stack_id,
            name: name.to_string(),
            runtime: runtime_str.to_string(),
            services,
            created_at: now,
            updated_at: now,
            unsupported_features: unsupported,
            compose_yaml: None,
        })
    }

    /// Create, record and start each service in dependency order. Every
    /// created container is pushed to `services` before anything else can
    /// fail, so on error the caller knows exactly which containers exist.
    #[allow(clippy::too_many_arguments)]
    async fn deploy_services(
        &self,
        stack_id: &str,
        name: &str,
        order: &[String],
        compose: &ComposeFile,
        runtime: ContainerRuntime,
        now: i64,
        services: &mut Vec<StackServiceSummary>,
    ) -> Result<()> {
        for service_name in order {
            let svc = match compose.services.get(service_name) {
                Some(s) => s,
                None => continue,
            };
            let config =
                Self::service_to_container_config(name, service_name, svc, runtime.clone());
            let created = self.container_manager.create_container(config).await?;
            services.push(StackServiceSummary {
                service_name: service_name.clone(),
                container_id: created.id.clone(),
            });
            db_stacks::add_stack_service(
                self.database.pool(),
                &db_stacks::StackServiceRecord {
                    id: uuid::Uuid::new_v4().to_string(),
                    stack_id: stack_id.to_string(),
                    service_name: service_name.clone(),
                    container_id: created.id.clone(),
                },
                now,
            )
            .await?;
            self.container_manager
                .start_container(&created.id)
                .await
                .map_err(|e| {
                    horcrux_common::Error::System(format!(
                        "Service '{service_name}' failed to start: {e}"
                    ))
                })?;
        }
        Ok(())
    }

    /// Merge `env` into every service's `environment` of a compose document
    /// and return the new YAML (list-form environments are converted to a
    /// map; the keys in `env` win).
    pub fn merge_env_into_yaml(
        yaml: &str,
        env: &std::collections::BTreeMap<String, String>,
    ) -> Result<String> {
        let mut doc: serde_yaml::Value = serde_yaml::from_str(yaml)
            .map_err(|e| horcrux_common::Error::Validation(format!("Invalid compose YAML: {e}")))?;
        let services = doc
            .get_mut("services")
            .and_then(|s| s.as_mapping_mut())
            .ok_or_else(|| {
                horcrux_common::Error::Validation("Compose file defines no services".into())
            })?;
        for (_, svc) in services.iter_mut() {
            let Some(svc) = svc.as_mapping_mut() else {
                continue;
            };
            let key = serde_yaml::Value::String("environment".into());
            let mut map = serde_yaml::Mapping::new();
            match svc.get(&key) {
                Some(serde_yaml::Value::Mapping(m)) => map = m.clone(),
                Some(serde_yaml::Value::Sequence(l)) => {
                    for item in l {
                        if let Some(kv) = item.as_str() {
                            let (k, v) = match kv.split_once('=') {
                                Some((k, v)) => (k, serde_yaml::Value::String(v.into())),
                                None => (kv, serde_yaml::Value::Null),
                            };
                            map.insert(serde_yaml::Value::String(k.into()), v);
                        }
                    }
                }
                _ => {}
            }
            for (k, v) in env {
                map.insert(
                    serde_yaml::Value::String(k.clone()),
                    serde_yaml::Value::String(v.clone()),
                );
            }
            svc.insert(key, serde_yaml::Value::Mapping(map));
        }
        serde_yaml::to_string(&doc)
            .map_err(|e| horcrux_common::Error::Validation(format!("Invalid compose YAML: {e}")))
    }

    /// Replace one service's container: stop and remove the old one (named
    /// and anonymous volumes are kept - `docker rm` without `-v`), create and
    /// start `cfg` (None = just remove it) and re-point the stack row.
    async fn replace_service(
        &self,
        stack_id: &str,
        service_name: &str,
        old: Option<&StackServiceSummary>,
        cfg: Option<ContainerConfig>,
    ) -> Result<()> {
        if let Some(old) = old {
            let _ = self
                .container_manager
                .stop_container(&old.container_id)
                .await;
            self.container_manager
                .delete_container(&old.container_id)
                .await?;
            db_stacks::delete_stack_service(self.database.pool(), stack_id, service_name).await?;
        }
        let Some(cfg) = cfg else {
            return Ok(());
        };
        let created = self.container_manager.create_container(cfg).await?;
        db_stacks::add_stack_service(
            self.database.pool(),
            &db_stacks::StackServiceRecord {
                id: uuid::Uuid::new_v4().to_string(),
                stack_id: stack_id.to_string(),
                service_name: service_name.to_string(),
                container_id: created.id.clone(),
            },
            chrono::Utc::now().timestamp(),
        )
        .await?;
        self.container_manager
            .start_container(&created.id)
            .await
            .map(|_| ())
            .map_err(|e| {
                horcrux_common::Error::System(format!(
                    "Service '{service_name}' failed to start: {e}"
                ))
            })
    }

    /// Update a deployed stack to a new compose file and/or environment
    /// (`compose up -d`): services whose configuration changed are pulled
    /// and recreated one at a time in dependency order, unchanged services
    /// keep running, new services are added, dropped ones removed. Volumes
    /// are kept. If a service fails, every service already replaced is put
    /// back to its previous configuration and the error is returned.
    pub async fn update_stack(
        &self,
        id_or_name: &str,
        compose_yaml: Option<&str>,
        env: Option<&std::collections::BTreeMap<String, String>>,
        force_recreate: bool,
    ) -> Result<StackUpdateReport> {
        let stack = db_stacks::get_stack(self.database.pool(), id_or_name)
            .await?
            .ok_or_else(|| {
                horcrux_common::Error::NotFound(format!("Stack {id_or_name} not found"))
            })?;
        let mut new_yaml = compose_yaml.unwrap_or(&stack.compose_yaml).to_string();
        if let Some(env) = env.filter(|e| !e.is_empty()) {
            new_yaml = Self::merge_env_into_yaml(&new_yaml, env)?;
        }
        let (new_compose, unsupported) = Self::validate_compose(&new_yaml)?;
        for service_name in new_compose.services.keys() {
            if !crate::container::is_valid_oci_name(service_name) {
                return Err(horcrux_common::Error::Validation(format!(
                    "Invalid service name '{service_name}'"
                )));
            }
        }
        let (old_compose, _) = Self::validate_compose(&stack.compose_yaml)?;
        let runtime = match stack.runtime.as_str() {
            "podman" => ContainerRuntime::Podman,
            _ => ContainerRuntime::Docker,
        };
        let current = self.get_stack(&stack.id).await?;
        let find_old = |name: &str| current.services.iter().find(|s| s.service_name == name);
        let same = |a: &ContainerConfig, b: &ContainerConfig| {
            a.memory == b.memory
                && a.cpus == b.cpus
                && a.rootfs == b.rootfs
                && a.options == b.options
        };
        let mut report = StackUpdateReport::default();
        let order = Self::deploy_order(&new_compose);
        // Plan first (nothing touched yet): which services change?
        let mut todo: Vec<(String, ContainerConfig, Option<ContainerConfig>)> = Vec::new();
        for name in &order {
            let Some(svc) = new_compose.services.get(name) else {
                continue;
            };
            let new_cfg =
                Self::service_to_container_config(&stack.name, name, svc, runtime.clone());
            let old_cfg = old_compose
                .services
                .get(name)
                .map(|o| Self::service_to_container_config(&stack.name, name, o, runtime.clone()));
            let running = find_old(name).is_some();
            match &old_cfg {
                Some(o) if running && !force_recreate && same(o, &new_cfg) => {
                    report.unchanged.push(name.clone())
                }
                _ => todo.push((name.clone(), new_cfg, old_cfg)),
            }
        }
        // Pull the new images before stopping anything: a bad tag must not
        // take a working stack down. (Pull is best effort for local images.)
        let runtime_str = stack.runtime.as_str();
        for (name, cfg, _) in &todo {
            let out = tokio::process::Command::new(runtime_str)
                .args(["pull", &cfg.rootfs])
                .output()
                .await;
            let pulled = out.as_ref().map(|o| o.status.success()).unwrap_or(false);
            if !pulled {
                let have = tokio::process::Command::new(runtime_str)
                    .args(["image", "inspect", &cfg.rootfs])
                    .output()
                    .await
                    .map(|o| o.status.success())
                    .unwrap_or(false);
                if !have {
                    return Err(horcrux_common::Error::Validation(format!(
                        "Service '{name}': image {} cannot be pulled and is not present locally",
                        cfg.rootfs
                    )));
                }
            }
        }
        Self::ensure_stack_network(
            runtime_str,
            &Self::stack_network_name(&stack.name),
            &stack.name,
        )
        .await?;
        // Rolling replace, dependency order.
        let mut done: Vec<(String, Option<ContainerConfig>)> = Vec::new();
        for (name, cfg, old_cfg) in todo {
            let old = find_old(&name).cloned();
            match self
                .replace_service(&stack.id, &name, old.as_ref(), Some(cfg))
                .await
            {
                Ok(()) => {
                    if old.is_some() {
                        report.recreated.push(name.clone());
                    } else {
                        report.added.push(name.clone());
                    }
                    done.push((name, old_cfg));
                }
                Err(e) => {
                    tracing::warn!(
                        "Updating stack {} failed at {name}: {e}; rolling back",
                        stack.name
                    );
                    // Put the failed service and the ones already replaced back.
                    let mut failed = vec![(name.clone(), old_cfg)];
                    failed.extend(done.into_iter().rev());
                    for (svc_name, prev) in failed {
                        let cur = db_stacks::list_stack_services(self.database.pool(), &stack.id)
                            .await
                            .unwrap_or_default()
                            .into_iter()
                            .find(|r| r.service_name == svc_name)
                            .map(|r| StackServiceSummary {
                                service_name: r.service_name,
                                container_id: r.container_id,
                            });
                        if let Err(re) = self
                            .replace_service(&stack.id, &svc_name, cur.as_ref(), prev)
                            .await
                        {
                            tracing::error!("Rollback of service {svc_name} failed: {re}");
                        }
                    }
                    return Err(horcrux_common::Error::System(format!(
                        "Updating service '{name}' failed (stack rolled back to its previous \
                         configuration): {e}"
                    )));
                }
            }
        }
        // Services dropped from the compose file.
        for old in &current.services {
            if !new_compose.services.contains_key(&old.service_name) {
                self.replace_service(&stack.id, &old.service_name, Some(old), None)
                    .await?;
                report.removed.push(old.service_name.clone());
            }
        }
        // Make sure the unchanged ones run too (an update of a stopped stack
        // starts it, like `compose up -d`).
        for name in &report.unchanged {
            if let Some(svc) = find_old(name) {
                let _ = self
                    .container_manager
                    .start_container(&svc.container_id)
                    .await;
            }
        }
        let now = chrono::Utc::now().timestamp();
        db_stacks::update_stack_compose(self.database.pool(), &stack.id, &new_yaml, now).await?;
        let mut summary = self.get_stack(&stack.id).await?;
        summary.unsupported_features = unsupported;
        report.stack = Some(summary);
        Ok(report)
    }

    /// Stop + delete containers concurrently (like `compose down`): a
    /// service whose PID 1 ignores SIGTERM takes the runtime's full stop
    /// grace period, so doing them one after another took ~10 s each.
    async fn teardown_services(&self, services: &[StackServiceSummary]) {
        futures::future::join_all(services.iter().map(|svc| async move {
            let _ = self
                .container_manager
                .stop_container(&svc.container_id)
                .await;
            if let Err(e) = self
                .container_manager
                .delete_container(&svc.container_id)
                .await
            {
                tracing::warn!(
                    "Removing stack service {} ({}) failed: {e}",
                    svc.service_name,
                    svc.container_id
                );
            }
        }))
        .await;
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
                compose_yaml: None,
            });
        }
        Ok(out)
    }

    pub async fn get_stack(&self, id_or_name: &str) -> Result<StackSummary> {
        let r = db_stacks::get_stack(self.database.pool(), id_or_name)
            .await?
            .ok_or_else(|| {
                horcrux_common::Error::NotFound(format!("Stack {id_or_name} not found"))
            })?;
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
            compose_yaml: Some(r.compose_yaml),
        })
    }

    /// Stop every container belonging to a stack (the stack record and its
    /// container mapping are kept, so it can be started again / removed).
    /// The services are stopped concurrently: each can take the runtime's
    /// full stop grace period (~10 s for a PID 1 that ignores SIGTERM), and
    /// stopping them in turn made a 3-service stack take 30 s, past typical
    /// client timeouts.
    pub async fn stop_stack(&self, id_or_name: &str) -> Result<()> {
        let stack = self.get_stack(id_or_name).await?;
        let results = futures::future::join_all(stack.services.iter().map(|svc| async move {
            (
                svc,
                self.container_manager
                    .stop_container(&svc.container_id)
                    .await,
            )
        }))
        .await;
        // A service that is already stopped is fine; any other failure must
        // reach the caller instead of leaving a running container behind a
        // success answer.
        let failed: Vec<String> = results
            .into_iter()
            .filter_map(|(svc, r)| match r {
                Err(e) if !e.to_string().contains("already stopped") => {
                    Some(format!("{}: {e}", svc.service_name))
                }
                _ => None,
            })
            .collect();
        if failed.is_empty() {
            Ok(())
        } else {
            Err(horcrux_common::Error::System(format!(
                "Failed to stop stack service(s): {}",
                failed.join("; ")
            )))
        }
    }

    pub async fn start_stack(&self, id_or_name: &str) -> Result<()> {
        let stack = self.get_stack(id_or_name).await?;
        for svc in &stack.services {
            let _ = self
                .container_manager
                .start_container(&svc.container_id)
                .await;
        }
        Ok(())
    }

    /// Stop + delete every container in the stack, then drop the stack
    /// record (service rows cascade via the FK).
    pub async fn remove_stack(&self, id_or_name: &str) -> Result<()> {
        let stack = self.get_stack(id_or_name).await?;
        self.teardown_services(&stack.services).await;
        Self::remove_stack_network(&stack.runtime, &Self::stack_network_name(&stack.name)).await;
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

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE_COMPOSE: &str = r#"
services:
  web:
    image: nginx:latest
    ports:
      - "8080:80"
    depends_on:
      - db
  db:
    image: postgres:16
    environment:
      - POSTGRES_PASSWORD=secret
    volumes:
      - dbdata:/var/lib/postgresql/data
"#;

    const UNSUPPORTED_COMPOSE: &str = r#"
services:
  app:
    build: .
    deploy:
      replicas: 3
"#;

    #[test]
    fn validates_simple_compose_with_no_unsupported_features() {
        let (compose, unsupported) = StackManager::validate_compose(SIMPLE_COMPOSE).unwrap();
        assert_eq!(compose.services.len(), 2);
        assert!(unsupported.is_empty());
    }

    #[test]
    fn flags_build_and_deploy_as_unsupported_without_failing() {
        let (compose, unsupported) = StackManager::validate_compose(UNSUPPORTED_COMPOSE).unwrap();
        assert_eq!(compose.services.len(), 1);
        assert!(unsupported.iter().any(|u| u.contains("build")));
        assert!(unsupported.iter().any(|u| u.contains("deploy")));
    }

    #[test]
    fn env_is_merged_into_every_service_and_survives_reparse() {
        let y = "services:\n  a:\n    image: x\n    environment: [\"K=old\", \"KEEP=1\"]\n  b:\n    image: y\n";
        let mut env = std::collections::BTreeMap::new();
        env.insert("K".to_string(), "new".to_string());
        let out = StackManager::merge_env_into_yaml(y, &env).unwrap();
        let (c, _) = StackManager::validate_compose(&out).unwrap();
        let cfg = |n: &str| {
            StackManager::service_to_container_config(
                "s",
                n,
                &c.services[n],
                ContainerRuntime::Docker,
            )
        };
        assert_eq!(cfg("a").options.env, vec!["K=new", "KEEP=1"]);
        assert_eq!(cfg("b").options.env, vec!["K=new"]);
        assert!(StackManager::merge_env_into_yaml("services: 3", &env).is_err());
    }

    #[test]
    fn rejects_empty_compose_file() {
        let result = StackManager::validate_compose("services: {}\n");
        assert!(result.is_err());
    }

    #[test]
    fn rejects_service_without_image_or_build() {
        let bad = "services:\n  web:\n    ports:\n      - \"80:80\"\n";
        let result = StackManager::validate_compose(bad);
        assert!(result.is_err());
    }

    #[test]
    fn deploy_order_respects_depends_on() {
        let (compose, _) = StackManager::validate_compose(SIMPLE_COMPOSE).unwrap();
        let order = StackManager::deploy_order(&compose);
        let web_pos = order.iter().position(|s| s == "web").unwrap();
        let db_pos = order.iter().position(|s| s == "db").unwrap();
        assert!(db_pos < web_pos, "db must start before web: {order:?}");
    }

    #[test]
    fn service_to_container_config_maps_image_and_resources() {
        let (compose, _) = StackManager::validate_compose(SIMPLE_COMPOSE).unwrap();
        let svc = compose.services.get("db").unwrap();
        let config = StackManager::service_to_container_config(
            "mystack",
            "db",
            svc,
            ContainerRuntime::Docker,
        );
        assert_eq!(config.rootfs, "postgres:16");
        assert_eq!(config.name, "mystack_db");
        assert_eq!(config.status, ContainerStatus::Stopped);
        // environment / volumes / network must reach the container config
        // (they used to be parsed and then silently dropped).
        assert_eq!(config.options.env, vec!["POSTGRES_PASSWORD=secret"]);
        assert_eq!(
            config.options.volumes,
            vec!["dbdata:/var/lib/postgresql/data"]
        );
        assert_eq!(config.options.network.as_deref(), Some("mystack_default"));
        assert_eq!(config.options.network_aliases, vec!["db"]);
        assert!(config.options.validate().is_ok());

        let web = StackManager::service_to_container_config(
            "mystack",
            "web",
            compose.services.get("web").unwrap(),
            ContainerRuntime::Docker,
        );
        assert_eq!(web.options.ports, vec!["8080:80"]);
    }

    #[test]
    fn parses_memory_limits() {
        assert_eq!(parse_mem_limit_mb("512m"), Some(512));
        assert_eq!(parse_mem_limit_mb("1g"), Some(1024));
        assert_eq!(parse_mem_limit_mb("2G"), Some(2048));
    }
}
