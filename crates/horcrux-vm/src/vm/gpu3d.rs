//! 3D virtio-gpu (Venus Vulkan + virgl OpenGL) for VMs: host check, render
//! node selection and start-time preparation (docs/design/hx-vgpu.md, B1).
//!
//! A VM whose video card is `virtio-3d` runs QEMU with
//! `-device virtio-vga-gl,...` and `-display egl-headless,rendernode=...`
//! on top of a memfd-backed guest RAM. The guest sees a virtio-gpu whose
//! Vulkan (Venus) and GL (virgl) commands the host's GPU drivers execute.
//!
//! What the host needs, and what this module tells the UI when it is missing:
//! * QEMU with the `virtio-vga-gl` device and the `egl-headless` display
//!   (built with USE `opengl virgl`),
//! * virglrenderer built with Venus (QEMU then offers the `venus` property),
//! * a GPU render node whose Vulkan driver exports dma-bufs
//!   (`VK_EXT_external_memory_dma_buf`); software Vulkan (lavapipe) cannot.
//!
//! Every path and limit comes from the node's `[vgpu]` settings
//! (`horcrux_core::config::VgpuConfig`).
use horcrux_common::{Error, Gpu3d, Result, VmDisplay};
use horcrux_core::config::VgpuConfig;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Name of the memfd memory backend that backs a 3D VM's RAM (the GPU
/// process maps guest memory, so it must be shareable).
pub const MEM_BACKEND_ID: &str = "hx3dmem";

/// What a started 3D VM got (stored in the VM's display state).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Runtime3d {
    pub render_node: String,
    pub hostmem_mib: u32,
    pub venus: bool,
    pub virgl: bool,
    /// `vendor:device` (hex) handed to Mesa's device selection so the
    /// Vulkan renderer uses the GPU behind `render_node`.
    pub device_select: Option<String>,
}

/// One thing the host lacks, with what to do about it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Issue {
    /// Stable code (`qemu-no-3d-device`, `qemu-no-egl-headless`,
    /// `qemu-no-venus`, `no-render-node`, `no-suitable-gpu`).
    pub reason: String,
    pub message: String,
    pub hint: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct QemuCaps {
    pub binary: String,
    pub found: bool,
    pub version: Option<String>,
    /// The `virtio-vga-gl` device exists.
    pub virtio_vga_gl: bool,
    /// The `egl-headless` display exists.
    pub egl_headless: bool,
    /// The device has the `venus` property (QEMU + virglrenderer with Venus).
    pub venus: bool,
    /// The device has the `blob` / `hostmem` properties Venus needs.
    pub blob: bool,
    pub hostmem: bool,
    /// The device has the `virgl` switch.
    pub virgl_switch: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RenderNode {
    /// `/dev/dri/renderD128`
    pub path: String,
    pub pci_address: Option<String>,
    pub vendor_id: Option<String>,
    pub device_id: Option<String>,
    /// Kernel driver (`i915`, `xe`, `amdgpu`, `nvidia`, ...).
    pub driver: Option<String>,
    /// Vulkan device name, when `vulkaninfo` could be matched to this GPU.
    pub name: Option<String>,
    /// The GPU's Vulkan driver exports dma-bufs: `None` = not verified
    /// (no `vulkaninfo`, or the GPU did not match).
    pub dmabuf_export: Option<bool>,
    /// Allowed by the node's `[vgpu] render_nodes` list.
    pub allowed: bool,
    /// Usable for a Venus VM now.
    pub suitable: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Host3d {
    /// A 3D VM can be started on this host now.
    pub ready: bool,
    pub qemu: QemuCaps,
    pub gpus: Vec<RenderNode>,
    /// The GPU `auto` would pick.
    pub default_node: Option<String>,
    pub default_hostmem_mib: u32,
    pub issues: Vec<Issue>,
    /// Soft findings (e.g. dma-bufs not verified).
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------- sysfs

fn read_trim(p: &Path) -> Option<String> {
    std::fs::read_to_string(p)
        .ok()
        .map(|s| s.trim().to_string())
}

/// Render nodes of the host with their GPU identity, from sysfs only.
pub fn list_render_nodes(cfg: &VgpuConfig) -> Vec<RenderNode> {
    let mut names: Vec<String> = std::fs::read_dir(&cfg.dri_dir)
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| n.starts_with("renderD"))
                .collect()
        })
        .unwrap_or_default();
    names.sort_by_key(|n| {
        n.trim_start_matches("renderD")
            .parse::<u32>()
            .unwrap_or(u32::MAX)
    });
    names
        .into_iter()
        .map(|n| {
            let dev = cfg.drm_sysfs_dir.join(&n).join("device");
            let pci = std::fs::read_link(&dev)
                .ok()
                .and_then(|l| l.file_name().map(|f| f.to_string_lossy().to_string()))
                .filter(|a| a.contains(':'));
            let driver = std::fs::read_link(dev.join("driver"))
                .ok()
                .and_then(|l| l.file_name().map(|f| f.to_string_lossy().to_string()));
            let hex = |f: &str| {
                read_trim(&dev.join(f)).map(|v| v.trim_start_matches("0x").to_lowercase())
            };
            let path = cfg.dri_dir.join(&n).to_string_lossy().to_string();
            let allowed = cfg.render_nodes.is_empty()
                || cfg
                    .render_nodes
                    .iter()
                    .any(|a| a == &path || Some(a.as_str()) == pci.as_deref());
            let ignored = driver
                .as_deref()
                .is_some_and(|d| cfg.ignore_drivers.iter().any(|i| i == d));
            RenderNode {
                path,
                pci_address: pci,
                vendor_id: hex("vendor"),
                device_id: hex("device"),
                driver,
                name: None,
                dmabuf_export: None,
                allowed,
                suitable: allowed && !ignored,
                reason: if ignored {
                    Some("software or virtual device, not a GPU".into())
                } else if !allowed {
                    Some("not in the node's [vgpu] render_nodes list".into())
                } else {
                    None
                },
            }
        })
        .collect()
}

// ------------------------------------------------------------ vulkaninfo

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VkGpu {
    pub vendor_id: String,
    pub device_id: String,
    pub name: String,
    pub dmabuf: bool,
    pub cpu: bool,
}

/// Parse `vulkaninfo` (full output): per GPU block the ids, the name, the
/// device type and whether `VK_EXT_external_memory_dma_buf` is offered.
pub fn parse_vulkaninfo(text: &str) -> Vec<VkGpu> {
    let mut out: Vec<VkGpu> = Vec::new();
    let mut cur: Option<VkGpu> = None;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("GPU")
            && t.ends_with(':')
            && t[3..t.len() - 1].chars().all(|c| c.is_ascii_digit())
        {
            if let Some(g) = cur.take() {
                out.push(g);
            }
            cur = Some(VkGpu {
                vendor_id: String::new(),
                device_id: String::new(),
                name: String::new(),
                dmabuf: false,
                cpu: false,
            });
            continue;
        }
        let Some(g) = cur.as_mut() else { continue };
        let val = |k: &str| {
            t.strip_prefix(k)
                .map(|r| r.trim_start().trim_start_matches('=').trim().to_string())
        };
        if g.vendor_id.is_empty() {
            if let Some(v) = val("vendorID") {
                g.vendor_id = v.trim_start_matches("0x").to_lowercase();
            }
        }
        if g.device_id.is_empty() {
            if let Some(v) = val("deviceID") {
                g.device_id = v.trim_start_matches("0x").to_lowercase();
            }
        }
        if g.name.is_empty() {
            if let Some(v) = val("deviceName") {
                g.name = v;
            }
        }
        if let Some(v) = val("deviceType") {
            g.cpu = v.contains("CPU");
        }
        if t.starts_with("VK_EXT_external_memory_dma_buf") {
            g.dmabuf = true;
        }
    }
    if let Some(g) = cur {
        out.push(g);
    }
    out.retain(|g| !g.vendor_id.is_empty());
    out
}

/// Fill the Vulkan facts into the render nodes (matched by PCI ids).
pub fn apply_vulkan(nodes: &mut [RenderNode], vk: &[VkGpu]) {
    for n in nodes.iter_mut() {
        let (Some(v), Some(d)) = (&n.vendor_id, &n.device_id) else {
            continue;
        };
        let pad = |s: &str| s.trim_start_matches('0').to_string();
        let hit = vk
            .iter()
            .find(|g| pad(&g.vendor_id) == pad(v) && pad(&g.device_id) == pad(d) && !g.cpu);
        if let Some(g) = hit {
            n.name = Some(g.name.clone());
            n.dmabuf_export = Some(g.dmabuf);
            if !g.dmabuf && n.suitable {
                n.suitable = false;
                n.reason = Some(
                    "its Vulkan driver cannot export dma-bufs (VK_EXT_external_memory_dma_buf): Venus needs it"
                        .into(),
                );
            }
        } else if n.suitable && !vk.is_empty() {
            // vulkaninfo ran but has no hardware Vulkan device for this GPU.
            n.dmabuf_export = Some(false);
            n.suitable = false;
            n.reason = Some(
                "no hardware Vulkan driver for this GPU (install its Mesa/vendor Vulkan driver)"
                    .into(),
            );
        }
    }
}

// ----------------------------------------------------------- QEMU probe

/// Properties of the device's `-device X,help` listing.
pub fn parse_device_help(text: &str) -> (bool, bool, bool, bool) {
    let has = |p: &str| {
        text.lines().any(|l| {
            let l = l.trim_start();
            l.strip_prefix(p).is_some_and(|r| r.starts_with('='))
        })
    };
    (has("venus"), has("blob"), has("hostmem"), has("virgl"))
}

async fn run(program: &str, args: &[&str], secs: u64) -> Option<String> {
    let fut = tokio::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output();
    let out = tokio::time::timeout(Duration::from_secs(secs.max(1)), fut)
        .await
        .ok()?
        .ok()?;
    let mut s = String::from_utf8_lossy(&out.stdout).to_string();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    Some(s)
}

async fn qemu_caps(binary: &str, secs: u64) -> QemuCaps {
    let mut c = QemuCaps {
        binary: binary.to_string(),
        ..Default::default()
    };
    let Some(ver) = run(binary, &["--version"], secs).await else {
        return c;
    };
    c.found = true;
    c.version = ver
        .split_whitespace()
        .skip_while(|w| *w != "version")
        .nth(1)
        .map(|s| s.to_string());
    let devices = run(binary, &["-device", "help"], secs)
        .await
        .unwrap_or_default();
    c.virtio_vga_gl = devices.contains("\"virtio-vga-gl\"");
    let displays = run(binary, &["-display", "help"], secs)
        .await
        .unwrap_or_default();
    c.egl_headless = displays.lines().any(|l| l.trim() == "egl-headless");
    if c.virtio_vga_gl {
        let help = run(binary, &["-device", "virtio-vga-gl,help"], secs)
            .await
            .unwrap_or_default();
        let (venus, blob, hostmem, virgl) = parse_device_help(&help);
        c.venus = venus;
        c.blob = blob;
        c.hostmem = hostmem;
        c.virgl_switch = virgl;
    }
    c
}

/// Everything the UI needs to explain what this host can do.
pub fn assess(
    cfg: &VgpuConfig,
    qemu: QemuCaps,
    mut gpus: Vec<RenderNode>,
    vk_checked: bool,
) -> Host3d {
    let mut issues = Vec::new();
    let mut warnings = Vec::new();
    if !qemu.found {
        issues.push(Issue {
            reason: "qemu-missing".into(),
            message: format!("{} was not found", qemu.binary),
            hint: "Install QEMU (app-emulation/qemu).".into(),
        });
    } else {
        if !qemu.virtio_vga_gl {
            issues.push(Issue {
                reason: "qemu-no-3d-device".into(),
                message: "This QEMU has no 3D virtio-gpu device (virtio-vga-gl).".into(),
                hint: "Rebuild app-emulation/qemu with USE=\"opengl virgl\" (Horcrux images ship it that way).".into(),
            });
        }
        if !qemu.egl_headless {
            issues.push(Issue {
                reason: "qemu-no-egl-headless".into(),
                message: "This QEMU has no egl-headless display, which renders the 3D guest without a local screen.".into(),
                hint: "Rebuild app-emulation/qemu with USE=\"opengl\" (needs Mesa with EGL/GBM).".into(),
            });
        }
        if qemu.virtio_vga_gl && !(qemu.venus && qemu.blob && qemu.hostmem) {
            issues.push(Issue {
                reason: "qemu-no-venus".into(),
                message: "Vulkan (Venus) is not available: the virtio-gpu device lacks the venus/blob/hostmem options.".into(),
                hint: "Use QEMU 9.2 or newer built against media-libs/virglrenderer with USE=venus; OpenGL-only (virgl) still works.".into(),
            });
        }
    }
    if gpus.is_empty() {
        issues.push(Issue {
            reason: "no-render-node".into(),
            message: format!("No GPU render node in {}.", cfg.dri_dir.display()),
            hint: "The host needs a GPU with a kernel DRM driver (i915, xe, amdgpu, nouveau, nvidia-drm).".into(),
        });
    } else if !gpus.iter().any(|g| g.suitable) {
        issues.push(Issue {
            reason: "no-suitable-gpu".into(),
            message: "No GPU on this host can serve a 3D guest.".into(),
            hint: gpus
                .iter()
                .filter_map(|g| g.reason.as_ref().map(|r| format!("{}: {r}", g.path)))
                .collect::<Vec<_>>()
                .join("; "),
        });
    }
    if !vk_checked && !gpus.is_empty() {
        warnings.push(format!(
            "{} is not installed, so the GPU's Vulkan driver was not checked for dma-buf export; a Venus guest fails at its first allocation when it cannot.",
            cfg.vulkaninfo
        ));
    }
    let default_node = gpus.iter().find(|g| g.suitable).map(|g| g.path.clone());
    for g in gpus.iter_mut() {
        if g.suitable && g.reason.is_none() && g.dmabuf_export.is_none() {
            g.reason = Some("dma-buf export not verified".into());
        }
    }
    Host3d {
        ready: issues.is_empty(),
        qemu,
        gpus,
        default_node,
        default_hostmem_mib: cfg.hostmem_mib,
        issues,
        warnings,
    }
}

static CACHE: Mutex<Option<(Instant, Host3d)>> = Mutex::new(None);

/// Probe this host (cached for `[vgpu] probe_cache_secs`; the commands take a moment).
pub async fn probe(cfg: &VgpuConfig) -> Host3d {
    if let Some((at, h)) = CACHE.lock().ok().and_then(|g| g.clone()) {
        if at.elapsed() < Duration::from_secs(cfg.probe_cache_secs) {
            return h;
        }
    }
    let secs = cfg.probe_timeout_secs;
    let qemu = qemu_caps("qemu-system-x86_64", secs).await;
    let mut gpus = list_render_nodes(cfg);
    let vk = run(&cfg.vulkaninfo, &[], secs).await;
    let checked = vk
        .as_deref()
        .is_some_and(|t| t.contains("GPU0:") || t.contains("GPU id"));
    if let Some(t) = &vk {
        apply_vulkan(&mut gpus, &parse_vulkaninfo(t));
    }
    let h = assess(cfg, qemu, gpus, checked);
    if let Ok(mut g) = CACHE.lock() {
        *g = Some((Instant::now(), h.clone()));
    }
    h
}

// -------------------------------------------------------------- selection

/// The GPU a VM uses: the one it names (path or PCI address) or the first
/// suitable one.
pub fn select_node<'a>(nodes: &'a [RenderNode], want: Option<&str>) -> Result<&'a RenderNode> {
    match want {
        Some(w) => {
            let n = nodes
                .iter()
                .find(|n| n.path == w || n.pci_address.as_deref() == Some(w))
                .ok_or_else(|| {
                    Error::Validation(format!(
                        "the VM's 3D GPU {w} is not a render node of this host"
                    ))
                })?;
            if !n.suitable {
                return Err(Error::Validation(format!(
                    "GPU {w} cannot serve a 3D guest: {}",
                    n.reason.clone().unwrap_or_else(|| "not suitable".into())
                )));
            }
            Ok(n)
        }
        None => nodes.iter().find(|n| n.suitable).ok_or_else(|| {
            Error::Validation("this host has no GPU that can serve a 3D guest".into())
        }),
    }
}

/// Mesa `MESA_VK_DEVICE_SELECT` value for a render node.
pub fn device_select(n: &RenderNode) -> Option<String> {
    match (&n.vendor_id, &n.device_id) {
        (Some(v), Some(d)) => Some(format!("{v}:{d}")),
        _ => None,
    }
}

/// Settings the VM's hardware must not combine with a 3D guest.
pub fn check_compat(
    adv: &horcrux_common::VmAdvanced,
    display: &VmDisplay,
) -> std::result::Result<(), String> {
    if !display.uses_3d() {
        return Ok(());
    }
    if adv.numa || adv.hugepages.is_some() || adv.max_memory_mb.is_some() {
        return Err(
            "A 3D virtio-gpu VM cannot use NUMA nodes, huge pages or memory hot-plug yet: its RAM is a shared memfd the GPU process maps"
                .into(),
        );
    }
    Ok(())
}

/// Start-time preparation: check the host, pick the GPU, record what the
/// VM got.
pub async fn prepare(display: &VmDisplay) -> Result<Runtime3d> {
    let cfg = horcrux_core::config::HorcruxConfig::load().vgpu;
    plan(&cfg, &display.gpu3d, &probe(&cfg).await)
}

/// Pure part of `prepare` (tested).
pub fn plan(cfg: &VgpuConfig, g: &Gpu3d, host: &Host3d) -> Result<Runtime3d> {
    // virgl-only guests need no Venus pieces; a Venus guest needs all of them.
    let blocking: Vec<&Issue> = host
        .issues
        .iter()
        .filter(|i| !(i.reason == "qemu-no-venus" && !g.venus))
        .collect();
    if let Some(i) = blocking.first() {
        return Err(Error::NodeLacks {
            reason: match i.reason.as_str() {
                "qemu-missing" => "qemu-missing",
                "qemu-no-3d-device" => "qemu-no-3d-device",
                "qemu-no-egl-headless" => "qemu-no-egl-headless",
                "qemu-no-venus" => "qemu-no-venus",
                "no-render-node" => "no-render-node",
                _ => "no-suitable-gpu",
            },
            message: i.message.clone(),
            hint: i.hint.clone(),
        });
    }
    let node = select_node(&host.gpus, g.render_node.as_deref())?;
    Ok(Runtime3d {
        render_node: node.path.clone(),
        hostmem_mib: g.hostmem_mib.unwrap_or(cfg.hostmem_mib),
        venus: g.venus,
        // Newer QEMU has no switch: the 3D device always offers virgl.
        virgl: g.virgl || !host.qemu.virgl_switch,
        device_select: device_select(node),
    })
}

/// QEMU arguments of the 3D device (replaces the VGA card).
pub fn device_args(rt: &Runtime3d) -> Vec<String> {
    let mut props = String::from("virtio-vga-gl");
    if rt.venus {
        props.push_str(&format!(
            ",hostmem={}M,blob=true,venus=true",
            rt.hostmem_mib
        ));
    }
    if !rt.virgl {
        props.push_str(",virgl=off");
    }
    vec!["-vga".into(), "none".into(), "-device".into(), props]
}

/// `-display` argument (egl-headless on the selected render node).
pub fn display_arg(rt: &Runtime3d) -> String {
    format!("egl-headless,rendernode={}", rt.render_node)
}

/// Environment the QEMU process gets.
pub fn env(rt: &Runtime3d) -> Vec<(String, String)> {
    rt.device_select
        .iter()
        .map(|d| ("MESA_VK_DEVICE_SELECT".to_string(), d.clone()))
        .collect()
}

/// Why a VM with a 3D virtio-gpu cannot be saved or migrated.
pub const NOT_SAVEABLE: &str = "QEMU cannot save 3D virtio-gpu state; guest hibernation is refused by the guest driver; will shut down cleanly";

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> VgpuConfig {
        VgpuConfig::default()
    }

    fn node(path: &str, pci: &str, v: &str, d: &str, drv: &str) -> RenderNode {
        RenderNode {
            path: path.into(),
            pci_address: Some(pci.into()),
            vendor_id: Some(v.into()),
            device_id: Some(d.into()),
            driver: Some(drv.into()),
            name: None,
            dmabuf_export: None,
            allowed: true,
            suitable: true,
            reason: None,
        }
    }

    const VKINFO: &str = "\
Devices:
========
GPU0:
VkPhysicalDeviceProperties:
---------------------------
\tapiVersion     = 1.4.318 (4210878)
\tvendorID       = 0x8086
\tdeviceID       = 0x46a6
\tdeviceType     = PHYSICAL_DEVICE_TYPE_INTEGRATED_GPU
\tdeviceName     = Intel(R) Iris(R) Xe Graphics (ADL GT2)
Device Extensions: count = 3
\tVK_EXT_external_memory_dma_buf         : extension revision 1
\tVK_KHR_swapchain                       : extension revision 70
GPU1:
VkPhysicalDeviceProperties:
\tvendorID       = 0x10005
\tdeviceID       = 0x0000
\tdeviceType     = PHYSICAL_DEVICE_TYPE_CPU
\tdeviceName     = llvmpipe (LLVM 21.1.0, 256 bits)
Device Extensions: count = 1
\tVK_EXT_external_memory_dma_buf         : extension revision 1
";

    #[test]
    fn vulkaninfo_gpus_are_parsed() {
        let g = parse_vulkaninfo(VKINFO);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].vendor_id, "8086");
        assert!(g[0].dmabuf && !g[0].cpu);
        assert!(g[1].cpu);
    }

    #[test]
    fn dmabuf_less_gpu_is_not_suitable_and_unmatched_is_refused() {
        let mut nodes = vec![
            node(
                "/dev/dri/renderD128",
                "0000:00:02.0",
                "8086",
                "46a6",
                "i915",
            ),
            node(
                "/dev/dri/renderD129",
                "0000:01:00.0",
                "10de",
                "2684",
                "nvidia",
            ),
        ];
        apply_vulkan(&mut nodes, &parse_vulkaninfo(VKINFO));
        assert!(nodes[0].suitable && nodes[0].dmabuf_export == Some(true));
        assert_eq!(
            nodes[0].name.as_deref(),
            Some("Intel(R) Iris(R) Xe Graphics (ADL GT2)")
        );
        assert!(!nodes[1].suitable);
        assert!(nodes[1]
            .reason
            .as_deref()
            .unwrap()
            .contains("no hardware Vulkan driver"));
        let no_ext = VKINFO.replace(
            "VK_EXT_external_memory_dma_buf         : extension revision 1\n\tVK_KHR_swapchain",
            "VK_KHR_swapchain",
        );
        let mut n2 = vec![node(
            "/dev/dri/renderD128",
            "0000:00:02.0",
            "8086",
            "46a6",
            "i915",
        )];
        apply_vulkan(&mut n2, &parse_vulkaninfo(&no_ext));
        assert!(!n2[0].suitable);
        assert!(n2[0].reason.as_deref().unwrap().contains("dma-buf"));
    }

    #[test]
    fn device_help_properties() {
        let h = "virtio-vga-gl options:\n  blob=<bool>\n  hostmem=<size>\n  venus=<bool>\n  virgl=<bool>\n  xres=<uint32>\n";
        assert_eq!(parse_device_help(h), (true, true, true, true));
        assert_eq!(
            parse_device_help("  xres=<uint32>\n"),
            (false, false, false, false)
        );
    }

    fn full_qemu() -> QemuCaps {
        QemuCaps {
            binary: "qemu-system-x86_64".into(),
            found: true,
            version: Some("11.1.3".into()),
            virtio_vga_gl: true,
            egl_headless: true,
            venus: true,
            blob: true,
            hostmem: true,
            virgl_switch: true,
        }
    }

    #[test]
    fn host_issues_name_what_is_missing() {
        let mut nodes = vec![node(
            "/dev/dri/renderD128",
            "0000:00:02.0",
            "8086",
            "46a6",
            "i915",
        )];
        apply_vulkan(&mut nodes, &parse_vulkaninfo(VKINFO));
        let ok = assess(&cfg(), full_qemu(), nodes.clone(), true);
        assert!(ok.ready && ok.issues.is_empty());
        assert_eq!(ok.default_node.as_deref(), Some("/dev/dri/renderD128"));
        let mut q = full_qemu();
        q.virtio_vga_gl = false;
        q.egl_headless = false;
        let bad = assess(&cfg(), q, nodes.clone(), true);
        let codes: Vec<_> = bad.issues.iter().map(|i| i.reason.as_str()).collect();
        assert!(codes.contains(&"qemu-no-3d-device") && codes.contains(&"qemu-no-egl-headless"));
        assert!(!bad.ready);
        let none = assess(&cfg(), full_qemu(), vec![], true);
        assert_eq!(none.issues[0].reason, "no-render-node");
        // software-only host: only a dma-buf-less device
        let mut sw = vec![node(
            "/dev/dri/renderD128",
            "0000:00:02.0",
            "8086",
            "46a6",
            "i915",
        )];
        apply_vulkan(
            &mut sw,
            &parse_vulkaninfo(&VKINFO.replace(
                "VK_EXT_external_memory_dma_buf         : extension revision 1\n\tVK_KHR_swapchain",
                "VK_KHR_swapchain",
            )),
        );
        let s = assess(&cfg(), full_qemu(), sw, true);
        assert_eq!(s.issues[0].reason, "no-suitable-gpu");
        let unverified = assess(&cfg(), full_qemu(), nodes, false);
        assert!(unverified.ready && !unverified.warnings.is_empty());
    }

    #[test]
    fn selection_auto_and_named() {
        let mut a = node(
            "/dev/dri/renderD128",
            "0000:00:02.0",
            "8086",
            "46a6",
            "i915",
        );
        let b = node(
            "/dev/dri/renderD129",
            "0000:03:00.0",
            "1002",
            "73bf",
            "amdgpu",
        );
        a.suitable = false;
        a.reason = Some("x".into());
        let nodes = vec![a, b];
        assert_eq!(
            select_node(&nodes, None).unwrap().path,
            "/dev/dri/renderD129"
        );
        assert_eq!(
            select_node(&nodes, Some("0000:03:00.0")).unwrap().path,
            "/dev/dri/renderD129"
        );
        assert!(select_node(&nodes, Some("/dev/dri/renderD128")).is_err());
        assert!(select_node(&nodes, Some("/dev/dri/renderD200")).is_err());
    }

    #[test]
    fn plan_uses_config_defaults_and_refuses_missing_pieces() {
        let mut nodes = vec![node(
            "/dev/dri/renderD128",
            "0000:00:02.0",
            "8086",
            "46a6",
            "i915",
        )];
        apply_vulkan(&mut nodes, &parse_vulkaninfo(VKINFO));
        let host = assess(&cfg(), full_qemu(), nodes.clone(), true);
        let rt = plan(&cfg(), &Gpu3d::default(), &host).unwrap();
        assert_eq!(rt.hostmem_mib, 2048);
        assert_eq!(rt.device_select.as_deref(), Some("8086:46a6"));
        let rt2 = plan(
            &cfg(),
            &Gpu3d {
                hostmem_mib: Some(512),
                ..Default::default()
            },
            &host,
        )
        .unwrap();
        assert_eq!(rt2.hostmem_mib, 512);
        // no Venus in QEMU: a Venus guest is refused, a virgl-only guest is fine
        let mut q = full_qemu();
        q.venus = false;
        let host2 = assess(&cfg(), q, nodes, true);
        let e = plan(&cfg(), &Gpu3d::default(), &host2).unwrap_err();
        assert!(
            matches!(
                e,
                Error::NodeLacks {
                    reason: "qemu-no-venus",
                    ..
                }
            ),
            "{e:?}"
        );
        assert!(plan(
            &cfg(),
            &Gpu3d {
                venus: false,
                ..Default::default()
            },
            &host2
        )
        .is_ok());
    }

    #[test]
    fn device_and_display_arguments() {
        let rt = Runtime3d {
            render_node: "/dev/dri/renderD128".into(),
            hostmem_mib: 2048,
            venus: true,
            virgl: true,
            device_select: Some("8086:46a6".into()),
        };
        assert_eq!(
            device_args(&rt).join(" "),
            "-vga none -device virtio-vga-gl,hostmem=2048M,blob=true,venus=true"
        );
        assert_eq!(
            display_arg(&rt),
            "egl-headless,rendernode=/dev/dri/renderD128"
        );
        assert_eq!(
            env(&rt),
            vec![("MESA_VK_DEVICE_SELECT".to_string(), "8086:46a6".to_string())]
        );
        let gl = Runtime3d {
            venus: false,
            virgl: true,
            ..rt.clone()
        };
        assert_eq!(
            device_args(&gl).join(" "),
            "-vga none -device virtio-vga-gl"
        );
        let vk = Runtime3d { virgl: false, ..rt };
        assert!(device_args(&vk).join(" ").ends_with(",virgl=off"));
    }

    #[test]
    fn sysfs_render_nodes_and_allow_list() {
        let t = std::env::temp_dir().join(format!("hx3d-{}", std::process::id()));
        let dri = t.join("dev/dri");
        let sys = t.join("sys/class/drm");
        let pci = t.join("sys/devices/pci0000:00/0000:00:02.0");
        std::fs::create_dir_all(&dri).unwrap();
        std::fs::create_dir_all(sys.join("renderD128")).unwrap();
        std::fs::create_dir_all(&pci).unwrap();
        std::fs::write(dri.join("renderD128"), "").unwrap();
        std::fs::write(dri.join("card0"), "").unwrap();
        std::fs::write(pci.join("vendor"), "0x8086\n").unwrap();
        std::fs::write(pci.join("device"), "0x46a6\n").unwrap();
        let drv = t.join("sys/bus/pci/drivers/i915");
        std::fs::create_dir_all(&drv).unwrap();
        std::os::unix::fs::symlink(&drv, pci.join("driver")).unwrap();
        std::os::unix::fs::symlink(&pci, sys.join("renderD128/device")).unwrap();
        let mut c = cfg();
        c.dri_dir = dri.clone();
        c.drm_sysfs_dir = sys.clone();
        let n = list_render_nodes(&c);
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].pci_address.as_deref(), Some("0000:00:02.0"));
        assert_eq!(n[0].vendor_id.as_deref(), Some("8086"));
        assert_eq!(n[0].driver.as_deref(), Some("i915"));
        assert!(n[0].suitable);
        c.render_nodes = vec!["0000:99:00.0".into()];
        assert!(!list_render_nodes(&c)[0].suitable);
        c.render_nodes = vec!["0000:00:02.0".into()];
        assert!(list_render_nodes(&c)[0].suitable);
        c.render_nodes.clear();
        c.ignore_drivers = vec!["i915".into()];
        assert!(!list_render_nodes(&c)[0].suitable);
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn incompatible_memory_options_are_refused() {
        let mut adv = horcrux_common::VmAdvanced::default();
        let d = VmDisplay {
            video: horcrux_common::VideoDevice::Virtio3d,
            ..Default::default()
        };
        assert!(check_compat(&adv, &d).is_ok());
        adv.numa = true;
        assert!(check_compat(&adv, &d).is_err());
        assert!(check_compat(&adv, &VmDisplay::default()).is_ok());
    }
}
