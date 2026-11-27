//! Container runtime detection end to end against the real binary, with a
//! stand-in `falco` that prints Falco's JSON lines: settings validation, the
//! generated falco.yaml, supervision (start, events, live filters, restart on
//! a setting that needs it), the cgroup limits and the 424-free "not installed"
//! status. The real Falco is exercised by the spike in docs/design/security-suite.md.

use crate::support::*;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

fn line(rule: &str, prio: &str, container: &str, t: &str) -> String {
    format!(
        r#"{{"hostname":"h","output":"{t}: {prio} {rule} happened (container={container})","output_fields":{{"container.name":"{container}","container.image.repository":"alpine","container.image.tag":"3.24","proc.name":"sh","proc.cmdline":"sh -c x","user.name":"root"}},"priority":"{prio}","rule":"{rule}","source":"syscall","time":"{t}"}}"#
    )
}

#[tokio::test]
async fn runtime_detection_supervises_falco_and_keeps_events() {
    let dir = scratch("security-runtime");
    let cg = dir.join("cgroup");
    std::fs::create_dir_all(&cg).unwrap();
    std::fs::write(cg.join("cgroup.controllers"), "cpu memory\n").unwrap();
    let starts = dir.join("starts.txt");
    let conf_copy = dir.join("falco-conf.yaml");
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let out = [
        line("HX write below /etc in container", "Error", "web", &now),
        line("HX shell in container", "Notice", "web", &now),
        line("HX crypto miner process in container", "Critical", "ci-build", &now),
        line("HX privileged container started", "Warning", "db", &now),
        r#"{"output":"Falco metrics snapshot","output_fields":{"falco.cpu_usage_perc":12.5,"falco.memory_rss_mb":133.3,"scap.evts_rate_sec":26000.0,"scap.n_drops":0},"priority":"Informational","rule":"Falco internal: metrics snapshot","source":"internal","time":"2026-11-27T12:00:00Z"}"#.to_string(),
    ]
    .join("\n");
    let lines = dir.join("falco-out.txt");
    std::fs::write(&lines, out + "\n").unwrap();
    let falco = dir.join("fake-falco");
    std::fs::write(
        &falco,
        format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'Falco version: 9.9.9'; exit 0; fi\necho start >> {}\ncp \"$2\" {}\ncat {}\nexec sleep 600\n",
            starts.display(),
            conf_copy.display(),
            lines.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&falco, std::fs::Permissions::from_mode(0o755)).unwrap();

    let mut srv = Server::new(&dir).env("HORCRUX_CGROUP_ROOT", cg.to_str().unwrap());
    srv.start();
    srv.wait_healthy().await;
    let a = srv.api().await;

    // Off by default; not installed is a status, not an error.
    let (s, st) = a.get("/security/runtime").await;
    assert_eq!(s, 200, "{st}");
    assert_eq!(st["enabled"], false);
    assert_eq!(st["running"], false);
    assert_eq!(st["available"], false);
    assert!(st["rules"].as_array().unwrap().len() >= 4, "{st}");
    let (_, set) = a.get("/security/settings").await;
    assert_eq!(set["runtime"]["enabled"], false);
    assert_eq!(set["runtime"]["memory_limit_mb"], 1024);

    // Validation.
    let mut bad = set.clone();
    bad["runtime"]["memory_limit_mb"] = json!(512);
    assert_eq!(a.put("/security/settings", bad).await.0, 400);
    let mut bad = set.clone();
    bad["runtime"]["min_priority"] = json!("debug");
    assert_eq!(a.put("/security/settings", bad).await.0, 400);

    // Enable with the stand-in; wait for the supervisor.
    let mut on = set.clone();
    on["runtime"]["enabled"] = json!(true);
    on["runtime"]["falco_path"] = json!(falco.to_str().unwrap());
    let (s, v) = a.put("/security/settings", on.clone()).await;
    assert_eq!(s, 200, "{v}");
    let t = Instant::now();
    let mut st = json!(null);
    while t.elapsed() < Duration::from_secs(60) {
        let (_, v) = a.get("/security/runtime").await;
        if v["running"] == true
            && v["events_kept"].as_u64().unwrap_or(0) >= 3
            && v["cpu_percent"] != 0.0
        {
            st = v;
            break;
        }
        st = v;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert_eq!(st["running"], true, "detector never came up: {st}");
    assert_eq!(st["version"], "9.9.9");
    assert_eq!(st["limits_applied"], true, "{st}");
    assert_eq!(st["events_per_sec"], 26000.0);
    // The notice-level event is below min_priority (warning): kept = 3.
    assert_eq!(st["events_kept"], 3, "{st}");

    // Limits written to the cgroup.
    let g = cg.join("horcrux-runtime-detection");
    assert_eq!(
        std::fs::read_to_string(g.join("cpu.max")).unwrap(),
        "100000 100000"
    );
    assert_eq!(
        std::fs::read_to_string(g.join("memory.max")).unwrap(),
        (1024u64 * 1024 * 1024).to_string()
    );
    assert!(std::fs::read_to_string(g.join("cgroup.procs"))
        .unwrap()
        .trim()
        .parse::<u32>()
        .is_ok());

    // The generated falco.yaml.
    let conf = std::fs::read_to_string(&conf_copy).unwrap();
    assert!(conf.contains("kind: modern_ebpf"), "{conf}");
    assert!(conf.contains("horcrux-rules.yaml"), "{conf}");
    let rules = std::fs::read_to_string(std::path::Path::new(
        conf.lines()
            .find(|l| l.contains("horcrux-rules.yaml"))
            .unwrap()
            .trim()
            .trim_start_matches("- ")
            .trim_matches('"'),
    ))
    .unwrap();
    assert!(rules.contains("HX crypto miner process in container"));

    // Events, newest first, with filters.
    let (s, ev) = a.get("/security/runtime/events").await;
    assert_eq!(s, 200, "{ev}");
    assert_eq!(ev.as_array().unwrap().len(), 3);
    assert_eq!(ev[0]["rule"], "HX privileged container started");
    assert_eq!(ev[0]["priority"], "warning");
    assert_eq!(ev[0]["image"], "alpine:3.24");
    let (_, ev) = a.get("/security/runtime/events?min_priority=error").await;
    assert_eq!(ev.as_array().unwrap().len(), 2);
    let (_, ev) = a.get("/security/runtime/events?container=web").await;
    assert_eq!(ev.as_array().unwrap().len(), 1);
    assert_eq!(
        ev[0]["message"]
            .as_str()
            .unwrap()
            .split(' ')
            .next()
            .unwrap(),
        "HX"
    );
    let (_, ev) = a
        .get("/security/runtime/events?rule=HX%20crypto%20miner%20process%20in%20container")
        .await;
    assert_eq!(ev.as_array().unwrap().len(), 1);
    let (_, ov) = a.get("/security/overview").await;
    assert_eq!(ov["runtime_enabled"], true);
    assert_eq!(ov["runtime_running"], true);
    assert_eq!(ov["runtime_events_24h"], 3);

    // A filter-only change applies live: no second start.
    let mut live = on.clone();
    live["runtime"]["ignore_containers"] = json!(["ci-build"]);
    assert_eq!(a.put("/security/settings", live.clone()).await.0, 200);
    tokio::time::sleep(Duration::from_secs(7)).await;
    assert_eq!(std::fs::read_to_string(&starts).unwrap().lines().count(), 1);

    // A setting that needs a restart restarts it (the stand-in prints its events again).
    let mut re = live.clone();
    re["runtime"]["memory_limit_mb"] = json!(2048);
    assert_eq!(a.put("/security/settings", re.clone()).await.0, 200);
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(40)
        && std::fs::read_to_string(&starts).unwrap().lines().count() < 2
    {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert_eq!(std::fs::read_to_string(&starts).unwrap().lines().count(), 2);
    assert_eq!(
        std::fs::read_to_string(g.join("memory.max")).unwrap(),
        (2048u64 * 1024 * 1024).to_string()
    );
    // The ignored container's event is not kept the second time.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let (_, ev) = a.get("/security/runtime/events?container=ci-build").await;
    assert_eq!(
        ev.as_array().unwrap().len(),
        1,
        "only the first run's event: {ev}"
    );

    // Clear, then turn it off: the process goes away.
    assert_eq!(a.delete("/security/runtime/events").await, 204);
    let (_, ev) = a.get("/security/runtime/events").await;
    assert!(ev.as_array().unwrap().is_empty());
    let mut off = re.clone();
    off["runtime"]["enabled"] = json!(false);
    assert_eq!(a.put("/security/settings", off).await.0, 200);
    let t = Instant::now();
    loop {
        let (_, v) = a.get("/security/runtime").await;
        if v["running"] == false {
            break;
        }
        assert!(t.elapsed() < Duration::from_secs(30), "still running: {v}");
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert!(!g.exists(), "cgroup removed when stopped");
    let (s, rules) = a.get("/security/runtime/rules").await;
    assert_eq!(s, 200);
    assert!(rules.to_string().contains("HX shell in container"));
}
