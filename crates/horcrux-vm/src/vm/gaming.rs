//! Gaming VM wizard backend (docs/design/gaming-vm.md, phase 2).
//!
//! [`compose`] turns a wizard request plus the host check
//! ([`super::handoff::GamingReport`]) into an ordinary [`VmConfig`]: q35 +
//! Secure Boot OVMF with Microsoft keys, a per-VM persistent TPM 2.0
//! (swtpm), Hyper-V enlightenments, nested virtualisation for VBS/HVCI,
//! CPU pinning (`auto`: P-cores / one CCD), huge pages, virtio disk with an
//! I/O thread, the QEMU guest agent, a stable SMBIOS identity generated
//! once, and the chosen GPU handed over at start. Nothing about it is
//! special afterwards: it is a VM like any other, editable on its pages.
//!
//! [`unattend_xml`] writes the Windows answer file (locale, local account,
//! virtio storage/network drivers from the virtio-win disc in Windows PE,
//! VBS/HVCI on, the virtio guest tools + QEMU guest agent at first logon,
//! Fast Startup off) and [`build_unattend_iso`] puts it on a small disc
//! next to the VM's disk ([`super::qemu::unattend_iso`]), attached while it
//! exists and deleted once the installed guest's agent answers.
//!
//! [`load_compat`] reads the per-game anti-cheat table
//! (`gaming-compat.toml`), [`match_games`] answers "can I play X in this
//! VM?" before anyone buys or launches it.
//!
//! Honest by design: the hypervisor is never hidden (Hyper-V
//! enlightenments stay on, no `kvm=off`, no vendor-id or SMBIOS copy of a
//! real machine), so games that refuse VMs keep refusing them, and the
//! wizard says so up front.
use super::handoff::GamingReport;
use horcrux_common::{
    BootDevice, Error, Firmware, MachineType, Result, UsbDevice, VmArchitecture, VmConfig,
    VmCpuPinning, VmDisk, VmDisplay, VmEvdev, VmHardware, VmHostPci, VmHugePages, VmHypervisor,
    VmNic, VmSmbios, VmStatus,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ------------------------------------------------------------- settings

/// Node settings of the wizard (`GET/PUT /api/vms/gaming/settings`); every
/// value has a default.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct GamingSettings {
    /// Microsoft's Windows 11 download page (the ISO link it hands out is
    /// personal and expires, so the user fetches it there).
    pub windows_download_page: String,
    /// Microsoft's Windows 11 Enterprise evaluation page (90-day ISO for
    /// trying the setup).
    pub windows_eval_page: String,
    /// Direct link to the Windows 11 Enterprise evaluation ISO (English,
    /// x64): what "Download the evaluation" fetches into the ISO library.
    pub windows_eval_iso_url: String,
    /// Stable virtio-win driver ISO (storage/network drivers, guest agent).
    pub virtio_win_url: String,
    /// Default guest memory: half the host RAM, at most this (MB).
    pub default_memory_cap_mb: u64,
    /// Smallest guest memory the wizard accepts (Windows 11 needs 4 GB).
    pub min_memory_mb: u64,
    /// Default system disk size (GB).
    pub default_disk_gb: u64,
    /// Smallest system disk (Windows 11 needs 64 GB).
    pub min_disk_gb: u64,
    /// Huge-page size for guest memory (KiB, 2048 or 1048576); 0 = off.
    pub hugepages_kb: u64,
    /// Turn on VBS / Memory Integrity (HVCI) in the guest (needs nested
    /// virtualisation on the host).
    pub vbs: bool,
    /// Also turn VBS on by default when this Horcrux host is itself a
    /// virtual machine (Windows' hypervisor then runs a third level deep;
    /// many clouds and CPUs can't, and Windows restarts in a loop at boot).
    #[serde(default)]
    pub vbs_on_virtual_host: bool,
    /// SMBIOS identity strings of gaming VMs (a Horcrux machine, never a
    /// real vendor's model).
    pub smbios_manufacturer: String,
    pub smbios_product: String,
    pub smbios_family: String,
    pub smbios_version: String,
    /// Windows setup defaults.
    pub ui_language: String,
    pub input_locale: String,
    pub timezone: String,
    /// install.wim image index used when the request names no edition.
    pub image_index: u32,
    /// Drive letters searched for the virtio-win and answer discs.
    pub driver_search_letters: String,
    /// Seconds a key is pressed after the first start, so the Windows
    /// disc's "Press any key to boot from CD or DVD" boots the installer.
    pub boot_key_secs: u32,
    /// How often (s) to check whether the installed Windows' guest agent
    /// answers, after which the answer-file disc is deleted.
    pub unattend_poll_secs: u64,
}

impl Default for GamingSettings {
    fn default() -> Self {
        Self {
            windows_download_page: "https://www.microsoft.com/software-download/windows11".into(),
            windows_eval_page:
                "https://www.microsoft.com/evalcenter/download-windows-11-enterprise".into(),
            windows_eval_iso_url:
                "https://go.microsoft.com/fwlink/?linkid=2289031&clcid=0x409&culture=en-us&country=us"
                    .into(),
            virtio_win_url:
                "https://fedorapeople.org/groups/virt/virtio-win/direct-downloads/stable-virtio/virtio-win.iso"
                    .into(),
            default_memory_cap_mb: 32768,
            min_memory_mb: 4096,
            default_disk_gb: 128,
            min_disk_gb: 64,
            hugepages_kb: 2048,
            vbs: true,
            vbs_on_virtual_host: false,
            smbios_manufacturer: "Horcrux".into(),
            smbios_product: "Horcrux Gaming VM".into(),
            smbios_family: "Virtual Machine".into(),
            smbios_version: "1".into(),
            ui_language: "en-US".into(),
            input_locale: "0409:00000409".into(),
            timezone: "UTC".into(),
            image_index: 1,
            driver_search_letters: "DEFGH".into(),
            boot_key_secs: 20,
            unattend_poll_secs: 30,
        }
    }
}

impl GamingSettings {
    pub fn validate(&self) -> std::result::Result<(), String> {
        for (what, url) in [
            ("windows_download_page", &self.windows_download_page),
            ("windows_eval_page", &self.windows_eval_page),
            ("windows_eval_iso_url", &self.windows_eval_iso_url),
            ("virtio_win_url", &self.virtio_win_url),
        ] {
            if !url.starts_with("https://") || url.len() > 512 {
                return Err(format!("{what} must be an https:// URL"));
            }
        }
        if self.min_memory_mb < 1024 || self.default_memory_cap_mb < self.min_memory_mb {
            return Err(
                "memory: min_memory_mb >= 1024 and default_memory_cap_mb >= min_memory_mb".into(),
            );
        }
        if self.min_disk_gb < 16 || self.default_disk_gb < self.min_disk_gb {
            return Err("disk: min_disk_gb >= 16 and default_disk_gb >= min_disk_gb".into());
        }
        if !matches!(self.hugepages_kb, 0 | 2048 | 1048576) {
            return Err("hugepages_kb must be 0 (off), 2048 or 1048576".into());
        }
        for (what, v) in [
            ("smbios_manufacturer", &self.smbios_manufacturer),
            ("smbios_product", &self.smbios_product),
            ("smbios_family", &self.smbios_family),
            ("smbios_version", &self.smbios_version),
        ] {
            if v.is_empty() || v.len() > 64 || !v.bytes().all(|b| (0x20..0x7f).contains(&b)) {
                return Err(format!("{what}: 1-64 printable ASCII characters"));
            }
        }
        check_locale("ui_language", &self.ui_language)?;
        check_input_locale(&self.input_locale)?;
        check_timezone(&self.timezone)?;
        if self.image_index == 0 || self.image_index > 64 {
            return Err("image_index must be 1-64".into());
        }
        let l = &self.driver_search_letters;
        if l.is_empty() || l.len() > 20 || !l.bytes().all(|b| b.is_ascii_uppercase() && b > b'C') {
            return Err("driver_search_letters: drive letters D-Z, e.g. DEFGH".into());
        }
        if self.boot_key_secs > 300 {
            return Err("boot_key_secs: 0-300".into());
        }
        if !(5..=3600).contains(&self.unattend_poll_secs) {
            return Err("unattend_poll_secs: 5-3600".into());
        }
        Ok(())
    }
}

fn check_locale(what: &str, v: &str) -> std::result::Result<(), String> {
    let ok =
        (2..=16).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(format!("{what} {v:?}: a language tag like en-US"))
    }
}

fn check_input_locale(v: &str) -> std::result::Result<(), String> {
    let ok = (2..=64).contains(&v.len())
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b':' | b';' | b'{' | b'}'));
    if ok {
        Ok(())
    } else {
        Err(format!("input_locale {v:?}: e.g. 0409:00000409 or en-US"))
    }
}

fn check_timezone(v: &str) -> std::result::Result<(), String> {
    let ok = (1..=64).contains(&v.len())
        && v.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b' ' | b'-' | b'(' | b')' | b'.' | b'+' | b'_')
        });
    if ok {
        Ok(())
    } else {
        Err(format!(
            "timezone {v:?}: a Windows time zone name such as \"Pacific Standard Time\" or UTC"
        ))
    }
}

// --------------------------------------------------------------- request

/// `POST /api/vms/gaming`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GamingVmRequest {
    pub name: String,
    /// VM id; default `vm-<unix time>`.
    pub id: Option<String>,
    /// PCI address of the GPU handed to the VM (from the gaming check);
    /// `None` = no GPU yet (install first, add the GPU later; also how CI
    /// proves the setup on machines without a spare GPU).
    pub gpu: Option<String>,
    /// Single-GPU passthrough: the GPU drives the host's only screen; the
    /// desktop is stopped while the VM runs and comes back afterwards.
    pub allow_display_loss: bool,
    pub vcpus: Option<u32>,
    pub memory_mb: Option<u64>,
    pub disk_gb: Option<u64>,
    /// Storage pool (id) for the system disk; `None` = VM storage dir.
    pub storage: Option<String>,
    /// Host bridge for the network card; `None` = the node's default
    /// network, empty string = no network card.
    pub bridge: Option<String>,
    /// Windows 11 ISO (a file in the ISO library).
    pub windows_iso: String,
    /// virtio-win ISO (a file in the ISO library).
    pub virtio_iso: String,
    /// Local Windows account created by setup (administrator).
    pub username: String,
    pub password: String,
    /// The user's own product key (optional).
    pub product_key: Option<String>,
    /// Edition: install.wim image name (e.g. "Windows 11 Pro") or index.
    pub image_name: Option<String>,
    pub image_index: Option<u32>,
    pub ui_language: Option<String>,
    pub input_locale: Option<String>,
    pub timezone: Option<String>,
    /// Host keyboards/mice for the guest (evdev, `/dev/input/by-id/...`).
    pub evdev: Vec<String>,
    /// Host USB devices (controllers, gamepads) for the guest.
    pub usb: Vec<UsbDevice>,
    pub hugepages: Option<bool>,
    /// VBS / Memory Integrity in the guest (default: the node setting).
    pub vbs: Option<bool>,
    /// Titles the user wants to play: their requirements are applied and
    /// blocked ones are reported before anything is created.
    pub games: Vec<String>,
    /// Steam Remote Play / Moonlight readiness (section 16): the VM gets a
    /// network card on the LAN when the host has one and the node side
    /// (Wake-on-LAN, relay, forwards) is set up.
    pub remote_play: Option<horcrux_common::remote_play::VmRemotePlay>,
    /// Start the VM (and with it the unattended install) right away.
    pub start: bool,
    /// Only compose and return the plan; create nothing.
    pub dry_run: bool,
}

/// What the host offers that [`compose`] needs beyond the gaming check.
#[derive(Debug, Clone, Default)]
pub struct HostFacts {
    pub memory_mb: u64,
    /// The node's default VM network (bridge).
    pub default_bridge: Option<String>,
    /// A bridge with a physical port (on the LAN), preferring the default.
    pub lan_bridge: Option<String>,
    /// A wired interface a macvtap card can use.
    pub macvtap_uplink: Option<String>,
}

/// Windows answer-file inputs.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct UnattendParams {
    pub computer_name: String,
    pub username: String,
    #[serde(skip)]
    pub password: String,
    #[serde(skip)]
    pub product_key: Option<String>,
    pub image_name: Option<String>,
    pub image_index: u32,
    pub ui_language: String,
    pub input_locale: String,
    pub timezone: String,
    pub vbs: bool,
    pub driver_letters: String,
}

/// The wizard's result: the VM config to create plus what the user should
/// know.
#[derive(Debug, Clone, Serialize)]
pub struct GamingPlan {
    pub config: VmConfig,
    pub unattend: UnattendParams,
    /// Things that will not work as hoped (blocked games, no GPU yet, VBS
    /// unavailable): shown before anything is created.
    pub warnings: Vec<String>,
    /// What the wizard decided and why.
    pub notes: Vec<String>,
    pub games: Vec<CompatMatch>,
}

fn bad(m: impl Into<String>) -> Error {
    Error::Validation(m.into())
}

/// Windows computer name from the VM name: letters, digits and '-',
/// at most 15 characters, not only digits.
pub fn computer_name(vm_name: &str) -> String {
    let mut s: String = vm_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '-'
            }
        })
        .collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    let mut s = s.trim_matches('-').to_string();
    s.truncate(15);
    let s = s.trim_end_matches('-').to_string();
    if s.is_empty() || s.bytes().all(|b| b.is_ascii_digit()) {
        format!("HX-{s}").chars().take(15).collect()
    } else {
        s
    }
}

/// A random UUID (v4) and serial: generated once per VM and stored in its
/// config, so the guest keeps the same identity forever (licences and game
/// accounts are not flagged for changing hardware).
pub fn new_smbios(s: &GamingSettings) -> VmSmbios {
    use rand::Rng;
    let mut b = [0u8; 16];
    rand::thread_rng().fill(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: Vec<String> = b.iter().map(|x| format!("{x:02x}")).collect();
    let uuid = format!(
        "{}-{}-{}-{}-{}",
        h[0..4].concat(),
        h[4..6].concat(),
        h[6..8].concat(),
        h[8..10].concat(),
        h[10..16].concat()
    );
    let serial = format!("HX-{}", h[10..16].concat().to_ascii_uppercase());
    VmSmbios {
        manufacturer: Some(s.smbios_manufacturer.clone()),
        product: Some(s.smbios_product.clone()),
        version: Some(s.smbios_version.clone()),
        serial: Some(serial),
        family: Some(s.smbios_family.clone()),
        uuid: Some(uuid),
    }
}

/// Build the VM config (pure: no I/O besides what is passed in).
pub fn compose(
    req: &GamingVmRequest,
    report: &GamingReport,
    host: &HostFacts,
    settings: &GamingSettings,
    compat: &[CompatEntry],
) -> Result<GamingPlan> {
    let name = req.name.trim();
    if name.is_empty() || name.len() > 128 {
        return Err(bad("name must be 1-128 characters"));
    }
    if !report.kvm {
        return Err(bad(
            "this host has no KVM (/dev/kvm): a gaming VM needs hardware virtualisation (VT-x/AMD-V) turned on in the firmware",
        ));
    }
    let mut warnings: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    // ---- games the user plans to play
    let games = match_games(compat, &req.games);
    let mut need_vbs = false;
    for g in &games {
        match &g.entry {
            None => warnings.push(format!(
                "{}: not in the compatibility list (not tested): its anti-cheat may refuse a VM and kick or ban the account",
                g.query
            )),
            Some(e) => {
                match e.vm_status.as_str() {
                    "blocked-by-publisher" => warnings.push(format!(
                        "{}: blocked in VMs by its publisher ({}). {}{}",
                        e.title,
                        e.anti_cheat,
                        e.note,
                        host_hint(e)
                    )),
                    "unknown" => warnings.push(format!(
                        "{}: no VM statement from the publisher ({}); it may refuse the VM.{}",
                        e.title,
                        e.anti_cheat,
                        host_hint(e)
                    )),
                    _ => notes.push(format!("{}: {}", e.title, e.note)),
                }
                if e.requires.iter().any(|r| r == "vbs") {
                    need_vbs = true;
                }
            }
        }
    }

    // ---- GPU
    let mut hostpci: Vec<VmHostPci> = Vec::new();
    match req.gpu.as_deref().map(str::trim).filter(|g| !g.is_empty()) {
        None => warnings.push(
            "no GPU chosen: Windows installs and runs with an emulated display; add a GPU on the VM's Hardware page (\"hand over at start\") to play".into(),
        ),
        Some(addr) => {
            let want = VmHostPci {
                device: addr.into(),
                ..Default::default()
            };
            let horcrux_common::HostPciKind::Pci(norm) = want.kind().map_err(bad)? else {
                return Err(bad("gpu must be a PCI address (0000:01:00.0)"));
            };
            let Some(g) = report.gpus.iter().find(|g| g.address == norm) else {
                return Err(bad(format!(
                    "GPU {norm} is not a GPU on this host (GPUs: {})",
                    report
                        .gpus
                        .iter()
                        .map(|g| g.address.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            };
            match g.verdict.as_str() {
                "blocked" => {
                    return Err(bad(format!(
                        "GPU {norm} can't be handed to a VM: {}",
                        g.reasons.join("; ")
                    )))
                }
                "single-gpu" if !req.allow_display_loss => {
                    return Err(bad(format!(
                        "GPU {norm} drives this host's only screen: confirm single-GPU passthrough (the local desktop stops while the VM runs and comes back when it shuts down), or connect the monitor to another GPU"
                    )))
                }
                "single-gpu" => warnings.push(
                    "single-GPU passthrough: this machine's screen belongs to Windows while the VM runs; the desktop comes back when Windows shuts down".into(),
                ),
                "ready-with-warnings" => {
                    warnings.extend(g.reasons.iter().map(|r| format!("GPU {norm}: {r}")))
                }
                _ => {}
            }
            if g.has_outputs == Some(false) {
                notes.push(format!(
                    "GPU {norm} has no display outputs: Windows renders into a virtual display shown through the Horcrux viewer (guest tools, phase 3); until then use the VM console"
                ));
            }
            hostpci.push(VmHostPci {
                device: norm,
                managed: true,
                allow_display_loss: g.verdict == "single-gpu" && req.allow_display_loss,
                ..Default::default()
            });
        }
    }

    // ---- CPUs and pinning
    let vcpus = req.vcpus.unwrap_or(report.suggested_vcpus).max(2);
    let pinning = super::tuning::auto_vcpu_cpus(&report.cpus, vcpus).map_err(|e| {
        bad(format!(
            "{e} (the wizard pins every vCPU to its own host CPU; suggested: {} vCPUs on {})",
            report.suggested_vcpus, report.suggested_pinning
        ))
    })?;
    notes.push(format!(
        "{vcpus} vCPUs pinned to host CPUs {}{}",
        horcrux_common::format_cpu_list(&pinning),
        if report.cpus.hybrid() {
            " (performance cores only)"
        } else if report.cpus.multi_l3() {
            " (kept on one CCD / L3 cache where it fits)"
        } else {
            ""
        }
    ));

    // ---- memory
    let default_mem = (host.memory_mb / 2)
        .min(settings.default_memory_cap_mb)
        .max(settings.min_memory_mb);
    let memory = req.memory_mb.unwrap_or(default_mem);
    if memory < settings.min_memory_mb {
        return Err(bad(format!(
            "memory must be at least {} MB (Windows 11)",
            settings.min_memory_mb
        )));
    }
    if host.memory_mb > 0 && memory + 2048 > host.memory_mb {
        return Err(bad(format!(
            "{memory} MB leaves the host less than 2 GB of its {} MB",
            host.memory_mb
        )));
    }
    let use_huge = req.hugepages.unwrap_or(settings.hugepages_kb != 0);
    let hugepages = use_huge.then_some(if settings.hugepages_kb == 0 {
        // settings say off, the request asks for them: the standard size
        VmHugePages::default()
    } else {
        VmHugePages {
            size_kb: settings.hugepages_kb,
        }
    });
    if let Some(h) = &hugepages {
        if !report.hugepages.iter().any(|(kb, _, _)| *kb == h.size_kb) {
            return Err(bad(format!(
                "this kernel offers no {} KiB huge pages: turn huge pages off for this VM",
                h.size_kb
            )));
        }
    }

    // ---- disk
    let disk_gb = req.disk_gb.unwrap_or(settings.default_disk_gb);
    if disk_gb < settings.min_disk_gb || disk_gb > 65536 {
        return Err(bad(format!(
            "disk must be {}-65536 GB (Windows 11 needs {} GB)",
            settings.min_disk_gb, settings.min_disk_gb
        )));
    }

    // ---- VBS / nested virtualisation
    const VBS_ON_VIRTUAL_HOST: &str = "this Horcrux host is itself a virtual machine, so Windows' own hypervisor (VBS) would run a third level deep; on many clouds and CPUs Windows then restarts in a loop right after the Windows Boot Manager starts.";
    let vbs_wanted = req.vbs.unwrap_or(settings.vbs) || need_vbs;
    let vbs = if vbs_wanted && !report.nested_virt {
        if req.vbs == Some(true) || need_vbs {
            return Err(bad(report.nested_virt_problem.clone()));
        }
        warnings.push(format!(
            "VBS / Memory Integrity stays off: {}",
            report.nested_virt_problem
        ));
        false
    } else if vbs_wanted && report.host_is_vm {
        if req.vbs == Some(true) || need_vbs || settings.vbs_on_virtual_host {
            warnings.push(VBS_ON_VIRTUAL_HOST.to_string());
            true
        } else {
            warnings.push(format!(
                "VBS / Memory Integrity stays off by default: {VBS_ON_VIRTUAL_HOST} Turn VBS on for this VM if this host is known to support it."
            ));
            false
        }
    } else {
        vbs_wanted
    };

    // ---- host prerequisites
    if !report.secure_boot_firmware {
        return Err(bad(
            "the Secure Boot build of OVMF (UEFI with Microsoft keys) is not installed on this host: install sys-firmware/edk2-bin (Debian/Ubuntu: ovmf)",
        ));
    }
    if !report.swtpm {
        return Err(bad(
            "swtpm (TPM 2.0 for VMs, required by Windows 11) is not installed on this host: install app-crypt/swtpm (Debian/Ubuntu: swtpm)",
        ));
    }
    if report.iso_tool.is_empty() {
        return Err(bad(
            "no ISO builder for the answer-file disc on this host: install dev-libs/libisoburn (xorriso) or app-cdr/cdrtools",
        ));
    }

    // ---- media and account
    if req.windows_iso.trim().is_empty() {
        return Err(bad(format!(
            "windows_iso: pick the Windows 11 ISO from the ISO library (download it from {} and add it to the library)",
            settings.windows_download_page
        )));
    }
    if req.virtio_iso.trim().is_empty() {
        return Err(bad(format!(
            "virtio_iso: pick the virtio-win ISO from the ISO library ({})",
            settings.virtio_win_url
        )));
    }
    check_username(&req.username)?;
    if req.password.is_empty() || req.password.len() > 127 {
        return Err(bad("password: 1-127 characters"));
    }
    if let Some(k) = req.product_key.as_deref().filter(|k| !k.trim().is_empty()) {
        let ok = k.len() == 29
            && k.split('-').count() == 5
            && k.split('-')
                .all(|p| p.len() == 5 && p.bytes().all(|b| b.is_ascii_alphanumeric()));
        if !ok {
            return Err(bad("product_key: XXXXX-XXXXX-XXXXX-XXXXX-XXXXX"));
        }
    }
    let ui_language = req
        .ui_language
        .clone()
        .unwrap_or_else(|| settings.ui_language.clone());
    check_locale("ui_language", &ui_language).map_err(bad)?;
    let input_locale = req
        .input_locale
        .clone()
        .unwrap_or_else(|| settings.input_locale.clone());
    check_input_locale(&input_locale).map_err(bad)?;
    let timezone = req
        .timezone
        .clone()
        .unwrap_or_else(|| settings.timezone.clone());
    check_timezone(&timezone).map_err(bad)?;
    if let Some(n) = &req.image_name {
        if n.len() > 128 || n.contains(['<', '>', '&']) {
            return Err(bad(
                "image_name: an edition name such as \"Windows 11 Pro\"",
            ));
        }
    }

    // ---- network
    let remote = req.remote_play.clone().filter(|r| r.enabled);
    if let Some(r) = &remote {
        r.validate().map_err(bad)?;
    }
    let wants_lan = remote
        .as_ref()
        .is_some_and(|r| r.network != horcrux_common::remote_play::RemotePlayNetwork::Nat);
    let mut attach = horcrux_common::NicAttach::Bridge;
    let bridge = match req.bridge.as_deref() {
        Some(b) => Some(b.trim().to_string()).filter(|b| !b.is_empty()),
        None if wants_lan => {
            if let Some(b) = &host.lan_bridge {
                notes.push(format!(
                    "Remote Play: the network card goes on {b}, a bridge on the LAN, so Steam/Moonlight clients find the VM directly"
                ));
                Some(b.clone())
            } else if let Some(i) = &host.macvtap_uplink {
                attach = horcrux_common::NicAttach::Macvtap;
                notes.push(format!(
                    "Remote Play: the network card sits directly on {i} (macvtap), so clients on the LAN find the VM; this host itself can't reach the VM over the network (the Horcrux console still works)"
                ));
                Some(i.clone())
            } else {
                warnings.push("Remote Play: this host has no bridge on the LAN and no wired interface for macvtap: the VM stays behind NAT and Horcrux relays discovery and forwards the ports".into());
                host.default_bridge.clone()
            }
        }
        None => host.default_bridge.clone(),
    };
    if bridge.is_none() {
        if remote.is_some() {
            return Err(bad(
                "Remote Play needs a network card: pick a network or leave it on the default",
            ));
        }
        notes.push("no network card: Windows installs offline with a local account; add one on the Hardware page later".into());
    }
    if remote.is_some() {
        notes.push("Remote Play: install Steam in Windows (or let Horcrux do it from the VM's Remote Play page once Windows is installed) and turn on automatic logon there, so a Wake-on-LAN start ends with Steam ready".into());
    }

    // ---- input
    let mut evdev: Vec<VmEvdev> = Vec::new();
    for (i, p) in req.evdev.iter().enumerate() {
        evdev.push(VmEvdev {
            path: p.clone(),
            // the first keyboard switches every grabbed device (both Ctrl keys)
            grab_all: i
                == req
                    .evdev
                    .iter()
                    .position(|x| x.contains("kbd"))
                    .unwrap_or(0),
        });
    }
    if !evdev.is_empty() {
        notes.push("keyboard/mouse go straight to Windows; press both Ctrl keys to switch them between host and VM".into());
    }

    let id = req
        .id
        .clone()
        .filter(|i| !i.trim().is_empty())
        .unwrap_or_else(|| format!("vm-{}", chrono::Utc::now().timestamp()));

    let mut hw = VmHardware {
        nets: bridge
            .into_iter()
            .map(|b| VmNic {
                model: Default::default(),
                bridge: b,
                attach,
                mac: None,
                vlan: None,
            })
            .collect(),
        cdrom: Some(req.windows_iso.clone()),
        extra_cdroms: vec![req.virtio_iso.clone()],
        boot: vec![BootDevice::Disk, BootDevice::Cdrom],
        cpu_type: None,
        sockets: None,
        machine: MachineType::Q35,
        firmware: Firmware::Ovmf,
        secure_boot: true,
        tpm: true,
        // CRB, like every new VM (what libvirt uses for Windows 11)
        tpm_model: Some(horcrux_common::TpmModel::Crb),
        // set once the TPM has been used (first start)
        tpm_state_expected: false,
        balloon: false,
        guest_agent: true,
        usb_controller: true,
        usb: req.usb.clone(),
        hostpci,
        adv: Default::default(),
    };
    hw.adv.hyperv = true;
    hw.adv.nested_virt = vbs;
    hw.adv.cpu_pinning = Some(VmCpuPinning {
        vcpus: "auto".into(),
        emulator: None,
    });
    hw.adv.hugepages = hugepages;
    hw.adv.evdev = evdev;
    hw.adv.smbios = Some(new_smbios(settings));
    hw.adv.vmgenid = Some("auto".into());
    hw.adv.remote_play = remote;

    let config = VmConfig {
        id,
        name: name.to_string(),
        hypervisor: VmHypervisor::Qemu,
        memory,
        cpus: vcpus,
        disk_size: disk_gb,
        status: VmStatus::Stopped,
        architecture: VmArchitecture::X86_64,
        disks: vec![VmDisk {
            path: String::new(),
            size_gb: disk_gb,
            disk_type: "virtio".into(),
            cache: "none".into(),
            discard: true,
            ssd: false,
            iothread: true,
            storage: req.storage.clone().filter(|s| !s.trim().is_empty()),
        }],
        display: VmDisplay::default(),
        hardware: hw,
    };
    config
        .hardware
        .validate_for(&config.architecture, config.cpus)
        .map_err(bad)?;

    let unattend = UnattendParams {
        computer_name: computer_name(name),
        username: req.username.clone(),
        password: req.password.clone(),
        product_key: req
            .product_key
            .clone()
            .filter(|k| !k.trim().is_empty())
            .map(|k| k.to_ascii_uppercase()),
        image_name: req.image_name.clone().filter(|n| !n.trim().is_empty()),
        image_index: req.image_index.unwrap_or(settings.image_index).max(1),
        ui_language,
        input_locale,
        timezone,
        vbs,
        driver_letters: settings.driver_search_letters.clone(),
    };
    Ok(GamingPlan {
        config,
        unattend,
        warnings,
        notes,
        games,
    })
}

fn host_hint(e: &CompatEntry) -> String {
    match e.host_status.as_str() {
        "works" | "supported" => " It runs on Horcrux itself through Steam instead.".into(),
        _ => String::new(),
    }
}

fn check_username(u: &str) -> Result<()> {
    let ok = !u.is_empty()
        && u.len() <= 20
        && !u.ends_with('.')
        && u.chars()
            .all(|c| c.is_ascii_graphic() && !"\"/\\[]:;|=,+*?<>@".contains(c) || c == ' ')
        && !u.trim().is_empty();
    let reserved = ["administrator", "guest", "defaultaccount", "system"];
    if !ok || reserved.contains(&u.to_ascii_lowercase().as_str()) {
        return Err(bad(
            "username: 1-20 characters, none of \" / \\ [ ] : ; | = , + * ? < > @, not Administrator/Guest",
        ));
    }
    Ok(())
}

// ------------------------------------------------------------- unattend

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

const COMPONENT_ATTRS: &str = r#"processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS""#;

/// The Windows answer file (`autounattend.xml`).
pub fn unattend_xml(p: &UnattendParams) -> String {
    let e = xml_escape;
    let a = COMPONENT_ATTRS;
    let mut drivers = String::new();
    let mut key = 1;
    for dir in ["viostor", "vioscsi", "NetKVM"] {
        for l in p.driver_letters.chars() {
            drivers.push_str(&format!(
                "        <PathAndCredentials wcm:action=\"add\" wcm:keyValue=\"{key}\"><Path>{l}:\\{dir}\\w11\\amd64</Path></PathAndCredentials>\n"
            ));
            key += 1;
        }
    }
    let image = match &p.image_name {
        Some(n) => format!(
            "<MetaData wcm:action=\"add\"><Key>/IMAGE/NAME</Key><Value>{}</Value></MetaData>",
            e(n)
        ),
        None => format!(
            "<MetaData wcm:action=\"add\"><Key>/IMAGE/INDEX</Key><Value>{}</Value></MetaData>",
            p.image_index
        ),
    };
    let product_key = p
        .product_key
        .as_ref()
        .map(|k| {
            format!(
                "        <ProductKey><Key>{}</Key><WillShowUI>OnError</WillShowUI></ProductKey>\n",
                e(k)
            )
        })
        .unwrap_or_default();
    let letters = p
        .driver_letters
        .chars()
        .map(String::from)
        .collect::<Vec<_>>()
        .join(" ");
    let mut specialize_cmds = String::new();
    if p.vbs {
        let dg = r"HKLM\SYSTEM\CurrentControlSet\Control\DeviceGuard";
        let cmds = [
            format!("reg add {dg} /v EnableVirtualizationBasedSecurity /t REG_DWORD /d 1 /f"),
            format!("reg add {dg} /v RequirePlatformSecurityFeatures /t REG_DWORD /d 1 /f"),
            format!(
                r"reg add {dg}\Scenarios\HypervisorEnforcedCodeIntegrity /v Enabled /t REG_DWORD /d 1 /f"
            ),
        ];
        for (i, c) in cmds.iter().enumerate() {
            specialize_cmds.push_str(&format!(
                "        <RunSynchronousCommand wcm:action=\"add\"><Order>{}</Order><Path>{}</Path><Description>VBS / Memory Integrity</Description></RunSynchronousCommand>\n",
                i + 1,
                e(c)
            ));
        }
    }
    let specialize_deploy = if specialize_cmds.is_empty() {
        String::new()
    } else {
        format!(
            "    <component name=\"Microsoft-Windows-Deployment\" {a}>\n      <RunSynchronous>\n{specialize_cmds}      </RunSynchronous>\n    </component>\n"
        )
    };
    let first_logon = [
        (
            format!("cmd /c for %d in ({letters}) do if exist %d:\\virtio-win-gt-x64.msi msiexec /i %d:\\virtio-win-gt-x64.msi /qn /norestart"),
            "virtio drivers",
        ),
        (
            format!("cmd /c for %d in ({letters}) do if exist %d:\\guest-agent\\qemu-ga-x86_64.msi msiexec /i %d:\\guest-agent\\qemu-ga-x86_64.msi /qn /norestart"),
            "QEMU guest agent",
        ),
        ("powercfg /h off".to_string(), "Fast Startup off"),
    ]
    .iter()
    .enumerate()
    .map(|(i, (c, d))| {
        format!(
            "        <SynchronousCommand wcm:action=\"add\"><Order>{}</Order><CommandLine>{}</CommandLine><Description>{d}</Description></SynchronousCommand>\n",
            i + 1,
            e(c)
        )
    })
    .collect::<String>();
    let user = e(&p.username);
    let pw = e(&p.password);
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<!-- Generated by Horcrux (gaming VM wizard). Removed from the VM once Windows is installed. -->
<unattend xmlns="urn:schemas-microsoft-com:unattend" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State">
  <settings pass="windowsPE">
    <component name="Microsoft-Windows-International-Core-WinPE" {a}>
      <SetupUILanguage><UILanguage>{lang}</UILanguage></SetupUILanguage>
      <InputLocale>{input}</InputLocale>
      <SystemLocale>{lang}</SystemLocale>
      <UILanguage>{lang}</UILanguage>
      <UserLocale>{lang}</UserLocale>
    </component>
    <component name="Microsoft-Windows-PnpCustomizationsWinPE" {a}>
      <DriverPaths>
{drivers}      </DriverPaths>
    </component>
    <component name="Microsoft-Windows-Setup" {a}>
      <DiskConfiguration>
        <Disk wcm:action="add">
          <DiskID>0</DiskID>
          <WillWipeDisk>true</WillWipeDisk>
          <CreatePartitions>
            <CreatePartition wcm:action="add"><Order>1</Order><Type>EFI</Type><Size>260</Size></CreatePartition>
            <CreatePartition wcm:action="add"><Order>2</Order><Type>MSR</Type><Size>16</Size></CreatePartition>
            <CreatePartition wcm:action="add"><Order>3</Order><Type>Primary</Type><Extend>true</Extend></CreatePartition>
          </CreatePartitions>
          <ModifyPartitions>
            <ModifyPartition wcm:action="add"><Order>1</Order><PartitionID>1</PartitionID><Format>FAT32</Format><Label>System</Label></ModifyPartition>
            <ModifyPartition wcm:action="add"><Order>2</Order><PartitionID>3</PartitionID><Format>NTFS</Format><Label>Windows</Label><Letter>C</Letter></ModifyPartition>
          </ModifyPartitions>
        </Disk>
      </DiskConfiguration>
      <ImageInstall>
        <OSImage>
          <InstallFrom>{image}</InstallFrom>
          <InstallTo><DiskID>0</DiskID><PartitionID>3</PartitionID></InstallTo>
        </OSImage>
      </ImageInstall>
      <UserData>
        <AcceptEula>true</AcceptEula>
        <FullName>{user}</FullName>
{product_key}      </UserData>
    </component>
  </settings>
  <settings pass="specialize">
    <component name="Microsoft-Windows-Shell-Setup" {a}>
      <ComputerName>{computer}</ComputerName>
      <TimeZone>{tz}</TimeZone>
    </component>
{specialize_deploy}  </settings>
  <settings pass="oobeSystem">
    <component name="Microsoft-Windows-International-Core" {a}>
      <InputLocale>{input}</InputLocale>
      <SystemLocale>{lang}</SystemLocale>
      <UILanguage>{lang}</UILanguage>
      <UserLocale>{lang}</UserLocale>
    </component>
    <component name="Microsoft-Windows-Shell-Setup" {a}>
      <OOBE>
        <HideEULAPage>true</HideEULAPage>
        <HideOnlineAccountScreens>true</HideOnlineAccountScreens>
        <HideWirelessSetupInOOBE>true</HideWirelessSetupInOOBE>
        <HideLocalAccountScreen>true</HideLocalAccountScreen>
        <ProtectYourPC>3</ProtectYourPC>
      </OOBE>
      <UserAccounts>
        <LocalAccounts>
          <LocalAccount wcm:action="add">
            <Name>{user}</Name>
            <Group>Administrators</Group>
            <Password><Value>{pw}</Value><PlainText>true</PlainText></Password>
          </LocalAccount>
        </LocalAccounts>
      </UserAccounts>
      <AutoLogon>
        <Enabled>true</Enabled>
        <Username>{user}</Username>
        <Password><Value>{pw}</Value><PlainText>true</PlainText></Password>
        <LogonCount>1</LogonCount>
      </AutoLogon>
      <FirstLogonCommands>
{first_logon}      </FirstLogonCommands>
    </component>
  </settings>
</unattend>
"#,
        lang = e(&p.ui_language),
        input = e(&p.input_locale),
        computer = e(&p.computer_name),
        tz = e(&p.timezone),
    )
}

/// Write `xml` as `autounattend.xml` on a small ISO at `out` (mode 0600:
/// it holds the account password until the install is done).
pub async fn build_unattend_iso(out: &Path, xml: &str) -> Result<()> {
    let tool = super::handoff::iso_tool()
        .ok_or_else(|| bad("no ISO builder installed (xorriso, genisoimage or mkisofs)"))?;
    let parent = out
        .parent()
        .ok_or_else(|| bad("answer disc path has no directory"))?;
    let staging = tempfile::Builder::new()
        .prefix(".unattend-")
        .tempdir_in(parent)
        .map_err(|e| Error::System(format!("answer disc staging dir: {e}")))?;
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(staging.path(), std::fs::Permissions::from_mode(0o700));
    }
    let src = staging.path().join("src");
    std::fs::create_dir(&src).map_err(|e| Error::System(format!("answer disc: {e}")))?;
    std::fs::write(src.join("autounattend.xml"), xml)
        .map_err(|e| Error::System(format!("answer disc: {e}")))?;
    let tmp = staging.path().join("unattend.iso");
    let mut cmd = tokio::process::Command::new(tool);
    if tool == "xorriso" {
        cmd.args(["-as", "mkisofs"]);
    }
    cmd.args(["-quiet", "-V", "UNATTEND", "-J", "-r", "-o"])
        .arg(&tmp)
        .arg(&src);
    let o = cmd
        .output()
        .await
        .map_err(|e| Error::System(format!("{tool}: {e}")))?;
    if !o.status.success() {
        return Err(Error::System(format!(
            "{tool} failed: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        )));
    }
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| Error::System(format!("answer disc: {e}")))?;
    }
    std::fs::rename(&tmp, out).map_err(|e| Error::System(format!("answer disc: {e}")))?;
    Ok(())
}

/// QMP keys pressed while a gaming VM boots its installer: the Windows ISO's
/// UEFI loader asks "Press any key to boot from CD or DVD" and falls through
/// to the (empty) disk without one.
pub async fn press_boot_key(disk: &Path, seconds: u32) {
    let sock = super::display::DisplayRuntime::qmp_socket(disk);
    for _ in 0..seconds {
        let _ = super::display::qmp_command(
            &sock,
            serde_json::json!({"execute": "send-key", "arguments": {"keys": [{"type": "qcode", "data": "spc"}]}}),
        )
        .await;
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

/// Does the installed guest's agent answer? (guest-ping over the VM's
/// guest-agent socket.)
pub async fn guest_agent_answers(disk: &Path) -> bool {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let sock = super::display::DisplayRuntime::qga_socket(disk);
    let fut = async {
        let mut s = tokio::net::UnixStream::connect(&sock).await.ok()?;
        let id = std::process::id() as u64 * 1000 + (chrono::Utc::now().timestamp() as u64 % 1000);
        s.write_all(
            format!("{{\"execute\":\"guest-sync\",\"arguments\":{{\"id\":{id}}}}}\n").as_bytes(),
        )
        .await
        .ok()?;
        let mut r = BufReader::new(s);
        let mut line = String::new();
        for _ in 0..8 {
            line.clear();
            r.read_line(&mut line).await.ok()?;
            if line.contains(&id.to_string()) {
                return Some(true);
            }
        }
        None
    };
    matches!(
        tokio::time::timeout(std::time::Duration::from_secs(5), fut).await,
        Ok(Some(true))
    )
}

// ------------------------------------------------------------ compat data

/// One row of `gaming-compat.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompatEntry {
    pub title: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub anti_cheat: String,
    /// `works`, `works-if`, `blocked-by-publisher` or `unknown`.
    pub vm_status: String,
    /// VM settings the game needs (`secure_boot`, `tpm`, `vbs`).
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub host_status: String,
    #[serde(default)]
    pub note: String,
    pub source: String,
    #[serde(default)]
    pub source_url: String,
    pub checked_on: String,
}

#[derive(Deserialize)]
struct CompatFile {
    #[serde(default)]
    game: Vec<CompatEntry>,
}

pub const VM_STATUSES: [&str; 4] = ["works", "works-if", "blocked-by-publisher", "unknown"];

/// Parse one compat table, checking every row.
pub fn parse_compat(text: &str) -> std::result::Result<Vec<CompatEntry>, String> {
    let f: CompatFile = toml::from_str(text).map_err(|e| e.to_string())?;
    for g in &f.game {
        if g.title.trim().is_empty() {
            return Err("a row has no title".into());
        }
        if !VM_STATUSES.contains(&g.vm_status.as_str()) {
            return Err(format!(
                "{}: vm_status {:?} is not one of {VM_STATUSES:?}",
                g.title, g.vm_status
            ));
        }
        for r in &g.requires {
            if !matches!(r.as_str(), "secure_boot" | "tpm" | "vbs") {
                return Err(format!("{}: unknown requirement {r:?}", g.title));
            }
        }
        if g.source.trim().is_empty() {
            return Err(format!("{}: no source", g.title));
        }
        if chrono::NaiveDate::parse_from_str(&g.checked_on, "%Y-%m-%d").is_err() {
            return Err(format!("{}: checked_on must be YYYY-MM-DD", g.title));
        }
    }
    Ok(f.game)
}

/// The table built into Horcrux (`data/gaming-compat.toml`).
pub const BUILTIN_COMPAT: &str = include_str!("../../data/gaming-compat.toml");

/// The built-in rows, then the configured files' rows on top: a later
/// row with the same title replaces an earlier one. Unreadable files are
/// skipped with a warning.
pub fn load_compat(files: &[PathBuf]) -> Vec<CompatEntry> {
    let mut out: Vec<CompatEntry> = parse_compat(BUILTIN_COMPAT).unwrap_or_default();
    for f in files {
        let Ok(s) = std::fs::read_to_string(f) else {
            continue;
        };
        match parse_compat(&s) {
            Ok(rows) => {
                for r in rows {
                    if let Some(i) = out
                        .iter()
                        .position(|x| x.title.eq_ignore_ascii_case(&r.title))
                    {
                        out[i] = r;
                    } else {
                        out.push(r);
                    }
                }
            }
            Err(e) => tracing::warn!("{}: {e}", f.display()),
        }
    }
    out
}

fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// The compat row for a title (exact title or alias, ignoring case and
/// punctuation).
pub fn find_game<'a>(rows: &'a [CompatEntry], query: &str) -> Option<&'a CompatEntry> {
    let q = norm(query);
    if q.is_empty() {
        return None;
    }
    rows.iter()
        .find(|r| norm(&r.title) == q || r.aliases.iter().any(|a| norm(a) == q))
}

/// A requested title and what is known about it.
#[derive(Debug, Clone, Serialize)]
pub struct CompatMatch {
    pub query: String,
    pub entry: Option<CompatEntry>,
}

pub fn match_games(rows: &[CompatEntry], titles: &[String]) -> Vec<CompatMatch> {
    titles
        .iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .map(|t| CompatMatch {
            query: t.to_string(),
            entry: find_game(rows, t).cloned(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::handoff::GpuCheck;
    use super::super::tuning::HostCpus;
    use super::*;

    fn report() -> GamingReport {
        GamingReport {
            kvm: true,
            iommu_groups: 20,
            vfio_pci_loaded: true,
            cpus: HostCpus {
                online: (0..24).collect(),
                performance: (0..8).collect(),
                efficiency: (8..24).collect(),
                ..Default::default()
            },
            suggested_vcpus: 8,
            suggested_pinning: "0-7".into(),
            hugepages: vec![(2048, 0, 0), (1048576, 0, 0)],
            gpus: vec![
                gpu("0000:00:02.0", "ready-with-warnings"),
                gpu("0000:02:00.0", "ready"),
                gpu("0000:03:00.0", "single-gpu"),
                gpu("0000:04:00.0", "blocked"),
            ],
            nested_virt: true,
            nested_virt_problem: String::new(),
            host_is_vm: false,
            secure_boot_firmware: true,
            swtpm: true,
            iso_tool: "xorriso".into(),
        }
    }

    fn gpu(addr: &str, verdict: &str) -> GpuCheck {
        GpuCheck {
            address: addr.into(),
            vendor_id: "10de".into(),
            device_id: "2c58".into(),
            vendor_name: "NVIDIA".into(),
            driver: Some("nvidia".into()),
            iommu_group: Some("14".into()),
            functions: vec![addr.into()],
            displays: vec![],
            connectors: vec![],
            has_outputs: Some(true),
            host_keeps_display: true,
            reset_methods: vec!["flr".into()],
            holders: vec![],
            quirks: vec![],
            verdict: verdict.into(),
            reasons: vec!["reason".into()],
        }
    }

    fn req() -> GamingVmRequest {
        GamingVmRequest {
            name: "Game Box".into(),
            id: Some("vm-g1".into()),
            gpu: Some("02:00.0".into()),
            windows_iso: "/var/lib/horcrux/images/win11.iso".into(),
            virtio_iso: "/var/lib/horcrux/images/virtio-win.iso".into(),
            username: "player".into(),
            password: "s3cret&<pw>".into(),
            ..Default::default()
        }
    }

    fn host() -> HostFacts {
        HostFacts {
            memory_mb: 65536,
            default_bridge: Some("vmbr0".into()),
            ..Default::default()
        }
    }

    #[test]
    fn remote_play_puts_the_card_on_the_lan_when_it_can() {
        use horcrux_common::remote_play::{RemotePlayNetwork, VmRemotePlay};
        use horcrux_common::NicAttach;
        let s = GamingSettings::default();
        let mut r = req();
        r.remote_play = Some(VmRemotePlay {
            enabled: true,
            ..Default::default()
        });
        // default network is NAT-only, a LAN bridge exists: that one
        let h = HostFacts {
            memory_mb: 65536,
            default_bridge: Some("natbr".into()),
            lan_bridge: Some("vmbr0".into()),
            macvtap_uplink: Some("enp5s0".into()),
        };
        let p = compose(&r, &report(), &h, &s, &compat()).unwrap();
        let n = &p.config.hardware.nets[0];
        assert_eq!((n.bridge.as_str(), n.attach), ("vmbr0", NicAttach::Bridge));
        assert!(p.config.hardware.adv.remote_play.as_ref().unwrap().enabled);
        // no LAN bridge: macvtap on the wired uplink
        let h2 = HostFacts {
            lan_bridge: None,
            ..h.clone()
        };
        let p = compose(&r, &report(), &h2, &s, &compat()).unwrap();
        let n = &p.config.hardware.nets[0];
        assert_eq!(
            (n.bridge.as_str(), n.attach),
            ("enp5s0", NicAttach::Macvtap)
        );
        // neither: NAT with the relay, said up front
        let h3 = HostFacts {
            lan_bridge: None,
            macvtap_uplink: None,
            ..h.clone()
        };
        let p = compose(&r, &report(), &h3, &s, &compat()).unwrap();
        assert_eq!(p.config.hardware.nets[0].bridge, "natbr");
        assert!(p.warnings.iter().any(|w| w.contains("relays discovery")));
        // the user asked for NAT, or named a network: kept
        r.remote_play.as_mut().unwrap().network = RemotePlayNetwork::Nat;
        let p = compose(&r, &report(), &h, &s, &compat()).unwrap();
        assert_eq!(p.config.hardware.nets[0].bridge, "natbr");
        r.remote_play.as_mut().unwrap().network = RemotePlayNetwork::Auto;
        r.bridge = Some("lab".into());
        let p = compose(&r, &report(), &h, &s, &compat()).unwrap();
        assert_eq!(p.config.hardware.nets[0].bridge, "lab");
        // no card at all is refused with remote play
        r.bridge = Some(String::new());
        assert!(compose(&r, &report(), &h, &s, &compat()).is_err());
        // off by default
        let p = compose(&req(), &report(), &h, &s, &compat()).unwrap();
        assert!(p.config.hardware.adv.remote_play.is_none());
    }

    fn compat() -> Vec<CompatEntry> {
        parse_compat(include_str!("../../data/gaming-compat.toml")).unwrap()
    }

    #[test]
    fn composes_an_honest_secure_modern_windows_vm() {
        let s = GamingSettings::default();
        let p = compose(&req(), &report(), &host(), &s, &compat()).unwrap();
        let c = &p.config;
        let hw = &c.hardware;
        assert_eq!(c.cpus, 8);
        assert_eq!(c.memory, 32768, "half of 64 GB, capped at 32 GB");
        assert_eq!(c.disk_size, 128);
        assert_eq!(hw.machine, MachineType::Q35);
        assert_eq!(hw.firmware, Firmware::Ovmf);
        assert!(hw.secure_boot && hw.tpm && hw.guest_agent);
        assert!(hw.adv.hyperv && hw.adv.nested_virt);
        assert_eq!(hw.adv.cpu_pinning.as_ref().unwrap().vcpus, "auto");
        assert_eq!(hw.adv.hugepages, Some(VmHugePages { size_kb: 2048 }));
        assert_eq!(hw.hostpci.len(), 1);
        assert_eq!(hw.hostpci[0].device, "0000:02:00.0");
        assert!(hw.hostpci[0].managed && !hw.hostpci[0].allow_display_loss);
        assert_eq!(
            hw.cdrom.as_deref(),
            Some("/var/lib/horcrux/images/win11.iso")
        );
        assert_eq!(
            hw.extra_cdroms,
            vec!["/var/lib/horcrux/images/virtio-win.iso"]
        );
        assert_eq!(hw.nets[0].bridge, "vmbr0");
        assert_eq!(c.disks[0].disk_type, "virtio");
        assert!(c.disks[0].iothread && c.disks[0].discard);
        // never a real vendor's identity, never hidden
        let sm = hw.adv.smbios.as_ref().unwrap();
        assert_eq!(sm.manufacturer.as_deref(), Some("Horcrux"));
        assert!(horcrux_common::valid_guid(sm.uuid.as_deref().unwrap()));
        assert!(hw.cpu_type.is_none());
        // a second VM gets its own identity
        let p2 = compose(&req(), &report(), &host(), &s, &compat()).unwrap();
        assert_ne!(p2.config.hardware.adv.smbios, hw.adv.smbios);
        assert_eq!(p.unattend.computer_name, "GAME-BOX");
        assert!(p.unattend.vbs);
        // the password never leaves the server in the plan
        let j = serde_json::to_string(&p).unwrap();
        assert!(!j.contains("s3cret"), "{j}");
    }

    #[test]
    fn gpu_choice_follows_the_check() {
        let s = GamingSettings::default();
        let mut r = req();
        r.gpu = Some("0000:04:00.0".into());
        assert!(compose(&r, &report(), &host(), &s, &[])
            .unwrap_err()
            .to_string()
            .contains("can't be handed"));
        r.gpu = Some("0000:09:00.0".into());
        assert!(compose(&r, &report(), &host(), &s, &[])
            .unwrap_err()
            .to_string()
            .contains("not a GPU on this host"));
        r.gpu = Some("0000:03:00.0".into());
        let e = compose(&r, &report(), &host(), &s, &[])
            .unwrap_err()
            .to_string();
        assert!(e.contains("only screen"), "{e}");
        r.allow_display_loss = true;
        let p = compose(&r, &report(), &host(), &s, &[]).unwrap();
        assert!(p.config.hardware.hostpci[0].allow_display_loss);
        assert!(p.warnings.iter().any(|w| w.contains("single-GPU")));
        r.gpu = None;
        let p = compose(&r, &report(), &host(), &s, &[]).unwrap();
        assert!(p.config.hardware.hostpci.is_empty());
        assert!(p.warnings.iter().any(|w| w.contains("no GPU chosen")));
        // a GPU without outputs: the viewer path is explained
        let mut rep = report();
        rep.gpus[1].has_outputs = Some(false);
        r.gpu = Some("02:00.0".into());
        let p = compose(&r, &rep, &host(), &s, &[]).unwrap();
        assert!(p.notes.iter().any(|n| n.contains("no display outputs")));
    }

    #[test]
    fn host_limits_and_missing_pieces_are_explained() {
        let s = GamingSettings::default();
        let mut rep = report();
        rep.nested_virt = false;
        rep.nested_virt_problem = "nested is off".into();
        let p = compose(&req(), &rep, &host(), &s, &[]).unwrap();
        assert!(!p.config.hardware.adv.nested_virt && !p.unattend.vbs);
        assert!(p.warnings.iter().any(|w| w.contains("VBS")));
        let mut r = req();
        r.vbs = Some(true);
        assert!(compose(&r, &rep, &host(), &s, &[]).is_err());
        rep = report();
        rep.swtpm = false;
        assert!(compose(&req(), &rep, &host(), &s, &[])
            .unwrap_err()
            .to_string()
            .contains("swtpm"));
        rep = report();
        rep.secure_boot_firmware = false;
        assert!(compose(&req(), &rep, &host(), &s, &[])
            .unwrap_err()
            .to_string()
            .contains("OVMF"));
        let mut r = req();
        r.memory_mb = Some(2048);
        assert!(compose(&r, &report(), &host(), &s, &[]).is_err());
        r.memory_mb = Some(65000);
        assert!(compose(&r, &report(), &host(), &s, &[]).is_err());
        r = req();
        r.vcpus = Some(12);
        let e = compose(&r, &report(), &host(), &s, &[])
            .unwrap_err()
            .to_string();
        assert!(e.contains("performance cores"), "{e}");
        r = req();
        r.username = "Administrator".into();
        assert!(compose(&r, &report(), &host(), &s, &[]).is_err());
        r.username = "a/b".into();
        assert!(compose(&r, &report(), &host(), &s, &[]).is_err());
        r = req();
        r.disk_gb = Some(32);
        assert!(compose(&r, &report(), &host(), &s, &[]).is_err());
        r = req();
        r.bridge = Some(String::new());
        assert!(compose(&r, &report(), &host(), &s, &[])
            .unwrap()
            .config
            .hardware
            .nets
            .is_empty());
        r = req();
        r.hugepages = Some(false);
        assert!(compose(&r, &report(), &host(), &s, &[])
            .unwrap()
            .config
            .hardware
            .adv
            .hugepages
            .is_none());
    }

    #[test]
    fn games_are_checked_before_anything_is_created() {
        let s = GamingSettings::default();
        let mut r = req();
        r.games = vec![
            "valorant".into(),
            "CS2".into(),
            "Some Indie Game".into(),
            "ARC Raiders".into(),
        ];
        let p = compose(&r, &report(), &host(), &s, &compat()).unwrap();
        assert!(p
            .warnings
            .iter()
            .any(|w| w.starts_with("Valorant: blocked in VMs")));
        assert!(p
            .warnings
            .iter()
            .any(|w| w.contains("ARC Raiders") && w.contains("through Steam")));
        assert!(p
            .warnings
            .iter()
            .any(|w| w.starts_with("Some Indie Game: not in the compatibility list")));
        assert!(p.notes.iter().any(|n| n.starts_with("Counter-Strike 2")));
        assert_eq!(p.games.len(), 4);
        assert!(p.games[2].entry.is_none());
    }

    #[test]
    fn the_shipped_compat_table_is_valid_and_complete() {
        let rows = compat();
        assert!(rows.len() >= 20);
        for r in &rows {
            assert!(
                !r.source_url.is_empty() && r.source_url.starts_with("https://"),
                "{}",
                r.title
            );
        }
        assert_eq!(find_game(&rows, "warzone").unwrap().anti_cheat, "RICOCHET");
        assert_eq!(
            find_game(&rows, "Counter Strike 2").unwrap().vm_status,
            "works"
        );
        assert!(parse_compat("[[game]]\ntitle='x'\nanti_cheat='y'\nvm_status='maybe'\nsource='s'\nchecked_on='2026-01-01'\n").is_err());
        assert!(parse_compat("[[game]]\ntitle='x'\nanti_cheat='y'\nvm_status='works'\nsource='s'\nchecked_on='soon'\n").is_err());
    }

    #[test]
    fn local_rows_override_shipped_ones() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("a.toml");
        let b = t.path().join("b.toml");
        std::fs::write(&a, include_str!("../../data/gaming-compat.toml")).unwrap();
        std::fs::write(
            &b,
            "[[game]]\ntitle='Valorant'\nanti_cheat='Riot Vanguard'\nvm_status='works'\nsource='our test'\nchecked_on='2026-12-01'\n",
        )
        .unwrap();
        let rows = load_compat(&[a.clone(), b, t.path().join("missing.toml")]);
        assert_eq!(rows.len(), load_compat(&[a]).len());
        assert_eq!(
            rows.len(),
            load_compat(&[]).len(),
            "built-in table without files"
        );
        assert_eq!(find_game(&rows, "valorant").unwrap().vm_status, "works");
        assert_eq!(
            find_game(&load_compat(&[]), "valorant").unwrap().vm_status,
            "blocked-by-publisher"
        );
    }

    #[test]
    fn answer_file_carries_the_choices_and_escapes_them() {
        let s = GamingSettings::default();
        let mut r = req();
        r.product_key = Some("aaaaa-bbbbb-ccccc-ddddd-eeeee".into());
        r.image_name = Some("Windows 11 Pro".into());
        r.timezone = Some("Pacific Standard Time".into());
        let p = compose(&r, &report(), &host(), &s, &[]).unwrap();
        let x = unattend_xml(&p.unattend);
        assert!(x.contains("<ComputerName>GAME-BOX</ComputerName>"));
        assert!(x.contains("<Value>s3cret&amp;&lt;pw&gt;</Value>"));
        assert!(!x.contains("s3cret&<"));
        assert!(x.contains("<Key>AAAAA-BBBBB-CCCCC-DDDDD-EEEEE</Key>"));
        assert!(x.contains("<Key>/IMAGE/NAME</Key><Value>Windows 11 Pro</Value>"));
        assert!(x.contains("<TimeZone>Pacific Standard Time</TimeZone>"));
        assert!(x.contains(r"D:\viostor\w11\amd64") && x.contains(r"H:\NetKVM\w11\amd64"));
        assert!(x.contains("EnableVirtualizationBasedSecurity"));
        assert!(x.contains("HypervisorEnforcedCodeIntegrity"));
        assert!(x.contains("qemu-ga-x86_64.msi") && x.contains("powercfg /h off"));
        // OOBE needs International-Core or it stops at the region page
        assert!(x.contains("Microsoft-Windows-International-Core\""));
        // without key/edition: image index, no ProductKey element
        let p = compose(&req(), &report(), &host(), &s, &[]).unwrap();
        let x = unattend_xml(&p.unattend);
        assert!(
            x.contains("<Key>/IMAGE/INDEX</Key><Value>1</Value>") && !x.contains("<ProductKey>")
        );
        // well-formed: every opened element is closed
        for tag in [
            "settings",
            "component",
            "unattend",
            "FirstLogonCommands",
            "DriverPaths",
        ] {
            assert_eq!(
                x.matches(&format!("<{tag}")).count(),
                x.matches(&format!("</{tag}>")).count(),
                "{tag}"
            );
        }
        let mut r = req();
        r.vbs = Some(false);
        let x = unattend_xml(&compose(&r, &report(), &host(), &s, &[]).unwrap().unattend);
        assert!(!x.contains("DeviceGuard"));
    }

    #[test]
    fn computer_names_are_valid_windows_names() {
        assert_eq!(computer_name("Game Box"), "GAME-BOX");
        assert_eq!(
            computer_name("my_very_long_gaming_vm_name"),
            "MY-VERY-LONG-GA"
        );
        assert_eq!(computer_name("1234"), "HX-1234");
        assert_eq!(computer_name("---"), "HX-");
    }

    #[test]
    fn settings_are_validated() {
        let mut s = GamingSettings::default();
        assert!(s.validate().is_ok());
        s.virtio_win_url = "http://x".into();
        assert!(s.validate().is_err());
        s = GamingSettings::default();
        s.hugepages_kb = 4;
        assert!(s.validate().is_err());
        s = GamingSettings::default();
        s.smbios_manufacturer = String::new();
        assert!(s.validate().is_err());
        s = GamingSettings::default();
        s.driver_search_letters = "C".into();
        assert!(s.validate().is_err());
    }

    #[tokio::test]
    async fn the_answer_disc_is_a_real_iso_with_the_file() {
        if super::super::handoff::iso_tool().is_none() {
            eprintln!("no ISO tool on this host; skipped");
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let out = t.path().join("vm.unattend.iso");
        build_unattend_iso(&out, "<unattend/>").await.unwrap();
        let bytes = std::fs::read(&out).unwrap();
        // ISO 9660 primary volume descriptor at sector 16
        assert_eq!(&bytes[0x8001..0x8006], b"CD001");
        assert!(bytes.windows(8).any(|w| w == b"UNATTEND"));
        assert!(bytes.windows(11).any(|w| w == b"<unattend/>"));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&out).unwrap().permissions().mode() & 0o777,
            0o600
        );
        // staging dir cleaned up
        assert_eq!(std::fs::read_dir(t.path()).unwrap().count(), 1);
    }

    #[test]
    fn vbs_stays_off_by_default_when_the_host_is_itself_a_vm() {
        let mut rep = report();
        rep.host_is_vm = true;
        let s = GamingSettings::default();
        let p = compose(&req(), &rep, &host(), &s, &[]).unwrap();
        assert!(!p.config.hardware.adv.nested_virt && !p.unattend.vbs);
        assert!(
            p.warnings.iter().any(|w| w.contains("third level")),
            "{:?}",
            p.warnings
        );
        // asked for explicitly: on, with the warning
        let mut r = req();
        r.vbs = Some(true);
        let p = compose(&r, &rep, &host(), &s, &[]).unwrap();
        assert!(p.config.hardware.adv.nested_virt && p.unattend.vbs);
        assert!(p.warnings.iter().any(|w| w.contains("third level")));
        // the node setting turns the default back on
        let s = GamingSettings {
            vbs_on_virtual_host: true,
            ..GamingSettings::default()
        };
        let p = compose(&req(), &rep, &host(), &s, &[]).unwrap();
        assert!(p.config.hardware.adv.nested_virt);
    }
}
