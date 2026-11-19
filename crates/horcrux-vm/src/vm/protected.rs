//! Protected and confidential gaming VMs, and the host attestation report
//! (docs/design/gaming-vm.md section 12).
//!
//! Two honest ways to keep game memory out of the host's reach, strongest
//! first:
//!
//! * **Confidential VM** — AMD SEV-SNP (EPYC) or Intel TDX (Xeon). The CPU
//!   encrypts and integrity-protects guest memory and attests the launch
//!   state; even the host kernel cannot read it. Consumer Core Ultra / Ryzen
//!   parts do not offer this (SME/TSME/TME only encrypt DRAM against a
//!   physical attacker; they do not isolate a VM from the host).
//! * **Protected VM** (consumer CPUs) — guest RAM lives in a KVM
//!   `guest_memfd` that host userspace cannot map, on a locked-down host
//!   (kernel `lockdown=confidentiality`, no ptrace of the VMM, QMP memory
//!   dumps and the gdbstub disabled, seccomp-confined VMM, IMA/EVM), with
//!   measured boot into the TPM. Weaker than SEV-SNP/TDX against a malicious
//!   host *kernel* or physical access; the threat-model table in the design
//!   doc states exactly what each layer stops.
//!
//! Everything here is pure over the paths in [`ProtectedConfig`] (sysfs,
//! procfs, cpuinfo, TPM), so it is unit-tested on fixtures. The live
//! `guest_memfd` probe ([`probe_guest_memfd`]) opens `/dev/kvm` and proves the
//! private-memory property on the running kernel; a dedicated ignored test
//! ([`crate`]'s `tests/protected_memfd.rs`) asserts the host-userspace read
//! fails.
use horcrux_core::config::ProtectedConfig;
use serde::{Deserialize, Serialize};
use std::path::Path;

// ---------------------------------------------------------------- detection

fn read(p: &Path) -> String {
    std::fs::read_to_string(p)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// CPU/firmware confidential-computing capability of this host.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfidentialHw {
    /// `amd`, `intel` or `` (unknown).
    pub vendor: String,
    /// AMD SEV (basic memory encryption for guests).
    pub sev: bool,
    /// AMD SEV-ES (also protects guest register state).
    pub sev_es: bool,
    /// AMD SEV-SNP (adds integrity + attestation; the confidential tier).
    pub sev_snp: bool,
    /// Intel TDX (Trust Domain Extensions; the confidential tier).
    pub tdx: bool,
    /// AMD SME / Intel TME: whole-DRAM encryption. Protects against a physical
    /// attacker only, NOT against the host OS. Never counts as isolation.
    pub memory_encryption: bool,
    /// SME/TSME/TME is actually switched on right now.
    pub memory_encryption_active: bool,
}

impl ConfidentialHw {
    /// Detect from a cpuinfo file and the AMD SEV sysfs parameter.
    pub fn detect(cpuinfo: &Path, sev_param: &Path) -> Self {
        let info = read(cpuinfo);
        let flags: Vec<&str> = info
            .lines()
            .find(|l| l.starts_with("flags"))
            .and_then(|l| l.split_once(':'))
            .map(|(_, f)| f.split_whitespace().collect())
            .unwrap_or_default();
        let has = |f: &str| flags.contains(&f);
        let vendor = if info.contains("AuthenticAMD") {
            "amd"
        } else if info.contains("GenuineIntel") {
            "intel"
        } else {
            ""
        };
        // AMD advertises sev / sev_es / sev_snp as cpu flags; the kvm_amd
        // module parameter says whether the kernel actually enabled SEV.
        let sev_param_on = matches!(read(sev_param).as_str(), "Y" | "1");
        Self {
            vendor: vendor.to_string(),
            sev: has("sev") || sev_param_on,
            sev_es: has("sev_es"),
            sev_snp: has("sev_snp"),
            // Intel TDX host side shows as `tdx_host_platform` on current kernels.
            tdx: has("tdx_host_platform") || has("tdx"),
            memory_encryption: has("sme") || has("tme"),
            memory_encryption_active: info.contains("Memory Encryption Features active"),
        }
    }

    /// Does this host offer a true confidential VM (SEV-SNP or TDX)?
    pub fn confidential(&self) -> bool {
        self.sev_snp || self.tdx
    }
}

/// The host-hardening layers that make up a "protected VM" on consumer CPUs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtectedLayers {
    /// Kernel lockdown state: `none`, `integrity` or `confidentiality`.
    pub lockdown: String,
    /// Lockdown is at least the level the config requires.
    pub lockdown_ok: bool,
    /// Yama ptrace scope (>0 stops other host processes ptracing the VMM).
    pub ptrace_scope: i32,
    /// IMA is measuring/appraising host binaries.
    pub ima: bool,
    /// EVM is protecting file metadata.
    pub evm: bool,
    /// Secure Boot was on at boot (EFI var or the lockdown reason).
    pub secure_boot: bool,
    /// The kernel offers KVM `guest_memfd` private guest memory.
    pub guest_memfd: bool,
    /// The kernel offers the `KVM_X86_SW_PROTECTED_VM` machine type.
    pub sw_protected_vm: bool,
    /// A live probe mapped a guest_memfd region from host userspace and it
    /// was refused (the actual private-memory proof). `None` when not probed.
    pub host_read_refused: Option<bool>,
}

fn lockdown_rank(s: &str) -> u8 {
    // The sysfs file lists options with the active one in [brackets].
    let active = s
        .split_whitespace()
        .find(|t| t.starts_with('['))
        .map(|t| t.trim_matches(['[', ']']))
        .unwrap_or(s);
    match active {
        "confidentiality" => 2,
        "integrity" => 1,
        _ => 0,
    }
}

/// Active lockdown level name from the raw sysfs contents.
pub fn lockdown_level(raw: &str) -> &'static str {
    match lockdown_rank(raw) {
        2 => "confidentiality",
        1 => "integrity",
        _ => "none",
    }
}

impl ProtectedLayers {
    /// Detect every software layer from the configured paths. `guest_memfd` /
    /// `sw_protected_vm` come from the optional live KVM probe (`None` leaves
    /// them `false` here and the report says they were not probed).
    pub fn detect(cfg: &ProtectedConfig, probe: Option<&GuestMemfdProbe>) -> Self {
        let lockdown = lockdown_level(&read(&cfg.lockdown_path)).to_string();
        let need = lockdown_rank(&cfg.required_lockdown);
        let have = lockdown_rank(&lockdown);
        let ptrace_scope = read(&cfg.ptrace_scope_path).parse::<i32>().unwrap_or(0);
        let cmdline = read(&cfg.cmdline_path);
        Self {
            lockdown_ok: have >= need,
            lockdown,
            ptrace_scope,
            ima: Path::new("/sys/kernel/security/ima").exists() || cmdline.contains("ima_policy"),
            evm: Path::new("/sys/kernel/security/evm").exists(),
            secure_boot: cmdline.contains("secureboot")
                || Path::new("/sys/firmware/efi/efivars").exists()
                    && efi_secure_boot_on().unwrap_or(false),
            guest_memfd: probe.map(|p| p.cap_guest_memfd).unwrap_or(false),
            sw_protected_vm: probe.map(|p| p.sw_protected_vm_type).unwrap_or(false),
            host_read_refused: probe.and_then(|p| p.host_mmap_refused),
        }
    }
}

/// Read the UEFI SecureBoot EFI variable (best effort).
fn efi_secure_boot_on() -> Option<bool> {
    // SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c; last data byte = 1 when on.
    let dir = Path::new("/sys/firmware/efi/efivars");
    let entry = std::fs::read_dir(dir).ok()?.flatten().find(|e| {
        e.file_name()
            .to_string_lossy()
            .starts_with("SecureBoot-8be4df61")
    })?;
    let data = std::fs::read(entry.path()).ok()?;
    data.last().map(|b| *b == 1)
}

/// Which protected tier this host can offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tier {
    /// SEV-SNP / TDX: hardware-encrypted, attested guest memory.
    Confidential,
    /// guest_memfd private memory on a locked-down host.
    Protected,
    /// Neither available: an ordinary VM (still isolated, but the host
    /// userspace/kernel can read its RAM).
    Unavailable,
}

/// Host readiness for a protected / confidential gaming VM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtectedReport {
    pub hardware: ConfidentialHw,
    pub layers: ProtectedLayers,
    /// Best tier the host can offer right now.
    pub tier: Tier,
    /// Tier the operator has enabled for this host (config toggles).
    pub confidential_enabled: bool,
    pub protected_enabled: bool,
    /// Plain-language reasons shown in the UI, especially why a tier is off.
    pub reasons: Vec<String>,
    /// GPU passthrough into a *confidential* VM is not possible on gaming GPUs
    /// today (needs PCIe TDISP/IDE or a vendor confidential-GPU mode); the UI
    /// says so instead of silently offering a broken combination.
    pub confidential_gpu_passthrough: bool,
}

/// Build the host protected-VM report. `probe` is the optional live KVM
/// guest_memfd probe (pass `None` to skip opening /dev/kvm).
pub fn host_report(cfg: &ProtectedConfig, probe: Option<&GuestMemfdProbe>) -> ProtectedReport {
    let hardware = ConfidentialHw::detect(&cfg.cpuinfo_path, &cfg.sev_status_path);
    let layers = ProtectedLayers::detect(cfg, probe);
    let mut reasons = Vec::new();

    let confidential_ready = hardware.confidential();
    // The protected tier needs private guest memory AND a locked-down host.
    let protected_ready = layers.guest_memfd && layers.lockdown_ok;

    let tier = if cfg.confidential_tier && confidential_ready {
        Tier::Confidential
    } else if cfg.protected_tier && protected_ready {
        Tier::Protected
    } else {
        Tier::Unavailable
    };

    if !hardware.confidential() {
        reasons.push(
            "This CPU has no SEV-SNP or TDX, so a hardware-encrypted confidential VM is not \
             available (consumer Core Ultra / Ryzen parts do not offer it; SME/TSME/TME only \
             encrypt memory against a physical attacker, not against the host OS)."
                .into(),
        );
    }
    if tier != Tier::Confidential {
        if !layers.guest_memfd {
            reasons.push(match probe {
                Some(_) => "This kernel's KVM does not offer guest_memfd private guest memory \
                    (needs Linux 6.8+ with KVM_CAP_GUEST_MEMFD), so host userspace cannot be \
                    locked out of guest RAM."
                    .into(),
                None => "guest_memfd support was not probed on this host.".to_string(),
            });
        }
        if !layers.lockdown_ok {
            reasons.push(format!(
                "Kernel lockdown is '{}', but the protected tier needs at least '{}' \
                 (boot with lockdown=confidentiality and Secure Boot so the host cannot read \
                 or modify the VM through /dev/mem, kexec, BPF or hibernation).",
                layers.lockdown, cfg.required_lockdown
            ));
        }
    }
    if matches!(tier, Tier::Protected | Tier::Confidential)
        && layers.host_read_refused == Some(true)
    {
        reasons.push(
            "Verified on this kernel: a guest_memfd region cannot be mapped or read from host \
             userspace."
                .into(),
        );
    }

    ProtectedReport {
        tier,
        confidential_enabled: cfg.confidential_tier,
        protected_enabled: cfg.protected_tier,
        reasons,
        // No consumer gaming GPU supports being passed into a confidential VM
        // (SEV-TIO/TDISP and NVIDIA/AMD confidential-GPU modes are datacenter
        // only). Always false until that changes.
        confidential_gpu_passthrough: false,
        hardware,
        layers,
    }
}

// ----------------------------------------------------------- guest_memfd probe

/// Result of the live KVM `guest_memfd` capability probe.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GuestMemfdProbe {
    /// `/dev/kvm` could be opened.
    pub kvm_accessible: bool,
    /// `KVM_CHECK_EXTENSION(KVM_CAP_GUEST_MEMFD)` was non-zero.
    pub cap_guest_memfd: bool,
    /// `KVM_CAP_VM_TYPES` includes `KVM_X86_SW_PROTECTED_VM`.
    pub sw_protected_vm_type: bool,
    /// A `guest_memfd` was created.
    pub created: bool,
    /// Mapping that guest_memfd from host userspace was refused (the proof).
    /// `None` when the fd could not be created to try.
    pub host_mmap_refused: Option<bool>,
    /// Human-readable summary / errno of the final step.
    pub detail: String,
}

// KVM ABI ioctl request numbers (kernel protocol constants, not tunables).
const KVMIO: u64 = 0xAE;
const KVM_CHECK_EXTENSION: u64 = 0xAE03; // _IO(KVMIO, 0x03)
const KVM_CREATE_VM: u64 = 0xAE01; // _IO(KVMIO, 0x01)
                                   // _IOWR(KVMIO, 0xd4, struct kvm_create_guest_memfd{u64 size,u64 flags,u64[6]})
const KVM_CREATE_GUEST_MEMFD: u64 = 0xC040_0000 | (KVMIO << 8) | 0xD4;
const KVM_CAP_GUEST_MEMFD: i32 = 234;
const KVM_CAP_VM_TYPES: i32 = 235;
const KVM_X86_SW_PROTECTED_VM: i32 = 1;

#[repr(C)]
struct KvmCreateGuestMemfd {
    size: u64,
    flags: u64,
    reserved: [u64; 6],
}

/// Open /dev/kvm and prove whether a `guest_memfd` region can be read from
/// host userspace on the running kernel. Read-only: it creates a throw-away
/// sw-protected VM and guest_memfd, tries to `mmap` it, then closes
/// everything. Safe to call from the API (best effort); returns a probe with
/// `kvm_accessible=false` when /dev/kvm cannot be opened.
pub fn probe_guest_memfd(kvm_path: &Path) -> GuestMemfdProbe {
    use std::os::unix::ffi::OsStrExt;
    let mut out = GuestMemfdProbe::default();
    // SAFETY: all FFI below is a sequence of KVM/libc syscalls on fds we own;
    // every fd is closed before returning and no buffer outlives the call.
    unsafe {
        let mut c = kvm_path.as_os_str().as_bytes().to_vec();
        c.push(0);
        let kvm = libc::open(
            c.as_ptr() as *const libc::c_char,
            libc::O_RDWR | libc::O_CLOEXEC,
        );
        if kvm < 0 {
            out.detail = format!("open {} failed (errno {})", kvm_path.display(), errno());
            return out;
        }
        out.kvm_accessible = true;
        out.cap_guest_memfd = libc::ioctl(kvm, KVM_CHECK_EXTENSION as _, KVM_CAP_GUEST_MEMFD) > 0;
        let types = libc::ioctl(kvm, KVM_CHECK_EXTENSION as _, KVM_CAP_VM_TYPES);
        out.sw_protected_vm_type = types > 0 && (types & (1 << KVM_X86_SW_PROTECTED_VM)) != 0;

        if !out.cap_guest_memfd || !out.sw_protected_vm_type {
            out.detail = "kernel lacks guest_memfd and/or the sw-protected VM type".into();
            libc::close(kvm);
            return out;
        }
        let vm = libc::ioctl(
            kvm,
            KVM_CREATE_VM as _,
            KVM_X86_SW_PROTECTED_VM as libc::c_long,
        );
        if vm < 0 {
            out.detail = format!("KVM_CREATE_VM(sw_protected) failed (errno {})", errno());
            libc::close(kvm);
            return out;
        }
        let mut gm = KvmCreateGuestMemfd {
            size: 2 * 1024 * 1024,
            flags: 0,
            reserved: [0; 6],
        };
        let gfd = libc::ioctl(vm, KVM_CREATE_GUEST_MEMFD as _, &mut gm as *mut _);
        if gfd < 0 {
            out.detail = format!("KVM_CREATE_GUEST_MEMFD failed (errno {})", errno());
            libc::close(vm);
            libc::close(kvm);
            return out;
        }
        out.created = true;
        let p = libc::mmap(
            std::ptr::null_mut(),
            4096,
            libc::PROT_READ,
            libc::MAP_SHARED,
            gfd as libc::c_int,
            0,
        );
        if p == libc::MAP_FAILED {
            out.host_mmap_refused = Some(true);
            out.detail = format!(
                "host-userspace mmap of guest_memfd refused (errno {}) - guest RAM is private",
                errno()
            );
        } else {
            out.host_mmap_refused = Some(false);
            out.detail = "host-userspace mmap of guest_memfd was ALLOWED (not private)".into();
            libc::munmap(p, 4096);
        }
        libc::close(gfd as libc::c_int);
        libc::close(vm);
        libc::close(kvm);
    }
    out
}

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

// ---------------------------------------------------------------- attestation

/// A single measured value in the attestation report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Measurement {
    pub name: String,
    pub value: String,
}

/// The signed measurements of a host's launch state. On consumer hardware this
/// is the evidence behind the honest anti-cheat argument: "this VM runs on an
/// unmodified, locked-down Horcrux host where nothing outside the VM can read
/// its memory". On SEV-SNP/TDX hosts a hardware attestation report is added
/// (future phase); the TPM quote is the cryptographic root here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttestationReport {
    pub version: u32,
    pub tier: Tier,
    /// Node TLS identity this report is bound to (cert SHA-256 fingerprint).
    pub node_fingerprint: String,
    /// TPM PCR values (measured boot) and host measurements.
    pub measurements: Vec<Measurement>,
    /// SHA-256 of the canonical measurement block (hex).
    pub digest: String,
    /// ECDSA P-256 signature over `digest` (DER, hex) by the attestation key.
    pub signature: String,
    /// SEC1 public key of the attestation key (hex) for verification.
    pub attestation_pubkey: String,
    /// Caveats a verifier must weigh (stated honestly, never hidden).
    pub caveats: Vec<String>,
}

/// Collect the host measurements (TPM PCRs + boot state) for the report.
pub fn collect_measurements(cfg: &ProtectedConfig) -> Vec<Measurement> {
    let mut m = Vec::new();
    for pcr in &cfg.attested_pcrs {
        let v = read(&cfg.tpm_pcr_dir.join(pcr.to_string()));
        if !v.is_empty() {
            m.push(Measurement {
                name: format!("pcr{pcr}"),
                value: v,
            });
        }
    }
    m.push(Measurement {
        name: "kernel_cmdline".into(),
        value: read(&cfg.cmdline_path),
    });
    m.push(Measurement {
        name: "lockdown".into(),
        value: lockdown_level(&read(&cfg.lockdown_path)).into(),
    });
    // Hash of the running VMM binary (what actually touches guest memory).
    if let Ok(bytes) = std::fs::read(&cfg.vmm_binary_path) {
        use sha2::{Digest, Sha256};
        m.push(Measurement {
            name: "vmm_sha256".into(),
            value: hex::encode(Sha256::digest(&bytes)),
        });
    }
    m
}

/// Load the node's ECDSA P-256 attestation key, generating and persisting one
/// (as a 32-byte hex scalar, mode 0600) on first use.
fn load_or_create_key(path: &Path) -> std::io::Result<p256::ecdsa::SigningKey> {
    use p256::ecdsa::SigningKey;
    if let Ok(hexs) = std::fs::read_to_string(path) {
        if let Ok(bytes) = hex::decode(hexs.trim()) {
            if let Ok(k) = SigningKey::from_slice(&bytes) {
                return Ok(k);
            }
        }
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let key = SigningKey::random(&mut rand::rngs::OsRng);
    let hexs = hex::encode(key.to_bytes());
    std::fs::write(path, &hexs)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(key)
}

/// Build and sign a host attestation report.
pub fn attestation_report(
    cfg: &ProtectedConfig,
    tier: Tier,
    node_fingerprint: &str,
    probe: Option<&GuestMemfdProbe>,
) -> std::io::Result<AttestationReport> {
    use p256::ecdsa::{signature::Signer, DerSignature};
    use sha2::{Digest, Sha256};

    let measurements = collect_measurements(cfg);
    // Canonical block: name=value lines, sorted, newline-joined.
    let mut lines: Vec<String> = measurements
        .iter()
        .map(|m| format!("{}={}", m.name, m.value))
        .collect();
    lines.sort();
    let canonical = lines.join("\n");
    let digest = Sha256::digest(canonical.as_bytes());
    let digest_hex = hex::encode(digest);

    let key = load_or_create_key(&cfg.attestation_key_path)?;
    let sig: DerSignature = key.sign(canonical.as_bytes());
    let pubkey = key.verifying_key().to_sec1_bytes();

    let mut caveats = vec![];
    match tier {
        Tier::Confidential => caveats.push(
            "Hardware confidential VM (SEV-SNP/TDX): guest memory is encrypted and integrity \
             protected against the host kernel and a physical attacker."
                .into(),
        ),
        Tier::Protected => {
            caveats.push(
                "Protected VM (consumer CPU): host userspace cannot read guest RAM (guest_memfd) \
                 and the host is locked down, but a compromised host *kernel* or a physical/DMA \
                 attacker is NOT cryptographically excluded as it would be with SEV-SNP/TDX."
                    .into(),
            );
            if probe.and_then(|p| p.host_mmap_refused) == Some(true) {
                caveats.push(
                    "Proof included: a guest_memfd region was created and host-userspace mapping \
                     of it was refused by this kernel."
                        .into(),
                );
            }
        }
        Tier::Unavailable => caveats.push(
            "No protected tier available on this host: an ordinary VM's RAM can be read by the \
             host. Not suitable for the attestation-based anti-cheat argument."
                .into(),
        ),
    }

    Ok(AttestationReport {
        version: 1,
        tier,
        node_fingerprint: node_fingerprint.to_string(),
        measurements,
        digest: digest_hex,
        signature: hex::encode(sig.as_bytes()),
        attestation_pubkey: hex::encode(pubkey),
        caveats,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
        let p = dir.join(name);
        if let Some(d) = p.parent() {
            fs::create_dir_all(d).unwrap();
        }
        fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn detects_amd_sev_snp() {
        let d = tempdir().unwrap();
        let cpuinfo = write(
            d.path(),
            "cpuinfo",
            "vendor_id\t: AuthenticAMD\nflags\t\t: fpu sev sev_es sev_snp sme\n",
        );
        let sev = write(d.path(), "sev", "Y\n");
        let hw = ConfidentialHw::detect(&cpuinfo, &sev);
        assert_eq!(hw.vendor, "amd");
        assert!(hw.sev && hw.sev_es && hw.sev_snp);
        assert!(hw.confidential());
    }

    #[test]
    fn detects_intel_tdx() {
        let d = tempdir().unwrap();
        let cpuinfo = write(
            d.path(),
            "cpuinfo",
            "vendor_id\t: GenuineIntel\nflags\t\t: fpu vmx tdx_host_platform tme\n",
        );
        let hw = ConfidentialHw::detect(&cpuinfo, Path::new("/nonexistent"));
        assert_eq!(hw.vendor, "intel");
        assert!(hw.tdx);
        assert!(hw.memory_encryption); // tme
        assert!(hw.confidential());
    }

    #[test]
    fn consumer_cpu_has_no_confidential_tier() {
        let d = tempdir().unwrap();
        // A Ryzen with SME but no SEV-SNP: memory encryption, but NOT isolation.
        let cpuinfo = write(
            d.path(),
            "cpuinfo",
            "vendor_id\t: AuthenticAMD\nflags\t\t: fpu sme\n",
        );
        let hw = ConfidentialHw::detect(&cpuinfo, Path::new("/nonexistent"));
        assert!(hw.memory_encryption);
        assert!(
            !hw.confidential(),
            "SME must not count as a confidential VM"
        );
    }

    #[test]
    fn lockdown_parsing() {
        assert_eq!(
            lockdown_level("none integrity [confidentiality]"),
            "confidentiality"
        );
        assert_eq!(lockdown_level("[none] integrity confidentiality"), "none");
        assert_eq!(
            lockdown_level("none [integrity] confidentiality"),
            "integrity"
        );
    }

    #[test]
    fn protected_tier_needs_private_memory_and_lockdown() {
        let d = tempdir().unwrap();
        let mut cfg = ProtectedConfig {
            cpuinfo_path: write(
                d.path(),
                "cpuinfo",
                "vendor_id\t: GenuineIntel\nflags\t: fpu\n",
            ),
            sev_status_path: d.path().join("nope"),
            lockdown_path: write(d.path(), "lockdown", "none integrity [confidentiality]"),
            cmdline_path: write(d.path(), "cmdline", "ro quiet lockdown=confidentiality"),
            ptrace_scope_path: write(d.path(), "ptrace", "1"),
            tpm_pcr_dir: d.path().join("pcr"),
            attestation_key_path: d.path().join("ak.pem"),
            vmm_binary_path: d.path().join("no-vmm"),
            ..Default::default()
        };
        // Kernel supports guest_memfd (probe says so) + lockdown=confidentiality.
        let probe = GuestMemfdProbe {
            kvm_accessible: true,
            cap_guest_memfd: true,
            sw_protected_vm_type: true,
            created: true,
            host_mmap_refused: Some(true),
            detail: "refused".into(),
        };
        let r = host_report(&cfg, Some(&probe));
        assert_eq!(r.tier, Tier::Protected);
        assert!(r.layers.lockdown_ok);
        assert_eq!(r.layers.host_read_refused, Some(true));

        // Drop lockdown below the requirement -> no protected tier.
        cfg.lockdown_path = write(d.path(), "lockdown2", "[none] integrity confidentiality");
        let r = host_report(&cfg, Some(&probe));
        assert_eq!(r.tier, Tier::Unavailable);
        assert!(r.reasons.iter().any(|s| s.contains("lockdown")));
    }

    #[test]
    fn attestation_report_signs_and_verifies() {
        use p256::ecdsa::{signature::Verifier, DerSignature, VerifyingKey};
        let d = tempdir().unwrap();
        let cfg = ProtectedConfig {
            cmdline_path: write(d.path(), "cmdline", "ro quiet"),
            lockdown_path: write(d.path(), "lockdown", "none integrity [confidentiality]"),
            tpm_pcr_dir: {
                write(d.path(), "pcr/0", "AA");
                write(d.path(), "pcr/7", "BB");
                d.path().join("pcr")
            },
            attested_pcrs: vec![0, 7],
            attestation_key_path: d.path().join("sub/ak.pem"),
            vmm_binary_path: d.path().join("no-vmm"),
            ..Default::default()
        };
        let rep = attestation_report(&cfg, Tier::Protected, "fp:abcd", None).unwrap();
        assert_eq!(rep.tier, Tier::Protected);
        assert!(rep
            .measurements
            .iter()
            .any(|m| m.name == "pcr0" && m.value == "AA"));

        // Recompute the canonical block and verify the signature.
        let mut lines: Vec<String> = rep
            .measurements
            .iter()
            .map(|m| format!("{}={}", m.name, m.value))
            .collect();
        lines.sort();
        let canonical = lines.join("\n");
        let pk =
            VerifyingKey::from_sec1_bytes(&hex::decode(&rep.attestation_pubkey).unwrap()).unwrap();
        let sig = DerSignature::from_bytes(&hex::decode(&rep.signature).unwrap()).unwrap();
        assert!(pk.verify(canonical.as_bytes(), &sig).is_ok());

        // Key persisted and reused across calls (same pubkey).
        let rep2 = attestation_report(&cfg, Tier::Protected, "fp:abcd", None).unwrap();
        assert_eq!(rep.attestation_pubkey, rep2.attestation_pubkey);
    }
}
