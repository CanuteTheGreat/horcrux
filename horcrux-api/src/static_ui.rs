//! Locating the built web UI (trunk `dist/` output) to serve as the
//! fallback service.
//!
//! The server used to hardcode the relative path `horcrux-ui/dist`, which
//! only resolved when the process happened to run from a directory holding
//! that exact subpath. No real install looks like that: the Gentoo ebuild
//! installs the bundle to `/opt/horcrux/horcrux-ui/` (the `dist/` contents
//! directly), docs/INSTALLATION.md copies it to `/opt/horcrux/dist`, and the
//! systemd/OpenRC services don't run from either directory. On an installed
//! system every UI request therefore fell through to a missing
//! `index.html` and returned 404.

use std::path::{Path, PathBuf};

/// Environment variable that overrides UI directory discovery.
pub const UI_DIR_ENV: &str = "HORCRUX_UI_DIR";

/// Candidate locations, most specific (packaged install) first.
pub const UI_DIR_CANDIDATES: &[&str] = &[
    // Gentoo ebuild (USE=webui): dist/ installed and renamed to horcrux-ui/
    "/opt/horcrux/horcrux-ui",
    // Manual install per docs/INSTALLATION.md
    "/opt/horcrux/dist",
    // Development: `cargo run -p horcrux-api` from the workspace root
    "horcrux-api/horcrux-ui/dist",
    // Development: running from inside horcrux-api/
    "horcrux-ui/dist",
];

/// Pick the directory to serve the UI from.
///
/// An explicit `override_dir` (from `HORCRUX_UI_DIR`) always wins, even if
/// it doesn't contain an `index.html` yet, so an operator's choice is never
/// silently ignored. Otherwise the first candidate containing `index.html`
/// is used. Returns `None` when nothing usable was found.
pub fn resolve_ui_dir<F>(
    override_dir: Option<&str>,
    candidates: &[&str],
    has_index: F,
) -> Option<PathBuf>
where
    F: Fn(&Path) -> bool,
{
    if let Some(dir) = override_dir.map(str::trim).filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    candidates
        .iter()
        .map(PathBuf::from)
        .find(|dir| has_index(dir))
}

/// Resolve the UI directory from the real environment and filesystem.
///
/// Falls back to the packaged path when nothing is found (the API still
/// works; UI requests 404, and a warning is logged at startup).
pub fn ui_dir() -> (PathBuf, bool) {
    let override_dir = std::env::var(UI_DIR_ENV).ok();
    match resolve_ui_dir(override_dir.as_deref(), UI_DIR_CANDIDATES, |d| {
        d.join("index.html").is_file()
    }) {
        Some(dir) => {
            let found = dir.join("index.html").is_file();
            (dir, found)
        }
        None => (PathBuf::from(UI_DIR_CANDIDATES[0]), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_wins_even_without_index() {
        let got = resolve_ui_dir(Some("/srv/ui"), UI_DIR_CANDIDATES, |_| true);
        assert_eq!(got, Some(PathBuf::from("/srv/ui")));
        let got = resolve_ui_dir(Some("/srv/ui"), UI_DIR_CANDIDATES, |_| false);
        assert_eq!(got, Some(PathBuf::from("/srv/ui")));
    }

    #[test]
    fn blank_override_is_ignored() {
        let got = resolve_ui_dir(Some("  "), UI_DIR_CANDIDATES, |d| {
            d == Path::new("/opt/horcrux/dist")
        });
        assert_eq!(got, Some(PathBuf::from("/opt/horcrux/dist")));
    }

    #[test]
    fn ebuild_install_path_is_found() {
        // The layout the Gentoo ebuild actually produces.
        let got = resolve_ui_dir(None, UI_DIR_CANDIDATES, |d| {
            d == Path::new("/opt/horcrux/horcrux-ui")
        });
        assert_eq!(got, Some(PathBuf::from("/opt/horcrux/horcrux-ui")));
    }

    #[test]
    fn packaged_path_preferred_over_dev_path() {
        let got = resolve_ui_dir(None, UI_DIR_CANDIDATES, |_| true);
        assert_eq!(got, Some(PathBuf::from("/opt/horcrux/horcrux-ui")));
    }

    #[test]
    fn workspace_dev_path_is_found() {
        let got = resolve_ui_dir(None, UI_DIR_CANDIDATES, |d| {
            d == Path::new("horcrux-api/horcrux-ui/dist")
        });
        assert_eq!(got, Some(PathBuf::from("horcrux-api/horcrux-ui/dist")));
    }

    #[test]
    fn nothing_found() {
        assert_eq!(resolve_ui_dir(None, UI_DIR_CANDIDATES, |_| false), None);
    }
}
