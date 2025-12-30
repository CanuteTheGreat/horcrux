//! ACL management module
//!
//! Manages POSIX and NFSv4 ACLs on shares.

use horcrux_common::{Error, Result};
use crate::nas::shares::{AclEntry, AclFlags, AclPermissions, AclType};
use tokio::process::Command;

/// Get POSIX ACLs for a path
pub async fn get_posix_acl(path: &str) -> Result<Vec<AclEntry>> {
    let output = Command::new("getfacl")
        .args(["--omit-header", "--absolute-names", path])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("getfacl failed: {}", e)))?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut entries = Vec::new();

    for line in stdout.lines() {
        if let Some(entry) = parse_posix_acl_line(line) {
            entries.push(entry);
        }
    }

    Ok(entries)
}

/// Parse a POSIX ACL line
fn parse_posix_acl_line(line: &str) -> Option<AclEntry> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }

    let parts: Vec<&str> = line.split(':').collect();
    if parts.len() < 3 {
        return None;
    }

    let (entry_type, principal) = match parts[0] {
        "user" => (AclType::Allow, format!("user:{}", parts[1])),
        "group" => (AclType::Allow, format!("group:{}", parts[1])),
        "other" => (AclType::Allow, "other".to_string()),
        "mask" => (AclType::Allow, "mask".to_string()),
        _ => return None,
    };

    let perms_str = parts.last()?;
    let permissions = AclPermissions {
        read: perms_str.contains('r'),
        write: perms_str.contains('w'),
        execute: perms_str.contains('x'),
        ..Default::default()
    };

    Some(AclEntry {
        entry_type,
        principal,
        permissions,
        flags: AclFlags::default(),
    })
}

/// Set POSIX ACL on a path
pub async fn set_posix_acl(path: &str, entries: &[AclEntry]) -> Result<()> {
    for entry in entries {
        let acl_spec = format_posix_acl_spec(entry);
        if acl_spec.is_empty() {
            continue;
        }

        let output = Command::new("setfacl")
            .args(["-m", &acl_spec, path])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("setfacl failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "setfacl failed: {}",
                stderr
            )));
        }
    }

    Ok(())
}

/// Format ACL entry as POSIX ACL specification
fn format_posix_acl_spec(entry: &AclEntry) -> String {
    let mut perms = String::new();
    if entry.permissions.read {
        perms.push('r');
    } else {
        perms.push('-');
    }
    if entry.permissions.write {
        perms.push('w');
    } else {
        perms.push('-');
    }
    if entry.permissions.execute {
        perms.push('x');
    } else {
        perms.push('-');
    }

    let parts: Vec<&str> = entry.principal.split(':').collect();
    if parts.len() == 2 {
        format!("{}:{}:{}", parts[0], parts[1], perms)
    } else {
        format!("{}::{}", entry.principal, perms)
    }
}

/// Remove POSIX ACL from path
pub async fn remove_posix_acl(path: &str) -> Result<()> {
    let output = Command::new("setfacl")
        .args(["-b", path])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("setfacl failed: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Internal(format!(
            "setfacl failed: {}",
            stderr
        )));
    }

    Ok(())
}

/// Set default POSIX ACL (for new files/directories)
pub async fn set_default_acl(path: &str, entries: &[AclEntry]) -> Result<()> {
    for entry in entries {
        let acl_spec = format!("d:{}", format_posix_acl_spec(entry));

        let output = Command::new("setfacl")
            .args(["-m", &acl_spec, path])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("setfacl failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "setfacl failed: {}",
                stderr
            )));
        }
    }

    Ok(())
}

/// Get NFSv4 ACLs (ZFS)
#[cfg(feature = "nas-zfs")]
pub async fn get_nfsv4_acl(path: &str) -> Result<Vec<AclEntry>> {
    let output = Command::new("nfs4_getfacl")
        .arg(path)
        .output()
        .await
        .map_err(|e| Error::Internal(format!("nfs4_getfacl failed: {}", e)))?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    // Parse NFSv4 ACL output
    // Format: A::OWNER@:rwatTnNcCy (type:flags:principal:permissions)
    let stdout = String::from_utf8_lossy(&output.stdout);
    let entries = parse_nfsv4_acl(&stdout);

    Ok(entries)
}

/// Parse NFSv4 ACL output
fn parse_nfsv4_acl(output: &str) -> Vec<AclEntry> {
    let mut entries = Vec::new();

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() < 4 {
            continue;
        }

        let entry_type = match parts[0] {
            "A" => AclType::Allow,
            "D" => AclType::Deny,
            "U" => AclType::Audit,
            "L" => AclType::Alarm,
            _ => continue,
        };

        let principal = parts[2].to_string();
        let perms_str = parts[3];

        let permissions = AclPermissions {
            read: perms_str.contains('r'),
            write: perms_str.contains('w'),
            execute: perms_str.contains('x'),
            append: perms_str.contains('a'),
            delete: perms_str.contains('d'),
            delete_child: perms_str.contains('D'),
            read_attributes: perms_str.contains('t'),
            write_attributes: perms_str.contains('T'),
            read_acl: perms_str.contains('c'),
            write_acl: perms_str.contains('C'),
            take_ownership: perms_str.contains('o'),
        };

        let flags_str = parts[1];
        let flags = AclFlags {
            file_inherit: flags_str.contains('f'),
            directory_inherit: flags_str.contains('d'),
            no_propagate_inherit: flags_str.contains('n'),
            inherit_only: flags_str.contains('i'),
        };

        entries.push(AclEntry {
            entry_type,
            principal,
            permissions,
            flags,
        });
    }

    entries
}

/// Set NFSv4 ACL on a path (ZFS)
#[cfg(feature = "nas-zfs")]
pub async fn set_nfsv4_acl(path: &str, entries: &[AclEntry]) -> Result<()> {
    for entry in entries {
        let acl_spec = format_nfsv4_acl_spec(entry);

        let output = Command::new("nfs4_setfacl")
            .args(["-a", &acl_spec, path])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("nfs4_setfacl failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "nfs4_setfacl failed: {}",
                stderr
            )));
        }
    }

    Ok(())
}

/// Format ACL entry as NFSv4 ACL specification
#[cfg(feature = "nas-zfs")]
fn format_nfsv4_acl_spec(entry: &AclEntry) -> String {
    let type_char = match entry.entry_type {
        AclType::Allow => 'A',
        AclType::Deny => 'D',
        AclType::Audit => 'U',
        AclType::Alarm => 'L',
    };

    let mut flags = String::new();
    if entry.flags.file_inherit { flags.push('f'); }
    if entry.flags.directory_inherit { flags.push('d'); }
    if entry.flags.no_propagate_inherit { flags.push('n'); }
    if entry.flags.inherit_only { flags.push('i'); }

    let mut perms = String::new();
    if entry.permissions.read { perms.push('r'); }
    if entry.permissions.write { perms.push('w'); }
    if entry.permissions.execute { perms.push('x'); }
    if entry.permissions.append { perms.push('a'); }
    if entry.permissions.delete { perms.push('d'); }
    if entry.permissions.delete_child { perms.push('D'); }
    if entry.permissions.read_attributes { perms.push('t'); }
    if entry.permissions.write_attributes { perms.push('T'); }
    if entry.permissions.read_acl { perms.push('c'); }
    if entry.permissions.write_acl { perms.push('C'); }
    if entry.permissions.take_ownership { perms.push('o'); }

    format!("{}:{}:{}:{}", type_char, flags, entry.principal, perms)
}

/// Remove all NFSv4 ACLs from a path
#[cfg(feature = "nas-zfs")]
pub async fn remove_nfsv4_acl(path: &str) -> Result<()> {
    let output = Command::new("nfs4_setfacl")
        .args(["-s", "A::OWNER@:rwatTnNcCoy", path])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("nfs4_setfacl failed: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Internal(format!(
            "nfs4_setfacl failed: {}",
            stderr
        )));
    }

    Ok(())
}

/// ACL Manager for managing file/directory permissions
pub struct AclManager;

impl AclManager {
    /// Create a new ACL manager
    pub fn new() -> Self {
        Self
    }

    /// Get ACLs for a path (auto-detects POSIX vs NFSv4)
    pub async fn get_acl(&self, path: &str) -> Result<Vec<AclEntry>> {
        // Try NFSv4 first (for ZFS)
        #[cfg(feature = "nas-zfs")]
        {
            if let Ok(entries) = get_nfsv4_acl(path).await {
                if !entries.is_empty() {
                    return Ok(entries);
                }
            }
        }

        // Fall back to POSIX ACLs
        get_posix_acl(path).await
    }

    /// Set ACLs on a path
    pub async fn set_acl(&self, path: &str, entries: &[AclEntry], use_nfsv4: bool) -> Result<()> {
        #[cfg(feature = "nas-zfs")]
        if use_nfsv4 {
            return set_nfsv4_acl(path, entries).await;
        }

        #[cfg(not(feature = "nas-zfs"))]
        let _ = use_nfsv4;

        set_posix_acl(path, entries).await
    }

    /// Remove all ACLs from a path
    pub async fn remove_acl(&self, path: &str, use_nfsv4: bool) -> Result<()> {
        #[cfg(feature = "nas-zfs")]
        if use_nfsv4 {
            return remove_nfsv4_acl(path).await;
        }

        #[cfg(not(feature = "nas-zfs"))]
        let _ = use_nfsv4;

        remove_posix_acl(path).await
    }

    /// Set default ACLs for inheritance (POSIX only)
    pub async fn set_default_acl(&self, path: &str, entries: &[AclEntry]) -> Result<()> {
        set_default_acl(path, entries).await
    }

    /// Apply ACLs recursively to a directory
    pub async fn apply_recursive(&self, path: &str, entries: &[AclEntry]) -> Result<()> {
        for entry in entries {
            let acl_spec = format_posix_acl_spec(entry);
            if acl_spec.is_empty() {
                continue;
            }

            let output = Command::new("setfacl")
                .args(["-R", "-m", &acl_spec, path])
                .output()
                .await
                .map_err(|e| Error::Internal(format!("setfacl failed: {}", e)))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(Error::Internal(format!(
                    "setfacl failed: {}",
                    stderr
                )));
            }
        }

        Ok(())
    }

    /// Copy ACLs from one path to another
    pub async fn copy_acl(&self, src: &str, dst: &str) -> Result<()> {
        let output = Command::new("getfacl")
            .args(["--omit-header", "--absolute-names", src])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("getfacl failed: {}", e)))?;

        if !output.status.success() {
            return Err(Error::Internal("Failed to get source ACL".to_string()));
        }

        let mut setfacl = Command::new("setfacl")
            .args(["--set-file=-", dst])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| Error::Internal(format!("setfacl spawn failed: {}", e)))?;

        if let Some(mut stdin) = setfacl.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin.write_all(&output.stdout).await
                .map_err(|e| Error::Internal(format!("Failed to write ACL: {}", e)))?;
        }

        let status = setfacl.wait().await
            .map_err(|e| Error::Internal(format!("setfacl failed: {}", e)))?;

        if !status.success() {
            return Err(Error::Internal("Failed to set destination ACL".to_string()));
        }

        Ok(())
    }

    /// Grant read access to a user
    pub async fn grant_read(&self, path: &str, user: &str) -> Result<()> {
        let entry = AclEntry {
            entry_type: AclType::Allow,
            principal: format!("user:{}", user),
            permissions: AclPermissions {
                read: true,
                execute: true,
                ..Default::default()
            },
            flags: AclFlags::default(),
        };
        set_posix_acl(path, &[entry]).await
    }

    /// Grant write access to a user
    pub async fn grant_write(&self, path: &str, user: &str) -> Result<()> {
        let entry = AclEntry {
            entry_type: AclType::Allow,
            principal: format!("user:{}", user),
            permissions: AclPermissions {
                read: true,
                write: true,
                execute: true,
                ..Default::default()
            },
            flags: AclFlags::default(),
        };
        set_posix_acl(path, &[entry]).await
    }

    /// Grant full access to a user
    pub async fn grant_full(&self, path: &str, user: &str) -> Result<()> {
        let entry = AclEntry {
            entry_type: AclType::Allow,
            principal: format!("user:{}", user),
            permissions: AclPermissions {
                read: true,
                write: true,
                execute: true,
                delete: true,
                ..Default::default()
            },
            flags: AclFlags::default(),
        };
        set_posix_acl(path, &[entry]).await
    }

    /// Revoke access for a user
    pub async fn revoke_user(&self, path: &str, user: &str) -> Result<()> {
        let output = Command::new("setfacl")
            .args(["-x", &format!("user:{}", user), path])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("setfacl failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("setfacl failed: {}", stderr)));
        }

        Ok(())
    }

    /// Grant access to a group
    pub async fn grant_group(&self, path: &str, group: &str, write: bool) -> Result<()> {
        let entry = AclEntry {
            entry_type: AclType::Allow,
            principal: format!("group:{}", group),
            permissions: AclPermissions {
                read: true,
                write,
                execute: true,
                ..Default::default()
            },
            flags: AclFlags::default(),
        };
        set_posix_acl(path, &[entry]).await
    }

    /// Revoke access for a group
    pub async fn revoke_group(&self, path: &str, group: &str) -> Result<()> {
        let output = Command::new("setfacl")
            .args(["-x", &format!("group:{}", group), path])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("setfacl failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("setfacl failed: {}", stderr)));
        }

        Ok(())
    }
}

impl Default for AclManager {
    fn default() -> Self {
        Self::new()
    }
}
