///! Horcrux CLI
///!
///! Command-line interface for Horcrux virtualization platform

mod api;
mod commands;
mod config;
mod output;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
struct Cli {
    /// API server address
    #[arg(short, long, default_value = "http://localhost:8006")]
    server: String,

    /// Output format (table, json, yaml)
    #[arg(short, long, default_value = "table")]
    output: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Manage virtual machines
    Vm {
        #[command(subcommand)]
        command: VmCommands,
    },
    /// Manage storage pools
    Storage {
        #[command(subcommand)]
        command: StorageCommands,
    },
    /// Manage backups
    Backup {
        #[command(subcommand)]
        command: BackupCommands,
    },
    /// Manage cluster nodes
    Cluster {
        #[command(subcommand)]
        command: ClusterCommands,
    },
    /// Manage users and authentication
    User {
        #[command(subcommand)]
        command: UserCommands,
    },
    /// Manage high availability
    Ha {
        #[command(subcommand)]
        command: HaCommands,
    },
    /// Live VM migration
    Migrate {
        /// VM ID to migrate
        vm_id: String,
        /// Target node name
        target_node: String,
        /// Migration type (live, offline, online)
        #[arg(short, long, default_value = "live")]
        migration_type: String,
    },
    /// Monitor system resources
    Monitor {
        #[command(subcommand)]
        command: MonitorCommands,
    },
    /// Audit log operations
    Audit {
        #[command(subcommand)]
        command: AuditCommands,
    },
    /// Authentication commands
    Auth {
        #[command(subcommand)]
        command: AuthCommands,
    },
    /// Manage containers
    Container {
        #[command(subcommand)]
        command: ContainerCommands,
    },
    /// Manage VM snapshots
    Snapshot {
        #[command(subcommand)]
        command: SnapshotCommands,
    },
    /// VM cloning operations
    Clone {
        #[command(subcommand)]
        command: CloneCommands,
    },
    /// Manage replication jobs
    Replication {
        #[command(subcommand)]
        command: ReplicationCommands,
    },
    /// NAS (Network Attached Storage) management
    Nas {
        #[command(subcommand)]
        command: NasCommands,
    },
    /// Generate shell completions
    Completions {
        /// Shell type
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

#[derive(Subcommand)]
enum VmCommands {
    /// List all VMs
    List,
    /// Show VM details
    Show { id: String },
    /// Create a new VM
    Create {
        /// VM name
        #[arg(short, long)]
        name: String,
        /// Memory in MB
        #[arg(short, long)]
        memory: u64,
        /// Number of CPUs
        #[arg(short, long)]
        cpus: u32,
        /// Disk size in GB
        #[arg(short, long)]
        disk: u64,
    },
    /// Start a VM
    Start { id: String },
    /// Stop a VM
    Stop { id: String },
    /// Restart a VM
    Restart { id: String },
    /// Delete a VM
    Delete { id: String },
    /// Clone a VM from template
    Clone {
        /// Template ID
        template_id: String,
        /// New VM name
        #[arg(short, long)]
        name: String,
    },
}

#[derive(Subcommand)]
enum StorageCommands {
    /// List storage pools
    List,
    /// Show storage pool details
    Show { id: String },
    /// Create a storage pool
    Create {
        /// Pool name
        #[arg(short, long)]
        name: String,
        /// Storage type (zfs, ceph, lvm, directory, etc.)
        #[arg(short = 't', long)]
        storage_type: String,
        /// Storage path
        #[arg(short, long)]
        path: String,
    },
    /// Delete a storage pool
    Delete { id: String },
    /// Create a volume in a pool
    CreateVolume {
        /// Pool ID
        pool_id: String,
        /// Volume name
        name: String,
        /// Size in GB
        size: u64,
    },
}

#[derive(Subcommand)]
enum BackupCommands {
    /// List backups
    List,
    /// Show backup details
    Show { id: String },
    /// Create a backup
    Create {
        /// VM ID
        vm_id: String,
        /// Backup mode (snapshot, suspend, stop)
        #[arg(short, long, default_value = "snapshot")]
        mode: String,
        /// Compression type (none, lzo, gzip, zstd)
        #[arg(short, long, default_value = "zstd")]
        compression: String,
    },
    /// Restore a backup
    Restore {
        /// Backup ID
        id: String,
        /// Target VM ID (optional, restores to original if not specified)
        #[arg(short, long)]
        target: Option<String>,
    },
    /// Delete a backup
    Delete { id: String },
    /// Schedule a backup job
    Schedule {
        /// Job name
        #[arg(short, long)]
        name: String,
        /// Cron schedule
        #[arg(short, long)]
        schedule: String,
        /// VMs to back up (comma-separated)
        #[arg(short, long)]
        vms: String,
    },
}

#[derive(Subcommand)]
enum ClusterCommands {
    /// List cluster nodes
    List,
    /// Show cluster status
    Status,
    /// Add a node to cluster
    Add {
        /// Node name
        name: String,
        /// Node address
        address: String,
    },
    /// Remove a node from cluster
    Remove { name: String },
    /// Show cluster architecture summary
    Architecture,
}

#[derive(Subcommand)]
enum UserCommands {
    /// List users
    List,
    /// Create a user
    Create {
        /// Username
        username: String,
        /// Password
        #[arg(short, long)]
        password: String,
        /// Role (admin, operator, user)
        #[arg(short, long, default_value = "user")]
        role: String,
    },
    /// Delete a user
    Delete { username: String },
    /// Change password
    Passwd {
        /// Username
        username: String,
    },
    /// List roles
    Roles,
    /// Grant permission
    Grant {
        /// Username
        username: String,
        /// Permission (e.g., "VM.Allocate")
        permission: String,
    },
}

#[derive(Subcommand)]
enum HaCommands {
    /// List HA resources
    List,
    /// Add VM to HA
    Add {
        /// VM ID
        vm_id: u32,
        /// HA group
        #[arg(short, long, default_value = "default")]
        group: String,
        /// Priority (0-1000)
        #[arg(short, long, default_value = "100")]
        priority: u32,
    },
    /// Remove VM from HA
    Remove { vm_id: u32 },
    /// Show HA status
    Status,
    /// Create HA group
    CreateGroup {
        /// Group name
        name: String,
        /// Allowed nodes (comma-separated)
        #[arg(short, long)]
        nodes: String,
    },
}

#[derive(Subcommand)]
enum MonitorCommands {
    /// Show node metrics
    Node,
    /// Show VM metrics
    Vm {
        /// VM ID (optional, shows all if not specified)
        id: Option<String>,
    },
    /// Show storage metrics
    Storage {
        /// Storage pool name (optional, shows all if not specified)
        name: Option<String>,
    },
    /// Show cluster metrics
    Cluster,
    /// Watch metrics in real-time
    Watch {
        /// Refresh interval in seconds
        #[arg(short, long, default_value = "2")]
        interval: u64,
    },
}

#[derive(Subcommand)]
enum AuditCommands {
    /// Query audit logs
    Query {
        /// Event type filter
        #[arg(short = 't', long)]
        event_type: Option<String>,
        /// User filter
        #[arg(short, long)]
        user: Option<String>,
        /// Severity filter (info, warning, error, critical)
        #[arg(short, long)]
        severity: Option<String>,
        /// Limit results
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },
    /// Show failed login attempts
    FailedLogins {
        /// User filter
        #[arg(short, long)]
        user: Option<String>,
        /// Limit results
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },
    /// Show security events
    Security {
        /// Limit results
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },
    /// Export audit logs
    Export {
        /// Output file path
        output: String,
    },
}

#[derive(Subcommand)]
enum ContainerCommands {
    /// List all containers
    List,
    /// Show container details
    Show { id: String },
    /// Create a new container
    Create {
        /// Container name
        #[arg(short, long)]
        name: String,
        /// Container runtime (lxc, docker, podman, lxd, incus)
        #[arg(short, long, default_value = "docker")]
        runtime: String,
        /// Container image
        #[arg(short, long)]
        image: String,
        /// Memory in MB
        #[arg(short, long)]
        memory: Option<u64>,
        /// Number of CPUs
        #[arg(short, long)]
        cpus: Option<u32>,
    },
    /// Start a container
    Start { id: String },
    /// Stop a container
    Stop { id: String },
    /// Delete a container
    Delete { id: String },
    /// Execute command in container
    Exec {
        /// Container ID
        id: String,
        /// Command to execute
        command: Vec<String>,
    },
}

#[derive(Subcommand)]
enum SnapshotCommands {
    /// List snapshots for a VM
    List {
        /// VM ID
        vm_id: String,
    },
    /// Show snapshot details
    Show {
        /// VM ID
        vm_id: String,
        /// Snapshot ID
        snapshot_id: String,
    },
    /// Create a snapshot
    Create {
        /// VM ID
        vm_id: String,
        /// Snapshot name
        #[arg(short, long)]
        name: String,
        /// Description
        #[arg(short, long)]
        description: Option<String>,
        /// Include memory (for running VMs)
        #[arg(short = 'm', long)]
        include_memory: bool,
    },
    /// Restore a snapshot
    Restore {
        /// VM ID
        vm_id: String,
        /// Snapshot ID
        snapshot_id: String,
    },
    /// Delete a snapshot
    Delete {
        /// VM ID
        vm_id: String,
        /// Snapshot ID
        snapshot_id: String,
    },
    /// Show snapshot tree
    Tree {
        /// VM ID
        vm_id: String,
    },
}

#[derive(Subcommand)]
enum CloneCommands {
    /// Clone a VM
    Create {
        /// Source VM ID
        vm_id: String,
        /// New VM name
        #[arg(short, long)]
        name: String,
        /// Full clone (vs linked clone)
        #[arg(short, long)]
        full: bool,
        /// Start after clone
        #[arg(short, long)]
        start: bool,
    },
    /// List clone jobs
    List,
    /// Show clone job status
    Status {
        /// Job ID
        job_id: String,
    },
    /// Cancel a clone job
    Cancel {
        /// Job ID
        job_id: String,
    },
}

#[derive(Subcommand)]
enum ReplicationCommands {
    /// List replication jobs
    List,
    /// Show replication job details
    Show { id: String },
    /// Create replication job
    Create {
        /// Source VM ID
        vm_id: String,
        /// Target node
        #[arg(short, long)]
        target_node: String,
        /// Schedule (hourly, daily, weekly, manual)
        #[arg(short, long, default_value = "daily")]
        schedule: String,
    },
    /// Execute replication job now
    Execute { id: String },
    /// Delete replication job
    Delete { id: String },
    /// Show replication status
    Status { id: String },
}

#[derive(Subcommand)]
pub enum NasCommands {
    // Health
    /// Show NAS health status
    Health,

    // Shares
    /// List NAS shares
    ShareList,
    /// Create a NAS share
    ShareCreate {
        /// Share name
        #[arg(short, long)]
        name: String,
        /// Filesystem path
        #[arg(short, long)]
        path: String,
        /// Enable SMB protocol
        #[arg(long)]
        smb: bool,
        /// Enable NFS protocol
        #[arg(long)]
        nfs: bool,
    },
    /// Delete a NAS share
    ShareDelete {
        /// Share ID
        id: String,
    },
    /// Enable a NAS share
    ShareEnable {
        /// Share ID
        id: String,
    },
    /// Disable a NAS share
    ShareDisable {
        /// Share ID
        id: String,
    },

    // Users
    /// List NAS users
    UserList,
    /// Create a NAS user
    UserCreate {
        /// Username
        #[arg(short, long)]
        username: String,
        /// Password
        #[arg(short, long)]
        password: String,
        /// Full name
        #[arg(short, long)]
        full_name: Option<String>,
    },
    /// Delete a NAS user
    UserDelete {
        /// User ID
        id: String,
    },
    /// Set NAS user password
    UserPassword {
        /// User ID
        id: String,
        /// New password
        #[arg(short, long)]
        password: String,
    },

    // Groups
    /// List NAS groups
    GroupList,
    /// Create a NAS group
    GroupCreate {
        /// Group name
        #[arg(short, long)]
        name: String,
        /// Description
        #[arg(short, long)]
        description: Option<String>,
    },
    /// Delete a NAS group
    GroupDelete {
        /// Group ID
        id: String,
    },

    // Storage Pools
    /// List NAS storage pools
    PoolList,
    /// Show NAS pool details
    PoolShow {
        /// Pool ID
        id: String,
    },
    /// Start pool scrub
    PoolScrub {
        /// Pool ID
        id: String,
    },

    // Snapshots
    /// List dataset snapshots
    SnapshotList {
        /// Dataset ID
        dataset_id: String,
    },
    /// Create a snapshot
    SnapshotCreate {
        /// Dataset ID
        dataset_id: String,
        /// Snapshot name
        #[arg(short, long)]
        name: String,
    },
    /// Delete a snapshot
    SnapshotDelete {
        /// Snapshot ID
        id: String,
    },
    /// Rollback to a snapshot
    SnapshotRollback {
        /// Snapshot ID
        id: String,
    },

    // Services
    /// List NAS services
    ServiceList,
    /// Start a NAS service
    ServiceStart {
        /// Service name (smbd, nfsd, netatalk, etc.)
        name: String,
    },
    /// Stop a NAS service
    ServiceStop {
        /// Service name
        name: String,
    },
    /// Restart a NAS service
    ServiceRestart {
        /// Service name
        name: String,
    },

    // SMB
    /// Show SMB connections
    SmbStatus,

    // NFS
    /// List NFS clients
    NfsClients,

    // iSCSI
    /// List iSCSI targets
    IscsiList,

    // S3
    /// Show S3 gateway status
    S3Status,
    /// List S3 buckets
    S3Buckets,

    // Directory Services - LDAP
    /// Show LDAP directory status
    LdapStatus,
    /// Configure LDAP directory server
    LdapConfigure {
        /// LDAP server URI (e.g., ldap://ldap.example.com)
        #[arg(short, long)]
        uri: String,
        /// Base DN (e.g., dc=example,dc=com)
        #[arg(short, long)]
        base_dn: String,
        /// Bind DN for authentication
        #[arg(long)]
        bind_dn: Option<String>,
        /// Bind password
        #[arg(long)]
        bind_password: Option<String>,
    },
    /// Sync users from LDAP
    LdapSync,
    /// Search LDAP users
    LdapSearchUsers {
        /// Search filter
        #[arg(short, long)]
        filter: String,
    },
    /// Search LDAP groups
    LdapSearchGroups {
        /// Search filter
        #[arg(short, long)]
        filter: String,
    },
    /// Test LDAP connection
    LdapTest,

    // Directory Services - Kerberos
    /// Show Kerberos status
    KerberosStatus,
    /// Configure Kerberos
    KerberosConfigure {
        /// Default realm
        #[arg(short, long)]
        realm: String,
        /// KDC server
        #[arg(short, long)]
        kdc: String,
        /// Admin server
        #[arg(long)]
        admin_server: Option<String>,
    },
    /// Obtain Kerberos ticket
    KerberosKinit {
        /// Principal name
        #[arg(short, long)]
        principal: String,
        /// Use keytab instead of password
        #[arg(long)]
        keytab: Option<String>,
    },
    /// List Kerberos tickets
    KerberosKlist,
    /// Destroy Kerberos tickets
    KerberosKdestroy,
    /// List keytab entries
    KerberosKeytabList {
        /// Keytab file path
        #[arg(short, long)]
        keytab: Option<String>,
    },
    /// Create keytab entry
    KerberosKeytabCreate {
        /// Principal name
        #[arg(short, long)]
        principal: String,
        /// Keytab file path
        #[arg(short, long)]
        keytab: Option<String>,
    },

    // Directory Services - Active Directory
    /// Show AD join status
    AdStatus,
    /// Join Active Directory domain
    AdJoin {
        /// AD domain (e.g., CORP.EXAMPLE.COM)
        #[arg(short, long)]
        domain: String,
        /// Admin username
        #[arg(short, long)]
        username: String,
        /// Computer OU path (optional)
        #[arg(long)]
        ou: Option<String>,
        /// Register DNS record
        #[arg(long)]
        register_dns: bool,
    },
    /// Leave Active Directory domain
    AdLeave {
        /// Admin username
        #[arg(short, long)]
        username: String,
    },
    /// List AD users
    AdUsers,
    /// List AD groups
    AdGroups,
    /// Show user's AD groups
    AdUserGroups {
        /// Username
        username: String,
    },
    /// Test AD trust relationship
    AdTestTrust,
    /// Ping domain controller
    AdPingDc,
    /// Verify AD join prerequisites
    AdVerifyPrereqs {
        /// AD domain
        #[arg(short, long)]
        domain: String,
    },

    // Scheduler Commands
    /// Show scheduler status
    SchedulerStatus,
    /// List scheduled jobs
    JobList,
    /// Show scheduled job details
    JobShow {
        /// Job ID
        id: String,
    },
    /// Create a scheduled job
    JobCreate {
        /// Job name
        #[arg(short, long)]
        name: String,
        /// Job type (snapshot, retention, replication, scrub, health_check, quota_check, smart_check)
        #[arg(short = 't', long)]
        job_type: String,
        /// Cron schedule expression
        #[arg(short, long)]
        schedule: String,
        /// Dataset path (for snapshot/retention jobs)
        #[arg(long)]
        dataset: Option<String>,
        /// Pool name (for scrub jobs)
        #[arg(long)]
        pool: Option<String>,
        /// Task ID (for replication jobs)
        #[arg(long)]
        task_id: Option<String>,
        /// Number of snapshots to keep (for retention jobs)
        #[arg(long)]
        keep_count: Option<usize>,
    },
    /// Delete a scheduled job
    JobDelete {
        /// Job ID
        id: String,
    },
    /// Run a scheduled job immediately
    JobRun {
        /// Job ID
        id: String,
    },
    /// Pause a scheduled job
    JobPause {
        /// Job ID
        id: String,
    },
    /// Resume a paused scheduled job
    JobResume {
        /// Job ID
        id: String,
    },
    /// Show job execution history
    JobHistory {
        /// Job ID
        id: String,
    },
}

use commands::auth::AuthCommands;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Load config
    let mut config = config::Config::load().unwrap_or_default();

    // Initialize API client
    let api_client = api::ApiClient::new(&cli.server);

    // Load token from config if available
    if let Some(token) = &config.token {
        api_client.set_token(token.clone()).await;
    }

    // Execute command
    match cli.command {
        Commands::Vm { command } => {
            commands::vm::handle_vm_command(command, &api_client, &cli.output).await?
        }
        Commands::Storage { command } => {
            commands::storage::handle_storage_command(command, &api_client, &cli.output).await?
        }
        Commands::Backup { command } => {
            commands::backup::handle_backup_command(command, &api_client, &cli.output).await?
        }
        Commands::Cluster { command } => {
            commands::cluster::handle_cluster_command(command, &api_client, &cli.output).await?
        }
        Commands::User { command } => {
            commands::user::handle_user_command(command, &api_client, &cli.output).await?
        }
        Commands::Ha { command } => {
            commands::ha::handle_ha_command(command, &api_client, &cli.output).await?
        }
        Commands::Migrate {
            vm_id,
            target_node,
            migration_type,
        } => {
            commands::migrate::handle_migrate_command(
                &vm_id,
                &target_node,
                &migration_type,
                &api_client,
            )
            .await?
        }
        Commands::Monitor { command } => {
            commands::monitor::handle_monitor_command(command, &api_client, &cli.output).await?
        }
        Commands::Audit { command } => {
            commands::audit::handle_audit_command(command, &api_client, &cli.output).await?
        }
        Commands::Auth { command } => {
            commands::auth::handle_auth_command(command, &api_client, &mut config).await?
        }
        Commands::Container { command } => {
            commands::container::handle_container_command(command, &api_client, &cli.output).await?
        }
        Commands::Snapshot { command } => {
            commands::snapshot::handle_snapshot_command(command, &api_client, &cli.output).await?
        }
        Commands::Clone { command } => {
            commands::clone::handle_clone_command(command, &api_client, &cli.output).await?
        }
        Commands::Replication { command } => {
            commands::replication::handle_replication_command(command, &api_client, &cli.output).await?
        }
        Commands::Nas { command } => {
            commands::nas::handle_nas_command(command, &api_client, &cli.output).await?
        }
        Commands::Completions { shell } => {
            generate_completions(shell);
        }
    }

    Ok(())
}

/// Generate shell completions
fn generate_completions(shell: clap_complete::Shell) {
    use clap::CommandFactory;
    use clap_complete::generate;
    use std::io;

    let mut cmd = Cli::command();
    let name = cmd.get_name().to_string();

    generate(shell, &mut cmd, name, &mut io::stdout());
}
