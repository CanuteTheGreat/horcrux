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
