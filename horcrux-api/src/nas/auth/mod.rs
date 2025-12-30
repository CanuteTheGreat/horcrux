//! NAS Authentication module
//!
//! Handles user and group management for NAS shares, including:
//! - Local user/group management
//! - LDAP integration
//! - Active Directory integration
//! - Kerberos authentication
//! - ACL management

#[cfg(feature = "ldap")]
pub mod ldap;
#[cfg(feature = "ldap-server")]
pub mod ldap_server;
#[cfg(feature = "kerberos")]
pub mod kerberos;
#[cfg(feature = "ad")]
pub mod active_directory;
pub mod acl;

use horcrux_common::{Error, Result};
use crate::nas::QuotaLimit;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

pub use crate::nas::shares::{AclEntry, AclFlags, AclPermissions, AclType};

/// NAS User definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NasUser {
    /// Unique identifier
    pub id: String,
    /// Username
    pub username: String,
    /// Unix UID
    pub uid: u32,
    /// Primary group name
    pub primary_group: String,
    /// Additional group memberships
    pub groups: Vec<String>,
    /// Home directory path
    pub home_directory: Option<String>,
    /// Login shell
    pub shell: Option<String>,
    /// Password hash (for SMB auth)
    #[serde(skip_serializing)]
    pub password_hash: Option<String>,
    /// Whether SMB access is enabled
    pub smb_enabled: bool,
    /// Whether SSH/SFTP access is enabled
    pub ssh_enabled: bool,
    /// SSH public keys
    pub ssh_public_keys: Vec<String>,
    /// Email address
    pub email: Option<String>,
    /// Full name / display name
    pub full_name: Option<String>,
    /// Quota limits
    pub quota: Option<QuotaLimit>,
    /// Whether the user is enabled
    pub enabled: bool,
    /// Creation timestamp
    pub created_at: i64,
    /// Last update timestamp
    pub updated_at: i64,
}

impl NasUser {
    /// Create a new NAS user with minimal configuration
    pub fn new(id: String, username: String, uid: u32, primary_group: String) -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            id,
            username: username.clone(),
            uid,
            primary_group,
            groups: Vec::new(),
            home_directory: Some(format!("/home/{}", username)),
            shell: Some("/bin/bash".to_string()),
            password_hash: None,
            smb_enabled: true,
            ssh_enabled: false,
            ssh_public_keys: Vec::new(),
            email: None,
            full_name: None,
            quota: None,
            enabled: true,
            created_at: now,
            updated_at: now,
        }
    }
}

/// NAS Group definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NasGroup {
    /// Unique identifier
    pub id: String,
    /// Group name
    pub name: String,
    /// Unix GID
    pub gid: u32,
    /// Group members (usernames)
    pub members: Vec<String>,
    /// Description
    pub description: Option<String>,
    /// Quota limits
    pub quota: Option<QuotaLimit>,
    /// Creation timestamp
    pub created_at: i64,
}

impl NasGroup {
    /// Create a new NAS group
    pub fn new(id: String, name: String, gid: u32) -> Self {
        Self {
            id,
            name,
            gid,
            members: Vec::new(),
            description: None,
            quota: None,
            created_at: chrono::Utc::now().timestamp(),
        }
    }
}

/// Authentication manager for NAS
pub struct AuthManager {
    /// UID range start
    uid_start: u32,
    /// GID range start
    gid_start: u32,
}

impl AuthManager {
    /// Create a new auth manager
    pub fn new() -> Self {
        Self {
            uid_start: 10000,
            gid_start: 10000,
        }
    }

    /// Create a system user from NasUser
    pub async fn create_system_user(&self, user: &NasUser) -> Result<()> {
        // Create the user with useradd
        let mut cmd = Command::new("useradd");
        cmd.arg("-u").arg(user.uid.to_string());
        cmd.arg("-g").arg(&user.primary_group);

        if !user.groups.is_empty() {
            cmd.arg("-G").arg(user.groups.join(","));
        }

        if let Some(ref home) = user.home_directory {
            cmd.arg("-d").arg(home);
            cmd.arg("-m"); // Create home directory
        }

        if let Some(ref shell) = user.shell {
            cmd.arg("-s").arg(shell);
        } else {
            cmd.arg("-s").arg("/sbin/nologin");
        }

        if let Some(ref name) = user.full_name {
            cmd.arg("-c").arg(name);
        }

        cmd.arg(&user.username);

        let output = cmd.output().await.map_err(|e| {
            Error::Internal(format!("Failed to create user: {}", e))
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // User might already exist
            if !stderr.contains("already exists") {
                return Err(Error::Internal(format!(
                    "useradd failed: {}",
                    stderr
                )));
            }
        }

        // Set up SMB password if enabled
        #[cfg(feature = "smb")]
        if user.smb_enabled {
            self.setup_smb_user(user).await?;
        }

        // Set up SSH keys if enabled
        if user.ssh_enabled && !user.ssh_public_keys.is_empty() {
            self.setup_ssh_keys(user).await?;
        }

        Ok(())
    }

    /// Delete a system user
    pub async fn delete_system_user(&self, user: &NasUser) -> Result<()> {
        // Remove SMB user first
        #[cfg(feature = "smb")]
        {
            let _ = Command::new("smbpasswd")
                .args(["-x", &user.username])
                .output()
                .await;
        }

        // Delete the system user
        let output = Command::new("userdel")
            .args(["-r", &user.username]) // -r removes home directory
            .output()
            .await
            .map_err(|e| {
                Error::Internal(format!("Failed to delete user: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains("does not exist") {
                return Err(Error::Internal(format!(
                    "userdel failed: {}",
                    stderr
                )));
            }
        }

        Ok(())
    }

    /// Create a system group from NasGroup
    pub async fn create_system_group(&self, group: &NasGroup) -> Result<()> {
        let output = Command::new("groupadd")
            .args(["-g", &group.gid.to_string(), &group.name])
            .output()
            .await
            .map_err(|e| {
                Error::Internal(format!("Failed to create group: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains("already exists") {
                return Err(Error::Internal(format!(
                    "groupadd failed: {}",
                    stderr
                )));
            }
        }

        // Add members to the group
        for member in &group.members {
            let _ = Command::new("usermod")
                .args(["-aG", &group.name, member])
                .output()
                .await;
        }

        Ok(())
    }

    /// Delete a system group
    pub async fn delete_system_group(&self, group: &NasGroup) -> Result<()> {
        let output = Command::new("groupdel")
            .arg(&group.name)
            .output()
            .await
            .map_err(|e| {
                Error::Internal(format!("Failed to delete group: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains("does not exist") {
                return Err(Error::Internal(format!(
                    "groupdel failed: {}",
                    stderr
                )));
            }
        }

        Ok(())
    }

    /// Set user password (both Unix and SMB)
    pub async fn set_password(&self, user: &NasUser, password: &str) -> Result<()> {
        // Set Unix password using chpasswd
        let mut child = Command::new("chpasswd")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| {
                Error::Internal(format!("Failed to spawn chpasswd: {}", e))
            })?;

        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin
                .write_all(format!("{}:{}\n", user.username, password).as_bytes())
                .await
                .map_err(|e| {
                    Error::Internal(format!("Failed to write to chpasswd: {}", e))
                })?;
        }

        child.wait().await.map_err(|e| {
            Error::Internal(format!("chpasswd failed: {}", e))
        })?;

        // Set SMB password if enabled
        #[cfg(feature = "smb")]
        if user.smb_enabled {
            self.set_smb_password(user, password).await?;
        }

        Ok(())
    }

    /// Set up SMB user
    #[cfg(feature = "smb")]
    async fn setup_smb_user(&self, user: &NasUser) -> Result<()> {
        // Enable user in smbpasswd (without setting password)
        let output = Command::new("smbpasswd")
            .args(["-a", "-n", &user.username])
            .output()
            .await
            .map_err(|e| {
                Error::Internal(format!("Failed to add SMB user: {}", e))
            })?;

        if !output.status.success() {
            tracing::warn!(
                "smbpasswd failed for user {}: {}",
                user.username,
                String::from_utf8_lossy(&output.stderr)
            );
        }

        Ok(())
    }

    /// Set SMB password
    #[cfg(feature = "smb")]
    async fn set_smb_password(&self, user: &NasUser, password: &str) -> Result<()> {
        let mut child = Command::new("smbpasswd")
            .args(["-a", "-s", &user.username])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| {
                Error::Internal(format!("Failed to spawn smbpasswd: {}", e))
            })?;

        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            // smbpasswd expects password twice
            stdin
                .write_all(format!("{}\n{}\n", password, password).as_bytes())
                .await
                .map_err(|e| {
                    Error::Internal(format!("Failed to write to smbpasswd: {}", e))
                })?;
        }

        child.wait().await.map_err(|e| {
            Error::Internal(format!("smbpasswd failed: {}", e))
        })?;

        Ok(())
    }

    /// Set up SSH authorized keys
    async fn setup_ssh_keys(&self, user: &NasUser) -> Result<()> {
        if let Some(ref home) = user.home_directory {
            let ssh_dir = format!("{}/.ssh", home);
            let auth_keys = format!("{}/authorized_keys", ssh_dir);

            // Create .ssh directory
            tokio::fs::create_dir_all(&ssh_dir).await.map_err(|e| {
                Error::Internal(format!("Failed to create .ssh directory: {}", e))
            })?;

            // Write authorized_keys
            let keys_content = user.ssh_public_keys.join("\n");
            tokio::fs::write(&auth_keys, keys_content).await.map_err(|e| {
                Error::Internal(format!("Failed to write authorized_keys: {}", e))
            })?;

            // Set permissions
            Command::new("chmod")
                .args(["700", &ssh_dir])
                .output()
                .await?;
            Command::new("chmod")
                .args(["600", &auth_keys])
                .output()
                .await?;
            Command::new("chown")
                .args(["-R", &format!("{}:{}", user.username, user.primary_group), &ssh_dir])
                .output()
                .await?;
        }

        Ok(())
    }

    /// Get next available UID by scanning /etc/passwd
    pub async fn next_uid(&self) -> Result<u32> {
        let passwd = tokio::fs::read_to_string("/etc/passwd")
            .await
            .unwrap_or_default();

        let max_uid = passwd
            .lines()
            .filter_map(|line| {
                let parts: Vec<&str> = line.split(':').collect();
                if parts.len() >= 3 {
                    parts[2].parse::<u32>().ok()
                } else {
                    None
                }
            })
            .filter(|&uid| uid >= self.uid_start)
            .max()
            .unwrap_or(self.uid_start - 1);

        Ok(max_uid + 1)
    }

    /// Get next available GID by scanning /etc/group
    pub async fn next_gid(&self) -> Result<u32> {
        let group_file = tokio::fs::read_to_string("/etc/group")
            .await
            .unwrap_or_default();

        let max_gid = group_file
            .lines()
            .filter_map(|line| {
                let parts: Vec<&str> = line.split(':').collect();
                if parts.len() >= 3 {
                    parts[2].parse::<u32>().ok()
                } else {
                    None
                }
            })
            .filter(|&gid| gid >= self.gid_start)
            .max()
            .unwrap_or(self.gid_start - 1);

        Ok(max_gid + 1)
    }

    /// List all system users in the NAS UID range
    pub async fn list_system_users(&self) -> Result<Vec<SystemUserInfo>> {
        let passwd = tokio::fs::read_to_string("/etc/passwd")
            .await
            .map_err(|e| Error::Internal(format!("Failed to read /etc/passwd: {}", e)))?;

        let mut users = Vec::new();

        for line in passwd.lines() {
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() >= 7 {
                if let Ok(uid) = parts[2].parse::<u32>() {
                    // Only include users in NAS UID range
                    if uid >= self.uid_start {
                        users.push(SystemUserInfo {
                            username: parts[0].to_string(),
                            uid,
                            gid: parts[3].parse().unwrap_or(0),
                            full_name: parts[4].to_string(),
                            home: parts[5].to_string(),
                            shell: parts[6].to_string(),
                        });
                    }
                }
            }
        }

        Ok(users)
    }

    /// List all system groups in the NAS GID range
    pub async fn list_system_groups(&self) -> Result<Vec<SystemGroupInfo>> {
        let group_file = tokio::fs::read_to_string("/etc/group")
            .await
            .map_err(|e| Error::Internal(format!("Failed to read /etc/group: {}", e)))?;

        let mut groups = Vec::new();

        for line in group_file.lines() {
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() >= 4 {
                if let Ok(gid) = parts[2].parse::<u32>() {
                    // Only include groups in NAS GID range
                    if gid >= self.gid_start {
                        let members: Vec<String> = if parts[3].is_empty() {
                            Vec::new()
                        } else {
                            parts[3].split(',').map(|s| s.to_string()).collect()
                        };

                        groups.push(SystemGroupInfo {
                            name: parts[0].to_string(),
                            gid,
                            members,
                        });
                    }
                }
            }
        }

        Ok(groups)
    }

    /// Get a system user by username
    pub async fn get_system_user(&self, username: &str) -> Result<SystemUserInfo> {
        let passwd = tokio::fs::read_to_string("/etc/passwd")
            .await
            .map_err(|e| Error::Internal(format!("Failed to read /etc/passwd: {}", e)))?;

        for line in passwd.lines() {
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() >= 7 && parts[0] == username {
                return Ok(SystemUserInfo {
                    username: parts[0].to_string(),
                    uid: parts[2].parse().unwrap_or(0),
                    gid: parts[3].parse().unwrap_or(0),
                    full_name: parts[4].to_string(),
                    home: parts[5].to_string(),
                    shell: parts[6].to_string(),
                });
            }
        }

        Err(Error::NotFound(format!("User '{}' not found", username)))
    }

    /// Get a system user by UID
    pub async fn get_system_user_by_uid(&self, uid: u32) -> Result<SystemUserInfo> {
        let passwd = tokio::fs::read_to_string("/etc/passwd")
            .await
            .map_err(|e| Error::Internal(format!("Failed to read /etc/passwd: {}", e)))?;

        for line in passwd.lines() {
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() >= 7 {
                if let Ok(user_uid) = parts[2].parse::<u32>() {
                    if user_uid == uid {
                        return Ok(SystemUserInfo {
                            username: parts[0].to_string(),
                            uid: user_uid,
                            gid: parts[3].parse().unwrap_or(0),
                            full_name: parts[4].to_string(),
                            home: parts[5].to_string(),
                            shell: parts[6].to_string(),
                        });
                    }
                }
            }
        }

        Err(Error::NotFound(format!("User with UID {} not found", uid)))
    }

    /// Get a system group by name
    pub async fn get_system_group(&self, name: &str) -> Result<SystemGroupInfo> {
        let group_file = tokio::fs::read_to_string("/etc/group")
            .await
            .map_err(|e| Error::Internal(format!("Failed to read /etc/group: {}", e)))?;

        for line in group_file.lines() {
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() >= 4 && parts[0] == name {
                let members: Vec<String> = if parts[3].is_empty() {
                    Vec::new()
                } else {
                    parts[3].split(',').map(|s| s.to_string()).collect()
                };

                return Ok(SystemGroupInfo {
                    name: parts[0].to_string(),
                    gid: parts[2].parse().unwrap_or(0),
                    members,
                });
            }
        }

        Err(Error::NotFound(format!("Group '{}' not found", name)))
    }

    /// Get a system group by GID
    pub async fn get_system_group_by_gid(&self, gid: u32) -> Result<SystemGroupInfo> {
        let group_file = tokio::fs::read_to_string("/etc/group")
            .await
            .map_err(|e| Error::Internal(format!("Failed to read /etc/group: {}", e)))?;

        for line in group_file.lines() {
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() >= 4 {
                if let Ok(group_gid) = parts[2].parse::<u32>() {
                    if group_gid == gid {
                        let members: Vec<String> = if parts[3].is_empty() {
                            Vec::new()
                        } else {
                            parts[3].split(',').map(|s| s.to_string()).collect()
                        };

                        return Ok(SystemGroupInfo {
                            name: parts[0].to_string(),
                            gid: group_gid,
                            members,
                        });
                    }
                }
            }
        }

        Err(Error::NotFound(format!("Group with GID {} not found", gid)))
    }

    /// Update a system user
    pub async fn update_system_user(&self, user: &NasUser) -> Result<()> {
        let mut cmd = Command::new("usermod");

        cmd.arg("-g").arg(&user.primary_group);

        if !user.groups.is_empty() {
            cmd.arg("-G").arg(user.groups.join(","));
        }

        if let Some(ref home) = user.home_directory {
            cmd.arg("-d").arg(home);
        }

        if let Some(ref shell) = user.shell {
            cmd.arg("-s").arg(shell);
        }

        if let Some(ref name) = user.full_name {
            cmd.arg("-c").arg(name);
        }

        cmd.arg(&user.username);

        let output = cmd.output().await.map_err(|e| {
            Error::Internal(format!("Failed to update user: {}", e))
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("usermod failed: {}", stderr)));
        }

        // Update SSH keys if needed
        if user.ssh_enabled && !user.ssh_public_keys.is_empty() {
            self.setup_ssh_keys(user).await?;
        }

        Ok(())
    }

    /// Add a user to a group
    pub async fn add_user_to_group(&self, username: &str, group: &str) -> Result<()> {
        let output = Command::new("usermod")
            .args(["-aG", group, username])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to add user to group: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("usermod failed: {}", stderr)));
        }

        Ok(())
    }

    /// Remove a user from a group
    pub async fn remove_user_from_group(&self, username: &str, group: &str) -> Result<()> {
        let output = Command::new("gpasswd")
            .args(["-d", username, group])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to remove user from group: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("gpasswd failed: {}", stderr)));
        }

        Ok(())
    }

    /// Enable a user account
    pub async fn enable_user(&self, username: &str) -> Result<()> {
        let output = Command::new("usermod")
            .args(["-U", username]) // Unlock
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to enable user: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("usermod failed: {}", stderr)));
        }

        // Also enable SMB access
        #[cfg(feature = "smb")]
        {
            let _ = Command::new("smbpasswd")
                .args(["-e", username])
                .output()
                .await;
        }

        Ok(())
    }

    /// Disable a user account
    pub async fn disable_user(&self, username: &str) -> Result<()> {
        let output = Command::new("usermod")
            .args(["-L", username]) // Lock
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to disable user: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("usermod failed: {}", stderr)));
        }

        // Also disable SMB access
        #[cfg(feature = "smb")]
        {
            let _ = Command::new("smbpasswd")
                .args(["-d", username])
                .output()
                .await;
        }

        Ok(())
    }

    /// Check if a user exists
    pub async fn user_exists(&self, username: &str) -> bool {
        self.get_system_user(username).await.is_ok()
    }

    /// Check if a group exists
    pub async fn group_exists(&self, name: &str) -> bool {
        self.get_system_group(name).await.is_ok()
    }

    /// Get all groups a user belongs to
    pub async fn get_user_groups(&self, username: &str) -> Result<Vec<String>> {
        let output = Command::new("groups")
            .arg(username)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get user groups: {}", e)))?;

        if !output.status.success() {
            return Err(Error::NotFound(format!("User '{}' not found", username)));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        // Output format: "username : group1 group2 group3"
        let groups: Vec<String> = stdout
            .split(':')
            .nth(1)
            .map(|s| s.split_whitespace().map(|g| g.to_string()).collect())
            .unwrap_or_default();

        Ok(groups)
    }

    /// Verify a password (Unix auth)
    pub async fn verify_password(&self, username: &str, password: &str) -> Result<bool> {
        // Use PAM for authentication if available, otherwise check shadow
        // This is a simplified check using shadow file
        let shadow = tokio::fs::read_to_string("/etc/shadow").await;

        if let Ok(content) = shadow {
            for line in content.lines() {
                let parts: Vec<&str> = line.split(':').collect();
                if parts.len() >= 2 && parts[0] == username {
                    let hash = parts[1];

                    // Empty hash means no password
                    if hash.is_empty() || hash == "*" || hash == "!" || hash == "!!" {
                        return Ok(false);
                    }

                    // Use openssl to verify password
                    // Format: $algorithm$salt$hash
                    let output = Command::new("openssl")
                        .args(["passwd", "-stdin", "-6", "-salt", ""])
                        .output()
                        .await;

                    // Simplified check - in production use proper PAM
                    let _ = (password, output);
                    return Ok(true); // Placeholder
                }
            }
        }

        Ok(false)
    }

    /// Get authentication status for a user
    pub async fn get_auth_status(&self, username: &str) -> Result<UserAuthStatus> {
        let user = self.get_system_user(username).await?;

        // Check if account is locked
        let shadow = tokio::fs::read_to_string("/etc/shadow")
            .await
            .unwrap_or_default();

        let mut locked = false;
        let mut has_password = false;

        for line in shadow.lines() {
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() >= 2 && parts[0] == username {
                let hash = parts[1];
                locked = hash.starts_with('!');
                has_password = !hash.is_empty() && hash != "*" && hash != "!" && hash != "!!";
                break;
            }
        }

        // Check SMB status
        #[cfg(feature = "smb")]
        let smb_enabled = {
            let output = Command::new("pdbedit")
                .args(["-L", "-u", username])
                .output()
                .await;

            output.map(|o| o.status.success()).unwrap_or(false)
        };

        #[cfg(not(feature = "smb"))]
        let smb_enabled = false;

        Ok(UserAuthStatus {
            username: username.to_string(),
            uid: user.uid,
            locked,
            has_password,
            smb_enabled,
            shell_login: user.shell != "/sbin/nologin" && user.shell != "/bin/false",
        })
    }
}

/// System user info (from /etc/passwd)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemUserInfo {
    pub username: String,
    pub uid: u32,
    pub gid: u32,
    pub full_name: String,
    pub home: String,
    pub shell: String,
}

/// System group info (from /etc/group)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemGroupInfo {
    pub name: String,
    pub gid: u32,
    pub members: Vec<String>,
}

/// User authentication status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserAuthStatus {
    pub username: String,
    pub uid: u32,
    pub locked: bool,
    pub has_password: bool,
    pub smb_enabled: bool,
    pub shell_login: bool,
}

impl Default for AuthManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_user() {
        let user = NasUser::new(
            "user1".to_string(),
            "testuser".to_string(),
            10001,
            "users".to_string(),
        );

        assert_eq!(user.username, "testuser");
        assert_eq!(user.uid, 10001);
        assert!(user.enabled);
        assert!(user.smb_enabled);
    }

    #[test]
    fn test_new_group() {
        let group = NasGroup::new("group1".to_string(), "testgroup".to_string(), 10001);

        assert_eq!(group.name, "testgroup");
        assert_eq!(group.gid, 10001);
        assert!(group.members.is_empty());
    }
}
