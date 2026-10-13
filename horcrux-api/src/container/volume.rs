//! Container volume management (Docker + Podman)
use bollard::volume::{CreateVolumeOptions, ListVolumesOptions, RemoveVolumeOptions};
use bollard::Docker;
use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::process::Command;
use tracing::warn;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeInfo {
    pub name: String,
    pub driver: String,
    pub mountpoint: String,
}

pub struct DockerVolumeManager {
    docker: Option<Arc<Docker>>,
}

impl Default for DockerVolumeManager {
    fn default() -> Self {
        Self::new()
    }
}

impl DockerVolumeManager {
    pub fn new() -> Self {
        let docker = match Docker::connect_with_local_defaults() {
            Ok(d) => Some(Arc::new(d)),
            Err(e) => {
                warn!("DockerVolumeManager: Docker API unavailable: {}", e);
                None
            }
        };
        Self { docker }
    }

    pub async fn list_volumes(&self) -> Result<Vec<VolumeInfo>> {
        if let Some(docker) = &self.docker {
            let resp = docker
                .list_volumes(Some(ListVolumesOptions::<String>::default()))
                .await
                .map_err(|e| horcrux_common::Error::System(format!("docker volume ls: {e}")))?;
            return Ok(resp
                .volumes
                .unwrap_or_default()
                .into_iter()
                .map(|v| VolumeInfo {
                    name: v.name,
                    driver: v.driver,
                    mountpoint: v.mountpoint,
                })
                .collect());
        }
        let output = Command::new("docker")
            .args(["volume", "ls", "--format", "{{.Name}}|{{.Driver}}"])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("docker volume ls: {e}")))?;
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|l| {
                let p: Vec<&str> = l.splitn(2, '|').collect();
                if p.len() == 2 {
                    Some(VolumeInfo {
                        name: p[0].to_string(),
                        driver: p[1].to_string(),
                        mountpoint: String::new(),
                    })
                } else {
                    None
                }
            })
            .collect())
    }

    pub async fn create_volume(&self, name: &str, driver: Option<&str>) -> Result<VolumeInfo> {
        if let Some(docker) = &self.docker {
            let v = docker
                .create_volume(CreateVolumeOptions {
                    name,
                    driver: driver.unwrap_or("local"),
                    ..Default::default()
                })
                .await
                .map_err(|e| horcrux_common::Error::System(format!("docker volume create: {e}")))?;
            return Ok(VolumeInfo {
                name: v.name,
                driver: v.driver,
                mountpoint: v.mountpoint,
            });
        }
        let mut cmd = Command::new("docker");
        cmd.args(["volume", "create"]);
        if let Some(d) = driver {
            cmd.args(["--driver", d]);
        }
        let output = cmd
            .arg(name)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("docker volume create: {e}")))?;
        if !output.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "docker volume create failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(VolumeInfo {
            name: name.to_string(),
            driver: driver.unwrap_or("local").to_string(),
            mountpoint: String::new(),
        })
    }

    pub async fn remove_volume(&self, name: &str, force: bool) -> Result<()> {
        if let Some(docker) = &self.docker {
            docker
                .remove_volume(name, Some(RemoveVolumeOptions { force }))
                .await
                .map_err(|e| horcrux_common::Error::System(format!("docker volume rm: {e}")))?;
            return Ok(());
        }
        let mut cmd = Command::new("docker");
        cmd.args(["volume", "rm"]);
        if force {
            cmd.arg("-f");
        }
        let output = cmd
            .arg(name)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("docker volume rm: {e}")))?;
        if !output.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "docker volume rm failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(())
    }

    pub async fn inspect_volume(&self, name: &str) -> Result<serde_json::Value> {
        if let Some(docker) = &self.docker {
            let v = docker
                .inspect_volume(name)
                .await
                .map_err(|e| horcrux_common::Error::System(format!("docker volume inspect: {e}")))?;
            return Ok(serde_json::to_value(v).unwrap_or(serde_json::Value::Null));
        }
        let output = Command::new("docker")
            .args(["volume", "inspect", name])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("docker volume inspect: {e}")))?;
        let value: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| horcrux_common::Error::System(format!("parse inspect json: {e}")))?;
        Ok(value)
    }
}

/// Podman volumes, CLI-based (same rationale as `PodmanImageManager`).
pub struct PodmanVolumeManager;

impl Default for PodmanVolumeManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PodmanVolumeManager {
    pub fn new() -> Self {
        Self
    }

    pub async fn list_volumes(&self) -> Result<Vec<VolumeInfo>> {
        let output = Command::new("podman")
            .args(["volume", "ls", "--format", "json"])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("podman volume ls: {e}")))?;
        #[derive(Deserialize)]
        struct PodmanVolume {
            #[serde(rename = "Name")]
            name: String,
            #[serde(rename = "Driver", default)]
            driver: String,
            #[serde(rename = "Mountpoint", default)]
            mountpoint: String,
        }
        let vols: Vec<PodmanVolume> = serde_json::from_slice(&output.stdout).unwrap_or_default();
        Ok(vols
            .into_iter()
            .map(|v| VolumeInfo {
                name: v.name,
                driver: v.driver,
                mountpoint: v.mountpoint,
            })
            .collect())
    }

    pub async fn create_volume(&self, name: &str, driver: Option<&str>) -> Result<VolumeInfo> {
        let mut cmd = Command::new("podman");
        cmd.args(["volume", "create"]);
        if let Some(d) = driver {
            cmd.args(["--driver", d]);
        }
        let output = cmd
            .arg(name)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("podman volume create: {e}")))?;
        if !output.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "podman volume create failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(VolumeInfo {
            name: name.to_string(),
            driver: driver.unwrap_or("local").to_string(),
            mountpoint: String::new(),
        })
    }

    pub async fn remove_volume(&self, name: &str, force: bool) -> Result<()> {
        let mut cmd = Command::new("podman");
        cmd.args(["volume", "rm"]);
        if force {
            cmd.arg("-f");
        }
        let output = cmd
            .arg(name)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("podman volume rm: {e}")))?;
        if !output.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "podman volume rm failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(())
    }

    pub async fn inspect_volume(&self, name: &str) -> Result<serde_json::Value> {
        let output = Command::new("podman")
            .args(["volume", "inspect", name])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("podman volume inspect: {e}")))?;
        let value: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| horcrux_common::Error::System(format!("parse inspect json: {e}")))?;
        Ok(value)
    }
}

pub struct VolumeManager {
    pub docker: DockerVolumeManager,
    pub podman: PodmanVolumeManager,
}

impl Default for VolumeManager {
    fn default() -> Self {
        Self::new()
    }
}

impl VolumeManager {
    pub fn new() -> Self {
        Self {
            docker: DockerVolumeManager::new(),
            podman: PodmanVolumeManager::new(),
        }
    }
}

