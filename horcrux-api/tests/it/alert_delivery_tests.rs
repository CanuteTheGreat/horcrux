//! Alert delivery proofs against the real binary: a real alert (a VM-count
//! rule firing) is delivered to a webhook channel, an ntfy channel, an e-mail
//! channel and an event webhook, each pointed at a local sink, with the
//! documented payload (docs/ALERT-DELIVERY.md). Sinks that fail (HTTP 500,
//! no answer within the time limit, connection refused, SMTP rejection) are
//! retried with backoff and, when every attempt failed, the failure is an
//! audit-log entry. Retry count, backoff and time limit come from the
//! `HORCRUX_NOTIFY_*` settings, which the tests shorten.

use crate::support::*;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

#[derive(Clone)]
struct Hit {
    at: Instant,
    /// request line + headers (HTTP) or the SMTP envelope transcript
    head: String,
    body: String,
}

impl Hit {
    fn header(&self, name: &str) -> Option<String> {
        let want = format!("{}:", name.to_lowercase());
        self.head.lines().find_map(|l| {
            l.to_lowercase()
                .starts_with(&want)
                .then(|| l[want.len()..].trim().to_string())
        })
    }
}

#[derive(Clone)]
enum Mode {
    Status(Vec<u16>),
    Hang,
}

struct Sink {
    port: u16,
    hits: Arc<Mutex<Vec<Hit>>>,
}

impl Sink {
    fn hits(&self) -> Vec<Hit> {
        self.hits.lock().unwrap().clone()
    }
    async fn wait(&self, n: usize) -> Vec<Hit> {
        for _ in 0..240 {
            if self.hits().len() >= n {
                return self.hits();
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        panic!(
            "sink on port {} got {} of {n} requests",
            self.port,
            self.hits().len()
        );
    }
    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

async fn http_sink(mode: Mode) -> Sink {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let hits: Arc<Mutex<Vec<Hit>>> = Arc::default();
    let h2 = hits.clone();
    tokio::spawn(async move {
        let mut n = 0usize;
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                return;
            };
            let (hits, mode, idx) = (h2.clone(), mode.clone(), n);
            n += 1;
            tokio::spawn(async move {
                let mut r = BufReader::new(&mut s);
                let mut head = String::new();
                let mut len = 0usize;
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                    head.push_str(&line);
                }
                let mut body = vec![0u8; len];
                if r.read_exact(&mut body).await.is_err() {
                    return;
                }
                hits.lock().unwrap().push(Hit {
                    at: Instant::now(),
                    head,
                    body: String::from_utf8_lossy(&body).into(),
                });
                let status = match &mode {
                    Mode::Hang => {
                        tokio::time::sleep(Duration::from_secs(120)).await;
                        return;
                    }
                    Mode::Status(v) => *v.get(idx).or(v.last()).unwrap_or(&200),
                };
                let _ = s
                    .write_all(
                        format!(
                            "HTTP/1.1 {status} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        )
                        .as_bytes(),
                    )
                    .await;
            });
        }
    });
    Sink { port, hits }
}

async fn refused_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    l.local_addr().unwrap().port()
}

/// SMTP server; `reject` answers MAIL FROM with a 451. A message is
/// recorded when the DATA section ends (clients may keep the connection open).
async fn smtp_sink(reject: bool) -> Sink {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let hits: Arc<Mutex<Vec<Hit>>> = Arc::default();
    let h2 = hits.clone();
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = l.accept().await else {
                return;
            };
            let hits = h2.clone();
            tokio::spawn(async move {
                let (rd, mut wr) = s.into_split();
                let mut rd = BufReader::new(rd);
                let (mut script, mut data, mut in_data) = (String::new(), String::new(), false);
                let _ = wr.write_all(b"220 sink ESMTP\r\n").await;
                loop {
                    let mut line = String::new();
                    if rd.read_line(&mut line).await.unwrap_or(0) == 0 {
                        break;
                    }
                    if in_data {
                        if line == ".\r\n" {
                            in_data = false;
                            hits.lock().unwrap().push(Hit {
                                at: Instant::now(),
                                head: script.clone(),
                                body: std::mem::take(&mut data),
                            });
                            let _ = wr.write_all(b"250 queued\r\n").await;
                        } else {
                            data.push_str(&line);
                        }
                        continue;
                    }
                    script.push_str(&line);
                    let up = line.to_uppercase();
                    let reply: &[u8] = if up.starts_with("MAIL FROM") {
                        if reject {
                            // every attempt is an SMTP session that is refused
                            hits.lock().unwrap().push(Hit {
                                at: Instant::now(),
                                head: script.clone(),
                                body: String::new(),
                            });
                            b"451 try again later\r\n"
                        } else {
                            b"250 ok\r\n"
                        }
                    } else if up.starts_with("DATA") {
                        in_data = true;
                        b"354 go\r\n"
                    } else if up.starts_with("QUIT") {
                        let _ = wr.write_all(b"221 bye\r\n").await;
                        break;
                    } else {
                        b"250 ok\r\n"
                    };
                    let _ = wr.write_all(reply).await;
                }
            });
        }
    });
    Sink { port, hits }
}

fn notify_env(srv: Server) -> Server {
    srv.env("HORCRUX_NOTIFY_ATTEMPTS", "3")
        .env("HORCRUX_NOTIFY_BACKOFF_MS", "200")
        .env("HORCRUX_NOTIFY_BACKOFF_FACTOR", "2")
        .env("HORCRUX_NOTIFY_TIMEOUT_SECS", "2")
}

async fn channel(a: &Api, name: &str, ty: &str, config: Value) -> String {
    let (s, b) = a
        .post(
            "/notifications/channels",
            json!({"name": name, "description": null, "channel_type": ty,
                   "enabled": true, "config": config}),
        )
        .await;
    assert_eq!(s, 201, "{name}: {b}");
    b["id"].as_str().unwrap().to_string()
}

async fn fire_alert(a: &Api) {
    let (s, b) = a
        .post(
            "/alerts/rules",
            json!({"id": "r1", "name": "any vm", "description": "", "severity": "warning", "enabled": true,
                   "condition": {"metric_type": "vm_count", "operator": "greaterthan", "threshold": 0.0,
                                 "target_pattern": "*", "duration_seconds": 0}}),
        )
        .await;
    assert!(s == 200 || s == 201, "{b}");
    let (s, b) = a
        .post(
            "/vms",
            json!({"id": "al1", "name": "al1", "hypervisor": "qemu", "architecture": "x86_64",
                   "cpus": 1, "memory": 128, "disk_size": 1, "status": "stopped",
                   "display": {"protocol": "none"}}),
        )
        .await;
    assert_eq!(s, 201, "{b}");
}

async fn failures(a: &Api) -> Vec<Value> {
    let (s, v) = a.get("/audit/search?limit=500").await;
    assert_eq!(s, 200, "{v}");
    v["events"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e["event_type"] == "NotificationDeliveryFailed")
        .collect()
}

fn details(e: &Value) -> Value {
    serde_json::from_str(e["details"].as_str().unwrap()).unwrap()
}

fn gaps(h: &[Hit]) -> Vec<Duration> {
    h.windows(2).map(|w| w[1].at - w[0].at).collect()
}

#[tokio::test]
async fn real_alert_reaches_webhook_ntfy_and_mail_sinks_with_the_documented_payload() {
    let dir = scratch("alert-delivery-ok");
    let (web, ntfy, flaky, mail) = (
        http_sink(Mode::Status(vec![200])).await,
        http_sink(Mode::Status(vec![200])).await,
        http_sink(Mode::Status(vec![503, 200])).await,
        smtp_sink(false).await,
    );
    let mut srv = notify_env(Server::new(&dir));
    srv.start();
    srv.wait_healthy().await;
    let a = srv.api().await;

    channel(
        &a,
        "ops-webhook",
        "webhook",
        json!({"type": "Webhook", "url": format!("{}/hook", web.url()), "method": "POST",
               "headers": {"X-Test": "1"}, "auth": {"type": "BearerToken", "token": "wh-token"}}),
    )
    .await;
    channel(
        &a,
        "ops-ntfy",
        "ntfy",
        json!({"type": "Ntfy", "server_url": ntfy.url(), "topic": "horcrux-alerts", "token": "tk_abc"}),
    )
    .await;
    channel(
        &a,
        "flaky-ntfy",
        "ntfy",
        json!({"type": "Ntfy", "server_url": flaky.url(), "topic": "flaky"}),
    )
    .await;
    channel(
        &a,
        "ops-mail",
        "email",
        json!({"type": "Email", "address": "ops@example.test, noc@example.test",
               "smtp_server": "127.0.0.1", "smtp_port": mail.port, "use_tls": false}),
    )
    .await;

    fire_alert(&a).await;

    // --- generic webhook channel ---
    let h = &web.wait(1).await[0];
    assert!(h.head.starts_with("POST /hook "), "{}", h.head);
    assert_eq!(h.header("x-test").as_deref(), Some("1"));
    assert_eq!(
        h.header("authorization").as_deref(),
        Some("Bearer wh-token")
    );
    assert!(h
        .header("content-type")
        .unwrap()
        .starts_with("application/json"));
    let v: Value = serde_json::from_str(&h.body).unwrap();
    for k in [
        "event",
        "title",
        "message",
        "severity",
        "channel",
        "timestamp",
        "alert",
    ] {
        assert!(v.get(k).is_some(), "payload lacks {k}: {v}");
    }
    assert_eq!(v["event"], "alert");
    assert_eq!(v["severity"], "warning");
    assert_eq!(v["channel"], "ops-webhook");
    assert_eq!(v["alert"]["rule_id"], "r1");
    assert!(v["title"].as_str().unwrap().contains("any vm"));

    // --- ntfy: plain text, Title / Priority / Tags, bearer token ---
    let h = &ntfy.wait(1).await[0];
    assert!(h.head.starts_with("POST /horcrux-alerts "), "{}", h.head);
    assert!(h.header("content-type").is_none_or(|c| !c.contains("json")));
    assert!(h.header("title").unwrap().contains("any vm"), "{}", h.head);
    assert_eq!(
        h.header("priority").as_deref(),
        Some("high"),
        "warning = high"
    );
    assert_eq!(h.header("tags").as_deref(), Some("alert,warning"));
    assert_eq!(h.header("authorization").as_deref(), Some("Bearer tk_abc"));
    assert!(h.body.contains("any vm"), "{}", h.body);

    // --- e-mail ---
    let h = &mail.wait(1).await[0];
    assert!(h.head.contains("ops@example.test"), "{}", h.head);
    assert!(h.head.contains("noc@example.test"), "{}", h.head);
    assert!(h.body.contains("Subject:"), "{}", h.body);
    assert!(h.body.contains("any vm"), "{}", h.body);

    // --- a sink that fails once is retried and then receives the alert ---
    let hits = flaky.wait(2).await;
    assert_eq!(hits.len(), 2, "one failure, then the retry");
    assert!(
        gaps(&hits)[0] >= Duration::from_millis(200),
        "{:?}",
        gaps(&hits)
    );
    for _ in 0..60 {
        let (_, l) = a.get("/notifications/channels").await;
        let c = l
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "flaky-ntfy")
            .cloned()
            .unwrap();
        if c["success_count"] == 1 {
            assert_eq!(c["failure_count"], 0, "{c}");
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(failures(&a).await.is_empty(), "nothing failed for good");
}

#[tokio::test]
async fn failing_sinks_are_retried_with_backoff_and_every_failure_is_audited() {
    let dir = scratch("alert-delivery-fail");
    let (s500, hang, evt) = (
        http_sink(Mode::Status(vec![500])).await,
        http_sink(Mode::Hang).await,
        http_sink(Mode::Status(vec![500])).await,
    );
    let mail = smtp_sink(true).await;
    let refused = refused_port().await;
    let mut srv = notify_env(Server::new(&dir));
    srv.start();
    srv.wait_healthy().await;
    let a = srv.api().await;

    channel(
        &a,
        "c-500",
        "webhook",
        json!({"type": "Webhook", "url": format!("{}/h", s500.url()), "method": "POST", "headers": {}}),
    )
    .await;
    channel(
        &a,
        "c-hang",
        "ntfy",
        json!({"type": "Ntfy", "server_url": hang.url(), "topic": "t"}),
    )
    .await;
    channel(
        &a,
        "c-refused",
        "webhook",
        json!({"type": "Webhook", "url": format!("http://127.0.0.1:{refused}/h"),
               "method": "POST", "headers": {}}),
    )
    .await;
    channel(
        &a,
        "c-mail",
        "email",
        json!({"type": "Email", "address": "ops@example.test",
               "smtp_server": "127.0.0.1", "smtp_port": mail.port, "use_tls": false}),
    )
    .await;
    // an event webhook (its own retry_count) that always answers 500
    let (s, b) = a
        .post(
            "/webhooks",
            json!({"id": "w1", "name": "event-hook", "url": format!("{}/e", evt.url()),
                   "retry_count": 2, "events": ["alert_triggered"]}),
        )
        .await;
    assert!(s == 200 || s == 201, "{b}");

    fire_alert(&a).await;

    // three attempts each, with growing waits (200 ms, then 400 ms)
    let h = s500.wait(3).await;
    assert_eq!(h.len(), 3);
    let g = gaps(&h);
    assert!(g[0] >= Duration::from_millis(200), "{g:?}");
    assert!(g[1] >= Duration::from_millis(400), "{g:?}");
    let h = hang.wait(3).await;
    assert!(
        gaps(&h)[0] >= Duration::from_secs(2),
        "the time limit (2 s) ends each attempt: {:?}",
        gaps(&h)
    );
    assert_eq!(mail.wait(3).await.len(), 3, "three SMTP sessions");
    let h = evt.wait(3).await;
    assert_eq!(h.len(), 3, "retry_count 2 = three attempts");

    // every failure is an audit entry
    let mut ev = vec![];
    for _ in 0..120 {
        ev = failures(&a).await;
        if ev.len() >= 5 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert_eq!(ev.len(), 5, "{ev:#?}");
    let mut seen: Vec<(String, u64)> = ev
        .iter()
        .map(|e| {
            let d = details(e);
            assert_eq!(e["result"], "Failure");
            assert!(!d["error"].as_str().unwrap().is_empty(), "{d}");
            (
                d["channel_name"]
                    .as_str()
                    .or(d["webhook_name"].as_str())
                    .unwrap()
                    .to_string(),
                d["attempts"].as_u64().unwrap(),
            )
        })
        .collect();
    seen.sort();
    assert_eq!(
        seen,
        [
            ("c-500".to_string(), 3),
            ("c-hang".to_string(), 3),
            ("c-mail".to_string(), 3),
            ("c-refused".to_string(), 3),
            ("event-hook".to_string(), 3),
        ]
    );
    // the channel list shows the failures too
    let (_, l) = a.get("/notifications/channels").await;
    for c in l.as_array().unwrap() {
        assert_eq!(c["failure_count"], 1, "{c}");
        assert_eq!(c["success_count"], 0, "{c}");
    }
}
