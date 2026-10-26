//! OVF / OVA import (VMware, VirtualBox, Proxmox `qm importovf`, XenServer
//! exports ...): the descriptor is mapped to a Horcrux VM and every disk is
//! converted to qcow2 with `qemu-img`.
//!
//! * `POST /api/vms/import/ovf/preview {source}` - what the import would
//!   create (CPU, memory, disks with their bus, network cards, firmware) and
//!   everything that cannot be mapped (warnings); nothing is created.
//! * `POST /api/vms/import/ovf {source, id?, name?, bridge?, storage?}` -
//!   creates the stopped VM (201 with the VM and the warnings).
//!
//! `source` is an `.ova` (tar of descriptor + disks), an `.ovf` with its disk
//! files next to it, or an http(s) URL of an `.ova`/`.ovf`. Local sources must
//! lie in the image directory or a storage pool directory (the same rule and
//! the same URL guards as `import-disk`).
//!
//! Safety: the descriptor is parsed by a small XML reader that refuses DTDs
//! (no entity expansion), is limited to 8 MiB and 64 nesting levels; disk
//! names are plain file names (no `/`, no `..`), looked up only inside the
//! archive or next to the `.ovf`; archive entries other than regular files
//! are ignored; every disk goes through the same qemu-img checks as
//! `import-disk` (no backing file, no external data file, no foreign VMDK
//! extents); a failed import removes the VM and every temporary file.
//!
//! Mapping: CPUs and memory as given (clamped, with a warning); IDE/SATA/SCSI
//! controllers become the disk bus (`ide`/`sata`/`scsi`; SCSI and virtio NICs
//! need guest drivers, so Windows guests get `sata` and `e1000` instead);
//! `e1000`/`e1000e` NICs stay `e1000`, other models become `virtio`;
//! `firmware=efi` becomes OVMF; CD-ROM, floppy, USB, sound and video devices
//! are reported, not imported.
use crate::api::vm_import::{
    allowed_local_source, checked_format, download, image_dir, qemu_img, TempFiles,
};
use crate::error::ApiError;
use crate::AppState;
use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use horcrux_common::VmConfig;
use serde::{Deserialize, Serialize};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/vms/import/ovf/preview", post(preview))
        .route("/api/vms/import/ovf", post(import))
}

const MAX_OVF_BYTES: u64 = 8 * 1024 * 1024;
const MAX_DISKS: usize = 16;

// ------------------------------------------------------------------ XML

/// A parsed element: local names only (prefixes dropped).
#[derive(Debug, Default, Clone)]
pub struct Node {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub text: String,
    pub children: Vec<Node>,
}

impl Node {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }
    pub fn kids<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> {
        self.children.iter().filter(move |c| c.name == name)
    }
    pub fn child_text(&self, name: &str) -> Option<String> {
        self.child(name)
            .map(|c| c.text.trim().to_string())
            .filter(|t| !t.is_empty())
    }
    fn find_all<'a>(&'a self, name: &str, out: &mut Vec<&'a Node>) {
        if self.name == name {
            out.push(self);
        }
        for c in &self.children {
            c.find_all(name, out);
        }
    }
}

fn local(n: &str) -> String {
    n.rsplit(':').next().unwrap_or(n).to_string()
}

fn decode_entities(s: &str) -> Result<String, String> {
    if !s.contains('&') {
        return Ok(s.to_string());
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 1..];
        let end = tail.find(';').ok_or("unterminated entity")?;
        let ent = &tail[..end];
        match ent {
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "amp" => out.push('&'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            _ => {
                let n = ent
                    .strip_prefix("#x")
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()))
                    .and_then(char::from_u32)
                    .ok_or_else(|| format!("unknown entity &{ent};"))?;
                out.push(n);
            }
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Parse an XML document into its root element.
pub fn parse_xml(src: &str) -> Result<Node, String> {
    if src.len() as u64 > MAX_OVF_BYTES {
        return Err("the descriptor is larger than 8 MiB".into());
    }
    let b = src.as_bytes();
    let mut i = 0;
    let mut stack: Vec<Node> = Vec::new();
    let mut root: Option<Node> = None;
    while i < b.len() {
        if b[i] != b'<' {
            let j = src[i..].find('<').map(|p| i + p).unwrap_or(b.len());
            if let Some(top) = stack.last_mut() {
                top.text.push_str(&decode_entities(&src[i..j])?);
            }
            i = j;
            continue;
        }
        let rest = &src[i..];
        if rest.starts_with("<!--") {
            let e = rest.find("-->").ok_or("unterminated comment")?;
            i += e + 3;
        } else if rest.starts_with("<![CDATA[") {
            let e = rest.find("]]>").ok_or("unterminated CDATA")?;
            if let Some(top) = stack.last_mut() {
                top.text.push_str(&rest[9..e]);
            }
            i += e + 3;
        } else if rest.starts_with("<?") {
            let e = rest
                .find("?>")
                .ok_or("unterminated processing instruction")?;
            i += e + 2;
        } else if rest.starts_with("<!") {
            // a DTD can define entities (billion laughs, external files)
            return Err("a DOCTYPE/DTD is not allowed in an OVF descriptor".into());
        } else if rest.starts_with("</") {
            let e = rest.find('>').ok_or("unterminated end tag")?;
            let name = local(rest[2..e].trim());
            let node = stack.pop().ok_or("unbalanced end tag")?;
            if node.name != name {
                return Err(format!("mismatched </{name}> for <{}>", node.name));
            }
            match stack.last_mut() {
                Some(p) => p.children.push(node),
                None => root = Some(node),
            }
            i += e + 1;
        } else {
            // start tag: find the closing '>' outside quotes
            let mut j = i + 1;
            let mut quote = 0u8;
            while j < b.len() {
                match (quote, b[j]) {
                    (0, b'"') | (0, b'\'') => quote = b[j],
                    (q, c) if q != 0 && c == q => quote = 0,
                    (0, b'>') => break,
                    _ => {}
                }
                j += 1;
            }
            if j >= b.len() {
                return Err("unterminated start tag".into());
            }
            let mut body = &src[i + 1..j];
            let selfclose = body.ends_with('/');
            if selfclose {
                body = &body[..body.len() - 1];
            }
            let body = body.trim();
            let (name, mut attrs_s) = match body.find(|c: char| c.is_whitespace()) {
                Some(p) => (&body[..p], body[p..].trim_start()),
                None => (body, ""),
            };
            let mut node = Node {
                name: local(name),
                ..Default::default()
            };
            while !attrs_s.is_empty() {
                let eq = attrs_s.find('=').ok_or("attribute without value")?;
                let key = local(attrs_s[..eq].trim());
                let v = attrs_s[eq + 1..].trim_start();
                let q = v.chars().next().filter(|c| *c == '"' || *c == '\'');
                let q = q.ok_or("attribute value without quotes")?;
                let end = v[1..].find(q).ok_or("unterminated attribute value")? + 1;
                node.attrs.push((key, decode_entities(&v[1..end])?));
                attrs_s = v[end + 1..].trim_start();
            }
            if selfclose {
                match stack.last_mut() {
                    Some(p) => p.children.push(node),
                    None => root = Some(node),
                }
            } else {
                if stack.len() >= 64 {
                    return Err("elements nested deeper than 64 levels".into());
                }
                stack.push(node);
            }
            i = j + 1;
        }
    }
    if !stack.is_empty() {
        return Err("unclosed element".into());
    }
    root.ok_or_else(|| "no root element".into())
}

// ------------------------------------------------------------------ OVF

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct OvfDisk {
    /// File name in the archive / next to the `.ovf`.
    pub file: String,
    pub capacity_bytes: Option<u64>,
    /// `virtio`, `scsi`, `sata` or `ide`.
    pub bus: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct OvfNic {
    /// `virtio`, `e1000` or `rtl8139`.
    pub model: String,
    /// The OVF network name (informational: the import attaches to a host bridge).
    pub network: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct OvfPlan {
    pub name: String,
    pub os: Option<String>,
    pub cpus: u32,
    pub memory_mb: u64,
    /// `bios` or `efi`
    pub firmware: String,
    pub disks: Vec<OvfDisk>,
    pub nics: Vec<OvfNic>,
    pub warnings: Vec<String>,
}

/// Bytes of `n` allocation units (`byte * 2^20`, `byte`, `KiloBytes`, ...).
fn unit_bytes(units: &str) -> u64 {
    let u = units.trim().to_ascii_lowercase();
    if let Some((_, p)) = u.split_once("2^") {
        return 1u64
            .checked_shl(p.trim().parse::<u32>().unwrap_or(0))
            .unwrap_or(1);
    }
    match u.as_str() {
        "kilobytes" => 1 << 10,
        "megabytes" => 1 << 20,
        "gigabytes" => 1 << 30,
        _ => 1,
    }
}

fn is_windows(desc: &str, os_type: &str) -> bool {
    let d = format!("{desc} {os_type}").to_ascii_lowercase();
    d.contains("windows") || d.contains("microsoft") || d.contains("winnt")
}

/// Map an OVF descriptor to a plan.
pub fn plan_from_ovf(xml: &str) -> Result<OvfPlan, String> {
    let root = parse_xml(xml)?;
    if root.name != "Envelope" {
        return Err(format!(
            "not an OVF descriptor (root element <{}>, expected <Envelope>)",
            root.name
        ));
    }
    let mut warnings = Vec::new();

    // file references and disks
    let files: Vec<(String, String)> = root
        .child("References")
        .map(|r| {
            r.kids("File")
                .filter_map(|f| Some((f.attr("id")?.to_string(), f.attr("href")?.to_string())))
                .collect()
        })
        .unwrap_or_default();
    struct DiskDef {
        id: String,
        file_ref: String,
        capacity: Option<u64>,
    }
    let mut diskdefs: Vec<DiskDef> = Vec::new();
    if let Some(ds) = root.child("DiskSection") {
        for d in ds.kids("Disk") {
            let cap = d
                .attr("capacity")
                .and_then(|c| c.trim().parse::<u64>().ok());
            let unit = d
                .attr("capacityAllocationUnits")
                .map(unit_bytes)
                .unwrap_or(1);
            diskdefs.push(DiskDef {
                id: d.attr("diskId").unwrap_or_default().to_string(),
                file_ref: d.attr("fileRef").unwrap_or_default().to_string(),
                capacity: cap.and_then(|c| c.checked_mul(unit)),
            });
        }
    }

    // the virtual system (first of a collection)
    let mut systems = Vec::new();
    root.find_all("VirtualSystem", &mut systems);
    let vs = *systems.first().ok_or("the OVF has no <VirtualSystem>")?;
    if systems.len() > 1 {
        warnings.push(format!(
            "the OVF holds {} virtual systems; only the first (\"{}\") is imported",
            systems.len(),
            vs.child_text("Name")
                .or_else(|| vs.attr("id").map(String::from))
                .unwrap_or_default()
        ));
    }
    let name = vs
        .child_text("Name")
        .or_else(|| vs.attr("id").map(String::from))
        .unwrap_or_else(|| "imported-vm".into());
    let os_sec = vs.child("OperatingSystemSection");
    let os_desc = os_sec
        .and_then(|o| o.child_text("Description"))
        .unwrap_or_default();
    let windows = is_windows(
        &os_desc,
        os_sec.and_then(|o| o.attr("osType")).unwrap_or_default(),
    );
    let hw = vs
        .child("VirtualHardwareSection")
        .ok_or("the OVF has no <VirtualHardwareSection>")?;

    let mut firmware = "bios".to_string();
    for c in hw
        .children
        .iter()
        .filter(|c| c.name == "Config" || c.name == "ExtraConfig")
    {
        if c.attr("key") == Some("firmware") && c.attr("value") == Some("efi") {
            firmware = "efi".into();
        }
    }

    struct Ctl {
        id: String,
        bus: &'static str,
    }
    let mut controllers: Vec<Ctl> = Vec::new();
    let (mut cpus, mut memory_mb) = (1u32, 512u64);
    let mut disk_items: Vec<(String, Option<String>, String)> = Vec::new(); // (host resource, parent, label)
    let mut nics = Vec::new();
    let mut seen_cpu = false;
    let mut seen_mem = false;
    for it in hw.kids("Item") {
        let rtype: u32 = it
            .child_text("ResourceType")
            .and_then(|t| t.parse().ok())
            .unwrap_or(0);
        let sub = it.child_text("ResourceSubType").unwrap_or_default();
        let sub_l = sub.to_ascii_lowercase();
        let label = it.child_text("ElementName").unwrap_or_default();
        let qty: u64 = it
            .child_text("VirtualQuantity")
            .and_then(|q| q.parse().ok())
            .unwrap_or(0);
        let inst = it.child_text("InstanceID").unwrap_or_default();
        match rtype {
            3 => {
                seen_cpu = true;
                cpus = qty.clamp(1, 512) as u32;
                if qty == 0 || qty > 512 {
                    warnings.push(format!("{qty} virtual CPUs is out of range: using {cpus}"));
                }
            }
            4 => {
                seen_mem = true;
                let unit = it
                    .child_text("AllocationUnits")
                    .map(|u| unit_bytes(&u))
                    .unwrap_or(1 << 20);
                memory_mb = (qty.saturating_mul(unit) >> 20).max(16);
            }
            5 => controllers.push(Ctl {
                id: inst,
                bus: "ide",
            }),
            6 => controllers.push(Ctl {
                id: inst,
                bus: "scsi",
            }),
            20 => controllers.push(Ctl {
                id: inst,
                bus: if sub_l.contains("ahci") || sub_l.is_empty() {
                    "sata"
                } else {
                    "scsi"
                },
            }),
            17 => disk_items.push((
                it.child_text("HostResource").unwrap_or_default(),
                it.child_text("Parent"),
                label,
            )),
            10 => {
                let model = if sub_l.contains("e1000") {
                    "e1000"
                } else if sub_l.contains("rtl8139") {
                    "rtl8139"
                } else if sub_l.is_empty() || sub_l.contains("virtio") {
                    "virtio"
                } else {
                    if !windows {
                        warnings.push(format!(
                            "network card \"{label}\" ({sub}) has no QEMU equivalent: imported as virtio"
                        ));
                    }
                    "virtio"
                };
                let model = if windows && model == "virtio" {
                    "e1000"
                } else {
                    model
                };
                if windows && model == "e1000" && !sub_l.contains("e1000") {
                    warnings.push(format!(
                        "network card \"{label}\" ({sub}): Windows has no virtio driver, imported as e1000"
                    ));
                }
                nics.push(OvfNic {
                    model: model.into(),
                    network: it.child_text("Connection").unwrap_or_default(),
                });
            }
            14 => warnings.push("floppy drive not imported".into()),
            15 => warnings
                .push("CD-ROM drive not imported (attach an ISO under the VM's Hardware)".into()),
            23 => warnings
                .push("USB controller not imported (add one under Hardware if needed)".into()),
            35 => {
                warnings.push("sound card not imported (set one under Hardware > Display)".into())
            }
            24 | 1 | 2 | 7 | 8 => {}
            other => warnings.push(format!(
                "hardware item \"{label}\" (resource type {other}) is not imported"
            )),
        }
    }
    if !seen_cpu {
        warnings.push("the OVF gives no CPU count: using 1".into());
    }
    if !seen_mem {
        warnings.push("the OVF gives no memory size: using 512 MB".into());
    }

    let mut disks = Vec::new();
    for (host, parent, label) in disk_items {
        let id = host.rsplit('/').next().unwrap_or("").to_string();
        let Some(def) = diskdefs.iter().find(|d| d.id == id) else {
            warnings.push(format!(
                "disk \"{label}\" ({host}) has no <Disk> entry: skipped"
            ));
            continue;
        };
        let Some((_, href)) = files.iter().find(|(fid, _)| *fid == def.file_ref) else {
            warnings.push(format!(
                "disk \"{label}\" refers to file {:?} that is not in <References>: skipped",
                def.file_ref
            ));
            continue;
        };
        let mut bus = parent
            .and_then(|p| controllers.iter().find(|c| c.id == p).map(|c| c.bus))
            .unwrap_or("virtio");
        if windows && matches!(bus, "scsi" | "virtio") {
            warnings.push(format!(
                "disk \"{label}\": Windows has no virtio/SCSI driver at boot, attached as sata"
            ));
            bus = "sata";
        }
        disks.push(OvfDisk {
            file: href.clone(),
            capacity_bytes: def.capacity,
            bus: bus.to_string(),
            label,
        });
    }
    if disks.is_empty() {
        return Err("the OVF describes no disk that could be imported".into());
    }
    if disks.len() > MAX_DISKS {
        return Err(format!(
            "the OVF has {} disks (at most {MAX_DISKS})",
            disks.len()
        ));
    }
    for d in &disks {
        if !safe_file_name(&d.file) {
            return Err(format!(
                "disk file name {:?} is not a plain file name (no '/', no '..')",
                d.file
            ));
        }
    }
    Ok(OvfPlan {
        name,
        os: (!os_desc.is_empty()).then_some(os_desc),
        cpus,
        memory_mb,
        firmware,
        disks,
        nics,
        warnings,
    })
}

fn safe_file_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 200
        && !n.starts_with('.')
        && !n.contains(['/', '\\', '\0'])
        && n != ".."
}

// ------------------------------------------------------------------ tar

/// One regular file of a tar archive.
#[derive(Debug, Clone, PartialEq)]
pub struct TarEntry {
    pub name: String,
    pub offset: u64,
    pub size: u64,
}

fn octal(b: &[u8]) -> Option<u64> {
    let s: String = b
        .iter()
        .take_while(|c| **c != 0)
        .map(|c| *c as char)
        .collect();
    let s = s.trim();
    if s.is_empty() {
        return Some(0);
    }
    u64::from_str_radix(s, 8).ok()
}

fn cstr(b: &[u8]) -> String {
    String::from_utf8_lossy(&b[..b.iter().position(|c| *c == 0).unwrap_or(b.len())]).to_string()
}

/// Index the regular files of a (ustar / GNU / pax) tar archive without
/// reading their contents.
pub fn tar_index<R: Read + Seek>(r: &mut R) -> Result<Vec<TarEntry>, String> {
    let len = r.seek(SeekFrom::End(0)).map_err(|e| e.to_string())?;
    let mut pos = 0u64;
    let mut out = Vec::new();
    let mut long_name: Option<String> = None;
    let mut zero_blocks = 0;
    while pos + 512 <= len {
        r.seek(SeekFrom::Start(pos)).map_err(|e| e.to_string())?;
        let mut h = [0u8; 512];
        r.read_exact(&mut h).map_err(|e| e.to_string())?;
        if h.iter().all(|b| *b == 0) {
            zero_blocks += 1;
            if zero_blocks >= 2 {
                break;
            }
            pos += 512;
            continue;
        }
        zero_blocks = 0;
        let size = octal(&h[124..136]).ok_or("not a tar archive (bad size field)")?;
        // checksum validates that this is a tar header at all
        let stored = octal(&h[148..156]).ok_or("not a tar archive (bad checksum field)")?;
        let sum: u64 = h
            .iter()
            .enumerate()
            .map(|(i, b)| {
                if (148..156).contains(&i) {
                    32
                } else {
                    *b as u64
                }
            })
            .sum();
        if sum != stored {
            return Err("not a tar archive (header checksum mismatch)".into());
        }
        let kind = h[156];
        let data = pos + 512;
        let padded = size.div_ceil(512) * 512;
        if data.checked_add(size).is_none_or(|e| e > len) {
            return Err("the archive is truncated".into());
        }
        let mut name = cstr(&h[0..100]);
        if &h[257..262] == b"ustar" {
            let prefix = cstr(&h[345..500]);
            if !prefix.is_empty() {
                name = format!("{prefix}/{name}");
            }
        }
        match kind {
            b'L' => {
                let mut buf = vec![0u8; size.min(4096) as usize];
                r.seek(SeekFrom::Start(data)).map_err(|e| e.to_string())?;
                r.read_exact(&mut buf).map_err(|e| e.to_string())?;
                long_name = Some(cstr(&buf));
            }
            b'x' => {
                let mut buf = vec![0u8; size.min(65536) as usize];
                r.seek(SeekFrom::Start(data)).map_err(|e| e.to_string())?;
                r.read_exact(&mut buf).map_err(|e| e.to_string())?;
                for rec in String::from_utf8_lossy(&buf).lines() {
                    if let Some((_, kv)) = rec.split_once(' ') {
                        if let Some(p) = kv.strip_prefix("path=") {
                            long_name = Some(p.to_string());
                        }
                    }
                }
            }
            b'0' | 0 => {
                out.push(TarEntry {
                    name: long_name.take().unwrap_or(name),
                    offset: data,
                    size,
                });
            }
            _ => {
                long_name = None;
            }
        }
        pos = data + padded;
    }
    Ok(out)
}

/// Copy `size` bytes at `offset` to a new file.
fn extract<R: Read + Seek>(r: &mut R, e: &TarEntry, to: &FsPath) -> std::io::Result<()> {
    r.seek(SeekFrom::Start(e.offset))?;
    let mut f = std::fs::File::create(to)?;
    let mut left = e.size;
    let mut buf = vec![0u8; 1 << 20];
    while left > 0 {
        let want = left.min(buf.len() as u64) as usize;
        let n = r.read(&mut buf[..want])?;
        if n == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        f.write_all(&buf[..n])?;
        left -= n as u64;
    }
    f.sync_all()
}

// ------------------------------------------------------------------ source

/// The descriptor text and where each disk file can be read from.
pub(crate) struct Source {
    pub(crate) ovf: String,
    /// `.ova`: the archive and its index; `.ovf`: the directory with the disks.
    pub(crate) ova: Option<(PathBuf, Vec<TarEntry>)>,
    pub(crate) dir: Option<PathBuf>,
    /// Temporary download, removed on drop.
    pub(crate) _tmp: TempFiles,
}

async fn open_source(state: &AppState, source: &str, stage: &FsPath) -> Result<Source, ApiError> {
    let mut tmp = Vec::new();
    let path: PathBuf = if source.starts_with("http://") || source.starts_with("https://") {
        let url_path = source.split(['?', '#']).next().unwrap_or(source);
        let ext = if url_path.to_ascii_lowercase().ends_with(".ovf") {
            "ovf"
        } else {
            "ova"
        };
        let to = stage.join(format!("download.{ext}"));
        tmp.push(to.clone());
        download(source, &to).await?;
        to
    } else {
        let p = std::fs::canonicalize(source)
            .map_err(|e| ApiError::BadRequest(format!("source {source}: {e}")))?;
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
    let lower = path.to_string_lossy().to_ascii_lowercase();
    if lower.ends_with(".ovf") {
        let len = std::fs::metadata(&path)?.len();
        if len > MAX_OVF_BYTES {
            return Err(ApiError::BadRequest("the .ovf is larger than 8 MiB".into()));
        }
        let ovf = std::fs::read_to_string(&path)
            .map_err(|e| ApiError::BadRequest(format!("reading the .ovf: {e}")))?;
        Ok(Source {
            ovf,
            ova: None,
            dir: path.parent().map(PathBuf::from),
            _tmp: TempFiles(tmp),
        })
    } else if lower.ends_with(".ova") || lower.ends_with(".tar") {
        let p2 = path.clone();
        let (ovf, index) =
            tokio::task::spawn_blocking(move || -> Result<(String, Vec<TarEntry>), String> {
                let mut f = std::fs::File::open(&p2).map_err(|e| e.to_string())?;
                let index = tar_index(&mut f)?;
                let e = index
                    .iter()
                    .find(|e| {
                        e.name.to_ascii_lowercase().ends_with(".ovf") && !e.name.contains("../")
                    })
                    .ok_or("the archive holds no .ovf descriptor")?;
                if e.size > MAX_OVF_BYTES {
                    return Err("the descriptor is larger than 8 MiB".into());
                }
                f.seek(SeekFrom::Start(e.offset))
                    .map_err(|e| e.to_string())?;
                let mut buf = vec![0u8; e.size as usize];
                f.read_exact(&mut buf).map_err(|e| e.to_string())?;
                Ok((String::from_utf8_lossy(&buf).to_string(), index))
            })
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?
            .map_err(ApiError::BadRequest)?;
        Ok(Source {
            ovf,
            ova: Some((path, index)),
            dir: None,
            _tmp: TempFiles(tmp),
        })
    } else {
        Err(ApiError::BadRequest(
            "the source must be an .ova, an .ovf or an http(s) URL of one".into(),
        ))
    }
}

impl Source {
    /// Make disk file `name` available as a plain file under `stage`.
    async fn stage_disk(&self, name: &str, stage: &FsPath, n: usize) -> Result<PathBuf, ApiError> {
        if !safe_file_name(name) {
            return Err(ApiError::BadRequest(format!(
                "disk file name {name:?} is not allowed"
            )));
        }
        let to = stage.join(format!("src-{n}.img"));
        match (&self.ova, &self.dir) {
            (Some((archive, index)), _) => {
                let e = index
                    .iter()
                    .find(|e| e.name.rsplit('/').next() == Some(name) && !e.name.contains(".."))
                    .cloned()
                    .ok_or_else(|| {
                        ApiError::BadRequest(format!(
                            "the archive does not contain disk file {name}"
                        ))
                    })?;
                let (a, t) = (archive.clone(), to.clone());
                tokio::task::spawn_blocking(move || {
                    let mut f = std::fs::File::open(&a)?;
                    extract(&mut f, &e, &t)
                })
                .await
                .map_err(|e| ApiError::Internal(e.to_string()))?
                .map_err(|e| ApiError::Internal(format!("extracting {name}: {e}")))?;
            }
            (None, Some(dir)) => {
                let src = std::fs::canonicalize(dir.join(name)).map_err(|_| {
                    ApiError::BadRequest(format!("disk file {name} is not next to the .ovf"))
                })?;
                if src.parent() != Some(dir.as_path()) || !src.is_file() {
                    return Err(ApiError::BadRequest(format!(
                        "disk file {name} must be a regular file next to the .ovf"
                    )));
                }
                // read in place (no copy): qemu-img converts straight from it
                return Ok(src);
            }
            _ => return Err(ApiError::Internal("no source".into())),
        }
        Ok(to)
    }
}

// ------------------------------------------------------------------ API

#[derive(Deserialize)]
struct PreviewRequest {
    source: String,
}

#[derive(Serialize)]
struct PreviewResponse {
    plan: OvfPlan,
    /// Id the VM would get.
    id: String,
}

fn slug(name: &str) -> String {
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
    let s = s.trim_matches('-').to_string();
    let s: String = s.chars().take(40).collect();
    if s.is_empty() {
        "vm".into()
    } else {
        s
    }
}

pub(crate) fn stage_dir(state: &AppState) -> Result<PathBuf, ApiError> {
    let d = state
        .config
        .paths
        .data_dir
        .join(format!("ovf-import-{:016x}", rand::random::<u64>()));
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

pub(crate) async fn free_id(state: &AppState, want: &str) -> String {
    let base = format!("ovf-{}", slug(want));
    let mut id = base.clone();
    let mut n = 2;
    while state.vm_manager.get_vm(&id).await.is_ok() {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

async fn preview(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PreviewRequest>,
) -> Result<Json<PreviewResponse>, ApiError> {
    let stage = stage_dir(&state)?;
    let res = async {
        let src = open_source(&state, &req.source, &stage).await?;
        let plan = plan_from_ovf(&src.ovf).map_err(ApiError::BadRequest)?;
        Ok::<_, ApiError>(plan)
    }
    .await;
    let _ = std::fs::remove_dir_all(&stage);
    let plan = res?;
    let id = free_id(&state, &plan.name).await;
    Ok(Json(PreviewResponse { plan, id }))
}

#[derive(Deserialize)]
struct ImportRequest {
    source: String,
    /// VM id (default `ovf-<name>`).
    #[serde(default)]
    id: Option<String>,
    /// VM name (default: the OVF's).
    #[serde(default)]
    name: Option<String>,
    /// Host bridge for the network cards (default: the first bridge).
    #[serde(default)]
    bridge: Option<String>,
    /// Storage pool for the disks (default: the VM storage directory).
    #[serde(default)]
    storage: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct ImportResponse {
    pub vm: VmConfig,
    pub plan: OvfPlan,
    pub warnings: Vec<String>,
}

async fn import(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ImportRequest>,
) -> Result<(StatusCode, Json<ImportResponse>), ApiError> {
    let stage = stage_dir(&state)?;
    let mut created: Option<String> = None;
    let res = run_import(&state, &req, &stage, &mut created).await;
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

/// What the caller may override, and where the VM came from (for the log).
pub(crate) struct Overrides {
    pub id: Option<String>,
    pub name: Option<String>,
    pub bridge: Option<String>,
    pub storage: Option<String>,
    pub label: String,
}

async fn run_import(
    state: &Arc<AppState>,
    req: &ImportRequest,
    stage: &FsPath,
    created: &mut Option<String>,
) -> Result<ImportResponse, ApiError> {
    let src = open_source(state, &req.source, stage).await?;
    let plan = plan_from_ovf(&src.ovf).map_err(ApiError::BadRequest)?;
    let ov = Overrides {
        id: req.id.clone(),
        name: req.name.clone(),
        bridge: req.bridge.clone(),
        storage: req.storage.clone(),
        label: req.source.clone(),
    };
    import_plan(state, &ov, plan, &src, stage, created).await
}

/// Convert the plan's disks and create the VM (shared by OVF/OVA and ESXi).
pub(crate) async fn import_plan(
    state: &Arc<AppState>,
    req: &Overrides,
    plan: OvfPlan,
    src: &Source,
    stage: &FsPath,
    created: &mut Option<String>,
) -> Result<ImportResponse, ApiError> {
    let mut warnings = plan.warnings.clone();
    let id = match req.id.as_deref().map(str::trim).filter(|i| !i.is_empty()) {
        Some(i) => {
            crate::validation::validate_vm_id(i)?;
            i.to_string()
        }
        None => free_id(state, &plan.name).await,
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

    // 1. convert every disk into the staging directory (fails before any VM exists)
    let mut converted: Vec<(PathBuf, u64)> = Vec::new(); // (qcow2, virtual bytes)
    for (n, d) in plan.disks.iter().enumerate() {
        let raw = src.stage_disk(&d.file, stage, n).await?;
        let rawp = raw.to_string_lossy().to_string();
        // a file qemu-img cannot even open (or whose extents are missing
        // files) is a bad disk, not a server fault
        let info_text = qemu_img(&["info", "--output=json", &rawp])
            .await
            .map_err(|e| {
                let why = match e {
                    ApiError::Internal(m) => m,
                    other => format!("{other:?}"),
                };
                ApiError::BadRequest(format!("disk {} is not a usable image: {why}", d.file))
            })?;
        let info: serde_json::Value = serde_json::from_str(&info_text)
            .map_err(|e| ApiError::Internal(format!("qemu-img info: {e}")))?;
        let fmt = checked_format(&info)
            .map_err(|e| ApiError::BadRequest(format!("disk {}: {e}", d.file)))?;
        let out = stage.join(format!("out-{n}.qcow2"));
        qemu_img(&[
            "convert",
            "-f",
            &fmt,
            "-O",
            "qcow2",
            &rawp,
            &out.to_string_lossy(),
        ])
        .await?;
        if raw.starts_with(stage) {
            let _ = std::fs::remove_file(&raw); // free the extracted copy at once
        }
        let virt = info["virtual-size"].as_u64().unwrap_or(0);
        converted.push((out, virt.max(d.capacity_bytes.unwrap_or(0))));
    }

    // 2. the VM: disk0 is created with the right size, then replaced
    let gb = |b: u64| b.div_ceil(1 << 30).max(1);
    let bridge = match req
        .bridge
        .as_deref()
        .map(str::trim)
        .filter(|b| !b.is_empty())
    {
        Some(b) => Some(b.to_string()),
        None => crate::api::vm_hw::host_bridges()
            .first()
            .map(|b| b.name.clone()),
    };
    if bridge.is_none() && !plan.nics.is_empty() {
        warnings.push(
            "this host has no bridge: the VM has no network card (add one under Hardware)".into(),
        );
    }
    // A UEFI guest needs OVMF on this host (the VM would be refused otherwise)
    let mut firmware = plan.firmware.clone();
    if firmware == "efi" && crate::vm::firmware::find_ovmf().is_none() {
        warnings.push(
            "the guest boots with UEFI but OVMF is not installed on this host: the VM is set to BIOS; install ovmf (edk2) and switch Hardware > Firmware to UEFI before starting it"
                .into(),
        );
        firmware = "bios".into();
    }
    let nets: Vec<serde_json::Value> = bridge
        .as_ref()
        .map(|b| {
            plan.nics
                .iter()
                .map(|n| serde_json::json!({"model": n.model, "bridge": b}))
                .collect()
        })
        .unwrap_or_default();
    let mut cfg: VmConfig = serde_json::from_value(serde_json::json!({
        "id": id, "name": name, "hypervisor": "qemu", "architecture": "x86_64",
        "cpus": plan.cpus, "memory": plan.memory_mb, "disk_size": gb(converted[0].1),
        "status": "stopped",
        "disks": [{"path": "", "size_gb": gb(converted[0].1), "disk_type": plan.disks[0].bus,
                   "cache": "", "storage": pool}],
        "hardware": {"nets": nets, "firmware": if firmware == "efi" { "ovmf" } else { "seabios" }},
        "display": {"protocol": "vnc"},
    }))
    .map_err(|e| ApiError::Internal(format!("building the VM: {e}")))?;
    if cfg.architecture != horcrux_common::VmArchitecture::X86_64 {
        // arch default is the host's; an OVF is x86 in practice
        cfg.architecture = horcrux_common::VmArchitecture::X86_64;
    }
    crate::api::storage_volumes::place_vm_disks(state, &mut cfg).await?;
    crate::api::vm_hw::check_host_hardware(state, &cfg.hardware).await?;
    let vm = state.vm_manager.create_vm(cfg).await?;
    *created = Some(vm.id.clone());

    // 3. put the converted images in place
    let place = |from: &FsPath, to: &FsPath, size_gb: u64| {
        let (from, to) = (from.to_path_buf(), to.to_path_buf());
        async move {
            let out = to.with_extension("import.tmp");
            // the staging directory may be on another file system: copy, then rename
            tokio::fs::copy(&from, &out).await?;
            let o = out.to_string_lossy().to_string();
            let info: serde_json::Value =
                serde_json::from_str(&qemu_img(&["info", "--output=json", &o]).await?)
                    .map_err(|e| ApiError::Internal(e.to_string()))?;
            if info["virtual-size"].as_u64().unwrap_or(0) < size_gb << 30 {
                qemu_img(&["resize", "-f", "qcow2", &o, &format!("{size_gb}G")]).await?;
            }
            tokio::fs::File::open(&out).await?.sync_all().await?;
            tokio::fs::rename(&out, &to).await?;
            Ok::<_, ApiError>(())
        }
    };
    let disk0 = PathBuf::from(&vm.disks[0].path);
    place(&converted[0].0, &disk0, vm.disks[0].size_gb).await?;
    for (n, d) in plan.disks.iter().enumerate().skip(1) {
        let mut spec: crate::vm::hardware::DiskSpec = serde_json::from_value(serde_json::json!({
            "size_gb": gb(converted[n].1), "bus": d.bus, "storage": pool,
        }))
        .map_err(|e| ApiError::Internal(e.to_string()))?;
        crate::api::storage_volumes::place_new_disk(state, &mut spec).await?;
        let size = spec.size_gb;
        let v = state.vm_manager.add_disk(&vm.id, spec).await?;
        let path = PathBuf::from(
            &v.disks
                .last()
                .ok_or_else(|| ApiError::Internal("no disk".into()))?
                .path,
        );
        place(&converted[n].0, &path, size).await?;
    }
    tracing::info!(
        "VM {}: imported from {} ({} disk(s))",
        vm.id,
        req.label,
        plan.disks.len()
    );
    *created = None;
    let vm = state.vm_manager.get_vm(&vm.id).await?;
    Ok(ImportResponse { vm, plan, warnings })
}

#[cfg(test)]
mod tests {
    use super::*;

    const OVF: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- exported by test -->
<Envelope vmw:buildId="build-1" xmlns="http://schemas.dmtf.org/ovf/envelope/1" xmlns:ovf="http://schemas.dmtf.org/ovf/envelope/1" xmlns:rasd="http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_ResourceAllocationSettingData" xmlns:vmw="http://www.vmware.com/schema/ovf" xmlns:vssd="http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_VirtualSystemSettingData">
  <References>
    <File ovf:href="web-disk1.vmdk" ovf:id="file1" ovf:size="1234"/>
    <File ovf:href="web-disk2.vmdk" ovf:id="file2"/>
  </References>
  <DiskSection>
    <Info>Virtual disk information</Info>
    <Disk ovf:capacity="20" ovf:capacityAllocationUnits="byte * 2^30" ovf:diskId="vmdisk1" ovf:fileRef="file1" ovf:format="http://www.vmware.com/interfaces/specifications/vmdk.html#streamOptimized"/>
    <Disk ovf:capacity="10737418240" ovf:diskId="vmdisk2" ovf:fileRef="file2"/>
  </DiskSection>
  <VirtualSystem ovf:id="web &amp; app">
    <Info>A virtual machine</Info>
    <Name>web &amp; app</Name>
    <OperatingSystemSection ovf:id="94" vmw:osType="ubuntu64Guest">
      <Info>The kind of installed guest operating system</Info>
      <Description>Ubuntu Linux (64-bit)</Description>
    </OperatingSystemSection>
    <VirtualHardwareSection>
      <Info>Virtual hardware requirements</Info>
      <System><vssd:ElementName>Virtual Hardware Family</vssd:ElementName></System>
      <Item><rasd:AllocationUnits>hertz * 10^6</rasd:AllocationUnits><rasd:ElementName>4 virtual CPU(s)</rasd:ElementName><rasd:InstanceID>1</rasd:InstanceID><rasd:ResourceType>3</rasd:ResourceType><rasd:VirtualQuantity>4</rasd:VirtualQuantity></Item>
      <Item><rasd:AllocationUnits>byte * 2^20</rasd:AllocationUnits><rasd:ElementName>8192MB of memory</rasd:ElementName><rasd:InstanceID>2</rasd:InstanceID><rasd:ResourceType>4</rasd:ResourceType><rasd:VirtualQuantity>8192</rasd:VirtualQuantity></Item>
      <Item><rasd:Address>0</rasd:Address><rasd:ElementName>SCSI Controller 0</rasd:ElementName><rasd:InstanceID>3</rasd:InstanceID><rasd:ResourceSubType>VirtualSCSI</rasd:ResourceSubType><rasd:ResourceType>6</rasd:ResourceType></Item>
      <Item><rasd:Address>0</rasd:Address><rasd:ElementName>SATA Controller 0</rasd:ElementName><rasd:InstanceID>4</rasd:InstanceID><rasd:ResourceSubType>AHCI</rasd:ResourceSubType><rasd:ResourceType>20</rasd:ResourceType></Item>
      <Item><rasd:AddressOnParent>0</rasd:AddressOnParent><rasd:ElementName>Hard Disk 1</rasd:ElementName><rasd:HostResource>ovf:/disk/vmdisk1</rasd:HostResource><rasd:InstanceID>5</rasd:InstanceID><rasd:Parent>3</rasd:Parent><rasd:ResourceType>17</rasd:ResourceType></Item>
      <Item><rasd:AddressOnParent>1</rasd:AddressOnParent><rasd:ElementName>Hard Disk 2</rasd:ElementName><rasd:HostResource>ovf:/disk/vmdisk2</rasd:HostResource><rasd:InstanceID>6</rasd:InstanceID><rasd:Parent>4</rasd:Parent><rasd:ResourceType>17</rasd:ResourceType></Item>
      <Item><rasd:AutomaticAllocation>true</rasd:AutomaticAllocation><rasd:Connection>VM Network</rasd:Connection><rasd:ElementName>Ethernet 1</rasd:ElementName><rasd:InstanceID>7</rasd:InstanceID><rasd:ResourceSubType>VmxNet3</rasd:ResourceSubType><rasd:ResourceType>10</rasd:ResourceType></Item>
      <Item><rasd:Connection>Mgmt</rasd:Connection><rasd:ElementName>Ethernet 2</rasd:ElementName><rasd:InstanceID>8</rasd:InstanceID><rasd:ResourceSubType>E1000e</rasd:ResourceSubType><rasd:ResourceType>10</rasd:ResourceType></Item>
      <Item><rasd:ElementName>CD/DVD Drive 1</rasd:ElementName><rasd:InstanceID>9</rasd:InstanceID><rasd:ResourceType>15</rasd:ResourceType></Item>
      <Item><rasd:ElementName>Video card</rasd:ElementName><rasd:InstanceID>10</rasd:InstanceID><rasd:ResourceType>24</rasd:ResourceType></Item>
      <Item><rasd:ElementName>Weird thing</rasd:ElementName><rasd:InstanceID>11</rasd:InstanceID><rasd:ResourceType>99</rasd:ResourceType></Item>
      <vmw:Config ovf:required="false" vmw:key="firmware" vmw:value="efi"/>
    </VirtualHardwareSection>
  </VirtualSystem>
</Envelope>"#;

    #[test]
    fn maps_cpu_memory_disks_buses_nics_firmware_and_reports_the_rest() {
        let p = plan_from_ovf(OVF).unwrap();
        assert_eq!(p.name, "web & app");
        assert_eq!((p.cpus, p.memory_mb), (4, 8192));
        assert_eq!(p.firmware, "efi");
        assert_eq!(p.os.as_deref(), Some("Ubuntu Linux (64-bit)"));
        assert_eq!(p.disks.len(), 2);
        assert_eq!(
            (
                p.disks[0].file.as_str(),
                p.disks[0].bus.as_str(),
                p.disks[0].capacity_bytes
            ),
            ("web-disk1.vmdk", "scsi", Some(20 << 30))
        );
        assert_eq!(
            (p.disks[1].bus.as_str(), p.disks[1].capacity_bytes),
            ("sata", Some(10 << 30))
        );
        assert_eq!(
            p.nics
                .iter()
                .map(|n| (n.model.as_str(), n.network.as_str()))
                .collect::<Vec<_>>(),
            vec![("virtio", "VM Network"), ("e1000", "Mgmt")]
        );
        let w = p.warnings.join("\n");
        assert!(w.contains("VmxNet3") && w.contains("virtio"), "{w}");
        assert!(w.contains("CD-ROM"), "{w}");
        assert!(w.contains("resource type 99"), "{w}");
        assert!(!w.contains("Video"), "video is silently ignored: {w}");
    }

    #[test]
    fn windows_guests_get_drivers_they_have() {
        let x = OVF
            .replace(
                "Ubuntu Linux (64-bit)",
                "Microsoft Windows Server 2019 (64-bit)",
            )
            .replace("web &amp; app", "win");
        let p = plan_from_ovf(&x).unwrap();
        assert!(p.disks.iter().all(|d| d.bus == "sata"), "{:?}", p.disks);
        assert!(p.nics.iter().all(|n| n.model == "e1000"), "{:?}", p.nics);
        assert!(p.warnings.iter().any(|w| w.contains("Windows")));
    }

    #[test]
    fn refuses_what_it_cannot_or_must_not_import() {
        assert!(plan_from_ovf("<html/>").unwrap_err().contains("Envelope"));
        assert!(plan_from_ovf("<Envelope><VirtualSystem/></Envelope>").is_err());
        // path-like disk names
        let bad = OVF.replace("web-disk1.vmdk", "../../etc/passwd");
        assert!(plan_from_ovf(&bad).unwrap_err().contains("plain file name"));
        let bad = OVF.replace("web-disk1.vmdk", "/etc/shadow");
        assert!(plan_from_ovf(&bad).is_err());
        // DTD / entity bombs
        let bomb = "<?xml version=\"1.0\"?><!DOCTYPE a [<!ENTITY x \"y\">]><Envelope/>";
        assert!(plan_from_ovf(bomb).unwrap_err().contains("DOCTYPE"));
        assert!(parse_xml("<a>&bogus;</a>").is_err());
        assert!(parse_xml("<a><b></a></b>").is_err());
        assert!(parse_xml("<a>").is_err());
        let deep = format!("{}{}", "<a>".repeat(80), "</a>".repeat(80));
        assert!(parse_xml(&deep).unwrap_err().contains("deeper"));
        // multiple virtual systems: first imported, said so
        let multi = OVF.replace(
            "</VirtualSystem>",
            "</VirtualSystem><VirtualSystem ovf:id=\"second\"/>",
        );
        let p = plan_from_ovf(&multi).unwrap();
        assert!(p.warnings.iter().any(|w| w.contains("2 virtual systems")));
    }

    #[test]
    fn xml_reader_handles_cdata_entities_quotes_and_comments() {
        let n = parse_xml(
            "<?xml version='1.0'?><r a='1 &gt; 0' b=\"x&#65;&#x42;\"><!-- c --><t><![CDATA[<raw & text>]]></t><s/></r>",
        )
        .unwrap();
        assert_eq!(n.attr("a"), Some("1 > 0"));
        assert_eq!(n.attr("b"), Some("xAB"));
        assert_eq!(n.child("t").unwrap().text, "<raw & text>");
        assert!(n.child("s").is_some());
    }

    /// Build a minimal ustar archive.
    fn tar(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        for (name, data) in files {
            let mut h = [0u8; 512];
            h[..name.len()].copy_from_slice(name.as_bytes());
            h[100..107].copy_from_slice(b"0000644");
            h[124..135].copy_from_slice(format!("{:011o}", data.len()).as_bytes());
            h[156] = b'0';
            h[257..262].copy_from_slice(b"ustar");
            h[148..156].copy_from_slice(b"        ");
            let sum: u32 = h.iter().map(|b| *b as u32).sum();
            h[148..155].copy_from_slice(format!("{sum:06o}\0").as_bytes());
            out.extend_from_slice(&h);
            out.extend_from_slice(data);
            out.extend(std::iter::repeat_n(0u8, (512 - data.len() % 512) % 512));
        }
        out.extend(std::iter::repeat_n(0u8, 1024));
        out
    }

    #[test]
    fn tar_index_finds_regular_files_and_extracts_them() {
        let t = tar(&[("a.ovf", b"<Envelope/>"), ("dir/disk.vmdk", &[7u8; 700])]);
        let mut c = std::io::Cursor::new(t);
        let idx = tar_index(&mut c).unwrap();
        assert_eq!(idx.len(), 2);
        assert_eq!((idx[0].name.as_str(), idx[0].size), ("a.ovf", 11));
        assert_eq!((idx[1].name.as_str(), idx[1].size), ("dir/disk.vmdk", 700));
        let d = std::env::temp_dir().join(format!("ovf-tar-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        extract(&mut c, &idx[1], &d.join("o")).unwrap();
        assert_eq!(std::fs::read(d.join("o")).unwrap(), vec![7u8; 700]);
        let _ = std::fs::remove_dir_all(&d);
        // not a tar / truncated
        assert!(tar_index(&mut std::io::Cursor::new(vec![1u8; 2048])).is_err());
        let mut cut = tar(&[("x", &[1u8; 5000])]);
        cut.truncate(1500);
        assert!(tar_index(&mut std::io::Cursor::new(cut))
            .unwrap_err()
            .contains("truncated"));
    }
}
