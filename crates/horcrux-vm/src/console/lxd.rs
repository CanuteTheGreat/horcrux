//! LXD / Incus REST API client over the daemon's local Unix socket, for the
//! in-browser consoles of LXD/Incus instances:
//!
//! * `POST /1.0/instances/<name>/console` `{type: console}` - the serial /
//!   text console (VMs: the guest's ttyS0; containers: /dev/console).
//! * `POST /1.0/instances/<name>/console` `{type: vga}` - the VM's SPICE
//!   display as a raw byte stream (see `console::spice_relay`).
//! * `POST /1.0/instances/<name>/exec` `{interactive: true}` - a shell on a
//!   PTY inside a container.
//! * `GET /1.0/instances/<name>/console` - console log history.
//!
//! Each of the POSTs returns a websocket operation; its data (`0`) and
//! `control` websockets are opened over the same Unix socket. The daemon
//! only starts the session once every websocket of the operation is
//! connected, so both are always connected.
//!
//! LXD and Incus share this API (Incus is the LXD fork); only the socket
//! path differs.
use futures::{SinkExt, StreamExt};
use horcrux_common::{Error, Result};
use serde_json::{json, Value};
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::WebSocketStream;

/// Which daemon an instance belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Lxd,
    Incus,
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self {
            Backend::Lxd => "LXD",
            Backend::Incus => "Incus",
        }
    }

    /// The daemon's local API socket. Overridable with
    /// HORCRUX_LXD_SOCKET / HORCRUX_INCUS_SOCKET; otherwise the first that
    /// exists of the daemon's own env (LXD_DIR / INCUS_SOCKET, INCUS_DIR)
    /// and the packaged defaults (Gentoo, Debian/Ubuntu, snap).
    pub fn socket_path(self) -> PathBuf {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let mut candidates: Vec<PathBuf> = Vec::new();
        match self {
            Backend::Lxd => {
                if let Some(p) = env("HORCRUX_LXD_SOCKET") {
                    return PathBuf::from(p);
                }
                if let Some(d) = env("LXD_DIR") {
                    candidates.push(PathBuf::from(d).join("unix.socket"));
                }
                candidates.push("/var/lib/lxd/unix.socket".into());
                candidates.push("/var/snap/lxd/common/lxd/unix.socket".into());
            }
            Backend::Incus => {
                if let Some(p) = env("HORCRUX_INCUS_SOCKET") {
                    return PathBuf::from(p);
                }
                if let Some(p) = env("INCUS_SOCKET") {
                    candidates.push(PathBuf::from(p));
                }
                if let Some(d) = env("INCUS_DIR") {
                    candidates.push(PathBuf::from(d).join("unix.socket"));
                }
                candidates.push("/var/lib/incus/unix.socket".into());
                candidates.push("/run/incus/unix.socket".into());
            }
        }
        candidates
            .iter()
            .find(|p| p.exists())
            .cloned()
            .unwrap_or_else(|| candidates[0].clone())
    }
}

pub type Ws = WebSocketStream<UnixStream>;

/// A running console/exec session: the data stream and its control channel.
pub struct Session {
    pub data: Ws,
    pub control: Ws,
}

/// Kind of console to open on an instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleKind {
    /// Serial/text console.
    Text,
    /// SPICE display of a VM.
    Vga,
}

pub struct Client {
    backend: Backend,
    socket: PathBuf,
    timeout: std::time::Duration,
}

/// A system container as listed by the daemon.
#[derive(Debug, Clone)]
pub struct ContainerSummary {
    pub name: String,
    pub status: String,
    pub cpus: u32,
    pub memory_mb: u64,
    pub created_at: Option<String>,
}

/// `limits.memory` ("512MB", "1GiB", "268435456") in MB.
fn parse_memory_mb(v: &str) -> u64 {
    let v = v.trim();
    let split = v.find(|c: char| !c.is_ascii_digit()).unwrap_or(v.len());
    let (num, unit) = v.split_at(split);
    let n: u64 = num.parse().unwrap_or(0);
    match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => n / (1024 * 1024),
        "kb" | "kib" => n / 1024,
        "mb" | "mib" => n,
        "gb" | "gib" => n * 1024,
        "tb" | "tib" => n * 1024 * 1024,
        _ => 0,
    }
}

/// Instance state as reported by the daemon.
#[derive(Debug, Clone)]
pub struct InstanceState {
    /// "container" or "virtual-machine"
    pub instance_type: String,
    /// "Running", "Stopped", "Frozen", ...
    pub status: String,
}

impl InstanceState {
    pub fn is_running(&self) -> bool {
        self.status.eq_ignore_ascii_case("running")
    }
    pub fn is_vm(&self) -> bool {
        self.instance_type == "virtual-machine"
    }
}

fn sys(msg: impl Into<String>) -> Error {
    Error::System(msg.into())
}

fn enc(name: &str) -> String {
    urlencoding::encode(name).into_owned()
}

impl Client {
    pub fn new(backend: Backend) -> Self {
        Self {
            backend,
            socket: backend.socket_path(),
            timeout: std::time::Duration::from_secs(60),
        }
    }

    /// Same client with a shorter request timeout (list refreshes).
    pub fn quick(backend: Backend) -> Self {
        Self {
            timeout: std::time::Duration::from_secs(5),
            ..Self::new(backend)
        }
    }

    /// Whether the daemon's socket exists at all (not installed / not
    /// running otherwise).
    pub fn available(&self) -> bool {
        self.socket.exists()
    }

    /// System containers known to the daemon.
    pub async fn containers(&self) -> Result<Vec<ContainerSummary>> {
        let v = self.call("GET", "/1.0/instances?recursion=1", None).await?;
        let list = v["metadata"].as_array().cloned().unwrap_or_default();
        Ok(list
            .iter()
            .filter(|i| i["type"].as_str() == Some("container"))
            .filter_map(|i| {
                Some(ContainerSummary {
                    name: i["name"].as_str()?.to_string(),
                    status: i["status"].as_str().unwrap_or("Unknown").to_string(),
                    cpus: i["config"]["limits.cpu"]
                        .as_str()
                        .and_then(|c| c.parse().ok())
                        .unwrap_or(0),
                    memory_mb: i["config"]["limits.memory"]
                        .as_str()
                        .map(parse_memory_mb)
                        .unwrap_or(0),
                    created_at: i["created_at"].as_str().map(str::to_string),
                })
            })
            .collect())
    }

    async fn connect(&self) -> Result<UnixStream> {
        UnixStream::connect(&self.socket).await.map_err(|e| {
            sys(format!(
                "cannot reach the {} daemon at {}: {e} (is it installed and running?)",
                self.backend.label(),
                self.socket.display()
            ))
        })
    }

    /// One HTTP/1.1 request (Connection: close) -> (status, body).
    async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(u16, Vec<u8>)> {
        let mut s = self.connect().await?;
        let payload = body.map(|b| b.to_string()).unwrap_or_default();
        let mut req = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nUser-Agent: horcrux\r\nConnection: close\r\n"
        );
        if body.is_some() {
            req.push_str(&format!(
                "Content-Type: application/json\r\nContent-Length: {}\r\n",
                payload.len()
            ));
        }
        req.push_str("\r\n");
        req.push_str(&payload);
        s.write_all(req.as_bytes())
            .await
            .map_err(|e| sys(format!("{} API write: {e}", self.backend.label())))?;
        let mut raw = Vec::new();
        tokio::time::timeout(self.timeout, s.read_to_end(&mut raw))
            .await
            .map_err(|_| sys(format!("{} API timed out", self.backend.label())))?
            .map_err(|e| sys(format!("{} API read: {e}", self.backend.label())))?;
        parse_http_response(&raw)
    }

    /// Request expecting a JSON envelope; returns its `metadata` (sync) or
    /// the whole envelope (async). Daemon errors become clear messages.
    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value> {
        let (status, raw) = self.request(method, path, body).await?;
        let v: Value = serde_json::from_slice(&raw).map_err(|e| {
            sys(format!(
                "{} API {method} {path}: HTTP {status}, unexpected body ({e})",
                self.backend.label()
            ))
        })?;
        if v.get("type").and_then(Value::as_str) == Some("error") || status >= 400 {
            let msg = v
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
                .to_string();
            let code = v
                .get("error_code")
                .and_then(Value::as_u64)
                .unwrap_or(status as u64);
            return Err(if code == 404 {
                Error::NotFound(format!("{} instance: {msg}", self.backend.label()))
            } else {
                sys(format!("{}: {msg}", self.backend.label()))
            });
        }
        Ok(v)
    }

    pub async fn instance_state(&self, name: &str) -> Result<InstanceState> {
        let v = self
            .call("GET", &format!("/1.0/instances/{}", enc(name)), None)
            .await?;
        let m = &v["metadata"];
        Ok(InstanceState {
            instance_type: m["type"].as_str().unwrap_or("container").to_string(),
            status: m["status"].as_str().unwrap_or("Unknown").to_string(),
        })
    }

    /// Console log history (what the console printed so far). VMs on older
    /// daemons don't keep one; that is not an error for the caller.
    pub async fn console_log(&self, name: &str) -> Result<Vec<u8>> {
        let (status, body) = self
            .request(
                "GET",
                &format!("/1.0/instances/{}/console", enc(name)),
                None,
            )
            .await?;
        if status >= 400 {
            let msg = serde_json::from_slice::<Value>(&body)
                .ok()
                .and_then(|v| v["error"].as_str().map(str::to_string))
                .unwrap_or_else(|| format!("HTTP {status}"));
            return Err(sys(format!("{} console log: {msg}", self.backend.label())));
        }
        Ok(body)
    }

    /// Start a websocket operation and connect all its websockets.
    async fn websocket_op(&self, path: &str, body: Value) -> Result<Session> {
        let v = self.call("POST", path, Some(&body)).await?;
        let op = &v["metadata"];
        let id = op["id"]
            .as_str()
            .ok_or_else(|| {
                sys(format!(
                    "{}: operation without id: {v}",
                    self.backend.label()
                ))
            })?
            .to_string();
        let fds = &op["metadata"]["fds"];
        let secret = |k: &str| {
            fds[k].as_str().map(str::to_string).ok_or_else(|| {
                sys(format!(
                    "{}: operation has no '{k}' websocket",
                    self.backend.label()
                ))
            })
        };
        let (data_secret, control_secret) = (secret("0")?, secret("control")?);
        // Control first: the daemon waits for every websocket before it
        // starts the session, so the order does not matter beyond that.
        let control = self.ws(&id, &control_secret).await?;
        let data = self.ws(&id, &data_secret).await?;
        Ok(Session { data, control })
    }

    async fn ws(&self, op: &str, secret: &str) -> Result<Ws> {
        let stream = self.connect().await?;
        let url = format!(
            "ws://localhost/1.0/operations/{}/websocket?secret={}",
            enc(op),
            enc(secret)
        );
        let (ws, _) = tokio_tungstenite::client_async(url, stream)
            .await
            .map_err(|e| sys(format!("{} websocket: {e}", self.backend.label())))?;
        Ok(ws)
    }

    pub async fn open_console(&self, name: &str, kind: ConsoleKind) -> Result<Session> {
        let body = match kind {
            ConsoleKind::Text => json!({"type": "console", "width": 120, "height": 40}),
            ConsoleKind::Vga => json!({"type": "vga"}),
        };
        self.websocket_op(&format!("/1.0/instances/{}/console", enc(name)), body)
            .await
    }

    /// Interactive command on a PTY (containers; VMs need the guest agent).
    pub async fn exec_pty(&self, name: &str, command: &[String]) -> Result<Session> {
        let command: Vec<String> = if command.is_empty() {
            vec!["/bin/sh".into(), "-l".into()]
        } else {
            command.to_vec()
        };
        let body = json!({
            "command": command,
            "environment": {
                "TERM": "xterm-256color",
                "HOME": "/root",
                "PATH": "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
            },
            "interactive": true,
            "wait-for-websocket": true,
            "width": 120,
            "height": 40,
        });
        self.websocket_op(&format!("/1.0/instances/{}/exec", enc(name)), body)
            .await
    }
}

/// Send a control message (exec: signal / window-resize).
pub async fn send_control(ws: &mut Ws, msg: Value) {
    let _ = ws.send(WsMessage::Text(msg.to_string())).await;
}

/// Bridge a browser websocket to a daemon console/exec session, both ways,
/// until either side closes. `history` is sent to the browser first.
/// `on_close` decides what to tell the daemon when the browser leaves.
pub async fn bridge(
    browser: axum::extract::ws::WebSocket,
    session: Session,
    history: Vec<u8>,
    hangup_exec: bool,
) {
    use axum::extract::ws::Message as AxMsg;
    let (mut b_tx, mut b_rx) = browser.split();
    let Session { data, mut control } = session;
    let (mut d_tx, mut d_rx) = data.split();

    if !history.is_empty() && b_tx.send(AxMsg::Binary(history)).await.is_err() {
        return;
    }

    let up = async {
        while let Some(msg) = b_rx.next().await {
            let bytes = match msg {
                Ok(AxMsg::Binary(d)) => d,
                Ok(AxMsg::Text(t)) => t.into_bytes(),
                Ok(AxMsg::Close(_)) | Err(_) => break,
                Ok(_) => continue,
            };
            if d_tx.send(WsMessage::Binary(bytes)).await.is_err() {
                break;
            }
        }
    };
    let down = async {
        while let Some(msg) = d_rx.next().await {
            let bytes = match msg {
                Ok(WsMessage::Binary(d)) => d,
                Ok(WsMessage::Text(t)) => t.into_bytes(),
                Ok(WsMessage::Close(_)) | Err(_) => break,
                Ok(_) => continue,
            };
            // An empty message marks end-of-stream on exec sessions.
            if bytes.is_empty() {
                continue;
            }
            if b_tx.send(AxMsg::Binary(bytes)).await.is_err() {
                break;
            }
        }
        let _ = b_tx.close().await;
    };
    tokio::select! {
        _ = up => {}
        _ = down => {}
    }
    if hangup_exec {
        // The shell gets SIGHUP like a closed terminal; otherwise it would
        // keep running inside the container after the tab is closed.
        send_control(&mut control, json!({"command": "signal", "signal": 1})).await;
    }
    let _ = control.close(None).await;
}

/// Minimal HTTP/1.1 response parser (Content-Length, chunked, or EOF body).
fn parse_http_response(raw: &[u8]) -> Result<(u16, Vec<u8>)> {
    let head_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| sys("malformed HTTP response from the daemon"))?;
    let head = String::from_utf8_lossy(&raw[..head_end]);
    let mut lines = head.split("\r\n");
    let status: u16 = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| sys("malformed HTTP status line from the daemon"))?;
    let mut chunked = false;
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            if k.trim().eq_ignore_ascii_case("transfer-encoding")
                && v.trim().eq_ignore_ascii_case("chunked")
            {
                chunked = true;
            }
        }
    }
    let body = &raw[head_end + 4..];
    if !chunked {
        return Ok((status, body.to_vec()));
    }
    let mut out = Vec::new();
    let mut rest = body;
    while let Some(nl) = rest.windows(2).position(|w| w == b"\r\n") {
        let size_str = String::from_utf8_lossy(&rest[..nl]);
        let size = usize::from_str_radix(size_str.split(';').next().unwrap_or("").trim(), 16)
            .map_err(|_| sys("malformed chunked body from the daemon"))?;
        rest = &rest[nl + 2..];
        if size == 0 || rest.len() < size {
            out.extend_from_slice(&rest[..size.min(rest.len())]);
            break;
        }
        out.extend_from_slice(&rest[..size]);
        rest = rest.get(size + 2..).unwrap_or_default();
    }
    Ok((status, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_chunked_responses() {
        let (s, b) =
            parse_http_response(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}").unwrap();
        assert_eq!((s, b.as_slice()), (200, &b"{}"[..]));
        let (s, b) = parse_http_response(
            b"HTTP/1.1 202 Accepted\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nabcd\r\n3\r\nefg\r\n0\r\n\r\n",
        )
        .unwrap();
        assert_eq!((s, b.as_slice()), (202, &b"abcdefg"[..]));
        assert!(parse_http_response(b"garbage").is_err());
    }

    #[test]
    fn memory_limits_in_mb() {
        assert_eq!(parse_memory_mb("256MB"), 256);
        assert_eq!(parse_memory_mb("1GiB"), 1024);
        assert_eq!(parse_memory_mb("268435456"), 256);
        assert_eq!(parse_memory_mb("bogus"), 0);
    }

    #[test]
    fn socket_override_wins() {
        std::env::set_var("HORCRUX_INCUS_SOCKET", "/tmp/hx-test-incus.sock");
        assert_eq!(
            Backend::Incus.socket_path(),
            PathBuf::from("/tmp/hx-test-incus.sock")
        );
        std::env::remove_var("HORCRUX_INCUS_SOCKET");
        // Without an override some default is always returned.
        assert!(Backend::Lxd.socket_path().ends_with("unix.socket"));
    }
}

/// Image source for an LXD/Incus container from the form's image field:
/// `remote:alias` as given, `distro/release` from the public `images:`
/// remote, anything else (a local alias or fingerprint) as given; empty
/// means Ubuntu 22.04.
pub fn instance_image(rootfs: &str) -> String {
    let r = rootfs.trim();
    if r.is_empty() {
        "images:ubuntu/22.04".to_string()
    } else if r.contains(':') || !r.contains('/') || r.starts_with('/') {
        r.to_string()
    } else {
        format!("images:{r}")
    }
}

#[cfg(test)]
mod instance_image_tests {
    #[test]
    fn image_sources() {
        assert_eq!(super::instance_image("alpine/3.20"), "images:alpine/3.20");
        assert_eq!(
            super::instance_image("images:debian/12"),
            "images:debian/12"
        );
        assert_eq!(super::instance_image("my-local"), "my-local");
        assert_eq!(super::instance_image(" "), "images:ubuntu/22.04");
    }
}
