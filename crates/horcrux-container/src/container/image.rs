//! Container image management (Docker + Podman)
//!
//! Mirrors the create/start/stop style already used in `docker.rs` /
//! `podman.rs`: prefer the real Docker Engine API via `bollard` for Docker,
//! fall back to / use the `podman` CLI for Podman (podman.rs is already
//! CLI-only, so image management follows the same precedent rather than
//! inventing a bollard-over-podman-socket path that nothing else in this
//! codebase relies on).
use bollard::image::{CreateImageOptions, ListImagesOptions, RemoveImageOptions, TagImageOptions};
use bollard::Docker;
use futures::StreamExt;
use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::process::Command;
use tracing::{info, warn};

/// Runtime-agnostic image summary returned to the API/UI layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageInfo {
    pub id: String,
    pub repo_tags: Vec<String>,
    pub size: i64,
    pub created: i64,
}

/// A single line of progress emitted while pulling an image, suitable for
/// forwarding straight onto a WebSocket or SSE connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullProgress {
    pub status: String,
    pub id: Option<String>,
    pub progress: Option<String>,
}

pub struct DockerImageManager {
    docker: Option<Arc<Docker>>,
}

impl Default for DockerImageManager {
    fn default() -> Self {
        Self::new()
    }
}

impl DockerImageManager {
    pub fn new() -> Self {
        let docker = match Docker::connect_with_local_defaults() {
            Ok(d) => Some(Arc::new(d)),
            Err(e) => {
                warn!("DockerImageManager: Docker API unavailable: {}", e);
                None
            }
        };
        Self { docker }
    }

    pub fn with_client(docker: Option<Arc<Docker>>) -> Self {
        Self { docker }
    }

    pub async fn list_images(&self) -> Result<Vec<ImageInfo>> {
        if let Some(docker) = &self.docker {
            let images = docker
                .list_images(Some(ListImagesOptions::<String> {
                    all: true,
                    ..Default::default()
                }))
                .await
                .map_err(|e| horcrux_common::Error::System(format!("docker list images: {e}")))?;
            return Ok(images
                .into_iter()
                .map(|i| ImageInfo {
                    id: i.id,
                    repo_tags: i.repo_tags,
                    size: i.size,
                    created: i.created,
                })
                .collect());
        }
        self.list_images_cli().await
    }

    async fn list_images_cli(&self) -> Result<Vec<ImageInfo>> {
        let output = Command::new("docker")
            .args([
                "images",
                "--no-trunc",
                "--format",
                "{{.ID}}|{{.Repository}}:{{.Tag}}|{{.Size}}",
            ])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("docker images: {e}")))?;
        let text = String::from_utf8_lossy(&output.stdout);
        Ok(text
            .lines()
            .filter_map(|l| {
                let parts: Vec<&str> = l.splitn(3, '|').collect();
                if parts.len() == 3 {
                    Some(ImageInfo {
                        id: parts[0].to_string(),
                        repo_tags: vec![parts[1].to_string()],
                        size: 0,
                        created: 0,
                    })
                } else {
                    None
                }
            })
            .collect())
    }

    /// Pull an image, invoking `on_progress` for every status line reported
    /// by the Docker Engine API. Used by the WebSocket handler to stream
    /// progress to the browser in real time.
    pub async fn pull_image<F>(&self, name: &str, tag: &str, mut on_progress: F) -> Result<()>
    where
        F: FnMut(PullProgress) + Send,
    {
        if let Some(docker) = &self.docker {
            let options = Some(CreateImageOptions {
                from_image: name,
                tag,
                ..Default::default()
            });
            let mut stream = docker.create_image(options, None, None);
            while let Some(result) = stream.next().await {
                let info = result
                    .map_err(|e| horcrux_common::Error::System(format!("docker pull: {e}")))?;
                on_progress(PullProgress {
                    status: info.status.unwrap_or_default(),
                    id: info.id,
                    progress: info.progress,
                });
            }
            info!("Pulled Docker image {}:{}", name, tag);
            return Ok(());
        }

        // CLI fallback: stream docker CLI stdout lines as progress too.
        let output = Command::new("docker")
            .args(["pull", &format!("{name}:{tag}")])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("docker pull: {e}")))?;
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            on_progress(PullProgress {
                status: line.to_string(),
                id: None,
                progress: None,
            });
        }
        if !output.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "docker pull failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(())
    }

    pub async fn remove_image(&self, id: &str, force: bool) -> Result<()> {
        if let Some(docker) = &self.docker {
            docker
                .remove_image(
                    id,
                    Some(RemoveImageOptions {
                        force,
                        ..Default::default()
                    }),
                    None,
                )
                .await
                .map_err(|e| horcrux_common::Error::System(format!("docker rmi: {e}")))?;
            return Ok(());
        }
        let mut cmd = Command::new("docker");
        cmd.arg("rmi");
        if force {
            cmd.arg("-f");
        }
        let output = cmd
            .arg(id)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("docker rmi: {e}")))?;
        if !output.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "docker rmi failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(())
    }

    pub async fn inspect_image(&self, id: &str) -> Result<serde_json::Value> {
        if let Some(docker) = &self.docker {
            let inspect = docker
                .inspect_image(id)
                .await
                .map_err(|e| horcrux_common::Error::System(format!("docker inspect: {e}")))?;
            return Ok(serde_json::to_value(inspect).unwrap_or(serde_json::Value::Null));
        }
        let output = Command::new("docker")
            .args(["image", "inspect", id])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("docker inspect: {e}")))?;
        let value: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| horcrux_common::Error::System(format!("parse inspect json: {e}")))?;
        Ok(value)
    }

    pub async fn tag_image(&self, id: &str, repo: &str, tag: &str) -> Result<()> {
        if let Some(docker) = &self.docker {
            docker
                .tag_image(id, Some(TagImageOptions { repo, tag }))
                .await
                .map_err(|e| horcrux_common::Error::System(format!("docker tag: {e}")))?;
            return Ok(());
        }
        let output = Command::new("docker")
            .args(["tag", id, &format!("{repo}:{tag}")])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("docker tag: {e}")))?;
        if !output.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "docker tag failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(())
    }
}

/// Podman images, implemented via the `podman` CLI (same precedent as
/// `podman.rs`'s container lifecycle code: podman is daemonless and the
/// REST API socket is an opt-in `podman system service` that isn't assumed
/// to be running, so CLI is the dependable path here).
pub struct PodmanImageManager;

impl Default for PodmanImageManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PodmanImageManager {
    pub fn new() -> Self {
        Self
    }

    pub async fn list_images(&self) -> Result<Vec<ImageInfo>> {
        let output = Command::new("podman")
            .args(["images", "--format", "json"])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("podman images: {e}")))?;
        #[derive(Deserialize)]
        struct PodmanImage {
            #[serde(rename = "Id")]
            id: String,
            #[serde(rename = "Names", default)]
            names: Vec<String>,
            #[serde(rename = "Size", default)]
            size: i64,
            #[serde(rename = "Created", default)]
            created: i64,
        }
        let images: Vec<PodmanImage> = serde_json::from_slice(&output.stdout).unwrap_or_default();
        Ok(images
            .into_iter()
            .map(|i| ImageInfo {
                id: i.id,
                repo_tags: i.names,
                size: i.size,
                created: i.created,
            })
            .collect())
    }

    pub async fn pull_image<F>(&self, name: &str, tag: &str, mut on_progress: F) -> Result<()>
    where
        F: FnMut(PullProgress) + Send,
    {
        let output = Command::new("podman")
            .args(["pull", &format!("{name}:{tag}")])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("podman pull: {e}")))?;
        for line in String::from_utf8_lossy(&output.stdout)
            .lines()
            .chain(String::from_utf8_lossy(&output.stderr).lines())
        {
            on_progress(PullProgress {
                status: line.to_string(),
                id: None,
                progress: None,
            });
        }
        if !output.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "podman pull failed for {name}:{tag}"
            )));
        }
        Ok(())
    }

    pub async fn remove_image(&self, id: &str, force: bool) -> Result<()> {
        let mut cmd = Command::new("podman");
        cmd.arg("rmi");
        if force {
            cmd.arg("-f");
        }
        let output = cmd
            .arg(id)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("podman rmi: {e}")))?;
        if !output.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "podman rmi failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(())
    }

    pub async fn inspect_image(&self, id: &str) -> Result<serde_json::Value> {
        let output = Command::new("podman")
            .args(["image", "inspect", id])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("podman inspect: {e}")))?;
        let value: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| horcrux_common::Error::System(format!("parse inspect json: {e}")))?;
        Ok(value)
    }

    pub async fn tag_image(&self, id: &str, repo: &str, tag: &str) -> Result<()> {
        let output = Command::new("podman")
            .args(["tag", id, &format!("{repo}:{tag}")])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("podman tag: {e}")))?;
        if !output.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "podman tag failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(())
    }
}

pub struct ImageManager {
    pub docker: DockerImageManager,
    pub podman: PodmanImageManager,
}

impl Default for ImageManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageManager {
    pub fn new() -> Self {
        Self {
            docker: DockerImageManager::new(),
            podman: PodmanImageManager::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_info_round_trips_through_json() {
        let info = ImageInfo {
            id: "sha256:abc".to_string(),
            repo_tags: vec!["nginx:latest".to_string()],
            size: 12345,
            created: 1700000000,
        };
        let json = serde_json::to_string(&info).unwrap();
        let back: ImageInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, info.id);
        assert_eq!(back.repo_tags, info.repo_tags);
    }

    #[test]
    fn image_manager_constructs_both_backends() {
        // Exercises the constructor path (may or may not find a live Docker
        // socket in CI, but must never panic either way).
        let _ = ImageManager::new();
    }
}
