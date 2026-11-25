//! Kernel line (`system.kernel_line`): which dist-kernel line Portage installs.
//!
//! * `default` (the default): the distribution's stable kernel, nothing pinned.
//! * `newest`: the newest kernel the repositories carry (testing keyword
//!   accepted), held at the newest line the installed OpenZFS module
//!   supports (zfs-kmod's `MODULES_KERNEL_MAX`): a ZFS root never gets a
//!   kernel without its module.
//!
//! The choice is a node setting (`<data_dir>/kernel-line.json`, declarative
//! `system.kernel_line`, the installer's `[system] kernel_line`) carried out
//! as two Portage files Horcrux owns: `package.accept_keywords/horcrux-kernel-line`
//! and `package.mask/horcrux-kernel-line`. The kernel itself arrives through
//! the normal update (`horcrux-autoupdate run`: boot-environment snapshot,
//! `emerge -uUND @world`, old kernels kept, nothing rebooted), so the boot
//! path is the one every update uses and the previous kernel stays in the
//! boot menu. The reboot is left to the administrator.
//!
//! * `GET  /api/system/kernel-line`        setting, running/installed kernels, what each line selects
//! * `PUT  /api/system/kernel-line {line}` save the setting and write the pins
//! * `POST /api/system/kernel-line/apply`  run the update now (a task)
use super::portage_tree::{self as pt, Keyword};
use super::{data_dir, rooted, sysroot, user_of, which};
use crate::api::tasks;
use crate::error::ApiError;
use crate::AppState;
use axum::{routing::get, routing::post, Extension, Json, Router};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/system/kernel-line", get(status).put(set))
        .route("/api/system/kernel-line/apply", post(apply))
}

type AuthExt = Option<Extension<crate::middleware::auth::AuthUser>>;

/// The file name of both pins (in package.accept_keywords/ and package.mask/).
pub const PIN_NAME: &str = "horcrux-kernel-line";
/// The dist-kernel packages a line applies to (binary and source-built kernels).
pub const KERNEL_PACKAGES: &[&str] = &[
    "sys-kernel/gentoo-kernel-bin",
    "sys-kernel/gentoo-kernel",
    "virtual/dist-kernel",
];
/// The package whose ebuild says which kernels its module builds against.
pub const ZFS_MODULE: &str = "sys-fs/zfs-kmod";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KernelLine {
    /// the distribution's stable kernel (no pin)
    #[default]
    Default,
    /// the newest kernel the repositories carry, capped by the ZFS module
    Newest,
}

impl KernelLine {
    pub fn as_str(self) -> &'static str {
        match self {
            KernelLine::Default => "default",
            KernelLine::Newest => "newest",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KernelLineSettings {
    #[serde(default)]
    pub line: KernelLine,
}

fn settings_path() -> std::path::PathBuf {
    data_dir().join("kernel-line.json")
}

pub fn load() -> KernelLineSettings {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Save the setting and write the pins that carry it out.
pub fn save(s: &KernelLineSettings) -> Result<KernelLineStatus, String> {
    let root = sysroot();
    let st = compute(&root, s.line);
    let (kw, mask) = pins(s.line, pt::portage_arch(), st.cap.as_ref());
    pt::write_pin(&root, "package.accept_keywords", PIN_NAME, kw.as_deref())?;
    pt::write_pin(&root, "package.mask", PIN_NAME, mask.as_deref())?;
    let p = settings_path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let text = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    std::fs::write(&p, text + "\n").map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(compute(&root, s.line))
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct KernelCap {
    /// newest `major.minor` the module supports
    pub max: String,
    /// the installed package that sets it (`sys-fs/zfs-kmod-2.3.9`)
    pub by: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct KernelLineStatus {
    pub line: KernelLine,
    pub arch: String,
    /// `uname -r`
    pub running: String,
    /// kernel trees in /lib/modules, newest first
    pub installed: Vec<String>,
    /// the dist-kernel package this node uses
    pub provider: String,
    pub provider_installed: Option<String>,
    /// the version each line selects (`None`: the repositories carry none)
    pub default_version: Option<String>,
    pub newest_version: Option<String>,
    /// newest version in the repositories, before the ZFS cap
    pub newest_available: Option<String>,
    pub cap: Option<KernelCap>,
    /// why `newest_available` is not offered (ZFS module limit)
    pub held_back: Option<String>,
    /// the selected line's version is not installed yet: run the update
    pub update_pending: Option<String>,
    /// the pin files match the setting
    pub pins_in_sync: bool,
    /// a newer kernel is installed than the one running
    pub reboot_required: bool,
}

/// The pin files for a line: (accept_keywords, mask). `default` writes none.
pub fn pins(line: KernelLine, arch: &str, cap: Option<&KernelCap>) -> (Option<String>, Option<String>) {
    match line {
        KernelLine::Default => (None, None),
        KernelLine::Newest => {
            let head = "# Written by Horcrux: system.kernel_line = newest (System > Packages > Kernel line).\n\
                        # Edit the setting, not this file: Horcrux rewrites it.\n";
            let kw = KERNEL_PACKAGES
                .iter()
                .map(|p| format!("{p} ~{arch}\n"))
                .collect::<String>();
            let mask = cap.and_then(|c| {
                let (maj, min) = pt::major_minor(&c.max);
                (maj > 0).then(|| {
                    format!(
                        "{head}# {} builds against kernels up to {}: newer kernel lines wait for it.\n{}",
                        c.by,
                        c.max,
                        KERNEL_PACKAGES
                            .iter()
                            .map(|p| format!(">={p}-{maj}.{}\n", min + 1))
                            .collect::<String>()
                    )
                })
            });
            (Some(format!("{head}{kw}")), mask)
        }
    }
}

fn within(v: &str, cap: Option<&KernelCap>) -> bool {
    cap.is_none_or(|c| pt::major_minor(v) <= pt::major_minor(&c.max))
}

pub fn zfs_cap(root: &Path) -> Option<KernelCap> {
    let ver = pt::installed(root, ZFS_MODULE).into_iter().next()?;
    let text = pt::installed_ebuild_text(root, ZFS_MODULE, &ver)?;
    let max = pt::ebuild_var(&text, "MODULES_KERNEL_MAX")?;
    Some(KernelCap {
        max,
        by: format!("{ZFS_MODULE}-{ver}"),
    })
}

fn installed_trees(root: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(root.join("lib/modules"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with(|c: char| c.is_ascii_digit()))
        .collect();
    v.sort_by(|a, b| pt::vercmp(&kver(b), &kver(a)));
    v
}

/// `6.18.64-gentoo-dist` -> `6.18.64` (for ordering kernel trees).
fn kver(release: &str) -> String {
    release.split('-').next().unwrap_or(release).to_string()
}

pub fn compute(root: &Path, line: KernelLine) -> KernelLineStatus {
    let arch = pt::portage_arch();
    let provider = KERNEL_PACKAGES[..2]
        .iter()
        .find(|p| !pt::installed(root, p).is_empty())
        .copied()
        .unwrap_or(KERNEL_PACKAGES[0]);
    let provider_installed = pt::installed(root, provider).into_iter().next();
    let cap = zfs_cap(root);
    let eb = pt::ebuilds(root, provider);
    let default_version = eb
        .iter()
        .find(|e| pt::keyword_for(&e.keywords, arch) == Keyword::Stable)
        .map(|e| e.version.clone());
    let offered: Vec<_> = eb
        .iter()
        .filter(|e| pt::keyword_for(&e.keywords, arch) != Keyword::Missing)
        .collect();
    let newest_available = offered.first().map(|e| e.version.clone());
    let newest_version = offered
        .iter()
        .find(|e| within(&e.version, cap.as_ref()))
        .map(|e| e.version.clone());
    let held_back = match (&newest_available, &newest_version, &cap) {
        (Some(a), n, Some(c)) if Some(a) != n.as_ref() => Some(format!(
            "{a} is available, but {} builds against kernels up to {}: the newest line follows once an OpenZFS update supports it",
            c.by, c.max
        )),
        _ => None,
    };
    let selected = match line {
        KernelLine::Default => default_version.clone(),
        KernelLine::Newest => newest_version.clone(),
    };
    let update_pending = selected.clone().filter(|s| {
        provider_installed
            .as_deref()
            .is_none_or(|i| pt::vercmp(i, s) == std::cmp::Ordering::Less)
    });
    let (kw, mask) = pins(line, arch, cap.as_ref());
    let pins_in_sync = pt::read_pin(root, "package.accept_keywords", PIN_NAME) == kw
        && pt::read_pin(root, "package.mask", PIN_NAME) == mask;
    let running = std::fs::read_to_string(root.join("proc/sys/kernel/osrelease"))
        .or_else(|_| std::fs::read_to_string("/proc/sys/kernel/osrelease"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let installed = installed_trees(root);
    let reboot_required = installed
        .first()
        .is_some_and(|n| !running.is_empty() && *n != running);
    KernelLineStatus {
        line,
        arch: arch.into(),
        running,
        installed,
        provider: provider.into(),
        provider_installed,
        default_version,
        newest_version,
        newest_available,
        cap,
        held_back,
        update_pending,
        pins_in_sync,
        reboot_required,
    }
}

async fn status() -> Json<KernelLineStatus> {
    Json(compute(&sysroot(), load().line))
}

async fn set(u: AuthExt, Json(s): Json<KernelLineSettings>) -> Result<Json<KernelLineStatus>, ApiError> {
    need_portage()?;
    let r = save(&s);
    tasks::record_done(
        "kernel-line",
        s.line.as_str(),
        &user_of(&u),
        &r.as_ref()
            .map(|st| format!("kernel line: {}; selects {:?}", s.line.as_str(), match s.line {
                KernelLine::Default => &st.default_version,
                KernelLine::Newest => &st.newest_version,
            }))
            .map_err(|e| e.clone()),
    );
    r.map(Json).map_err(ApiError::Internal)
}

fn need_portage() -> Result<(), ApiError> {
    if which("emerge").is_none() || !rooted("var/db/pkg").is_dir() {
        return Err(ApiError::NotAvailable {
            reason: "portage-missing",
            message: "This node does not use Portage, so the kernel line cannot be chosen here.".into(),
            hint: "The kernel line applies to Gentoo-based Horcrux installs; on other systems the distribution's own kernel packages decide.".into(),
        });
    }
    Ok(())
}

#[derive(Serialize)]
struct TaskRef {
    upid: String,
}

async fn apply(u: AuthExt) -> Result<Json<TaskRef>, ApiError> {
    need_portage()?;
    super::packages::guard_one_at_a_time()?;
    // carry the current setting out first (a pin deleted by hand comes back)
    save(&load()).map_err(ApiError::Internal)?;
    let who = user_of(&u);
    let upid = if which("horcrux-autoupdate").is_some() {
        tasks::spawn_command("kernel-update", "kernel", &who, "horcrux-autoupdate", &["run".into()], &[])?
    } else {
        tasks::spawn_command(
            "kernel-update",
            "kernel",
            &who,
            "emerge",
            &super::packages::world_update_args(),
            &[],
        )?
    };
    Ok(Json(TaskRef { upid }))
}

#[cfg(test)]
mod tests {
    use super::super::portage_tree::tests::{ebuild, vdb};
    use super::*;

    fn tree() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        let a = pt::portage_arch();
        for (v, kw) in [
            ("7.3.6", format!("~{a}")),
            ("6.18.65", format!("~{a}")),
            ("6.18.64_p1", a.to_string()),
            ("6.12.118_p1", a.to_string()),
        ] {
            ebuild(t.path(), "gentoo", "sys-kernel/gentoo-kernel-bin", v, &format!("KEYWORDS=\"{kw}\"\n"));
        }
        vdb(t.path(), "sys-kernel/gentoo-kernel-bin", "6.18.64_p1", "");
        vdb(t.path(), ZFS_MODULE, "2.3.9", "MODULES_KERNEL_MAX=7.2\n");
        std::fs::create_dir_all(t.path().join("lib/modules/6.18.64-p1-gentoo-dist/kernel")).unwrap();
        std::fs::create_dir_all(t.path().join("proc/sys/kernel")).unwrap();
        std::fs::write(t.path().join("proc/sys/kernel/osrelease"), "6.18.64-p1-gentoo-dist\n").unwrap();
        t
    }

    #[test]
    fn default_line_is_the_stable_kernel_with_no_pins() {
        let t = tree();
        let s = compute(t.path(), KernelLine::Default);
        assert_eq!(s.default_version.as_deref(), Some("6.18.64_p1"));
        assert_eq!(s.update_pending, None);
        assert!(s.pins_in_sync);
        assert!(!s.reboot_required);
        assert_eq!(pins(KernelLine::Default, "amd64", None), (None, None));
    }

    #[test]
    fn newest_line_is_capped_by_the_zfs_module() {
        let t = tree();
        let s = compute(t.path(), KernelLine::Newest);
        assert_eq!(s.newest_available.as_deref(), Some("7.3.6"));
        assert_eq!(s.newest_version.as_deref(), Some("6.18.65"));
        assert_eq!(s.cap.as_ref().unwrap().by, "sys-fs/zfs-kmod-2.3.9");
        assert!(s.held_back.as_deref().unwrap().contains("up to 7.2"));
        assert_eq!(s.update_pending.as_deref(), Some("6.18.65"));
        assert!(!s.pins_in_sync);
        let (kw, mask) = pins(KernelLine::Newest, "amd64", s.cap.as_ref());
        let kw = kw.unwrap();
        assert!(kw.contains("sys-kernel/gentoo-kernel-bin ~amd64\n"));
        assert!(kw.contains("virtual/dist-kernel ~amd64\n"));
        let mask = mask.unwrap();
        assert!(mask.contains(">=sys-kernel/gentoo-kernel-bin-7.3\n"), "{mask}");
        assert!(mask.contains(">=virtual/dist-kernel-7.3\n"));
    }

    #[test]
    fn without_zfs_newest_is_the_newest_kernel() {
        let t = tree();
        std::fs::remove_dir_all(t.path().join("var/db/pkg/sys-fs")).unwrap();
        let s = compute(t.path(), KernelLine::Newest);
        assert_eq!(s.newest_version.as_deref(), Some("7.3.6"));
        assert!(s.held_back.is_none());
        assert_eq!(pins(KernelLine::Newest, "amd64", None).1, None);
    }

    #[test]
    fn a_newer_installed_kernel_than_the_running_one_wants_a_reboot() {
        let t = tree();
        std::fs::create_dir_all(t.path().join("lib/modules/6.18.65-gentoo-dist/kernel")).unwrap();
        let s = compute(t.path(), KernelLine::Default);
        assert_eq!(s.installed[0], "6.18.65-gentoo-dist");
        assert!(s.reboot_required);
    }

    #[test]
    fn saving_writes_the_pins_and_switching_back_removes_them() {
        let _g = super::super::SYSROOT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let t = tree();
        let data = tempfile::tempdir().unwrap();
        std::env::set_var("HORCRUX_SYSROOT", t.path());
        std::env::set_var("HORCRUX_DATA_DIR", data.path());
        let s = save(&KernelLineSettings { line: KernelLine::Newest }).unwrap();
        assert!(s.pins_in_sync);
        assert_eq!(load().line, KernelLine::Newest);
        assert!(pt::read_pin(t.path(), "package.mask", PIN_NAME).is_some());
        let s = save(&KernelLineSettings::default()).unwrap();
        assert!(s.pins_in_sync);
        assert!(pt::read_pin(t.path(), "package.accept_keywords", PIN_NAME).is_none());
        assert!(pt::read_pin(t.path(), "package.mask", PIN_NAME).is_none());
        std::env::remove_var("HORCRUX_SYSROOT");
        std::env::remove_var("HORCRUX_DATA_DIR");
    }

    #[test]
    fn the_setting_reads_from_json() {
        let s: KernelLineSettings = serde_json::from_str(r#"{"line":"newest"}"#).unwrap();
        assert_eq!(s.line, KernelLine::Newest);
        let s: KernelLineSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.line, KernelLine::Default);
        assert!(serde_json::from_str::<KernelLineSettings>(r#"{"line":"lts"}"#).is_err());
    }
}
