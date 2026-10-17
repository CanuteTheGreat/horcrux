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

use axum::body::Body;
use axum::http::{header, Method, Request, Response, StatusCode};
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use tower_http::services::ServeDir;

/// Environment variable that overrides UI directory discovery.
pub const UI_DIR_ENV: &str = "HORCRUX_UI_DIR";

/// File extensions of real build assets. A request for one of these that
/// matches no file is a genuine 404 (a missing stylesheet must not be
/// answered with the HTML shell); anything else is a client-side route.
const ASSET_EXTENSIONS: &[&str] = &[
    "css", "js", "mjs", "wasm", "map", "ico", "png", "jpg", "jpeg", "gif", "svg", "webp", "woff",
    "woff2", "ttf", "json", "txt", "html",
];

/// Whether a request that matched no file under the UI directory is a
/// client-side (SPA) route that should get `index.html` with 200.
/// `/api/...` never does: an unknown API route is a 404.
pub fn serves_spa_shell(path: &str) -> bool {
    if path == "/api" || path.starts_with("/api/") {
        return false;
    }
    let last = path.rsplit('/').next().unwrap_or("");
    match last.rsplit_once('.') {
        Some((_, ext)) => !ASSET_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()),
        None => true,
    }
}

/// Fallback behind the UI's `ServeDir`.
pub type SpaFallback = tower::util::BoxCloneService<Request<Body>, Response<Body>, Infallible>;

/// Answers deep links (`/vms`, `/backup/jobs`, `/login`...) with the SPA
/// shell and **200**. This used to be `ServeDir::not_found_service`, which
/// keeps the 404 status: pages rendered, but every full page load was a 404
/// (browser console error, monitoring/crawlers see a broken site).
fn spa_shell(index: PathBuf) -> SpaFallback {
    tower::util::BoxCloneService::new(tower::service_fn(move |req: Request<Body>| {
        let index = index.clone();
        async move {
            let not_found = || {
                Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                    .body(Body::from("Not Found"))
                    .unwrap()
            };
            let page = matches!(*req.method(), Method::GET | Method::HEAD)
                && serves_spa_shell(req.uri().path());
            if !page {
                return Ok(not_found());
            }
            Ok(match tokio::fs::read(&index).await {
                Ok(html) => Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                    .header(header::CACHE_CONTROL, "no-cache")
                    .body(Body::from(html))
                    .unwrap(),
                Err(_) => not_found(),
            })
        }
    }))
}

/// The service serving the web UI: real files from `ui_dir`, the SPA shell
/// (200) for client-side routes, 404 for missing assets and API paths.
pub fn ui_service(ui_dir: &Path) -> ServeDir<SpaFallback> {
    ServeDir::new(ui_dir).fallback(spa_shell(ui_dir.join("index.html")))
}

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

    /// GET `path` through the UI service mounted the way main.rs mounts it.
    async fn get(dir: &Path, path: &str) -> (StatusCode, String, String) {
        use tower::ServiceExt;
        let app = axum::Router::new().fallback_service(ui_service(dir));
        let res = app
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let ctype = res
            .headers()
            .get(header::CONTENT_TYPE)
            .map(|v| v.to_str().unwrap().to_string())
            .unwrap_or_default();
        let body = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, ctype, String::from_utf8_lossy(&body).into_owned())
    }

    #[tokio::test]
    async fn spa_deep_links_get_the_shell_with_200() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "<html>shell</html>").unwrap();
        std::fs::write(dir.path().join("main-abc.css"), "body{}").unwrap();
        for path in [
            "/",
            "/login",
            "/vms",
            "/vms/vm-1/console",
            "/backup/jobs",
            "/monitoring/alerts",
            "/vms/my.vm",
        ] {
            let (status, ctype, body) = get(dir.path(), path).await;
            assert_eq!(status, StatusCode::OK, "{path}");
            assert!(ctype.starts_with("text/html"), "{path}: {ctype}");
            assert_eq!(body, "<html>shell</html>", "{path}");
        }
        // Real files are served as themselves.
        let (status, ctype, body) = get(dir.path(), "/main-abc.css").await;
        assert_eq!((status, body.as_str()), (StatusCode::OK, "body{}"));
        assert!(ctype.starts_with("text/css"), "{ctype}");
        // Missing assets and unknown API paths stay real 404s, not HTML.
        for path in [
            "/pkg/horcrux-ui.css",
            "/missing.wasm",
            "/favicon.ico",
            "/api/nope",
        ] {
            let (status, _, body) = get(dir.path(), path).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
            assert!(!body.contains("shell"), "{path}: {body}");
        }
    }
}
