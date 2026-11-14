//! Boot environments (ZFS): list them, snapshot, pin, roll back, destroy. A thin layer over `horcrux-be`
//! (shipped in `sys-apps/horcrux-tools`), which owns all the logic and works identically on OpenRC and systemd
//! (it never touches the init system: Portage's bashrc takes the pre-upgrade snapshot, the boot menu is GRUB).
//! Design: docs/design/boot-environments.md.
//!
//! * `GET    /api/system/boot-environments`                      environments, snapshots, /boot space, data-dataset check
//! * `POST   /api/system/boot-environments/snapshot`             `{name?}`
//! * `POST   /api/system/boot-environments/rollback`             `{snapshot?, name?}` (no snapshot = the newest pre-upgrade one)
//! * `POST   /api/system/boot-environments/:name/activate`       boot it next time
//! * `POST   /api/system/boot-environments/pin`                  `{target: "be" | "be@snap", pinned: bool}`
//! * `DELETE /api/system/boot-environments/:name`
//! * `DELETE /api/system/boot-environments/snapshots/:be/:snap`
//!
//! A node without `horcrux-be` or without a ZFS root answers a structured 424, never a 500. Nothing here reboots.
use super::{run_secs, user_of, which};
use crate::error::ApiError;
use crate::AppState;
use axum::{
    extract::Path,
    routing::{delete, get, post},
    Extension, Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

type AuthExt = Option<Extension<crate::middleware::auth::AuthUser>>;
type Res<T> = Result<Json<T>, ApiError>;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/system/boot-environments", get(list))
        .route("/api/system/boot-environments/snapshot", post(snapshot))
        .route("/api/system/boot-environments/rollback", post(rollback))
        .route("/api/system/boot-environments/pin", post(pin))
        .route(
            "/api/system/boot-environments/:name/activate",
            post(activate),
        )
        .route("/api/system/boot-environments/:name", delete(destroy))
        .route(
            "/api/system/boot-environments/snapshots/:be/:snap",
            delete(destroy_snapshot),
        )
}

/// Same rule as `horcrux-be` (liveinst/be.py `valid_name`): nothing here ever reaches a shell, but names end up in ZFS dataset names.
pub fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 41
        && n.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

fn name_or_400(n: &str, what: &str) -> Result<(), ApiError> {
    if valid_name(n) {
        Ok(())
    } else {
        Err(ApiError::BadRequest(format!(
            "{what} must be 1-41 letters, digits, '_', '.' or '-' and start with a letter or digit"
        )))
    }
}

fn target_or_400(t: &str) -> Result<(), ApiError> {
    let mut it = t.splitn(2, '@');
    name_or_400(it.next().unwrap_or(""), "boot environment")?;
    if let Some(s) = it.next() {
        name_or_400(s, "snapshot")?;
    }
    Ok(())
}

fn not_available(reason: &'static str, message: &str, hint: &str) -> ApiError {
    ApiError::NotAvailable {
        reason,
        message: message.into(),
        hint: hint.into(),
    }
}

fn need_tool() -> Result<(), ApiError> {
    if which("horcrux-be").is_none() {
        return Err(not_available(
            "boot-environments-tool-missing",
            "The boot environment tool (horcrux-be) isn't installed on this node.",
            "Install sys-apps/horcrux-tools on a Horcrux host installed on ZFS.",
        ));
    }
    Ok(())
}

fn no_zfs_root() -> ApiError {
    not_available(
        "no-zfs-boot-environments",
        "This node isn't booted from a ZFS boot environment (rpool/ROOT/...), so there is nothing to roll back to.",
        "Boot environments exist on Horcrux hosts installed with the ZFS root option; other layouts can use backups instead.",
    )
}

/// Run `horcrux-be`; a failure is a 409 with its message (the CLI's messages are written for people).
async fn be(args: &[&str]) -> Result<String, ApiError> {
    need_tool()?;
    run_secs("horcrux-be", args, 300)
        .await
        .map_err(|e| ApiError::Conflict(e.replacen("horcrux-be ", "", 1)))
}

async fn list() -> Res<Value> {
    need_tool()?;
    let out = run_secs("horcrux-be", &["list", "--json"], 60)
        .await
        .map_err(|_| no_zfs_root())?;
    let v: Value = serde_json::from_str(out.trim())
        .map_err(|e| ApiError::Internal(format!("horcrux-be list --json: {e}")))?;
    if v.get("running").is_none_or(Value::is_null) && v.get("active").is_none_or(Value::is_null) {
        return Err(no_zfs_root());
    }
    Ok(Json(v))
}

#[derive(Deserialize, Default)]
struct SnapshotReq {
    name: Option<String>,
}

async fn snapshot(u: AuthExt, body: Option<Json<SnapshotReq>>) -> Res<Value> {
    let name = body.and_then(|b| b.0.name).filter(|n| !n.is_empty());
    let mut args = vec!["snapshot"];
    if let Some(n) = name.as_deref() {
        name_or_400(n, "snapshot name")?;
        args.push(n);
    }
    let out = be(&args).await?;
    tracing::info!(user = %user_of(&u), "boot environment snapshot: {}", out.trim());
    Ok(Json(json!({ "ok": true, "message": out.trim() })))
}

#[derive(Deserialize, Default)]
struct RollbackReq {
    snapshot: Option<String>,
    name: Option<String>,
}

async fn rollback(u: AuthExt, body: Option<Json<RollbackReq>>) -> Res<Value> {
    let b = body.map(|b| b.0).unwrap_or_default();
    let mut args = vec!["rollback"];
    if let Some(s) = b.snapshot.as_deref().filter(|s| !s.is_empty()) {
        if !s.contains('@') {
            return Err(ApiError::BadRequest(
                "snapshot must be BOOT_ENVIRONMENT@SNAPSHOT".into(),
            ));
        }
        target_or_400(s)?;
        args.push(s);
    }
    if let Some(n) = b.name.as_deref().filter(|n| !n.is_empty()) {
        name_or_400(n, "name")?;
        args.push("--name");
        args.push(n);
    }
    let out = be(&args).await?;
    tracing::info!(user = %user_of(&u), "boot environment rollback: {}", out.trim().replace('\n', " | "));
    Ok(Json(json!({
        "ok": true,
        "message": out.trim(),
        "reboot_required": true,
    })))
}

async fn activate(u: AuthExt, Path(name): Path<String>) -> Res<Value> {
    name_or_400(&name, "boot environment")?;
    let out = be(&["activate", &name]).await?;
    tracing::info!(user = %user_of(&u), "boot environment activate {name}");
    Ok(Json(
        json!({ "ok": true, "message": out.trim(), "reboot_required": true }),
    ))
}

#[derive(Deserialize)]
struct PinReq {
    target: String,
    pinned: bool,
}

async fn pin(u: AuthExt, Json(b): Json<PinReq>) -> Res<Value> {
    target_or_400(&b.target)?;
    let out = be(&[if b.pinned { "pin" } else { "unpin" }, &b.target]).await?;
    tracing::info!(user = %user_of(&u), "boot environment {} {}", if b.pinned { "pin" } else { "unpin" }, b.target);
    Ok(Json(json!({ "ok": true, "message": out.trim() })))
}

async fn destroy(u: AuthExt, Path(name): Path<String>) -> Res<Value> {
    name_or_400(&name, "boot environment")?;
    be(&["destroy", &name]).await?;
    tracing::info!(user = %user_of(&u), "boot environment destroy {name}");
    Ok(Json(json!({ "ok": true })))
}

async fn destroy_snapshot(u: AuthExt, Path((b, s)): Path<(String, String)>) -> Res<Value> {
    name_or_400(&b, "boot environment")?;
    name_or_400(&s, "snapshot")?;
    let t = format!("{b}@{s}");
    be(&["destroy-snapshot", &t]).await?;
    tracing::info!(user = %user_of(&u), "boot environment destroy-snapshot {t}");
    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_cli_rule() {
        for ok in ["horcrux", "rollback-20261113-101500", "a", "x_1.2"] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in [
            "",
            "-x",
            ".x",
            "a b",
            "a/b",
            "a@b",
            "a;rm",
            "x".repeat(42).as_str(),
        ] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn targets_are_be_or_be_at_snapshot() {
        assert!(target_or_400("horcrux").is_ok());
        assert!(target_or_400("horcrux@pre-upgrade-20261113-101500").is_ok());
        assert!(target_or_400("horcrux@").is_err());
        assert!(target_or_400("@x").is_err());
        assert!(target_or_400("a/b@c").is_err());
    }

    #[test]
    fn a_missing_zfs_root_is_a_424_reason() {
        match no_zfs_root() {
            ApiError::NotAvailable { reason, .. } => assert_eq!(reason, "no-zfs-boot-environments"),
            _ => panic!("expected NotAvailable"),
        }
    }
}
