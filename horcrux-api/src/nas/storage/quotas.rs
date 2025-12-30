//! Quota management
//!
//! Handles user and group quotas on NAS shares.

use horcrux_common::{Error, Result};
use crate::nas::QuotaLimit;
use tokio::process::Command;

/// Quota usage information
#[derive(Debug, Clone)]
pub struct QuotaUsage {
    /// User or group name
    pub name: String,
    /// Used bytes
    pub used_bytes: u64,
    /// Soft limit bytes
    pub soft_limit_bytes: Option<u64>,
    /// Hard limit bytes
    pub hard_limit_bytes: Option<u64>,
    /// Used inodes
    pub used_inodes: u64,
    /// Inode soft limit
    pub inode_soft: Option<u64>,
    /// Inode hard limit
    pub inode_hard: Option<u64>,
    /// In grace period
    pub in_grace: bool,
}

/// Set user quota on a ZFS dataset
#[cfg(feature = "nas-zfs")]
pub async fn set_zfs_user_quota(dataset: &str, user: &str, limit: &QuotaLimit) -> Result<()> {
    let quota_str = format!("{}G", limit.hard_limit_gb);

    let output = Command::new("zfs")
        .args(["set", &format!("userquota@{}={}", user, quota_str), dataset])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("Failed to set quota: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Internal(format!(
            "Failed to set quota: {}",
            stderr
        )));
    }

    Ok(())
}

/// Set group quota on a ZFS dataset
#[cfg(feature = "nas-zfs")]
pub async fn set_zfs_group_quota(dataset: &str, group: &str, limit: &QuotaLimit) -> Result<()> {
    let quota_str = format!("{}G", limit.hard_limit_gb);

    let output = Command::new("zfs")
        .args(["set", &format!("groupquota@{}={}", group, quota_str), dataset])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("Failed to set quota: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Internal(format!(
            "Failed to set quota: {}",
            stderr
        )));
    }

    Ok(())
}

/// Get user quota usage from ZFS
#[cfg(feature = "nas-zfs")]
pub async fn get_zfs_user_quota(dataset: &str, user: &str) -> Result<QuotaUsage> {
    // Get used space
    let used_output = Command::new("zfs")
        .args(["get", "-H", "-o", "value", &format!("userused@{}", user), dataset])
        .output()
        .await?;

    let used_bytes = if used_output.status.success() {
        let stdout = String::from_utf8_lossy(&used_output.stdout);
        super::parse_size(stdout.trim()).unwrap_or(0)
    } else {
        0
    };

    // Get quota
    let quota_output = Command::new("zfs")
        .args(["get", "-H", "-o", "value", &format!("userquota@{}", user), dataset])
        .output()
        .await?;

    let hard_limit_bytes = if quota_output.status.success() {
        let stdout = String::from_utf8_lossy(&quota_output.stdout);
        let value = stdout.trim();
        if value != "none" && value != "-" {
            super::parse_size(value)
        } else {
            None
        }
    } else {
        None
    };

    Ok(QuotaUsage {
        name: user.to_string(),
        used_bytes,
        soft_limit_bytes: None,
        hard_limit_bytes,
        used_inodes: 0,
        inode_soft: None,
        inode_hard: None,
        in_grace: false,
    })
}

/// Get group quota usage from ZFS
#[cfg(feature = "nas-zfs")]
pub async fn get_zfs_group_quota(dataset: &str, group: &str) -> Result<QuotaUsage> {
    let used_output = Command::new("zfs")
        .args(["get", "-H", "-o", "value", &format!("groupused@{}", group), dataset])
        .output()
        .await?;

    let used_bytes = if used_output.status.success() {
        let stdout = String::from_utf8_lossy(&used_output.stdout);
        super::parse_size(stdout.trim()).unwrap_or(0)
    } else {
        0
    };

    let quota_output = Command::new("zfs")
        .args(["get", "-H", "-o", "value", &format!("groupquota@{}", group), dataset])
        .output()
        .await?;

    let hard_limit_bytes = if quota_output.status.success() {
        let stdout = String::from_utf8_lossy(&quota_output.stdout);
        let value = stdout.trim();
        if value != "none" && value != "-" {
            super::parse_size(value)
        } else {
            None
        }
    } else {
        None
    };

    Ok(QuotaUsage {
        name: group.to_string(),
        used_bytes,
        soft_limit_bytes: None,
        hard_limit_bytes,
        used_inodes: 0,
        inode_soft: None,
        inode_hard: None,
        in_grace: false,
    })
}

/// List all user quotas on a ZFS dataset
#[cfg(feature = "nas-zfs")]
pub async fn list_zfs_user_quotas(dataset: &str) -> Result<Vec<QuotaUsage>> {
    let output = Command::new("zfs")
        .args(["userspace", "-H", "-o", "name,used,quota", dataset])
        .output()
        .await?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut quotas = Vec::new();

    for line in stdout.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 3 {
            let name = parts[0].to_string();
            let used_bytes = super::parse_size(parts[1]).unwrap_or(0);
            let hard_limit_bytes = if parts[2] != "none" {
                super::parse_size(parts[2])
            } else {
                None
            };

            quotas.push(QuotaUsage {
                name,
                used_bytes,
                soft_limit_bytes: None,
                hard_limit_bytes,
                used_inodes: 0,
                inode_soft: None,
                inode_hard: None,
                in_grace: false,
            });
        }
    }

    Ok(quotas)
}

/// Remove user quota
#[cfg(feature = "nas-zfs")]
pub async fn remove_zfs_user_quota(dataset: &str, user: &str) -> Result<()> {
    let output = Command::new("zfs")
        .args(["set", &format!("userquota@{}=none", user), dataset])
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Internal(format!(
            "Failed to remove quota: {}",
            stderr
        )));
    }

    Ok(())
}

/// Set Linux quota (for non-ZFS filesystems)
pub async fn set_linux_user_quota(
    path: &str,
    user: &str,
    soft_limit_gb: Option<u64>,
    hard_limit_gb: u64,
) -> Result<()> {
    // Convert GB to KB for quota command
    let soft_kb = soft_limit_gb.unwrap_or(0) * 1024 * 1024;
    let hard_kb = hard_limit_gb * 1024 * 1024;

    let output = Command::new("setquota")
        .args([
            "-u",
            user,
            &soft_kb.to_string(),
            &hard_kb.to_string(),
            "0", // soft inode
            "0", // hard inode
            path,
        ])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("setquota failed: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Internal(format!(
            "setquota failed: {}",
            stderr
        )));
    }

    Ok(())
}

/// Get Linux quota
pub async fn get_linux_user_quota(path: &str, user: &str) -> Result<QuotaUsage> {
    let output = Command::new("quota")
        .args(["-u", user, "-w", "-p", "-f", path])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("quota failed: {}", e)))?;

    if !output.status.success() {
        return Ok(QuotaUsage {
            name: user.to_string(),
            used_bytes: 0,
            soft_limit_bytes: None,
            hard_limit_bytes: None,
            used_inodes: 0,
            inode_soft: None,
            inode_hard: None,
            in_grace: false,
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_linux_quota_output(&stdout, user)
}

/// Set Linux group quota
pub async fn set_linux_group_quota(
    path: &str,
    group: &str,
    soft_limit_gb: Option<u64>,
    hard_limit_gb: u64,
) -> Result<()> {
    let soft_kb = soft_limit_gb.unwrap_or(0) * 1024 * 1024;
    let hard_kb = hard_limit_gb * 1024 * 1024;

    let output = Command::new("setquota")
        .args([
            "-g",
            group,
            &soft_kb.to_string(),
            &hard_kb.to_string(),
            "0",
            "0",
            path,
        ])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("setquota failed: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Internal(format!("setquota failed: {}", stderr)));
    }

    Ok(())
}

/// Get Linux group quota
pub async fn get_linux_group_quota(path: &str, group: &str) -> Result<QuotaUsage> {
    let output = Command::new("quota")
        .args(["-g", group, "-w", "-p", "-f", path])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("quota failed: {}", e)))?;

    if !output.status.success() {
        return Ok(QuotaUsage {
            name: group.to_string(),
            used_bytes: 0,
            soft_limit_bytes: None,
            hard_limit_bytes: None,
            used_inodes: 0,
            inode_soft: None,
            inode_hard: None,
            in_grace: false,
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_linux_quota_output(&stdout, group)
}

/// Parse Linux quota output
fn parse_linux_quota_output(output: &str, name: &str) -> Result<QuotaUsage> {
    let mut usage = QuotaUsage {
        name: name.to_string(),
        used_bytes: 0,
        soft_limit_bytes: None,
        hard_limit_bytes: None,
        used_inodes: 0,
        inode_soft: None,
        inode_hard: None,
        in_grace: false,
    };

    for line in output.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 4 {
            // Format: filesystem blocks quota limit grace files quota limit grace
            if let Ok(blocks) = parts[1].trim_end_matches('*').parse::<u64>() {
                usage.used_bytes = blocks * 1024; // blocks are in KB
                usage.in_grace = parts[1].ends_with('*');
            }
            if let Ok(soft) = parts[2].parse::<u64>() {
                if soft > 0 {
                    usage.soft_limit_bytes = Some(soft * 1024);
                }
            }
            if let Ok(hard) = parts[3].parse::<u64>() {
                if hard > 0 {
                    usage.hard_limit_bytes = Some(hard * 1024);
                }
            }
            if parts.len() >= 8 {
                if let Ok(inodes) = parts[5].trim_end_matches('*').parse::<u64>() {
                    usage.used_inodes = inodes;
                }
                if let Ok(soft) = parts[6].parse::<u64>() {
                    if soft > 0 {
                        usage.inode_soft = Some(soft);
                    }
                }
                if let Ok(hard) = parts[7].parse::<u64>() {
                    if hard > 0 {
                        usage.inode_hard = Some(hard);
                    }
                }
            }
        }
    }

    Ok(usage)
}

/// Enable quotas on a filesystem
pub async fn enable_quotas(path: &str) -> Result<()> {
    // Check if quota is already enabled
    let output = Command::new("quotaon")
        .args(["-p", path])
        .output()
        .await;

    if let Ok(o) = output {
        if o.status.success() {
            return Ok(());
        }
    }

    // Enable quotas
    let output = Command::new("quotaon")
        .args(["-avug"])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("quotaon failed: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!("quotaon warning: {}", stderr);
    }

    Ok(())
}

/// Check quota database
pub async fn check_quotas(path: &str) -> Result<()> {
    let output = Command::new("quotacheck")
        .args(["-avugm"])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("quotacheck failed: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!("quotacheck warning: {}", stderr);
    }

    let _ = path;
    Ok(())
}

/// Quota status for a user or group
#[derive(Debug, Clone)]
pub enum QuotaStatus {
    /// Under quota
    Ok,
    /// Over soft limit, in grace period
    Warning { percent_used: u8, grace_remaining: Option<i64> },
    /// Over hard limit
    Exceeded,
    /// No quota set
    NoQuota,
}

/// Quota Manager for managing filesystem quotas
pub struct QuotaManager {
    /// Use ZFS quotas when available
    use_zfs: bool,
}

impl QuotaManager {
    /// Create a new quota manager
    pub fn new() -> Self {
        Self { use_zfs: false }
    }

    /// Create a quota manager with ZFS support
    #[cfg(feature = "nas-zfs")]
    pub fn with_zfs() -> Self {
        Self { use_zfs: true }
    }

    /// Set user quota
    pub async fn set_user_quota(
        &self,
        path: &str,
        user: &str,
        soft_limit_gb: Option<u64>,
        hard_limit_gb: u64,
    ) -> Result<()> {
        #[cfg(feature = "nas-zfs")]
        if self.use_zfs {
            let limit = QuotaLimit {
                soft_limit_gb: soft_limit_gb.unwrap_or(hard_limit_gb),
                hard_limit_gb,
                grace_period_days: 7,
            };
            return set_zfs_user_quota(path, user, &limit).await;
        }

        set_linux_user_quota(path, user, soft_limit_gb, hard_limit_gb).await
    }

    /// Set group quota
    pub async fn set_group_quota(
        &self,
        path: &str,
        group: &str,
        soft_limit_gb: Option<u64>,
        hard_limit_gb: u64,
    ) -> Result<()> {
        #[cfg(feature = "nas-zfs")]
        if self.use_zfs {
            let limit = QuotaLimit {
                soft_limit_gb: soft_limit_gb.unwrap_or(hard_limit_gb),
                hard_limit_gb,
                grace_period_days: 7,
            };
            return set_zfs_group_quota(path, group, &limit).await;
        }

        set_linux_group_quota(path, group, soft_limit_gb, hard_limit_gb).await
    }

    /// Get user quota usage
    pub async fn get_user_quota(&self, path: &str, user: &str) -> Result<QuotaUsage> {
        #[cfg(feature = "nas-zfs")]
        if self.use_zfs {
            return get_zfs_user_quota(path, user).await;
        }

        get_linux_user_quota(path, user).await
    }

    /// Get group quota usage
    pub async fn get_group_quota(&self, path: &str, group: &str) -> Result<QuotaUsage> {
        #[cfg(feature = "nas-zfs")]
        if self.use_zfs {
            return get_zfs_group_quota(path, group).await;
        }

        get_linux_group_quota(path, group).await
    }

    /// Remove user quota
    pub async fn remove_user_quota(&self, path: &str, user: &str) -> Result<()> {
        #[cfg(feature = "nas-zfs")]
        if self.use_zfs {
            return remove_zfs_user_quota(path, user).await;
        }

        set_linux_user_quota(path, user, None, 0).await
    }

    /// Remove group quota
    pub async fn remove_group_quota(&self, path: &str, group: &str) -> Result<()> {
        #[cfg(feature = "nas-zfs")]
        if self.use_zfs {
            let output = Command::new("zfs")
                .args(["set", &format!("groupquota@{}=none", group), path])
                .output()
                .await?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(Error::Internal(format!("Failed to remove quota: {}", stderr)));
            }
            return Ok(());
        }

        set_linux_group_quota(path, group, None, 0).await
    }

    /// Check quota status for a user
    pub async fn check_user_status(&self, path: &str, user: &str) -> Result<QuotaStatus> {
        let usage = self.get_user_quota(path, user).await?;
        Ok(compute_quota_status(&usage))
    }

    /// Check quota status for a group
    pub async fn check_group_status(&self, path: &str, group: &str) -> Result<QuotaStatus> {
        let usage = self.get_group_quota(path, group).await?;
        Ok(compute_quota_status(&usage))
    }

    /// List all user quotas
    pub async fn list_user_quotas(&self, path: &str) -> Result<Vec<QuotaUsage>> {
        #[cfg(feature = "nas-zfs")]
        if self.use_zfs {
            return list_zfs_user_quotas(path).await;
        }

        // For Linux quotas, use repquota
        let output = Command::new("repquota")
            .args(["-u", "-n", "-p", path])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("repquota failed: {}", e)))?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut quotas = Vec::new();

        for line in stdout.lines().skip(5) { // Skip header lines
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 7 {
                let name = parts[0].to_string();
                let used_bytes = parts[2].parse::<u64>().unwrap_or(0) * 1024;
                let soft = parts[3].parse::<u64>().ok().filter(|&v| v > 0).map(|v| v * 1024);
                let hard = parts[4].parse::<u64>().ok().filter(|&v| v > 0).map(|v| v * 1024);

                quotas.push(QuotaUsage {
                    name,
                    used_bytes,
                    soft_limit_bytes: soft,
                    hard_limit_bytes: hard,
                    used_inodes: parts[5].parse().unwrap_or(0),
                    inode_soft: parts[6].parse::<u64>().ok().filter(|&v| v > 0),
                    inode_hard: parts.get(7).and_then(|s| s.parse::<u64>().ok()).filter(|&v| v > 0),
                    in_grace: parts[1].contains('+'),
                });
            }
        }

        Ok(quotas)
    }

    /// List all group quotas
    pub async fn list_group_quotas(&self, path: &str) -> Result<Vec<QuotaUsage>> {
        #[cfg(feature = "nas-zfs")]
        if self.use_zfs {
            let output = Command::new("zfs")
                .args(["groupspace", "-H", "-o", "name,used,quota", path])
                .output()
                .await?;

            if !output.status.success() {
                return Ok(Vec::new());
            }

            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut quotas = Vec::new();

            for line in stdout.lines() {
                let parts: Vec<&str> = line.split('\t').collect();
                if parts.len() >= 3 {
                    quotas.push(QuotaUsage {
                        name: parts[0].to_string(),
                        used_bytes: super::parse_size(parts[1]).unwrap_or(0),
                        soft_limit_bytes: None,
                        hard_limit_bytes: if parts[2] != "none" {
                            super::parse_size(parts[2])
                        } else {
                            None
                        },
                        used_inodes: 0,
                        inode_soft: None,
                        inode_hard: None,
                        in_grace: false,
                    });
                }
            }

            return Ok(quotas);
        }

        // For Linux quotas
        let output = Command::new("repquota")
            .args(["-g", "-n", "-p", path])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("repquota failed: {}", e)))?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut quotas = Vec::new();

        for line in stdout.lines().skip(5) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 7 {
                quotas.push(QuotaUsage {
                    name: parts[0].to_string(),
                    used_bytes: parts[2].parse::<u64>().unwrap_or(0) * 1024,
                    soft_limit_bytes: parts[3].parse::<u64>().ok().filter(|&v| v > 0).map(|v| v * 1024),
                    hard_limit_bytes: parts[4].parse::<u64>().ok().filter(|&v| v > 0).map(|v| v * 1024),
                    used_inodes: parts[5].parse().unwrap_or(0),
                    inode_soft: parts[6].parse::<u64>().ok().filter(|&v| v > 0),
                    inode_hard: parts.get(7).and_then(|s| s.parse::<u64>().ok()).filter(|&v| v > 0),
                    in_grace: parts[1].contains('+'),
                });
            }
        }

        Ok(quotas)
    }

    /// Generate quota report
    pub async fn generate_report(&self, path: &str) -> Result<QuotaReport> {
        let user_quotas = self.list_user_quotas(path).await?;
        let group_quotas = self.list_group_quotas(path).await?;

        let mut over_quota_users = Vec::new();
        let mut warning_users = Vec::new();

        for usage in &user_quotas {
            match compute_quota_status(usage) {
                QuotaStatus::Exceeded => over_quota_users.push(usage.name.clone()),
                QuotaStatus::Warning { .. } => warning_users.push(usage.name.clone()),
                _ => {}
            }
        }

        let mut over_quota_groups = Vec::new();
        let mut warning_groups = Vec::new();

        for usage in &group_quotas {
            match compute_quota_status(usage) {
                QuotaStatus::Exceeded => over_quota_groups.push(usage.name.clone()),
                QuotaStatus::Warning { .. } => warning_groups.push(usage.name.clone()),
                _ => {}
            }
        }

        Ok(QuotaReport {
            path: path.to_string(),
            user_quotas,
            group_quotas,
            over_quota_users,
            warning_users,
            over_quota_groups,
            warning_groups,
        })
    }
}

impl Default for QuotaManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute quota status from usage
fn compute_quota_status(usage: &QuotaUsage) -> QuotaStatus {
    let hard_limit = match usage.hard_limit_bytes {
        Some(limit) if limit > 0 => limit,
        _ => return QuotaStatus::NoQuota,
    };

    if usage.used_bytes >= hard_limit {
        return QuotaStatus::Exceeded;
    }

    if let Some(soft_limit) = usage.soft_limit_bytes {
        if soft_limit > 0 && usage.used_bytes >= soft_limit {
            let percent = ((usage.used_bytes as f64 / hard_limit as f64) * 100.0) as u8;
            return QuotaStatus::Warning {
                percent_used: percent,
                grace_remaining: None, // Would need to parse grace period
            };
        }
    }

    QuotaStatus::Ok
}

/// Quota report for a filesystem
#[derive(Debug, Clone)]
pub struct QuotaReport {
    /// Filesystem path
    pub path: String,
    /// User quotas
    pub user_quotas: Vec<QuotaUsage>,
    /// Group quotas
    pub group_quotas: Vec<QuotaUsage>,
    /// Users over quota
    pub over_quota_users: Vec<String>,
    /// Users in warning (over soft limit)
    pub warning_users: Vec<String>,
    /// Groups over quota
    pub over_quota_groups: Vec<String>,
    /// Groups in warning
    pub warning_groups: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quota_usage_defaults() {
        let usage = QuotaUsage {
            name: "test".to_string(),
            used_bytes: 1000,
            soft_limit_bytes: None,
            hard_limit_bytes: Some(10000),
            used_inodes: 100,
            inode_soft: None,
            inode_hard: None,
            in_grace: false,
        };

        assert_eq!(usage.used_bytes, 1000);
        assert_eq!(usage.hard_limit_bytes, Some(10000));
    }
}
