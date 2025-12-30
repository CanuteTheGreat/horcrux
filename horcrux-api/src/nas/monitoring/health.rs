//! NAS health check module

use horcrux_common::{Error, Result};
use crate::nas::monitoring::HealthStatus;

/// Check SMB health
#[cfg(feature = "smb")]
pub async fn check_smb_health() -> Result<HealthStatus> {
    use tokio::process::Command;

    let output = Command::new("smbstatus")
        .args(["--version"])
        .output()
        .await;

    match output {
        Ok(o) if o.status.success() => Ok(HealthStatus::Healthy),
        Ok(_) => Ok(HealthStatus::Warning),
        Err(_) => Ok(HealthStatus::Unknown),
    }
}

/// Check NFS health
#[cfg(feature = "nfs-server")]
pub async fn check_nfs_health() -> Result<HealthStatus> {
    use tokio::process::Command;

    let output = Command::new("rpcinfo")
        .args(["-p", "localhost"])
        .output()
        .await;

    match output {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            if stdout.contains("nfs") {
                Ok(HealthStatus::Healthy)
            } else {
                Ok(HealthStatus::Warning)
            }
        }
        Err(_) => Ok(HealthStatus::Unknown),
    }
}

/// Check ZFS pool health
#[cfg(feature = "nas-zfs")]
pub async fn check_zfs_health() -> Result<HealthStatus> {
    use tokio::process::Command;

    let output = Command::new("zpool")
        .args(["status", "-x"])
        .output()
        .await;

    match output {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            if stdout.contains("all pools are healthy") {
                Ok(HealthStatus::Healthy)
            } else if stdout.contains("DEGRADED") {
                Ok(HealthStatus::Warning)
            } else if stdout.contains("FAULTED") || stdout.contains("UNAVAIL") {
                Ok(HealthStatus::Critical)
            } else {
                Ok(HealthStatus::Unknown)
            }
        }
        Err(_) => Ok(HealthStatus::Unknown),
    }
}
