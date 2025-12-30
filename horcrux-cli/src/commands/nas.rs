//! NAS CLI commands
//!
//! Commands for managing NAS functionality including shares, users, groups,
//! storage pools, snapshots, services, and protocols.

use crate::api::ApiClient;
use crate::output;
use crate::NasCommands;
use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct NasHealth {
    status: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct NasShare {
    id: String,
    name: String,
    path: String,
    enabled: bool,
    smb_enabled: bool,
    nfs_enabled: bool,
    afp_enabled: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct NasUser {
    id: String,
    username: String,
    full_name: Option<String>,
    uid: u32,
    enabled: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct NasGroup {
    id: String,
    name: String,
    gid: u32,
}

#[derive(Debug, Serialize, Deserialize)]
struct NasPool {
    id: String,
    name: String,
    pool_type: String,
    health: String,
    total_bytes: u64,
    used_bytes: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct NasSnapshot {
    id: String,
    name: String,
    full_name: String,
    created_at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct NasService {
    name: String,
    running: bool,
    enabled: bool,
}

#[derive(Serialize)]
struct CreateShareRequest {
    name: String,
    path: String,
    smb_enabled: bool,
    nfs_enabled: bool,
}

#[derive(Serialize)]
struct CreateUserRequest {
    username: String,
    password: String,
    full_name: Option<String>,
}

#[derive(Serialize)]
struct CreateGroupRequest {
    name: String,
    description: Option<String>,
}

#[derive(Serialize)]
struct CreateSnapshotRequest {
    name: String,
}

pub async fn handle_nas_command(
    command: NasCommands,
    api: &ApiClient,
    output_format: &str,
) -> Result<()> {
    // Try directory commands first
    if handle_directory_command(&command, api, output_format).await? {
        return Ok(());
    }

    // Try scheduler commands
    if handle_scheduler_command(&command, api, output_format).await? {
        return Ok(());
    }

    match command {
        // Health
        NasCommands::Health => {
            let health: NasHealth = api.get("/api/nas/health").await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&health)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&health)?);
            } else {
                println!("NAS Health: {}", health.status);
            }
        }

        // Shares
        NasCommands::ShareList => {
            let shares: Vec<NasShare> = api.get("/api/nas/shares").await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&shares)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&shares)?);
            } else {
                println!(
                    "{:<36} {:<20} {:<30} {:<6} {:<5} {:<5} {:<5}",
                    "ID", "NAME", "PATH", "ACTIVE", "SMB", "NFS", "AFP"
                );
                println!("{}", "-".repeat(120));
                for share in shares {
                    println!(
                        "{:<36} {:<20} {:<30} {:<6} {:<5} {:<5} {:<5}",
                        share.id,
                        share.name,
                        share.path,
                        if share.enabled { "yes" } else { "no" },
                        if share.smb_enabled { "yes" } else { "-" },
                        if share.nfs_enabled { "yes" } else { "-" },
                        if share.afp_enabled { "yes" } else { "-" }
                    );
                }
            }
        }

        NasCommands::ShareCreate { name, path, smb, nfs } => {
            let request = CreateShareRequest {
                name: name.clone(),
                path,
                smb_enabled: smb,
                nfs_enabled: nfs,
            };
            let share: NasShare = api.post("/api/nas/shares", &request).await?;
            output::print_success(&format!("Share '{}' created (ID: {})", name, share.id));
        }

        NasCommands::ShareDelete { id } => {
            api.delete(&format!("/api/nas/shares/{}", id)).await?;
            output::print_success(&format!("Share {} deleted", id));
        }

        NasCommands::ShareEnable { id } => {
            api.post_empty(&format!("/api/nas/shares/{}/enable", id), &()).await?;
            output::print_success(&format!("Share {} enabled", id));
        }

        NasCommands::ShareDisable { id } => {
            api.post_empty(&format!("/api/nas/shares/{}/disable", id), &()).await?;
            output::print_success(&format!("Share {} disabled", id));
        }

        // Users
        NasCommands::UserList => {
            let users: Vec<NasUser> = api.get("/api/nas/users").await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&users)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&users)?);
            } else {
                println!(
                    "{:<36} {:<20} {:<30} {:<8} {:<8}",
                    "ID", "USERNAME", "FULL NAME", "UID", "ENABLED"
                );
                println!("{}", "-".repeat(100));
                for user in users {
                    println!(
                        "{:<36} {:<20} {:<30} {:<8} {:<8}",
                        user.id,
                        user.username,
                        user.full_name.unwrap_or_default(),
                        user.uid,
                        if user.enabled { "yes" } else { "no" }
                    );
                }
            }
        }

        NasCommands::UserCreate { username, password, full_name } => {
            let request = CreateUserRequest {
                username: username.clone(),
                password,
                full_name,
            };
            let user: NasUser = api.post("/api/nas/users", &request).await?;
            output::print_success(&format!("User '{}' created (ID: {})", username, user.id));
        }

        NasCommands::UserDelete { id } => {
            api.delete(&format!("/api/nas/users/{}", id)).await?;
            output::print_success(&format!("User {} deleted", id));
        }

        NasCommands::UserPassword { id, password } => {
            let body = serde_json::json!({ "password": password });
            api.post_empty(&format!("/api/nas/users/{}/password", id), &body).await?;
            output::print_success("Password updated");
        }

        // Groups
        NasCommands::GroupList => {
            let groups: Vec<NasGroup> = api.get("/api/nas/groups").await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&groups)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&groups)?);
            } else {
                println!("{:<36} {:<20} {:<8}", "ID", "NAME", "GID");
                println!("{}", "-".repeat(70));
                for group in groups {
                    println!("{:<36} {:<20} {:<8}", group.id, group.name, group.gid);
                }
            }
        }

        NasCommands::GroupCreate { name, description } => {
            let request = CreateGroupRequest {
                name: name.clone(),
                description,
            };
            let group: NasGroup = api.post("/api/nas/groups", &request).await?;
            output::print_success(&format!("Group '{}' created (ID: {})", name, group.id));
        }

        NasCommands::GroupDelete { id } => {
            api.delete(&format!("/api/nas/groups/{}", id)).await?;
            output::print_success(&format!("Group {} deleted", id));
        }

        // Storage Pools
        NasCommands::PoolList => {
            let pools: Vec<NasPool> = api.get("/api/nas/pools").await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&pools)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&pools)?);
            } else {
                println!(
                    "{:<36} {:<15} {:<10} {:<10} {:<12} {:<12}",
                    "ID", "NAME", "TYPE", "HEALTH", "USED", "TOTAL"
                );
                println!("{}", "-".repeat(100));
                for pool in pools {
                    let used = format_size(pool.used_bytes);
                    let total = format_size(pool.total_bytes);
                    println!(
                        "{:<36} {:<15} {:<10} {:<10} {:<12} {:<12}",
                        pool.id, pool.name, pool.pool_type, pool.health, used, total
                    );
                }
            }
        }

        NasCommands::PoolShow { id } => {
            let pool: NasPool = api.get(&format!("/api/nas/pools/{}", id)).await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&pool)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&pool)?);
            } else {
                println!("Pool: {}", pool.name);
                println!("  ID:     {}", pool.id);
                println!("  Type:   {}", pool.pool_type);
                println!("  Health: {}", pool.health);
                println!("  Used:   {}", format_size(pool.used_bytes));
                println!("  Total:  {}", format_size(pool.total_bytes));
            }
        }

        NasCommands::PoolScrub { id } => {
            api.post_empty(&format!("/api/nas/pools/{}/scrub", id), &()).await?;
            output::print_success(&format!("Scrub started for pool {}", id));
        }

        // Snapshots
        NasCommands::SnapshotList { dataset_id } => {
            let snapshots: Vec<NasSnapshot> =
                api.get(&format!("/api/nas/datasets/{}/snapshots", dataset_id)).await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&snapshots)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&snapshots)?);
            } else {
                println!("{:<36} {:<30} {:<50}", "ID", "NAME", "FULL NAME");
                println!("{}", "-".repeat(120));
                for snap in snapshots {
                    println!("{:<36} {:<30} {:<50}", snap.id, snap.name, snap.full_name);
                }
            }
        }

        NasCommands::SnapshotCreate { dataset_id, name } => {
            let request = CreateSnapshotRequest { name: name.clone() };
            let snap: NasSnapshot =
                api.post(&format!("/api/nas/datasets/{}/snapshots", dataset_id), &request).await?;
            output::print_success(&format!("Snapshot '{}' created (ID: {})", name, snap.id));
        }

        NasCommands::SnapshotDelete { id } => {
            api.delete(&format!("/api/nas/snapshots/{}", id)).await?;
            output::print_success(&format!("Snapshot {} deleted", id));
        }

        NasCommands::SnapshotRollback { id } => {
            api.post_empty(&format!("/api/nas/snapshots/{}/rollback", id), &()).await?;
            output::print_success(&format!("Rolled back to snapshot {}", id));
        }

        // Services
        NasCommands::ServiceList => {
            let services: Vec<NasService> = api.get("/api/nas/services").await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&services)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&services)?);
            } else {
                println!("{:<20} {:<10} {:<10}", "SERVICE", "RUNNING", "ENABLED");
                println!("{}", "-".repeat(45));
                for svc in services {
                    println!(
                        "{:<20} {:<10} {:<10}",
                        svc.name,
                        if svc.running { "yes" } else { "no" },
                        if svc.enabled { "yes" } else { "no" }
                    );
                }
            }
        }

        NasCommands::ServiceStart { name } => {
            api.post_empty(&format!("/api/nas/services/{}/start", name), &()).await?;
            output::print_success(&format!("Service {} started", name));
        }

        NasCommands::ServiceStop { name } => {
            api.post_empty(&format!("/api/nas/services/{}/stop", name), &()).await?;
            output::print_success(&format!("Service {} stopped", name));
        }

        NasCommands::ServiceRestart { name } => {
            api.post_empty(&format!("/api/nas/services/{}/restart", name), &()).await?;
            output::print_success(&format!("Service {} restarted", name));
        }

        // SMB
        NasCommands::SmbStatus => {
            let connections: Vec<serde_json::Value> = api.get("/api/nas/smb/connections").await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&connections)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&connections)?);
            } else {
                println!("SMB Connections: {}", connections.len());
                for conn in connections {
                    if let Some(user) = conn.get("user") {
                        if let Some(share) = conn.get("share") {
                            println!("  {} -> {}", user, share);
                        }
                    }
                }
            }
        }

        // NFS
        NasCommands::NfsClients => {
            let clients: Vec<serde_json::Value> = api.get("/api/nas/nfs/clients").await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&clients)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&clients)?);
            } else {
                println!("NFS Clients: {}", clients.len());
                for client in clients {
                    if let Some(ip) = client.get("ip") {
                        println!("  {}", ip);
                    }
                }
            }
        }

        // iSCSI
        NasCommands::IscsiList => {
            let targets: Vec<serde_json::Value> = api.get("/api/nas/iscsi/targets").await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&targets)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&targets)?);
            } else {
                println!("{:<36} {:<30} {:<50}", "ID", "NAME", "IQN");
                println!("{}", "-".repeat(120));
                for target in targets {
                    let id = target.get("id").and_then(|v| v.as_str()).unwrap_or("-");
                    let name = target.get("name").and_then(|v| v.as_str()).unwrap_or("-");
                    let iqn = target.get("iqn").and_then(|v| v.as_str()).unwrap_or("-");
                    println!("{:<36} {:<30} {:<50}", id, name, iqn);
                }
            }
        }

        // S3
        NasCommands::S3Status => {
            let status: serde_json::Value = api.get("/api/nas/s3/status").await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&status)?);
            } else {
                let running = status.get("running").and_then(|v| v.as_bool()).unwrap_or(false);
                println!("S3 Gateway: {}", if running { "running" } else { "stopped" });
            }
        }

        NasCommands::S3Buckets => {
            let buckets: Vec<serde_json::Value> = api.get("/api/nas/s3/buckets").await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&buckets)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&buckets)?);
            } else {
                println!("{:<36} {:<30}", "ID", "NAME");
                println!("{}", "-".repeat(70));
                for bucket in buckets {
                    let id = bucket.get("id").and_then(|v| v.as_str()).unwrap_or("-");
                    let name = bucket.get("name").and_then(|v| v.as_str()).unwrap_or("-");
                    println!("{:<36} {:<30}", id, name);
                }
            }
        }

        // Directory commands are handled by handle_directory_command above
        _ => {
            // Already handled by handle_directory_command
        }
    }

    Ok(())
}

fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.1} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

// Directory Service Types

#[derive(Debug, Serialize, Deserialize)]
struct LdapStatusInfo {
    configured: bool,
    connected: bool,
    uri: Option<String>,
    base_dn: Option<String>,
    user_count: u32,
    group_count: u32,
}

#[derive(Debug, Serialize, Deserialize)]
struct LdapUser {
    dn: String,
    uid: String,
    cn: String,
    uid_number: Option<u32>,
    gid_number: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize)]
struct LdapGroup {
    dn: String,
    cn: String,
    gid_number: Option<u32>,
    member_count: u32,
}

#[derive(Debug, Serialize, Deserialize)]
struct KerberosStatusInfo {
    configured: bool,
    default_realm: String,
    keytab_exists: bool,
    active_tickets: u32,
    realms: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct KerberosTicket {
    principal: String,
    expires: String,
    flags: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AdStatusInfo {
    joined: bool,
    domain: Option<String>,
    domain_controller: Option<String>,
    winbind_running: bool,
    kerberos_realm: String,
    idmap_backend: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct AdUser {
    username: String,
    uid: u32,
    gid: u32,
    full_name: Option<String>,
    home_directory: String,
    shell: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct AdGroup {
    name: String,
    gid: u32,
    members: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TrustStatus {
    secret_valid: bool,
    dc_reachable: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct DcPingResult {
    success: bool,
    latency_ms: u64,
    dc_name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct JoinPrerequisites {
    dns_resolves: bool,
    dc_reachable: bool,
    ports_open: Vec<u16>,
    time_synced: bool,
    samba_installed: bool,
    winbind_installed: bool,
    krb5_installed: bool,
    errors: Vec<String>,
}

#[derive(Serialize)]
struct LdapConfigRequest {
    uri: String,
    base_dn: String,
    bind_dn: Option<String>,
    bind_password: Option<String>,
}

#[derive(Serialize)]
struct KerberosConfigRequest {
    realm: String,
    kdc: String,
    admin_server: Option<String>,
}

#[derive(Serialize)]
struct AdJoinRequest {
    domain: String,
    username: String,
    password: String,
    computer_ou: Option<String>,
    register_dns: bool,
}

#[derive(Serialize)]
struct AdLeaveRequest {
    username: String,
    password: String,
}

pub async fn handle_directory_command(
    command: &crate::NasCommands,
    api: &ApiClient,
    output_format: &str,
) -> Result<bool> {
    use crate::NasCommands::*;

    match command {
        // LDAP Commands
        LdapStatus => {
            let status: LdapStatusInfo = api.get("/api/nas/directory/ldap/status").await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&status)?);
            } else {
                println!("LDAP Directory Status");
                println!("  Configured: {}", if status.configured { "yes" } else { "no" });
                println!("  Connected:  {}", if status.connected { "yes" } else { "no" });
                if let Some(uri) = &status.uri {
                    println!("  Server:     {}", uri);
                }
                if let Some(base_dn) = &status.base_dn {
                    println!("  Base DN:    {}", base_dn);
                }
                println!("  Users:      {}", status.user_count);
                println!("  Groups:     {}", status.group_count);
            }
            Ok(true)
        }

        LdapConfigure { uri, base_dn, bind_dn, bind_password } => {
            let request = LdapConfigRequest {
                uri: uri.clone(),
                base_dn: base_dn.clone(),
                bind_dn: bind_dn.clone(),
                bind_password: bind_password.clone(),
            };
            api.post_empty("/api/nas/directory/ldap/configure", &request).await?;
            output::print_success("LDAP configured successfully");
            Ok(true)
        }

        LdapSync => {
            let result: serde_json::Value = api.post("/api/nas/directory/ldap/sync", &serde_json::json!({})).await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                let created = result.get("created").and_then(|v| v.as_u64()).unwrap_or(0);
                let updated = result.get("updated").and_then(|v| v.as_u64()).unwrap_or(0);
                let deleted = result.get("deleted").and_then(|v| v.as_u64()).unwrap_or(0);
                output::print_success(&format!(
                    "LDAP sync complete: {} created, {} updated, {} deleted",
                    created, updated, deleted
                ));
            }
            Ok(true)
        }

        LdapSearchUsers { filter } => {
            let users: Vec<LdapUser> = api.get(&format!(
                "/api/nas/directory/ldap/users?filter={}",
                urlencoding::encode(filter)
            )).await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&users)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&users)?);
            } else {
                println!("{:<20} {:<30} {:<10} {:<10}", "UID", "CN", "UIDNUM", "GIDNUM");
                println!("{}", "-".repeat(75));
                for user in users {
                    println!(
                        "{:<20} {:<30} {:<10} {:<10}",
                        user.uid,
                        user.cn,
                        user.uid_number.map(|n| n.to_string()).unwrap_or("-".to_string()),
                        user.gid_number.map(|n| n.to_string()).unwrap_or("-".to_string())
                    );
                }
            }
            Ok(true)
        }

        LdapSearchGroups { filter } => {
            let groups: Vec<LdapGroup> = api.get(&format!(
                "/api/nas/directory/ldap/groups?filter={}",
                urlencoding::encode(filter)
            )).await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&groups)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&groups)?);
            } else {
                println!("{:<30} {:<10} {:<10}", "CN", "GID", "MEMBERS");
                println!("{}", "-".repeat(55));
                for group in groups {
                    println!(
                        "{:<30} {:<10} {:<10}",
                        group.cn,
                        group.gid_number.map(|n| n.to_string()).unwrap_or("-".to_string()),
                        group.member_count
                    );
                }
            }
            Ok(true)
        }

        LdapTest => {
            let result: serde_json::Value = api.get("/api/nas/directory/ldap/test").await?;
            let success = result.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
            if success {
                output::print_success("LDAP connection test successful");
            } else {
                let error = result.get("error").and_then(|v| v.as_str()).unwrap_or("Unknown error");
                println!("LDAP connection test failed: {}", error);
            }
            Ok(true)
        }

        // Kerberos Commands
        KerberosStatus => {
            let status: KerberosStatusInfo = api.get("/api/nas/directory/kerberos/status").await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&status)?);
            } else {
                println!("Kerberos Status");
                println!("  Configured:     {}", if status.configured { "yes" } else { "no" });
                println!("  Default Realm:  {}", status.default_realm);
                println!("  Keytab Exists:  {}", if status.keytab_exists { "yes" } else { "no" });
                println!("  Active Tickets: {}", status.active_tickets);
                if !status.realms.is_empty() {
                    println!("  Realms:         {}", status.realms.join(", "));
                }
            }
            Ok(true)
        }

        KerberosConfigure { realm, kdc, admin_server } => {
            let request = KerberosConfigRequest {
                realm: realm.clone(),
                kdc: kdc.clone(),
                admin_server: admin_server.clone(),
            };
            api.post_empty("/api/nas/directory/kerberos/configure", &request).await?;
            output::print_success(&format!("Kerberos configured for realm {}", realm));
            Ok(true)
        }

        KerberosKinit { principal, keytab } => {
            let body = serde_json::json!({
                "principal": principal,
                "keytab": keytab
            });

            if keytab.is_some() {
                api.post_empty("/api/nas/directory/kerberos/kinit", &body).await?;
                output::print_success(&format!("Obtained ticket for {} using keytab", principal));
            } else {
                // Need password - prompt user
                print!("Password for {}: ", principal);
                use std::io::Write;
                std::io::stdout().flush()?;
                let password = rpassword::read_password()?;
                let body = serde_json::json!({
                    "principal": principal,
                    "password": password
                });
                api.post_empty("/api/nas/directory/kerberos/kinit", &body).await?;
                output::print_success(&format!("Obtained ticket for {}", principal));
            }
            Ok(true)
        }

        KerberosKlist => {
            let tickets: Vec<KerberosTicket> = api.get("/api/nas/directory/kerberos/tickets").await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&tickets)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&tickets)?);
            } else {
                if tickets.is_empty() {
                    println!("No active Kerberos tickets");
                } else {
                    println!("{:<50} {:<25} {:<20}", "PRINCIPAL", "EXPIRES", "FLAGS");
                    println!("{}", "-".repeat(100));
                    for ticket in tickets {
                        println!(
                            "{:<50} {:<25} {:<20}",
                            ticket.principal,
                            ticket.expires,
                            ticket.flags.join(",")
                        );
                    }
                }
            }
            Ok(true)
        }

        KerberosKdestroy => {
            api.post_empty("/api/nas/directory/kerberos/kdestroy", &()).await?;
            output::print_success("Kerberos tickets destroyed");
            Ok(true)
        }

        KerberosKeytabList { keytab } => {
            let url = match keytab {
                Some(kt) => format!("/api/nas/directory/kerberos/keytab?path={}", urlencoding::encode(kt)),
                None => "/api/nas/directory/kerberos/keytab".to_string(),
            };
            let entries: Vec<String> = api.get(&url).await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&entries)?);
            } else {
                println!("Keytab entries:");
                for entry in entries {
                    println!("  {}", entry);
                }
            }
            Ok(true)
        }

        KerberosKeytabCreate { principal, keytab } => {
            // Need password for keytab creation
            print!("Password for {}: ", principal);
            use std::io::Write;
            std::io::stdout().flush()?;
            let password = rpassword::read_password()?;

            let body = serde_json::json!({
                "principal": principal,
                "password": password,
                "keytab": keytab
            });
            api.post_empty("/api/nas/directory/kerberos/keytab", &body).await?;
            output::print_success(&format!("Keytab entry created for {}", principal));
            Ok(true)
        }

        // Active Directory Commands
        AdStatus => {
            let status: AdStatusInfo = api.get("/api/nas/directory/ad/status").await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&status)?);
            } else {
                println!("Active Directory Status");
                println!("  Joined:           {}", if status.joined { "yes" } else { "no" });
                if let Some(domain) = &status.domain {
                    println!("  Domain:           {}", domain);
                }
                if let Some(dc) = &status.domain_controller {
                    println!("  Domain Controller: {}", dc);
                }
                println!("  Winbind Running:  {}", if status.winbind_running { "yes" } else { "no" });
                println!("  Kerberos Realm:   {}", status.kerberos_realm);
                println!("  ID Map Backend:   {}", status.idmap_backend);
            }
            Ok(true)
        }

        AdJoin { domain, username, ou, register_dns } => {
            // Need password for domain join
            print!("Password for {}: ", username);
            use std::io::Write;
            std::io::stdout().flush()?;
            let password = rpassword::read_password()?;

            let request = AdJoinRequest {
                domain: domain.clone(),
                username: username.clone(),
                password,
                computer_ou: ou.clone(),
                register_dns: *register_dns,
            };
            api.post_empty("/api/nas/directory/ad/join", &request).await?;
            output::print_success(&format!("Successfully joined domain {}", domain));
            Ok(true)
        }

        AdLeave { username } => {
            // Need password for domain leave
            print!("Password for {}: ", username);
            use std::io::Write;
            std::io::stdout().flush()?;
            let password = rpassword::read_password()?;

            let request = AdLeaveRequest {
                username: username.clone(),
                password,
            };
            api.post_empty("/api/nas/directory/ad/leave", &request).await?;
            output::print_success("Successfully left Active Directory domain");
            Ok(true)
        }

        AdUsers => {
            let users: Vec<AdUser> = api.get("/api/nas/directory/ad/users").await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&users)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&users)?);
            } else {
                println!("{:<25} {:<8} {:<8} {:<30}", "USERNAME", "UID", "GID", "FULL NAME");
                println!("{}", "-".repeat(75));
                for user in users {
                    println!(
                        "{:<25} {:<8} {:<8} {:<30}",
                        user.username,
                        user.uid,
                        user.gid,
                        user.full_name.unwrap_or_default()
                    );
                }
            }
            Ok(true)
        }

        AdGroups => {
            let groups: Vec<AdGroup> = api.get("/api/nas/directory/ad/groups").await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&groups)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&groups)?);
            } else {
                println!("{:<30} {:<8} {:<10}", "NAME", "GID", "MEMBERS");
                println!("{}", "-".repeat(55));
                for group in groups {
                    println!("{:<30} {:<8} {:<10}", group.name, group.gid, group.members.len());
                }
            }
            Ok(true)
        }

        AdUserGroups { username } => {
            let groups: Vec<String> = api.get(&format!(
                "/api/nas/directory/ad/users/{}/groups",
                urlencoding::encode(username)
            )).await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&groups)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&groups)?);
            } else {
                println!("Groups for {}:", username);
                for group in groups {
                    println!("  {}", group);
                }
            }
            Ok(true)
        }

        AdTestTrust => {
            let status: TrustStatus = api.get("/api/nas/directory/ad/trust/test").await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&status)?);
            } else {
                println!("Trust Relationship Status");
                println!("  Machine Secret Valid: {}", if status.secret_valid { "yes" } else { "no" });
                println!("  DC Reachable:         {}", if status.dc_reachable { "yes" } else { "no" });
                if status.secret_valid && status.dc_reachable {
                    output::print_success("Trust relationship is healthy");
                } else {
                    println!("Warning: Trust relationship may have issues");
                }
            }
            Ok(true)
        }

        AdPingDc => {
            let result: DcPingResult = api.get("/api/nas/directory/ad/dc/ping").await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&result)?);
            } else {
                if result.success {
                    output::print_success(&format!(
                        "DC {} responded in {} ms",
                        result.dc_name.unwrap_or_else(|| "unknown".to_string()),
                        result.latency_ms
                    ));
                } else {
                    println!("Failed to ping domain controller");
                }
            }
            Ok(true)
        }

        AdVerifyPrereqs { domain } => {
            let prereqs: JoinPrerequisites = api.get(&format!(
                "/api/nas/directory/ad/prerequisites?domain={}",
                urlencoding::encode(domain)
            )).await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&prereqs)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&prereqs)?);
            } else {
                println!("AD Join Prerequisites for {}", domain);
                println!();
                print_prereq_status("DNS Resolution", prereqs.dns_resolves);
                print_prereq_status("DC Reachable", prereqs.dc_reachable);
                print_prereq_status("Time Synced", prereqs.time_synced);
                print_prereq_status("Samba Installed", prereqs.samba_installed);
                print_prereq_status("Winbind Installed", prereqs.winbind_installed);
                print_prereq_status("Kerberos Installed", prereqs.krb5_installed);

                if !prereqs.ports_open.is_empty() {
                    println!("  Ports Open:           {:?}", prereqs.ports_open);
                }

                if !prereqs.errors.is_empty() {
                    println!();
                    println!("Issues:");
                    for error in &prereqs.errors {
                        println!("  - {}", error);
                    }
                }

                println!();
                if prereqs.errors.is_empty() {
                    output::print_success("All prerequisites met - ready to join domain");
                } else {
                    println!("Warning: {} issue(s) found - review before joining", prereqs.errors.len());
                }
            }
            Ok(true)
        }

        // Not a directory command
        _ => Ok(false),
    }
}

fn print_prereq_status(name: &str, status: bool) {
    let icon = if status { "✓" } else { "✗" };
    println!("  {} {:<20} {}", icon, name, if status { "OK" } else { "FAILED" });
}

// Scheduler Types

#[derive(Debug, Serialize, Deserialize)]
struct NasSchedulerStatus {
    running: bool,
    total_jobs: i64,
    enabled_jobs: i64,
    running_jobs: i64,
    recent_failures_24h: i64,
    next_scheduled: Option<NasNextScheduledJob>,
}

#[derive(Debug, Serialize, Deserialize)]
struct NasNextScheduledJob {
    job_id: String,
    job_name: String,
    next_run: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct NasScheduledJob {
    id: String,
    name: String,
    job_type: String,
    schedule: String,
    enabled: bool,
    last_run: Option<i64>,
    next_run: Option<i64>,
    last_status: Option<String>,
    created_at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct NasJobHistoryEntry {
    id: String,
    started_at: i64,
    completed_at: Option<i64>,
    status: String,
    error_message: Option<String>,
    output: Option<String>,
}

#[derive(Serialize)]
struct NasCreateJobRequest {
    name: String,
    job_type: String,
    schedule: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    dataset: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    keep_count: Option<usize>,
}

pub async fn handle_scheduler_command(
    command: &crate::NasCommands,
    api: &ApiClient,
    output_format: &str,
) -> Result<bool> {
    use crate::NasCommands::*;

    match command {
        SchedulerStatus => {
            let status: NasSchedulerStatus = api.get("/api/nas/scheduler/status").await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&status)?);
            } else {
                println!("Scheduler Status");
                println!("  Running:          {}", if status.running { "yes" } else { "no" });
                println!("  Total Jobs:       {}", status.total_jobs);
                println!("  Enabled Jobs:     {}", status.enabled_jobs);
                println!("  Running Jobs:     {}", status.running_jobs);
                println!("  Failures (24h):   {}", status.recent_failures_24h);
                if let Some(next) = &status.next_scheduled {
                    println!("  Next Job:         {} ({})", next.job_name, format_timestamp(next.next_run));
                }
            }
            Ok(true)
        }

        JobList => {
            let jobs: Vec<NasScheduledJob> = api.get("/api/nas/scheduler/jobs").await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&jobs)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&jobs)?);
            } else {
                println!(
                    "{:<36} {:<20} {:<12} {:<15} {:<8} {:<15}",
                    "ID", "NAME", "TYPE", "SCHEDULE", "ENABLED", "LAST RUN"
                );
                println!("{}", "-".repeat(110));
                for job in jobs {
                    let job_type = parse_job_type(&job.job_type);
                    let last_run = job.last_run.map(format_timestamp).unwrap_or_else(|| "never".to_string());
                    println!(
                        "{:<36} {:<20} {:<12} {:<15} {:<8} {:<15}",
                        job.id,
                        truncate_str(&job.name, 18),
                        job_type,
                        truncate_str(&job.schedule, 13),
                        if job.enabled { "yes" } else { "no" },
                        last_run
                    );
                }
            }
            Ok(true)
        }

        JobShow { id } => {
            let job: NasScheduledJob = api.get(&format!("/api/nas/scheduler/jobs/{}", id)).await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&job)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&job)?);
            } else {
                println!("Job: {}", job.name);
                println!("  ID:         {}", job.id);
                println!("  Type:       {}", parse_job_type(&job.job_type));
                println!("  Schedule:   {}", job.schedule);
                println!("  Enabled:    {}", if job.enabled { "yes" } else { "no" });
                println!("  Last Run:   {}", job.last_run.map(format_timestamp).unwrap_or_else(|| "never".to_string()));
                println!("  Next Run:   {}", job.next_run.map(format_timestamp).unwrap_or_else(|| "-".to_string()));
                if let Some(status) = &job.last_status {
                    println!("  Last Status: {}", status);
                }
            }
            Ok(true)
        }

        JobCreate { name, job_type, schedule, dataset, pool, task_id, keep_count } => {
            let request = NasCreateJobRequest {
                name: name.clone(),
                job_type: job_type.clone(),
                schedule: schedule.clone(),
                dataset: dataset.clone(),
                pool: pool.clone(),
                task_id: task_id.clone(),
                keep_count: *keep_count,
            };
            let job: NasScheduledJob = api.post("/api/nas/scheduler/jobs", &request).await?;
            output::print_success(&format!("Job '{}' created (ID: {})", name, job.id));
            Ok(true)
        }

        JobDelete { id } => {
            api.delete(&format!("/api/nas/scheduler/jobs/{}", id)).await?;
            output::print_success(&format!("Job {} deleted", id));
            Ok(true)
        }

        JobRun { id } => {
            let result: serde_json::Value = api.post(&format!("/api/nas/scheduler/jobs/{}/run", id), &serde_json::json!({})).await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                let history_id = result.get("history_id").and_then(|v| v.as_str()).unwrap_or("unknown");
                output::print_success(&format!("Job {} started (execution ID: {})", id, history_id));
            }
            Ok(true)
        }

        JobPause { id } => {
            api.post_empty(&format!("/api/nas/scheduler/jobs/{}/pause", id), &()).await?;
            output::print_success(&format!("Job {} paused", id));
            Ok(true)
        }

        JobResume { id } => {
            api.post_empty(&format!("/api/nas/scheduler/jobs/{}/resume", id), &()).await?;
            output::print_success(&format!("Job {} resumed", id));
            Ok(true)
        }

        JobHistory { id } => {
            let history: Vec<NasJobHistoryEntry> = api.get(&format!("/api/nas/scheduler/jobs/{}/history", id)).await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&history)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&history)?);
            } else {
                println!(
                    "{:<36} {:<20} {:<20} {:<10} {:<30}",
                    "ID", "STARTED", "COMPLETED", "STATUS", "ERROR"
                );
                println!("{}", "-".repeat(120));
                for entry in history {
                    let started = format_timestamp(entry.started_at);
                    let completed = entry.completed_at.map(format_timestamp).unwrap_or_else(|| "-".to_string());
                    let error = entry.error_message.as_ref().map(|e| truncate_str(e, 28)).unwrap_or_else(|| "-".to_string());
                    println!(
                        "{:<36} {:<20} {:<20} {:<10} {:<30}",
                        entry.id,
                        started,
                        completed,
                        entry.status,
                        error
                    );
                }
            }
            Ok(true)
        }

        _ => Ok(false),
    }
}

fn format_timestamp(ts: i64) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let diff = now - ts;

    if diff < 0 {
        // Future time
        let abs_diff = -diff;
        if abs_diff < 60 {
            format!("in {}s", abs_diff)
        } else if abs_diff < 3600 {
            format!("in {}m", abs_diff / 60)
        } else if abs_diff < 86400 {
            format!("in {}h", abs_diff / 3600)
        } else {
            format!("in {}d", abs_diff / 86400)
        }
    } else if diff < 60 {
        format!("{}s ago", diff)
    } else if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86400 {
        format!("{}h ago", diff / 3600)
    } else {
        format!("{}d ago", diff / 86400)
    }
}

fn parse_job_type(job_type: &str) -> String {
    // Parse JSON job type if present
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(job_type) {
        if let Some(obj) = parsed.as_object() {
            if obj.contains_key("Snapshot") {
                return "snapshot".to_string();
            } else if obj.contains_key("RetentionCleanup") {
                return "retention".to_string();
            } else if obj.contains_key("Replication") {
                return "replication".to_string();
            } else if obj.contains_key("Scrub") {
                return "scrub".to_string();
            } else if obj.contains_key("CustomScript") {
                return "custom".to_string();
            } else if obj.contains_key("HealthCheck") {
                return "health".to_string();
            } else if obj.contains_key("QuotaCheck") {
                return "quota".to_string();
            } else if obj.contains_key("SmartCheck") {
                return "smart".to_string();
            }
        }
    }
    job_type.to_string()
}

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}
