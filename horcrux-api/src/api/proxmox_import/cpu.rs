//! Proxmox CPU model -> a model stock QEMU knows.
//!
//! Proxmox's pve-qemu ships models stock QEMU does not have
//! (`x86-64-v2`, `x86-64-v2-AES`, `x86-64-v3`, `x86-64-v4`) and resolves
//! `custom-*` models from its own `cpu-models.conf`; a VM started with one
//! of them on stock QEMU fails with "unable to find CPU model". Each one maps
//! to the upstream `qemu64` base with Proxmox's own feature flags;
//! `host` stays `host` where KVM is available and becomes `max` (the TCG
//! equivalent) where it is not. Every other model name is stock QEMU's own
//! and passes through. Flags are kept (see [`parse_flags`]).

use std::collections::BTreeMap;

/// The result of mapping one Proxmox `cpu:` setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpuMap {
    /// Model for `hardware.cpu_type`.
    pub model: String,
    /// Feature flags for `hardware.cpu_flags`.
    pub flags: Vec<String>,
    /// Why the model/flags differ from the Proxmox line (empty = unchanged).
    pub notes: Vec<String>,
    /// Proxmox options that have no equivalent here.
    pub dropped: Vec<String>,
}

/// Proxmox-only model -> (stock model, what the level means).
// Match the built-in definitions in Proxmox qemu-server CPUConfig.pm.
const PVE_ONLY: &[(&str, &str)] = &[
    ("x86-64-v2", "+popcnt;+pni;+sse4.1;+sse4.2;+ssse3"),
    ("x86-64-v2-AES", "+aes;+popcnt;+pni;+sse4.1;+sse4.2;+ssse3"),
    ("x86-64-v3", "+aes;+popcnt;+pni;+sse4.1;+sse4.2;+ssse3;+avx;+avx2;+bmi1;+bmi2;+f16c;+fma;+abm;+movbe;+xsave"),
    ("x86-64-v4", "+aes;+popcnt;+pni;+sse4.1;+sse4.2;+ssse3;+avx;+avx2;+bmi1;+bmi2;+f16c;+fma;+abm;+movbe;+xsave;+avx512f;+avx512bw;+avx512cd;+avx512dq;+avx512vl"),
];

/// The model Proxmox uses for a config without a `cpu:` key, mapped.
pub const DEFAULT_PVE_MODEL: &str = "kvm64";

/// Map Proxmox's `cpu:` option map (`kv` of the value: key `""` or
/// `cputype` is the model) for a host with (`kvm`) or without KVM.
pub fn map_cpu(opts: &BTreeMap<String, String>, kvm: bool) -> CpuMap {
    let model = opts
        .get("")
        .or(opts.get("cputype"))
        .cloned()
        .unwrap_or_else(|| DEFAULT_PVE_MODEL.to_string());
    let (mut flags, dropped, mut notes) = parse_flags(opts);
    if let Some((_, features)) = PVE_ONLY.iter().find(|(from, _)| *from == model) {
        let mut baseline: Vec<String> = features.split(';').map(str::to_string).collect();
        baseline.retain(|base| !flags.iter().any(|flag| flag.get(1..) == base.get(1..)));
        baseline.append(&mut flags);
        flags = baseline;
    }
    let (out, why) = map_model(&model, kvm);
    if let Some(w) = why {
        notes.insert(0, w);
    }
    CpuMap {
        model: out,
        flags,
        notes,
        dropped,
    }
}

/// The model alone (`pve` name -> stock name, reason when it changed).
pub fn map_model(model: &str, kvm: bool) -> (String, Option<String>) {
    if PVE_ONLY.iter().any(|(from, _)| *from == model) {
        return (
            "qemu64".into(),
            Some(format!(
                "{model} is a Proxmox-specific model; qemu64 and its Proxmox feature flags are used"
            )),
        );
    }
    if model == "host" {
        return if kvm {
            ("host".into(), None)
        } else {
            (
                "max".into(),
                Some("host needs KVM and this node has no /dev/kvm: max is the emulator's all-features model".into()),
            )
        };
    }
    if model.starts_with("custom-") {
        return (
            "max".into(),
            Some(format!(
                "{model} is defined in Proxmox's cpu-models.conf, which Horcrux does not read: max is used; pick a model under Hardware"
            )),
        );
    }
    (model.to_string(), None)
}

/// `flags=+aes;-pcid`, `hidden=1` -> QEMU flags; the rest is reported.
/// Returns (flags, dropped options, notes).
pub fn parse_flags(opts: &BTreeMap<String, String>) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut flags: Vec<String> = Vec::new();
    let mut dropped = Vec::new();
    let mut notes = Vec::new();
    if let Some(f) = opts.get("flags") {
        for t in f.split(';').map(str::trim).filter(|t| !t.is_empty()) {
            if horcrux_common::valid_cpu_flag(t) && t.starts_with(['+', '-']) {
                flags.push(t.to_string());
            } else {
                dropped.push(format!("flags={t}"));
            }
        }
    }
    if opts.get("hidden").is_some_and(|v| v == "1") {
        flags.push("kvm=off".into());
        notes.push("hidden=1 becomes the QEMU flag kvm=off".into());
    }
    for (k, v) in opts {
        if !matches!(k.as_str(), "" | "cputype" | "flags" | "hidden") {
            dropped.push(format!("{k}={v}"));
        }
    }
    (flags, dropped, notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(s: &str) -> BTreeMap<String, String> {
        super::super::conf::kv(s)
    }

    #[test]
    fn pve_only_models_map_to_stock_levels() {
        for pve in ["x86-64-v2", "x86-64-v2-AES", "x86-64-v3", "x86-64-v4"] {
            for kvm in [true, false] {
                let m = map_cpu(&opts(pve), kvm);
                assert_eq!(m.model, "qemu64");
                assert!(!m.flags.is_empty());
                assert!(m.notes[0].contains(pve) && m.notes[0].contains("qemu64"));
            }
        }
    }

    #[test]
    fn explicit_flags_override_the_proxmox_baseline() {
        let m = map_cpu(&opts("x86-64-v2-AES,flags=-aes;+pcid"), true);
        assert_eq!(m.model, "qemu64");
        assert!(m.flags.contains(&"-aes".to_string()));
        assert!(!m.flags.contains(&"+aes".to_string()));
        assert!(m.flags.contains(&"+sse4.2".to_string()));
        assert!(m.flags.contains(&"+pcid".to_string()));
    }

    #[test]
    fn no_cpu_key_uses_the_mapped_default() {
        let m = map_cpu(&BTreeMap::new(), true);
        assert_eq!(m.model, "kvm64");
        assert!(!m.model.starts_with("x86-64"));
    }

    #[test]
    fn host_is_host_with_kvm_else_max() {
        assert_eq!(map_cpu(&opts("host"), true).model, "host");
        assert!(map_cpu(&opts("host"), true).notes.is_empty());
        let m = map_cpu(&opts("host"), false);
        assert_eq!(m.model, "max");
        assert!(m.notes[0].contains("no /dev/kvm"));
    }

    #[test]
    fn stock_models_pass_through_and_custom_becomes_max() {
        for s in [
            "kvm64",
            "qemu64",
            "max",
            "EPYC-v4",
            "Skylake-Client-v4",
            "Haswell",
        ] {
            let m = map_cpu(&opts(s), false);
            assert_eq!((m.model.as_str(), m.notes.is_empty()), (s, true));
        }
        let m = map_cpu(&opts("custom-mine"), true);
        assert_eq!(m.model, "max");
        assert!(m.notes[0].contains("cpu-models.conf"));
    }

    #[test]
    fn flags_are_kept_and_the_rest_is_reported() {
        let m = map_cpu(
            &opts(
                "x86-64-v3,flags=+aes;-pcid;+md-clear,hidden=1,hv-vendor-id=proxmox,phys-bits=host",
            ),
            true,
        );
        assert_eq!(m.model, "qemu64");
        assert!(m.flags.contains(&"+aes".to_string()));
        assert!(m.flags.contains(&"-pcid".to_string()));
        assert!(m.flags.contains(&"+md-clear".to_string()));
        assert!(m.flags.contains(&"kvm=off".to_string()));
        assert_eq!(m.dropped, ["hv-vendor-id=proxmox", "phys-bits=host"]);
        // a flag with a comma-injection shape is dropped, not passed on
        let m = map_cpu(&opts("kvm64,flags=+aes;pcid;+a=b"), true);
        assert_eq!(m.flags, ["+aes"]);
        assert_eq!(m.dropped, ["flags=pcid", "flags=+a=b"]);
    }
}
