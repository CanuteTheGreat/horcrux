//! Notification routing end to end against the real binary: matcher trees
//! (nested any/all, calendar, severity, event type) decide which webhook
//! channel receives which event. Local mock webhooks count what arrives.

use crate::support::*;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

type Hits = Arc<Mutex<Vec<String>>>;

async fn receiver() -> (u16, Hits) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let hits: Hits = Arc::new(Mutex::new(Vec::new()));
    let h = hits.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                return;
            };
            let h = h.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 8192];
                loop {
                    let n = s.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..p]).to_lowercase();
                        let len = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if buf.len() >= p + 4 + len {
                            break;
                        }
                    }
                }
                h.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buf).to_string());
                let _ = s
                    .write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok",
                    )
                    .await;
            });
        }
    });
    (port, hits)
}

fn got(h: &Hits, title: &str) -> usize {
    h.lock()
        .unwrap()
        .iter()
        .filter(|r| r.contains(title))
        .count()
}

async fn channel(a: &Api, name: &str, port: u16) -> String {
    let (s, v) = a
        .post(
            "/notifications/channels",
            json!({"name": name, "description": null, "channel_type": "webhook", "enabled": true,
                   "config": {"type": "Webhook", "url": format!("http://127.0.0.1:{port}/hook"),
                              "method": "POST", "headers": {}, "auth": null}}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    v["id"].as_str().unwrap().to_string()
}

fn ev(kind: &str, sub: &str, sev: &str, title: &str, at: &str) -> Value {
    json!({"kind": kind, "subtype": sub, "severity": sev, "title": title, "timestamp": at})
}

#[tokio::test]
async fn matchers_route_events_to_the_chosen_channels() {
    let dir = scratch("notif-matchers");
    let (p_ops, ops) = receiver().await;
    let (p_other, other) = receiver().await;
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let a = srv.api().await;

    // upgrade/fresh start: one default matcher, same behaviour as before routing
    let (s, list) = a.get("/notifications/matchers").await;
    assert_eq!(s, 200, "{list}");
    assert_eq!(list.as_array().unwrap().len(), 1, "{list}");
    assert_eq!(list[0]["id"], "default");
    assert_eq!(list[0]["all_channels"], true);

    let ops_id = channel(&a, "ops-hook", p_ops).await;
    let other_id = channel(&a, "other-hook", p_other).await;

    // default: alerts, backups and tests reach every channel; newer sources do not
    let (s, r) = a
        .post(
            "/notifications/events",
            ev(
                "alert",
                "firing",
                "warning",
                "T-default-alert",
                "2026-11-25T10:00:00Z",
            ),
        )
        .await;
    assert_eq!(s, 200, "{r}");
    assert_eq!(
        (r["delivered"].clone(), r["failed"].clone()),
        (json!(2), json!(0)),
        "{r}"
    );
    let (_, r) = a
        .post(
            "/notifications/events",
            ev(
                "package_update",
                "updates_available",
                "info",
                "T-default-pkg",
                "2026-11-25T10:00:00Z",
            ),
        )
        .await;
    assert_eq!(r["delivered"], 0, "{r}");
    assert_eq!(got(&ops, "T-default-alert"), 1);
    assert_eq!(got(&other, "T-default-alert"), 1);
    assert_eq!(got(&ops, "T-default-pkg"), 0);

    // replace the default with explicit routing:
    //   ops   <- (package update AND Mon-Fri 09:00-17:00) OR severity >= warning
    //   other <- backup failures
    let s = a.delete("/notifications/matchers/default").await;
    assert_eq!(s, 204);
    let business = json!({
        "name": "business-hours-updates-or-problems", "mode": "any",
        "children": [
            {"type": "group", "mode": "all", "children": [
                {"type": "event_type", "kinds": ["package_update"]},
                {"type": "calendar", "weekdays": ["mon","tue","wed","thu","fri"],
                 "start": "09:00", "end": "17:00", "timezone": "UTC"}]},
            {"type": "severity", "min": "warning"}],
        "targets": [ops_id]});
    let (s, m1) = a.post("/notifications/matchers", business.clone()).await;
    assert_eq!(s, 201, "{m1}");
    let (s, _) = a
        .post(
            "/notifications/matchers",
            json!({"name": "backup-failures",
                   "children": [{"type": "event_type", "kinds": ["backup"], "subtypes": ["backup_failed"]}],
                   "targets": [other_id]}),
        )
        .await;
    assert_eq!(s, 201);
    // names are unique
    let (s, e) = a.post("/notifications/matchers", business).await;
    assert_eq!(s, 400, "{e}");

    // Wed 10:00 update: in business hours -> ops only
    // Wed 20:00 update, Sat 10:00 update, info login: nothing
    // Sat 03:00 failed backup (error): severity rule -> ops, backup rule -> other
    for (k, sub, sev, t, at) in [
        (
            "package_update",
            "updates_available",
            "info",
            "T-wed-day",
            "2026-11-25T10:00:00Z",
        ),
        (
            "package_update",
            "updates_available",
            "info",
            "T-wed-night",
            "2026-11-25T20:00:00Z",
        ),
        (
            "package_update",
            "updates_available",
            "info",
            "T-sat-day",
            "2026-11-28T10:00:00Z",
        ),
        (
            "security",
            "login_success",
            "info",
            "T-login",
            "2026-11-25T10:00:00Z",
        ),
        (
            "backup",
            "backup_failed",
            "error",
            "T-backup-fail",
            "2026-11-28T03:00:00Z",
        ),
        (
            "backup",
            "backup_ok",
            "info",
            "T-backup-ok",
            "2026-11-28T03:00:00Z",
        ),
        ("ha", "failover", "warning", "T-ha", "2026-11-28T03:00:00Z"),
    ] {
        let (s, r) = a
            .post("/notifications/events", ev(k, sub, sev, t, at))
            .await;
        assert_eq!(s, 200, "{r}");
    }
    let n = |h: &Hits, t: &str| got(h, t);
    assert_eq!(n(&ops, "T-wed-day"), 1);
    assert_eq!(n(&ops, "T-wed-night"), 0);
    assert_eq!(n(&ops, "T-sat-day"), 0);
    assert_eq!(n(&ops, "T-login"), 0);
    assert_eq!(n(&ops, "T-backup-fail"), 1);
    assert_eq!(n(&other, "T-backup-fail"), 1);
    assert_eq!(n(&ops, "T-backup-ok"), 0);
    assert_eq!(n(&other, "T-backup-ok"), 0);
    assert_eq!(n(&ops, "T-ha"), 1);
    assert_eq!(n(&other, "T-ha"), 0);
    for t in ["T-wed-day", "T-wed-night", "T-sat-day", "T-login"] {
        assert_eq!(n(&other, t), 0, "{t}");
    }

    // dry run: reasons per node, nothing is sent, `at` moves the calendar
    let before = ops.lock().unwrap().len();
    let (s, t) = a
        .post(
            "/notifications/matchers/test",
            json!({"event": ev("package_update", "updates_available", "info", "T-dry", "2026-11-25T20:00:00Z")}),
        )
        .await;
    assert_eq!(s, 200, "{t}");
    assert_eq!(t["targets"].as_array().unwrap().len(), 0, "{t}");
    let m = &t["matchers"][0];
    assert_eq!(m["matched"], false);
    assert!(
        m["trace"]["children"][0]["children"][1]["reason"]
            .as_str()
            .unwrap()
            .contains("Wed 20:00"),
        "{m}"
    );
    let (_, t) = a
        .post(
            "/notifications/matchers/test",
            json!({"event": ev("package_update", "updates_available", "info", "T-dry", "2026-11-25T20:00:00Z"),
                   "at": "2026-11-25T10:30:00Z"}),
        )
        .await;
    assert_eq!(t["targets"][0]["name"], "ops-hook", "{t}");
    // a draft matcher can be tried before it is saved, and a bad one is refused
    let (s, t) = a
        .post(
            "/notifications/matchers/test",
            json!({"event": ev("alert", "firing", "critical", "x", "2026-11-25T10:00:00Z"),
                   "matcher": {"name": "draft", "children": [{"type": "severity", "levels": ["critical"]}], "targets": [other_id]}}),
        )
        .await;
    assert_eq!(
        (s, t["matchers"][0]["matched"].clone()),
        (200, json!(true)),
        "{t}"
    );
    let (s, _) = a
        .post(
            "/notifications/matchers/test",
            json!({"event": ev("alert", "", "info", "x", "2026-11-25T10:00:00Z"),
                   "matcher": {"name": "bad", "children": [{"type": "field", "field": "title", "regex": "("}]}}),
        )
        .await;
    assert_eq!(s, 400);
    assert_eq!(
        ops.lock().unwrap().len(),
        before,
        "a dry run sent something"
    );

    // validation
    for bad in [
        json!({"name": "b1", "children": [{"type": "calendar", "start": "9am", "end": "17:00"}]}),
        json!({"name": "b2", "children": [{"type": "calendar", "timezone": "Mars/Base"}]}),
        json!({"name": "b3", "children": [{"type": "severity"}]}),
        json!({"name": "b4", "targets": ["no-such-channel"]}),
        json!({"name": "", "children": []}),
    ] {
        let (s, e) = a.post("/notifications/matchers", bad.clone()).await;
        assert_eq!(s, 400, "{bad} -> {e}");
    }

    // update + reorder + reload
    let m1_id = m1["id"].as_str().unwrap().to_string();
    let mut upd = m1.clone();
    upd["invert"] = json!(true);
    let (s, u) = a
        .put(&format!("/notifications/matchers/{m1_id}"), upd)
        .await;
    assert_eq!(s, 200, "{u}");
    assert_eq!(u["invert"], true);
    let (_, all) = a.get("/notifications/matchers").await;
    let ids: Vec<String> = all
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_string())
        .collect();
    let rev: Vec<&String> = ids.iter().rev().collect();
    let (s, _) = a
        .put("/notifications/matchers/order", json!({"ids": rev}))
        .await;
    assert_eq!(s, 204);
    let (_, all2) = a.get("/notifications/matchers").await;
    assert_eq!(all2[0]["id"], all[1]["id"]);

    // declarative: export carries the matchers by channel NAME; apply converges
    let (s, ex) = a.get("/declarative/export").await;
    assert_eq!(s, 200);
    let text = ex["spec"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| ex.to_string());
    assert!(text.contains("notification_matchers"), "{text}");
    assert!(text.contains("ops-hook"), "{text}");
    let spec = "version: 1\nnotification_matchers:\n  - name: from-spec\n    mode: any\n    children:\n      - {type: severity, min: error}\n    targets: [other-hook]\n";
    let (s, p) = a
        .post("/declarative/plan", json!({"spec": spec, "prune": true}))
        .await;
    assert_eq!(s, 200, "{p}");
    let kinds: Vec<String> = p["plan"]["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            format!(
                "{}:{}",
                c["kind"].as_str().unwrap(),
                c["action"].as_str().unwrap()
            )
        })
        .collect();
    assert!(
        kinds.contains(&"notification_matcher:create".to_string()),
        "{p}"
    );
    assert_eq!(
        kinds
            .iter()
            .filter(|k| *k == "notification_matcher:delete")
            .count(),
        2,
        "{p}"
    );
    let (s, r) = a
        .post("/declarative/apply", json!({"spec": spec, "prune": true}))
        .await;
    assert_eq!((s, r["ok"].clone()), (200, json!(true)), "{r}");
    let (_, now) = a.get("/notifications/matchers").await;
    assert_eq!(now.as_array().unwrap().len(), 1, "{now}");
    assert_eq!(now[0]["name"], "from-spec");
    assert_eq!(now[0]["targets"][0], other_id);
    let (_, p2) = a
        .post("/declarative/plan", json!({"spec": spec, "prune": true}))
        .await;
    assert_eq!(p2["in_sync"], true, "converged: {p2}");
    let (s, p3) = a
        .post(
            "/declarative/plan",
            json!({"spec": "notification_matchers:\n  - {name: x, targets: [ghost]}\n"}),
        )
        .await;
    assert!(s == 200 || s == 400, "{s}");
    assert!(
        p3.to_string().contains("not a notification channel"),
        "{p3}"
    );

    // deleting a channel removes it from matcher targets
    let s = a
        .delete(&format!("/notifications/channels/{other_id}"))
        .await;
    assert_eq!(s, 204);
    let (_, now) = a.get("/notifications/matchers").await;
    assert_eq!(now[0]["targets"].as_array().unwrap().len(), 0, "{now}");

    // survives a restart
    srv.restart_after(nix::sys::signal::Signal::SIGTERM).await;
    let a = srv.api().await;
    let (_, now) = a.get("/notifications/matchers").await;
    assert_eq!(now[0]["name"], "from-spec", "{now}");
}
