//! Import a disk image into a VM (Proxmox `qm disk import` / `import-from`).
//!
//! `POST /api/vms/:id/import-disk {"source": "<http(s) URL or path>"}`
//! replaces the (stopped) VM's first disk with the given cloud image or
//! disk image, converted to qcow2 and grown to at least the VM's disk size.
//! This is how a VM gets an operating system from a cloud image (CirrOS,
//! Debian/Ubuntu/Alpine genericcloud qcow2, ...).
//!
//! Safety:
//! - local sources must live under `<data_dir>/images` or a storage pool's
//!   directory, so the API can't be used to read arbitrary host files;
//! - the source format is probed once with `qemu-img info` and then passed
//!   explicitly (`-f`), and images with a backing file are refused (a
//!   crafted image could otherwise pull in a host file as its backing);
//! - the conversion writes a temporary file next to the disk and renames it
//!   over the old disk only when complete, so a failure (disk full, bad
//!   image, server killed mid-import) leaves the VM's old disk intact and
//!   no temporary file behind.
use crate::error::ApiError;
use crate::AppState;
use axum::{
    extract::{Path, State},
    routing::post,
    Json, Router,
};
use horcrux_common::{VmConfig, VmHypervisor};
use serde::Deserialize;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use tokio::process::Command;

const FORMATS: &[&str] = &["qcow2", "raw", "vmdk", "vdi", "vhdx", "vpc", "qed"];

/// Disk import route. Also, once at startup, undoes guest holds left by an
/// operation the API was killed in the middle of (see `vm::hold`).
pub fn routes(state: &Arc<AppState>) -> Router<Arc<AppState>> {
    let vms = state.vm_manager.clone();
    tokio::spawn(async move {
        let recovered = crate::vm::hold::recover(&vms).await;
        if !recovered.is_empty() {
            tracing::warn!("Recovered guests after an interrupted operation: {recovered:?}");
        }
    });
    Router::new().route("/api/vms/:id/import-disk", post(import_disk))
}

#[derive(Deserialize)]
pub struct ImportDiskRequest {
    pub source: String,
}

/// Directory local import sources may come from (besides storage pools):
/// `HORCRUX_IMAGE_DIR`, default `<data_dir>/images`.
pub fn image_dir(data_dir: &FsPath) -> PathBuf {
    std::env::var("HORCRUX_IMAGE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| data_dir.join("images"))
}

/// Whether `source` (canonical) lies inside one of `roots`.
pub fn allowed_local_source(source: &FsPath, roots: &[PathBuf]) -> bool {
    roots
        .iter()
        .filter_map(|r| std::fs::canonicalize(r).ok())
        .any(|r| source.starts_with(&r) && source != r)
}

/// Format of a probed image, refusing backing files and unknown formats.
pub fn checked_format(info: &serde_json::Value) -> Result<String, String> {
    if info.get("backing-filename").is_some() || info.get("full-backing-filename").is_some() {
        return Err("images with a backing file can't be imported".into());
    }
    let fmt = info["format"].as_str().unwrap_or_default();
    if !FORMATS.contains(&fmt) {
        return Err(format!("unsupported image format {fmt:?}"));
    }
    Ok(fmt.to_string())
}

async fn qemu_img(args: &[&str]) -> Result<String, ApiError> {
    let out = Command::new("qemu-img")
        .args(args)
        .output()
        .await
        .map_err(|e| ApiError::Internal(format!("qemu-img: {e}")))?;
    if !out.status.success() {
        return Err(ApiError::Internal(format!(
            "qemu-img {}: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Removes the temporary files on every exit path.
struct TempFiles(Vec<PathBuf>);
impl Drop for TempFiles {
    fn drop(&mut self) {
        for p in &self.0 {
            let _ = std::fs::remove_file(p);
        }
    }
}

async fn download(url: &str, to: &FsPath) -> Result<(), ApiError> {
    use tokio::io::AsyncWriteExt;
    let mut resp = reqwest::Client::new()
        .get(url)
        .send()
        .await
        .map_err(|e| ApiError::BadRequest(format!("download {url}: {e}")))?;
    if !resp.status().is_success() {
        return Err(ApiError::BadRequest(format!(
            "download {url}: HTTP {}",
            resp.status()
        )));
    }
    let mut f = tokio::fs::File::create(to).await?;
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| ApiError::BadRequest(format!("download {url}: {e}")))?
    {
        f.write_all(&chunk).await?;
    }
    f.flush().await?;
    f.sync_all().await?;
    Ok(())
}

async fn import_disk(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<ImportDiskRequest>,
) -> Result<Json<VmConfig>, ApiError> {
    let vm = state.vm_manager.get_vm(&id).await?;
    if vm.hypervisor != VmHypervisor::Qemu {
        return Err(ApiError::BadRequest(
            "disk import is for QEMU VMs".to_string(),
        ));
    }
    if crate::vm::qemu::QemuManager::process_running(&vm.id) {
        return Err(ApiError::Conflict(format!(
            "VM {id} is running; stop it before importing a disk"
        )));
    }
    let disk = PathBuf::from(
        &vm.disks
            .first()
            .ok_or_else(|| ApiError::BadRequest(format!("VM {id} has no disk")))?
            .path,
    );
    let dir = disk
        .parent()
        .ok_or_else(|| ApiError::Internal("disk has no directory".into()))?;
    let tmp_src = dir.join(format!(".{id}.import-src.tmp"));
    let tmp_out = dir.join(format!(".{id}.import.qcow2.tmp"));
    let _cleanup = TempFiles(vec![tmp_src.clone(), tmp_out.clone()]);

    let source: PathBuf = if req.source.starts_with("http://") || req.source.starts_with("https://")
    {
        download(&req.source, &tmp_src).await?;
        tmp_src.clone()
    } else {
        let p = std::fs::canonicalize(&req.source)
            .map_err(|e| ApiError::BadRequest(format!("source {}: {e}", req.source)))?;
        let mut roots = vec![image_dir(&state.config.paths.data_dir)];
        roots.extend(
            state
                .storage_manager
                .list_pools()
                .await
                .into_iter()
                .filter(|p| p.path.starts_with('/'))
                .map(|p| PathBuf::from(p.path)),
        );
        if !allowed_local_source(&p, &roots) || !p.is_file() {
            return Err(ApiError::Forbidden(format!(
                "local import sources must be files under {} or a storage pool directory",
                image_dir(&state.config.paths.data_dir).display()
            )));
        }
        p
    };

    let src = source.to_string_lossy().to_string();
    let info: serde_json::Value =
        serde_json::from_str(&qemu_img(&["info", "--output=json", &src]).await?)
            .map_err(|e| ApiError::Internal(format!("qemu-img info: {e}")))?;
    let fmt = checked_format(&info).map_err(ApiError::BadRequest)?;
    let out = tmp_out.to_string_lossy().to_string();
    qemu_img(&["convert", "-f", &fmt, "-O", "qcow2", &src, &out]).await?;
    let virtual_size = info["virtual-size"].as_u64().unwrap_or(0);
    let wanted = vm.disk_size.saturating_mul(1024 * 1024 * 1024);
    if wanted > virtual_size {
        qemu_img(&["resize", "-f", "qcow2", &out, &format!("{}G", vm.disk_size)]).await?;
    }
    // fsync the new image before it replaces the old one.
    tokio::fs::File::open(&tmp_out).await?.sync_all().await?;
    tokio::fs::rename(&tmp_out, &disk).await?;
    tracing::info!("VM {id}: imported disk from {}", req.source);
    Ok(Json(state.vm_manager.get_vm(&id).await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn backing_files_and_unknown_formats_are_refused() {
        assert_eq!(
            checked_format(&json!({"format": "qcow2"})).unwrap(),
            "qcow2"
        );
        assert_eq!(checked_format(&json!({"format": "raw"})).unwrap(), "raw");
        assert!(
            checked_format(&json!({"format": "qcow2", "backing-filename": "/etc/shadow"})).is_err()
        );
        assert!(checked_format(&json!({"format": "qcow2", "full-backing-filename": "x"})).is_err());
        assert!(checked_format(&json!({"format": "vvfat"})).is_err());
        assert!(checked_format(&json!({})).is_err());
    }

    #[test]
    fn local_sources_must_be_inside_an_allowed_root() {
        let d = std::env::temp_dir().join(format!("vm-import-test-{}", std::process::id()));
        let images = d.join("images");
        std::fs::create_dir_all(&images).unwrap();
        let inside = images.join("a.qcow2");
        std::fs::write(&inside, b"x").unwrap();
        let outside = d.join("b.qcow2");
        std::fs::write(&outside, b"x").unwrap();
        let roots = vec![images.clone()];
        let canon = |p: &PathBuf| std::fs::canonicalize(p).unwrap();
        assert!(allowed_local_source(&canon(&inside), &roots));
        assert!(!allowed_local_source(&canon(&outside), &roots));
        assert!(
            !allowed_local_source(&canon(&images), &roots),
            "the root itself"
        );
        // A ../ escape canonicalizes outside the root.
        let escape = images.join("../b.qcow2");
        assert!(!allowed_local_source(&canon(&escape), &roots));
        assert!(!allowed_local_source(&canon(&inside), &[d.join("missing")]));
        std::fs::remove_dir_all(&d).unwrap();
    }
}
