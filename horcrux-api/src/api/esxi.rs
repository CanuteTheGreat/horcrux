//! Import a VM from a standalone ESXi host (Proxmox "ESXi importer").
//!
//! * `POST /api/vms/import/esxi/list    {host, username, password, ...}` - the
//!   datastores of the host and the VMs (folders with a `.vmx`) on them
//! * `POST /api/vms/import/esxi/preview {..., datastore, folder}` - what the
//!   VM would become (nothing is copied)
//! * `POST /api/vms/import/esxi         {..., datastore, folder, id?, name?, bridge?, storage?}`
//!   copies the `.vmx` and the flat disks over HTTPS, converts them with
//!   `qemu-img` and creates the stopped VM (the same mapping, safety checks
//!   and rollback as the OVF importer)
//!
//! It uses the ESXi host's datastore browser (`https://<host>/folder/...`,
//! HTTP basic auth with an ESXi account), so no extra tool is needed. The VM
//! must be powered off (a `.vmx.lck` makes the import refuse) and have no
//! snapshots (delta disks cannot be read in isolation: consolidate first).
//! vCenter is not supported - point it at the ESXi host that runs the VM.
//! Credentials are used for this request only: never stored, never logged.
use crate::api::ovf::{
    free_id, import_plan, stage_dir, ImportResponse, Overrides, OvfDisk, OvfNic, OvfPlan, Source,
};
use crate::api::vm_import::{download_client_with, TempFiles};
use crate::error::ApiError;
use crate::AppState;
use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/vms/import/esxi/list", post(list))
        .route("/api/vms/import/esxi/preview", post(preview))
        .route("/api/vms/import/esxi", post(import))
}

const MAX_TEXT: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize)]
pub struct Conn {
    pub host: String,
    pub username: String,
    pub password: String,
    /// Check the host's HTTPS certificate (default off: ESXi ships a self-signed one).
    #[serde(default)]
    pub verify_tls: bool,
    /// Datacenter path (default `ha-datacenter`, what a standalone ESXi uses).
    #[serde(default)]
    pub datacenter: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
}

// ------------------------------------------------------------------ parsing

/// `(href, text)` of every `<a>` in an HTML listing.
pub fn anchors(html: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let lower = html.to_ascii_lowercase();
    let mut i = 0;
    while let Some(p) = lower[i..].find("<a ") {
        let start = i + p;
        let Some(gt) = lower[start..].find('>') else {
            break;
        };
        let tag = &html[start..start + gt];
        let Some(hp) = tag.to_ascii_lowercase().find("href=") else {
            i = start + gt;
            continue;
        };
        let v = &tag[hp + 5..];
        let (q, v) = match v.chars().next() {
            Some(c @ ('"' | '\'')) => (c, &v[1..]),
            _ => (' ', v),
        };
        let href = v.split(q).next().unwrap_or("").replace("&amp;", "&");
        let text_start = start + gt + 1;
        let end = lower[text_start..]
            .find("</a>")
            .map(|e| text_start + e)
            .unwrap_or(html.len());
        let text = html[text_start..end]
            .replace("&amp;", "&")
            .trim()
            .to_string();
        out.push((href, text));
        i = end;
    }
    out
}

/// `key = "value"` lines of a `.vmx` (keys lower-cased).
pub fn parse_vmx(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| {
            let (k, v) = l.split_once('=')?;
            let k = k.trim().to_ascii_lowercase();
            if k.is_empty() || k.starts_with('#') {
                return None;
            }
            Some((k, v.trim().trim_matches('"').to_string()))
        })
        .collect()
}

fn truthy(v: Option<&String>) -> bool {
    v.is_some_and(|s| s.eq_ignore_ascii_case("true"))
}

/// Map a `.vmx` to a plan. Disk `file`s are the *flat* extent names; the
/// descriptors (`<name>.vmdk`) the vmx names are returned for inspection.
pub fn plan_from_vmx(vmx: &HashMap<String, String>) -> Result<(OvfPlan, Vec<String>), String> {
    let mut warnings = Vec::new();
    let get = |k: &str| vmx.get(k);
    let name = get("displayname")
        .cloned()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "esxi-vm".into());
    let guest = get("guestos").cloned().unwrap_or_default();
    let windows = guest.to_ascii_lowercase().contains("windows");
    let cpus = get("numvcpus")
        .and_then(|c| c.parse::<u32>().ok())
        .unwrap_or(1);
    let (cpus_c, cw) = (cpus.clamp(1, 512), cpus == 0 || cpus > 512);
    if cw {
        warnings.push(format!(
            "{cpus} virtual CPUs is out of range: using {cpus_c}"
        ));
    }
    let memory_mb = get("memsize")
        .and_then(|m| m.parse::<u64>().ok())
        .unwrap_or(512)
        .max(16);
    let firmware = if get("firmware").is_some_and(|f| f.eq_ignore_ascii_case("efi")) {
        "efi"
    } else {
        "bios"
    };

    let mut disks = Vec::new();
    let mut descriptors = Vec::new();
    for bus_kind in ["scsi", "sata", "ide", "nvme"] {
        for ctl in 0..4 {
            let ctl_key = format!("{bus_kind}{ctl}");
            let dev = get(&format!("{ctl_key}.virtualdev"))
                .cloned()
                .unwrap_or_default();
            for unit in 0..32 {
                let k = format!("{ctl_key}:{unit}");
                if !truthy(get(&format!("{k}.present"))) {
                    continue;
                }
                let Some(file) = get(&format!("{k}.filename")) else {
                    continue;
                };
                let dtype = get(&format!("{k}.devicetype")).cloned().unwrap_or_default();
                if dtype.contains("cdrom") || file.to_ascii_lowercase().ends_with(".iso") {
                    warnings.push(format!(
                        "CD-ROM {k} not imported (attach an ISO under Hardware)"
                    ));
                    continue;
                }
                if !file.to_ascii_lowercase().ends_with(".vmdk") {
                    warnings.push(format!("{k}: {file} is not a .vmdk disk: skipped"));
                    continue;
                }
                if file.contains('/') || file.contains("..") || file.starts_with('.') {
                    return Err(format!(
                        "disk {k} is {file:?}: only disks in the VM's own folder can be imported"
                    ));
                }
                let mut bus = match bus_kind {
                    "sata" => "sata",
                    "ide" => "ide",
                    "nvme" => "virtio",
                    _ => "scsi",
                };
                if bus_kind == "scsi"
                    && !dev.is_empty()
                    && !dev.contains("lsi")
                    && !dev.contains("pvscsi")
                    && !dev.contains("buslogic")
                {
                    warnings.push(format!(
                        "{ctl_key} controller {dev:?} is unknown: disk on scsi"
                    ));
                }
                if windows && matches!(bus, "scsi" | "virtio") {
                    warnings.push(format!(
                        "disk {k}: Windows has no virtio/SCSI driver at boot, attached as sata"
                    ));
                    bus = "sata";
                }
                descriptors.push(file.clone());
                disks.push(OvfDisk {
                    file: format!("{}-flat.vmdk", file.trim_end_matches(".vmdk")),
                    capacity_bytes: None,
                    bus: bus.into(),
                    label: k,
                });
            }
        }
    }
    if disks.is_empty() {
        return Err("the VM has no .vmdk disk that could be imported".into());
    }

    let mut nics = Vec::new();
    for n in 0..10 {
        let k = format!("ethernet{n}");
        if !truthy(get(&format!("{k}.present"))) {
            continue;
        }
        let dev = get(&format!("{k}.virtualdev"))
            .cloned()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let mut model = if dev.contains("e1000") {
            "e1000"
        } else {
            "virtio"
        };
        if !dev.contains("e1000") && !dev.contains("virtio") && !dev.is_empty() && !windows {
            warnings.push(format!(
                "network card {k} ({dev}) has no QEMU equivalent: imported as virtio"
            ));
        }
        if windows && model == "virtio" {
            model = "e1000";
            warnings.push(format!(
                "network card {k} ({dev}): Windows has no virtio driver, imported as e1000"
            ));
        }
        nics.push(OvfNic {
            model: model.into(),
            network: get(&format!("{k}.networkname"))
                .cloned()
                .unwrap_or_default(),
        });
    }
    if get("sound.present").is_some_and(|v| v.eq_ignore_ascii_case("true")) {
        warnings.push("sound card not imported".into());
    }
    if truthy(get("usb.present")) || truthy(get("ehci.present")) || truthy(get("usb_xhci.present"))
    {
        warnings.push("USB controller not imported (add one under Hardware if needed)".into());
    }
    Ok((
        OvfPlan {
            name,
            os: (!guest.is_empty()).then_some(guest),
            cpus: cpus_c,
            memory_mb,
            firmware: firmware.into(),
            disks,
            nics,
            warnings,
        },
        descriptors,
    ))
}

/// Why a vmdk descriptor cannot be imported by copying its flat extent.
pub fn descriptor_problem(desc: &str, name: &str) -> Option<String> {
    let kv = |key: &str| {
        desc.lines().find_map(|l| {
            let (k, v) = l.split_once('=')?;
            (k.trim().eq_ignore_ascii_case(key)).then(|| v.trim().trim_matches('"').to_string())
        })
    };
    if kv("parentFileNameHint").is_some() {
        return Some(format!(
            "{name} is a snapshot (delta) disk: the VM has snapshots; delete or consolidate them in ESXi first"
        ));
    }
    match kv("createType").as_deref() {
        Some("vmfs") | Some("vmfsThin") | Some("monolithicFlat") => None,
        Some(t) => Some(format!(
            "{name} has disk type {t:?}: only flat / thin VMFS disks can be copied (vmfsSparse and sesparse are snapshots)"
        )),
        None => Some(format!("{name} is not a vmdk descriptor")),
    }
}

// ------------------------------------------------------------------ client

struct Esxi {
    client: reqwest::Client,
    base: String,
    user: String,
    pass: String,
    dc: String,
}

fn name_ok(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 200
        && n != "."
        && n != ".."
        && !n.starts_with('.')
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || " -_.()+".contains(c))
}

fn bad(e: impl std::fmt::Display) -> ApiError {
    ApiError::BadRequest(e.to_string())
}

impl Esxi {
    fn new(c: &Conn) -> Result<Self, ApiError> {
        let host = c.host.trim();
        if host.is_empty()
            || host.len() > 253
            || !host
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ".-:_[]".contains(ch))
        {
            return Err(bad(format!("{host:?} is not a host name or address")));
        }
        if c.username.is_empty() {
            return Err(bad("an ESXi user name is required"));
        }
        // a literal address is judged here (names are judged when resolved by
        // the guarded resolver): never the node itself or cloud metadata
        if let Ok(ip) = host.trim_matches(['[', ']']).parse::<std::net::IpAddr>() {
            if let Some(why) = crate::api::vm_import::refused_ip(ip) {
                return Err(ApiError::Forbidden(format!(
                    "refusing to connect to {host}: {why}"
                )));
            }
        }
        let dc = c
            .datacenter
            .clone()
            .unwrap_or_else(|| "ha-datacenter".into());
        if !name_ok(&dc) {
            return Err(bad("invalid datacenter name"));
        }
        let host = if host.contains(':') && !host.starts_with('[') {
            format!("[{host}]")
        } else {
            host.to_string()
        };
        let port = c.port.map(|p| format!(":{p}")).unwrap_or_default();
        let client = download_client_with(!c.verify_tls)
            .map_err(|e| ApiError::Internal(format!("http client: {e}")))?;
        Ok(Self {
            client,
            base: format!("https://{host}{port}"),
            user: c.username.clone(),
            pass: c.password.clone(),
            dc,
        })
    }

    fn url(&self, path: &[&str], ds: Option<&str>) -> String {
        let mut u = format!("{}/folder", self.base);
        for p in path {
            u.push('/');
            u.push_str(&urlencoding::encode(p));
        }
        u.push_str(&format!("?dcPath={}", urlencoding::encode(&self.dc)));
        if let Some(ds) = ds {
            u.push_str(&format!("&dsName={}", urlencoding::encode(ds)));
        }
        u
    }

    async fn send(
        &self,
        req: reqwest::RequestBuilder,
        what: &str,
    ) -> Result<reqwest::Response, ApiError> {
        let r = req
            .basic_auth(&self.user, Some(&self.pass))
            .send()
            .await
            .map_err(|e| {
                bad(format!(
                    "{what}: {}",
                    crate::api::vm_import::error_chain(&e)
                ))
            })?;
        match r.status().as_u16() {
            200..=299 => Ok(r),
            401 | 403 => Err(bad(format!(
                "{what}: the ESXi host refused the user name / password (HTTP {})",
                r.status()
            ))),
            404 => Err(bad(format!(
                "{what}: not found on the ESXi host (HTTP 404)"
            ))),
            s => Err(bad(format!("{what}: HTTP {s}"))),
        }
    }

    async fn text(&self, url: String, what: &str) -> Result<String, ApiError> {
        let r = self.send(self.client.get(url), what).await?;
        if r.content_length().is_some_and(|l| l > MAX_TEXT as u64) {
            return Err(bad(format!("{what}: the answer is too large")));
        }
        let b = r.bytes().await.map_err(|e| bad(format!("{what}: {e}")))?;
        if b.len() > MAX_TEXT {
            return Err(bad(format!("{what}: the answer is too large")));
        }
        Ok(String::from_utf8_lossy(&b).to_string())
    }

    /// Names (anchor texts without the trailing `/`) in a listing.
    async fn names(
        &self,
        path: &[&str],
        ds: Option<&str>,
        what: &str,
    ) -> Result<Vec<String>, ApiError> {
        let html = self.text(self.url(path, ds), what).await?;
        Ok(anchors(&html)
            .into_iter()
            .filter(|(h, _)| h.contains("/folder"))
            .map(|(_, t)| t.trim_end_matches('/').to_string())
            .filter(|t| !t.is_empty() && !t.starts_with("Parent") && t != "..")
            .collect())
    }

    async fn download(
        &self,
        path: &[&str],
        ds: &str,
        to: &FsPath,
        what: &str,
    ) -> Result<(), ApiError> {
        use tokio::io::AsyncWriteExt;
        let mut r = self
            .send(self.client.get(self.url(path, Some(ds))), what)
            .await?;
        let mut f = tokio::fs::File::create(to).await?;
        while let Some(chunk) = r.chunk().await.map_err(|e| bad(format!("{what}: {e}")))? {
            f.write_all(&chunk).await?;
        }
        f.flush().await?;
        f.sync_all().await?;
        Ok(())
    }

    async fn size(&self, path: &[&str], ds: &str) -> Option<u64> {
        let r = self
            .client
            .head(self.url(path, Some(ds)))
            .basic_auth(&self.user, Some(&self.pass))
            .send()
            .await
            .ok()?;
        r.status()
            .is_success()
            .then(|| r.content_length())
            .flatten()
    }
}

// ------------------------------------------------------------------ API

#[derive(Serialize)]
struct EsxiVm {
    folder: String,
    vmx: String,
}

#[derive(Serialize)]
struct EsxiDatastore {
    name: String,
    vms: Vec<EsxiVm>,
}

async fn list(Json(c): Json<Conn>) -> Result<Json<Vec<EsxiDatastore>>, ApiError> {
    let e = Esxi::new(&c)?;
    let mut out = Vec::new();
    for ds in e.names(&[], None, "listing datastores").await? {
        if !name_ok(&ds) {
            continue;
        }
        let mut vms = Vec::new();
        for folder in e
            .names(&[], Some(&ds), &format!("listing datastore {ds}"))
            .await?
        {
            if !name_ok(&folder) {
                continue;
            }
            let files = e
                .names(&[&folder], Some(&ds), &format!("listing {ds}/{folder}"))
                .await
                .unwrap_or_default();
            if let Some(vmx) = files
                .into_iter()
                .find(|f| f.to_ascii_lowercase().ends_with(".vmx"))
            {
                vms.push(EsxiVm { folder, vmx });
            }
        }
        out.push(EsxiDatastore { name: ds, vms });
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
struct Target {
    #[serde(flatten)]
    conn: Conn,
    datastore: String,
    folder: String,
}

/// Fetch the vmx (and descriptors) and build the plan; returns the VM's
/// files so the import can copy the flat disks.
async fn load(e: &Esxi, t: &Target, stage: &FsPath) -> Result<(OvfPlan, String), ApiError> {
    if !name_ok(&t.datastore) || !name_ok(&t.folder) {
        return Err(bad("invalid datastore or folder name"));
    }
    let (ds, folder) = (t.datastore.as_str(), t.folder.as_str());
    let files = e
        .names(&[folder], Some(ds), &format!("listing {ds}/{folder}"))
        .await?;
    if files.iter().any(|f| {
        f.to_ascii_lowercase().ends_with(".vmx.lck") || f.to_ascii_lowercase().ends_with(".lck")
    }) {
        return Err(ApiError::Conflict(format!(
            "{folder} has lock files: the VM looks powered on or suspended - power it off in ESXi first"
        )));
    }
    let vmx_name = files
        .iter()
        .find(|f| f.to_ascii_lowercase().ends_with(".vmx"))
        .ok_or_else(|| bad(format!("{ds}/{folder} has no .vmx file")))?
        .clone();
    if !name_ok(&vmx_name) {
        return Err(bad("invalid .vmx file name"));
    }
    let vmx_text = e
        .text(e.url(&[folder, &vmx_name], Some(ds)), "reading the .vmx")
        .await?;
    let (mut plan, descriptors) = plan_from_vmx(&parse_vmx(&vmx_text)).map_err(bad)?;
    for (d, desc_name) in plan.disks.iter_mut().zip(&descriptors) {
        if !name_ok(desc_name) {
            return Err(bad(format!("invalid disk file name {desc_name:?}")));
        }
        let desc = e
            .text(
                e.url(&[folder, desc_name], Some(ds)),
                &format!("reading {desc_name}"),
            )
            .await?;
        if let Some(p) = descriptor_problem(&desc, desc_name) {
            return Err(ApiError::Conflict(p));
        }
        if !files.contains(&d.file) {
            return Err(bad(format!(
                "{} (the data of {desc_name}) is not in {ds}/{folder}",
                d.file
            )));
        }
        d.capacity_bytes = e.size(&[folder, &d.file], ds).await;
    }
    let _ = stage;
    Ok((plan, vmx_name))
}

#[derive(Serialize)]
struct PreviewResponse {
    plan: OvfPlan,
    id: String,
}

async fn preview(
    State(state): State<Arc<AppState>>,
    Json(t): Json<Target>,
) -> Result<Json<PreviewResponse>, ApiError> {
    let e = Esxi::new(&t.conn)?;
    let stage = PathBuf::new();
    let (plan, _) = load(&e, &t, &stage).await?;
    let id = free_id(&state, &plan.name).await;
    Ok(Json(PreviewResponse { plan, id }))
}

#[derive(Deserialize)]
struct ImportRequest {
    #[serde(flatten)]
    target: Target,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    bridge: Option<String>,
    #[serde(default)]
    storage: Option<String>,
}

async fn import(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ImportRequest>,
) -> Result<(StatusCode, Json<ImportResponse>), ApiError> {
    let stage = stage_dir(&state)?;
    let mut created: Option<String> = None;
    let res = run(&state, &req, &stage, &mut created).await;
    let _ = std::fs::remove_dir_all(&stage);
    match res {
        Ok(r) => Ok((StatusCode::CREATED, Json(r))),
        Err(e) => {
            if let Some(id) = created {
                let _ = state.vm_manager.delete_vm(&id).await;
            }
            Err(e)
        }
    }
}

async fn run(
    state: &Arc<AppState>,
    req: &ImportRequest,
    stage: &FsPath,
    created: &mut Option<String>,
) -> Result<ImportResponse, ApiError> {
    let e = Esxi::new(&req.target.conn)?;
    let (plan, _vmx) = load(&e, &req.target, stage).await?;
    let (ds, folder) = (req.target.datastore.as_str(), req.target.folder.as_str());
    // copy the flat disks next to each other in the staging directory
    let dir = std::fs::canonicalize(stage)?;
    let mut files = Vec::new();
    for d in &plan.disks {
        let to = dir.join(&d.file);
        files.push(to.clone());
        e.download(&[folder, &d.file], ds, &to, &format!("copying {}", d.file))
            .await?;
    }
    let src = Source {
        ovf: String::new(),
        ova: None,
        dir: Some(dir),
        _tmp: TempFiles(files),
    };
    let ov = Overrides {
        id: req.id.clone(),
        name: req.name.clone(),
        bridge: req.bridge.clone(),
        storage: req.storage.clone(),
        label: format!("ESXi {}:{ds}/{folder}", req.target.conn.host),
    };
    import_plan(state, &ov, plan, &src, stage, created).await
}

#[cfg(test)]
mod tests {
    use super::*;

    const VMX: &str = r#"
.encoding = "UTF-8"
displayName = "web01"
guestOS = "ubuntu-64"
numvcpus = "4"
memsize = "4096"
firmware = "efi"
scsi0.present = "TRUE"
scsi0.virtualDev = "pvscsi"
scsi0:0.present = "TRUE"
scsi0:0.fileName = "web01.vmdk"
scsi0:1.present = "TRUE"
scsi0:1.fileName = "web01_1.vmdk"
sata0.present = "TRUE"
sata0:0.present = "TRUE"
sata0:0.deviceType = "cdrom-image"
sata0:0.fileName = "/vmfs/volumes/ds/ubuntu.iso"
ethernet0.present = "TRUE"
ethernet0.virtualDev = "vmxnet3"
ethernet0.networkName = "VM Network"
ethernet1.present = "TRUE"
ethernet1.virtualDev = "e1000e"
sound.present = "TRUE"
"#;

    #[test]
    fn vmx_maps_to_a_plan_with_flat_extents() {
        let (p, desc) = plan_from_vmx(&parse_vmx(VMX)).unwrap();
        assert_eq!(
            (p.name.as_str(), p.cpus, p.memory_mb, p.firmware.as_str()),
            ("web01", 4, 4096, "efi")
        );
        assert_eq!(desc, vec!["web01.vmdk", "web01_1.vmdk"]);
        assert_eq!(p.disks[0].file, "web01-flat.vmdk");
        assert_eq!(p.disks[1].file, "web01_1-flat.vmdk");
        assert!(p.disks.iter().all(|d| d.bus == "scsi"));
        assert_eq!(
            p.nics.iter().map(|n| n.model.as_str()).collect::<Vec<_>>(),
            vec!["virtio", "e1000"]
        );
        let w = p.warnings.join("\n");
        assert!(
            w.contains("CD-ROM") && w.contains("vmxnet3") && w.contains("sound"),
            "{w}"
        );
    }

    #[test]
    fn windows_guests_and_bad_disks() {
        let win = VMX.replace("ubuntu-64", "windows2019srv-64");
        let (p, _) = plan_from_vmx(&parse_vmx(&win)).unwrap();
        assert!(p.disks.iter().all(|d| d.bus == "sata"));
        assert!(p.nics.iter().all(|n| n.model == "e1000"));
        let evil = VMX.replace("web01.vmdk", "../other/web01.vmdk");
        assert!(plan_from_vmx(&parse_vmx(&evil))
            .unwrap_err()
            .contains("own folder"));
        let sub = VMX.replace("\"web01.vmdk\"", "\"/vmfs/volumes/x/web01.vmdk\"");
        assert!(plan_from_vmx(&parse_vmx(&sub)).is_err());
        assert!(plan_from_vmx(&parse_vmx("displayName = \"x\""))
            .unwrap_err()
            .contains("no .vmdk"));
    }

    #[test]
    fn descriptors_with_snapshots_or_sparse_types_are_refused() {
        let ok = "# Disk DescriptorFile\ncreateType=\"vmfsThin\"\n";
        assert!(descriptor_problem(ok, "a.vmdk").is_none());
        assert!(descriptor_problem("createType=\"vmfs\"\n", "a.vmdk").is_none());
        let snap = "createType=\"vmfsSparse\"\nparentFileNameHint=\"a.vmdk\"\n";
        assert!(descriptor_problem(snap, "a-000001.vmdk")
            .unwrap()
            .contains("snapshot"));
        assert!(descriptor_problem("createType=\"seSparse\"\n", "a.vmdk")
            .unwrap()
            .contains("seSparse"));
        assert!(descriptor_problem("hello", "a.vmdk").is_some());
    }

    #[test]
    fn listing_html_yields_hrefs_and_names() {
        let html = r#"<html><body><h1>Datastores</h1><table>
<tr><td><a href="/folder?dcPath=ha%2ddatacenter&amp;dsName=datastore1">datastore1</a></td></tr>
<tr><td><a HREF='/folder/web01?dcPath=ha%2ddatacenter&amp;dsName=ds'>web01/</a></td></tr>
<tr><td><a href="/ui">Back &amp; up</a></td></tr></table></body></html>"#;
        let a = anchors(html);
        assert_eq!(a.len(), 3);
        assert_eq!(
            a[0],
            (
                "/folder?dcPath=ha%2ddatacenter&dsName=datastore1".into(),
                "datastore1".into()
            )
        );
        assert_eq!(a[1].1, "web01/");
        assert_eq!(a[2].1, "Back & up");
    }

    #[test]
    fn connection_input_is_validated() {
        let c = |h: &str| Conn {
            host: h.into(),
            username: "root".into(),
            password: "x".into(),
            verify_tls: false,
            datacenter: None,
            port: None,
        };
        assert!(Esxi::new(&c("esxi1.lab")).is_ok());
        assert!(Esxi::new(&c("10.0.0.5")).is_ok());
        assert!(Esxi::new(&c("a b")).is_err());
        assert!(Esxi::new(&c("h/../x")).is_err());
        assert!(Esxi::new(&c("")).is_err());
        for refused in ["127.0.0.1", "169.254.169.254", "0.0.0.0", "[::1]", "::1"] {
            assert!(
                matches!(Esxi::new(&c(refused)), Err(ApiError::Forbidden(_))),
                "{refused}"
            );
        }
        let e = Esxi::new(&c("esxi1.lab")).unwrap();
        assert_eq!(
            e.url(&["web 01", "a.vmx"], Some("data store")),
            "https://esxi1.lab/folder/web%2001/a.vmx?dcPath=ha-datacenter&dsName=data%20store"
        );
        assert!(
            !name_ok("../x") && !name_ok("a/b") && !name_ok(".hidden") && name_ok("web 01 (old)")
        );
    }
}
