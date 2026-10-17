//! LXC container backend (the `pct` equivalent).
//!
//! Horcrux owns each container's LXC config: it is rendered from the
//! container's settings (stored next to it in `<lxcpath>/<name>/horcrux.json`)
//! by `lxc_config`, so every option the API accepts really lands in LXC and
//! survives API restarts. Templates come from `lxc_template` (a verified
//! local cache of images.linuxcontainers.org images); the rootfs is a plain
//! directory or, with a disk size, an ext4 image that can be grown.
use super::lxc_config::{self, ConfigInput, GuestNet, HostEnv, RootfsKind};
use super::lxc_template::{TemplateRef, TemplateStore, DEFAULT_IMAGES_URL};
use super::Container;
use horcrux_common::{
    ContainerConfig, ContainerOptions, ContainerRuntime, ContainerStatus, Error, LxcOptions,
    LxcSnapshot, Result,
};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;
use tracing::{debug, info, warn};

const META: &str = "horcrux.json";
const SNAPS: &str = "horcrux-snaps";
/// Seconds a clean shutdown may take before reboot/shutdown(force) kills.
pub const DEFAULT_SHUTDOWN_TIMEOUT: u32 = 60;

/// What Horcrux stores about a container it manages.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LxcMeta {
    pub id: String,
    pub name: String,
    pub memory: u64,
    pub cpus: u32,
    /// The template the container was created from (as requested).
    pub template: String,
    /// Distro (from the rootfs's os-release), selects LXC's per-distro conf.
    pub distro: String,
    pub arch: String,
    pub rootfs_kind: String,
    /// The guest's network config system (ifupdown/netplan/networkd/none).
    pub guest_net: String,
    pub lxc: LxcOptions,
    pub created_at: String,
    /// Settings changed while running that only apply after a restart.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending: Vec<String>,
}

impl LxcMeta {
    fn kind(&self) -> RootfsKind {
        if self.rootfs_kind == "loop" {
            RootfsKind::Loop
        } else {
            RootfsKind::Dir
        }
    }

    fn guest(&self) -> GuestNet {
        match self.guest_net.as_str() {
            "ifupdown" => GuestNet::Ifupdown,
            "netplan" => GuestNet::Netplan,
            "networkd" => GuestNet::Networkd,
            _ => GuestNet::Unmanaged,
        }
    }

    pub fn to_container(&self, status: ContainerStatus) -> Container {
        Container {
            id: self.id.clone(),
            name: self.name.clone(),
            runtime: ContainerRuntime::Lxc,
            memory: self.memory,
            cpus: self.cpus,
            rootfs: self.template.clone(),
            status,
            options: ContainerOptions::default(),
            created_at: Some(self.created_at.clone()),
            lxc: Some(self.lxc.clone()),
        }
    }
}

/// Response of GET/PUT /api/containers/:id/lxc/config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LxcConfigView {
    pub id: String,
    pub name: String,
    pub memory: u64,
    pub cpus: u32,
    pub template: String,
    pub distro: String,
    pub arch: String,
    pub rootfs_kind: String,
    pub lxc: LxcOptions,
    /// Settings that apply after the next (re)start.
    pub pending: Vec<String>,
    /// The generated LXC config file, for inspection.
    pub raw_config: String,
}

/// GET /api/lxc/host: what the host offers for the create form.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LxcHostInfo {
    pub available: bool,
    pub version: Option<String>,
    pub arch: String,
    pub lxc_path: String,
    pub template_dir: String,
    pub apparmor: bool,
    /// Bridges a container NIC can attach to, with VLAN filtering state.
    pub bridges: Vec<LxcBridge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LxcBridge {
    pub name: String,
    pub vlan_aware: bool,
}

/// LXC container manager
pub struct LxcManager {
    lxc_path: PathBuf,
    share_config: PathBuf,
    templates: TemplateStore,
}

impl Default for LxcManager {
    fn default() -> Self {
        Self::new()
    }
}

fn sys(msg: impl Into<String>) -> Error {
    Error::System(msg.into())
}

/// Run a command, returning stdout or an error carrying stderr.
async fn run(cmd: &mut Command, what: &str) -> Result<String> {
    let out = cmd
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|e| sys(format!("failed to run {what}: {e}")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let msg = if stderr.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            stderr.trim().to_string()
        };
        return Err(sys(format!("{what} failed: {msg}")));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Valid LXC container name (also a directory name under lxcpath).
pub fn valid_lxc_name(name: &str) -> bool {
    let mut c = name.chars();
    matches!(c.next(), Some(f) if f.is_ascii_alphanumeric())
        && name.len() <= 64
        && c.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

/// Snapshot names (Proxmox rules: start with a letter, then letters,
/// digits, '-' or '_').
pub fn valid_snapshot_name(name: &str) -> bool {
    let mut c = name.chars();
    matches!(c.next(), Some(f) if f.is_ascii_alphabetic())
        && name.len() <= 40
        && c.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
}

/// Map an os-release ID to LXC's per-distro config name.
pub fn lxc_distro_name(os_id: &str) -> String {
    match os_id {
        "arch" | "archarm" => "archlinux",
        "opensuse" | "opensuse-leap" | "opensuse-tumbleweed" | "sles" => "opensuse",
        "rhel" | "rocky" | "almalinux" | "centos" => "centos",
        "ol" => "oracle",
        "void" => "voidlinux",
        other => other,
    }
    .to_string()
}

fn os_release_id(rootfs: &Path) -> Option<String> {
    for rel in ["etc/os-release", "usr/lib/os-release"] {
        if let Some(text) = read_inside(rootfs, rel) {
            for l in text.lines() {
                if let Some(v) = l.strip_prefix("ID=") {
                    return Some(v.trim_matches('"').trim_matches('\'').to_lowercase());
                }
            }
        }
    }
    None
}

/// True when every component of `path` below `root` exists and is not a
/// symlink (the guest controls the rootfs; following its symlinks from the
/// host could reach host files).
fn is_inside_without_symlinks(root: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else {
        return false;
    };
    let mut cur = root.to_path_buf();
    for comp in rel.components() {
        cur.push(comp);
        match std::fs::symlink_metadata(&cur) {
            Ok(m) if !m.file_type().is_symlink() => {}
            _ => return false,
        }
    }
    true
}

/// Write `content` to `root/rel` without following symlinks anywhere on the
/// way: missing parent dirs are created (0755), a symlink at the final
/// component is replaced. Files are owned by `owner` (the container's root).
pub fn safe_write(
    root: &Path,
    rel: &str,
    content: &[u8],
    mode: u32,
    owner: (u32, u32),
) -> std::io::Result<()> {
    use std::io::{Error as IoError, ErrorKind, Write as _};
    let rel = Path::new(rel.trim_start_matches('/'));
    let mut cur = root.to_path_buf();
    let comps: Vec<_> = rel.components().collect();
    let (last, parents) = comps
        .split_last()
        .ok_or_else(|| IoError::new(ErrorKind::InvalidInput, "empty path"))?;
    for comp in parents {
        cur.push(comp);
        match std::fs::symlink_metadata(&cur) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => {
                return Err(IoError::new(
                    ErrorKind::InvalidInput,
                    format!("{} is not a directory (symlink?)", cur.display()),
                ))
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {
                std::fs::create_dir(&cur)?;
                std::fs::set_permissions(&cur, std::fs::Permissions::from_mode(0o755))?;
                std::os::unix::fs::lchown(&cur, Some(owner.0), Some(owner.1))?;
            }
            Err(e) => return Err(e),
        }
    }
    cur.push(last);
    if let Ok(m) = std::fs::symlink_metadata(&cur) {
        if m.file_type().is_symlink() {
            std::fs::remove_file(&cur)?;
        } else if m.is_dir() {
            return Err(IoError::new(
                ErrorKind::InvalidInput,
                format!("{} is a directory", cur.display()),
            ));
        }
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(&cur)?;
    f.write_all(content)?;
    f.set_permissions(std::fs::Permissions::from_mode(mode))?;
    std::os::unix::fs::fchown(&f, Some(owner.0), Some(owner.1))?;
    Ok(())
}

/// `root/rel` exists (any file type, the last component may be a symlink)
/// and no parent directory on the way is a symlink.
fn exists_inside(root: &Path, rel: &str) -> bool {
    let p = root.join(rel);
    match p.parent() {
        Some(parent) if parent != root => {
            is_inside_without_symlinks(root, parent) && std::fs::symlink_metadata(&p).is_ok()
        }
        _ => std::fs::symlink_metadata(&p).is_ok(),
    }
}

fn read_inside(root: &Path, rel: &str) -> Option<String> {
    let p = root.join(rel);
    if is_inside_without_symlinks(root, &p) {
        std::fs::read_to_string(p).ok()
    } else {
        None
    }
}

/// Make sure root may map `base..base+size` in /etc/subuid and /etc/subgid
/// (newuidmap refuses unlisted ranges). Appends `root:<base>:<size>` when no
/// existing root entry covers it.
pub fn ensure_subid_range(file: &Path, base: u32, size: u32) -> std::io::Result<bool> {
    let text = std::fs::read_to_string(file).unwrap_or_default();
    let covered = text.lines().any(|l| {
        let f: Vec<&str> = l.trim().split(':').collect();
        if f.len() != 3 || !(f[0] == "root" || f[0] == "0") {
            return false;
        }
        match (f[1].parse::<u64>(), f[2].parse::<u64>()) {
            (Ok(s), Ok(n)) => s <= base as u64 && base as u64 + size as u64 <= s + n,
            _ => false,
        }
    });
    if covered {
        return Ok(false);
    }
    let mut new = text;
    if !new.is_empty() && !new.ends_with('\n') {
        new.push('\n');
    }
    new.push_str(&format!("root:{base}:{size}\n"));
    std::fs::write(file, new)?;
    Ok(true)
}

fn apparmor_usable() -> bool {
    if let Ok(v) = std::env::var("HORCRUX_LXC_APPARMOR") {
        return v == "1";
    }
    let enabled = std::fs::read_to_string("/sys/module/apparmor/parameters/enabled")
        .map(|s| s.trim() == "Y")
        .unwrap_or(false);
    enabled
        && Path::new("/sys/kernel/security/apparmor/profiles").exists()
        && which("apparmor_parser")
}

fn which(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
        .unwrap_or(false)
}

fn tar_decompress_flag(path: &Path) -> &'static str {
    let s = path.to_string_lossy();
    if s.ends_with(".xz") {
        "-J"
    } else if s.ends_with(".gz") || s.ends_with(".tgz") {
        "-z"
    } else if s.ends_with(".zst") {
        "--zstd"
    } else {
        ""
    }
}

impl LxcManager {
    pub fn new() -> Self {
        let lxc_path = std::env::var("HORCRUX_LXC_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/var/lib/lxc"));
        let template_dir = std::env::var("HORCRUX_LXC_TEMPLATE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                std::env::var("HORCRUX_DATA_DIR")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| PathBuf::from("/var/lib/horcrux"))
                    .join("lxc-templates")
            });
        let url = std::env::var("HORCRUX_LXC_IMAGES_URL")
            .unwrap_or_else(|_| DEFAULT_IMAGES_URL.to_string());
        Self::with_paths(lxc_path, template_dir, url)
    }

    pub fn with_paths(lxc_path: PathBuf, template_dir: PathBuf, images_url: String) -> Self {
        Self {
            lxc_path,
            share_config: std::env::var("HORCRUX_LXC_SHARE_CONFIG")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/usr/share/lxc/config")),
            templates: TemplateStore::new(template_dir, images_url),
        }
    }

    pub fn templates(&self) -> &TemplateStore {
        &self.templates
    }

    pub fn lxc_path(&self) -> &Path {
        &self.lxc_path
    }

    fn dir(&self, name: &str) -> PathBuf {
        self.lxc_path.join(name)
    }

    /// `lxc-<tool> -P <lxcpath> -n <name>`
    fn lxc(&self, tool: &str, name: &str) -> Command {
        let mut c = Command::new(format!("lxc-{tool}"));
        c.arg("-P").arg(&self.lxc_path).arg("-n").arg(name);
        c
    }

    fn host_env(&self) -> HostEnv {
        HostEnv {
            lxc_share_config: self.share_config.clone(),
            apparmor: apparmor_usable(),
        }
    }

    pub fn read_meta(&self, name: &str) -> Result<LxcMeta> {
        let p = self.dir(name).join(META);
        let text = std::fs::read_to_string(&p).map_err(|_| {
            Error::InvalidConfig(format!(
                "LXC container {name} was not created by Horcrux (no {META}); \
                 it can be started/stopped but not reconfigured here"
            ))
        })?;
        let mut meta: LxcMeta =
            serde_json::from_str(&text).map_err(|e| sys(format!("{}: {e}", p.display())))?;
        // The directory is authoritative (a restored/copied container dir
        // may carry another container's name in its metadata).
        meta.name = name.to_string();
        Ok(meta)
    }

    /// Re-own a container directory that was restored from a backup (or
    /// copied in) under `name`: record its Horcrux id and re-render its
    /// config, seccomp policy and volume paths for the new location.
    /// Containers without Horcrux metadata are left as they are.
    pub fn adopt_restored(&self, name: &str, id: &str) -> Result<()> {
        match self.read_meta(name) {
            Ok(mut meta) => {
                meta.id = id.to_string();
                meta.pending.clear();
                self.write_all(&meta)
            }
            Err(Error::InvalidConfig(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn write_meta(&self, meta: &LxcMeta) -> Result<()> {
        let p = self.dir(&meta.name).join(META);
        let tmp = p.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(meta).map_err(|e| Error::Internal(e.to_string()))?,
        )?;
        std::fs::rename(&tmp, &p)?;
        Ok(())
    }

    fn render(&self, meta: &LxcMeta) -> String {
        let dir = self.dir(&meta.name);
        lxc_config::render_config(&ConfigInput {
            name: &meta.name,
            dir: &dir,
            distro: &meta.distro,
            arch: &meta.arch,
            memory_mb: meta.memory,
            cpus: meta.cpus,
            rootfs: meta.kind(),
            opts: &meta.lxc,
            guest_net_managed: meta.guest() != GuestNet::Unmanaged,
            host: &self.host_env(),
        })
    }

    /// Write config + seccomp policy + metadata, and create managed volumes.
    fn write_all(&self, meta: &LxcMeta) -> Result<()> {
        let dir = self.dir(&meta.name);
        let owner = self.root_owner(&meta.lxc);
        for (i, m) in meta.lxc.mounts.iter().enumerate() {
            match &m.source {
                None => {
                    let v = lxc_config::mount_source(&dir, i, m);
                    if !v.exists() {
                        std::fs::create_dir_all(&v)?;
                        std::os::unix::fs::lchown(&v, Some(owner.0), Some(owner.1))?;
                    }
                }
                Some(src) => {
                    if !Path::new(src).is_dir() {
                        return Err(Error::Validation(format!(
                            "bind mount source {src} is not a directory on this host"
                        )));
                    }
                }
            }
        }
        std::fs::write(
            dir.join("seccomp.conf"),
            lxc_config::render_seccomp(&meta.lxc),
        )?;
        let tmp = dir.join("config.tmp");
        std::fs::write(&tmp, self.render(meta))?;
        std::fs::rename(&tmp, dir.join("config"))?;
        self.write_meta(meta)
    }

    fn root_owner(&self, o: &LxcOptions) -> (u32, u32) {
        if o.unprivileged {
            (o.idmap_base(), o.idmap_base())
        } else {
            (0, 0)
        }
    }

    /// Host prerequisites: LXC tools, lxcpath, subuid/subgid for the map.
    fn prepare_host(&self, o: &LxcOptions) -> Result<()> {
        if !Self::check_lxc_available() {
            return Err(sys(
                "LXC is not installed on this host (lxc-start not found; emerge app-containers/lxc)",
            ));
        }
        std::fs::create_dir_all(&self.lxc_path)?;
        if o.unprivileged {
            for f in ["subuid", "subgid"] {
                let f = std::env::var("HORCRUX_LXC_SUBID_DIR")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| PathBuf::from("/etc"))
                    .join(f);
                if ensure_subid_range(&f, o.idmap_base(), o.idmap_size())? {
                    info!(
                        "Added root:{}:{} to {} for unprivileged LXC containers",
                        o.idmap_base(),
                        o.idmap_size(),
                        f.display()
                    );
                }
            }
        }
        Ok(())
    }

    /// Extract a rootfs tarball into `target` (an empty directory). For an
    /// unprivileged container tar runs inside the container's user
    /// namespace, so file owners land shifted by the id map.
    async fn extract(&self, tarball: &Path, target: &Path, o: &LxcOptions) -> Result<()> {
        let file = std::fs::File::open(tarball)
            .map_err(|e| sys(format!("open {}: {e}", tarball.display())))?;
        let flag = tar_decompress_flag(tarball);
        let mut tar_args: Vec<String> = vec!["tar".into(), "--numeric-owner".into(), "-xp".into()];
        if !flag.is_empty() {
            tar_args.push(flag.into());
        }
        // /dev is populated by LXC (autodev); device nodes can't be created
        // in a user namespace anyway.
        tar_args.extend(["--exclude=./dev/*".into(), "-f".into(), "-".into()]);
        let mut cmd = if o.unprivileged {
            let (b, n) = (o.idmap_base(), o.idmap_size());
            std::os::unix::fs::lchown(target, Some(b), Some(b))?;
            let mut c = Command::new("lxc-usernsexec");
            c.arg("-m")
                .arg(format!("b:0:{b}:{n}"))
                .arg("--")
                .args(&tar_args);
            c
        } else {
            let mut c = Command::new(&tar_args[0]);
            c.args(&tar_args[1..]);
            c
        };
        // cwd is opened as root before the user namespace is entered, so
        // the mapped root can extract even if lxcpath is root-only (0700).
        cmd.current_dir(target).stdin(Stdio::from(file));
        let out = cmd
            .output()
            .await
            .map_err(|e| sys(format!("failed to run tar: {e}")))?;
        if !out.status.success() {
            return Err(sys(format!(
                "extracting {} failed: {}",
                tarball.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(())
    }

    /// Mount a loop rootfs image at `<ct>/.setup-mnt` for host-side work.
    async fn mount_image(&self, name: &str) -> Result<PathBuf> {
        let dir = self.dir(name);
        let mnt = dir.join(".setup-mnt");
        std::fs::create_dir_all(&mnt)?;
        run(
            Command::new("mount")
                .arg("-o")
                .arg("loop,nosuid,nodev")
                .arg(dir.join("rootfs.raw"))
                .arg(&mnt),
            "mount rootfs image",
        )
        .await?;
        Ok(mnt)
    }

    async fn umount_image(&self, mnt: &Path) {
        if let Err(e) = run(Command::new("umount").arg(mnt), "umount rootfs image").await {
            warn!("{e}");
        }
        let _ = std::fs::remove_dir(mnt);
    }

    /// Run `f` with the stopped container's rootfs available on the host.
    async fn with_rootfs<T>(
        &self,
        meta: &LxcMeta,
        f: impl FnOnce(&Path) -> Result<T>,
    ) -> Result<T> {
        match meta.kind() {
            RootfsKind::Dir => f(&self.dir(&meta.name).join("rootfs")),
            RootfsKind::Loop => {
                let mnt = self.mount_image(&meta.name).await?;
                let r = f(&mnt);
                self.umount_image(&mnt).await;
                r
            }
        }
    }

    /// Write hostname, /etc/hosts, network config, resolv.conf and SSH keys
    /// into the (stopped) container, like Proxmox does on every start. A
    /// file is left alone when `/etc/.horcrux-ignore.<file>` exists.
    fn setup_guest(meta: &LxcMeta, rootfs: &Path, owner: (u32, u32)) -> Result<()> {
        let o = &meta.lxc;
        let ignored = |name: &str| {
            is_inside_without_symlinks(rootfs, &rootfs.join(format!("etc/.horcrux-ignore.{name}")))
        };
        let w = |rel: &str, content: &str, mode: u32| -> Result<()> {
            safe_write(rootfs, rel, content.as_bytes(), mode, owner)
                .map_err(|e| sys(format!("writing /{rel} in the container: {e}")))
        };
        let hostname = o.hostname.clone().unwrap_or_else(|| meta.name.clone());
        if !ignored("hostname") {
            w("etc/hostname", &format!("{hostname}\n"), 0o644)?;
        }
        if !ignored("hosts") {
            let existing = read_inside(rootfs, "etc/hosts").unwrap_or_default();
            w(
                "etc/hosts",
                &lxc_config::render_hosts(&existing, &hostname),
                0o644,
            )?;
        }
        let dns = lxc_config::dns_of(o);
        match meta.guest() {
            GuestNet::Ifupdown if !ignored("interfaces") => w(
                "etc/network/interfaces",
                &lxc_config::render_ifupdown(&o.nets),
                0o644,
            )?,
            GuestNet::Netplan if !ignored("netplan") => w(
                "etc/netplan/10-lxc.yaml",
                &lxc_config::render_netplan(&o.nets, dns.as_ref()),
                0o600,
            )?,
            GuestNet::Networkd if !ignored("network") => {
                for (i, n) in o.nets.iter().enumerate() {
                    w(
                        &format!("etc/systemd/network/{}.network", n.if_name(i)),
                        &lxc_config::render_networkd(i, n),
                        0o644,
                    )?;
                }
            }
            _ => {}
        }
        // DNS set in the container settings wins over DHCP: write
        // resolv.conf and stop the guest's DHCP client from replacing it
        // (dhcpcd `nohook resolv.conf`, busybox udhcpc RESOLV_CONF=no).
        // Clearing the setting hands resolv.conf back to DHCP.
        const MARK: &str = "# Added by Horcrux: DNS comes from the container settings";
        let manage_dns = dns.is_some() && !ignored("resolv.conf");
        if let Some(d) = dns.as_ref().filter(|_| manage_dns) {
            w("etc/resolv.conf", &lxc_config::render_resolv_conf(d), 0o644)?;
        }
        if exists_inside(rootfs, "etc/dhcpcd.conf") {
            let cur = read_inside(rootfs, "etc/dhcpcd.conf").unwrap_or_default();
            let has = cur.contains(MARK);
            if manage_dns && !has {
                let mut new = cur.clone();
                if !new.is_empty() && !new.ends_with('\n') {
                    new.push('\n');
                }
                new.push_str(&format!("{MARK}\nnohook resolv.conf\n"));
                w("etc/dhcpcd.conf", &new, 0o644)?;
            } else if !manage_dns && has {
                let new = cur.replace(&format!("{MARK}\nnohook resolv.conf\n"), "");
                w("etc/dhcpcd.conf", &new, 0o644)?;
            }
        }
        let udhcpc_conf = read_inside(rootfs, "etc/udhcpc/udhcpc.conf");
        let ours = udhcpc_conf
            .as_deref()
            .map(|t| t.contains(MARK))
            .unwrap_or(false);
        if manage_dns
            && exists_inside(rootfs, "usr/share/udhcpc/default.script")
            && (udhcpc_conf.is_none() || ours)
        {
            w(
                "etc/udhcpc/udhcpc.conf",
                &format!("{MARK}\nRESOLV_CONF=\"no\"\n"),
                0o644,
            )?;
        } else if !manage_dns && ours {
            let _ = std::fs::remove_file(rootfs.join("etc/udhcpc/udhcpc.conf"));
        }
        if !o.ssh_public_keys.is_empty() {
            let mut keys = o.ssh_public_keys.join("\n");
            keys.push('\n');
            w("root/.ssh/authorized_keys", &keys, 0o600)?;
            let ssh = rootfs.join("root/.ssh");
            if is_inside_without_symlinks(rootfs, &ssh) {
                let _ = std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o700));
            }
        }
        Ok(())
    }

    /// Which network config the guest actually applies at boot: netplan
    /// (Ubuntu), ifupdown/ifupdown-ng when `ifup` is installed (Alpine,
    /// Debian with ifupdown), systemd-networkd, or none we know.
    fn detect_guest_net(rootfs: &Path) -> GuestNet {
        // Binaries are often symlinks (/sbin/ifup -> ifupdown), and /sbin
        // itself is one on merged-/usr distros: check both locations.
        let has = |rel: &str| exists_inside(rootfs, rel);
        let any = |rels: &[&str]| rels.iter().any(|r| has(r));
        if any(&["usr/sbin/netplan", "etc/netplan"]) {
            GuestNet::Netplan
        } else if has("etc/network/interfaces") && any(&["sbin/ifup", "usr/sbin/ifup"]) {
            GuestNet::Ifupdown
        } else if any(&[
            "lib/systemd/systemd-networkd",
            "usr/lib/systemd/systemd-networkd",
        ]) {
            GuestNet::Networkd
        } else {
            GuestNet::Unmanaged
        }
    }

    /// Create a new LXC container (`pct create`).
    pub async fn create_container(&self, config: &ContainerConfig) -> Result<Container> {
        info!(
            "Creating LXC container: {} (ID: {})",
            config.name, config.id
        );
        if !valid_lxc_name(&config.name) {
            return Err(Error::Validation(format!(
                "invalid LXC container name '{}': letters, digits, '-', '_' or '.', starting \
                 with a letter or digit",
                config.name
            )));
        }
        let mut opts = config.lxc.clone().unwrap_or_default();
        opts.validate().map_err(Error::Validation)?;
        let arch = opts
            .arch
            .clone()
            .unwrap_or_else(|| lxc_config::host_arch().to_string());
        let tref = TemplateRef::parse(&config.rootfs, &arch).map_err(Error::Validation)?;
        if let TemplateRef::Catalog { arch: a, .. } = &tref {
            if *a != arch {
                return Err(Error::Validation(format!(
                    "template architecture {a} does not match the container's ({arch})"
                )));
            }
        }
        let dir = self.dir(&config.name);
        if dir.exists() {
            return Err(Error::AlreadyExists(format!(
                "an LXC container named {} already exists on this host",
                config.name
            )));
        }
        self.prepare_host(&opts)?;
        let (tarball, distro_hint) = self.templates.resolve(&tref).await?;

        // Fill in what the user left to us, so it's stable from now on.
        for n in opts.nets.iter_mut() {
            if n.hwaddr.is_none() {
                n.hwaddr = Some(lxc_config::random_mac());
            }
        }
        lxc_config::assign_volume_names(&mut opts.mounts);

        std::fs::create_dir(&dir)?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755))?;
        let result = self
            .create_inner(config, &opts, &arch, &tarball, distro_hint)
            .await;
        match result {
            Ok(meta) => {
                info!("LXC container {} created", config.id);
                Ok(meta.to_container(ContainerStatus::Stopped))
            }
            Err(e) => {
                // Leave nothing half-created behind.
                let mnt = dir.join(".setup-mnt");
                if mnt.exists() {
                    self.umount_image(&mnt).await;
                }
                if let Err(rm) = std::fs::remove_dir_all(&dir) {
                    warn!("cleanup of {} failed: {rm}", dir.display());
                }
                Err(e)
            }
        }
    }

    async fn create_inner(
        &self,
        config: &ContainerConfig,
        opts: &LxcOptions,
        arch: &str,
        tarball: &Path,
        distro_hint: Option<String>,
    ) -> Result<LxcMeta> {
        let dir = self.dir(&config.name);
        let owner = self.root_owner(opts);
        let kind = if opts.disk_gb.is_some() {
            RootfsKind::Loop
        } else {
            RootfsKind::Dir
        };
        let target = match kind {
            RootfsKind::Dir => {
                let r = dir.join("rootfs");
                std::fs::create_dir(&r)?;
                r
            }
            RootfsKind::Loop => {
                let img = dir.join("rootfs.raw");
                let size = opts.disk_gb.unwrap_or(0) * 1024 * 1024 * 1024;
                std::fs::File::create(&img)?.set_len(size)?;
                std::fs::set_permissions(&img, std::fs::Permissions::from_mode(0o600))?;
                run(
                    Command::new("mkfs.ext4")
                        .arg("-q")
                        .arg("-F")
                        .arg("-E")
                        .arg(format!("root_owner={}:{}", owner.0, owner.1))
                        .arg(&img),
                    "mkfs.ext4",
                )
                .await?;
                self.mount_image(&config.name).await?
            }
        };
        let res = async {
            self.extract(tarball, &target, opts).await?;
            let distro = os_release_id(&target)
                .map(|id| lxc_distro_name(&id))
                .or(distro_hint)
                .unwrap_or_else(|| "common".into());
            let guest = Self::detect_guest_net(&target);
            let meta = LxcMeta {
                id: config.id.clone(),
                name: config.name.clone(),
                memory: config.memory,
                cpus: config.cpus,
                template: config.rootfs.trim().to_string(),
                distro,
                arch: arch.to_string(),
                rootfs_kind: if kind == RootfsKind::Loop {
                    "loop".into()
                } else {
                    "dir".into()
                },
                guest_net: match guest {
                    GuestNet::Ifupdown => "ifupdown",
                    GuestNet::Netplan => "netplan",
                    GuestNet::Networkd => "networkd",
                    GuestNet::Unmanaged => "none",
                }
                .into(),
                lxc: opts.clone(),
                created_at: chrono::Utc::now().to_rfc3339(),
                pending: Vec::new(),
            };
            Self::setup_guest(&meta, &target, owner)?;
            Ok::<_, Error>(meta)
        }
        .await;
        if kind == RootfsKind::Loop {
            self.umount_image(&target).await;
        }
        let meta = res?;
        self.write_all(&meta)?;
        Ok(meta)
    }

    async fn wait_state(&self, name: &str, state: &str, secs: u32) -> Result<()> {
        run(
            self.lxc("wait", name)
                .arg("-s")
                .arg(state)
                .arg("-t")
                .arg(secs.to_string()),
            &format!("waiting for {name} to reach {state}"),
        )
        .await
        .map(|_| ())
    }

    /// Start (`pct start`): re-applies guest settings, then lxc-start.
    pub async fn start_container(&self, container: &Container) -> Result<()> {
        info!("Starting LXC container: {}", container.name);
        let status = self.get_container_status(&container.name).await?;
        if status == ContainerStatus::Running {
            return Err(Error::InvalidConfig(format!(
                "Container {} is already running",
                container.id
            )));
        }
        if let Ok(mut meta) = self.read_meta(&container.name) {
            let owner = self.root_owner(&meta.lxc);
            let m2 = meta.clone();
            self.with_rootfs(&meta, move |root| Self::setup_guest(&m2, root, owner))
                .await?;
            meta.pending.clear();
            self.write_all(&meta)?;
        }
        let log = self.dir(&container.name).join("start.log");
        let _ = std::fs::remove_file(&log);
        let started = run(
            self.lxc("start", &container.name)
                .arg("-l")
                .arg("WARN")
                .arg("-o")
                .arg(&log),
            "lxc-start",
        )
        .await;
        if let Err(e) = started {
            let detail = std::fs::read_to_string(&log)
                .map(|t| {
                    t.lines()
                        .filter(|l| l.contains("ERROR"))
                        .take(5)
                        .collect::<Vec<_>>()
                        .join("; ")
                })
                .unwrap_or_default();
            return Err(sys(format!("{e} {detail}")));
        }
        self.wait_state(&container.name, "RUNNING", 30).await?;
        info!("LXC container {} started", container.id);
        Ok(())
    }

    /// Hard stop (`pct stop`): kill the container immediately.
    pub async fn stop_container(&self, container: &Container) -> Result<()> {
        info!("Stopping LXC container: {}", container.name);
        if self.get_container_status(&container.name).await? == ContainerStatus::Stopped {
            return Err(Error::InvalidConfig(format!(
                "Container {} is already stopped",
                container.id
            )));
        }
        run(self.lxc("stop", &container.name).arg("-k"), "lxc-stop").await?;
        Ok(())
    }

    /// Clean shutdown (`pct shutdown`): ask init to power off, wait up to
    /// `timeout` seconds; then kill if `force`, else report the failure.
    pub async fn shutdown_container(&self, name: &str, timeout: u32, force: bool) -> Result<()> {
        info!("Shutting down LXC container {name} (timeout {timeout}s, force {force})");
        if self.get_container_status(name).await? == ContainerStatus::Stopped {
            return Err(Error::InvalidConfig(format!(
                "Container {name} is already stopped"
            )));
        }
        let mut c = self.lxc("stop", name);
        c.arg("-t").arg(timeout.to_string());
        if !force {
            c.arg("--nokill");
        }
        let r = run(&mut c, "lxc-stop (shutdown)").await;
        if self.get_container_status(name).await? == ContainerStatus::Stopped {
            return Ok(());
        }
        r?;
        Err(Error::Conflict(format!(
            "container {name} did not shut down within {timeout}s (retry with force)"
        )))
    }

    /// Reboot (`pct reboot`): clean shutdown (killed after `timeout`), then
    /// start, which also applies pending configuration changes.
    pub async fn reboot_container(&self, container: &Container, timeout: u32) -> Result<()> {
        if self.get_container_status(&container.name).await? != ContainerStatus::Running {
            return Err(Error::InvalidConfig(format!(
                "Container {} is not running",
                container.id
            )));
        }
        self.shutdown_container(&container.name, timeout, true)
            .await?;
        self.start_container(container).await
    }

    /// Delete (`pct destroy`): container, its snapshots and managed volumes.
    pub async fn delete_container(&self, container: &Container) -> Result<()> {
        info!("Deleting LXC container: {}", container.name);
        match self.get_container_status(&container.name).await {
            Ok(ContainerStatus::Stopped) | Err(Error::ContainerNotFound(_)) => {}
            Ok(_) => {
                return Err(Error::InvalidConfig(format!(
                    "Cannot delete running container {}. Stop it first.",
                    container.id
                )))
            }
            Err(e) => return Err(e),
        }
        let dir = self.dir(&container.name);
        let snaps = dir.join(SNAPS);
        if snaps.exists() {
            tokio::fs::remove_dir_all(&snaps).await?;
        }
        if dir.join("config").exists() {
            run(&mut self.lxc("destroy", &container.name), "lxc-destroy").await?;
        }
        if dir.exists() {
            tokio::fs::remove_dir_all(&dir).await?;
        }
        Ok(())
    }

    /// Check if LXC is available
    pub fn check_lxc_available() -> bool {
        std::process::Command::new("lxc-start")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Get LXC version
    pub async fn get_lxc_version() -> Result<String> {
        let out = run(
            Command::new("lxc-start").arg("--version"),
            "lxc-start --version",
        )
        .await?;
        Ok(out.trim().to_string())
    }

    /// Get container status
    pub async fn get_container_status(&self, name: &str) -> Result<ContainerStatus> {
        if !self.dir(name).join("config").exists() {
            return Err(Error::ContainerNotFound(name.to_string()));
        }
        let out = run(self.lxc("info", name).arg("-s"), "lxc-info")
            .await
            .map_err(|_| Error::ContainerNotFound(name.to_string()))?;
        Ok(parse_state(&out))
    }

    /// Pause/freeze a container
    pub async fn pause_container(&self, container: &Container) -> Result<()> {
        run(&mut self.lxc("freeze", &container.name), "lxc-freeze").await?;
        Ok(())
    }

    /// Resume/unfreeze a container
    pub async fn resume_container(&self, container: &Container) -> Result<()> {
        run(&mut self.lxc("unfreeze", &container.name), "lxc-unfreeze").await?;
        Ok(())
    }

    /// Get container information
    pub async fn get_container_info(&self, name: &str) -> Result<ContainerInfo> {
        let out = run(&mut self.lxc("info", name), "lxc-info")
            .await
            .map_err(|_| Error::ContainerNotFound(name.to_string()))?;
        Ok(parse_container_info(&out))
    }

    /// Execute command in container
    pub async fn exec_command(&self, name: &str, command: &[String]) -> Result<String> {
        debug!("Executing command in container {}: {:?}", name, command);
        if command.is_empty() {
            return Err(Error::Validation("command is empty".into()));
        }
        let mut cmd = self.lxc("attach", name);
        cmd.arg("--clear-env")
            .arg("--set-var")
            .arg("PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin")
            .arg("--")
            .args(command);
        run(&mut cmd, "lxc-attach").await
    }

    /// The `lxc-attach` argv for an interactive shell (used by the web
    /// terminal, which runs it on a PTY).
    pub fn attach_argv(&self, name: &str, command: &[String]) -> Vec<String> {
        let mut v = vec![
            "lxc-attach".to_string(),
            "-P".into(),
            self.lxc_path.to_string_lossy().to_string(),
            "-n".into(),
            name.into(),
            "--clear-env".into(),
            "--set-var".into(),
            "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".into(),
            "--set-var".into(),
            "TERM=xterm-256color".into(),
            "--set-var".into(),
            "HOME=/root".into(),
            "--".into(),
        ];
        if command.is_empty() {
            v.extend(["/bin/sh".into(), "-l".into()]);
        } else {
            v.extend(command.iter().cloned());
        }
        v
    }

    /// LXC name for a Horcrux container id (they differ for clones made
    /// with a different target id); falls back to the id itself.
    pub fn resolve_name(&self, id: &str) -> String {
        if valid_lxc_name(id) && self.dir(id).join("config").exists() {
            if let Ok(m) = self.read_meta(id) {
                if m.id == id {
                    return id.to_string();
                }
            } else {
                return id.to_string();
            }
        }
        if let Ok(rd) = std::fs::read_dir(&self.lxc_path) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if let Ok(m) = self.read_meta(&name) {
                    if m.id == id {
                        return name;
                    }
                }
            }
        }
        id.to_string()
    }

    /// The container's console log (`lxc.console.logfile`).
    pub fn console_log_path(&self, name: &str) -> PathBuf {
        self.dir(name).join("console.log")
    }

    /// All containers in lxcpath: (name, state), via one `lxc-ls` call.
    pub async fn list_states(&self) -> Result<Vec<(String, ContainerStatus)>> {
        if !self.lxc_path.is_dir() || !Self::check_lxc_available() {
            return Ok(Vec::new());
        }
        let out = run(
            Command::new("lxc-ls")
                .arg("-P")
                .arg(&self.lxc_path)
                .arg("-f")
                .arg("-F")
                .arg("NAME,STATE"),
            "lxc-ls",
        )
        .await?;
        Ok(out
            .lines()
            .skip(1)
            .filter_map(|l| {
                let mut w = l.split_whitespace();
                let name = w.next()?.to_string();
                Some((name, parse_state(w.next().unwrap_or(""))))
            })
            .collect())
    }

    /// Every container on the host, as Horcrux containers. Ones Horcrux
    /// created carry their settings; others are adopted by name.
    pub async fn discover(&self) -> Result<Vec<Container>> {
        let states = self.list_states().await?;
        Ok(states
            .into_iter()
            .map(|(name, status)| match self.read_meta(&name) {
                Ok(meta) => meta.to_container(status),
                Err(_) => Container {
                    id: name.clone(),
                    name: name.clone(),
                    runtime: ContainerRuntime::Lxc,
                    memory: 0,
                    cpus: 0,
                    rootfs: self.dir(&name).join("rootfs").to_string_lossy().to_string(),
                    status,
                    options: ContainerOptions::default(),
                    created_at: None,
                    lxc: None,
                },
            })
            .collect())
    }

    pub fn config_view(&self, meta: &LxcMeta) -> LxcConfigView {
        LxcConfigView {
            id: meta.id.clone(),
            name: meta.name.clone(),
            memory: meta.memory,
            cpus: meta.cpus,
            template: meta.template.clone(),
            distro: meta.distro.clone(),
            arch: meta.arch.clone(),
            rootfs_kind: meta.rootfs_kind.clone(),
            lxc: meta.lxc.clone(),
            pending: meta.pending.clone(),
            raw_config: std::fs::read_to_string(self.dir(&meta.name).join("config"))
                .unwrap_or_default(),
        }
    }

    /// Change settings (`pct set`). Resource limits are applied to a running
    /// container at once; everything else is written to the config and
    /// listed as pending until the next start/reboot.
    pub async fn update_config(
        &self,
        name: &str,
        memory: Option<u64>,
        cpus: Option<u32>,
        new: LxcOptions,
    ) -> Result<LxcMeta> {
        let mut meta = self.read_meta(name)?;
        let mut new = new;
        new.validate().map_err(Error::Validation)?;
        let old = meta.lxc.clone();
        if new.unprivileged != old.unprivileged
            || (new.unprivileged
                && (new.idmap_base() != old.idmap_base() || new.idmap_size() != old.idmap_size()))
        {
            return Err(Error::Validation(
                "unprivileged/idmap can't be changed after creation (the rootfs ownership \
                 depends on it); clone or recreate the container instead"
                    .into(),
            ));
        }
        if new.disk_gb != old.disk_gb {
            return Err(Error::Validation(
                "use POST /api/containers/<id>/resize to change the disk size".into(),
            ));
        }
        if new.arch.is_some() && new.arch.as_deref() != Some(meta.arch.as_str()) {
            return Err(Error::Validation(
                "the architecture of an existing container can't be changed".into(),
            ));
        }
        new.arch = old.arch.clone();
        for (i, n) in new.nets.iter_mut().enumerate() {
            if n.hwaddr.is_none() {
                n.hwaddr = old
                    .nets
                    .get(i)
                    .and_then(|o| o.hwaddr.clone())
                    .or_else(|| Some(lxc_config::random_mac()));
            }
        }
        lxc_config::assign_volume_names(&mut new.mounts);

        let running = self.get_container_status(name).await? != ContainerStatus::Stopped;
        let memory = memory.unwrap_or(meta.memory);
        let cpus = cpus.unwrap_or(meta.cpus);
        if running {
            if memory > 0 {
                self.set_cgroup(name, "memory.max", &(memory * 1024 * 1024).to_string())
                    .await?;
                self.set_cgroup(
                    name,
                    "memory.swap.max",
                    &(new.swap_mb * 1024 * 1024).to_string(),
                )
                .await?;
            }
            let cpu =
                lxc_config::cpu_max(cpus, new.cpu_limit).unwrap_or_else(|| "max 100000".into());
            self.set_cgroup(name, "cpu.max", &cpu).await?;
            self.set_cgroup(
                name,
                "cpu.weight",
                &new.cpu_units.unwrap_or(100).to_string(),
            )
            .await?;
            let mut pending: Vec<String> = meta.pending.clone();
            let mut mark = |what: &str, changed: bool| {
                if changed && !pending.iter().any(|p| p == what) {
                    pending.push(what.to_string());
                }
            };
            mark("network", new.nets != old.nets);
            mark("mounts", new.mounts != old.mounts);
            mark("features", new.features != old.features);
            mark("hostname", new.hostname != old.hostname);
            mark(
                "dns",
                new.nameserver != old.nameserver || new.searchdomain != old.searchdomain,
            );
            mark(
                "ssh_public_keys",
                new.ssh_public_keys != old.ssh_public_keys,
            );
            meta.pending = pending;
        }
        meta.memory = memory;
        meta.cpus = cpus;
        meta.lxc = new;
        self.write_all(&meta)?;
        Ok(meta)
    }

    async fn set_cgroup(&self, name: &str, key: &str, value: &str) -> Result<()> {
        run(
            self.lxc("cgroup", name).arg(key).arg(value),
            &format!("lxc-cgroup {key}"),
        )
        .await
        .map(|_| ())
    }

    /// Grow the root disk (`pct resize`). Works online: the image is grown,
    /// the loop device re-read and ext4 resized through the running
    /// container's mounted root.
    pub async fn resize(&self, name: &str, disk_gb: u64) -> Result<LxcMeta> {
        let mut meta = self.read_meta(name)?;
        let current = match (meta.kind(), meta.lxc.disk_gb) {
            (RootfsKind::Loop, Some(g)) => g,
            _ => {
                return Err(Error::Validation(format!(
                    "container {name} has a directory rootfs with no size limit; only \
                     containers created with a disk size can be resized"
                )))
            }
        };
        if disk_gb <= current {
            return Err(Error::Validation(format!(
                "disks can only grow: {name} is {current} GB, asked for {disk_gb} GB"
            )));
        }
        if disk_gb > 1 << 20 {
            return Err(Error::Validation("disk size too large".into()));
        }
        let img = self.dir(name).join("rootfs.raw");
        let bytes = disk_gb * 1024 * 1024 * 1024;
        let status = self.get_container_status(name).await?;
        std::fs::OpenOptions::new()
            .write(true)
            .open(&img)?
            .set_len(bytes)?;
        if status == ContainerStatus::Stopped {
            // e2fsck exits 1 when it fixed something: still fine to resize.
            let fsck = Command::new("e2fsck")
                .arg("-f")
                .arg("-p")
                .arg(&img)
                .stdin(Stdio::null())
                .output()
                .await
                .map_err(|e| sys(format!("failed to run e2fsck: {e}")))?;
            if !matches!(fsck.status.code(), Some(0) | Some(1)) {
                return Err(sys(format!(
                    "e2fsck of {} failed: {}",
                    img.display(),
                    String::from_utf8_lossy(&fsck.stdout).trim()
                )));
            }
            run(Command::new("resize2fs").arg(&img), "resize2fs").await?;
        } else {
            let losetup = run(Command::new("losetup").arg("-j").arg(&img), "losetup -j").await?;
            let dev = losetup
                .lines()
                .next()
                .and_then(|l| l.split(':').next())
                .filter(|d| !d.is_empty())
                .ok_or_else(|| sys(format!("no loop device found for {}", img.display())))?
                .to_string();
            run(Command::new("losetup").arg("-c").arg(&dev), "losetup -c").await?;
            let info = self.get_container_info(name).await?;
            let pid = info
                .pid
                .ok_or_else(|| sys(format!("no init pid for {name}")))?;
            online_grow_ext4(&format!("/proc/{pid}/root"), bytes)?;
        }
        meta.lxc.disk_gb = Some(disk_gb);
        self.write_all(&meta)?;
        info!("Resized {name} root disk to {disk_gb} GB");
        Ok(meta)
    }

    fn snaps_dir(&self, name: &str) -> PathBuf {
        self.dir(name).join(SNAPS)
    }

    /// Copy the rootfs (and managed volumes) from `src_dir` to `dst_dir`.
    async fn copy_disks(&self, meta: &LxcMeta, src_dir: &Path, dst_dir: &Path) -> Result<()> {
        match meta.kind() {
            RootfsKind::Dir => {
                run(
                    Command::new("cp")
                        .arg("-a")
                        .arg("--reflink=auto")
                        .arg(src_dir.join("rootfs"))
                        .arg(dst_dir.join("rootfs")),
                    "copying rootfs",
                )
                .await?;
            }
            RootfsKind::Loop => {
                run(
                    Command::new("cp")
                        .arg("--sparse=always")
                        .arg("--reflink=auto")
                        .arg("-p")
                        .arg(src_dir.join("rootfs.raw"))
                        .arg(dst_dir.join("rootfs.raw")),
                    "copying rootfs image",
                )
                .await?;
            }
        }
        if src_dir.join("volumes").is_dir() {
            run(
                Command::new("cp")
                    .arg("-a")
                    .arg("--reflink=auto")
                    .arg(src_dir.join("volumes"))
                    .arg(dst_dir.join("volumes")),
                "copying volumes",
            )
            .await?;
        }
        Ok(())
    }

    /// Run `f` with the container frozen if it is running (a consistent
    /// copy of a live container, like vzdump's suspend mode).
    async fn frozen<T>(
        &self,
        name: &str,
        f: impl std::future::Future<Output = Result<T>>,
    ) -> Result<T> {
        let running = self.get_container_status(name).await? == ContainerStatus::Running;
        if running {
            run(&mut self.lxc("freeze", name), "lxc-freeze").await?;
            // Flush the container's filesystem (for an image rootfs this
            // pushes ext4's dirty blocks through the loop device into the
            // image file we are about to copy).
            if let Ok(Some(pid)) = self.get_container_info(name).await.map(|i| i.pid) {
                if let Ok(f) = std::fs::File::open(format!("/proc/{pid}/root")) {
                    use std::os::fd::AsRawFd;
                    // SAFETY: valid fd for the duration of the call.
                    unsafe { nix::libc::syncfs(f.as_raw_fd()) };
                }
            }
        }
        let r = f.await;
        if running {
            if let Err(e) = run(&mut self.lxc("unfreeze", name), "lxc-unfreeze").await {
                warn!("{e}");
            }
        }
        r
    }

    pub async fn list_snapshots(&self, name: &str) -> Result<Vec<LxcSnapshot>> {
        self.read_meta(name)?;
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(self.snaps_dir(name)) {
            for e in rd.flatten() {
                if let Ok(t) = std::fs::read_to_string(e.path().join("snapshot.json")) {
                    if let Ok(s) = serde_json::from_str::<LxcSnapshot>(&t) {
                        out.push(s);
                    }
                }
            }
        }
        out.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(out)
    }

    /// Snapshot (`pct snapshot`): a full copy of the rootfs, managed volumes
    /// and settings. A running container is frozen while it's copied.
    pub async fn create_snapshot(
        &self,
        name: &str,
        snap: &str,
        comment: Option<String>,
    ) -> Result<LxcSnapshot> {
        if !valid_snapshot_name(snap) {
            return Err(Error::Validation(format!(
                "invalid snapshot name '{snap}': start with a letter, then letters, digits, '-' or '_'"
            )));
        }
        let meta = self.read_meta(name)?;
        let sdir = self.snaps_dir(name).join(snap);
        if sdir.exists() {
            return Err(Error::AlreadyExists(format!(
                "snapshot {snap} of {name} already exists"
            )));
        }
        std::fs::create_dir_all(&sdir)?;
        let src = self.dir(name);
        let res = self.frozen(name, self.copy_disks(&meta, &src, &sdir)).await;
        if let Err(e) = res {
            let _ = std::fs::remove_dir_all(&sdir);
            return Err(e);
        }
        std::fs::write(
            sdir.join(META),
            serde_json::to_vec_pretty(&meta).map_err(|e| Error::Internal(e.to_string()))?,
        )?;
        let s = LxcSnapshot {
            name: snap.to_string(),
            comment: comment.filter(|c| !c.trim().is_empty()),
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        std::fs::write(
            sdir.join("snapshot.json"),
            serde_json::to_vec_pretty(&s).map_err(|e| Error::Internal(e.to_string()))?,
        )?;
        info!("Snapshot {snap} of {name} created");
        Ok(s)
    }

    /// Roll back (`pct rollback`): the container is stopped if running,
    /// rootfs/volumes/settings are restored, and it is started again if it
    /// was running.
    pub async fn rollback_snapshot(&self, container: &Container, snap: &str) -> Result<()> {
        let name = &container.name;
        if !valid_snapshot_name(snap) {
            return Err(Error::Validation(format!("invalid snapshot name '{snap}'")));
        }
        let sdir = self.snaps_dir(name).join(snap);
        let snap_meta: LxcMeta = serde_json::from_str(
            &std::fs::read_to_string(sdir.join(META))
                .map_err(|_| Error::NotFound(format!("snapshot {snap} of {name} not found")))?,
        )
        .map_err(|e| sys(format!("snapshot metadata: {e}")))?;
        let was_running = self.get_container_status(name).await? != ContainerStatus::Stopped;
        if was_running {
            run(self.lxc("stop", name).arg("-k"), "lxc-stop").await?;
        }
        let dir = self.dir(name);
        let old = dir.join(".rollback-old");
        if old.exists() {
            tokio::fs::remove_dir_all(&old).await?;
        }
        std::fs::create_dir(&old)?;
        for item in ["rootfs", "rootfs.raw", "volumes"] {
            if dir.join(item).exists() {
                std::fs::rename(dir.join(item), old.join(item))?;
            }
        }
        if let Err(e) = self.copy_disks(&snap_meta, &sdir, &dir).await {
            // Put the pre-rollback state back.
            for item in ["rootfs", "rootfs.raw", "volumes"] {
                let _ = std::fs::remove_dir_all(dir.join(item));
                let _ = std::fs::remove_file(dir.join(item));
                if old.join(item).exists() {
                    let _ = std::fs::rename(old.join(item), dir.join(item));
                }
            }
            return Err(e);
        }
        tokio::fs::remove_dir_all(&old).await?;
        let mut meta = snap_meta;
        meta.pending.clear();
        self.write_all(&meta)?;
        info!("Container {name} rolled back to {snap}");
        if was_running {
            self.start_container(container).await?;
        }
        Ok(())
    }

    pub async fn delete_snapshot(&self, name: &str, snap: &str) -> Result<()> {
        if !valid_snapshot_name(snap) {
            return Err(Error::Validation(format!("invalid snapshot name '{snap}'")));
        }
        let sdir = self.snaps_dir(name).join(snap);
        if !sdir.join("snapshot.json").exists() {
            return Err(Error::NotFound(format!(
                "snapshot {snap} of {name} not found"
            )));
        }
        tokio::fs::remove_dir_all(&sdir).await?;
        Ok(())
    }

    /// Full clone (`pct clone --full`): copies rootfs and volumes (the
    /// source is frozen while copied if running), new MACs, new name.
    pub async fn clone_container(
        &self,
        source: &str,
        target_id: &str,
        target: &str,
        snapshot: bool,
    ) -> Result<Container> {
        info!("Cloning LXC container: {source} -> {target}");
        if snapshot {
            return Err(Error::Validation(
                "linked (snapshot) clones need copy-on-write storage; LXC containers are \
                 cloned as full copies (snapshot: false)"
                    .into(),
            ));
        }
        if !valid_lxc_name(target) {
            return Err(Error::Validation(format!(
                "invalid LXC container name '{target}'"
            )));
        }
        let src_meta = self.read_meta(source)?;
        let dst = self.dir(target);
        if dst.exists() {
            return Err(Error::AlreadyExists(format!(
                "an LXC container named {target} already exists"
            )));
        }
        std::fs::create_dir(&dst)?;
        std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(0o755))?;
        let src = self.dir(source);
        if let Err(e) = self
            .frozen(source, self.copy_disks(&src_meta, &src, &dst))
            .await
        {
            let _ = std::fs::remove_dir_all(&dst);
            return Err(e);
        }
        let mut meta = src_meta;
        meta.id = target_id.to_string();
        meta.name = target.to_string();
        if meta.lxc.hostname.as_deref() == Some(source) {
            meta.lxc.hostname = None;
        }
        for n in meta.lxc.nets.iter_mut() {
            n.hwaddr = Some(lxc_config::random_mac());
        }
        meta.created_at = chrono::Utc::now().to_rfc3339();
        meta.pending.clear();
        if let Err(e) = self.write_all(&meta) {
            let _ = std::fs::remove_dir_all(&dst);
            return Err(e);
        }
        info!("Container cloned: {source} -> {target}");
        Ok(meta.to_container(ContainerStatus::Stopped))
    }

    /// Host capabilities for the UI.
    pub async fn host_info(&self) -> LxcHostInfo {
        let version = Self::get_lxc_version().await.ok();
        let mut bridges = Vec::new();
        if let Ok(rd) = std::fs::read_dir("/sys/class/net") {
            for e in rd.flatten() {
                let p = e.path();
                if p.join("bridge").is_dir() {
                    let vlan_aware = std::fs::read_to_string(p.join("bridge/vlan_filtering"))
                        .map(|s| s.trim() == "1")
                        .unwrap_or(false);
                    bridges.push(LxcBridge {
                        name: e.file_name().to_string_lossy().to_string(),
                        vlan_aware,
                    });
                }
            }
        }
        bridges.sort_by(|a, b| a.name.cmp(&b.name));
        LxcHostInfo {
            available: version.is_some(),
            version,
            arch: lxc_config::host_arch().to_string(),
            lxc_path: self.lxc_path.to_string_lossy().to_string(),
            template_dir: self.templates.dir().to_string_lossy().to_string(),
            apparmor: apparmor_usable(),
            bridges,
        }
    }
}

// EXT4_IOC_RESIZE_FS = _IOW('f', 16, __u64)
nix::ioctl_write_ptr!(ext4_resize_fs, b'f', 16, u64);

/// EXT4_IOC_RESIZE_FS on the filesystem mounted at `mountpoint`: the
/// kernel's online grow (what resize2fs does for a mounted ext4).
fn online_grow_ext4(mountpoint: &str, bytes: u64) -> Result<()> {
    use std::os::fd::AsRawFd;
    let st = nix::sys::statvfs::statvfs(mountpoint)
        .map_err(|e| sys(format!("statvfs {mountpoint}: {e}")))?;
    let bsize = st.fragment_size() as u64;
    if bsize == 0 {
        return Err(sys("filesystem reports block size 0"));
    }
    let blocks: u64 = bytes / bsize;
    let f = std::fs::File::open(mountpoint).map_err(|e| sys(format!("open {mountpoint}: {e}")))?;
    // SAFETY: valid fd for the duration of the call; the kernel reads one u64.
    unsafe { ext4_resize_fs(f.as_raw_fd(), &blocks) }
        .map_err(|e| sys(format!("online ext4 resize: {e}")))?;
    Ok(())
}

fn parse_state(s: &str) -> ContainerStatus {
    if s.contains("RUNNING") {
        ContainerStatus::Running
    } else if s.contains("STOPPED") {
        ContainerStatus::Stopped
    } else if s.contains("FROZEN") {
        ContainerStatus::Paused
    } else {
        ContainerStatus::Unknown
    }
}

/// Parse lxc-info output
pub fn parse_container_info(output: &str) -> ContainerInfo {
    let mut info = ContainerInfo::default();
    for line in output.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        match key {
            "Name" => info.name = value.to_string(),
            "State" => info.state = parse_state(value),
            "PID" => info.pid = value.parse().ok(),
            "IP" => info.ip_addresses.push(value.to_string()),
            "CPU use" => {
                info.cpu_usage = value.split_whitespace().next().and_then(|s| s.parse().ok())
            }
            "Memory use" => info.memory_usage = parse_memory(value),
            _ => {}
        }
    }
    info
}

/// Parse memory string (e.g., "512.00 MiB" -> bytes)
pub fn parse_memory(mem_str: &str) -> Option<u64> {
    let parts: Vec<&str> = mem_str.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }
    let value: f64 = parts[0].parse().ok()?;
    let mult = match parts[1] {
        "KiB" => 1024.0,
        "MiB" => 1024.0 * 1024.0,
        "GiB" => 1024.0 * 1024.0 * 1024.0,
        "B" | "bytes" => 1.0,
        _ => return None,
    };
    Some((value * mult) as u64)
}

/// Container information
#[derive(Debug, Clone, Default)]
pub struct ContainerInfo {
    pub name: String,
    pub state: ContainerStatus,
    pub pid: Option<u32>,
    pub ip_addresses: Vec<String>,
    pub cpu_usage: Option<f64>,
    pub memory_usage: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_memory() {
        assert_eq!(parse_memory("512.00 MiB"), Some(536870912));
        assert_eq!(parse_memory("1.50 GiB"), Some(1610612736));
        assert_eq!(parse_memory("100 KiB"), Some(102400));
        assert_eq!(parse_memory("invalid"), None);
    }

    #[test]
    fn parses_lxc_info() {
        let i = parse_container_info(
            "Name:           t1\nState:          RUNNING\nPID:            1614\n\
             IP:             10.77.0.50\nIP:             10.20.0.50\nMemory use:     1.50 MiB\n",
        );
        assert_eq!(i.name, "t1");
        assert_eq!(i.state, ContainerStatus::Running);
        assert_eq!(i.pid, Some(1614));
        assert_eq!(i.ip_addresses, vec!["10.77.0.50", "10.20.0.50"]);
        assert_eq!(i.memory_usage, Some(1572864));
        assert_eq!(parse_state("State: FROZEN"), ContainerStatus::Paused);
    }

    #[test]
    fn names() {
        assert!(valid_lxc_name("ct-1.web_a"));
        assert!(!valid_lxc_name("-x"));
        assert!(!valid_lxc_name("a/b"));
        assert!(!valid_lxc_name(".."));
        assert!(!valid_lxc_name("a b"));
        assert!(valid_snapshot_name("before-upgrade_1"));
        assert!(!valid_snapshot_name("1st"));
        assert!(!valid_snapshot_name("a/b"));
        assert!(!valid_snapshot_name(""));
        assert_eq!(lxc_distro_name("rocky"), "centos");
        assert_eq!(lxc_distro_name("alpine"), "alpine");
        assert_eq!(lxc_distro_name("arch"), "archlinux");
    }

    #[test]
    fn subid_ranges() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("subuid");
        // Missing file: entry added.
        assert!(ensure_subid_range(&f, 100000, 65536).unwrap());
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "root:100000:65536\n");
        // Covered: untouched.
        assert!(!ensure_subid_range(&f, 100000, 65536).unwrap());
        assert!(!ensure_subid_range(&f, 110000, 1000).unwrap());
        // Another user's range doesn't count, partial overlap isn't enough.
        std::fs::write(&f, "alice:200000:65536\nroot:100000:65536").unwrap();
        assert!(ensure_subid_range(&f, 200000, 65536).unwrap());
        assert_eq!(
            std::fs::read_to_string(&f).unwrap(),
            "alice:200000:65536\nroot:100000:65536\nroot:200000:65536\n"
        );
        assert!(ensure_subid_range(&f, 150000, 65536).unwrap());
    }

    #[test]
    fn safe_write_never_follows_guest_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("rootfs");
        let outside = tmp.path().join("host-secret");
        std::fs::create_dir_all(root.join("etc")).unwrap();
        std::fs::write(&outside, "host data").unwrap();
        let me = {
            use std::os::unix::fs::MetadataExt;
            let m = std::fs::metadata(tmp.path()).unwrap();
            (m.uid(), m.gid())
        };

        // A guest symlink at the final component is replaced, not followed.
        std::os::unix::fs::symlink(&outside, root.join("etc/hostname")).unwrap();
        safe_write(&root, "etc/hostname", b"ct\n", 0o644, me).unwrap();
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "host data");
        assert_eq!(
            std::fs::read_to_string(root.join("etc/hostname")).unwrap(),
            "ct\n"
        );
        assert!(!std::fs::symlink_metadata(root.join("etc/hostname"))
            .unwrap()
            .file_type()
            .is_symlink());

        // A symlinked parent directory is refused.
        std::fs::create_dir_all(tmp.path().join("hostdir")).unwrap();
        std::os::unix::fs::symlink(tmp.path().join("hostdir"), root.join("root")).unwrap();
        assert!(safe_write(&root, "root/.ssh/authorized_keys", b"k", 0o600, me).is_err());
        assert!(!tmp.path().join("hostdir/.ssh").exists());

        // Missing parents are created.
        safe_write(&root, "etc/systemd/network/eth0.network", b"x", 0o644, me).unwrap();
        assert!(root.join("etc/systemd/network/eth0.network").is_file());
        // Reading through a symlink is refused too.
        assert!(read_inside(&root, "root/x").is_none());
    }

    #[test]
    fn guest_setup_writes_network_hostname_dns_and_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("etc/network")).unwrap();
        std::fs::write(root.join("etc/network/interfaces"), "auto eth0\n").unwrap();
        std::fs::write(root.join("etc/hosts"), "127.0.0.1 localhost\n").unwrap();
        std::fs::write(root.join("etc/os-release"), "NAME=\"Alpine\"\nID=alpine\n").unwrap();
        // interfaces without ifup installed is not ifupdown.
        assert_eq!(LxcManager::detect_guest_net(root), GuestNet::Unmanaged);
        std::fs::create_dir_all(root.join("sbin")).unwrap();
        std::fs::write(root.join("sbin/ifupdown"), "").unwrap();
        // Alpine: /sbin/ifup -> ifupdown (a symlink) still counts.
        std::os::unix::fs::symlink("ifupdown", root.join("sbin/ifup")).unwrap();
        assert_eq!(LxcManager::detect_guest_net(root), GuestNet::Ifupdown);
        assert_eq!(os_release_id(root).as_deref(), Some("alpine"));
        let meta = LxcMeta {
            id: "ct".into(),
            name: "ct".into(),
            memory: 64,
            cpus: 1,
            template: "alpine/3.22".into(),
            distro: "alpine".into(),
            arch: "amd64".into(),
            rootfs_kind: "dir".into(),
            guest_net: "ifupdown".into(),
            lxc: LxcOptions {
                hostname: Some("web".into()),
                nets: vec![horcrux_common::LxcNet {
                    bridge: "br0".into(),
                    ip: Some("10.1.0.2/24".into()),
                    gateway: Some("10.1.0.1".into()),
                    ..Default::default()
                }],
                nameserver: Some("10.1.0.1".into()),
                ssh_public_keys: vec!["ssh-ed25519 AAAA k".into()],
                ..Default::default()
            },
            created_at: "now".into(),
            pending: vec![],
        };
        let me = {
            use std::os::unix::fs::MetadataExt;
            let m = std::fs::metadata(tmp.path()).unwrap();
            (m.uid(), m.gid())
        };
        LxcManager::setup_guest(&meta, root, me).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("etc/hostname")).unwrap(),
            "web\n"
        );
        assert!(std::fs::read_to_string(root.join("etc/hosts"))
            .unwrap()
            .contains("127.0.1.1 web\n"));
        assert!(std::fs::read_to_string(root.join("etc/network/interfaces"))
            .unwrap()
            .contains("iface eth0 inet static\n\taddress 10.1.0.2/24\n\tgateway 10.1.0.1\n"));
        assert!(std::fs::read_to_string(root.join("etc/resolv.conf"))
            .unwrap()
            .contains("nameserver 10.1.0.1\n"));
        assert_eq!(
            std::fs::read_to_string(root.join("root/.ssh/authorized_keys")).unwrap(),
            "ssh-ed25519 AAAA k\n"
        );
        // A DHCP client must not replace the configured DNS...
        std::fs::write(root.join("etc/dhcpcd.conf"), "hostname\n").unwrap();
        LxcManager::setup_guest(&meta, root, me).unwrap();
        let dc = std::fs::read_to_string(root.join("etc/dhcpcd.conf")).unwrap();
        assert!(
            dc.starts_with("hostname\n") && dc.ends_with("nohook resolv.conf\n"),
            "{dc}"
        );
        LxcManager::setup_guest(&meta, root, me).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("etc/dhcpcd.conf")).unwrap(),
            dc
        );
        // ...until the DNS setting is cleared.
        let mut no_dns = meta.clone();
        no_dns.lxc.nameserver = None;
        LxcManager::setup_guest(&no_dns, root, me).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("etc/dhcpcd.conf")).unwrap(),
            "hostname\n"
        );
        // Opt-out marker keeps a file untouched.
        std::fs::write(root.join("etc/.horcrux-ignore.hostname"), "").unwrap();
        std::fs::write(root.join("etc/hostname"), "custom\n").unwrap();
        LxcManager::setup_guest(&meta, root, me).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("etc/hostname")).unwrap(),
            "custom\n"
        );
    }
}
