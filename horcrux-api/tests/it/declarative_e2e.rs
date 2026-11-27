//! Declarative config end to end against a freshly spawned horcrux-api:
//! plan/apply/export over HTTP with real storage pools, SDN zones and VMs
//! (and a VNet + subnet when the sandbox lets us create a bridge). No init
//! system is involved, so the same test runs on OpenRC and systemd nodes.

use crate::support::{scratch, Server};
use serde_json::{json, Value};

fn can_make_bridges() -> bool {
    let ok = std::process::Command::new("ip")
        .args(["link", "add", "hxdclprobe", "type", "bridge"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if ok {
        let _ = std::process::Command::new("ip")
            .args(["link", "del", "hxdclprobe"])
            .output();
    }
    ok
}

fn kinds(plan: &Value) -> Vec<String> {
    plan["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            format!(
                "{} {} {}",
                c["action"].as_str().unwrap(),
                c["kind"].as_str().unwrap(),
                c["name"].as_str().unwrap()
            )
        })
        .collect()
}

#[tokio::test]
async fn export_plan_apply_converge_and_prune() {
    let dir = scratch("declarative");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;

    let pool_dir = dir.join("decl-pool");
    std::fs::create_dir_all(&pool_dir).unwrap();
    let with_net = can_make_bridges();
    let net = if with_net {
        "  vnets: [{id: dcl1, zone: lab}]\n  subnets: [{vnet: dcl1, cidr: 10.77.0.0/24, gateway: 10.77.0.1}]\n"
    } else {
        ""
    };
    let spec = format!(
        "version: 1\nstorage_pools:\n  - {{name: decl, type: directory, path: {}}}\nnetworks:\n  zones: [{{id: lab, type: simple}}]\n{net}vms:\n  - name: decl-vm\n    memory: 256\n    cpus: 1\n    disks: [{{size_gb: 1, storage: decl}}]\n    display: {{protocol: none}}\n    power: stopped\n",
        pool_dir.display()
    );

    // a VM made outside the spec must survive a prune-less apply
    let (s, v) = api
        .post(
            "/vms",
            json!({"id": "bystander", "name": "bystander", "hypervisor": "qemu",
                   "architecture": "x86_64", "cpus": 1, "memory": 64, "disk_size": 1,
                   "status": "stopped"}),
        )
        .await;
    assert!(s == 200 || s == 201, "{v}");

    // bad spec: 422, nothing touched
    let (s, v) = api
        .post("/declarative/plan", json!({"spec": "containers: []"}))
        .await;
    assert_eq!(s, 422, "{v}");

    // plan changes nothing
    let (s, v) = api.post("/declarative/plan", json!({"spec": spec})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["in_sync"], false);
    assert!(kinds(&v["plan"]).contains(&"create storage_pool decl".to_string()));
    assert!(kinds(&v["plan"]).contains(&"create vm decl-vm".to_string()));
    assert_eq!(api.get("/vms").await.1.as_array().unwrap().len(), 1);

    // dry run = plan; apply = real
    let (s, v) = api
        .post("/declarative/apply", json!({"spec": spec, "dry_run": true}))
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(v["results"].as_array().unwrap().is_empty());
    let (s, v) = api.post("/declarative/apply", json!({"spec": spec})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["ok"], true, "{v}");
    for r in v["results"].as_array().unwrap() {
        assert_eq!(r["status"], "applied", "{r}");
    }

    // real effects
    let vms = api.get("/vms").await.1;
    let vm = vms
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == "decl-vm")
        .expect("declared VM exists");
    let disk = vm["disks"][0]["path"].as_str().unwrap();
    assert!(
        disk.starts_with(pool_dir.to_str().unwrap()),
        "disk {disk} placed in the declared pool"
    );
    assert!(vms
        .as_array()
        .unwrap()
        .iter()
        .any(|x| x["name"] == "bystander"));
    assert!(api
        .get("/storage/pools")
        .await
        .1
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["name"] == "decl"));
    assert_eq!(api.get("/sdn/zones").await.1.as_array().unwrap().len(), 1);
    if with_net {
        assert_eq!(api.get("/sdn/subnets").await.1.as_array().unwrap().len(), 1);
    }

    // idempotent: the second apply has nothing to do
    let (_, v) = api.post("/declarative/plan", json!({"spec": spec})).await;
    assert_eq!(v["in_sync"], true, "{v}");
    let (_, v) = api.post("/declarative/apply", json!({"spec": spec})).await;
    assert_eq!(v["ok"], true);
    assert!(v["results"].as_array().unwrap().is_empty(), "{v}");

    // export -> plan is in sync, and the export lists everything
    let (s, ex) = api.get("/declarative/export").await;
    assert_eq!(s, 200, "{ex}");
    let text = ex["spec"].as_str().unwrap();
    for want in ["decl-vm", "bystander", "name: decl", "id: lab"] {
        assert!(text.contains(want), "{want} in export:\n{text}");
    }
    let (_, v) = api
        .post("/declarative/plan", json!({"spec": text, "prune": true}))
        .await;
    assert_eq!(v["in_sync"], true, "export is a fixed point: {v}");

    // drift: changing memory is reported, not silently applied
    let drifted = spec.replace("memory: 256", "memory: 512");
    let (_, v) = api
        .post("/declarative/apply", json!({"spec": drifted}))
        .await;
    assert_eq!(v["ok"], false);
    assert_eq!(v["results"][0]["status"], "skipped", "{v}");
    assert!(v["results"][0]["details"][0]
        .as_str()
        .unwrap()
        .starts_with("memory: 256 -> 512"));

    // prune: only managed sections lose unlisted resources
    let only_vms = "vms:\n  - {name: decl-vm, memory: 256, cpus: 1, power: stopped}\n";
    let (_, v) = api
        .post("/declarative/plan", json!({"spec": only_vms}))
        .await;
    assert_eq!(v["in_sync"], true, "bystander kept without prune: {v}");
    let (_, v) = api
        .post(
            "/declarative/apply",
            json!({"spec": only_vms, "prune": true}),
        )
        .await;
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(kinds(&v["plan"]), ["delete vm bystander"]);
    let names: Vec<String> = api
        .get("/vms")
        .await
        .1
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, ["decl-vm"]);
    assert_eq!(
        api.get("/storage/pools")
            .await
            .1
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p["id"] != "local") // the install's default pool
            .count(),
        1,
        "pools are unmanaged by that spec"
    );

    // empty managed sections + prune tear the rest down, dependents first
    let (_, v) = api
        .post(
            "/declarative/apply",
            json!({"spec": "storage_pools: []\nnetworks: {zones: [], vnets: [], subnets: []}\nvms: []\n", "prune": true}),
        )
        .await;
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(api.get("/vms").await.1.as_array().unwrap().len(), 0);
    assert_eq!(api.get("/sdn/zones").await.1.as_array().unwrap().len(), 0);
    assert!(api
        .get("/storage/pools")
        .await
        .1
        .as_array()
        .unwrap()
        .iter()
        .all(|p| p["name"] != "decl"));
}

#[tokio::test]
async fn needs_a_login() {
    let dir = scratch("declarative-auth");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let r = srv
        .client()
        .get(format!("{}/declarative/export", srv.base()))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 401);
}

fn tool_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../build/live-iso/common/overlay/usr/bin/default-editor")
}

/// A scratch system root with the real `default-editor` tool and some editor
/// binaries (stand-in executables) under it.
fn editor_root(dir: &std::path::Path, tool: bool, bins: &[&str]) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let root = dir.join("sysroot");
    std::fs::create_dir_all(root.join("usr/sbin")).unwrap();
    std::fs::create_dir_all(root.join("usr/bin")).unwrap();
    std::fs::create_dir_all(root.join("etc/env.d")).unwrap();
    if tool {
        let t = root.join("usr/sbin/default-editor");
        std::fs::copy(tool_path(), &t).unwrap();
        std::fs::set_permissions(&t, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    for b in bins {
        let p = root.join("usr/bin").join(b);
        std::fs::write(&p, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    root
}

#[tokio::test]
async fn system_editor_export_plan_apply_and_rollback() {
    let dir = scratch("declarative-editor");
    let root = editor_root(&dir, true, &["vim", "nvim"]);
    let mut srv = Server::new(&dir).env("HORCRUX_SYSROOT", root.to_str().unwrap());
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;
    let envd = root.join("etc/env.d/99editor");

    // export: nothing configured = the default (vim), and it is a fixed point
    let (s, ex) = api.get("/declarative/export").await;
    assert_eq!(s, 200, "{ex}");
    let before = ex["spec"].as_str().unwrap().to_string();
    assert!(before.contains("system:\n  editor: vim"), "{before}");
    let (_, v) = api.post("/declarative/plan", json!({"spec": before})).await;
    assert_eq!(v["in_sync"], true, "{v}");

    // plan changes nothing; invalid values are 422 / plan errors
    let want = "system:\n  editor: neovim\n";
    let (s, v) = api.post("/declarative/plan", json!({"spec": want})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(kinds(&v["plan"]), ["update system_editor editor"]);
    assert!(!envd.exists());
    let (s, v) = api
        .post(
            "/declarative/apply",
            json!({"spec": "system: {editor: emacs}"}),
        )
        .await;
    assert!(s == 422 || v["ok"] == false, "{s} {v}");
    assert!(
        v.to_string().contains("system.editor"),
        "clear error expected: {v}"
    );
    assert!(!root.join("etc/default-editor.conf").exists());

    // apply: the conf and EDITOR/VISUAL point at nvim
    let (s, v) = api.post("/declarative/apply", json!({"spec": want})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["results"][0]["status"], "applied", "{v}");
    let conf = std::fs::read_to_string(root.join("etc/default-editor.conf")).unwrap();
    assert!(conf.contains("EDITOR_CHOICE=neovim"), "{conf}");
    let e = std::fs::read_to_string(&envd).unwrap();
    assert!(e.contains("EDITOR=\"/usr/bin/nvim\""), "{e}");
    let (_, v) = api.post("/declarative/plan", json!({"spec": want})).await;
    assert_eq!(v["in_sync"], true, "{v}");

    // rollback: apply the earlier export again
    let (_, v) = api
        .post("/declarative/apply", json!({"spec": before}))
        .await;
    assert_eq!(v["ok"], true, "{v}");
    let e = std::fs::read_to_string(&envd).unwrap();
    assert!(e.contains("EDITOR=\"/usr/bin/vim\""), "{e}");
    let (_, ex) = api.get("/declarative/export").await;
    assert!(ex["spec"].as_str().unwrap().contains("editor: vim"));
}

#[tokio::test]
async fn system_editor_endpoint_get_put_audit_and_424() {
    let dir = scratch("system-editor-api");
    let root = editor_root(&dir, true, &["vim", "nvim"]);
    let mut srv = Server::new(&dir).env("HORCRUX_SYSROOT", root.to_str().unwrap());
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;

    let (s, v) = api.get("/system/editor").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["choice"], "vim");
    assert_eq!(v["effective"], "/usr/bin/vim");
    let inst = |c: &str| {
        v["installed"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["choice"] == c)
            .unwrap()["installed"]
            .clone()
    };
    assert_eq!(inst("neovim"), true);
    assert_eq!(inst("nano"), false);

    let (s, v) = api.put("/system/editor", json!({"choice": "neovim"})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["choice"], "neovim");
    assert_eq!(v["effective"], "/usr/bin/nvim");
    // a choice that is not installed falls back, and says so
    let (s, v) = api.put("/system/editor", json!({"choice": "nano"})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["choice"], "nano");
    assert_eq!(v["effective"], "/usr/bin/vim");

    for bad in ["emacs", "", "/a b", "vim; id"] {
        let (s, v) = api.put("/system/editor", json!({"choice": bad})).await;
        assert!(s == 400 || s == 422, "{bad}: {s} {v}");
    }
    let conf = std::fs::read_to_string(root.join("etc/default-editor.conf")).unwrap();
    assert!(conf.contains("EDITOR_CHOICE=nano"), "{conf}");

    // audit logged
    let (s, a) = api.get("/audit/search?search=default%20editor").await;
    assert_eq!(s, 200, "{a}");
    assert!(
        a.to_string().contains("default editor set to neovim"),
        "{a}"
    );
}

#[tokio::test]
async fn system_editor_without_the_tool_is_a_424() {
    let dir = scratch("system-editor-424");
    let root = editor_root(&dir, false, &["vim"]);
    let mut srv = Server::new(&dir).env("HORCRUX_SYSROOT", root.to_str().unwrap());
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;
    let (s, v) = api.get("/system/editor").await;
    assert_eq!(s, 424, "{v}");
    assert!(v.to_string().contains("default-editor-missing"), "{v}");
    let (s, v) = api.put("/system/editor", json!({"choice": "vim"})).await;
    assert_eq!(s, 424, "{v}");
    // declarative: the section is blocked, and export omits it
    let (_, ex) = api.get("/declarative/export").await;
    assert!(!ex["spec"].as_str().unwrap().contains("system:"), "{ex}");
    let (_, v) = api
        .post(
            "/declarative/apply",
            json!({"spec": "system: {editor: vim}"}),
        )
        .await;
    assert_eq!(v["ok"], false, "{v}");
    assert_eq!(v["results"][0]["status"], "skipped", "{v}");
}
