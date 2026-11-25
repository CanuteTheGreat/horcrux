//! Running a plugin: read one request from stdin, dispatch, write one
//! response to stdout.
use crate::protocol::*;
use std::io::{Read, Write};

/// A storage plugin. Only [`StoragePlugin::describe`] is mandatory; every
/// other operation answers `unsupported` unless implemented. The manager
/// validates names, sizes and the pool config before calling.
pub trait StoragePlugin {
    fn describe(&self) -> PluginDescription;

    fn activate(&self, _pool: &PoolContext) -> PluginResult<ActivateResult> {
        Err(PluginError::unsupported(Op::Activate))
    }
    fn deactivate(&self, _pool: &PoolContext) -> PluginResult<()> {
        Ok(())
    }
    fn status(&self, _pool: &PoolContext) -> PluginResult<StatusResult> {
        Err(PluginError::unsupported(Op::Status))
    }
    fn list_volumes(&self, _pool: &PoolContext) -> PluginResult<Vec<Volume>> {
        Err(PluginError::unsupported(Op::ListVolumes))
    }
    fn create_volume(&self, _pool: &PoolContext, _args: &VolumeArgs) -> PluginResult<Volume> {
        Err(PluginError::unsupported(Op::CreateVolume))
    }
    fn delete_volume(&self, _pool: &PoolContext, _args: &VolumeArgs) -> PluginResult<()> {
        Err(PluginError::unsupported(Op::DeleteVolume))
    }
    fn resize_volume(&self, _pool: &PoolContext, _args: &VolumeArgs) -> PluginResult<Volume> {
        Err(PluginError::unsupported(Op::ResizeVolume))
    }
    fn list_snapshots(
        &self,
        _pool: &PoolContext,
        _args: &VolumeArgs,
    ) -> PluginResult<Vec<Snapshot>> {
        Err(PluginError::unsupported(Op::ListSnapshots))
    }
    fn create_snapshot(&self, _pool: &PoolContext, _args: &VolumeArgs) -> PluginResult<()> {
        Err(PluginError::unsupported(Op::CreateSnapshot))
    }
    fn delete_snapshot(&self, _pool: &PoolContext, _args: &VolumeArgs) -> PluginResult<()> {
        Err(PluginError::unsupported(Op::DeleteSnapshot))
    }
    fn rollback_snapshot(&self, _pool: &PoolContext, _args: &VolumeArgs) -> PluginResult<()> {
        Err(PluginError::unsupported(Op::RollbackSnapshot))
    }
    fn volume_path(&self, _pool: &PoolContext, _args: &VolumeArgs) -> PluginResult<PathResult> {
        Err(PluginError::unsupported(Op::VolumePath))
    }
    /// Default: the volume needs no mapping, its path is `volume_path`.
    fn map_volume(&self, pool: &PoolContext, args: &VolumeArgs) -> PluginResult<PathResult> {
        self.volume_path(pool, args)
    }
    fn unmap_volume(&self, _pool: &PoolContext, _args: &VolumeArgs) -> PluginResult<()> {
        Ok(())
    }
}

fn ok<T: serde::Serialize>(r: PluginResult<T>) -> Response {
    match r {
        Ok(v) => Response::success(&v),
        Err(e) => Response::failure(e),
    }
}

fn need_name(args: &VolumeArgs, snapshot: bool) -> PluginResult<()> {
    if !valid_volume_name(&args.name) {
        return Err(PluginError::invalid(format!(
            "invalid volume name {:?}",
            args.name
        )));
    }
    if snapshot && !args.snapshot.as_deref().is_some_and(valid_volume_name) {
        return Err(PluginError::invalid("missing or invalid snapshot name"));
    }
    Ok(())
}

/// Dispatch one request (exposed for plugin unit tests).
pub fn handle_request<P: StoragePlugin>(p: &P, req: &Request) -> Response {
    if req.api_version == 0 || (req.api_version > crate::API_VERSION && req.op != Op::Describe) {
        return Response::failure(PluginError::new(
            ErrorCode::Unsupported,
            format!(
                "protocol version {} is not supported by this plugin (speaks {})",
                req.api_version,
                crate::API_VERSION
            ),
        ));
    }
    if req.op == Op::Describe {
        return Response::success(&p.describe());
    }
    let Some(pool) = req.pool.as_ref() else {
        return Response::failure(PluginError::invalid(format!(
            "{} needs a pool",
            req.op.as_str()
        )));
    };
    let a = &req.args;
    match req.op {
        Op::Describe => unreachable!(),
        Op::Activate => ok(p.activate(pool)),
        Op::Deactivate => ok(p.deactivate(pool).map(|_| serde_json::json!({}))),
        Op::Status => ok(p.status(pool)),
        Op::ListVolumes => ok(p.list_volumes(pool)),
        Op::CreateVolume => ok(need_name(a, false).and_then(|_| {
            if a.size_bytes.unwrap_or(0) == 0 {
                return Err(PluginError::invalid("size_bytes must be > 0"));
            }
            p.create_volume(pool, a)
        })),
        Op::DeleteVolume => ok(need_name(a, false)
            .and_then(|_| p.delete_volume(pool, a))
            .map(|_| serde_json::json!({}))),
        Op::ResizeVolume => ok(need_name(a, false).and_then(|_| p.resize_volume(pool, a))),
        Op::ListSnapshots => ok(need_name(a, false).and_then(|_| p.list_snapshots(pool, a))),
        Op::CreateSnapshot => ok(need_name(a, true)
            .and_then(|_| p.create_snapshot(pool, a))
            .map(|_| serde_json::json!({}))),
        Op::DeleteSnapshot => ok(need_name(a, true)
            .and_then(|_| p.delete_snapshot(pool, a))
            .map(|_| serde_json::json!({}))),
        Op::RollbackSnapshot => ok(need_name(a, true)
            .and_then(|_| p.rollback_snapshot(pool, a))
            .map(|_| serde_json::json!({}))),
        Op::VolumePath => ok(need_name(a, false).and_then(|_| p.volume_path(pool, a))),
        Op::MapVolume => ok(need_name(a, false).and_then(|_| p.map_volume(pool, a))),
        Op::UnmapVolume => ok(need_name(a, false)
            .and_then(|_| p.unmap_volume(pool, a))
            .map(|_| serde_json::json!({}))),
    }
}

/// Run `p` as a plugin executable: one request on stdin, one response on
/// stdout. `<plugin> describe` (no stdin) prints the description, which is
/// handy while developing. Returns the process exit code (0 even for a
/// structured failure: the manager reads `ok`).
pub fn serve<P: StoragePlugin>(p: P) -> i32 {
    let args: Vec<String> = std::env::args().collect();
    let resp = if args.get(1).map(String::as_str) == Some("describe") {
        Response::success(&p.describe())
    } else {
        let mut input = String::new();
        match std::io::stdin().take(16 << 20).read_to_string(&mut input) {
            Err(e) => Response::failure(PluginError::invalid(format!("reading stdin: {e}"))),
            Ok(_) => match serde_json::from_str::<Request>(&input) {
                Err(e) => Response::failure(PluginError::invalid(format!(
                    "request is not valid JSON for protocol {}: {e}",
                    crate::API_VERSION
                ))),
                Ok(req) => handle_request(&p, &req),
            },
        }
    };
    let mut out = std::io::stdout().lock();
    let _ = serde_json::to_writer(&mut out, &resp);
    let _ = out.write_all(b"\n");
    let _ = out.flush();
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Minimal;
    impl StoragePlugin for Minimal {
        fn describe(&self) -> PluginDescription {
            PluginDescription {
                id: "minimal".into(),
                name: "Minimal".into(),
                version: "1".into(),
                api_version: crate::API_VERSION,
                description: String::new(),
                schema: vec![],
                capabilities: Default::default(),
            }
        }
    }

    fn req(op: Op, args: VolumeArgs) -> Request {
        Request {
            api_version: 1,
            op,
            pool: Some(PoolContext {
                id: "p".into(),
                state_dir: "/tmp/x".into(),
                ..Default::default()
            }),
            args,
        }
    }

    #[test]
    fn unimplemented_ops_answer_unsupported() {
        let r = handle_request(&Minimal, &req(Op::ListVolumes, VolumeArgs::default()));
        assert!(!r.ok);
        assert_eq!(r.error.unwrap().code, ErrorCode::Unsupported);
        // deactivate defaults to success (idempotent no-op)
        assert!(handle_request(&Minimal, &req(Op::Deactivate, VolumeArgs::default())).ok);
    }

    #[test]
    fn names_are_checked_before_the_plugin_runs() {
        let r = handle_request(
            &Minimal,
            &req(
                Op::DeleteVolume,
                VolumeArgs {
                    name: "../etc".into(),
                    ..Default::default()
                },
            ),
        );
        assert_eq!(r.error.unwrap().code, ErrorCode::Invalid);
        let r = handle_request(
            &Minimal,
            &req(
                Op::CreateSnapshot,
                VolumeArgs {
                    name: "v".into(),
                    ..Default::default()
                },
            ),
        );
        assert_eq!(r.error.unwrap().code, ErrorCode::Invalid);
    }

    #[test]
    fn describe_needs_no_pool_and_newer_protocols_are_refused() {
        let mut r = req(Op::Describe, VolumeArgs::default());
        r.pool = None;
        let resp = handle_request(&Minimal, &r);
        assert!(resp.ok);
        assert_eq!(resp.result.unwrap()["id"], "minimal");
        let mut r = req(Op::Status, VolumeArgs::default());
        r.api_version = 99;
        assert_eq!(
            handle_request(&Minimal, &r).error.unwrap().code,
            ErrorCode::Unsupported
        );
    }
}
