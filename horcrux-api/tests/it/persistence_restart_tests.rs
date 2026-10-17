//! Restart survival of persisted configuration.
//!
//! Runs the real `horcrux-api` binary against a throwaway data directory,
//! creates one of every persisted configuration object through the HTTP API
//! (storage pool, firewall rule, HA group + resource, template, alert rule,
//! notification channel, replication job, snapshot schedule, cluster node,
//! backup job, backup), then:
//!
//! 1. kills the process with SIGKILL (no graceful shutdown, so nothing can be
//!    flushed on exit: the data must already be in the database) and starts
//!    it again on the same database - every object must still be listed;
//! 2. deletes every object, stops the process with SIGTERM, starts it again -
//!    every object must stay deleted.
//!
//! Before this, all of these lived only in memory and vanished on restart.
//!
//! Needs `qemu-img` (VM creation allocates a real qcow2 disk, which the
//! template then copies), the same as the rest of the API test suite.

use reqwest::{Client, StatusCode};
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
            .env("HORCRUX_SSL_DIR", dir.join("ssl"))
            .env("ADMIN_PASSWORD", "admin")
            .env("RATE_LIMIT_MAX_REQUESTS", "100000")
            .env("AUTH_RATE_LIMIT_MAX_REQUESTS", "100000")
            .env("RUST_LOG", "info")
            .stdout(Stdio::from(log_file.try_clone().unwrap()))
            .stderr(Stdio::from(log_file))
            .spawn()
            .expect("failed to spawn horcrux-api");
        Server { child, port, log }
    }

    fn base(&self) -> String {
        // HTTPS by default (certificate from the node-local CA)
        format!("https://127.0.0.1:{}/api", self.port)
    }

    fn log_text(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    async fn wait_healthy(&mut self, client: &Client) {
        for _ in 0..120 {
            if let Some(status) = self.child.try_wait().unwrap() {
                panic!("horcrux-api exited early ({status}):\n{}", self.log_text());
            }
            if let Ok(r) = client.get(format!("{}/health", self.base())).send().await {
                if r.status().is_success() {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        panic!("horcrux-api never became healthy:\n{}", self.log_text());
    }

    fn signal(&mut self, sig: nix::sys::signal::Signal) {
        let pid = nix::unistd::Pid::from_raw(self.child.id() as i32);
        nix::sys::signal::kill(pid, sig).expect("kill failed");
        for _ in 0..200 {
            if self.child.try_wait().unwrap().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
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

async fn login(client: &Client, base: &str) -> String {
    let r = client
        .post(format!("{base}/auth/login"))
        .json(&json!({"username": "admin", "password": "admin"}))
        .send()
        .await
        .expect("login request failed");
    assert!(r.status().is_success(), "login failed: {}", r.status());
    let body: Value = r.json().await.unwrap();
    body["ticket"].as_str().expect("no ticket").to_string()
}

/// POST and require a 2xx; returns the parsed body (Null if empty).
async fn post(client: &Client, token: &str, url: String, body: Value) -> Value {
    let r = client
        .post(&url)
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .unwrap_or_else(|e| panic!("POST {url} failed: {e}"));
    let status = r.status();
    let text = r.text().await.unwrap_or_default();
    assert!(status.is_success(), "POST {url} -> {status}: {text}");
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

async fn delete(client: &Client, token: &str, url: String) {
    let r = client
        .delete(&url)
        .bearer_auth(token)
        .send()
        .await
        .unwrap_or_else(|e| panic!("DELETE {url} failed: {e}"));
    let status = r.status();
    assert!(
        status.is_success(),
        "DELETE {url} -> {status}: {}",
        r.text().await.unwrap_or_default()
    );
}

async fn get_text(client: &Client, token: &str, url: String) -> String {
    let r = client
        .get(&url)
        .bearer_auth(token)
        .send()
        .await
        .unwrap_or_else(|e| panic!("GET {url} failed: {e}"));
    assert_eq!(r.status(), StatusCode::OK, "GET {url}");
    r.text().await.unwrap()
}

/// Every persisted object created by the test: (list endpoint, unique id
/// that must appear in that list's JSON).
struct Created {
    checks: Vec<(&'static str, String)>,
}

async fn assert_listed(client: &Client, token: &str, base: &str, created: &Created, present: bool) {
    let mut failures = Vec::new();
    for (list, id) in &created.checks {
        let body = get_text(client, token, format!("{base}{list}")).await;
        if body.contains(id.as_str()) != present {
            failures.push(format!(
                "{list}: expected {id} to be {}; got {body}",
                if present { "present" } else { "absent" }
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn configuration_survives_process_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::create_dir_all(dir.join("data")).unwrap();
    let pool_dir = dir.join("pool");
    let backup_dir = dir.join("backups");
    std::fs::create_dir_all(&pool_dir).unwrap();
    std::fs::create_dir_all(&backup_dir).unwrap();

    let client = Client::builder()
        .timeout(Duration::from_secs(60))
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap();
    let port = free_port();

    // ---- run 1: create one of everything -------------------------------
    let mut server = Server::start(dir, port, 1);
    server.wait_healthy(&client).await;
    let base = server.base();
    let token = login(&client, &base).await;

    let suffix = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
    let vm_id = format!("persist-vm-{suffix}");
    post(
        &client,
        &token,
        format!("{base}/vms"),
        json!({
            "id": vm_id, "name": "persistence test", "hypervisor": "qemu",
            "architecture": "x86_64", "cpus": 1, "memory": 256,
            "disk_size": 1, "status": "stopped"
        }),
    )
    .await;

    let pool = post(
        &client,
        &token,
        format!("{base}/storage/pools"),
        json!({"name": format!("persist-pool-{suffix}"), "storage_type": "directory",
               "path": pool_dir.to_string_lossy()}),
    )
    .await;
    let pool_id = pool["id"].as_str().expect("pool id").to_string();

    let fw_id = format!("persist-fw-{suffix}");
    post(
        &client,
        &token,
        format!("{base}/firewall/rules"),
        json!({"id": fw_id, "enabled": true, "action": "ACCEPT", "direction": "in",
               "protocol": "tcp", "source": null, "dest": null, "sport": null,
               "dport": "2222", "comment": "persistence test", "log": false,
               "position": 0}),
    )
    .await;

    let ha_group = format!("persist-ha-{suffix}");
    post(
        &client,
        &token,
        format!("{base}/ha/groups"),
        json!({"name": ha_group, "nodes": ["node-a", "node-b"]}),
    )
    .await;
    post(
        &client,
        &token,
        format!("{base}/ha/resources"),
        json!({"vm_id": vm_id, "group": ha_group, "priority": 2}),
    )
    .await;

    let template = post(
        &client,
        &token,
        format!("{base}/templates"),
        json!({"vm_id": vm_id, "name": format!("persist-tpl-{suffix}")}),
    )
    .await;
    let template_id = template["id"].as_str().expect("template id").to_string();

    let alert_id = format!("persist-alert-{suffix}");
    post(
        &client,
        &token,
        format!("{base}/alerts/rules"),
        json!({"id": alert_id, "name": "persistence test", "description": "cpu high",
               "severity": "warning", "enabled": true,
               "condition": {"metric_type": "cpu_usage", "operator": "greaterthan",
                             "threshold": 95.0, "target_pattern": "*",
                             "duration_seconds": 60}}),
    )
    .await;

    let channel = format!("persist-channel-{suffix}");
    post(
        &client,
        &token,
        format!("{base}/alerts/notifications"),
        json!({"type": "syslog", "name": channel, "enabled": true,
               "min_severity": "warning"}),
    )
    .await;

    let repl = post(
        &client,
        &token,
        format!("{base}/replication/jobs"),
        json!({"source_vm_id": vm_id, "source_snapshot": "base",
               "target_node": "node-b", "target_pool": "tank", "schedule": "manual"}),
    )
    .await;
    let repl_id = repl["id"].as_str().expect("replication id").to_string();

    let sched = post(
        &client,
        &token,
        format!("{base}/snapshot-schedules"),
        json!({"vm_id": vm_id, "name": format!("persist-sched-{suffix}"),
               "frequency": {"daily": {"hour": 3}}, "retention_count": 3,
               "enabled": true, "include_memory": false}),
    )
    .await;
    let sched_id = sched["id"].as_str().expect("schedule id").to_string();

    let node = format!("persist-node-{suffix}");
    post(
        &client,
        &token,
        format!("{base}/cluster/nodes/{node}"),
        json!({"ip": "192.0.2.10"}),
    )
    .await;

    let job_id = format!("persist-job-{suffix}");
    post(
        &client,
        &token,
        format!("{base}/backup-jobs"),
        json!({"id": job_id, "enabled": true, "schedule": "0 2 * * *",
               "targets": [vm_id], "storage": backup_dir.to_string_lossy(),
               "mode": "snapshot", "compression": "gzip",
               "retention": {"keep_hourly": null, "keep_daily": 7, "keep_weekly": null,
                             "keep_monthly": null, "keep_yearly": null},
               "notify": null}),
    )
    .await;

    let backup = post(
        &client,
        &token,
        format!("{base}/backups"),
        json!({"id": "", "name": "persistence test", "target_type": "vm",
               "target_id": vm_id, "storage": backup_dir.to_string_lossy(),
               "mode": "snapshot", "compression": "gzip", "notes": null}),
    )
    .await;
    let backup_id = backup["id"].as_str().expect("backup id").to_string();

    let created = Created {
        checks: vec![
            ("/storage/pools", pool_id.clone()),
            ("/firewall/rules", fw_id.clone()),
            ("/ha/groups", ha_group.clone()),
            ("/ha/resources", vm_id.clone()),
            ("/templates", template_id.clone()),
            ("/alerts/rules", alert_id.clone()),
            ("/alerts/notifications", channel.clone()),
            ("/replication/jobs", repl_id.clone()),
            ("/snapshot-schedules", sched_id.clone()),
            ("/cluster/nodes", node.clone()),
            ("/backup-jobs", job_id.clone()),
            ("/backups", backup_id.clone()),
        ],
    };
    assert_listed(&client, &token, &base, &created, true).await;

    // ---- run 2: hard kill, restart, everything is still there ----------
    server.signal(nix::sys::signal::Signal::SIGKILL);
    drop(server);
    let mut server = Server::start(dir, port, 2);
    server.wait_healthy(&client).await;
    let token = login(&client, &base).await;
    assert_listed(&client, &token, &base, &created, true).await;
    let log = server.log_text();
    assert!(
        log.contains("Loaded 1 storage pool(s) from database"),
        "startup log should report the loaded pool:\n{log}"
    );

    // Loaded objects are fully functional, not just listed: the template
    // still clones from its persisted disk copy.
    let clone_id = format!("persist-clone-{suffix}");
    post(
        &client,
        &token,
        format!("{base}/templates/{template_id}/clone"),
        json!({"new_vm_id": clone_id, "new_vm_name": "persist clone",
               "clone_type": "full", "storage_pool": null, "cloudinit_config": null}),
    )
    .await;

    // ---- delete everything, graceful restart, it stays deleted ---------
    delete(&client, &token, format!("{base}/backups/{backup_id}")).await;
    delete(&client, &token, format!("{base}/backup-jobs/{job_id}")).await;
    delete(&client, &token, format!("{base}/cluster/nodes/{node}")).await;
    delete(
        &client,
        &token,
        format!("{base}/snapshot-schedules/{sched_id}"),
    )
    .await;
    delete(
        &client,
        &token,
        format!("{base}/replication/jobs/{repl_id}"),
    )
    .await;
    delete(&client, &token, format!("{base}/alerts/rules/{alert_id}")).await;
    delete(&client, &token, format!("{base}/templates/{template_id}")).await;
    delete(&client, &token, format!("{base}/ha/resources/{vm_id}")).await;
    delete(&client, &token, format!("{base}/ha/groups/{ha_group}")).await;
    delete(&client, &token, format!("{base}/firewall/rules/{fw_id}")).await;
    delete(&client, &token, format!("{base}/storage/pools/{pool_id}")).await;

    server.signal(nix::sys::signal::Signal::SIGTERM);
    drop(server);
    let mut server = Server::start(dir, port, 3);
    server.wait_healthy(&client).await;
    let token = login(&client, &base).await;
    // The notification channel has no delete route; it must still be there.
    let deleted = Created {
        checks: created
            .checks
            .iter()
            .filter(|(list, _)| *list != "/alerts/notifications")
            .cloned()
            .collect(),
    };
    assert_listed(&client, &token, &base, &deleted, false).await;
    let channels = get_text(&client, &token, format!("{base}/alerts/notifications")).await;
    assert!(channels.contains(&channel), "channel lost: {channels}");
    drop(server);
}
