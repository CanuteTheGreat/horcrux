//! Import (adopt) existing libvirt/QEMU domains: a VM that was defined
//! directly in libvirt (virt-manager, `virsh define`, a migration script) is
//! invisible to Horcrux. These routes list such domains, preview what they
//! become, and import them.
//!
//! * `POST /api/vms/import/libvirt/list` - every libvirt domain of this host
//!   with the Horcrux VM that already owns its disk (if any). A domain with
//!   `imported_as: null` is *unmanaged*: the VM list shows it with an
//!   "Import" action, so a VM can never stay invisible.
//! * `POST /api/vms/import/libvirt/preview {domain}` - what the import would
//!   create and everything that cannot be mapped (warnings); nothing changes.
//! * `POST /api/vms/import/libvirt {domain, id?, name?, bridge?, disks?,
//!   undefine?}` - creates the stopped Horcrux VM (201).
//!
//! Mapping: CPUs (and sockets), memory, machine type (q35/i440fx), CPU model,
//! disks (qcow2; bus virtio/scsi/sata/ide, discard), NICs (model, **MAC kept**,
//! bridge), UEFI (OVMF) + Secure Boot, the OVMF variable store (`<nvram>`,
//! converted to the VM's `<disk0>.efivars.qcow2` when its layout matches this
//! host's OVMF) and the swtpm vTPM (libvirt's state directory is copied to
//! `<disk0>.tpm/`), guest agent, balloon, xHCI.
//!
//! Disks: always **copied** into Horcrux's own storage (the VM storage
//! directory or the chosen pool: where a newly created VM's disk goes),
//! at idle I/O priority with progress, checked with `qemu-img compare`, and
//! only then does the VM point at them. A VM never references a libvirt path.
//! The OVMF variable store and swtpm state are copied next to the first disk.
//!
//! Safety: the domain must be shut off; free space is checked first; the
//! libvirt XML is saved (`<data_dir>/libvirt-import/<id>.xml`); the import
//! deletes nothing of the source: the old libvirt definition, disk files,
//! NVRAM and TPM copies are offered for removal through `api::import_cleanup`
//! (opt-in, with sizes). A failed import deletes only the new copies.
//! `POST /api/vms/import/libvirt` answers 202 `{job}`; poll
//! `POST /api/vms/import/libvirt/status {job}` for `percent`/`result`.
use crate::api::ovf::{parse_xml, Node};
use crate::api::vm_import::{checked_format, qemu_img, qemu_img_error};
use crate::error::ApiError;
use crate::AppState;
use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use horcrux_common::VmConfig;
use serde::{Deserialize, Serialize};
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use tokio::process::Command;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/vms/import/libvirt/list", post(list))
        .route("/api/vms/import/libvirt/list-cluster", post(list_cluster))
        .route("/api/ha/peer/libvirt/list", post(peer_list))
        .route("/api/vms/import/libvirt/preview", post(preview))
        .route("/api/vms/import/libvirt", post(import))
        .route("/api/vms/import/libvirt/status", post(status))
}

// ------------------------------------------------------------------ virsh

fn virsh_bin() -> String {
    std::env::var("HORCRUX_VIRSH").unwrap_or_else(|_| "virsh".into())
}

fn libvirt_uri() -> String {
    std::env::var("HORCRUX_LIBVIRT_URI").unwrap_or_else(|_| "qemu:///system".into())
}

/// libvirt's swtpm state root (`<root>/<uuid>/tpm2/`).
fn swtpm_root() -> PathBuf {
    std::env::var("HORCRUX_LIBVIRT_SWTPM_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/var/lib/libvirt/swtpm"))
}

/// `Ok(None)`: virsh is not installed (libvirt is not used on this host).
pub(crate) async fn virsh(args: &[&str]) -> Result<Option<String>, ApiError> {
    let out = Command::new(virsh_bin())
        .arg("-c")
        .arg(libvirt_uri())
        .args(args)
        .output()
        .await;
    match out {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ApiError::Internal(format!("virsh: {e}"))),
        Ok(o) if o.status.success() => Ok(Some(String::from_utf8_lossy(&o.stdout).to_string())),
        Ok(o) => Err(ApiError::BadRequest(format!(
            "virsh {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&o.stderr).trim()
        ))),
    }
}

// ------------------------------------------------------------------ plan

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LvDisk {
    pub path: String,
    pub format: String,
    /// `virtio`, `scsi`, `sata` or `ide`.
    pub bus: String,
    pub discard: bool,
    pub target: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LvNic {
    pub mac: Option<String>,
    /// `virtio`, `e1000`, `e1000e`, `rtl8139` or `vmxnet3`.
    pub model: String,
    /// libvirt network or bridge name (informational).
    pub network: String,
    /// Host bridge named by the domain (`type='bridge'`).
    pub bridge: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LibvirtPlan {
    pub domain: String,
    pub uuid: String,
    pub name: String,
    pub description: Option<String>,
    pub cpus: u32,
    pub sockets: Option<u32>,
    pub memory_mb: u64,
    /// `q35` or `pc`
    pub machine: String,
    /// `bios` or `efi`
    pub firmware: String,
    pub secure_boot: bool,
    pub tpm: bool,
    pub nvram: Option<String>,
    /// libvirt's swtpm state directory when it exists.
    pub tpm_state: Option<String>,
    pub cpu_type: Option<String>,
    pub guest_agent: bool,
    pub balloon: bool,
    pub usb_controller: bool,
    pub disks: Vec<LvDisk>,
    pub nics: Vec<LvNic>,
    pub warnings: Vec<String>,
}

async fn free_id_with(state: &AppState, want: &str) -> String {
    let mut id = want.to_string();
    let mut n = 2;
    while state.vm_manager.get_vm(&id).await.is_ok() {
        id = format!("{want}-{n}");
        n += 1;
    }
    id
}

fn kib(n: &Node) -> u64 {
    let v: u64 = n.text.trim().parse().unwrap_or(0);
    match n.attr("unit").unwrap_or("KiB") {
        "b" | "bytes" => v / 1024,
        "KB" => v * 1000 / 1024,
        "MB" => v * 1000 * 1000 / 1024,
        "GB" => v * 1000 * 1000 * 1000 / 1024,
        "M" | "MiB" => v << 10,
        "G" | "GiB" => v << 20,
        "T" | "TiB" => v << 30,
        _ => v,
    }
}

/// Map a libvirt domain XML to a plan. `swtpm_root` is where libvirt keeps
/// vTPM state.
pub fn plan_from_domain(xml: &str, swtpm_root: &FsPath) -> Result<LibvirtPlan, String> {
    let d = parse_xml(xml)?;
    if d.name != "domain" {
        return Err(format!("not a libvirt domain (root element <{}>)", d.name));
    }
    if d.attr("type") != Some("kvm") && d.attr("type") != Some("qemu") {
        return Err(format!(
            "domain type {:?} is not QEMU/KVM",
            d.attr("type").unwrap_or_default()
        ));
    }
    let mut w: Vec<String> = Vec::new();
    let name = d.child_text("name").ok_or("the domain has no name")?;
    let uuid = d.child_text("uuid").unwrap_or_default();
    let description = d.child_text("description");
    let mem_kib = d
        .child("memory")
        .map(kib)
        .filter(|m| *m > 0)
        .ok_or("the domain has no memory size")?;
    let memory_mb = (mem_kib / 1024).max(1);
    let cpus: u32 = d
        .child_text("vcpu")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1)
        .clamp(1, 512);

    // CPU model + topology
    let mut cpu_type = None;
    let mut sockets = None;
    if let Some(c) = d.child("cpu") {
        match c.attr("mode").unwrap_or("custom") {
            "host-passthrough" | "host-model" | "maximum" => cpu_type = Some("host".to_string()),
            _ => {
                if let Some(m) = c.child_text("model") {
                    if horcrux_common::valid_cpu_type(&m) {
                        cpu_type = Some(m);
                    } else {
                        w.push(format!(
                            "CPU model {m:?} is not usable: QEMU's default is used"
                        ));
                    }
                }
            }
        }
        if let Some(t) = c.child("topology") {
            let g = |k: &str| t.attr(k).and_then(|v| v.parse::<u32>().ok()).unwrap_or(1);
            let (s, co, th) = (g("sockets"), g("cores"), g("threads"));
            if s > 1 && s * co * th == cpus && th == 1 {
                sockets = Some(s);
            } else if s * co * th != cpus {
                w.push("the CPU topology does not match the vCPU count: one socket".into());
            }
        }
    }

    // machine, firmware
    let os = d.child("os").ok_or("the domain has no <os>")?;
    let mt = os
        .child("type")
        .and_then(|t| t.attr("machine"))
        .unwrap_or("pc");
    let machine = if mt.contains("q35") {
        "q35"
    } else if mt == "pc" || mt.contains("i440fx") {
        "pc"
    } else {
        w.push(format!(
            "machine type {mt:?} is not supported: i440FX is used"
        ));
        "pc"
    };
    let loader = os.child("loader");
    let efi = os.attr("firmware") == Some("efi")
        || loader.is_some_and(|l| l.attr("type") == Some("pflash"));
    let mut secure_boot = loader.is_some_and(|l| l.attr("secure") == Some("yes"));
    if let Some(f) = os.child("firmware") {
        for ft in f.kids("feature") {
            if ft.attr("name") == Some("secure-boot") && ft.attr("enabled") == Some("yes") {
                secure_boot = true;
            }
        }
    }
    if secure_boot && !efi {
        secure_boot = false;
    }
    let nvram = os.child_text("nvram").filter(|p| p.starts_with('/'));

    let dev = d.child("devices").ok_or("the domain has no <devices>")?;

    // disks
    let mut disks: Vec<(u32, LvDisk)> = Vec::new();
    for dk in dev.kids("disk") {
        let kind = dk.attr("device").unwrap_or("disk");
        let target = dk
            .child("target")
            .and_then(|t| t.attr("dev"))
            .unwrap_or("?")
            .to_string();
        if kind != "disk" {
            w.push(format!(
                "{kind} drive {target} is not imported (attach media under Hardware)"
            ));
            continue;
        }
        let Some(path) = dk
            .child("source")
            .and_then(|s| s.attr("file"))
            .filter(|p| p.starts_with('/'))
        else {
            return Err(format!(
                "disk {target} is not a plain image file (type {:?}): only file-backed disks can be imported",
                dk.attr("type").unwrap_or_default()
            ));
        };
        let format = dk
            .child("driver")
            .and_then(|x| x.attr("type"))
            .unwrap_or("raw")
            .to_string();
        let bus = match dk.child("target").and_then(|t| t.attr("bus")).unwrap_or("") {
            "virtio" => "virtio",
            "scsi" => "scsi",
            "sata" => "sata",
            "ide" => "ide",
            other => {
                w.push(format!(
                    "disk {target}: bus {other:?} is not supported, it is attached as SATA"
                ));
                "sata"
            }
        }
        .to_string();
        if dk.child("readonly").is_some() {
            w.push(format!(
                "disk {target} is read-only in libvirt: it becomes read-write"
            ));
        }
        let order = dk
            .child("boot")
            .and_then(|b| b.attr("order"))
            .and_then(|o| o.parse().ok())
            .unwrap_or(u32::MAX);
        disks.push((
            order,
            LvDisk {
                path: path.to_string(),
                format,
                bus,
                discard: dk.child("driver").and_then(|x| x.attr("discard")) == Some("unmap"),
                target,
            },
        ));
    }
    disks.sort_by_key(|(o, _)| *o); // stable: boot disk first, then document order
    let disks: Vec<LvDisk> = disks.into_iter().map(|(_, d)| d).collect();

    // NICs
    let mut nics = Vec::new();
    for i in dev.kids("interface") {
        let model = match i.child("model").and_then(|m| m.attr("type")).unwrap_or("") {
            "virtio" | "virtio-net" | "virtio-net-pci" => "virtio",
            "e1000" => "e1000",
            "e1000e" => "e1000e",
            "rtl8139" => "rtl8139",
            "vmxnet3" => "vmxnet3",
            other => {
                w.push(format!("network card model {other:?} becomes virtio"));
                "virtio"
            }
        };
        let src = i.child("source");
        let bridge = (i.attr("type") == Some("bridge"))
            .then(|| src.and_then(|s| s.attr("bridge")).map(String::from))
            .flatten();
        let network = src
            .and_then(|s| s.attr("network").or(s.attr("bridge")))
            .unwrap_or_default()
            .to_string();
        if !matches!(i.attr("type"), Some("network") | Some("bridge")) {
            w.push(format!(
                "network card of type {:?} is attached to a host bridge instead",
                i.attr("type").unwrap_or_default()
            ));
        }
        nics.push(LvNic {
            mac: i
                .child("mac")
                .and_then(|m| m.attr("address"))
                .map(|m| m.to_ascii_lowercase()),
            model: model.into(),
            network,
            bridge,
        });
    }

    // TPM
    let mut tpm = false;
    let mut tpm_state = None;
    if let Some(t) = dev.child("tpm") {
        match t.child("backend").and_then(|b| b.attr("type")) {
            Some("emulator") => {
                tpm = true;
                let dir = swtpm_root.join(&uuid).join("tpm2");
                if !uuid.is_empty() && dir.is_dir() {
                    tpm_state = Some(dir.to_string_lossy().to_string());
                } else {
                    w.push("the domain has a vTPM but libvirt holds no TPM state for it (never started): a fresh TPM is created at the first start".into());
                }
                if t.attr("model") == Some("tpm-crb") {
                    w.push("the TPM device is tpm-tis instead of tpm-crb (the TPM state is the same)".into());
                }
            }
            _ => w.push(
                "a pass-through host TPM cannot be imported: the VM has no TPM (enable the emulated TPM under Hardware)"
                    .into(),
            ),
        }
    }

    let guest_agent = dev
        .kids("channel")
        .any(|c| c.child("target").and_then(|t| t.attr("name")) == Some("org.qemu.guest_agent.0"));
    let balloon = dev
        .child("memballoon")
        .is_some_and(|m| m.attr("model") != Some("none"));
    let usb_controller = dev
        .kids("controller")
        .any(|c| c.attr("type") == Some("usb") && c.attr("model") != Some("none"));
    for h in dev.kids("hostdev") {
        w.push(format!(
            "host device passthrough ({}) is not imported (add it under Hardware)",
            h.attr("type").unwrap_or("hostdev")
        ));
    }
    if d.child("clock").and_then(|c| c.attr("offset")) == Some("localtime") {
        w.push("the guest clock was set to local time; Horcrux VMs keep UTC: Windows corrects it with time sync".into());
    }
    if d.child("features")
        .and_then(|f| f.child("hyperv"))
        .is_some()
    {
        w.push("Hyper-V enlightenments are not imported (the guest runs without them)".into());
    }
    for g in dev.kids("graphics") {
        w.push(format!(
            "{} display becomes a VNC console",
            g.attr("type").unwrap_or("graphics")
        ));
    }
    if disks.is_empty() {
        return Err("the domain has no disk".into());
    }
    Ok(LibvirtPlan {
        domain: name.clone(),
        uuid,
        name,
        description,
        cpus,
        sockets,
        memory_mb,
        machine: machine.into(),
        firmware: if efi { "efi" } else { "bios" }.into(),
        secure_boot,
        tpm,
        nvram,
        tpm_state,
        cpu_type,
        guest_agent,
        balloon,
        usb_controller,
        disks,
        nics,
        warnings: w,
    })
}

// ------------------------------------------------------------------ routes

#[derive(Deserialize)]
struct DomainRequest {
    domain: String,
}

fn check_domain_name(n: &str) -> Result<&str, ApiError> {
    let n = n.trim();
    if n.is_empty()
        || n.len() > 128
        || n.starts_with('-')
        || n.chars().any(|c| c.is_control() || c == '/')
    {
        return Err(ApiError::BadRequest("invalid libvirt domain name".into()));
    }
    Ok(n)
}

async fn domain_xml(domain: &str) -> Result<String, ApiError> {
    virsh(&["dumpxml", "--inactive", domain])
        .await?
        .ok_or_else(|| ApiError::BadRequest("libvirt (virsh) is not installed on this host".into()))
}

#[derive(Serialize, Deserialize, Clone)]
struct ListedDomain {
    domain: String,
    uuid: String,
    state: String,
    cpus: u32,
    memory_mb: u64,
    disks: Vec<String>,
    /// Horcrux VM that already uses the domain's first disk.
    imported_as: Option<String>,
    /// Why it cannot be imported (the domain's XML is unusable).
    problem: Option<String>,
    /// An import of this domain is running or its last one failed (accurate
    /// state instead of plain "unmanaged"); `None` otherwise.
    #[serde(default)]
    import_state: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
struct ListResponse {
    /// False when virsh is not installed.
    available: bool,
    domains: Vec<ListedDomain>,
}

async fn list(State(state): State<Arc<AppState>>) -> Result<Json<ListResponse>, ApiError> {
    Ok(Json(list_local(&state).await?))
}

/// One node's unmanaged-VM report in the cluster-wide list.
#[derive(Serialize, Debug, PartialEq)]
struct NodeReport {
    node: String,
    /// The node that answered this request (imports run on it from its own UI/API).
    local: bool,
    /// Set when the node could not be asked.
    error: Option<String>,
    available: bool,
    domains: Vec<ListedDomain>,
}

#[derive(Serialize, Debug, PartialEq)]
struct ClusterList {
    nodes: Vec<NodeReport>,
    /// Domains without a Horcrux VM, over every node that answered.
    unmanaged: usize,
}

impl PartialEq for ListedDomain {
    fn eq(&self, o: &Self) -> bool {
        self.domain == o.domain
            && self.uuid == o.uuid
            && self.imported_as == o.imported_as
            && self.import_state == o.import_state
    }
}
impl std::fmt::Debug for ListedDomain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ListedDomain({})", self.domain)
    }
}

/// Combine this node's list with what each peer answered (pure, tested).
fn merge_cluster(
    local_name: &str,
    local: ListResponse,
    peers: Vec<(String, Result<ListResponse, String>)>,
) -> ClusterList {
    let mut nodes = vec![NodeReport {
        node: local_name.to_string(),
        local: true,
        error: None,
        available: local.available,
        domains: local.domains,
    }];
    for (name, r) in peers {
        nodes.push(match r {
            Ok(l) => NodeReport {
                node: name,
                local: false,
                error: None,
                available: l.available,
                domains: l.domains,
            },
            Err(e) => NodeReport {
                node: name,
                local: false,
                error: Some(e),
                available: false,
                domains: vec![],
            },
        });
    }
    let unmanaged = nodes
        .iter()
        .flat_map(|n| n.domains.iter())
        .filter(|d| d.imported_as.is_none() && d.problem.is_none())
        .count();
    ClusterList { nodes, unmanaged }
}

/// `POST /api/vms/import/libvirt/list-cluster`: the same list for every online
/// node of the cluster (remote nodes are asked over the cluster peer API).
async fn list_cluster(State(state): State<Arc<AppState>>) -> Result<Json<ClusterList>, ApiError> {
    let local_name = state
        .cluster_manager
        .get_local_node_name()
        .await
        .unwrap_or_else(|_| "localhost".to_string());
    let local = list_local(&state).await?;
    let all = state.cluster_manager.list_nodes().await;
    // Peers are asked over the cluster peer API, which only exists in a `cluster` build; a build
    // without it is a one-node list.
    #[cfg(feature = "cluster")]
    let peers = {
        let mut peers = Vec::new();
        for n in super::drain::peer_nodes(&all, &local_name) {
            let r = super::drain::call_peer::<_, ListResponse>(
                &state,
                &n,
                "/api/ha/peer/libvirt/list",
                &serde_json::json!({}),
            )
            .await;
            peers.push((n.name.clone(), r));
        }
        peers
    };
    #[cfg(not(feature = "cluster"))]
    let peers: Vec<(String, Result<ListResponse, String>)> = {
        let _ = &all;
        Vec::new()
    };
    Ok(Json(merge_cluster(&local_name, local, peers)))
}

/// Peer route: another cluster node asks for this node's list.
async fn peer_list(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::middleware::auth::AuthUser>>,
) -> Result<Json<ListResponse>, ApiError> {
    match &user {
        Some(axum::Extension(u)) if u.role == crate::middleware::auth::CLUSTER_HA_ROLE => {}
        _ => {
            return Err(ApiError::Forbidden(
                "this route is only for other cluster nodes".into(),
            ))
        }
    }
    Ok(Json(list_local(&state).await?))
}

async fn list_local(state: &Arc<AppState>) -> Result<ListResponse, ApiError> {
    let Some(names) = virsh(&["list", "--all", "--name"]).await? else {
        return Ok(ListResponse {
            available: false,
            domains: vec![],
        });
    };
    let vms = state.vm_manager.list_vms().await;
    let vm_ids: std::collections::HashSet<String> = vms.iter().map(|v| v.id.clone()).collect();
    let sources = recorded_sources(state);
    let mut domains = Vec::new();
    for n in names.lines().map(str::trim).filter(|n| !n.is_empty()) {
        let st = virsh(&["domstate", n])
            .await
            .ok()
            .flatten()
            .unwrap_or_default()
            .trim()
            .to_string();
        let mut e = ListedDomain {
            domain: n.to_string(),
            uuid: String::new(),
            state: st,
            cpus: 0,
            memory_mb: 0,
            disks: vec![],
            imported_as: None,
            problem: None,
            import_state: import_job_state(n),
        };
        match domain_xml(n)
            .await
            .and_then(|x| plan_from_domain(&x, &swtpm_root()).map_err(ApiError::BadRequest))
        {
            Ok(p) => {
                e.uuid = p.uuid;
                e.cpus = p.cpus;
                e.memory_mb = p.memory_mb;
                e.disks = p.disks.iter().map(|d| d.path.clone()).collect();
                e.imported_as = vms
                    .iter()
                    .find(|v| v.disks.iter().any(|vd| e.disks.contains(&vd.path)))
                    .map(|v| v.id.clone());
            }
            Err(err) => e.problem = Some(format!("{err:?}")),
        }
        // The disks of a finished import are Horcrux's own copies, so the path match above misses
        // the old source: the importer's own records say which domain became which VM.
        if e.imported_as.is_none() {
            e.imported_as = source_owner(&sources, &vm_ids, &e.domain, &e.uuid);
        }
        if e.imported_as.is_some() {
            e.import_state = None;
        }
        domains.push(e);
    }
    Ok(ListResponse {
        available: true,
        domains,
    })
}

/// A libvirt domain that an import recorded as its source.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RecordedSource {
    pub vm_id: String,
    /// Domain name (cleanup record) or `None` when only the XML backup is known.
    pub name: Option<String>,
    /// Domain UUID (the XML saved by the import) or `None` for an older record.
    pub uuid: Option<String>,
}

/// Every source the importer recorded: the cleanup records (`import-cleanup/<vm>.json`, names)
/// and the saved XML of each import (`libvirt-import/<vm>.xml`, UUID + name).
pub(crate) fn recorded_sources(state: &AppState) -> Vec<RecordedSource> {
    let mut out: Vec<RecordedSource> = Vec::new();
    for rec in crate::api::import_cleanup::list_records(state) {
        for it in rec.items.iter().filter(|i| i.kind == "libvirt-domain") {
            out.push(RecordedSource {
                vm_id: rec.vm_id.clone(),
                name: Some(it.path.clone()),
                uuid: None,
            });
        }
    }
    let bdir = state.config.paths.data_dir.join("libvirt-import");
    if let Ok(rd) = std::fs::read_dir(bdir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            let Some(vm_id) = n.strip_suffix(".xml") else {
                continue;
            };
            let Ok(xml) = std::fs::read_to_string(e.path()) else {
                continue;
            };
            if let Ok(d) = parse_xml(&xml) {
                out.push(RecordedSource {
                    vm_id: vm_id.to_string(),
                    name: d.child_text("name"),
                    uuid: d.child_text("uuid").filter(|u| !u.is_empty()),
                });
            }
        }
    }
    out
}

/// The Horcrux VM that `domain` (name + UUID) was imported as, if a recorded source matches and
/// that VM still exists (pure, tested). A UUID match wins; a name match is accepted only when the
/// record has no UUID to contradict it.
pub(crate) fn source_owner(
    sources: &[RecordedSource],
    live_vm_ids: &std::collections::HashSet<String>,
    domain: &str,
    uuid: &str,
) -> Option<String> {
    sources
        .iter()
        .filter(|s| live_vm_ids.contains(&s.vm_id))
        .find(|s| match (&s.uuid, &s.name) {
            (Some(u), _) if !uuid.is_empty() => u == uuid,
            (_, Some(n)) => n == domain,
            _ => false,
        })
        .map(|s| s.vm_id.clone())
}

/// `importing (42%)` / `last import failed: ...` for a domain with an import job on record.
fn import_job_state(domain: &str) -> Option<String> {
    let j = jobs().lock().ok()?;
    let mut mine: Vec<&Job> = j.values().filter(|x| x.domain == domain).collect();
    mine.sort_by_key(|x| x.seq);
    if let Some(r) = mine.iter().find(|x| x.state == "running") {
        return Some(format!("import in progress ({:.0}%)", r.percent));
    }
    match mine.last() {
        Some(x) if x.state == "failed" => Some(format!(
            "last import failed: {}",
            x.error.as_deref().unwrap_or("unknown error")
        )),
        _ => None,
    }
}

#[derive(Serialize)]
struct PreviewResponse {
    plan: LibvirtPlan,
    id: String,
}

fn default_id(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let s: String = s.trim_matches('-').chars().take(40).collect();
    if s.is_empty() {
        "vm".into()
    } else {
        s
    }
}

async fn preview(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DomainRequest>,
) -> Result<Json<PreviewResponse>, ApiError> {
    let dom = check_domain_name(&req.domain)?;
    let xml = domain_xml(dom).await?;
    let mut plan = plan_from_domain(&xml, &swtpm_root()).map_err(ApiError::BadRequest)?;
    let st = virsh(&["domstate", dom]).await?.unwrap_or_default();
    if st.trim() != "shut off" {
        plan.warnings.push(format!(
            "the domain is {}: shut it down before importing",
            st.trim()
        ));
    }
    let id = free_id_with(&state, &default_id(&plan.name)).await;
    Ok(Json(PreviewResponse { plan, id }))
}

#[derive(Deserialize)]
struct ImportRequest {
    domain: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    /// Host bridge for NICs that do not name one (default: the bridge of the
    /// libvirt network, else the first bridge).
    #[serde(default)]
    bridge: Option<String>,
    /// Storage pool for the disks (default: the VM storage directory, where
    /// a newly created VM's disk goes too).
    #[serde(default)]
    storage: Option<String>,
}

#[derive(Serialize)]
struct ImportResponse {
    vm: VmConfig,
    plan: LibvirtPlan,
    warnings: Vec<String>,
    /// An opt-in cleanup of the old libvirt definition and the old disk
    /// files is offered (`/api/vms/import/cleanup`).
    cleanup_offered: bool,
    xml_backup: String,
}

// ------------------------------------------------------------------ jobs

/// A running or finished import (a big disk takes minutes).
#[derive(Debug, Clone, Serialize, Default)]
pub struct Job {
    /// `running`, `done` or `failed`.
    pub state: String,
    pub percent: f32,
    pub message: String,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
    /// The libvirt domain this job imports.
    #[serde(skip)]
    pub domain: String,
    /// Start order (newest = highest).
    #[serde(skip)]
    pub seq: u64,
}

fn jobs() -> &'static std::sync::Mutex<std::collections::HashMap<String, Job>> {
    static J: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Job>>> =
        std::sync::OnceLock::new();
    J.get_or_init(Default::default)
}

fn job_update(id: &str, f: impl FnOnce(&mut Job)) {
    if let Ok(mut j) = jobs().lock() {
        if let Some(x) = j.get_mut(id) {
            f(x);
        }
    }
}

#[derive(Deserialize)]
struct StatusRequest {
    job: String,
}

async fn status(Json(req): Json<StatusRequest>) -> Result<Json<Job>, ApiError> {
    jobs()
        .lock()
        .ok()
        .and_then(|j| j.get(&req.job).cloned())
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("no import job {}", req.job)))
}

/// The VM this domain was already imported as (recorded source, by UUID when known, else name).
async fn already_imported_as(state: &AppState, domain: &str, uuid: &str) -> Option<String> {
    let ids: std::collections::HashSet<String> = state
        .vm_manager
        .list_vms()
        .await
        .into_iter()
        .map(|v| v.id)
        .collect();
    source_owner(&recorded_sources(state), &ids, domain, uuid)
}

/// 409 at request time (before a job exists) when the domain's UUID/name is a recorded source.
async fn refuse_imported_source(state: &AppState, domain: &str) -> Result<(), ApiError> {
    let uuid = match domain_xml(domain).await {
        Ok(x) => parse_xml(&x)
            .ok()
            .and_then(|d| d.child_text("uuid"))
            .unwrap_or_default(),
        Err(_) => String::new(),
    };
    match already_imported_as(state, domain, &uuid).await {
        Some(vm) => Err(ApiError::Conflict(format!(
            "libvirt domain {domain} was already imported as VM {vm}: remove the old source with the cleanup offer instead"
        ))),
        None => Ok(()),
    }
}

/// Starts the import and returns at once (202 `{job}`); poll `status`.
async fn import(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ImportRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    check_domain_name(&req.domain)?;
    refuse_imported_source(&state, &req.domain).await?;
    let job = format!("lv-{:016x}", rand::random::<u64>());
    {
        let mut j = jobs()
            .lock()
            .map_err(|_| ApiError::Internal("jobs".into()))?;
        if j.values().any(|x| x.state == "running") {
            return Err(ApiError::Conflict(
                "another libvirt import is running: wait for it (it copies disks at low priority)"
                    .into(),
            ));
        }
        j.insert(
            job.clone(),
            Job {
                state: "running".into(),
                message: "starting".into(),
                domain: req.domain.clone(),
                seq: {
                    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                    SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                },
                ..Default::default()
            },
        );
    }
    let id = job.clone();
    tokio::spawn(async move {
        let mut undo = Undo::default();
        let r = run_import(&state, &req, &mut undo, &id).await;
        match r {
            Ok(r) => job_update(&id, |j| {
                j.state = "done".into();
                j.percent = 100.0;
                j.message = "imported".into();
                j.result = serde_json::to_value(&r).ok();
            }),
            Err(e) => {
                if let Some(vm) = &undo.vm_id {
                    // the VM's disks are Horcrux's own copies: safe to delete
                    let _ = state.vm_manager.delete_vm(vm).await;
                }
                for f in &undo.files {
                    let _ = std::fs::remove_file(f);
                }
                for d in &undo.dirs {
                    let _ = std::fs::remove_dir_all(d);
                }
                let msg = match e {
                    ApiError::BadRequest(m)
                    | ApiError::Conflict(m)
                    | ApiError::Internal(m)
                    | ApiError::NotFound(m) => m,
                    other => format!("{other:?}"),
                };
                job_update(&id, |j| {
                    j.state = "failed".into();
                    j.error = Some(msg);
                });
            }
        }
    });
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "job": job })),
    ))
}

/// What a failed import must undo (the source is never touched).
#[derive(Default)]
struct Undo {
    vm_id: Option<String>,
    files: Vec<PathBuf>,
    dirs: Vec<PathBuf>,
}

// ------------------------------------------------------------------ disks

/// `Err` when `avail` cannot hold `need` plus a safety margin.
pub fn check_free_space(need: u64, avail: u64, dir: &FsPath) -> Result<(), String> {
    const MARGIN: u64 = 1 << 30;
    if avail < need.saturating_add(MARGIN) {
        return Err(format!(
            "not enough free space in {}: the disks need about {:.1} GiB (+1 GiB margin), {:.1} GiB are free. Nothing was copied.",
            dir.display(),
            need as f64 / (1u64 << 30) as f64,
            avail as f64 / (1u64 << 30) as f64
        ));
    }
    Ok(())
}

/// Every disk of the imported VM must live inside a Horcrux storage root
/// (never at its libvirt source).
pub fn verify_placement(
    roots: &[PathBuf],
    disks: &[PathBuf],
    sources: &[PathBuf],
) -> Result<(), String> {
    for d in disks {
        if sources.iter().any(|s| s == d) {
            return Err(format!("{} is still the libvirt source file", d.display()));
        }
        if !roots.iter().any(|r| d.starts_with(r)) {
            return Err(format!(
                "{} is outside the Horcrux storage ({})",
                d.display(),
                roots
                    .iter()
                    .map(|r| r.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    Ok(())
}

/// `qemu-img convert -p` prints `    (12.34/100%)`.
pub fn parse_progress(chunk: &str) -> Option<f32> {
    let end = chunk.rfind("/100%)")?;
    let start = chunk[..end].rfind('(')? + 1;
    chunk[start..end].trim().parse().ok()
}

fn free_bytes(dir: &FsPath) -> u64 {
    nix::sys::statvfs::statvfs(dir)
        .map(|s| s.blocks_available() * s.fragment_size())
        .unwrap_or(u64::MAX)
}

/// A command at idle I/O and CPU priority (the owner works on this machine).
fn low_priority(program: &str, args: &[&str]) -> Command {
    let mut c = Command::new("nice");
    c.args(["-n", "19"]);
    if std::process::Command::new("ionice")
        .arg("-h")
        .output()
        .is_ok()
    {
        c.args(["ionice", "-c", "3"]);
    }
    c.arg(program).args(args);
    c
}

/// Copy `src` (qcow2) to `dst` as a new qcow2 with progress, verify it with
/// `qemu-img compare`, then sync it. `report(fraction 0..1)`.
async fn copy_disk(src: &str, dst: &FsPath, report: &(dyn Fn(f32) + Sync)) -> Result<(), ApiError> {
    use tokio::io::AsyncReadExt;
    let d = dst.to_string_lossy().to_string();
    let mut child = low_priority(
        "qemu-img",
        &["convert", "-p", "-f", "qcow2", "-O", "qcow2", src, &d],
    )
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped())
    .spawn()
    .map_err(|e| ApiError::Internal(format!("qemu-img convert: {e}")))?;
    let mut out = child.stdout.take().expect("piped");
    let mut buf = [0u8; 256];
    loop {
        let n = out.read(&mut buf).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        if let Some(p) = parse_progress(&String::from_utf8_lossy(&buf[..n])) {
            report(p / 100.0);
        }
    }
    let o = child
        .wait_with_output()
        .await
        .map_err(|e| ApiError::Internal(format!("qemu-img convert: {e}")))?;
    if !o.status.success() {
        return Err(ApiError::Internal(qemu_img_error(
            &format!("convert {src}"),
            &String::from_utf8_lossy(&o.stderr),
        )));
    }
    // verify the copy against the source before anything points at it
    let c = low_priority(
        "qemu-img",
        &["compare", "-f", "qcow2", "-F", "qcow2", src, &d],
    )
    .output()
    .await
    .map_err(|e| ApiError::Internal(format!("qemu-img compare: {e}")))?;
    if !c.status.success() {
        return Err(ApiError::Internal(format!(
            "the copy of {src} differs from the source: {}{}",
            String::from_utf8_lossy(&c.stdout).trim(),
            String::from_utf8_lossy(&c.stderr).trim()
        )));
    }
    tokio::fs::File::open(dst)
        .await?
        .sync_all()
        .await
        .map_err(ApiError::from)?;
    Ok(())
}

async fn run_import(
    state: &Arc<AppState>,
    req: &ImportRequest,
    undo: &mut Undo,
    job: &str,
) -> Result<ImportResponse, ApiError> {
    let say = |pct: f32, m: String| {
        job_update(job, |j| {
            j.percent = pct;
            j.message = m;
        })
    };
    let dom = check_domain_name(&req.domain)?;
    let st = virsh(&["domstate", dom]).await?.ok_or_else(|| {
        ApiError::BadRequest("libvirt (virsh) is not installed on this host".into())
    })?;
    if st.trim() != "shut off" {
        return Err(ApiError::Conflict(format!(
            "domain {dom} is {}: shut it down before importing",
            st.trim()
        )));
    }
    let xml = domain_xml(dom).await?;
    let plan = plan_from_domain(&xml, &swtpm_root()).map_err(ApiError::BadRequest)?;
    if let Some(vm) = already_imported_as(state, dom, &plan.uuid).await {
        return Err(ApiError::Conflict(format!(
            "libvirt domain {dom} was already imported as VM {vm}: remove the old source with the cleanup offer instead"
        )));
    }
    let mut warnings = plan.warnings.clone();

    let id = match req.id.as_deref().map(str::trim).filter(|i| !i.is_empty()) {
        Some(i) => {
            crate::validation::validate_vm_id(i)?;
            if state.vm_manager.get_vm(i).await.is_ok() {
                return Err(ApiError::Conflict(format!("VM {i} already exists")));
            }
            i.to_string()
        }
        None => free_id_with(state, &default_id(&plan.name)).await,
    };
    let name = req
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or(&plan.name)
        .to_string();
    let pool = req
        .storage
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    // 1. sources: qcow2 only, readable, not already a Horcrux VM's disk
    say(1.0, "checking the disks".into());
    let existing = state.vm_manager.list_vms().await;
    let (mut sizes, mut actual) = (Vec::new(), 0u64);
    for d in &plan.disks {
        if d.format != "qcow2" {
            return Err(ApiError::BadRequest(format!(
                "disk {} is {}: only qcow2 images can be imported (convert it with qemu-img first)",
                d.path, d.format
            )));
        }
        if let Some(v) = existing
            .iter()
            .find(|v| v.disks.iter().any(|x| x.path == d.path))
        {
            return Err(ApiError::Conflict(format!(
                "disk {} already belongs to Horcrux VM {}",
                d.path, v.id
            )));
        }
        let info: serde_json::Value =
            serde_json::from_str(&qemu_img(&["info", "--output=json", &d.path]).await?)
                .map_err(|e| ApiError::Internal(format!("qemu-img info: {e}")))?;
        let fmt = checked_format(&info)
            .map_err(|e| ApiError::BadRequest(format!("disk {}: {e}", d.path)))?;
        if fmt != "qcow2" {
            return Err(ApiError::BadRequest(format!(
                "disk {} is {fmt}, not qcow2",
                d.path
            )));
        }
        sizes.push(
            info["virtual-size"]
                .as_u64()
                .unwrap_or(0)
                .div_ceil(1 << 30)
                .max(1),
        );
        actual += info["actual-size"].as_u64().unwrap_or(0);
    }

    // 2. where a new VM's disks go, and whether they fit
    let target_dir = match pool {
        Some(p) => crate::api::storage_volumes::vm_disk_dir(state, p, sizes.iter().sum()).await?,
        None => state.config.paths.vm_storage.clone(),
    };
    std::fs::create_dir_all(&target_dir)?;
    check_free_space(actual, free_bytes(&target_dir), &target_dir).map_err(ApiError::BadRequest)?;

    // 3. the XML backup (libvirt rollback: `virsh define <file>`)
    let bdir = state.config.paths.data_dir.join("libvirt-import");
    std::fs::create_dir_all(&bdir)?;
    let xml_backup = bdir.join(format!("{id}.xml"));
    std::fs::write(&xml_backup, &xml)?;

    // 4. firmware and swtpm must exist here
    let mut firmware = plan.firmware.clone();
    let mut secure = plan.secure_boot;
    if firmware == "efi" {
        let have = if secure {
            crate::vm::firmware::find_ovmf_for(true)
        } else {
            crate::vm::firmware::find_ovmf()
        };
        if have.is_none() {
            return Err(ApiError::BadRequest(if secure {
                crate::vm::firmware::ovmf_secboot_missing().to_string()
            } else {
                crate::vm::firmware::ovmf_missing().to_string()
            }));
        }
    } else {
        firmware = "bios".into();
        secure = false;
    }
    if plan.tpm && !crate::api::vm_hw::in_path("swtpm") {
        return Err(ApiError::BadRequest(
            "the domain has a vTPM but swtpm is not installed on this host (app-crypt/swtpm)"
                .into(),
        ));
    }

    // 5. NICs (MAC kept)
    let host_bridges: Vec<String> = crate::api::vm_hw::host_bridges()
        .into_iter()
        .map(|b| b.name)
        .collect();
    let fallback_bridge = match req
        .bridge
        .as_deref()
        .map(str::trim)
        .filter(|b| !b.is_empty())
    {
        Some(b) => Some(b.to_string()),
        None => {
            let mut b = None;
            if let Some(n) = plan
                .nics
                .iter()
                .find(|n| n.bridge.is_none() && !n.network.is_empty())
            {
                if let Ok(Some(x)) = virsh(&["net-dumpxml", &n.network]).await {
                    b = parse_xml(&x).ok().and_then(|r| {
                        r.child("bridge")
                            .and_then(|b| b.attr("name").map(String::from))
                    });
                }
            }
            b.filter(|b| host_bridges.contains(b)).or_else(|| {
                host_bridges
                    .iter()
                    .find(|b| *b != "docker0")
                    .or(host_bridges.first())
                    .cloned()
            })
        }
    };
    let mut nets = Vec::new();
    for n in &plan.nics {
        match n.bridge.clone().or_else(|| fallback_bridge.clone()) {
            Some(b) if host_bridges.contains(&b) => {
                nets.push(serde_json::json!({ "model": n.model, "bridge": b, "mac": n.mac }))
            }
            Some(b) => warnings.push(format!(
                "bridge {b} does not exist on this host: a network card was left out"
            )),
            None => warnings.push(
                "this host has no bridge: the VM has no network card (add one under Hardware)"
                    .into(),
            ),
        }
    }

    // 6. the VM record: Horcrux creates its disks where any new VM's go
    let mut hardware = serde_json::json!({
        "nets": nets,
        "machine": plan.machine.replace("pc", "i440fx"),
        "firmware": if firmware == "efi" { "ovmf" } else { "seabios" },
        "secure_boot": secure,
        "tpm": plan.tpm,
        "guest_agent": plan.guest_agent,
        "balloon": plan.balloon,
        "usb_controller": plan.usb_controller,
    });
    if let Some(c) = &plan.cpu_type {
        hardware["cpu_type"] = c.clone().into();
    }
    if let Some(s) = plan.sockets {
        hardware["sockets"] = s.into();
    }
    let d0 = &plan.disks[0];
    let mut cfg: VmConfig = serde_json::from_value(serde_json::json!({
        "id": id, "name": name, "hypervisor": "qemu", "architecture": "x86_64",
        "cpus": plan.cpus, "memory": plan.memory_mb, "disk_size": sizes[0],
        "status": "stopped", "hardware": hardware, "display": {"protocol": "vnc"},
        "disks": [{"path": "", "size_gb": sizes[0], "disk_type": d0.bus, "cache": "",
                   "discard": d0.discard, "storage": pool}],
    }))
    .map_err(|e| ApiError::Internal(format!("building the VM: {e}")))?;
    cfg.hardware
        .validate_for(&cfg.architecture, cfg.cpus)
        .map_err(ApiError::BadRequest)?;
    crate::api::storage_volumes::place_vm_disks(state, &mut cfg).await?;
    crate::api::vm_hw::check_host_hardware(state, &cfg.hardware).await?;
    let vm = state.vm_manager.create_vm(cfg).await?;
    undo.vm_id = Some(vm.id.clone());
    for (n, d) in plan.disks.iter().enumerate().skip(1) {
        let mut spec: crate::vm::hardware::DiskSpec = serde_json::from_value(serde_json::json!({
            "size_gb": sizes[n], "bus": d.bus, "storage": pool, "discard": d.discard,
        }))
        .map_err(|e| ApiError::Internal(e.to_string()))?;
        crate::api::storage_volumes::place_new_disk(state, &mut spec).await?;
        state.vm_manager.add_disk(&vm.id, spec).await?;
    }
    let vm = state.vm_manager.get_vm(&vm.id).await?;
    let new_paths: Vec<PathBuf> = vm.disks.iter().map(|d| PathBuf::from(&d.path)).collect();
    if new_paths.len() != plan.disks.len() {
        return Err(ApiError::Internal("the VM has not got every disk".into()));
    }
    let mut roots = vec![state.config.paths.vm_storage.clone(), target_dir.clone()];
    roots.dedup();
    let sources: Vec<PathBuf> = plan.disks.iter().map(|d| PathBuf::from(&d.path)).collect();
    verify_placement(&roots, &new_paths, &sources).map_err(ApiError::Internal)?;

    // 7. copy + verify each disk into its place (low priority, with progress)
    let total = plan.disks.len() as f32;
    for (n, (d, to)) in plan.disks.iter().zip(&new_paths).enumerate() {
        let tmp = to.with_extension("import.tmp");
        undo.files.push(tmp.clone());
        let label = format!(
            "copying disk {} of {} ({})",
            n + 1,
            plan.disks.len(),
            d.path
        );
        say(2.0 + 95.0 * n as f32 / total, label.clone());
        let base = n as f32;
        copy_disk(&d.path, &tmp, &|f| {
            job_update(job, |j| {
                j.percent = 2.0 + 95.0 * (base + f) / total;
                j.message = label.clone();
            })
        })
        .await?;
        tokio::fs::rename(&tmp, to).await?;
    }

    // 8. OVMF variables and vTPM state go to the VM's own location (copies:
    //    the old ones stay for the cleanup offer)
    let disk0 = new_paths[0].clone();
    say(97.0, "firmware variables and TPM state".into());
    if firmware == "efi" {
        if let Some(nv) = &plan.nvram {
            let nvp = FsPath::new(nv);
            if nvp.is_file() {
                let vars = crate::vm::firmware::efivars_path(&disk0);
                let tpl = crate::vm::firmware::find_ovmf_for(secure).map(|o| o.vars_template);
                let nv_info: serde_json::Value =
                    serde_json::from_str(&qemu_img(&["info", "--output=json", nv]).await?)
                        .map_err(|e| ApiError::Internal(format!("qemu-img info: {e}")))?;
                let tpl_len = tpl
                    .as_ref()
                    .and_then(|t| std::fs::metadata(t).ok())
                    .map(|m| m.len());
                if tpl_len.is_some() && nv_info["virtual-size"].as_u64() == tpl_len {
                    let f = nv_info["format"].as_str().unwrap_or("raw").to_string();
                    qemu_img(&[
                        "convert",
                        "-f",
                        &f,
                        "-O",
                        "qcow2",
                        nv,
                        &vars.to_string_lossy(),
                    ])
                    .await?;
                    undo.files.push(vars);
                } else {
                    warnings.push(format!(
                        "the OVMF variable store {nv} has another layout than this host's OVMF: a fresh one is created (re-add custom boot entries/keys)"
                    ));
                }
            }
        }
    }
    if let Some(dir) = &plan.tpm_state {
        let to = crate::vm::firmware::tpm_state_dir(&disk0);
        std::fs::create_dir_all(&to)?;
        undo.dirs.push(to.clone());
        for e in std::fs::read_dir(dir)? {
            let e = e?;
            if e.file_type()?.is_file() {
                std::fs::copy(e.path(), to.join(e.file_name()))?;
            }
        }
    }

    // 9. verify the stored VM: its disks are the verified copies inside the storage
    let got = state.vm_manager.get_vm(&vm.id).await?;
    let got_paths: Vec<PathBuf> = got.disks.iter().map(|d| PathBuf::from(&d.path)).collect();
    verify_placement(&roots, &got_paths, &sources).map_err(ApiError::Internal)?;
    if got_paths.iter().any(|p| !p.is_file()) || got.hardware.nets.len() != nets.len() {
        return Err(ApiError::Internal(
            "the stored VM does not match the domain".into(),
        ));
    }

    // 10. the old source is only OFFERED for removal (opt-in, with sizes)
    use crate::api::import_cleanup::Item;
    let efi_copy = crate::vm::firmware::efivars_path(&disk0);
    let tpm_copy = crate::vm::firmware::tpm_state_dir(&disk0);
    let mut items = vec![Item {
        kind: "libvirt-domain".into(),
        path: dom.to_string(),
        requires: None,
    }];
    for (d, new) in plan.disks.iter().zip(&new_paths) {
        items.push(Item {
            kind: "file".into(),
            path: d.path.clone(),
            requires: Some(new.to_string_lossy().to_string()),
        });
    }
    if let Some(nv) = plan.nvram.as_deref().filter(|_| firmware == "efi") {
        items.push(Item {
            kind: "file".into(),
            path: nv.to_string(),
            requires: Some(efi_copy.to_string_lossy().to_string()),
        });
    }
    if let Some(t) = &plan.tpm_state {
        items.push(Item {
            kind: "dir".into(),
            path: t.clone(),
            requires: Some(tpm_copy.to_string_lossy().to_string()),
        });
    }
    let cleanup_offered = crate::api::import_cleanup::write_record(
        state,
        &crate::api::import_cleanup::Record {
            vm_id: vm.id.clone(),
            source: "libvirt".into(),
            items,
        },
    )
    .is_ok();
    warnings.push(format!(
        "libvirt still defines {dom} (shut off) and the old disk files remain: do not start it, both would use one OS. Remove the old source with the cleanup offer."
    ));
    undo.vm_id = None; // done: nothing to roll back
    undo.files.clear();
    undo.dirs.clear();
    tracing::info!(
        "VM {}: imported from libvirt domain {dom} (disks copied into {}, old source kept for cleanup)",
        vm.id,
        target_dir.display()
    );
    Ok(ImportResponse {
        vm: got,
        plan,
        warnings,
        cleanup_offered,
        xml_backup: xml_backup.to_string_lossy().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIN: &str = r#"<domain type='kvm'><name>redweek-win11</name><uuid>5880ef7c-4801-4911-88a7-79475a357d24</uuid>
<description>BitLocker key id 8ED5</description><memory unit='KiB'>8388608</memory><vcpu placement='static'>4</vcpu>
<os firmware='efi'><type arch='x86_64' machine='pc-q35-10.2'>hvm</type>
<firmware><feature enabled='yes' name='enrolled-keys'/><feature enabled='yes' name='secure-boot'/></firmware>
<loader readonly='yes' secure='yes' type='pflash' format='qcow2'>/c.qcow2</loader>
<nvram template='/t.qcow2' format='qcow2'>/var/lib/libvirt/qemu/nvram/x_VARS.qcow2</nvram></os>
<cpu mode='host-passthrough' check='none'/><clock offset='localtime'/>
<devices><disk type='file' device='disk'><driver name='qemu' type='qcow2' discard='unmap'/><source file='/img/w.qcow2'/><target dev='sda' bus='sata'/><boot order='1'/></disk>
<disk type='file' device='cdrom'><driver name='qemu' type='raw'/><target dev='sdb' bus='sata'/></disk>
<interface type='network'><mac address='52:54:00:1E:2F:09'/><source network='default'/><model type='e1000e'/></interface>
<channel type='unix'><target type='virtio' name='org.qemu.guest_agent.0'/></channel>
<tpm model='tpm-crb'><backend type='emulator' version='2.0'/></tpm>
<memballoon model='virtio'/><controller type='usb' index='0' model='qemu-xhci'/></devices></domain>"#;

    #[test]
    fn windows_domain_maps_hardware_firmware_tpm_and_mac() {
        let p = plan_from_domain(WIN, FsPath::new("/nonexistent")).unwrap();
        assert_eq!((p.cpus, p.memory_mb), (4, 8192));
        assert_eq!(p.machine, "q35");
        assert_eq!(p.firmware, "efi");
        assert!(p.secure_boot && p.tpm && p.guest_agent && p.balloon && p.usb_controller);
        assert_eq!(p.cpu_type.as_deref(), Some("host"));
        assert_eq!(
            p.nvram.as_deref(),
            Some("/var/lib/libvirt/qemu/nvram/x_VARS.qcow2")
        );
        assert_eq!(p.disks.len(), 1, "the cdrom is reported, not imported");
        assert_eq!(
            (
                p.disks[0].bus.as_str(),
                p.disks[0].discard,
                p.disks[0].format.as_str()
            ),
            ("sata", true, "qcow2")
        );
        assert_eq!(p.nics[0].mac.as_deref(), Some("52:54:00:1e:2f:09"));
        assert_eq!(p.nics[0].model, "e1000e", "e1000e stays e1000e");
        let w = p.warnings.join("|");
        assert!(!w.contains("e1000e"), "no NIC downgrade warning: {w}");
        for k in ["cdrom", "never started", "local time", "tpm-tis"] {
            assert!(w.contains(k), "{k} in {w}");
        }
    }

    #[test]
    fn tpm_state_dir_is_found_by_uuid() {
        let d = tempfile::tempdir().unwrap();
        let t = d
            .path()
            .join("5880ef7c-4801-4911-88a7-79475a357d24")
            .join("tpm2");
        std::fs::create_dir_all(&t).unwrap();
        let p = plan_from_domain(WIN, d.path()).unwrap();
        assert_eq!(p.tpm_state.as_deref(), Some(t.to_str().unwrap()));
        assert!(!p.warnings.join("|").contains("never started"));
    }

    #[test]
    fn refusals() {
        assert!(plan_from_domain("<vm/>", FsPath::new("/")).is_err());
        assert!(plan_from_domain("<!DOCTYPE x><domain/>", FsPath::new("/")).is_err());
        let blk = WIN.replace("<source file='/img/w.qcow2'/>", "<source dev='/dev/sda'/>");
        assert!(plan_from_domain(&blk, FsPath::new("/"))
            .unwrap_err()
            .contains("plain image file"));
        let xen = WIN.replace("type='kvm'", "type='xen'");
        assert!(plan_from_domain(&xen, FsPath::new("/")).is_err());
    }

    #[test]
    fn an_imported_disk_must_be_inside_the_horcrux_storage() {
        let roots = vec![PathBuf::from("/var/lib/horcrux/vms")];
        let src = vec![PathBuf::from("/var/lib/libvirt/images/w.qcow2")];
        assert!(verify_placement(
            &roots,
            &[PathBuf::from("/var/lib/horcrux/vms/w.qcow2")],
            &src
        )
        .is_ok());
        let e = verify_placement(
            &roots,
            &[PathBuf::from("/var/lib/libvirt/images/w.qcow2")],
            &src,
        )
        .unwrap_err();
        assert!(e.contains("source"), "{e}");
        let e = verify_placement(&roots, &[PathBuf::from("/mnt/other/w.qcow2")], &src).unwrap_err();
        assert!(e.contains("outside the Horcrux storage"), "{e}");
        // a sibling directory with the same prefix is not inside the root
        assert!(verify_placement(
            &roots,
            &[PathBuf::from("/var/lib/horcrux/vms2/w.qcow2")],
            &src
        )
        .is_err());
    }

    #[test]
    fn free_space_is_checked_before_copying() {
        let gib = 1u64 << 30;
        let d = FsPath::new("/var/lib/horcrux/vms");
        assert!(check_free_space(10 * gib, 12 * gib, d).is_ok());
        let e = check_free_space(10 * gib, 10 * gib, d).unwrap_err();
        assert!(
            e.contains("not enough free space") && e.contains("Nothing was copied"),
            "{e}"
        );
    }

    #[test]
    fn qemu_img_progress_is_parsed() {
        assert_eq!(parse_progress("    (12.34/100%)"), Some(12.34));
        assert_eq!(
            parse_progress("\r    (1.00/100%)\r    (55.50/100%)"),
            Some(55.5)
        );
        assert_eq!(parse_progress("warning"), None);
    }

    #[tokio::test]
    async fn copy_disk_makes_a_verified_independent_copy() {
        if std::process::Command::new("qemu-img")
            .arg("--version")
            .output()
            .is_err()
        {
            return; // qemu-utils not installed here
        }
        let d = tempfile::tempdir().unwrap();
        let (src, dst) = (d.path().join("src.qcow2"), d.path().join("dst.qcow2"));
        assert!(std::process::Command::new("qemu-img")
            .args(["create", "-f", "qcow2", src.to_str().unwrap(), "8M"])
            .output()
            .unwrap()
            .status
            .success());
        copy_disk(src.to_str().unwrap(), &dst, &|_| {})
            .await
            .unwrap();
        assert!(dst.is_file() && src.is_file(), "the source stays");
        assert_ne!(
            std::fs::metadata(&dst).unwrap().len(),
            0,
            "a real qcow2 copy"
        );
        let e = copy_disk("/nonexistent/x.qcow2", &d.path().join("y.qcow2"), &|_| {})
            .await
            .unwrap_err();
        assert!(format!("{e:?}").contains("qemu-img convert"));
    }

    #[test]
    fn cluster_list_merges_nodes_and_reports_unreachable_ones() {
        let dom = |n: &str, imp: Option<&str>| ListedDomain {
            domain: n.into(),
            uuid: n.into(),
            state: "shut off".into(),
            cpus: 1,
            memory_mb: 1,
            disks: vec![],
            imported_as: imp.map(String::from),
            problem: None,
            import_state: None,
        };
        let local = ListResponse {
            available: true,
            domains: vec![dom("a", None), dom("b", Some("100"))],
        };
        let remote = ListResponse {
            available: true,
            domains: vec![dom("c", None)],
        };
        let r = merge_cluster(
            "n1",
            local,
            vec![
                ("n2".into(), Ok(remote)),
                ("n3".into(), Err("connection refused".into())),
            ],
        );
        assert_eq!(r.nodes.len(), 3);
        assert!(r.nodes[0].local && !r.nodes[1].local);
        assert_eq!(r.nodes[1].domains[0].domain, "c");
        assert_eq!(r.nodes[2].error.as_deref(), Some("connection refused"));
        assert_eq!(r.unmanaged, 2);
    }

    #[test]
    fn a_recorded_import_source_is_not_unmanaged() {
        let ids: std::collections::HashSet<String> = ["100".to_string(), "101".to_string()].into();
        let sources = vec![
            // cleanup record (name only), VM 100 exists
            RecordedSource {
                vm_id: "100".into(),
                name: Some("captainjacksparrow".into()),
                uuid: None,
            },
            // saved XML (uuid + name), VM 101 exists
            RecordedSource {
                vm_id: "101".into(),
                name: Some("redweek-win11".into()),
                uuid: Some("uuid-win11".into()),
            },
            // import whose VM is gone (failed/rolled back/deleted): not a source any more
            RecordedSource {
                vm_id: "102".into(),
                name: Some("gone".into()),
                uuid: Some("uuid-gone".into()),
            },
        ];
        let o = |d: &str, u: &str| source_owner(&sources, &ids, d, u);
        assert_eq!(o("captainjacksparrow", "any").as_deref(), Some("100"));
        assert_eq!(
            o("renamed-in-libvirt", "uuid-win11").as_deref(),
            Some("101")
        );
        // the same name with a different UUID is a different domain
        assert_eq!(o("redweek-win11", "uuid-other"), None);
        assert_eq!(o("gone", "uuid-gone"), None);
        assert_eq!(o("genuinely-unmanaged", "uuid-x"), None);
        // and so it does not count as unmanaged in the cluster list
        let dom = |n: &str, imp: Option<String>| ListedDomain {
            domain: n.into(),
            uuid: n.into(),
            state: "shut off".into(),
            cpus: 1,
            memory_mb: 1,
            disks: vec![],
            imported_as: imp,
            problem: None,
            import_state: None,
        };
        let list = ListResponse {
            available: true,
            domains: vec![
                dom("captainjacksparrow", o("captainjacksparrow", "u")),
                dom("genuinely-unmanaged", o("genuinely-unmanaged", "u")),
            ],
        };
        assert_eq!(merge_cluster("n1", list, vec![]).unmanaged, 1);
    }

    #[test]
    fn import_job_state_reports_running_and_failed_imports() {
        let mk = |state: &str, seq: u64, err: Option<&str>| Job {
            state: state.into(),
            percent: 42.0,
            domain: "uxqa-state-dom".into(),
            seq,
            error: err.map(String::from),
            ..Default::default()
        };
        assert_eq!(import_job_state("uxqa-state-dom"), None);
        jobs()
            .lock()
            .unwrap()
            .insert("t1".into(), mk("failed", 1, Some("no space")));
        assert_eq!(
            import_job_state("uxqa-state-dom").as_deref(),
            Some("last import failed: no space")
        );
        jobs()
            .lock()
            .unwrap()
            .insert("t2".into(), mk("running", 2, None));
        assert_eq!(
            import_job_state("uxqa-state-dom").as_deref(),
            Some("import in progress (42%)")
        );
        jobs()
            .lock()
            .unwrap()
            .retain(|_, j| j.domain != "uxqa-state-dom");
    }

    #[test]
    fn memory_units() {
        let n = |u: &str, v: &str| Node {
            attrs: vec![("unit".into(), u.into())],
            text: v.into(),
            ..Default::default()
        };
        assert_eq!(kib(&n("GiB", "2")), 2 << 20);
        assert_eq!(kib(&n("MiB", "512")), 512 << 10);
        assert_eq!(kib(&n("KiB", "100")), 100);
    }
}
