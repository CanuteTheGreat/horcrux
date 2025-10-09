//! CIFS/SMB storage backend
//!
//! Provides Windows file share (SMB/CIFS) storage support.
//! Compatible with Windows Server, Samba, and Azure Files.

use super::StoragePool;
use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::process::Command;
use tokio::process::Command as AsyncCommand;
use tracing::{error, info};

/// CIFS/SMB storage manager
pub struct CifsManager {}

impl CifsManager {
    pub fn new() -> Self {
        Self {}
    }

    /// Validate a CIFS share
    pub async fn validate_pool(&self, pool: &StoragePool) -> Result<()> {
        let config = CifsConfig::parse(&pool.path)?;

        // Test mount
        self.test_mount(&config).await?;

        Ok(())
    }

    /// Mount CIFS share
    pub async fn mount_share(&self, config: &CifsConfig, mount_point: &str) -> Result<()> {
        info!("Mounting CIFS share {} to {}", config.share, mount_point);

        // Create mount point
        tokio::fs::create_dir_all(mount_point).await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to create mount point: {}", e)))?;

        let mut args = vec![
            "-t".to_string(),
            "cifs".to_string(),
            config.share.clone(),
            mount_point.to_string(),
            "-o".to_string(),
        ];

        let mut options = vec![];

        // Add credentials
        if let Some(ref username) = config.username {
            options.push(format!("username={}", username));
        }

        if let Some(ref password) = config.password {
            options.push(format!("password={}", password));
        }

        if let Some(ref domain) = config.domain {
            options.push(format!("domain={}", domain));
        }

        // Add mount options
        options.push(format!("vers={}", config.version.as_str()));

        if config.read_only {
            options.push("ro".to_string());
        }

        args.push(options.join(","));

        let output = AsyncCommand::new("mount")
            .args(&args)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to mount CIFS share: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "CIFS mount failed: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Unmount CIFS share
    pub async fn unmount_share(&self, mount_point: &str) -> Result<()> {
        info!("Unmounting CIFS share from {}", mount_point);

        let output = AsyncCommand::new("umount")
            .arg(mount_point)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to unmount: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Unmount failed: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Test CIFS mount
    async fn test_mount(&self, config: &CifsConfig) -> Result<()> {
        let test_point = format!("/tmp/horcrux-cifs-test-{}", uuid::Uuid::new_v4());

        match self.mount_share(config, &test_point).await {
            Ok(_) => {
                // Unmount test mount
                let _ = self.unmount_share(&test_point).await;
                // Remove test directory
                let _ = tokio::fs::remove_dir(&test_point).await;
                Ok(())
            }
            Err(e) => {
                let _ = tokio::fs::remove_dir(&test_point).await;
                Err(e)
            }
        }
    }

    /// Create a file (volume) on CIFS share
    pub async fn create_volume(
        &self,
        mount_point: &str,
        volume_name: &str,
        size_gb: u64,
    ) -> Result<String> {
        let volume_path = format!("{}/{}.qcow2", mount_point, volume_name);

        info!("Creating CIFS volume: {} ({}GB)", volume_path, size_gb);

        // Create qcow2 image
        let output = AsyncCommand::new("qemu-img")
            .args(&[
                "create",
                "-f",
                "qcow2",
                &volume_path,
                &format!("{}G", size_gb),
            ])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to create volume: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Volume creation failed: {}",
                stderr
            )));
        }

        Ok(volume_path)
    }

    /// Delete a volume from CIFS share
    pub async fn delete_volume(&self, volume_path: &str) -> Result<()> {
        info!("Deleting CIFS volume: {}", volume_path);

        tokio::fs::remove_file(volume_path).await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to delete volume: {}", e)))?;

        Ok(())
    }

    /// Check if CIFS client is available
    pub fn check_cifs_available() -> bool {
        Command::new("mount.cifs").arg("--version").output().is_ok()
    }
}

/// CIFS configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CifsConfig {
    pub share: String,      // //server/share
    pub username: Option<String>,
    pub password: Option<String>,
    pub domain: Option<String>,
    pub version: SmbVersion,
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SmbVersion {
    #[serde(rename = "3.0")]
    V3_0,
    #[serde(rename = "3.1.1")]
    V3_1_1,
    #[serde(rename = "2.1")]
    V2_1,
}

impl SmbVersion {
    fn as_str(&self) -> &str {
        match self {
            SmbVersion::V3_0 => "3.0",
            SmbVersion::V3_1_1 => "3.1.1",
            SmbVersion::V2_1 => "2.1",
        }
    }
}

impl CifsConfig {
    /// Parse CIFS path
    pub fn parse(path: &str) -> Result<Self> {
        // Expected format: cifs://username:password@server/share
        // Or: cifs://server/share

        if !path.starts_with("cifs://") {
            return Err(horcrux_common::Error::InvalidConfig(
                "Invalid CIFS path format".to_string(),
            ));
        }

        let path = path.strip_prefix("cifs://").unwrap();

        let (credentials, share_path) = if path.contains('@') {
            let parts: Vec<&str> = path.splitn(2, '@').collect();
            (Some(parts[0]), parts[1])
        } else {
            (None, path)
        };

        let (username, password) = if let Some(creds) = credentials {
            if creds.contains(':') {
                let parts: Vec<&str> = creds.splitn(2, ':').collect();
                (Some(parts[0].to_string()), Some(parts[1].to_string()))
            } else {
                (Some(creds.to_string()), None)
            }
        } else {
            (None, None)
        };

        Ok(CifsConfig {
            share: format!("//{}", share_path),
            username,
            password,
            domain: None,
            version: SmbVersion::V3_1_1,
            read_only: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_cifs_path() {
        let config = CifsConfig::parse("cifs://user:pass@server/share").unwrap();
        assert_eq!(config.share, "//server/share");
        assert_eq!(config.username, Some("user".to_string()));
        assert_eq!(config.password, Some("pass".to_string()));
    }

    #[test]
    fn test_parse_cifs_path_no_creds() {
        let config = CifsConfig::parse("cifs://server/share").unwrap();
        assert_eq!(config.share, "//server/share");
        assert_eq!(config.username, None);
    }
}
