//! Performance tuning for latency-sensitive guests (gaming VMs, see
//! docs/design/gaming-vm.md): CPU pinning, huge-page backing, Hyper-V
//! enlightenments and evdev input.
//!
//! * [`HostCpus`] reads the host's CPU layout from sysfs, including the
//!   performance/efficiency split of hybrid Intel CPUs (`cpu_core` /
//!   `cpu_atom` PMUs), so `cpu_pinning.vcpus = "auto"` puts the guest on
//!   P-cores only (a guest that sees identical vCPUs but runs half of them on
//!   E-cores stutters).
//! * [`resolve_pinning`] turns a [`VmCpuPinning`] into one host CPU per vCPU
//!   plus the set for QEMU's own threads; [`apply_pinning`] applies it to the
//!   running QEMU (vCPU thread ids from QMP `query-cpus-fast`).
//! * [`reserve_hugepages`] / [`release_hugepages`] grow the kernel's huge
//!   page pool by what a VM needs at start and give it back at stop; the
//!   reservation is recorded next to the VM's disk, so a stop after an API
//!   restart still releases it.
//!
//! Everything takes a sysfs/procfs root so it is unit-tested on fixtures.
use horcrux_common::{
    format_cpu_list, parse_cpu_list, Error, Result, VmAdvanced, VmCpuPinning, VmHugePages,
};
use horcrux_core::config::PassthroughConfig;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The node's `[passthrough]` settings (read once; defaults when the
/// config file has none).
pub fn config() -> &'static PassthroughConfig {
    static CFG: OnceLock<PassthroughConfig> = OnceLock::new();
    CFG.get_or_init(|| horcrux_core::config::HorcruxConfig::load().passthrough)
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// The host's CPUs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct HostCpus {
    pub online: Vec<u32>,
    /// Hybrid CPUs: performance cores (`cpu_core` PMU); empty otherwise.
    pub performance: Vec<u32>,
    /// Hybrid CPUs: efficiency cores (`cpu_atom` PMU); empty otherwise.
    pub efficiency: Vec<u32>,
    /// CPUs that share a core with another CPU (SMT siblings), as groups.
    pub smt_groups: Vec<Vec<u32>>,
    /// CPUs sharing one last-level (L3) cache, as groups: an AMD CCD/CCX
    /// each, one group on most Intel CPUs. A guest whose vCPUs span two
    /// groups pays cross-die latency, so `auto` pinning keeps it in one.
    pub l3_groups: Vec<Vec<u32>>,
    /// L3 size in KiB per entry of `l3_groups` (X3D parts have one bigger
    /// 3D V-Cache die, which `auto` prefers).
    pub l3_sizes_kb: Vec<u64>,
}

/// Parse a sysfs cache size (`32768K`, `96M`) into KiB.
fn cache_kb(s: &str) -> u64 {
    let s = s.trim();
    let (num, mul) = match s.chars().last() {
        Some('K') => (&s[..s.len() - 1], 1),
        Some('M') => (&s[..s.len() - 1], 1024),
        Some('G') => (&s[..s.len() - 1], 1024 * 1024),
        _ => (s, 1),
    };
    num.parse::<u64>().map(|n| n * mul).unwrap_or(0)
}

impl HostCpus {
    /// Read from a sysfs root (`/sys`).
    pub fn read(sys: &Path) -> Self {
        let list = |p: PathBuf| parse_cpu_list(&read(&p)).unwrap_or_default();
        let online = list(sys.join("devices/system/cpu/online"));
        let mut smt_groups: Vec<Vec<u32>> = Vec::new();
        let mut l3_groups: Vec<Vec<u32>> = Vec::new();
        let mut l3_sizes_kb: Vec<u64> = Vec::new();
        for c in &online {
            let cpu = sys.join(format!("devices/system/cpu/cpu{c}"));
            let s = list(cpu.join("topology/thread_siblings_list"));
            if s.len() > 1 && !smt_groups.contains(&s) {
                smt_groups.push(s);
            }
            for idx in 0..8 {
                let d = cpu.join(format!("cache/index{idx}"));
                if !d.is_dir() {
                    break;
                }
                if read(&d.join("level")) != "3" {
                    continue;
                }
                let shared = list(d.join("shared_cpu_list"));
                if !shared.is_empty() && !l3_groups.contains(&shared) {
                    l3_groups.push(shared);
                    l3_sizes_kb.push(cache_kb(&read(&d.join("size"))));
                }
                break;
            }
        }
        Self {
            performance: list(sys.join("devices/cpu_core/cpus")),
            efficiency: list(sys.join("devices/cpu_atom/cpus")),
            online,
            smt_groups,
            l3_groups,
            l3_sizes_kb,
        }
    }

    pub fn hybrid(&self) -> bool {
        !self.performance.is_empty() && !self.efficiency.is_empty()
    }

    /// More than one L3 domain (AMD CCDs/CCXs, multi-socket hosts).
    pub fn multi_l3(&self) -> bool {
        self.l3_groups.len() > 1
    }

    /// L3 domains, best for a latency-sensitive guest first: the biggest
    /// cache (3D V-Cache die), then one without CPU 0 (housekeeping and
    /// most interrupts), then by CPU number.
    pub fn l3_order(&self) -> Vec<Vec<u32>> {
        let mut g: Vec<(u64, bool, u32, Vec<u32>)> = self
            .l3_groups
            .iter()
            .enumerate()
            .map(|(i, c)| {
                (
                    self.l3_sizes_kb.get(i).copied().unwrap_or(0),
                    c.contains(&0),
                    c.first().copied().unwrap_or(0),
                    c.clone(),
                )
            })
            .collect();
        g.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        g.into_iter().map(|x| x.3).collect()
    }

    /// `cpus` ordered core by core (SMT siblings next to each other), the
    /// core holding CPU 0 last.
    fn by_core(&self, cpus: &[u32]) -> Vec<u32> {
        let mut cores: Vec<Vec<u32>> = Vec::new();
        for c in cpus {
            if cores.iter().any(|k| k.contains(c)) {
                continue;
            }
            let core: Vec<u32> = self
                .smt_groups
                .iter()
                .find(|g| g.contains(c))
                .map(|g| g.iter().copied().filter(|x| cpus.contains(x)).collect())
                .unwrap_or_else(|| vec![*c]);
            cores.push(core);
        }
        cores.sort_by_key(|k| (k.contains(&0), k.first().copied().unwrap_or(0)));
        cores.into_iter().flatten().collect()
    }
}

/// Is nested virtualisation enabled in the host's KVM module? `None` = yes;
/// otherwise why not and how to turn it on.
pub fn nested_kvm_problem(sys: &Path) -> Option<String> {
    for m in ["kvm_intel", "kvm_amd"] {
        let d = sys.join("module").join(m);
        if d.is_dir() {
            let v = read(&d.join("parameters/nested"));
            return if matches!(v.as_str(), "Y" | "y" | "1") {
                None
            } else {
                Some(format!(
                    "the VM needs nested virtualisation (Windows VBS / Memory Integrity, Hyper-V), but {m} has it turned off on this host: add \"options {m} nested=1\" to /etc/modprobe.d/kvm.conf and reload the {m} module (or reboot the host), or turn nested virtualisation off for this VM"
                ))
            };
        }
    }
    Some("KVM (kvm_intel / kvm_amd) is not loaded on this host, so nested virtualisation is not available".into())
}

/// The CPUID "hypervisor" bit is set: this host is itself a virtual machine.
pub fn host_is_virtual() -> bool {
    cpuinfo_says_virtual(&std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default())
}

fn cpuinfo_says_virtual(cpuinfo: &str) -> bool {
    cpuinfo
        .lines()
        .find(|l| l.starts_with("flags"))
        .is_some_and(|l| l.split_whitespace().any(|f| f == "hypervisor"))
}

#[cfg(test)]
#[test]
fn the_hypervisor_cpu_flag_marks_a_virtual_host() {
    assert!(cpuinfo_says_virtual(
        "vendor_id\t: GenuineIntel\nflags\t\t: fpu vme vmx hypervisor lahf_lm\n"
    ));
    assert!(!cpuinfo_says_virtual(
        "vendor_id\t: AuthenticAMD\nflags\t\t: fpu svm hypervisor_x lahf_lm\n"
    ));
    assert!(!cpuinfo_says_virtual(""));
}

/// `GenuineIntel` / `AuthenticAMD` (from /proc/cpuinfo), cached.
pub fn host_cpu_vendor() -> &'static str {
    static V: OnceLock<String> = OnceLock::new();
    V.get_or_init(|| {
        std::fs::read_to_string("/proc/cpuinfo")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find_map(|l| l.strip_prefix("vendor_id"))
                    .map(|v| v.trim_start_matches([' ', '\t', ':']).trim().to_string())
            })
            .unwrap_or_default()
    })
}

/// `auto` vCPU placement: `n` host CPUs from the performance cores of a
/// hybrid CPU (never mixed with E-cores), otherwise from all CPUs; CPU 0
/// (where the kernel's housekeeping and most interrupts land) is used last.
pub fn auto_vcpu_cpus(h: &HostCpus, n: u32) -> Result<Vec<u32>> {
    // Several L3 domains (AMD CCDs/CCXs, multi-socket) on a non-hybrid CPU:
    // fill the best domain first, whole cores (SMT siblings together), and
    // only spill into the next one when the guest is bigger than a domain.
    if !h.hybrid() && h.multi_l3() {
        let want = n as usize;
        if want > h.online.len() {
            return Err(Error::Validation(format!(
                "the VM has {n} vCPUs but the host only {} CPUs",
                h.online.len()
            )));
        }
        let mut picked: Vec<u32> = Vec::new();
        for g in h.l3_order() {
            if picked.len() >= want {
                break;
            }
            let online: Vec<u32> = g.into_iter().filter(|c| h.online.contains(c)).collect();
            for c in h.by_core(&online) {
                if picked.len() >= want {
                    break;
                }
                picked.push(c);
            }
        }
        picked.sort_unstable();
        return Ok(picked);
    }
    let pool: &Vec<u32> = if h.hybrid() {
        &h.performance
    } else {
        &h.online
    };
    let n = n as usize;
    if n > pool.len() {
        return Err(Error::Validation(if h.hybrid() {
            format!(
                "automatic CPU pinning places vCPUs on performance cores only and this host has {} ({}); lower the VM's CPU count or list host CPUs in cpu_pinning.vcpus",
                pool.len(),
                format_cpu_list(pool)
            )
        } else {
            format!("the VM has {n} vCPUs but the host only {} CPUs", pool.len())
        }));
    }
    let mut order: Vec<u32> = pool.iter().copied().filter(|c| *c != 0).collect();
    if pool.contains(&0) {
        order.push(0);
    }
    let mut picked: Vec<u32> = order.into_iter().take(n).collect();
    picked.sort_unstable();
    Ok(picked)
}

/// Resolved pinning: host CPU of vCPU i, and QEMU's other threads' CPUs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Pinning {
    pub vcpus: Vec<u32>,
    pub emulator: Vec<u32>,
}

pub fn resolve_pinning(p: &VmCpuPinning, h: &HostCpus, vcpus: u32) -> Result<Pinning> {
    let v = if p.vcpus.trim() == "auto" {
        auto_vcpu_cpus(h, vcpus)?
    } else {
        let l = parse_cpu_list(&p.vcpus).map_err(Error::Validation)?;
        if let Some(bad) = l.iter().find(|c| !h.online.contains(c)) {
            return Err(Error::Validation(format!(
                "cpu_pinning.vcpus: host CPU {bad} is not online (online: {})",
                format_cpu_list(&h.online)
            )));
        }
        if l.len() < vcpus as usize {
            return Err(Error::Validation(format!(
                "cpu_pinning.vcpus lists {} host CPUs for {vcpus} vCPUs",
                l.len()
            )));
        }
        l.into_iter().take(vcpus as usize).collect()
    };
    let emulator = match p.emulator.as_deref().map(str::trim) {
        Some(e) if e != "auto" => {
            let l = parse_cpu_list(e).map_err(Error::Validation)?;
            if let Some(bad) = l.iter().find(|c| !h.online.contains(c)) {
                return Err(Error::Validation(format!(
                    "cpu_pinning.emulator: host CPU {bad} is not online"
                )));
            }
            l
        }
        _ => {
            let rest: Vec<u32> = h
                .online
                .iter()
                .copied()
                .filter(|c| !v.contains(c))
                .collect();
            if rest.is_empty() {
                h.online.clone()
            } else {
                rest
            }
        }
    };
    Ok(Pinning { vcpus: v, emulator })
}

fn cpuset(cpus: &[u32]) -> Result<nix::sched::CpuSet> {
    let mut set = nix::sched::CpuSet::new();
    for c in cpus {
        set.set(*c as usize)
            .map_err(|e| Error::System(format!("CPU {c}: {e}")))?;
    }
    Ok(set)
}

/// Pin the running QEMU `pid`: every vCPU thread to its host CPU, every
/// other thread (and the main thread, so threads QEMU starts later inherit
/// it) to the emulator set. `vcpu_threads` = thread id per vCPU index.
pub fn apply_pinning(proc_root: &Path, pid: i32, vcpu_threads: &[i32], p: &Pinning) -> Result<()> {
    use nix::unistd::Pid;
    for (i, tid) in vcpu_threads.iter().enumerate() {
        let Some(cpu) = p.vcpus.get(i) else { break };
        nix::sched::sched_setaffinity(Pid::from_raw(*tid), &cpuset(&[*cpu])?)
            .map_err(|e| Error::System(format!("pinning vCPU {i} (thread {tid}): {e}")))?;
    }
    let emu = cpuset(&p.emulator)?;
    let tasks = std::fs::read_dir(proc_root.join(pid.to_string()).join("task"))
        .map_err(|e| Error::System(format!("listing threads of QEMU {pid}: {e}")))?;
    for t in tasks.flatten() {
        let Ok(tid) = t.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        if vcpu_threads.contains(&tid) {
            continue;
        }
        // a thread may exit meanwhile: not an error
        let _ = nix::sched::sched_setaffinity(Pid::from_raw(tid), &emu);
    }
    Ok(())
}

/// vCPU thread ids, by vCPU index, from QMP `query-cpus-fast`.
pub fn vcpu_threads_from_qmp(reply: &serde_json::Value) -> Vec<i32> {
    let mut v: Vec<(i64, i32)> = reply
        .get("return")
        .and_then(|r| r.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|c| {
                    Some((
                        c.get("cpu-index")?.as_i64()?,
                        c.get("thread-id")?.as_i64()? as i32,
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_unstable();
    v.into_iter().map(|(_, t)| t).collect()
}

// ------------------------------------------------------------- huge pages

/// Huge pages of `size_kb` a guest of `memory_mb` needs.
pub fn pages_needed(memory_mb: u64, size_kb: u64) -> u64 {
    (memory_mb * 1024).div_ceil(size_kb)
}

/// What a VM start added to the kernel's huge-page pool.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HugeReservation {
    pub size_kb: u64,
    pub added: u64,
}

fn pool_dir(sys: &Path, size_kb: u64) -> PathBuf {
    sys.join(format!("kernel/mm/hugepages/hugepages-{size_kb}kB"))
}

/// Make sure `pages` free huge pages of `size_kb` exist, growing the pool
/// by the shortfall. Fails (and undoes the growth) when the kernel can't
/// find that much contiguous memory.
pub fn reserve_hugepages(sys: &Path, size_kb: u64, pages: u64) -> Result<HugeReservation> {
    let dir = pool_dir(sys, size_kb);
    if !dir.is_dir() {
        return Err(Error::Validation(format!(
            "this kernel offers no {} huge pages",
            human_kb(size_kb)
        )));
    }
    let num = |f: &str| read(&dir.join(f)).parse::<u64>().unwrap_or(0);
    let free = num("free_hugepages");
    let short = pages.saturating_sub(free);
    if short == 0 {
        return Ok(HugeReservation { size_kb, added: 0 });
    }
    let before = num("nr_hugepages");
    let w = |v: u64| {
        std::fs::write(dir.join("nr_hugepages"), v.to_string()).map_err(|e| {
            Error::System(format!(
                "growing the {} huge-page pool: {e}",
                human_kb(size_kb)
            ))
        })
    };
    w(before + short)?;
    let got = num("nr_hugepages").saturating_sub(before);
    if got < short {
        let _ = w(before);
        return Err(Error::Validation(format!(
            "the VM needs {pages} huge pages of {} ({} MB) but only {} more could be set aside: the host's free memory is too small or too fragmented. Stop other VMs or programs, or reserve them at boot (kernel option hugepages=)",
            human_kb(size_kb),
            pages * size_kb / 1024,
            free + got
        )));
    }
    Ok(HugeReservation {
        size_kb,
        added: short,
    })
}

/// Give back what [`reserve_hugepages`] added.
pub fn release_hugepages(sys: &Path, r: &HugeReservation) -> Result<()> {
    if r.added == 0 {
        return Ok(());
    }
    let dir = pool_dir(sys, r.size_kb);
    let nr = read(&dir.join("nr_hugepages")).parse::<u64>().unwrap_or(0);
    std::fs::write(
        dir.join("nr_hugepages"),
        nr.saturating_sub(r.added).to_string(),
    )
    .map_err(|e| Error::System(format!("shrinking the huge-page pool: {e}")))
}

fn human_kb(kb: u64) -> String {
    if kb >= 1048576 {
        format!("{} GiB", kb / 1048576)
    } else {
        format!("{} MiB", kb / 1024)
    }
}

/// Where a VM's huge-page reservation is recorded (next to its disk).
pub fn reservation_file(disk: &Path) -> PathBuf {
    disk.with_extension("hugepages.json")
}

/// Pool sizes of this host: (page KiB, total, free).
pub fn hugepage_pools(sys: &Path) -> Vec<(u64, u64, u64)> {
    let mut v: Vec<(u64, u64, u64)> = std::fs::read_dir(sys.join("kernel/mm/hugepages"))
        .map(|d| {
            d.flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    let kb = name
                        .strip_prefix("hugepages-")?
                        .strip_suffix("kB")?
                        .parse::<u64>()
                        .ok()?;
                    let n = |f: &str| read(&e.path().join(f)).parse::<u64>().unwrap_or(0);
                    Some((kb, n("nr_hugepages"), n("free_hugepages")))
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_unstable();
    v
}

// ------------------------------------------------------------- QEMU args

/// `-object memory-backend-memfd` options for huge-page backed memory.
pub fn hugepage_backend(h: &VmHugePages, shared: bool) -> String {
    format!(
        "memory-backend-memfd,hugetlb=on,hugetlbsize={}K,prealloc=on{}",
        h.size_kb,
        if shared { ",share=on" } else { "" }
    )
}

/// The `-cpu` value of a VM: its model plus the Hyper-V enlightenments and,
/// with `nested_virt`, the host's virtualisation extension.
pub fn cpu_arg(
    cpu_type: Option<&str>,
    adv: &VmAdvanced,
    cfg: &PassthroughConfig,
) -> Option<String> {
    cpu_arg_for(cpu_type, adv, cfg, host_cpu_vendor())
}

/// [`cpu_arg`] for a given host CPU vendor (`GenuineIntel`, `AuthenticAMD`).
pub fn cpu_arg_for(
    cpu_type: Option<&str>,
    adv: &VmAdvanced,
    cfg: &PassthroughConfig,
    vendor: &str,
) -> Option<String> {
    let nested = adv.nested_virt.then(|| {
        if vendor == "AuthenticAMD" || vendor == "HygonGenuine" {
            "svm=on"
        } else {
            "vmx=on"
        }
    });
    if !adv.hyperv {
        return match (cpu_type, nested) {
            (c, None) => c.map(str::to_string),
            (c, Some(flag)) => Some(format!("{},{flag}", c.unwrap_or("host"))),
        };
    }
    let model = cpu_type.unwrap_or(cfg.hyperv_cpu_model.as_str());
    let mut parts = vec![model.to_string()];
    parts.extend(
        cfg.hyperv_enlightenments
            .iter()
            .filter(|f| {
                !f.is_empty()
                    && f.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'=' | b'_'))
            })
            .cloned(),
    );
    if let Some(flag) = nested {
        parts.push(flag.to_string());
    }
    Some(parts.join(","))
}

/// Extra arguments for Hyper-V guests and evdev input devices.
pub fn device_args(adv: &VmAdvanced) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    if adv.hyperv {
        // Windows' clock: don't replay timer ticks a stalled guest missed.
        a.extend(["-global".into(), "kvm-pit.lost_tick_policy=discard".into()]);
    }
    for (i, e) in adv.evdev.iter().enumerate() {
        a.extend([
            "-object".into(),
            format!(
                "input-linux,id=hxevdev{i},evdev={}{},repeat=on",
                e.path,
                if e.grab_all { ",grab_all=on" } else { "" }
            ),
        ]);
    }
    a
}

// ------------------------------------------------------------- lifecycle

use super::qemu::QemuVm;

/// Does this VM use anything that needs work around its start/stop?
pub fn needs_lifecycle(vm: &QemuVm) -> bool {
    let adv = &vm.hardware.adv;
    adv.hugepages.is_some()
        || adv.cpu_pinning.is_some()
        || vm.hardware.hostpci.iter().any(|d| d.managed)
}

fn release_reservation(disk: &Path) {
    let f = reservation_file(disk);
    // take the record first so two releases never shrink the pool twice
    let taken = f.with_extension("json.releasing");
    if std::fs::rename(&f, &taken).is_err() {
        return;
    }
    if let Ok(r) =
        serde_json::from_slice::<HugeReservation>(&std::fs::read(&taken).unwrap_or_default())
    {
        if config().hugepages_release_on_stop {
            if let Err(e) = release_hugepages(Path::new("/sys"), &r) {
                tracing::warn!("{e}");
            }
        }
    }
    let _ = std::fs::remove_file(&taken);
}

/// Before QEMU starts: huge pages, then the managed device hand-off.
pub async fn prepare_start(vm: &QemuVm) -> Result<()> {
    if let Some(h) = &vm.hardware.adv.hugepages {
        let r = reserve_hugepages(
            Path::new("/sys"),
            h.size_kb,
            pages_needed(vm.memory, h.size_kb),
        )?;
        if r.added > 0 {
            std::fs::write(
                reservation_file(&vm.disk_path),
                serde_json::to_vec(&r).unwrap_or_default(),
            )
            .map_err(|e| Error::System(format!("recording the huge-page reservation: {e}")))?;
        }
    }
    if let Err(e) = super::handoff::acquire(
        &super::handoff::Host::default(),
        &vm.disk_path,
        &vm.hardware.hostpci,
    )
    .await
    {
        release_reservation(&vm.disk_path);
        return Err(e);
    }
    Ok(())
}

/// A start failed after [`prepare_start`]: undo it.
pub async fn undo_start(vm: &QemuVm) {
    super::handoff::release(
        &super::handoff::Host::default(),
        &vm.disk_path,
        std::time::Duration::from_secs(30),
    )
    .await;
    release_reservation(&vm.disk_path);
}

/// QEMU is gone (or going): give back devices and huge pages. The device
/// release waits in the background until QEMU has closed the device.
pub fn after_exit(vm: &QemuVm) {
    if !super::handoff::record_file(&vm.disk_path).exists()
        && !reservation_file(&vm.disk_path).exists()
    {
        return;
    }
    let disk = vm.disk_path.clone();
    tokio::spawn(async move {
        super::handoff::release(
            &super::handoff::Host::default(),
            &disk,
            std::time::Duration::from_secs(600),
        )
        .await;
        release_reservation(&disk);
    });
}

/// QEMU runs: pin its threads, and watch for its exit (a guest that powers
/// itself off must give the GPU back too).
pub async fn after_start(vm: &QemuVm) {
    let pid = std::fs::read_to_string(super::qemu::pidfile(&vm.id))
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok());
    if let (Some(p), Some(pid)) = (&vm.hardware.adv.cpu_pinning, pid) {
        let r = async {
            let pin = resolve_pinning(p, &HostCpus::read(Path::new("/sys")), vm.cpus)?;
            let reply = super::display::qmp_command(
                &super::display::DisplayRuntime::qmp_socket(&vm.disk_path),
                serde_json::json!({"execute": "query-cpus-fast"}),
            )
            .await?;
            let threads = vcpu_threads_from_qmp(&reply);
            apply_pinning(Path::new("/proc"), pid, &threads, &pin)?;
            tracing::info!(
                "VM {}: vCPUs pinned to {}, QEMU threads to {}",
                vm.id,
                format_cpu_list(&pin.vcpus),
                format_cpu_list(&pin.emulator)
            );
            Ok::<(), Error>(())
        }
        .await;
        if let Err(e) = r {
            tracing::warn!("VM {}: CPU pinning failed: {e}", vm.id);
        }
    }
    if let Some(pid) = pid {
        watch_exit(vm.clone(), pid as u32);
    }
}

/// Release a VM's hand-off/huge pages once its QEMU `pid` has exited.
pub fn watch_exit(vm: QemuVm, pid: u32) {
    if !super::handoff::record_file(&vm.disk_path).exists()
        && !reservation_file(&vm.disk_path).exists()
    {
        return;
    }
    tokio::spawn(async move {
        while super::qemu::pid_alive(pid) {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        after_exit(&vm);
    });
}

/// After an API restart: re-attach exit watchers to running VMs that hold
/// a hand-off or huge pages, and release what stopped VMs still hold.
pub fn reconcile(vms: &[QemuVm]) {
    for vm in vms {
        let pid = std::fs::read_to_string(super::qemu::pidfile(&vm.id))
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok());
        match pid {
            Some(p) if super::qemu::QemuManager::process_running(&vm.id) => {
                watch_exit(vm.clone(), p)
            }
            _ => after_exit(vm),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn sysfs(online: &str, core: Option<&str>, atom: Option<&str>) -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        let cpu = t.path().join("devices/system/cpu");
        fs::create_dir_all(&cpu).unwrap();
        fs::write(cpu.join("online"), format!("{online}\n")).unwrap();
        for c in parse_cpu_list(online).unwrap() {
            let d = cpu.join(format!("cpu{c}/topology"));
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("thread_siblings_list"), format!("{c}\n")).unwrap();
        }
        for (pmu, l) in [("cpu_core", core), ("cpu_atom", atom)] {
            if let Some(l) = l {
                let d = t.path().join("devices").join(pmu);
                fs::create_dir_all(&d).unwrap();
                fs::write(d.join("cpus"), format!("{l}\n")).unwrap();
            }
        }
        t
    }

    /// A hybrid Intel CPU: 8 P-cores (0-7), 16 E-cores (8-23), no SMT.
    fn arrow_lake() -> tempfile::TempDir {
        sysfs("0-23", Some("0-7"), Some("8-23"))
    }

    #[test]
    fn reads_hybrid_layout() {
        let t = arrow_lake();
        let h = HostCpus::read(t.path());
        assert!(h.hybrid());
        assert_eq!(h.performance, (0..8).collect::<Vec<_>>());
        assert_eq!(h.efficiency.len(), 16);
        assert!(h.smt_groups.is_empty());
    }

    #[test]
    fn auto_uses_p_cores_only_and_cpu0_last() {
        let h = HostCpus::read(arrow_lake().path());
        assert_eq!(auto_vcpu_cpus(&h, 6).unwrap(), vec![1, 2, 3, 4, 5, 6]);
        assert_eq!(auto_vcpu_cpus(&h, 8).unwrap(), (0..8).collect::<Vec<_>>());
        let e = auto_vcpu_cpus(&h, 10).unwrap_err().to_string();
        assert!(e.contains("performance cores") && e.contains("0-7"), "{e}");
        let p = resolve_pinning(
            &VmCpuPinning {
                vcpus: "auto".into(),
                emulator: None,
            },
            &h,
            7,
        )
        .unwrap();
        assert_eq!(p.vcpus, vec![1, 2, 3, 4, 5, 6, 7]);
        // QEMU's own threads: everything else (CPU 0 + the E-cores)
        assert_eq!(format_cpu_list(&p.emulator), "0,8-23");
    }

    #[test]
    fn auto_on_a_plain_cpu() {
        let h = HostCpus::read(sysfs("0-3", None, None).path());
        assert!(!h.hybrid());
        assert_eq!(auto_vcpu_cpus(&h, 2).unwrap(), vec![1, 2]);
        assert!(auto_vcpu_cpus(&h, 5).is_err());
        // all CPUs taken: QEMU's threads may use any
        let p = resolve_pinning(
            &VmCpuPinning {
                vcpus: "0-3".into(),
                emulator: Some("auto".into()),
            },
            &h,
            4,
        )
        .unwrap();
        assert_eq!(p.emulator, vec![0, 1, 2, 3]);
    }

    /// A two-CCD Ryzen (8 cores + SMT per CCD): CPUs n and n+16 are
    /// siblings; CCD0 = 0-7,16-23, CCD1 = 8-15,24-31. `vcache` gives CCD1
    /// the bigger L3 (an X3D part with the V-Cache die second).
    fn ryzen(vcache: bool) -> tempfile::TempDir {
        let t = sysfs("0-31", None, None);
        for c in 0..32u32 {
            let d = t.path().join(format!("devices/system/cpu/cpu{c}"));
            let core = c % 16;
            fs::create_dir_all(d.join("topology")).unwrap();
            fs::write(
                d.join("topology/thread_siblings_list"),
                format!("{core},{}", core + 16),
            )
            .unwrap();
            for (idx, level) in [(0, "1"), (1, "1"), (2, "2"), (3, "3")] {
                let i = d.join(format!("cache/index{idx}"));
                fs::create_dir_all(&i).unwrap();
                fs::write(i.join("level"), level).unwrap();
                if level == "3" {
                    let ccd1 = core >= 8;
                    fs::write(
                        i.join("shared_cpu_list"),
                        if ccd1 { "8-15,24-31" } else { "0-7,16-23" },
                    )
                    .unwrap();
                    fs::write(
                        i.join("size"),
                        if ccd1 && vcache { "98304K" } else { "32768K" },
                    )
                    .unwrap();
                } else {
                    fs::write(i.join("shared_cpu_list"), c.to_string()).unwrap();
                }
            }
        }
        t
    }

    #[test]
    fn auto_keeps_a_guest_on_one_ccd_with_whole_cores() {
        let h = HostCpus::read(ryzen(false).path());
        assert_eq!(h.l3_groups.len(), 2);
        assert_eq!(h.l3_sizes_kb, vec![32768, 32768]);
        // same-size CCDs: the one without CPU 0 first, siblings together
        assert_eq!(
            auto_vcpu_cpus(&h, 8).unwrap(),
            vec![8, 9, 10, 11, 24, 25, 26, 27]
        );
        assert_eq!(
            format_cpu_list(&auto_vcpu_cpus(&h, 16).unwrap()),
            "8-15,24-31"
        );
        // bigger than a CCD: spills into CCD0, CPU 0's core last
        let v = auto_vcpu_cpus(&h, 18).unwrap();
        assert_eq!(format_cpu_list(&v), "1,8-15,17,24-31");
        assert!(auto_vcpu_cpus(&h, 33).is_err());
    }

    #[test]
    fn auto_prefers_the_v_cache_die() {
        let h = HostCpus::read(ryzen(true).path());
        assert_eq!(h.l3_sizes_kb, vec![32768, 98304]);
        assert_eq!(
            h.l3_order()[0],
            vec![8, 9, 10, 11, 12, 13, 14, 15, 24, 25, 26, 27, 28, 29, 30, 31]
        );
        assert_eq!(cache_kb("96M"), 98304);
        assert_eq!(cache_kb("512K"), 512);
    }

    #[test]
    fn nested_virtualisation_flag_and_host_check() {
        let adv = VmAdvanced {
            nested_virt: true,
            ..Default::default()
        };
        let cfg = PassthroughConfig::default();
        assert_eq!(
            cpu_arg_for(None, &adv, &cfg, "GenuineIntel").unwrap(),
            "host,vmx=on"
        );
        assert_eq!(
            cpu_arg_for(Some("EPYC-v4"), &adv, &cfg, "AuthenticAMD").unwrap(),
            "EPYC-v4,svm=on"
        );
        let hv = VmAdvanced {
            nested_virt: true,
            hyperv: true,
            ..Default::default()
        };
        let a = cpu_arg_for(None, &hv, &cfg, "GenuineIntel").unwrap();
        assert!(
            a.starts_with("host,hv-relaxed") && a.ends_with(",vmx=on"),
            "{a}"
        );
        // off: unchanged
        assert_eq!(
            cpu_arg_for(None, &VmAdvanced::default(), &cfg, "GenuineIntel"),
            None
        );

        let t = tempfile::tempdir().unwrap();
        assert!(nested_kvm_problem(t.path()).unwrap().contains("not loaded"));
        let p = t.path().join("module/kvm_amd/parameters");
        fs::create_dir_all(&p).unwrap();
        fs::write(p.join("nested"), "0\n").unwrap();
        let why = nested_kvm_problem(t.path()).unwrap();
        assert!(why.contains("options kvm_amd nested=1"), "{why}");
        fs::write(p.join("nested"), "1\n").unwrap();
        assert_eq!(nested_kvm_problem(t.path()), None);
    }

    #[test]
    fn explicit_lists_are_checked() {
        let h = HostCpus::read(arrow_lake().path());
        let pin = |v: &str, e: Option<&str>| VmCpuPinning {
            vcpus: v.into(),
            emulator: e.map(Into::into),
        };
        let p = resolve_pinning(&pin("4-7,2", Some("8-9")), &h, 3).unwrap();
        assert_eq!(p.vcpus, vec![2, 4, 5]);
        assert_eq!(p.emulator, vec![8, 9]);
        assert!(resolve_pinning(&pin("30-31", None), &h, 2)
            .unwrap_err()
            .to_string()
            .contains("not online"));
        assert!(resolve_pinning(&pin("1", None), &h, 2).is_err());
        assert!(resolve_pinning(&pin("1-2", Some("99")), &h, 2).is_err());
    }

    #[test]
    fn qmp_thread_ids_by_index() {
        let r = serde_json::json!({"return": [
            {"cpu-index": 1, "thread-id": 202, "qom-path": "/machine/unattached/device[1]"},
            {"cpu-index": 0, "thread-id": 201},
        ]});
        assert_eq!(vcpu_threads_from_qmp(&r), vec![201, 202]);
        assert!(vcpu_threads_from_qmp(&serde_json::json!({"error": {}})).is_empty());
    }

    #[test]
    fn pins_real_threads_of_this_process() {
        // Pin this test's own thread to a CPU it may use, through the real
        // syscall; the "emulator" pass walks /proc/self/task.
        let mine = nix::sched::sched_getaffinity(nix::unistd::Pid::from_raw(0)).unwrap();
        let cpu = (0..nix::sched::CpuSet::count())
            .find(|c| mine.is_set(*c).unwrap_or(false))
            .unwrap() as u32;
        let tid = nix::unistd::gettid().as_raw();
        let pid = std::process::id() as i32;
        let p = Pinning {
            vcpus: vec![cpu],
            emulator: vec![cpu],
        };
        apply_pinning(Path::new("/proc"), pid, &[tid], &p).unwrap();
        let now = nix::sched::sched_getaffinity(nix::unistd::Pid::from_raw(tid)).unwrap();
        assert!(now.is_set(cpu as usize).unwrap());
        assert_eq!(
            (0..nix::sched::CpuSet::count())
                .filter(|c| now.is_set(*c).unwrap_or(false))
                .count(),
            1
        );
        // restore
        nix::sched::sched_setaffinity(nix::unistd::Pid::from_raw(tid), &mine).unwrap();
    }

    fn pool(t: &Path, kb: u64, nr: u64, free: u64) {
        let d = pool_dir(t, kb);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("nr_hugepages"), nr.to_string()).unwrap();
        fs::write(d.join("free_hugepages"), free.to_string()).unwrap();
    }

    #[test]
    fn hugepage_reservation_grows_and_shrinks_the_pool() {
        assert_eq!(pages_needed(16384, 2048), 8192);
        assert_eq!(pages_needed(16384, 1048576), 16);
        assert_eq!(pages_needed(1000, 1048576), 1);
        let t = tempfile::tempdir().unwrap();
        pool(t.path(), 2048, 100, 40);
        // the fixture's nr_hugepages is a plain file: the write "succeeds"
        let r = reserve_hugepages(t.path(), 2048, 100).unwrap();
        assert_eq!(
            r,
            HugeReservation {
                size_kb: 2048,
                added: 60
            }
        );
        assert_eq!(read(&pool_dir(t.path(), 2048).join("nr_hugepages")), "160");
        release_hugepages(t.path(), &r).unwrap();
        assert_eq!(read(&pool_dir(t.path(), 2048).join("nr_hugepages")), "100");
        // enough free pages: nothing added, nothing released
        let r = reserve_hugepages(t.path(), 2048, 10).unwrap();
        assert_eq!(r.added, 0);
        assert!(reserve_hugepages(t.path(), 1048576, 1)
            .unwrap_err()
            .to_string()
            .contains("no 1 GiB huge pages"));
        assert_eq!(hugepage_pools(t.path()), vec![(2048, 100, 40)]);
    }

    #[test]
    fn hugepage_shortfall_is_undone() {
        // The kernel gives fewer pages than asked: emulate it with a
        // directory where nr_hugepages can't be written (a dir, not a file)
        let t = tempfile::tempdir().unwrap();
        let d = pool_dir(t.path(), 2048);
        fs::create_dir_all(d.join("nr_hugepages")).unwrap();
        fs::write(d.join("free_hugepages"), "0").unwrap();
        let e = reserve_hugepages(t.path(), 2048, 4)
            .unwrap_err()
            .to_string();
        assert!(e.contains("huge-page pool"), "{e}");
    }

    #[test]
    fn hyperv_cpu_and_device_args() {
        let cfg = PassthroughConfig::default();
        let mut adv = VmAdvanced::default();
        assert_eq!(cpu_arg(None, &adv, &cfg), None);
        assert_eq!(cpu_arg(Some("max"), &adv, &cfg).as_deref(), Some("max"));
        adv.hyperv = true;
        let c = cpu_arg(None, &adv, &cfg).unwrap();
        assert!(c.starts_with("host,hv-relaxed,"), "{c}");
        assert!(c.contains("hv-time") && c.contains("hv-stimer"), "{c}");
        let c = cpu_arg(Some("EPYC-v4"), &adv, &cfg).unwrap();
        assert!(c.starts_with("EPYC-v4,hv-"), "{c}");
        // configurable list; unsafe entries never reach the command line
        let cfg2 = PassthroughConfig {
            hyperv_enlightenments: vec!["hv-relaxed".into(), "x,y".into(), "".into()],
            hyperv_cpu_model: "max".into(),
            ..Default::default()
        };
        assert_eq!(cpu_arg(None, &adv, &cfg2).unwrap(), "max,hv-relaxed");
        adv.evdev = vec![
            horcrux_common::VmEvdev {
                path: "/dev/input/by-id/usb-kbd-event-kbd".into(),
                grab_all: true,
            },
            horcrux_common::VmEvdev {
                path: "/dev/input/event5".into(),
                grab_all: false,
            },
        ];
        let a = device_args(&adv).join(" ");
        assert!(
            a.contains("-global kvm-pit.lost_tick_policy=discard"),
            "{a}"
        );
        assert!(a.contains(
            "-object input-linux,id=hxevdev0,evdev=/dev/input/by-id/usb-kbd-event-kbd,grab_all=on,repeat=on"
        ));
        assert!(a.contains("-object input-linux,id=hxevdev1,evdev=/dev/input/event5,repeat=on"));
        assert_eq!(
            hugepage_backend(&VmHugePages { size_kb: 1048576 }, false),
            "memory-backend-memfd,hugetlb=on,hugetlbsize=1048576K,prealloc=on"
        );
    }
}
