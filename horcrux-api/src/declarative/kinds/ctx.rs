//! What the kinds' plans need that is not spec-shaped (`Live::kinds`), and
//! how the caller's identity reaches them.
//!
//! Plans are pure: everything arrives in `Live`. `gather_ctx` collects it once
//! per gather (runtime probes are cached for a few seconds, the rest is a
//! couple of local queries).
//!
//! Who is applying travels in a task-local set by `caller_layer`, a tower
//! layer on the declarative router: every endpoint under it (plan, apply,
//! desired/apply, rollback, gitops sync triggered by a user...) sees the
//! caller without its handler knowing. Work that runs with no HTTP caller
//! (the drift timer, a scheduled sync) has none: it can read everything but
//! cannot change users, groups, roles or ACL, and its plans skip the
//! calling-user checks.
use super::access::CallerInfo;
use super::containers::RuntimeInfo;
use crate::middleware::auth::AuthUser;
use crate::AppState;
use axum::extract::ConnectInfo;
use axum::response::IntoResponse;
use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

#[derive(Debug, Clone, Default)]
pub struct UserPriv {
    pub id: String,
    /// The live password hash (never exported unless asked).
    pub hash: String,
}

/// `Live::kinds`.
#[derive(Debug, Clone, Default)]
pub struct Ctx {
    pub caller: Option<CallerInfo>,
    pub caller_ip: Option<IpAddr>,
    /// The API's own TCP port (never cut off by a firewall change).
    pub api_port: u16,
    pub runtimes: Vec<RuntimeInfo>,
    /// username -> id and hash.
    pub users: BTreeMap<String, UserPriv>,
    /// Built-in roles (name, privileges): not in the exported section, but
    /// the administrator check needs their privileges.
    pub builtin_roles: Vec<(String, Vec<String>)>,
    /// HA group -> number of HA resources using it.
    pub ha_usage: BTreeMap<String, usize>,
}

#[derive(Clone)]
pub struct CallerCtx {
    pub user: AuthUser,
    pub ip: Option<IpAddr>,
    /// `?include_hashes=true` on the request.
    pub include_hashes: bool,
}

tokio::task_local! {
    pub static CALLER: CallerCtx;
}

pub fn caller() -> Option<CallerCtx> {
    CALLER.try_with(|c| c.clone()).ok()
}

/// Tower layer: run the rest of the request knowing who asked and from where.
pub async fn caller_layer(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let user = req.extensions().get::<AuthUser>().cloned();
    let ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip());
    let include_hashes = req
        .uri()
        .query()
        .map(|q| {
            q.split('&')
                .any(|p| matches!(p, "include_hashes=true" | "include_hashes=1"))
        })
        .unwrap_or(false);
    // Password hashes are for administrators: refuse, never silently omit.
    if include_hashes {
        let admin = match &user {
            Some(u) => crate::auth::store::Principal::load(state.database.pool(), &u.user_id)
                .await
                .map(|p| p.is_admin())
                .unwrap_or(false),
            None => false,
        };
        if !admin {
            return crate::error::ApiError::Forbidden(
                "exporting password hashes needs Permissions.Modify on /".into(),
            )
            .into_response();
        }
    }
    match user {
        Some(user) => {
            CALLER
                .scope(
                    CallerCtx {
                        user,
                        ip,
                        include_hashes,
                    },
                    next.run(req),
                )
                .await
        }
        None => next.run(req).await,
    }
}

/// Is the HTTP caller (if any) allowed to read/change access control?
/// No caller (an internal job) = yes for reading.
pub async fn caller_is_admin(state: &Arc<AppState>) -> Option<bool> {
    let c = caller()?;
    Some(
        crate::auth::store::Principal::load(state.database.pool(), &c.user.user_id)
            .await
            .map(|p| p.is_admin())
            .unwrap_or(false),
    )
}

pub async fn gather_ctx(state: &Arc<AppState>) -> Ctx {
    use crate::auth::store;
    let pool = state.database.pool();
    let c = caller();
    let caller_info = match &c {
        Some(c) => Some(CallerInfo {
            user_id: c.user.user_id.clone(),
            username: c.user.username.clone(),
            is_admin: caller_is_admin(state).await.unwrap_or(false),
        }),
        None => None,
    };
    let mut users = BTreeMap::new();
    if let Ok(recs) = store::list_user_records(pool).await {
        for r in recs {
            let hash = store::find_user(pool, &r.id)
                .await
                .ok()
                .flatten()
                .map(|u| u.password_hash)
                .unwrap_or_default();
            users.insert(r.username, UserPriv { id: r.id, hash });
        }
    }
    let builtin_roles = store::list_roles(pool)
        .await
        .map(|v| {
            v.into_iter()
                .filter(|r| r.builtin)
                .map(|r| {
                    (
                        r.name,
                        r.privileges.iter().map(|p| p.name().to_string()).collect(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let runtimes = crate::container::probe::probe_all_cached(false)
        .await
        .into_iter()
        .map(|p| RuntimeInfo {
            runtime: p.runtime,
            available: p.available,
            reason: p.reason,
            message: p.message,
            hint: p.hint,
        })
        .collect();
    Ctx {
        caller: caller_info,
        caller_ip: c.and_then(|c| c.ip),
        api_port: state.config.server.port,
        runtimes,
        users,
        builtin_roles,
        ha_usage: ha_usage(state).await,
    }
}

#[cfg(feature = "cluster")]
async fn ha_usage(state: &Arc<AppState>) -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for r in state.ha_manager.list_resources().await {
        *m.entry(r.group).or_insert(0) += 1;
    }
    m
}

#[cfg(not(feature = "cluster"))]
async fn ha_usage(_: &Arc<AppState>) -> BTreeMap<String, usize> {
    BTreeMap::new()
}
