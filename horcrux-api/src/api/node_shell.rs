//! Node shell (Proxmox "Node > Shell"): a real login shell on this host in a
//! PTY, bridged to the browser over a WebSocket.
//!
//! * `POST /api/nodes/shell/ticket` mints a single-use ticket (60 s) for the
//!   calling user; needs `Sys.Modify` (it is a root shell)
//! * `GET  /api/nodes/shell/ws/:ticket` upgrades to the shell session. Binary
//!   frames are the terminal bytes in both directions; a text frame
//!   `{"resize":[cols,rows]}` sets the window size
//!
//! The shell runs as the API's own user with a clean environment (the API's
//! secrets never reach it), is killed when the socket closes or after 30
//! minutes without input, and every session is recorded in the task log with
//! who opened it and for how long.
use crate::api::tasks;
use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::AppState;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path,
    },
    response::Response,
    routing::{get, post},
    Extension, Json, Router,
};
use nix::pty::{openpty, Winsize};
use serde::Serialize;
use std::collections::HashMap;
use std::os::fd::AsRawFd;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const TICKET_TTL: Duration = Duration::from_secs(60);
const IDLE: Duration = Duration::from_secs(30 * 60);

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/nodes/shell/ticket", post(mint))
        .route("/api/nodes/shell/ws/:ticket", get(upgrade))
}

type Tickets = Mutex<HashMap<String, (String, Instant)>>;

fn tickets() -> &'static Tickets {
    static T: OnceLock<Tickets> = OnceLock::new();
    T.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Serialize)]
struct TicketResponse {
    ticket: String,
    ws_path: String,
}

async fn mint(user: Option<Extension<AuthUser>>) -> Result<Json<TicketResponse>, ApiError> {
    let who = user
        .map(|u| u.0.username.clone())
        .ok_or(ApiError::AuthenticationFailed)?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    let mut t = tickets().lock().unwrap();
    t.retain(|_, (_, at)| at.elapsed() < TICKET_TTL);
    t.insert(id.clone(), (who, Instant::now()));
    Ok(Json(TicketResponse {
        ws_path: format!("/api/nodes/shell/ws/{id}"),
        ticket: id,
    }))
}

/// Redeem a ticket for `who`; it works once.
fn redeem(id: &str, who: &str) -> bool {
    let mut t = tickets().lock().unwrap();
    match t.remove(id) {
        Some((owner, at)) => owner == who && at.elapsed() < TICKET_TTL,
        None => false,
    }
}

async fn upgrade(
    ws: WebSocketUpgrade,
    user: Option<Extension<AuthUser>>,
    Path(ticket): Path<String>,
) -> Result<Response, ApiError> {
    let who = user
        .map(|u| u.0.username.clone())
        .ok_or(ApiError::AuthenticationFailed)?;
    if !redeem(&ticket, &who) {
        return Err(ApiError::NotFound(
            "shell ticket is unknown, expired, already used, or belongs to another user".into(),
        ));
    }
    Ok(ws.on_upgrade(move |s| session(s, who)))
}

pub fn shell_program() -> String {
    if let Ok(s) = std::env::var("HORCRUX_NODE_SHELL") {
        if std::path::Path::new(&s).is_file() {
            return s;
        }
    }
    for p in ["/bin/bash", "/usr/bin/bash", "/bin/sh"] {
        if std::path::Path::new(p).is_file() {
            return p.into();
        }
    }
    "/bin/sh".into()
}

/// `{"resize":[cols,rows]}` -> (cols, rows)
pub fn parse_resize(text: &str) -> Option<(u16, u16)> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let a = v.get("resize")?.as_array()?;
    let c = u16::try_from(a.first()?.as_u64()?).ok()?;
    let r = u16::try_from(a.get(1)?.as_u64()?).ok()?;
    (c > 0 && r > 0 && c <= 1000 && r <= 1000).then_some((c, r))
}

nix::ioctl_write_ptr_bad!(tiocswinsz, nix::libc::TIOCSWINSZ, nix::libc::winsize);
nix::ioctl_write_int_bad!(tiocsctty, nix::libc::TIOCSCTTY);

fn set_winsize(fd: i32, cols: u16, rows: u16) {
    let ws = nix::libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: fd is the PTY master we own for the session's lifetime.
    unsafe {
        let _ = tiocswinsz(fd, &ws);
    }
}

async fn session(mut socket: WebSocket, who: String) {
    let started = Instant::now();
    let shell = shell_program();
    let pty = match openpty(
        Some(&Winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }),
        None,
    ) {
        Ok(p) => p,
        Err(e) => {
            let _ = socket
                .send(Message::Text(format!("[cannot open a pty: {e}]\r\n")))
                .await;
            return;
        }
    };
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    let user = std::env::var("USER").unwrap_or_else(|_| "root".into());
    let mut cmd = tokio::process::Command::new(&shell);
    cmd.arg("-l")
        .env_clear()
        .env("TERM", "xterm-256color")
        .env("HOME", &home)
        .env("USER", &user)
        .env("LOGNAME", &user)
        .env("SHELL", &shell)
        .env(
            "LANG",
            std::env::var("LANG").unwrap_or_else(|_| "C.UTF-8".into()),
        )
        .env(
            "PATH",
            "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        )
        .current_dir(if std::path::Path::new(&home).is_dir() {
            home.as_str()
        } else {
            "/"
        })
        .stdin(std::process::Stdio::from(pty.slave.try_clone().unwrap()))
        .stdout(std::process::Stdio::from(pty.slave.try_clone().unwrap()))
        .stderr(std::process::Stdio::from(pty.slave))
        .kill_on_drop(true);
    // SAFETY: only async-signal-safe calls (setsid, ioctl) between fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            nix::unistd::setsid().map_err(std::io::Error::from)?;
            tiocsctty(0, 0).map_err(std::io::Error::from)?;
            Ok(())
        });
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = socket
                .send(Message::Text(format!("[cannot start {shell}: {e}]\r\n")))
                .await;
            return;
        }
    };
    let master_fd = pty.master.as_raw_fd();
    let master = std::fs::File::from(pty.master);
    let mut writer = tokio::fs::File::from_std(master.try_clone().unwrap());
    let mut reader = tokio::fs::File::from_std(master);

    let mut buf = vec![0u8; 8192];
    let mut bytes_in = 0usize;
    let mut last_input = Instant::now();
    let mut reason = "closed by the browser".to_string();
    loop {
        tokio::select! {
            n = reader.read(&mut buf) => {
                match n {
                    Ok(0) | Err(_) => { reason = "the shell exited".into(); break; }
                    Ok(n) => {
                        if socket.send(Message::Binary(buf[..n].to_vec())).await.is_err() {
                            break;
                        }
                    }
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(t))) => {
                        last_input = Instant::now();
                        if let Some((c, r)) = parse_resize(&t) {
                            set_winsize(master_fd, c, r);
                        } else {
                            bytes_in += t.len();
                            if writer.write_all(t.as_bytes()).await.is_err() { break; }
                        }
                    }
                    Some(Ok(Message::Binary(b))) => {
                        last_input = Instant::now();
                        bytes_in += b.len();
                        if writer.write_all(&b).await.is_err() { break; }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    Some(Ok(_)) => {}
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(30)) => {
                if last_input.elapsed() > IDLE {
                    reason = "idle for 30 minutes".into();
                    let _ = socket.send(Message::Text("\r\n[session closed: idle]\r\n".into())).await;
                    break;
                }
            }
        }
    }
    // SIGHUP the shell's session like a closed terminal would, then make sure.
    if let Some(pid) = child.id() {
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(pid as i32),
            nix::sys::signal::Signal::SIGHUP,
        );
    }
    tokio::select! {
        _ = child.wait() => {}
        _ = tokio::time::sleep(Duration::from_secs(3)) => { let _ = child.kill().await; }
    }
    tasks::record_done(
        "node-shell",
        &who,
        &who,
        &Ok(format!(
            "shell session for {who} lasted {} s, {bytes_in} bytes typed; {reason}",
            started.elapsed().as_secs()
        )),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_messages_parse_and_are_bounded() {
        assert_eq!(parse_resize(r#"{"resize":[120,40]}"#), Some((120, 40)));
        assert_eq!(parse_resize(r#"{"resize":[0,40]}"#), None);
        assert_eq!(parse_resize(r#"{"resize":[99999,40]}"#), None);
        assert_eq!(parse_resize("ls -l\r"), None);
        assert_eq!(parse_resize(r#"{"other":1}"#), None);
    }

    #[test]
    fn a_ticket_works_once_and_only_for_its_user() {
        let id = "t1".to_string();
        tickets()
            .lock()
            .unwrap()
            .insert(id.clone(), ("alice".into(), Instant::now()));
        assert!(
            !redeem(&id, "bob"),
            "another user can't use it (and it is burnt)"
        );
        assert!(!redeem(&id, "alice"));
        tickets()
            .lock()
            .unwrap()
            .insert("t2".into(), ("alice".into(), Instant::now()));
        assert!(redeem("t2", "alice"));
        assert!(!redeem("t2", "alice"), "single use");
        tickets().lock().unwrap().insert(
            "t3".into(),
            ("alice".into(), Instant::now() - Duration::from_secs(61)),
        );
        assert!(!redeem("t3", "alice"), "expired");
    }

    #[tokio::test]
    async fn a_real_pty_shell_runs_commands_and_takes_a_resize() {
        // The bridge's moving parts: openpty + setsid/TIOCSCTTY + resize.
        let pty = openpty(None, None).unwrap();
        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.arg("-c")
            .arg("stty size; echo pty-ok")
            .stdin(std::process::Stdio::from(pty.slave.try_clone().unwrap()))
            .stdout(std::process::Stdio::from(pty.slave.try_clone().unwrap()))
            .stderr(std::process::Stdio::from(pty.slave));
        let fd = pty.master.as_raw_fd();
        set_winsize(fd, 100, 30);
        unsafe {
            cmd.pre_exec(|| {
                nix::unistd::setsid().map_err(std::io::Error::from)?;
                tiocsctty(0, 0).map_err(std::io::Error::from)?;
                Ok(())
            });
        }
        let mut child = cmd.spawn().unwrap();
        let mut r = tokio::fs::File::from_std(std::fs::File::from(pty.master));
        let mut out = Vec::new();
        let mut b = [0u8; 256];
        while let Ok(n) = r.read(&mut b).await {
            if n == 0 {
                break;
            }
            out.extend_from_slice(&b[..n]);
            if String::from_utf8_lossy(&out).contains("pty-ok") {
                break;
            }
        }
        let _ = child.wait().await;
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("30 100") && s.contains("pty-ok"), "got {s:?}");
    }
}
