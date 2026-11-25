//! Third-party storage plugins: discovery, validation and the stdio runner.
//!
//! A plugin is an executable in the plugin directory (setting
//! `storage_plugins.plugin_dir`, default `/usr/libexec/horcrux/storage-plugins`)
//! whose file name is its id. Every operation starts the executable once,
//! writes one JSON request to its stdin and reads one JSON response from its
//! stdout (protocol: crate `horcrux-storage-plugin-sdk`, docs/storage-plugins.md).
//! Nothing a plugin does runs inside the API process: a crash, hang or flood
//! of output only fails that call (the process is killed at the operation's
//! timeout, its output is capped).
use horcrux_common::storage_plugins::StoragePluginSettings;
use horcrux_common::{Error, Result};
pub use horcrux_storage_plugin_sdk as sdk;
use sdk::{ErrorCode, Op, PluginDescription, PoolContext, Request, Response, VolumeArgs};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

/// Protocol versions this manager speaks.
pub const SUPPORTED_API_VERSIONS: &[u32] = &[1];

/// Stored settings (`<data_dir>/storage-plugins.json`, written by the API and
/// declarative apply; wins over config.toml `[storage_plugins]`).
pub mod settings {
    use super::StoragePluginSettings;
    use std::path::PathBuf;
    use std::sync::RwLock;

    static CURRENT: RwLock<Option<StoragePluginSettings>> = RwLock::new(None);

    /// Where API/declarative changes are kept (`HORCRUX_STORAGE_PLUGIN_SETTINGS`
    /// overrides the path).
    pub fn store_path() -> PathBuf {
        if let Ok(p) = std::env::var("HORCRUX_STORAGE_PLUGIN_SETTINGS") {
            if !p.trim().is_empty() {
                return PathBuf::from(p);
            }
        }
        horcrux_core::config::HorcruxConfig::load()
            .paths
            .data_dir
            .join("storage-plugins.json")
    }

    fn load_uncached() -> StoragePluginSettings {
        let path = store_path();
        if let Ok(bytes) = std::fs::read(&path) {
            match serde_json::from_slice::<StoragePluginSettings>(&bytes) {
                Ok(s) if s.validate().is_ok() => return s,
                _ => tracing::warn!(
                    "{} is not valid storage plugin settings; using config.toml [storage_plugins]",
                    path.display()
                ),
            }
        }
        horcrux_core::config::HorcruxConfig::load().storage_plugins
    }

    /// The node's effective settings.
    pub fn get() -> StoragePluginSettings {
        if let Ok(g) = CURRENT.read() {
            if let Some(s) = g.as_ref() {
                return s.clone();
            }
        }
        let s = load_uncached();
        if let Ok(mut g) = CURRENT.write() {
            *g = Some(s.clone());
        }
        s
    }

    /// Validate, persist (atomically, mode 0600) and activate.
    pub fn set(s: StoragePluginSettings) -> Result<StoragePluginSettings, String> {
        s.validate()?;
        let path = store_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_vec_pretty(&s).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, json).map_err(|e| format!("{}: {e}", tmp.display()))?;
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))?;
        if let Ok(mut g) = CURRENT.write() {
            *g = Some(s.clone());
        }
        Ok(s)
    }

    /// Forget the cached value.
    pub fn reload() -> StoragePluginSettings {
        if let Ok(mut g) = CURRENT.write() {
            *g = None;
        }
        get()
    }
}

/// One plugin found in the plugin directory (`GET /api/storage/plugins`).
#[derive(Debug, Clone, Serialize)]
pub struct PluginInfo {
    /// File name (= plugin id when usable).
    pub id: String,
    pub path: String,
    /// True when the plugin can be used: it passed the ownership check,
    /// answered `describe` with a valid schema and a supported protocol.
    pub usable: bool,
    /// Why it can't be used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<PluginDescription>,
}

/// The plugin directory listing plus what the manager supports.
#[derive(Debug, Clone, Serialize)]
pub struct PluginList {
    pub plugin_dir: String,
    /// False when the plugin directory does not exist (no plugin installed).
    pub plugin_dir_exists: bool,
    pub supported_api_versions: Vec<u32>,
    pub plugins: Vec<PluginInfo>,
}

/// Runs plugins. Holds a describe cache keyed by the executable's path,
/// size and mtime (a package upgrade changes both, so it is re-read).
pub struct PluginHost {
    fixed: Option<StoragePluginSettings>,
    cache: Mutex<HashMap<PathBuf, (u64, SystemTime, PluginDescription)>>,
}

impl Default for PluginHost {
    fn default() -> Self {
        Self::new()
    }
}

fn op_timeout(s: &StoragePluginSettings, op: Op) -> (u32, &'static str) {
    let t = &s.timeouts;
    match op {
        Op::Describe => (t.describe_secs, "describe_secs"),
        Op::Activate => (t.activate_secs, "activate_secs"),
        Op::Deactivate => (t.deactivate_secs, "deactivate_secs"),
        Op::Status => (t.status_secs, "status_secs"),
        Op::ListVolumes | Op::ListSnapshots => (t.list_secs, "list_secs"),
        Op::CreateVolume => (t.create_secs, "create_secs"),
        Op::DeleteVolume => (t.delete_secs, "delete_secs"),
        Op::ResizeVolume => (t.resize_secs, "resize_secs"),
        Op::CreateSnapshot | Op::DeleteSnapshot | Op::RollbackSnapshot => {
            (t.snapshot_secs, "snapshot_secs")
        }
        Op::VolumePath => (t.path_secs, "path_secs"),
        Op::MapVolume | Op::UnmapVolume => (t.map_secs, "map_secs"),
    }
}

/// Map a structured plugin failure to the manager's error type.
pub fn plugin_error(id: &str, op: Op, e: sdk::PluginError) -> Error {
    let msg = format!("storage plugin {id}: {}: {}", op.as_str(), e.message);
    let with_hint = if e.hint.is_empty() {
        msg.clone()
    } else {
        format!("{msg} ({})", e.hint)
    };
    match e.code {
        ErrorCode::Invalid | ErrorCode::Unsupported => Error::InvalidConfig(with_hint),
        ErrorCode::NotFound => Error::NotFound(with_hint),
        ErrorCode::Exists => Error::AlreadyExists(with_hint),
        ErrorCode::Unavailable => Error::NodeLacks {
            reason: "storage_plugin_unavailable",
            message: msg,
            hint: if e.hint.is_empty() {
                "check the storage the pool points at and the tools the plugin needs".into()
            } else {
                e.hint
            },
        },
        ErrorCode::Failed => Error::System(with_hint),
    }
}

/// Last `n` bytes of `b` as text (stderr tail for messages).
fn tail(b: &[u8], n: usize) -> String {
    let s = &b[b.len().saturating_sub(n)..];
    String::from_utf8_lossy(s).trim().to_string()
}

/// Is `p` owned by root and not writable by group or others?
fn root_owned(p: &Path) -> std::result::Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(p).map_err(|e| format!("{}: {e}", p.display()))?;
    if m.uid() != 0 {
        return Err(format!("{} is not owned by root", p.display()));
    }
    if m.mode() & 0o022 != 0 {
        return Err(format!(
            "{} is writable by group or others (mode {:o})",
            p.display(),
            m.mode() & 0o7777
        ));
    }
    Ok(())
}

impl PluginHost {
    pub fn new() -> Self {
        Self {
            fixed: None,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// A host with fixed settings instead of the node's (tests).
    pub fn with_settings(s: StoragePluginSettings) -> Self {
        Self {
            fixed: Some(s),
            cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn settings(&self) -> StoragePluginSettings {
        self.fixed.clone().unwrap_or_else(settings::get)
    }

    /// Per-pool working directory.
    pub fn pool_state_dir(&self, pool_id: &str) -> PathBuf {
        let s = self.settings();
        let base = if s.state_dir.is_empty() {
            horcrux_core::config::HorcruxConfig::load()
                .paths
                .data_dir
                .join("storage-plugins")
        } else {
            PathBuf::from(&s.state_dir)
        };
        base.join(pool_id)
    }

    /// The executable of plugin `id`, checked (exists, executable, owner).
    fn executable(&self, s: &StoragePluginSettings, id: &str) -> Result<PathBuf> {
        if !sdk::valid_plugin_id(id) {
            return Err(Error::InvalidConfig(format!(
                "{id:?} is not a valid storage plugin id ([a-z0-9][a-z0-9-]{{0,31}})"
            )));
        }
        let dir = Path::new(&s.plugin_dir);
        let path = dir.join(id);
        let missing = || Error::NodeLacks {
            reason: "storage_plugin_missing",
            message: format!(
                "storage plugin {id} is not installed on this node ({} not found)",
                path.display()
            ),
            hint: format!(
                "install the plugin's package (it puts an executable named {id} in {})",
                s.plugin_dir
            ),
        };
        let meta = std::fs::metadata(&path).map_err(|_| missing())?;
        use std::os::unix::fs::PermissionsExt;
        if !meta.is_file() || meta.permissions().mode() & 0o111 == 0 {
            return Err(missing());
        }
        if s.require_root_owned {
            let bad = |m: String| Error::NodeLacks {
                reason: "storage_plugin_untrusted",
                message: format!("storage plugin {id} is not run: {m}"),
                hint: "plugins run as root: install them from a package (root-owned, mode 0755)"
                    .into(),
            };
            root_owned(dir).map_err(bad)?;
            root_owned(&path).map_err(bad)?;
        }
        Ok(path)
    }

    /// Run one operation of plugin `id`; returns the `result` JSON.
    pub async fn call(
        &self,
        id: &str,
        op: Op,
        pool: Option<PoolContext>,
        args: VolumeArgs,
    ) -> Result<serde_json::Value> {
        let s = self.settings();
        let exe = self.executable(&s, id)?;
        self.run(&s, id, &exe, op, pool, args).await
    }

    async fn run(
        &self,
        s: &StoragePluginSettings,
        id: &str,
        exe: &Path,
        op: Op,
        pool: Option<PoolContext>,
        args: VolumeArgs,
    ) -> Result<serde_json::Value> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let cwd = match &pool {
            Some(p) => {
                let d = PathBuf::from(&p.state_dir);
                std::fs::create_dir_all(&d).map_err(|e| {
                    Error::System(format!(
                        "storage plugin {id}: creating {}: {e}",
                        d.display()
                    ))
                })?;
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700));
                }
                d
            }
            None => PathBuf::from("/"),
        };
        let req = Request {
            api_version: sdk::API_VERSION,
            op,
            pool,
            args,
        };
        let input = serde_json::to_vec(&req)
            .map_err(|e| Error::System(format!("storage plugin request: {e}")))?;
        let (secs, key) = op_timeout(s, op);
        let limit = s.max_output_kib as usize * 1024;
        let mut child = tokio::process::Command::new(exe)
            .env_clear()
            .env(
                "PATH",
                "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
            )
            .env("LANG", "C.UTF-8")
            .env("HORCRUX_STORAGE_PLUGIN_API", sdk::API_VERSION.to_string())
            .current_dir(&cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                Error::System(format!(
                    "storage plugin {id}: starting {}: {e}",
                    exe.display()
                ))
            })?;
        let mut stdin = child.stdin.take();
        let mut stdout = child.stdout.take();
        // stderr is drained on its own task (keeping the last 64 KiB), so a
        // chatty plugin never blocks on a full pipe.
        let err_task = child.stderr.take().map(|mut r| {
            tokio::spawn(async move {
                let mut keep = Vec::new();
                let mut buf = [0u8; 8192];
                loop {
                    match r.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            keep.extend_from_slice(&buf[..n]);
                            if keep.len() > 65536 {
                                let cut = keep.len() - 65536;
                                keep.drain(..cut);
                            }
                        }
                    }
                }
                keep
            })
        });
        let work = async {
            if let Some(mut w) = stdin.take() {
                // a plugin that exits without reading is fine (EPIPE)
                let _ = w.write_all(&input).await;
                let _ = w.shutdown().await;
            }
            let mut out = Vec::new();
            if let Some(r) = stdout.as_mut() {
                let _ = r.take(limit as u64 + 1).read_to_end(&mut out).await;
            }
            if out.len() > limit {
                // over the cap: stop it instead of waiting for it to finish
                let _ = child.start_kill();
            }
            let status = child.wait().await;
            (out, status)
        };
        let timed = tokio::time::timeout(Duration::from_secs(secs as u64), work).await;
        let err = match (&timed, err_task) {
            (Ok(_), Some(t)) => {
                let abort = t.abort_handle();
                match tokio::time::timeout(Duration::from_secs(2), t).await {
                    Ok(Ok(v)) => v,
                    _ => {
                        abort.abort();
                        Vec::new()
                    }
                }
            }
            (Err(_), Some(t)) => {
                t.abort();
                Vec::new()
            }
            (_, None) => Vec::new(),
        };
        let (out, status) = match timed {
            Ok(r) => r,
            Err(_) => {
                // the child is killed on drop
                return Err(Error::NodeLacks {
                    reason: "storage_plugin_timeout",
                    message: format!(
                        "storage plugin {id}: {} did not finish within {secs} s",
                        op.as_str()
                    ),
                    hint: format!(
                        "check the storage the pool points at; the limit is the \
                         storage_plugins.timeouts.{key} setting"
                    ),
                });
            }
        };
        if !err.is_empty() {
            tracing::debug!(
                "storage plugin {id} {}: stderr: {}",
                op.as_str(),
                tail(&err, 4096)
            );
        }
        if out.len() > limit {
            return Err(Error::System(format!(
                "storage plugin {id}: {} answered more than {} KiB (setting \
                 storage_plugins.max_output_kib)",
                op.as_str(),
                s.max_output_kib
            )));
        }
        let resp: Response = match serde_json::from_slice(&out) {
            Ok(r) => r,
            Err(e) => {
                let code = status
                    .ok()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "unknown status".into());
                return Err(Error::System(format!(
                    "storage plugin {id}: {} gave no valid answer ({e}; {code}){}",
                    op.as_str(),
                    if err.is_empty() {
                        String::new()
                    } else {
                        format!(": {}", tail(&err, 600))
                    }
                )));
            }
        };
        if resp.ok {
            Ok(resp.result.unwrap_or(serde_json::Value::Null))
        } else {
            Err(plugin_error(
                id,
                op,
                resp.error
                    .unwrap_or_else(|| sdk::PluginError::failed("failed without an error message")),
            ))
        }
    }

    /// Typed call.
    pub async fn call_as<T: serde::de::DeserializeOwned>(
        &self,
        id: &str,
        op: Op,
        pool: Option<PoolContext>,
        args: VolumeArgs,
    ) -> Result<T> {
        let v = self.call(id, op, pool, args).await?;
        serde_json::from_value(v).map_err(|e| {
            Error::System(format!(
                "storage plugin {id}: {} answer has the wrong shape: {e}",
                op.as_str()
            ))
        })
    }

    /// `describe` of plugin `id`, checked: id matches the file name, protocol
    /// supported, schema valid. Cached until the executable changes.
    pub async fn describe(&self, id: &str) -> Result<PluginDescription> {
        let s = self.settings();
        let exe = self.executable(&s, id)?;
        self.describe_exe(&s, id, &exe).await
    }

    async fn describe_exe(
        &self,
        s: &StoragePluginSettings,
        id: &str,
        exe: &Path,
    ) -> Result<PluginDescription> {
        let stamp = std::fs::metadata(exe)
            .ok()
            .map(|m| (m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH)));
        if let (Some((len, mtime)), Ok(c)) = (stamp, self.cache.lock()) {
            if let Some((l, m, d)) = c.get(exe) {
                if *l == len && *m == mtime {
                    return Ok(d.clone());
                }
            }
        }
        let v = self
            .run(s, id, exe, Op::Describe, None, VolumeArgs::default())
            .await?;
        let d: PluginDescription = serde_json::from_value(v).map_err(|e| {
            Error::System(format!(
                "storage plugin {id}: describe answer has the wrong shape: {e}"
            ))
        })?;
        let refuse = |m: String| Error::NodeLacks {
            reason: "storage_plugin_incompatible",
            message: format!("storage plugin {id} can't be used: {m}"),
            hint: "update the plugin package (or Horcrux) so both speak the same protocol".into(),
        };
        if d.id != id {
            return Err(refuse(format!(
                "it calls itself {:?} but is installed as {id:?}",
                d.id
            )));
        }
        if !SUPPORTED_API_VERSIONS.contains(&d.api_version) {
            return Err(refuse(format!(
                "it speaks storage plugin protocol {} and this manager speaks {:?}",
                d.api_version, SUPPORTED_API_VERSIONS
            )));
        }
        sdk::check_schema(&d.schema).map_err(|m| refuse(format!("its config schema: {m}")))?;
        if let (Some((len, mtime)), Ok(mut c)) = (stamp, self.cache.lock()) {
            c.insert(exe.to_path_buf(), (len, mtime, d.clone()));
        }
        Ok(d)
    }

    /// Every executable in the plugin directory, described.
    pub async fn list(&self) -> PluginList {
        let s = self.settings();
        let dir = Path::new(&s.plugin_dir);
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| {
                        use std::os::unix::fs::PermissionsExt;
                        e.metadata()
                            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                            .unwrap_or(false)
                    })
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        let mut plugins = Vec::new();
        for id in names {
            let path = dir.join(&id).to_string_lossy().to_string();
            let r = match self.executable(&s, &id) {
                Ok(exe) => self.describe_exe(&s, &id, &exe).await,
                Err(e) => Err(e),
            };
            plugins.push(match r {
                Ok(d) => PluginInfo {
                    id,
                    path,
                    usable: true,
                    error: None,
                    description: Some(d),
                },
                Err(e) => PluginInfo {
                    id,
                    path,
                    usable: false,
                    error: Some(e.to_string()),
                    description: None,
                },
            });
        }
        PluginList {
            plugin_dir: s.plugin_dir.clone(),
            plugin_dir_exists: dir.is_dir(),
            supported_api_versions: SUPPORTED_API_VERSIONS.to_vec(),
            plugins,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A plugin directory with shell-script plugins (tests run as the build
    /// user, so the root-ownership check is off).
    pub(crate) fn host_with(scripts: &[(&str, &str)]) -> (PluginHost, PathBuf) {
        let d = std::env::temp_dir().join(format!(
            "hx-plugins-{}-{}",
            std::process::id(),
            rand_suffix()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("bin")).unwrap();
        for (name, body) in scripts {
            let f = d.join("bin").join(name);
            std::fs::write(&f, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let s = StoragePluginSettings {
            plugin_dir: d.join("bin").to_string_lossy().to_string(),
            state_dir: d.join("state").to_string_lossy().to_string(),
            require_root_owned: false,
            ..Default::default()
        };
        (PluginHost::with_settings(s), d)
    }

    fn rand_suffix() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        N.fetch_add(1, Ordering::SeqCst)
    }

    const DESCRIBE: &str = r#"cat >/dev/null; echo '{"ok":true,"result":{"id":"good","name":"Good","version":"1","api_version":1,"schema":[{"key":"port","type":"int","label":"Port","min":1,"max":10}]}}'"#;

    #[tokio::test]
    async fn describe_list_and_refusals() {
        let (h, d) = host_with(&[
            ("good", DESCRIBE),
            (
                "liar",
                r#"cat >/dev/null; echo '{"ok":true,"result":{"id":"other","name":"x","version":"1","api_version":1}}'"#,
            ),
            (
                "future",
                r#"cat >/dev/null; echo '{"ok":true,"result":{"id":"future","name":"x","version":"1","api_version":99}}'"#,
            ),
            (
                "badschema",
                r#"cat >/dev/null; echo '{"ok":true,"result":{"id":"badschema","name":"x","version":"1","api_version":1,"schema":[{"key":"e","type":"enum","label":"E"}]}}'"#,
            ),
            (
                "garbage",
                "cat >/dev/null; echo not json; echo oops >&2; exit 3",
            ),
        ]);
        let d1 = h.describe("good").await.unwrap();
        assert_eq!(d1.schema[0].max, Some(10));
        let l = h.list().await;
        assert!(l.plugin_dir_exists);
        let by: HashMap<&str, &PluginInfo> = l.plugins.iter().map(|p| (p.id.as_str(), p)).collect();
        assert!(by["good"].usable);
        assert!(by["liar"]
            .error
            .as_deref()
            .unwrap()
            .contains("calls itself"));
        assert!(by["future"]
            .error
            .as_deref()
            .unwrap()
            .contains("protocol 99"));
        assert!(by["badschema"]
            .error
            .as_deref()
            .unwrap()
            .contains("no options"));
        let g = by["garbage"].error.as_deref().unwrap();
        assert!(g.contains("no valid answer") && g.contains("oops"), "{g}");
        // missing plugin: a structured 424, not a 500
        match h.describe("nope").await {
            Err(Error::NodeLacks { reason, .. }) => assert_eq!(reason, "storage_plugin_missing"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            h.describe("../etc/passwd").await,
            Err(Error::InvalidConfig(_))
        ));
        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn errors_are_structured_and_hung_plugins_are_killed() {
        let (mut h, d) = host_with(&[
            (
                "err",
                r#"cat >/dev/null; echo '{"ok":false,"error":{"code":"unavailable","message":"no fuse","hint":"load fuse"}}'"#,
            ),
            ("hang", "sleep 30"),
            ("flood", "cat >/dev/null; yes x | head -c 3000000"),
        ]);
        let pool = || {
            Some(PoolContext {
                id: "p1".into(),
                name: "p1".into(),
                state_dir: d.join("state/p1").to_string_lossy().to_string(),
                ..Default::default()
            })
        };
        match h
            .call("err", Op::Status, pool(), VolumeArgs::default())
            .await
        {
            Err(Error::NodeLacks {
                reason,
                message,
                hint,
            }) => {
                assert_eq!(reason, "storage_plugin_unavailable");
                assert!(message.contains("no fuse"));
                assert_eq!(hint, "load fuse");
            }
            other => panic!("{other:?}"),
        }
        // the pool's state dir was created, mode 0700
        let m = std::fs::metadata(d.join("state/p1")).unwrap();
        assert_eq!(m.permissions().mode() & 0o777, 0o700);

        let mut s = h.settings();
        s.timeouts.status_secs = 1;
        h.fixed = Some(s);
        let t0 = std::time::Instant::now();
        match h
            .call("hang", Op::Status, pool(), VolumeArgs::default())
            .await
        {
            Err(Error::NodeLacks { reason, hint, .. }) => {
                assert_eq!(reason, "storage_plugin_timeout");
                assert!(hint.contains("status_secs"));
            }
            other => panic!("{other:?}"),
        }
        assert!(t0.elapsed() < Duration::from_secs(10));
        let e = h
            .call("flood", Op::Status, pool(), VolumeArgs::default())
            .await
            .unwrap_err();
        assert!(e.to_string().contains("max_output_kib"), "{e}");
        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn untrusted_plugins_are_not_run() {
        let (h, d) = host_with(&[("good", DESCRIBE)]);
        let mut s = h.settings();
        s.require_root_owned = true;
        let h = PluginHost::with_settings(s);
        // the test tree is not root-owned (or, as root, is group-writable)
        std::fs::set_permissions(d.join("bin/good"), std::fs::Permissions::from_mode(0o777))
            .unwrap();
        match h.describe("good").await {
            Err(Error::NodeLacks { reason, .. }) => assert_eq!(reason, "storage_plugin_untrusted"),
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn error_codes_map_to_statuses() {
        use sdk::PluginError as P;
        assert!(matches!(
            plugin_error("x", Op::CreateVolume, P::invalid("bad")),
            Error::InvalidConfig(_)
        ));
        assert!(matches!(
            plugin_error("x", Op::CreateVolume, P::exists("dup")),
            Error::AlreadyExists(_)
        ));
        assert!(matches!(
            plugin_error("x", Op::DeleteVolume, P::not_found("gone")),
            Error::NotFound(_)
        ));
        assert!(matches!(
            plugin_error("x", Op::Status, P::failed("boom")),
            Error::System(_)
        ));
    }
}
