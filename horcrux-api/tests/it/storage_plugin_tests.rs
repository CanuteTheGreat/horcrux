//! Third-party storage plugins over HTTP against a freshly spawned
//! horcrux-api: a plugin written in Python (the protocol is plain JSON over
//! stdio, no SDK needed) is listed with its schema, a pool config is
//! validated against that schema (out-of-range number, missing secret: 400
//! naming the field), the secret reaches the plugin but never comes back
//! from the API, and volumes are created, grown, snapshotted, rolled back
//! and deleted through the plugin. The pool and its secret survive a
//! restart (the plugin is activated again). The reference SSHFS plugin is
//! tested against a real SSH server in tests/storage_plugin_e2e.sh.
use crate::support::{scratch, Server};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;

const PLUGIN: &str = r#"#!/usr/bin/env python3
import json, os, shutil, sys
req = json.load(sys.stdin)
op = req["op"]
def out(ok, v):
    print(json.dumps({"ok": True, "result": v} if ok else {"ok": False, "error": v}))
    sys.exit(0)
def err(code, msg, hint=""):
    out(False, {"code": code, "message": msg, "hint": hint})
if op == "describe":
    out(True, {"id": "pydir", "name": "Py Dir", "version": "0.1", "api_version": 1,
        "description": "test plugin",
        "schema": [
            {"key": "label", "type": "string", "label": "Label", "default": "x",
             "pattern": "[a-z]+", "hint": "lower-case letters"},
            {"key": "quota_gb", "type": "int", "label": "Quota", "default": 10,
             "min": 1, "max": 100, "hint": "Largest size of the pool in GB"},
            {"key": "token", "type": "secret", "label": "Token", "required": True}],
        "capabilities": {"content": ["images"], "snapshots": True, "resize": True,
                         "vm_disk_dir": True, "formats": ["raw"]}})
pool = req["pool"]; st = pool["state_dir"]; vols = os.path.join(st, "vols")
a = req.get("args", {}); name = a.get("name", "")
def vpath(n): return os.path.join(vols, n + ".raw")
def vinfo(n):
    p = vpath(n)
    return {"name": n, "path": p, "format": "raw", "size_bytes": os.path.getsize(p),
            "allocated_bytes": 0}
if op == "activate":
    if pool["secrets"].get("token") != "s3cret":
        err("invalid", "wrong token")
    os.makedirs(vols, exist_ok=True)
    open(os.path.join(st, "active"), "w").write(json.dumps(pool["config"]))
    out(True, {"local_dir": vols})
if op == "deactivate":
    try: os.remove(os.path.join(st, "active"))
    except FileNotFoundError: pass
    out(True, {})
if not os.path.exists(os.path.join(st, "active")):
    err("unavailable", "pool not active", "activate it")
q = pool["config"]["quota_gb"] * 2**30
if op == "status":
    used = sum(os.path.getsize(os.path.join(vols, f)) for f in os.listdir(vols))
    out(True, {"active": True, "total_bytes": q, "available_bytes": q - used})
if op == "list_volumes":
    out(True, [vinfo(f[:-4]) for f in sorted(os.listdir(vols)) if f.endswith(".raw")])
if op == "create_volume":
    if os.path.exists(vpath(name)): err("exists", name + " exists")
    with open(vpath(name), "wb") as f: f.truncate(a["size_bytes"])
    out(True, vinfo(name))
if op in ("delete_volume", "resize_volume", "volume_path", "list_snapshots",
          "create_snapshot", "delete_snapshot", "rollback_snapshot"):
    if not os.path.exists(vpath(name)): err("not_found", "no volume " + name)
snap = os.path.join(vols, name + ".raw@" + str(a.get("snapshot")))
if op == "delete_volume":
    os.remove(vpath(name)); out(True, {})
if op == "resize_volume":
    with open(vpath(name), "r+b") as f: f.truncate(a["size_bytes"])
    out(True, vinfo(name))
if op == "volume_path":
    out(True, {"path": vpath(name), "format": "raw"})
if op == "list_snapshots":
    out(True, [{"name": f.split("@", 1)[1]} for f in sorted(os.listdir(vols))
               if f.startswith(name + ".raw@")])
if op == "create_snapshot":
    shutil.copy(vpath(name), snap); out(True, {})
if op == "delete_snapshot":
    if not os.path.exists(snap): err("not_found", "no snapshot")
    os.remove(snap); out(True, {})
if op == "rollback_snapshot":
    if not os.path.exists(snap): err("not_found", "no snapshot")
    shutil.copy(snap, vpath(name)); out(True, {})
err("unsupported", op)
"#;

#[tokio::test]
async fn plugin_pool_lifecycle_over_http() {
    let dir = scratch("storage-plugins");
    let plugins = dir.join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    let exe = plugins.join("pydir");
    std::fs::write(&exe, PLUGIN).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    // the test tree is not root-owned when run as a normal user
    let settings = dir.join("storage-plugins.json");
    std::fs::write(
        &settings,
        json!({"plugin_dir": plugins, "state_dir": dir.join("pstate"),
               "require_root_owned": false})
        .to_string(),
    )
    .unwrap();
    let mut srv = Server::new(&dir).env(
        "HORCRUX_STORAGE_PLUGIN_SETTINGS",
        settings.to_str().unwrap(),
    );
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;

    // listed with its schema
    let (s, l) = api.get("/storage/plugins").await;
    assert_eq!(s, 200, "{l}");
    let p = l["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "pydir")
        .unwrap_or_else(|| panic!("pydir not listed: {l}"));
    assert_eq!(p["usable"], true, "{p}");
    assert_eq!(p["description"]["schema"][1]["max"], 100);
    let (s, d) = api.get("/storage/plugins/pydir").await;
    assert_eq!((s, d["name"].as_str()), (200, Some("Py Dir")));
    let (s, e) = api.get("/storage/plugins/nope").await;
    assert_eq!(s, 424, "a missing plugin is a structured 424: {e}");

    // schema validation: every bad field named, nothing created
    let (s, e) = api
        .post(
            "/storage/pools",
            json!({"name": "bad", "storage_type": "plugin:pydir",
                   "config": {"quota_gb": 1000, "label": "UPPER", "bogus": 1}}),
        )
        .await;
    assert_eq!(s, 400, "{e}");
    let msg = e.to_string();
    for k in ["quota_gb", "label", "bogus", "token"] {
        assert!(msg.contains(k), "{k} not in {msg}");
    }
    assert!(msg.contains("at most 100"), "{msg}");
    // the plugin itself refuses a wrong secret: nothing stored either
    let (s, e) = api
        .post(
            "/storage/pools",
            json!({"name": "bad", "storage_type": "plugin:pydir", "config": {"token": "nope"}}),
        )
        .await;
    assert_eq!(s, 400, "{e}");
    let (_, pools) = api.get("/storage/pools").await;
    assert!(!pools.to_string().contains("\"bad\""), "{pools}");

    let pool = api
        .ok(
            reqwest::Method::POST,
            "/storage/pools",
            Some(json!({"name": "py", "storage_type": "plugin:pydir",
                        "config": {"quota_gb": "5", "token": "s3cret"}})),
        )
        .await;
    let pid = pool["id"].as_str().unwrap().to_string();
    assert_eq!(pool["storage_type"], "plugin:pydir");
    assert_eq!(pool["plugin_config"]["quota_gb"], 5, "{pool}");
    assert_eq!(pool["plugin_config"]["label"], "x", "default filled in");
    assert_eq!(pool["total"], 5, "capacity from the plugin's status");
    assert!(
        !pool.to_string().contains("s3cret"),
        "secret returned: {pool}"
    );
    let (_, listed) = api.get("/storage/pools").await;
    assert!(!listed.to_string().contains("s3cret"));
    let active = dir.join(format!("pstate/{pid}/active"));
    assert!(active.exists(), "the plugin was activated");

    // volumes through the plugin
    let v = api
        .ok(
            reqwest::Method::POST,
            &format!("/storage/pools/{pid}/volumes"),
            Some(json!({"name": "v1", "size": 1})),
        )
        .await;
    assert_eq!(v["size_bytes"], 1u64 << 30, "{v}");
    let (s, _) = api
        .post(
            &format!("/storage/pools/{pid}/volumes"),
            json!({"name": "v1", "size": 1}),
        )
        .await;
    assert_eq!(s, 409, "a duplicate volume is a conflict");
    let (s, _) = api
        .post(
            &format!("/storage/pools/{pid}/volumes"),
            json!({"name": "../x", "size": 1}),
        )
        .await;
    assert_eq!(s, 400);
    let v = api
        .ok(
            reqwest::Method::POST,
            &format!("/storage/pools/{pid}/volumes/v1/resize"),
            Some(json!({"size": 2})),
        )
        .await;
    assert_eq!(v["size_bytes"], 2u64 << 30);
    let (s, _) = api
        .post(
            &format!("/storage/pools/{pid}/volumes/v1/resize"),
            json!({"size": 1}),
        )
        .await;
    assert_eq!(s, 400, "shrinking is refused before the plugin runs");

    // snapshots: create, list, rollback (size comes back), delete
    api.ok(
        reqwest::Method::POST,
        &format!("/storage/pools/{pid}/volumes/v1/snapshots"),
        Some(json!({"name": "s1"})),
    )
    .await;
    let snaps = api
        .ok(
            reqwest::Method::GET,
            &format!("/storage/pools/{pid}/volumes/v1/snapshots"),
            None,
        )
        .await;
    assert_eq!(snaps[0]["name"], "s1", "{snaps}");
    api.ok(
        reqwest::Method::POST,
        &format!("/storage/pools/{pid}/volumes/v1/resize"),
        Some(json!({"size": 3})),
    )
    .await;
    api.ok(
        reqwest::Method::POST,
        &format!("/storage/pools/{pid}/volumes/v1/snapshots/s1/rollback"),
        None,
    )
    .await;
    let vols = api
        .ok(
            reqwest::Method::GET,
            &format!("/storage/pools/{pid}/volumes"),
            None,
        )
        .await;
    assert_eq!(vols[0]["size_bytes"], 2u64 << 30, "rolled back: {vols}");
    assert_eq!(
        api.delete(&format!("/storage/pools/{pid}/volumes/v1/snapshots/s1"))
            .await,
        200
    );
    assert_eq!(
        api.delete(&format!("/storage/pools/{pid}/volumes/v1/snapshots/s1"))
            .await,
        404
    );

    // the pool and its secret survive a restart: activated again
    std::fs::remove_file(&active).unwrap();
    srv.restart_after(nix::sys::signal::Signal::SIGTERM).await;
    let api = srv.api().await;
    crate::support::wait_until("plugin pool re-activated", 30, || active.exists()).await;
    let vols = api
        .ok(
            reqwest::Method::GET,
            &format!("/storage/pools/{pid}/volumes"),
            None,
        )
        .await;
    assert_eq!(vols[0]["name"], "v1", "{vols}");

    assert_eq!(
        api.delete(&format!("/storage/pools/{pid}/volumes/v1"))
            .await,
        200
    );
    assert_eq!(api.delete(&format!("/storage/pools/{pid}")).await, 200);
    assert!(!active.exists(), "removing the pool deactivated it");

    // settings: readable, validated, persisted
    let (s, st) = api.get("/storage/plugin-settings").await;
    assert_eq!(s, 200);
    assert_eq!(st["timeouts"]["create_secs"], 600);
    let mut bad = st.clone();
    bad["timeouts"]["create_secs"] = json!(0);
    let (s, _) = api.put("/storage/plugin-settings", bad).await;
    assert_eq!(s, 400);
    let mut good = st.clone();
    good["timeouts"]["status_secs"] = json!(7);
    let (s, back) = api.put("/storage/plugin-settings", good).await;
    assert_eq!(
        (s, back["timeouts"]["status_secs"].as_u64()),
        (200, Some(7))
    );
}
