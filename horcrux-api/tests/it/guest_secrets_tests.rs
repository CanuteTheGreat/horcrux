//! Guest provisioning secrets end to end against the real binary: values are
//! write-only, encrypted on disk, absent from every export, referenced by
//! name from provisioning and the declarative spec, injected only into the
//! seed disc, rotated and audited.

use crate::support::*;
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;

const PW1: &str = "Zx9-first-Pass-w0rd";
const PW2: &str = "Qm4-second-Pass-w0rd";
const TOKEN: &str = "tok_live_8f3a1c9d77";

/// Every file under `root` that contains `needle` (also inside sqlite WAL files).
fn files_containing(root: &std::path::Path, needle: &str) -> Vec<String> {
    let mut hits = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(b) = std::fs::read(&p) {
                if b.windows(needle.len()).any(|w| w == needle.as_bytes()) {
                    hits.push(p.display().to_string());
                }
            }
        }
    }
    hits
}

fn have(bin: &str) -> bool {
    std::process::Command::new("sh")
        .args(["-c", &format!("command -v {bin}")])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[tokio::test]
async fn secrets_are_write_only_encrypted_referenced_and_audited() {
    let dir = scratch("guest-secrets");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;

    // create: the answer never carries the value
    let (s, v) = api
        .post(
            "/security/guest-secrets",
            json!({"name": "ops-pw", "kind": "password", "description": "ops login", "value": PW1}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert!(!v.to_string().contains(PW1));
    let (s, _) = api
        .post(
            "/security/guest-secrets",
            json!({"name": "api-tok", "kind": "token", "value": TOKEN}),
        )
        .await;
    assert_eq!(s, 201);
    // duplicate name, bad name, empty value
    assert_eq!(
        api.post(
            "/security/guest-secrets",
            json!({"name": "ops-pw", "value": "x"})
        )
        .await
        .0,
        409
    );
    assert_eq!(
        api.post(
            "/security/guest-secrets",
            json!({"name": "Bad Name", "value": "x"})
        )
        .await
        .0,
        400
    );
    assert_eq!(
        api.post(
            "/security/guest-secrets",
            json!({"name": "empty", "value": ""})
        )
        .await
        .0,
        400
    );
    let (s, list) = api.get("/security/guest-secrets").await;
    assert_eq!(s, 200);
    assert_eq!(list.as_array().unwrap().len(), 2);
    assert!(!list.to_string().contains(PW1) && !list.to_string().contains(TOKEN));

    // a VM, and its provisioning: references only
    let (s, v) = api
        .post(
            "/vms",
            json!({"id": "gs-vm", "name": "gs-vm", "hypervisor": "qemu",
                   "architecture": "x86_64", "cpus": 1, "memory": 64, "disk_size": 1,
                   "status": "stopped"}),
        )
        .await;
    assert!(s == 200 || s == 201, "{v}");
    let prov = json!({"cloud_init": {
        "hostname": "gsvm", "user": {"name": "ops", "password": null,
        "plain_password": "secret:ops-pw", "sudo": true, "shell": null},
        "ssh_keys": [], "network": null, "packages": [],
        "runcmd": ["echo token={{secret:api-tok}} > /root/t"]}});
    let (s, v) = api.put("/security/provisioning/gs-vm", prov.clone()).await;
    assert_eq!(s, 200, "{v}");
    // a literal password, an unknown reference and a private key are refused
    let mut bad = prov.clone();
    bad["cloud_init"]["user"]["plain_password"] = json!(PW1);
    let (s, v) = api.put("/security/provisioning/gs-vm", bad).await;
    assert_eq!(s, 400, "{v}");
    assert!(v.to_string().contains("secret reference"), "{v}");
    let mut bad = prov.clone();
    bad["cloud_init"]["runcmd"] = json!(["echo {{secret:nope}}"]);
    assert_eq!(api.put("/security/provisioning/gs-vm", bad).await.0, 400);
    let mut bad = prov.clone();
    bad["cloud_init"]["ssh_keys"] = json!(["-----BEGIN OPENSSH PRIVATE KEY-----"]);
    assert_eq!(api.put("/security/provisioning/gs-vm", bad).await.0, 400);
    // a secret that provisioning uses cannot be deleted
    assert_eq!(api.delete("/security/guest-secrets/ops-pw").await, 409);

    // the declarative export lists names and references, never a value
    let (s, ex) = api.get("/declarative/export").await;
    assert_eq!(s, 200, "{ex}");
    let text = ex["spec"].as_str().unwrap();
    assert!(
        text.contains("ops-pw") && text.contains("secret:ops-pw"),
        "{text}"
    );
    for v in [PW1, TOKEN] {
        assert!(!text.contains(v), "export leaks a value");
    }
    let (_, plan) = api.post("/declarative/plan", json!({"spec": text})).await;
    assert_eq!(plan["in_sync"], true, "{plan}");

    // a spec declaring an unknown secret is told it needs a value
    let spec =
        "version: 1\nguest_secrets:\n  secrets:\n    - {name: wanted-secret, kind: password}\n";
    let (s, plan) = api.post("/declarative/plan", json!({"spec": spec})).await;
    assert_eq!(s, 200, "{plan}");
    assert!(plan.to_string().contains("no value yet"), "{plan}");
    // a spec referencing an undeclared secret is an error
    let spec = "version: 1\nguest_secrets:\n  provisioning:\n    gs-vm:\n      cloud_init: {hostname: h, user: null, ssh_keys: [], network: null, packages: [], runcmd: ['x {{secret:ghost}}']}\n";
    let (_, plan) = api.post("/declarative/plan", json!({"spec": spec})).await;
    assert!(
        plan.to_string().contains("neither declared nor present"),
        "{plan}"
    );

    // the seed disc: built from the stored references, private, hashed password
    if have("xorriso") || have("genisoimage") || have("mkisofs") {
        let (s, v) = api
            .post("/security/provisioning/gs-vm/seed", json!({}))
            .await;
        assert_eq!(s, 200, "{v}");
        let seed = v["attached_as"]
            .as_str()
            .expect("seed attached")
            .to_string();
        let mode = std::fs::metadata(&seed).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "seed disc is private");
        // the manager's own copy is gone
        let leftovers = std::fs::read_dir(srv.data().join("cloudinit"))
            .map(|d| {
                d.flatten()
                    .filter(|e| e.file_name().to_string_lossy().ends_with(".iso"))
                    .count()
            })
            .unwrap_or(0);
        assert_eq!(leftovers, 0, "no seed copy kept by the manager");
        if have("xorriso") {
            let ud_path = dir.join("extracted-user-data");
            let out = std::process::Command::new("xorriso")
                .args(["-osirrox", "on", "-indev", &seed, "-extract", "/user-data"])
                .arg(&ud_path)
                .output()
                .expect("xorriso runs");
            let ud = std::fs::read_to_string(&ud_path).unwrap_or_else(|_| {
                panic!(
                    "user-data extracted from the seed: {}",
                    String::from_utf8_lossy(&out.stderr)
                )
            });
            assert!(ud.contains("#cloud-config"), "{ud}");
            assert!(
                ud.contains(&format!("token={TOKEN}")),
                "token injected: {ud}"
            );
            assert!(!ud.contains(PW1), "password only as a hash");
            let hash = ud
                .lines()
                .find_map(|l| l.trim().strip_prefix("passwd: "))
                .expect("passwd line")
                .trim()
                .trim_matches(|c| c == '\'' || c == '"')
                .to_string();
            let salt = hash.split('$').nth(2).unwrap().to_string();
            if have("openssl") {
                let want = run("openssl", &["passwd", "-6", "-salt", &salt, PW1]);
                assert_eq!(want.trim(), hash, "the VM gets the referenced password");
            }
            let _ = std::fs::remove_file(&ud_path);
        }
    }

    // rotation: new value, version 2, audit trail, no value in it
    let (s, v) = api
        .put("/security/guest-secrets/ops-pw", json!({"value": PW2}))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["version"], 2);
    let (s, a) = api.get("/security/guest-secrets-audit?name=ops-pw").await;
    assert_eq!(s, 200);
    let actions: Vec<String> = a
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["action"].as_str().unwrap().to_string())
        .collect();
    assert!(
        actions.contains(&"created".into()) && actions.contains(&"rotated".into()),
        "{a}"
    );
    assert!(
        a.as_array().unwrap().iter().all(|e| e["actor"] == "admin"),
        "{a}"
    );
    for v in [PW1, PW2, TOKEN] {
        assert!(!a.to_string().contains(v));
    }

    // nothing on disk (database, WAL, exports, logs) holds a value in clear
    for v in [PW1, PW2, TOKEN] {
        let hits = files_containing(&dir, v);
        // the seed disc may hold the token (it is the injection target)
        let hits: Vec<_> = hits
            .into_iter()
            .filter(|h| !h.ends_with(".cidata.iso"))
            .collect();
        assert!(hits.is_empty(), "{v} found in {hits:?}");
    }
    let key = srv.data().join("guest-secrets.key");
    assert_eq!(
        std::fs::metadata(&key).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(!srv.log_text().contains(PW1) && !srv.log_text().contains(TOKEN));

    // unprovisioning frees the secret for deletion
    assert_eq!(api.delete("/security/provisioning/gs-vm").await, 204);
    assert_eq!(api.delete("/security/guest-secrets/ops-pw").await, 204);
    let (s, _) = api.get("/security/guest-secrets/ops-pw").await;
    assert_eq!(s, 404);
    let _: Value = json!(null);
}
