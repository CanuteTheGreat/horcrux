//! Database access for the access-control subsystem: users, roles, groups,
//! ACLs, realms, second factors, recovery codes, login challenges, settings.
//!
//! This is the one user store. The login handler, the auth middleware, the
//! user-management API and the UI all go through these tables (there used to
//! be a second, in-memory user list that the UI wrote to and login never read).

use super::acl::{self, AclEntry};
use super::privileges;
use horcrux_common::auth::{Permission, Privilege, Role, User};
use horcrux_common::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};
use std::collections::{BTreeSet, HashMap};

fn db_err(context: &str) -> impl Fn(sqlx::Error) -> Error + '_ {
    move |e| Error::System(format!("{}: {}", context, e))
}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

// ---------------------------------------------------------------------------
// Users
// ---------------------------------------------------------------------------

/// A user row as the management API shows it (no password hash).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRecord {
    pub id: String,
    pub username: String,
    pub email: String,
    /// Legacy coarse role column ("admin"/"user"); permissions come from ACLs.
    pub role: String,
    pub realm: String,
    pub enabled: bool,
    pub comment: Option<String>,
    pub last_login: Option<i64>,
    pub tfa_required: bool,
    pub tfa_configured: bool,
    pub groups: Vec<String>,
    /// Roles granted on `/` (shown in the user list).
    pub roles: Vec<String>,
    pub created_at: Option<String>,
}

fn row_to_user(row: &sqlx::sqlite::SqliteRow) -> User {
    User {
        id: row.get("id"),
        username: row.get("username"),
        password_hash: row.get("password_hash"),
        email: row
            .try_get::<Option<String>, _>("email")
            .ok()
            .flatten()
            .unwrap_or_default(),
        role: row.get("role"),
        realm: row.get("realm"),
        enabled: row.get("enabled"),
        roles: Vec::new(),
        comment: row.try_get::<Option<String>, _>("comment").ok().flatten(),
    }
}

/// Any user (enabled or not) by id or username.
pub async fn find_user(pool: &SqlitePool, id_or_name: &str) -> Result<Option<User>> {
    let row = sqlx::query("SELECT * FROM users WHERE id = ? OR username = ? LIMIT 1")
        .bind(id_or_name)
        .bind(id_or_name)
        .fetch_optional(pool)
        .await
        .map_err(db_err("user lookup"))?;
    Ok(row.as_ref().map(row_to_user))
}

pub async fn user_count(pool: &SqlitePool) -> Result<i64> {
    let row = sqlx::query("SELECT COUNT(*) AS n FROM users")
        .fetch_one(pool)
        .await
        .map_err(db_err("count users"))?;
    Ok(row.get("n"))
}

pub async fn user_tfa_required(pool: &SqlitePool, user_id: &str) -> Result<bool> {
    let row = sqlx::query("SELECT tfa_required FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map_err(db_err("user tfa flag"))?;
    Ok(row
        .map(|r| r.get::<i64, _>("tfa_required") != 0)
        .unwrap_or(false))
}

pub async fn list_user_records(pool: &SqlitePool) -> Result<Vec<UserRecord>> {
    let rows = sqlx::query(
        "SELECT u.*, \
            (SELECT COUNT(*) FROM auth_tfa t WHERE t.user_id = u.id AND t.enabled = 1) AS tfa_count \
         FROM users u ORDER BY u.username",
    )
    .fetch_all(pool)
    .await
    .map_err(db_err("list users"))?;
    let memberships = sqlx::query("SELECT group_name, user_id FROM auth_group_members")
        .fetch_all(pool)
        .await
        .map_err(db_err("list memberships"))?;
    let root_acl = sqlx::query(
        "SELECT subject, role FROM auth_acl WHERE subject_type = 'user' AND path = '/' ORDER BY role",
    )
    .fetch_all(pool)
    .await
    .map_err(db_err("list acl"))?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let id: String = r.get("id");
        out.push(UserRecord {
            groups: memberships
                .iter()
                .filter(|m| m.get::<String, _>("user_id") == id)
                .map(|m| m.get("group_name"))
                .collect(),
            roles: root_acl
                .iter()
                .filter(|m| m.get::<String, _>("subject") == id)
                .map(|m| m.get("role"))
                .collect(),
            username: r.get("username"),
            email: r
                .try_get::<Option<String>, _>("email")
                .ok()
                .flatten()
                .unwrap_or_default(),
            role: r.get("role"),
            realm: r.get("realm"),
            enabled: r.get("enabled"),
            comment: r.try_get::<Option<String>, _>("comment").ok().flatten(),
            last_login: r.try_get::<Option<i64>, _>("last_login").ok().flatten(),
            tfa_required: r.get::<i64, _>("tfa_required") != 0,
            tfa_configured: r.get::<i64, _>("tfa_count") > 0,
            created_at: r.try_get::<Option<String>, _>("created_at").ok().flatten(),
            id,
        });
    }
    Ok(out)
}

pub async fn get_user_record(pool: &SqlitePool, id_or_name: &str) -> Result<Option<UserRecord>> {
    let Some(u) = find_user(pool, id_or_name).await? else {
        return Ok(None);
    };
    Ok(list_user_records(pool)
        .await?
        .into_iter()
        .find(|r| r.id == u.id))
}

pub async fn insert_user(pool: &SqlitePool, user: &User) -> Result<()> {
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, email, role, realm, enabled, comment) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&user.id)
    .bind(&user.username)
    .bind(&user.password_hash)
    .bind(&user.email)
    .bind(&user.role)
    .bind(&user.realm)
    .bind(user.enabled)
    .bind(&user.comment)
    .execute(pool)
    .await
    .map_err(|e| {
        if e.to_string().contains("UNIQUE") {
            Error::Conflict(format!("user '{}' already exists", user.username))
        } else {
            Error::System(format!("create user: {}", e))
        }
    })?;
    Ok(())
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct UserUpdate {
    pub email: Option<String>,
    pub comment: Option<String>,
    pub enabled: Option<bool>,
    pub tfa_required: Option<bool>,
}

pub async fn update_user(pool: &SqlitePool, user_id: &str, upd: &UserUpdate) -> Result<()> {
    sqlx::query(
        "UPDATE users SET \
            email = COALESCE(?, email), \
            comment = COALESCE(?, comment), \
            enabled = COALESCE(?, enabled), \
            tfa_required = COALESCE(?, tfa_required), \
            updated_at = CURRENT_TIMESTAMP \
         WHERE id = ?",
    )
    .bind(&upd.email)
    .bind(&upd.comment)
    .bind(upd.enabled)
    .bind(upd.tfa_required)
    .bind(user_id)
    .execute(pool)
    .await
    .map_err(db_err("update user"))?;
    if upd.enabled == Some(false) {
        // a disabled account must lose its live sessions immediately
        sqlx::query("DELETE FROM sessions WHERE user_id = ?")
            .bind(user_id)
            .execute(pool)
            .await
            .map_err(db_err("drop sessions"))?;
    }
    Ok(())
}

pub async fn set_password_hash(pool: &SqlitePool, user_id: &str, hash: &str) -> Result<()> {
    sqlx::query("UPDATE users SET password_hash = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?")
        .bind(hash)
        .bind(user_id)
        .execute(pool)
        .await
        .map_err(db_err("set password"))?;
    Ok(())
}

pub async fn touch_last_login(pool: &SqlitePool, user_id: &str) -> Result<()> {
    sqlx::query("UPDATE users SET last_login = ? WHERE id = ?")
        .bind(now())
        .bind(user_id)
        .execute(pool)
        .await
        .map_err(db_err("last login"))?;
    Ok(())
}

pub async fn delete_user(pool: &SqlitePool, user_id: &str) -> Result<bool> {
    let r = sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(user_id)
        .execute(pool)
        .await
        .map_err(db_err("delete user"))?;
    Ok(r.rows_affected() > 0)
}

pub async fn delete_user_sessions(pool: &SqlitePool, user_id: &str) -> Result<u64> {
    let r = sqlx::query("DELETE FROM sessions WHERE user_id = ?")
        .bind(user_id)
        .execute(pool)
        .await
        .map_err(db_err("delete sessions"))?;
    Ok(r.rows_affected())
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

/// Create a login session; returns its id.
pub async fn create_session(
    pool: &SqlitePool,
    user_id: &str,
    scope: Option<&str>,
    ttl_secs: i64,
    ip: Option<&str>,
    user_agent: Option<&str>,
) -> Result<String> {
    let id = crate::auth::random_token(43);
    sqlx::query(
        "INSERT INTO sessions (id, user_id, expires_at, scope, ip_address, user_agent) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(user_id)
    .bind(now() + ttl_secs)
    .bind(scope)
    .bind(ip)
    .bind(user_agent.map(|u| u.chars().take(256).collect::<String>()))
    .execute(pool)
    .await
    .map_err(db_err("create session"))?;
    Ok(id)
}

pub async fn session_scope(pool: &SqlitePool, session_id: &str) -> Result<Option<String>> {
    let row = sqlx::query("SELECT scope FROM sessions WHERE id = ?")
        .bind(session_id)
        .fetch_optional(pool)
        .await
        .map_err(db_err("session scope"))?;
    Ok(row.and_then(|r| r.get::<Option<String>, _>("scope")))
}

/// Live sessions with client details (`user_id` limits to one user).
pub async fn list_sessions_detailed(
    pool: &SqlitePool,
    user_id: Option<&str>,
) -> Result<Vec<serde_json::Value>> {
    let rows = sqlx::query(
        "SELECT s.id, s.user_id, s.expires_at, s.scope, s.ip_address, s.user_agent, \
                CAST(strftime('%s', s.created_at) AS INTEGER) AS created, u.username, u.realm \
         FROM sessions s JOIN users u ON u.id = s.user_id \
         WHERE s.expires_at >= ? AND (? IS NULL OR s.user_id = ?) ORDER BY s.created_at DESC",
    )
    .bind(now())
    .bind(user_id)
    .bind(user_id)
    .fetch_all(pool)
    .await
    .map_err(db_err("list sessions"))?;
    Ok(rows
        .iter()
        .map(|r| {
            let id: String = r.get("id");
            let expires: i64 = r.get("expires_at");
            serde_json::json!({
                "id": id,
                "session_id": id,
                "user_id": r.get::<String, _>("user_id"),
                "username": r.get::<String, _>("username"),
                "realm": r.get::<String, _>("realm"),
                "created": r.try_get::<i64, _>("created").unwrap_or(0),
                "expires": expires,
                "expires_at": chrono::DateTime::from_timestamp(expires, 0).map(|d| d.to_rfc3339()),
                "scope": r.get::<Option<String>, _>("scope"),
                "ip_address": r.get::<Option<String>, _>("ip_address"),
                "user_agent": r.get::<Option<String>, _>("user_agent"),
            })
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Roles
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleRecord {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub privileges: Vec<Privilege>,
    #[serde(default)]
    pub builtin: bool,
}

impl RoleRecord {
    /// The UI's legacy shape: one permission entry on `/`.
    pub fn to_role(&self) -> Role {
        Role {
            name: self.name.clone(),
            description: self.description.clone(),
            permissions: vec![Permission {
                path: "/".into(),
                privileges: self.privileges.clone(),
            }],
        }
    }
}

/// (Re)create the built-in roles and, once, convert pre-RBAC users: the old
/// `users.role = 'admin'` becomes an `Administrator` grant on `/`. Any other
/// pre-existing account (which could only have come from the formerly public
/// self-registration) gets no grants; an administrator decides what it may do.
pub async fn ensure_builtin_roles(pool: &SqlitePool) -> Result<()> {
    for (name, desc, privs) in privileges::builtin_roles() {
        let json = serde_json::to_string(&privs).unwrap_or_else(|_| "[]".into());
        sqlx::query(
            "INSERT INTO auth_roles (name, description, privileges, builtin) VALUES (?, ?, ?, 1) \
             ON CONFLICT(name) DO UPDATE SET description = excluded.description, \
                 privileges = excluded.privileges, builtin = 1",
        )
        .bind(name)
        .bind(desc)
        .bind(json)
        .execute(pool)
        .await
        .map_err(db_err("seed roles"))?;
    }
    if get_setting(pool, "legacy_roles_migrated").await?.is_none() {
        let admins = sqlx::query("SELECT id, username FROM users WHERE role = 'admin'")
            .fetch_all(pool)
            .await
            .map_err(db_err("legacy admins"))?;
        for a in &admins {
            let id: String = a.get("id");
            add_acl(pool, "/", "user", &id, "Administrator", true).await?;
            tracing::info!(
                "RBAC migration: granted Administrator on / to existing admin '{}'",
                a.get::<String, _>("username")
            );
        }
        let others = sqlx::query("SELECT username FROM users WHERE role != 'admin'")
            .fetch_all(pool)
            .await
            .map_err(db_err("legacy users"))?;
        for o in others {
            tracing::warn!(
                "RBAC migration: existing account '{}' has no permissions until an administrator grants a role",
                o.get::<String, _>("username")
            );
        }
        set_setting(pool, "legacy_roles_migrated", "1").await?;
    }
    Ok(())
}

pub async fn list_roles(pool: &SqlitePool) -> Result<Vec<RoleRecord>> {
    let rows = sqlx::query("SELECT * FROM auth_roles ORDER BY builtin DESC, name")
        .fetch_all(pool)
        .await
        .map_err(db_err("list roles"))?;
    Ok(rows
        .iter()
        .map(|r| RoleRecord {
            name: r.get("name"),
            description: r.get("description"),
            privileges: parse_privs(&r.get::<String, _>("privileges")),
            builtin: r.get::<i64, _>("builtin") != 0,
        })
        .collect())
}

fn parse_privs(json: &str) -> Vec<Privilege> {
    let names: Vec<String> = serde_json::from_str(json).unwrap_or_default();
    names.iter().filter_map(|n| Privilege::parse(n)).collect()
}

pub async fn get_role(pool: &SqlitePool, name: &str) -> Result<Option<RoleRecord>> {
    Ok(list_roles(pool).await?.into_iter().find(|r| r.name == name))
}

pub async fn upsert_role(pool: &SqlitePool, role: &RoleRecord, create: bool) -> Result<()> {
    if let Some(existing) = get_role(pool, &role.name).await? {
        if existing.builtin {
            return Err(Error::Validation(format!(
                "'{}' is a built-in role and can't be changed",
                role.name
            )));
        }
        if create {
            return Err(Error::Conflict(format!(
                "role '{}' already exists",
                role.name
            )));
        }
    } else if !create {
        return Err(Error::NotFound(format!("role '{}' not found", role.name)));
    }
    if role.name.trim().is_empty()
        || !role
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    {
        return Err(Error::Validation(
            "role names are letters, digits, _ - . only".into(),
        ));
    }
    let json = serde_json::to_string(&role.privileges).unwrap_or_else(|_| "[]".into());
    sqlx::query(
        "INSERT INTO auth_roles (name, description, privileges, builtin) VALUES (?, ?, ?, 0) \
         ON CONFLICT(name) DO UPDATE SET description = excluded.description, privileges = excluded.privileges",
    )
    .bind(&role.name)
    .bind(&role.description)
    .bind(json)
    .execute(pool)
    .await
    .map_err(db_err("save role"))?;
    Ok(())
}

pub async fn delete_role(pool: &SqlitePool, name: &str) -> Result<()> {
    match get_role(pool, name).await? {
        None => Err(Error::NotFound(format!("role '{}' not found", name))),
        Some(r) if r.builtin => Err(Error::Validation(format!(
            "'{}' is a built-in role and can't be deleted",
            name
        ))),
        Some(_) => {
            sqlx::query("DELETE FROM auth_roles WHERE name = ?")
                .bind(name)
                .execute(pool)
                .await
                .map_err(db_err("delete role"))?;
            Ok(())
        }
    }
}

pub async fn role_map(pool: &SqlitePool) -> Result<HashMap<String, Vec<Privilege>>> {
    Ok(list_roles(pool)
        .await?
        .into_iter()
        .map(|r| (r.name, r.privileges))
        .collect())
}

// ---------------------------------------------------------------------------
// Groups
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupRecord {
    pub name: String,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub members: Vec<String>,
}

pub async fn list_groups(pool: &SqlitePool) -> Result<Vec<GroupRecord>> {
    let groups = sqlx::query("SELECT * FROM auth_groups ORDER BY name")
        .fetch_all(pool)
        .await
        .map_err(db_err("list groups"))?;
    let members = sqlx::query(
        "SELECT m.group_name, u.username FROM auth_group_members m JOIN users u ON u.id = m.user_id ORDER BY u.username",
    )
    .fetch_all(pool)
    .await
    .map_err(db_err("list members"))?;
    Ok(groups
        .iter()
        .map(|g| {
            let name: String = g.get("name");
            GroupRecord {
                members: members
                    .iter()
                    .filter(|m| m.get::<String, _>("group_name") == name)
                    .map(|m| m.get("username"))
                    .collect(),
                comment: g.get("comment"),
                name,
            }
        })
        .collect())
}

pub async fn create_group(pool: &SqlitePool, name: &str, comment: Option<&str>) -> Result<()> {
    if name.trim().is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    {
        return Err(Error::Validation(
            "group names are letters, digits, _ - . only".into(),
        ));
    }
    sqlx::query("INSERT INTO auth_groups (name, comment) VALUES (?, ?)")
        .bind(name)
        .bind(comment)
        .execute(pool)
        .await
        .map_err(|e| {
            if e.to_string().contains("UNIQUE") {
                Error::Conflict(format!("group '{}' already exists", name))
            } else {
                Error::System(format!("create group: {}", e))
            }
        })?;
    Ok(())
}

pub async fn delete_group(pool: &SqlitePool, name: &str) -> Result<bool> {
    let r = sqlx::query("DELETE FROM auth_groups WHERE name = ?")
        .bind(name)
        .execute(pool)
        .await
        .map_err(db_err("delete group"))?;
    Ok(r.rows_affected() > 0)
}

pub async fn set_group_members(pool: &SqlitePool, name: &str, user_ids: &[String]) -> Result<()> {
    let mut tx = pool.begin().await.map_err(db_err("begin"))?;
    sqlx::query("DELETE FROM auth_group_members WHERE group_name = ?")
        .bind(name)
        .execute(&mut *tx)
        .await
        .map_err(db_err("clear members"))?;
    for uid in user_ids {
        sqlx::query("INSERT INTO auth_group_members (group_name, user_id) VALUES (?, ?)")
            .bind(name)
            .bind(uid)
            .execute(&mut *tx)
            .await
            .map_err(db_err("add member"))?;
    }
    tx.commit().await.map_err(db_err("commit"))?;
    Ok(())
}

pub async fn add_group_member(pool: &SqlitePool, name: &str, user_id: &str) -> Result<()> {
    sqlx::query("INSERT OR IGNORE INTO auth_group_members (group_name, user_id) VALUES (?, ?)")
        .bind(name)
        .bind(user_id)
        .execute(pool)
        .await
        .map_err(db_err("add member"))?;
    Ok(())
}

pub async fn user_groups(pool: &SqlitePool, user_id: &str) -> Result<Vec<String>> {
    let rows = sqlx::query("SELECT group_name FROM auth_group_members WHERE user_id = ?")
        .bind(user_id)
        .fetch_all(pool)
        .await
        .map_err(db_err("user groups"))?;
    Ok(rows.iter().map(|r| r.get("group_name")).collect())
}

// ---------------------------------------------------------------------------
// ACL
// ---------------------------------------------------------------------------

fn row_to_acl(r: &sqlx::sqlite::SqliteRow) -> AclEntry {
    AclEntry {
        id: r.get("id"),
        path: r.get("path"),
        subject_type: r.get("subject_type"),
        subject: r.get("subject"),
        role: r.get("role"),
        propagate: r.get::<i64, _>("propagate") != 0,
    }
}

pub async fn list_acl(pool: &SqlitePool) -> Result<Vec<AclEntry>> {
    let rows = sqlx::query("SELECT * FROM auth_acl ORDER BY path, subject_type, subject, role")
        .fetch_all(pool)
        .await
        .map_err(db_err("list acl"))?;
    Ok(rows.iter().map(row_to_acl).collect())
}

pub async fn add_acl(
    pool: &SqlitePool,
    path: &str,
    subject_type: &str,
    subject: &str,
    role: &str,
    propagate: bool,
) -> Result<()> {
    if subject_type != "user" && subject_type != "group" {
        return Err(Error::Validation(
            "subject_type must be 'user' or 'group'".into(),
        ));
    }
    if get_role(pool, role).await?.is_none() {
        return Err(Error::Validation(format!("unknown role '{}'", role)));
    }
    sqlx::query(
        "INSERT INTO auth_acl (path, subject_type, subject, role, propagate) VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT(path, subject_type, subject, role) DO UPDATE SET propagate = excluded.propagate",
    )
    .bind(acl::normalize_path(path))
    .bind(subject_type)
    .bind(subject)
    .bind(role)
    .bind(propagate)
    .execute(pool)
    .await
    .map_err(db_err("add acl"))?;
    Ok(())
}

pub async fn delete_acl(
    pool: &SqlitePool,
    path: &str,
    subject_type: &str,
    subject: &str,
    role: Option<&str>,
) -> Result<u64> {
    let r = sqlx::query(
        "DELETE FROM auth_acl WHERE path = ? AND subject_type = ? AND subject = ? AND (? IS NULL OR role = ?)",
    )
    .bind(acl::normalize_path(path))
    .bind(subject_type)
    .bind(subject)
    .bind(role)
    .bind(role)
    .execute(pool)
    .await
    .map_err(db_err("delete acl"))?;
    Ok(r.rows_affected())
}

/// Everything needed to answer permission questions for one user.
#[derive(Debug, Clone)]
pub struct Principal {
    pub user_id: String,
    pub groups: Vec<String>,
    pub acl: Vec<AclEntry>,
    pub roles: HashMap<String, Vec<Privilege>>,
}

impl Principal {
    pub async fn load(pool: &SqlitePool, user_id: &str) -> Result<Self> {
        let groups = user_groups(pool, user_id).await?;
        let mut acl = Vec::new();
        let rows =
            sqlx::query("SELECT * FROM auth_acl WHERE subject_type = 'user' AND subject = ?")
                .bind(user_id)
                .fetch_all(pool)
                .await
                .map_err(db_err("user acl"))?;
        acl.extend(rows.iter().map(row_to_acl));
        for g in &groups {
            let rows =
                sqlx::query("SELECT * FROM auth_acl WHERE subject_type = 'group' AND subject = ?")
                    .bind(g)
                    .fetch_all(pool)
                    .await
                    .map_err(db_err("group acl"))?;
            acl.extend(rows.iter().map(row_to_acl));
        }
        Ok(Self {
            user_id: user_id.to_string(),
            groups,
            acl,
            roles: role_map(pool).await?,
        })
    }

    pub fn privileges(&self, path: &str) -> BTreeSet<Privilege> {
        acl::effective_privileges(&self.acl, &self.roles, &self.user_id, &self.groups, path)
    }

    pub fn has(&self, privilege: Privilege, path: &str) -> bool {
        self.privileges(path).contains(&privilege)
    }

    pub fn has_below(&self, privilege: Privilege, prefix: &str) -> bool {
        acl::has_privilege_below(
            &self.acl,
            &self.roles,
            &self.user_id,
            &self.groups,
            prefix,
            privilege,
        )
    }

    /// Whether this principal is effectively an administrator (can grant
    /// permissions on `/`).
    pub fn is_admin(&self) -> bool {
        self.has(Privilege::PermissionsModify, "/")
    }
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

pub async fn get_setting(pool: &SqlitePool, key: &str) -> Result<Option<String>> {
    let row = sqlx::query("SELECT value FROM auth_settings WHERE key = ?")
        .bind(key)
        .fetch_optional(pool)
        .await
        .map_err(db_err("get setting"))?;
    Ok(row.map(|r| r.get("value")))
}

pub async fn set_setting(pool: &SqlitePool, key: &str, value: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO auth_settings (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await
    .map_err(db_err("set setting"))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Second factors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TfaEntry {
    pub id: String,
    pub user_id: String,
    /// `totp` or `webauthn`
    pub kind: String,
    pub name: String,
    #[serde(skip_serializing)]
    pub data: String,
    pub enabled: bool,
    pub sign_count: i64,
    pub created_at: i64,
    pub last_used: Option<i64>,
}

fn row_to_tfa(r: &sqlx::sqlite::SqliteRow) -> TfaEntry {
    TfaEntry {
        id: r.get("id"),
        user_id: r.get("user_id"),
        kind: r.get("kind"),
        name: r.get("name"),
        data: r.get("data"),
        enabled: r.get::<i64, _>("enabled") != 0,
        sign_count: r.get("sign_count"),
        created_at: r.get("created_at"),
        last_used: r.get("last_used"),
    }
}

pub async fn list_tfa(pool: &SqlitePool, user_id: &str) -> Result<Vec<TfaEntry>> {
    let rows = sqlx::query("SELECT * FROM auth_tfa WHERE user_id = ? ORDER BY created_at")
        .bind(user_id)
        .fetch_all(pool)
        .await
        .map_err(db_err("list tfa"))?;
    Ok(rows.iter().map(row_to_tfa).collect())
}

pub async fn add_tfa(
    pool: &SqlitePool,
    user_id: &str,
    kind: &str,
    name: &str,
    data: &str,
) -> Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO auth_tfa (id, user_id, kind, name, data, enabled, sign_count, created_at) \
         VALUES (?, ?, ?, ?, ?, 1, 0, ?)",
    )
    .bind(&id)
    .bind(user_id)
    .bind(kind)
    .bind(name)
    .bind(data)
    .bind(now())
    .execute(pool)
    .await
    .map_err(db_err("add tfa"))?;
    Ok(id)
}

pub async fn delete_tfa(pool: &SqlitePool, user_id: &str, id: &str) -> Result<bool> {
    let r = sqlx::query("DELETE FROM auth_tfa WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(user_id)
        .execute(pool)
        .await
        .map_err(db_err("delete tfa"))?;
    Ok(r.rows_affected() > 0)
}

pub async fn delete_all_tfa(pool: &SqlitePool, user_id: &str) -> Result<()> {
    sqlx::query("DELETE FROM auth_tfa WHERE user_id = ?")
        .bind(user_id)
        .execute(pool)
        .await
        .map_err(db_err("reset tfa"))?;
    sqlx::query("DELETE FROM auth_recovery_codes WHERE user_id = ?")
        .bind(user_id)
        .execute(pool)
        .await
        .map_err(db_err("reset recovery"))?;
    Ok(())
}

pub async fn mark_tfa_used(pool: &SqlitePool, id: &str, sign_count: Option<i64>) -> Result<()> {
    sqlx::query(
        "UPDATE auth_tfa SET last_used = ?, sign_count = COALESCE(?, sign_count) WHERE id = ?",
    )
    .bind(now())
    .bind(sign_count)
    .bind(id)
    .execute(pool)
    .await
    .map_err(db_err("tfa used"))?;
    Ok(())
}

/// Store the last accepted TOTP time step so a code can't be replayed.
pub async fn set_totp_last_step(pool: &SqlitePool, id: &str, step: i64) -> Result<()> {
    sqlx::query("UPDATE auth_tfa SET sign_count = ?, last_used = ? WHERE id = ?")
        .bind(step)
        .bind(now())
        .bind(id)
        .execute(pool)
        .await
        .map_err(db_err("totp step"))?;
    Ok(())
}

pub fn hash_recovery_code(code: &str) -> String {
    let normalized: String = code
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    hex::encode(Sha256::digest(normalized.as_bytes()))
}

pub async fn replace_recovery_codes(
    pool: &SqlitePool,
    user_id: &str,
    codes: &[String],
) -> Result<()> {
    let mut tx = pool.begin().await.map_err(db_err("begin"))?;
    sqlx::query("DELETE FROM auth_recovery_codes WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(db_err("clear recovery"))?;
    for c in codes {
        sqlx::query("INSERT INTO auth_recovery_codes (user_id, code_hash) VALUES (?, ?)")
            .bind(user_id)
            .bind(hash_recovery_code(c))
            .execute(&mut *tx)
            .await
            .map_err(db_err("add recovery"))?;
    }
    tx.commit().await.map_err(db_err("commit"))?;
    Ok(())
}

/// Consume a recovery code. True only the first time a valid code is used.
pub async fn use_recovery_code(pool: &SqlitePool, user_id: &str, code: &str) -> Result<bool> {
    let r = sqlx::query(
        "UPDATE auth_recovery_codes SET used_at = ? WHERE user_id = ? AND code_hash = ? AND used_at IS NULL",
    )
    .bind(now())
    .bind(user_id)
    .bind(hash_recovery_code(code))
    .execute(pool)
    .await
    .map_err(db_err("use recovery"))?;
    Ok(r.rows_affected() == 1)
}

pub async fn recovery_codes_remaining(pool: &SqlitePool, user_id: &str) -> Result<i64> {
    let row = sqlx::query(
        "SELECT COUNT(*) AS n FROM auth_recovery_codes WHERE user_id = ? AND used_at IS NULL",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .map_err(db_err("count recovery"))?;
    Ok(row.get("n"))
}

// ---------------------------------------------------------------------------
// Challenges (pending TFA logins, WebAuthn ceremonies, OIDC state)
// ---------------------------------------------------------------------------

pub async fn put_challenge(
    pool: &SqlitePool,
    kind: &str,
    user_id: Option<&str>,
    data: &serde_json::Value,
    ttl_secs: i64,
) -> Result<String> {
    let id = crate::auth::random_token(32);
    sqlx::query("DELETE FROM auth_challenges WHERE expires_at < ?")
        .bind(now())
        .execute(pool)
        .await
        .map_err(db_err("expire challenges"))?;
    sqlx::query(
        "INSERT INTO auth_challenges (id, kind, user_id, data, expires_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(kind)
    .bind(user_id)
    .bind(data.to_string())
    .bind(now() + ttl_secs)
    .execute(pool)
    .await
    .map_err(db_err("store challenge"))?;
    Ok(id)
}

/// Fetch a live challenge without consuming it.
pub async fn peek_challenge(
    pool: &SqlitePool,
    id: &str,
    kind: &str,
) -> Result<Option<(Option<String>, serde_json::Value)>> {
    let row = sqlx::query(
        "SELECT user_id, data FROM auth_challenges WHERE id = ? AND kind = ? AND expires_at >= ?",
    )
    .bind(id)
    .bind(kind)
    .bind(now())
    .fetch_optional(pool)
    .await
    .map_err(db_err("load challenge"))?;
    Ok(row.map(|r| {
        (
            r.get::<Option<String>, _>("user_id"),
            serde_json::from_str(&r.get::<String, _>("data")).unwrap_or(serde_json::Value::Null),
        )
    }))
}

pub async fn update_challenge(pool: &SqlitePool, id: &str, data: &serde_json::Value) -> Result<()> {
    sqlx::query("UPDATE auth_challenges SET data = ? WHERE id = ?")
        .bind(data.to_string())
        .bind(id)
        .execute(pool)
        .await
        .map_err(db_err("update challenge"))?;
    Ok(())
}

/// Fetch and delete a live challenge (single use).
pub async fn take_challenge(
    pool: &SqlitePool,
    id: &str,
    kind: &str,
) -> Result<Option<(Option<String>, serde_json::Value)>> {
    let got = peek_challenge(pool, id, kind).await?;
    if got.is_some() {
        let r = sqlx::query("DELETE FROM auth_challenges WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await
            .map_err(db_err("consume challenge"))?;
        if r.rows_affected() == 0 {
            // raced with another request redeeming the same challenge
            return Ok(None);
        }
    }
    Ok(got)
}

// ---------------------------------------------------------------------------
// Realms
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealmRecord {
    pub name: String,
    /// `pam`, `ldap`, `ad` or `openid`
    pub realm_type: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub comment: Option<String>,
    /// Type-specific settings (see `auth::realms`).
    #[serde(default)]
    pub config: serde_json::Value,
    /// Group new users from this realm are added to on first login.
    #[serde(default)]
    pub default_group: Option<String>,
    #[serde(default)]
    pub tfa_required: bool,
}

fn yes() -> bool {
    true
}

pub async fn list_realms(pool: &SqlitePool) -> Result<Vec<RealmRecord>> {
    let rows = sqlx::query("SELECT * FROM auth_realms ORDER BY name")
        .fetch_all(pool)
        .await
        .map_err(db_err("list realms"))?;
    Ok(rows
        .iter()
        .map(|r| RealmRecord {
            name: r.get("name"),
            realm_type: r.get("realm_type"),
            enabled: r.get::<i64, _>("enabled") != 0,
            comment: r.get("comment"),
            config: serde_json::from_str(&r.get::<String, _>("config")).unwrap_or_default(),
            default_group: r.get("default_group"),
            tfa_required: r.get::<i64, _>("tfa_required") != 0,
        })
        .collect())
}

pub async fn get_realm(pool: &SqlitePool, name: &str) -> Result<Option<RealmRecord>> {
    Ok(list_realms(pool)
        .await?
        .into_iter()
        .find(|r| r.name == name))
}

pub async fn upsert_realm(pool: &SqlitePool, realm: &RealmRecord) -> Result<()> {
    sqlx::query(
        "INSERT INTO auth_realms (name, realm_type, enabled, comment, config, default_group, tfa_required) \
         VALUES (?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(name) DO UPDATE SET realm_type = excluded.realm_type, enabled = excluded.enabled, \
            comment = excluded.comment, config = excluded.config, default_group = excluded.default_group, \
            tfa_required = excluded.tfa_required",
    )
    .bind(&realm.name)
    .bind(&realm.realm_type)
    .bind(realm.enabled)
    .bind(&realm.comment)
    .bind(realm.config.to_string())
    .bind(&realm.default_group)
    .bind(realm.tfa_required)
    .execute(pool)
    .await
    .map_err(db_err("save realm"))?;
    Ok(())
}

pub async fn delete_realm(pool: &SqlitePool, name: &str) -> Result<bool> {
    let r = sqlx::query("DELETE FROM auth_realms WHERE name = ?")
        .bind(name)
        .execute(pool)
        .await
        .map_err(db_err("delete realm"))?;
    Ok(r.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn db() -> SqlitePool {
        let db = crate::db::Database::new("sqlite::memory:").await.unwrap();
        db.migrate().await.unwrap();
        let pool = db.pool().clone();
        ensure_builtin_roles(&pool).await.unwrap();
        pool
    }

    fn user(name: &str, role: &str) -> User {
        User {
            id: uuid::Uuid::new_v4().to_string(),
            username: name.into(),
            password_hash: "x".into(),
            email: format!("{}@example.com", name),
            role: role.into(),
            realm: "local".into(),
            enabled: true,
            roles: vec![],
            comment: None,
        }
    }

    #[tokio::test]
    async fn builtin_roles_are_seeded_and_immutable() {
        let pool = db().await;
        let roles = list_roles(&pool).await.unwrap();
        assert!(roles.iter().any(|r| r.name == "Administrator" && r.builtin));
        let err = delete_role(&pool, "Administrator").await.unwrap_err();
        assert!(matches!(err, Error::Validation(_)));
        let custom = RoleRecord {
            name: "Backup-Op".into(),
            description: "d".into(),
            privileges: vec![Privilege::VmBackup, Privilege::VmAudit],
            builtin: false,
        };
        upsert_role(&pool, &custom, true).await.unwrap();
        assert_eq!(
            get_role(&pool, "Backup-Op")
                .await
                .unwrap()
                .unwrap()
                .privileges
                .len(),
            2
        );
        delete_role(&pool, "Backup-Op").await.unwrap();
    }

    #[tokio::test]
    async fn acl_grants_flow_through_groups_and_are_dropped_with_the_user() {
        let pool = db().await;
        let alice = user("alice", "user");
        insert_user(&pool, &alice).await.unwrap();
        create_group(&pool, "ops", None).await.unwrap();
        add_group_member(&pool, "ops", &alice.id).await.unwrap();
        add_acl(&pool, "/vms", "group", "ops", "VMUser", true)
            .await
            .unwrap();
        let p = Principal::load(&pool, &alice.id).await.unwrap();
        assert!(p.has(Privilege::VmPowerMgmt, "/vms/1"));
        assert!(!p.has(Privilege::VmAllocate, "/vms/1"));
        assert!(!p.has(Privilege::SysAudit, "/"));

        add_acl(&pool, "/", "user", &alice.id, "Auditor", true)
            .await
            .unwrap();
        assert_eq!(list_acl(&pool).await.unwrap().len(), 2);
        delete_user(&pool, &alice.id).await.unwrap();
        // the user's own grant went with the account
        assert_eq!(list_acl(&pool).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn recovery_codes_are_single_use() {
        let pool = db().await;
        let u = user("bob", "user");
        insert_user(&pool, &u).await.unwrap();
        replace_recovery_codes(&pool, &u.id, &["abcd-efgh".into(), "ijkl-mnop".into()])
            .await
            .unwrap();
        assert_eq!(recovery_codes_remaining(&pool, &u.id).await.unwrap(), 2);
        assert!(use_recovery_code(&pool, &u.id, "ABCD EFGH").await.unwrap());
        assert!(!use_recovery_code(&pool, &u.id, "abcd-efgh").await.unwrap());
        assert!(!use_recovery_code(&pool, &u.id, "nope-nope").await.unwrap());
        assert_eq!(recovery_codes_remaining(&pool, &u.id).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn challenges_are_single_use() {
        let pool = db().await;
        let id = put_challenge(&pool, "tfa", Some("u"), &serde_json::json!({"a": 1}), 60)
            .await
            .unwrap();
        assert!(take_challenge(&pool, &id, "other").await.unwrap().is_none());
        let (uid, data) = take_challenge(&pool, &id, "tfa").await.unwrap().unwrap();
        assert_eq!(uid.as_deref(), Some("u"));
        assert_eq!(data["a"], 1);
        assert!(take_challenge(&pool, &id, "tfa").await.unwrap().is_none());
        let expired = put_challenge(&pool, "tfa", None, &serde_json::json!({}), -1)
            .await
            .unwrap();
        assert!(take_challenge(&pool, &expired, "tfa")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn legacy_admin_gets_administrator_once() {
        let db = crate::db::Database::new("sqlite::memory:").await.unwrap();
        db.migrate().await.unwrap();
        let pool = db.pool().clone();
        let admin = user("root", "admin");
        let joe = user("joe", "user");
        insert_user(&pool, &admin).await.unwrap();
        insert_user(&pool, &joe).await.unwrap();
        ensure_builtin_roles(&pool).await.unwrap();
        ensure_builtin_roles(&pool).await.unwrap();
        let acl = list_acl(&pool).await.unwrap();
        assert_eq!(acl.len(), 1);
        assert_eq!(acl[0].subject, admin.id);
        assert!(Principal::load(&pool, &admin.id).await.unwrap().is_admin());
        assert!(Principal::load(&pool, &joe.id)
            .await
            .unwrap()
            .privileges("/")
            .is_empty());
    }
}
