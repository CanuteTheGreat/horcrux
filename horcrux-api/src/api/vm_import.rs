//! Import a disk image into a VM (Proxmox `qm disk import` / `import-from`).
//!
//! `POST /api/vms/:id/import-disk {"source": "<http(s) URL or path>"}`
//! replaces the (stopped) VM's first disk with the given cloud image or
//! disk image, converted to qcow2 and grown to at least the VM's disk size.
//! This is how a VM gets an operating system from a cloud image (CirrOS,
//! Debian/Ubuntu/Alpine genericcloud qcow2, ...).
//!
//! Safety:
//! - local sources must live under `<data_dir>/images` or a storage pool's
//!   directory, so the API can't be used to read arbitrary host files;
//! - the source format is probed once with `qemu-img info` and then passed
//!   explicitly (`-f`), and images that would make qemu-img read another
//!   file are refused: a backing file, a qcow2 external data file, or VMDK
//!   extents other than the image itself (a crafted image could otherwise
//!   copy a host file into the VM's disk);
//! - URL sources may not point at the node itself or at link-local
//!   addresses (cloud metadata at 169.254.169.254, `fd00:ec2::254`): the
//!   check covers literal IPs, every address a host name resolves to (the
//!   connection only uses checked addresses, so DNS rebinding can't slip
//!   past it) and every redirect hop, so a VM-modify user can't make the node
//!   fetch its own localhost-only services or instance credentials into a VM
//!   disk. LAN mirrors (RFC 1918 / ULA addresses) stay allowed;
//! - the conversion writes a temporary file next to the disk and renames it
//!   over the old disk only when complete, so a failure (disk full, bad
//!   image, server killed mid-import) leaves the VM's old disk intact and
//!   no temporary file behind.
use crate::error::ApiError;
use crate::AppState;
use axum::{
    extract::{Path, State},
    routing::post,
    Json, Router,
};
use horcrux_common::{VmConfig, VmHypervisor};
use serde::Deserialize;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use tokio::process::Command;

const FORMATS: &[&str] = &["qcow2", "raw", "vmdk", "vdi", "vhdx", "vpc", "qed"];

/// Disk import route. Also, once at startup, undoes guest holds left by an
/// operation the API was killed in the middle of (see `vm::hold`).
pub fn routes(state: &Arc<AppState>) -> Router<Arc<AppState>> {
    let vms = state.vm_manager.clone();
    tokio::spawn(async move {
        let recovered = crate::vm::hold::recover(&vms).await;
        if !recovered.is_empty() {
            tracing::warn!("Recovered guests after an interrupted operation: {recovered:?}");
        }
    });
    Router::new().route("/api/vms/:id/import-disk", post(import_disk))
}

#[derive(Deserialize)]
pub struct ImportDiskRequest {
    pub source: String,
}

/// Directory local import sources may come from (besides storage pools):
/// `HORCRUX_IMAGE_DIR`, default `<data_dir>/images`.
pub fn image_dir(data_dir: &FsPath) -> PathBuf {
    std::env::var("HORCRUX_IMAGE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| data_dir.join("images"))
}

/// Whether `source` (canonical) lies inside one of `roots`.
pub fn allowed_local_source(source: &FsPath, roots: &[PathBuf]) -> bool {
    roots
        .iter()
        .filter_map(|r| std::fs::canonicalize(r).ok())
        .any(|r| source.starts_with(&r) && source != r)
}

/// Format of a probed image, refusing unknown formats and every way an image
/// can make qemu-img read a second file: a backing file, a qcow2 external
/// data file (`data_file`), or a VMDK descriptor whose extents are other
/// files (`RW 2048 FLAT "/etc/shadow" 0`). Without the last two checks an
/// upload or URL of a few hundred bytes would copy any host file qemu-img can
/// open into the VM's disk.
pub fn checked_format(info: &serde_json::Value) -> Result<String, String> {
    if info.get("backing-filename").is_some() || info.get("full-backing-filename").is_some() {
        return Err("images with a backing file can't be imported".into());
    }
    let fmt = info["format"].as_str().unwrap_or_default();
    if !FORMATS.contains(&fmt) {
        return Err(format!("unsupported image format {fmt:?}"));
    }
    let specific = &info["format-specific"]["data"];
    if specific.get("data-file").is_some() {
        return Err("images with an external data file can't be imported".into());
    }
    if fmt == "vmdk" {
        // A single-file VMDK (monolithicSparse, streamOptimized) has exactly
        // one extent: the image itself.
        let own = info["filename"].as_str();
        let extents = specific["extents"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        if own.is_none()
            || extents.is_empty()
            || extents.iter().any(|e| e["filename"].as_str() != own)
        {
            return Err("only single-file VMDK images can be imported".into());
        }
    }
    Ok(fmt.to_string())
}

async fn qemu_img(args: &[&str]) -> Result<String, ApiError> {
    let out = Command::new("qemu-img")
        .args(args)
        .output()
        .await
        .map_err(|e| ApiError::Internal(format!("qemu-img: {e}")))?;
    if !out.status.success() {
        return Err(ApiError::Internal(format!(
            "qemu-img {}: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Removes the temporary files on every exit path.
struct TempFiles(Vec<PathBuf>);
impl Drop for TempFiles {
    fn drop(&mut self) {
        for p in &self.0 {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Why a URL source may not be fetched from `ip`, if it may not: the node
/// itself (loopback, `0.0.0.0/8`, unspecified), link-local (cloud metadata
/// lives at 169.254.169.254), AWS's IPv6 metadata address, multicast and
/// broadcast. IPv4-mapped and NAT64 (`64:ff9b::/96`) forms are judged by the
/// IPv4 address they carry.
pub fn refused_ip(ip: std::net::IpAddr) -> Option<&'static str> {
    const AWS_METADATA_V6: std::net::Ipv6Addr =
        std::net::Ipv6Addr::new(0xfd00, 0xec2, 0, 0, 0, 0, 0, 0x254);
    use std::net::{IpAddr, Ipv4Addr};
    let v4 = |a: Ipv4Addr| {
        if a.is_loopback() || a.octets()[0] == 0 {
            Some("the node itself")
        } else if a.is_link_local() {
            Some("a link-local (cloud metadata) address")
        } else if a.is_multicast() || a.is_broadcast() {
            Some("a multicast/broadcast address")
        } else {
            None
        }
    };
    match ip {
        IpAddr::V4(a) => v4(a),
        IpAddr::V6(a) => {
            if let Some(m) = a.to_ipv4_mapped() {
                return v4(m);
            }
            let seg = a.segments();
            if seg[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
                return v4(Ipv4Addr::new(
                    (seg[6] >> 8) as u8,
                    seg[6] as u8,
                    (seg[7] >> 8) as u8,
                    seg[7] as u8,
                ));
            }
            if a.is_loopback() || a.is_unspecified() {
                Some("the node itself")
            } else if seg[0] & 0xffc0 == 0xfe80 || a == AWS_METADATA_V6 {
                Some("a link-local (cloud metadata) address")
            } else if a.is_multicast() {
                Some("a multicast/broadcast address")
            } else {
                None
            }
        }
    }
}

/// Why `url` may not be fetched, if it may not: only http(s), and a literal
/// IP host must pass [`refused_ip`]. Host names are checked when they are
/// resolved ([`GuardedResolver`]).
pub fn refused_url(url: &reqwest::Url) -> Option<String> {
    if !matches!(url.scheme(), "http" | "https") {
        return Some(format!("{url}: only http(s) sources can be imported"));
    }
    let ip = match url.host() {
        Some(url::Host::Ipv4(a)) => std::net::IpAddr::V4(a),
        Some(url::Host::Ipv6(a)) => std::net::IpAddr::V6(a),
        Some(url::Host::Domain(_)) => return None,
        None => return Some(format!("{url}: no host")),
    };
    refused_ip(ip).map(|why| format!("{url}: refusing to download from {why}"))
}

/// Resolves host names like the system resolver but hands reqwest only
/// addresses that pass [`refused_ip`], so the address checked is the
/// address connected to.
struct GuardedResolver;
impl reqwest::dns::Resolve for GuardedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(resolve_guarded(name.as_str().to_string()))
    }
}

async fn resolve_guarded(
    host: String,
) -> Result<reqwest::dns::Addrs, Box<dyn std::error::Error + Send + Sync>> {
    let all: Vec<std::net::SocketAddr> =
        tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
    let ok: Vec<std::net::SocketAddr> = all
        .iter()
        .copied()
        .filter(|a| refused_ip(a.ip()).is_none())
        .collect();
    if ok.is_empty() {
        let why = all
            .first()
            .and_then(|a| refused_ip(a.ip()))
            .unwrap_or("no address");
        return Err(format!("{host} resolves to {why}; refusing to download").into());
    }
    Ok(Box::new(ok.into_iter()))
}

/// `e` and its causes, so a refused redirect or resolution says why.
pub(crate) fn error_chain(e: &dyn std::error::Error) -> String {
    let mut msg = e.to_string();
    let mut src = e.source();
    while let Some(s) = src {
        msg.push_str(": ");
        msg.push_str(&s.to_string());
        src = s.source();
    }
    msg
}

/// HTTP client for URL sources: guarded resolution, and every redirect hop
/// re-checked with [`refused_url`] (at most 10 hops). Shared with the ISO
/// library's URL downloads (`vm_hw`).
///
/// System proxies (`HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY`) are ignored: a
/// proxied request hands the host *name* to the proxy, which resolves and
/// connects on its own, so [`GuardedResolver`] would never see it and a
/// name for the node's loopback services or the cloud metadata address
/// would be fetched by the proxy. Pre-checking the name here wouldn't
/// help either - the proxy resolves it again and may get another answer.
/// Downloads always connect directly from the node.
pub(crate) fn download_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .no_proxy()
        .dns_resolver(Arc::new(GuardedResolver))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 10 {
                attempt.error("too many redirects")
            } else if let Some(why) = refused_url(attempt.url()) {
                attempt.error(why)
            } else {
                attempt.follow()
            }
        }))
        .build()
}

async fn download(url: &str, to: &FsPath) -> Result<(), ApiError> {
    use tokio::io::AsyncWriteExt;
    let parsed = reqwest::Url::parse(url)
        .map_err(|e| ApiError::BadRequest(format!("download {url}: {e}")))?;
    if let Some(why) = refused_url(&parsed) {
        return Err(ApiError::Forbidden(why));
    }
    let client = download_client().map_err(|e| ApiError::Internal(format!("http client: {e}")))?;
    let mut resp = client
        .get(parsed)
        .send()
        .await
        .map_err(|e| ApiError::BadRequest(format!("download {url}: {}", error_chain(&e))))?;
    if !resp.status().is_success() {
        return Err(ApiError::BadRequest(format!(
            "download {url}: HTTP {}",
            resp.status()
        )));
    }
    let mut f = tokio::fs::File::create(to).await?;
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| ApiError::BadRequest(format!("download {url}: {e}")))?
    {
        f.write_all(&chunk).await?;
    }
    f.flush().await?;
    f.sync_all().await?;
    Ok(())
}

async fn import_disk(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<ImportDiskRequest>,
) -> Result<Json<VmConfig>, ApiError> {
    let vm = state.vm_manager.get_vm(&id).await?;
    if vm.hypervisor != VmHypervisor::Qemu {
        return Err(ApiError::BadRequest(
            "disk import is for QEMU VMs".to_string(),
        ));
    }
    if crate::vm::qemu::QemuManager::process_running(&vm.id) {
        return Err(ApiError::Conflict(format!(
            "VM {id} is running; stop it before importing a disk"
        )));
    }
    let disk = PathBuf::from(
        &vm.disks
            .first()
            .ok_or_else(|| ApiError::BadRequest(format!("VM {id} has no disk")))?
            .path,
    );
    let dir = disk
        .parent()
        .ok_or_else(|| ApiError::Internal("disk has no directory".into()))?;
    let tmp_src = dir.join(format!(".{id}.import-src.tmp"));
    let tmp_out = dir.join(format!(".{id}.import.qcow2.tmp"));
    let _cleanup = TempFiles(vec![tmp_src.clone(), tmp_out.clone()]);

    let source: PathBuf = if req.source.starts_with("http://") || req.source.starts_with("https://")
    {
        download(&req.source, &tmp_src).await?;
        tmp_src.clone()
    } else {
        let p = std::fs::canonicalize(&req.source)
            .map_err(|e| ApiError::BadRequest(format!("source {}: {e}", req.source)))?;
        let mut roots = vec![image_dir(&state.config.paths.data_dir)];
        roots.extend(
            state
                .storage_manager
                .list_pools()
                .await
                .into_iter()
                .filter(|p| p.path.starts_with('/'))
                .map(|p| PathBuf::from(p.path)),
        );
        if !allowed_local_source(&p, &roots) || !p.is_file() {
            return Err(ApiError::Forbidden(format!(
                "local import sources must be files under {} or a storage pool directory",
                image_dir(&state.config.paths.data_dir).display()
            )));
        }
        p
    };

    let src = source.to_string_lossy().to_string();
    let info: serde_json::Value =
        serde_json::from_str(&qemu_img(&["info", "--output=json", &src]).await?)
            .map_err(|e| ApiError::Internal(format!("qemu-img info: {e}")))?;
    let fmt = checked_format(&info).map_err(ApiError::BadRequest)?;
    let out = tmp_out.to_string_lossy().to_string();
    qemu_img(&["convert", "-f", &fmt, "-O", "qcow2", &src, &out]).await?;
    let virtual_size = info["virtual-size"].as_u64().unwrap_or(0);
    let wanted = vm.disk_size.saturating_mul(1024 * 1024 * 1024);
    if wanted > virtual_size {
        qemu_img(&["resize", "-f", "qcow2", &out, &format!("{}G", vm.disk_size)]).await?;
    }
    // fsync the new image before it replaces the old one.
    tokio::fs::File::open(&tmp_out).await?.sync_all().await?;
    tokio::fs::rename(&tmp_out, &disk).await?;
    tracing::info!("VM {id}: imported disk from {}", req.source);
    Ok(Json(state.vm_manager.get_vm(&id).await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn backing_files_and_unknown_formats_are_refused() {
        assert_eq!(
            checked_format(&json!({"format": "qcow2"})).unwrap(),
            "qcow2"
        );
        assert_eq!(checked_format(&json!({"format": "raw"})).unwrap(), "raw");
        assert!(
            checked_format(&json!({"format": "qcow2", "backing-filename": "/etc/shadow"})).is_err()
        );
        assert!(checked_format(&json!({"format": "qcow2", "full-backing-filename": "x"})).is_err());
        assert!(checked_format(&json!({"format": "vvfat"})).is_err());
        assert!(checked_format(&json!({})).is_err());
    }

    #[test]
    fn external_data_files_and_foreign_vmdk_extents_are_refused() {
        let qcow2 = |data: serde_json::Value| {
            json!({"format": "qcow2", "filename": "/i/a",
                   "format-specific": {"type": "qcow2", "data": data}})
        };
        assert!(checked_format(&qcow2(json!({"compat": "1.1"}))).is_ok());
        assert!(checked_format(&qcow2(json!({"data-file": "/etc/shadow"}))).is_err());
        let vmdk = |extents: serde_json::Value| {
            json!({"format": "vmdk", "filename": "/i/a.vmdk",
                   "format-specific": {"type": "vmdk", "data": {"extents": extents}}})
        };
        assert_eq!(
            checked_format(&vmdk(json!([{"filename": "/i/a.vmdk"}]))).unwrap(),
            "vmdk"
        );
        assert!(checked_format(&vmdk(json!([{"filename": "/etc/shadow"}]))).is_err());
        assert!(checked_format(&vmdk(
            json!([{"filename": "/i/a.vmdk"}, {"filename": "/etc/shadow"}])
        ))
        .is_err());
        assert!(checked_format(&vmdk(json!([]))).is_err(), "no extent info");
        assert!(checked_format(&json!({"format": "vmdk", "filename": "/i/a.vmdk"})).is_err());
        let mut no_name = vmdk(json!([{"filename": "/i/a.vmdk"}]));
        no_name.as_object_mut().unwrap().remove("filename");
        assert!(checked_format(&no_name).is_err());
    }

    fn qemu_img_info(path: &FsPath) -> serde_json::Value {
        let out = std::process::Command::new("qemu-img")
            .args(["info", "--output=json"])
            .arg(path)
            .output()
            .expect("qemu-img");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn qemu_img_create(args: &[&str], path: &FsPath) {
        let st = std::process::Command::new("qemu-img")
            .args(["create", "-q"])
            .args(args)
            .arg(path)
            .arg("1M")
            .status()
            .expect("qemu-img");
        assert!(st.success());
    }

    /// The same checks against what the real qemu-img reports for crafted
    /// images that point at a "host secret" outside the import roots.
    #[test]
    fn real_images_that_read_another_host_file_are_refused() {
        let d = std::env::temp_dir().join(format!("vm-import-real-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let secret = d.join("secret");
        std::fs::write(&secret, vec![0x5au8; 1 << 20]).unwrap();

        // Ordinary single-file images are still accepted.
        let plain = d.join("plain.qcow2");
        qemu_img_create(&["-f", "qcow2"], &plain);
        assert_eq!(checked_format(&qemu_img_info(&plain)).unwrap(), "qcow2");
        let sparse = d.join("sparse.vmdk");
        qemu_img_create(&["-f", "vmdk"], &sparse);
        assert_eq!(checked_format(&qemu_img_info(&sparse)).unwrap(), "vmdk");

        // qcow2 whose guest data lives in an external file.
        let ext = d.join("ext.qcow2");
        let data = d.join("ext.data");
        let opt = format!("data_file={},data_file_raw=on", data.display());
        qemu_img_create(&["-f", "qcow2", "-o", &opt], &ext);
        let err = checked_format(&qemu_img_info(&ext)).unwrap_err();
        assert!(err.contains("external data file"), "{err}");

        // VMDK descriptor (plain text) whose extent is the secret.
        let desc = d.join("desc.vmdk");
        std::fs::write(
            &desc,
            format!(
                "# Disk DescriptorFile\nversion=1\nCID=fffffffe\nparentCID=ffffffff\n\
                 createType=\"monolithicFlat\"\n\nRW 2048 FLAT \"{}\" 0\n\n\
                 ddb.adapterType = \"ide\"\n",
                secret.display()
            ),
        )
        .unwrap();
        let info = qemu_img_info(&desc);
        assert_eq!(info["format"], "vmdk", "{info}");
        let err = checked_format(&info).unwrap_err();
        assert!(err.contains("single-file VMDK"), "{err}");

        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn url_sources_may_not_reach_the_node_or_metadata() {
        let ip = |s: &str| refused_ip(s.parse().unwrap());
        for bad in [
            "127.0.0.1",
            "127.8.9.10",
            "0.0.0.0",
            "0.1.2.3",
            "169.254.169.254",
            "224.0.0.1",
            "255.255.255.255",
            "::1",
            "::",
            "fe80::1",
            "fd00:ec2::254",
            "ff02::1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "64:ff9b::a9fe:a9fe",
        ] {
            assert!(ip(bad).is_some(), "{bad} must be refused");
        }
        for good in [
            "10.0.0.5",
            "192.168.1.10",
            "172.16.0.1",
            "8.8.8.8",
            "fd12:3456::1",
            "2001:db8::1",
            "::ffff:192.168.1.10",
        ] {
            assert_eq!(ip(good), None, "{good} is a normal LAN/internet address");
        }

        let url = |s: &str| refused_url(&reqwest::Url::parse(s).unwrap());
        assert_eq!(url("https://cloud.debian.org/images/x.qcow2"), None);
        assert_eq!(url("http://192.168.1.10:8080/cirros.img"), None);
        assert!(url("http://169.254.169.254/latest/meta-data/").is_some());
        assert!(url("http://[::1]:8006/api").is_some());
        assert!(url("http://2130706433/").is_some(), "decimal 127.0.0.1");
        assert!(url("http://0x7f.1/").is_some(), "hex 127.0.0.1");
        assert!(url("file:///etc/shadow").is_some());
        assert!(url("ftp://example.com/x").is_some());
    }

    #[tokio::test]
    async fn downloads_never_connect_to_refused_addresses() {
        // A listener that records whether anything connected.
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let hit = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let h = hit.clone();
        let srv = tokio::spawn(async move {
            while l.accept().await.is_ok() {
                h.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        });
        let to = std::env::temp_dir().join(format!("vm-import-dl-{}", std::process::id()));
        // A literal loopback IP is refused before any request is made.
        let err = download(&format!("http://127.0.0.1:{port}/a.qcow2"), &to)
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::Forbidden(_)), "{err:?}");
        // A host name resolving to loopback is refused by the resolver.
        let err = download(&format!("http://localhost:{port}/a.qcow2"), &to)
            .await
            .unwrap_err();
        assert!(format!("{err:?}").contains("refusing"), "{err:?}");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(
            !hit.load(std::sync::atomic::Ordering::SeqCst),
            "nothing may connect to the node's own services"
        );
        assert!(!to.exists());
        srv.abort();
    }

    /// Set (to anything) in the child process
    /// [`downloads_ignore_system_proxies`] runs itself as.
    const PROXY_CHILD: &str = "HORCRUX_TEST_DOWNLOAD_PROXY_CHILD";

    /// With `HTTP(S)_PROXY`/`ALL_PROXY` pointing at a fake proxy, a host
    /// name for the node itself is still refused and nothing reaches the
    /// proxy (a proxy would resolve the name itself, past the guard). Runs
    /// in a child process so the proxy environment can't leak into other
    /// tests of this binary.
    #[test]
    fn downloads_ignore_system_proxies() {
        if std::env::var_os(PROXY_CHILD).is_some() {
            return proxied_downloads_child();
        }
        use std::io::{Read, Write};
        // The fake proxy: records each request line, answers 200.
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy = format!("http://{}", l.local_addr().unwrap());
        let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let s = seen.clone();
        std::thread::spawn(move || {
            for mut c in l.incoming().flatten() {
                let mut got = Vec::new();
                let mut buf = [0u8; 2048];
                while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                    match c.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => got.extend_from_slice(&buf[..n]),
                    }
                }
                let line = String::from_utf8_lossy(&got)
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_string();
                s.lock().unwrap().push(line);
                let _ = c.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                );
            }
        });
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "downloads_ignore_system_proxies",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(PROXY_CHILD, "1")
            .env_remove("NO_PROXY")
            .env_remove("no_proxy")
            .env_remove("REQUEST_METHOD");
        for var in [
            "HTTP_PROXY",
            "http_proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "ALL_PROXY",
            "all_proxy",
        ] {
            child.env(var, &proxy);
        }
        let out = child.output().expect("run the proxied child");
        let log = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.status.success(), "child failed:\n{log}");
        assert!(log.contains("1 passed"), "child ran nothing:\n{log}");
        let seen = seen.lock().unwrap().clone();
        // Only the control request (a plain client, which does use the
        // environment's proxy) may have reached the fake proxy.
        assert_eq!(
            seen,
            vec!["GET http://proxy-control.invalid/ HTTP/1.1".to_string()],
            "a guarded download went through the proxy:\n{log}"
        );
    }

    fn proxied_downloads_child() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            // Control: the proxy environment is in effect (a default client
            // sends the unresolved .invalid name to the fake proxy).
            let r = reqwest::Client::new()
                .get("http://proxy-control.invalid/")
                .send()
                .await
                .expect("the fake proxy answers the control request");
            assert_eq!(r.status(), 200);

            let to = std::env::temp_dir().join(format!("vm-import-proxy-{}", std::process::id()));
            for url in [
                "http://localhost:18080/canary.qcow2",
                "https://localhost/canary.qcow2",
            ] {
                let err = download(url, &to).await.unwrap_err();
                assert!(format!("{err:?}").contains("refusing"), "{url}: {err:?}");
            }
            // literal metadata address: refused before any request
            let err = download("http://169.254.169.254/latest/meta-data/", &to)
                .await
                .unwrap_err();
            assert!(matches!(err, ApiError::Forbidden(_)), "{err:?}");
            // the client shared with the ISO library refuses the same way
            let err = download_client()
                .unwrap()
                .get("http://localhost/x.iso")
                .send()
                .await
                .unwrap_err();
            assert!(error_chain(&err).contains("refusing"), "{err:?}");
            assert!(!to.exists());
        });
    }

    #[test]
    fn local_sources_must_be_inside_an_allowed_root() {
        let d = std::env::temp_dir().join(format!("vm-import-test-{}", std::process::id()));
        let images = d.join("images");
        std::fs::create_dir_all(&images).unwrap();
        let inside = images.join("a.qcow2");
        std::fs::write(&inside, b"x").unwrap();
        let outside = d.join("b.qcow2");
        std::fs::write(&outside, b"x").unwrap();
        let roots = vec![images.clone()];
        let canon = |p: &PathBuf| std::fs::canonicalize(p).unwrap();
        assert!(allowed_local_source(&canon(&inside), &roots));
        assert!(!allowed_local_source(&canon(&outside), &roots));
        assert!(
            !allowed_local_source(&canon(&images), &roots),
            "the root itself"
        );
        // A ../ escape canonicalizes outside the root.
        let escape = images.join("../b.qcow2");
        assert!(!allowed_local_source(&canon(&escape), &roots));
        assert!(!allowed_local_source(&canon(&inside), &[d.join("missing")]));
        std::fs::remove_dir_all(&d).unwrap();
    }
}
