//! LXC template store: the `pveam` equivalent.
//!
//! The catalog is the system image index of images.linuxcontainers.org
//! (`meta/1.0/index-system`, the same one `lxc-create -t download` reads).
//! Downloading fetches the image's `rootfs.tar.xz`, verifies it against the
//! build's `SHA256SUMS`, and keeps it in `<data_dir>/lxc-templates/
//! <distro>/<release>/<arch>/<variant>/` so containers are created from the
//! local copy without network access.
use horcrux_common::{Error, LxcCatalogEntry, LxcTemplate, Result};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use tracing::info;

pub const DEFAULT_IMAGES_URL: &str = "https://images.linuxcontainers.org";
const CATALOG_TTL: Duration = Duration::from_secs(30 * 60);
const ROOTFS: &str = "rootfs.tar.xz";

/// One path component of a template id (`alpine`, `3.22`, `amd64`,
/// `default`): no separators, no `.`/`..`.
pub fn valid_component(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s != "."
        && s != ".."
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// `HORCRUX_IMAGE_DIR`, default `<HORCRUX_DATA_DIR or /var/lib/horcrux>/images`.
pub fn default_image_dir() -> PathBuf {
    std::env::var("HORCRUX_IMAGE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HORCRUX_DATA_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/var/lib/horcrux"))
                .join("images")
        })
}

/// Parse `index-system`: `distro;release;arch;variant;build;path` lines.
pub fn parse_index(text: &str) -> Vec<LxcCatalogEntry> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.trim().split(';').collect();
            if f.len() < 6 || !f[..4].iter().all(|c| valid_component(c)) {
                return None;
            }
            Some(LxcCatalogEntry {
                distro: f[0].into(),
                release: f[1].into(),
                arch: f[2].into(),
                variant: f[3].into(),
                build: f[4].into(),
                id: format!("{}/{}/{}/{}", f[0], f[1], f[2], f[3]),
            })
        })
        .collect()
}

/// Server path of an image build (`/images/<d>/<r>/<a>/<v>/<build>/`).
fn build_path(e: &LxcCatalogEntry) -> String {
    format!(
        "/images/{}/{}/{}/{}/{}/",
        e.distro, e.release, e.arch, e.variant, e.build
    )
}

/// Find `name`'s hash in a SHA256SUMS file.
pub fn sha256_for(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        let (h, f) = (it.next()?, it.next()?);
        (f.trim_start_matches('*') == name && h.len() == 64).then(|| h.to_ascii_lowercase())
    })
}

/// A template reference in a create request, resolved to a local tarball.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateRef {
    /// `distro/release[/variant]` or `distro/release/arch/variant`.
    Catalog {
        distro: String,
        release: String,
        arch: String,
        variant: String,
    },
    /// Absolute path of a local rootfs tarball (.tar, .tar.gz, .tar.xz, .tar.zst).
    Local(PathBuf),
}

impl TemplateRef {
    pub fn parse(spec: &str, default_arch: &str) -> std::result::Result<Self, String> {
        let spec = spec.trim();
        if spec.starts_with('/') {
            if spec.contains("..") {
                return Err(format!("invalid template path '{spec}'"));
            }
            let ok_ext = [".tar", ".tar.gz", ".tgz", ".tar.xz", ".tar.zst"]
                .iter()
                .any(|e| spec.ends_with(e));
            if !ok_ext {
                return Err(format!(
                    "local template '{spec}' must be a .tar, .tar.gz, .tar.xz or .tar.zst rootfs archive"
                ));
            }
            return Ok(Self::Local(PathBuf::from(spec)));
        }
        let parts: Vec<&str> = spec.split('/').collect();
        let (d, r, a, v) = match parts.as_slice() {
            [d, r] => (*d, *r, default_arch, "default"),
            [d, r, v] => (*d, *r, default_arch, *v),
            [d, r, a, v] => (*d, *r, *a, *v),
            _ => {
                return Err(format!(
                    "template '{spec}' must be distro/release (e.g. alpine/3.22, debian/bookworm), \
                     distro/release/variant, distro/release/arch/variant, or an absolute path to a \
                     rootfs tarball"
                ))
            }
        };
        for c in [d, r, a, v] {
            if !valid_component(c) {
                return Err(format!("invalid template '{spec}'"));
            }
        }
        Ok(Self::Catalog {
            distro: d.into(),
            release: r.into(),
            arch: a.into(),
            variant: v.into(),
        })
    }
}

pub struct TemplateStore {
    dir: PathBuf,
    /// Directory local rootfs tarballs may come from (besides `dir`).
    image_dir: PathBuf,
    base_url: String,
    client: reqwest::Client,
    catalog: Mutex<Option<(Instant, Vec<LxcCatalogEntry>)>>,
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl TemplateStore {
    pub fn new(dir: PathBuf, base_url: String) -> Self {
        Self::with_image_dir(dir, default_image_dir(), base_url)
    }

    /// Like `new`, with an explicit directory for local rootfs tarballs.
    pub fn with_image_dir(dir: PathBuf, image_dir: PathBuf, base_url: String) -> Self {
        Self {
            dir,
            image_dir,
            base_url: base_url.trim_end_matches('/').to_string(),
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(20))
                .timeout(Duration::from_secs(1800))
                .build()
                .unwrap_or_default(),
            catalog: Mutex::new(None),
            locks: Mutex::new(HashMap::new()),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Where local rootfs tarballs must live (`HORCRUX_IMAGE_DIR`, default
    /// `<data_dir>/images`, the same directory disk imports and ISOs use).
    pub fn image_dir(&self) -> &Path {
        &self.image_dir
    }

    fn template_dir(&self, d: &str, r: &str, a: &str, v: &str) -> PathBuf {
        self.dir.join(d).join(r).join(a).join(v)
    }

    async fn get_text(&self, url: &str) -> Result<String> {
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| Error::System(format!("GET {url}: {e}")))?;
        if !resp.status().is_success() {
            return Err(Error::System(format!("GET {url}: HTTP {}", resp.status())));
        }
        resp.text()
            .await
            .map_err(|e| Error::System(format!("GET {url}: {e}")))
    }

    /// The image catalog (cached for 30 minutes; `refresh` forces a fetch).
    pub async fn catalog(&self, refresh: bool) -> Result<Vec<LxcCatalogEntry>> {
        let mut cache = self.catalog.lock().await;
        if let Some((at, list)) = cache.as_ref() {
            if !refresh && at.elapsed() < CATALOG_TTL {
                return Ok(list.clone());
            }
        }
        let text = self
            .get_text(&format!("{}/meta/1.0/index-system", self.base_url))
            .await?;
        let list = parse_index(&text);
        if list.is_empty() {
            return Err(Error::System(
                "image index from the template server is empty or unreadable".into(),
            ));
        }
        *cache = Some((Instant::now(), list.clone()));
        Ok(list)
    }

    /// Downloaded templates.
    pub async fn list(&self) -> Result<Vec<LxcTemplate>> {
        let mut out = Vec::new();
        let pattern = |p: &Path| -> Vec<PathBuf> {
            std::fs::read_dir(p)
                .map(|rd| {
                    rd.flatten()
                        .map(|e| e.path())
                        .filter(|p| p.is_dir())
                        .collect()
                })
                .unwrap_or_default()
        };
        for d in pattern(&self.dir) {
            for r in pattern(&d) {
                for a in pattern(&r) {
                    for v in pattern(&a) {
                        if let Ok(text) = std::fs::read_to_string(v.join("template.json")) {
                            if let Ok(t) = serde_json::from_str::<LxcTemplate>(&text) {
                                if v.join(ROOTFS).is_file() {
                                    out.push(t);
                                }
                            }
                        }
                    }
                }
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    pub async fn get(&self, id: &str) -> Option<LxcTemplate> {
        self.list().await.ok()?.into_iter().find(|t| t.id == id)
    }

    async fn lock_for(&self, id: &str) -> Arc<Mutex<()>> {
        self.locks
            .lock()
            .await
            .entry(id.to_string())
            .or_default()
            .clone()
    }

    /// Download (or re-download a newer build of) a template. Returns the
    /// cached copy when the newest build is already present.
    pub async fn download(
        &self,
        distro: &str,
        release: &str,
        arch: &str,
        variant: &str,
    ) -> Result<LxcTemplate> {
        for c in [distro, release, arch, variant] {
            if !valid_component(c) {
                return Err(Error::Validation(format!(
                    "invalid template component '{c}'"
                )));
            }
        }
        let id = format!("{distro}/{release}/{arch}/{variant}");
        let lock = self.lock_for(&id).await;
        let _guard = lock.lock().await;

        let catalog = self.catalog(false).await?;
        let entry = match catalog.iter().find(|e| e.id == id) {
            Some(e) => e.clone(),
            None => {
                // Maybe the cache is stale (new release published).
                let fresh = self.catalog(true).await?;
                fresh.into_iter().find(|e| e.id == id).ok_or_else(|| {
                    Error::NotFound(format!(
                        "template {id} is not in the image catalog (see GET /api/lxc/templates/catalog)"
                    ))
                })?
            }
        };
        if let Some(t) = self.get(&id).await {
            if t.build == entry.build {
                return Ok(t);
            }
        }

        let tdir = self.template_dir(distro, release, arch, variant);
        tokio::fs::create_dir_all(&tdir).await?;
        let base = format!("{}{}", self.base_url, build_path(&entry));
        let sums = self.get_text(&format!("{base}SHA256SUMS")).await?;
        let want = sha256_for(&sums, ROOTFS)
            .ok_or_else(|| Error::System(format!("no {ROOTFS} checksum in {base}SHA256SUMS")))?;

        let url = format!("{base}{ROOTFS}");
        info!(
            "Downloading LXC template {id} build {} from {url}",
            entry.build
        );
        let mut resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| Error::System(format!("GET {url}: {e}")))?;
        if !resp.status().is_success() {
            return Err(Error::System(format!("GET {url}: HTTP {}", resp.status())));
        }
        let part = tdir.join(format!("{ROOTFS}.part"));
        let mut file = tokio::fs::File::create(&part).await?;
        let mut hasher = Sha256::new();
        let mut size = 0u64;
        loop {
            let chunk = match resp.chunk().await {
                Ok(Some(c)) => c,
                Ok(None) => break,
                Err(e) => {
                    let _ = tokio::fs::remove_file(&part).await;
                    return Err(Error::System(format!("download {url}: {e}")));
                }
            };
            hasher.update(&chunk);
            size += chunk.len() as u64;
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        file.sync_all().await?;
        drop(file);
        let got = hex::encode(hasher.finalize());
        if got != want {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(Error::System(format!(
                "checksum mismatch for {url}: expected {want}, got {got}"
            )));
        }
        tokio::fs::rename(&part, tdir.join(ROOTFS)).await?;
        // World-readable: unprivileged extraction reads it from a user ns.
        let _ = tokio::fs::set_permissions(
            tdir.join(ROOTFS),
            std::os::unix::fs::PermissionsExt::from_mode(0o644),
        )
        .await;
        let t = LxcTemplate {
            id: id.clone(),
            distro: distro.into(),
            release: release.into(),
            arch: arch.into(),
            variant: variant.into(),
            build: entry.build,
            size_bytes: size,
            sha256: got,
            downloaded_at: chrono::Utc::now().to_rfc3339(),
        };
        tokio::fs::write(
            tdir.join("template.json"),
            serde_json::to_vec_pretty(&t).map_err(|e| Error::Internal(e.to_string()))?,
        )
        .await?;
        info!("LXC template {id} downloaded ({size} bytes, sha256 verified)");
        Ok(t)
    }

    pub async fn delete(&self, id: &str) -> Result<()> {
        let parts: Vec<&str> = id.split('/').collect();
        if parts.len() != 4 || !parts.iter().all(|c| valid_component(c)) {
            return Err(Error::Validation(format!(
                "template id must be distro/release/arch/variant, got '{id}'"
            )));
        }
        let lock = self.lock_for(id).await;
        let _guard = lock.lock().await;
        let tdir = self.template_dir(parts[0], parts[1], parts[2], parts[3]);
        if !tdir.join("template.json").exists() {
            return Err(Error::NotFound(format!("template {id} is not downloaded")));
        }
        tokio::fs::remove_dir_all(&tdir).await?;
        Ok(())
    }

    /// Local rootfs tarball for a create request, downloading a catalog
    /// template first if it isn't cached yet. Returns (tarball, distro hint).
    pub async fn resolve(&self, r: &TemplateRef) -> Result<(PathBuf, Option<String>)> {
        match r {
            TemplateRef::Local(p) => {
                // Only tarballs the admin put in the image directory (or the
                // template cache). Anywhere else, a user allowed to create
                // containers could unpack any host archive they can name -
                // another guest's backup (`rootfs.tar`), a config tarball -
                // into a container they own and read it from inside.
                let canon = std::fs::canonicalize(p).ok();
                let inside = canon.as_deref().is_some_and(|c| {
                    [&self.image_dir, &self.dir]
                        .iter()
                        .filter_map(|r| std::fs::canonicalize(r).ok())
                        .any(|r| c.starts_with(&r) && c != r)
                });
                if !inside {
                    return Err(Error::Validation(format!(
                        "local template {} must be a file under {}",
                        p.display(),
                        self.image_dir.display()
                    )));
                }
                let canon = canon.unwrap_or_default();
                if !canon.is_file() {
                    return Err(Error::Validation(format!(
                        "template file {} does not exist",
                        p.display()
                    )));
                }
                Ok((canon, None))
            }
            TemplateRef::Catalog {
                distro,
                release,
                arch,
                variant,
            } => {
                let id = format!("{distro}/{release}/{arch}/{variant}");
                let have = self.get(&id).await;
                if have.is_none() {
                    self.download(distro, release, arch, variant).await?;
                }
                Ok((
                    self.template_dir(distro, release, arch, variant)
                        .join(ROOTFS),
                    Some(distro.clone()),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_index() {
        let idx = "alpine;3.22;amd64;default;20261016_13:00;/images/alpine/3.22/amd64/default/20261016_13:00/\n\
                   debian;bookworm;arm64;cloud;20261016_05:24;/images/debian/bookworm/arm64/cloud/20261016_05:24/\n\
                   bad line\n\
                   ../etc;x;amd64;default;1;/x/\n";
        let e = parse_index(idx);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].id, "alpine/3.22/amd64/default");
        assert_eq!(e[0].build, "20261016_13:00");
        assert_eq!(
            build_path(&e[0]),
            "/images/alpine/3.22/amd64/default/20261016_13:00/"
        );
        assert_eq!(e[1].variant, "cloud");
    }

    #[test]
    fn finds_checksums() {
        let sums = "3bd11db6cf66be61279502045ce8407dd534bf80341d3bd1a0bdc2e316e4a784  rootfs.tar.xz\n\
                    d06d22dd390adc68315993aff2b43572c9f4d7b5e95b3e2d6e306d2cd6b60467  meta.tar.xz\n";
        assert_eq!(
            sha256_for(sums, "rootfs.tar.xz").as_deref(),
            Some("3bd11db6cf66be61279502045ce8407dd534bf80341d3bd1a0bdc2e316e4a784")
        );
        assert!(sha256_for(sums, "disk.qcow2").is_none());
    }

    #[test]
    fn parses_template_refs() {
        let c = |d: &str, r: &str, a: &str, v: &str| TemplateRef::Catalog {
            distro: d.into(),
            release: r.into(),
            arch: a.into(),
            variant: v.into(),
        };
        assert_eq!(
            TemplateRef::parse("alpine/3.22", "amd64"),
            Ok(c("alpine", "3.22", "amd64", "default"))
        );
        assert_eq!(
            TemplateRef::parse("debian/bookworm/cloud", "arm64"),
            Ok(c("debian", "bookworm", "arm64", "cloud"))
        );
        assert_eq!(
            TemplateRef::parse("ubuntu/noble/amd64/default", "arm64"),
            Ok(c("ubuntu", "noble", "amd64", "default"))
        );
        assert_eq!(
            TemplateRef::parse("/srv/t/debian-12.tar.zst", "amd64"),
            Ok(TemplateRef::Local("/srv/t/debian-12.tar.zst".into()))
        );
        for bad in [
            "alpine",
            "ubuntu:22.04",
            "../../etc/x",
            "alpine/../x",
            "/srv/../etc/shadow.tar",
            "/srv/notatarball",
            "a/b/c/d/e",
        ] {
            assert!(TemplateRef::parse(bad, "amd64").is_err(), "{bad} accepted");
        }
    }

    #[tokio::test]
    async fn list_and_delete_cached_templates() {
        let tmp = tempfile::tempdir().unwrap();
        let store = TemplateStore::new(tmp.path().to_path_buf(), DEFAULT_IMAGES_URL.into());
        assert!(store.list().await.unwrap().is_empty());
        let tdir = tmp.path().join("alpine/3.22/amd64/default");
        std::fs::create_dir_all(&tdir).unwrap();
        std::fs::write(tdir.join(ROOTFS), b"x").unwrap();
        let t = LxcTemplate {
            id: "alpine/3.22/amd64/default".into(),
            distro: "alpine".into(),
            release: "3.22".into(),
            arch: "amd64".into(),
            variant: "default".into(),
            build: "1".into(),
            size_bytes: 1,
            sha256: "0".repeat(64),
            downloaded_at: "2026-10-16T00:00:00Z".into(),
        };
        std::fs::write(tdir.join("template.json"), serde_json::to_vec(&t).unwrap()).unwrap();
        assert_eq!(store.list().await.unwrap(), vec![t.clone()]);
        // A cached template resolves without touching the network.
        let (p, d) = store
            .resolve(&TemplateRef::parse("alpine/3.22", "amd64").unwrap())
            .await
            .unwrap();
        assert_eq!(p, tdir.join(ROOTFS));
        assert_eq!(d.as_deref(), Some("alpine"));
        assert!(store.delete("../x/y/z").await.is_err());
        store.delete(&t.id).await.unwrap();
        assert!(store.list().await.unwrap().is_empty());
        assert!(matches!(store.delete(&t.id).await, Err(Error::NotFound(_))));
    }

    #[tokio::test]
    async fn local_templates_must_come_from_the_image_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("cache");
        let images = tmp.path().join("images");
        let elsewhere = tmp.path().join("backups/ct1");
        for d in [&cache, &images, &elsewhere] {
            std::fs::create_dir_all(d).unwrap();
        }
        let store =
            TemplateStore::with_image_dir(cache.clone(), images.clone(), DEFAULT_IMAGES_URL.into());
        let local = |p: &Path| TemplateRef::parse(p.to_str().unwrap(), "amd64").unwrap();

        // An admin-provided tarball in the image directory works.
        let ok = images.join("debian-12.tar.zst");
        std::fs::write(&ok, b"x").unwrap();
        let (p, d) = store.resolve(&local(&ok)).await.unwrap();
        assert_eq!(p, std::fs::canonicalize(&ok).unwrap());
        assert_eq!(d, None);

        // Another guest's backup archive (or any host tarball) is refused.
        let secret = elsewhere.join("rootfs.tar");
        std::fs::write(&secret, b"other tenant").unwrap();
        let err = store.resolve(&local(&secret)).await.unwrap_err();
        assert!(matches!(err, Error::Validation(_)), "{err:?}");

        // So is a symlink planted in the image directory that points out.
        let link = images.join("sneaky.tar");
        std::os::unix::fs::symlink(&secret, &link).unwrap();
        assert!(matches!(
            store.resolve(&local(&link)).await,
            Err(Error::Validation(_))
        ));

        // A directory, or a missing file, inside the image dir is refused too.
        let dir_tar = images.join("d.tar");
        std::fs::create_dir(&dir_tar).unwrap();
        assert!(store.resolve(&local(&dir_tar)).await.is_err());
        assert!(store
            .resolve(&local(&images.join("missing.tar")))
            .await
            .is_err());
    }
}
