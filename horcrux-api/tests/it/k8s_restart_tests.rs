//! Restart behaviour of the Kubernetes integration and the Monitoring >
//! Notifications channel store, against the real `horcrux-api` binary.
//!
//! Connects a real cluster (CI's k3s from tests/k8s_helm_setup.sh, via
//! HORCRUX_CONTRACT_KUBECONFIG), creates a notification channel, SIGKILLs the
//! server and starts it again on the same database, then checks:
//! * the cluster's pages work again without pressing Reconnect (clients are
//!   re-created at startup; before, every page failed "cluster not connected"
//!   while the list still said "connected");
//! * Helm still reaches the cluster with the stored kubeconfig;
//! * the kubeconfig is not stored in plaintext;
//! * the notification channel survived.
//!
//! Without HORCRUX_CONTRACT_KUBECONFIG the test is skipped, unless
//! HORCRUX_TEST_REQUIRE_K8S is set (CI), in which case it fails.
#![cfg(feature = "kubernetes")]

use reqwest::Client;
use serde_json::{json, Value};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

struct Server {
    child: Child,
    port: u16,
    log: PathBuf,
}

impl Server {
    fn start(dir: &Path, port: u16, run: u32) -> Server {
        let log = dir.join(format!("server-{run}.log"));
        let log_file = std::fs::File::create(&log).unwrap();
        let cni = dir.join("cni");
        std::fs::create_dir_all(cni.join("bin")).unwrap();
        std::fs::create_dir_all(cni.join("conf.d")).unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_horcrux-api"))
            .env("HORCRUX_HOST", "127.0.0.1")
            .env("HORCRUX_PORT", port.to_string())
            .env("HORCRUX_DATA_DIR", dir.join("data"))
            .env(
                "HORCRUX_DATABASE_URL",
                format!("sqlite://{}", dir.join("data/horcrux.db").display()),
            )
            .env("HORCRUX_LOG_DIR", dir.join("log"))
            .env("HORCRUX_QEMU_RUN_DIR", dir.join("run"))
            .env("HORCRUX_CNI_BIN_DIR", cni.join("bin"))
            .env("HORCRUX_CNI_CONF_DIR", cni.join("conf.d"))
            .env("ADMIN_PASSWORD", "admin")
            .env("RATE_LIMIT_MAX_REQUESTS", "100000")
            .env("RUST_LOG", "info")
            .stdout(Stdio::from(log_file.try_clone().unwrap()))
            .stderr(Stdio::from(log_file))
            .spawn()
            .expect("start horcrux-api");
        Server { child, port, log }
    }

    fn base(&self) -> String {
        // The API serves HTTPS (self-issued node certificate); plain HTTP
        // only answers with a 308 redirect.
        format!("https://127.0.0.1:{}/api", self.port)
    }

    async fn wait_healthy(&self, c: &Client) {
        for _ in 0..120 {
            if let Ok(r) = c.get(format!("{}/health", self.base())).send().await {
                if r.status().is_success() {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        panic!(
            "horcrux-api did not become healthy:\n{}",
            std::fs::read_to_string(&self.log).unwrap_or_default()
        );
    }

    fn kill(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn login(c: &Client, base: &str) -> String {
    let v: Value = c
        .post(format!("{base}/auth/login"))
        .json(&json!({"username": "admin", "password": "admin"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    v["ticket"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn kubernetes_and_channels_work_after_restart() {
    let Ok(kubeconfig_path) = std::env::var("HORCRUX_CONTRACT_KUBECONFIG") else {
        if std::env::var("HORCRUX_TEST_REQUIRE_K8S").is_ok() {
            panic!("HORCRUX_TEST_REQUIRE_K8S is set but HORCRUX_CONTRACT_KUBECONFIG is not");
        }
        eprintln!("skipped: no HORCRUX_CONTRACT_KUBECONFIG");
        return;
    };
    let kubeconfig = std::fs::read_to_string(&kubeconfig_path).unwrap();
    let token_in_kubeconfig = kubeconfig
        .lines()
        .find_map(|l| {
            let l = l.trim();
            l.strip_prefix("client-key-data:")
                .or_else(|| l.strip_prefix("token:"))
        })
        .map(|s| s.trim().to_string())
        .expect("kubeconfig has a credential");

    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let c = Client::builder()
        .timeout(Duration::from_secs(120))
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap();
    let port = free_port();

    let s = Server::start(dir, port, 1);
    s.wait_healthy(&c).await;
    let t = login(&c, &s.base()).await;
    let r = c
        .post(format!("{}/k8s/clusters", s.base()))
        .bearer_auth(&t)
        .json(&json!({"name": "restart-k3s", "kubeconfig": kubeconfig}))
        .send()
        .await
        .unwrap();
    assert!(
        r.status().is_success(),
        "connect: {}",
        r.text().await.unwrap()
    );
    let cid = r.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = c
        .post(format!("{}/notifications/channels", s.base()))
        .bearer_auth(&t)
        .json(
            &json!({"name": "survivor", "description": null, "channel_type": "discord",
                      "enabled": true, "config": {"type": "Discord",
                      "webhook_url": "https://discord.example.org/api/webhooks/1/x",
                      "username": null, "avatar_url": null}}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 201);

    // SIGKILL: nothing gets flushed on the way out.
    s.kill();

    // Kubeconfig at rest: not the plaintext credential.
    let db = dir.join("data/horcrux.db");
    let out = Command::new("python3")
        .arg("-c")
        .arg(
            "import sqlite3,sys; print(sqlite3.connect(sys.argv[1]).execute(\
             'select kubeconfig_encrypted from k8s_clusters').fetchone()[0])",
        )
        .arg(&db)
        .output()
        .expect("python3 sqlite3");
    let stored = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(!stored.trim().is_empty(), "kubeconfig not stored: {out:?}");
    assert!(
        !stored.contains(&token_in_kubeconfig) && !stored.contains("apiVersion"),
        "kubeconfig stored in plaintext"
    );

    let s = Server::start(dir, port, 2);
    s.wait_healthy(&c).await;
    let t = login(&c, &s.base()).await;
    let mut last = String::new();
    let mut ok = false;
    for _ in 0..60 {
        let r = c
            .get(format!(
                "{}/k8s/clusters/{cid}/namespaces/kube-system/pods",
                s.base()
            ))
            .bearer_auth(&t)
            .send()
            .await
            .unwrap();
        let status = r.status();
        last = r.text().await.unwrap_or_default();
        if status.is_success() {
            ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert!(ok, "cluster pages never worked after restart: {last}");
    let r = c
        .get(format!("{}/k8s/clusters/{cid}/helm/releases", s.base()))
        .bearer_auth(&t)
        .send()
        .await
        .unwrap();
    assert!(
        r.status().is_success(),
        "helm after restart: {}",
        r.text().await.unwrap()
    );
    let channels: Vec<Value> = c
        .get(format!("{}/notifications/channels", s.base()))
        .bearer_auth(&t)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        channels.iter().any(|ch| ch["name"] == "survivor"),
        "{channels:?}"
    );
    let _ = c
        .delete(format!("{}/k8s/clusters/{cid}", s.base()))
        .bearer_auth(&t)
        .send()
        .await;
    s.kill();
}
