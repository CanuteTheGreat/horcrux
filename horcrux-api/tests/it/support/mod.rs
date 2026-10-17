//! Shared harness for the test-depth suites (journeys, restart/crash,
//! upgrade, cluster/HA, RBAC matrix): runs the real `horcrux-api` binary
//! on its own port and data directory and talks to it over HTTP(S).
//!
//! Transport (the API serves HTTPS by default from a node-local CA):
//! - `HORCRUX_DEPTH_SCHEME` (`https` default; `http` for a server built or
//!   configured without TLS);
//! - `HORCRUX_DEPTH_CA`: PEM CA to trust; `{ssl}` in it is replaced by the
//!   spawned server's SSL directory. Default: `{ssl}/ca.pem`, the node-local
//!   CA the server creates on first start (certificate verified for real,
//!   never `danger_accept_invalid_certs`);
//! - `HORCRUX_DEPTH_ADMIN_PASSWORD` (default `admin`, passed to the server
//!   as `ADMIN_PASSWORD`).
#![allow(dead_code)]

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub struct Server {
    pub child: Option<Child>,
    pub port: u16,
    pub dir: PathBuf,
    pub bin: PathBuf,
    pub extra_env: Vec<(String, String)>,
    /// `https` (default, see `scheme()`) or `http` (e.g. an old release).
    pub scheme: String,
    run: u32,
}

pub fn scheme() -> String {
    std::env::var("HORCRUX_DEPTH_SCHEME").unwrap_or_else(|_| "https".into())
}

pub fn admin_password() -> String {
    std::env::var("HORCRUX_DEPTH_ADMIN_PASSWORD").unwrap_or_else(|_| "admin".into())
}

/// HTTP client trusting `HORCRUX_DEPTH_CA` (with `{ssl}` -> `ssl_dir`).
pub fn http_client(ssl_dir: Option<&Path>, timeout: Duration) -> reqwest::Client {
    let mut b = reqwest::Client::builder().timeout(timeout);
    let ca = std::env::var("HORCRUX_DEPTH_CA").unwrap_or_else(|_| "{ssl}/ca.pem".into());
    let ca = match ssl_dir {
        Some(d) => Some(ca.replace("{ssl}", &d.to_string_lossy())),
        None if !ca.contains("{ssl}") => Some(ca),
        None => None,
    };
    if let Some(ca) = ca {
        // Missing until the server has created its CA on first start.
        if let Ok(pem) = std::fs::read(&ca) {
            let cert =
                reqwest::Certificate::from_pem(&pem).unwrap_or_else(|e| panic!("CA {ca}: {e}"));
            b = b.add_root_certificate(cert);
        }
    }
    b.build().unwrap()
}

pub fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A fresh scratch directory under $HORCRUX_DEPTH_DIR (default the
/// system temp dir), removed first if a previous run left it behind.
pub fn scratch(name: &str) -> PathBuf {
    let root = std::env::var("HORCRUX_DEPTH_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    let d = root.join(format!("hcx-depth-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

impl Server {
    pub fn new(dir: &Path) -> Server {
        Self::with_bin(dir, PathBuf::from(env!("CARGO_BIN_EXE_horcrux-api")))
    }

    pub fn with_bin(dir: &Path, bin: PathBuf) -> Server {
        Server {
            child: None,
            port: free_port(),
            dir: dir.to_path_buf(),
            bin,
            extra_env: Vec::new(),
            scheme: scheme(),
            run: 0,
        }
    }

    pub fn env(mut self, k: &str, v: &str) -> Self {
        self.extra_env.push((k.into(), v.into()));
        self
    }

    pub fn data(&self) -> PathBuf {
        self.dir.join("data")
    }

    pub fn start(&mut self) {
        self.run += 1;
        let log = self.dir.join(format!("server-{}.log", self.run));
        let log_file = std::fs::File::create(&log).unwrap();
        let cni = self.dir.join("cni");
        std::fs::create_dir_all(cni.join("bin")).unwrap();
        std::fs::create_dir_all(cni.join("conf.d")).unwrap();
        std::fs::create_dir_all(self.data()).unwrap();
        let mut cmd = Command::new(&self.bin);
        cmd.env("HORCRUX_HOST", "127.0.0.1")
            .env("HORCRUX_PORT", self.port.to_string())
            .env("HORCRUX_DATA_DIR", self.data())
            .env(
                "HORCRUX_DATABASE_URL",
                format!("sqlite://{}", self.data().join("horcrux.db").display()),
            )
            .env("HORCRUX_LOG_DIR", self.dir.join("log"))
            .env("HORCRUX_AUDIT_LOG", self.dir.join("log/audit.log"))
            .env("HORCRUX_SSL_DIR", self.dir.join("ssl"))
            .env("HORCRUX_QEMU_RUN_DIR", self.dir.join("run"))
            .env("HORCRUX_CNI_BIN_DIR", cni.join("bin"))
            .env("HORCRUX_CNI_CONF_DIR", cni.join("conf.d"))
            .env("ADMIN_PASSWORD", admin_password())
            .env("RATE_LIMIT_MAX_REQUESTS", "100000")
            .env("AUTH_RATE_LIMIT_MAX_REQUESTS", "100000")
            .env("RUST_LOG", "info")
            .stdout(Stdio::from(log_file.try_clone().unwrap()))
            .stderr(Stdio::from(log_file));
        for (k, v) in &self.extra_env {
            cmd.env(k, v);
        }
        self.child = Some(cmd.spawn().expect("failed to spawn horcrux-api"));
    }

    pub fn pid(&self) -> u32 {
        self.child.as_ref().unwrap().id()
    }

    pub fn base(&self) -> String {
        format!("{}://127.0.0.1:{}/api", self.scheme, self.port)
    }

    pub fn client(&self) -> reqwest::Client {
        http_client(Some(&self.dir.join("ssl")), Duration::from_secs(600))
    }

    pub fn log_text(&self) -> String {
        std::fs::read_to_string(self.dir.join(format!("server-{}.log", self.run)))
            .unwrap_or_default()
    }

    pub async fn wait_healthy(&mut self) {
        for _ in 0..240 {
            // Rebuilt each time: the CA file appears once the server is up.
            let c = self.client();
            if let Some(status) = self.child.as_mut().unwrap().try_wait().unwrap() {
                panic!("horcrux-api exited early ({status}):\n{}", self.log_text());
            }
            if let Ok(r) = c.get(format!("{}/health", self.base())).send().await {
                if r.status().is_success() {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        panic!("horcrux-api never became healthy:\n{}", self.log_text());
    }

    pub fn signal(&mut self, sig: nix::sys::signal::Signal) {
        let Some(child) = self.child.as_mut() else {
            return;
        };
        let pid = nix::unistd::Pid::from_raw(child.id() as i32);
        let _ = nix::sys::signal::kill(pid, sig);
        for _ in 0..300 {
            if child.try_wait().unwrap().is_some() {
                self.child = None;
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = child.kill();
        let _ = child.wait();
        self.child = None;
    }

    pub fn kill9(&mut self) {
        self.signal(nix::sys::signal::Signal::SIGKILL)
    }

    pub fn term(&mut self) {
        self.signal(nix::sys::signal::Signal::SIGTERM)
    }

    pub async fn restart_after(&mut self, sig: nix::sys::signal::Signal) {
        self.signal(sig);
        self.start();
        self.wait_healthy().await;
    }

    pub async fn api(&self) -> Api {
        Api::login_with(self.client(), &self.base(), "admin", &admin_password()).await
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(c) = self.child.as_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

#[derive(Clone)]
pub struct Api {
    pub c: reqwest::Client,
    pub base: String,
    pub token: String,
}

impl Api {
    pub async fn login_with(c: reqwest::Client, base: &str, user: &str, pass: &str) -> Api {
        Self::try_login(c, base, user, pass)
            .await
            .unwrap_or_else(|e| panic!("login {user}: {e}"))
    }

    pub async fn try_login(
        c: reqwest::Client,
        base: &str,
        user: &str,
        pass: &str,
    ) -> Result<Api, String> {
        for _ in 0..60 {
            let r = c
                .post(format!("{base}/auth/login"))
                .json(&json!({"username": user, "password": pass}))
                .send()
                .await
                .map_err(|e| e.to_string())?;
            if r.status() == 429 {
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
            let st = r.status();
            let v: Value = r.json().await.unwrap_or(Value::Null);
            if !st.is_success() {
                return Err(format!("{st} {v}"));
            }
            let token = v["ticket"]
                .as_str()
                .or_else(|| v["token"].as_str())
                .ok_or_else(|| format!("no token in {v}"))?
                .to_string();
            return Ok(Api {
                c,
                base: base.to_string(),
                token,
            });
        }
        Err("rate limited".into())
    }

    pub async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let mut r = self
            .c
            .request(method, format!("{}{}", self.base, path))
            .bearer_auth(&self.token);
        if let Some(b) = body {
            r = r.json(&b);
        }
        let r = match r.send().await {
            Ok(r) => r,
            Err(e) => return (0, Value::String(e.to_string())),
        };
        let status = r.status().as_u16();
        let text = r.text().await.unwrap_or_default();
        (
            status,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }
    pub async fn get(&self, p: &str) -> (u16, Value) {
        self.call(reqwest::Method::GET, p, None).await
    }
    pub async fn post(&self, p: &str, b: Value) -> (u16, Value) {
        self.call(reqwest::Method::POST, p, Some(b)).await
    }
    pub async fn put(&self, p: &str, b: Value) -> (u16, Value) {
        self.call(reqwest::Method::PUT, p, Some(b)).await
    }
    pub async fn delete(&self, p: &str) -> u16 {
        self.call(reqwest::Method::DELETE, p, None).await.0
    }

    pub async fn ok(&self, method: reqwest::Method, p: &str, b: Option<Value>) -> Value {
        let (s, v) = self.call(method.clone(), p, b).await;
        assert!((200..300).contains(&s), "{method} {p}: {s} {v}");
        v
    }
}

pub fn run(prog: &str, args: &[&str]) -> String {
    let out = Command::new(prog).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "{prog} {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// SHA-256 of a disk image's guest-visible contents (any format qemu-img reads).
pub fn content_sha(disk: &Path) -> String {
    let raw = disk.with_extension(format!("cmp-{}.raw", std::process::id()));
    let _ = std::fs::remove_file(&raw);
    run(
        "qemu-img",
        &[
            "convert",
            "-U",
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

/// PID of the QEMU process horcrux started for `vm_id`, if it is alive
/// (and not a zombie).
pub fn qemu_pid(vm_id: &str) -> Option<u32> {
    let pid: u32 = std::fs::read_to_string(format!("/var/run/horcrux-vm-{vm_id}.pid"))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let state = stat.rsplit(')').next()?.split_whitespace().next()?;
    (state != "Z").then_some(pid)
}

pub async fn wait_until<F: FnMut() -> bool>(what: &str, secs: u64, mut f: F) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("timed out after {secs}s waiting for {what}");
}

/// One QMP command against a VM's QMP socket (QMP serves one client at a
/// time: connect, negotiate, run, disconnect).
pub fn qmp(sock: &Path, cmd: Value) -> Value {
    let mut s =
        UnixStream::connect(sock).unwrap_or_else(|e| panic!("QMP connect {}: {e}", sock.display()));
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    let mut rd = BufReader::new(s.try_clone().unwrap());
    let mut line = String::new();
    rd.read_line(&mut line).unwrap(); // greeting
    s.write_all(b"{\"execute\":\"qmp_capabilities\"}\n")
        .unwrap();
    loop {
        line.clear();
        rd.read_line(&mut line).unwrap();
        if line.contains("\"return\"") {
            break;
        }
    }
    s.write_all(format!("{cmd}\n").as_bytes()).unwrap();
    loop {
        line.clear();
        if rd.read_line(&mut line).unwrap() == 0 {
            panic!("QMP closed");
        }
        let v: Value = serde_json::from_str(&line).unwrap();
        if v.get("return").is_some() || v.get("error").is_some() {
            return v;
        }
    }
}

/// Interactive session on a guest's serial console Unix socket.
pub struct Serial {
    s: UnixStream,
    pub seen: String,
}

impl Serial {
    pub fn connect(sock: &Path) -> Serial {
        let s = UnixStream::connect(sock)
            .unwrap_or_else(|e| panic!("serial connect {}: {e}", sock.display()));
        s.set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        Serial {
            s,
            seen: String::new(),
        }
    }

    pub fn send(&mut self, text: &str) {
        self.s.write_all(text.as_bytes()).unwrap();
    }

    /// Read until `needle` appears in the output (after the current
    /// position), poking the console with `poke` every few seconds.
    pub fn expect(&mut self, needle: &str, secs: u64, poke: Option<&str>) -> String {
        let start = self.seen.len();
        let deadline = Instant::now() + Duration::from_secs(secs);
        let mut last_poke = Instant::now();
        let mut buf = [0u8; 4096];
        while Instant::now() < deadline {
            match self.s.read(&mut buf) {
                Ok(0) => panic!("serial console closed; seen:\n{}", self.tail()),
                Ok(n) => self.seen.push_str(&String::from_utf8_lossy(&buf[..n])),
                Err(_) => {}
            }
            if let Some(i) = self.seen[start..].find(needle) {
                return self.seen[start..start + i + needle.len()].to_string();
            }
            if let Some(p) = poke {
                if last_poke.elapsed() > Duration::from_secs(5) {
                    self.send(p);
                    last_poke = Instant::now();
                }
            }
        }
        panic!(
            "serial: no {needle:?} within {secs}s; last output:\n{}",
            self.tail()
        );
    }

    pub fn tail(&self) -> String {
        let n = self.seen.len();
        self.seen[n.saturating_sub(3000)..].to_string()
    }

    /// Log in to a CirrOS guest and wait for the shell prompt.
    pub fn cirros_login(&mut self, boot_secs: u64) {
        self.expect("login:", boot_secs, Some("\n"));
        self.send("cirros\n");
        self.expect("assword:", 60, None);
        self.send("gocubsgo\n");
        self.expect("$ ", 60, Some("\n"));
    }

    /// Run a shell command and return its output between two markers.
    pub fn sh(&mut self, cmd: &str, secs: u64) -> String {
        let tag = format!("M{}", rand_tag());
        self.send(&format!("echo {tag}B; {cmd}; echo {tag}E\n"));
        // The echoed command line contains the markers too; wait for the
        // output lines (marker at line start).
        self.expect(&format!("\n{tag}B"), secs, None);
        let out = self.expect(&format!("\n{tag}E"), secs, None);
        out.trim_end_matches(&format!("{tag}E"))
            .trim()
            .replace('\r', "")
    }
}

fn rand_tag() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
        % 1_000_000_007
}

/// Path of a CirrOS (or other) cloud image to boot, from
/// HORCRUX_DEPTH_IMAGE. Panics with a clear message when unset, since the
/// depth suites must never pass by silently testing nothing.
pub fn cloud_image() -> PathBuf {
    let p = std::env::var("HORCRUX_DEPTH_IMAGE")
        .expect("HORCRUX_DEPTH_IMAGE must point at a CirrOS cloud image (qcow2)");
    let p = PathBuf::from(p);
    assert!(p.is_file(), "{} missing", p.display());
    p
}
