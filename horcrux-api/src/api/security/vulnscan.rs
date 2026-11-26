//! Vulnerability scanning of container images and container root file
//! systems with a free scanner (Grype or Trivy, both Apache-2.0).
//!
//! Scans run one at a time by default (`vuln_scan.max_concurrent`), at low
//! CPU priority and idle IO class, with a hard timeout. Results are kept as
//! one JSON file per scan under `<data_dir>/security/scans/` and pruned to
//! `vuln_scan.keep_scans`.
use horcrux_common::security::{
    ScanResult, ScanStatus, ScanSummary, ScanTarget, ScanTargetKind, Severity, SeverityCounts,
    VulnFinding, VulnScanSettings,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Parse Grype's `-o json` output.
pub fn parse_grype(v: &Value) -> Vec<VulnFinding> {
    let mut out = Vec::new();
    for m in v["matches"].as_array().into_iter().flatten() {
        let vu = &m["vulnerability"];
        let a = &m["artifact"];
        let fixed_in = vu["fix"]["versions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| x.as_str().map(String::from))
            .collect();
        let url = vu["dataSource"]
            .as_str()
            .map(String::from)
            .or_else(|| vu["urls"][0].as_str().map(String::from))
            .unwrap_or_default();
        out.push(VulnFinding {
            id: vu["id"].as_str().unwrap_or("").to_string(),
            severity: Severity::parse(vu["severity"].as_str().unwrap_or("")),
            package: a["name"].as_str().unwrap_or("").to_string(),
            installed: a["version"].as_str().unwrap_or("").to_string(),
            fixed_in,
            pkg_type: a["type"].as_str().unwrap_or("").to_string(),
            title: vu["description"]
                .as_str()
                .unwrap_or("")
                .chars()
                .take(300)
                .collect(),
            url,
        });
    }
    out
}

/// Parse Trivy's `--format json` output.
pub fn parse_trivy(v: &Value) -> Vec<VulnFinding> {
    let mut out = Vec::new();
    for r in v["Results"].as_array().into_iter().flatten() {
        let ty = r["Type"].as_str().unwrap_or("").to_string();
        for x in r["Vulnerabilities"].as_array().into_iter().flatten() {
            let fixed: Vec<String> = x["FixedVersion"]
                .as_str()
                .unwrap_or("")
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            out.push(VulnFinding {
                id: x["VulnerabilityID"].as_str().unwrap_or("").to_string(),
                severity: Severity::parse(x["Severity"].as_str().unwrap_or("")),
                package: x["PkgName"].as_str().unwrap_or("").to_string(),
                installed: x["InstalledVersion"].as_str().unwrap_or("").to_string(),
                fixed_in: fixed,
                pkg_type: ty.clone(),
                title: x["Title"].as_str().unwrap_or("").to_string(),
                url: x["PrimaryURL"].as_str().unwrap_or("").to_string(),
            });
        }
    }
    out
}

/// Sort worst first, then by id; count.
pub fn finish(mut f: Vec<VulnFinding>) -> (Vec<VulnFinding>, SeverityCounts) {
    f.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then_with(|| a.id.cmp(&b.id))
            .then_with(|| a.package.cmp(&b.package))
    });
    f.dedup_by(|a, b| a.id == b.id && a.package == b.package && a.installed == b.installed);
    let mut c = SeverityCounts::default();
    for x in &f {
        c.add(x.severity, !x.fixed_in.is_empty());
    }
    (f, c)
}

/// Image references and container names a scan may name: no option
/// injection, no path tricks.
pub fn valid_target(kind: ScanTargetKind, t: &str) -> Result<(), String> {
    if t.is_empty() || t.len() > 255 {
        return Err("target: 1-255 characters".into());
    }
    if t.starts_with('-') {
        return Err("target: must not start with '-'".into());
    }
    let ok = if kind.is_image() {
        t.chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-/:@".contains(c))
            && !t.contains("..")
    } else {
        t.chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
            && t != "."
            && t != ".."
    };
    if ok {
        Ok(())
    } else {
        Err(format!("target: {t:?} is not a valid {}", kind.label()))
    }
}

/// Root file system of a container target.
pub fn rootfs(s: &VulnScanSettings, kind: ScanTargetKind, name: &str) -> Option<PathBuf> {
    let p = match kind {
        ScanTargetKind::Lxc => Path::new(&s.lxc_path).join(name).join("rootfs"),
        ScanTargetKind::Incus => Path::new(&s.incus_path)
            .join("containers")
            .join(name)
            .join("rootfs"),
        _ => return None,
    };
    p.is_dir().then_some(p)
}

/// Scanner database directory.
pub fn db_dir(s: &VulnScanSettings, data_dir: &Path) -> PathBuf {
    if s.db_dir.trim().is_empty() {
        data_dir.join("security").join("scanner-db")
    } else {
        PathBuf::from(s.db_dir.trim())
    }
}

/// Arguments and environment for one scan (program is `s.program()`).
pub fn command(
    s: &VulnScanSettings,
    kind: ScanTargetKind,
    target: &str,
    rootfs: Option<&Path>,
    db: &Path,
) -> (Vec<String>, Vec<(String, String)>) {
    let mut args = Vec::new();
    let mut env = Vec::new();
    let db = db.display().to_string();
    if s.scanner == "trivy" {
        args.push(if kind.is_image() { "image" } else { "rootfs" }.to_string());
        args.extend(["--format", "json", "--quiet", "--scanners", "vuln"].map(String::from));
        args.extend(["--cache-dir".to_string(), db]);
        if kind.is_image() {
            args.extend(["--image-src".to_string(), kind.as_str().to_string()]);
        }
        if !s.update_db {
            args.push("--skip-db-update".into());
        }
        if !s.db_url.trim().is_empty() {
            args.extend(["--db-repository".to_string(), s.db_url.trim().to_string()]);
        }
        if s.only_fixed {
            args.push("--ignore-unfixed".into());
        }
        args.push("--".into());
        args.push(match rootfs {
            Some(p) => p.display().to_string(),
            None => target.to_string(),
        });
        env.push(("TRIVY_NO_PROGRESS".into(), "true".into()));
    } else {
        args.push(match (kind, rootfs) {
            (_, Some(p)) => format!("dir:{}", p.display()),
            (k, None) => format!("{}:{target}", k.as_str()),
        });
        args.extend(["-o", "json", "-q"].map(String::from));
        if s.only_fixed {
            args.push("--only-fixed".into());
        }
        env.push(("GRYPE_DB_CACHE_DIR".into(), db));
        env.push(("GRYPE_DB_AUTO_UPDATE".into(), s.update_db.to_string()));
        env.push(("GRYPE_CHECK_FOR_APP_UPDATE".into(), "false".into()));
        if !s.update_db {
            // A copied-in database must not be refused for its age.
            env.push(("GRYPE_DB_VALIDATE_AGE".into(), "false".into()));
        }
        if !s.db_url.trim().is_empty() {
            env.push(("GRYPE_DB_UPDATE_URL".into(), s.db_url.trim().to_string()));
        }
    }
    (args, env)
}

/// Parse a scanner's output into findings.
pub fn parse_output(scanner: &str, stdout: &str) -> Result<Vec<VulnFinding>, String> {
    let v: Value = serde_json::from_str(stdout.trim())
        .map_err(|e| format!("scanner output is not JSON: {e}"))?;
    Ok(if scanner == "trivy" {
        parse_trivy(&v)
    } else {
        parse_grype(&v)
    })
}

/// Scanner version from `<scanner> version` text (first `Version:` line).
pub fn parse_version(text: &str) -> String {
    for l in text.lines() {
        let l = l.trim();
        if let Some(v) = l
            .strip_prefix("Version:")
            .or_else(|| l.strip_prefix("version:"))
        {
            return v.trim().to_string();
        }
    }
    text.lines().next().unwrap_or("").trim().to_string()
}

/// `<repo>:<tag> <id>` lines from `docker/podman images --format`.
pub fn parse_image_list(kind: ScanTargetKind, text: &str) -> Vec<ScanTarget> {
    let mut out: Vec<ScanTarget> = text
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let name = it.next()?;
            let id = it.next().unwrap_or("");
            if name.contains("<none>") || valid_target(kind, name).is_err() {
                return None;
            }
            Some(ScanTarget {
                kind,
                name: name.to_string(),
                detail: id.chars().take(19).collect(),
                last_scan: None,
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.dedup_by(|a, b| a.name == b.name);
    out
}

/// Containers below `base` (a directory per container with a `rootfs`).
pub fn list_rootfs(kind: ScanTargetKind, base: &Path) -> Vec<ScanTarget> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(base) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if e.path().join("rootfs").is_dir() && valid_target(kind, &name).is_ok() {
                out.push(ScanTarget {
                    kind,
                    name,
                    detail: String::new(),
                    last_scan: None,
                });
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Scan files on disk.
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.join("security").join("scans"),
        }
    }

    fn path(&self, id: &str) -> Option<PathBuf> {
        (!id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
            .then(|| self.dir.join(format!("{id}.json")))
    }

    pub fn save(&self, r: &ScanResult) -> Result<(), String> {
        let p = self.path(&r.summary.id).ok_or("bad scan id")?;
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("{}: {e}", self.dir.display()))?;
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(r).unwrap_or_default())
            .map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &p).map_err(|e| format!("{}: {e}", p.display()))
    }

    pub fn get(&self, id: &str) -> Option<ScanResult> {
        let b = std::fs::read(self.path(id)?).ok()?;
        serde_json::from_slice(&b).ok()
    }

    pub fn delete(&self, id: &str) -> bool {
        self.path(id)
            .map(|p| std::fs::remove_file(p).is_ok())
            .unwrap_or(false)
    }

    /// Every scan, newest first, without findings.
    pub fn list(&self) -> Vec<ScanSummary> {
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) != Some("json") {
                    continue;
                }
                if let Ok(b) = std::fs::read(&p) {
                    if let Ok(r) = serde_json::from_slice::<ScanResult>(&b) {
                        out.push(r.summary);
                    }
                }
            }
        }
        out.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| b.id.cmp(&a.id)));
        out
    }

    /// Remove finished scans beyond `keep` (oldest first).
    pub fn prune(&self, keep: usize) -> usize {
        let done: Vec<ScanSummary> = self
            .list()
            .into_iter()
            .filter(|s| matches!(s.status, ScanStatus::Done | ScanStatus::Failed))
            .collect();
        let mut n = 0;
        for s in done.iter().skip(keep) {
            if self.delete(&s.id) {
                n += 1;
            }
        }
        n
    }
}

/// Newest finished scan per (kind, target).
pub fn latest_by_target(scans: &[ScanSummary]) -> BTreeMap<(String, String), ScanSummary> {
    let mut m: BTreeMap<(String, String), ScanSummary> = BTreeMap::new();
    for s in scans {
        if s.status != ScanStatus::Done && s.status != ScanStatus::Failed {
            continue;
        }
        let k = (
            s.kind.map(|k| k.as_str().to_string()).unwrap_or_default(),
            s.target.clone(),
        );
        // `scans` is newest first: keep the first one seen.
        m.entry(k).or_insert_with(|| s.clone());
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn grype_output_parses() {
        let v = json!({"matches":[
            {"vulnerability":{"id":"CVE-2023-1","severity":"High","fix":{"versions":["1.2.4"],"state":"fixed"},
              "dataSource":"https://nvd.nist.gov/vuln/detail/CVE-2023-1","description":"bad"},
             "artifact":{"name":"openssl","version":"1.2.3","type":"apk"}},
            {"vulnerability":{"id":"CVE-2023-2","severity":"Critical","fix":{"versions":[],"state":"not-fixed"},"urls":["https://x/2"]},
             "artifact":{"name":"zlib","version":"1.0","type":"apk"}},
            {"vulnerability":{"id":"CVE-2023-2","severity":"Critical","fix":{"versions":[]}},
             "artifact":{"name":"zlib","version":"1.0","type":"apk"}}
        ]});
        let (f, c) = finish(parse_grype(&v));
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].id, "CVE-2023-2");
        assert_eq!(f[0].url, "https://x/2");
        assert_eq!(f[1].fixed_in, vec!["1.2.4"]);
        assert_eq!((c.critical, c.high, c.fixable, c.total()), (1, 1, 1, 2));
    }

    #[test]
    fn trivy_output_parses() {
        let v = json!({"Results":[{"Target":"alpine","Type":"alpine","Vulnerabilities":[
            {"VulnerabilityID":"CVE-2024-9","PkgName":"musl","InstalledVersion":"1.2","FixedVersion":"1.3, 1.2.5",
             "Severity":"MEDIUM","Title":"t","PrimaryURL":"https://avd/9"}]},{"Target":"x","Type":"gobinary"}]});
        let (f, c) = finish(parse_trivy(&v));
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].fixed_in, vec!["1.3", "1.2.5"]);
        assert_eq!(f[0].severity, Severity::Medium);
        assert_eq!(c.medium, 1);
    }

    #[test]
    fn targets_are_validated() {
        assert!(valid_target(ScanTargetKind::Podman, "docker.io/library/alpine:3.19").is_ok());
        assert!(valid_target(ScanTargetKind::Docker, "alpine@sha256:abc").is_ok());
        assert!(valid_target(ScanTargetKind::Docker, "--config=/etc/x").is_err());
        assert!(valid_target(ScanTargetKind::Docker, "a b").is_err());
        assert!(valid_target(ScanTargetKind::Lxc, "web-1").is_ok());
        assert!(valid_target(ScanTargetKind::Lxc, "../etc").is_err());
        assert!(valid_target(ScanTargetKind::Incus, "a/b").is_err());
    }

    #[test]
    fn commands_for_both_scanners() {
        let mut s = VulnScanSettings::default();
        let db = Path::new("/d");
        let (a, e) = command(&s, ScanTargetKind::Podman, "alpine:3", None, db);
        assert_eq!(a[0], "podman:alpine:3");
        assert!(e.contains(&("GRYPE_DB_CACHE_DIR".into(), "/d".into())));
        let (a, _) = command(
            &s,
            ScanTargetKind::Lxc,
            "c1",
            Some(Path::new("/var/lib/lxc/c1/rootfs")),
            db,
        );
        assert_eq!(a[0], "dir:/var/lib/lxc/c1/rootfs");
        s.update_db = false;
        s.db_url = "https://mirror/listing.json".into();
        let (_, e) = command(&s, ScanTargetKind::Docker, "x", None, db);
        assert!(e.contains(&("GRYPE_DB_AUTO_UPDATE".into(), "false".into())));
        assert!(e.contains(&(
            "GRYPE_DB_UPDATE_URL".into(),
            "https://mirror/listing.json".into()
        )));
        s.scanner = "trivy".into();
        s.only_fixed = true;
        let (a, _) = command(&s, ScanTargetKind::Docker, "nginx:1", None, db);
        assert_eq!(a[0], "image");
        assert!(a.windows(2).any(|w| w == ["--image-src", "docker"]));
        assert!(
            a.contains(&"--skip-db-update".to_string())
                && a.contains(&"--ignore-unfixed".to_string())
        );
        assert_eq!(a[a.len() - 2..], ["--".to_string(), "nginx:1".to_string()]);
    }

    #[test]
    fn image_lists_and_versions() {
        let t = parse_image_list(
            ScanTargetKind::Docker,
            "alpine:3.19 sha256:aaaa\n<none>:<none> sha256:bbbb\nalpine:3.19 sha256:aaaa\nnginx:1 cccc\n",
        );
        assert_eq!(
            t.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(),
            ["alpine:3.19", "nginx:1"]
        );
        assert_eq!(
            parse_version("Application: grype\nVersion: 0.92.0\n"),
            "0.92.0"
        );
        assert_eq!(
            parse_version("Version: 0.58.1\nVulnerability DB:\n"),
            "0.58.1"
        );
    }

    #[test]
    fn store_round_trip_and_prune() {
        let d = tempfile::tempdir().unwrap();
        let st = Store::new(d.path());
        for i in 0..5 {
            st.save(&ScanResult {
                summary: ScanSummary {
                    id: format!("s{i}"),
                    kind: Some(ScanTargetKind::Docker),
                    target: "a".into(),
                    status: ScanStatus::Done,
                    created: i,
                    ..Default::default()
                },
                findings: vec![],
            })
            .unwrap();
        }
        assert_eq!(st.list()[0].id, "s4");
        assert_eq!(
            latest_by_target(&st.list()).values().next().unwrap().id,
            "s4"
        );
        assert_eq!(st.prune(2), 3);
        assert_eq!(st.list().len(), 2);
        assert!(st.get("../x").is_none());
        assert!(st.get("s4").is_some());
    }
}
