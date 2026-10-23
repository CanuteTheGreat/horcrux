//! Package management: Portage on Gentoo (`/var/db/pkg` is the installed
//! database, `emerge` changes it), apt on Debian-family hosts. Reading is
//! instant; changes run as tasks (see `api/tasks.rs`) because `emerge` takes
//! minutes to hours, one at a time (Portage locks anyway).
use super::{data_dir, rooted, run, sysroot, user_of, which};
use crate::api::tasks;
use crate::error::ApiError;
use crate::AppState;
use axum::{
    extract::{Path, Query},
    routing::{get, post},
    Extension, Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path as FsPath;
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/system/packages", get(list))
        .route("/api/system/packages/search", get(search))
        .route("/api/system/packages/update", post(update_db))
        .route("/api/system/packages/upgrade-all", post(upgrade_all))
        .route("/api/system/packages/bulk-install", post(bulk_install))
        .route("/api/system/packages/:name/install", post(install))
        .route("/api/system/packages/:name/remove", post(remove))
        .route("/api/system/packages/:name/upgrade", post(upgrade))
}

type AuthExt = Option<Extension<crate::middleware::auth::AuthUser>>;
type Res<T> = Result<Json<T>, ApiError>;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PackageInfo {
    pub name: String,
    pub description: String,
    /// installed | upgradeable | not-installed
    pub status: String,
    pub installed_version: Option<String>,
    pub available_version: Option<String>,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pm {
    Portage,
    Apt,
}

pub fn detect() -> Option<Pm> {
    if which("emerge").is_some() && rooted("var/db/pkg").is_dir() {
        Some(Pm::Portage)
    } else if which("dpkg-query").is_some() && which("apt-get").is_some() {
        Some(Pm::Apt)
    } else {
        None
    }
}

fn need_pm() -> Result<Pm, ApiError> {
    detect().ok_or_else(|| {
        ApiError::ServiceUnavailable(
            "no supported package manager (Portage/emerge or apt) on this host".into(),
        )
    })
}

const PKG_TASKS: &[&str] = &[
    "package-install",
    "package-remove",
    "package-upgrade",
    "package-sync",
];

// ----------------------------------------------------------------- Portage

/// `foo-bar-1.2.3-r1` -> (`foo-bar`, `1.2.3-r1`): the version starts after the
/// last `-` that is followed by a digit.
pub fn split_pf(pf: &str) -> Option<(String, String)> {
    let b = pf.as_bytes();
    (1..b.len()).rev().find_map(|i| {
        (b[i - 1] == b'-' && b[i].is_ascii_digit() && i > 1)
            .then(|| (pf[..i - 1].to_string(), pf[i..].to_string()))
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct Installed {
    pub atom: String,
    pub version: String,
    pub description: String,
    pub size: Option<u64>,
}

pub fn portage_installed(root: &FsPath) -> Vec<Installed> {
    let mut out = Vec::new();
    for cat in std::fs::read_dir(root.join("var/db/pkg"))
        .into_iter()
        .flatten()
        .flatten()
    {
        let cat_name = cat.file_name().to_string_lossy().to_string();
        if !cat.path().is_dir() {
            continue;
        }
        for pkg in std::fs::read_dir(cat.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            let pf = pkg.file_name().to_string_lossy().to_string();
            let Some((pn, ver)) = split_pf(&pf) else {
                continue;
            };
            let rd = |f: &str| std::fs::read_to_string(pkg.path().join(f)).ok();
            out.push(Installed {
                atom: format!("{cat_name}/{pn}"),
                version: ver,
                description: rd("DESCRIPTION").unwrap_or_default().trim().to_string(),
                size: rd("SIZE").and_then(|s| s.trim().parse().ok()),
            });
        }
    }
    out.sort_by(|a, b| a.atom.cmp(&b.atom));
    out
}

/// `[ebuild     U  ] sys-apps/foo-2.0:0 [1.9]` -> (atom, new version).
pub fn parse_pretend_upgrades(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for l in text.lines() {
        let Some(rest) = l.strip_prefix("[ebuild") else {
            continue;
        };
        let Some((flags, rest)) = rest.split_once(']') else {
            continue;
        };
        if !flags.contains('U') {
            continue;
        }
        let Some(tok) = rest.split_whitespace().next() else {
            continue;
        };
        let tok = tok.split(':').next().unwrap_or(tok);
        let Some((cat, pf)) = tok.split_once('/') else {
            continue;
        };
        if let Some((pn, ver)) = split_pf(pf) {
            out.insert(format!("{cat}/{pn}"), ver);
        }
    }
    out
}

fn repo_dirs(root: &FsPath) -> Vec<std::path::PathBuf> {
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

fn ebuild_description(pkg_dir: &FsPath) -> String {
    let mut ebuilds: Vec<_> = std::fs::read_dir(pkg_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "ebuild"))
        .collect();
    ebuilds.sort();
    ebuilds
        .last()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| {
            t.lines().find_map(|l| {
                l.strip_prefix("DESCRIPTION=")
                    .map(|d| d.trim().trim_matches(['"', '\'']).to_string())
            })
        })
        .unwrap_or_default()
}

/// `category/package` atoms in the repositories that contain `query`.
pub fn portage_search(root: &FsPath, query: &str, max: usize) -> Vec<(String, String)> {
    let q = query.to_ascii_lowercase();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for repo in repo_dirs(root) {
        for cat in std::fs::read_dir(&repo).into_iter().flatten().flatten() {
            let cn = cat.file_name().to_string_lossy().to_string();
            if !cat.path().is_dir() || cn.starts_with('.') || !(cn.contains('-') || cn == "virtual")
            {
                continue;
            }
            for pkg in std::fs::read_dir(cat.path())
                .into_iter()
                .flatten()
                .flatten()
            {
                let pn = pkg.file_name().to_string_lossy().to_string();
                let atom = format!("{cn}/{pn}");
                if atom.to_ascii_lowercase().contains(&q) && seen.insert(atom.clone()) {
                    out.push((atom, ebuild_description(&pkg.path())));
                }
            }
        }
    }
    out.sort();
    out.truncate(max);
    out
}

pub fn valid_atom(a: &str) -> bool {
    let ok = |s: &str| {
        !s.is_empty()
            && !s.starts_with(['-', '.', '+'])
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '_' | '.' | '-'))
    };
    match a.split_once('/') {
        Some((c, p)) => ok(c) && ok(p) && !p.contains('/'),
        None => false,
    }
}

/// A bare name like `vim` resolves to its atom if exactly one package has it.
pub fn resolve_portage(root: &FsPath, name: &str) -> Result<String, String> {
    if name.contains('/') {
        return if valid_atom(name) {
            Ok(name.to_string())
        } else {
            Err(format!("'{name}' is not a category/package atom"))
        };
    }
    if !valid_atom(&format!("x-y/{name}")) {
        return Err(format!("'{name}' is not a package name"));
    }
    let hits: Vec<String> = portage_search(root, name, 500)
        .into_iter()
        .map(|(a, _)| a)
        .filter(|a| a.rsplit('/').next() == Some(name))
        .collect();
    match hits.len() {
        1 => Ok(hits[0].clone()),
        0 => Err(format!("no package named '{name}' in the Portage tree")),
        _ => Err(format!("'{name}' is ambiguous: {}", hits.join(", "))),
    }
}

fn pending_file() -> std::path::PathBuf {
    data_dir().join("packages").join("pending-upgrades.txt")
}

fn pending_upgrades() -> HashMap<String, String> {
    parse_pretend_upgrades(&std::fs::read_to_string(pending_file()).unwrap_or_default())
}

// --------------------------------------------------------------------- apt

pub fn parse_dpkg_query(text: &str) -> Vec<(String, String, String, Option<u64>)> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() < 5 || !f[0].starts_with("ii") {
                return None;
            }
            Some((
                f[1].to_string(),
                f[2].to_string(),
                f[4].to_string(),
                f[3].parse::<u64>().ok().map(|k| k * 1024),
            ))
        })
        .collect()
}

/// `apt list --upgradable` -> name -> new version.
pub fn parse_apt_upgradable(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| {
            let (name, rest) = l.split_once('/')?;
            let ver = rest.split_whitespace().nth(1)?;
            Some((name.to_string(), ver.to_string()))
        })
        .collect()
}

pub fn parse_apt_search(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| {
            l.split_once(" - ")
                .map(|(n, d)| (n.trim().to_string(), d.trim().to_string()))
        })
        .collect()
}

pub fn valid_apt_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() < 100
        && n.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && n.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '+' | '.' | '-' | ':')
        })
}

// ---------------------------------------------------------------- handlers

async fn list() -> Res<Vec<PackageInfo>> {
    match need_pm()? {
        Pm::Portage => {
            let pend = pending_upgrades();
            Ok(Json(
                portage_installed(&sysroot())
                    .into_iter()
                    .map(|p| {
                        let up = pend.get(&p.atom).cloned();
                        PackageInfo {
                            status: if up.is_some() {
                                "upgradeable"
                            } else {
                                "installed"
                            }
                            .into(),
                            available_version: up,
                            installed_version: Some(p.version),
                            description: p.description,
                            size: p.size,
                            name: p.atom,
                        }
                    })
                    .collect(),
            ))
        }
        Pm::Apt => {
            let out = run(
                "dpkg-query",
                &["-W", "-f=${db:Status-Abbrev}\t${Package}\t${Version}\t${Installed-Size}\t${binary:Summary}\n"],
            )
            .await
            .map_err(ApiError::Internal)?;
            let up = parse_apt_upgradable(
                &run("apt", &["list", "--upgradable"])
                    .await
                    .unwrap_or_default(),
            );
            Ok(Json(
                parse_dpkg_query(&out)
                    .into_iter()
                    .map(|(name, ver, desc, size)| {
                        let avail = up.get(&name).cloned();
                        PackageInfo {
                            status: if avail.is_some() {
                                "upgradeable"
                            } else {
                                "installed"
                            }
                            .into(),
                            available_version: avail,
                            installed_version: Some(ver),
                            description: desc,
                            size,
                            name,
                        }
                    })
                    .collect(),
            ))
        }
    }
}

#[derive(Deserialize)]
struct SearchQ {
    q: String,
}

async fn search(Query(q): Query<SearchQ>) -> Res<Vec<PackageInfo>> {
    let q = q.q.trim().to_string();
    if q.len() < 2
        || q.len() > 64
        || !q
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '_' | '.' | '-' | '/'))
    {
        return Err(ApiError::BadRequest(
            "search needs 2-64 letters, digits or + _ . - /".into(),
        ));
    }
    let installed: HashMap<String, String> = match need_pm()? {
        Pm::Portage => portage_installed(&sysroot())
            .into_iter()
            .map(|p| (p.atom, p.version))
            .collect(),
        Pm::Apt => parse_dpkg_query(
            &run(
                "dpkg-query",
                &["-W", "-f=${db:Status-Abbrev}\t${Package}\t${Version}\t${Installed-Size}\t${binary:Summary}\n"],
            )
            .await
            .unwrap_or_default(),
        )
        .into_iter()
        .map(|(n, v, _, _)| (n, v))
        .collect(),
    };
    let hits: Vec<(String, String)> = match need_pm()? {
        Pm::Portage => portage_search(&sysroot(), &q, 100),
        Pm::Apt => {
            let mut v = parse_apt_search(
                &run("apt-cache", &["search", "--names-only", &q])
                    .await
                    .map_err(ApiError::Internal)?,
            );
            v.truncate(100);
            v
        }
    };
    Ok(Json(
        hits.into_iter()
            .map(|(name, description)| {
                let iv = installed.get(&name).cloned();
                PackageInfo {
                    status: if iv.is_some() {
                        "installed"
                    } else {
                        "not-installed"
                    }
                    .into(),
                    installed_version: iv,
                    available_version: None,
                    size: None,
                    description,
                    name,
                }
            })
            .collect(),
    ))
}

#[derive(Serialize)]
struct TaskRef {
    upid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pending: Option<usize>,
}

fn guard_one_at_a_time() -> Result<(), ApiError> {
    if let Some(u) = tasks::any_running(PKG_TASKS) {
        return Err(ApiError::Conflict(format!(
            "a package task is already running ({u}); wait for it or stop it from the Tasks page"
        )));
    }
    Ok(())
}

fn emerge_args(extra: &[&str], atoms: &[String]) -> Vec<String> {
    let mut a: Vec<String> = ["--ask=n", "--color=n", "--nospinner"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    a.extend(extra.iter().map(|s| s.to_string()));
    a.extend(atoms.iter().cloned());
    a
}

fn apt_args(extra: &[&str], names: &[String]) -> Vec<String> {
    let mut a: Vec<String> = ["-y", "-o", "Dpkg::Options::=--force-confold"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    a.extend(extra.iter().map(|s| s.to_string()));
    a.extend(names.iter().cloned());
    a
}

async fn start(
    kind: &str,
    target: &str,
    who: &str,
    portage: Vec<String>,
    apt: (&str, Vec<String>),
) -> Res<TaskRef> {
    let pm = need_pm()?;
    guard_one_at_a_time()?;
    let upid = match pm {
        Pm::Portage => tasks::spawn_command(kind, target, who, "emerge", &portage, &[])?,
        Pm::Apt => tasks::spawn_command(
            kind,
            target,
            who,
            "apt-get",
            &{
                let mut a = vec![apt.0.to_string()];
                a.extend(apt.1);
                a
            },
            &[("DEBIAN_FRONTEND", "noninteractive")],
        )?,
    };
    Ok(Json(TaskRef {
        upid,
        pending: None,
    }))
}

/// Resolve a path/name to the manager's package identifier.
fn resolve(pm: Pm, name: &str) -> Result<String, ApiError> {
    match pm {
        Pm::Portage => resolve_portage(&sysroot(), name).map_err(ApiError::ValidationError),
        Pm::Apt => {
            if valid_apt_name(name) {
                Ok(name.to_string())
            } else {
                Err(ApiError::ValidationError(format!(
                    "'{name}' is not a package name"
                )))
            }
        }
    }
}

async fn is_installed(pm: Pm, id: &str) -> bool {
    match pm {
        Pm::Portage => portage_installed(&sysroot()).iter().any(|p| p.atom == id),
        Pm::Apt => run("dpkg-query", &["-W", "-f=${db:Status-Abbrev}", id])
            .await
            .map(|s| s.starts_with("ii"))
            .unwrap_or(false),
    }
}

async fn install(u: AuthExt, Path(name): Path<String>) -> Res<TaskRef> {
    let pm = need_pm()?;
    let id = resolve(pm, &name)?;
    start(
        "package-install",
        &id,
        &user_of(&u),
        emerge_args(&["--noreplace"], std::slice::from_ref(&id)),
        (
            "install",
            apt_args(&[], std::slice::from_ref(&id))[..].to_vec(),
        ),
    )
    .await
}

async fn bulk_install(u: AuthExt, Json(names): Json<Vec<String>>) -> Res<TaskRef> {
    let pm = need_pm()?;
    if names.is_empty() || names.len() > 50 {
        return Err(ApiError::ValidationError("give 1 to 50 packages".into()));
    }
    let ids = names
        .iter()
        .map(|n| resolve(pm, n))
        .collect::<Result<Vec<_>, _>>()?;
    start(
        "package-install",
        &ids.join(","),
        &user_of(&u),
        emerge_args(&["--noreplace"], &ids),
        ("install", apt_args(&[], &ids)),
    )
    .await
}

async fn remove(u: AuthExt, Path(name): Path<String>) -> Res<TaskRef> {
    let pm = need_pm()?;
    let id = resolve(pm, &name)?;
    if !is_installed(pm, &id).await {
        return Err(ApiError::NotFound(format!("{id} is not installed")));
    }
    // Portage: --depclean refuses to remove what other packages need.
    start(
        "package-remove",
        &id,
        &user_of(&u),
        emerge_args(&["--depclean"], std::slice::from_ref(&id)),
        ("remove", apt_args(&[], std::slice::from_ref(&id))),
    )
    .await
}

async fn upgrade(u: AuthExt, Path(name): Path<String>) -> Res<TaskRef> {
    let pm = need_pm()?;
    let id = resolve(pm, &name)?;
    if !is_installed(pm, &id).await {
        return Err(ApiError::NotFound(format!("{id} is not installed")));
    }
    start(
        "package-upgrade",
        &id,
        &user_of(&u),
        emerge_args(&["--update", "--oneshot"], std::slice::from_ref(&id)),
        (
            "install",
            apt_args(&["--only-upgrade"], std::slice::from_ref(&id)),
        ),
    )
    .await
}

async fn upgrade_all(u: AuthExt) -> Res<TaskRef> {
    let pm = need_pm()?;
    let pending = match pm {
        Pm::Portage => pending_upgrades().len(),
        Pm::Apt => parse_apt_upgradable(
            &run("apt", &["list", "--upgradable"])
                .await
                .unwrap_or_default(),
        )
        .len(),
    };
    let Json(mut t) = start(
        "package-upgrade",
        "all",
        &user_of(&u),
        emerge_args(&["--update", "--deep", "--newuse", "@world"], &[]),
        ("upgrade", apt_args(&[], &[])),
    )
    .await?;
    t.pending = Some(pending);
    Ok(Json(t))
}

/// Sync the package database, then record what is upgradable.
async fn update_db(u: AuthExt) -> Res<TaskRef> {
    let pm = need_pm()?;
    guard_one_at_a_time()?;
    let who = user_of(&u);
    let upid = match pm {
        Pm::Portage => {
            let dir = data_dir().join("packages");
            let _ = std::fs::create_dir_all(&dir);
            let out = pending_file();
            let script = format!(
                "emerge --sync --quiet && echo '--- checking @world for updates' && \
                 emerge --pretend --update --deep --newuse --color=n --nospinner --quiet @world | tee '{}'",
                out.display()
            );
            tasks::spawn_command(
                "package-sync",
                "emerge --sync",
                &who,
                "sh",
                &["-c".into(), script],
                &[],
            )?
        }
        Pm::Apt => tasks::spawn_command(
            "package-sync",
            "apt-get update",
            &who,
            "apt-get",
            &["update".into()],
            &[("DEBIAN_FRONTEND", "noninteractive")],
        )?,
    };
    Ok(Json(TaskRef {
        upid,
        pending: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pf_splits_at_the_version() {
        assert_eq!(
            split_pf("vim-9.0.1-r1"),
            Some(("vim".into(), "9.0.1-r1".into()))
        );
        assert_eq!(
            split_pf("gtk+-3.24.1"),
            Some(("gtk+".into(), "3.24.1".into()))
        );
        assert_eq!(
            split_pf("foo-2d-1.0_rc1"),
            Some(("foo-2d".into(), "1.0_rc1".into()))
        );
        assert_eq!(split_pf("noversion"), None);
    }

    #[test]
    fn pretend_output_lists_only_upgrades() {
        let t = "[ebuild     U  ] sys-apps/foo-2.0:0 [1.9] USE=\"a\"\n[ebuild  N     ] dev-libs/new-1.0\n[ebuild   R    ] x/y-1\n[ebuild     UD ] app-misc/down-1.0 [2.0]\nrandom\n";
        let u = parse_pretend_upgrades(t);
        assert_eq!(u.len(), 2);
        assert_eq!(u["sys-apps/foo"], "2.0");
        assert_eq!(u["app-misc/down"], "1.0");
    }

    #[test]
    fn atoms_are_vetted() {
        assert!(valid_atom("app-editors/vim") && valid_atom("dev-libs/gtk+"));
        for bad in [
            "vim",
            "a/b/c",
            "../x",
            "-x/y",
            "app-editors/vim; rm",
            "a b/c",
            "app-editors/--help",
        ] {
            assert!(!valid_atom(bad), "{bad}");
        }
        assert!(
            valid_apt_name("libc6")
                && valid_apt_name("g++")
                && !valid_apt_name("-y")
                && !valid_apt_name("a;b")
        );
    }

    fn tree() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        let w = |p: &str, t: &str| {
            let f = r.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, t).unwrap();
        };
        w(
            "var/db/pkg/app-editors/vim-9.0.1-r1/DESCRIPTION",
            "Vim, an improved vi\n",
        );
        w("var/db/pkg/app-editors/vim-9.0.1-r1/SIZE", "3145728\n");
        w(
            "var/db/pkg/sys-apps/busybox-1.36.1/DESCRIPTION",
            "Utilities\n",
        );
        w(
            "var/db/repos/gentoo/app-editors/vim/vim-9.0.1.ebuild",
            "EAPI=8\nDESCRIPTION=\"Vim, an improved vi-style text editor\"\n",
        );
        w(
            "var/db/repos/gentoo/app-editors/nano/nano-7.2.ebuild",
            "DESCRIPTION=\"GNU GPL'd Pico clone\"\n",
        );
        w(
            "var/db/repos/gentoo/app-misc/nano/nano-1.ebuild",
            "DESCRIPTION=\"dup name\"\n",
        );
        w("var/db/repos/gentoo/profiles/x", "");
        d
    }

    #[test]
    fn installed_packages_come_from_the_vdb() {
        let d = tree();
        let v = portage_installed(d.path());
        assert_eq!(v.len(), 2);
        assert_eq!(
            v[0],
            Installed {
                atom: "app-editors/vim".into(),
                version: "9.0.1-r1".into(),
                description: "Vim, an improved vi".into(),
                size: Some(3145728)
            }
        );
        assert_eq!(v[1].atom, "sys-apps/busybox");
    }

    #[test]
    fn tree_search_and_name_resolution() {
        let d = tree();
        let hits = portage_search(d.path(), "NANO", 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].0, "app-editors/nano");
        assert_eq!(hits[0].1, "GNU GPL'd Pico clone");
        assert!(
            portage_search(d.path(), "profiles", 10).is_empty(),
            "non-category dirs are skipped"
        );
        assert_eq!(resolve_portage(d.path(), "vim").unwrap(), "app-editors/vim");
        assert!(resolve_portage(d.path(), "nano")
            .unwrap_err()
            .contains("ambiguous"));
        assert!(resolve_portage(d.path(), "nosuch").is_err());
        assert_eq!(
            resolve_portage(d.path(), "app-misc/nano").unwrap(),
            "app-misc/nano"
        );
        assert!(resolve_portage(d.path(), "../etc").is_err());
    }

    #[test]
    fn apt_output_parses() {
        let q = "ii \tbash\t5.2\t1500\tGNU Bourne Again SHell\nrc \told\t1\t5\tgone\n";
        let v = parse_dpkg_query(q);
        assert_eq!(
            v,
            vec![(
                "bash".into(),
                "5.2".into(),
                "GNU Bourne Again SHell".into(),
                Some(1536000)
            )]
        );
        let u = parse_apt_upgradable(
            "Listing...\ncurl/noble-updates 8.5.0-2 amd64 [upgradable from: 8.4]\n",
        );
        assert_eq!(u["curl"], "8.5.0-2");
        assert_eq!(
            parse_apt_search("hello - example\n")[0],
            ("hello".into(), "example".into())
        );
    }
}
