//! Storage pool content types (Proxmox `content`: images, rootdir, iso,
//! vztmpl, backup, snippets): list, upload and delete files per pool.
//!
//! - `GET    /api/storage/content-types`                           the types and where they live in a pool
//! - `GET    /api/storage/pools/:pool_id/content[?content=iso]`    list
//! - `POST   /api/storage/pools/:pool_id/upload?content=iso&filename=x[&sha256=..]`
//!   raw request body streamed to disk (no size limit, no buffering), checked
//!   against the optional SHA-256, moved into place atomically
//! - `DELETE /api/storage/pools/:pool_id/content/:content/:name`
//!
//! Layout inside a directory pool (same as Proxmox): `template/iso`,
//! `template/cache` (container templates), `dump` (backups), `snippets`,
//! `images`, `private` (container rootdirs).
use crate::error::ApiError;
use crate::AppState;
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/storage/content-types", get(content_types))
        .route("/api/storage/pools/:pool_id/content", get(list_content))
        .route(
            "/api/storage/pools/:pool_id/upload",
            post(upload).layer(DefaultBodyLimit::disable()),
        )
        .route(
            "/api/storage/pools/:pool_id/content/:content/:name",
            delete(delete_content),
        )
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ContentType {
    pub id: &'static str,
    pub label: &'static str,
    /// Directory below the pool path.
    pub dir: &'static str,
    /// Accepted file name endings (lower case); empty = any plain name.
    pub extensions: &'static [&'static str],
    pub uploadable: bool,
}

pub const CONTENT_TYPES: &[ContentType] = &[
    ContentType {
        id: "iso",
        label: "ISO image",
        dir: "template/iso",
        extensions: &[".iso", ".img"],
        uploadable: true,
    },
    ContentType {
        id: "vztmpl",
        label: "Container template",
        dir: "template/cache",
        extensions: &[".tar.gz", ".tar.xz", ".tar.zst", ".tar.bz2", ".tgz"],
        uploadable: true,
    },
    ContentType {
        id: "snippets",
        label: "Snippet (cloud-init, hookscript)",
        dir: "snippets",
        extensions: &[],
        uploadable: true,
    },
    ContentType {
        id: "backup",
        label: "Backup archive",
        dir: "dump",
        extensions: &[".tar", ".tar.gz", ".tar.zst", ".vma", ".vma.zst", ".vma.gz"],
        uploadable: true,
    },
    ContentType {
        id: "images",
        label: "VM disk image",
        dir: "images",
        extensions: &[".qcow2", ".raw", ".img", ".vmdk", ".ova", ".ovf"],
        uploadable: true,
    },
    ContentType {
        id: "rootdir",
        label: "Container root directory",
        dir: "private",
        extensions: &[],
        uploadable: false,
    },
];

pub fn content_type(id: &str) -> Result<&'static ContentType, ApiError> {
    CONTENT_TYPES
        .iter()
        .find(|c| c.id == id)
        .ok_or_else(|| ApiError::BadRequest(format!("unknown content type {id:?}")))
}

fn name_ok(ct: &ContentType, name: &str) -> Result<(), ApiError> {
    crate::validation::validate_volume_name(name).map_err(|e| match e {
        horcrux_common::Error::Validation(m) | horcrux_common::Error::InvalidConfig(m) => {
            ApiError::BadRequest(m)
        }
        other => ApiError::from(other),
    })?;
    let lower = name.to_ascii_lowercase();
    if !ct.extensions.is_empty() && !ct.extensions.iter().any(|e| lower.ends_with(e)) {
        return Err(ApiError::BadRequest(format!(
            "{} files must end in {}",
            ct.id,
            ct.extensions.join(", ")
        )));
    }
    Ok(())
}

async fn pool_base(state: &AppState, id: &str) -> Result<PathBuf, ApiError> {
    let pool = state
        .storage_manager
        .get_pool(id)
        .await
        .map_err(|_| ApiError::NotFound(format!("storage pool {id} not found")))?;
    if !pool.path.starts_with('/') {
        return Err(ApiError::BadRequest(format!(
            "pool {id} is not a directory-backed pool (path {:?})",
            pool.path
        )));
    }
    Ok(PathBuf::from(pool.path))
}

async fn content_types() -> Json<&'static [ContentType]> {
    Json(CONTENT_TYPES)
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct ContentItem {
    pub content: String,
    pub name: String,
    pub size: u64,
    pub path: String,
    /// Unix seconds.
    pub modified: i64,
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    content: Option<String>,
}

async fn list_content(
    State(state): State<Arc<AppState>>,
    Path(pool_id): Path<String>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<ContentItem>>, ApiError> {
    let base = pool_base(&state, &pool_id).await?;
    let types: Vec<&ContentType> = match q.content.as_deref() {
        Some(c) if !c.is_empty() => vec![content_type(c)?],
        _ => CONTENT_TYPES.iter().collect(),
    };
    let mut out = Vec::new();
    for ct in types {
        let Ok(rd) = std::fs::read_dir(base.join(ct.dir)) else {
            continue;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue; // in-flight uploads (.name.part)
            }
            let Ok(md) = e.metadata() else { continue };
            // rootdir holds directories; everything else regular files.
            if ct.id == "rootdir" {
                if !md.is_dir() {
                    continue;
                }
            } else if !md.is_file()
                || (!ct.extensions.is_empty()
                    && !ct
                        .extensions
                        .iter()
                        .any(|x| name.to_ascii_lowercase().ends_with(x)))
            {
                continue;
            }
            out.push(ContentItem {
                content: ct.id.into(),
                size: if md.is_file() { md.len() } else { 0 },
                path: e.path().to_string_lossy().to_string(),
                modified: md
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
                name,
            });
        }
    }
    out.sort_by(|a, b| (&a.content, &a.name).cmp(&(&b.content, &b.name)));
    Ok(Json(out))
}

#[derive(Debug, Deserialize)]
struct UploadQuery {
    content: String,
    filename: String,
    sha256: Option<String>,
}

async fn upload(
    State(state): State<Arc<AppState>>,
    Path(pool_id): Path<String>,
    Query(q): Query<UploadQuery>,
    body: Body,
) -> Result<(StatusCode, Json<ContentItem>), ApiError> {
    use tokio::io::AsyncWriteExt;
    let ct = content_type(&q.content)?;
    if !ct.uploadable {
        return Err(ApiError::BadRequest(format!(
            "{} cannot be uploaded",
            ct.label
        )));
    }
    name_ok(ct, &q.filename)?;
    if let Some(s) = &q.sha256 {
        if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ApiError::BadRequest("sha256 must be 64 hex digits".into()));
        }
    }
    let dir = pool_base(&state, &pool_id).await?.join(ct.dir);
    tokio::fs::create_dir_all(&dir).await?;
    let target = dir.join(&q.filename);
    if target.exists() {
        return Err(ApiError::Conflict(format!("{} already exists", q.filename)));
    }
    let part = dir.join(format!(".{}.part", q.filename));
    let mut f = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&part)
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                ApiError::Conflict(format!("{} is already being uploaded", q.filename))
            } else {
                ApiError::from(e)
            }
        })?;
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut stream = body.into_data_stream();
    let result: Result<(), ApiError> = async {
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| ApiError::BadRequest(format!("upload aborted: {e}")))?;
            hasher.update(&chunk);
            size += chunk.len() as u64;
            f.write_all(&chunk).await?;
        }
        f.flush().await?;
        f.sync_all().await?;
        if size == 0 {
            return Err(ApiError::BadRequest("empty upload".into()));
        }
        let got = hex::encode(hasher.finalize_reset());
        if let Some(want) = &q.sha256 {
            if !want.eq_ignore_ascii_case(&got) {
                return Err(ApiError::BadRequest(format!(
                    "sha256 mismatch: uploaded data is {got}"
                )));
            }
        }
        Ok(())
    }
    .await;
    drop(f);
    if let Err(e) = result {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(e);
    }
    if ct.id == "snippets" {
        // snippets double as VM hookscripts, which must be executable
        use std::os::unix::fs::PermissionsExt;
        let _ = tokio::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o755)).await;
    }
    // hard_link + remove loses races cleanly (rename would overwrite)
    if let Err(e) = tokio::fs::hard_link(&part, &target).await {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(if e.kind() == std::io::ErrorKind::AlreadyExists {
            ApiError::Conflict(format!("{} already exists", q.filename))
        } else {
            ApiError::from(e)
        });
    }
    let _ = tokio::fs::remove_file(&part).await;
    tracing::info!("uploaded {} ({size} bytes) to pool {pool_id}", q.filename);
    Ok((
        StatusCode::CREATED,
        Json(ContentItem {
            content: ct.id.into(),
            name: q.filename,
            size,
            path: target.to_string_lossy().to_string(),
            modified: chrono::Utc::now().timestamp(),
        }),
    ))
}

async fn delete_content(
    State(state): State<Arc<AppState>>,
    Path((pool_id, content, name)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    let ct = content_type(&content)?;
    name_ok(ct, &name)?;
    let path = pool_base(&state, &pool_id).await?.join(ct.dir).join(&name);
    let md = tokio::fs::symlink_metadata(&path)
        .await
        .map_err(|_| ApiError::NotFound(format!("{content}/{name} not found")))?;
    if md.is_dir() {
        if ct.id != "rootdir" {
            return Err(ApiError::BadRequest(format!("{name} is a directory")));
        }
        tokio::fs::remove_dir_all(&path).await?;
    } else {
        tokio::fs::remove_file(&path).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_are_enforced() {
        let iso = content_type("iso").unwrap();
        assert!(name_ok(iso, "debian-12.iso").is_ok());
        assert!(name_ok(iso, "evil.sh").is_err());
        assert!(name_ok(iso, "../x.iso").is_err());
        let sn = content_type("snippets").unwrap();
        assert!(name_ok(sn, "user-data.yaml").is_ok());
        assert!(name_ok(sn, ".hidden").is_err());
        assert!(content_type("nope").is_err());
        assert!(!content_type("rootdir").unwrap().uploadable);
    }
}
