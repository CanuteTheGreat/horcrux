//! Security suite end to end against the real binary: a vulnerability scan
//! of an LXC root file system through a stand-in scanner that prints
//! Grype's JSON, the 424 when no scanner is installed, and the hardening
//! baseline (evaluate, dry run, apply, report, revert) on a fake host root.

use crate::support::*;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

const GRYPE_JSON: &str = r#"{"matches":[
 {"vulnerability":{"id":"CVE-2099-0001","severity":"Critical","fix":{"versions":["1.0.1"],"state":"fixed"},"dataSource":"https://example.invalid/CVE-2099-0001"},
  "artifact":{"name":"libdemo","version":"1.0.0","type":"apk"}},
 {"vulnerability":{"id":"CVE-2099-0002","severity":"Low","fix":{"versions":[],"state":"not-fixed"}},
  "artifact":{"name":"demo-tools","version":"2.0","type":"apk"}}],
 "descriptor":{"name":"grype","version":"9.9.9"}}"#;

fn write(path: &std::path::Path, text: &str, mode: u32) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[tokio::test]
async fn scans_and_hardening_work_end_to_end() {
    let dir = scratch("security-suite");
    let root = dir.join("sysroot");
    write(&root.join("etc/shadow"), "root:$6$x:1::::::\n", 0o644);
    write(
        &root.join("etc/passwd"),
        "root:x:0:0::/root:/bin/sh\n",
        0o644,
    );
    write(&root.join("proc/sys/kernel/kptr_restrict"), "0\n", 0o644);

    // Stand-in scanner: answers `version` and prints Grype JSON, recording its argv.
    let args_file = dir.join("scanner-args.txt");
    let json_file = dir.join("grype.json");
    write(&json_file, GRYPE_JSON, 0o644);
    let scanner = dir.join("fake-grype");
    write(
        &scanner,
        &format!(
            "#!/bin/sh\nif [ \"$1\" = version ]; then echo 'Version: 9.9.9'; exit 0; fi\necho \"$@\" > {}\necho \"$GRYPE_DB_CACHE_DIR\" >> {}\ncat {}\n",
            args_file.display(),
            args_file.display(),
            json_file.display()
        ),
        0o755,
    );
    let lxc = dir.join("lxc");
    std::fs::create_dir_all(lxc.join("web1/rootfs/etc")).unwrap();

    let mut srv = Server::new(&dir).env("HORCRUX_SYSROOT", root.to_str().unwrap());
    srv.start();
    srv.wait_healthy().await;
    let a = srv.api().await;

    // Defaults, then point the node at the stand-in scanner and LXC dir.
    let (s, mut set) = a.get("/security/settings").await;
    assert_eq!(s, 200, "{set}");
    assert_eq!(set["vuln_scan"]["scanner"], "grype");
    set["vuln_scan"]["scanner_path"] = json!("/nonexistent/grype");
    set["vuln_scan"]["lxc_path"] = json!(lxc.to_str().unwrap());
    set["vuln_scan"]["schedule_hours"] = json!(0);
    let (s, v) = a.put("/security/settings", set.clone()).await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = a
        .post("/security/scans", json!({"kind": "lxc", "target": "web1"}))
        .await;
    assert_eq!(s, 424, "no scanner must be a 424: {v}");
    assert!(v.to_string().contains("scanner-missing"), "{v}");

    set["vuln_scan"]["scanner_path"] = json!(scanner.to_str().unwrap());
    let (s, v) = a.put("/security/settings", set.clone()).await;
    assert_eq!(s, 200, "{v}");
    let mut bad = set.clone();
    bad["vuln_scan"]["scanner"] = json!("clamav");
    let (s, _) = a.put("/security/settings", bad).await;
    assert_eq!(s, 400);

    let (s, t) = a.get("/security/targets").await;
    assert_eq!(s, 200, "{t}");
    assert!(
        t.as_array()
            .unwrap()
            .iter()
            .any(|x| x["kind"] == "lxc" && x["name"] == "web1"),
        "{t}"
    );
    let (s, v) = a
        .post(
            "/security/scans",
            json!({"kind": "lxc", "target": "../../etc"}),
        )
        .await;
    assert_eq!(s, 400, "{v}");
    let (s, v) = a
        .post(
            "/security/scans",
            json!({"kind": "lxc", "target": "nosuch"}),
        )
        .await;
    assert_eq!(s, 404, "{v}");

    let (s, v) = a
        .post("/security/scans", json!({"kind": "lxc", "target": "web1"}))
        .await;
    assert_eq!(s, 202, "{v}");
    let id = v["id"].as_str().unwrap().to_string();
    let mut scan = json!({});
    for _ in 0..100 {
        let (_, x) = a.get(&format!("/security/scans/{id}")).await;
        if x["status"] == "done" || x["status"] == "failed" {
            scan = x;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(scan["status"], "done", "{scan}");
    assert_eq!(scan["scanner_version"], "9.9.9", "{scan}");
    assert_eq!(scan["counts"]["critical"], 1, "{scan}");
    assert_eq!(scan["counts"]["low"], 1, "{scan}");
    assert_eq!(scan["counts"]["fixable"], 1, "{scan}");
    assert_eq!(scan["findings"][0]["id"], "CVE-2099-0001", "{scan}");
    let argv = std::fs::read_to_string(&args_file).unwrap();
    assert!(
        argv.contains(&format!("dir:{}/web1/rootfs -o json", lxc.display())),
        "{argv}"
    );
    assert!(argv.contains("security/scanner-db"), "db dir: {argv}");

    let (_, list) = a.get("/security/scans?kind=lxc").await;
    assert_eq!(list[0]["id"], id.as_str(), "{list}");
    let (_, t) = a.get("/security/targets").await;
    let web1 = t
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == "web1")
        .unwrap()
        .clone();
    assert_eq!(web1["last_scan"]["id"], id.as_str(), "{web1}");
    let (s, o) = a.get("/security/overview").await;
    assert_eq!(s, 200, "{o}");
    assert_eq!(o["scanner_available"], true, "{o}");
    assert!(o["counts"]["critical"].as_u64().unwrap() >= 1, "{o}");

    // Hardening baseline on the fake root.
    let (s, r) = a.get("/security/hardening").await;
    assert_eq!(s, 200, "{r}");
    let rule = |r: &serde_json::Value, id: &str| {
        r["rules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["id"] == id)
            .cloned()
            .unwrap()
    };
    assert_eq!(rule(&r, "file-shadow")["status"], "fail", "{r}");
    assert_eq!(rule(&r, "kernel-kptr-restrict")["status"], "fail", "{r}");
    let score_before = r["score"].as_f64().unwrap();

    let (s, d) = a
        .post("/security/hardening/apply", json!({"dry_run": true}))
        .await;
    assert_eq!(s, 200, "{d}");
    assert!(d["changes"].to_string().contains("kptr_restrict"), "{d}");
    assert_eq!(
        std::fs::read_to_string(root.join("proc/sys/kernel/kptr_restrict")).unwrap(),
        "0\n"
    );

    let (s, d) = a
        .post(
            "/security/hardening/apply",
            json!({"rules": ["file-shadow", "kernel-kptr-restrict", "ssh-root-password"]}),
        )
        .await;
    assert_eq!(s, 200, "{d}");
    assert!(
        d["changes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["ok"] == true),
        "{d}"
    );
    assert!(
        d["skipped"].to_string().contains("ssh-root-password"),
        "{d}"
    );
    let mode = std::fs::metadata(root.join("etc/shadow"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o640);
    let conf =
        std::fs::read_to_string(root.join("etc/sysctl.d/60-horcrux-hardening.conf")).unwrap();
    assert!(conf.contains("kernel.kptr_restrict = 1"), "{conf}");
    let (_, r2) = a.get("/security/hardening").await;
    assert_eq!(rule(&r2, "file-shadow")["status"], "pass", "{r2}");
    assert_eq!(rule(&r2, "file-shadow")["applied"], true, "{r2}");
    assert!(r2["score"].as_f64().unwrap() > score_before, "{r2}");

    let resp =
        a.c.get(format!("{}/security/hardening/report?format=text", a.base))
            .bearer_auth(&a.token)
            .send()
            .await
            .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp
        .headers()
        .get("content-disposition")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("attachment"));
    let text = resp.text().await.unwrap();
    assert!(
        text.contains("Score:") && text.contains("[PASS] file-shadow"),
        "{text}"
    );

    let (s, d) = a.post("/security/hardening/revert", json!({})).await;
    assert_eq!(s, 200, "{d}");
    let mode = std::fs::metadata(root.join("etc/shadow"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o644, "revert restores the mode");
    assert_eq!(
        std::fs::read_to_string(root.join("proc/sys/kernel/kptr_restrict")).unwrap(),
        "0\n"
    );
    assert!(!root.join("etc/sysctl.d/60-horcrux-hardening.conf").exists());

    let s = a.delete(&format!("/security/scans/{id}")).await;
    assert_eq!(s, 204);
    let (s, _) = a.get(&format!("/security/scans/{id}")).await;
    assert_eq!(s, 404);
}

async fn wait_idle(a: &Api) -> serde_json::Value {
    for _ in 0..100 {
        let (_, st) = a.get("/security/ids").await;
        if st["integrity_running"].is_null() {
            return st;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("integrity run did not finish");
}

/// A 202-accepted integrity run is asynchronous: wait for it to be idle AND
/// for the difference count to reach `want` (the status is read a moment
/// after `running` clears).
async fn wait_diff(a: &Api, want: i64) -> serde_json::Value {
    for _ in 0..100 {
        let st = wait_idle(a).await;
        if st["integrity_last_differences"].as_i64() == Some(want) {
            return st;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let st = wait_idle(a).await;
    panic!("integrity_last_differences never reached {want}: {st}");
}

const AIDE_JSON: &str = r#"{"outline":"AIDE found differences between database and filesystem!!",
 "number_of_entries":{"total":120,"added":0,"removed":0,"changed":1},
 "changed":{"/usr/bin/horcrux-api":"f > ... ....H      "},
 "details":{"/usr/bin/horcrux-api":{"size":{"old":10,"new":11},"sha256":{"old":"AAAA","new":"BBBB"}}}}"#;

const AUDIT_RAW: &str = r#"type=SYSCALL msg=audit(4102444800.123:901): arch=c000003e syscall=257 success=yes exit=3 auid=0 uid=0 exe="/usr/bin/vim" key="hx-identity"
type=PATH msg=audit(4102444800.123:901): item=1 name="/etc/passwd" inode=3 nametype=NORMAL
type=USER_AUTH msg=audit(4102444801.000:902): pid=301 uid=0 auid=4294967295 msg='op=PAM:authentication acct="mallory" exe="/usr/sbin/sshd" addr=192.0.2.9 res=failed'
"#;

/// Intrusion detection and file integrity through stand-in audit and AIDE
/// programs (the real tools run in tests/security_ids_e2e.sh): settings,
/// the rule file, event collection and dedupe, AIDE init/check/accept,
/// the events API filters and the hardening rules.
#[tokio::test]
async fn ids_and_integrity_work_end_to_end() {
    let dir = scratch("security-ids");
    let root = dir.join("sysroot");
    write(
        &root.join("etc/passwd"),
        "root:x:0:0::/root:/bin/sh\n",
        0o644,
    );
    write(&root.join("etc/shadow"), "root:*:1::::::\n", 0o640);
    std::fs::create_dir_all(root.join("etc/horcrux")).unwrap();
    std::fs::create_dir_all(root.join("etc/audit/rules.d")).unwrap();
    let bin = dir.join("bin");
    let loaded = dir.join("auditctl-loaded.rules");
    let raw = dir.join("audit.raw");
    write(&raw, AUDIT_RAW, 0o644);
    write(
        &bin.join("auditctl"),
        &format!(
            "#!/bin/sh\ncase \"$1\" in\n -s) echo 'enabled 1';;\n -D) : > {l}.d;;\n -R) cp \"$2\" {l};;\n -l) cat {l} 2>/dev/null | grep '^-[wa]';;\nesac\nexit 0\n",
            l = loaded.display()
        ),
        0o755,
    );
    write(
        &bin.join("ausearch"),
        &format!("#!/bin/sh\ncat {}\n", raw.display()),
        0o755,
    );
    let json = dir.join("aide.json");
    write(&json, AIDE_JSON, 0o644);
    let aide = dir.join("fake-aide");
    write(
        &aide,
        &format!(
            "#!/bin/sh\n[ \"$1\" = --version ] && {{ echo 'AIDE 0.19.1'; exit 0; }}\nconf=${{1#--config=}}\nd=$(dirname \"$conf\")\ncase \"$2\" in\n --init|--update) echo x > \"$d/aide.db.new\"; echo '{{}}'; exit 0;;\n --check) cat {}; exit 4;;\n --version) echo 'AIDE 0.19.1'; exit 0;;\nesac\n",
            json.display()
        ),
        0o755,
    );
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut srv = Server::new(&dir)
        .env("HORCRUX_SYSROOT", root.to_str().unwrap())
        .env("PATH", &path);
    srv.start();
    srv.wait_healthy().await;
    let a = srv.api().await;

    let (s, mut set) = a.get("/security/settings").await;
    assert_eq!(s, 200, "{set}");
    assert_eq!(set["audit"]["enabled"], false);
    assert_eq!(set["integrity"]["paths"], json!(["/usr", "/etc", "/boot"]));

    // Integrity off: 424 with the reason.
    let (s, v) = a.post("/security/integrity/init", json!({})).await;
    assert_eq!(s, 424, "{v}");
    assert!(v.to_string().contains("integrity-disabled"), "{v}");

    // Audit on: the rule file lands under the (fake) root and is loaded.
    set["audit"]["enabled"] = json!(true);
    set["audit"]["manage_service"] = json!(false);
    set["integrity"]["enabled"] = json!(true);
    set["integrity"]["aide_path"] = json!("/nonexistent/aide");
    let (s, v) = a.put("/security/settings", set.clone()).await;
    assert_eq!(s, 200, "{v}");
    let rules = std::fs::read_to_string(root.join("etc/audit/rules.d/50-horcrux.rules")).unwrap();
    assert!(
        rules.contains("-w /etc/passwd -p wa -k hx-identity"),
        "{rules}"
    );
    assert!(
        rules.contains("-w /etc/horcrux -p wa -k hx-horcrux"),
        "{rules}"
    );
    assert!(
        !rules.contains("sshd_config"),
        "missing paths are skipped: {rules}"
    );
    assert!(std::fs::read_to_string(&loaded)
        .unwrap()
        .contains("hx-identity"));

    // AIDE missing: 424.
    let (s, v) = a.post("/security/integrity/init", json!({})).await;
    assert_eq!(s, 424, "{v}");
    assert!(v.to_string().contains("aide-missing"), "{v}");

    // Collect: the watermark was set to "now" when audit was turned on, so
    // only the (future-dated) stand-in events come in, and only once.
    let (s, v) = a.post("/security/audit/collect", json!({})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["added"], 2, "{v}");
    let (_, v) = a.post("/security/audit/collect", json!({})).await;
    assert_eq!(v["added"], 0, "no duplicates: {v}");
    let (s, p) = a.get("/security/events?source=audit").await;
    assert_eq!(s, 200, "{p}");
    assert_eq!(p["total"], 2, "{p}");
    assert_eq!(p["events"][0]["kind"], "auth");
    assert_eq!(p["events"][0]["user"], "mallory");
    assert_eq!(p["events"][0]["addr"], "192.0.2.9");
    assert_eq!(p["events"][1]["path"], "/etc/passwd");
    assert_eq!(p["events"][1]["user"], "root");
    let (_, p) = a.get("/security/events?severity=high").await;
    assert_eq!(p["total"], 1, "{p}");
    let (_, p) = a.get("/security/events?q=mallory").await;
    assert_eq!(p["total"], 1, "{p}");

    // AIDE: init, check (one changed binary), accept.
    set["integrity"]["aide_path"] = json!(aide.to_str().unwrap());
    let (s, v) = a.put("/security/settings", set.clone()).await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = a.post("/security/integrity/check", json!({})).await;
    assert_eq!(s, 409, "check before init: {v}");
    let (s, v) = a.post("/security/integrity/init", json!({})).await;
    assert_eq!(s, 202, "{v}");
    let st = wait_idle(&a).await;
    assert_eq!(st["integrity_initialized"], true, "{st}");
    assert!(st["integrity_error"].is_null(), "{st}");
    assert_eq!(st["aide_version"], "0.19.1", "{st}");
    let (s, _) = a.post("/security/integrity/check", json!({})).await;
    assert_eq!(s, 202);
    let _ = wait_diff(&a, 1).await;
    let (_, p) = a
        .get("/security/events?source=integrity&kind=changed")
        .await;
    assert_eq!(p["total"], 1, "{p}");
    assert_eq!(p["events"][0]["path"], "/usr/bin/horcrux-api");
    assert_eq!(p["events"][0]["severity"], "high");
    assert!(
        p["events"][0]["detail"]
            .as_str()
            .unwrap()
            .contains("size: 10 -> 11"),
        "{p}"
    );
    let (s, _) = a.post("/security/integrity/accept", json!({})).await;
    assert_eq!(s, 202);
    let st = wait_idle(&a).await;
    assert_eq!(st["integrity_last_differences"], 0, "{st}");
    assert!(st["audit_rules_loaded"].as_u64().unwrap() > 0, "{st}");

    // Hardening: integrity active now passes.
    let (_, r) = a.get("/security/hardening").await;
    let rule = r["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == "svc-integrity")
        .unwrap()
        .clone();
    assert_eq!(rule["status"], "pass", "{rule}");

    // Audit off: our rules and the file are removed.
    set["audit"]["enabled"] = json!(false);
    let (s, v) = a.put("/security/settings", set.clone()).await;
    assert_eq!(s, 200, "{v}");
    assert!(!root.join("etc/audit/rules.d/50-horcrux.rules").exists());
    let mut bad = set.clone();
    bad["integrity"]["paths"] = json!(["relative"]);
    let (s, _) = a.put("/security/settings", bad).await;
    assert_eq!(s, 400);
}
