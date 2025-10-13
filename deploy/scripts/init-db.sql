-- Horcrux Database Schema
-- SQLite database initialization script

-- Enable foreign keys
PRAGMA foreign_keys = ON;

-- VMs table
CREATE TABLE IF NOT EXISTS vms (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    memory INTEGER NOT NULL,
    cpus INTEGER NOT NULL,
    disk_size INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'stopped',
    os_type TEXT,
    boot_order TEXT,
    machine_type TEXT,
    cpu_type TEXT,
    bios_type TEXT,
    network_bridge TEXT,
    storage_pool TEXT,
    ha_enabled BOOLEAN DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    created_by TEXT,
    node TEXT,
    description TEXT,
    tags TEXT,
    metadata TEXT
);

CREATE INDEX IF NOT EXISTS idx_vms_status ON vms(status);
CREATE INDEX IF NOT EXISTS idx_vms_node ON vms(node);
CREATE INDEX IF NOT EXISTS idx_vms_created_at ON vms(created_at);

-- Containers table
CREATE TABLE IF NOT EXISTS containers (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    runtime TEXT NOT NULL,
    image TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'stopped',
    created_at INTEGER,
    node TEXT,
    memory_limit INTEGER,
    cpu_limit INTEGER,
    network_mode TEXT,
    ports TEXT,
    volumes TEXT,
    environment TEXT,
    labels TEXT,
    metadata TEXT
);

CREATE INDEX IF NOT EXISTS idx_containers_runtime ON containers(runtime);
CREATE INDEX IF NOT EXISTS idx_containers_status ON containers(status);

-- Snapshots table
CREATE TABLE IF NOT EXISTS snapshots (
    id TEXT PRIMARY KEY,
    vm_id TEXT NOT NULL,
    name TEXT NOT NULL,
    description TEXT,
    include_memory BOOLEAN DEFAULT 0,
    size_bytes INTEGER DEFAULT 0,
    created_at TEXT NOT NULL,
    parent_id TEXT,
    FOREIGN KEY (vm_id) REFERENCES vms(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_snapshots_vm_id ON snapshots(vm_id);
CREATE INDEX IF NOT EXISTS idx_snapshots_created_at ON snapshots(created_at);

-- Storage pools table
CREATE TABLE IF NOT EXISTS storage_pools (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    storage_type TEXT NOT NULL,
    path TEXT NOT NULL,
    total_bytes INTEGER NOT NULL DEFAULT 0,
    available_bytes INTEGER NOT NULL DEFAULT 0,
    enabled BOOLEAN DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    config TEXT
);

-- Users table
CREATE TABLE IF NOT EXISTS users (
    id TEXT PRIMARY KEY,
    username TEXT NOT NULL,
    email TEXT,
    password_hash TEXT,
    realm TEXT NOT NULL DEFAULT 'pam',
    enabled BOOLEAN DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    last_login INTEGER,
    roles TEXT,
    metadata TEXT,
    UNIQUE(username, realm)
);

CREATE INDEX IF NOT EXISTS idx_users_username ON users(username);
CREATE INDEX IF NOT EXISTS idx_users_enabled ON users(enabled);

-- Sessions table
CREATE TABLE IF NOT EXISTS sessions (
    session_id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    username TEXT NOT NULL,
    realm TEXT NOT NULL,
    created INTEGER NOT NULL,
    expires INTEGER NOT NULL,
    csrf_token TEXT,
    ip_address TEXT,
    user_agent TEXT,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_sessions_user_id ON sessions(user_id);
CREATE INDEX IF NOT EXISTS idx_sessions_expires ON sessions(expires);

-- API tokens table
CREATE TABLE IF NOT EXISTS api_tokens (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    name TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    enabled BOOLEAN DEFAULT 1,
    created_at INTEGER NOT NULL,
    expires_at INTEGER,
    last_used_at INTEGER,
    permissions TEXT,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_api_tokens_user_id ON api_tokens(user_id);
CREATE INDEX IF NOT EXISTS idx_api_tokens_token_hash ON api_tokens(token_hash);

-- Audit log table
CREATE TABLE IF NOT EXISTS audit_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp INTEGER NOT NULL,
    user_id TEXT,
    username TEXT,
    action TEXT NOT NULL,
    resource_type TEXT,
    resource_id TEXT,
    details TEXT,
    ip_address TEXT,
    success BOOLEAN DEFAULT 1
);

CREATE INDEX IF NOT EXISTS idx_audit_log_timestamp ON audit_log(timestamp);
CREATE INDEX IF NOT EXISTS idx_audit_log_user_id ON audit_log(user_id);
CREATE INDEX IF NOT EXISTS idx_audit_log_action ON audit_log(action);

-- Alerts table
CREATE TABLE IF NOT EXISTS alerts (
    id TEXT PRIMARY KEY,
    rule_name TEXT NOT NULL,
    severity TEXT NOT NULL,
    target TEXT NOT NULL,
    message TEXT NOT NULL,
    triggered_at INTEGER NOT NULL,
    resolved_at INTEGER,
    acknowledged BOOLEAN DEFAULT 0,
    acknowledged_by TEXT,
    acknowledged_at INTEGER,
    metadata TEXT
);

CREATE INDEX IF NOT EXISTS idx_alerts_triggered_at ON alerts(triggered_at);
CREATE INDEX IF NOT EXISTS idx_alerts_resolved_at ON alerts(resolved_at);
CREATE INDEX IF NOT EXISTS idx_alerts_severity ON alerts(severity);

-- Backups table
CREATE TABLE IF NOT EXISTS backups (
    id TEXT PRIMARY KEY,
    vm_id TEXT NOT NULL,
    name TEXT NOT NULL,
    backup_type TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    compressed BOOLEAN DEFAULT 1,
    encrypted BOOLEAN DEFAULT 0,
    created_at INTEGER NOT NULL,
    expires_at INTEGER,
    storage_path TEXT NOT NULL,
    status TEXT DEFAULT 'completed',
    error_message TEXT,
    duration_seconds INTEGER,
    FOREIGN KEY (vm_id) REFERENCES vms(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_backups_vm_id ON backups(vm_id);
CREATE INDEX IF NOT EXISTS idx_backups_created_at ON backups(created_at);
CREATE INDEX IF NOT EXISTS idx_backups_status ON backups(status);

-- Replication jobs table
CREATE TABLE IF NOT EXISTS replication_jobs (
    id TEXT PRIMARY KEY,
    vm_id TEXT NOT NULL,
    source_node TEXT NOT NULL,
    target_node TEXT NOT NULL,
    schedule TEXT NOT NULL,
    enabled BOOLEAN DEFAULT 1,
    last_sync INTEGER,
    last_status TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY (vm_id) REFERENCES vms(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_replication_vm_id ON replication_jobs(vm_id);
CREATE INDEX IF NOT EXISTS idx_replication_enabled ON replication_jobs(enabled);

-- Clone jobs table
CREATE TABLE IF NOT EXISTS clone_jobs (
    id TEXT PRIMARY KEY,
    source_vm_id TEXT NOT NULL,
    target_vm_id TEXT,
    clone_type TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    progress INTEGER DEFAULT 0,
    started_at INTEGER,
    completed_at INTEGER,
    error_message TEXT,
    FOREIGN KEY (source_vm_id) REFERENCES vms(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_clone_jobs_status ON clone_jobs(status);
CREATE INDEX IF NOT EXISTS idx_clone_jobs_started_at ON clone_jobs(started_at);

-- Network templates table
CREATE TABLE IF NOT EXISTS network_templates (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    template_type TEXT NOT NULL,
    config TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    created_by TEXT
);

-- Metrics history table (for historical data)
CREATE TABLE IF NOT EXISTS metrics_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp INTEGER NOT NULL,
    resource_type TEXT NOT NULL,
    resource_id TEXT NOT NULL,
    metric_name TEXT NOT NULL,
    metric_value REAL NOT NULL,
    unit TEXT
);

CREATE INDEX IF NOT EXISTS idx_metrics_timestamp ON metrics_history(timestamp);
CREATE INDEX IF NOT EXISTS idx_metrics_resource ON metrics_history(resource_type, resource_id);
CREATE INDEX IF NOT EXISTS idx_metrics_name ON metrics_history(metric_name);

-- Database version table
CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER PRIMARY KEY,
    applied_at INTEGER NOT NULL,
    description TEXT
);

-- Insert initial schema version
INSERT OR IGNORE INTO schema_version (version, applied_at, description)
VALUES (1, strftime('%s', 'now'), 'Initial schema');

-- Create default admin user (password: admin123)
-- Password hash generated with Argon2
INSERT OR IGNORE INTO users (
    id,
    username,
    email,
    password_hash,
    realm,
    enabled,
    created_at,
    updated_at,
    roles
) VALUES (
    'admin@pam',
    'admin',
    'admin@horcrux.local',
    '$argon2id$v=19$m=19456,t=2,p=1$somesalt$hash',
    'pam',
    1,
    strftime('%s', 'now'),
    strftime('%s', 'now'),
    '["Administrator"]'
);

-- Create default storage pool
INSERT OR IGNORE INTO storage_pools (
    id,
    name,
    storage_type,
    path,
    total_bytes,
    available_bytes,
    enabled,
    created_at,
    updated_at
) VALUES (
    'local',
    'local',
    'dir',
    '/var/lib/horcrux/vms',
    1099511627776,
    1099511627776,
    1,
    strftime('%s', 'now'),
    strftime('%s', 'now')
);

-- Vacuum and optimize
VACUUM;
ANALYZE;
