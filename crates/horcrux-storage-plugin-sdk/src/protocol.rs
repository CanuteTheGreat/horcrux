//! Wire types of the storage plugin protocol (version [`crate::API_VERSION`]).
//!
//! One call = one process: the manager writes a [`Request`] to the plugin's
//! stdin and closes it, the plugin writes a [`Response`] to stdout and exits.
//! Anything a plugin prints to stderr is kept (last 4 KiB) for the manager's
//! log and error messages. Every field added in a later protocol version is
//! optional, so an older plugin keeps working (see the changelog).
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::schema::{Capabilities, Field};

/// The operations of protocol version 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    /// Who the plugin is, its config schema and capabilities. No pool.
    Describe,
    /// Make the pool usable on this node (mount, log in, ...). Called when a
    /// pool is added and again at every manager start. Must be idempotent.
    Activate,
    /// Undo `activate` (pool removed). Must be idempotent.
    Deactivate,
    /// Capacity and health of the pool.
    Status,
    /// Volumes in the pool.
    ListVolumes,
    CreateVolume,
    DeleteVolume,
    /// Grow a volume (the manager refuses shrinking before calling).
    ResizeVolume,
    ListSnapshots,
    CreateSnapshot,
    DeleteSnapshot,
    RollbackSnapshot,
    /// What QEMU opens for a volume (a file, a block device or a QEMU URL).
    VolumePath,
    /// Make a volume available as a local path (e.g. map an RBD image) and
    /// return it; plugins whose volumes are always reachable return the path.
    MapVolume,
    UnmapVolume,
}

impl Op {
    pub const ALL: [Op; 15] = [
        Op::Describe,
        Op::Activate,
        Op::Deactivate,
        Op::Status,
        Op::ListVolumes,
        Op::CreateVolume,
        Op::DeleteVolume,
        Op::ResizeVolume,
        Op::ListSnapshots,
        Op::CreateSnapshot,
        Op::DeleteSnapshot,
        Op::RollbackSnapshot,
        Op::VolumePath,
        Op::MapVolume,
        Op::UnmapVolume,
    ];

    /// The wire name (`create_volume`).
    pub fn as_str(&self) -> &'static str {
        match self {
            Op::Describe => "describe",
            Op::Activate => "activate",
            Op::Deactivate => "deactivate",
            Op::Status => "status",
            Op::ListVolumes => "list_volumes",
            Op::CreateVolume => "create_volume",
            Op::DeleteVolume => "delete_volume",
            Op::ResizeVolume => "resize_volume",
            Op::ListSnapshots => "list_snapshots",
            Op::CreateSnapshot => "create_snapshot",
            Op::DeleteSnapshot => "delete_snapshot",
            Op::RollbackSnapshot => "rollback_snapshot",
            Op::VolumePath => "volume_path",
            Op::MapVolume => "map_volume",
            Op::UnmapVolume => "unmap_volume",
        }
    }
}

/// The pool an operation is about. `config` holds the validated non-secret
/// values (defaults filled in), `secrets` the secret fields; neither carries
/// keys the plugin did not declare.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PoolContext {
    /// Stable pool id (also the last component of `state_dir`).
    pub id: String,
    /// Display name.
    pub name: String,
    #[serde(default)]
    pub config: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub secrets: BTreeMap<String, String>,
    /// A directory owned by this pool that the plugin may use for mount
    /// points, keys, caches (`<state_dir setting>/<pool id>`). The manager
    /// creates it (mode 0700) before every call.
    pub state_dir: String,
}

impl PoolContext {
    /// A string config value (`None` when unset or not a string).
    pub fn str(&self, key: &str) -> Option<&str> {
        self.config.get(key).and_then(|v| v.as_str())
    }
    pub fn int(&self, key: &str) -> Option<i64> {
        self.config.get(key).and_then(|v| v.as_i64())
    }
    pub fn bool(&self, key: &str) -> Option<bool> {
        self.config.get(key).and_then(|v| v.as_bool())
    }
    pub fn secret(&self, key: &str) -> Option<&str> {
        self.secrets.get(key).map(String::as_str)
    }
}

/// Arguments of the volume and snapshot operations (unused fields absent).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeArgs {
    /// Volume name, already checked by the manager: 1-64 of
    /// `[A-Za-z0-9._-]`, not starting with `.` or `-`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// create/resize: wanted virtual size in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// create: preferred format (`qcow2`, `raw`); the plugin may ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// snapshot operations: snapshot name (same rules as volume names).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
}

/// What the manager sends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// Protocol version of the manager's request.
    pub api_version: u32,
    pub op: Op,
    /// Absent for `describe`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<PoolContext>,
    #[serde(default)]
    pub args: VolumeArgs,
}

/// `describe` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginDescription {
    /// Must equal the executable's file name: `[a-z0-9][a-z0-9-]{0,31}`.
    pub id: String,
    /// Human name ("SSHFS").
    pub name: String,
    /// The plugin's own version.
    pub version: String,
    /// Protocol version implemented.
    pub api_version: u32,
    #[serde(default)]
    pub description: String,
    /// Config fields, in form order.
    #[serde(default)]
    pub schema: Vec<Field>,
    #[serde(default)]
    pub capabilities: Capabilities,
}

/// `activate` result.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivateResult {
    /// A local directory the pool's volumes live in (a mount point). With
    /// capability `vm_disk_dir` the manager also creates VM disks there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_dir: Option<String>,
}

/// `status` result.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusResult {
    pub active: bool,
    pub total_bytes: u64,
    pub available_bytes: u64,
    /// Why the pool is not active (shown in the UI).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// One volume (`list_volumes` items, `create_volume`/`resize_volume` result).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Volume {
    pub name: String,
    /// What a VM disk points at (same as `volume_path`).
    pub path: String,
    /// `qcow2`, `raw`, ...
    pub format: String,
    pub size_bytes: u64,
    #[serde(default)]
    pub allocated_bytes: u64,
}

/// One snapshot (`list_snapshots` items).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub name: String,
    /// Seconds since the epoch, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<i64>,
}

/// `volume_path` / `map_volume` result.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathResult {
    pub path: String,
    #[serde(default)]
    pub format: String,
}

/// Machine-readable failure class. The manager maps it to an HTTP status:
/// `invalid` 400, `not_found` 404, `exists` 409, `unsupported` 400,
/// `unavailable` 424 (the node lacks something: a tool, a kernel module, a
/// reachable server), `failed` 500.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Invalid,
    NotFound,
    Exists,
    Unsupported,
    Unavailable,
    Failed,
}

/// A structured plugin failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginError {
    pub code: ErrorCode,
    pub message: String,
    /// What the user can do about it (shown under the message).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hint: String,
}

impl PluginError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            hint: String::new(),
        }
    }
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }
    pub fn invalid(m: impl Into<String>) -> Self {
        Self::new(ErrorCode::Invalid, m)
    }
    pub fn not_found(m: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, m)
    }
    pub fn exists(m: impl Into<String>) -> Self {
        Self::new(ErrorCode::Exists, m)
    }
    pub fn unavailable(m: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unavailable, m)
    }
    pub fn failed(m: impl Into<String>) -> Self {
        Self::new(ErrorCode::Failed, m)
    }
    pub fn unsupported(op: Op) -> Self {
        Self::new(
            ErrorCode::Unsupported,
            format!("this plugin does not implement {}", op.as_str()),
        )
    }
}

impl std::fmt::Display for PluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)?;
        if !self.hint.is_empty() {
            write!(f, " ({})", self.hint)?;
        }
        Ok(())
    }
}

impl std::error::Error for PluginError {}

pub type PluginResult<T> = Result<T, PluginError>;

/// What the plugin writes. `result` is the op's result type as JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<PluginError>,
}

impl Response {
    pub fn success<T: Serialize>(v: &T) -> Self {
        match serde_json::to_value(v) {
            Ok(v) => Self {
                ok: true,
                result: Some(v),
                error: None,
            },
            Err(e) => Self::failure(PluginError::failed(format!("serializing the result: {e}"))),
        }
    }
    pub fn failure(e: PluginError) -> Self {
        Self {
            ok: false,
            result: None,
            error: Some(e),
        }
    }
}

/// Plugin ids: `[a-z0-9][a-z0-9-]{0,31}` (also the executable's file name).
pub fn valid_plugin_id(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b.len() <= 32
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// Volume and snapshot names the manager passes (and a plugin may rely on):
/// 1-64 of `[A-Za-z0-9._-]`, not starting with `.` or `-`.
pub fn valid_volume_name(n: &str) -> bool {
    let b = n.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b[0] != b'.'
        && b[0] != b'-'
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn op_wire_names_round_trip() {
        for op in Op::ALL {
            let j = serde_json::to_value(op).unwrap();
            assert_eq!(j, serde_json::json!(op.as_str()));
            assert_eq!(serde_json::from_value::<Op>(j).unwrap(), op);
        }
    }

    #[test]
    fn request_without_pool_and_args_parses() {
        let r: Request = serde_json::from_str(r#"{"api_version":1,"op":"describe"}"#).unwrap();
        assert_eq!(r.op, Op::Describe);
        assert!(r.pool.is_none());
        assert_eq!(r.args, VolumeArgs::default());
    }

    #[test]
    fn ids_and_names() {
        assert!(valid_plugin_id("sshfs"));
        assert!(valid_plugin_id("my-nas2"));
        for bad in ["", "-x", "Upper", "a/b", "a.b", &"x".repeat(33)] {
            assert!(!valid_plugin_id(bad), "{bad}");
        }
        assert!(valid_volume_name("vm-100-disk-0.qcow2"));
        for bad in ["", ".x", "-x", "a/b", "../x", &"x".repeat(65)] {
            assert!(!valid_volume_name(bad), "{bad}");
        }
    }

    #[test]
    fn error_display_has_hint() {
        let e = PluginError::unavailable("sshfs missing").with_hint("install net-fs/sshfs");
        assert_eq!(e.to_string(), "sshfs missing (install net-fs/sshfs)");
        let j = serde_json::to_value(Response::failure(e)).unwrap();
        assert_eq!(j["error"]["code"], "unavailable");
        assert_eq!(j["ok"], false);
    }
}
