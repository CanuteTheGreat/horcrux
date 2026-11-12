//! Opt-in cleanup of the *old source* after an import (libvirt domain, local
//! OVF/OVA files). Nothing is deleted by an import: it only writes a record
//! of what the old source consists of (`<data_dir>/import-cleanup/<vm>.json`).
//! The UI shows that list with sizes (`preview`) and only removes it on an
//! explicit `confirm: true` (`cleanup`).
//!
//! * `POST /api/vms/import/cleanup/pending {}` - VMs with a cleanup offer.
//! * `POST /api/vms/import/cleanup/preview {vm_id}` - exactly what would go.
//! * `POST /api/vms/import/cleanup {vm_id, confirm: true}` - remove it.
//!
//! Safety: the VM must still exist; a path that any VM uses as a disk (or
//! that holds a VM's disk, firmware or TPM state) is never removed; a leftover
//! copy (NVRAM, TPM state) goes only when its Horcrux copy exists; the libvirt
//! XML is saved before the domain is undefined; the record is dropped when
//! nothing is left to remove.
use crate::api::libvirt_import::virsh;
use crate::error::ApiError;
use crate::AppState;
use axum::{extract::State, routing::post, Json, Router};
use serde::{Deserialize, Serialize};
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/vms/import/cleanup/pending", post(pending))
        .route("/api/vms/import/cleanup/preview", post(preview))
        .route("/api/vms/import/cleanup", post(cleanup))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Item {
    /// `libvirt-domain`, `file` or `dir`.
    pub kind: String,
    /// File/dir path, or the libvirt domain name.
    pub path: String,
    /// The Horcrux copy that must exist before this leftover may go.
    #[serde(default)]
    pub requires: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Record {
    pub vm_id: String,
    /// `libvirt`, `ovf`, ...
    pub source: String,
    pub items: Vec<Item>,
}

fn dir(state: &AppState) -> PathBuf {
    state.config.paths.data_dir.join("import-cleanup")
}

fn record_path(state: &AppState, vm_id: &str) -> PathBuf {
    dir(state).join(format!("{vm_id}.json"))
}

/// Remember what the old source of `rec.vm_id` is (nothing is removed).
pub fn write_record(state: &AppState, rec: &Record) -> std::io::Result<()> {
    if rec.items.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(dir(state))?;
    std::fs::write(
        record_path(state, &rec.vm_id),
        serde_json::to_vec_pretty(rec).unwrap_or_default(),
    )
}

fn read_record(state: &AppState, vm_id: &str) -> Result<Record, ApiError> {
    crate::validation::validate_vm_id(vm_id)?;
    let b = std::fs::read(record_path(state, vm_id))
        .map_err(|_| ApiError::NotFound(format!("VM {vm_id} has no pending import cleanup")))?;
    serde_json::from_slice(&b).map_err(|e| ApiError::Internal(format!("cleanup record: {e}")))
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Planned {
    pub kind: String,
    pub path: String,
    pub bytes: u64,
    /// Removed by `cleanup`; false = kept, see `reason`.
    pub remove: bool,
    pub reason: Option<String>,
}

fn size_of(p: &FsPath) -> u64 {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => std::fs::read_dir(p)
            .map(|rd| rd.flatten().map(|e| size_of(&e.path())).sum())
            .unwrap_or(0),
        Ok(m) => m.len(),
        Err(_) => 0,
    }
}

/// Every path an existing VM needs (disks, and the firmware/TPM state that
/// lives next to the first disk).
fn in_use(vms: &[horcrux_common::VmConfig]) -> Vec<PathBuf> {
    let mut v = Vec::new();
    for vm in vms {
        for d in &vm.disks {
            v.push(PathBuf::from(&d.path));
        }
        if let Some(d0) = vm.disks.first() {
            let d0 = PathBuf::from(&d0.path);
            v.push(crate::vm::firmware::efivars_path(&d0));
            v.push(crate::vm::firmware::tpm_state_dir(&d0));
        }
    }
    v
}

/// Decide for each item whether it may go. Pure over the file system and the
/// VM list: `domain_defined` answers whether libvirt still defines a domain.
pub fn evaluate(
    rec: &Record,
    vms: &[horcrux_common::VmConfig],
    domain_state: &dyn Fn(&str) -> Option<String>,
) -> Vec<Planned> {
    let used = in_use(vms);
    rec.items
        .iter()
        .map(|it| {
            let mut p = Planned {
                kind: it.kind.clone(),
                path: it.path.clone(),
                bytes: 0,
                remove: false,
                reason: None,
            };
            if it.kind == "libvirt-domain" {
                match domain_state(&it.path).as_deref() {
                    None => p.reason = Some("libvirt no longer defines this domain".into()),
                    Some("shut off") => p.remove = true,
                    Some(s) => p.reason = Some(format!("the domain is {s}: shut it down first")),
                }
                return p;
            }
            let path = PathBuf::from(&it.path);
            if it.kind != "file" && it.kind != "dir" {
                p.reason = Some("unknown item".into());
            } else if !path.is_absolute()
                || path.components().count() < 3
                || path.components().any(|c| c.as_os_str() == "..")
            {
                p.reason = Some("path refused".into());
            } else if !path.exists() {
                p.reason = Some("already gone".into());
            } else if used
                .iter()
                .any(|u| u == &path || u.starts_with(&path) || path.starts_with(u))
            {
                p.bytes = size_of(&path);
                p.reason = Some("in use by a Horcrux VM: kept".into());
            } else if it
                .requires
                .as_deref()
                .is_some_and(|r| !FsPath::new(r).exists())
            {
                p.bytes = size_of(&path);
                p.reason = Some("the Horcrux copy is missing: kept".into());
            } else {
                p.bytes = size_of(&path);
                p.remove = true;
            }
            p
        })
        .collect()
}

async fn domain_state(name: &str) -> Option<String> {
    virsh(&["domstate", name])
        .await
        .ok()
        .flatten()
        .map(|s| s.trim().to_string())
}

async fn plan(state: &AppState, vm_id: &str) -> Result<(Record, Vec<Planned>), ApiError> {
    let rec = read_record(state, vm_id)?;
    state.vm_manager.get_vm(vm_id).await?;
    let vms = state.vm_manager.list_vms().await;
    let mut states = std::collections::HashMap::new();
    for it in rec.items.iter().filter(|i| i.kind == "libvirt-domain") {
        states.insert(it.path.clone(), domain_state(&it.path).await);
    }
    let items = evaluate(&rec, &vms, &|d| states.get(d).cloned().flatten());
    Ok((rec, items))
}

#[derive(Serialize)]
struct Offer {
    vm_id: String,
    source: String,
    items: Vec<Planned>,
    total_bytes: u64,
}

async fn offer(state: &AppState, vm_id: &str) -> Result<Offer, ApiError> {
    let (rec, items) = plan(state, vm_id).await?;
    let total_bytes = items.iter().filter(|i| i.remove).map(|i| i.bytes).sum();
    Ok(Offer {
        vm_id: rec.vm_id,
        source: rec.source,
        items,
        total_bytes,
    })
}

#[derive(Deserialize)]
struct VmRequest {
    vm_id: String,
    #[serde(default)]
    confirm: bool,
}

async fn pending(State(state): State<Arc<AppState>>) -> Result<Json<Vec<Offer>>, ApiError> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir(&state)) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            let Some(id) = n.strip_suffix(".json") else {
                continue;
            };
            if let Ok(o) = offer(&state, id).await {
                // nothing left to remove: the offer is done
                if o.items.iter().any(|i| i.remove) {
                    out.push(o);
                } else if o.items.iter().all(|i| {
                    i.reason.as_deref().is_some_and(|r| {
                        r.starts_with("already gone") || r.starts_with("libvirt no longer")
                    })
                }) {
                    let _ = std::fs::remove_file(e.path());
                } else {
                    out.push(o);
                }
            }
        }
    }
    out.sort_by(|a, b| a.vm_id.cmp(&b.vm_id));
    Ok(Json(out))
}

async fn preview(
    State(state): State<Arc<AppState>>,
    Json(req): Json<VmRequest>,
) -> Result<Json<Offer>, ApiError> {
    Ok(Json(offer(&state, &req.vm_id).await?))
}

#[derive(Serialize)]
struct Done {
    removed: Vec<Planned>,
    kept: Vec<Planned>,
    xml_backup: Option<String>,
}

async fn cleanup(
    State(state): State<Arc<AppState>>,
    Json(req): Json<VmRequest>,
) -> Result<Json<Done>, ApiError> {
    if !req.confirm {
        return Err(ApiError::BadRequest(
            "nothing is removed without confirm: true (use preview to see the list)".into(),
        ));
    }
    let (rec, items) = plan(&state, &req.vm_id).await?;
    // 1. back up the libvirt XML before anything goes
    let mut xml_backup = None;
    for it in items
        .iter()
        .filter(|i| i.kind == "libvirt-domain" && i.remove)
    {
        let xml = virsh(&["dumpxml", "--inactive", &it.path])
            .await?
            .ok_or_else(|| ApiError::BadRequest("virsh is not installed".into()))?;
        let d = state.config.paths.data_dir.join("libvirt-import");
        std::fs::create_dir_all(&d)?;
        let f = d.join(format!("{}.cleanup-backup.xml", req.vm_id));
        std::fs::write(&f, xml)?;
        xml_backup = Some(f.to_string_lossy().to_string());
    }
    // 2. remove (the domain first: its NVRAM/TPM leftovers then have no owner)
    let (mut removed, mut kept) = (Vec::new(), Vec::new());
    for it in items {
        if !it.remove {
            kept.push(it);
            continue;
        }
        let r: Result<(), String> = if it.kind == "libvirt-domain" {
            let a = virsh(&["undefine", &it.path, "--keep-nvram", "--keep-tpm"]).await;
            match a {
                Err(ApiError::BadRequest(m)) if m.contains("keep-tpm") => {
                    virsh(&["undefine", &it.path, "--keep-nvram"])
                        .await
                        .map(|_| ())
                }
                other => other.map(|_| ()),
            }
            .map_err(|e| format!("{e:?}"))
        } else if it.kind == "dir" {
            std::fs::remove_dir_all(&it.path).map_err(|e| e.to_string())
        } else {
            std::fs::remove_file(&it.path).map_err(|e| e.to_string())
        };
        match r {
            Ok(()) => removed.push(it),
            Err(e) => {
                let mut it = it;
                it.remove = false;
                it.reason = Some(format!("failed: {e}"));
                kept.push(it);
            }
        }
    }
    tracing::info!(
        "VM {}: import cleanup removed {} item(s), kept {}",
        rec.vm_id,
        removed.len(),
        kept.len()
    );
    if !kept.iter().any(|k| {
        k.reason
            .as_deref()
            .is_some_and(|r| r.starts_with("failed") || r.contains("shut it down"))
    }) {
        let _ = std::fs::remove_file(record_path(&state, &rec.vm_id));
    }
    Ok(Json(Done {
        removed,
        kept,
        xml_backup,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vm(disk: &str) -> horcrux_common::VmConfig {
        serde_json::from_value(serde_json::json!({
            "id": "a", "name": "a", "hypervisor": "qemu", "memory": 1, "cpus": 1,
            "disk_size": 1, "status": "stopped",
            "disks": [{"path": disk, "size_gb": 1, "disk_type": "virtio", "cache": ""}],
        }))
        .unwrap()
    }

    fn rec(items: Vec<Item>) -> Record {
        Record {
            vm_id: "a".into(),
            source: "libvirt".into(),
            items,
        }
    }

    fn item(kind: &str, path: &str, requires: Option<&str>) -> Item {
        Item {
            kind: kind.into(),
            path: path.into(),
            requires: requires.map(String::from),
        }
    }

    #[test]
    fn never_removes_a_disk_the_vm_uses_and_reports_sizes() {
        let d = tempfile::tempdir().unwrap();
        let disk = d.path().join("used.qcow2");
        let old = d.path().join("old.ova");
        std::fs::write(&disk, vec![0u8; 100]).unwrap();
        std::fs::write(&old, vec![0u8; 50]).unwrap();
        let r = rec(vec![
            item("file", disk.to_str().unwrap(), None),
            item("file", old.to_str().unwrap(), None),
            item("file", d.path().join("gone").to_str().unwrap(), None),
        ]);
        let p = evaluate(&r, &[vm(disk.to_str().unwrap())], &|_| None);
        assert!(!p[0].remove && p[0].reason.as_deref().unwrap().contains("in use"));
        assert!(p[1].remove && p[1].bytes == 50);
        assert!(!p[2].remove);
    }

    #[test]
    fn a_leftover_goes_only_when_its_horcrux_copy_exists() {
        let d = tempfile::tempdir().unwrap();
        let old = d.path().join("nvram.qcow2");
        std::fs::write(&old, b"x").unwrap();
        let copy = d.path().join("vm.efivars.qcow2");
        let r = rec(vec![item(
            "file",
            old.to_str().unwrap(),
            Some(copy.to_str().unwrap()),
        )]);
        assert!(!evaluate(&r, &[], &|_| None)[0].remove);
        std::fs::write(&copy, b"y").unwrap();
        assert!(evaluate(&r, &[], &|_| None)[0].remove);
    }

    #[test]
    fn a_domain_goes_only_while_shut_off_and_defined() {
        let r = rec(vec![item("libvirt-domain", "dom", None)]);
        assert!(evaluate(&r, &[], &|_| Some("shut off".into()))[0].remove);
        let running = evaluate(&r, &[], &|_| Some("running".into()));
        assert!(
            !running[0].remove
                && running[0]
                    .reason
                    .as_deref()
                    .unwrap()
                    .contains("shut it down")
        );
        assert!(!evaluate(&r, &[], &|_| None)[0].remove);
    }

    #[test]
    fn refuses_odd_paths_and_parents_of_used_disks() {
        let d = tempfile::tempdir().unwrap();
        let disk = d.path().join("sub/used.qcow2");
        std::fs::create_dir_all(disk.parent().unwrap()).unwrap();
        std::fs::write(&disk, b"x").unwrap();
        let r = rec(vec![
            item("dir", d.path().join("sub").to_str().unwrap(), None),
            item("file", "/etc", None),
            item("file", "relative/x", None),
            item("dir", "/tmp/../etc/x", None),
        ]);
        let p = evaluate(&r, &[vm(disk.to_str().unwrap())], &|_| None);
        assert!(p.iter().all(|i| !i.remove), "{p:?}");
    }

    #[test]
    fn record_round_trips() {
        let r = rec(vec![item("file", "/a/b/c", Some("/x/y/z"))]);
        let j = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<Record>(&j).unwrap(), r);
    }
}
