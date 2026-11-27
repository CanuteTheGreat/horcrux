//! The system-wide default text editor (`EDITOR` / `VISUAL`).
//!
//! * `GET /api/system/editor`  the setting, what it resolves to, what is installed
//! * `PUT /api/system/editor`  `{choice}` = vim | neovim | nano | vi | /absolute/path
//!
//! The setting itself lives in `/etc/default-editor.conf` and is applied by
//! the `default-editor` tool (shipped by horcrux-core and the live images; it
//! writes `/etc/env.d/99editor`, init-neutral). This module only reads the
//! file, validates, and runs `default-editor set <choice>`. A node without the
//! tool answers 424 `default-editor-missing`. The declarative key
//! `system.editor` goes through the same functions.
//!
//! Tests point `HORCRUX_SYSROOT` at a scratch tree: the tool is then looked up
//! under it and run with `DEFAULT_EDITOR_ROOT` set to it.
use super::{rooted, run_env, user_of, which};
use crate::error::ApiError;
use crate::AppState;
use axum::{extract::State, routing::get, Extension, Json, Router};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/api/system/editor", get(get_editor).put(put_editor))
}

/// The named choices (anything else must be an absolute path).
pub const NAMED: [&str; 4] = ["vim", "neovim", "nano", "vi"];
/// What the tool uses when `/etc/default-editor.conf` has no choice.
pub const DEFAULT_CHOICE: &str = "vim";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EditorState {
    /// The configured choice (`vim` when nothing is configured).
    pub choice: String,
    /// The binary `EDITOR` ends up as (the tool falls back when the choice is
    /// not installed); `None` when no editor is installed at all.
    pub effective: Option<String>,
    /// The named choices and whether their binary is installed.
    pub installed: Vec<EditorInstalled>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EditorInstalled {
    pub choice: String,
    pub installed: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EditorPut {
    pub choice: String,
}

/// `Ok` for vim|neovim|nano|vi or a plain absolute path.
pub fn validate_choice(c: &str) -> Result<(), String> {
    if NAMED.contains(&c) {
        return Ok(());
    }
    if c.starts_with('/') {
        let plain = c.len() <= 255
            && !c.split('/').any(|p| p == "..")
            && c.chars().all(|ch| {
                ch.is_ascii_graphic() && !matches!(ch, '"' | '\'' | '#' | '\\' | '$' | '`')
            });
        if plain {
            return Ok(());
        }
        return Err(format!(
            "editor path '{c}' must be a plain absolute path (no spaces, quotes, '..' or shell characters)"
        ));
    }
    Err(format!(
        "unknown editor '{c}': use vim, neovim, nano, vi or an absolute path to an editor binary"
    ))
}

/// `EDITOR_CHOICE=` out of `default-editor.conf` (last assignment wins,
/// quotes and trailing comments allowed), like the tool reads it.
pub fn parse_choice(text: &str) -> Option<String> {
    text.lines()
        .filter_map(|l| {
            let v = l.trim_start().strip_prefix("EDITOR_CHOICE=")?;
            let v = v.trim_start().trim_start_matches('"');
            let end = v
                .find(|c: char| c == '"' || c == '#' || c.is_whitespace())
                .unwrap_or(v.len());
            Some(v[..end].to_string()).filter(|s| !s.is_empty())
        })
        .last()
}

/// The configured choice (the tool's default when nothing is configured).
pub fn configured_choice() -> String {
    std::fs::read_to_string(rooted("etc/default-editor.conf"))
        .ok()
        .and_then(|t| parse_choice(&t))
        .unwrap_or_else(|| DEFAULT_CHOICE.to_string())
}

/// Where the `default-editor` tool is, or `None` on a node without it.
pub fn tool() -> Option<PathBuf> {
    if std::env::var_os("HORCRUX_SYSROOT").is_some() {
        let p = rooted("usr/sbin/default-editor");
        return p.is_file().then_some(p);
    }
    which("default-editor")
}

pub fn lacks_tool() -> ApiError {
    ApiError::NotAvailable {
        reason: "default-editor-missing",
        message: "This node has no default-editor tool, so the default editor cannot be set here."
            .into(),
        hint: "Install the horcrux-core package (it ships /usr/sbin/default-editor), or set EDITOR with eselect/update-alternatives."
            .into(),
    }
}

fn candidates(choice: &str) -> Vec<String> {
    let l = |v: &[&str]| v.iter().map(|s| s.to_string()).collect();
    match choice {
        "vim" => l(&["/usr/bin/vim", "/bin/vim", "/usr/sbin/vim"]),
        "neovim" | "nvim" => l(&["/usr/bin/nvim"]),
        "nano" => l(&["/bin/nano", "/usr/bin/nano"]),
        "vi" => l(&["/usr/bin/vi", "/bin/vi"]),
        p if p.starts_with('/') => vec![p.to_string()],
        other => vec![format!("/usr/bin/{other}")],
    }
}

fn is_installed(choice: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    candidates(choice).iter().any(|p| {
        std::fs::metadata(rooted(p))
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    })
}

/// `choice=vim editor=/usr/bin/vim` (or `editor=none`) from `default-editor show`.
pub fn parse_show(text: &str) -> Option<String> {
    let v = text
        .split_whitespace()
        .find_map(|w| w.strip_prefix("editor="))?;
    (v != "none" && !v.is_empty()).then(|| v.to_string())
}

fn env() -> Vec<(&'static str, String)> {
    if std::env::var_os("HORCRUX_SYSROOT").is_some() {
        vec![(
            "DEFAULT_EDITOR_ROOT",
            super::sysroot().to_string_lossy().to_string(),
        )]
    } else {
        Vec::new()
    }
}

async fn tool_run(tool: &std::path::Path, args: &[&str]) -> Result<String, String> {
    run_env(&tool.to_string_lossy(), args, &env()).await
}

/// The setting as seen right now; `None` without the tool.
pub async fn current() -> Option<EditorState> {
    let t = tool()?;
    let choice = configured_choice();
    let effective = tool_run(&t, &["show"])
        .await
        .ok()
        .and_then(|o| parse_show(&o));
    Some(EditorState {
        installed: NAMED
            .iter()
            .map(|c| EditorInstalled {
                choice: c.to_string(),
                installed: is_installed(c),
            })
            .collect(),
        choice,
        effective,
    })
}

/// Validate and apply a choice (`default-editor set <choice>`). Err text is
/// for the declarative report; the HTTP handler maps it to an `ApiError`.
pub async fn set(choice: &str) -> Result<(), ApiError> {
    validate_choice(choice).map_err(ApiError::ValidationError)?;
    let t = tool().ok_or_else(lacks_tool)?;
    tool_run(&t, &["set", choice])
        .await
        .map_err(|e| ApiError::Internal(format!("default-editor set {choice}: {e}")))?;
    Ok(())
}

async fn get_editor() -> Result<Json<EditorState>, ApiError> {
    current().await.map(Json).ok_or_else(lacks_tool)
}

async fn put_editor(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<crate::middleware::auth::AuthUser>>,
    Json(b): Json<EditorPut>,
) -> Result<Json<EditorState>, ApiError> {
    let r = set(&b.choice).await;
    state
        .audit_logger
        .log(crate::audit::AuditEvent {
            timestamp: chrono::Utc::now(),
            event_type: crate::audit::AuditEventType::ConfigChanged,
            severity: crate::audit::AuditSeverity::Info,
            user: Some(user_of(&user)),
            source_ip: None,
            resource: Some("system/editor".into()),
            action: format!("default editor set to {}", b.choice),
            result: if r.is_ok() {
                crate::audit::AuditResult::Success
            } else {
                crate::audit::AuditResult::Failure
            },
            details: r.as_ref().err().map(|e| format!("{e:?}")),
            session_id: None,
        })
        .await;
    r?;
    current().await.map(Json).ok_or_else(lacks_tool)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_are_validated() {
        for ok in [
            "vim",
            "neovim",
            "nano",
            "vi",
            "/usr/bin/emacs",
            "/opt/ed/bin/x-y_z",
        ] {
            assert!(validate_choice(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "emacs",
            "nvim",
            "Vim",
            "relative/vim",
            "/usr/bin/vim; rm",
            "/a b",
            "/a/../b",
            "/x\"y",
            "/x#y",
            "/x$y",
            "/x\ny",
        ] {
            assert!(validate_choice(bad).is_err(), "{bad:?}");
        }
        let e = validate_choice("emacs").unwrap_err();
        assert!(e.contains("vim, neovim, nano, vi"), "{e}");
    }

    #[test]
    fn conf_parsing_matches_the_tool() {
        assert_eq!(
            parse_choice("# c\nEDITOR_CHOICE=neovim\n").as_deref(),
            Some("neovim")
        );
        assert_eq!(
            parse_choice("EDITOR_CHOICE=\"nano\" # x\n").as_deref(),
            Some("nano")
        );
        assert_eq!(
            parse_choice("EDITOR_CHOICE=vi\nEDITOR_CHOICE=/usr/bin/ed\n").as_deref(),
            Some("/usr/bin/ed")
        );
        assert_eq!(parse_choice("# EDITOR_CHOICE=nano\n"), None);
        assert_eq!(parse_choice("EDITOR_CHOICE=\n"), None);
    }

    #[test]
    fn show_output_is_parsed() {
        assert_eq!(
            parse_show("choice=vim editor=/usr/bin/vim\n").as_deref(),
            Some("/usr/bin/vim")
        );
        assert_eq!(parse_show("choice=vim editor=none\n"), None);
    }
}
