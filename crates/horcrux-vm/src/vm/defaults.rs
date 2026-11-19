//! Node-wide defaults for VMs that are created from now on.
//!
//! The one default today is the CPU model of x86 VMs. QEMU's own default
//! (`qemu64`, what a VM gets when no `-cpu` is passed) lacks SSE4.2/POPCNT,
//! so Windows 11 24H2 hangs at the boot logo on it and x86-64-v2 Linux
//! distributions fail the same way. A new x86 VM therefore gets the node's
//! default CPU model (`x86-64-v2-AES` unless configured) written into its own
//! configuration at create time:
//!
//! * a later change of the default never changes a guest that exists;
//! * VMs created by an older release keep `cpu_type = None`, which still
//!   starts with no `-cpu` (their exact command line), and the UI offers a
//!   one-click switch on their hardware page ([`legacy_cpu_model`]);
//! * VMs with Hyper-V enlightenments (`adv.hyperv`, Windows gaming guests)
//!   keep taking `[passthrough].hyperv_cpu_model` (default `host`) when they
//!   name no model, see `tuning::cpu_arg`.
//!
//! Where the value comes from, highest priority first:
//! 1. the run-time setting (`PUT /api/vms/defaults`, Settings > Configuration
//!    or the `vm_defaults` section of a declarative spec), kept in the node's
//!    database and handed to this module at start-up and on every change;
//! 2. `[vm_defaults] cpu_model` in `/etc/horcrux/config.toml` (or
//!    `HORCRUX_VM_DEFAULT_CPU_MODEL`);
//! 3. the built-in `x86-64-v2-AES`.
use horcrux_common::{valid_cpu_type, VmArchitecture, VmConfig, VmHypervisor};
use horcrux_core::config::{HorcruxConfig, DEFAULT_VM_CPU_MODEL};
use std::sync::{OnceLock, RwLock};

/// Where the effective default came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Set at run time (API, UI or declarative spec).
    Api,
    /// `[vm_defaults]` in config.toml or the environment.
    Config,
    /// The built-in default.
    Builtin,
}

static OVERRIDE: RwLock<Option<String>> = RwLock::new(None);

/// What the node's config file / environment says (read once).
fn file_value() -> Option<String> {
    static FILE: OnceLock<Option<String>> = OnceLock::new();
    FILE.get_or_init(|| {
        let m = HorcruxConfig::load().vm_defaults.cpu_model;
        if m == DEFAULT_VM_CPU_MODEL {
            None
        } else if valid_cpu_type(&m) {
            Some(m)
        } else {
            tracing::warn!(
                "[vm_defaults] cpu_model {m:?} is not a plausible QEMU CPU model; using {DEFAULT_VM_CPU_MODEL}"
            );
            None
        }
    })
    .clone()
}

/// Set (or clear, with `None`) the run-time default. Called at start-up with
/// the stored value and whenever the setting changes.
pub fn set_cpu_model_override(model: Option<String>) {
    *OVERRIDE.write().unwrap_or_else(|e| e.into_inner()) = model.filter(|m| valid_cpu_type(m));
}

/// The default CPU model of new x86 VMs and where it comes from.
pub fn cpu_model() -> (String, Source) {
    resolve(
        OVERRIDE.read().unwrap_or_else(|e| e.into_inner()).clone(),
        file_value(),
    )
}

fn resolve(run_time: Option<String>, file: Option<String>) -> (String, Source) {
    match (run_time, file) {
        (Some(m), _) => (m, Source::Api),
        (None, Some(m)) => (m, Source::Config),
        (None, None) => (DEFAULT_VM_CPU_MODEL.to_string(), Source::Builtin),
    }
}

/// QEMU has no `x86-64-v2-AES`/`x86-64-v3` models of its own (they are
/// libvirt's and Proxmox's names for a baseline, and `-cpu x86-64-v2-AES`
/// fails with "unable to find CPU model"). This expands them the way Proxmox
/// VE does: QEMU's portable `qemu64` plus the baseline's feature flags, so
/// every host that has the features can run the guest and migrate it. Any
/// other model name is passed through unchanged.
///
/// * `x86-64-v1` = `qemu64`
/// * `x86-64-v2` = + SSE3/SSSE3/SSE4.1/SSE4.2/POPCNT; `x86-64-v2-AES` adds AES
/// * `x86-64-v3` = v2-AES + AVX/AVX2/BMI1/BMI2/F16C/FMA/LZCNT/MOVBE/XSAVE
/// * `x86-64-v4` = v3 + AVX-512 F/BW/CD/DQ/VL
pub fn expand_cpu_model(model: &str) -> String {
    const V2: &str = ",+popcnt,+pni,+ssse3,+sse4.1,+sse4.2";
    const AES: &str = ",+aes";
    const V3: &str = ",+avx,+avx2,+bmi1,+bmi2,+f16c,+fma,+abm,+movbe,+xsave";
    const V4: &str = ",+avx512f,+avx512bw,+avx512cd,+avx512dq,+avx512vl";
    match model {
        "x86-64-v1" => "qemu64".to_string(),
        "x86-64-v2" => format!("qemu64{V2}"),
        "x86-64-v2-AES" => format!("qemu64{V2}{AES}"),
        "x86-64-v3" => format!("qemu64{V2}{AES}{V3}"),
        "x86-64-v4" => format!("qemu64{V2}{AES}{V3}{V4}"),
        other => other.to_string(),
    }
}

/// Is `model` one the node can start: listed by QEMU (`known`, from
/// [`parse_cpu_help`]) or one of the baselines [`expand_cpu_model`] builds.
pub fn model_is_known(model: &str, known: &[String]) -> bool {
    expand_cpu_model(model) != model || model == "x86-64-v1" || known.iter().any(|k| k == model)
}

/// The CPU model names in `qemu-system-x86_64 -cpu help`. QEMU 7 and older
/// print `x86 <name> <description>` lines, newer ones `  <name> <description>`
/// below an `Available CPUs:` header; the flag lists (`Recognized CPUID
/// flags:`) and aliases noted in parentheses are not models.
pub fn parse_cpu_help(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        if line.starts_with("Recognized") {
            break;
        }
        let l = line.trim_start();
        let l = l.strip_prefix("x86 ").unwrap_or(l).trim_start();
        if let Some(name) = l.split_whitespace().next() {
            if valid_cpu_type(name) && name != "Available" && name != "CPUs:" {
                out.push(name.to_string());
            }
        }
    }
    out
}

/// Does this VM start without `-cpu` (QEMU's bare `qemu64`)? True for x86
/// QEMU VMs created before the default existed; the hardware page warns about
/// them.
pub fn legacy_cpu_model(vm: &VmConfig) -> bool {
    vm.hypervisor == VmHypervisor::Qemu
        && vm.architecture == VmArchitecture::X86_64
        && vm.hardware.cpu_type.is_none()
        && !vm.hardware.adv.hyperv
}

/// Fill what a create request leaves open. Returns the CPU model written, if
/// any. Only x86 QEMU VMs are touched (the model names are x86 ones), and
/// Hyper-V VMs keep their own model rule.
pub fn apply_create_defaults(vm: &mut VmConfig) -> Option<String> {
    if !legacy_cpu_model(vm) {
        return None;
    }
    let (model, _) = cpu_model();
    vm.hardware.cpu_type = Some(model.clone());
    Some(model)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vm() -> VmConfig {
        VmConfig {
            id: "d1".into(),
            name: "d1".into(),
            hypervisor: VmHypervisor::Qemu,
            memory: 256,
            cpus: 2,
            disk_size: 1,
            status: horcrux_common::VmStatus::Stopped,
            architecture: VmArchitecture::X86_64,
            disks: vec![],
            display: Default::default(),
            hardware: Default::default(),
        }
    }

    #[test]
    fn cpu_help_lists_models_in_both_qemu_formats() {
        let new = "Available CPUs:\n  486                  (alias configured by machine type)\n  Skylake-Client-v4    Intel Core Processor\n  x86-64-v2-AES        Intel/AMD\n  host                 processor with all supported host features\n\nRecognized CPUID flags:\n  aes avx\n";
        let m = parse_cpu_help(new);
        assert!(m.contains(&"x86-64-v2-AES".to_string()) && m.contains(&"host".to_string()));
        assert!(!m.contains(&"aes".to_string()) && !m.contains(&"Available".to_string()));
        let old = "x86 qemu64  QEMU Virtual CPU\nx86 Haswell-v4  Intel\n";
        assert_eq!(parse_cpu_help(old), vec!["qemu64", "Haswell-v4"]);
        assert!(parse_cpu_help("").is_empty());
    }

    #[test]
    fn baseline_names_expand_to_qemu64_plus_flags() {
        // QEMU itself has no x86-64-v2-AES model: passing the name through
        // makes it refuse to start ("unable to find CPU model")
        assert_eq!(expand_cpu_model("x86-64-v1"), "qemu64");
        assert_eq!(
            expand_cpu_model("x86-64-v2-AES"),
            "qemu64,+popcnt,+pni,+ssse3,+sse4.1,+sse4.2,+aes"
        );
        let v3 = expand_cpu_model("x86-64-v3");
        assert!(
            v3.starts_with("qemu64,+popcnt") && v3.contains(",+aes,+avx,+avx2,+bmi1"),
            "{v3}"
        );
        assert!(expand_cpu_model("x86-64-v4").contains("+avx512vl"));
        // real QEMU models and anything else go through untouched
        for m in ["host", "max", "qemu64", "EPYC-v4", "Skylake-Server"] {
            assert_eq!(expand_cpu_model(m), m);
        }
        let known = vec!["host".to_string(), "EPYC-v4".to_string()];
        assert!(model_is_known("x86-64-v2-AES", &known) && model_is_known("EPYC-v4", &known));
        assert!(!model_is_known("no-such-cpu", &known));
    }

    #[test]
    fn resolution_order_is_run_time_then_file_then_builtin() {
        assert_eq!(
            resolve(Some("host".into()), Some("x86-64-v3".into())),
            ("host".into(), Source::Api)
        );
        assert_eq!(
            resolve(None, Some("x86-64-v3".into())),
            ("x86-64-v3".into(), Source::Config)
        );
        assert_eq!(
            resolve(None, None),
            ("x86-64-v2-AES".into(), Source::Builtin)
        );
    }

    // The run-time override is process-global: tests that change it live in
    // tests/cpu_default.rs (their own process), so the unit tests here and in
    // qemu.rs only ever see the default.
    #[test]
    fn create_defaults_fill_only_what_is_open() {
        assert_eq!(cpu_model().0, DEFAULT_VM_CPU_MODEL);

        // an open x86 VM gets the default, written into its own config
        let mut v = vm();
        assert!(legacy_cpu_model(&v));
        assert_eq!(
            apply_create_defaults(&mut v).as_deref(),
            Some("x86-64-v2-AES")
        );
        assert_eq!(v.hardware.cpu_type.as_deref(), Some("x86-64-v2-AES"));
        assert!(!legacy_cpu_model(&v));

        // an explicit model is never replaced
        let mut v = vm();
        v.hardware.cpu_type = Some("qemu64".into());
        assert_eq!(apply_create_defaults(&mut v), None);
        assert_eq!(v.hardware.cpu_type.as_deref(), Some("qemu64"));

        // other architectures and hypervisors are left alone
        let mut v = vm();
        v.architecture = VmArchitecture::Aarch64;
        assert_eq!(apply_create_defaults(&mut v), None);
        assert!(v.hardware.cpu_type.is_none());
        let mut v = vm();
        v.hypervisor = VmHypervisor::Incus;
        assert_eq!(apply_create_defaults(&mut v), None);

        // Hyper-V guests keep [passthrough].hyperv_cpu_model
        let mut v = vm();
        v.hardware.adv.hyperv = true;
        assert_eq!(apply_create_defaults(&mut v), None);
        assert!(v.hardware.cpu_type.is_none());
    }
}
