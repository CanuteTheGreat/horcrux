//! Docker/Podman **container-runtime** network management.
//!
//! This is scoped strictly to `docker network ...` / `podman network ...`
//! style bridge networks that containers attach to - it is unrelated to
//! Horcrux's own SDN/CNI networking (`src/sdn/cni`, wired into
//! `/api/cni/*` in `main.rs`), which manages VM/container network
//! namespaces at the host level. Do not merge these two concerns.
use bollard::network::{
    ConnectNetworkOptions, CreateNetworkOptions, DisconnectNetworkOptions, ListNetworksOptions,
};
use bollard::Docker;
use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::process::Command;
use tracing::warn;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkInfo {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub scope: String,
}

pub struct DockerNetworkManager {
    docker: Option<Arc<Docker>>,
}

impl Default for DockerNetworkManager {
    fn default() -> Self {
        Self::new()
    }
}

impl DockerNetworkManager {
    pub fn new() -> Self {
        let docker = match Docker::connect_with_local_defaults() {
            Ok(d) => Some(Arc::new(d)),
            Err(e) => {
                warn!("DockerNetworkManager: Docker API unavailable: {}", e);
                None
            }
        };
        Self { docker }
    }

    pub async fn list_networks(&self) -> Result<Vec<NetworkInfo>> {
        if let Some(docker) = &self.docker {
            let nets = docker
                .list_networks(Some(ListNetworksOptions::<String>::default()))
                .await
                .map_err(|e| horcrux_common::Error::System(format!("docker network ls: {e}")))?;
            return Ok(nets
                .into_iter()
                .map(|n| NetworkInfo {
                    id: n.id.unwrap_or_default(),
                    name: n.name.unwrap_or_default(),
                    driver: n.driver.unwrap_or_default(),
                    scope: n.scope.unwrap_or_default(),
                })
                .collect());
        }
        let output = Command::new("docker")
            .args(["network", "ls", "--format", "{{.ID}}|{{.Name}}|{{.Driver}}|{{.Scope}}"])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("docker network ls: {e}")))?;
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|l| {
                let p: Vec<&str> = l.splitn(4, '|').collect();
                if p.len() == 4 {
                    Some(NetworkInfo {
                        id: p[0].to_string(),
                        name: p[1].to_string(),
                        driver: p[2].to_string(),
                        scope: p[3].to_string(),
                    })
                } else {
                    None
                }
            })
            .collect())
    }

    pub async fn create_network(&self, name: &str, driver: Option<&str>) -> Result<String> {
        if let Some(docker) = &self.docker {
            let resp = docker
                .create_network(CreateNetworkOptions {
                    name,
                    driver: driver.unwrap_or("bridge"),
                    ..Default::default()
                })
                .await
                .map_err(|e| horcrux_common::Error::System(format!("docker network create: {e}")))?;
            return Ok(resp.id.unwrap_or_default());
        }
        let mut cmd = Command::new("docker");
        cmd.args(["network", "create"]);
        if let Some(d) = driver {
            cmd.args(["--driver", d]);
        }
        let output = cmd
            .arg(name)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("docker network create: {e}")))?;
        if !output.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "docker network create failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    pub async fn remove_network(&self, id: &str) -> Result<()> {
        if let Some(docker) = &self.docker {
            docker
                .remove_network(id)
                .await
                .map_err(|e| horcrux_common::Error::System(format!("docker network rm: {e}")))?;
            return Ok(());
        }
        let output = Command::new("docker")
            .args(["network", "rm", id])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("docker network rm: {e}")))?;
        if !output.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "docker network rm failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(())
    }

    pub async fn inspect_network(&self, id: &str) -> Result<serde_json::Value> {
        if let Some(docker) = &self.docker {
            let n = docker
                .inspect_network::<String>(id, None)
                .await
                .map_err(|e| horcrux_common::Error::System(format!("docker network inspect: {e}")))?;
            return Ok(serde_json::to_value(n).unwrap_or(serde_json::Value::Null));
        }
        let output = Command::new("docker")
            .args(["network", "inspect", id])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("docker network inspect: {e}")))?;
        let value: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| horcrux_common::Error::System(format!("parse inspect json: {e}")))?;
        Ok(value)
    }

    pub async fn connect_container(&self, network_id: &str, container_id: &str) -> Result<()> {
        if let Some(docker) = &self.docker {
            docker
                .connect_network(
                    network_id,
                    ConnectNetworkOptions {
                        container: container_id,
                        ..Default::default()
                    },
                )
                .await
                .map_err(|e| horcrux_common::Error::System(format!("docker network connect: {e}")))?;
            return Ok(());
        }
        let output = Command::new("docker")
            .args(["network", "connect", network_id, container_id])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("docker network connect: {e}")))?;
        if !output.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "docker network connect failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(())
    }

    pub async fn disconnect_container(
        &self,
        network_id: &str,
        container_id: &str,
        force: bool,
    ) -> Result<()> {
        if let Some(docker) = &self.docker {
            docker
                .disconnect_network(
                    network_id,
                    DisconnectNetworkOptions {
                        container: container_id,
                        force,
                    },
                )
                .await
                .map_err(|e| {
                    horcrux_common::Error::System(format!("docker network disconnect: {e}"))
                })?;
            return Ok(());
        }
        let mut cmd = Command::new("docker");
        cmd.args(["network", "disconnect"]);
        if force {
            cmd.arg("-f");
        }
        let output = cmd
            .args([network_id, container_id])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("docker network disconnect: {e}")))?;
        if !output.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "docker network disconnect failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(())
    }
}

/// Podman container networks, CLI-based. Podman's `network connect` /
/// `disconnect` subcommands exist and behave the same as Docker's, so the
/// subset below has full CLI parity even without a bollard-compatible
/// client; nothing here is silently stubbed.
pub struct PodmanNetworkManager;

impl Default for PodmanNetworkManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PodmanNetworkManager {
    pub fn new() -> Self {
        Self
    }

    pub async fn list_networks(&self) -> Result<Vec<NetworkInfo>> {
        let output = Command::new("podman")
            .args(["network", "ls", "--format", "json"])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("podman network ls: {e}")))?;
        #[derive(Deserialize)]
        struct PodmanNetwork {
            #[serde(rename = "id", default)]
            id: String,
            #[serde(rename = "name", default)]
            name: String,
            #[serde(rename = "driver", default)]
            driver: String,
        }
        let nets: Vec<PodmanNetwork> = serde_json::from_slice(&output.stdout).unwrap_or_default();
        Ok(nets
            .into_iter()
            .map(|n| NetworkInfo {
                id: n.id,
                name: n.name,
                driver: n.driver,
                scope: "local".to_string(),
            })
            .collect())
    }

    pub async fn create_network(&self, name: &str, driver: Option<&str>) -> Result<String> {
        let mut cmd = Command::new("podman");
        cmd.args(["network", "create"]);
        if let Some(d) = driver {
            cmd.args(["--driver", d]);
        }
        let output = cmd
            .arg(name)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("podman network create: {e}")))?;
        if !output.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "podman network create failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    pub async fn remove_network(&self, id: &str) -> Result<()> {
        let output = Command::new("podman")
            .args(["network", "rm", id])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("podman network rm: {e}")))?;
        if !output.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "podman network rm failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(())
    }

    pub async fn inspect_network(&self, id: &str) -> Result<serde_json::Value> {
        let output = Command::new("podman")
            .args(["network", "inspect", id])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("podman network inspect: {e}")))?;
        let value: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| horcrux_common::Error::System(format!("parse inspect json: {e}")))?;
        Ok(value)
    }

    pub async fn connect_container(&self, network_id: &str, container_id: &str) -> Result<()> {
        let output = Command::new("podman")
            .args(["network", "connect", network_id, container_id])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("podman network connect: {e}")))?;
        if !output.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "podman network connect failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(())
    }

    pub async fn disconnect_container(
        &self,
        network_id: &str,
        container_id: &str,
        force: bool,
    ) -> Result<()> {
        let mut cmd = Command::new("podman");
        cmd.args(["network", "disconnect"]);
        if force {
            cmd.arg("-f");
        }
        let output = cmd
            .args([network_id, container_id])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("podman network disconnect: {e}")))?;
        if !output.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "podman network disconnect failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(())
    }
}

pub struct ContainerNetworkManager {
    pub docker: DockerNetworkManager,
    pub podman: PodmanNetworkManager,
}

impl Default for ContainerNetworkManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ContainerNetworkManager {
    pub fn new() -> Self {
        Self {
            docker: DockerNetworkManager::new(),
            podman: PodmanNetworkManager::new(),
        }
    }
}
