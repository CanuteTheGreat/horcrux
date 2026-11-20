//! `horcrux-guest-agent --drivers install|remove|status`: installs the Horcrux
//! guest drivers (virtual display hxidd, virtual gamepad hxpad) shipped next
//! to the agent, run by the MSI as SYSTEM.
//!
//! Signing (v1): the packages are signed in the guest with a certificate
//! generated for this guest only (sign-drivers.ps1; the private key is
//! deleted right after), which Windows trusts through LocalMachine Root and
//! TrustedPublisher. That installs user-mode (UMDF) drivers on x64 and Arm64
//! without test mode and with Secure Boot on. No Microsoft signing programme
//! is used; see docs/design/horcrux-console.md, "Windows driver signing
//! without a paid certificate". `install-xinput`/`remove-xinput` handle the
//! optional third-party ViGEm bus (XInput) in the same module.
//!
//! The devices are software devices (SwDeviceCreate) that live until
//! removed: one virtual display adapter (no monitors until the agent asks
//! for some) and `gamepads` virtual pads.
use crate::config::Config;
use std::path::{Path, PathBuf};
use std::process::Command;
use windows::core::{HRESULT, PCWSTR};
use windows::Win32::Devices::Enumeration::Pnp::*;
use windows::Win32::System::Threading::*;

pub const ENUMERATOR: &str = "HorcruxEnum";
const PACKAGES: [&str; 2] = ["hxidd", "hxpad"];
const SIGN_SCRIPT: &str = include_str!("../../drivers/windows/sign-drivers.ps1");

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn source_dir(cfg: &Config) -> PathBuf {
    if cfg.drivers_dir.is_empty() {
        exe_dir().join("drivers")
    } else {
        PathBuf::from(&cfg.drivers_dir)
    }
}

/// Signed copies live next to the agent (Program Files: writable by
/// administrators only, so nobody can swap a file between signing and
/// installing).
fn stage_dir() -> PathBuf {
    exe_dir().join("drivers-signed")
}

fn run(cmd: &mut Command) -> Result<String, String> {
    let out = cmd
        .output()
        .map_err(|e| format!("{:?}: {e}", cmd.get_program()))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if out.status.success() {
        Ok(text)
    } else {
        Err(format!(
            "{:?} exited {}: {}",
            cmd.get_program(),
            out.status,
            text.trim()
        ))
    }
}

fn system32(exe: &str) -> PathBuf {
    let win = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    PathBuf::from(win).join("System32").join(exe)
}

fn powershell(script: &str, args: &[&str]) -> Result<String, String> {
    let dir = stage_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let f = dir.join("sign-drivers.ps1");
    std::fs::write(&f, script).map_err(|e| format!("{}: {e}", f.display()))?;
    let mut c = Command::new(system32(r"WindowsPowerShell\v1.0\powershell.exe"));
    c.args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
    ])
    .arg(&f)
    .args(args);
    let r = run(&mut c);
    let _ = std::fs::remove_file(&f);
    r
}

/// Published driver-store names (oemNN.inf) of our packages, found by the
/// catalog name inside each INF (locale-independent, unlike pnputil text).
fn published(name: &str) -> Vec<String> {
    let win = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let mut v = Vec::new();
    let Ok(rd) = std::fs::read_dir(Path::new(&win).join("INF")) else {
        return v;
    };
    let needle = format!("{name}.cat");
    for e in rd.flatten() {
        let fname = e.file_name().to_string_lossy().to_string();
        if !(fname.to_ascii_lowercase().starts_with("oem")
            && fname.to_ascii_lowercase().ends_with(".inf"))
        {
            continue;
        }
        if let Ok(t) = std::fs::read(e.path()) {
            let t = String::from_utf8_lossy(&t).to_ascii_lowercase();
            if t.contains("horcrux")
                && t.lines().any(|l| {
                    let l = l.replace(' ', "");
                    l.starts_with("catalogfile=") && l.ends_with(&needle)
                })
            {
                v.push(fname);
            }
        }
    }
    v
}

struct Created {
    hr: HRESULT,
    id: String,
}

unsafe extern "system" fn created_cb(
    _h: HSWDEVICE,
    hr: HRESULT,
    ctx: *const core::ffi::c_void,
    id: PCWSTR,
) {
    // SAFETY: ctx is the (Mutex<Option<Created>>, event) pair owned by create_device.
    let (slot, ev) = &*(ctx as *const (
        std::sync::Mutex<Option<Created>>,
        windows::Win32::Foundation::HANDLE,
    ));
    let id = if id.is_null() {
        String::new()
    } else {
        id.to_string().unwrap_or_default()
    };
    if let Ok(mut s) = slot.lock() {
        *s = Some(Created { hr, id });
    }
    let _ = SetEvent(*ev);
}

/// Create (or re-open) a persistent software device.
fn create_device(
    instance: &str,
    hwid: &str,
    desc: &str,
    timeout_ms: u32,
) -> Result<String, String> {
    let inst = wide(instance);
    let mut hw: Vec<u16> = hwid.encode_utf16().collect();
    hw.extend([0, 0]);
    let compat = [0u16, 0];
    let d = wide(desc);
    let en = wide(ENUMERATOR);
    let parent = wide(r"HTREE\ROOT\0");
    let info = SW_DEVICE_CREATE_INFO {
        cbSize: std::mem::size_of::<SW_DEVICE_CREATE_INFO>() as u32,
        pszInstanceId: PCWSTR(inst.as_ptr()),
        pszzHardwareIds: PCWSTR(hw.as_ptr()),
        pszzCompatibleIds: PCWSTR(compat.as_ptr()),
        pContainerId: std::ptr::null(),
        CapabilityFlags: (SWDeviceCapabilitiesSilentInstall.0
            | SWDeviceCapabilitiesDriverRequired.0) as u32,
        pszDeviceDescription: PCWSTR(d.as_ptr()),
        pszDeviceLocation: PCWSTR::null(),
        pSecurityDescriptor: std::ptr::null(),
    };
    // SAFETY: every pointer in `info` and `ctx` outlives the wait below; the
    // callback only runs until the event is set.
    unsafe {
        let ev = CreateEventW(None, true, false, PCWSTR::null()).map_err(|e| e.to_string())?;
        let ctx = Box::new((std::sync::Mutex::new(None::<Created>), ev));
        let h = SwDeviceCreate(
            PCWSTR(en.as_ptr()),
            PCWSTR(parent.as_ptr()),
            &info,
            None,
            Some(created_cb),
            Some(&*ctx as *const _ as *const core::ffi::c_void),
        )
        .map_err(|e| format!("SwDeviceCreate {instance}: {e}"))?;
        let w = WaitForSingleObject(ev, timeout_ms);
        let res = ctx.0.lock().ok().and_then(|mut s| s.take());
        let r = match res {
            Some(c) if c.hr.is_ok() => SwDeviceSetLifetime(h, SWDeviceLifetimeParentPresent)
                .map_err(|e| format!("SwDeviceSetLifetime {instance}: {e}"))
                .map(|_| c.id),
            Some(c) => Err(format!("create {instance}: {}", c.hr)),
            None => Err(format!(
                "create {instance}: no answer within {timeout_ms} ms (wait {w:?})"
            )),
        };
        SwDeviceClose(h);
        let _ = windows::Win32::Foundation::CloseHandle(ev);
        drop(ctx);
        r
    }
}

fn device_ids(cfg: &Config) -> Vec<(String, &'static str, &'static str)> {
    let mut v = Vec::new();
    if cfg.virtual_display != "off" {
        v.push((
            "HxIdd0".to_string(),
            "HorcruxVirtualDisplay",
            "Horcrux Virtual Display",
        ));
    }
    // the xinput backend feeds the ViGEm bus instead: a second (HID) pad
    // would show up as a second player in games that read both
    for i in 0..if cfg.gamepad_backend == "xinput" {
        0
    } else {
        cfg.gamepads
    } {
        v.push((
            format!("HxPad{i}"),
            "HorcruxVirtualGamepad",
            "Horcrux Virtual Gamepad",
        ));
    }
    v
}

/// Is the device `SWD\HorcruxEnum\<instance>` present?
fn device_present(instance: &str) -> bool {
    use windows::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Locate_DevNodeW, CM_LOCATE_DEVNODE_NORMAL, CR_SUCCESS,
    };
    let id = wide(&format!(r"SWD\{ENUMERATOR}\{instance}"));
    let mut dn = 0u32;
    // SAFETY: id is NUL-terminated; dn is a valid out pointer.
    unsafe {
        CM_Locate_DevNodeW(&mut dn, PCWSTR(id.as_ptr()), CM_LOCATE_DEVNODE_NORMAL) == CR_SUCCESS
    }
}

/// Service start: when the driver packages are installed, (re)create any
/// configured device that is missing (e.g. after an unclean power-off lost
/// the device registration). Returns what it did, for the service log.
pub fn ensure(cfg: &Config) -> Vec<String> {
    let mut done = Vec::new();
    for (inst, hw, desc) in device_ids(cfg) {
        let pkg = if hw == "HorcruxVirtualDisplay" {
            "hxidd"
        } else {
            "hxpad"
        };
        if published(pkg).is_empty() || device_present(&inst) {
            continue;
        }
        match create_device(&inst, hw, desc, cfg.driver_device_timeout_ms as u32) {
            Ok(id) => done.push(format!("re-created {id}")),
            Err(e) => done.push(e),
        }
    }
    done
}

pub fn install(cfg: &Config) -> Result<(), String> {
    let src = source_dir(cfg);
    let stage = stage_dir();
    let _ = std::fs::remove_dir_all(&stage);
    for p in PACKAGES {
        let to = stage.join(p);
        std::fs::create_dir_all(&to).map_err(|e| format!("{}: {e}", to.display()))?;
        for ext in ["dll", "inf"] {
            let f = src.join(p).join(format!("{p}.{ext}"));
            std::fs::copy(&f, to.join(format!("{p}.{ext}")))
                .map_err(|e| format!("{}: {e}", f.display()))?;
        }
    }
    // old packages first: their certificate goes with them
    let old_certs = remove_packages();
    let subject = format!("CN={}", cfg.driver_cert_subject);
    let out = powershell(
        SIGN_SCRIPT,
        &["-Dir", &stage.display().to_string(), "-Subject", &subject],
    )?;
    let thumb = out
        .lines()
        .find_map(|l| l.trim().strip_prefix("THUMBPRINT="))
        .ok_or(format!("signing gave no certificate: {}", out.trim()))?
        .to_string();
    remove_certs(&cfg.driver_cert_subject, Some(&thumb));
    let _ = old_certs;
    let pnputil = system32("pnputil.exe");
    for p in PACKAGES {
        let inf = stage.join(p).join(format!("{p}.inf"));
        run(Command::new(&pnputil)
            .arg("/add-driver")
            .arg(&inf)
            .arg("/install"))?;
        println!("installed {p} ({})", published(p).join(", "));
    }
    for (inst, hw, desc) in device_ids(cfg) {
        let id = create_device(&inst, hw, desc, cfg.driver_device_timeout_ms as u32)?;
        println!("device {id}");
    }
    println!("certificate {thumb} ({subject}; private key deleted)");
    Ok(())
}

/// Replace (or append) `key = value` in the guest-tools settings file.
fn set_conf_key(key: &str, value: &str) -> Result<(), String> {
    let file = crate::config::config_file();
    let old = std::fs::read_to_string(&file).unwrap_or_default();
    let mut found = false;
    let mut out: Vec<String> = old
        .lines()
        .map(|l| {
            let t = l.trim();
            let is_key = !t.starts_with('#')
                && t.split_once('=')
                    .is_some_and(|(k, _)| k.trim().replace('-', "_") == key);
            if is_key {
                found = true;
                format!("{key} = {value}")
            } else {
                l.to_string()
            }
        })
        .collect();
    if !found {
        out.push(format!("{key} = {value}"));
    }
    if let Some(dir) = Path::new(&file).parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&file, out.join("\r\n") + "\r\n").map_err(|e| format!("{file}: {e}"))
}

fn vigem_installer(cfg: &Config) -> Result<PathBuf, String> {
    if !cfg.vigem_installer.is_empty() {
        return Ok(PathBuf::from(&cfg.vigem_installer));
    }
    let dir = exe_dir().join("optional");
    std::fs::read_dir(&dir)
        .ok()
        .and_then(|rd| {
            rd.flatten().map(|e| e.path()).find(|p| {
                let n = p
                    .file_name()
                    .map(|n| n.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default();
                n.starts_with("vigembus") && n.ends_with(".exe")
            })
        })
        .ok_or(format!(
            "no ViGEm bus setup program in {} (or set vigem_installer)",
            dir.display()
        ))
}

/// SHA-256 of a file as the inbox `certutil` prints it (lowercase hex).
fn sha256_of(f: &Path) -> Result<String, String> {
    let out = run(Command::new(system32("certutil.exe"))
        .arg("-hashfile")
        .arg(f)
        .arg("SHA256"))?;
    out.lines()
        .map(|l| l.replace(' ', "").to_ascii_lowercase())
        .find(|l| l.len() == 64 && l.bytes().all(|c| c.is_ascii_hexdigit()))
        .ok_or(format!("certutil gave no hash: {}", out.trim()))
}

/// Refuse a setup program that is not the pinned one or is not validly
/// signed by the expected publisher. Both checks run: the hash pins the
/// exact file, the signature shows who vouches for the kernel driver in it.
fn check_vigem_installer(cfg: &Config, f: &Path) -> Result<(), String> {
    let got = sha256_of(f)?;
    if got != cfg.vigem_sha256 {
        return Err(format!(
            "{}: SHA-256 {got} is not the expected {} (vigem_sha256)",
            f.display(),
            cfg.vigem_sha256
        ));
    }
    let script = "$s = Get-AuthenticodeSignature -LiteralPath $args[0]; \
         Write-Output ('STATUS=' + $s.Status); \
         if ($s.SignerCertificate) { Write-Output ('SIGNER=' + $s.SignerCertificate.Subject) }";
    let out = run(
        Command::new(system32(r"WindowsPowerShell\v1.0\powershell.exe")).args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!(
                "& {{ {script} }} '{}'",
                f.display().to_string().replace('\'', "''")
            ),
        ]),
    )?;
    let status = out
        .lines()
        .find_map(|l| l.trim().strip_prefix("STATUS="))
        .unwrap_or("");
    let signer = out
        .lines()
        .find_map(|l| l.trim().strip_prefix("SIGNER="))
        .unwrap_or("");
    if status != "Valid" || !signer.contains(&cfg.vigem_signer) {
        return Err(format!(
            "{}: signature {status:?} by {signer:?}, expected a valid one from {:?} (vigem_signer)",
            f.display(),
            cfg.vigem_signer
        ));
    }
    Ok(())
}

/// Wait until the ViGEm bus appears (or vanishes) after setup/uninstall.
fn wait_vigem(present: bool, timeout_ms: u64) -> bool {
    let end = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    loop {
        if super::xinput::bus_present() == present {
            return true;
        }
        if std::time::Instant::now() >= end {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

/// Present while the ViGEm bus on this guest was put there by us; only then
/// do we remove it again (a bus the guest owner installed is theirs).
fn vigem_marker() -> PathBuf {
    Path::new(&crate::config::config_file()).with_file_name("xinput-installed-by-horcrux")
}

/// Run the ViGEm uninstall when we installed it. Returns whether it ran.
fn uninstall_vigem_if_ours(cfg: &Config) -> Result<bool, String> {
    if !vigem_marker().exists() {
        return Ok(false);
    }
    if super::xinput::bus_present() {
        let f = vigem_installer(cfg)?;
        check_vigem_installer(cfg, &f)?;
        let out = Command::new(&f)
            .args(["/uninstall", "/quiet", "/norestart", "/log"])
            .arg(vigem_log())
            .output()
            .map_err(|e| format!("{}: {e}", f.display()))?;
        let code = out.status.code().unwrap_or(-1);
        if code != 0 && code != 3010 {
            return Err(format!(
                "{} /uninstall exited {code}: {}",
                f.display(),
                vigem_log_hint()
            ));
        }
        wait_vigem(false, cfg.driver_device_timeout_ms);
    }
    let _ = std::fs::remove_file(vigem_marker());
    Ok(true)
}

/// Where the ViGEm setup program writes its log (next to the settings file).
fn vigem_log() -> PathBuf {
    Path::new(&crate::config::config_file()).with_file_name("vigem-setup.log")
}

/// The setup's own explanation of a failure: its (Windows Installer) log lines
/// that say why, e.g. the launch condition that refuses Windows Server.
fn vigem_log_hint() -> String {
    let text = std::fs::read(vigem_log())
        .map(|b| String::from_utf8_lossy(&b).replace('\0', ""))
        .unwrap_or_default();
    let mut seen: Vec<String> = Vec::new();
    for l in text.lines() {
        let low = l.to_ascii_lowercase();
        if low.contains("can not be installed")
            || low.contains("cannot be installed")
            || low.contains("installation failed")
            || low.contains("error 0x")
        {
            // "MSI (s) (..) [..]: Product: Name -- Reason": keep the reason
            let l = l.rsplit(" -- ").next().unwrap_or(l).trim().to_string();
            if !l.is_empty() && !seen.contains(&l) {
                seen.push(l);
            }
        }
    }
    if seen.is_empty() {
        format!("(setup log: {})", vigem_log().display())
    } else {
        format!(
            "{} (setup log: {})",
            seen.iter().take(3).cloned().collect::<Vec<_>>().join(" | "),
            vigem_log().display()
        )
    }
}

/// `--drivers install-xinput`: install the third-party ViGEm bus driver
/// (opt-in; see the module docs of `xinput`), switch the agent's gamepad
/// backend to it and drop the Horcrux HID pads so games see one controller.
pub fn install_xinput(cfg: &Config) -> Result<(), String> {
    let f = vigem_installer(cfg)?;
    check_vigem_installer(cfg, &f)?;
    if !super::xinput::bus_present() {
        let marker = vigem_marker();
        if let Some(dir) = marker.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(
            &marker,
            "ViGEm bus installed by horcrux-guest-agent --drivers install-xinput\r\n",
        )
        .map_err(|e| format!("{}: {e}", marker.display()))?;
        let out = Command::new(&f)
            .args(["/quiet", "/norestart", "/log"])
            .arg(vigem_log())
            .output()
            .map_err(|e| format!("{}: {e}", f.display()))?;
        // 0 = installed, 3010 = installed, restart pending
        let code = out.status.code().unwrap_or(-1);
        if code != 0 && code != 3010 {
            let _ = std::fs::remove_file(vigem_marker()); // nothing was installed
            return Err(format!(
                "{} exited {code}: {}",
                f.display(),
                vigem_log_hint()
            ));
        }
        if !wait_vigem(true, cfg.driver_device_timeout_ms) {
            return Err(format!(
                "ViGEm bus setup finished (exit {code}) but the bus did not appear{}",
                if code == 3010 {
                    "; a restart is pending"
                } else {
                    ""
                }
            ));
        }
    }
    set_conf_key("gamepad_backend", "xinput")?;
    let pnputil = system32("pnputil.exe");
    for i in 0..cfg.gamepads.max(16) {
        let _ =
            run(Command::new(&pnputil)
                .args(["/remove-device", &format!(r"SWD\{ENUMERATOR}\HxPad{i}")]));
    }
    println!("xinput backend ready (ViGEm bus present; Horcrux HID pads removed)");
    Ok(())
}

/// `--drivers remove-xinput`: back to the Horcrux HID gamepad.
pub fn remove_xinput(cfg: &Config) -> Result<(), String> {
    set_conf_key("gamepad_backend", "hid")?;
    if !uninstall_vigem_if_ours(cfg)? && super::xinput::bus_present() {
        println!("the ViGEm bus was not installed by Horcrux: left in place");
    }
    // bring the HID pads back when the Horcrux drivers are installed
    let mut hid = cfg.clone();
    hid.gamepad_backend = "hid".into();
    for (inst, hw, desc) in device_ids(&hid) {
        if hw == "HorcruxVirtualGamepad" && !published("hxpad").is_empty() && !device_present(&inst)
        {
            let id = create_device(&inst, hw, desc, cfg.driver_device_timeout_ms as u32)?;
            println!("device {id}");
        }
    }
    println!("xinput backend removed");
    Ok(())
}

/// Facts the guest's owner needs to judge the signing situation, read from
/// the running system (never changed by us).
fn security_state() -> serde_json::Value {
    let reg = |args: &[&str]| run(Command::new(system32("reg.exe")).args(args)).unwrap_or_default();
    let sb = reg(&[
        "query",
        r"HKLM\SYSTEM\CurrentControlSet\Control\SecureBoot\State",
        "/v",
        "UEFISecureBootEnabled",
    ]);
    let secure_boot = if sb.contains("0x1") {
        serde_json::json!(true)
    } else if sb.contains("0x0") {
        serde_json::json!(false)
    } else {
        serde_json::Value::Null // legacy BIOS or unreadable
    };
    // test mode shows as a watermark; the loader option is in the BCD store
    let bcd = run(Command::new(system32("bcdedit.exe")).args(["/enum", "{current}"]))
        .unwrap_or_default()
        .to_ascii_lowercase();
    let test_signing = bcd
        .lines()
        .any(|l| l.trim_start().starts_with("testsigning") && l.contains("yes"));
    serde_json::json!({"secure_boot": secure_boot, "test_signing": test_signing})
}

/// Delete our driver packages from the driver store (devices using them
/// lose their driver). Returns the published names removed.
fn remove_packages() -> Vec<String> {
    let pnputil = system32("pnputil.exe");
    let mut gone = Vec::new();
    for p in PACKAGES {
        for oem in published(p) {
            if run(Command::new(&pnputil).args(["/delete-driver", &oem, "/uninstall", "/force"]))
                .is_ok()
            {
                gone.push(oem);
            }
        }
    }
    gone
}

/// Remove trusted Horcrux driver certificates (all, or all but `keep`).
fn remove_certs(subject: &str, keep: Option<&str>) {
    let keep = keep.unwrap_or("");
    let script = format!(
        "Get-ChildItem Cert:\\LocalMachine\\Root, Cert:\\LocalMachine\\TrustedPublisher | \
         Where-Object {{ $_.Subject -eq 'CN={}' -and $_.Thumbprint -ne '{}' }} | Remove-Item",
        subject.replace('\'', "''"),
        keep
    );
    let _ = run(
        Command::new(system32(r"WindowsPowerShell\v1.0\powershell.exe")).args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &script,
        ]),
    );
}

pub fn remove(cfg: &Config) -> Result<(), String> {
    if let Err(e) = uninstall_vigem_if_ours(cfg) {
        eprintln!("horcrux-guest-agent: {e}");
    }
    let pnputil = system32("pnputil.exe");
    // every instance we may have created (gamepads may have been lowered since)
    let mut ids: Vec<String> = vec!["HxIdd0".into()];
    ids.extend((0..cfg.gamepads.max(16)).map(|i| format!("HxPad{i}")));
    for inst in ids {
        let _ =
            run(Command::new(&pnputil)
                .args(["/remove-device", &format!(r"SWD\{ENUMERATOR}\{inst}")]));
    }
    for oem in remove_packages() {
        println!("removed {oem}");
    }
    remove_certs(&cfg.driver_cert_subject, None);
    let _ = std::fs::remove_dir_all(stage_dir());
    Ok(())
}

pub fn status() -> String {
    let mut s = serde_json::Map::new();
    for p in PACKAGES {
        s.insert(format!("{p}_packages"), serde_json::json!(published(p)));
    }
    match super::idd::Idd::open().and_then(|i| i.info()) {
        Ok(i) => {
            let mons: Vec<_> = i
                .active()
                .iter()
                .map(|m| {
                    serde_json::json!({"id": m.id, "flags": m.flags, "width": m.width, "height": m.height,
                        "refresh_mhz": m.refresh_mhz, "frames": m.frames, "swapchain": m.has_swapchain != 0,
                        "share_generation": m.share_generation, "last_error": format!("0x{:08x}", m.last_error)})
                })
                .collect();
            s.insert(
                "virtual_display".into(),
                serde_json::json!({"driver_version": format!("{}.{}", i.driver_version >> 16, i.driver_version & 0xffff),
                    "iddcx_version": format!("0x{:x}", i.iddcx_version), "max_monitors": i.max_monitors,
                    "adapter_ready": i.adapter_ready != 0, "monitors": mons}),
            );
        }
        Err(e) => {
            s.insert("virtual_display_error".into(), serde_json::json!(e));
        }
    }
    s.insert(
        "gamepads".into(),
        serde_json::json!(super::pad::Pads::open().count()),
    );
    s.insert(
        "xinput_bus".into(),
        serde_json::json!(super::xinput::bus_present()),
    );
    s.insert("signing".into(), security_state());
    serde_json::to_string_pretty(&s).unwrap_or_default()
}
