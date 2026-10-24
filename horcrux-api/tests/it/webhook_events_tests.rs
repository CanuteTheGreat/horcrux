//! Event webhooks end to end: the real `horcrux-api` binary, a real HTTP
//! receiver, real VM / storage / login requests.
//!
//! - vm.created, vm.deleted, storage.pool.added and auth.failed deliveries
//!   reach the receiver (with the stored URL's path + query and an HMAC
//!   signature) and the webhook's counters go up;
//! - the API (what an auditor reads) never shows the URL's userinfo
//!   password or query value, and an update that sends the masked URL back
//!   keeps the stored one;
//! - after SIGKILL + restart the webhook is still there and still delivers.

use crate::support::*;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone, Debug)]
struct Hit {
    request_line: String,
    headers: String,
    body: Value,
}

/// A receiver that answers 200 to everything and records each request.
async fn receiver() -> (u16, Arc<Mutex<Vec<Hit>>>) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let hits: Arc<Mutex<Vec<Hit>>> = Arc::new(Mutex::new(Vec::new()));
    let h = hits.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                return;
            };
            let h = h.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                let (head_end, len) = loop {
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
                        break (p + 4, len);
                    }
                };
                while buf.len() < head_end + len {
                    let n = s.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                }
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                let body = serde_json::from_slice(&buf[head_end..]).unwrap_or(Value::Null);
                h.lock().unwrap().push(Hit {
                    request_line: head.lines().next().unwrap_or("").to_string(),
                    headers: head.to_lowercase(),
                    body,
                });
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

/// Wait for a delivery of `event` whose data has `key == val`.
async fn delivery(hits: &Arc<Mutex<Vec<Hit>>>, event: &str, key: &str, val: &str) -> Hit {
    for _ in 0..200 {
        if let Some(h) = hits
            .lock()
            .unwrap()
            .iter()
            .find(|h| h.body["event_type"] == event && h.body["data"][key] == val)
        {
            return h.clone();
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!(
        "no {event} delivery with {key}={val}; receiver saw: {:#?}",
        hits.lock().unwrap()
    );
}

async fn create_vm(a: &Api, id: &str) {
    let (s, b) = a
        .post(
            "/vms",
            json!({"id": id, "name": id, "hypervisor": "qemu", "architecture": "x86_64",
                   "cpus": 1, "memory": 128, "disk_size": 1, "status": "stopped",
                   "display": {"protocol": "none"}}),
        )
        .await;
    assert_eq!(s, 201, "create {id}: {b}");
}

#[tokio::test]
async fn event_webhooks_deliver_real_events_and_survive_a_restart() {
    let dir = scratch("webhook-events");
    let (port, hits) = receiver().await;
    let mut server = Server::new(&dir);
    server.start();
    server.wait_healthy().await;
    let a = server.api().await;

    let hook_url =
        format!("http://hookuser:USERINFOSECRET@127.0.0.1:{port}/hook?token=QUERYSECRET");
    let (s, b) = a
        .post(
            "/webhooks",
            json!({"id": "wh1", "name": "events", "url": hook_url, "retry_count": 0,
                   "secret": "signing-secret-0123456789",
                   "events": ["vm_created", "vm_deleted", "storage_pool_added",
                              "authentication_failed"]}),
        )
        .await;
    assert!(s == 200 || s == 201, "create webhook: {s} {b}");

    // What readers (Sys.Audit) see carries neither credential.
    let (_, list) = a.get("/webhooks").await;
    let shown = list.to_string();
    for secret in ["USERINFOSECRET", "QUERYSECRET", "signing-secret"] {
        assert!(!shown.contains(secret), "{secret} leaked: {shown}");
    }
    // The UI sends the masked view back (renamed): the stored URL stays.
    let mut masked = list
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"] == "wh1")
        .cloned()
        .expect("listed");
    masked["name"] = json!("events renamed");
    let (s, b) = a.post("/webhooks/wh1", masked).await;
    assert!(s == 200 || s == 204, "update with masked url: {s} {b}");

    // A failed login -> auth.failed.
    let r = server
        .client()
        .post(format!("{}/auth/login", server.base()))
        .json(&json!({"username": "admin", "password": "definitely-wrong"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let h = delivery(&hits, "auth.failed", "user", "admin").await;
    assert!(
        h.request_line.starts_with("POST /hook?token=QUERYSECRET"),
        "the stored URL (path + query) is used: {}",
        h.request_line
    );
    assert!(
        h.headers.contains("-signature: sha256="),
        "signed: {}",
        h.headers
    );
    assert!(
        h.headers.contains("authorization: basic"),
        "userinfo kept after the masked update: {}",
        h.headers
    );

    // Storage pool add.
    let pool_dir = dir.join("pool");
    std::fs::create_dir_all(&pool_dir).unwrap();
    let (s, pool) = a
        .post(
            "/storage/pools",
            json!({"name": "wh-pool", "storage_type": "directory",
                   "path": pool_dir.to_string_lossy()}),
        )
        .await;
    assert_eq!(s, 200, "{pool}");
    delivery(&hits, "storage.pool.added", "name", "wh-pool").await;

    // VM create + delete.
    create_vm(&a, "wh-vm1").await;
    let h = delivery(&hits, "vm.created", "vm_id", "wh-vm1").await;
    assert_eq!(h.body["source"], "horcrux");
    assert_eq!(a.delete("/vms/wh-vm1").await, 204);
    delivery(&hits, "vm.deleted", "vm_id", "wh-vm1").await;

    // Counters + delivery log.
    let mut ok_count = 0;
    for _ in 0..50 {
        let (_, w) = a.get("/webhooks/wh1").await;
        ok_count = w["success_count"].as_u64().unwrap_or(0);
        if ok_count >= 4 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(ok_count >= 4, "success_count {ok_count}");
    let (_, d) = a.get("/webhooks/wh1/deliveries").await;
    assert!(d.as_array().map(|d| d.len()).unwrap_or(0) >= 4, "{d}");

    // Restart without a graceful shutdown: still configured, still delivers.
    server
        .restart_after(nix::sys::signal::Signal::SIGKILL)
        .await;
    let a = server.api().await;
    let (s, w) = a.get("/webhooks/wh1").await;
    assert_eq!(s, 200, "webhook survives a restart: {w}");
    assert_eq!(w["name"], "events renamed");
    create_vm(&a, "wh-vm2").await;
    delivery(&hits, "vm.created", "vm_id", "wh-vm2").await;
    assert_eq!(a.delete("/vms/wh-vm2").await, 204);
    assert_eq!(a.delete("/webhooks/wh1").await, 200);
}
