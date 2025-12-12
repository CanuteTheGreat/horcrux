///! Database layer using SQLite
///!
///! Provides persistent storage for VMs, users, sessions, audit logs, etc.

pub mod migrations;

use horcrux_common::Result;
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use std::path::Path;

/// Database connection pool
pub struct Database {
    pool: SqlitePool,
}

impl Database {
    /// Create a new database connection
    pub async fn new(database_url: &str) -> Result<Self> {
        // Create parent directory if needed
        if let Some(path) = database_url.strip_prefix("sqlite://") {
            if let Some(parent) = Path::new(path).parent() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    horcrux_common::Error::System(format!("Failed to create DB directory: {}", e))
                })?;
            }
        }

        // Create connection pool
        let pool = SqlitePoolOptions::new()
            .max_connections(32)
            .connect(database_url)
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Database connection failed: {}", e)))?;

        tracing::info!("Database connection established");

        Ok(Self { pool })
    }

    /// Run database migrations
    pub async fn migrate(&self) -> Result<()> {
        migrations::run_migrations(&self.pool).await?;
        tracing::info!("Database migrations completed");
        Ok(())
    }

    /// Get the connection pool
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    // VM operations
    pub async fn list_vms(&self) -> Result<Vec<horcrux_common::VmConfig>> {
        vms::list_vms(&self.pool).await
    }

    pub async fn get_vm(&self, id: &str) -> Result<horcrux_common::VmConfig> {
        vms::get_vm(&self.pool, id).await
    }

    pub async fn create_vm(&self, vm: &horcrux_common::VmConfig) -> Result<()> {
        vms::create_vm(&self.pool, vm).await
    }

    #[allow(dead_code)]
    pub async fn update_vm(&self, vm: &horcrux_common::VmConfig) -> Result<()> {
        vms::update_vm(&self.pool, vm).await
    }

    pub async fn delete_vm(&self, id: &str) -> Result<()> {
        vms::delete_vm(&self.pool, id).await
    }

    /// Close the database connection
    pub async fn close(&self) {
        self.pool.close().await;
        tracing::info!("Database connection closed");
    }

    /// Health check - verify database connectivity
    pub async fn health_check(&self) -> Result<()> {
        sqlx::query("SELECT 1")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Database health check failed: {}", e)))?;
        Ok(())
    }
}

/// VM database operations
pub mod vms {
    use super::*;
    use horcrux_common::{VmConfig, VmStatus, VmArchitecture, VmHypervisor};

    pub async fn create_vm(pool: &SqlitePool, vm: &VmConfig) -> Result<()> {
        let status_str = format!("{:?}", vm.status).to_lowercase();
        let arch_str = format!("{:?}", vm.architecture).to_lowercase();
        let hypervisor_str = format!("{:?}", vm.hypervisor).to_lowercase();

        sqlx::query(
            "INSERT INTO vms (id, name, hypervisor, memory, cpus, disk_size, status, architecture)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&vm.id)
        .bind(&vm.name)
        .bind(&hypervisor_str)
        .bind(vm.memory as i64)
        .bind(vm.cpus as i64)
        .bind(vm.disk_size as i64)
        .bind(&status_str)
        .bind(&arch_str)
        .execute(pool)
        .await
        .map_err(|e| horcrux_common::Error::System(format!("Failed to create VM: {}", e)))?;

        Ok(())
    }

    pub async fn get_vm(pool: &SqlitePool, id: &str) -> Result<VmConfig> {
        let row = sqlx::query("SELECT * FROM vms WHERE id = ?")
            .bind(id)
            .fetch_one(pool)
            .await
            .map_err(|_e| horcrux_common::Error::VmNotFound(id.to_string()))?;

        Ok(row_to_vm(&row)?)
    }

    pub async fn list_vms(pool: &SqlitePool) -> Result<Vec<VmConfig>> {
        let rows = sqlx::query("SELECT * FROM vms ORDER BY name")
            .fetch_all(pool)
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to list VMs: {}", e)))?;

        // Pre-allocate vector with exact capacity for better performance
        let mut vms = Vec::with_capacity(rows.len());
        for row in rows {
            vms.push(row_to_vm(&row)?);
        }

        Ok(vms)
    }

    #[allow(dead_code)]
    pub async fn update_vm(pool: &SqlitePool, vm: &VmConfig) -> Result<()> {
        let status_str = format!("{:?}", vm.status).to_lowercase();
        let arch_str = format!("{:?}", vm.architecture).to_lowercase();
        let hypervisor_str = format!("{:?}", vm.hypervisor).to_lowercase();

        sqlx::query(
            "UPDATE vms SET name = ?, hypervisor = ?, memory = ?, cpus = ?, disk_size = ?,
             status = ?, architecture = ?, updated_at = CURRENT_TIMESTAMP
             WHERE id = ?"
        )
        .bind(&vm.name)
        .bind(&hypervisor_str)
        .bind(vm.memory as i64)
        .bind(vm.cpus as i64)
        .bind(vm.disk_size as i64)
        .bind(&status_str)
        .bind(&arch_str)
        .bind(&vm.id)
        .execute(pool)
        .await
        .map_err(|e| horcrux_common::Error::System(format!("Failed to update VM: {}", e)))?;

        Ok(())
    }

    pub async fn delete_vm(pool: &SqlitePool, id: &str) -> Result<()> {
        let result = sqlx::query("DELETE FROM vms WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to delete VM: {}", e)))?;

        if result.rows_affected() == 0 {
            return Err(horcrux_common::Error::VmNotFound(id.to_string()));
        }

        Ok(())
    }

    fn row_to_vm(row: &sqlx::sqlite::SqliteRow) -> Result<VmConfig> {
        use sqlx::Row;
        let status_str: String = row.get("status");
        let arch_str: String = row.get("architecture");
        let hypervisor_str: String = row.get("hypervisor");

        let status = match status_str.as_str() {
            "running" => VmStatus::Running,
            "stopped" => VmStatus::Stopped,
            "paused" => VmStatus::Paused,
            _ => VmStatus::Unknown,
        };

        let architecture = match arch_str.as_str() {
            "x86_64" => VmArchitecture::X86_64,
            "aarch64" => VmArchitecture::Aarch64,
            "riscv64" => VmArchitecture::Riscv64,
            "ppc64le" => VmArchitecture::Ppc64le,
            _ => VmArchitecture::X86_64,
        };

        let hypervisor = match hypervisor_str.as_str() {
            "qemu" => VmHypervisor::Qemu,
            "lxd" => VmHypervisor::Lxd,
            "incus" => VmHypervisor::Incus,
            _ => VmHypervisor::Qemu,
        };

        Ok(VmConfig {
            id: row.get("id"),
            name: row.get("name"),
            hypervisor,
            memory: row.get::<i64, _>("memory") as u64,
            cpus: row.get::<i64, _>("cpus") as u32,
            disk_size: row.get::<i64, _>("disk_size") as u64,
            status,
            architecture,
            disks: Vec::new(), // Disks are managed separately via storage module
        })
    }
}

/// User and session database operations
pub mod users {
    use super::*;
    use horcrux_common::auth::{User, Session};
    use sqlx::Row;
    

    pub async fn create_user(pool: &SqlitePool, user: &User) -> Result<()> {
        sqlx::query(
            "INSERT INTO users (id, username, password_hash, email, role, realm, enabled)
             VALUES (?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&user.id)
        .bind(&user.username)
        .bind(&user.password_hash)
        .bind(&user.email)
        .bind(&user.role)
        .bind(&user.realm)
        .bind(user.enabled)
        .execute(pool)
        .await
        .map_err(|e| horcrux_common::Error::System(format!("Failed to create user: {}", e)))?;

        Ok(())
    }

    pub async fn get_user_by_username(pool: &SqlitePool, username: &str) -> Result<User> {
        let row = sqlx::query("SELECT * FROM users WHERE username = ? AND enabled = 1")
            .bind(username)
            .fetch_one(pool)
            .await
            .map_err(|_| horcrux_common::Error::AuthenticationFailed)?;

        Ok(row_to_user(&row))
    }

    pub async fn list_users(pool: &SqlitePool) -> Result<Vec<User>> {
        let rows = sqlx::query("SELECT * FROM users ORDER BY username")
            .fetch_all(pool)
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to list users: {}", e)))?;

        Ok(rows.iter().map(row_to_user).collect())
    }

    #[allow(dead_code)]
    pub async fn delete_user(pool: &SqlitePool, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM users WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to delete user: {}", e)))?;

        Ok(())
    }

    pub async fn create_session(pool: &SqlitePool, session: &Session) -> Result<()> {
        let expires_at = session.expires_at.timestamp();

        sqlx::query(
            "INSERT INTO sessions (id, user_id, expires_at)
             VALUES (?, ?, ?)"
        )
        .bind(&session.id)
        .bind(&session.user_id)
        .bind(expires_at)
        .execute(pool)
        .await
        .map_err(|e| horcrux_common::Error::System(format!("Failed to create session: {}", e)))?;

        Ok(())
    }

    pub async fn get_session(pool: &SqlitePool, session_id: &str) -> Result<Session> {
        let row = sqlx::query("SELECT * FROM sessions WHERE id = ?")
            .bind(session_id)
            .fetch_one(pool)
            .await
            .map_err(|_| horcrux_common::Error::InvalidSession)?;

        let expires_at_timestamp: i64 = row.get("expires_at");
        let expires_at = chrono::DateTime::from_timestamp(expires_at_timestamp, 0)
            .ok_or_else(|| horcrux_common::Error::System("Invalid timestamp".to_string()))?
            .with_timezone(&chrono::Utc);

        // Check if session is expired
        if expires_at < chrono::Utc::now() {
            return Err(horcrux_common::Error::InvalidSession);
        }

        let id: String = row.get("id");
        Ok(Session {
            id: id.clone(),
            user_id: row.get("user_id"),
            expires_at,
            session_id: id,
            username: String::new(),
            realm: String::new(),
            created: 0,
            expires: expires_at_timestamp,
        })
    }

    #[allow(dead_code)]
    pub async fn delete_session(pool: &SqlitePool, session_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(session_id)
            .execute(pool)
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to delete session: {}", e)))?;

        Ok(())
    }

    pub async fn cleanup_expired_sessions(pool: &SqlitePool) -> Result<()> {
        let now = chrono::Utc::now().timestamp();

        sqlx::query("DELETE FROM sessions WHERE expires_at < ?")
            .bind(now)
            .execute(pool)
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to cleanup sessions: {}", e)))?;

        Ok(())
    }

    fn row_to_user(row: &sqlx::sqlite::SqliteRow) -> User {
        User {
            id: row.get("id"),
            username: row.get("username"),
            password_hash: row.get("password_hash"),
            email: row.get("email"),
            role: row.get("role"),
            realm: row.get("realm"),
            enabled: row.get("enabled"),
            roles: Vec::new(),
            comment: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horcrux_common::{VmConfig, VmStatus, VmArchitecture, VmHypervisor};

    async fn create_test_db() -> Database {
        // Use in-memory database for tests
        let db_url = "sqlite::memory:";

        let db = Database::new(&db_url).await.expect("Failed to create database");
        db.migrate().await.expect("Failed to run migrations");
        db
    }

    fn create_test_vm_config(id: &str, name: &str) -> VmConfig {
        VmConfig {
            id: id.to_string(),
            name: name.to_string(),
            hypervisor: VmHypervisor::Qemu,
            memory: 2048,
            cpus: 2,
            disk_size: 20 * 1024 * 1024 * 1024,
            status: VmStatus::Stopped,
            architecture: VmArchitecture::X86_64,
            disks: Vec::new(),
        }
    }

    #[tokio::test]
    async fn test_database_connection() {
        let _db = create_test_db().await;
        // If we get here, connection was successful
    }

    #[tokio::test]
    async fn test_vm_crud_operations() {
        let db = create_test_db().await;

        // Create VM
        let vm_config = create_test_vm_config("test-vm-1", "Test VM 1");
        db.create_vm(&vm_config).await.expect("Failed to create VM");

        // Get VM
        let retrieved = db.get_vm("test-vm-1").await.expect("Failed to get VM");
        assert_eq!(retrieved.id, "test-vm-1");
        assert_eq!(retrieved.name, "Test VM 1");
        assert_eq!(retrieved.memory, 2048);
        assert_eq!(retrieved.cpus, 2);

        // List VMs
        let vms = db.list_vms().await.expect("Failed to list VMs");
        assert_eq!(vms.len(), 1);
        assert_eq!(vms[0].id, "test-vm-1");

        // Update VM
        let mut updated_config = retrieved.clone();
        updated_config.name = "Updated Test VM 1".to_string();
        updated_config.memory = 4096;
        db.update_vm(&updated_config).await.expect("Failed to update VM");

        let updated = db.get_vm("test-vm-1").await.expect("Failed to get updated VM");
        assert_eq!(updated.name, "Updated Test VM 1");
        assert_eq!(updated.memory, 4096);

        // Delete VM
        db.delete_vm("test-vm-1").await.expect("Failed to delete VM");

        let result = db.get_vm("test-vm-1").await;
        assert!(result.is_err(), "VM should not exist after deletion");
    }

    #[tokio::test]
    async fn test_list_multiple_vms() {
        let db = create_test_db().await;

        // Create multiple VMs
        for i in 1..=5 {
            let vm_config = create_test_vm_config(
                &format!("test-vm-{}", i),
                &format!("Test VM {}", i),
            );
            db.create_vm(&vm_config).await.expect("Failed to create VM");
        }

        // List all VMs
        let vms = db.list_vms().await.expect("Failed to list VMs");
        assert_eq!(vms.len(), 5);

        // VMs should be sorted by name
        for (i, vm) in vms.iter().enumerate() {
            assert_eq!(vm.name, format!("Test VM {}", i + 1));
        }
    }

    #[tokio::test]
    async fn test_get_nonexistent_vm() {
        let db = create_test_db().await;

        let result = db.get_vm("nonexistent").await;
        assert!(result.is_err());
        match result {
            Err(horcrux_common::Error::VmNotFound(id)) => {
                assert_eq!(id, "nonexistent");
            }
            _ => panic!("Expected VmNotFound error"),
        }
    }

    #[tokio::test]
    async fn test_delete_nonexistent_vm() {
        let db = create_test_db().await;

        let result = db.delete_vm("nonexistent").await;
        assert!(result.is_err());
        match result {
            Err(horcrux_common::Error::VmNotFound(id)) => {
                assert_eq!(id, "nonexistent");
            }
            _ => panic!("Expected VmNotFound error"),
        }
    }

    #[tokio::test]
    async fn test_vm_status_persistence() {
        let db = create_test_db().await;

        // Test all VM statuses
        let statuses = vec![
            VmStatus::Running,
            VmStatus::Stopped,
            VmStatus::Paused,
            VmStatus::Unknown,
        ];

        for (i, status) in statuses.iter().enumerate() {
            let mut vm_config = create_test_vm_config(
                &format!("test-vm-status-{}", i),
                &format!("Status Test {}", i),
            );
            vm_config.status = status.clone();
            db.create_vm(&vm_config).await.expect("Failed to create VM");

            let retrieved = db.get_vm(&format!("test-vm-status-{}", i))
                .await
                .expect("Failed to get VM");
            assert_eq!(retrieved.status, *status);
        }
    }

    #[tokio::test]
    async fn test_vm_architecture_persistence() {
        let db = create_test_db().await;

        let architectures = vec![
            VmArchitecture::X86_64,
            VmArchitecture::Aarch64,
            VmArchitecture::Riscv64,
            VmArchitecture::Ppc64le,
        ];

        for (i, arch) in architectures.iter().enumerate() {
            let mut vm_config = create_test_vm_config(
                &format!("test-vm-arch-{}", i),
                &format!("Arch Test {}", i),
            );
            vm_config.architecture = arch.clone();
            db.create_vm(&vm_config).await.expect("Failed to create VM");

            let retrieved = db.get_vm(&format!("test-vm-arch-{}", i))
                .await
                .expect("Failed to get VM");
            assert_eq!(retrieved.architecture, *arch);
        }
    }

    #[tokio::test]
    async fn test_vm_hypervisor_persistence() {
        let db = create_test_db().await;

        let hypervisors = vec![
            VmHypervisor::Qemu,
            VmHypervisor::Lxd,
            VmHypervisor::Incus,
        ];

        for (i, hypervisor) in hypervisors.iter().enumerate() {
            let mut vm_config = create_test_vm_config(
                &format!("test-vm-hyp-{}", i),
                &format!("Hypervisor Test {}", i),
            );
            vm_config.hypervisor = hypervisor.clone();
            db.create_vm(&vm_config).await.expect("Failed to create VM");

            let retrieved = db.get_vm(&format!("test-vm-hyp-{}", i))
                .await
                .expect("Failed to get VM");
            assert_eq!(retrieved.hypervisor, *hypervisor);
        }
    }
}

/// Audit log database operations
pub mod audit {
    use super::*;
    use crate::audit::{AuditEvent, AuditEventType, AuditSeverity, AuditResult};
    use sqlx::Row;

    #[allow(dead_code)]
    pub async fn log_event(pool: &SqlitePool, event: &AuditEvent) -> Result<()> {
        let event_type = format!("{:?}", event.event_type);
        let severity = format!("{:?}", event.severity);
        let result = format!("{:?}", event.result);

        sqlx::query(
            "INSERT INTO audit_logs (timestamp, event_type, severity, user, source_ip,
             resource, action, result, details, session_id)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(event.timestamp.timestamp())
        .bind(&event_type)
        .bind(&severity)
        .bind(&event.user)
        .bind(&event.source_ip)
        .bind(&event.resource)
        .bind(&event.action)
        .bind(&result)
        .bind(&event.details)
        .bind(&event.session_id)
        .execute(pool)
        .await
        .map_err(|e| horcrux_common::Error::System(format!("Failed to log audit event: {}", e)))?;

        Ok(())
    }

    #[allow(dead_code)]
    pub async fn query_events(
        pool: &SqlitePool,
        event_type: Option<&str>,
        user: Option<&str>,
        severity: Option<&str>,
        limit: usize,
    ) -> Result<Vec<AuditEvent>> {
        let mut query = "SELECT * FROM audit_logs WHERE 1=1".to_string();

        if event_type.is_some() {
            query.push_str(" AND event_type = ?");
        }
        if user.is_some() {
            query.push_str(" AND user = ?");
        }
        if severity.is_some() {
            query.push_str(" AND severity = ?");
        }

        query.push_str(" ORDER BY timestamp DESC LIMIT ?");

        let mut sql_query = sqlx::query(&query);

        if let Some(et) = event_type {
            sql_query = sql_query.bind(et);
        }
        if let Some(u) = user {
            sql_query = sql_query.bind(u);
        }
        if let Some(s) = severity {
            sql_query = sql_query.bind(s);
        }
        sql_query = sql_query.bind(limit as i64);

        let rows = sql_query
            .fetch_all(pool)
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to query audit logs: {}", e)))?;

        let mut events = Vec::new();
        for row in rows {
            if let Ok(event) = row_to_event(&row) {
                events.push(event);
            }
        }

        Ok(events)
    }

    #[allow(dead_code)]
    fn row_to_event(row: &sqlx::sqlite::SqliteRow) -> Result<AuditEvent> {
        let timestamp: i64 = row.get("timestamp");
        let event_type_str: String = row.get("event_type");
        let severity_str: String = row.get("severity");
        let result_str: String = row.get("result");

        // Parse event type (simplified - would need full mapping)
        let event_type = match event_type_str.as_str() {
            "Login" => AuditEventType::Login,
            "Logout" => AuditEventType::Logout,
            "LoginFailed" => AuditEventType::LoginFailed,
            _ => AuditEventType::Login, // Default
        };

        let severity = match severity_str.as_str() {
            "Info" => AuditSeverity::Info,
            "Warning" => AuditSeverity::Warning,
            "Error" => AuditSeverity::Error,
            "Critical" => AuditSeverity::Critical,
            _ => AuditSeverity::Info,
        };

        let result = match result_str.as_str() {
            "Success" => AuditResult::Success,
            "Failure" => AuditResult::Failure,
            "Partial" => AuditResult::Partial,
            _ => AuditResult::Success,
        };

        Ok(AuditEvent {
            timestamp: chrono::DateTime::from_timestamp(timestamp, 0)
                .ok_or_else(|| horcrux_common::Error::System("Invalid timestamp".to_string()))?
                .with_timezone(&chrono::Utc),
            event_type,
            severity,
            user: row.get("user"),
            source_ip: row.get("source_ip"),
            resource: row.get("resource"),
            action: row.get("action"),
            result,
            details: row.get("details"),
            session_id: row.get("session_id"),
        })
    }
}
