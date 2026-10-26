//! External metric servers end to end: the real `horcrux-api` binary sends
//! this node's metrics to a real InfluxDB-style HTTP receiver and a Graphite
//! TCP/UDP receiver on its own, answers test sends with the target's error,
//! and never shows tokens/passwords.

use crate::support::*;
use serde_json::json;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

type Hits = Arc<Mutex<Vec<String>>>;

/// HTTP receiver answering `status` to every request; records head + body.
async fn http_receiver(status: &'static str) -> (u16, Hits) {
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
                let body = if status.starts_with("204") {
                    ""
                } else {
                    "{\"message\":\"unauthorized\"}"
                };
                let _ = s
                    .write_all(
                        format!("HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len())
                            .as_bytes(),
                    )
                    .await;
            });
        }
    });
    (port, hits)
}

async fn graphite_tcp() -> (u16, Hits) {
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
                let mut v = String::new();
                let _ = s.read_to_string(&mut v).await;
                h.lock().unwrap().push(v);
            });
        }
    });
    (port, hits)
}

async fn wait_hits(h: &Hits, what: &str) -> Vec<String> {
    for _ in 0..300 {
        if !h.lock().unwrap().is_empty() {
            return h.lock().unwrap().clone();
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("{what}: nothing received in 30 s");
}

#[tokio::test]
async fn influx_and_graphite_receive_metrics_and_secrets_stay_hidden() {
    let dir = scratch("metric-servers");
    let (ipt, influx) = http_receiver("204 No Content").await;
    let (gpt, graphite) = graphite_tcp().await;
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let a = srv.api().await;

    // validation
    for (body, what) in [
        (
            json!({"name": "", "type": "influxdb", "host": "x", "organization": "o", "bucket": "b"}),
            "name",
        ),
        (
            json!({"name": "x", "type": "influxdb", "host": "a b", "organization": "o", "bucket": "b"}),
            "host",
        ),
        (
            json!({"name": "x", "type": "influxdb", "host": "h"}),
            "organization",
        ),
        (
            json!({"name": "x", "type": "graphite", "host": "h", "protocol": "http"}),
            "tcp or udp",
        ),
        (
            json!({"name": "x", "type": "graphite", "host": "h", "interval_secs": 1}),
            "interval",
        ),
        (
            json!({"name": "x", "type": "influxdb", "host": "h", "organization": "o", "bucket": "b", "token": "********"}),
            "placeholder",
        ),
    ] {
        let (s, v) = a.post("/observability/metric-servers", body).await;
        assert!(s == 400 || s == 422, "{what}: {s} {v}");
        assert!(v.to_string().contains(what), "{what}: {v}");
    }

    let (s, v) = a
        .post(
            "/observability/metric-servers",
            json!({"name": "influx", "type": "influxdb", "host": "127.0.0.1", "port": ipt,
                   "organization": "lab", "bucket": "hx", "token": "TOKEN-SECRET-1", "interval_secs": 5}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let id = v["id"].as_str().unwrap().to_string();
    assert_eq!(v["token"], "********", "{v}");
    let (s, v) = a
        .post(
            "/observability/metric-servers",
            json!({"name": "graphite", "type": "graphite", "host": "127.0.0.1", "port": gpt,
                   "prefix": "lab.horcrux", "interval_secs": 5}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let (s, _) = a
        .post(
            "/observability/metric-servers",
            json!({"name": "influx", "type": "graphite", "host": "127.0.0.1"}),
        )
        .await;
    assert_eq!(s, 409, "duplicate name");

    // the background loop delivers on its own
    let got = wait_hits(&influx, "influx").await;
    assert!(
        got[0].starts_with("POST /api/v2/write?org=lab&bucket=hx&precision=ns"),
        "{}",
        got[0]
    );
    assert!(
        got[0]
            .to_lowercase()
            .contains("authorization: token token-secret-1"),
        "{}",
        got[0]
    );
    assert!(got[0].contains("horcrux_node,node="), "{}", got[0]);
    assert!(got[0].contains("memory_total_bytes="), "{}", got[0]);
    let g = wait_hits(&graphite, "graphite").await;
    assert!(
        g[0].contains("lab.horcrux.nodes.") && g[0].contains(".memory_total_bytes "),
        "{}",
        g[0]
    );

    // list: masked, with the send status
    let (_, list) = a.get("/observability/metric-servers").await;
    let shown = list.to_string();
    assert!(!shown.contains("TOKEN-SECRET-1"), "{shown}");
    let me = list
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == id.as_str())
        .unwrap()
        .clone();
    assert!(
        me["status"]["last_success"].is_number()
            && me["status"]["points_sent"].as_u64().unwrap() > 0,
        "{me}"
    );

    // editing with the mask keeps the stored token
    let mut upd = me.clone();
    upd["name"] = json!("influx renamed");
    let (s, v) = a
        .put(&format!("/observability/metric-servers/{id}"), upd)
        .await;
    assert_eq!(s, 200, "{v}");
    let before = influx.lock().unwrap().len();
    let (s, v) = a
        .post(
            &format!("/observability/metric-servers/{id}/test"),
            json!({}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(v["points"].as_u64().unwrap() >= 1, "{v}");
    assert!(influx.lock().unwrap().len() > before);
    assert!(
        influx
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .to_lowercase()
            .contains("authorization: token token-secret-1"),
        "stored token kept"
    );

    // a target that refuses: the test reports why (502), the status says so
    let (bad_port, _) = http_receiver("401 Unauthorized").await;
    let (s, v) = a
        .post(
            "/observability/metric-servers",
            json!({"name": "rejecting", "type": "influxdb", "host": "127.0.0.1", "port": bad_port,
                   "organization": "o", "bucket": "b", "token": "t"}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let bid = v["id"].as_str().unwrap().to_string();
    let (s, v) = a
        .post(
            &format!("/observability/metric-servers/{bid}/test"),
            json!({}),
        )
        .await;
    assert_eq!(s, 502, "{v}");
    assert!(
        v.to_string().contains("401") && v.to_string().contains("unauthorized"),
        "{v}"
    );
    let (_, one) = a.get(&format!("/observability/metric-servers/{bid}")).await;
    assert!(
        one["status"]["last_error"]
            .as_str()
            .unwrap()
            .contains("401"),
        "{one}"
    );

    // survives a restart; delete
    srv.restart_after(nix::sys::signal::Signal::SIGKILL).await;
    let a = srv.api().await;
    let (_, list) = a.get("/observability/metric-servers").await;
    assert_eq!(list.as_array().unwrap().len(), 3, "{list}");
    assert_eq!(
        a.delete(&format!("/observability/metric-servers/{bid}"))
            .await,
        204
    );
    assert_eq!(
        a.delete(&format!("/observability/metric-servers/{bid}"))
            .await,
        404
    );
}
