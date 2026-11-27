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

const HOST_GLSA: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<glsa id="209901-01">
  <title>Demo: remote code execution</title>
  <synopsis>Demo issue.</synopsis>
  <product type="ebuild">demo</product>
  <announced>2099-01-01</announced>
  <affected>
    <package name="dev-libs/demo" auto="yes" arch="*">
      <unaffected range="ge">2.0</unaffected>
      <vulnerable range="lt">2.0</vulnerable>
    </package>
  </affected>
  <impact type="high"><p>x</p></impact>
  <references><uri link="https://example.invalid/c">CVE-2099-0100</uri></references>
</glsa>"#;

#[tokio::test]
async fn host_advisories_are_matched_on_a_fake_gentoo_root() {
    let dir = scratch("security-host-cve");
    let root = dir.join("sysroot");
    write(
        &root.join("var/db/repos/gentoo/metadata/glsa/glsa-209901-01.xml"),
        HOST_GLSA,
        0o644,
    );
    write(
        &root.join("var/db/pkg/dev-libs/demo-1.5-r1/SLOT"),
        "0\n",
        0o644,
    );
    write(&root.join("var/db/pkg/sys-apps/other-3/SLOT"), "0\n", 0o644);

    let mut srv = Server::new(&dir).env("HORCRUX_SYSROOT", root.to_str().unwrap());
    srv.start();
    srv.wait_healthy().await;
    let a = srv.api().await;

    let (s, mut set) = a.get("/security/settings").await;
    assert_eq!(s, 200, "{set}");
    assert_eq!(set["host_cve"]["enabled"], true);
    assert_eq!(set["host_cve"]["schedule_hours"], 24);
    assert!(set["host_cve"]["update_command"]
        .as_str()
        .unwrap()
        .contains("--with-bdeps=y @world"));

    let (s, r) = a.post("/security/host/check", json!({})).await;
    assert_eq!(s, 200, "{r}");
    assert_eq!(r["supported"], true, "{r}");
    assert_eq!(r["installed_packages"], 2);
    assert_eq!(r["advisories"].as_array().unwrap().len(), 1, "{r}");
    let adv = &r["advisories"][0];
    assert_eq!(adv["id"], "209901-01");
    assert_eq!(adv["severity"], "high");
    assert_eq!(adv["cves"][0], "CVE-2099-0100");
    assert_eq!(adv["packages"][0]["package"], "dev-libs/demo");
    assert_eq!(adv["packages"][0]["installed"], "1.5-r1");
    assert_eq!(adv["packages"][0]["fixed"][0], ">=2.0");

    // Ignoring the advisory removes it from the list and counts it.
    set["host_cve"]["ignore"] = json!(["209901-01"]);
    let (s, v) = a.put("/security/settings", set.clone()).await;
    assert_eq!(s, 200, "{v}");
    let (s, r) = a.post("/security/host/check", json!({})).await;
    assert_eq!(s, 200, "{r}");
    assert_eq!(r["advisories"].as_array().unwrap().len(), 0, "{r}");
    assert_eq!(r["ignored"], 1);

    // Invalid settings are refused with the reason.
    set["host_cve"]["glsa_dir"] = json!("relative/path");
    let (s, v) = a.put("/security/settings", set).await;
    assert_eq!(s, 400, "{v}");
    assert!(v.to_string().contains("host_cve.glsa_dir"), "{v}");

    // GET serves the cached report.
    let (s, r) = a.get("/security/host").await;
    assert_eq!(s, 200, "{r}");
    assert_eq!(r["ignored"], 1);
}
