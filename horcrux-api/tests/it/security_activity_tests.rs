//! Security events end to end: failed logins show up on the dashboard, the
//! brute-force detector fires after N failures from one client (journaled,
//! never blocking when blocking is off), alert rules fire, blocks refuse
//! unsafe addresses, and settings are validated.

use crate::support::*;
use serde_json::json;

async fn bad_logins(srv: &Server, n: usize, user: &str) {
    let c = srv.client();
    let url = format!("{}/auth/login", srv.base());
    let mut set = Vec::new();
    for _ in 0..n {
        let (c, url, user) = (c.clone(), url.clone(), user.to_string());
        set.push(tokio::spawn(async move {
            c.post(url)
                .json(&json!({"username": user, "password": "definitely-wrong"}))
                .send()
                .await
                .map(|r| r.status().as_u16())
                .unwrap_or(0)
        }));
    }
    for h in set {
        assert_eq!(h.await.unwrap(), 401);
    }
}

#[tokio::test]
async fn failed_logins_are_counted_detected_and_never_block_when_blocking_is_off() {
    let dir = scratch("sec-events");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let a = srv.api().await;

    // Defaults: detector on, blocking off, three starter rules.
    let (s, set) = a.get("/security/settings").await;
    assert_eq!(s, 200, "{set}");
    assert_eq!(set["detection"]["brute_force"]["block"], false, "{set}");
    assert_eq!(set["detection"]["brute_force"]["failures"], 5, "{set}");
    assert_eq!(
        set["detection"]["rules"].as_array().unwrap().len(),
        3,
        "{set}"
    );

    // Settings are validated.
    let mut bad = set.clone();
    bad["detection"]["brute_force"]["failures"] = json!(0);
    let (s, v) = a.put("/security/settings", bad).await;
    assert_eq!(s, 400, "{v}");
    let mut bad = set.clone();
    bad["detection"]["rules"][0]["kinds"] = json!(["nonsense"]);
    assert_eq!(a.put("/security/settings", bad).await.0, 400);

    // N=3 in 10 minutes, plus a rule on lockouts and failed logins together.
    let mut mine = set.clone();
    mine["detection"]["brute_force"]["failures"] = json!(3);
    mine["detection"]["rules"] = json!([{
        "id": "login-storm", "name": "Login storm", "kinds": ["login_failed", "lockout"],
        "group_by": "none", "threshold": 4, "window_minutes": 5, "severity": "critical"
    }]);
    let (s, v) = a.put("/security/settings", mine).await;
    assert_eq!(s, 200, "{v}");

    bad_logins(&srv, 4, "intruder").await;

    // The dashboard sees them, per client and per user, and the timeline adds up.
    let (s, d) = a.get("/security/activity?hours=1").await;
    assert_eq!(s, 200, "{d}");
    let t = &d["totals"];
    let n = t["failed_logins"].as_u64().unwrap() + t["lockouts"].as_u64().unwrap();
    assert!(n >= 4, "{d}");
    assert_eq!(d["top_users"][0]["key"], "intruder", "{d}");
    assert!(
        d["top_users"][0]["failed_logins"].as_u64().unwrap() >= 4,
        "{d}"
    );
    assert!(!d["top_clients"].as_array().unwrap().is_empty(), "{d}");
    let in_buckets: u64 = d["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["failed_logins"].as_u64().unwrap())
        .sum();
    assert_eq!(in_buckets, n, "{d}");
    assert_eq!(d["block_enabled"], false, "{d}");

    // Filters: by kind and user, newest first.
    let (_, e) = a
        .get("/security/activity/events?user=intruder&kind=login_failed,lockout")
        .await;
    assert!(e["total"].as_u64().unwrap() >= 4, "{e}");
    assert_eq!(e["events"][0]["user"], "intruder");
    let (_, none) = a.get("/security/activity/events?user=nobody-here").await;
    assert_eq!(none["total"], 0, "{none}");

    // Run the detector now: brute force fires once for the client and the rule fires.
    let (s, r) = a.post("/security/activity/check", json!({})).await;
    assert_eq!(s, 200, "{r}");
    assert_eq!(r["brute_force"].as_array().unwrap().len(), 1, "{r}");
    assert_eq!(r["rules"].as_array().unwrap().len(), 1, "{r}");
    assert!(
        r["rules"][0].as_str().unwrap().starts_with("login-storm:"),
        "{r}"
    );
    assert!(
        r["blocked"].as_array().unwrap().is_empty(),
        "blocking is off: {r}"
    );
    // Same window again: nothing new (cool-down).
    let (_, r2) = a.post("/security/activity/check", json!({})).await;
    assert!(r2["brute_force"].as_array().unwrap().is_empty(), "{r2}");
    assert!(r2["rules"].as_array().unwrap().is_empty(), "{r2}");

    // The detections are in the timeline (journal) and survive a restart.
    let (_, e) = a
        .get("/security/activity/events?kind=brute_force,rule_fired")
        .await;
    assert_eq!(e["total"], 2, "{e}");
    assert_eq!(
        e["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|x| x["severity"] == "critical")
            .count(),
        2,
        "{e}"
    );
    srv.restart_after(nix::sys::signal::Signal::SIGTERM).await;
    let a = srv.api().await;
    let (_, e) = a
        .get("/security/activity/events?kind=brute_force,rule_fired&since=0")
        .await;
    assert_eq!(e["total"], 2, "journal kept over a restart: {e}");

    // Blocks: unsafe or malformed addresses are refused, nothing is listed.
    for ip in ["127.0.0.1", "::1", "0.0.0.0", "not-an-ip"] {
        let (s, v) = a.post("/security/blocks", json!({"ip": ip})).await;
        assert_eq!(s, 400, "{ip}: {v}");
    }
    let (s, v) = a.get("/security/blocks").await;
    assert_eq!((s, v.as_array().map(|x| x.len())), (200, Some(0)), "{v}");
    assert_eq!(a.delete("/security/blocks?ip=198.51.100.7").await, 404);
}

#[tokio::test]
async fn events_need_a_login() {
    let dir = scratch("sec-events-rbac");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let a = srv.api().await;
    // No credentials: refused.
    let c = srv.client();
    let r = c
        .get(format!("{}/security/activity", srv.base()))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 401);
    let (s, _) = a.get("/security/activity").await;
    assert_eq!(s, 200);
}
