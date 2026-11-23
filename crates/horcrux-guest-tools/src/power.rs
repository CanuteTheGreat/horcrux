//! Guest-cooperative hibernate (docs/SAVE_STATE.md, "Guests that own hardware").
//!
//! A VM that owns hardware QEMU cannot save (a passthrough GPU) is put to
//! sleep by the guest itself when the host shuts down: the guest writes its
//! memory to its own disk, the VM powers off, and the next host boot starts
//! the VM again, where the guest resumes. The node reaches this through the
//! hypervisor's guest-agent channel, which exists whether or not a viewer
//! is connected:
//!
//! * `horcrux-guest-agent --hibernate-status`  prints a JSON readiness report
//! * `horcrux-guest-agent --hibernate-setup`   the install step: prepares the
//!   guest unless `hibernate_at_install = false`
//! * `horcrux-guest-agent --hibernate-enable`  prepares the guest (Linux: swap
//!   file, `resume=` on the kernel command line, initramfs resume support;
//!   Windows: `powercfg /h on`)
//! * `horcrux-guest-agent --hibernate`         checks readiness, answers, and
//!   starts the hibernate a moment later (so the answer is delivered)
//!
//! Every value is a setting (`guest-tools.conf`, see [`crate::config`]).
//! The parsers and planners are pure and unit-tested; the OS calls are thin.
#![allow(dead_code)] // the pure helpers serve both OS back ends
use crate::config::{Config, HibernateAction};
use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;

/// Entry point for the one-shot actions. Returns the process exit code.
pub fn run(cfg: &Config, action: HibernateAction) -> i32 {
    let out = match action {
        HibernateAction::Status => status(cfg),
        HibernateAction::Setup if !cfg.hibernate_at_install => {
            json!({"ok": true, "skipped": "hibernate_at_install is off in the guest tools settings"})
        }
        HibernateAction::Setup | HibernateAction::Enable => enable(cfg),
        HibernateAction::Hibernate => hibernate(cfg),
    };
    println!("{}", serde_json::to_string(&out).unwrap_or_default());
    // status always succeeds; the others report failure in the exit code
    if action == HibernateAction::Status || out["ok"].as_bool() == Some(true) {
        0
    } else {
        3
    }
}

// ------------------------------------------------------------ pure parsing

pub fn meminfo_kb(text: &str, key: &str) -> Option<u64> {
    text.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        (k.trim() == key)
            .then(|| v.split_whitespace().next()?.parse().ok())
            .flatten()
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Swap {
    pub name: String,
    pub kind: String,
    pub size_kb: u64,
}

/// `/proc/swaps`: `Filename Type Size Used Priority`.
pub fn parse_swaps(text: &str) -> Vec<Swap> {
    text.lines()
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            (f.len() >= 3).then(|| Swap {
                name: f[0].to_string(),
                kind: f[1].to_string(),
                size_kb: f[2].parse().unwrap_or(0),
            })
        })
        .collect()
}

/// Does `/sys/power/state` offer `disk` (hibernation compiled in and allowed)?
pub fn power_state_has_disk(s: &str) -> bool {
    s.split_whitespace().any(|w| w == "disk")
}

/// The value of a `key=` argument on the kernel command line.
pub fn cmdline_arg(cmdline: &str, key: &str) -> Option<String> {
    cmdline
        .split_whitespace()
        .find_map(|w| w.strip_prefix(&format!("{key}=")).map(str::to_string))
}

/// Swap the guest must have for `ram_kb` of RAM at `pct` percent.
pub fn need_swap_kb(ram_kb: u64, pct: u64) -> u64 {
    ram_kb * pct / 100
}

/// A swap area is big enough when it is within a page or two of the need: the
/// kernel reports the usable size, which is a page short of a file made to the
/// exact size (the swap header takes the first page).
pub fn swap_enough(have_kb: u64, need_kb: u64) -> bool {
    have_kb * 100 >= need_kb * SWAP_TOLERANCE_PCT
}

/// How close to the wanted size a swap area must be (percent).
const SWAP_TOLERANCE_PCT: u64 = 99;

/// First physical extent start of `filefrag -v` output (file-system blocks),
/// converted to pages (the kernel's `resume_offset` unit).
pub fn filefrag_resume_offset(text: &str, block_size: u64, page_size: u64) -> Option<u64> {
    for l in text.lines() {
        // "   0:        0..   32767:      34816..     67583:  32768:"
        let f: Vec<&str> = l.split(':').collect();
        if f.len() >= 4 && f[0].trim().parse::<u64>().is_ok() {
            let start = f[2].split("..").next()?.trim().parse::<u64>().ok()?;
            return Some(start * block_size / page_size.max(1));
        }
    }
    None
}

/// Add `args` to the `GRUB_CMDLINE_LINUX` line of /etc/default/grub text
/// (idempotent: arguments already present are kept as they are).
pub fn grub_add_args(text: &str, args: &[String]) -> String {
    let mut out = Vec::new();
    let mut done = false;
    for l in text.lines() {
        let t = l.trim_start();
        if !done && t.starts_with("GRUB_CMDLINE_LINUX=") && !t.starts_with("GRUB_CMDLINE_LINUX_") {
            let (k, v) = t.split_once('=').unwrap();
            let q = v
                .chars()
                .next()
                .filter(|c| *c == '"' || *c == '\'')
                .unwrap_or('"');
            let inner = v.trim().trim_matches(|c| c == '"' || c == '\'');
            let mut words: Vec<String> = inner.split_whitespace().map(str::to_string).collect();
            for a in args {
                let key = a.split('=').next().unwrap_or(a);
                words.retain(|w| w.split('=').next() != Some(key));
                words.push(a.clone());
            }
            out.push(format!("{k}={q}{}{q}", words.join(" ")));
            done = true;
        } else {
            out.push(l.to_string());
        }
    }
    if !done {
        out.push(format!("GRUB_CMDLINE_LINUX=\"{}\"", args.join(" ")));
    }
    out.join("\n") + "\n"
}

/// The command that hibernates a Linux guest: the configured one, else the
/// first of systemd, elogind, pm-utils, uswsusp that exists.
pub fn pick_command(
    configured: &str,
    systemd: bool,
    has: &dyn Fn(&str) -> bool,
) -> Option<Vec<String>> {
    if !configured.trim().is_empty() {
        return Some(configured.split_whitespace().map(str::to_string).collect());
    }
    let c = |a: &[&str]| Some(a.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    if systemd && has("systemctl") {
        return c(&["systemctl", "hibernate"]);
    }
    if has("loginctl") {
        return c(&["loginctl", "hibernate"]);
    }
    if has("pm-hibernate") {
        return c(&["pm-hibernate"]);
    }
    if has("s2disk") {
        return c(&["s2disk"]);
    }
    None
}

/// `HibernateEnabled    REG_DWORD    0x1` (language independent).
pub fn reg_dword(text: &str, name: &str) -> Option<u32> {
    text.lines().find_map(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        (f.len() >= 3 && f[0] == name && f[1] == "REG_DWORD").then(|| {
            let v = f[2].trim_start_matches("0x");
            u32::from_str_radix(v, if f[2].starts_with("0x") { 16 } else { 10 }).ok()
        })?
    })
}

/// Reasons a Linux guest cannot hibernate yet (empty = ready).
pub struct LinuxFacts<'a> {
    pub ram_kb: u64,
    pub swaps: &'a [Swap],
    pub power_state: &'a str,
    pub cmdline: &'a str,
    pub sys_resume: &'a str,
    pub swap_pct: u64,
    pub command: Option<&'a [String]>,
    /// resume= was written to the boot configuration but this boot does not
    /// have it yet.
    pub pending_restart: bool,
}

pub fn linux_blockers(f: &LinuxFacts) -> Vec<String> {
    let mut why = Vec::new();
    if !power_state_has_disk(f.power_state) {
        why.push("the guest kernel does not offer hibernation (/sys/power/state has no \"disk\"; kernel lockdown, or built without it)".into());
    }
    let swap: u64 = f.swaps.iter().map(|s| s.size_kb).sum();
    let need = need_swap_kb(f.ram_kb, f.swap_pct);
    if !swap_enough(swap, need) {
        why.push(format!(
            "swap is {} MB but {} MB is needed ({} % of RAM): run horcrux-guest-agent --hibernate-enable",
            swap / 1024,
            need / 1024,
            f.swap_pct
        ));
    }
    let resume_set =
        cmdline_arg(f.cmdline, "resume").is_some() || !matches!(f.sys_resume.trim(), "" | "0:0");
    if !resume_set {
        why.push(if f.pending_restart {
            "hibernation is set up; restart the guest once to finish enabling it".to_string()
        } else {
            "no resume device is configured (resume= on the kernel command line): run horcrux-guest-agent --hibernate-enable".to_string()
        });
    }
    if f.command.is_none() {
        why.push(
            "no hibernate command found (systemctl, loginctl, pm-hibernate or s2disk)".to_string(),
        );
    }
    why
}

// ------------------------------------------------------------------- Linux

#[cfg(unix)]
fn read(p: &str) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

#[cfg(unix)]
fn which(c: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(c).is_file()))
        .unwrap_or(false)
        || ["/sbin", "/usr/sbin", "/bin", "/usr/bin"]
            .iter()
            .any(|d| Path::new(d).join(c).is_file())
}

#[cfg(unix)]
fn sh(cmd: &str, args: &[&str]) -> Result<String, String> {
    let o = Command::new(cmd)
        .args(args)
        .output()
        .map_err(|e| format!("{cmd}: {e}"))?;
    if o.status.success() {
        Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
    } else {
        Err(format!(
            "{cmd} {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&o.stderr).trim()
        ))
    }
}

#[cfg(unix)]
fn status(cfg: &Config) -> Value {
    let mem = read("/proc/meminfo");
    let ram_kb = meminfo_kb(&mem, "MemTotal").unwrap_or(0);
    let swaps = parse_swaps(&read("/proc/swaps"));
    let command = pick_command(
        &cfg.hibernate_command,
        Path::new("/run/systemd/system").exists(),
        &which,
    );
    let cmdline = read("/proc/cmdline");
    let pending = Path::new(&cfg.hibernate_pending_file).exists()
        && cmdline_arg(&cmdline, "resume").is_none();
    let why = linux_blockers(&LinuxFacts {
        ram_kb,
        swaps: &swaps,
        power_state: &read("/sys/power/state"),
        cmdline: &cmdline,
        sys_resume: &read("/sys/power/resume"),
        swap_pct: cfg.hibernate_swap_pct,
        command: command.as_deref(),
        pending_restart: pending,
    });
    json!({
        "os": "linux",
        "ready": why.is_empty(),
        "reason": why.first(),
        "reasons": why,
        "method": command.as_ref().map(|c| c.join(" ")),
        "ram_mb": ram_kb / 1024,
        "swap_mb": swaps.iter().map(|s| s.size_kb).sum::<u64>() / 1024,
        "restart_needed": pending,
        "agent_version": env!("CARGO_PKG_VERSION"),
    })
}

#[cfg(unix)]
fn hibernate(cfg: &Config) -> Value {
    let st = status(cfg);
    if st["ready"].as_bool() != Some(true) {
        return json!({"ok": false, "reason": st["reason"]});
    }
    let cmd = pick_command(
        &cfg.hibernate_command,
        Path::new("/run/systemd/system").exists(),
        &which,
    )
    .unwrap_or_default();
    // Detached: the guest freezes mid-way, and the answer must be out first.
    let delay = format!("{:.3}", cfg.hibernate_delay_ms as f64 / 1000.0);
    let script = format!("sleep {delay}; sync; exec \"$@\"");
    use std::os::unix::process::CommandExt;
    let mut c = Command::new("/bin/sh");
    c.arg("-c")
        .arg(script)
        .arg("horcrux-hibernate")
        .args(&cmd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // SAFETY: setsid in the child before exec; async-signal-safe.
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    match c.spawn() {
        Ok(_) => {
            json!({"ok": true, "scheduled": true, "method": cmd.join(" "), "delay_ms": cfg.hibernate_delay_ms})
        }
        Err(e) => json!({"ok": false, "reason": format!("cannot start {}: {e}", cmd.join(" "))}),
    }
}

/// Prepare a Linux guest: swap, resume device, boot configuration, initramfs.
#[cfg(unix)]
fn enable(cfg: &Config) -> Value {
    let mut done: Vec<String> = Vec::new();
    match enable_linux(cfg, &mut done) {
        Ok(()) => {
            let st = status(cfg);
            json!({"ok": true, "changed": done, "status": st})
        }
        Err(e) => json!({"ok": false, "reason": e, "changed": done}),
    }
}

#[cfg(unix)]
fn enable_linux(cfg: &Config, done: &mut Vec<String>) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let ram_kb = meminfo_kb(&read("/proc/meminfo"), "MemTotal").ok_or("cannot read RAM size")?;
    let need = need_swap_kb(ram_kb, cfg.hibernate_swap_pct);
    let swaps = parse_swaps(&read("/proc/swaps"));
    // 1. a swap area that is big enough, or a new swap file
    let mut target = swaps.iter().find(|s| swap_enough(s.size_kb, need)).cloned();
    if target.is_none() {
        let path = cfg.hibernate_swapfile.clone();
        let dir = Path::new(&path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "/".into());
        let fs = sh("findmnt", &["-no", "FSTYPE", "-T", &dir]).unwrap_or_default();
        if !matches!(fs.as_str(), "ext4" | "ext3" | "xfs" | "btrfs" | "f2fs") {
            return Err(format!(
                "{dir} is on '{fs}', which cannot hold a swap file for hibernation; add a swap partition of {} MB or point hibernate_swapfile at ext4/xfs/btrfs storage",
                need / 1024
            ));
        }
        if !Path::new(&path).exists() {
            if fs == "btrfs" {
                sh(
                    "btrfs",
                    &[
                        "filesystem",
                        "mkswapfile",
                        "--size",
                        &format!("{need}k"),
                        &path,
                    ],
                )?;
            } else {
                sh("fallocate", &["-l", &format!("{need}K"), &path])?;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                    .map_err(|e| e.to_string())?;
                sh("mkswap", &["-f", &path])?;
            }
            done.push(format!("created swap file {path} ({} MB)", need / 1024));
        }
        sh("swapon", &[&path])?;
        done.push(format!("activated {path}"));
        let fstab = read("/etc/fstab");
        if !fstab
            .lines()
            .any(|l| l.split_whitespace().next() == Some(path.as_str()))
        {
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open("/etc/fstab")
                .map_err(|e| format!("/etc/fstab: {e}"))?;
            use std::io::Write;
            let nl = if fstab.ends_with('\n') || fstab.is_empty() {
                ""
            } else {
                "\n"
            };
            writeln!(f, "{nl}{path} none swap defaults,pri=0 0 0").map_err(|e| e.to_string())?;
            done.push("added the swap file to /etc/fstab".into());
        }
        target = Some(Swap {
            name: path,
            kind: "file".into(),
            size_kb: need,
        });
    }
    let t = target.ok_or("no swap area")?;
    // 2. resume= (and resume_offset= for a file)
    let (uuid, offset) = if t.kind == "file" {
        let uuid = sh("findmnt", &["-no", "UUID", "-T", &t.name])?;
        let fs = sh("findmnt", &["-no", "FSTYPE", "-T", &t.name]).unwrap_or_default();
        let off = if fs == "btrfs" {
            sh(
                "btrfs",
                &["inspect-internal", "map-swapfile", "-r", &t.name],
            )?
            .trim()
            .parse::<u64>()
            .map_err(|e| format!("btrfs resume offset: {e}"))?
        } else {
            let bs = sh("stat", &["-f", "-c", "%S", &t.name])?
                .parse::<u64>()
                .unwrap_or(4096);
            let ff = sh("filefrag", &["-v", &t.name])?;
            filefrag_resume_offset(&ff, bs, 4096)
                .ok_or("cannot read the swap file's first block")?
        };
        (uuid, Some(off))
    } else {
        (sh("blkid", &["-s", "UUID", "-o", "value", &t.name])?, None)
    };
    if uuid.is_empty() {
        return Err(format!(
            "{} has no UUID, cannot name it as the resume device",
            t.name
        ));
    }
    let mut args = vec![format!("resume=UUID={uuid}")];
    if let Some(o) = offset {
        args.push(format!("resume_offset={o}"));
    }
    // 3. boot configuration (kernel command line)
    let cmdline = read("/proc/cmdline");
    let live_ok = args
        .iter()
        .all(|a| cmdline.split_whitespace().any(|w| w == a));
    if !live_ok {
        set_boot_args(&args, done)?;
        if let Some(d) = Path::new(&cfg.hibernate_pending_file).parent() {
            std::fs::create_dir_all(d).ok();
        }
        std::fs::write(&cfg.hibernate_pending_file, args.join(" ")).map_err(|e| e.to_string())?;
    }
    // 4. initramfs knows how to resume
    set_initramfs_resume(&uuid, offset, done)?;
    Ok(())
}

#[cfg(unix)]
fn set_boot_args(args: &[String], done: &mut Vec<String>) -> Result<(), String> {
    if which("grubby") {
        let a = args.join(" ");
        sh("grubby", &["--update-kernel=ALL", &format!("--args={a}")])?;
        done.push(format!("grubby: added {a}"));
        return Ok(());
    }
    if Path::new("/etc/default/grub").exists() {
        let old = read("/etc/default/grub");
        let new = grub_add_args(&old, args);
        if new != old {
            std::fs::write("/etc/default/grub", new).map_err(|e| e.to_string())?;
        }
        let mk: Vec<(&str, Vec<&str>)> = vec![
            ("update-grub", vec![]),
            ("grub-mkconfig", vec!["-o", "/boot/grub/grub.cfg"]),
            ("grub2-mkconfig", vec!["-o", "/boot/grub2/grub.cfg"]),
        ];
        for (c, a) in mk {
            if which(c) {
                sh(c, &a)?;
                done.push(format!("GRUB: added {} and ran {c}", args.join(" ")));
                return Ok(());
            }
        }
        return Err("GRUB's configuration tool was not found (update-grub, grub-mkconfig)".into());
    }
    Err(format!(
        "boot loader not recognised: add `{}` to the kernel command line yourself",
        args.join(" ")
    ))
}

#[cfg(unix)]
fn set_initramfs_resume(
    uuid: &str,
    offset: Option<u64>,
    done: &mut Vec<String>,
) -> Result<(), String> {
    if which("update-initramfs") {
        let dir = "/etc/initramfs-tools/conf.d";
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        std::fs::write(
            format!("{dir}/horcrux-resume"),
            format!("RESUME=UUID={uuid}\n"),
        )
        .map_err(|e| e.to_string())?;
        sh("update-initramfs", &["-u", "-k", "all"])?;
        done.push("initramfs-tools: resume device set, initramfs rebuilt".into());
    } else if which("dracut") {
        std::fs::create_dir_all("/etc/dracut.conf.d").map_err(|e| e.to_string())?;
        std::fs::write(
            "/etc/dracut.conf.d/horcrux-resume.conf",
            "add_dracutmodules+=\" resume \"\n",
        )
        .map_err(|e| e.to_string())?;
        sh("dracut", &["-f", "--regenerate-all"])?;
        done.push("dracut: resume module added, initramfs rebuilt".into());
    } else if which("mkinitcpio") {
        let conf = read("/etc/mkinitcpio.conf");
        if !conf
            .lines()
            .any(|l| l.starts_with("HOOKS") && l.contains("resume"))
        {
            let new: Vec<String> = conf
                .lines()
                .map(|l| {
                    if l.starts_with("HOOKS") && l.contains("filesystems") {
                        l.replacen("filesystems", "filesystems resume", 1)
                    } else {
                        l.to_string()
                    }
                })
                .collect();
            std::fs::write("/etc/mkinitcpio.conf", new.join("\n") + "\n")
                .map_err(|e| e.to_string())?;
        }
        sh("mkinitcpio", &["-P"])?;
        done.push("mkinitcpio: resume hook added, initramfs rebuilt".into());
    } else {
        // no initramfs generator: the kernel resumes itself from resume=
        let _ = offset;
        done.push("no initramfs tool found: the kernel resumes from resume= by itself".into());
    }
    Ok(())
}

// ----------------------------------------------------------------- Windows

#[cfg(windows)]
fn power_key() -> String {
    let o = Command::new("reg")
        .args([
            "query",
            r"HKLM\SYSTEM\CurrentControlSet\Control\Power",
            "/v",
            "HibernateEnabled",
        ])
        .output();
    o.map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
}

#[cfg(windows)]
fn status(_cfg: &Config) -> Value {
    let enabled = reg_dword(&power_key(), "HibernateEnabled") == Some(1);
    let file = Path::new(r"C:\hiberfil.sys").exists()
        || std::env::var("SystemDrive")
            .map(|d| Path::new(&format!(r"{d}\hiberfil.sys")).exists())
            .unwrap_or(false);
    let mut why = Vec::new();
    if !enabled {
        why.push("hibernation is off in Windows: run horcrux-guest-agent --hibernate-enable (powercfg /h on)".to_string());
    } else if !file {
        why.push("hiberfil.sys does not exist yet: restart Windows once".to_string());
    }
    json!({
        "os": "windows", "ready": why.is_empty(), "reason": why.first(), "reasons": why,
        "method": "shutdown /h", "agent_version": env!("CARGO_PKG_VERSION"),
    })
}

#[cfg(windows)]
fn enable(cfg: &Config) -> Value {
    let mut done = Vec::new();
    let run = |args: &[&str]| -> Result<(), String> {
        let o = Command::new("powercfg")
            .args(args)
            .output()
            .map_err(|e| e.to_string())?;
        if o.status.success() {
            Ok(())
        } else {
            Err(format!(
                "powercfg {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&o.stderr).trim()
            ))
        }
    };
    let r = run(&["/h", "on"]).and_then(|_| {
        done.push("powercfg /h on".to_string());
        if cfg.hibernate_hiberfile_pct > 0 {
            run(&["/h", "/size", &cfg.hibernate_hiberfile_pct.to_string()])?;
            done.push(format!(
                "hiberfil.sys size {} %",
                cfg.hibernate_hiberfile_pct
            ));
        }
        Ok(())
    });
    match r {
        Ok(()) => json!({"ok": true, "changed": done, "status": status(cfg)}),
        Err(e) => json!({"ok": false, "reason": e, "changed": done}),
    }
}

#[cfg(windows)]
fn hibernate(cfg: &Config) -> Value {
    use std::os::windows::process::CommandExt;
    let st = status(cfg);
    if st["ready"].as_bool() != Some(true) {
        return json!({"ok": false, "reason": st["reason"]});
    }
    // ping is the delay that works without a console (timeout.exe refuses redirected input)
    let secs = (cfg.hibernate_delay_ms / 1000).max(1) + 1;
    const DETACHED_PROCESS: u32 = 0x8;
    const NEW_GROUP: u32 = 0x200;
    match Command::new("cmd")
        .args([
            "/c",
            &format!("ping -n {secs} 127.0.0.1 >nul & shutdown /h"),
        ])
        .creation_flags(DETACHED_PROCESS | NEW_GROUP)
        .spawn()
    {
        Ok(_) => {
            json!({"ok": true, "scheduled": true, "method": "shutdown /h", "delay_ms": cfg.hibernate_delay_ms})
        }
        Err(e) => json!({"ok": false, "reason": format!("cannot start shutdown /h: {e}")}),
    }
}

// ----------------------------------------------------------------- others

#[cfg(not(any(unix, windows)))]
fn status(_cfg: &Config) -> Value {
    json!({"os": std::env::consts::OS, "ready": false, "reason": "hibernation is not supported on this OS", "reasons": []})
}
#[cfg(not(any(unix, windows)))]
fn enable(cfg: &Config) -> Value {
    status(cfg)
}
#[cfg(not(any(unix, windows)))]
fn hibernate(cfg: &Config) -> Value {
    status(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SWAPS: &str = "Filename\t\t\t\tType\t\tSize\t\tUsed\t\tPriority\n/dev/vdb2                               partition\t2097148\t\t0\t\t-2\n/swap-horcrux.img                       file\t\t1048576\t\t0\t\t-3\n";

    #[test]
    fn parses_meminfo_and_swaps() {
        assert_eq!(
            meminfo_kb("MemTotal:  8167936 kB\nMemFree: 1 kB\n", "MemTotal"),
            Some(8167936)
        );
        let s = parse_swaps(SWAPS);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].kind, "partition");
        assert_eq!(s[1].size_kb, 1048576);
        assert!(parse_swaps("Filename Type Size Used Priority\n").is_empty());
    }

    #[test]
    fn resume_offset_from_filefrag_is_in_pages() {
        let t = "Filesystem type is: ef53\nFile size of /swap is 1 (1 block of 4096 bytes)\n ext:     logical_offset:        physical_offset: length:   expected: flags:\n   0:        0..   32767:      34816..     67583:  32768:             unwritten\n";
        assert_eq!(filefrag_resume_offset(t, 4096, 4096), Some(34816));
        assert_eq!(filefrag_resume_offset(t, 1024, 4096), Some(8704));
        assert_eq!(filefrag_resume_offset("nothing", 4096, 4096), None);
    }

    #[test]
    fn grub_line_gets_the_resume_arguments_once() {
        let a = vec![
            "resume=UUID=abc".to_string(),
            "resume_offset=34816".to_string(),
        ];
        let t = "GRUB_TIMEOUT=5\nGRUB_CMDLINE_LINUX_DEFAULT=\"quiet\"\nGRUB_CMDLINE_LINUX=\"console=ttyS0 resume=UUID=old\"\n";
        let n = grub_add_args(t, &a);
        assert!(
            n.contains("GRUB_CMDLINE_LINUX=\"console=ttyS0 resume=UUID=abc resume_offset=34816\"")
        );
        assert!(n.contains("GRUB_CMDLINE_LINUX_DEFAULT=\"quiet\""));
        assert_eq!(grub_add_args(&n, &a), n);
        let none = grub_add_args("GRUB_TIMEOUT=5\n", &a);
        assert!(none.ends_with("GRUB_CMDLINE_LINUX=\"resume=UUID=abc resume_offset=34816\"\n"));
    }

    #[test]
    fn hibernate_command_is_configurable_then_detected() {
        let has = |c: &str| matches!(c, "loginctl" | "pm-hibernate");
        assert_eq!(
            pick_command("my-hib --now", true, &has),
            Some(vec!["my-hib".into(), "--now".into()])
        );
        assert_eq!(
            pick_command("", true, &has),
            Some(vec!["loginctl".into(), "hibernate".into()])
        );
        let sysd = |c: &str| c == "systemctl";
        assert_eq!(
            pick_command("", true, &sysd),
            Some(vec!["systemctl".into(), "hibernate".into()])
        );
        assert_eq!(pick_command("", false, &sysd), None);
    }

    fn facts<'a>(
        swaps: &'a [Swap],
        cmdline: &'a str,
        state: &'a str,
        cmd: Option<&'a [String]>,
    ) -> LinuxFacts<'a> {
        LinuxFacts {
            ram_kb: 2_000_000,
            swaps,
            power_state: state,
            cmdline,
            sys_resume: "0:0",
            swap_pct: 100,
            command: cmd,
            pending_restart: false,
        }
    }

    #[test]
    fn linux_readiness_names_every_missing_piece() {
        let cmd = vec!["systemctl".to_string(), "hibernate".to_string()];
        let big = parse_swaps(SWAPS);
        let ok = linux_blockers(&facts(
            &big,
            "ro resume=UUID=x",
            "freeze mem disk",
            Some(&cmd),
        ));
        assert!(ok.is_empty(), "{ok:?}");
        let w = linux_blockers(&facts(&[], "ro", "freeze mem", None));
        assert_eq!(w.len(), 4, "{w:?}");
        assert!(w[0].contains("\"disk\""));
        assert!(w[1].contains("swap is 0 MB"));
        let mut f = facts(&big, "ro", "disk", Some(&cmd));
        f.pending_restart = true;
        assert!(linux_blockers(&f)[0].contains("restart the guest once"));
        // /sys/power/resume set at runtime counts
        f.sys_resume = "254:2";
        assert!(linux_blockers(&f).is_empty());
    }

    #[test]
    fn a_swap_file_made_to_the_exact_size_counts_as_big_enough() {
        // the kernel reports one page less than the file (715 MB -> 715 MB minus 4 KB)
        assert!(swap_enough(732_156, 732_160));
        assert!(swap_enough(2_000_000, 1_000_000));
        assert!(!swap_enough(500_000, 1_000_000));
        assert!(!swap_enough(0, 1));
    }

    #[test]
    fn swap_need_follows_the_percentage() {
        assert_eq!(need_swap_kb(4_000_000, 100), 4_000_000);
        assert_eq!(need_swap_kb(4_000_000, 50), 2_000_000);
    }

    #[test]
    fn registry_dword_is_read_without_language() {
        let t = "\nHKEY_LOCAL_MACHINE\\SYSTEM\\CurrentControlSet\\Control\\Power\n    HibernateEnabled    REG_DWORD    0x1\n";
        assert_eq!(reg_dword(t, "HibernateEnabled"), Some(1));
        assert_eq!(
            reg_dword(
                "    HibernateEnabled    REG_DWORD    0x0",
                "HibernateEnabled"
            ),
            Some(0)
        );
        assert_eq!(reg_dword("", "HibernateEnabled"), None);
    }

    #[test]
    fn power_state_and_cmdline_args() {
        assert!(power_state_has_disk("freeze mem disk\n"));
        assert!(!power_state_has_disk("freeze mem\n"));
        assert_eq!(
            cmdline_arg("ro resume=UUID=a resume_offset=5", "resume").as_deref(),
            Some("UUID=a")
        );
        assert_eq!(cmdline_arg("ro resume_offset=5", "resume"), None);
    }
}
