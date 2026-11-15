//! iSCSI storage backend
//!
//! Provides iSCSI target and initiator management for SAN storage.
//! Supports enterprise storage arrays and iSCSI-based shared storage.

#![allow(dead_code)]

use super::StoragePool;
use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command as StdCommand;
use tokio::process::Command;
use tracing::{error, info};

/// iSCSI storage manager
pub struct IscsiManager {}

impl Default for IscsiManager {
    fn default() -> Self {
        Self::new()
    }
}

impl IscsiManager {
    pub fn new() -> Self {
        Self {}
    }

    /// Validate an iSCSI target
    pub async fn validate_pool(&self, pool: &StoragePool) -> Result<()> {
        // Parse iSCSI target from pool path
        let target = IscsiTarget::parse(&pool.path)?;

        // The portal must answer and offer this very target: discovery
        // succeeding for some other IQN used to pass, and the pool then
        // had no disks at all.
        let offered = self.discover_target(&target.portal).await?;
        if !offered.iter().any(|t| t == &target.iqn) {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Portal {} does not offer target {} (it offers: {})",
                target.portal,
                target.iqn,
                if offered.is_empty() {
                    "nothing".to_string()
                } else {
                    offered.join(", ")
                }
            )));
        }

        Ok(())
    }

    /// Discover iSCSI targets on a portal
    pub async fn discover_target(&self, portal: &str) -> Result<Vec<String>> {
        info!("Discovering iSCSI targets on portal: {}", portal);

        let output = Command::new("iscsiadm")
            .args(["-m", "discovery", "-t", "st", "-p", portal])
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to discover iSCSI targets: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "iSCSI discovery failed: {}",
                stderr
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let targets: Vec<String> = stdout
            .lines()
            .filter_map(|line| {
                // Parse format: "portal:port,tag iqn.target.name"
                line.split_whitespace().nth(1).map(|s| s.to_string())
            })
            .collect();

        Ok(targets)
    }

    /// Login to an iSCSI target
    pub async fn login_target(&self, target: &IscsiTarget) -> Result<()> {
        info!("Logging into iSCSI target: {}", target.iqn);

        // Discovery first if needed
        self.discover_target(&target.portal).await?;

        // Login to target
        let output = Command::new("iscsiadm")
            .args([
                "-m",
                "node",
                "-T",
                &target.iqn,
                "-p",
                &target.portal,
                "--login",
            ])
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to login to iSCSI target: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Ignore "already logged in" errors
            if !stderr.contains("already present") {
                return Err(horcrux_common::Error::System(format!(
                    "iSCSI login failed: {}",
                    stderr
                )));
            }
        }

        Ok(())
    }

    /// Logout from an iSCSI target
    pub async fn logout_target(&self, target: &IscsiTarget) -> Result<()> {
        info!("Logging out from iSCSI target: {}", target.iqn);

        let output = Command::new("iscsiadm")
            .args([
                "-m",
                "node",
                "-T",
                &target.iqn,
                "-p",
                &target.portal,
                "--logout",
            ])
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to logout from iSCSI target: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("iSCSI logout failed: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "iSCSI logout failed: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// List active iSCSI sessions
    pub async fn list_sessions(&self) -> Result<Vec<IscsiSession>> {
        let output = Command::new("iscsiadm")
            .args(["-m", "session"])
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to list iSCSI sessions: {}", e))
            })?;

        if !output.status.success() {
            // No active sessions is not an error
            return Ok(vec![]);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let sessions: Vec<IscsiSession> = stdout.lines().filter_map(IscsiSession::parse).collect();

        Ok(sessions)
    }

    /// Get block devices for an iSCSI target
    pub async fn get_block_devices(&self, target: &IscsiTarget) -> Result<Vec<String>> {
        // After login, find the block devices created
        let output = Command::new("lsscsi").output().await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to list SCSI devices: {}", e))
        })?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(
                "Failed to list SCSI devices".to_string(),
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let devices: Vec<String> = stdout
            .lines()
            .filter_map(|line| {
                // Parse format: "[0:0:0:0]    disk    IET      ...  /dev/sda"
                if line.contains(&target.iqn) || line.contains("iSCSI") {
                    line.split_whitespace().last().map(|s| s.to_string())
                } else {
                    None
                }
            })
            .collect();

        Ok(devices)
    }

    /// Log in to the pool's target and wait until its LUNs appear as block
    /// devices. Safe to repeat (an existing session is kept).
    pub async fn attach(&self, target: &IscsiTarget) -> Result<Vec<IscsiLun>> {
        self.login_target(target).await?;
        for _ in 0..40 {
            let luns = Self::list_luns(target);
            if !luns.is_empty() {
                return Ok(luns);
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        Err(horcrux_common::Error::System(format!(
            "Logged in to {} but no LUN showed up as a block device within 20 s",
            target.iqn
        )))
    }

    /// Log out of the pool's target (no error when there is no session).
    pub async fn detach(&self, target: &IscsiTarget) -> Result<()> {
        let sessions = self.list_sessions().await.unwrap_or_default();
        if !sessions.iter().any(|s| s.iqn == target.iqn) {
            return Ok(());
        }
        self.logout_target(target).await
    }

    /// The LUNs of a logged-in target, from the stable udev links
    /// (`/dev/disk/by-path/ip-<portal>-iscsi-<iqn>-lun-<n>`), sorted by LUN.
    pub fn list_luns(target: &IscsiTarget) -> Vec<IscsiLun> {
        Self::list_luns_in(Path::new("/dev/disk/by-path"), target)
    }

    pub(crate) fn list_luns_in(dir: &Path, target: &IscsiTarget) -> Vec<IscsiLun> {
        let prefix = target.lun_link_prefix();
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(dir) else {
            return out;
        };
        for e in rd.flatten() {
            let file = e.file_name().to_string_lossy().to_string();
            let Some(n) = file
                .strip_prefix(&prefix)
                .and_then(|n| n.parse::<u32>().ok())
            else {
                continue;
            };
            let path = e.path();
            let size_bytes = std::fs::canonicalize(&path)
                .ok()
                .and_then(|dev| dev.file_name().map(|d| d.to_string_lossy().to_string()))
                .and_then(|d| std::fs::read_to_string(format!("/sys/class/block/{d}/size")).ok())
                .and_then(|s| s.trim().parse::<u64>().ok())
                .map(|sectors| sectors * 512)
                .unwrap_or(0);
            out.push(IscsiLun {
                lun: n,
                path: path.to_string_lossy().to_string(),
                size_bytes,
            });
        }
        out.sort_by_key(|l| l.lun);
        out
    }

    /// Create an iSCSI volume (LUN)
    pub async fn create_volume(
        &self,
        target: &IscsiTarget,
        lun_id: u32,
        size_gb: u64,
    ) -> Result<String> {
        info!(
            "Creating iSCSI volume: {} LUN {} ({}GB)",
            target.iqn, lun_id, size_gb
        );

        // This assumes we're using targetcli for local iSCSI target management
        // For remote SAN arrays, this would use vendor-specific APIs

        // Create backstores/fileio
        let backstore_name = format!("disk{}", lun_id);
        let file_path = format!("/var/lib/iscsi/{}.img", backstore_name);

        let output = Command::new("targetcli")
            .args([
                "/backstores/fileio",
                "create",
                &backstore_name,
                &file_path,
                &format!("{}G", size_gb),
            ])
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to create iSCSI backstore: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to create iSCSI backstore: {}",
                stderr
            )));
        }

        // Create LUN
        let output = Command::new("targetcli")
            .args([
                &format!("/iscsi/{}/tpg1/luns", target.iqn),
                "create",
                &format!("/backstores/fileio/{}", backstore_name),
                &lun_id.to_string(),
            ])
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to create iSCSI LUN: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to create iSCSI LUN: {}",
                stderr
            )));
        }

        Ok(format!("{} LUN {}", target.iqn, lun_id))
    }

    /// Delete an iSCSI volume (LUN)
    pub async fn delete_volume(&self, target: &IscsiTarget, lun_id: u32) -> Result<()> {
        info!("Deleting iSCSI volume: {} LUN {}", target.iqn, lun_id);

        let output = Command::new("targetcli")
            .args([
                &format!("/iscsi/{}/tpg1/luns/lun{}", target.iqn, lun_id),
                "delete",
            ])
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to delete iSCSI LUN: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to delete iSCSI LUN: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Configure CHAP authentication
    pub async fn set_chap_auth(
        &self,
        target: &IscsiTarget,
        username: &str,
        password: &str,
    ) -> Result<()> {
        info!("Configuring CHAP authentication for target: {}", target.iqn);

        let output = Command::new("iscsiadm")
            .args([
                "-m",
                "node",
                "-T",
                &target.iqn,
                "-p",
                &target.portal,
                "--op=update",
                "--name",
                "node.session.auth.authmethod",
                "--value=CHAP",
            ])
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to set CHAP auth method: {}", e))
            })?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(
                "Failed to set CHAP auth method".to_string(),
            ));
        }

        // Set username
        let output = Command::new("iscsiadm")
            .args([
                "-m",
                "node",
                "-T",
                &target.iqn,
                "-p",
                &target.portal,
                "--op=update",
                "--name",
                "node.session.auth.username",
                "--value",
                username,
            ])
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to set CHAP username: {}", e))
            })?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(
                "Failed to set CHAP username".to_string(),
            ));
        }

        // Set password
        let output = Command::new("iscsiadm")
            .args([
                "-m",
                "node",
                "-T",
                &target.iqn,
                "-p",
                &target.portal,
                "--op=update",
                "--name",
                "node.session.auth.password",
                "--value",
                password,
            ])
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to set CHAP password: {}", e))
            })?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(
                "Failed to set CHAP password".to_string(),
            ));
        }

        Ok(())
    }

    /// Check if iSCSI tools are available
    pub fn check_iscsi_available() -> bool {
        StdCommand::new("iscsiadm")
            .arg("--version")
            .output()
            .is_ok()
    }
}

/// iSCSI target configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IscsiTarget {
    pub portal: String, // IP:port (e.g., "192.168.1.100:3260")
    pub iqn: String,    // iSCSI Qualified Name (e.g., "iqn.2024-01.com.example:storage")
}

/// One LUN of a logged-in target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IscsiLun {
    pub lun: u32,
    /// Stable udev link (`/dev/disk/by-path/...-lun-N`).
    pub path: String,
    pub size_bytes: u64,
}

impl IscsiTarget {
    /// The portal as udev names it: the port is always present.
    pub fn portal_with_port(&self) -> String {
        if self.portal.contains(':') {
            self.portal.clone()
        } else {
            format!("{}:3260", self.portal)
        }
    }

    /// File-name prefix of this target's LUN links in /dev/disk/by-path.
    pub fn lun_link_prefix(&self) -> String {
        format!("ip-{}-iscsi-{}-lun-", self.portal_with_port(), self.iqn)
    }

    /// Stable device path of LUN `n`.
    pub fn lun_path(&self, n: u32) -> String {
        format!("/dev/disk/by-path/{}{}", self.lun_link_prefix(), n)
    }

    /// Volume name of LUN `n` in a pool (`lun0`, `lun1`, ...).
    pub fn volume_name(n: u32) -> String {
        format!("lun{n}")
    }

    /// LUN number of a volume name made by `volume_name`.
    pub fn parse_volume_name(name: &str) -> Option<u32> {
        name.strip_prefix("lun")?.parse().ok()
    }

    /// Parse iSCSI target from path string
    pub fn parse(path: &str) -> Result<Self> {
        // Expected format: "iscsi://portal:port/iqn"
        // Example: "iscsi://192.168.1.100:3260/iqn.2024-01.com.example:storage"

        if !path.starts_with("iscsi://") {
            return Err(horcrux_common::Error::InvalidConfig(
                "Invalid iSCSI path format".to_string(),
            ));
        }

        let path = path.strip_prefix("iscsi://").unwrap();
        let parts: Vec<&str> = path.splitn(2, '/').collect();

        if parts.len() != 2 {
            return Err(horcrux_common::Error::InvalidConfig(
                "Invalid iSCSI path format".to_string(),
            ));
        }

        Ok(IscsiTarget {
            portal: parts[0].to_string(),
            iqn: parts[1].to_string(),
        })
    }
}

/// iSCSI session information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IscsiSession {
    pub session_id: String,
    pub portal: String,
    pub iqn: String,
    pub state: String,
}

impl IscsiSession {
    fn parse(line: &str) -> Option<Self> {
        // Parse format: "tcp: [1] 192.168.1.100:3260,1 iqn.2024-01.com.example:storage (non-flash)"
        let parts: Vec<&str> = line.split_whitespace().collect();

        if parts.len() < 4 {
            return None;
        }

        let session_id = parts[1].trim_matches(|c| c == '[' || c == ']').to_string();
        let portal = parts[2].trim_end_matches(',').to_string();
        let iqn = parts[3].to_string();
        let state = if parts.len() > 4 {
            parts[4].to_string()
        } else {
            "unknown".to_string()
        };

        Some(IscsiSession {
            session_id,
            portal,
            iqn,
            state,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_iscsi_target() {
        let path = "iscsi://192.168.1.100:3260/iqn.2024-01.com.example:storage";
        let target = IscsiTarget::parse(path).unwrap();

        assert_eq!(target.portal, "192.168.1.100:3260");
        assert_eq!(target.iqn, "iqn.2024-01.com.example:storage");
    }

    #[test]
    fn test_invalid_iscsi_path() {
        let path = "invalid://path";
        assert!(IscsiTarget::parse(path).is_err());
    }

    #[test]
    fn lun_links_and_names() {
        let t = IscsiTarget::parse("iscsi://10.0.0.5/iqn.2024-01.com.example:s").unwrap();
        assert_eq!(
            t.lun_path(2),
            "/dev/disk/by-path/ip-10.0.0.5:3260-iscsi-iqn.2024-01.com.example:s-lun-2"
        );
        assert_eq!(IscsiTarget::volume_name(3), "lun3");
        assert_eq!(IscsiTarget::parse_volume_name("lun3"), Some(3));
        assert_eq!(IscsiTarget::parse_volume_name("disk3"), None);
        assert_eq!(IscsiTarget::parse_volume_name("lunx"), None);
    }

    #[test]
    fn list_luns_reads_only_this_targets_links() {
        let d = std::env::temp_dir().join(format!("hx-iscsi-luns-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let t = IscsiTarget::parse("iscsi://10.0.0.5:3260/iqn.x:a").unwrap();
        for f in [
            "ip-10.0.0.5:3260-iscsi-iqn.x:a-lun-1",
            "ip-10.0.0.5:3260-iscsi-iqn.x:a-lun-0",
            "ip-10.0.0.5:3260-iscsi-iqn.x:a-lun-0-part1",
            "ip-10.0.0.5:3260-iscsi-iqn.x:b-lun-0",
            "pci-0000:00:1f.2-ata-1",
        ] {
            std::fs::write(d.join(f), b"").unwrap();
        }
        let luns = IscsiManager::list_luns_in(&d, &t);
        assert_eq!(luns.iter().map(|l| l.lun).collect::<Vec<_>>(), vec![0, 1]);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
