//! NAS Services module
//!
//! Manages NAS-related system services including:
//! - SMB (smbd, nmbd, winbindd)
//! - NFS (nfs-server, rpcbind, mountd)
//! - AFP (netatalk)
//! - S3 Gateway (minio)
//! - iSCSI Target (tgt)
//! - Rsync (rsyncd)

#[cfg(feature = "s3-gateway")]
pub mod s3;
#[cfg(feature = "iscsi-target")]
pub mod iscsi;
#[cfg(feature = "rsync-server")]
pub mod rsync;
#[cfg(feature = "timemachine")]
pub mod timemachine;

use horcrux_common::{Error, Result};
use crate::nas::ServiceStatus;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

/// NAS service types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NasService {
    /// Samba SMB daemon
    Smbd,
    /// NetBIOS name service daemon
    Nmbd,
    /// Winbind daemon for AD/domain support
    Winbindd,
    /// NFS server
    NfsServer,
    /// RPC portmapper
    Rpcbind,
    /// NFS mount daemon
    Mountd,
    /// Netatalk (AFP)
    Netatalk,
    /// ProFTPD
    Proftpd,
    /// OpenSSH (for SFTP)
    Sshd,
    /// MinIO (S3 gateway)
    Minio,
    /// iSCSI target daemon
    Tgtd,
    /// Rsync daemon
    Rsyncd,
}

impl NasService {
    /// Get all NAS services
    pub fn all() -> Vec<NasService> {
        vec![
            NasService::Smbd,
            NasService::Nmbd,
            NasService::Winbindd,
            NasService::NfsServer,
            NasService::Rpcbind,
            NasService::Mountd,
            NasService::Netatalk,
            NasService::Proftpd,
            NasService::Sshd,
            NasService::Minio,
            NasService::Tgtd,
            NasService::Rsyncd,
        ]
    }

    /// Get the systemd/OpenRC service name
    pub fn service_name(&self) -> &'static str {
        match self {
            NasService::Smbd => "smbd",
            NasService::Nmbd => "nmbd",
            NasService::Winbindd => "winbindd",
            NasService::NfsServer => "nfs-server",
            NasService::Rpcbind => "rpcbind",
            NasService::Mountd => "nfs-mountd",
            NasService::Netatalk => "netatalk",
            NasService::Proftpd => "proftpd",
            NasService::Sshd => "sshd",
            NasService::Minio => "minio",
            NasService::Tgtd => "tgtd",
            NasService::Rsyncd => "rsyncd",
        }
    }

    /// Get the OpenRC service name (may differ from systemd)
    pub fn openrc_name(&self) -> &'static str {
        match self {
            NasService::Smbd => "samba",
            NasService::Nmbd => "samba",
            NasService::Winbindd => "samba",
            NasService::NfsServer => "nfs",
            NasService::Rpcbind => "rpcbind",
            NasService::Mountd => "nfs",
            NasService::Netatalk => "netatalk",
            NasService::Proftpd => "proftpd",
            NasService::Sshd => "sshd",
            NasService::Minio => "minio",
            NasService::Tgtd => "tgtd",
            NasService::Rsyncd => "rsyncd",
        }
    }
}

impl std::fmt::Display for NasService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.service_name())
    }
}

/// Service action
pub enum ServiceAction {
    Start,
    Stop,
    Restart,
    Reload,
    Enable,
    Disable,
}

impl ServiceAction {
    fn as_str(&self) -> &'static str {
        match self {
            ServiceAction::Start => "start",
            ServiceAction::Stop => "stop",
            ServiceAction::Restart => "restart",
            ServiceAction::Reload => "reload",
            ServiceAction::Enable => "enable",
            ServiceAction::Disable => "disable",
        }
    }
}

/// Init system type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitSystem {
    Systemd,
    OpenRC,
}

/// Detect the init system
pub fn detect_init_system() -> InitSystem {
    if std::path::Path::new("/run/systemd/system").exists() {
        InitSystem::Systemd
    } else {
        InitSystem::OpenRC
    }
}

/// Manage a NAS service
pub async fn manage_service(service: &NasService, action: ServiceAction) -> Result<()> {
    let init_system = detect_init_system();

    match init_system {
        InitSystem::Systemd => {
            let output = Command::new("systemctl")
                .args([action.as_str(), service.service_name()])
                .output()
                .await
                .map_err(|e| {
                    Error::Internal(format!("Failed to run systemctl: {}", e))
                })?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(Error::Internal(format!(
                    "systemctl {} {} failed: {}",
                    action.as_str(),
                    service.service_name(),
                    stderr
                )));
            }
        }
        InitSystem::OpenRC => {
            let (cmd, args) = match action {
                ServiceAction::Enable => ("rc-update", vec!["add", service.openrc_name(), "default"]),
                ServiceAction::Disable => ("rc-update", vec!["del", service.openrc_name(), "default"]),
                _ => ("rc-service", vec![service.openrc_name(), action.as_str()]),
            };

            let output = Command::new(cmd)
                .args(&args)
                .output()
                .await
                .map_err(|e| {
                    Error::Internal(format!("Failed to run {}: {}", cmd, e))
                })?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(Error::Internal(format!(
                    "{} {:?} failed: {}",
                    cmd,
                    args,
                    stderr
                )));
            }
        }
    }

    Ok(())
}

/// Get service status
pub async fn get_service_status(service: &NasService) -> Result<ServiceStatus> {
    let init_system = detect_init_system();

    let (running, enabled, pid) = match init_system {
        InitSystem::Systemd => get_systemd_status(service).await?,
        InitSystem::OpenRC => get_openrc_status(service).await?,
    };

    // Get uptime and connections based on service type
    let (uptime_seconds, connections) = match service {
        #[cfg(feature = "smb")]
        NasService::Smbd => get_smb_stats().await.unwrap_or((None, 0)),
        #[cfg(feature = "nfs-server")]
        NasService::NfsServer => get_nfs_stats().await.unwrap_or((None, 0)),
        _ => (None, 0),
    };

    Ok(ServiceStatus {
        service: *service,
        running,
        enabled,
        pid,
        uptime_seconds,
        connections,
        last_error: None,
    })
}

/// Get systemd service status
async fn get_systemd_status(service: &NasService) -> Result<(bool, bool, Option<u32>)> {
    // Check if running
    let is_active = Command::new("systemctl")
        .args(["is-active", "--quiet", service.service_name()])
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false);

    // Check if enabled
    let is_enabled = Command::new("systemctl")
        .args(["is-enabled", "--quiet", service.service_name()])
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false);

    // Get PID
    let pid = if is_active {
        let output = Command::new("systemctl")
            .args(["show", "--property=MainPID", "--value", service.service_name()])
            .output()
            .await
            .ok();

        output.and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse::<u32>()
                .ok()
                .filter(|&p| p > 0)
        })
    } else {
        None
    };

    Ok((is_active, is_enabled, pid))
}

/// Get OpenRC service status
async fn get_openrc_status(service: &NasService) -> Result<(bool, bool, Option<u32>)> {
    // Check if running
    let status_output = Command::new("rc-service")
        .args([service.openrc_name(), "status"])
        .output()
        .await
        .ok();

    let is_active = status_output
        .map(|o| {
            let stdout = String::from_utf8_lossy(&o.stdout);
            stdout.contains("started") || stdout.contains("running")
        })
        .unwrap_or(false);

    // Check if enabled
    let enabled_output = Command::new("rc-update")
        .args(["show", "default"])
        .output()
        .await
        .ok();

    let is_enabled = enabled_output
        .map(|o| {
            let stdout = String::from_utf8_lossy(&o.stdout);
            stdout.contains(service.openrc_name())
        })
        .unwrap_or(false);

    // Get PID from /var/run
    let pid_file = format!("/var/run/{}.pid", service.openrc_name());
    let pid = tokio::fs::read_to_string(&pid_file)
        .await
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok());

    Ok((is_active, is_enabled, pid))
}

/// Get SMB statistics
#[cfg(feature = "smb")]
async fn get_smb_stats() -> Result<(Option<u64>, u32)> {
    let output = Command::new("smbstatus")
        .args(["--shares", "--json"])
        .output()
        .await
        .ok();

    if let Some(output) = output {
        if output.status.success() {
            // Parse JSON output to count connections
            let stdout = String::from_utf8_lossy(&output.stdout);
            let connections = stdout.matches("\"pid\"").count() as u32;
            return Ok((None, connections));
        }
    }

    Ok((None, 0))
}

/// Get NFS statistics
#[cfg(feature = "nfs-server")]
async fn get_nfs_stats() -> Result<(Option<u64>, u32)> {
    // Count NFS clients from /proc/fs/nfsd/clients
    let clients_dir = std::path::Path::new("/proc/fs/nfsd/clients");
    if clients_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(clients_dir) {
            let count = entries.count() as u32;
            return Ok((None, count));
        }
    }

    Ok((None, 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_service_names() {
        assert_eq!(NasService::Smbd.service_name(), "smbd");
        assert_eq!(NasService::NfsServer.service_name(), "nfs-server");
        assert_eq!(NasService::Minio.service_name(), "minio");
    }

    #[test]
    fn test_openrc_names() {
        // Samba has different OpenRC naming
        assert_eq!(NasService::Smbd.openrc_name(), "samba");
        assert_eq!(NasService::NfsServer.openrc_name(), "nfs");
    }

    #[test]
    fn test_all_services() {
        let all = NasService::all();
        assert!(all.len() >= 12);
        assert!(all.contains(&NasService::Smbd));
    }
}
