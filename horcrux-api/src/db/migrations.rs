///! Database migrations

use horcrux_common::Result;
use sqlx::SqlitePool;

pub async fn run_migrations(pool: &SqlitePool) -> Result<()> {
    // Create migrations table
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS migrations (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            executed_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        )"
    )
    .execute(pool)
    .await
    .map_err(|e| horcrux_common::Error::System(format!("Failed to create migrations table: {}", e)))?;

    // Run migrations in order
    run_migration(pool, "001_create_vms_table", MIGRATION_001_CREATE_VMS).await?;
    run_migration(pool, "002_create_users_table", MIGRATION_002_CREATE_USERS).await?;
    run_migration(pool, "003_create_sessions_table", MIGRATION_003_CREATE_SESSIONS).await?;
    run_migration(pool, "004_create_audit_logs_table", MIGRATION_004_CREATE_AUDIT_LOGS).await?;
    run_migration(pool, "005_create_storage_pools_table", MIGRATION_005_CREATE_STORAGE_POOLS).await?;
    run_migration(pool, "006_create_backups_table", MIGRATION_006_CREATE_BACKUPS).await?;
    run_migration(pool, "007_create_cluster_nodes_table", MIGRATION_007_CREATE_CLUSTER_NODES).await?;
    run_migration(pool, "008_create_api_keys_table", MIGRATION_008_CREATE_API_KEYS).await?;
    run_migration(pool, "009_create_k8s_clusters_table", MIGRATION_009_CREATE_K8S_CLUSTERS).await?;
    run_migration(pool, "010_create_k8s_helm_repos_table", MIGRATION_010_CREATE_K8S_HELM_REPOS).await?;

    // NAS migrations (011-019)
    #[cfg(feature = "nas")]
    {
        run_migration(pool, "011_create_nas_shares_table", MIGRATION_011_CREATE_NAS_SHARES).await?;
        run_migration(pool, "012_create_nas_users_table", MIGRATION_012_CREATE_NAS_USERS).await?;
        run_migration(pool, "013_create_nas_groups_table", MIGRATION_013_CREATE_NAS_GROUPS).await?;
        run_migration(pool, "014_create_nas_pools_table", MIGRATION_014_CREATE_NAS_POOLS).await?;
        run_migration(pool, "015_create_nas_snapshots_table", MIGRATION_015_CREATE_NAS_SNAPSHOTS).await?;
        run_migration(pool, "016_create_nas_replication_table", MIGRATION_016_CREATE_NAS_REPLICATION).await?;
        run_migration(pool, "017_create_nas_iscsi_table", MIGRATION_017_CREATE_NAS_ISCSI).await?;
        run_migration(pool, "018_create_nas_s3_table", MIGRATION_018_CREATE_NAS_S3).await?;
        run_migration(pool, "019_create_nas_scheduler_table", MIGRATION_019_CREATE_NAS_SCHEDULER).await?;
    }

    Ok(())
}

async fn run_migration(pool: &SqlitePool, name: &str, sql: &str) -> Result<()> {
    use sqlx::Row;

    // Check if migration already ran
    let row = sqlx::query("SELECT COUNT(*) as count FROM migrations WHERE name = ?")
        .bind(name)
        .fetch_one(pool)
        .await
        .map_err(|e| horcrux_common::Error::System(format!("Migration check failed: {}", e)))?;

    let count: i64 = row.get("count");
    if count > 0 {
        tracing::debug!("Migration {} already applied", name);
        return Ok(());
    }

    tracing::info!("Running migration: {}", name);

    // Run migration
    sqlx::query(sql)
        .execute(pool)
        .await
        .map_err(|e| horcrux_common::Error::System(format!("Migration {} failed: {}", name, e)))?;

    // Record migration
    sqlx::query("INSERT INTO migrations (name) VALUES (?)")
        .bind(name)
        .execute(pool)
        .await
        .map_err(|e| horcrux_common::Error::System(format!("Failed to record migration: {}", e)))?;

    tracing::info!("Migration {} completed", name);

    Ok(())
}

const MIGRATION_001_CREATE_VMS: &str = "
CREATE TABLE vms (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    hypervisor TEXT NOT NULL,
    memory INTEGER NOT NULL,
    cpus INTEGER NOT NULL,
    disk_size INTEGER NOT NULL,
    status TEXT NOT NULL,
    architecture TEXT NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_vms_name ON vms(name);
CREATE INDEX idx_vms_status ON vms(status);
";

const MIGRATION_002_CREATE_USERS: &str = "
CREATE TABLE users (
    id TEXT PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    email TEXT,
    role TEXT NOT NULL,
    realm TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_users_username ON users(username);
CREATE INDEX idx_users_role ON users(role);
";

const MIGRATION_003_CREATE_SESSIONS: &str = "
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    expires_at INTEGER NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE INDEX idx_sessions_user ON sessions(user_id);
CREATE INDEX idx_sessions_expires ON sessions(expires_at);
";

const MIGRATION_004_CREATE_AUDIT_LOGS: &str = "
CREATE TABLE audit_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp INTEGER NOT NULL,
    event_type TEXT NOT NULL,
    severity TEXT NOT NULL,
    user TEXT,
    source_ip TEXT,
    resource TEXT,
    action TEXT NOT NULL,
    result TEXT NOT NULL,
    details TEXT,
    session_id TEXT
);

CREATE INDEX idx_audit_timestamp ON audit_logs(timestamp);
CREATE INDEX idx_audit_event_type ON audit_logs(event_type);
CREATE INDEX idx_audit_user ON audit_logs(user);
CREATE INDEX idx_audit_severity ON audit_logs(severity);
";

const MIGRATION_005_CREATE_STORAGE_POOLS: &str = "
CREATE TABLE storage_pools (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    storage_type TEXT NOT NULL,
    path TEXT NOT NULL,
    available INTEGER NOT NULL,
    total INTEGER NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_storage_name ON storage_pools(name);
CREATE INDEX idx_storage_type ON storage_pools(storage_type);
";

const MIGRATION_006_CREATE_BACKUPS: &str = "
CREATE TABLE backups (
    id TEXT PRIMARY KEY,
    vm_id TEXT NOT NULL,
    vm_name TEXT NOT NULL,
    path TEXT NOT NULL,
    size INTEGER NOT NULL,
    mode TEXT NOT NULL,
    compression TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    FOREIGN KEY (vm_id) REFERENCES vms(id) ON DELETE CASCADE
);

CREATE INDEX idx_backups_vm ON backups(vm_id);
CREATE INDEX idx_backups_created ON backups(created_at);
";

const MIGRATION_007_CREATE_CLUSTER_NODES: &str = "
CREATE TABLE cluster_nodes (
    name TEXT PRIMARY KEY,
    address TEXT NOT NULL,
    architecture TEXT NOT NULL,
    total_memory INTEGER NOT NULL,
    total_cpus INTEGER NOT NULL,
    online INTEGER NOT NULL DEFAULT 1,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_nodes_online ON cluster_nodes(online);
CREATE INDEX idx_nodes_arch ON cluster_nodes(architecture);
";

const MIGRATION_008_CREATE_API_KEYS: &str = "
CREATE TABLE api_keys (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    key_hash TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    expires_at INTEGER,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    last_used_at TIMESTAMP,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE INDEX idx_api_keys_user ON api_keys(user_id);
CREATE INDEX idx_api_keys_hash ON api_keys(key_hash);
CREATE INDEX idx_api_keys_enabled ON api_keys(enabled);
";

const MIGRATION_009_CREATE_K8S_CLUSTERS: &str = "
CREATE TABLE k8s_clusters (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    context TEXT NOT NULL,
    api_server TEXT NOT NULL,
    version TEXT,
    status TEXT NOT NULL DEFAULT 'disconnected',
    node_count INTEGER DEFAULT 0,
    provider TEXT NOT NULL DEFAULT 'external',
    kubeconfig_encrypted TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX idx_k8s_clusters_name ON k8s_clusters(name);
CREATE INDEX idx_k8s_clusters_status ON k8s_clusters(status);
CREATE INDEX idx_k8s_clusters_provider ON k8s_clusters(provider);
";

const MIGRATION_010_CREATE_K8S_HELM_REPOS: &str = "
CREATE TABLE k8s_helm_repos (
    id TEXT PRIMARY KEY,
    cluster_id TEXT NOT NULL,
    name TEXT NOT NULL,
    url TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    FOREIGN KEY (cluster_id) REFERENCES k8s_clusters(id) ON DELETE CASCADE
);

CREATE INDEX idx_k8s_helm_repos_cluster ON k8s_helm_repos(cluster_id);
CREATE UNIQUE INDEX idx_k8s_helm_repos_name ON k8s_helm_repos(cluster_id, name);
";

// ============================================================================
// NAS Migrations
// ============================================================================

#[cfg(feature = "nas")]
const MIGRATION_011_CREATE_NAS_SHARES: &str = "
-- NAS Shares table
CREATE TABLE nas_shares (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    path TEXT NOT NULL,
    description TEXT,
    enabled INTEGER NOT NULL DEFAULT 1,

    -- Protocol flags (which protocols export this share)
    smb_enabled INTEGER NOT NULL DEFAULT 0,
    nfs_enabled INTEGER NOT NULL DEFAULT 0,
    afp_enabled INTEGER NOT NULL DEFAULT 0,
    webdav_enabled INTEGER NOT NULL DEFAULT 0,
    ftp_enabled INTEGER NOT NULL DEFAULT 0,

    -- SMB configuration (JSON)
    smb_config TEXT,
    -- NFS configuration (JSON)
    nfs_config TEXT,
    -- AFP configuration (JSON)
    afp_config TEXT,
    -- WebDAV configuration (JSON)
    webdav_config TEXT,
    -- FTP configuration (JSON)
    ftp_config TEXT,

    -- Access control
    guest_access INTEGER NOT NULL DEFAULT 0,
    browseable INTEGER NOT NULL DEFAULT 1,
    read_only INTEGER NOT NULL DEFAULT 0,

    -- Quota
    quota_bytes INTEGER,

    -- ACLs (JSON array)
    acl TEXT,

    -- Pool/Dataset reference
    pool_id TEXT,
    dataset_path TEXT,

    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX idx_nas_shares_name ON nas_shares(name);
CREATE INDEX idx_nas_shares_enabled ON nas_shares(enabled);
CREATE INDEX idx_nas_shares_pool ON nas_shares(pool_id);
";

#[cfg(feature = "nas")]
const MIGRATION_012_CREATE_NAS_USERS: &str = "
-- NAS Users table (separate from platform users)
CREATE TABLE nas_users (
    id TEXT PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,

    -- Password hashes
    password_hash TEXT,           -- For PAM/local auth
    smb_password_hash TEXT,       -- NT hash for SMB

    -- User info
    full_name TEXT,
    email TEXT,
    shell TEXT DEFAULT '/sbin/nologin',
    home_directory TEXT,

    -- System user mapping
    uid INTEGER NOT NULL UNIQUE,
    primary_gid INTEGER NOT NULL,

    -- Status
    enabled INTEGER NOT NULL DEFAULT 1,
    locked INTEGER NOT NULL DEFAULT 0,

    -- Quota
    quota_bytes INTEGER,

    -- SSH authorized keys (JSON array)
    ssh_keys TEXT,

    -- External directory reference (LDAP DN, AD SID)
    external_id TEXT,
    external_source TEXT,         -- 'ldap', 'ad', 'local'

    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX idx_nas_users_username ON nas_users(username);
CREATE INDEX idx_nas_users_uid ON nas_users(uid);
CREATE INDEX idx_nas_users_enabled ON nas_users(enabled);

-- NAS user group membership (many-to-many)
CREATE TABLE nas_user_groups (
    user_id TEXT NOT NULL,
    group_id TEXT NOT NULL,
    PRIMARY KEY (user_id, group_id),
    FOREIGN KEY (user_id) REFERENCES nas_users(id) ON DELETE CASCADE,
    FOREIGN KEY (group_id) REFERENCES nas_groups(id) ON DELETE CASCADE
);
";

#[cfg(feature = "nas")]
const MIGRATION_013_CREATE_NAS_GROUPS: &str = "
-- NAS Groups table
CREATE TABLE nas_groups (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    description TEXT,

    -- System group mapping
    gid INTEGER NOT NULL UNIQUE,

    -- External directory reference
    external_id TEXT,
    external_source TEXT,

    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX idx_nas_groups_name ON nas_groups(name);
CREATE INDEX idx_nas_groups_gid ON nas_groups(gid);
";

#[cfg(feature = "nas")]
const MIGRATION_014_CREATE_NAS_POOLS: &str = "
-- NAS Storage Pools table
CREATE TABLE nas_pools (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,

    -- Pool type: zfs, btrfs, mdraid, lvm, directory
    pool_type TEXT NOT NULL,

    -- Health status
    health TEXT NOT NULL DEFAULT 'unknown',
    status TEXT NOT NULL DEFAULT 'unknown',

    -- Capacity (in bytes)
    total_bytes INTEGER NOT NULL DEFAULT 0,
    used_bytes INTEGER NOT NULL DEFAULT 0,
    available_bytes INTEGER NOT NULL DEFAULT 0,

    -- RAID configuration
    raid_level TEXT,

    -- Devices (JSON array)
    devices TEXT,

    -- Properties (JSON - compression, dedup, etc.)
    properties TEXT,

    -- Mount point
    mount_path TEXT,

    -- Scrub/check info
    last_scrub INTEGER,
    scrub_errors INTEGER DEFAULT 0,

    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX idx_nas_pools_name ON nas_pools(name);
CREATE INDEX idx_nas_pools_type ON nas_pools(pool_type);
CREATE INDEX idx_nas_pools_health ON nas_pools(health);

-- NAS Datasets table
CREATE TABLE nas_datasets (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    full_path TEXT NOT NULL UNIQUE,
    pool_id TEXT NOT NULL,

    -- Dataset type: filesystem, volume
    dataset_type TEXT NOT NULL DEFAULT 'filesystem',

    -- Mount point
    mount_path TEXT,

    -- Capacity
    used_bytes INTEGER NOT NULL DEFAULT 0,
    referenced_bytes INTEGER NOT NULL DEFAULT 0,
    available_bytes INTEGER NOT NULL DEFAULT 0,

    -- Quotas
    quota_bytes INTEGER,
    refquota_bytes INTEGER,

    -- Properties
    compression TEXT,
    recordsize INTEGER,
    atime INTEGER DEFAULT 1,
    sync_mode TEXT DEFAULT 'standard',

    -- Extended properties (JSON)
    properties TEXT,

    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,

    FOREIGN KEY (pool_id) REFERENCES nas_pools(id) ON DELETE CASCADE
);

CREATE INDEX idx_nas_datasets_pool ON nas_datasets(pool_id);
CREATE INDEX idx_nas_datasets_path ON nas_datasets(full_path);
";

#[cfg(feature = "nas")]
const MIGRATION_015_CREATE_NAS_SNAPSHOTS: &str = "
-- NAS Snapshots table
CREATE TABLE nas_snapshots (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    full_name TEXT NOT NULL UNIQUE,

    -- Parent dataset/pool
    dataset_id TEXT,
    pool_id TEXT,

    -- Snapshot info
    used_bytes INTEGER NOT NULL DEFAULT 0,
    referenced_bytes INTEGER NOT NULL DEFAULT 0,

    -- Retention
    hold INTEGER NOT NULL DEFAULT 0,
    hold_tag TEXT,

    -- Auto-snapshot policy that created this
    policy TEXT,

    created_at INTEGER NOT NULL,

    FOREIGN KEY (dataset_id) REFERENCES nas_datasets(id) ON DELETE CASCADE,
    FOREIGN KEY (pool_id) REFERENCES nas_pools(id) ON DELETE CASCADE
);

CREATE INDEX idx_nas_snapshots_dataset ON nas_snapshots(dataset_id);
CREATE INDEX idx_nas_snapshots_pool ON nas_snapshots(pool_id);
CREATE INDEX idx_nas_snapshots_created ON nas_snapshots(created_at);
CREATE INDEX idx_nas_snapshots_policy ON nas_snapshots(policy);

-- Snapshot policies/schedules
CREATE TABLE nas_snapshot_policies (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,

    -- What to snapshot (dataset path pattern)
    target_pattern TEXT NOT NULL,
    recursive INTEGER NOT NULL DEFAULT 1,

    -- Schedule (cron expression)
    schedule TEXT NOT NULL,

    -- Retention
    keep_count INTEGER NOT NULL DEFAULT 10,
    keep_days INTEGER,
    keep_weeks INTEGER,
    keep_months INTEGER,

    enabled INTEGER NOT NULL DEFAULT 1,

    -- Last run info
    last_run INTEGER,
    last_status TEXT,

    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX idx_nas_snapshot_policies_enabled ON nas_snapshot_policies(enabled);
";

#[cfg(feature = "nas")]
const MIGRATION_016_CREATE_NAS_REPLICATION: &str = "
-- NAS Replication Tasks table
CREATE TABLE nas_replication_tasks (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,

    -- Source
    source_dataset TEXT NOT NULL,
    source_host TEXT,             -- NULL = local

    -- Destination
    dest_dataset TEXT NOT NULL,
    dest_host TEXT,               -- NULL = local

    -- Transport: ssh, local, netcat
    transport TEXT NOT NULL DEFAULT 'ssh',
    ssh_key_id TEXT,
    ssh_port INTEGER DEFAULT 22,

    -- Replication type: full, incremental
    replication_type TEXT NOT NULL DEFAULT 'incremental',
    recursive INTEGER NOT NULL DEFAULT 1,

    -- Options
    compressed INTEGER NOT NULL DEFAULT 1,
    rate_limit_kbps INTEGER,

    -- Schedule (cron expression)
    schedule TEXT,

    -- Retention policy for dest (JSON)
    retention_policy TEXT,

    enabled INTEGER NOT NULL DEFAULT 1,

    -- Last run info
    last_run INTEGER,
    last_status TEXT,
    last_snapshot TEXT,
    last_bytes_transferred INTEGER,
    last_duration_secs INTEGER,

    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX idx_nas_replication_enabled ON nas_replication_tasks(enabled);
CREATE INDEX idx_nas_replication_source ON nas_replication_tasks(source_dataset);
CREATE INDEX idx_nas_replication_dest ON nas_replication_tasks(dest_dataset);

-- Replication run history
CREATE TABLE nas_replication_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL,

    started_at INTEGER NOT NULL,
    completed_at INTEGER,

    status TEXT NOT NULL,         -- running, success, failed
    snapshot_name TEXT,
    bytes_transferred INTEGER,
    duration_secs INTEGER,
    error_message TEXT,

    FOREIGN KEY (task_id) REFERENCES nas_replication_tasks(id) ON DELETE CASCADE
);

CREATE INDEX idx_nas_replication_history_task ON nas_replication_history(task_id);
CREATE INDEX idx_nas_replication_history_started ON nas_replication_history(started_at);
";

#[cfg(feature = "nas")]
const MIGRATION_017_CREATE_NAS_ISCSI: &str = "
-- iSCSI Targets table
CREATE TABLE nas_iscsi_targets (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    iqn TEXT NOT NULL UNIQUE,      -- iSCSI Qualified Name

    -- Target settings
    alias TEXT,
    max_sessions INTEGER DEFAULT 0, -- 0 = unlimited

    -- CHAP authentication (optional)
    chap_user TEXT,
    chap_password_encrypted TEXT,
    mutual_chap_user TEXT,
    mutual_chap_password_encrypted TEXT,

    enabled INTEGER NOT NULL DEFAULT 1,

    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX idx_nas_iscsi_targets_name ON nas_iscsi_targets(name);
CREATE INDEX idx_nas_iscsi_targets_iqn ON nas_iscsi_targets(iqn);
CREATE INDEX idx_nas_iscsi_targets_enabled ON nas_iscsi_targets(enabled);

-- iSCSI LUNs table
CREATE TABLE nas_iscsi_luns (
    id TEXT PRIMARY KEY,
    target_id TEXT NOT NULL,
    lun_number INTEGER NOT NULL,

    -- Backing store: zvol path or file path
    backing_store TEXT NOT NULL,
    backing_type TEXT NOT NULL,    -- zvol, file

    -- Size
    size_bytes INTEGER NOT NULL,

    -- LUN settings
    read_only INTEGER NOT NULL DEFAULT 0,
    write_cache INTEGER NOT NULL DEFAULT 1,

    created_at INTEGER NOT NULL,

    FOREIGN KEY (target_id) REFERENCES nas_iscsi_targets(id) ON DELETE CASCADE,
    UNIQUE (target_id, lun_number)
);

CREATE INDEX idx_nas_iscsi_luns_target ON nas_iscsi_luns(target_id);

-- iSCSI Initiator ACLs (which hosts can connect)
CREATE TABLE nas_iscsi_acls (
    id TEXT PRIMARY KEY,
    target_id TEXT NOT NULL,

    -- Initiator filter (IQN pattern or IP/CIDR)
    initiator_iqn TEXT,
    initiator_ip TEXT,

    -- Per-initiator CHAP (optional, overrides target CHAP)
    chap_user TEXT,
    chap_password_encrypted TEXT,

    comment TEXT,

    FOREIGN KEY (target_id) REFERENCES nas_iscsi_targets(id) ON DELETE CASCADE
);

CREATE INDEX idx_nas_iscsi_acls_target ON nas_iscsi_acls(target_id);
";

#[cfg(feature = "nas")]
const MIGRATION_018_CREATE_NAS_S3: &str = "
-- S3 Buckets table
CREATE TABLE nas_s3_buckets (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,

    -- Storage location (maps to filesystem path)
    path TEXT NOT NULL,

    -- Bucket settings
    versioning INTEGER NOT NULL DEFAULT 0,
    object_locking INTEGER NOT NULL DEFAULT 0,

    -- Quotas
    quota_bytes INTEGER,
    max_objects INTEGER,

    -- Owner
    owner_key_id TEXT,

    -- Lifecycle rules (JSON)
    lifecycle_rules TEXT,

    -- CORS configuration (JSON)
    cors_config TEXT,

    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX idx_nas_s3_buckets_name ON nas_s3_buckets(name);
CREATE INDEX idx_nas_s3_buckets_owner ON nas_s3_buckets(owner_key_id);

-- S3 Access Keys table
CREATE TABLE nas_s3_access_keys (
    id TEXT PRIMARY KEY,
    access_key TEXT NOT NULL UNIQUE,
    secret_key_encrypted TEXT NOT NULL,

    -- Owner (references nas_users)
    user_id TEXT,

    -- Key settings
    name TEXT,
    enabled INTEGER NOT NULL DEFAULT 1,

    -- Permissions (JSON array of bucket:permission)
    permissions TEXT,

    -- Expiry
    expires_at INTEGER,

    -- Usage tracking
    last_used_at INTEGER,

    created_at INTEGER NOT NULL,

    FOREIGN KEY (user_id) REFERENCES nas_users(id) ON DELETE CASCADE
);

CREATE INDEX idx_nas_s3_keys_access_key ON nas_s3_access_keys(access_key);
CREATE INDEX idx_nas_s3_keys_user ON nas_s3_access_keys(user_id);
CREATE INDEX idx_nas_s3_keys_enabled ON nas_s3_access_keys(enabled);

-- NAS Directory/Auth Configuration table
CREATE TABLE nas_directory_config (
    id TEXT PRIMARY KEY,

    -- LDAP settings
    ldap_enabled INTEGER NOT NULL DEFAULT 0,
    ldap_uri TEXT,
    ldap_base_dn TEXT,
    ldap_bind_dn TEXT,
    ldap_bind_password_encrypted TEXT,
    ldap_user_filter TEXT,
    ldap_group_filter TEXT,
    ldap_tls INTEGER NOT NULL DEFAULT 1,
    ldap_tls_cert TEXT,

    -- Kerberos settings
    kerberos_enabled INTEGER NOT NULL DEFAULT 0,
    kerberos_realm TEXT,
    kerberos_kdc TEXT,
    kerberos_admin_server TEXT,

    -- Active Directory settings
    ad_enabled INTEGER NOT NULL DEFAULT 0,
    ad_domain TEXT,
    ad_workgroup TEXT,
    ad_realm TEXT,
    ad_dc_hostname TEXT,
    ad_join_user TEXT,
    ad_computer_account TEXT,
    ad_idmap_backend TEXT DEFAULT 'rid',
    ad_idmap_range TEXT DEFAULT '10000-999999',

    updated_at INTEGER NOT NULL
);
";

#[cfg(feature = "nas")]
const MIGRATION_019_CREATE_NAS_SCHEDULER: &str = "
-- NAS Scheduled Jobs table
CREATE TABLE nas_scheduled_jobs (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,

    -- Job type (JSON containing job details: snapshot, retention, replication, scrub, etc.)
    job_type TEXT NOT NULL,

    -- Cron schedule expression (e.g., '0 0 * * *' for daily at midnight)
    schedule TEXT NOT NULL,

    -- Job state
    enabled INTEGER NOT NULL DEFAULT 1,
    paused INTEGER NOT NULL DEFAULT 0,

    -- Run at system startup
    run_on_startup INTEGER NOT NULL DEFAULT 0,

    -- Timing
    last_run INTEGER,
    next_run INTEGER,
    last_duration_secs INTEGER,

    -- Status of last run
    last_status TEXT,
    last_error TEXT,

    -- Retry configuration
    max_retries INTEGER DEFAULT 0,
    retry_delay_secs INTEGER DEFAULT 60,
    retry_count INTEGER DEFAULT 0,

    -- Job priority (lower = higher priority)
    priority INTEGER DEFAULT 50,

    -- Concurrent execution policy
    allow_concurrent INTEGER NOT NULL DEFAULT 0,

    -- Failure notification
    notify_on_failure INTEGER NOT NULL DEFAULT 1,
    notification_email TEXT,

    -- Description
    description TEXT,

    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX idx_nas_scheduled_jobs_name ON nas_scheduled_jobs(name);
CREATE INDEX idx_nas_scheduled_jobs_enabled ON nas_scheduled_jobs(enabled);
CREATE INDEX idx_nas_scheduled_jobs_next_run ON nas_scheduled_jobs(next_run);
CREATE INDEX idx_nas_scheduled_jobs_priority ON nas_scheduled_jobs(priority);

-- Job execution history
CREATE TABLE nas_job_history (
    id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL,

    -- Execution timing
    started_at INTEGER NOT NULL,
    completed_at INTEGER,
    duration_secs INTEGER,

    -- Status: pending, running, success, failed, cancelled
    status TEXT NOT NULL,

    -- Result details
    error_message TEXT,
    output TEXT,

    -- Metrics
    bytes_processed INTEGER,
    items_processed INTEGER,

    -- Retry info
    retry_attempt INTEGER DEFAULT 0,

    FOREIGN KEY (job_id) REFERENCES nas_scheduled_jobs(id) ON DELETE CASCADE
);

CREATE INDEX idx_nas_job_history_job ON nas_job_history(job_id);
CREATE INDEX idx_nas_job_history_started ON nas_job_history(started_at);
CREATE INDEX idx_nas_job_history_status ON nas_job_history(status);

-- Job dependencies (run job B after job A completes)
CREATE TABLE nas_job_dependencies (
    id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL,
    depends_on_job_id TEXT NOT NULL,

    -- Dependency type: success (only if predecessor succeeded), always (run regardless)
    dependency_type TEXT NOT NULL DEFAULT 'success',

    FOREIGN KEY (job_id) REFERENCES nas_scheduled_jobs(id) ON DELETE CASCADE,
    FOREIGN KEY (depends_on_job_id) REFERENCES nas_scheduled_jobs(id) ON DELETE CASCADE,
    UNIQUE (job_id, depends_on_job_id)
);

CREATE INDEX idx_nas_job_deps_job ON nas_job_dependencies(job_id);
CREATE INDEX idx_nas_job_deps_depends ON nas_job_dependencies(depends_on_job_id);
";
