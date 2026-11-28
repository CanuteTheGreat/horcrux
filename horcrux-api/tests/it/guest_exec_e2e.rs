//! `POST /api/vms/:id/guest-exec` end to end against a freshly spawned
//! horcrux-api: auth (JWT, API key as X-API-Key and as a bearer token),
//! RBAC, audit, limits, and the answer shapes, with a scripted guest agent
//! listening on the VM's `<disk>.qga.sock` (the real agent protocol: the
//! `guest-sync-delimited` handshake, `guest-exec`, `guest-exec-status`).
//! Also the hard-reset route's refusal/RBAC and the declarative-free parts
//! of the service-account story.

use crate::support::{scratch, Server};
use base64::Engine as _;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::sync::{Arc, Mutex};

fn b64(s: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(s)
}

/// A guest agent that answers like qemu-ga. Programs: `/bin/false` exits 3,
/// `/bin/sleep-forever` never exits, `/bin/killed` dies from signal 9,
/// `/bin/blocked` is refused by the agent's RPC policy, `/bin/big` prints
/// 64 KiB; anything else echoes `path args` on stdout and its stdin on
/// stderr and exits 0. A command reports "running" on its first status
/// poll, then its result.
fn spawn_fake_agent(sock: &Path) {
    let _ = std::fs::remove_file(sock);
    let l = UnixListener::bind(sock).unwrap();
    #[derive(Clone)]
    struct Job {
        path: String,
        args: Vec<String>,
        stdin: Vec<u8>,
        polls: u32,
    }
    let jobs: Arc<Mutex<HashMap<i64, Job>>> = Arc::default();
    std::thread::spawn(move || {
        for (pid_base, conn) in (1001..).zip(l.incoming().flatten()) {
            let jobs = jobs.clone();
            std::thread::spawn(move || {
                let mut w = conn.try_clone().unwrap();
                let mut r = BufReader::new(conn);
                let mut line = Vec::new();
                while r.read_until(b'\n', &mut line).unwrap_or(0) > 0 {
                    let raw: Vec<u8> = line.iter().copied().skip_while(|b| *b == 0xFF).collect();
                    line.clear();
                    let Ok(req) = serde_json::from_slice::<Value>(&raw) else {
                        continue;
                    };
                    let a = &req["arguments"];
                    let reply = match req["execute"].as_str().unwrap_or("") {
                        "guest-sync-delimited" => {
                            let mut out = vec![0xFFu8];
                            out.extend(json!({"return": a["id"]}).to_string().bytes());
                            out.push(b'\n');
                            w.write_all(&out).unwrap();
                            continue;
                        }
                        "guest-exec" => {
                            let path = a["path"].as_str().unwrap_or("").to_string();
                            if path == "/bin/blocked" {
                                json!({"error": {"class": "CommandNotFound",
                                    "desc": "The command guest-exec has been disabled for this instance"}})
                            } else {
                                let pid = pid_base * 10 + jobs.lock().unwrap().len() as i64;
                                let stdin = a["input-data"]
                                    .as_str()
                                    .map(|s| {
                                        base64::engine::general_purpose::STANDARD.decode(s).unwrap()
                                    })
                                    .unwrap_or_default();
                                let args = a["arg"]
                                    .as_array()
                                    .map(|v| {
                                        v.iter()
                                            .map(|x| x.as_str().unwrap_or("").to_string())
                                            .collect()
                                    })
                                    .unwrap_or_default();
                                jobs.lock().unwrap().insert(
                                    pid,
                                    Job {
                                        path,
                                        args,
                                        stdin,
                                        polls: 0,
                                    },
                                );
                                json!({"return": {"pid": pid}})
                            }
                        }
                        "guest-exec-status" => {
                            let pid = a["pid"].as_i64().unwrap_or(0);
                            let mut m = jobs.lock().unwrap();
                            match m.get_mut(&pid) {
                                None => json!({"error": {"desc": "PID not found"}}),
                                Some(j) => {
                                    j.polls += 1;
                                    if j.polls < 2 || j.path == "/bin/sleep-forever" {
                                        json!({"return": {"exited": false}})
                                    } else if j.path == "/bin/killed" {
                                        json!({"return": {"exited": true, "signal": 9}})
                                    } else if j.path == "/bin/big" {
                                        json!({"return": {"exited": true, "exitcode": 0,
                                            "out-data": b64(&vec![b'x'; 65536]),
                                            "out-truncated": false}})
                                    } else {
                                        let code = if j.path == "/bin/false" { 3 } else { 0 };
                                        let out = format!("{} {}", j.path, j.args.join(" "));
                                        json!({"return": {"exited": true, "exitcode": code,
                                            "out-data": b64(out.as_bytes()),
                                            "err-data": b64(&j.stdin)}})
                                    }
                                }
                            }
                        }
                        _ => json!({"error": {"desc": "unknown command"}}),
                    };
                    writeln!(w, "{reply}").unwrap();
                }
            });
        }
    });
}

async fn declared_vm(api: &crate::support::Api, dir: &Path) -> (String, std::path::PathBuf) {
    let pool = dir.join("pool");
    std::fs::create_dir_all(&pool).unwrap();
    let spec = format!(
        "version: 1\nstorage_pools:\n  - {{name: gx, type: directory, path: {}}}\nvms:\n  - name: gx-vm\n    memory: 128\n    cpus: 1\n    disks: [{{size_gb: 1, storage: gx}}]\n    display: {{protocol: none}}\n    power: stopped\n",
        pool.display()
    );
    let (s, v) = api.post("/declarative/apply", json!({"spec": spec})).await;
    assert!(s == 200 && v["ok"] == true, "{v}");
    let (_, vms) = api.get("/vms").await;
    let vm = vms
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == "gx-vm")
        .expect("vm exists");
    (
        vm["id"].as_str().unwrap().to_string(),
        std::path::PathBuf::from(vm["disks"][0]["path"].as_str().unwrap()),
    )
}

/// Service accounts are system (PAM) accounts, as on a real node: the local
/// realm has no writable store without ADMIN_PASSWORD.
async fn user_with_privs(api: &crate::support::Api, name: &str, privs: &[&str]) -> String {
    let (s, v) = api
        .post(
            "/roles",
            json!({"name": format!("role-{name}"), "privileges": privs}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let (s, v) = api
        .post(
            "/users",
            json!({"username": name, "password": "svc-test-pass-1", "email": format!("{name}@example.com")}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let (s, v) = api
        .post(
            "/acl",
            json!({"path": "/vms", "subject_type": "user", "subject": format!("{name}@pam"), "role": format!("role-{name}")}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let (s, v) = api
        .post(&format!("/users/{name}@pam/api-keys"), json!({"name": "svc"}))
        .await;
    assert_eq!(s, 200, "{v}");
    v["key"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn guest_exec_runs_commands_through_the_agent_with_rbac_audit_and_keys() {
    let dir = scratch("guest-exec");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;
    let (vm, disk) = declared_vm(&api, &dir).await;
    let sock = disk.with_extension("qga.sock");
    let url = format!("/vms/{vm}/guest-exec");

    // no agent channel yet: a stopped VM is a conflict, not a 500
    let (s, v) = api.post(&url, json!({"path": "/bin/true"})).await;
    assert_eq!(s, 409, "{v}");
    spawn_fake_agent(&sock);

    // output, stdin and exit status come back
    let (s, v) = api
        .post(
            &url,
            json!({"path": "/bin/echo", "args": ["hi", "there"], "stdin": "fed in", "env": ["A=1"]}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["exit_code"], 0, "{v}");
    assert_eq!(v["stdout"], "/bin/echo hi there");
    assert_eq!(v["stderr"], "fed in");
    assert_eq!(v["timed_out"], false);
    assert_eq!(v["encoding"], "utf8");

    // a non-zero exit is still HTTP 200; binary stdin and base64 output
    let (s, v) = api
        .post(
            &url,
            json!({"path": "/bin/false", "stdin_base64": b64(&[0, 1, 255]), "encoding": "base64"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["exit_code"], 3);
    assert_eq!(v["stderr"], b64(&[0, 1, 255]));
    assert_eq!(v["stdout"], b64(b"/bin/false "));

    // killed by a signal
    let (_, v) = api.post(&url, json!({"path": "/bin/killed"})).await;
    assert_eq!(
        (&v["signal"], &v["exit_code"]),
        (&json!(9), &Value::Null),
        "{v}"
    );

    // a command that does not finish: timed_out after timeout_secs
    let t = std::time::Instant::now();
    let (s, v) = api
        .post(
            &url,
            json!({"path": "/bin/sleep-forever", "timeout_secs": 1}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        (&v["timed_out"], &v["exited"]),
        (&json!(true), &json!(false)),
        "{v}"
    );
    assert!(t.elapsed().as_secs() < 10);

    // the agent's own policy: 424 with a reason code
    let (s, v) = api.post(&url, json!({"path": "/bin/blocked"})).await;
    assert_eq!(s, 424, "{v}");
    assert_eq!(v["reason"], "guest-exec-disabled", "{v}");

    // input checks: 422
    for bad in [
        json!({"path": ""}),
        json!({"path": "x", "env": ["NOEQ"]}),
        json!({"path": "x", "timeout_secs": 0}),
        json!({"path": "x", "timeout_secs": 999_999}),
        json!({"path": "x", "stdin_base64": "!!"}),
        json!({"path": "x", "encoding": "latin1"}),
    ] {
        let (s, v) = api.post(&url, bad.clone()).await;
        assert_eq!(s, 422, "{bad}: {v}");
    }
    let (s, _) = api
        .post("/vms/no-such-vm/guest-exec", json!({"path": "x"}))
        .await;
    assert_eq!(s, 404);

    // output is cut at the node's limit, flagged
    let (_, v) = api.post(&url, json!({"path": "/bin/big"})).await;
    assert_eq!(
        v["stdout"].as_str().unwrap().len(),
        65536,
        "default limit is larger"
    );

    // RBAC: VM.Audit alone may not run commands; VM.Console may. Keys carry
    // their owner's rights, as X-API-Key and as a bearer token.
    let viewer_key = user_with_privs(&api, "gx-viewer", &["VM.Audit"]).await;
    let runner_key = user_with_privs(&api, "gx-runner", &["VM.Audit", "VM.Console"]).await;
    let c = srv.client();
    let post_with = |key: &str, bearer: bool, p: &str| {
        let r = c
            .post(format!("{}{}", srv.base(), p))
            .json(&json!({"path": "/bin/echo", "args": ["k"]}));
        let r = if bearer {
            r.bearer_auth(key)
        } else {
            r.header("X-API-Key", key)
        };
        r.send()
    };
    assert_eq!(
        post_with(&viewer_key, true, &url).await.unwrap().status(),
        403
    );
    assert_eq!(
        post_with(&viewer_key, false, &url).await.unwrap().status(),
        403
    );
    for bearer in [true, false] {
        let r = post_with(&runner_key, bearer, &url).await.unwrap();
        assert_eq!(r.status(), 200, "bearer={bearer}");
        let v: Value = r.json().await.unwrap();
        assert_eq!(v["stdout"], "/bin/echo k");
    }
    // a wrong or malformed key is a 401, not a fallback to something else
    for k in ["hx_wrongwrongwrongwrongwrongwrongwrong", "hx_short"] {
        assert_eq!(post_with(k, true, &url).await.unwrap().status(), 401, "{k}");
        assert_eq!(
            post_with(k, false, &url).await.unwrap().status(),
            401,
            "{k}"
        );
    }
    // reset needs VM.PowerMgmt (and a running VM)
    let reset = format!("/vms/{vm}/reset");
    assert_eq!(
        post_with(&runner_key, true, &reset).await.unwrap().status(),
        403
    );
    let (s, v) = api.post(&reset, json!({})).await;
    assert_eq!(s, 409, "reset of a stopped VM: {v}");

    // every call left an audit record naming the user and the program
    let (s, a) = api
        .get("/audit/search?event_type=GuestExec&limit=100")
        .await;
    assert_eq!(s, 200, "{a}");
    let events = a["events"].as_array().unwrap();
    assert!(events.len() >= 8, "{a}");
    assert!(events.iter().any(|e| e["user"] == "gx-runner@pam"
        && e["details"]
            .as_str()
            .unwrap_or("")
            .contains("program=/bin/echo")));
    // argument values stay out of the audit record by default
    assert!(!a.to_string().contains("there"), "{a}");

    // switch the route off node-wide: 424
    let mut srv2 = Server::new(&dir.join("off")).env("HORCRUX_GUEST_EXEC_ENABLED", "false");
    std::fs::create_dir_all(dir.join("off")).unwrap();
    srv2.start();
    srv2.wait_healthy().await;
    let api2 = srv2.api().await;
    let (s, v) = api2.post("/vms/x/guest-exec", json!({"path": "x"})).await;
    assert_eq!(s, 424, "{v}");
    assert_eq!(v["reason"], "guest-exec-off");
}
