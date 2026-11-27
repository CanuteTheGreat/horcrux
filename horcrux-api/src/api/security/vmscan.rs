//! VM vulnerability scanning (docs/design/security-suite.md, phase 2).
//!
//! A VM is never scanned by reading its live disk. Instead:
//!
//! * **Running VM** (`vm_agent_scan`): the Horcrux guest agent reports the
//!   guest's package inventory (`horcrux-guest-agent --inventory`, run through
//!   the QEMU guest-agent channel). The node turns that into an SBOM and the
//!   scanner reads it (`sbom:`). No host access to the guest file system.
//! * **Stopped VM** (`vm_disk_scan`): the disk is attached read-only through
//!   `qemu-nbd --read-only` (the image is never written), the guest file
//!   system is mounted read-only, scanned with `dir:`, then unmounted and
//!   disconnected. An encrypted guest volume (LUKS, BitLocker) is skipped
//!   with a clear reason — Horcrux never unlocks a guest disk.
//!
//! `prepare` produces the scanner input (an SBOM file, or a mounted
//! directory) wrapped in a guard that unmounts / removes it on drop; the
//! caller runs the scanner and drops the guard.
use super::super::system_admin::{data_dir, which};
use super::vulnscan::ScanInput;
use crate::vm::devices::qga_exec;
use crate::vm::display::DisplayRuntime;
use horcrux_common::security::{
    encrypted_fstype, GuestInventory, ScanTarget, ScanTargetKind, VulnScanSettings,
};
use horcrux_common::{VmConfig, VmHypervisor, VmStatus};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Scannable VMs: QEMU guests only (LXD/Incus VMs are managed as containers).
/// The detail carries the name and run state so the UI can show a badge.
pub fn vm_targets(vms: &[VmConfig]) -> Vec<ScanTarget> {
    let mut out: Vec<ScanTarget> = vms
        .iter()
        .filter(|v| v.hypervisor == VmHypervisor::Qemu && !v.disks.is_empty())
        .map(|v| ScanTarget {
            kind: ScanTargetKind::Vm,
            name: v.id.clone(),
            detail: format!("{} ({})", v.name, v.status),
            last_scan: None,
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The VM a scan names (by id).
pub fn find_vm<'a>(vms: &'a [VmConfig], id: &str) -> Option<&'a VmConfig> {
    vms.iter().find(|v| v.id == id)
}

/// A prepared scanner input and the cleanup it needs.
pub enum Prepared {
    /// An SBOM file (running VM); removed on drop.
    Sbom(TempFile),
    /// A read-only mount of a stopped VM's guest file system; unmounted and
    /// the nbd export disconnected on drop.
    Dir(Mount),
}

impl Prepared {
    pub fn input(&self) -> ScanInput<'_> {
        match self {
            Prepared::Sbom(f) => ScanInput::Sbom(&f.path),
            Prepared::Dir(m) => ScanInput::Dir(&m.mountpoint),
        }
    }
}

/// Prepare the scanner input for a VM (running: SBOM from the guest agent;
/// stopped: a read-only mount of its disk). Errors carry a clear reason the
/// UI shows (guest tools missing, encrypted disk, no mountable file system).
pub async fn prepare(vs: &VulnScanSettings, vm: &VmConfig) -> Result<Prepared, String> {
    let running = matches!(vm.status, VmStatus::Running | VmStatus::Paused);
    if running {
        if !vs.vm_agent_scan {
            return Err("scanning running VMs is turned off (vuln_scan.vm_agent_scan)".into());
        }
        let inv = running_inventory(vm, vs.timeout_secs.min(120) as u64).await?;
        if inv.packages.is_empty() {
            return Err(
                "the guest reported no packages (is a supported package manager present?)".into(),
            );
        }
        let f = TempFile::new("sbom", "json")?;
        std::fs::write(&f.path, inv.to_sbom()).map_err(|e| format!("{}: {e}", f.path.display()))?;
        Ok(Prepared::Sbom(f))
    } else {
        if !vs.vm_disk_scan {
            return Err("scanning stopped VMs is turned off (vuln_scan.vm_disk_scan)".into());
        }
        let disk = vm
            .disks
            .first()
            .map(|d| PathBuf::from(&d.path))
            .ok_or("the VM has no disk to scan")?;
        if !disk.is_file() {
            return Err(format!("disk image {} not found", disk.display()));
        }
        scan_mount(&disk, vs.vm_mount_timeout_secs as u64).await
    }
}

// ----------------------------------------------------- running VM: inventory

/// Run `horcrux-guest-agent --inventory` in the guest and parse its JSON.
async fn running_inventory(vm: &VmConfig, secs: u64) -> Result<GuestInventory, String> {
    let disk = PathBuf::from(&vm.disks.first().map(|d| d.path.clone()).unwrap_or_default());
    let sock = DisplayRuntime::qga_socket(&disk);
    // Windows guests install the agent as horcrux-guest-agent.exe; try both.
    let mut last = String::new();
    for path in ["horcrux-guest-agent", "horcrux-guest-agent.exe"] {
        match guest_run(&sock, path, &["--inventory".into()], secs).await {
            Ok((code, out)) => {
                if code != 0 && out.trim().is_empty() {
                    last = format!("the guest agent exited with {code}");
                    continue;
                }
                return parse_inventory_output(&out);
            }
            Err(e) => last = e,
        }
    }
    Err(format!(
        "could not read the guest inventory: {last}. Install the Horcrux guest tools in the guest \
         and enable the guest agent."
    ))
}

/// Parse `horcrux-guest-agent --inventory` stdout into an inventory.
pub fn parse_inventory_output(stdout: &str) -> Result<GuestInventory, String> {
    let line = stdout
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with('{'))
        .ok_or("the guest agent did not return an inventory")?;
    serde_json::from_str(line.trim()).map_err(|e| format!("guest inventory is not valid JSON: {e}"))
}

/// Run `path args` in the guest through the QEMU guest agent and wait for it;
/// returns (exit code, stdout).
async fn guest_run(
    sock: &Path,
    path: &str,
    args: &[String],
    secs: u64,
) -> Result<(i64, String), String> {
    if !sock.exists() {
        return Err("the VM has no guest agent channel".into());
    }
    let r = qga_exec(
        sock,
        "guest-exec",
        json!({"path": path, "arg": args, "capture-output": true}),
        secs.min(15),
    )
    .await
    .map_err(|e| e.to_string())?;
    let pid = r["pid"]
        .as_i64()
        .ok_or("guest-exec returned no pid".to_string())?;
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(400)).await;
        let Ok(st) = qga_exec(
            sock,
            "guest-exec-status",
            json!({ "pid": pid }),
            secs.min(10),
        )
        .await
        else {
            continue;
        };
        if st["exited"].as_bool() == Some(true) {
            return Ok((
                st["exitcode"].as_i64().unwrap_or(-1),
                decode_b64(st.get("out-data")),
            ));
        }
    }
    Err(format!("the guest agent did not finish within {secs} s"))
}

/// Decode a base64 `out-data` field (QGA captures stdout base64-encoded).
fn decode_b64(v: Option<&Value>) -> String {
    let Some(s) = v.and_then(|x| x.as_str()) else {
        return String::new();
    };
    match b64_decode(s) {
        Some(b) => String::from_utf8_lossy(&b).into_owned(),
        None => String::new(),
    }
}

/// Minimal standard base64 decoder (QGA out-data), no padding tolerance.
fn b64_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::new();
    let mut buf = 0u32;
    let mut bits = 0u32;
    for c in s.bytes() {
        if c == b'=' || c.is_ascii_whitespace() {
            continue;
        }
        let v = val(c)?;
        buf = (buf << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Some(out)
}

// ------------------------------------------------- stopped VM: disk mount

/// One block device row from `lsblk -J`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    pub path: String,
    pub fstype: String,
    pub kind: String,
}

/// Parse `lsblk -J -o PATH,FSTYPE,TYPE <dev>` recursively into a flat list.
pub fn parse_lsblk(json_text: &str) -> Vec<Part> {
    fn walk(v: &Value, out: &mut Vec<Part>) {
        for d in v["blockdevices"].as_array().into_iter().flatten() {
            collect(d, out);
        }
    }
    fn collect(d: &Value, out: &mut Vec<Part>) {
        out.push(Part {
            path: d["path"].as_str().unwrap_or("").to_string(),
            fstype: d["fstype"].as_str().unwrap_or("").to_string(),
            kind: d["type"].as_str().unwrap_or("").to_string(),
        });
        for c in d["children"].as_array().into_iter().flatten() {
            collect(c, out);
        }
    }
    let v: Value = serde_json::from_str(json_text).unwrap_or(Value::Null);
    let mut out = Vec::new();
    walk(&v, &mut out);
    out
}

/// If any volume is an encrypted file system, the reason to skip the whole VM.
pub fn encrypted_reason(parts: &[Part]) -> Option<String> {
    parts.iter().find_map(|p| {
        encrypted_fstype(&p.fstype).map(|r| format!("the guest disk is encrypted ({r})"))
    })
}

/// File systems the scan can mount read-only.
pub fn is_mountable(fstype: &str) -> bool {
    matches!(
        fstype,
        "ext2" | "ext3" | "ext4" | "xfs" | "btrfs" | "vfat" | "ntfs" | "ntfs3" | "f2fs"
    )
}

/// Mount a stopped VM's disk read-only and return the mount guard.
async fn scan_mount(disk: &Path, secs: u64) -> Result<Prepared, String> {
    // nbd and mounting need root; refuse clearly otherwise.
    if !is_root() {
        return Err("stopped-VM disk scanning needs root on the node".into());
    }
    if which("qemu-nbd").is_none() {
        return Err("qemu-nbd is not installed (app-emulation/qemu with the nbd use flag)".into());
    }
    let fut = mount_and_probe(disk);
    match tokio::time::timeout(Duration::from_secs(secs.max(10)), fut).await {
        Ok(r) => r,
        Err(_) => Err(format!("preparing the disk scan took longer than {secs} s")),
    }
}

async fn mount_and_probe(disk: &Path) -> Result<Prepared, String> {
    let fmt = qemu_img_format(disk).await.unwrap_or_else(|| "raw".into());
    let dev = connect_nbd(disk, &fmt).await?;
    // From here on, always disconnect on an error path.
    let guard = NbdGuard { dev: dev.clone() };
    // Give the kernel a moment to scan the partition table.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let lsblk = run_out("lsblk", &["-J", "-o", "PATH,FSTYPE,TYPE", &dev])
        .await
        .unwrap_or_default();
    let parts = parse_lsblk(&lsblk);
    if let Some(reason) = encrypted_reason(&parts) {
        return Err(format!(
            "{reason}; scan the running guest through its agent instead"
        ));
    }
    // Candidate file systems: the whole device if it has one, else partitions.
    let candidates: Vec<&Part> = parts
        .iter()
        .filter(|p| is_mountable(&p.fstype) && (p.kind == "part" || p.kind == "disk"))
        .collect();
    if candidates.is_empty() {
        return Err("no mountable guest file system was found on the disk".into());
    }
    let mp = mkmount_dir()?;
    for p in &candidates {
        if mount_ro(&p.path, &p.fstype, &mp).await.is_ok() {
            // A root file system has an os-release or a package database.
            if looks_like_root(&mp) {
                let m = Mount {
                    mountpoint: mp,
                    _nbd: guard,
                };
                return Ok(Prepared::Dir(m));
            }
            let _ = run_out("umount", &[mp.to_string_lossy().as_ref()]).await;
        }
    }
    // Fall back to the first that mounts at all (e.g. a single-fs appliance).
    for p in &candidates {
        if mount_ro(&p.path, &p.fstype, &mp).await.is_ok() {
            let m = Mount {
                mountpoint: mp,
                _nbd: guard,
            };
            return Ok(Prepared::Dir(m));
        }
    }
    let _ = std::fs::remove_dir(&mp);
    Err("the guest file system could not be mounted read-only".into())
}

/// A file system that holds an OS (so `dir:` finds real packages).
fn looks_like_root(mp: &Path) -> bool {
    mp.join("etc/os-release").exists()
        || mp.join("var/lib/dpkg/status").exists()
        || mp.join("var/lib/rpm").is_dir()
        || mp.join("lib/apk/db/installed").exists()
        || mp.join("var/db/pkg").is_dir()
        || mp.join("Windows/System32").is_dir()
}

async fn qemu_img_format(disk: &Path) -> Option<String> {
    let out = run_out(
        "qemu-img",
        &["info", "--output=json", disk.to_string_lossy().as_ref()],
    )
    .await
    .ok()?;
    let v: Value = serde_json::from_str(&out).ok()?;
    v["format"].as_str().map(|s| s.to_string())
}

/// Attach the disk read-only to a free /dev/nbdN and return the device.
async fn connect_nbd(disk: &Path, fmt: &str) -> Result<String, String> {
    let _ = run_out("modprobe", &["nbd", "max_part=16"]).await;
    for i in 0..16 {
        let dev = format!("/dev/nbd{i}");
        let size = std::fs::read_to_string(format!("/sys/class/block/nbd{i}/size"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        if size != "0" {
            continue; // in use
        }
        let r = run_out(
            "qemu-nbd",
            &[
                "--read-only",
                "--format",
                fmt,
                "--connect",
                &dev,
                disk.to_string_lossy().as_ref(),
            ],
        )
        .await;
        if r.is_ok() {
            return Ok(dev);
        }
    }
    Err("no free nbd device to attach the disk read-only".into())
}

async fn mount_ro(dev: &str, fstype: &str, mp: &Path) -> Result<(), String> {
    // Read-only, never execute or honour setuid/dev nodes from the guest, and
    // never replay a journal (the backing nbd export is read-only).
    let opts = match fstype {
        "ext2" | "ext3" | "ext4" => "ro,noload,noexec,nosuid,nodev",
        "xfs" => "ro,norecovery,noexec,nosuid,nodev",
        "ntfs" | "ntfs3" => "ro,noexec,nosuid,nodev",
        _ => "ro,noexec,nosuid,nodev",
    };
    run_out(
        "mount",
        &["-t", fstype, "-o", opts, dev, mp.to_string_lossy().as_ref()],
    )
    .await
    .map(|_| ())
}

fn mkmount_dir() -> Result<PathBuf, String> {
    let base = data_dir().join("security").join("vm-mounts");
    std::fs::create_dir_all(&base).map_err(|e| format!("{}: {e}", base.display()))?;
    let p = base.join(format!(
        "m{}",
        std::process::id().wrapping_add(rand_suffix())
    ));
    std::fs::create_dir_all(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(p)
}

fn rand_suffix() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0)
}

fn is_root() -> bool {
    // SAFETY: geteuid is always safe.
    unsafe { nix::libc::geteuid() == 0 }
}

async fn run_out(prog: &str, args: &[&str]) -> Result<String, String> {
    let out = tokio::process::Command::new(prog)
        .args(args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| format!("{prog}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!(
            "{prog} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Removes a temp file on drop.
pub struct TempFile {
    pub path: PathBuf,
}

impl TempFile {
    fn new(prefix: &str, ext: &str) -> Result<Self, String> {
        let base = data_dir().join("security").join("tmp");
        std::fs::create_dir_all(&base).map_err(|e| format!("{}: {e}", base.display()))?;
        let path = base.join(format!(
            "{prefix}-{}-{}.{ext}",
            std::process::id(),
            rand_suffix()
        ));
        Ok(Self { path })
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Disconnects a /dev/nbdN export on drop.
pub struct NbdGuard {
    dev: String,
}

impl Drop for NbdGuard {
    fn drop(&mut self) {
        let _ = std::process::Command::new("qemu-nbd")
            .args(["--disconnect", &self.dev])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

/// Unmounts and removes the mount dir on drop (the nbd guard disconnects after).
pub struct Mount {
    pub mountpoint: PathBuf,
    _nbd: NbdGuard,
}

impl Drop for Mount {
    fn drop(&mut self) {
        let _ = std::process::Command::new("umount")
            .arg(&self.mountpoint)
            .status();
        let _ = std::fs::remove_dir(&self.mountpoint);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horcrux_common::security::disk_encryption;
    use horcrux_common::VmDisk;

    fn vm(id: &str, hyp: VmHypervisor, status: VmStatus, disk: bool) -> VmConfig {
        VmConfig {
            id: id.into(),
            name: format!("name-{id}"),
            hypervisor: hyp,
            memory: 1024,
            cpus: 1,
            disk_size: 10,
            status,
            architecture: Default::default(),
            disks: if disk {
                vec![VmDisk {
                    path: format!("/vms/{id}.qcow2"),
                    ..Default::default()
                }]
            } else {
                vec![]
            },
            display: Default::default(),
            hardware: Default::default(),
        }
    }

    #[test]
    fn only_qemu_vms_with_a_disk_are_targets() {
        let vms = vec![
            vm("web", VmHypervisor::Qemu, VmStatus::Running, true),
            vm("db", VmHypervisor::Qemu, VmStatus::Stopped, true),
            vm("lxd", VmHypervisor::Lxd, VmStatus::Running, true),
            vm("nodisk", VmHypervisor::Qemu, VmStatus::Stopped, false),
        ];
        let t = vm_targets(&vms);
        assert_eq!(
            t.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(),
            ["db", "web"]
        );
        assert!(t.iter().all(|x| x.kind == ScanTargetKind::Vm));
        assert!(t[1].detail.contains("running"));
        assert!(find_vm(&vms, "web").is_some() && find_vm(&vms, "nope").is_none());
    }

    #[test]
    fn inventory_output_is_parsed_from_the_last_json_line() {
        let out = "some agent log line\n{\"os\":\"linux\",\"distro_id\":\"debian\",\"packages\":[{\"name\":\"bash\",\"version\":\"5.1\",\"type\":\"deb\"}]}\n";
        let inv = parse_inventory_output(out).unwrap();
        assert_eq!(inv.distro_id, "debian");
        assert_eq!(inv.packages.len(), 1);
        assert!(parse_inventory_output("no json here").is_err());
    }

    #[test]
    fn base64_out_data_decodes() {
        // "hello" base64 = aGVsbG8=
        assert_eq!(decode_b64(Some(&json!("aGVsbG8="))), "hello");
        assert_eq!(decode_b64(None), "");
    }

    #[test]
    fn lsblk_is_parsed_and_encryption_detected() {
        let j = r#"{"blockdevices":[{"path":"/dev/nbd0","fstype":null,"type":"disk","children":[
            {"path":"/dev/nbd0p1","fstype":"vfat","type":"part"},
            {"path":"/dev/nbd0p2","fstype":"crypto_LUKS","type":"part"}]}]}"#;
        let parts = parse_lsblk(j);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[2].fstype, "crypto_LUKS");
        let reason = encrypted_reason(&parts).unwrap();
        assert!(reason.contains("LUKS"));
        // a plain disk is not flagged
        let plain = r#"{"blockdevices":[{"path":"/dev/nbd0","fstype":null,"type":"disk","children":[
            {"path":"/dev/nbd0p1","fstype":"ext4","type":"part"}]}]}"#;
        assert!(encrypted_reason(&parse_lsblk(plain)).is_none());
        assert!(is_mountable("ext4") && is_mountable("ntfs") && !is_mountable("crypto_LUKS"));
    }

    #[test]
    fn magic_byte_encryption_detector() {
        // the common detector is reused for a raw partition header read
        assert_eq!(disk_encryption(b"LUKS\xba\xbe\x00\x01"), Some("LUKS"));
    }
}
