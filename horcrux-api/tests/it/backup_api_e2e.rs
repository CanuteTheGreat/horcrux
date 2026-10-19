//! Backup/restore end to end over HTTP against the live horcrux-api CI
//! starts on :8006 (same host, so the test can read and damage the VM's
//! real disk file): back up a VM holding known data, verify, corrupt and
//! delete the original, restore, compare the disk contents; restore to a
//! new VM; and prove the server's own scheduler loop fires a cron job and
//! applies its retention.
//!
//! Override the server with HORCRUX_E2E_API (default
//! https://127.0.0.1:8006/api).

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

fn api() -> String {
    std::env::var("HORCRUX_E2E_API").unwrap_or_else(|_| "https://127.0.0.1:8006/api".into())
}

/// The guest's run state ("running", "paused", ...) from QMP query-status,
/// `None` when the socket can't be read right now.
fn qmp_status(sock: &Path) -> Option<String> {
    use std::io::{BufRead, BufReader, Write};
    let s = std::os::unix::net::UnixStream::connect(sock).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut w = s.try_clone().ok()?;
    let mut r = BufReader::new(s);
    let mut line = String::new();
    r.read_line(&mut line).ok()?; // greeting
    writeln!(w, "{}", json!({"execute": "qmp_capabilities"})).ok()?;
    writeln!(w, "{}", json!({"execute": "query-status"})).ok()?;
    for _ in 0..20 {
        line.clear();
        if r.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let v: Value = serde_json::from_str(&line).ok()?;
        if let Some(st) = v["return"]["status"].as_str() {
            return Some(st.to_string());
        }
    }
    None
}

struct Api {
    c: reqwest::Client,
    token: String,
}

impl Api {
    async fn login() -> Self {
        let c = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap();
        for _ in 0..60 {
            let r = c
                .post(format!("{}/auth/login", api()))
                .json(&json!({"username": "admin", "password": "admin"}))
                .send()
                .await
                .expect("horcrux-api must be running for the backup e2e test");
            if r.status() == 429 {
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
            assert!(r.status().is_success(), "login: {}", r.status());
            let v: Value = r.json().await.unwrap();
            let token = v["ticket"]
                .as_str()
                .or_else(|| v["token"].as_str())
                .unwrap()
                .to_string();
            return Self { c, token };
        }
        panic!("login rate limited");
    }

    async fn call(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut r = self
            .c
            .request(method, format!("{}{}", api(), path))
            .bearer_auth(&self.token);
        if let Some(b) = body {
            r = r.json(&b);
        }
        let r = r.send().await.unwrap();
        let status = r.status().as_u16();
        let text = r.text().await.unwrap_or_default();
        (
            status,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }
    async fn get(&self, p: &str) -> (u16, Value) {
        self.call(reqwest::Method::GET, p, None).await
    }
    async fn post(&self, p: &str, b: Value) -> (u16, Value) {
        self.call(reqwest::Method::POST, p, Some(b)).await
    }
    async fn delete(&self, p: &str) -> u16 {
        self.call(reqwest::Method::DELETE, p, None).await.0
    }
}

fn run(prog: &str, args: &[&str]) -> String {
    let out = Command::new(prog).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "{prog} {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// SHA-256 of a disk image's guest-visible contents.
fn content_sha(disk: &Path) -> String {
    let raw = std::env::temp_dir().join(format!("bk-e2e-{}.raw", std::process::id()));
    let _ = std::fs::remove_file(&raw);
    run(
        "qemu-img",
        &[
            "convert",
            "-O",
            "raw",
            &disk.to_string_lossy(),
            &raw.to_string_lossy(),
        ],
    );
    let s = run("sha256sum", &[&raw.to_string_lossy()]);
    std::fs::remove_file(&raw).unwrap();
    s.split_whitespace().next().unwrap().to_string()
}

async fn vm_disk(a: &Api, id: &str) -> PathBuf {
    let (s, vm) = a.get(&format!("/vms/{id}")).await;
    assert_eq!(s, 200, "{vm}");
    PathBuf::from(vm["disks"][0]["path"].as_str().expect("vm disk path"))
}

#[tokio::test]
async fn backup_corrupt_restore_and_scheduled_job_over_http() {
    let a = Api::login().await;
    let vm = "bk-e2e-vm";
    let copy = "bk-e2e-vm-copy";
    let job = "bk-e2e-job";
    a.delete(&format!("/backup-jobs/{job}")).await;
    for id in [vm, copy] {
        a.delete(&format!("/vms/{id}")).await;
    }

    let (s, b) = a
        .post(
            "/vms",
            json!({"id": vm, "name": vm, "hypervisor": "qemu", "architecture": "x86_64",
                   "cpus": 1, "memory": 128, "disk_size": 1, "status": "stopped"}),
        )
        .await;
    assert!(s == 200 || s == 201, "create vm: {s} {b}");
    let disk = vm_disk(&a, vm).await;
    // Fill the disk with 2 MiB of known data.
    let seed = std::env::temp_dir().join(format!("bk-e2e-seed-{}.raw", std::process::id()));
    let data: Vec<u8> = (0..2 * 1024 * 1024u32)
        .map(|i| (i.wrapping_mul(2246822519) >> 11) as u8)
        .collect();
    std::fs::write(&seed, data).unwrap();
    run(
        "qemu-img",
        &[
            "convert",
            "-f",
            "raw",
            "-O",
            "qcow2",
            &seed.to_string_lossy(),
            &disk.to_string_lossy(),
        ],
    );
    std::fs::remove_file(&seed).unwrap();
    let original = content_sha(&disk);

    // Back up.
    let (s, backup) = a
        .post(
            "/backups",
            json!({"target_id": vm, "mode": "stop", "compression": "zstd", "notes": "e2e"}),
        )
        .await;
    assert_eq!(s, 201, "create backup: {backup}");
    let bid = backup["id"].as_str().unwrap().to_string();
    let archive = PathBuf::from(backup["path"].as_str().unwrap());
    assert!(archive.is_file(), "archive {} exists", archive.display());
    let listing = run(
        "sh",
        &["-c", &format!("zstd -dc '{}' | tar -t", archive.display())],
    );
    assert!(listing.contains("disk-0.qcow2"), "{listing}");
    let (s, v) = a.post(&format!("/backups/{bid}/verify"), json!({})).await;
    assert_eq!(s, 200);
    assert_eq!(v["ok"], true, "{v}");

    // Corrupt, then delete the original disk; restore each time.
    std::fs::write(&disk, vec![0u8; 65536]).unwrap();
    assert_ne!(
        std::fs::read(&disk).unwrap().len(),
        0,
        "disk really overwritten"
    );
    let (s, r) = a.post(&format!("/backups/{bid}/restore"), json!({})).await;
    assert_eq!(s, 200, "restore over corrupted: {r}");
    assert_eq!(r["replaced"], true);
    assert_eq!(content_sha(&vm_disk(&a, vm).await), original);

    std::fs::remove_file(vm_disk(&a, vm).await).unwrap();
    let (s, r) = a.post(&format!("/backups/{bid}/restore"), json!({})).await;
    assert_eq!(s, 200, "restore over deleted disk: {r}");
    assert_eq!(content_sha(&vm_disk(&a, vm).await), original);

    // Restore to a new VM.
    let (s, r) = a
        .post(
            &format!("/backups/{bid}/restore"),
            json!({"target_id": copy, "name": "restored copy"}),
        )
        .await;
    assert_eq!(s, 200, "restore to new vm: {r}");
    let (s, c) = a.get(&format!("/vms/{copy}")).await;
    assert_eq!(s, 200);
    assert_eq!(c["name"], "restored copy");
    assert_eq!(content_sha(&vm_disk(&a, copy).await), original);
    // ...and not over it again without force.
    let (s, _) = a
        .post(
            &format!("/backups/{bid}/restore"),
            json!({"target_id": copy}),
        )
        .await;
    assert_eq!(s, 409);

    // Running VM: suspend mode pauses it over QMP for the copy and resumes
    // it; stop mode stops, copies and restarts it. Both archives hold the
    // same disk content.
    let (s, r) = a.post(&format!("/vms/{vm}/start"), json!({})).await;
    assert!(s == 200 || s == 201, "start vm: {s} {r}");
    let pid = || -> Option<u32> {
        std::fs::read_to_string(format!("/var/run/horcrux-vm-{vm}.pid"))
            .ok()
            .and_then(|p| p.trim().parse().ok())
            .filter(|p| Path::new(&format!("/proc/{p}")).exists())
    };
    let first_pid = pid().expect("VM process running");
    let (s, live) = a
        .post(
            "/backups",
            json!({"target_id": vm, "mode": "suspend", "compression": "zstd"}),
        )
        .await;
    assert_eq!(s, 201, "suspend-mode backup of a running VM: {live}");
    assert_eq!(
        pid(),
        Some(first_pid),
        "suspend mode keeps the same QEMU process"
    );
    let (s, r) = a
        .post(
            &format!("/backups/{}/restore", live["id"].as_str().unwrap()),
            json!({"target_id": copy, "force": true}),
        )
        .await;
    assert_eq!(s, 200, "restore live backup: {r}");
    assert_eq!(content_sha(&vm_disk(&a, copy).await), original);

    // Snapshot mode backs the running VM up without pausing it (QMP
    // blockdev-backup copy-before-write): the guest's run state is polled
    // over QMP for the whole request and must never leave "running".
    let qmp = vm_disk(&a, vm).await.with_extension("qmp.sock");
    let stop_poll = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let poller = {
        let stop_poll = stop_poll.clone();
        std::thread::spawn(move || {
            let mut seen = std::collections::BTreeSet::new();
            while !stop_poll.load(std::sync::atomic::Ordering::Relaxed) {
                if let Some(st) = qmp_status(&qmp) {
                    seen.insert(st);
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            seen
        })
    };
    let (s, snap) = a
        .post(
            "/backups",
            json!({"target_id": vm, "mode": "snapshot", "compression": "zstd"}),
        )
        .await;
    stop_poll.store(true, std::sync::atomic::Ordering::Relaxed);
    let states = poller.join().unwrap();
    assert_eq!(s, 201, "snapshot-mode backup of a running VM: {snap}");
    assert_eq!(
        pid(),
        Some(first_pid),
        "snapshot mode keeps the same QEMU process"
    );
    assert!(
        !states.is_empty(),
        "QMP status was observed during the backup"
    );
    assert!(
        states.iter().all(|s| s == "running"),
        "the guest must not be paused by a snapshot-mode backup: saw {states:?}"
    );
    let (s, r) = a
        .post(
            &format!("/backups/{}/restore", snap["id"].as_str().unwrap()),
            json!({"target_id": copy, "force": true}),
        )
        .await;
    assert_eq!(s, 200, "restore snapshot-mode backup: {r}");
    assert_eq!(content_sha(&vm_disk(&a, copy).await), original);
    let (s, stopped) = a
        .post(
            "/backups",
            json!({"target_id": vm, "mode": "stop", "compression": "gzip"}),
        )
        .await;
    assert_eq!(s, 201, "stop-mode backup of a running VM: {stopped}");
    let second_pid = pid().expect("VM restarted after stop-mode backup");
    assert_ne!(second_pid, first_pid, "stop mode restarted QEMU");
    let (_, v) = a
        .post(
            &format!("/backups/{}/verify", stopped["id"].as_str().unwrap()),
            json!({}),
        )
        .await;
    assert_eq!(v["ok"], true, "{v}");
    // Restoring over a running VM is refused.
    let (s, _) = a.post(&format!("/backups/{bid}/restore"), json!({})).await;
    assert_eq!(s, 409);
    let (s, r) = a.post(&format!("/vms/{vm}/stop"), json!({})).await;
    assert!(s == 200 || s == 201, "stop vm: {s} {r}");
    for _ in 0..100 {
        if pid().is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Protect blocks delete.
    let (s, _) = a
        .call(
            reqwest::Method::PUT,
            &format!("/backups/{bid}"),
            Some(json!({"protected": true})),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(a.delete(&format!("/backups/{bid}")).await, 409);

    // A corrupted archive fails verification.
    let mut bytes = std::fs::read(&archive).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0x5a;
    std::fs::write(&archive, &bytes).unwrap();
    let (_, v) = a.post(&format!("/backups/{bid}/verify"), json!({})).await;
    assert_eq!(v["ok"], false, "{v}");
    let (_, b) = a.get(&format!("/backups/{bid}")).await;
    assert_eq!(b["verify_state"], "failed");

    // The server's scheduler loop fires an every-minute job, which keeps
    // only the newest backup it makes (keep_last=1, its own backups and
    // the protected one aside).
    let (s, j) = a
        .post(
            "/backup-jobs",
            json!({"id": job, "schedule": "* * * * *", "targets": [vm], "mode": "snapshot",
                   "compression": "gzip", "retention": {"keep_last": 1}}),
        )
        .await;
    assert_eq!(s, 201, "create job: {j}");
    assert!(j["next_run"].as_i64().is_some(), "{j}");
    let deadline = Instant::now() + Duration::from_secs(200);
    let mut runs = 0;
    let mut last_run: Option<i64> = None;
    while Instant::now() < deadline && runs < 2 {
        let (_, j) = a.get(&format!("/backup-jobs/{job}")).await;
        let lr = j["last_run"].as_i64();
        if lr.is_some() && lr != last_run && j["last_status"].is_string() {
            assert_eq!(j["last_status"], "ok", "scheduled run failed: {j}");
            last_run = lr;
            runs += 1;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    assert_eq!(runs, 2, "scheduler did not run the every-minute job twice");
    let (_, list) = a.get(&format!("/backups?target_id={vm}")).await;
    let list = list.as_array().unwrap();
    let from_job: Vec<&Value> = list.iter().filter(|b| b["job_id"] == job).collect();
    assert_eq!(from_job.len(), 1, "keep_last=1 retention applied: {list:?}");
    assert!(
        list.iter().any(|b| b["id"] == bid.as_str()),
        "protected backup survives retention"
    );

    // Cleanup.
    a.delete(&format!("/backup-jobs/{job}")).await;
    a.call(
        reqwest::Method::PUT,
        &format!("/backups/{bid}"),
        Some(json!({"protected": false})),
    )
    .await;
    let (_, list) = a.get(&format!("/backups?target_id={vm}")).await;
    for b in list.as_array().unwrap() {
        a.delete(&format!("/backups/{}", b["id"].as_str().unwrap()))
            .await;
    }
    for id in [vm, copy] {
        a.delete(&format!("/vms/{id}")).await;
    }
}
