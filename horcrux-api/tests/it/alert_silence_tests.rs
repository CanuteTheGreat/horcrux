//! Alert silence + preview end to end against the real binary: a real alert
//! fires (VM count rule), reaches an event webhook, is silenced through the
//! API, fires again while silenced without a notification, and notifies
//! again once the silence is lifted. Preview evaluates an unsaved condition.

use crate::support::*;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn receiver() -> (u16, Arc<Mutex<Vec<String>>>) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let hits = Arc::new(Mutex::new(Vec::new()));
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

async fn vm(a: &Api, id: &str) {
    let (s, b) = a
        .post(
            "/vms",
            json!({"id": id, "name": id, "hypervisor": "qemu", "architecture": "x86_64",
                   "cpus": 1, "memory": 128, "disk_size": 1, "status": "stopped",
                   "display": {"protocol": "none"}}),
        )
        .await;
    assert_eq!(s, 201, "{b}");
}

async fn active(a: &Api) -> Vec<Value> {
    a.get("/alerts/active")
        .await
        .1
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn triggered(h: &Arc<Mutex<Vec<String>>>) -> usize {
    h.lock()
        .unwrap()
        .iter()
        .filter(|r| r.contains("alert.triggered"))
        .count()
}

#[tokio::test]
async fn preview_evaluates_an_unsaved_condition() {
    let dir = scratch("alert-preview");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let a = srv.api().await;
    vm(&a, "pv1").await;
    let cond = |m: &str, op: &str, th: f64, t: &str| json!({"metric_type": m, "operator": op, "threshold": th, "target_pattern": t, "duration_seconds": 0});
    let (s, v) = a
        .post("/alerts/preview", cond("vm_count", "greaterthan", 0.0, "*"))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["would_fire"], 1, "{v}");
    assert_eq!(v["targets"][0]["value"], 1.0, "{v}");
    let (_, v) = a
        .post("/alerts/preview", cond("vm_count", "greaterthan", 5.0, "*"))
        .await;
    assert_eq!(
        (
            v["would_fire"].clone(),
            v["targets"].as_array().unwrap().len()
        ),
        (json!(0), 1),
        "{v}"
    );
    let (_, v) = a
        .post(
            "/alerts/preview",
            cond("vm_count", "greaterthan", 0.0, "nonexistent-*"),
        )
        .await;
    assert!(
        v["targets"].as_array().unwrap().is_empty()
            && v["note"].as_str().unwrap().contains("No target"),
        "{v}"
    );
    let (_, v) = a
        .post("/alerts/preview", cond("disk_io", "greaterthan", 0.0, "*"))
        .await;
    assert!(v["note"].as_str().unwrap().contains("never fire"), "{v}");
    let (s, _) = a
        .post("/alerts/preview", cond("vm_count", "greaterthan", 0.0, " "))
        .await;
    assert!(s == 400 || s == 422, "{s}");
    // nothing was stored or fired
    assert_eq!(a.get("/alerts/rules").await.1.as_array().unwrap().len(), 0);
    assert!(active(&a).await.is_empty());
}

#[tokio::test]
async fn silenced_alert_stays_listed_but_sends_nothing_until_lifted() {
    let dir = scratch("alert-silence");
    let (port, hits) = receiver().await;
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let a = srv.api().await;

    let (s, b) = a
        .post(
            "/webhooks",
            json!({"id": "w1", "name": "w", "url": format!("http://127.0.0.1:{port}/h"),
                   "retry_count": 0, "events": ["alert_triggered"]}),
        )
        .await;
    assert!(s == 200 || s == 201, "{b}");
    let (s, b) = a
        .post(
            "/alerts/rules",
            json!({"id": "r1", "name": "any vm", "description": "", "severity": "warning", "enabled": true,
                   "condition": {"metric_type": "vm_count", "operator": "greaterthan", "threshold": 0.0,
                                 "target_pattern": "*", "duration_seconds": 0}}),
        )
        .await;
    assert!(s == 200 || s == 201, "{b}");

    vm(&a, "s1").await;
    for _ in 0..120 {
        if triggered(&hits) == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert_eq!(triggered(&hits), 1, "alert fires and reaches the webhook");
    let alerts = active(&a).await;
    assert_eq!(alerts.len(), 1, "{alerts:?}");
    let id = alerts[0]["id"].as_str().unwrap().to_string();

    // refusals
    let (s, _) = a
        .post("/alerts/no-such-alert/silence", json!({"minutes": 60}))
        .await;
    assert_eq!(s, 404);
    for m in [0, 43201] {
        let (s, v) = a
            .post(&format!("/alerts/{id}/silence"), json!({"minutes": m}))
            .await;
        assert!(s == 400 || s == 422, "{m}: {s} {v}");
    }
    assert_eq!(
        a.delete(&format!("/alerts/{id}/silence")).await,
        404,
        "not silenced yet"
    );

    let (s, v) = a
        .post(
            &format!("/alerts/{id}/silence"),
            json!({"minutes": 60, "comment": "planned"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        (
            v["rule_id"].clone(),
            v["created_by"].clone(),
            v["comment"].clone()
        ),
        (json!("r1"), json!("admin"), json!("planned"))
    );
    let (_, l) = a.get("/alerts/silences").await;
    assert_eq!(l.as_array().unwrap().len(), 1, "{l}");

    // resolve, fire again while silenced: listed, no second notification
    assert!(matches!(a.delete("/vms/s1").await, 200 | 204));
    wait_resolved(&a).await;
    vm(&a, "s2").await;
    for _ in 0..80 {
        if !active(&a).await.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    let alerts = active(&a).await;
    assert_eq!(alerts.len(), 1, "fires again: {alerts:?}");
    tokio::time::sleep(std::time::Duration::from_secs(20)).await;
    assert_eq!(
        triggered(&hits),
        1,
        "silenced: no new webhook\n{:#?}",
        hits.lock().unwrap()
    );

    // lift: the next firing notifies
    let id2 = alerts[0]["id"].as_str().unwrap().to_string();
    assert_eq!(a.delete(&format!("/alerts/{id2}/silence")).await, 204);
    assert_eq!(
        a.get("/alerts/silences").await.1.as_array().unwrap().len(),
        0
    );
    assert!(matches!(a.delete("/vms/s2").await, 200 | 204));
    wait_resolved(&a).await;
    vm(&a, "s3").await;
    for _ in 0..120 {
        if triggered(&hits) == 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert_eq!(
        triggered(&hits),
        2,
        "notified again after the silence is lifted"
    );
}

async fn wait_resolved(a: &Api) {
    for _ in 0..160 {
        if active(a).await.is_empty() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    panic!("alert did not resolve in 80 s");
}
