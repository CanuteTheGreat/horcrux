//! Reference Horcrux storage plugin: a directory on an SSH server, mounted
//! with SSHFS, holding qcow2 (or raw) volumes.
//!
//! Installed as `<plugin dir>/sshfs` (default
//! /usr/libexec/horcrux/storage-plugins/sshfs). Needs `sshfs` (FUSE) and
//! `qemu-img` on the node. Volumes are `<mount>/<name>.qcow2`; snapshots are
//! qcow2 internal snapshots (`qemu-img snapshot`). Every Horcrux node that
//! uses the pool mounts the same remote directory, so the pool is shared.
//!
//! This file is also the worked example of docs/storage-plugins.md: the
//! schema below is all the UI the plugin has.
use horcrux_storage_plugin_sdk::{
    serve, ActivateResult, Capabilities, Field, FieldType, PathResult, PluginDescription,
    PluginError, PluginResult, PoolContext, Snapshot, StatusResult, StoragePlugin, Volume,
    VolumeArgs, API_VERSION,
};
use serde_json::json;
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

struct Sshfs;

fn schema() -> Vec<Field> {
    vec![
        Field::new("host", FieldType::String, "SSH server")
            .required()
            .pattern(r"[A-Za-z0-9][A-Za-z0-9.:\-]*")
            .range(1, 253)
            .placeholder("nas.example.org")
            .hint("Host name or IP address of the SSH server that holds the volumes."),
        Field::new("port", FieldType::Int, "Port")
            .default_value(json!(22))
            .range(1, 65535)
            .hint("TCP port of the SSH server (22 unless the server was changed)."),
        Field::new("user", FieldType::String, "User")
            .default_value(json!("root"))
            .pattern(r"[A-Za-z_][A-Za-z0-9._\-]*")
            .range(1, 32)
            .hint("Account on the SSH server; it must be able to write the remote directory."),
        Field::new("remote_dir", FieldType::Path, "Remote directory")
            .required()
            .placeholder("/srv/horcrux")
            .hint("Absolute directory on the server where volume files are kept."),
        Field::new("auth", FieldType::Enum, "Login with")
            .options(&[("key", "SSH key"), ("password", "Password")])
            .default_value(json!("key"))
            .hint("An SSH key needs no stored password; the key must be on every node."),
        Field::new("identity_file", FieldType::Path, "Private key file")
            .depends_on("auth", Some(json!("key")))
            .placeholder("/root/.ssh/id_ed25519")
            .hint("Key file on this node. Leave empty to use the node's default SSH keys."),
        Field::new("password", FieldType::Secret, "Password")
            .required()
            .depends_on("auth", Some(json!("password")))
            .hint("Kept in the node's secrets store and never shown again."),
        Field::new(
            "connect_timeout",
            FieldType::Int,
            "Connect timeout (seconds)",
        )
        .default_value(json!(10))
        .range(1, 300)
        .hint("How long to wait for the server before the mount fails."),
        Field::new("host_key_policy", FieldType::Enum, "Host key check")
            .options(&[
                ("accept-new", "Trust on first use"),
                ("strict", "Known hosts only"),
            ])
            .default_value(json!("accept-new"))
            .hint(
                "Trust on first use records the server's key at the first mount and refuses a \
                 changed key later. Known hosts only needs the key in the node's known_hosts.",
            ),
        Field::new("compression", FieldType::Bool, "Compression")
            .default_value(json!(false))
            .hint("Compress traffic; helps on slow links, costs CPU on fast ones."),
    ]
}

fn mount_dir(pool: &PoolContext) -> PathBuf {
    Path::new(&pool.state_dir).join("mnt")
}

/// Whether `dir` is a mount point (from /proc/self/mounts).
fn is_mounted(dir: &Path) -> bool {
    let want = dir.to_string_lossy().replace(' ', "\\040");
    std::fs::read_to_string("/proc/self/mounts")
        .map(|m| {
            m.lines()
                .any(|l| l.split(' ').nth(1) == Some(want.as_str()))
        })
        .unwrap_or(false)
}

fn tool_missing(tool: &str, pkg: &str) -> PluginError {
    PluginError::unavailable(format!("{tool} is not installed on this node"))
        .with_hint(format!("install {pkg} (it provides {tool})"))
}

fn run(cmd: &mut Command, tool: &str, pkg: &str) -> PluginResult<String> {
    let out = cmd.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            tool_missing(tool, pkg)
        } else {
            PluginError::failed(format!("running {tool}: {e}"))
        }
    })?;
    if !out.status.success() {
        return Err(PluginError::failed(format!(
            "{tool} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn qemu_img(args: &[&str]) -> PluginResult<String> {
    run(
        Command::new("qemu-img").args(args),
        "qemu-img",
        "app-emulation/qemu",
    )
}

/// The file of volume `name` (qcow2 first, then raw).
fn volume_file(pool: &PoolContext, name: &str) -> Option<(PathBuf, &'static str)> {
    let dir = mount_dir(pool);
    for (ext, fmt) in [("qcow2", "qcow2"), ("raw", "raw")] {
        let p = dir.join(format!("{name}.{ext}"));
        if p.is_file() {
            return Some((p, fmt));
        }
    }
    None
}

fn need_mounted(pool: &PoolContext) -> PluginResult<PathBuf> {
    let d = mount_dir(pool);
    if !is_mounted(&d) {
        return Err(PluginError::unavailable(format!(
            "the SSHFS mount of pool {} is not active",
            pool.name
        ))
        .with_hint(
            "check that the SSH server is reachable; the manager mounts it again at start",
        ));
    }
    Ok(d)
}

fn virtual_size(path: &Path, fmt: &str) -> u64 {
    if fmt == "qcow2" {
        use std::io::Read;
        let mut hdr = [0u8; 32];
        if std::fs::File::open(path)
            .and_then(|mut f| f.read_exact(&mut hdr))
            .is_ok()
            && hdr[..4] == *b"QFI\xfb"
        {
            return u64::from_be_bytes(hdr[24..32].try_into().unwrap_or([0; 8]));
        }
    }
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn volume_info(name: &str, path: &Path, fmt: &str) -> Volume {
    let allocated = std::fs::metadata(path)
        .map(|m| m.blocks() * 512)
        .unwrap_or(0);
    Volume {
        name: name.to_string(),
        path: path.to_string_lossy().to_string(),
        format: fmt.to_string(),
        size_bytes: virtual_size(path, fmt),
        allocated_bytes: allocated,
    }
}

fn find_volume(pool: &PoolContext, name: &str) -> PluginResult<(PathBuf, &'static str)> {
    need_mounted(pool)?;
    volume_file(pool, name)
        .ok_or_else(|| PluginError::not_found(format!("volume {name} not found in {}", pool.name)))
}

fn need_qcow2(fmt: &str) -> PluginResult<()> {
    if fmt != "qcow2" {
        return Err(PluginError::invalid(
            "snapshots need a qcow2 volume (this one is raw)",
        ));
    }
    Ok(())
}

impl StoragePlugin for Sshfs {
    fn describe(&self) -> PluginDescription {
        PluginDescription {
            id: "sshfs".into(),
            name: "SSHFS".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            description: "A directory on an SSH server, mounted with SSHFS. Volumes are qcow2 \
                          files with internal snapshots; every node mounts the same directory."
                .into(),
            schema: schema(),
            capabilities: Capabilities {
                content: vec!["images".into()],
                snapshots: true,
                clone: false,
                resize: true,
                shared: true,
                vm_disk_dir: true,
                formats: vec!["qcow2".into(), "raw".into()],
            },
        }
    }

    fn activate(&self, pool: &PoolContext) -> PluginResult<ActivateResult> {
        let dir = mount_dir(pool);
        let local_dir = Some(dir.to_string_lossy().to_string());
        if is_mounted(&dir) {
            return Ok(ActivateResult { local_dir });
        }
        std::fs::create_dir_all(&dir)
            .map_err(|e| PluginError::failed(format!("creating {}: {e}", dir.display())))?;
        let host = pool.str("host").unwrap_or_default();
        let user = pool.str("user").unwrap_or("root");
        let remote = pool.str("remote_dir").unwrap_or("/");
        let port = pool.int("port").unwrap_or(22);
        let known = Path::new(&pool.state_dir).join("known_hosts");
        let policy = match pool.str("host_key_policy") {
            Some("strict") => "yes",
            _ => "accept-new",
        };
        let mut opts = vec![
            "reconnect".to_string(),
            "allow_other".to_string(),
            "ServerAliveInterval=15".to_string(),
            "ServerAliveCountMax=3".to_string(),
            format!("StrictHostKeyChecking={policy}"),
            format!(
                "ConnectTimeout={}",
                pool.int("connect_timeout").unwrap_or(10)
            ),
        ];
        if policy == "accept-new" {
            opts.push(format!("UserKnownHostsFile={}", known.display()));
        }
        if pool.bool("compression") == Some(true) {
            opts.push("compression=yes".into());
        }
        let password = pool.str("auth") == Some("password");
        if password {
            opts.push("password_stdin".into());
        } else {
            opts.push("BatchMode=yes".into());
            if let Some(k) = pool.str("identity_file") {
                opts.push(format!("IdentityFile={k}"));
            }
        }
        let target = if host.contains(':') {
            format!("{user}@[{host}]:{remote}")
        } else {
            format!("{user}@{host}:{remote}")
        };
        let mut cmd = Command::new("sshfs");
        cmd.arg(&target)
            .arg(&dir)
            .arg("-p")
            .arg(port.to_string())
            .arg("-o")
            .arg(opts.join(","))
            .stdin(Stdio::piped())
            .stdout(Stdio::null());
        // stderr goes to a file, not a pipe: the ssh child sshfs starts keeps
        // its stderr open for the life of the mount, so a pipe never ends.
        let log = Path::new(&pool.state_dir).join("sshfs.log");
        let logf = std::fs::File::create(&log)
            .map_err(|e| PluginError::failed(format!("creating {}: {e}", log.display())))?;
        cmd.stderr(Stdio::from(logf));
        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                tool_missing("sshfs", "net-fs/sshfs")
            } else {
                PluginError::failed(format!("running sshfs: {e}"))
            }
        })?;
        if let Some(mut stdin) = child.stdin.take() {
            if password {
                let _ = writeln!(stdin, "{}", pool.secret("password").unwrap_or_default());
            }
        }
        let status = child
            .wait()
            .map_err(|e| PluginError::failed(format!("waiting for sshfs: {e}")))?;
        if !status.success() || !is_mounted(&dir) {
            let err = std::fs::read_to_string(&log)
                .unwrap_or_default()
                .trim()
                .to_string();
            let hint = if err.contains("Host key verification failed")
                || err.contains("REMOTE HOST IDENTIFICATION HAS CHANGED")
            {
                "the server's host key is unknown or changed; check the server, then fix known_hosts"
            } else if err.contains("Permission denied") {
                "the server refused the login: check user, key or password"
            } else if err.contains("fuse: device not found") || err.contains("/dev/fuse") {
                "the node has no FUSE device: load the fuse kernel module"
            } else {
                "check that the server is reachable and the remote directory exists"
            };
            return Err(
                PluginError::unavailable(format!("mounting {target} failed: {err}"))
                    .with_hint(hint),
            );
        }
        Ok(ActivateResult { local_dir })
    }

    fn deactivate(&self, pool: &PoolContext) -> PluginResult<()> {
        let dir = mount_dir(pool);
        if !is_mounted(&dir) {
            return Ok(());
        }
        let r = run(
            Command::new("fusermount3").arg("-u").arg(&dir),
            "fusermount3",
            "sys-fs/fuse",
        );
        if r.is_err() && is_mounted(&dir) {
            run(
                Command::new("umount").arg(&dir),
                "umount",
                "sys-apps/util-linux",
            )?;
        }
        let _ = std::fs::remove_dir(&dir);
        Ok(())
    }

    fn status(&self, pool: &PoolContext) -> PluginResult<StatusResult> {
        let dir = mount_dir(pool);
        if !is_mounted(&dir) {
            return Ok(StatusResult {
                active: false,
                message: Some("not mounted".into()),
                ..Default::default()
            });
        }
        // block size, total blocks, available blocks
        let out = run(
            Command::new("stat")
                .args(["-f", "-c", "%S %b %a"])
                .arg(&dir),
            "stat",
            "sys-apps/coreutils",
        )?;
        let n: Vec<u64> = out
            .split_whitespace()
            .filter_map(|x| x.parse().ok())
            .collect();
        let (bs, total, avail) = match n.as_slice() {
            [a, b, c] => (*a, *b, *c),
            _ => {
                return Err(PluginError::failed(format!(
                    "unexpected stat output {out:?}"
                )))
            }
        };
        Ok(StatusResult {
            active: true,
            total_bytes: bs.saturating_mul(total),
            available_bytes: bs.saturating_mul(avail),
            message: None,
        })
    }

    fn list_volumes(&self, pool: &PoolContext) -> PluginResult<Vec<Volume>> {
        let dir = need_mounted(pool)?;
        let rd = std::fs::read_dir(&dir)
            .map_err(|e| PluginError::failed(format!("reading {}: {e}", dir.display())))?;
        let mut out = Vec::new();
        for e in rd.flatten() {
            let f = e.file_name().to_string_lossy().to_string();
            let (name, fmt) = if let Some(n) = f.strip_suffix(".qcow2") {
                (n.to_string(), "qcow2")
            } else if let Some(n) = f.strip_suffix(".raw") {
                (n.to_string(), "raw")
            } else {
                continue;
            };
            if !horcrux_storage_plugin_sdk::valid_volume_name(&name) || !e.path().is_file() {
                continue;
            }
            out.push(volume_info(&name, &e.path(), fmt));
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    fn create_volume(&self, pool: &PoolContext, a: &VolumeArgs) -> PluginResult<Volume> {
        let dir = need_mounted(pool)?;
        if volume_file(pool, &a.name).is_some() {
            return Err(PluginError::exists(format!(
                "volume {} already exists in {}",
                a.name, pool.name
            )));
        }
        let fmt = match a.format.as_deref() {
            None | Some("") | Some("qcow2") => "qcow2",
            Some("raw") => "raw",
            Some(f) => return Err(PluginError::invalid(format!("format {f} is not supported"))),
        };
        let path = dir.join(format!("{}.{fmt}", a.name));
        let size = a.size_bytes.unwrap_or(0).to_string();
        qemu_img(&["create", "-q", "-f", fmt, &path.to_string_lossy(), &size])?;
        Ok(volume_info(&a.name, &path, fmt))
    }

    fn delete_volume(&self, pool: &PoolContext, a: &VolumeArgs) -> PluginResult<()> {
        let (path, _) = find_volume(pool, &a.name)?;
        std::fs::remove_file(&path)
            .map_err(|e| PluginError::failed(format!("removing {}: {e}", path.display())))
    }

    fn resize_volume(&self, pool: &PoolContext, a: &VolumeArgs) -> PluginResult<Volume> {
        let (path, fmt) = find_volume(pool, &a.name)?;
        let want = a.size_bytes.unwrap_or(0);
        if want < virtual_size(&path, fmt) {
            return Err(PluginError::invalid("shrinking a volume is not supported"));
        }
        qemu_img(&[
            "resize",
            "-q",
            "-f",
            fmt,
            &path.to_string_lossy(),
            &want.to_string(),
        ])?;
        Ok(volume_info(&a.name, &path, fmt))
    }

    fn list_snapshots(&self, pool: &PoolContext, a: &VolumeArgs) -> PluginResult<Vec<Snapshot>> {
        let (path, fmt) = find_volume(pool, &a.name)?;
        if fmt != "qcow2" {
            return Ok(Vec::new());
        }
        let out = qemu_img(&[
            "info",
            "--output=json",
            "-f",
            "qcow2",
            "-U",
            &path.to_string_lossy(),
        ])?;
        let v: serde_json::Value = serde_json::from_str(&out)
            .map_err(|e| PluginError::failed(format!("qemu-img info output: {e}")))?;
        Ok(v["snapshots"]
            .as_array()
            .map(|s| {
                s.iter()
                    .filter_map(|x| {
                        Some(Snapshot {
                            name: x["name"].as_str()?.to_string(),
                            created: x["date-sec"].as_i64(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    fn create_snapshot(&self, pool: &PoolContext, a: &VolumeArgs) -> PluginResult<()> {
        let (path, fmt) = find_volume(pool, &a.name)?;
        need_qcow2(fmt)?;
        let snap = a.snapshot.as_deref().unwrap_or_default();
        if self.list_snapshots(pool, a)?.iter().any(|s| s.name == snap) {
            return Err(PluginError::exists(format!(
                "snapshot {snap} of {} already exists",
                a.name
            )));
        }
        qemu_img(&["snapshot", "-c", snap, &path.to_string_lossy()]).map(|_| ())
    }

    fn delete_snapshot(&self, pool: &PoolContext, a: &VolumeArgs) -> PluginResult<()> {
        let (path, fmt) = find_volume(pool, &a.name)?;
        need_qcow2(fmt)?;
        let snap = a.snapshot.as_deref().unwrap_or_default();
        if !self.list_snapshots(pool, a)?.iter().any(|s| s.name == snap) {
            return Err(PluginError::not_found(format!(
                "volume {} has no snapshot {snap}",
                a.name
            )));
        }
        qemu_img(&["snapshot", "-d", snap, &path.to_string_lossy()]).map(|_| ())
    }

    fn rollback_snapshot(&self, pool: &PoolContext, a: &VolumeArgs) -> PluginResult<()> {
        let (path, fmt) = find_volume(pool, &a.name)?;
        need_qcow2(fmt)?;
        let snap = a.snapshot.as_deref().unwrap_or_default();
        if !self.list_snapshots(pool, a)?.iter().any(|s| s.name == snap) {
            return Err(PluginError::not_found(format!(
                "volume {} has no snapshot {snap}",
                a.name
            )));
        }
        qemu_img(&["snapshot", "-a", snap, &path.to_string_lossy()]).map(|_| ())
    }

    fn volume_path(&self, pool: &PoolContext, a: &VolumeArgs) -> PluginResult<PathResult> {
        let (path, fmt) = find_volume(pool, &a.name)?;
        Ok(PathResult {
            path: path.to_string_lossy().to_string(),
            format: fmt.to_string(),
        })
    }
}

fn main() {
    std::process::exit(serve(Sshfs));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_valid() {
        horcrux_storage_plugin_sdk::check_schema(&schema()).unwrap();
        assert_eq!(Sshfs.describe().id, "sshfs");
    }

    #[test]
    fn password_is_required_only_for_password_logins() {
        let s = schema();
        let base = json!({"host": "nas", "remote_dir": "/srv/hx"});
        let cfg: std::collections::BTreeMap<String, serde_json::Value> =
            serde_json::from_value(base.clone()).unwrap();
        let v = horcrux_storage_plugin_sdk::validate_config(&s, &cfg).unwrap();
        assert_eq!(v.config["port"], json!(22));
        let mut cfg = cfg;
        cfg.insert("auth".into(), json!("password"));
        let e = horcrux_storage_plugin_sdk::validate_config(&s, &cfg).unwrap_err();
        assert_eq!(e[0].key, "password");
    }

    #[test]
    fn unmounted_pool_is_reported_not_active() {
        let d = std::env::temp_dir().join(format!("hx-sshfs-test-{}", std::process::id()));
        let pool = PoolContext {
            id: "p".into(),
            name: "p".into(),
            state_dir: d.to_string_lossy().to_string(),
            ..Default::default()
        };
        let st = Sshfs.status(&pool).unwrap();
        assert!(!st.active);
        let e = Sshfs.list_volumes(&pool).unwrap_err();
        assert_eq!(e.code, horcrux_storage_plugin_sdk::ErrorCode::Unavailable);
    }
}
