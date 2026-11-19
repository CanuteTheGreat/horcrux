//! Managed PCI hand-off for passthrough VMs (docs/design/gaming-vm.md).
//!
//! A `hostpci` entry with `managed: true` names a device the host normally
//! uses (typically a discrete GPU). When the VM starts, Horcrux:
//!
//! 1. collects the device and the other non-bridge functions of its IOMMU
//!    group (a GPU's HDMI audio, USB-C controller, ...), because VFIO can
//!    only hand a whole group to a guest;
//! 2. refuses when a monitor on this GPU is the host's only display (unless
//!    `allow_display_loss`: single-GPU passthrough), or when a host program
//!    still has the device open (unbinding a driver under an open file hangs
//!    or crashes that program) - the error names the programs;
//! 3. stops the configured helper services (`passthrough.handoff_stop_services`,
//!    e.g. nvidia-persistenced) through the host's init system;
//! 4. moves every function to `vfio-pci` (driver_override), with a timeout;
//! 5. records what it did in `<disk>.handoff.json`.
//!
//! When the VM is gone (stop, shutdown, crash), [`release`] waits until no
//! process holds the group's `/dev/vfio/N` any more, gives every function back
//! to its host driver and starts the stopped services again. The record makes
//! that work after an API restart too.
//!
//! [`host_report`] answers "can this host run a gaming VM, and with which
//! GPU?" for the UI wizard and `GET /api/gpu/pci/gaming-check`.
use super::pci::PciManager;
use super::tuning::{self, HostCpus};
use horcrux_common::{format_cpu_list, Error, HostPciKind, Result, VmHostPci};
use horcrux_core::config::PassthroughConfig;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;
use tracing::{info, warn};

/// Host services (init-system neutral; horcrux-api registers the real
/// OpenRC/systemd backend at startup).
#[async_trait::async_trait]
pub trait HostServices: Send + Sync {
    async fn is_active(&self, name: &str) -> bool;
    async fn stop(&self, name: &str) -> std::result::Result<(), String>;
    async fn start(&self, name: &str) -> std::result::Result<(), String>;
}

static SERVICES: OnceLock<Box<dyn HostServices>> = OnceLock::new();

/// Register the host's init system (once, at API startup).
pub fn set_host_services(s: Box<dyn HostServices>) {
    let _ = SERVICES.set(s);
}

fn services() -> Option<&'static dyn HostServices> {
    SERVICES.get().map(|b| b.as_ref())
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn base(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn link_base(p: &Path) -> Option<String> {
    std::fs::read_link(p).ok().map(|l| base(&l))
}

/// A process with one of the device's files open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Holder {
    pub pid: u32,
    pub command: String,
    pub path: String,
}

/// What a hand-off moved (`<disk>.handoff.json`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffRecord {
    /// (address, host driver before the hand-off)
    pub devices: Vec<(String, Option<String>)>,
    /// IOMMU groups the VM holds (`/dev/vfio/N`).
    pub groups: Vec<String>,
    pub stopped_services: Vec<String>,
    /// Single-GPU passthrough: display services stopped for the VM.
    #[serde(default)]
    pub display_services: Vec<String>,
    /// Single-GPU passthrough: vtconsoles detached (`vtcon1`).
    #[serde(default)]
    pub vtconsoles: Vec<String>,
    /// Single-GPU passthrough: (platform driver, device) of detached boot
    /// framebuffers (`efi-framebuffer`, `efi-framebuffer.0`).
    #[serde(default)]
    pub framebuffers: Vec<(String, String)>,
}

pub fn record_file(disk: &Path) -> PathBuf {
    disk.with_extension("handoff.json")
}

/// Sysfs/procfs/devfs roots (fixtures in tests).
#[derive(Debug, Clone)]
pub struct Host {
    pub sys: PathBuf,
    pub proc_: PathBuf,
    pub dev: PathBuf,
}

impl Default for Host {
    fn default() -> Self {
        Self {
            sys: "/sys".into(),
            proc_: "/proc".into(),
            dev: "/dev".into(),
        }
    }
}

impl Host {
    fn pci(&self) -> PciManager {
        PciManager::with_sysfs(&self.sys)
    }

    fn devdir(&self, addr: &str) -> PathBuf {
        self.sys.join("bus/pci/devices").join(addr)
    }

    /// The device plus the non-bridge members of its IOMMU group.
    pub fn group_functions(&self, addr: &str) -> Result<(String, Vec<String>)> {
        let d = self.devdir(addr);
        if !d.exists() {
            return Err(Error::Validation(format!(
                "PCI device {addr} does not exist on this host"
            )));
        }
        let Some(group) = link_base(&d.join("iommu_group")) else {
            return Err(Error::Validation(format!(
                "PCI device {addr} is in no IOMMU group: turn on VT-d/AMD-Vi in the firmware and the IOMMU in the kernel (intel_iommu=on / amd_iommu=on)"
            )));
        };
        let dev = self.pci().list();
        let mut f: Vec<String> = dev
            .iter()
            .filter(|p| {
                p.iommu_group.as_deref() == Some(group.as_str()) && p.class_name != "bridge"
            })
            .map(|p| p.address.clone())
            .collect();
        f.sort();
        Ok((group, f))
    }

    /// DRM connectors with a monitor attached, per PCI address.
    pub fn connected_displays(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(self.sys.join("class/drm")) else {
            return out;
        };
        let mut names: Vec<String> = rd.flatten().map(|e| base(&e.path())).collect();
        names.sort();
        for n in names {
            // connectors look like card1-HDMI-A-1
            let Some((card, _)) = n.split_once('-') else {
                continue;
            };
            if !card.starts_with("card") {
                continue;
            }
            let c = self.sys.join("class/drm").join(&n);
            if read(&c.join("status")) != "connected" {
                continue;
            }
            let dev = link_base(&self.sys.join("class/drm").join(card).join("device"));
            if let Some(addr) = dev {
                out.push((addr, n.clone()));
            }
        }
        out
    }

    /// Device files of these PCI functions (DRM nodes, NVIDIA nodes).
    pub fn device_nodes(&self, addrs: &[String]) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = Vec::new();
        for a in addrs {
            let d = self.devdir(a);
            if let Ok(rd) = std::fs::read_dir(d.join("drm")) {
                for e in rd.flatten() {
                    let n = base(&e.path());
                    if n.starts_with("card") || n.starts_with("renderD") {
                        v.push(self.dev.join("dri").join(n));
                    }
                }
            }
            if link_base(&d.join("driver")).as_deref() == Some("nvidia") {
                let info = read(
                    &self
                        .proc_
                        .join("driver/nvidia/gpus")
                        .join(a)
                        .join("information"),
                );
                if let Some(m) = info
                    .lines()
                    .find_map(|l| l.strip_prefix("Device Minor:"))
                    .and_then(|m| m.trim().parse::<u32>().ok())
                {
                    v.push(self.dev.join(format!("nvidia{m}")));
                }
                // the last GPU on the nvidia driver: its control nodes too
                let others = self
                    .pci()
                    .list()
                    .into_iter()
                    .filter(|p| {
                        p.driver.as_deref() == Some("nvidia") && !addrs.contains(&p.address)
                    })
                    .count();
                if others == 0 {
                    for n in [
                        "nvidiactl",
                        "nvidia-modeset",
                        "nvidia-uvm",
                        "nvidia-uvm-tools",
                    ] {
                        v.push(self.dev.join(n));
                    }
                }
            }
        }
        v.sort();
        v.dedup();
        v
    }

    /// Processes that have any of `paths` open.
    pub fn holders(&self, paths: &[PathBuf]) -> Vec<Holder> {
        let mut out = Vec::new();
        if paths.is_empty() {
            return out;
        }
        let me = std::process::id();
        let Ok(rd) = std::fs::read_dir(&self.proc_) else {
            return out;
        };
        let mut pids: Vec<u32> = rd
            .flatten()
            .filter_map(|e| base(&e.path()).parse::<u32>().ok())
            .collect();
        pids.sort_unstable();
        for pid in pids {
            if pid == me {
                continue;
            }
            let pdir = self.proc_.join(pid.to_string());
            let Ok(fds) = std::fs::read_dir(pdir.join("fd")) else {
                continue;
            };
            let mut hit: Vec<String> = Vec::new();
            for fd in fds.flatten() {
                if let Ok(t) = std::fs::read_link(fd.path()) {
                    if paths.contains(&t) {
                        let s = t.to_string_lossy().to_string();
                        if !hit.contains(&s) {
                            hit.push(s);
                        }
                    }
                }
            }
            for path in hit {
                out.push(Holder {
                    pid,
                    command: read(&pdir.join("comm")),
                    path,
                });
            }
        }
        out
    }
}

fn holders_message(h: &[Holder]) -> String {
    let mut names: Vec<String> = h
        .iter()
        .map(|x| format!("{} (pid {}, {})", x.command, x.pid, x.path))
        .collect();
    names.dedup();
    names.join(", ")
}

/// Run a blocking sysfs step with a timeout (a driver unbind can hang).
async fn with_timeout<T: Send + 'static>(
    secs: u64,
    what: String,
    f: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    match tokio::time::timeout(Duration::from_secs(secs.max(1)), tokio::task::spawn_blocking(f)).await
    {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => Err(Error::System(format!("{what}: {e}"))),
        Err(_) => Err(Error::System(format!(
            "{what} did not finish within {secs} s (the driver is stuck; the device may need a host power cycle)"
        ))),
    }
}

/// Managed devices of a VM's hostpci list.
pub fn managed(hostpci: &[VmHostPci]) -> Vec<(String, bool)> {
    hostpci
        .iter()
        .filter(|d| d.managed)
        .filter_map(|d| match d.kind() {
            Ok(HostPciKind::Pci(a)) => Some((a, d.allow_display_loss)),
            _ => None,
        })
        .collect()
}

/// Checks before anything changes: groups, display, holders. Returns the
/// functions to move and their groups.
pub fn preflight(
    host: &Host,
    devices: &[(String, bool)],
    cfg: &PassthroughConfig,
    ignore_holders_of_services: bool,
) -> Result<(Vec<String>, Vec<String>)> {
    let mut funcs: Vec<String> = Vec::new();
    let mut groups: Vec<String> = Vec::new();
    let displays = host.connected_displays();
    for (addr, allow_loss) in devices {
        let (g, f) = host.group_functions(addr)?;
        let here: Vec<&String> = displays
            .iter()
            .filter(|(a, _)| f.contains(a))
            .map(|(_, c)| c)
            .collect();
        let elsewhere = displays.iter().any(|(a, _)| !f.contains(a));
        if !here.is_empty() && !elsewhere && !allow_loss {
            return Err(Error::Validation(format!(
                "{addr} drives the host's only connected display ({}): handing it to the VM would leave the host without a screen. Connect the monitor to another GPU, or allow single-GPU passthrough (allow_display_loss)",
                here.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
            )));
        }
        if !groups.contains(&g) {
            groups.push(g);
        }
        for x in f {
            if !funcs.contains(&x) {
                funcs.push(x);
            }
        }
    }
    if cfg.handoff_refuse_when_held && !ignore_holders_of_services {
        let held = host.holders(&host.device_nodes(&funcs));
        if !held.is_empty() {
            return Err(Error::Conflict(format!(
                "the GPU is still in use on the host by {}. Close those programs (a desktop session keeps every GPU open unless the GPU is marked for passthrough; see the Gaming VM guide) and start the VM again",
                holders_message(&held)
            )));
        }
    }
    Ok((funcs, groups))
}

/// Does handing these devices over take the host's only screen (single-GPU
/// passthrough, allowed with `allow_display_loss`)?
pub fn takes_only_display(host: &Host, devices: &[(String, bool)]) -> bool {
    let displays = host.connected_displays();
    let mut funcs: Vec<String> = Vec::new();
    for (addr, allow) in devices {
        if !allow {
            continue;
        }
        if let Ok((_, f)) = host.group_functions(addr) {
            funcs.extend(f);
        }
    }
    let here = displays.iter().any(|(a, _)| funcs.contains(a));
    let elsewhere = displays.iter().any(|(a, _)| !funcs.contains(a));
    here && !elsewhere
}

/// Detach the kernel's framebuffer console from the GPU (single-GPU
/// hand-off): unbind every vtconsole that is a framebuffer console, then
/// the boot framebuffer devices (EFI/simple/VESA). Records what it did.
fn unbind_consoles(host: &Host, rec: &mut HandoffRecord) {
    let vt = host.sys.join("class/vtconsole");
    if let Ok(rd) = std::fs::read_dir(&vt) {
        let mut names: Vec<String> = rd.flatten().map(|e| base(&e.path())).collect();
        names.sort();
        for n in names {
            let d = vt.join(&n);
            if read(&d.join("name")).contains("frame buffer") && read(&d.join("bind")) == "1" {
                match std::fs::write(d.join("bind"), "0") {
                    Ok(()) => {
                        info!("hand-off: console {n} detached");
                        rec.vtconsoles.push(n);
                    }
                    Err(e) => warn!("hand-off: detaching console {n}: {e}"),
                }
            }
        }
    }
    for drv in ["efi-framebuffer", "simple-framebuffer", "vesa-framebuffer"] {
        let d = host.sys.join("bus/platform/drivers").join(drv);
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        let prefix = format!("{drv}.");
        let mut devs: Vec<String> = rd
            .flatten()
            .map(|e| base(&e.path()))
            .filter(|n| n.starts_with(&prefix))
            .collect();
        devs.sort();
        for dev in devs {
            match std::fs::write(d.join("unbind"), &dev) {
                Ok(()) => {
                    info!("hand-off: {dev} detached from {drv}");
                    rec.framebuffers.push((drv.to_string(), dev));
                }
                Err(e) => warn!("hand-off: detaching {dev}: {e}"),
            }
        }
    }
}

/// Undo [`unbind_consoles`].
fn rebind_consoles(host: &Host, rec: &HandoffRecord) {
    for (drv, dev) in &rec.framebuffers {
        let f = host.sys.join("bus/platform/drivers").join(drv).join("bind");
        if let Err(e) = std::fs::write(&f, dev) {
            warn!("hand-off: re-attaching {dev} to {drv}: {e}");
        }
    }
    for n in &rec.vtconsoles {
        let f = host.sys.join("class/vtconsole").join(n).join("bind");
        if let Err(e) = std::fs::write(&f, "1") {
            warn!("hand-off: re-attaching console {n}: {e}");
        }
    }
}

/// Move a VM's managed devices to vfio-pci (see the module docs).
pub async fn acquire(host: &Host, disk: &Path, hostpci: &[VmHostPci]) -> Result<()> {
    let cfg = tuning::config().clone();
    acquire_with(host, disk, hostpci, &cfg, services()).await
}

/// [`acquire`] with explicit settings and init backend (tests).
pub async fn acquire_with(
    host: &Host,
    disk: &Path,
    hostpci: &[VmHostPci],
    cfg: &PassthroughConfig,
    svc: Option<&dyn HostServices>,
) -> Result<()> {
    let devices = managed(hostpci);
    if devices.is_empty() {
        return Ok(());
    }
    // Cheap checks first (no service touched yet).
    let (funcs, groups) = preflight(host, &devices, cfg, true)?;
    let mut rec = HandoffRecord {
        groups,
        ..Default::default()
    };
    let needs_driver_move = funcs
        .iter()
        .any(|f| link_base(&host.devdir(f).join("driver")).as_deref() != Some("vfio-pci"));
    // Single-GPU passthrough: the local desktop lets go of the GPU first
    // (display manager stopped through the init system, framebuffer
    // console detached); all of it comes back when the VM stops.
    if needs_driver_move && takes_only_display(host, &devices) {
        if let Some(s) = svc {
            for name in &cfg.handoff_display_services {
                if s.is_active(name).await {
                    if let Err(e) = s.stop(name).await {
                        give_back_with(host, &rec, cfg, svc).await;
                        return Err(Error::System(format!("stopping {name}: {e}")));
                    }
                    info!("hand-off: stopped {name} (single-GPU passthrough)");
                    rec.display_services.push(name.clone());
                }
            }
        }
        // the compositor and its helpers take a moment to exit
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(cfg.handoff_display_release_secs);
        while !host.holders(&host.device_nodes(&funcs)).is_empty()
            && tokio::time::Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        if cfg.handoff_unbind_consoles {
            unbind_consoles(host, &mut rec);
        }
    }
    // Helper services that keep the GPU open.
    if needs_driver_move {
        if let Some(s) = svc {
            for name in &cfg.handoff_stop_services {
                if s.is_active(name).await {
                    if let Err(e) = s.stop(name).await {
                        give_back_with(host, &rec, cfg, svc).await;
                        return Err(Error::System(format!("stopping {name}: {e}")));
                    }
                    rec.stopped_services.push(name.clone());
                }
            }
        }
    }
    // Now the holders check proper.
    if cfg.handoff_refuse_when_held {
        let held = host.holders(&host.device_nodes(&funcs));
        if !held.is_empty() {
            give_back_with(host, &rec, cfg, svc).await;
            return Err(Error::Conflict(format!(
                "the GPU is still in use on the host by {}. Close those programs (a desktop session keeps every GPU open unless the GPU is marked for passthrough; see the Gaming VM guide) and start the VM again",
                holders_message(&held)
            )));
        }
    }
    for f in &funcs {
        let before = link_base(&host.devdir(f).join("driver"));
        let pci = host.pci();
        let a = f.clone();
        let r = with_timeout(
            cfg.handoff_timeout_secs,
            format!("moving {f} to vfio-pci"),
            move || pci.bind_vfio(&a).map(|_| ()),
        )
        .await;
        if let Err(e) = r {
            // undo what is done so far
            let partial = rec.clone();
            give_back_with(host, &partial, cfg, svc).await;
            return Err(e);
        }
        info!(
            "hand-off: {f} {} -> vfio-pci",
            before.as_deref().unwrap_or("(no driver)")
        );
        rec.devices.push((f.clone(), before));
    }
    let body = serde_json::to_vec_pretty(&rec).unwrap_or_default();
    if let Err(e) = std::fs::write(record_file(disk), body) {
        give_back_with(host, &rec, cfg, svc).await;
        return Err(Error::System(format!("recording the hand-off: {e}")));
    }
    Ok(())
}

async fn restart_services(svc: Option<&dyn HostServices>, names: &[String]) {
    if let Some(s) = svc {
        for n in names {
            if let Err(e) = s.start(n).await {
                warn!("hand-off: starting {n} again failed: {e}");
            }
        }
    }
}

async fn give_back(host: &Host, rec: &HandoffRecord, cfg: &PassthroughConfig) {
    give_back_with(host, rec, cfg, services()).await
}

async fn give_back_with(
    host: &Host,
    rec: &HandoffRecord,
    cfg: &PassthroughConfig,
    svc: Option<&dyn HostServices>,
) {
    for (addr, before) in &rec.devices {
        let pci = host.pci();
        let a = addr.clone();
        // a device that was already on vfio-pci before stays there
        if before.as_deref() == Some("vfio-pci") {
            continue;
        }
        match with_timeout(
            cfg.handoff_timeout_secs,
            format!("giving {addr} back to the host"),
            move || pci.unbind_vfio(&a).map(|d| d.driver),
        )
        .await
        {
            Ok(now) => info!(
                "hand-off: {addr} back on {}",
                now.as_deref().unwrap_or("(no driver)")
            ),
            Err(e) => warn!("hand-off: {e}"),
        }
    }
    restart_services(svc, &rec.stopped_services).await;
    // single-GPU: the console first, then the desktop
    rebind_consoles(host, rec);
    restart_services(svc, &rec.display_services).await;
}

/// Give a VM's managed devices back once QEMU no longer holds them. Waits
/// (bounded by `wait`) for the group files to be closed; a QEMU that is
/// still alive keeps the devices.
pub async fn release(host: &Host, disk: &Path, wait: Duration) {
    let path = record_file(disk);
    let Ok(raw) = std::fs::read(&path) else {
        return;
    };
    let Ok(rec) = serde_json::from_slice::<HandoffRecord>(&raw) else {
        warn!("hand-off record {} is unreadable", path.display());
        return;
    };
    let nodes: Vec<PathBuf> = rec
        .groups
        .iter()
        .map(|g| host.dev.join("vfio").join(g))
        .collect();
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let held = host.holders(&nodes);
        if held.is_empty() {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            warn!(
                "hand-off: {} still held by {}; leaving it on vfio-pci",
                rec.groups.join(","),
                holders_message(&held)
            );
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    // take the record, so two releases racing (stop + exit watcher) never
    // give the devices back twice
    let taken = path.with_extension("json.releasing");
    if std::fs::rename(&path, &taken).is_err() {
        return;
    }
    let cfg = tuning::config().clone();
    give_back(host, &rec, &cfg).await;
    let _ = std::fs::remove_file(&taken);
}

// ----------------------------------------------------------------- report

/// One known issue (from the quirks tables).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quirk {
    pub vendor: String,
    /// Inclusive device-id ranges (`["2b80", "2bff"]`).
    #[serde(default)]
    pub device_ranges: Vec<[String; 2]>,
    /// `info`, `warn` or `block`.
    #[serde(default = "default_severity")]
    pub severity: String,
    pub title: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub links: Vec<String>,
}

fn default_severity() -> String {
    "warn".into()
}

#[derive(Deserialize)]
struct QuirkFile {
    #[serde(default)]
    quirk: Vec<Quirk>,
}

/// Quirks from the configured tables (missing files are skipped).
pub fn load_quirks(files: &[PathBuf]) -> Vec<Quirk> {
    let mut v = Vec::new();
    for f in files {
        let Ok(s) = std::fs::read_to_string(f) else {
            continue;
        };
        match toml::from_str::<QuirkFile>(&s) {
            Ok(q) => v.extend(q.quirk),
            Err(e) => warn!("{}: {e}", f.display()),
        }
    }
    v
}

fn quirk_matches(q: &Quirk, vendor: &str, device: &str) -> bool {
    let h = |s: &str| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok();
    if !q.vendor.eq_ignore_ascii_case(vendor) {
        return false;
    }
    let Some(d) = h(device) else { return false };
    q.device_ranges.is_empty()
        || q.device_ranges
            .iter()
            .any(|[a, b]| matches!((h(a), h(b)), (Some(a), Some(b)) if a <= d && d <= b))
}

/// One GPU in the gaming report.
#[derive(Debug, Clone, Serialize)]
pub struct GpuCheck {
    pub address: String,
    pub vendor_id: String,
    pub device_id: String,
    pub vendor_name: String,
    pub driver: Option<String>,
    pub iommu_group: Option<String>,
    /// Functions that move with it (its IOMMU group, bridges excluded).
    pub functions: Vec<String>,
    /// Monitors connected to this GPU right now.
    pub displays: Vec<String>,
    /// Every display connector of this GPU (connected or not), from DRM.
    pub connectors: Vec<String>,
    /// Does the GPU have display outputs? `false` for "3D controllers"
    /// (datacenter/compute cards, muxless laptop GPUs) and cards whose
    /// driver lists no connectors; `None` when unknown (no DRM driver bound).
    /// Without outputs the picture can only reach the player through the
    /// Horcrux viewer with a virtual display in the guest.
    pub has_outputs: Option<bool>,
    /// Is a monitor connected to some other GPU (the host keeps a screen)?
    pub host_keeps_display: bool,
    pub reset_methods: Vec<String>,
    pub holders: Vec<Holder>,
    pub quirks: Vec<Quirk>,
    /// `ready`, `ready-with-warnings`, `single-gpu` or `blocked`.
    pub verdict: String,
    pub reasons: Vec<String>,
}

/// Host readiness for a gaming VM.
#[derive(Debug, Clone, Serialize)]
pub struct GamingReport {
    pub kvm: bool,
    pub iommu_groups: usize,
    pub vfio_pci_loaded: bool,
    pub cpus: HostCpus,
    /// Suggested vCPU count (all performance cores, or all CPUs but one).
    pub suggested_vcpus: u32,
    pub suggested_pinning: String,
    /// (page KiB, total, free)
    pub hugepages: Vec<(u64, u64, u64)>,
    pub gpus: Vec<GpuCheck>,
    /// Nested virtualisation is on in the host's KVM module (Windows VBS /
    /// Memory Integrity and Hyper-V inside the guest need it).
    pub nested_virt: bool,
    /// Why not, and how to turn it on (empty when available).
    pub nested_virt_problem: String,
    /// The Secure Boot build of OVMF (with Microsoft keys) is installed.
    pub secure_boot_firmware: bool,
    /// swtpm (the emulated TPM 2.0) is installed.
    pub swtpm: bool,
    /// Program that builds the answer-file disc (xorriso, genisoimage or
    /// mkisofs); empty = none installed.
    pub iso_tool: String,
}

/// The ISO 9660 builder on this host (xorriso, genisoimage, mkisofs).
pub fn iso_tool() -> Option<&'static str> {
    ["xorriso", "genisoimage", "mkisofs"]
        .into_iter()
        .find(|t| in_path(t))
}

fn in_path(prog: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(prog).is_file()))
        .unwrap_or(false)
}

/// DRM connectors of a PCI device (`card1-HDMI-A-1` -> `HDMI-A-1`), and
/// whether it has a DRM card at all.
fn connectors_of(host: &Host, addr: &str) -> (bool, Vec<String>) {
    let mut out = Vec::new();
    let mut has_card = false;
    let Ok(rd) = std::fs::read_dir(host.sys.join("class/drm")) else {
        return (false, out);
    };
    let mut names: Vec<String> = rd.flatten().map(|e| base(&e.path())).collect();
    names.sort();
    for n in names {
        let Some((card, conn)) = n.split_once('-') else {
            if n.starts_with("card")
                && link_base(&host.sys.join("class/drm").join(&n).join("device")).as_deref()
                    == Some(addr)
            {
                has_card = true;
            }
            continue;
        };
        if !card.starts_with("card") {
            continue;
        }
        if link_base(&host.sys.join("class/drm").join(card).join("device")).as_deref() == Some(addr)
        {
            has_card = true;
            out.push(conn.to_string());
        }
    }
    (has_card, out)
}

/// Build the report (pure over the given roots, plus the quirks tables).
pub fn host_report(host: &Host, quirks: &[Quirk]) -> GamingReport {
    let pci = host.pci();
    let devs = pci.list();
    let displays = host.connected_displays();
    let cpus = HostCpus::read(&host.sys);
    let suggested_vcpus = if cpus.hybrid() {
        cpus.performance.len() as u32
    } else if cpus.multi_l3() {
        // one CCD/CCX (the best one): no cross-die latency in the guest
        cpus.l3_order()
            .first()
            .map(|g| g.iter().filter(|c| cpus.online.contains(c)).count() as u32)
            .unwrap_or(1)
            .max(1)
    } else {
        (cpus.online.len() as u32).saturating_sub(1).max(1)
    };
    let suggested_pinning = tuning::auto_vcpu_cpus(&cpus, suggested_vcpus)
        .map(|v| format_cpu_list(&v))
        .unwrap_or_default();
    let mut gpus = Vec::new();
    for d in devs.iter().filter(|d| d.class_name == "gpu") {
        let functions = host
            .group_functions(&d.address)
            .map(|(_, f)| f)
            .unwrap_or_else(|_| vec![d.address.clone()]);
        let mine: Vec<String> = displays
            .iter()
            .filter(|(a, _)| functions.contains(a))
            .map(|(_, c)| c.clone())
            .collect();
        let host_keeps_display = displays.iter().any(|(a, _)| !functions.contains(a));
        let (has_card, connectors) = connectors_of(host, &d.address);
        // PCI class 0302xx = 3D controller: no display engine wired out
        let has_outputs = if d.class.trim_start_matches("0x").starts_with("0302") {
            Some(false)
        } else if has_card {
            Some(!connectors.is_empty())
        } else {
            None
        };
        let reset_methods: Vec<String> = read(&host.devdir(&d.address).join("reset_method"))
            .split_whitespace()
            .map(str::to_string)
            .collect();
        let holders = host.holders(&host.device_nodes(&functions));
        let q: Vec<Quirk> = quirks
            .iter()
            .filter(|q| quirk_matches(q, &d.vendor_id, &d.device_id))
            .cloned()
            .collect();
        let mut reasons = Vec::new();
        let mut blocked = false;
        let mut warn_ = false;
        if d.iommu_group.is_none() {
            blocked = true;
            reasons.push("no IOMMU group: turn on VT-d / AMD-Vi and the kernel IOMMU".into());
        }
        // other devices in the group that are not functions of this card
        let foreign: Vec<&String> = functions
            .iter()
            .filter(|f| f.rsplit_once('.').map(|x| x.0) != d.address.rsplit_once('.').map(|x| x.0))
            .collect();
        if !foreign.is_empty() {
            warn_ = true;
            reasons.push(format!(
                "its IOMMU group also contains other devices that would move to the VM with it: {}",
                foreign
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if reset_methods.is_empty() {
            warn_ = true;
            reasons.push(
                "the device offers no reset method: it may not come back after the VM stops".into(),
            );
        }
        let single = !mine.is_empty() && !host_keeps_display;
        if single {
            reasons.push(format!(
                "drives the host's only connected display ({}): single-GPU passthrough, the host screen goes dark while the VM runs",
                mine.join(", ")
            ));
        }
        if !holders.is_empty() {
            warn_ = true;
            reasons.push(format!(
                "in use on the host by {}; these must let go before the VM starts",
                holders_message(&holders)
            ));
        }
        if has_outputs == Some(false) {
            reasons.push(
                "no display outputs (compute/datacenter card or muxless laptop GPU): the guest renders into a virtual display and is shown through the Horcrux viewer (guest tools, phase 3); no monitor or dummy plug needed".into(),
            );
        }
        if matches!(d.driver.as_deref(), Some("i915") | Some("xe")) {
            reasons.push(
                "integrated GPU: keep it for the host desktop and pass the discrete GPU".into(),
            );
        }
        for x in &q {
            match x.severity.as_str() {
                "block" => blocked = true,
                "warn" => warn_ = true,
                _ => {}
            }
            reasons.push(x.title.clone());
        }
        let verdict = if blocked {
            "blocked"
        } else if single {
            "single-gpu"
        } else if warn_ {
            "ready-with-warnings"
        } else {
            "ready"
        };
        gpus.push(GpuCheck {
            address: d.address.clone(),
            vendor_id: d.vendor_id.clone(),
            device_id: d.device_id.clone(),
            vendor_name: d.vendor_name.clone(),
            driver: d.driver.clone(),
            iommu_group: d.iommu_group.clone(),
            functions,
            displays: mine,
            connectors,
            has_outputs,
            host_keeps_display,
            reset_methods,
            holders,
            quirks: q,
            verdict: verdict.into(),
            reasons,
        });
    }
    GamingReport {
        kvm: host.dev.join("kvm").exists(),
        iommu_groups: pci.iommu_groups(),
        vfio_pci_loaded: host.sys.join("bus/pci/drivers/vfio-pci").exists(),
        suggested_vcpus,
        suggested_pinning,
        hugepages: tuning::hugepage_pools(&host.sys),
        cpus,
        gpus,
        nested_virt_problem: tuning::nested_kvm_problem(&host.sys).unwrap_or_default(),
        nested_virt: tuning::nested_kvm_problem(&host.sys).is_none(),
        secure_boot_firmware: super::firmware::find_ovmf_for(true).is_some(),
        swtpm: in_path("swtpm"),
        iso_tool: iso_tool().unwrap_or_default().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    /// A hybrid laptop: Intel iGPU 00:02.0 (group 0, the
    /// internal panel), NVIDIA dGPU 02:00.0 + its audio 02:00.1 (group 14,
    /// external ports), a root port bridge in group 4.
    struct Fx {
        t: tempfile::TempDir,
    }

    impl Fx {
        fn sys(&self) -> PathBuf {
            self.t.path().join("sys")
        }
        fn host(&self) -> Host {
            Host {
                sys: self.sys(),
                proc_: self.t.path().join("proc"),
                dev: self.t.path().join("dev"),
            }
        }
        fn dev(
            &self,
            addr: &str,
            class: &str,
            vendor: &str,
            device: &str,
            driver: Option<&str>,
            group: &str,
        ) {
            let s = self.sys();
            let d = s.join("devices/pci").join(addr);
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("class"), class).unwrap();
            fs::write(d.join("vendor"), format!("0x{vendor}")).unwrap();
            fs::write(d.join("device"), format!("0x{device}")).unwrap();
            fs::write(d.join("driver_override"), "(null)").unwrap();
            fs::write(d.join("reset_method"), "flr bus").unwrap();
            if let Some(drv) = driver {
                let dd = s.join("bus/pci/drivers").join(drv);
                fs::create_dir_all(&dd).unwrap();
                fs::write(dd.join("unbind"), "").unwrap();
                symlink(&dd, d.join("driver")).unwrap();
            }
            let g = s.join("kernel/iommu_groups").join(group).join("devices");
            fs::create_dir_all(&g).unwrap();
            symlink(&d, g.join(addr)).unwrap();
            symlink(
                s.join("kernel/iommu_groups").join(group),
                d.join("iommu_group"),
            )
            .unwrap();
            fs::create_dir_all(s.join("bus/pci/devices")).unwrap();
            symlink(&d, s.join("bus/pci/devices").join(addr)).unwrap();
        }
        fn connector(&self, card: &str, addr: &str, name: &str, status: &str) {
            let s = self.sys();
            let c = s.join("class/drm");
            fs::create_dir_all(&c).unwrap();
            let cd = c.join(card);
            if !cd.exists() {
                fs::create_dir_all(&cd).unwrap();
                symlink(s.join("devices/pci").join(addr), cd.join("device")).unwrap();
                let drm = s.join("devices/pci").join(addr).join("drm").join(card);
                fs::create_dir_all(&drm).unwrap();
            }
            let n = c.join(format!("{card}-{name}"));
            fs::create_dir_all(&n).unwrap();
            fs::write(n.join("status"), status).unwrap();
        }
        fn open_by(&self, pid: u32, comm: &str, target: &Path) {
            let p = self.t.path().join("proc").join(pid.to_string());
            fs::create_dir_all(p.join("fd")).unwrap();
            fs::write(p.join("comm"), comm).unwrap();
            let n = fs::read_dir(p.join("fd")).unwrap().count();
            symlink(target, p.join("fd").join(n.to_string())).unwrap();
        }
    }

    fn laptop() -> Fx {
        let fx = Fx {
            t: tempfile::tempdir().unwrap(),
        };
        fs::create_dir_all(fx.t.path().join("proc")).unwrap();
        fs::create_dir_all(fx.t.path().join("dev")).unwrap();
        fx.dev(
            "0000:00:02.0",
            "0x030000",
            "8086",
            "7d67",
            Some("i915"),
            "0",
        );
        fx.dev(
            "0000:00:06.0",
            "0x060400",
            "8086",
            "ae4d",
            Some("pcieport"),
            "4",
        );
        fx.dev(
            "0000:02:00.0",
            "0x030000",
            "10de",
            "2c58",
            Some("nvidia"),
            "14",
        );
        fx.dev(
            "0000:02:00.1",
            "0x040300",
            "10de",
            "22e9",
            Some("snd_hda_intel"),
            "14",
        );
        let s = fx.sys();
        fs::create_dir_all(s.join("bus/pci/drivers/vfio-pci")).unwrap();
        fs::write(s.join("bus/pci/drivers/vfio-pci/unbind"), "").unwrap();
        fs::write(s.join("bus/pci/drivers_probe"), "").unwrap();
        fx.connector("card0", "0000:00:02.0", "eDP-1", "connected");
        fx.connector("card1", "0000:02:00.0", "HDMI-A-1", "disconnected");
        fx.connector("card1", "0000:02:00.0", "eDP-2", "disconnected");
        // cpus
        let cpu = s.join("devices/system/cpu");
        fs::create_dir_all(&cpu).unwrap();
        fs::write(cpu.join("online"), "0-23").unwrap();
        fs::create_dir_all(s.join("devices/cpu_core")).unwrap();
        fs::write(s.join("devices/cpu_core/cpus"), "0-7").unwrap();
        fs::create_dir_all(s.join("devices/cpu_atom")).unwrap();
        fs::write(s.join("devices/cpu_atom/cpus"), "8-23").unwrap();
        fx
    }

    #[test]
    fn group_functions_skip_bridges_and_include_the_audio_function() {
        let fx = laptop();
        let (g, f) = fx.host().group_functions("0000:02:00.0").unwrap();
        assert_eq!(g, "14");
        assert_eq!(f, vec!["0000:02:00.0", "0000:02:00.1"]);
        assert!(fx.host().group_functions("0000:09:00.0").is_err());
    }

    #[test]
    fn dgpu_with_igpu_on_the_panel_is_ready() {
        let fx = laptop();
        let h = fx.host();
        let (funcs, groups) = preflight(
            &h,
            &[("0000:02:00.0".into(), false)],
            &PassthroughConfig::default(),
            false,
        )
        .unwrap();
        assert_eq!(funcs, vec!["0000:02:00.0", "0000:02:00.1"]);
        assert_eq!(groups, vec!["14"]);
    }

    #[test]
    fn the_hosts_only_display_is_refused_unless_allowed() {
        let fx = laptop();
        let h = fx.host();
        // the iGPU drives the only connected panel
        let e = preflight(
            &h,
            &[("0000:00:02.0".into(), false)],
            &PassthroughConfig::default(),
            false,
        )
        .unwrap_err()
        .to_string();
        assert!(
            e.contains("only connected display") && e.contains("card0-eDP-1"),
            "{e}"
        );
        assert!(preflight(
            &h,
            &[("0000:00:02.0".into(), true)],
            &PassthroughConfig::default(),
            false
        )
        .is_ok());
        // a monitor on the dGPU too: the panel stays with the host
        fx.connector("card1", "0000:02:00.0", "DP-1", "connected");
        assert!(preflight(
            &h,
            &[("0000:02:00.0".into(), false)],
            &PassthroughConfig::default(),
            false
        )
        .is_ok());
    }

    #[test]
    fn programs_holding_the_gpu_are_named_and_block_the_handoff() {
        let fx = laptop();
        let h = fx.host();
        let card1 = fx.t.path().join("dev/dri/card1");
        fx.open_by(4242, "gnome-shell", &card1);
        fx.open_by(4343, "bash", &fx.t.path().join("dev/null"));
        let nodes = h.device_nodes(&["0000:02:00.0".to_string()]);
        assert!(nodes.contains(&card1), "{nodes:?}");
        // last GPU on nvidia: control nodes count too
        assert!(nodes.contains(&fx.t.path().join("dev/nvidiactl")));
        let e = preflight(
            &h,
            &[("0000:02:00.0".into(), false)],
            &PassthroughConfig::default(),
            false,
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("gnome-shell (pid 4242"), "{e}");
        assert!(!e.contains("bash"));
        // the check can be switched off by the node setting
        let cfg = PassthroughConfig {
            handoff_refuse_when_held: false,
            ..Default::default()
        };
        assert!(preflight(&h, &[("0000:02:00.0".into(), false)], &cfg, false).is_ok());
    }

    #[tokio::test]
    async fn acquire_moves_the_group_records_it_and_release_gives_it_back() {
        let fx = laptop();
        let h = fx.host();
        let disk = fx.t.path().join("win.qcow2");
        let hp = vec![VmHostPci {
            device: "02:00.0".into(),
            managed: true,
            ..Default::default()
        }];
        acquire(&h, &disk, &hp).await.unwrap();
        let s = fx.sys();
        for a in ["0000:02:00.0", "0000:02:00.1"] {
            assert_eq!(
                read(&s.join("devices/pci").join(a).join("driver_override")),
                "vfio-pci"
            );
        }
        // the old drivers were told to let go
        assert_eq!(
            read(&s.join("bus/pci/drivers/nvidia/unbind")),
            "0000:02:00.0"
        );
        assert_eq!(
            read(&s.join("bus/pci/drivers/snd_hda_intel/unbind")),
            "0000:02:00.1"
        );
        let rec: HandoffRecord =
            serde_json::from_slice(&fs::read(record_file(&disk)).unwrap()).unwrap();
        assert_eq!(rec.groups, vec!["14"]);
        assert_eq!(
            rec.devices,
            vec![
                ("0000:02:00.0".to_string(), Some("nvidia".to_string())),
                (
                    "0000:02:00.1".to_string(),
                    Some("snd_hda_intel".to_string())
                )
            ]
        );
        // QEMU still holds the group: nothing is given back yet
        fx.open_by(777, "qemu-system-x86", &fx.t.path().join("dev/vfio/14"));
        release(&h, &disk, Duration::from_millis(600)).await;
        assert!(record_file(&disk).exists());
        // QEMU gone
        fs::remove_dir_all(fx.t.path().join("proc/777")).unwrap();
        release(&h, &disk, Duration::from_secs(2)).await;
        assert!(!record_file(&disk).exists());
        for a in ["0000:02:00.0", "0000:02:00.1"] {
            assert_eq!(
                read(&s.join("devices/pci").join(a).join("driver_override")),
                ""
            );
        }
        assert_eq!(read(&s.join("bus/pci/drivers_probe")), "0000:02:00.1");
    }

    /// Init backend double: records stop/start; stopping `kills` removes a
    /// fake process (the compositor exiting with its display manager).
    struct MockSvc {
        active: Vec<String>,
        kills: Option<PathBuf>,
        log: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl HostServices for MockSvc {
        async fn is_active(&self, name: &str) -> bool {
            self.active.iter().any(|a| a == name)
        }
        async fn stop(&self, name: &str) -> std::result::Result<(), String> {
            self.log.lock().unwrap().push(format!("stop {name}"));
            if let Some(p) = &self.kills {
                let _ = fs::remove_dir_all(p);
            }
            Ok(())
        }
        async fn start(&self, name: &str) -> std::result::Result<(), String> {
            self.log.lock().unwrap().push(format!("start {name}"));
            Ok(())
        }
    }

    #[tokio::test]
    async fn single_gpu_handoff_stops_the_desktop_detaches_the_console_and_restores_both() {
        let fx = laptop();
        let h = fx.host();
        let s = fx.sys();
        // a desktop with one GPU in use: the panel is off, the monitor is on
        // the dGPU, and the compositor has the card open
        fs::write(s.join("class/drm/card0-eDP-1/status"), "disconnected").unwrap();
        fx.connector("card1", "0000:02:00.0", "HDMI-A-1", "connected");
        fx.open_by(4242, "gnome-shell", &fx.t.path().join("dev/dri/card1"));
        for (n, name, bind) in [
            ("vtcon0", "(S) dummy device", "1"),
            ("vtcon1", "(M) frame buffer device", "1"),
        ] {
            let d = s.join("class/vtconsole").join(n);
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("name"), name).unwrap();
            fs::write(d.join("bind"), bind).unwrap();
        }
        let fb = s.join("bus/platform/drivers/efi-framebuffer");
        fs::create_dir_all(fb.join("efi-framebuffer.0")).unwrap();
        fs::write(fb.join("unbind"), "").unwrap();
        fs::write(fb.join("bind"), "").unwrap();

        let hp = |allow: bool| {
            vec![VmHostPci {
                device: "02:00.0".into(),
                managed: true,
                allow_display_loss: allow,
                ..Default::default()
            }]
        };
        let devices = managed(&hp(true));
        assert!(takes_only_display(&h, &devices));
        let cfg = PassthroughConfig {
            handoff_display_release_secs: 2,
            ..Default::default()
        };
        let svc = MockSvc {
            active: vec!["display-manager".into()],
            kills: Some(fx.t.path().join("proc/4242")),
            log: Default::default(),
        };
        let disk = fx.t.path().join("win.qcow2");
        // without allow_display_loss: refused, nothing stopped
        let e = acquire_with(&h, &disk, &hp(false), &cfg, Some(&svc))
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("only connected display"), "{e}");
        assert!(svc.log.lock().unwrap().is_empty());

        acquire_with(&h, &disk, &hp(true), &cfg, Some(&svc))
            .await
            .unwrap();
        assert_eq!(*svc.log.lock().unwrap(), vec!["stop display-manager"]);
        assert_eq!(read(&s.join("class/vtconsole/vtcon1/bind")), "0");
        assert_eq!(read(&s.join("class/vtconsole/vtcon0/bind")), "1");
        assert_eq!(read(&fb.join("unbind")), "efi-framebuffer.0");
        assert_eq!(
            read(&s.join("devices/pci/0000:02:00.0/driver_override")),
            "vfio-pci"
        );
        let rec: HandoffRecord =
            serde_json::from_slice(&fs::read(record_file(&disk)).unwrap()).unwrap();
        assert_eq!(rec.display_services, vec!["display-manager"]);
        assert_eq!(rec.vtconsoles, vec!["vtcon1"]);
        assert_eq!(
            rec.framebuffers,
            vec![(
                "efi-framebuffer".to_string(),
                "efi-framebuffer.0".to_string()
            )]
        );

        // the VM is gone: GPU back, then console, then the desktop
        give_back_with(&h, &rec, &cfg, Some(&svc)).await;
        assert_eq!(
            read(&s.join("devices/pci/0000:02:00.0/driver_override")),
            ""
        );
        assert_eq!(read(&s.join("class/vtconsole/vtcon1/bind")), "1");
        assert_eq!(read(&fb.join("bind")), "efi-framebuffer.0");
        assert_eq!(
            *svc.log.lock().unwrap(),
            vec!["stop display-manager", "start display-manager"]
        );
    }

    #[tokio::test]
    async fn a_desktop_that_keeps_the_gpu_undoes_the_single_gpu_steps() {
        let fx = laptop();
        let h = fx.host();
        let s = fx.sys();
        fs::write(s.join("class/drm/card0-eDP-1/status"), "disconnected").unwrap();
        fx.connector("card1", "0000:02:00.0", "HDMI-A-1", "connected");
        // a program that does not exit with the display manager
        fx.open_by(5150, "obs", &fx.t.path().join("dev/dri/card1"));
        let cfg = PassthroughConfig {
            handoff_display_release_secs: 1,
            ..Default::default()
        };
        let svc = MockSvc {
            active: vec!["display-manager".into()],
            kills: None,
            log: Default::default(),
        };
        let hp = vec![VmHostPci {
            device: "02:00.0".into(),
            managed: true,
            allow_display_loss: true,
            ..Default::default()
        }];
        let disk = fx.t.path().join("win.qcow2");
        let e = acquire_with(&h, &disk, &hp, &cfg, Some(&svc))
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("obs (pid 5150"), "{e}");
        // the desktop came back, the GPU never moved
        assert_eq!(
            *svc.log.lock().unwrap(),
            vec!["stop display-manager", "start display-manager"]
        );
        assert_eq!(
            read(&s.join("devices/pci/0000:02:00.0/driver_override")),
            "(null)"
        );
        assert!(!record_file(&disk).exists());
    }

    #[test]
    fn gpus_without_outputs_are_recognised() {
        let fx = laptop();
        // a datacenter card: 3D controller class
        fx.dev(
            "0000:05:00.0",
            "0x030200",
            "10de",
            "27b8",
            Some("nvidia"),
            "20",
        );
        let r = host_report(&fx.host(), &[]);
        let l4 = r.gpus.iter().find(|g| g.address == "0000:05:00.0").unwrap();
        assert_eq!(l4.has_outputs, Some(false));
        assert!(l4.reasons.iter().any(|x| x.contains("no display outputs")));
        let dgpu = r.gpus.iter().find(|g| g.address == "0000:02:00.0").unwrap();
        assert_eq!(dgpu.has_outputs, Some(true));
        assert_eq!(dgpu.connectors, vec!["HDMI-A-1", "eDP-2"]);
    }

    #[tokio::test]
    async fn acquire_refuses_without_touching_anything() {
        let fx = laptop();
        let h = fx.host();
        fx.open_by(4242, "gnome-shell", &fx.t.path().join("dev/dri/card1"));
        let disk = fx.t.path().join("win.qcow2");
        let hp = vec![VmHostPci {
            device: "0000:02:00.0".into(),
            managed: true,
            ..Default::default()
        }];
        assert!(acquire(&h, &disk, &hp).await.is_err());
        assert_eq!(
            read(&fx.sys().join("devices/pci/0000:02:00.0/driver_override")),
            "(null)"
        );
        assert!(!record_file(&disk).exists());
        // unmanaged devices are not this module's business
        let hp = vec![VmHostPci {
            device: "0000:02:00.0".into(),
            ..Default::default()
        }];
        acquire(&h, &disk, &hp).await.unwrap();
        assert!(!record_file(&disk).exists());
    }

    #[test]
    fn quirks_match_by_vendor_and_device_range() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("q.toml");
        fs::write(
            &f,
            r#"
[[quirk]]
vendor = "10de"
device_ranges = [["2b80", "2bff"], ["2c00", "2cff"]]
severity = "warn"
title = "Blackwell reset"
"#,
        )
        .unwrap();
        let q = load_quirks(&[t.path().join("missing.toml"), f]);
        assert_eq!(q.len(), 1);
        assert!(quirk_matches(&q[0], "10de", "2c58"));
        assert!(!quirk_matches(&q[0], "10de", "2684"));
        assert!(!quirk_matches(&q[0], "1002", "2c58"));
    }

    #[test]
    fn report_for_a_hybrid_laptop() {
        let fx = laptop();
        let q = vec![Quirk {
            vendor: "10de".into(),
            device_ranges: vec![["2c00".into(), "2cff".into()]],
            severity: "warn".into(),
            title: "Blackwell reset".into(),
            ..Default::default()
        }];
        let r = host_report(&fx.host(), &q);
        assert_eq!(r.suggested_vcpus, 8);
        assert_eq!(r.suggested_pinning, "0-7");
        assert!(r.vfio_pci_loaded);
        assert_eq!(r.gpus.len(), 2);
        let igpu = &r.gpus[0];
        assert_eq!(igpu.verdict, "single-gpu", "{:?}", igpu.reasons);
        let d = &r.gpus[1];
        assert_eq!(d.address, "0000:02:00.0");
        assert!(d.host_keeps_display);
        assert_eq!(d.functions, vec!["0000:02:00.0", "0000:02:00.1"]);
        assert_eq!(d.reset_methods, vec!["flr", "bus"]);
        assert_eq!(d.verdict, "ready-with-warnings");
        assert!(d.reasons.iter().any(|x| x == "Blackwell reset"));
    }

    #[test]
    fn the_shipped_quirks_table_parses() {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("data/gpu-quirks.toml");
        let q = load_quirks(&[p]);
        assert!(q.iter().any(|x| quirk_matches(x, "10de", "2c58")), "{q:?}");
        for x in &q {
            assert!(matches!(x.severity.as_str(), "info" | "warn" | "block"));
            assert!(!x.title.is_empty());
        }
    }
}
