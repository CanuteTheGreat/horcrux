//! Where Proxmox guests are read from: a directory (a copy of the guest
//! configs next to the received volumes: the usual case when a host is
//! reinstalled and its pool was `zfs send`-ed aside), a live Proxmox node
//! over SSH, or a vzdump archive.
use super::conf::{PveVol, Resolved, VolKind};
use crate::error::ApiError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Where the volumes of one Proxmox storage now are.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageLoc {
    /// A ZFS dataset prefix: `vm-*` zvols are `/dev/zvol/<prefix>/<vol>`,
    /// `subvol-*` datasets are mounted directories.
    Zfs(String),
    /// A directory: Proxmox's `images/<vmid>/<file>` layout or files and
    /// directories directly inside it.
    Dir(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Source {
    /// A directory holding the guest configs (`qemu-server/*.conf`,
    /// `lxc/*.conf`, a copy of `/etc/pve`, or the `.conf` files directly),
    /// optionally `storage.cfg` and saved `qm showcmd` output.
    Dir {
        path: String,
        /// Proxmox storage id -> where its volumes are on this host.
        #[serde(default)]
        storage_map: BTreeMap<String, StorageLoc>,
    },
    /// A running Proxmox node (`ssh [-p port] [-i identity] host`), read only.
    Ssh {
        host: String,
        #[serde(default)]
        port: Option<u16>,
        #[serde(default)]
        identity: Option<String>,
    },
    /// One vzdump archive (`vzdump-lxc-<id>-*.tar[.zst|.gz|.lzo]`).
    Vzdump { path: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Vm,
    Ct,
}

#[derive(Debug, Clone)]
pub struct Guest {
    pub vmid: u32,
    pub kind: Kind,
    pub conf: String,
    /// `qm showcmd` of the source (when known).
    pub showcmd: Option<String>,
}

/// What the volumes resolve to (precomputed for SSH, where every lookup is a
/// remote call).
#[derive(Debug, Clone, Default)]
pub struct Volumes {
    pub map: BTreeMap<String, Result<Resolved, String>>,
}

fn safe_path(p: &str) -> Result<PathBuf, ApiError> {
    let pb = PathBuf::from(p.trim());
    if !pb.is_absolute() || pb.components().any(|c| c.as_os_str() == "..") {
        return Err(ApiError::BadRequest(format!(
            "{p:?}: an absolute path without '..' is required"
        )));
    }
    Ok(pb)
}

fn valid_host(h: &str) -> bool {
    !h.is_empty()
        && !h.starts_with('-')
        && h.len() <= 255
        && h.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '@' | ':' | '[' | ']')
        })
}

impl Source {
    pub fn validate(&self) -> Result<(), ApiError> {
        match self {
            Source::Dir { path, storage_map } => {
                let p = safe_path(path)?;
                if !p.is_dir() {
                    return Err(ApiError::BadRequest(format!(
                        "{path} is not a directory on this host"
                    )));
                }
                for (id, loc) in storage_map {
                    match loc {
                        StorageLoc::Dir(d) => {
                            safe_path(d)?;
                        }
                        StorageLoc::Zfs(ds) => {
                            if ds.is_empty()
                                || ds.starts_with('/')
                                || ds.contains("..")
                                || !ds.chars().all(|c| {
                                    c.is_ascii_alphanumeric()
                                        || matches!(c, '/' | '-' | '_' | '.' | ':')
                                })
                            {
                                return Err(ApiError::BadRequest(format!(
                                    "storage {id}: {ds:?} is not a ZFS dataset name"
                                )));
                            }
                        }
                    }
                }
                Ok(())
            }
            Source::Ssh { host, identity, .. } => {
                if !valid_host(host) {
                    return Err(ApiError::BadRequest(format!(
                        "{host:?} is not a host name ([user@]host)"
                    )));
                }
                if let Some(i) = identity {
                    safe_path(i)?;
                }
                Ok(())
            }
            Source::Vzdump { path } => {
                let p = safe_path(path)?;
                if !p.is_file() {
                    return Err(ApiError::BadRequest(format!(
                        "{path} is not a file on this host"
                    )));
                }
                Ok(())
            }
        }
    }

    /// `ssh` argv prefix for an SSH source.
    pub fn ssh_argv(&self) -> Option<Vec<String>> {
        let Source::Ssh {
            host,
            port,
            identity,
        } = self
        else {
            return None;
        };
        let mut a: Vec<String> = vec![
            "ssh".into(),
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            "ConnectTimeout=15".into(),
        ];
        if let Some(p) = port {
            a.extend(["-p".into(), p.to_string()]);
        }
        if let Some(i) = identity {
            a.extend(["-i".into(), i.clone()]);
        }
        a.push(host.clone());
        Some(a)
    }
}

/// Run `cmd` (a shell command) on the SSH source.
pub async fn ssh_run(src: &Source, cmd: &str) -> Result<Vec<u8>, ApiError> {
    let argv = src
        .ssh_argv()
        .ok_or_else(|| ApiError::Internal("not an SSH source".into()))?;
    let out = tokio::process::Command::new(&argv[0])
        .args(&argv[1..])
        .arg(cmd)
        .output()
        .await
        .map_err(|e| ApiError::Internal(format!("ssh: {e}")))?;
    if !out.status.success() {
        return Err(ssh_error(src, &String::from_utf8_lossy(&out.stderr)));
    }
    Ok(out.stdout)
}

/// A readable error for a failed ssh call (the host key case says what to do).
pub fn ssh_error(src: &Source, stderr: &str) -> ApiError {
    let e = stderr.trim();
    if e.contains("Host key verification failed")
        || e.contains("REMOTE HOST IDENTIFICATION HAS CHANGED")
    {
        let (host, port) = match src {
            Source::Ssh { host, port, .. } => (
                host.rsplit('@').next().unwrap_or(host).to_string(),
                port.unwrap_or(22),
            ),
            _ => (String::new(), 22),
        };
        return ApiError::BadRequest(format!(
            "the Proxmox node's SSH host key is not known to this host (or changed), so the connection was refused: check the node's fingerprint (on the node: ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub), then add it on this host as root: ssh-keyscan -p {port} {host} >> /root/.ssh/known_hosts"
        ));
    }
    if e.contains("Permission denied") {
        return ApiError::BadRequest(format!(
            "the Proxmox node refused the login ({e}): give a key file this host's root can read whose public key is in the node's /root/.ssh/authorized_keys"
        ));
    }
    ApiError::BadRequest(format!("on the Proxmox node: {e}"))
}

/// Single-quote for a remote shell.
pub fn sq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn conf_kind(text: &str) -> Kind {
    let c = super::conf::parse_conf(text);
    if c.get("rootfs").is_some()
        || (c.get("hostname").is_some() && c.get("ostype").is_some() && c.get("name").is_none())
    {
        Kind::Ct
    } else {
        Kind::Vm
    }
}

fn vmid_of(file: &str) -> Option<u32> {
    file.strip_suffix(".conf")?.parse().ok()
}

/// Every guest of the source.
pub async fn list(src: &Source) -> Result<Vec<Guest>, ApiError> {
    match src {
        Source::Dir { path, .. } => list_dir(Path::new(path)),
        Source::Vzdump { path } => list_vzdump(Path::new(path)).await,
        Source::Ssh { .. } => list_ssh(src).await,
    }
}

fn list_dir(root: &Path) -> Result<Vec<Guest>, ApiError> {
    let mut dirs: Vec<(PathBuf, Option<Kind>)> = Vec::new();
    for base in [root.to_path_buf(), root.join("etc/pve")] {
        dirs.push((base.join("qemu-server"), Some(Kind::Vm)));
        dirs.push((base.join("lxc"), Some(Kind::Ct)));
        if let Ok(rd) = std::fs::read_dir(base.join("nodes")) {
            for n in rd.flatten() {
                dirs.push((n.path().join("qemu-server"), Some(Kind::Vm)));
                dirs.push((n.path().join("lxc"), Some(Kind::Ct)));
            }
        }
    }
    dirs.push((root.to_path_buf(), None));
    let showcmd = read_showcmds(root);
    let mut out: BTreeMap<u32, Guest> = BTreeMap::new();
    for (d, kind) in dirs {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            let Some(vmid) = vmid_of(&n) else { continue };
            if out.contains_key(&vmid) || !e.path().is_file() {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(e.path()) else {
                continue;
            };
            let kind = kind.unwrap_or_else(|| conf_kind(&text));
            out.insert(
                vmid,
                Guest {
                    vmid,
                    kind,
                    showcmd: showcmd.get(&vmid).cloned(),
                    conf: text,
                },
            );
        }
    }
    if out.is_empty() {
        return Err(ApiError::BadRequest(format!(
            "no Proxmox guest configs (<id>.conf) in {} (looked in qemu-server/, lxc/, etc/pve/ and the directory itself)",
            root.display()
        )));
    }
    Ok(out.into_values().collect())
}

/// Saved `qm showcmd` output: `showcmd/<id>.txt`, `<id>.showcmd`, or one dump
/// with `===== VM <id>` headers (`guests-qm-showcmd.txt`).
fn read_showcmds(root: &Path) -> BTreeMap<u32, String> {
    let mut m = BTreeMap::new();
    for f in ["guests-qm-showcmd.txt", "qm-showcmd.txt"] {
        if let Ok(t) = std::fs::read_to_string(root.join(f)) {
            m.extend(super::cmdline::split_showcmd_dump(&t));
        }
    }
    if let Ok(rd) = std::fs::read_dir(root.join("showcmd")) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if let Some(id) = n.strip_suffix(".txt").and_then(|x| x.parse().ok()) {
                if let Ok(t) = std::fs::read_to_string(e.path()) {
                    m.insert(id, t);
                }
            }
        }
    }
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if let Some(id) = n.strip_suffix(".showcmd").and_then(|x| x.parse().ok()) {
                if let Ok(t) = std::fs::read_to_string(e.path()) {
                    m.insert(id, t);
                }
            }
        }
    }
    m
}

async fn list_vzdump(path: &Path) -> Result<Vec<Guest>, ApiError> {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    if name.contains(".vma") {
        return Err(ApiError::NotAvailable {
            reason: "vzdump-vma",
            message: format!("{name} is a VM backup in Proxmox's VMA format, which Horcrux cannot read directly"),
            hint: "Restore it on a Proxmox node (qmrestore) and import from there over SSH, or unpack it with Proxmox's `vma extract` into a directory together with its qemu-server.conf and import that directory".into(),
        });
    }
    let parts: Vec<&str> = name.split('-').collect();
    let vmid = parts
        .iter()
        .position(|p| *p == "lxc")
        .and_then(|i| parts.get(i + 1))
        .and_then(|v| v.parse::<u32>().ok())
        .ok_or_else(|| {
            ApiError::BadRequest(format!(
                "{name} is not a container vzdump archive (vzdump-lxc-<id>-<time>.tar[.zst|.gz])"
            ))
        })?;
    let flag = tar_flag(&name);
    let mut cmd = tokio::process::Command::new("tar");
    if !flag.is_empty() {
        cmd.arg(flag);
    }
    let out = cmd
        .arg("-xOf")
        .arg(path)
        .arg("./etc/vzdump/pct.conf")
        .output()
        .await
        .map_err(|e| ApiError::Internal(format!("tar: {e}")))?;
    if !out.status.success() {
        return Err(ApiError::BadRequest(format!(
            "{name}: no ./etc/vzdump/pct.conf inside ({})",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(vec![Guest {
        vmid,
        kind: Kind::Ct,
        conf: String::from_utf8_lossy(&out.stdout).to_string(),
        showcmd: None,
    }])
}

pub fn tar_flag(name: &str) -> &'static str {
    if name.ends_with(".zst") {
        "--zstd"
    } else if name.ends_with(".gz") || name.ends_with(".tgz") {
        "-z"
    } else if name.ends_with(".lzo") {
        "--lzop"
    } else if name.ends_with(".xz") {
        "-J"
    } else {
        ""
    }
}

async fn list_ssh(src: &Source) -> Result<Vec<Guest>, ApiError> {
    // One round trip: every config, separated by markers.
    let script = r#"for f in /etc/pve/qemu-server/*.conf /etc/pve/lxc/*.conf; do [ -f "$f" ] || continue; echo "=====HXCONF $f"; cat "$f"; done"#;
    let out = ssh_run(src, script).await?;
    let text = String::from_utf8_lossy(&out);
    let mut guests = Vec::new();
    let mut cur: Option<(u32, Kind)> = None;
    let mut buf = String::new();
    let flush = |cur: &mut Option<(u32, Kind)>, buf: &mut String, guests: &mut Vec<Guest>| {
        if let Some((vmid, kind)) = cur.take() {
            guests.push(Guest {
                vmid,
                kind,
                conf: std::mem::take(buf),
                showcmd: None,
            });
        }
        buf.clear();
    };
    for l in text.lines() {
        if let Some(f) = l.strip_prefix("=====HXCONF ") {
            flush(&mut cur, &mut buf, &mut guests);
            let kind = if f.contains("/lxc/") {
                Kind::Ct
            } else {
                Kind::Vm
            };
            cur = Path::new(f)
                .file_name()
                .and_then(|n| vmid_of(&n.to_string_lossy()))
                .map(|id| (id, kind));
            continue;
        }
        buf.push_str(l);
        buf.push('\n');
    }
    flush(&mut cur, &mut buf, &mut guests);
    if guests.is_empty() {
        return Err(ApiError::BadRequest(
            "the node has no guests in /etc/pve/qemu-server or /etc/pve/lxc (is it a Proxmox VE node?)".into(),
        ));
    }
    // qm showcmd for the VMs (best effort).
    for g in guests.iter_mut().filter(|g| g.kind == Kind::Vm) {
        if let Ok(o) = ssh_run(src, &format!("qm showcmd {} --pretty", g.vmid)).await {
            g.showcmd = Some(String::from_utf8_lossy(&o).to_string());
        }
    }
    Ok(guests)
}

/// Every volume spec a guest config names (for one batched lookup).
pub fn volume_specs(conf: &super::conf::PveConf) -> Vec<String> {
    let mut v = Vec::new();
    for (k, val) in &conf.keys {
        let is_vol = k == "efidisk0"
            || k == "tpmstate0"
            || k == "rootfs"
            || (k.starts_with("mp") && k[2..].parse::<u32>().is_ok())
            || ["scsi", "virtio", "sata", "ide"]
                .iter()
                .any(|p| k.strip_prefix(p).is_some_and(|n| n.parse::<u32>().is_ok()));
        if is_vol {
            let pv = PveVol::parse(val);
            if pv.storage.is_some() {
                v.push(pv.spec());
            }
        }
    }
    v
}

/// Proxmox `storage.cfg` -> storage id -> (type, pool or path).
pub fn parse_storage_cfg(text: &str) -> BTreeMap<String, (String, String)> {
    let mut m = BTreeMap::new();
    let mut cur: Option<(String, String)> = None;
    for l in text.lines() {
        if !l.starts_with([' ', '\t']) && l.contains(':') {
            let (t, id) = l.split_once(':').unwrap_or_default();
            cur = Some((id.trim().to_string(), t.trim().to_string()));
            if let Some((id, t)) = &cur {
                m.insert(id.clone(), (t.clone(), String::new()));
            }
            continue;
        }
        let t = l.trim();
        if let (Some((id, _)), Some((k, v))) = (&cur, t.split_once(char::is_whitespace)) {
            if k == "pool" || k == "path" {
                if let Some(e) = m.get_mut(id) {
                    e.1 = v.trim().to_string();
                }
            }
        }
    }
    m
}

fn format_of(path: &str) -> String {
    if path.ends_with(".qcow2") {
        "qcow2".into()
    } else if path.ends_with(".vmdk") {
        "vmdk".into()
    } else {
        "raw".into()
    }
}

/// `(mounted, mountpoint)` of a dataset (None: no such dataset / no zfs).
fn zfs_mount(ds: &str) -> Option<(bool, PathBuf)> {
    let out = std::process::Command::new("zfs")
        .args(["get", "-H", "-o", "value", "mounted,mountpoint", ds])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let mut l = text.lines().map(str::trim);
    let mounted = l.next()? == "yes";
    let mp = l.next()?.to_string();
    mp.starts_with('/').then(|| (mounted, PathBuf::from(mp)))
}

fn local(path: PathBuf) -> Result<Resolved, String> {
    use std::os::unix::fs::FileTypeExt;
    let m = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let p = path.to_string_lossy().to_string();
    if m.is_dir() {
        Ok(Resolved {
            path: p,
            kind: VolKind::Dir,
            format: "dir".into(),
            size: None,
        })
    } else if m.file_type().is_block_device() {
        Ok(Resolved {
            path: p,
            kind: VolKind::Block,
            format: "raw".into(),
            size: None,
        })
    } else {
        Ok(Resolved {
            format: format_of(&p),
            path: p,
            kind: VolKind::File,
            size: None,
        })
    }
}

/// Resolve one volume of a Dir source on this host.
pub fn resolve_dir(
    root: &Path,
    storage_map: &BTreeMap<String, StorageLoc>,
    storage_cfg: &BTreeMap<String, (String, String)>,
    vol: &PveVol,
) -> Result<Resolved, String> {
    let Some(sid) = &vol.storage else {
        if vol.volume.starts_with('/') {
            return local(PathBuf::from(&vol.volume));
        }
        return Err(format!("{} is not a storage volume", vol.volume));
    };
    let base = vol
        .volume
        .rsplit('/')
        .next()
        .unwrap_or(&vol.volume)
        .to_string();
    let loc = storage_map.get(sid).cloned().or_else(|| {
        storage_cfg.get(sid).and_then(|(t, v)| match t.as_str() {
            "zfspool" if !v.is_empty() => Some(StorageLoc::Zfs(v.clone())),
            "dir" if !v.is_empty() => Some(StorageLoc::Dir(v.clone())),
            _ => None,
        })
    });
    let mut tried: Vec<String> = Vec::new();
    match loc {
        Some(StorageLoc::Zfs(prefix)) => {
            let ds = format!("{}/{base}", prefix.trim_end_matches('/'));
            if base.starts_with("subvol-") || base.starts_with("basevol-") {
                match zfs_mount(&ds) {
                    // a received dataset (`zfs recv -u`) is not mounted: its
                    // mountpoint is an empty directory, never import that
                    Some((false, mp)) => {
                        return Err(format!(
                            "dataset {ds} is not mounted (its files are not at {}): mount it with `zfs mount {ds}` and plan again",
                            mp.display()
                        ))
                    }
                    Some((true, mp)) => return local(mp),
                    None => {}
                }
                let guess = PathBuf::from(format!("/{ds}"));
                tried.push(format!("the mountpoint of dataset {ds}"));
                if guess.is_dir() {
                    return local(guess);
                }
            } else {
                let dev = PathBuf::from(format!("/dev/zvol/{ds}"));
                tried.push(dev.display().to_string());
                if dev.exists() {
                    return local(dev);
                }
            }
        }
        Some(StorageLoc::Dir(d)) => {
            let d = PathBuf::from(d);
            for c in [
                d.join("images").join(&vol.volume),
                d.join(&vol.volume),
                d.join(&base),
            ] {
                tried.push(c.display().to_string());
                if c.exists() {
                    return local(c);
                }
            }
        }
        None => {}
    }
    // files copied next to the configs
    for c in [root.join(&base), root.join("images").join(&base)] {
        tried.push(c.display().to_string());
        if c.exists() {
            return local(c);
        }
    }
    Err(if tried.is_empty() {
        format!("storage {sid} is unknown: map it in storage_map (zfs dataset prefix or directory)")
    } else {
        format!(
            "not found (looked at {}); map storage {sid} in storage_map",
            tried.join(", ")
        )
    })
}

/// Look every volume up on the SSH source in one call (`pvesm path`).
pub async fn resolve_ssh(src: &Source, specs: &[String]) -> Volumes {
    let mut vols = Volumes::default();
    if specs.is_empty() {
        return vols;
    }
    let mut script = String::new();
    for s in specs {
        script.push_str(&format!(
            "p=$(pvesm path {q} 2>/dev/null); z=''; if [ -b \"$p\" ]; then t=block; z=$(blockdev --getsize64 \"$p\" 2>/dev/null); elif [ -d \"$p\" ]; then t=dir; elif [ -f \"$p\" ]; then t=file; z=$(stat -c %s \"$p\" 2>/dev/null); else t=missing; fi; echo \"HXVOL {s}|$t|$z|$p\"\n",
            q = sq(s)
        ));
    }
    match ssh_run(src, &script).await {
        Ok(out) => {
            for l in String::from_utf8_lossy(&out).lines() {
                let Some(r) = l.strip_prefix("HXVOL ") else {
                    continue;
                };
                let f: Vec<&str> = r.splitn(4, '|').collect();
                if f.len() < 4 {
                    continue;
                }
                let size = f[2].trim().parse::<u64>().ok();
                let f = [f[0], f[1], f[3]];
                let res = match f[1] {
                    "block" => Ok(Resolved {
                        path: f[2].into(),
                        kind: VolKind::Block,
                        format: "raw".into(),
                        size,
                    }),
                    "dir" => Ok(Resolved {
                        path: f[2].into(),
                        kind: VolKind::Dir,
                        format: "dir".into(),
                        size: None,
                    }),
                    "file" => Ok(Resolved {
                        path: f[2].into(),
                        kind: VolKind::File,
                        format: format_of(f[2]),
                        size,
                    }),
                    _ => Err(format!("pvesm path {} found nothing on the node", f[0])),
                };
                vols.map.insert(f[0].to_string(), res);
            }
        }
        Err(e) => {
            for s in specs {
                vols.map.insert(s.clone(), Err(format!("{e:?}")));
            }
        }
    }
    vols
}
