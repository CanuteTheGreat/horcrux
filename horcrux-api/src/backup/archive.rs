//! Backup archive format and the external tools that read/write it.
//!
//! An archive is a tar file (optionally compressed with zstd, gzip or lzo)
//! holding:
//! - `horcrux-backup.json`: the [`Manifest`] (guest config + file list with
//!   SHA-256 of every payload file),
//! - VM backups: `disk-N.qcow2`, a standalone (flattened) copy of every VM
//!   disk made with `qemu-img convert`,
//! - container backups: `rootfs.tar` (the container filesystem; for LXC the
//!   whole container directory including its `config`) and, for
//!   Docker/Podman, `inspect.json` (image config needed to recreate it).
use super::{Compression, TargetType};
use horcrux_common::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const MANIFEST_NAME: &str = "horcrux-backup.json";
pub const FORMAT_VERSION: u32 = 1;

/// One payload file in the archive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveFile {
    /// File name inside the archive.
    pub name: String,
    /// What the file is: "disk", "rootfs" or "inspect".
    pub kind: String,
    pub size: u64,
    pub sha256: String,
    /// Disk image format inside the archive ("qcow2"), for disks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// Path of the source the file was made from (informational).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// `horcrux-backup.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub format_version: u32,
    pub backup_id: String,
    pub target_type: TargetType,
    pub target_id: String,
    pub target_name: String,
    pub created_at: i64,
    /// `VmConfig` or `ContainerConfig` of the guest at backup time.
    pub config: serde_json::Value,
    /// Container runtime ("docker", "podman", "lxc", ...), for containers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    pub files: Vec<ArchiveFile>,
}

pub fn file_extension(c: &Compression) -> &'static str {
    match c {
        Compression::None => "tar",
        Compression::Gzip => "tar.gz",
        Compression::Lzo => "tar.lzo",
        Compression::Zstd => "tar.zst",
    }
}

/// (program, compress args, decompress args) for a compression.
fn compressor(
    c: &Compression,
) -> Option<(
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
)> {
    match c {
        Compression::None => None,
        Compression::Gzip => Some(("gzip", &["-c"], &["-d", "-c"])),
        Compression::Lzo => Some(("lzop", &["-c"], &["-d", "-c"])),
        Compression::Zstd => Some(("zstd", &["-q", "-T0", "-c"], &["-q", "-d", "-c"])),
    }
}

fn spawn_err(prog: &str, e: std::io::Error) -> Error {
    if e.kind() == std::io::ErrorKind::NotFound {
        Error::System(format!(
            "'{prog}' is not installed on this host (needed for backups)"
        ))
    } else {
        Error::System(format!("failed to run {prog}: {e}"))
    }
}

fn check_status(prog: &str, out: std::process::Output) -> Result<std::process::Output> {
    if out.status.success() {
        Ok(out)
    } else {
        Err(Error::System(format!(
            "{prog} failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}

/// Run a command to completion (blocking; call from spawn_blocking).
pub fn run(prog: &str, args: &[&str]) -> Result<std::process::Output> {
    let out = Command::new(prog)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| spawn_err(prog, e))?;
    check_status(prog, out)
}

/// Async wrapper for [`run`] with owned arguments.
pub async fn run_async(prog: &'static str, args: Vec<String>) -> Result<std::process::Output> {
    tokio::task::spawn_blocking(move || {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        run(prog, &refs)
    })
    .await
    .map_err(|e| Error::System(format!("task failed: {e}")))?
}

/// Write `files` (relative to `dir`) into a (compressed) tar at `out`.
/// Writes to `<out>.partial` and renames on success, so a crash never
/// leaves a truncated archive under the final name.
pub fn write_archive(dir: &Path, files: &[String], out: &Path, c: &Compression) -> Result<()> {
    let partial = PathBuf::from(format!("{}.partial", out.display()));
    let result = (|| -> Result<()> {
        match compressor(c) {
            None => {
                let mut args: Vec<String> = vec![
                    "-cf".into(),
                    partial.to_string_lossy().into(),
                    "-C".into(),
                    dir.to_string_lossy().into(),
                    "--".into(),
                ];
                args.extend(files.iter().cloned());
                let refs: Vec<&str> = args.iter().map(String::as_str).collect();
                run("tar", &refs)?;
            }
            Some((prog, cargs, _)) => {
                let mut tar = Command::new("tar")
                    .arg("-cf")
                    .arg("-")
                    .arg("-C")
                    .arg(dir)
                    .arg("--")
                    .args(files)
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .map_err(|e| spawn_err("tar", e))?;
                let tar_out = tar.stdout.take().expect("piped stdout");
                let file = std::fs::File::create(&partial)?;
                let comp = Command::new(prog)
                    .args(cargs)
                    .stdin(Stdio::from(tar_out))
                    .stdout(Stdio::from(file))
                    .stderr(Stdio::piped())
                    .spawn();
                let comp = match comp {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = tar.kill();
                        let _ = tar.wait();
                        return Err(spawn_err(prog, e));
                    }
                };
                let comp_out = comp.wait_with_output()?;
                let tar_out = tar.wait_with_output()?;
                check_status("tar", tar_out)?;
                check_status(prog, comp_out)?;
            }
        }
        std::fs::rename(&partial, out)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result
}

/// Extract the whole archive into `dir` (which must exist).
pub fn extract_archive(archive: &Path, dir: &Path, c: &Compression) -> Result<()> {
    match compressor(c) {
        None => {
            run(
                "tar",
                &[
                    "-xf",
                    &archive.to_string_lossy(),
                    "-C",
                    &dir.to_string_lossy(),
                ],
            )?;
        }
        Some((prog, _, dargs)) => {
            let input = std::fs::File::open(archive)?;
            let mut dec = Command::new(prog)
                .args(dargs)
                .stdin(Stdio::from(input))
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| spawn_err(prog, e))?;
            let dec_out = dec.stdout.take().expect("piped stdout");
            let tar = Command::new("tar")
                .arg("-xf")
                .arg("-")
                .arg("-C")
                .arg(dir)
                .stdin(Stdio::from(dec_out))
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn();
            let tar = match tar {
                Ok(t) => t,
                Err(e) => {
                    let _ = dec.kill();
                    let _ = dec.wait();
                    return Err(spawn_err("tar", e));
                }
            };
            let tar_out = tar.wait_with_output()?;
            let dec_out = dec.wait_with_output()?;
            check_status(prog, dec_out)?;
            check_status("tar", tar_out)?;
        }
    }
    Ok(())
}

/// SHA-256 (hex) and size of a file (blocking).
pub fn sha256_file(path: &Path) -> Result<(String, u64)> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = std::fs::File::open(path)
        .map_err(|e| Error::System(format!("cannot open {}: {e}", path.display())))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        h.update(&buf[..n]);
    }
    Ok((hex::encode(h.finalize()), total))
}

pub async fn sha256_file_async(path: PathBuf) -> Result<(String, u64)> {
    tokio::task::spawn_blocking(move || sha256_file(&path))
        .await
        .map_err(|e| Error::System(format!("task failed: {e}")))?
}

/// Image format of a disk file per `qemu-img info`.
pub fn disk_format(path: &Path, force_share: bool) -> Result<String> {
    let p = path.to_string_lossy();
    let mut args = vec!["info", "--output=json"];
    if force_share {
        args.push("-U");
    }
    args.push(&p);
    let out = run("qemu-img", &args)?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| Error::System(format!("qemu-img info output: {e}")))?;
    Ok(v["format"].as_str().unwrap_or("raw").to_string())
}

/// Standalone qcow2 copy of a VM disk. The VM must be stopped, or paused
/// (`force_share`: QEMU still holds the image lock but has flushed it).
pub fn export_disk(src: &Path, dst: &Path, force_share: bool) -> Result<()> {
    if !src.exists() {
        return Err(Error::System(format!(
            "VM disk {} does not exist",
            src.display()
        )));
    }
    let fmt = disk_format(src, force_share)?;
    let (src_s, dst_s) = (src.to_string_lossy(), dst.to_string_lossy());
    let mut args = vec!["convert"];
    if force_share {
        args.push("-U");
    }
    args.extend(["-f", &fmt, "-O", "qcow2", &src_s, &dst_s]);
    run("qemu-img", &args)?;
    Ok(())
}

/// `qemu-img check` a qcow2 image. Leaked clusters (exit 3) are harmless
/// and accepted; corruption (exit 2) or an unreadable image is an error.
pub fn check_disk(path: &Path) -> Result<()> {
    let out = Command::new("qemu-img")
        .args(["check", "-f", "qcow2"])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| spawn_err("qemu-img", e))?;
    match out.status.code() {
        Some(0) | Some(3) => Ok(()),
        _ => Err(Error::System(format!(
            "qemu-img check {}: {}{}",
            path.display(),
            String::from_utf8_lossy(&out.stdout).trim(),
            String::from_utf8_lossy(&out.stderr).trim()
        ))),
    }
}

/// Read and validate the manifest in an extracted archive directory.
pub fn read_manifest(dir: &Path) -> Result<Manifest> {
    let raw = std::fs::read(dir.join(MANIFEST_NAME))
        .map_err(|e| Error::System(format!("archive has no {MANIFEST_NAME}: {e}")))?;
    let m: Manifest = serde_json::from_slice(&raw)
        .map_err(|e| Error::System(format!("invalid {MANIFEST_NAME}: {e}")))?;
    if m.format_version > FORMAT_VERSION {
        return Err(Error::System(format!(
            "backup format version {} is newer than this Horcrux supports ({FORMAT_VERSION})",
            m.format_version
        )));
    }
    for f in &m.files {
        if f.name.contains('/') || f.name.starts_with('.') {
            return Err(Error::System(format!(
                "invalid file name '{}' in manifest",
                f.name
            )));
        }
    }
    Ok(m)
}

/// Check every payload file in an extracted archive against the manifest.
pub fn verify_extracted(dir: &Path, m: &Manifest) -> Result<()> {
    for f in &m.files {
        let p = dir.join(&f.name);
        let (sum, size) = sha256_file(&p)?;
        if sum != f.sha256 || size != f.size {
            return Err(Error::System(format!(
                "{} checksum mismatch (expected {} / {} bytes, got {} / {} bytes)",
                f.name, f.sha256, f.size, sum, size
            )));
        }
        if f.kind == "disk" {
            check_disk(&p)?;
        }
    }
    Ok(())
}
