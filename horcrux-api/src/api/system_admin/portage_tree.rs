//! Reading the Portage tree and the installed-package database, and writing
//! the small `/etc/portage` pin files Horcrux owns (Ceph release, kernel
//! line). Pure file work: everything is rooted at `HORCRUX_SYSROOT` so the
//! unit tests run against a scratch tree.
use std::cmp::Ordering;
use std::path::{Path, PathBuf};

/// Portage's keyword for the architecture this binary was built for.
pub fn portage_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "riscv64" => "riscv",
        "x86" => "x86",
        "powerpc64" => "ppc64",
        other => other,
    }
}

/// How an ebuild is keyworded for one architecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Keyword {
    /// `amd64`: the distribution's stable branch
    Stable,
    /// `~amd64`: the testing branch (needs package.accept_keywords)
    Testing,
    /// not keyworded (or `-amd64`): not offered for this architecture
    Missing,
}

pub fn keyword_for(keywords: &[String], arch: &str) -> Keyword {
    if keywords.iter().any(|k| k == arch) {
        Keyword::Stable
    } else if keywords.iter().any(|k| k == &format!("~{arch}")) {
        Keyword::Testing
    } else {
        Keyword::Missing
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ebuild {
    pub version: String,
    pub repo: String,
    pub keywords: Vec<String>,
    pub path: PathBuf,
}

/// The repositories under `var/db/repos` (plus the legacy `usr/portage`).
pub fn repo_dirs(root: &Path) -> Vec<PathBuf> {
    let mut v: Vec<_> = std::fs::read_dir(root.join("var/db/repos"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    let legacy = root.join("usr/portage");
    if legacy.is_dir() {
        v.push(legacy);
    }
    v.sort();
    v
}

/// A shell-style variable assignment in an ebuild (`NAME=value`, `NAME="value"`).
pub fn ebuild_var(text: &str, name: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let l = l.trim_start();
        l.strip_prefix(name)
            .and_then(|r| r.strip_prefix('='))
            .map(|v| {
                let v = v.split(" #").next().unwrap_or(v).trim();
                v.trim_matches(['"', '\'']).to_string()
            })
    })
}

/// Every ebuild of `cat/pkg` in every repository, newest version first.
pub fn ebuilds(root: &Path, atom: &str) -> Vec<Ebuild> {
    let Some((_, pn)) = atom.split_once('/') else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for repo in repo_dirs(root) {
        let dir = repo.join(atom);
        let repo_name = std::fs::read_to_string(repo.join("profiles/repo_name"))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| {
                repo.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default()
            });
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let f = e.file_name().to_string_lossy().to_string();
            let Some(stem) = f.strip_suffix(".ebuild") else {
                continue;
            };
            let Some(ver) = stem.strip_prefix(&format!("{pn}-")) else {
                continue;
            };
            if !ver.starts_with(|c: char| c.is_ascii_digit()) {
                continue;
            }
            let text = std::fs::read_to_string(e.path()).unwrap_or_default();
            let keywords = ebuild_var(&text, "KEYWORDS")
                .unwrap_or_default()
                .split_whitespace()
                .map(str::to_string)
                .collect();
            out.push(Ebuild {
                version: ver.to_string(),
                repo: repo_name.clone(),
                keywords,
                path: e.path(),
            });
        }
    }
    out.sort_by(|a, b| vercmp(&b.version, &a.version));
    out
}

/// Installed versions of `cat/pkg` (the vdb), newest first.
pub fn installed(root: &Path, atom: &str) -> Vec<String> {
    let Some((cat, pn)) = atom.split_once('/') else {
        return Vec::new();
    };
    let mut out: Vec<String> = std::fs::read_dir(root.join("var/db/pkg").join(cat))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let pf = e.file_name().to_string_lossy().to_string();
            let ver = pf.strip_prefix(&format!("{pn}-"))?;
            ver.starts_with(|c: char| c.is_ascii_digit())
                .then(|| ver.to_string())
        })
        .collect();
    out.sort_by(|a, b| vercmp(b, a));
    out
}

/// The vdb copy of an installed package's ebuild (read for variables such
/// as zfs-kmod's `MODULES_KERNEL_MAX`).
pub fn installed_ebuild_text(root: &Path, atom: &str, version: &str) -> Option<String> {
    let (cat, pn) = atom.split_once('/')?;
    let d = root
        .join("var/db/pkg")
        .join(cat)
        .join(format!("{pn}-{version}"));
    std::fs::read_to_string(d.join(format!("{pn}-{version}.ebuild"))).ok()
}

fn split_rev(v: &str) -> (&str, u64) {
    match v.rsplit_once("-r") {
        Some((a, r)) if r.chars().all(|c| c.is_ascii_digit()) && !r.is_empty() => {
            (a, r.parse().unwrap_or(0))
        }
        _ => (v, 0),
    }
}

fn suffix_rank(s: &str) -> (i32, u64) {
    let (name, num): (String, String) = (
        s.chars().take_while(|c| c.is_ascii_alphabetic()).collect(),
        s.chars().skip_while(|c| c.is_ascii_alphabetic()).collect(),
    );
    let r = match name.as_str() {
        "alpha" => -4,
        "beta" => -3,
        "pre" => -2,
        "rc" => -1,
        "p" => 1,
        _ => 0,
    };
    (r, num.parse().unwrap_or(0))
}

/// Gentoo version comparison (PMS algorithm 3.3, the parts real ebuild
/// versions use): numeric components, a trailing letter, `_alpha` / `_beta` /
/// `_pre` / `_rc` / `_p` suffixes, `-rN` revisions.
pub fn vercmp(a: &str, b: &str) -> Ordering {
    let (a, ra) = split_rev(a);
    let (b, rb) = split_rev(b);
    let mut sa = a.split('_');
    let mut sb = b.split('_');
    let (na, nb) = (sa.next().unwrap_or(""), sb.next().unwrap_or(""));
    let parts = |s: &str| -> (Vec<u64>, Option<char>) {
        let letter = s.chars().last().filter(|c| c.is_ascii_alphabetic());
        let body = letter.map_or(s, |_| &s[..s.len() - 1]);
        (
            body.split('.').map(|p| p.parse().unwrap_or(0)).collect(),
            letter,
        )
    };
    let ((pa, la), (pb, lb)) = (parts(na), parts(nb));
    for i in 0..pa.len().max(pb.len()) {
        match (pa.get(i), pb.get(i)) {
            (Some(x), Some(y)) if x != y => return x.cmp(y),
            (Some(_), None) => return Ordering::Greater,
            (None, Some(_)) => return Ordering::Less,
            _ => {}
        }
    }
    if la != lb {
        return la.cmp(&lb);
    }
    let xa: Vec<_> = sa.map(suffix_rank).collect();
    let xb: Vec<_> = sb.map(suffix_rank).collect();
    for i in 0..xa.len().max(xb.len()) {
        let x = xa.get(i).copied().unwrap_or((0, 0));
        let y = xb.get(i).copied().unwrap_or((0, 0));
        if x != y {
            return x.cmp(&y);
        }
    }
    ra.cmp(&rb)
}

/// `major.minor` of a version (`6.18.64_p1` -> (6, 18)).
pub fn major_minor(v: &str) -> (u64, u64) {
    let mut it = v
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().unwrap_or(0));
    (it.next().unwrap_or(0), it.next().unwrap_or(0))
}

/// Write (or with `None` remove) `/etc/portage/<kind>/<name>`. `kind` is a
/// directory Portage reads (`package.mask`, `package.accept_keywords`,
/// `package.use`); a host where it is a plain file gets a clear error
/// instead of a silently ignored pin.
pub fn write_pin(root: &Path, kind: &str, name: &str, content: Option<&str>) -> Result<(), String> {
    let dir = root.join("etc/portage").join(kind);
    let path = dir.join(name);
    match content {
        None => match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        },
        Some(text) => {
            if dir.is_file() {
                return Err(format!(
                    "{} is a file, not a directory: move its lines into {}/ (any file name) so Horcrux can add its own",
                    dir.display(),
                    dir.display()
                ));
            }
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            let tmp = dir.join(format!(".{name}.tmp"));
            std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
            std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))
        }
    }
}

pub fn read_pin(root: &Path, kind: &str, name: &str) -> Option<String> {
    std::fs::read_to_string(root.join("etc/portage").join(kind).join(name)).ok()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn ebuild(root: &Path, repo: &str, atom: &str, ver: &str, body: &str) {
        let (_, pn) = atom.split_once('/').unwrap();
        let d = root.join("var/db/repos").join(repo).join(atom);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::create_dir_all(root.join("var/db/repos").join(repo).join("profiles")).unwrap();
        std::fs::write(
            root.join("var/db/repos")
                .join(repo)
                .join("profiles/repo_name"),
            format!("{repo}\n"),
        )
        .unwrap();
        std::fs::write(d.join(format!("{pn}-{ver}.ebuild")), body).unwrap();
    }

    pub fn vdb(root: &Path, atom: &str, ver: &str, ebuild_body: &str) {
        let (cat, pn) = atom.split_once('/').unwrap();
        let d = root
            .join("var/db/pkg")
            .join(cat)
            .join(format!("{pn}-{ver}"));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(format!("{pn}-{ver}.ebuild")), ebuild_body).unwrap();
    }

    #[test]
    fn versions_compare_like_portage() {
        use Ordering::*;
        assert_eq!(vercmp("6.18.65", "6.18.64_p1"), Greater);
        assert_eq!(vercmp("6.18.64_p1", "6.18.64"), Greater);
        assert_eq!(vercmp("7.3.0_rc1", "7.3.0"), Less);
        assert_eq!(vercmp("20.2.4-r3", "20.2.4"), Greater);
        assert_eq!(vercmp("20.2.10", "20.2.9"), Greater);
        assert_eq!(vercmp("21.2.0", "20.2.5"), Greater);
        assert_eq!(vercmp("1.2", "1.2.0"), Less);
        assert_eq!(vercmp("1.2a", "1.2"), Greater);
        assert_eq!(vercmp("2.4.4", "2.4.4"), Equal);
        assert_eq!(major_minor("6.18.64_p1"), (6, 18));
        assert_eq!(major_minor("7.2"), (7, 2));
    }

    #[test]
    fn ebuilds_carry_keywords_and_sort_newest_first() {
        let t = tempfile::tempdir().unwrap();
        ebuild(
            t.path(),
            "gentoo",
            "sys-cluster/ceph",
            "20.2.5",
            "EAPI=8\nKEYWORDS=\"~amd64 ~arm64\"\n",
        );
        ebuild(
            t.path(),
            "gentoo",
            "sys-cluster/ceph",
            "20.1.1-r1",
            "KEYWORDS=\"amd64 ~arm64 ppc64\"\n",
        );
        ebuild(
            t.path(),
            "horcrux",
            "sys-cluster/ceph",
            "21.2.0",
            "KEYWORDS=\"~amd64\" # ported\n",
        );
        let e = ebuilds(t.path(), "sys-cluster/ceph");
        assert_eq!(
            e.iter().map(|x| x.version.as_str()).collect::<Vec<_>>(),
            ["21.2.0", "20.2.5", "20.1.1-r1"]
        );
        assert_eq!(e[0].repo, "horcrux");
        assert_eq!(keyword_for(&e[0].keywords, "amd64"), Keyword::Testing);
        assert_eq!(keyword_for(&e[0].keywords, "arm64"), Keyword::Missing);
        assert_eq!(keyword_for(&e[2].keywords, "amd64"), Keyword::Stable);
    }

    #[test]
    fn pins_are_written_and_removed() {
        let t = tempfile::tempdir().unwrap();
        write_pin(t.path(), "package.mask", "horcrux-x", Some(">=a/b-2\n")).unwrap();
        assert_eq!(
            read_pin(t.path(), "package.mask", "horcrux-x").unwrap(),
            ">=a/b-2\n"
        );
        write_pin(t.path(), "package.mask", "horcrux-x", None).unwrap();
        assert!(read_pin(t.path(), "package.mask", "horcrux-x").is_none());
        write_pin(t.path(), "package.mask", "horcrux-x", None).unwrap();
        std::fs::write(t.path().join("etc/portage/package.use"), "x/y foo\n").unwrap();
        let e = write_pin(t.path(), "package.use", "horcrux-x", Some("a/b c\n")).unwrap_err();
        assert!(e.contains("is a file"), "{e}");
    }

    #[test]
    fn vdb_versions_and_ebuild_vars() {
        let t = tempfile::tempdir().unwrap();
        vdb(
            t.path(),
            "sys-fs/zfs-kmod",
            "2.3.9",
            "MODULES_KERNEL_MAX=7.2\nMODULES_KERNEL_MIN=4.18\n",
        );
        vdb(t.path(), "sys-fs/zfs", "2.3.9", "");
        assert_eq!(installed(t.path(), "sys-fs/zfs-kmod"), ["2.3.9"]);
        let txt = installed_ebuild_text(t.path(), "sys-fs/zfs-kmod", "2.3.9").unwrap();
        assert_eq!(
            ebuild_var(&txt, "MODULES_KERNEL_MAX").as_deref(),
            Some("7.2")
        );
        assert!(installed(t.path(), "sys-fs/nothing").is_empty());
    }
}
