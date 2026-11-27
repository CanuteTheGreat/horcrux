//! Host hardening baseline ("Horcrux baseline", CIS-derived and adapted to a
//! hypervisor): evaluate, report, apply, revert.
//!
//! The rules are product content (like a benchmark), not node settings:
//! what a node does with them is set by `hardening.exclude`,
//! `hardening.overrides` and `hardening.sysctl_file`. Rules that would break
//! guests on a hypervisor are deliberately not in the baseline (IPv4/IPv6
//! forwarding off, strict reverse-path filtering, unprivileged user
//! namespaces off); docs/design/security-suite.md lists them with the reason.
//!
//! Only kernel parameters and file modes are changed automatically; both
//! are recorded with their previous value and can be reverted. SSH daemon,
//! account and mount findings are reported with the exact fix and never
//! changed by Horcrux (a wrong sshd change locks the operator out).
//!
//! Every path is read below `HORCRUX_SYSROOT` (tests use a fake root).
use super::super::system_admin::rooted;
use horcrux_common::security::{
    FixKind, HardeningApplyResult, HardeningChange, HardeningReport, HardeningRuleResult,
    HardeningSettings, RuleStatus, Severity,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const BASELINE: &str = "horcrux-baseline-1";

/// What a rule checks.
#[derive(Debug, Clone, Copy)]
pub enum Check {
    /// Kernel parameter: `ok` are accepted values, `set` is what apply writes.
    Sysctl {
        key: &'static str,
        ok: &'static [&'static str],
        set: &'static str,
    },
    /// File mode: the bits in `deny` must be clear; apply clears them.
    Mode { path: &'static str, deny: u32 },
    /// `sshd -T` keyword; values accepted (lower case).
    Sshd {
        key: &'static str,
        ok: &'static [&'static str],
    },
    /// `sshd -T` numeric keyword at most `max`.
    SshdMax { key: &'static str, max: u32 },
    /// One of these programs is installed.
    Present(&'static [&'static str]),
    /// None of these programs is installed.
    Absent(&'static [&'static str]),
    /// No CPU vulnerability reported as unmitigated by the kernel.
    CpuVulns,
    /// No account other than root has UID 0.
    NoExtraUid0,
    /// No account has an empty password field in /etc/shadow.
    NoEmptyPasswords,
    /// If `path` is a separate mount it carries every option.
    MountOpts {
        path: &'static str,
        opts: &'static [&'static str],
    },
    /// An nftables ruleset with at least one table is loaded.
    Firewall,
    /// The API access log is on.
    AccessLog,
    /// A time-sync daemon is running.
    TimeSync,
}

pub struct Rule {
    pub id: &'static str,
    pub category: &'static str,
    pub title: &'static str,
    pub severity: Severity,
    pub check: Check,
    pub rationale: &'static str,
    /// Manual rules: what to do. Auto rules: generated from the check.
    pub remediation: &'static str,
    pub reference: &'static str,
}

#[allow(clippy::too_many_arguments)]
const fn sysctl(
    id: &'static str,
    title: &'static str,
    severity: Severity,
    key: &'static str,
    ok: &'static [&'static str],
    set: &'static str,
    rationale: &'static str,
    reference: &'static str,
) -> Rule {
    Rule {
        id,
        category: "kernel",
        title,
        severity,
        check: Check::Sysctl { key, ok, set },
        rationale,
        remediation: "",
        reference,
    }
}

const fn netsysctl(
    id: &'static str,
    title: &'static str,
    key: &'static str,
    ok: &'static [&'static str],
    set: &'static str,
    rationale: &'static str,
    reference: &'static str,
) -> Rule {
    Rule {
        id,
        category: "network",
        title,
        severity: Severity::Medium,
        check: Check::Sysctl { key, ok, set },
        rationale,
        remediation: "",
        reference,
    }
}

const fn mode(
    id: &'static str,
    title: &'static str,
    severity: Severity,
    path: &'static str,
    deny: u32,
    rationale: &'static str,
    reference: &'static str,
) -> Rule {
    Rule {
        id,
        category: "files",
        title,
        severity,
        check: Check::Mode { path, deny },
        rationale,
        remediation: "",
        reference,
    }
}

/// The Horcrux baseline.
pub static RULES: &[Rule] = &[
    // ---------------------------------------------------------- kernel
    sysctl("kernel-aslr", "Address space layout randomisation is full", Severity::High,
        "kernel.randomize_va_space", &["2"], "2",
        "Randomised memory layout makes exploits of the management stack (QEMU, the API, sshd) much harder.",
        "CIS 1.5.3"),
    sysctl("kernel-kptr-restrict", "Kernel pointers are hidden from unprivileged users", Severity::Medium,
        "kernel.kptr_restrict", &["1", "2"], "1",
        "Leaked kernel addresses defeat KASLR, the first step of a guest-to-host or user-to-root escape.",
        "kernel.org sysctl/kernel kptr_restrict"),
    sysctl("kernel-dmesg-restrict", "Kernel log is readable by root only", Severity::Low,
        "kernel.dmesg_restrict", &["1"], "1",
        "The kernel log shows addresses and device details useful to an attacker.",
        "kernel.org sysctl/kernel dmesg_restrict"),
    sysctl("kernel-ptrace-scope", "Processes cannot trace unrelated processes", Severity::Medium,
        "kernel.yama.ptrace_scope", &["1", "2", "3"], "1",
        "Without Yama a compromised service can read the memory of other processes of the same user, including QEMU's guest RAM.",
        "kernel.org admin-guide/LSM/Yama"),
    sysctl("kernel-unpriv-bpf", "Unprivileged eBPF is disabled", Severity::Medium,
        "kernel.unprivileged_bpf_disabled", &["1", "2"], "2",
        "Unprivileged eBPF has been the entry point of many local privilege escalations. Value 2 can still be changed back without a reboot.",
        "kernel.org sysctl/kernel unprivileged_bpf_disabled"),
    sysctl("kernel-bpf-jit-harden", "eBPF JIT hardening is on for every user", Severity::Low,
        "net.core.bpf_jit_harden", &["2"], "2",
        "Blinds constants in JIT output, which blocks JIT spraying.",
        "kernel.org sysctl/net bpf_jit_harden"),
    sysctl("kernel-suid-dumpable", "Set-UID programs do not dump core", Severity::Medium,
        "fs.suid_dumpable", &["0"], "0",
        "Core dumps of privileged programs can leak password hashes and keys.",
        "CIS 1.5.1"),
    sysctl("fs-protected-hardlinks", "Hard links to files you do not own are refused", Severity::Medium,
        "fs.protected_hardlinks", &["1"], "1",
        "Blocks the classic hard-link races against root-owned programs.",
        "kernel.org sysctl/fs protected_hardlinks"),
    sysctl("fs-protected-symlinks", "Symlinks in sticky directories are followed safely", Severity::Medium,
        "fs.protected_symlinks", &["1"], "1",
        "Blocks /tmp symlink races against root-owned programs.",
        "kernel.org sysctl/fs protected_symlinks"),
    sysctl("fs-protected-fifos", "FIFOs in sticky directories are protected", Severity::Low,
        "fs.protected_fifos", &["1", "2"], "2",
        "Stops data spoofing through attacker-owned FIFOs in /tmp.",
        "kernel.org sysctl/fs protected_fifos"),
    sysctl("fs-protected-regular", "Regular files in sticky directories are protected", Severity::Low,
        "fs.protected_regular", &["1", "2"], "2",
        "Stops O_CREAT races on attacker-owned files in /tmp.",
        "kernel.org sysctl/fs protected_regular"),
    // ---------------------------------------------------------- network
    netsysctl("net-ipv4-accept-redirects", "IPv4 ICMP redirects are ignored",
        "net.ipv4.conf.all.accept_redirects", &["0"], "0",
        "A redirect lets anyone on the LAN reroute the host's traffic (management and storage) through themselves.",
        "CIS 3.3.2"),
    netsysctl("net-ipv4-default-accept-redirects", "IPv4 ICMP redirects are ignored on new interfaces",
        "net.ipv4.conf.default.accept_redirects", &["0"], "0",
        "Bridges and taps created for guests inherit the default.",
        "CIS 3.3.2"),
    netsysctl("net-ipv4-secure-redirects", "IPv4 'secure' redirects from gateways are ignored",
        "net.ipv4.conf.all.secure_redirects", &["0"], "0",
        "A spoofed gateway address is enough to use them.",
        "CIS 3.3.3"),
    netsysctl("net-ipv6-accept-redirects", "IPv6 ICMP redirects are ignored",
        "net.ipv6.conf.all.accept_redirects", &["0"], "0",
        "Same attack as IPv4 redirects.",
        "CIS 3.3.2"),
    netsysctl("net-ipv4-send-redirects", "The host does not send ICMP redirects",
        "net.ipv4.conf.all.send_redirects", &["0"], "0",
        "A hypervisor that routes guest networks should not teach clients other routes.",
        "CIS 3.3.1"),
    netsysctl("net-ipv4-source-route", "Source-routed IPv4 packets are dropped",
        "net.ipv4.conf.all.accept_source_route", &["0"], "0",
        "Source routing lets a sender pick the path, bypassing firewall placement.",
        "CIS 3.3.1"),
    netsysctl("net-ipv6-source-route", "Source-routed IPv6 packets are dropped",
        "net.ipv6.conf.all.accept_source_route", &["0"], "0",
        "Same as IPv4 source routing.",
        "CIS 3.3.1"),
    netsysctl("net-ipv4-log-martians", "Packets with impossible addresses are logged",
        "net.ipv4.conf.all.log_martians", &["1"], "1",
        "Spoofing attempts show up in the host log (System > Logs).",
        "CIS 3.3.4"),
    netsysctl("net-ipv4-syncookies", "TCP SYN cookies are on",
        "net.ipv4.tcp_syncookies", &["1"], "1",
        "Keeps the web UI and SSH reachable during a SYN flood.",
        "CIS 3.3.8"),
    netsysctl("net-ipv4-icmp-broadcast", "Broadcast pings are ignored",
        "net.ipv4.icmp_echo_ignore_broadcasts", &["1"], "1",
        "Stops the host taking part in smurf amplification.",
        "CIS 3.3.5"),
    netsysctl("net-ipv4-bogus-icmp", "Bogus ICMP error responses are ignored",
        "net.ipv4.icmp_ignore_bogus_error_responses", &["1"], "1",
        "Keeps the log free of forged error noise.",
        "CIS 3.3.6"),
    // ---------------------------------------------------------- files
    mode("file-shadow", "/etc/shadow is not readable by other users", Severity::High,
        "/etc/shadow", 0o037,
        "Password hashes of every local and PAM web-UI account are in this file.",
        "CIS 7.1.5"),
    mode("file-gshadow", "/etc/gshadow is not readable by other users", Severity::Medium,
        "/etc/gshadow", 0o037,
        "Group password hashes.",
        "CIS 7.1.6"),
    mode("file-passwd", "/etc/passwd is writable by root only", Severity::High,
        "/etc/passwd", 0o022,
        "Anyone who can write it can add a UID 0 account.",
        "CIS 7.1.1"),
    mode("file-group", "/etc/group is writable by root only", Severity::High,
        "/etc/group", 0o022,
        "Anyone who can write it can join wheel or horcrux-admin.",
        "CIS 7.1.3"),
    mode("file-sshd-config", "sshd_config is readable by root only", Severity::Low,
        "/etc/ssh/sshd_config", 0o077,
        "Hides the SSH policy from local users.",
        "CIS 5.1.1"),
    mode("file-crontab", "/etc/crontab is root-only", Severity::Low,
        "/etc/crontab", 0o077,
        "A writable crontab is a root shell.",
        "CIS 2.4.1.2"),
    mode("file-horcrux-config", "Horcrux configuration is not readable by other users", Severity::Medium,
        "/etc/horcrux/config.toml", 0o007,
        "It holds the database location, realm settings and may hold secrets.",
        "Horcrux"),
    // ---------------------------------------------------------- ssh (manual)
    Rule { id: "ssh-root-password", category: "ssh", title: "SSH root login with a password is off",
        severity: Severity::High,
        check: Check::Sshd { key: "permitrootlogin", ok: &["no", "prohibit-password", "without-password", "forced-commands-only"] },
        rationale: "Password guessing against root is the most common attack on internet-facing hypervisors.",
        remediation: "Add 'PermitRootLogin prohibit-password' to /etc/ssh/sshd_config.d/ (after you have a working key), check with 'sshd -t', then reload sshd. Horcrux never changes sshd itself: a wrong change locks you out.",
        reference: "CIS 5.1.20" },
    Rule { id: "ssh-empty-passwords", category: "ssh", title: "SSH refuses empty passwords",
        severity: Severity::Critical,
        check: Check::Sshd { key: "permitemptypasswords", ok: &["no"] },
        rationale: "An account with an empty password would be open to anyone.",
        remediation: "Set 'PermitEmptyPasswords no' in sshd_config, then reload sshd.",
        reference: "CIS 5.1.19" },
    Rule { id: "ssh-max-auth-tries", category: "ssh", title: "SSH allows at most 4 tries per connection",
        severity: Severity::Low,
        check: Check::SshdMax { key: "maxauthtries", max: 4 },
        rationale: "Slows password guessing.",
        remediation: "Set 'MaxAuthTries 4' in sshd_config, then reload sshd.",
        reference: "CIS 5.1.16" },
    Rule { id: "ssh-x11-forwarding", category: "ssh", title: "SSH X11 forwarding is off",
        severity: Severity::Low,
        check: Check::Sshd { key: "x11forwarding", ok: &["no"] },
        rationale: "A server OS has no reason to forward X11, which exposes the client's display.",
        remediation: "Set 'X11Forwarding no' in sshd_config, then reload sshd.",
        reference: "CIS 5.1.12" },
    // ---------------------------------------------------------- accounts (manual)
    Rule { id: "accounts-uid0", category: "accounts", title: "root is the only account with UID 0",
        severity: Severity::Critical, check: Check::NoExtraUid0,
        rationale: "A second UID 0 account is a classic backdoor.",
        remediation: "Remove or renumber the listed accounts (usermod -u / userdel) after checking who created them.",
        reference: "CIS 7.2.9" },
    Rule { id: "accounts-empty-password", category: "accounts", title: "No account has an empty password",
        severity: Severity::Critical, check: Check::NoEmptyPasswords,
        rationale: "An empty password logs in on the console and, with PAM, in the web UI.",
        remediation: "Lock the listed accounts (passwd -l NAME) or give them a password.",
        reference: "CIS 7.2.2" },
    // ---------------------------------------------------------- mounts (manual)
    Rule { id: "mount-tmp", category: "mounts", title: "/tmp, when a separate mount, is nodev,nosuid",
        severity: Severity::Low,
        check: Check::MountOpts { path: "/tmp", opts: &["nodev", "nosuid"] },
        rationale: "Stops set-UID programs and device files planted in /tmp.",
        remediation: "Add nodev,nosuid to the /tmp entry in /etc/fstab and remount (mount -o remount /tmp).",
        reference: "CIS 1.1.2" },
    Rule { id: "mount-dev-shm", category: "mounts", title: "/dev/shm is nodev,nosuid,noexec",
        severity: Severity::Low,
        check: Check::MountOpts { path: "/dev/shm", opts: &["nodev", "nosuid", "noexec"] },
        rationale: "Shared memory is world-writable.",
        remediation: "Add an fstab entry 'tmpfs /dev/shm tmpfs nodev,nosuid,noexec 0 0' and remount /dev/shm.",
        reference: "CIS 1.1.8" },
    // ---------------------------------------------------------- hypervisor
    Rule { id: "cpu-vulnerabilities", category: "hypervisor", title: "Every CPU vulnerability the kernel knows is mitigated",
        severity: Severity::Critical, check: Check::CpuVulns,
        rationale: "Unmitigated speculative-execution bugs (L1TF, MDS, Retbleed, ...) let one guest read another guest's or the host's memory.",
        remediation: "Install current CPU microcode (sys-firmware/intel-microcode or linux-firmware for AMD) and the newest kernel, and remove 'mitigations=off' from the kernel command line. Where the kernel says 'SMT vulnerable', consider 'nosmt' for untrusted guests.",
        reference: "kernel.org admin-guide/hw-vuln" },
    // ---------------------------------------------------------- services (manual)
    Rule { id: "svc-firewall", category: "services", title: "A host firewall ruleset is loaded",
        severity: Severity::High, check: Check::Firewall,
        rationale: "Without one every service on the host is reachable from every network it is on.",
        remediation: "Create rules under Firewall (node scope) or load a base nftables ruleset.",
        reference: "CIS 4.3" },
    Rule { id: "svc-time-sync", category: "services", title: "Time is synchronised",
        severity: Severity::Medium, check: Check::TimeSync,
        rationale: "TLS, TOTP two-factor codes, cluster quorum and log correlation all need a correct clock.",
        remediation: "Install and start chrony (System > Services).",
        reference: "CIS 2.3.1" },
    Rule { id: "svc-audit-log", category: "services", title: "The API access log is on",
        severity: Severity::Medium, check: Check::AccessLog,
        rationale: "Without it there is no record of who used the management API from where.",
        remediation: "Set [access_log] enabled = true in /etc/horcrux/config.toml and restart horcrux-api.",
        reference: "Horcrux" },
    Rule { id: "svc-auditd", category: "services", title: "The Linux audit daemon is installed",
        severity: Severity::Low, check: Check::Present(&["auditd"]),
        rationale: "Kernel-level records of logins, privilege use and changes to system files.",
        remediation: "emerge sys-process/audit and enable the auditd service on OpenRC (rc-update add auditd default) or systemd (systemctl enable --now auditd).",
        reference: "CIS 6.3.1" },
    Rule { id: "svc-integrity", category: "services", title: "A file integrity checker is installed",
        severity: Severity::Low, check: Check::Present(&["aide"]),
        rationale: "Detects changed system binaries and configuration after a compromise.",
        remediation: "emerge app-forensics/aide, run 'aide --init' once and schedule 'aide --check'.",
        reference: "CIS 6.1.1" },
    Rule { id: "svc-legacy-servers", category: "services", title: "No clear-text remote login servers are installed",
        severity: Severity::High,
        check: Check::Absent(&["in.telnetd", "telnetd", "in.rshd", "in.rlogind", "in.tftpd"]),
        rationale: "telnet/rsh/rlogin/tftp send credentials in clear text.",
        remediation: "Remove the package that installed the listed program (equery belongs PROGRAM, then emerge --depclean).",
        reference: "CIS 2.2" },
];

pub fn rule(id: &str) -> Option<&'static Rule> {
    RULES.iter().find(|r| r.id == id)
}

pub fn fix_kind(c: &Check) -> FixKind {
    match c {
        Check::Sysctl { .. } | Check::Mode { .. } => FixKind::Auto,
        _ => FixKind::Manual,
    }
}

/// `kernel.yama.ptrace_scope` -> `<root>/proc/sys/kernel/yama/ptrace_scope`.
pub fn sysctl_path(key: &str) -> PathBuf {
    rooted(&format!("/proc/sys/{}", key.replace('.', "/")))
}

/// Facts gathered once per check (commands; everything else is read from files).
#[derive(Debug, Clone, Default)]
pub struct Facts {
    /// `sshd -T` keyword -> value (lower case); `None` = sshd not installed.
    pub sshd: Option<BTreeMap<String, String>>,
    pub firewall_tables: Option<u32>,
    pub access_log: bool,
    pub time_sync: bool,
    /// Programs found by name.
    pub programs: Vec<String>,
}

pub fn parse_sshd_t(out: &str) -> BTreeMap<String, String> {
    out.lines()
        .filter_map(|l| {
            let mut it = l.trim().splitn(2, ' ');
            Some((
                it.next()?.to_ascii_lowercase(),
                it.next().unwrap_or("").trim().to_ascii_lowercase(),
            ))
        })
        .filter(|(k, _)| !k.is_empty())
        .collect()
}

/// Recorded state of what Horcrux applied (`<data_dir>/security/hardening-state.json`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AppliedState {
    /// rule id -> previous value (sysctl) or previous mode as octal (files).
    pub previous: BTreeMap<String, String>,
}

pub fn state_path(data_dir: &std::path::Path) -> PathBuf {
    data_dir.join("security").join("hardening-state.json")
}

pub fn load_state(data_dir: &std::path::Path) -> AppliedState {
    std::fs::read(state_path(data_dir))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save_state(data_dir: &std::path::Path, s: &AppliedState) -> Result<(), String> {
    let p = state_path(data_dir);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(s).unwrap_or_default())
        .map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &p).map_err(|e| format!("{}: {e}", p.display()))
}

fn read_trim(p: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(p)
        .ok()
        .map(|s| s.trim().to_string())
}

fn file_mode(path: &str) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(rooted(path))
        .ok()
        .map(|m| m.permissions().mode() & 0o7777)
}

/// Wanted values of a sysctl rule after `overrides`.
fn sysctl_want(
    r: &Rule,
    s: &HardeningSettings,
    ok: &'static [&'static str],
    set: &'static str,
) -> (Vec<String>, String) {
    match s.overrides.get(r.id) {
        Some(v) => (vec![v.clone()], v.clone()),
        None => (ok.iter().map(|x| x.to_string()).collect(), set.to_string()),
    }
}

/// Evaluate one rule: (status, current, expected).
fn evaluate_rule(r: &Rule, s: &HardeningSettings, f: &Facts) -> (RuleStatus, String, String) {
    use RuleStatus::*;
    match r.check {
        Check::Sysctl { key, ok, set } => {
            let (want, _) = sysctl_want(r, s, ok, set);
            let exp = format!("{key} = {}", want.join(" or "));
            match read_trim(&sysctl_path(key)) {
                None => (Na, "not available on this kernel".into(), exp),
                Some(v) => {
                    let v = v.split_whitespace().collect::<Vec<_>>().join(" ");
                    (
                        if want.contains(&v) { Pass } else { Fail },
                        format!("{key} = {v}"),
                        exp,
                    )
                }
            }
        }
        Check::Mode { path, deny } => {
            let exp = format!("no bits of {deny:04o} set");
            match file_mode(path) {
                None => (Na, format!("{path} does not exist"), exp),
                Some(m) => (
                    if m & deny == 0 { Pass } else { Fail },
                    format!("{path} mode {m:04o}"),
                    exp,
                ),
            }
        }
        Check::Sshd { key, ok } => {
            let exp = format!("{key} {}", ok.join(" | "));
            match &f.sshd {
                None => (Na, "sshd is not installed".into(), exp),
                Some(m) => {
                    let v = m.get(key).cloned().unwrap_or_default();
                    (
                        if ok.contains(&v.as_str()) {
                            Pass
                        } else {
                            Manual
                        },
                        format!("{key} {}", if v.is_empty() { "(unset)" } else { &v }),
                        exp,
                    )
                }
            }
        }
        Check::SshdMax { key, max } => {
            let exp = format!("{key} <= {max}");
            match &f.sshd {
                None => (Na, "sshd is not installed".into(), exp),
                Some(m) => {
                    let v = m.get(key).cloned().unwrap_or_default();
                    let ok = v.parse::<u32>().map(|n| n <= max).unwrap_or(false);
                    (if ok { Pass } else { Manual }, format!("{key} {v}"), exp)
                }
            }
        }
        Check::Present(names) => {
            let found: Vec<&str> = names
                .iter()
                .copied()
                .filter(|n| f.programs.iter().any(|p| p == n))
                .collect();
            if found.is_empty() {
                (Manual, "not installed".into(), names.join(" or "))
            } else {
                (
                    Pass,
                    format!("{} installed", found.join(", ")),
                    names.join(" or "),
                )
            }
        }
        Check::Absent(names) => {
            let found: Vec<&str> = names
                .iter()
                .copied()
                .filter(|n| f.programs.iter().any(|p| p == n))
                .collect();
            if found.is_empty() {
                (Pass, "none installed".into(), "none installed".into())
            } else {
                (
                    Manual,
                    format!("installed: {}", found.join(", ")),
                    "none installed".into(),
                )
            }
        }
        Check::CpuVulns => {
            let dir = rooted("/sys/devices/system/cpu/vulnerabilities");
            let Ok(rd) = std::fs::read_dir(&dir) else {
                return (
                    Na,
                    "the kernel does not report CPU vulnerabilities".into(),
                    "all mitigated".into(),
                );
            };
            let mut bad = Vec::new();
            let mut names: Vec<_> = rd.flatten().collect();
            names.sort_by_key(|e| e.file_name());
            for e in names {
                let v = read_trim(&e.path()).unwrap_or_default();
                if v.starts_with("Vulnerable") || v.contains("SMT vulnerable") {
                    bad.push(format!("{}: {v}", e.file_name().to_string_lossy()));
                }
            }
            if bad.is_empty() {
                (
                    Pass,
                    "all mitigated or not affected".into(),
                    "all mitigated".into(),
                )
            } else {
                (Fail, bad.join("; "), "all mitigated".into())
            }
        }
        Check::NoExtraUid0 => {
            let Some(pw) = std::fs::read_to_string(rooted("/etc/passwd")).ok() else {
                return (Na, "/etc/passwd unreadable".into(), "root only".into());
            };
            let extra: Vec<&str> = pw
                .lines()
                .filter_map(|l| {
                    let f: Vec<&str> = l.split(':').collect();
                    (f.len() > 2 && f[2] == "0" && f[0] != "root").then_some(f[0])
                })
                .collect();
            if extra.is_empty() {
                (Pass, "root only".into(), "root only".into())
            } else {
                (
                    Fail,
                    format!("UID 0: {}", extra.join(", ")),
                    "root only".into(),
                )
            }
        }
        Check::NoEmptyPasswords => {
            let Some(sh) = std::fs::read_to_string(rooted("/etc/shadow")).ok() else {
                return (Na, "/etc/shadow unreadable".into(), "none".into());
            };
            let empty: Vec<&str> = sh
                .lines()
                .filter_map(|l| {
                    let f: Vec<&str> = l.split(':').collect();
                    (f.len() > 1 && f[1].is_empty()).then_some(f[0])
                })
                .collect();
            if empty.is_empty() {
                (Pass, "none".into(), "none".into())
            } else {
                (Fail, format!("empty: {}", empty.join(", ")), "none".into())
            }
        }
        Check::MountOpts { path, opts } => {
            let exp = opts.join(",");
            let mounts = std::fs::read_to_string(rooted("/proc/mounts")).unwrap_or_default();
            let line = mounts
                .lines()
                .rfind(|l| l.split_whitespace().nth(1) == Some(path));
            match line {
                None => (Na, format!("{path} is not a separate mount"), exp),
                Some(l) => {
                    let have: Vec<&str> = l
                        .split_whitespace()
                        .nth(3)
                        .unwrap_or("")
                        .split(',')
                        .collect();
                    let missing: Vec<&str> =
                        opts.iter().copied().filter(|o| !have.contains(o)).collect();
                    if missing.is_empty() {
                        (Pass, format!("{path}: {exp}"), exp)
                    } else {
                        (Manual, format!("{path} lacks {}", missing.join(",")), exp)
                    }
                }
            }
        }
        Check::Firewall => match f.firewall_tables {
            None => (
                Manual,
                "nft is not installed".into(),
                "a loaded ruleset".into(),
            ),
            Some(0) => (
                Fail,
                "no nftables tables loaded".into(),
                "a loaded ruleset".into(),
            ),
            Some(n) => (
                Pass,
                format!("{n} nftables tables"),
                "a loaded ruleset".into(),
            ),
        },
        Check::AccessLog => (
            if f.access_log { Pass } else { Fail },
            if f.access_log { "on" } else { "off" }.into(),
            "on".into(),
        ),
        Check::TimeSync => (
            if f.time_sync { Pass } else { Fail },
            if f.time_sync {
                "running"
            } else {
                "no NTP daemon running"
            }
            .into(),
            "chronyd or ntpd running".into(),
        ),
    }
}

fn auto_remediation(r: &Rule, s: &HardeningSettings) -> String {
    match r.check {
        Check::Sysctl { key, ok, set } => {
            let (_, set) = sysctl_want(r, s, ok, set);
            format!(
                "Apply sets {key} = {set} now and writes it to {} (kept across reboots). Revert restores the previous value.",
                s.sysctl_file
            )
        }
        Check::Mode { path, deny } => format!(
            "Apply removes the {deny:04o} permission bits from {path}. Revert restores the previous mode."
        ),
        _ => r.remediation.to_string(),
    }
}

/// Weight of a rule in the score.
fn weight(s: Severity) -> f64 {
    match s {
        Severity::Critical => 8.0,
        Severity::High => 4.0,
        Severity::Medium => 2.0,
        _ => 1.0,
    }
}

/// Evaluate the whole baseline (pure apart from reading files below the sysroot).
pub fn evaluate(
    s: &HardeningSettings,
    f: &Facts,
    applied: &AppliedState,
    node: &str,
    now: i64,
) -> HardeningReport {
    let mut rep = HardeningReport {
        node: node.to_string(),
        checked_at: now,
        baseline: BASELINE.to_string(),
        ..Default::default()
    };
    let (mut got, mut total) = (0.0, 0.0);
    for r in RULES {
        let (status, current, expected) = if s.exclude.iter().any(|e| e == r.id) {
            (
                RuleStatus::Excluded,
                "excluded in hardening.exclude".into(),
                String::new(),
            )
        } else {
            evaluate_rule(r, s, f)
        };
        match status {
            RuleStatus::Pass => {
                rep.passed += 1;
                got += weight(r.severity);
                total += weight(r.severity);
            }
            RuleStatus::Fail => {
                rep.failed += 1;
                total += weight(r.severity);
            }
            RuleStatus::Manual => {
                rep.manual += 1;
                total += weight(r.severity);
            }
            RuleStatus::Na => rep.not_applicable += 1,
            RuleStatus::Excluded => rep.excluded += 1,
        }
        rep.rules.push(HardeningRuleResult {
            id: r.id.into(),
            category: r.category.into(),
            title: r.title.into(),
            severity: r.severity,
            status,
            fix: fix_kind(&r.check),
            current,
            expected,
            rationale: r.rationale.into(),
            remediation: auto_remediation(r, s),
            reference: r.reference.into(),
            applied: applied.previous.contains_key(r.id),
        });
    }
    rep.score = if total == 0.0 {
        100.0
    } else {
        (got * 1000.0 / total).round() / 10.0
    };
    rep
}

/// The managed sysctl drop-in for these values (sorted, one per line).
pub fn render_sysctl_file(values: &BTreeMap<String, String>) -> String {
    let mut out = String::from(
        "# Managed by Horcrux (Security > Hardening). Changes here are overwritten;\n\
         # exclude a rule in the security settings instead.\n",
    );
    for (k, v) in values {
        out.push_str(&format!("{k} = {v}\n"));
    }
    out
}

/// Parse the managed drop-in back (`key = value` lines).
pub fn parse_sysctl_file(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(|l| {
            let (k, v) = l.split_once('=')?;
            let (k, v) = (k.trim(), v.trim());
            (!k.is_empty()).then(|| (k.to_string(), v.to_string()))
        })
        .collect()
}

fn write_atomic(path: &std::path::Path, data: &str, mode: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let tmp = path.with_extension("conf.tmp");
    std::fs::write(&tmp, data).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode));
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

fn set_mode(path: &str, m: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(rooted(path), std::fs::Permissions::from_mode(m))
        .map_err(|e| format!("{path}: {e}"))
}

/// Apply rules. `report` is a fresh evaluation; only failing auto rules change.
pub fn apply(
    s: &HardeningSettings,
    report: &HardeningReport,
    req_rules: &[String],
    dry_run: bool,
    state: &mut AppliedState,
) -> HardeningApplyResult {
    let mut res = HardeningApplyResult {
        dry_run,
        ..Default::default()
    };
    let wanted: Vec<&HardeningRuleResult> = if req_rules.is_empty() {
        report
            .rules
            .iter()
            .filter(|r| r.status == RuleStatus::Fail && r.fix == FixKind::Auto)
            .collect()
    } else {
        let mut v = Vec::new();
        for id in req_rules {
            match report.rules.iter().find(|r| &r.id == id) {
                None => res.skipped.push(HardeningChange {
                    rule: id.clone(),
                    change: String::new(),
                    ok: false,
                    error: Some("no such rule".into()),
                }),
                Some(r) if r.fix != FixKind::Auto => res.skipped.push(HardeningChange {
                    rule: id.clone(),
                    change: String::new(),
                    ok: false,
                    error: Some("needs a manual fix (see remediation)".into()),
                }),
                Some(r) if r.status == RuleStatus::Excluded || r.status == RuleStatus::Na => {
                    res.skipped.push(HardeningChange {
                        rule: id.clone(),
                        change: String::new(),
                        ok: false,
                        error: Some(format!("rule is {:?}", r.status).to_lowercase()),
                    })
                }
                Some(r) if r.status == RuleStatus::Pass => res.skipped.push(HardeningChange {
                    rule: id.clone(),
                    change: String::new(),
                    ok: true,
                    error: Some("already passes".into()),
                }),
                Some(r) => v.push(r),
            }
        }
        v
    };
    let file = rooted(&s.sysctl_file);
    let mut managed = std::fs::read_to_string(&file)
        .map(|t| parse_sysctl_file(&t))
        .unwrap_or_default();
    let mut file_changed = false;
    for r in wanted {
        let Some(def) = rule(&r.id) else { continue };
        match def.check {
            Check::Sysctl { key, ok, set } => {
                let (_, set) = sysctl_want(def, s, ok, set);
                let p = sysctl_path(key);
                let prev = read_trim(&p).unwrap_or_default();
                let change = format!("set {key} {prev} -> {set}");
                if dry_run {
                    res.changes.push(HardeningChange {
                        rule: r.id.clone(),
                        change,
                        ok: true,
                        error: None,
                    });
                    continue;
                }
                match std::fs::write(&p, format!("{set}\n")) {
                    Ok(()) => {
                        state.previous.entry(r.id.clone()).or_insert(prev);
                        managed.insert(key.to_string(), set);
                        file_changed = true;
                        res.changes.push(HardeningChange {
                            rule: r.id.clone(),
                            change,
                            ok: true,
                            error: None,
                        });
                    }
                    Err(e) => res.changes.push(HardeningChange {
                        rule: r.id.clone(),
                        change,
                        ok: false,
                        error: Some(format!("{}: {e}", p.display())),
                    }),
                }
            }
            Check::Mode { path, deny } => {
                let Some(m) = file_mode(path) else { continue };
                let new = m & !deny;
                let change = format!("chmod {path} {m:04o} -> {new:04o}");
                if dry_run {
                    res.changes.push(HardeningChange {
                        rule: r.id.clone(),
                        change,
                        ok: true,
                        error: None,
                    });
                    continue;
                }
                match set_mode(path, new) {
                    Ok(()) => {
                        state
                            .previous
                            .entry(r.id.clone())
                            .or_insert(format!("{m:o}"));
                        res.changes.push(HardeningChange {
                            rule: r.id.clone(),
                            change,
                            ok: true,
                            error: None,
                        });
                    }
                    Err(e) => res.changes.push(HardeningChange {
                        rule: r.id.clone(),
                        change,
                        ok: false,
                        error: Some(e),
                    }),
                }
            }
            _ => {}
        }
    }
    if file_changed {
        if let Err(e) = write_atomic(&file, &render_sysctl_file(&managed), 0o644) {
            res.changes.push(HardeningChange {
                rule: "sysctl-file".into(),
                change: format!("write {}", s.sysctl_file),
                ok: false,
                error: Some(e),
            });
        }
    }
    res
}

/// Revert rules Horcrux applied (all when `req_rules` is empty).
pub fn revert(
    s: &HardeningSettings,
    req_rules: &[String],
    dry_run: bool,
    state: &mut AppliedState,
) -> HardeningApplyResult {
    let mut res = HardeningApplyResult {
        dry_run,
        ..Default::default()
    };
    let ids: Vec<String> = if req_rules.is_empty() {
        state.previous.keys().cloned().collect()
    } else {
        req_rules.to_vec()
    };
    let file = rooted(&s.sysctl_file);
    let mut managed = std::fs::read_to_string(&file)
        .map(|t| parse_sysctl_file(&t))
        .unwrap_or_default();
    let mut file_changed = false;
    for id in ids {
        let (Some(prev), Some(def)) = (state.previous.get(&id).cloned(), rule(&id)) else {
            res.skipped.push(HardeningChange {
                rule: id.clone(),
                change: String::new(),
                ok: false,
                error: Some("Horcrux has not applied this rule".into()),
            });
            continue;
        };
        let (change, outcome) = match def.check {
            Check::Sysctl { key, .. } => {
                let change = format!("set {key} -> {prev}");
                if dry_run {
                    (change, Ok(()))
                } else {
                    let r = std::fs::write(sysctl_path(key), format!("{prev}\n"))
                        .map_err(|e| format!("{key}: {e}"));
                    if r.is_ok() {
                        managed.remove(key);
                        file_changed = true;
                    }
                    (change, r)
                }
            }
            Check::Mode { path, .. } => {
                let change = format!("chmod {path} -> {prev}");
                let m = u32::from_str_radix(&prev, 8).map_err(|e| e.to_string());
                if dry_run {
                    (change, m.map(|_| ()))
                } else {
                    (change, m.and_then(|m| set_mode(path, m)))
                }
            }
            _ => continue,
        };
        let ok = outcome.is_ok();
        if ok && !dry_run {
            state.previous.remove(&id);
        }
        res.changes.push(HardeningChange {
            rule: id,
            change,
            ok,
            error: outcome.err(),
        });
    }
    if file_changed {
        let r = if managed.is_empty() {
            match std::fs::remove_file(&file) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    Err(format!("{}: {e}", file.display()))
                }
                _ => Ok(()),
            }
        } else {
            write_atomic(&file, &render_sysctl_file(&managed), 0o644)
        };
        if let Err(e) = r {
            res.changes.push(HardeningChange {
                rule: "sysctl-file".into(),
                change: format!("write {}", s.sysctl_file),
                ok: false,
                error: Some(e),
            });
        }
    }
    res
}

/// Plain-text compliance report.
pub fn render_text(r: &HardeningReport) -> String {
    let when = chrono::DateTime::from_timestamp(r.checked_at, 0)
        .map(|d| d.to_rfc3339())
        .unwrap_or_default();
    let mut out = format!(
        "Horcrux hardening report\nNode: {}\nChecked: {when}\nBaseline: {}\nScore: {:.1}%\nPassed {}  Failed {}  Manual {}  Not applicable {}  Excluded {}\n\n",
        r.node, r.baseline, r.score, r.passed, r.failed, r.manual, r.not_applicable, r.excluded
    );
    for st in [
        RuleStatus::Fail,
        RuleStatus::Manual,
        RuleStatus::Pass,
        RuleStatus::Na,
        RuleStatus::Excluded,
    ] {
        for x in r.rules.iter().filter(|x| x.status == st) {
            out.push_str(&format!(
                "[{}] {} ({}, {}) {}\n    now: {}\n",
                format!("{st:?}").to_uppercase(),
                x.id,
                x.category,
                x.severity.as_str(),
                x.title,
                x.current
            ));
            if matches!(st, RuleStatus::Fail | RuleStatus::Manual) {
                out.push_str(&format!(
                    "    want: {}\n    fix: {}\n",
                    x.expected, x.remediation
                ));
            }
        }
    }
    out
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Self-contained HTML compliance report (printable, light and dark).
pub fn render_html(r: &HardeningReport) -> String {
    let when = chrono::DateTime::from_timestamp(r.checked_at, 0)
        .map(|d| d.to_rfc3339())
        .unwrap_or_default();
    let mut rows = String::new();
    for x in &r.rules {
        let st = format!("{:?}", x.status).to_lowercase();
        rows.push_str(&format!(
            "<tr class=\"{st}\"><td>{st}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>\n",
            esc(x.severity.as_str()), esc(&x.id), esc(&x.title), esc(&x.current), esc(&x.expected),
            if matches!(x.status, RuleStatus::Fail | RuleStatus::Manual) { esc(&x.remediation) } else { String::new() }
        ));
    }
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"color-scheme\" content=\"light dark\">\
<title>Hardening report {node}</title><style>body{{font:14px system-ui,sans-serif;margin:24px}}table{{border-collapse:collapse;width:100%}}\
td,th{{border:1px solid #8884;padding:4px 6px;text-align:left;vertical-align:top}}tr.fail td:first-child{{color:#b42318;font-weight:600}}\
tr.manual td:first-child{{color:#8a5200;font-weight:600}}tr.pass td:first-child{{color:#1d7a3e}}@media (prefers-color-scheme:dark){{body{{background:#0f1216;color:#e6e9ed}}\
tr.fail td:first-child{{color:#ff7d73}}tr.manual td:first-child{{color:#f0b545}}tr.pass td:first-child{{color:#56c77a}}}}</style></head><body>\
<h1>Hardening report</h1><p>Node <b>{node}</b>, checked {when}, baseline {base}.</p>\
<p>Score <b>{score:.1}%</b>: {p} passed, {f} failed, {m} need a manual fix, {na} not applicable, {ex} excluded.</p>\
<table><thead><tr><th>Status</th><th>Severity</th><th>Rule</th><th>Title</th><th>Now</th><th>Wanted</th><th>Fix</th></tr></thead><tbody>{rows}</tbody></table></body></html>",
        node = esc(&r.node), base = esc(&r.baseline), score = r.score, p = r.passed, f = r.failed,
        m = r.manual, na = r.not_applicable, ex = r.excluded
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn fake_root() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let w = |p: &str, s: &str| {
            let f = d.path().join(p.trim_start_matches('/'));
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, s).unwrap();
        };
        w("/proc/sys/kernel/randomize_va_space", "2\n");
        w("/proc/sys/kernel/kptr_restrict", "0\n");
        w("/proc/sys/net/ipv4/conf/all/accept_redirects", "1\n");
        w("/etc/passwd", "root:x:0:0::/root:/bin/bash\ntoor:x:0:0::/:/bin/sh\nbob:x:1000:1000::/home/bob:/bin/bash\n");
        w("/etc/shadow", "root:$6$abc:1::::::\nbob::1::::::\n");
        w(
            "/proc/mounts",
            "tmpfs /tmp tmpfs rw,nosuid 0 0\ntmpfs /dev/shm tmpfs rw,nosuid,nodev,noexec 0 0\n",
        );
        w(
            "/sys/devices/system/cpu/vulnerabilities/meltdown",
            "Mitigation: PTI\n",
        );
        w(
            "/sys/devices/system/cpu/vulnerabilities/l1tf",
            "Mitigation: PTE Inversion; VMX: conditional cache flushes, SMT vulnerable\n",
        );
        std::fs::set_permissions(
            d.path().join("etc/shadow"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        std::fs::set_permissions(
            d.path().join("etc/passwd"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        d
    }

    fn by<'a>(r: &'a HardeningReport, id: &str) -> &'a HardeningRuleResult {
        r.rules.iter().find(|x| x.id == id).unwrap()
    }

    #[test]
    fn rule_ids_unique_and_documented() {
        let mut ids: Vec<&str> = RULES.iter().map(|r| r.id).collect();
        ids.sort();
        let n = ids.len();
        ids.dedup();
        assert_eq!(n, ids.len(), "duplicate rule id");
        for r in RULES {
            assert!(
                !r.rationale.is_empty() && !r.reference.is_empty(),
                "{}",
                r.id
            );
            if fix_kind(&r.check) == FixKind::Manual {
                assert!(!r.remediation.is_empty(), "{} needs a remediation", r.id);
            }
            if let Check::Sysctl { ok, set, key } = r.check {
                assert!(ok.contains(&set), "{}: apply value must pass", r.id);
                // Hypervisor safety: never turn forwarding off or touch user namespaces.
                assert!(
                    !key.contains("ip_forward")
                        && !key.contains("forwarding")
                        && !key.contains("userns"),
                    "{}",
                    r.id
                );
            }
        }
    }

    #[test]
    fn evaluate_apply_revert_round_trip() {
        let _g = crate::api::system_admin::SYSROOT_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let root = fake_root();
        std::env::set_var("HORCRUX_SYSROOT", root.path());
        let s = HardeningSettings::default();
        let f = Facts {
            sshd: Some(parse_sshd_t(
                "permitrootlogin yes\npermitemptypasswords no\nmaxauthtries 6\nx11forwarding no\n",
            )),
            firewall_tables: Some(0),
            access_log: true,
            time_sync: true,
            programs: vec!["aide".into()],
        };
        let mut st = AppliedState::default();
        let r = evaluate(&s, &f, &st, "n1", 1);
        assert_eq!(by(&r, "kernel-aslr").status, RuleStatus::Pass);
        assert_eq!(by(&r, "kernel-kptr-restrict").status, RuleStatus::Fail);
        assert_eq!(by(&r, "kernel-dmesg-restrict").status, RuleStatus::Na);
        assert_eq!(by(&r, "file-shadow").status, RuleStatus::Fail);
        assert_eq!(by(&r, "file-passwd").status, RuleStatus::Pass);
        assert_eq!(by(&r, "accounts-uid0").status, RuleStatus::Fail);
        assert!(by(&r, "accounts-uid0").current.contains("toor"));
        assert_eq!(by(&r, "accounts-empty-password").status, RuleStatus::Fail);
        assert_eq!(by(&r, "mount-tmp").status, RuleStatus::Manual);
        assert_eq!(by(&r, "mount-dev-shm").status, RuleStatus::Pass);
        assert_eq!(by(&r, "cpu-vulnerabilities").status, RuleStatus::Fail);
        assert_eq!(by(&r, "ssh-root-password").status, RuleStatus::Manual);
        assert_eq!(by(&r, "ssh-max-auth-tries").status, RuleStatus::Manual);
        assert_eq!(by(&r, "ssh-empty-passwords").status, RuleStatus::Pass);
        assert_eq!(by(&r, "svc-firewall").status, RuleStatus::Fail);
        assert_eq!(by(&r, "svc-integrity").status, RuleStatus::Pass);
        assert!(r.score > 0.0 && r.score < 100.0);

        // Dry run changes nothing.
        let d = apply(&s, &r, &[], true, &mut st);
        assert!(d.changes.iter().any(|c| c.rule == "kernel-kptr-restrict"));
        assert!(st.previous.is_empty());
        assert_eq!(
            read_trim(&sysctl_path("kernel.kptr_restrict")).unwrap(),
            "0"
        );

        // Manual rules are refused by name.
        let m = apply(&s, &r, &["ssh-root-password".into()], false, &mut st);
        assert!(m.changes.is_empty() && m.skipped.len() == 1);

        let a = apply(&s, &r, &[], false, &mut st);
        assert!(a.changes.iter().all(|c| c.ok), "{:?}", a.changes);
        assert_eq!(
            read_trim(&sysctl_path("kernel.kptr_restrict")).unwrap(),
            "1"
        );
        assert_eq!(file_mode("/etc/shadow").unwrap(), 0o640);
        let conf = std::fs::read_to_string(rooted(&s.sysctl_file)).unwrap();
        assert!(
            conf.contains("kernel.kptr_restrict = 1")
                && conf.contains("net.ipv4.conf.all.accept_redirects = 0")
        );
        let r2 = evaluate(&s, &f, &st, "n1", 2);
        assert_eq!(by(&r2, "kernel-kptr-restrict").status, RuleStatus::Pass);
        assert!(by(&r2, "kernel-kptr-restrict").applied);
        assert!(r2.score > r.score);

        let v = revert(&s, &[], false, &mut st);
        assert!(v.changes.iter().all(|c| c.ok), "{:?}", v.changes);
        assert!(st.previous.is_empty());
        assert_eq!(
            read_trim(&sysctl_path("kernel.kptr_restrict")).unwrap(),
            "0"
        );
        assert_eq!(file_mode("/etc/shadow").unwrap(), 0o644);
        assert!(!rooted(&s.sysctl_file).exists());
        std::env::remove_var("HORCRUX_SYSROOT");
    }

    #[test]
    fn exclude_and_override() {
        let _g = crate::api::system_admin::SYSROOT_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let root = fake_root();
        std::env::set_var("HORCRUX_SYSROOT", root.path());
        let mut s = HardeningSettings::default();
        s.exclude.push("kernel-kptr-restrict".into());
        s.overrides.insert("kernel-aslr".into(), "1".into());
        let r = evaluate(&s, &Facts::default(), &AppliedState::default(), "n", 0);
        assert_eq!(by(&r, "kernel-kptr-restrict").status, RuleStatus::Excluded);
        assert_eq!(by(&r, "kernel-aslr").status, RuleStatus::Fail);
        let mut st = AppliedState::default();
        let a = apply(&s, &r, &["kernel-kptr-restrict".into()], false, &mut st);
        assert!(a.changes.is_empty() && a.skipped.len() == 1);
        std::env::remove_var("HORCRUX_SYSROOT");
    }

    #[test]
    fn reports_render() {
        let r = HardeningReport {
            node: "n<1>".into(),
            rules: vec![HardeningRuleResult {
                id: "x".into(),
                category: "c".into(),
                title: "t".into(),
                severity: Severity::High,
                status: RuleStatus::Fail,
                fix: FixKind::Manual,
                current: "a".into(),
                expected: "b".into(),
                rationale: String::new(),
                remediation: "do <it>".into(),
                reference: String::new(),
                applied: false,
            }],
            ..Default::default()
        };
        assert!(render_text(&r).contains("[FAIL] x"));
        let h = render_html(&r);
        assert!(
            h.contains("n&lt;1&gt;") && h.contains("do &lt;it&gt;") && h.contains("color-scheme")
        );
        assert_eq!(
            parse_sysctl_file(&render_sysctl_file(&BTreeMap::from([(
                "a.b".into(),
                "1".into()
            )])))
            .get("a.b")
            .unwrap(),
            "1"
        );
    }
}
