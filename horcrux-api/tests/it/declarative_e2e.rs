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

/// The spec text minus some top-level sections.
fn without(text: &str, keys: &[&str]) -> String {
    let mut v: serde_yaml::Value = serde_yaml::from_str(text).unwrap();
    for k in keys {
        v.as_mapping_mut().unwrap().remove(*k);
    }
    serde_yaml::to_string(&v).unwrap()
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
        .post("/declarative/plan", json!({"spec": "frobnicators: []"}))
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
    // (containers and volumes are the shared CI host's own: another job may
    // start or remove one between the export and the plan)
    let (_, v) = api
        .post(
            "/declarative/plan",
            json!({"spec": without(text, &["containers", "volumes"]), "prune": true}),
        )
        .await;
    assert_eq!(v["in_sync"], true, "export is a fixed point: {v}");

    // phase 2: changing memory of a stopped VM is applied in place
    let drifted = spec.replace("memory: 256", "memory: 512");
    let (_, v) = api
        .post("/declarative/apply", json!({"spec": drifted}))
        .await;
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(kinds(&v["plan"]), ["update vm decl-vm"], "{v}");
    assert_eq!(v["results"][0]["status"], "applied", "{v}");
    assert!(v["results"][0]["details"][0]
        .as_str()
        .unwrap()
        .starts_with("memory: 256 -> 512"));
    assert_eq!(v["results"][0]["fields"][0]["field"], "memory");
    let vms = api.get("/vms").await.1;
    let vm = vms
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == "decl-vm")
        .unwrap();
    assert_eq!(vm["memory"], 512, "memory really changed");
    // and back: the original spec is the drift now
    let (_, v) = api.post("/declarative/apply", json!({"spec": spec})).await;
    assert_eq!(v["ok"], true, "{v}");
    let (_, v) = api.post("/declarative/plan", json!({"spec": spec})).await;
    assert_eq!(v["in_sync"], true, "{v}");

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

// ------------------------------------------------------------------------
// Phase 2/3/5: stored desired state + drift + history + rollback

async fn vm_memory(api: &crate::support::Api, name: &str) -> Option<u64> {
    api.get("/vms")
        .await
        .1
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == name)
        .map(|x| x["memory"].as_u64().unwrap())
}

fn drift_has(v: &Value, kind: &str, name: &str, field: &str) -> Option<(String, String)> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == kind && i["name"] == name && i["field"] == field)
        .map(|i| {
            (
                i["expected"].as_str().unwrap().to_string(),
                i["actual"].as_str().unwrap().to_string(),
            )
        })
}

#[tokio::test]
async fn desired_state_drift_history_rollback() {
    let dir = scratch("declarative-drift");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;
    let vm_spec = |mem: u32, extra: &str| {
        format!(
            "version: 1\nvms:\n  - {{name: dd-a, memory: {mem}, cpus: 1, power: stopped}}\n{extra}"
        )
    };

    // nothing stored yet
    let (s, v) = api.get("/declarative/desired").await;
    assert_eq!((s, &v["configured"]), (200, &json!(false)), "{v}");
    let (_, d) = api.get("/declarative/drift").await;
    assert_eq!(d["configured"], false);
    assert_eq!(d["in_sync"], true);

    // an invalid spec is refused (422) and stores nothing
    let (s, _) = api
        .put("/declarative/desired", json!({"spec": "version: 9"}))
        .await;
    assert_eq!(s, 422);
    assert_eq!(api.get("/declarative/desired").await.1["configured"], false);
    // validate answers inline, never stores
    let (s, v) = api
        .post(
            "/declarative/desired/validate",
            json!({"spec": "vms: [{name: a b, memory: 1}]"}),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["ok"], false, "{v}");
    assert!(v["error"].as_str().unwrap().contains("VM"));

    // store the spec: the node lacks the VM, drift says so, per field
    let spec1 = vm_spec(128, "");
    let (s, v) = api
        .put(
            "/declarative/desired",
            json!({"spec": spec1, "source": "ui"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["in_sync"], false);
    let (_, d) = api.get("/declarative/drift").await;
    assert_eq!(d["configured"], true);
    assert_eq!(
        drift_has(&d, "vm", "dd-a", "(resource)"),
        Some(("present".into(), "missing".into())),
        "{d}"
    );
    assert!(d["count"].as_u64().unwrap() >= 1);
    let (_, g) = api.get("/declarative/desired").await;
    assert_eq!(g["spec"].as_str().unwrap(), spec1);
    assert_eq!(g["source"], "ui");

    // the drift alert rule exists (created by the background task) and the
    // settings validate
    let (s, _) = api
        .put(
            "/declarative/settings",
            json!({"drift_interval_secs": 1, "history_limit": 10, "drift_alert": true}),
        )
        .await;
    assert_eq!(s, 422, "interval below the floor is refused");
    let (s, v) = api
        .put(
            "/declarative/settings",
            json!({"drift_interval_secs": 5, "history_limit": 10, "drift_alert": true}),
        )
        .await;
    assert_eq!(s, 200, "{v}");

    // apply the stored spec
    let (s, v) = api
        .post("/declarative/desired/apply", json!({"dry_run": true}))
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(v["history_id"].is_null(), "a dry run is not recorded: {v}");
    let (s, v) = api.post("/declarative/desired/apply", json!({})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["ok"], true, "{v}");
    let h1 = v["history_id"].as_str().expect("recorded").to_string();
    assert_eq!(vm_memory(&api, "dd-a").await, Some(128));
    let (_, d) = api.get("/declarative/drift?refresh=true").await;
    assert_eq!(d["in_sync"], true, "{d}");
    assert_eq!(d["count"], 0);

    // hand-change the live VM: drift names the field with expected vs actual
    let (s, v) = api.put("/vms/dd-a/config", json!({"memory": 300})).await;
    assert_eq!(s, 200, "{v}");
    let (_, d) = api.post("/declarative/drift/check", json!({})).await;
    assert_eq!(d["in_sync"], false, "{d}");
    assert_eq!(
        drift_has(&d, "vm", "dd-a", "memory"),
        Some(("128".into(), "300".into())),
        "{d}"
    );
    // the background check + alert hook: rule exists, an alert fires
    let mut fired = false;
    for _ in 0..30 {
        let rules = api.get("/alerts/rules").await.1;
        let has_rule = rules
            .as_array()
            .map(|r| r.iter().any(|x| x["name"] == "Configuration drift"))
            .unwrap_or(false);
        let active = api.get("/alerts/active").await.1;
        fired = has_rule
            && active
                .as_array()
                .map(|a| a.iter().any(|x| x["rule_name"] == "Configuration drift"))
                .unwrap_or(false);
        if fired {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    assert!(fired, "the drift alert fires while the node differs");

    // correcting the drift records an apply (source api by default)
    let (_, v) = api.post("/declarative/desired/apply", json!({})).await;
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(kinds(&v["plan"]), ["update vm dd-a"]);
    assert_eq!(vm_memory(&api, "dd-a").await, Some(128));
    let h2 = v["history_id"].as_str().unwrap().to_string();

    // history: newest first, spec and per-change results kept
    let (_, h) = api.get("/declarative/history").await;
    let ids: Vec<&str> = h["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids[..2], [h2.as_str(), h1.as_str()]);
    let (s, e) = api.get(&format!("/declarative/history/{h1}")).await;
    assert_eq!(s, 200);
    assert_eq!(e["spec"].as_str().unwrap(), spec1);
    assert_eq!(e["results"][0]["status"], "applied");
    assert_eq!(e["actor"], "admin");
    assert_eq!(api.get("/declarative/history/nope").await.0, 404);

    // a bystander VM made outside the spec must survive a rollback
    let (s, _) = api
        .post(
            "/vms",
            json!({"id": "dd-bystander", "name": "dd-bystander", "hypervisor": "qemu",
                   "architecture": "x86_64", "cpus": 1, "memory": 64, "disk_size": 1,
                   "status": "stopped"}),
        )
        .await;
    assert!(s == 200 || s == 201);

    // a bigger apply: new VM + memory change
    let spec3 = format!(
        "version: 1\nvms:\n  - {{name: dd-a, memory: 192, cpus: 1, power: stopped}}\n  - {{name: dd-b, memory: 64, cpus: 1, power: stopped}}\n"
    );
    let (_, v) = api
        .post(
            "/declarative/apply",
            json!({"spec": spec3, "source": "cli"}),
        )
        .await;
    assert_eq!(v["ok"], true, "{v}");
    let h3 = v["history_id"].as_str().unwrap().to_string();
    assert_eq!(vm_memory(&api, "dd-b").await, Some(64));
    let (_, e) = api.get(&format!("/declarative/history/{h3}")).await;
    assert_eq!(e["source"], "cli");

    // rollback preview: revert memory, delete only what h3 created
    let (s, pv) = api
        .post(
            &format!("/declarative/history/{h3}/rollback"),
            json!({"dry_run": true}),
        )
        .await;
    assert_eq!(s, 200, "{pv}");
    assert_eq!(pv["restored_from"], json!(h2));
    assert_eq!(
        kinds(&pv["report"]["plan"]),
        ["update vm dd-a", "delete vm dd-b"],
        "{pv}"
    );
    assert_eq!(
        vm_memory(&api, "dd-b").await,
        Some(64),
        "preview changes nothing"
    );
    // really roll back
    let (s, rb) = api
        .post(&format!("/declarative/history/{h3}/rollback"), json!({}))
        .await;
    assert_eq!(s, 200, "{rb}");
    assert_eq!(rb["report"]["ok"], true, "{rb}");
    assert_eq!(vm_memory(&api, "dd-a").await, Some(128));
    assert_eq!(vm_memory(&api, "dd-b").await, None, "created VM removed");
    assert_eq!(
        vm_memory(&api, "dd-bystander").await,
        Some(64),
        "a VM the rolled-back apply didn't create stays"
    );
    let rb_id = rb["report"]["history_id"].as_str().unwrap();
    let (_, e) = api.get(&format!("/declarative/history/{rb_id}")).await;
    assert_eq!(e["source"], "rollback");
    assert_eq!(e["rollback_of"], json!(h3));
    // the oldest record has nothing before it
    let (s, v) = api
        .post(&format!("/declarative/history/{h1}/rollback"), json!({}))
        .await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(
        api.post("/declarative/history/nope/rollback", json!({}))
            .await
            .0,
        404
    );

    // retention: the cap (10) drops the oldest entries
    for i in 0..12 {
        let mem = 100 + i;
        let (_, v) = api
            .post("/declarative/apply", json!({"spec": vm_spec(mem, "")}))
            .await;
        assert_eq!(v["ok"], true, "{v}");
    }
    let (_, h) = api.get("/declarative/history").await;
    assert_eq!(h["total"], 10, "{h}");
    assert_eq!(api.get(&format!("/declarative/history/{h1}")).await.0, 404);

    // clearing the desired spec clears the drift view
    assert_eq!(api.delete("/declarative/desired").await, 200);
    let (_, d) = api.get("/declarative/drift").await;
    assert_eq!(
        (d["configured"].clone(), d["in_sync"].clone()),
        (json!(false), json!(true))
    );
}

// ------------------------------------------------------------------------
// Phase 3: GitOps against a real git repository (file:// remote) and the
// real `git` / `ssh-keygen` binaries.

struct Repo {
    dir: std::path::PathBuf,
}

impl Repo {
    fn new(root: &std::path::Path) -> Repo {
        let dir = root.join("gitrepo");
        std::fs::create_dir_all(&dir).unwrap();
        let r = Repo { dir };
        r.git(&["init", "-q", "-b", "main"]);
        r
    }

    fn git(&self, args: &[&str]) -> String {
        let o = std::process::Command::new("git")
            .current_dir(&self.dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["-c", "user.name=CI", "-c", "user.email=ci@example.com"])
            .args(args)
            .output()
            .expect("git is needed for the GitOps tests");
        assert!(
            o.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    fn url(&self) -> String {
        format!("file://{}", self.dir.display())
    }

    /// Write `file` and commit it; returns the commit sha.
    fn commit(&self, file: &str, text: &str, msg: &str) -> String {
        let p = self.dir.join(file);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", msg]);
        self.git(&["rev-parse", "HEAD"])
    }

    /// Same, signed with an SSH key.
    fn commit_signed(&self, key: &std::path::Path, file: &str, text: &str, msg: &str) -> String {
        let p = self.dir.join(file);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
        self.git(&["add", "-A"]);
        self.git(&[
            "-c",
            "gpg.format=ssh",
            "-c",
            &format!("user.signingkey={}", key.display()),
            "commit",
            "-q",
            "-S",
            "-m",
            msg,
        ]);
        self.git(&["rev-parse", "HEAD"])
    }
}

fn ssh_key(dir: &std::path::Path, name: &str) -> (std::path::PathBuf, String) {
    let key = dir.join(name);
    let o = std::process::Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "ci", "-f"])
        .arg(&key)
        .output()
        .expect("ssh-keygen is needed for the signed-commit tests");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let public = std::fs::read_to_string(dir.join(format!("{name}.pub"))).unwrap();
    (key, public.trim().to_string())
}

fn vm_spec_text(name: &str, mem: u32) -> String {
    format!("version: 1\nvms:\n  - {{name: {name}, memory: {mem}, cpus: 1, power: stopped}}\n")
}

async fn git_status(api: &crate::support::Api) -> Value {
    api.get("/declarative/gitops/status").await.1
}

async fn history_count(api: &crate::support::Api) -> u64 {
    api.get("/declarative/history").await.1["total"]
        .as_u64()
        .unwrap()
}

#[tokio::test]
async fn gitops_auto_mode_follows_the_repo_and_corrects_drift() {
    let dir = scratch("declarative-gitops-auto");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;
    let repo = Repo::new(&dir);

    // not configured: status says so, sync has nothing to do
    let st = git_status(&api).await;
    assert_eq!(st["configured"], false, "{st}");
    assert_eq!(api.post("/declarative/gitops/sync", json!({})).await.0, 404);

    // config validation: references only, sane url/path
    for bad in [
        json!({"url": repo.url(), "credentials": {"secret": "hunter2"}}),
        json!({"url": "https://tok:pw@git.example.com/x.git"}),
        json!({"url": "--upload-pack=evil"}),
        json!({"url": repo.url(), "path": "../x.yaml"}),
        json!({"url": repo.url(), "verify_signatures": true}),
        json!({"url": repo.url(), "interval_secs": 1}),
    ] {
        let (s, v) = api.put("/declarative/gitops/config", bad.clone()).await;
        assert_eq!(s, 422, "{bad} -> {v}");
    }
    // a good https config with a file reference is accepted and shown back
    let (s, v) = api
        .put(
            "/declarative/gitops/config",
            json!({"url": "https://git.example.com/x.git", "enabled": false,
                   "credentials": {"username": "bot", "secret": "file:/etc/horcrux/git-token"}}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["config"]["credentials"]["secret"],
        "file:/etc/horcrux/git-token"
    );

    // the real config: auto mode, long interval (the test syncs by hand)
    let (s, v) = api
        .put(
            "/declarative/gitops/config",
            json!({"url": repo.url(), "branch": "main", "mode": "auto", "interval_secs": 3600}),
        )
        .await;
    assert_eq!(s, 200, "{v}");

    let c1 = repo.commit("horcrux.yaml", &vm_spec_text("g1", 128), "initial spec");
    let (s, o) = api.post("/declarative/gitops/sync", json!({})).await;
    assert_eq!(s, 200, "{o}");
    assert_eq!(o["commit"], json!(c1));
    assert_eq!(o["applied"], true, "{o}");
    assert_eq!(vm_memory(&api, "g1").await, Some(128));
    let st = git_status(&api).await;
    assert_eq!(st["last_commit"], json!(c1));
    assert_eq!(st["last_applied_commit"], json!(c1));
    assert_eq!(st["last_commit_subject"], "initial spec");
    assert!(st["last_error"].is_null(), "{st}");
    // the history knows the commit
    let (_, h) = api.get("/declarative/history").await;
    assert_eq!(h["items"][0]["source"], "gitops");
    assert_eq!(h["items"][0]["commit"], json!(c1));
    // and the repo spec became the desired state
    let (_, d) = api.get("/declarative/desired").await;
    assert_eq!(d["source"], "gitops");
    assert_eq!(d["commit"], json!(c1));

    // an in-sync poll changes nothing and writes no history
    let n = history_count(&api).await;
    let (_, o) = api.post("/declarative/gitops/sync", json!({})).await;
    assert_eq!(o["in_sync"], true, "{o}");
    assert_eq!(history_count(&api).await, n);

    // drift by hand -> visible in the drift view -> corrected on the next sync
    assert_eq!(
        api.put("/vms/g1/config", json!({"memory": 300})).await.0,
        200
    );
    let (_, dr) = api.post("/declarative/drift/check", json!({})).await;
    assert_eq!(
        drift_has(&dr, "vm", "g1", "memory"),
        Some(("128".into(), "300".into()))
    );
    let (_, o) = api.post("/declarative/gitops/sync", json!({})).await;
    assert_eq!(o["applied"], true, "{o}");
    assert_eq!(vm_memory(&api, "g1").await, Some(128));
    assert_eq!(history_count(&api).await, n + 1);

    // a new commit is followed
    let c2 = repo.commit("horcrux.yaml", &vm_spec_text("g1", 256), "more memory");
    let (_, o) = api.post("/declarative/gitops/sync", json!({})).await;
    assert_eq!(o["commit"], json!(c2));
    assert_eq!(vm_memory(&api, "g1").await, Some(256));

    // a broken spec is refused with the reason; the node keeps its state
    repo.commit("horcrux.yaml", "version: 9\n", "bad spec");
    let (s, o) = api.post("/declarative/gitops/sync", json!({})).await;
    assert_eq!(s, 422, "{o}");
    assert!(o.to_string().contains("invalid"), "{o}");
    assert_eq!(vm_memory(&api, "g1").await, Some(256));
    let st = git_status(&api).await;
    assert!(
        st["last_error"].as_str().unwrap().contains("invalid"),
        "{st}"
    );
    // fixing it clears the error
    repo.commit("horcrux.yaml", &vm_spec_text("g1", 192), "fixed");
    let (s, _) = api.post("/declarative/gitops/sync", json!({})).await;
    assert_eq!(s, 200);
    assert!(git_status(&api).await["last_error"].is_null());
    assert_eq!(vm_memory(&api, "g1").await, Some(192));

    // per-node files in one shared repo: {node} in the path
    let (s, v) = api
        .put(
            "/declarative/gitops/config",
            json!({"url": repo.url(), "mode": "auto", "interval_secs": 3600,
                   "path": "nodes/{node}.yaml", "node_name": "n1"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, o) = api.post("/declarative/gitops/sync", json!({})).await;
    assert_eq!(s, 422, "no file for node n1 yet: {o}");
    assert!(o.to_string().contains("nodes/n1.yaml"), "{o}");
    repo.commit("nodes/n1.yaml", &vm_spec_text("g1", 96), "node n1");
    repo.commit("nodes/n2.yaml", &vm_spec_text("g1", 160), "node n2");
    let (s, _) = api.post("/declarative/gitops/sync", json!({})).await;
    assert_eq!(s, 200);
    assert_eq!(
        vm_memory(&api, "g1").await,
        Some(96),
        "this node reads its own file"
    );
    assert_eq!(git_status(&api).await["path"], "nodes/n1.yaml");

    // removing the config clears the status
    assert_eq!(api.delete("/declarative/gitops/config").await, 200);
    assert_eq!(git_status(&api).await["configured"], false);
}

#[tokio::test]
async fn gitops_approve_mode_queues_a_plan_until_approved() {
    let dir = scratch("declarative-gitops-approve");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;
    let repo = Repo::new(&dir);
    let (s, v) = api
        .put(
            "/declarative/gitops/config",
            json!({"url": repo.url(), "interval_secs": 3600}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["config"]["mode"], "approve", "approval is the default");

    let c1 = repo.commit("horcrux.yaml", &vm_spec_text("ga", 128), "add ga");
    let (s, o) = api.post("/declarative/gitops/sync", json!({})).await;
    assert_eq!(s, 200, "{o}");
    assert_eq!(o["applied"], false);
    let hash1 = o["pending"].as_str().expect("a pending plan").to_string();
    assert_eq!(
        vm_memory(&api, "ga").await,
        None,
        "nothing applied before approval"
    );
    let st = git_status(&api).await;
    assert_eq!(st["pending"].as_array().unwrap().len(), 1);
    assert_eq!(st["pending"][0]["hash"], json!(hash1));
    assert_eq!(st["pending"][0]["commit"], json!(c1));
    assert_eq!(kinds(&st["pending"][0]["plan"]), ["create vm ga"]);
    assert_eq!(api.get("/declarative/history").await.1["total"], 0);

    // unknown hash
    assert_eq!(
        api.post("/declarative/gitops/approve/deadbeef", json!({}))
            .await
            .0,
        404
    );
    // the head moves: approving the old plan is refused as stale
    let c2 = repo.commit("horcrux.yaml", &vm_spec_text("ga", 256), "bigger ga");
    let (s, v) = api
        .post(&format!("/declarative/gitops/approve/{hash1}"), json!({}))
        .await;
    assert_eq!(s, 409, "{v}");
    assert!(v.to_string().contains("moved"), "{v}");
    assert_eq!(vm_memory(&api, "ga").await, None);
    // the next sync replaces the stale plan with one for the new head
    let (_, o) = api.post("/declarative/gitops/sync", json!({})).await;
    let hash2 = o["pending"].as_str().unwrap().to_string();
    assert_ne!(hash1, hash2);
    let st = git_status(&api).await;
    assert_eq!(st["pending"].as_array().unwrap().len(), 1);
    assert_eq!(st["pending"][0]["commit"], json!(c2));

    // approve it: applied, recorded with the commit and the approving user
    let (s, rep) = api
        .post(&format!("/declarative/gitops/approve/{hash2}"), json!({}))
        .await;
    assert_eq!(s, 200, "{rep}");
    assert_eq!(rep["ok"], true, "{rep}");
    assert_eq!(vm_memory(&api, "ga").await, Some(256));
    let (_, h) = api.get("/declarative/history").await;
    assert_eq!(h["items"][0]["source"], "gitops");
    assert_eq!(h["items"][0]["commit"], json!(c2));
    assert_eq!(h["items"][0]["actor"], "admin");
    let st = git_status(&api).await;
    assert!(st["pending"].as_array().unwrap().is_empty());
    assert_eq!(st["last_applied_commit"], json!(c2));
    // an approval can't be used twice
    assert_eq!(
        api.post(&format!("/declarative/gitops/approve/{hash2}"), json!({}))
            .await
            .0,
        404
    );

    // drift in approve mode is shown as a pending plan, not auto-corrected
    assert_eq!(
        api.put("/vms/ga/config", json!({"memory": 300})).await.0,
        200
    );
    let (_, o) = api.post("/declarative/gitops/sync", json!({})).await;
    let hash3 = o["pending"]
        .as_str()
        .expect("drift shows up as pending")
        .to_string();
    assert_eq!(
        vm_memory(&api, "ga").await,
        Some(300),
        "not corrected without approval"
    );
    let st = git_status(&api).await;
    assert_eq!(kinds(&st["pending"][0]["plan"]), ["update vm ga"]);
    // reject: gone, remembered, not queued again for the same commit
    let (s, _) = api
        .post(
            &format!("/declarative/gitops/reject/{hash3}"),
            json!({"reason": "keep the hand change"}),
        )
        .await;
    assert_eq!(s, 200);
    let st = git_status(&api).await;
    assert!(st["pending"].as_array().unwrap().is_empty());
    assert_eq!(st["rejected_commit"], json!(c2));
    assert!(st["last_result"]
        .as_str()
        .unwrap()
        .contains("keep the hand change"));
    let (_, o) = api.post("/declarative/gitops/sync", json!({})).await;
    assert!(o["pending"].is_null(), "{o}");
    assert_eq!(vm_memory(&api, "ga").await, Some(300));
    // a new commit is queued again
    repo.commit("horcrux.yaml", &vm_spec_text("ga", 320), "next");
    let (_, o) = api.post("/declarative/gitops/sync", json!({})).await;
    assert!(o["pending"].is_string(), "{o}");
}

#[tokio::test]
async fn gitops_signed_commits_and_webhook() {
    let dir = scratch("declarative-gitops-signed");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;
    let repo = Repo::new(&dir);
    let (key, public) = ssh_key(&dir, "ci_key");
    let (rogue, _) = ssh_key(&dir, "rogue_key");
    let secret_file = dir.join("hook-secret");
    std::fs::write(&secret_file, "s3cret-value\n").unwrap();
    let (s, v) = api
        .put(
            "/declarative/gitops/config",
            json!({"url": repo.url(), "mode": "auto", "interval_secs": 3600,
                   "verify_signatures": true,
                   "signing": {"format": "ssh",
                               "allowed_signers": format!("ci@example.com {public}")},
                   "webhook_secret": format!("file:{}", secret_file.display())}),
        )
        .await;
    assert_eq!(s, 200, "{v}");

    // a commit signed by an allowed key is applied; the signer is shown
    let c1 = repo.commit_signed(&key, "horcrux.yaml", &vm_spec_text("gs", 128), "signed");
    let (s, o) = api.post("/declarative/gitops/sync", json!({})).await;
    assert_eq!(s, 200, "{o}");
    assert_eq!(vm_memory(&api, "gs").await, Some(128));
    let st = git_status(&api).await;
    assert_eq!(st["signature"], "good", "{st}");
    assert!(st["signer"].as_str().unwrap().len() > 3, "{st}");

    // an unsigned commit is refused, with the reason, and nothing changes
    let c2 = repo.commit("horcrux.yaml", &vm_spec_text("gs", 256), "unsigned");
    let (s, o) = api.post("/declarative/gitops/sync", json!({})).await;
    assert_eq!(s, 422, "{o}");
    assert!(
        o.to_string().contains("signature verification failed"),
        "{o}"
    );
    assert_eq!(vm_memory(&api, "gs").await, Some(128));
    let st = git_status(&api).await;
    assert_eq!(st["signature"], "bad");
    assert_eq!(st["last_commit"], json!(c2));
    assert_eq!(st["last_applied_commit"], json!(c1));
    assert!(st["last_error"]
        .as_str()
        .unwrap()
        .contains("signature verification failed"));

    // signed by a key nobody allowed: refused too
    repo.commit_signed(&rogue, "horcrux.yaml", &vm_spec_text("gs", 384), "rogue");
    let (s, o) = api.post("/declarative/gitops/sync", json!({})).await;
    assert_eq!(s, 422, "{o}");
    assert_eq!(vm_memory(&api, "gs").await, Some(128));

    // a signed commit on top is accepted again
    repo.commit_signed(
        &key,
        "horcrux.yaml",
        &vm_spec_text("gs", 192),
        "signed again",
    );
    let (s, _) = api.post("/declarative/gitops/sync", json!({})).await;
    assert_eq!(s, 200);
    assert_eq!(vm_memory(&api, "gs").await, Some(192));

    // webhook: HMAC-authenticated, wakes the sync loop
    let hook = format!("{}/declarative/gitops/webhook", srv.base());
    let body = br#"{"ref":"refs/heads/main"}"#;
    let c = srv.client();
    let r = c.post(&hook).body(body.to_vec()).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 401, "no signature");
    let r = c
        .post(&hook)
        .header("X-Hub-Signature-256", "sha256=00ff")
        .body(body.to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 401, "bad signature");
    let sign = |b: &[u8]| horcrux_api::webhooks::sign_payload("s3cret-value", b);
    let other = br#"{"ref":"refs/heads/feature"}"#;
    let r = c
        .post(&hook)
        .header(
            "X-Forgejo-Signature",
            sign(other).trim_start_matches("sha256="),
        )
        .body(other.to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["accepted"], false, "other branches are ignored: {v}");

    let c5 = repo.commit_signed(
        &key,
        "horcrux.yaml",
        &vm_spec_text("gs", 224),
        "via webhook",
    );
    let r = c
        .post(&hook)
        .header("X-Hub-Signature-256", sign(body))
        .body(body.to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["accepted"], true, "{v}");
    // the background loop syncs without anybody calling /sync
    let mut ok = false;
    for _ in 0..30 {
        let st = git_status(&api).await;
        if st["last_applied_commit"] == json!(c5) {
            ok = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    assert!(
        ok,
        "the webhook woke the sync loop: {}",
        git_status(&api).await
    );
    assert_eq!(vm_memory(&api, "gs").await, Some(224));
}

// ===================================================================== kinds

fn errors(v: &Value) -> String {
    v["plan"]["errors"]
        .as_array()
        .or_else(|| v["errors"].as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .collect::<Vec<_>>()
                .join(" | ")
        })
        .unwrap_or_default()
}

fn all_applied(v: &Value, except: &[&str]) {
    assert_eq!(v["plan"]["errors"].as_array().map(Vec::len), Some(0), "{v}");
    for r in v["results"].as_array().unwrap() {
        if except.contains(&r["kind"].as_str().unwrap()) {
            continue;
        }
        assert_eq!(r["status"], "applied", "{r}");
    }
}

#[tokio::test]
async fn users_groups_roles_acl_apply_with_real_logins_and_safety_rules() {
    use crate::support::{admin_password, Api};
    let dir = scratch("declarative-access");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;
    let alice = "agent-test-decl-alice";
    let sysop = "agent-test-decl-sysop";
    let pwfile = dir.join("pw");
    std::fs::write(&pwfile, "first-secret-pw-1\n").unwrap();
    let spec = format!(
        "roles:\n  - {{name: Operator, description: Run guests, privileges: [VM.Audit, VM.PowerMgmt]}}\n  - {{name: SysOnly, privileges: [Sys.Audit, Sys.Modify]}}\n\
         users:\n  - {{username: admin}}\n  - {{username: {alice}, password_ref: {{file: {pw}}}}}\n  - {{username: {sysop}, password_ref: {{file: {pw}}}}}\n\
         groups:\n  - {{name: decl-ops, members: [{alice}]}}\n\
         acl:\n  - {{path: /, subject_type: user, subject: admin, role: Administrator}}\n  - {{path: /vms, subject_type: group, subject: decl-ops, role: Operator}}\n  - {{path: /, subject_type: user, subject: {sysop}, role: SysOnly}}\n",
        pw = pwfile.display()
    );

    // plan: nothing happens, no secret is shown
    let (s, v) = api.post("/declarative/plan", json!({"spec": spec})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["in_sync"], false, "{v}");
    let k = kinds(&v["plan"]);
    for want in [
        "create role Operator",
        "create user agent-test-decl-alice",
        "create group decl-ops",
        "create acl /vms group decl-ops Operator",
    ] {
        assert!(k.contains(&want.to_string()), "{want} in {k:?}");
    }
    assert!(!v.to_string().contains("first-secret"), "{v}");
    assert!(api
        .get("/users")
        .await
        .1
        .as_array()
        .unwrap()
        .iter()
        .all(|u| u["username"] != alice));

    // apply: real accounts that can log in
    let (s, v) = api.post("/declarative/apply", json!({"spec": spec})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["ok"], true, "{v}");
    all_applied(&v, &[]);
    let users = api.get("/users").await.1;
    assert!(
        users
            .as_array()
            .unwrap()
            .iter()
            .any(|u| u["username"] == alice),
        "{users}"
    );
    assert!(api
        .get("/roles")
        .await
        .1
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["name"] == "Operator"));
    let groups = api.get("/groups").await.1;
    assert_eq!(groups[0]["members"], json!([alice]), "{groups}");
    let acl = api.get("/acl").await.1;
    assert!(
        acl.as_array()
            .unwrap()
            .iter()
            .any(|a| a["path"] == "/vms" && a["subject"] == "decl-ops" && a["role"] == "Operator"),
        "{acl}"
    );
    let c = srv.client();
    Api::try_login(c.clone(), &srv.base(), alice, "first-secret-pw-1")
        .await
        .expect("the declared password works");
    assert!(
        Api::try_login(c.clone(), &srv.base(), alice, "wrong-password-1")
            .await
            .is_err()
    );

    // idempotent (the password reference is checked against the live hash)
    let (_, v) = api.post("/declarative/plan", json!({"spec": spec})).await;
    assert_eq!(v["in_sync"], true, "{v}");

    // rotate the secret: drift is found and fixed, the old password dies
    std::fs::write(&pwfile, "second-secret-pw-2\n").unwrap();
    let (_, v) = api.post("/declarative/plan", json!({"spec": spec})).await;
    assert!(
        kinds(&v["plan"]).contains(&"update user agent-test-decl-alice".to_string()),
        "{v}"
    );
    assert!(!v.to_string().contains("second-secret"), "{v}");
    let (_, v) = api.post("/declarative/apply", json!({"spec": spec})).await;
    assert_eq!(v["ok"], true, "{v}");
    Api::try_login(c.clone(), &srv.base(), alice, "second-secret-pw-2")
        .await
        .expect("rotated");
    assert!(
        Api::try_login(c.clone(), &srv.base(), alice, "first-secret-pw-1")
            .await
            .is_err()
    );

    // export never leaks hashes by default; with the flag an administrator gets
    // them and the result is still a fixed point
    let (_, ex) = api.get("/declarative/export").await;
    let text = ex["spec"].as_str().unwrap();
    assert!(
        !text.contains("argon2") && !text.contains("password"),
        "{text}"
    );
    assert!(text.contains(alice));
    let strip = |t: &str| without(t, &["containers", "volumes"]);
    let (_, v) = api
        .post(
            "/declarative/plan",
            json!({"spec": strip(text), "prune": true}),
        )
        .await;
    assert_eq!(v["in_sync"], true, "{v}");
    let (s, ex) = api.get("/declarative/export?include_hashes=true").await;
    assert_eq!(s, 200, "{ex}");
    let with = ex["spec"].as_str().unwrap();
    assert!(with.contains("$argon2"), "hashes requested");
    let (_, v) = api
        .post(
            "/declarative/plan",
            json!({"spec": strip(with), "prune": true}),
        )
        .await;
    assert_eq!(v["in_sync"], true, "{v}");

    // safety 1: a prune that would leave no administrator is refused whole
    let before = api.get("/users").await.1.as_array().unwrap().len();
    let lonely = format!(
        "users:\n  - {{username: {alice}, password_ref: {{file: {}}}}}\n",
        pwfile.display()
    );
    let (s, v) = api
        .post("/declarative/apply", json!({"spec": lonely, "prune": true}))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["ok"], false, "{v}");
    assert!(errors(&v).contains("administrator"), "{v}");
    assert!(v["results"].as_array().unwrap().is_empty(), "{v}");
    assert_eq!(api.get("/users").await.1.as_array().unwrap().len(), before);
    // safety 2: the calling user is never disabled or deleted through apply
    let (_, v) = api
        .post(
            "/declarative/plan",
            json!({"spec": "users: [{username: admin, enabled: false}]"}),
        )
        .await;
    assert!(
        kinds(&v["plan"]).contains(&"blocked user admin".to_string()),
        "{v}"
    );
    assert!(!errors(&v).is_empty(), "{v}");

    // safety 3: Sys.Modify alone can't touch access control, and sees none of it
    let op = Api::login_with(c.clone(), &srv.base(), sysop, "second-secret-pw-2").await;
    let (s, v) = op
        .post(
            "/declarative/apply",
            json!({"spec": "groups: [{name: decl-evil}]"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["ok"], false, "{v}");
    assert!(errors(&v).contains("Permissions.Modify"), "{v}");
    assert!(api
        .get("/groups")
        .await
        .1
        .as_array()
        .unwrap()
        .iter()
        .all(|g| g["name"] != "decl-evil"));
    let (s, ex) = op.get("/declarative/export").await;
    assert_eq!(s, 200, "{ex}");
    assert!(!ex["spec"].as_str().unwrap().contains(alice), "{ex}");
    let (s, _) = op.get("/declarative/export?include_hashes=true").await;
    assert_eq!(s, 403);
    let _ = admin_password();

    // teardown through prune: the declared users, group, grants and roles go;
    // the built-in roles and the caller stay
    let down = "roles: []\nusers: [{username: admin}]\ngroups: []\nacl: [{path: /, subject_type: user, subject: admin, role: Administrator}]\n";
    let (_, v) = api
        .post("/declarative/apply", json!({"spec": down, "prune": true}))
        .await;
    assert_eq!(v["ok"], true, "{v}");
    let users = api.get("/users").await.1;
    assert_eq!(users.as_array().unwrap().len(), 1, "{users}");
    assert!(api.get("/groups").await.1.as_array().unwrap().is_empty());
    let roles = api.get("/roles").await.1;
    assert!(roles
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["name"] != "Operator" && r["name"] != "SysOnly"));
    assert!(roles
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["name"] == "Administrator"));
}

#[tokio::test]
async fn firewall_applies_orders_and_refuses_to_cut_the_api() {
    let dir = scratch("declarative-fw");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;
    let spec = "firewall:\n  aliases: [{name: decl-office, cidr: 192.0.2.0/24}]\n  ipsets: [{name: decl-admins, entries: [192.0.2.10, 192.0.2.11]}]\n  security_groups:\n    - {name: decl-web, rules: [{id: https, action: ACCEPT, protocol: tcp, dport: '443'}]}\n  rules:\n    - {scope: datacenter, id: decl-ssh, action: ACCEPT, protocol: tcp, source: decl-office, dport: '22'}\n    - {scope: datacenter, id: decl-http, action: ACCEPT, protocol: tcp, source: '+decl-admins', dport: '80'}\n";
    let (s, v) = api.post("/declarative/apply", json!({"spec": spec})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["plan"]["errors"].as_array().map(Vec::len), Some(0), "{v}");
    // loading into nftables needs nft; everything else is plain config
    for r in v["results"].as_array().unwrap() {
        if r["kind"] == "fw_reload" {
            assert!(
                r["status"] == "applied"
                    || r["message"]
                        .as_str()
                        .unwrap()
                        .to_lowercase()
                        .contains("nft"),
                "{r}"
            );
        } else {
            assert_eq!(r["status"], "applied", "{r}");
        }
    }
    let ids = |v: Value| -> Vec<String> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(
        ids(api.get("/firewall/rules").await.1),
        ["decl-ssh", "decl-http"]
    );
    assert!(api
        .get("/firewall/aliases")
        .await
        .1
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["name"] == "decl-office"));
    assert!(api
        .get("/firewall/ipsets")
        .await
        .1
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["name"] == "decl-admins"));
    assert!(api
        .get("/firewall/security-groups")
        .await
        .1
        .as_array()
        .unwrap()
        .iter()
        .any(|g| g["name"] == "decl-web"));

    // idempotent, and the export is a fixed point
    let (_, v) = api.post("/declarative/plan", json!({"spec": spec})).await;
    assert_eq!(v["in_sync"], true, "{v}");
    let (_, ex) = api.get("/declarative/export").await;
    let text = ex["spec"].as_str().unwrap();
    let (_, v) = api
        .post(
            "/declarative/plan",
            json!({"spec": without(text, &["containers", "volumes"]), "prune": true}),
        )
        .await;
    assert_eq!(v["in_sync"], true, "{v}");

    // a security group can't be edited yet: said, not skipped
    let edited = spec.replace("dport: '443'", "dport: '8443'");
    let (_, v) = api.post("/declarative/plan", json!({"spec": edited})).await;
    assert_eq!(
        kinds(&v["plan"]),
        ["blocked fw_security_group decl-web"],
        "{v}"
    );

    // order matters: swapping the rules re-adds them in the new order
    let swapped = "firewall:\n  aliases: [{name: decl-office, cidr: 192.0.2.0/24}]\n  ipsets: [{name: decl-admins, entries: [192.0.2.10, 192.0.2.11]}]\n  rules:\n    - {scope: datacenter, id: decl-http, action: ACCEPT, protocol: tcp, source: '+decl-admins', dport: '80'}\n    - {scope: datacenter, id: decl-ssh, action: ACCEPT, protocol: tcp, source: decl-office, dport: '22'}\n";
    let (_, v) = api
        .post("/declarative/apply", json!({"spec": swapped}))
        .await;
    assert_eq!(
        kinds(&v["plan"]),
        [
            "update fw_rule datacenter/decl-http",
            "update fw_rule datacenter/decl-ssh",
            "update fw_reload datacenter"
        ],
        "{v}"
    );
    assert_eq!(
        ids(api.get("/firewall/rules").await.1),
        ["decl-http", "decl-ssh"]
    );

    // anti-lockout: a rule that drops the API port is refused, nothing added
    let port = srv.port;
    let cut = format!(
        "firewall:\n  rules:\n    - {{scope: datacenter, id: decl-http, action: ACCEPT, protocol: tcp, source: '+decl-admins', dport: '80'}}\n    - {{scope: datacenter, id: decl-ssh, action: ACCEPT, protocol: tcp, source: decl-office, dport: '22'}}\n    - {{scope: datacenter, id: decl-cut, action: DROP, protocol: tcp, dport: '{port}'}}\n"
    );
    let (s, v) = api.post("/declarative/apply", json!({"spec": cut})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["ok"], false, "{v}");
    assert!(errors(&v).contains("cut this session off"), "{v}");
    assert!(v["results"].as_array().unwrap().is_empty());
    assert_eq!(
        ids(api.get("/firewall/rules").await.1),
        ["decl-http", "decl-ssh"]
    );
    // a range that covers it is refused too; an explicit override lifts the refusal
    // (planned only: it would really cut this test server off)
    let range = cut.replace(&format!("'{port}'"), "'1024:65535'");
    let (_, v) = api.post("/declarative/plan", json!({"spec": range})).await;
    assert!(errors(&v).contains("cut this session off"), "{v}");
    let forced = format!("{cut}  allow_lockout: true\n");
    let (_, v) = api.post("/declarative/plan", json!({"spec": forced})).await;
    assert_eq!(errors(&v), "", "{v}");

    // teardown by prune: rules go before the sets they use
    let down = "firewall:\n  aliases: []\n  ipsets: []\n  rules: []\n";
    let (_, v) = api
        .post("/declarative/apply", json!({"spec": down, "prune": true}))
        .await;
    assert_eq!(v["plan"]["errors"].as_array().map(Vec::len), Some(0), "{v}");
    for r in v["results"].as_array().unwrap() {
        if r["kind"] != "fw_reload" {
            assert_eq!(r["status"], "applied", "{r}");
        }
    }
    assert!(api
        .get("/firewall/rules")
        .await
        .1
        .as_array()
        .unwrap()
        .is_empty());
    assert!(api
        .get("/firewall/aliases")
        .await
        .1
        .as_array()
        .unwrap()
        .is_empty());
    assert!(api
        .get("/firewall/ipsets")
        .await
        .1
        .as_array()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn backup_jobs_and_ha_groups_converge_and_prune() {
    let dir = scratch("declarative-backup-ha");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;
    let spec = "backup_jobs:\n  - {id: decl-nightly, schedule: '0 2 * * *', targets: [decl-guest], retention: {keep_last: 3}, notify: failure}\nha_groups:\n  - {name: decl-g1, nodes: [n1, n2], restricted: true}\n";
    let (s, v) = api.post("/declarative/apply", json!({"spec": spec})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["ok"], true, "{v}");
    all_applied(&v, &[]);
    let jobs = api.get("/backup-jobs").await.1;
    assert_eq!(jobs[0]["id"], "decl-nightly", "{jobs}");
    assert_eq!(jobs[0]["retention"]["keep_last"], 3);
    assert_eq!(jobs[0]["compression"], "zstd");
    let groups = api.get("/ha/groups").await.1;
    assert_eq!(groups[0]["name"], "decl-g1", "{groups}");
    assert_eq!(groups[0]["restricted"], true);

    let (_, v) = api.post("/declarative/plan", json!({"spec": spec})).await;
    assert_eq!(v["in_sync"], true, "{v}");

    // change both: one update each
    let changed = spec
        .replace("0 2 * * *", "30 3 * * *")
        .replace("[n1, n2]", "[n1, n3]");
    let (_, v) = api
        .post("/declarative/apply", json!({"spec": changed}))
        .await;
    assert_eq!(
        kinds(&v["plan"]),
        ["update backup_job decl-nightly", "update ha_group decl-g1"],
        "{v}"
    );
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(api.get("/backup-jobs").await.1[0]["schedule"], "30 3 * * *");
    assert_eq!(
        api.get("/ha/groups").await.1[0]["nodes"],
        json!(["n1", "n3"])
    );
    // the run history of a job is server state, not drift
    let (_, v) = api
        .post("/declarative/plan", json!({"spec": changed}))
        .await;
    assert_eq!(v["in_sync"], true, "{v}");

    // export is a fixed point, defaults omitted
    let (_, ex) = api.get("/declarative/export").await;
    let text = ex["spec"].as_str().unwrap();
    assert!(
        text.contains("decl-nightly") && text.contains("decl-g1"),
        "{text}"
    );
    let (_, v) = api
        .post(
            "/declarative/plan",
            json!({"spec": without(text, &["containers", "volumes"]), "prune": true}),
        )
        .await;
    assert_eq!(v["in_sync"], true, "{v}");

    // unlisted jobs/groups survive without prune, go with it
    let (_, v) = api
        .post(
            "/declarative/apply",
            json!({"spec": "backup_jobs: []\nha_groups: []\n"}),
        )
        .await;
    assert!(v["in_sync"].as_bool().unwrap_or(true), "{v}");
    assert_eq!(api.get("/backup-jobs").await.1.as_array().unwrap().len(), 1);
    let (_, v) = api
        .post(
            "/declarative/apply",
            json!({"spec": "backup_jobs: []\nha_groups: []\n", "prune": true}),
        )
        .await;
    assert_eq!(v["ok"], true, "{v}");
    assert!(api
        .get("/backup-jobs")
        .await
        .1
        .as_array()
        .unwrap()
        .is_empty());
    assert!(api.get("/ha/groups").await.1.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn containers_and_volumes_use_a_real_runtime_or_say_what_the_node_lacks() {
    let dir = scratch("declarative-containers");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;
    let name = format!("decl-ct-{}", std::process::id());
    let vol = format!("decl-vol-{}", std::process::id());
    let rts = api.get("/containers/runtimes").await.1;
    let usable: Vec<String> = rts["runtimes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["available"] == true)
        .map(|r| r["runtime"].as_str().unwrap().to_string())
        .filter(|r| r == "podman" || r == "docker")
        .collect();
    let spec = |env: &str, extra: &str| {
        format!(
            "containers:\n  - name: {name}\n    image: busybox:latest\n    command: [sleep, '300']\n    env: {{DECL: {env}}}\n    restart: unless-stopped\n    power: running\n{extra}volumes:\n  - name: {vol}\n"
        )
    };

    // bad specs are 422 and touch nothing
    let (s, _) = api
        .post(
            "/declarative/plan",
            json!({"spec": "containers: [{name: a, image: x, env: {API_TOKEN: abc}}]"}),
        )
        .await;
    assert_eq!(s, 422);

    if usable.is_empty() {
        // no podman, no docker: the plan says so with the same reason code as the 424
        let (s, v) = api
            .post("/declarative/apply", json!({"spec": spec("one", "")}))
            .await;
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["ok"], false, "{v}");
        let blocked: Vec<_> = v["results"].as_array().unwrap().iter().collect();
        assert!(!blocked.is_empty());
        for r in blocked {
            assert_eq!(r["status"], "skipped", "{r}");
            assert!(r["message"].as_str().unwrap().contains("node lacks"), "{r}");
        }
        return;
    }

    // the default runtime is podman, falling back to docker
    let want = if usable.contains(&"podman".to_string()) {
        "podman"
    } else {
        "docker"
    };
    let (s, v) = api
        .post("/declarative/plan", json!({"spec": spec("one", "")}))
        .await;
    assert_eq!(s, 200, "{v}");
    let ct = v["plan"]["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["kind"] == "container" && c["action"] == "create")
        .unwrap_or_else(|| panic!("a container create in {v}"));
    assert!(
        ct["details"][0].as_str().unwrap().starts_with(want),
        "the default runtime is {want}: {v}"
    );
    let (s, v) = api
        .post("/declarative/apply", json!({"spec": spec("one", "")}))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["ok"], true, "{v}");
    all_applied(&v, &[]);
    let cts = api.get("/containers").await.1;
    let ct = cts
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name.as_str())
        .expect("declared container exists");
    assert_eq!(ct["runtime"], want, "{ct}");
    assert_eq!(ct["status"], "running", "{ct}");
    assert_eq!(ct["restart"], "unless-stopped", "{ct}");
    let vols = api.get(&format!("/volumes?runtime={want}")).await.1;
    assert!(
        vols.as_array()
            .unwrap()
            .iter()
            .any(|x| x["name"] == vol.as_str()),
        "{vols}"
    );

    // idempotent
    let (_, v) = api
        .post("/declarative/plan", json!({"spec": spec("one", "")}))
        .await;
    assert_eq!(v["in_sync"], true, "{v}");

    // a changed env is blocked until `replace: true`, then it is re-created and runs again
    let (_, v) = api
        .post("/declarative/apply", json!({"spec": spec("two", "")}))
        .await;
    assert_eq!(v["ok"], false, "{v}");
    assert_eq!(v["results"][0]["status"], "skipped", "{v}");
    let (_, v) = api
        .post(
            "/declarative/apply",
            json!({"spec": spec("two", "    replace: true\n")}),
        )
        .await;
    assert_eq!(v["ok"], true, "{v}");
    let ct = api.get(&format!("/containers/{name}")).await.1;
    assert_eq!(ct["status"], "running", "{ct}");
    assert!(
        ct["env"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e == "DECL=two"),
        "{ct}"
    );

    // power: stopped stops it
    let stopped = spec("two", "").replace("power: running", "power: stopped");
    let (_, v) = api
        .post("/declarative/apply", json!({"spec": stopped}))
        .await;
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(
        api.get(&format!("/containers/{name}")).await.1["status"],
        "stopped"
    );

    // export: a fixed point for the container, no secret env anywhere
    let (_, ex) = api.get("/declarative/export").await;
    let text = ex["spec"].as_str().unwrap();
    assert!(text.contains(&name), "{text}");
    // prune is only ever planned here: the CI host's other containers are not ours
    let (s, v) = api
        .post(
            "/declarative/apply",
            json!({"spec": "containers: []", "prune": true, "dry_run": true}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(v["results"].as_array().unwrap().is_empty());

    // cleanup through the REST API
    let _ = api.delete(&format!("/containers/{name}")).await;
    let _ = api.delete(&format!("/volumes/{vol}?runtime={want}")).await;
}
