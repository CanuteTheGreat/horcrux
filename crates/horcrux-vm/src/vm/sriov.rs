//! Declarative SR-IOV provisioning of GPU physical functions.
//!
//! The desired state is an [`SriovConfig`] (stored by the node, part of the
//! declarative config): per PF the number of VFs, which VFIO driver the VFs
//! are bound to, and, where the PF driver exposes it (Intel `xe`
//! `sriov_admin`), the VRAM quota and scheduling of each VF.
//!
//! * [`plan`] is pure: desired + observed state -> ordered steps or errors.
//!   A PF that is missing or not SR-IOV capable is refused with the reason.
//! * [`apply`] carries the steps out over sysfs, in the order the kernel needs:
//!   autoprobe off, VFs removed when the layout changes, quotas written while no
//!   VF exists, `sriov_numvfs`, then each VF is bound to its VFIO driver and the
//!   scheduling parameters (changeable while running) are written.
//! * [`SriovStore`] keeps the desired config and a list of generations so a
//!   change can be rolled back.
//!
//! Nothing here is hard-coded: counts, drivers, quotas, the settle timeout and
//! the number of generations kept are config keys with defaults.
use super::gpucap::{read, sriov_refusal, HostPaths};
use horcrux_common::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn yes() -> bool {
    true
}
fn default_settle() -> u64 {
    15
}
fn default_keep() -> usize {
    10
}

/// Which VFIO driver the VFs of a PF are bound to.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum VfDriver {
    /// `xe-vfio-pci` for an `xe` PF when that driver is loaded (it carries
    /// the VFIO migration support), else `vfio-pci`.
    #[default]
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "vfio-pci")]
    VfioPci,
    #[serde(rename = "xe-vfio-pci")]
    XeVfioPci,
}

/// Per-VF overrides (1-based VF number as in `sriov_admin/vfN`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct VfSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vram_quota_mib: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_quantum_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preempt_timeout_us: Option<u64>,
}

/// Desired SR-IOV state of one PF.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PfSpec {
    /// PCI address of the PF (`0000:03:00.0`).
    pub pf: String,
    /// Number of VFs; 0 removes them.
    pub num_vfs: u32,
    #[serde(default)]
    pub vf_driver: VfDriver,
    /// `sriov_drivers_autoprobe`; off by default so the host driver never
    /// grabs a VF that is meant for a VM.
    #[serde(default)]
    pub autoprobe: bool,
    /// Defaults for every VF (overridden per VF in `vfs`).
    #[serde(flatten)]
    pub all: VfSpec,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub vfs: BTreeMap<u32, VfSpec>,
}

impl PfSpec {
    fn for_vf(&self, vf: u32) -> VfSpec {
        let o = self.vfs.get(&vf);
        VfSpec {
            vram_quota_mib: o.and_then(|o| o.vram_quota_mib).or(self.all.vram_quota_mib),
            exec_quantum_ms: o
                .and_then(|o| o.exec_quantum_ms)
                .or(self.all.exec_quantum_ms),
            preempt_timeout_us: o
                .and_then(|o| o.preempt_timeout_us)
                .or(self.all.preempt_timeout_us),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SriovConfig {
    /// Apply the stored config when the API starts (before VMs autostart).
    #[serde(default = "yes")]
    pub apply_on_boot: bool,
    /// Seconds to wait for VFs to appear after writing `sriov_numvfs`.
    #[serde(default = "default_settle")]
    pub settle_timeout_s: u64,
    /// Generations kept for rollback.
    #[serde(default = "default_keep")]
    pub generations_keep: usize,
    #[serde(default)]
    pub pfs: Vec<PfSpec>,
}

impl Default for SriovConfig {
    fn default() -> Self {
        Self {
            apply_on_boot: true,
            settle_timeout_s: default_settle(),
            generations_keep: default_keep(),
            pfs: Vec::new(),
        }
    }
}

impl SriovConfig {
    /// Checks that need nothing but the config.
    pub fn validate(&self) -> Result<()> {
        let mut seen = BTreeSet::new();
        for p in &self.pfs {
            let a = norm_addr(&p.pf)
                .ok_or_else(|| Error::Validation(format!("{:?} is not a PCI address", p.pf)))?;
            if !seen.insert(a.clone()) {
                return Err(Error::Validation(format!("PF {a} is listed twice")));
            }
            for (n, v) in &p.vfs {
                if *n == 0 || *n > p.num_vfs {
                    return Err(Error::Validation(format!(
                        "PF {a}: vfs.{n} is outside 1..={} (num_vfs)",
                        p.num_vfs
                    )));
                }
                if v.vram_quota_mib == Some(0) {
                    return Err(Error::Validation(format!(
                        "PF {a}: vfs.{n}.vram_quota_mib must be > 0 (leave it out for the default)"
                    )));
                }
            }
            if p.all.vram_quota_mib == Some(0) {
                return Err(Error::Validation(format!(
                    "PF {a}: vram_quota_mib must be > 0 (leave it out for the default)"
                )));
            }
        }
        Ok(())
    }
}

/// `[dddd:]bb:dd.f` -> lower case `dddd:bb:dd.f`.
pub fn norm_addr(s: &str) -> Option<String> {
    let d = s.trim().to_ascii_lowercase();
    let full = match d.matches(':').count() {
        2 => d,
        1 => format!("0000:{d}"),
        _ => return None,
    };
    let ok = full.len() == 12
        && full.as_bytes()[4] == b':'
        && full.as_bytes()[7] == b':'
        && full.as_bytes()[10] == b'.'
        && full
            .chars()
            .enumerate()
            .all(|(i, c)| matches!(i, 4 | 7 | 10) || c.is_ascii_hexdigit());
    ok.then_some(full)
}

// ------------------------------------------------------------------ plan

/// What one PF looks like now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Observed {
    pub pf: String,
    pub exists: bool,
    pub vendor_id: String,
    pub driver: Option<String>,
    pub total_vfs: u32,
    pub current_vfs: u32,
    pub autoprobe: Option<bool>,
    /// `sriov_admin` exists: per-VF resources are settable.
    pub admin: bool,
    /// 1-based VF number -> (address, bound driver)
    pub vfs: BTreeMap<u32, (String, Option<String>)>,
    pub vram_mib: BTreeMap<u32, u64>,
    pub exec_quantum_ms: BTreeMap<u32, u64>,
    pub preempt_timeout_us: BTreeMap<u32, u64>,
    /// VFIO drivers that are loaded (`vfio-pci`, `xe-vfio-pci`).
    pub vfio_drivers: Vec<String>,
}

pub fn observe(paths: &HostPaths, pf: &str) -> Observed {
    let a = norm_addr(pf).unwrap_or_else(|| pf.to_string());
    let d = paths.pci_dev(&a);
    let mut o = Observed {
        pf: a.clone(),
        exists: d.exists(),
        ..Default::default()
    };
    if !o.exists {
        return o;
    }
    o.vendor_id = read(&d.join("vendor")).trim_start_matches("0x").to_string();
    o.driver = std::fs::read_link(d.join("driver"))
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()));
    o.total_vfs = read(&d.join("sriov_totalvfs")).parse().unwrap_or(0);
    o.current_vfs = read(&d.join("sriov_numvfs")).parse().unwrap_or(0);
    o.autoprobe = match read(&d.join("sriov_drivers_autoprobe")).as_str() {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    };
    let admin = d.join("sriov_admin");
    o.admin = admin.is_dir();
    for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(n) = name
            .strip_prefix("virtfn")
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        let Some(addr) = std::fs::read_link(e.path())
            .ok()
            .and_then(|l| l.file_name().map(|n| n.to_string_lossy().to_string()))
        else {
            continue;
        };
        let drv = std::fs::read_link(paths.pci_dev(&addr).join("driver"))
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()));
        o.vfs.insert(n + 1, (addr, drv));
    }
    for vf in 1..=o.total_vfs.max(o.current_vfs) {
        let prof = admin.join(format!("vf{vf}/profile"));
        let num = |f: &str| read(&prof.join(f)).parse::<u64>().ok();
        if let Some(b) = num("vram_quota") {
            o.vram_mib.insert(vf, b / (1024 * 1024));
        }
        if let Some(v) = num("exec_quantum_ms") {
            o.exec_quantum_ms.insert(vf, v);
        }
        if let Some(v) = num("preempt_timeout_us") {
            o.preempt_timeout_us.insert(vf, v);
        }
    }
    let drivers = paths.sys.join("bus/pci/drivers");
    for want in ["vfio-pci", "xe-vfio-pci", "xe_vfio_pci"] {
        if drivers.join(want).exists() {
            o.vfio_drivers.push(want.replace('_', "-"));
        }
    }
    o.vfio_drivers.dedup();
    o
}

/// One thing the apply does.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Step {
    pub pf: String,
    /// `autoprobe`, `disable_vfs`, `vram_quota`, `enable_vfs`, `bind_vf`,
    /// `exec_quantum`, `preempt_timeout`.
    pub action: String,
    pub target: String,
    pub detail: String,
    /// Written value (`sriov_numvfs`, quota in bytes, driver name, ...).
    pub value: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Plan {
    pub steps: Vec<Step>,
    pub errors: Vec<String>,
}

impl Plan {
    pub fn in_sync(&self) -> bool {
        self.steps.is_empty() && self.errors.is_empty()
    }
}

/// VFs that VMs list: all of them, and the ones a running VM holds.
#[derive(Debug, Clone, Default)]
pub struct Usage {
    /// VF address -> VM id.
    pub assigned: BTreeMap<String, String>,
    pub running: BTreeSet<String>,
}

fn resolve_driver(p: &PfSpec, o: &Observed) -> std::result::Result<String, String> {
    let has = |d: &str| o.vfio_drivers.iter().any(|x| x == d);
    match p.vf_driver {
        VfDriver::VfioPci if has("vfio-pci") => Ok("vfio-pci".into()),
        VfDriver::VfioPci => Err("the vfio-pci driver is not loaded (modprobe vfio-pci)".into()),
        VfDriver::XeVfioPci if has("xe-vfio-pci") => Ok("xe-vfio-pci".into()),
        VfDriver::XeVfioPci => {
            Err("the xe-vfio-pci driver is not loaded (Linux 6.19+, modprobe xe-vfio-pci)".into())
        }
        VfDriver::Auto if o.driver.as_deref() == Some("xe") && has("xe-vfio-pci") => {
            Ok("xe-vfio-pci".into())
        }
        VfDriver::Auto if has("vfio-pci") => Ok("vfio-pci".into()),
        VfDriver::Auto => Err("no VFIO driver is loaded (modprobe vfio-pci)".into()),
    }
}

fn step(pf: &str, action: &str, target: &str, value: &str, detail: String) -> Step {
    Step {
        pf: pf.into(),
        action: action.into(),
        target: target.into(),
        value: value.into(),
        detail,
    }
}

/// Pure: what must change to reach `cfg` from `observed`.
pub fn plan(cfg: &SriovConfig, observed: &[Observed], usage: &Usage) -> Plan {
    let mut out = Plan::default();
    if let Err(e) = cfg.validate() {
        out.errors.push(e.to_string());
        return out;
    }
    for spec in &cfg.pfs {
        let addr = norm_addr(&spec.pf).unwrap_or_default();
        let Some(o) = observed.iter().find(|o| o.pf == addr) else {
            out.errors.push(format!("PF {addr}: not inspected"));
            continue;
        };
        if !o.exists {
            out.errors
                .push(format!("PF {addr}: no such PCI device on this host"));
            continue;
        }
        if o.total_vfs == 0 {
            out.errors.push(format!(
                "PF {addr}: not SR-IOV capable: {}",
                sriov_refusal(&o.vendor_id, o.driver.as_deref())
            ));
            continue;
        }
        if spec.num_vfs > o.total_vfs {
            out.errors.push(format!(
                "PF {addr}: {} VFs requested but the device supports at most {}",
                spec.num_vfs, o.total_vfs
            ));
            continue;
        }
        let wants_res = |v: &VfSpec| {
            v.vram_quota_mib.is_some()
                || v.exec_quantum_ms.is_some()
                || v.preempt_timeout_us.is_some()
        };
        if !o.admin && (wants_res(&spec.all) || spec.vfs.values().any(wants_res)) {
            out.errors.push(format!(
                "PF {addr}: the {} driver exposes no per-VF resource controls (sriov_admin); \
                 remove vram_quota_mib / exec_quantum_ms / preempt_timeout_us",
                o.driver.as_deref().unwrap_or("bound")
            ));
            continue;
        }
        let driver = match resolve_driver(spec, o) {
            Ok(d) => d,
            Err(e) if spec.num_vfs > 0 => {
                out.errors.push(format!("PF {addr}: {e}"));
                continue;
            }
            Err(_) => String::new(),
        };

        let n = spec.num_vfs;
        // do the VFs need to be re-created (count or quota layout change)?
        let quota_differs = (1..=n).any(|vf| {
            spec.for_vf(vf)
                .vram_quota_mib
                .is_some_and(|m| o.vram_mib.get(&vf) != Some(&m))
        });
        let recreate = o.current_vfs > 0 && (o.current_vfs != n || (quota_differs && n > 0));
        // Safety: never pull a VF away from a VM.
        let mut blocked = false;
        if recreate {
            for (vf, (a, _)) in &o.vfs {
                let vm = usage.assigned.get(a).cloned().unwrap_or_default();
                if usage.running.contains(a) {
                    out.errors.push(format!(
                        "PF {addr}: VF {a} is in use by running VM {vm}; stop it first"
                    ));
                    blocked = true;
                } else if *vf > n && !vm.is_empty() {
                    out.errors.push(format!(
                        "PF {addr}: VF {a} is assigned to VM {vm}; remove it from the VM before \
                         lowering the VF count"
                    ));
                    blocked = true;
                }
            }
        }
        if blocked {
            continue;
        }

        if o.autoprobe.is_some_and(|a| a != spec.autoprobe) {
            out.steps.push(step(
                &addr,
                "autoprobe",
                "sriov_drivers_autoprobe",
                if spec.autoprobe { "1" } else { "0" },
                format!(
                    "VF autoprobe {} -> {}",
                    o.autoprobe.unwrap_or(false),
                    spec.autoprobe
                ),
            ));
        }
        if recreate {
            out.steps.push(step(
                &addr,
                "disable_vfs",
                "sriov_numvfs",
                "0",
                format!("remove the {} existing VFs", o.current_vfs),
            ));
        }
        let recreated = recreate || o.current_vfs == 0;
        // quotas are written while no VF exists
        if n > 0 && recreated {
            for vf in 1..=n {
                if let Some(m) = spec.for_vf(vf).vram_quota_mib {
                    out.steps.push(step(
                        &addr,
                        "vram_quota",
                        &format!("vf{vf}"),
                        &(m * 1024 * 1024).to_string(),
                        format!("VF {vf} VRAM quota {m} MiB"),
                    ));
                }
            }
        }
        if (recreate || o.current_vfs != n) && n > 0 {
            out.steps.push(step(
                &addr,
                "enable_vfs",
                "sriov_numvfs",
                &n.to_string(),
                format!("create {n} VFs"),
            ));
        }
        if n > 0 {
            for vf in 1..=n {
                let bound = o.vfs.get(&vf).and_then(|(_, d)| d.as_deref());
                if recreated || bound != Some(driver.as_str()) {
                    out.steps.push(step(
                        &addr,
                        "bind_vf",
                        &format!("vf{vf}"),
                        &driver,
                        format!("bind VF {vf} to {driver}"),
                    ));
                }
                let want = spec.for_vf(vf);
                if let Some(v) = want.exec_quantum_ms {
                    if recreated || o.exec_quantum_ms.get(&vf) != Some(&v) {
                        out.steps.push(step(
                            &addr,
                            "exec_quantum",
                            &format!("vf{vf}"),
                            &v.to_string(),
                            format!("VF {vf} execution quantum {v} ms"),
                        ));
                    }
                }
                if let Some(v) = want.preempt_timeout_us {
                    if recreated || o.preempt_timeout_us.get(&vf) != Some(&v) {
                        out.steps.push(step(
                            &addr,
                            "preempt_timeout",
                            &format!("vf{vf}"),
                            &v.to_string(),
                            format!("VF {vf} preemption timeout {v} us"),
                        ));
                    }
                }
            }
        }
    }
    out
}

// ----------------------------------------------------------------- apply

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StepResult {
    #[serde(flatten)]
    pub step: Step,
    /// `applied`, `failed` or `skipped` (an earlier step of the PF failed).
    pub status: String,
    #[serde(default)]
    pub message: String,
}

/// Executes a plan over sysfs.
pub struct Applier {
    pub paths: HostPaths,
    pub settle: Duration,
    /// Check the driver a VF ended up on (off in fixture trees, where
    /// writing `drivers_probe` binds nothing).
    pub verify_binding: bool,
}

impl Applier {
    pub fn new(paths: HostPaths, settle_timeout_s: u64) -> Self {
        Self {
            paths,
            settle: Duration::from_secs(settle_timeout_s),
            verify_binding: true,
        }
    }

    fn w(&self, p: &Path, v: &str) -> std::result::Result<(), String> {
        std::fs::write(p, v).map_err(|e| format!("writing {} to {}: {e}", v.trim(), p.display()))
    }

    fn wait_vfs(&self, pf_dir: &Path, n: u32) -> std::result::Result<(), String> {
        let t0 = Instant::now();
        loop {
            let have = (0..n)
                .filter(|i| pf_dir.join(format!("virtfn{i}")).exists())
                .count() as u32;
            if have == n {
                return Ok(());
            }
            if t0.elapsed() >= self.settle {
                return Err(format!(
                    "only {have} of {n} VFs appeared within {} s",
                    self.settle.as_secs()
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn one(&self, s: &Step) -> std::result::Result<String, String> {
        let dir = self.paths.pci_dev(&s.pf);
        let vf_n = |t: &str| t.strip_prefix("vf").and_then(|n| n.parse::<u32>().ok());
        match s.action.as_str() {
            "autoprobe" | "disable_vfs" => {
                self.w(&dir.join(&s.target), &format!("{}\n", s.value))?;
                Ok(String::new())
            }
            "enable_vfs" => {
                let n: u32 = s.value.parse().map_err(|_| "bad VF count".to_string())?;
                self.w(&dir.join("sriov_numvfs"), &format!("{n}\n"))?;
                self.wait_vfs(&dir, n)?;
                Ok(String::new())
            }
            "vram_quota" | "exec_quantum" | "preempt_timeout" => {
                let vf = vf_n(&s.target).ok_or("bad VF")?;
                let file = match s.action.as_str() {
                    "vram_quota" => "vram_quota",
                    "exec_quantum" => "exec_quantum_ms",
                    _ => "preempt_timeout_us",
                };
                self.w(
                    &dir.join(format!("sriov_admin/vf{vf}/profile/{file}")),
                    &format!("{}\n", s.value),
                )?;
                Ok(String::new())
            }
            "bind_vf" => {
                let vf = vf_n(&s.target).ok_or("bad VF")?;
                let link = dir.join(format!("virtfn{}", vf - 1));
                let addr = std::fs::read_link(&link)
                    .ok()
                    .and_then(|l| l.file_name().map(|n| n.to_string_lossy().to_string()))
                    .ok_or_else(|| format!("VF {vf} does not exist"))?;
                let vd = self.paths.pci_dev(&addr);
                let current = std::fs::read_link(vd.join("driver"))
                    .ok()
                    .and_then(|l| l.file_name().map(|n| n.to_string_lossy().to_string()));
                if current.as_deref() != Some(s.value.as_str()) {
                    self.w(&vd.join("driver_override"), &s.value)?;
                    if let Some(cur) = current {
                        self.w(
                            &self
                                .paths
                                .sys
                                .join("bus/pci/drivers")
                                .join(cur)
                                .join("unbind"),
                            &addr,
                        )?;
                    }
                    self.w(&self.paths.sys.join("bus/pci/drivers_probe"), &addr)?;
                }
                super::gpucap::forget_migration(&self.paths, &addr);
                if self.verify_binding {
                    let now = std::fs::read_link(vd.join("driver"))
                        .ok()
                        .and_then(|l| l.file_name().map(|n| n.to_string_lossy().to_string()));
                    if now.as_deref() != Some(s.value.as_str()) {
                        return Err(format!(
                            "VF {addr} is bound to {} instead of {}",
                            now.unwrap_or_else(|| "no driver".into()),
                            s.value
                        ));
                    }
                }
                Ok(addr)
            }
            other => Err(format!("unknown step {other}")),
        }
    }

    /// Run the steps in order. A failed step skips the rest of its PF; other
    /// PFs continue.
    pub fn apply(&self, plan: &Plan) -> Vec<StepResult> {
        let mut failed: BTreeSet<String> = BTreeSet::new();
        let mut out = Vec::new();
        for s in &plan.steps {
            if failed.contains(&s.pf) {
                out.push(StepResult {
                    step: s.clone(),
                    status: "skipped".into(),
                    message: "an earlier step for this PF failed".into(),
                });
                continue;
            }
            match self.one(s) {
                Ok(m) => out.push(StepResult {
                    step: s.clone(),
                    status: "applied".into(),
                    message: m,
                }),
                Err(e) => {
                    failed.insert(s.pf.clone());
                    out.push(StepResult {
                        step: s.clone(),
                        status: "failed".into(),
                        message: e,
                    });
                }
            }
        }
        out
    }
}

// ----------------------------------------------------------------- store

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Generation {
    pub id: u64,
    /// Unix seconds.
    pub at: u64,
    /// `apply`, `rollback` or `boot`.
    pub origin: String,
    pub config: SriovConfig,
}

/// Desired config + generations as JSON files in a directory.
#[derive(Debug, Clone)]
pub struct SriovStore {
    dir: PathBuf,
}

impl SriovStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// `<HORCRUX_DATA_DIR or /var/lib/horcrux>/gpu`
    pub fn from_env() -> Self {
        let base = std::env::var("HORCRUX_DATA_DIR").unwrap_or_else(|_| "/var/lib/horcrux".into());
        Self::new(Path::new(&base).join("gpu"))
    }

    fn write(&self, name: &str, v: &impl Serialize) -> Result<()> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| Error::System(format!("{}: {e}", self.dir.display())))?;
        let tmp = self.dir.join(format!(".{name}.tmp"));
        let text = serde_json::to_string_pretty(v).map_err(|e| Error::System(e.to_string()))?;
        std::fs::write(&tmp, text)
            .and_then(|_| std::fs::rename(&tmp, self.dir.join(name)))
            .map_err(|e| Error::System(format!("writing {name}: {e}")))
    }

    pub fn config(&self) -> SriovConfig {
        std::fs::read_to_string(self.dir.join("sriov.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn set_config(&self, c: &SriovConfig) -> Result<()> {
        c.validate()?;
        self.write("sriov.json", c)
    }

    pub fn generations(&self) -> Vec<Generation> {
        std::fs::read_to_string(self.dir.join("sriov-generations.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// Record an applied config as a new generation, trimming old ones.
    pub fn record(&self, cfg: &SriovConfig, origin: &str) -> Result<Generation> {
        let mut g = self.generations();
        let id = g.last().map(|x| x.id + 1).unwrap_or(1);
        let gen = Generation {
            id,
            at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            origin: origin.into(),
            config: cfg.clone(),
        };
        g.push(gen.clone());
        let keep = cfg.generations_keep.max(1);
        if g.len() > keep {
            g.drain(..g.len() - keep);
        }
        self.write("sriov-generations.json", &g)?;
        Ok(gen)
    }
}

/// Outcome of an apply/rollback.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApplyReport {
    pub ok: bool,
    pub dry_run: bool,
    pub plan: Plan,
    pub results: Vec<StepResult>,
    pub generation: Option<u64>,
}

/// Plan against the live host, and (unless `dry_run`) apply and record.
pub fn reconcile(
    paths: &HostPaths,
    store: &SriovStore,
    cfg: &SriovConfig,
    usage: &Usage,
    dry_run: bool,
    origin: &str,
) -> Result<ApplyReport> {
    cfg.validate()?;
    let observed: Vec<Observed> = cfg.pfs.iter().map(|p| observe(paths, &p.pf)).collect();
    let pl = plan(cfg, &observed, usage);
    if dry_run {
        return Ok(ApplyReport {
            ok: pl.errors.is_empty(),
            dry_run,
            plan: pl,
            results: vec![],
            generation: None,
        });
    }
    if !pl.errors.is_empty() {
        return Ok(ApplyReport {
            ok: false,
            dry_run,
            plan: pl,
            results: vec![],
            generation: None,
        });
    }
    let applier = Applier::new(paths.clone(), cfg.settle_timeout_s);
    let results = applier.apply(&pl);
    let ok = results.iter().all(|r| r.status == "applied");
    let mut generation = None;
    if ok {
        let changed = store.config() != *cfg || !pl.steps.is_empty();
        store.set_config(cfg)?;
        if changed {
            generation = Some(store.record(cfg, origin)?.id);
        }
    }
    Ok(ApplyReport {
        ok,
        dry_run,
        plan: pl,
        results,
        generation,
    })
}

#[cfg(test)]
mod tests {
    use super::super::gpucap::fixture::*;
    use super::*;
    use tempfile::tempdir;

    fn paths(t: &tempfile::TempDir) -> HostPaths {
        HostPaths::with_roots(t.path().join("sys"), t.path().join("dev"))
    }

    fn cfg(n: u32) -> SriovConfig {
        SriovConfig {
            pfs: vec![PfSpec {
                pf: "03:00.0".into(),
                num_vfs: n,
                vf_driver: VfDriver::Auto,
                autoprobe: false,
                all: VfSpec {
                    vram_quota_mib: Some(2048),
                    ..Default::default()
                },
                vfs: BTreeMap::new(),
            }],
            ..Default::default()
        }
    }

    fn driver_dirs(p: &HostPaths) {
        for d in ["vfio-pci", "xe-vfio-pci"] {
            std::fs::create_dir_all(p.sys.join("bus/pci/drivers").join(d)).unwrap();
        }
    }

    #[test]
    fn address_normalisation() {
        assert_eq!(norm_addr("03:00.0").unwrap(), "0000:03:00.0");
        assert_eq!(norm_addr("0000:0A:00.1").unwrap(), "0000:0a:00.1");
        assert!(norm_addr("../x").is_none());
        assert!(norm_addr("0000:03:00.g").is_none());
    }

    #[test]
    fn config_defaults_and_validation() {
        let c: SriovConfig = serde_json::from_str(
            r#"{"pfs":[{"pf":"0000:03:00.0","num_vfs":2,"vram_quota_mib":1024,"vfs":{"2":{"exec_quantum_ms":8}}}]}"#,
        )
        .unwrap();
        assert!(c.apply_on_boot);
        assert_eq!((c.settle_timeout_s, c.generations_keep), (15, 10));
        assert_eq!(c.pfs[0].vf_driver, VfDriver::Auto);
        assert!(!c.pfs[0].autoprobe);
        assert_eq!(c.pfs[0].for_vf(2).vram_quota_mib, Some(1024));
        assert_eq!(c.pfs[0].for_vf(2).exec_quantum_ms, Some(8));
        assert!(c.validate().is_ok());
        let mut bad = c.clone();
        bad.pfs[0].vfs.insert(3, VfSpec::default());
        assert!(bad.validate().unwrap_err().to_string().contains("outside"));
        let mut dup = c.clone();
        dup.pfs.push(dup.pfs[0].clone());
        assert!(dup.validate().unwrap_err().to_string().contains("twice"));
    }

    #[test]
    fn plan_creates_vfs_in_kernel_order_and_is_idempotent() {
        let t = tempdir().unwrap();
        let p = paths(&t);
        driver_dirs(&p);
        xe_pf(&p.sys, &p.dev, "0000:03:00.0", 7, 0, "vfio-pci");
        let o = observe(&p, "0000:03:00.0");
        assert_eq!(o.total_vfs, 7);
        assert!(o.admin);
        let pl = plan(&cfg(2), &[o], &Usage::default());
        assert!(pl.errors.is_empty(), "{:?}", pl.errors);
        let acts: Vec<&str> = pl.steps.iter().map(|s| s.action.as_str()).collect();
        assert_eq!(acts[..2], ["vram_quota", "vram_quota"], "{acts:?}");
        let en = acts.iter().position(|a| *a == "enable_vfs").unwrap();
        assert!(en == 2 && acts[3] == "bind_vf", "{acts:?}");
        assert_eq!(pl.steps[0].value, (2048u64 * 1024 * 1024).to_string());
        assert_eq!(pl.steps[3].value, "xe-vfio-pci", "auto picks xe-vfio-pci");
    }

    #[test]
    fn already_matching_host_has_an_empty_plan() {
        let t = tempdir().unwrap();
        let p = paths(&t);
        driver_dirs(&p);
        xe_pf(&p.sys, &p.dev, "0000:03:00.0", 7, 2, "xe-vfio-pci");
        for i in 1..=2 {
            w(
                &p.pci_dev("0000:03:00.0")
                    .join(format!("sriov_admin/vf{i}/profile/vram_quota")),
                &format!("{}\n", 2048u64 * 1024 * 1024),
            );
        }
        let o = observe(&p, "0000:03:00.0");
        let pl = plan(&cfg(2), &[o], &Usage::default());
        assert!(pl.in_sync(), "{pl:?}");
    }

    #[test]
    fn refuses_missing_incapable_and_oversized_pfs_with_reasons() {
        let t = tempdir().unwrap();
        let p = paths(&t);
        driver_dirs(&p);
        dev(
            &p.sys,
            "0000:01:00.0",
            "0x10de",
            "0x030000",
            Some("nvidia"),
            "1",
        );
        xe_pf(&p.sys, &p.dev, "0000:03:00.0", 4, 0, "vfio-pci");
        let mut c = cfg(9);
        c.pfs.push(PfSpec {
            pf: "0000:01:00.0".into(),
            num_vfs: 1,
            ..c.pfs[0].clone()
        });
        c.pfs.push(PfSpec {
            pf: "0000:09:00.0".into(),
            num_vfs: 1,
            ..c.pfs[0].clone()
        });
        let obs: Vec<Observed> = c.pfs.iter().map(|x| observe(&p, &x.pf)).collect();
        let pl = plan(&c, &obs, &Usage::default());
        assert_eq!(pl.errors.len(), 3, "{:?}", pl.errors);
        assert!(pl.errors[0].contains("at most 4"));
        assert!(
            pl.errors[1].contains("not SR-IOV capable")
                && pl.errors[1].contains("passthrough only")
        );
        assert!(pl.errors[2].contains("no such PCI device"));
        assert!(pl.steps.is_empty());
    }

    #[test]
    fn per_vf_resources_refused_on_a_pf_without_sriov_admin() {
        let t = tempdir().unwrap();
        let p = paths(&t);
        driver_dirs(&p);
        dev(
            &p.sys,
            "0000:41:00.0",
            "0x10de",
            "0x030200",
            Some("nvidia"),
            "5",
        );
        let d = p.pci_dev("0000:41:00.0");
        w(&d.join("sriov_totalvfs"), "16\n");
        w(&d.join("sriov_numvfs"), "0\n");
        let c = SriovConfig {
            pfs: vec![PfSpec {
                pf: "0000:41:00.0".into(),
                num_vfs: 4,
                vf_driver: VfDriver::VfioPci,
                autoprobe: false,
                all: VfSpec {
                    vram_quota_mib: Some(1024),
                    ..Default::default()
                },
                vfs: BTreeMap::new(),
            }],
            ..Default::default()
        };
        let pl = plan(&c, &[observe(&p, "0000:41:00.0")], &Usage::default());
        assert!(
            pl.errors[0].contains("no per-VF resource controls"),
            "{:?}",
            pl.errors
        );
        // without per-VF resources it is a plain VF count + bind
        let mut c2 = c.clone();
        c2.pfs[0].all = VfSpec::default();
        let pl = plan(&c2, &[observe(&p, "0000:41:00.0")], &Usage::default());
        assert!(pl.errors.is_empty());
        assert_eq!(pl.steps.iter().filter(|s| s.action == "bind_vf").count(), 4);
    }

    #[test]
    fn missing_vfio_driver_is_an_error() {
        let t = tempdir().unwrap();
        let p = paths(&t);
        xe_pf(&p.sys, &p.dev, "0000:03:00.0", 4, 0, "vfio-pci");
        let mut c = cfg(1);
        c.pfs[0].vf_driver = VfDriver::XeVfioPci;
        let pl = plan(&c, &[observe(&p, "0000:03:00.0")], &Usage::default());
        assert!(pl.errors[0].contains("xe-vfio-pci driver is not loaded"));
    }

    #[test]
    fn shrinking_or_recreating_never_pulls_a_vf_from_a_vm() {
        let t = tempdir().unwrap();
        let p = paths(&t);
        driver_dirs(&p);
        xe_pf(&p.sys, &p.dev, "0000:03:00.0", 7, 2, "xe-vfio-pci");
        let o = observe(&p, "0000:03:00.0");
        // lower to 1 while VF 2 belongs to a stopped VM: refused
        let mut u = Usage::default();
        u.assigned.insert("0000:03:00.2".into(), "vm2".into());
        let pl = plan(&cfg(1), &[o.clone()], &u);
        assert!(
            pl.errors[0].contains("assigned to VM vm2"),
            "{:?}",
            pl.errors
        );
        // quota change re-creates all VFs: refused while one runs
        let mut u = Usage::default();
        u.assigned.insert("0000:03:00.1".into(), "vm1".into());
        u.running.insert("0000:03:00.1".into());
        let pl = plan(&cfg(2), &[o.clone()], &u);
        assert!(pl.errors[0].contains("running VM vm1"), "{:?}", pl.errors);
        // same with the VM stopped: allowed, recreate
        u.running.clear();
        let pl = plan(&cfg(2), &[o], &u);
        assert!(pl.errors.is_empty(), "{:?}", pl.errors);
        assert_eq!(pl.steps[0].action, "disable_vfs");
    }

    #[test]
    fn apply_writes_the_sysfs_files_in_order_and_records_generations() {
        let t = tempdir().unwrap();
        let p = paths(&t);
        driver_dirs(&p);
        // 2 VFs already exist in the fixture so `wait_vfs` finds them
        xe_pf(&p.sys, &p.dev, "0000:03:00.0", 7, 2, "vfio-pci");
        let pf = p.pci_dev("0000:03:00.0");
        let store = SriovStore::new(t.path().join("state"));
        let mut c = cfg(2);
        c.pfs[0].all.exec_quantum_ms = Some(8);
        c.pfs[0].vfs.insert(
            2,
            VfSpec {
                vram_quota_mib: Some(1024),
                ..Default::default()
            },
        );
        let dry = reconcile(&p, &store, &c, &Usage::default(), true, "apply").unwrap();
        assert!(dry.ok && dry.results.is_empty() && !dry.plan.steps.is_empty());
        assert!(store.generations().is_empty(), "dry run records nothing");
        let applier = Applier {
            verify_binding: false,
            ..Applier::new(p.clone(), 1)
        };
        let res = applier.apply(&dry.plan);
        assert!(res.iter().all(|r| r.status == "applied"), "{res:?}");
        assert_eq!(read(&pf.join("sriov_numvfs")), "2");
        assert_eq!(
            read(&pf.join("sriov_admin/vf1/profile/vram_quota")),
            (2048u64 << 20).to_string()
        );
        assert_eq!(
            read(&pf.join("sriov_admin/vf2/profile/vram_quota")),
            (1024u64 << 20).to_string()
        );
        assert_eq!(
            read(&pf.join("sriov_admin/vf1/profile/exec_quantum_ms")),
            "8"
        );
        let vf1 = p.pci_dev("0000:03:00.1");
        assert_eq!(read(&vf1.join("driver_override")), "xe-vfio-pci");
        assert_eq!(read(&p.sys.join("bus/pci/drivers_probe")), "0000:03:00.2");
    }

    #[test]
    fn a_failed_step_skips_the_rest_of_its_pf_and_records_nothing() {
        let t = tempdir().unwrap();
        let p = paths(&t);
        driver_dirs(&p);
        xe_pf(&p.sys, &p.dev, "0000:03:00.0", 7, 0, "vfio-pci");
        std::fs::remove_file(p.pci_dev("0000:03:00.0").join("sriov_drivers_autoprobe")).unwrap();
        // VFs never appear (nothing creates virtfn links) and the timeout is 0
        let store = SriovStore::new(t.path().join("state"));
        let mut c = cfg(2);
        c.settle_timeout_s = 0;
        let r = reconcile(&p, &store, &c, &Usage::default(), false, "apply").unwrap();
        assert!(!r.ok);
        let failed = r.results.iter().find(|x| x.status == "failed").unwrap();
        assert_eq!(failed.step.action, "enable_vfs");
        assert!(failed.message.contains("only 0 of 2 VFs appeared"));
        assert!(r.results.iter().any(|x| x.status == "skipped"));
        assert!(store.generations().is_empty());
        assert_eq!(store.config(), SriovConfig::default());
    }

    #[test]
    fn generations_are_numbered_and_trimmed() {
        let t = tempdir().unwrap();
        let store = SriovStore::new(t.path());
        let mut c = SriovConfig {
            generations_keep: 2,
            ..Default::default()
        };
        for _ in 0..3 {
            store.record(&c, "apply").unwrap();
            c.settle_timeout_s += 1;
        }
        let g = store.generations();
        assert_eq!(g.iter().map(|x| x.id).collect::<Vec<_>>(), vec![2, 3]);
        assert_eq!(g[0].config.settle_timeout_s, 16);
        // an unreadable/invalid stored config falls back to the default
        std::fs::write(t.path().join("sriov.json"), "{").unwrap();
        assert_eq!(store.config(), SriovConfig::default());
    }
}
