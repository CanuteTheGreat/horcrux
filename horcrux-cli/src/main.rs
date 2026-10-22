//! Horcrux CLI
//!
//! Command-line interface for Horcrux virtualization platform
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
    /// API server address (HTTPS; plain http:// is redirected)
    #[arg(
        short,
        long,
        env = "HORCRUX_SERVER",
        default_value = "https://localhost:8006"
    )]
    server: String,

    /// CA certificate (PEM) to verify the server with. Defaults to the
    /// local node's CA (/etc/horcrux/ssl/ca.pem) when that file is readable.
    #[arg(long, env = "HORCRUX_CA_CERT")]
    ca_cert: Option<std::path::PathBuf>,

    /// Don't verify the server's TLS certificate. HORCRUX_INSECURE accepts
    /// 1/0, true/false, yes/no, on/off.
    #[arg(
        long,
        env = "HORCRUX_INSECURE",
        action = clap::ArgAction::SetTrue,
        value_parser = clap::builder::BoolishValueParser::new()
    )]
    insecure: bool,

    /// Output format (table, json, yaml)
    #[arg(short, long, default_value = "table")]
    output: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Save state on host shutdown / resume on boot (Hyper-V style)
    Power {
        #[command(subcommand)]
        command: PowerCommands,
    },
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
    /// Open a console session for a VM (VNC by default, or serial)
    Console {
        /// VM ID to connect to
        vm_id: String,
        /// Use the serial console instead of VNC
        #[arg(long)]
        serial: bool,
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
pub enum PowerCommands {
    /// Node defaults, CRIU availability and every guest's saved state
    Status,
    /// Save a running guest's state to disk and stop it
    Save {
        /// vm or container
        kind: String,
        id: String,
    },
    /// Resume a guest from its saved state
    Resume { kind: String, id: String },
    /// Delete a guest's saved state (it then starts normally)
    Discard { kind: String, id: String },
    /// Set a guest's automatic stop/start action (overrides the node default)
    Policy {
        kind: String,
        id: String,
        /// save | shutdown | poweroff
        #[arg(long)]
        stop: Option<String>,
        /// resume_if_running | always | nothing
        #[arg(long)]
        start: Option<String>,
        /// Start delay in seconds
        #[arg(long)]
        delay: Option<u32>,
        /// Start order (lower first)
        #[arg(long)]
        order: Option<i32>,
        /// Remove all overrides (follow the node default)
        #[arg(long)]
        clear: bool,
    },
    /// Set the node default actions
    NodeDefault {
        #[arg(long)]
        stop: Option<String>,
        #[arg(long)]
        start: Option<String>,
        #[arg(long)]
        delay: Option<u32>,
        /// Directory for saved states ("" = default)
        #[arg(long)]
        dir: Option<String>,
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

/// Retention options shared by `backup prune` and `backup schedule`.
#[derive(clap::Args, Debug, Clone, Default)]
pub struct KeepArgs {
    /// Keep the N newest backups
    #[arg(long)]
    keep_last: Option<u32>,
    /// Keep the newest backup of the last N hours
    #[arg(long)]
    keep_hourly: Option<u32>,
    /// Keep the newest backup of the last N days
    #[arg(long)]
    keep_daily: Option<u32>,
    /// Keep the newest backup of the last N weeks
    #[arg(long)]
    keep_weekly: Option<u32>,
    /// Keep the newest backup of the last N months
    #[arg(long)]
    keep_monthly: Option<u32>,
    /// Keep the newest backup of the last N years
    #[arg(long)]
    keep_yearly: Option<u32>,
}

#[derive(Subcommand)]
enum BackupCommands {
    /// List backups
    List {
        /// Only backups of this VM/container
        #[arg(short, long)]
        target: Option<String>,
    },
    /// Show backup details
    Show { id: String },
    /// Back up a VM or container now
    Create {
        /// VM or container ID
        target_id: String,
        /// Backup mode (snapshot, suspend, stop)
        #[arg(short, long, default_value = "snapshot")]
        mode: String,
        /// Compression type (none, lzo, gzip, zstd)
        #[arg(short, long, default_value = "zstd")]
        compression: String,
        /// Storage: "local", a storage pool id, or an absolute directory
        #[arg(short, long, default_value = "local")]
        storage: String,
        /// Notes
        #[arg(short, long)]
        notes: Option<String>,
        /// Protect the backup from pruning/deletion
        #[arg(long)]
        protected: bool,
    },
    /// Restore a backup (to the original guest, or a new id)
    Restore {
        /// Backup ID
        id: String,
        /// Target VM/container ID (default: the original)
        #[arg(short, long)]
        target: Option<String>,
        /// Name of the restored guest
        #[arg(short, long)]
        name: Option<String>,
        /// Overwrite a different existing guest
        #[arg(long)]
        force: bool,
        /// Start the guest after restoring
        #[arg(long)]
        start: bool,
    },
    /// Verify a backup archive (checksums, unpack, qemu-img check)
    Verify { id: String },
    /// Delete a backup
    Delete { id: String },
    /// Apply a retention policy to one guest's backups
    Prune {
        /// VM or container ID
        target_id: String,
        #[command(flatten)]
        keep: KeepArgs,
        /// Only this storage
        #[arg(long)]
        storage: Option<String>,
        /// Only show what would be removed
        #[arg(long)]
        dry_run: bool,
    },
    /// Create a scheduled backup job
    Schedule {
        /// Job ID
        #[arg(short, long)]
        name: String,
        /// Cron schedule ("0 2 * * *") or daily/weekly/monthly
        #[arg(short, long)]
        schedule: String,
        /// VMs/containers to back up (comma-separated)
        #[arg(short, long)]
        vms: String,
        /// Storage: "local", a storage pool id, or an absolute directory
        #[arg(long, default_value = "local")]
        storage: String,
        /// Backup mode (snapshot, suspend, stop)
        #[arg(short, long, default_value = "snapshot")]
        mode: String,
        /// Compression type (none, lzo, gzip, zstd)
        #[arg(short, long, default_value = "zstd")]
        compression: String,
        #[command(flatten)]
        keep: KeepArgs,
    },
    /// List scheduled backup jobs
    Jobs,
    /// Run a backup job now
    RunJob { id: String },
    /// Delete a backup job (its backups are kept)
    DeleteJob { id: String },
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
    /// Set a node's placement/HA weight (0-1000, default 100). Higher is
    /// preferred for new VM placement and tried first for HA failover;
    /// give older/slower nodes in a mixed-age cluster a lower weight.
    SetPriority {
        /// Node name
        name: String,
        /// New placement weight (0-1000)
        priority: u32,
    },
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
        /// Role granted on / (e.g. Administrator, Auditor, VMAdmin);
        /// "user" grants nothing (use `grant` for narrower paths)
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
    /// Grant a role on a path (Proxmox-style ACL)
    Grant {
        /// Username
        username: String,
        /// Role, optionally with the path it applies to: "VMUser@/vms/100"
        /// (default path "/"). See `horcrux user roles`.
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
    /// Create a new container (`pct create` for --runtime lxc)
    Create {
        /// Container name
        #[arg(short, long)]
        name: String,
        /// Container runtime (lxc, docker, podman, lxd, incus)
        #[arg(short, long, default_value = "docker")]
        runtime: String,
        /// Image (docker/podman) or LXC template: distro/release[/variant]
        /// (e.g. alpine/3.22, debian/bookworm) or the path of a rootfs tarball in
        /// the server's image directory (HORCRUX_IMAGE_DIR)
        #[arg(short, long)]
        image: String,
        /// Memory in MB
        #[arg(short, long, default_value_t = 512)]
        memory: u64,
        /// Number of CPUs
        #[arg(short, long, default_value_t = 1)]
        cpus: u32,
        #[command(flatten)]
        lxc: LxcCreateArgs,
    },
    /// Start a container
    Start { id: String },
    /// Stop a container (LXC: immediate, like `pct stop`)
    Stop { id: String },
    /// Clean shutdown of an LXC container (`pct shutdown`)
    Shutdown {
        id: String,
        /// Seconds to wait for a clean power-off
        #[arg(long, default_value_t = 60)]
        timeout: u32,
        /// Kill the container if it is still running after the timeout
        #[arg(long)]
        force: bool,
    },
    /// Reboot an LXC container, applying pending settings (`pct reboot`)
    Reboot { id: String },
    /// Delete a container
    Delete { id: String },
    /// Execute command in container
    Exec {
        /// Container ID
        id: String,
        /// Command to execute
        command: Vec<String>,
    },
    /// Show an LXC container's settings and generated config (`pct config`)
    Config { id: String },
    /// Change an LXC container's settings (`pct set`)
    Set {
        id: String,
        #[arg(long)]
        memory: Option<u64>,
        #[arg(long)]
        cpus: Option<u32>,
        #[arg(long)]
        swap: Option<u64>,
        /// CPU cap in cores (e.g. 0.5)
        #[arg(long)]
        cpu_limit: Option<f64>,
        #[arg(long)]
        hostname: Option<String>,
        /// Replace the NICs: repeat --net name=eth0,bridge=vmbr0,tag=20,ip=dhcp
        #[arg(long = "net")]
        nets: Vec<String>,
        /// Replace the mount points: repeat --mp mp=/srv,source=/host/dir[,ro=1]
        #[arg(long = "mp")]
        mounts: Vec<String>,
        /// nesting=1,keyctl=1,fuse=1
        #[arg(long)]
        features: Option<String>,
        #[arg(long)]
        onboot: Option<bool>,
    },
    /// Grow an LXC container's root disk (`pct resize`)
    Resize {
        id: String,
        /// New size in GB
        disk_gb: u64,
    },
    /// Take a snapshot of an LXC container (`pct snapshot`)
    Snapshot {
        id: String,
        name: String,
        #[arg(long)]
        comment: Option<String>,
    },
    /// List an LXC container's snapshots (`pct listsnapshot`)
    Snapshots { id: String },
    /// Roll back to a snapshot (`pct rollback`)
    Rollback { id: String, name: String },
    /// Delete a snapshot (`pct delsnapshot`)
    DelSnapshot { id: String, name: String },
    /// Full clone of a container (`pct clone --full`)
    Clone { id: String, target: String },
    /// LXC templates (`pveam`)
    Template {
        #[command(subcommand)]
        command: TemplateCommands,
    },
}

/// LXC options for `container create --runtime lxc`.
#[derive(clap::Args, Debug, Default)]
pub struct LxcCreateArgs {
    /// Run privileged (no user namespace); default is unprivileged
    #[arg(long)]
    pub privileged: bool,
    /// Root disk size in GB (ext4 image; default: directory, no limit)
    #[arg(long)]
    pub disk: Option<u64>,
    /// Swap in MB
    #[arg(long, default_value_t = 0)]
    pub swap: u64,
    /// CPU cap in cores (e.g. 0.5)
    #[arg(long)]
    pub cpu_limit: Option<f64>,
    #[arg(long)]
    pub hostname: Option<String>,
    /// NIC, repeatable: name=eth0,bridge=vmbr0,tag=20,ip=dhcp|10.0.0.5/24,gw=10.0.0.1
    #[arg(long = "net")]
    pub nets: Vec<String>,
    /// Mount point, repeatable: mp=/srv,source=/host/dir[,ro=1] or mp=/data
    #[arg(long = "mp")]
    pub mounts: Vec<String>,
    /// nesting=1,keyctl=1,fuse=1
    #[arg(long)]
    pub features: Option<String>,
    /// Start when the host boots
    #[arg(long)]
    pub onboot: bool,
    /// DNS server(s)
    #[arg(long)]
    pub nameserver: Option<String>,
    /// Root SSH public key, repeatable
    #[arg(long = "ssh-key")]
    pub ssh_keys: Vec<String>,
}

#[derive(Subcommand)]
enum TemplateCommands {
    /// Downloaded templates (`pveam list`)
    List,
    /// Templates on images.linuxcontainers.org (`pveam available`)
    Available {
        #[arg(long)]
        distro: Option<String>,
        /// amd64, arm64, ... or "all" (default: this host's)
        #[arg(long)]
        arch: Option<String>,
    },
    /// Download a template: distro/release[/variant] (`pveam download`)
    Download { template: String },
    /// Delete a downloaded template (distro/release/arch/variant)
    Delete { id: String },
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
        /// Full clone: an independent copy of the disk (the default)
        #[arg(short, long, conflicts_with = "linked")]
        full: bool,
        /// Linked clone: a qcow2 overlay on the source VM's disk (the
        /// source can't be deleted while the clone exists)
        #[arg(long)]
        linked: bool,
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
    let ca_cert = cli.ca_cert.clone().or_else(|| {
        let local = std::path::PathBuf::from("/etc/horcrux/ssl/ca.pem");
        std::fs::File::open(&local).is_ok().then_some(local)
    });
    let api_client = api::ApiClient::with_tls(&cli.server, ca_cert.as_deref(), cli.insecure)?;

    // Load token from config if available
    if let Some(token) = &config.token {
        api_client.set_token(token.clone()).await;
    }

    // Execute command
    match cli.command {
        Commands::Power { command } => {
            commands::power::handle_power_command(command, &api_client, &cli.output).await?
        }
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
        Commands::Console { vm_id, serial } => {
            commands::console::handle_console_command(&vm_id, serial, &api_client).await?
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
            commands::replication::handle_replication_command(command, &api_client, &cli.output)
                .await?
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

#[cfg(test)]
mod cli_tests {
    use super::*;

    /// `HORCRUX_INSECURE=1` used to be refused ("invalid value '1' for
    /// '--insecure' [possible values: true, false]"). Env access is
    /// process-wide, so every case runs in this one test.
    #[test]
    fn insecure_flag_and_env_accept_boolish_values() {
        let parse = || Cli::try_parse_from(["horcrux", "vm", "list"]).map(|c| c.insecure);
        std::env::remove_var("HORCRUX_INSECURE");
        assert!(!parse().unwrap());
        for (v, want) in [
            ("1", true),
            ("yes", true),
            ("true", true),
            ("on", true),
            ("0", false),
            ("no", false),
            ("false", false),
            ("off", false),
        ] {
            std::env::set_var("HORCRUX_INSECURE", v);
            assert_eq!(parse().unwrap(), want, "HORCRUX_INSECURE={v}");
        }
        std::env::remove_var("HORCRUX_INSECURE");
        assert!(
            Cli::try_parse_from(["horcrux", "--insecure", "vm", "list"])
                .unwrap()
                .insecure
        );
    }
}
