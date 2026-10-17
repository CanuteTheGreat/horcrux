//! WebSocket bridges for container logs, interactive exec, and the live
//! Docker/Podman events feed.
//!
//! Structured exactly like `console::serial_ws`: a ticket is verified on
//! WebSocket upgrade, then the connection is bridged to the real backend
//! (a bollard log/exec/events stream for Docker, or a piped CLI process for
//! Podman / Docker-without-API-access).
use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, State, WebSocketUpgrade,
    },
    response::Response,
};
use bollard::container::LogsOptions;
use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
use bollard::system::EventsOptions;
use bollard::Docker;
use futures::{SinkExt, StreamExt};
use horcrux_common::ContainerRuntime;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tracing::{debug, error, info};

use super::ws::{ContainerWsKind, ContainerWsManager};

fn unauthorized(msg: &str) -> Response {
    axum::response::Response::builder()
        .status(axum::http::StatusCode::UNAUTHORIZED)
        .body(axum::body::Body::from(msg.to_string()))
        .unwrap()
}

/// Shared entry point for all three container WebSocket endpoints -
/// `/api/containers/ws/logs/:ticket`, `/api/containers/ws/exec/:ticket`,
/// `/api/containers/ws/events/:ticket`. The route determines which `kind`
/// is expected; a ticket minted for a different kind is rejected.
async fn upgrade_for_kind(
    ws: WebSocketUpgrade,
    ticket_id: String,
    expected: ContainerWsKind,
    ws_manager: Arc<ContainerWsManager>,
) -> Response {
    let ticket = match ws_manager.verify_ticket(&ticket_id).await {
        Ok(t) => t,
        Err(e) => {
            error!("Invalid container ws ticket {}: {}", ticket_id, e);
            return unauthorized("Invalid or expired ticket");
        }
    };
    if ticket.kind != expected {
        return unauthorized("Ticket not valid for this endpoint");
    }

    match ticket.kind {
        ContainerWsKind::Logs => {
            ws.on_upgrade(move |socket| handle_logs(socket, ticket.container_id, ticket.runtime))
        }
        ContainerWsKind::Exec => ws.on_upgrade(move |socket| {
            handle_exec(
                socket,
                ticket.container_id,
                ticket.runtime,
                ticket.exec_command,
            )
        }),
        ContainerWsKind::Events => {
            ws.on_upgrade(move |socket| handle_events(socket, ticket.runtime))
        }
        ContainerWsKind::Console => {
            ws.on_upgrade(move |socket| handle_console(socket, ticket.container_id, ticket.runtime))
        }
    }
}

pub async fn handle_console_ws(
    ws: WebSocketUpgrade,
    Path(ticket_id): Path<String>,
    State(ws_manager): State<Arc<ContainerWsManager>>,
) -> Response {
    upgrade_for_kind(ws, ticket_id, ContainerWsKind::Console, ws_manager).await
}

// ---------------------------------------------------------------------
// LXD / Incus (instance API over the daemon's Unix socket)
// ---------------------------------------------------------------------

#[cfg(any(feature = "lxd", feature = "incus"))]
fn instance_backend(runtime: &ContainerRuntime) -> Option<crate::console::lxd::Backend> {
    match runtime {
        #[cfg(feature = "lxd")]
        ContainerRuntime::Lxd => Some(crate::console::lxd::Backend::Lxd),
        #[cfg(feature = "incus")]
        ContainerRuntime::Incus => Some(crate::console::lxd::Backend::Incus),
        _ => None,
    }
}

/// Open an LXD/Incus session (console or PTY exec) and bridge it.
#[cfg(any(feature = "lxd", feature = "incus"))]
async fn instance_session(
    mut ws_socket: WebSocket,
    backend: crate::console::lxd::Backend,
    name: String,
    exec: Option<Vec<String>>,
) {
    use crate::console::lxd::{bridge, Client, ConsoleKind};
    let client = Client::new(backend);
    let opened = match &exec {
        Some(cmd) => client.exec_pty(&name, cmd).await.map(|s| (Vec::new(), s)),
        None => {
            let history = client.console_log(&name).await.unwrap_or_default();
            client
                .open_console(&name, ConsoleKind::Text)
                .await
                .map(|s| (history, s))
        }
    };
    match opened {
        Ok((history, session)) => {
            bridge(ws_socket, session, history, exec.is_some()).await;
            info!(
                "{} {} session closed for {}",
                backend.label(),
                if exec.is_some() {
                    "terminal"
                } else {
                    "console"
                },
                name
            );
        }
        Err(e) => {
            error!("{} session for {}: {}", backend.label(), name, e);
            let _ = ws_socket
                .send(Message::Text(format!("[error] {e}\r\n")))
                .await;
        }
    }
}

/// LXD/Incus console log: what is there now, then whatever is appended.
#[cfg(any(feature = "lxd", feature = "incus"))]
async fn stream_instance_console_log(
    ws_sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    backend: crate::console::lxd::Backend,
    name: &str,
) {
    let client = crate::console::lxd::Client::new(backend);
    let mut sent = 0usize;
    loop {
        match client.console_log(name).await {
            Ok(log) => {
                if log.len() < sent {
                    sent = 0;
                }
                if log.len() > sent {
                    let text = String::from_utf8_lossy(&log[sent..]).to_string();
                    sent = log.len();
                    if ws_sender.send(Message::Text(text)).await.is_err() {
                        return;
                    }
                }
            }
            Err(e) => {
                let _ = ws_sender
                    .send(Message::Text(format!("[error] {e}\n")))
                    .await;
                return;
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        if ws_sender.send(Message::Ping(Vec::new())).await.is_err() {
            return;
        }
    }
}

// ---------------------------------------------------------------------
// System console (LXC / LXD / Incus)
// ---------------------------------------------------------------------

async fn handle_console(ws_socket: WebSocket, container_id: String, runtime: ContainerRuntime) {
    #[cfg(any(feature = "lxd", feature = "incus"))]
    if let Some(backend) = instance_backend(&runtime) {
        return instance_session(ws_socket, backend, container_id, None).await;
    }
    #[cfg(feature = "lxc")]
    if runtime == ContainerRuntime::Lxc {
        let mgr = super::lxc::LxcManager::new();
        let name = mgr.resolve_name(&container_id);
        let argv = vec![
            "lxc-console".to_string(),
            "-P".into(),
            mgr.lxc_path().to_string_lossy().to_string(),
            "-n".into(),
            name.clone(),
            "-t".into(),
            "0".into(),
        ];
        // Recent console output first (the console log), then the live tty.
        let history = match tokio::fs::read(mgr.console_log_path(&name)).await {
            Ok(b) => b[b.len().saturating_sub(16 * 1024)..].to_vec(),
            Err(_) => Vec::new(),
        };
        return run_on_pty(ws_socket, argv, name, history).await;
    }
    let (mut tx, _) = ws_socket.split();
    let _ = tx
        .send(Message::Text(format!(
            "[error] {runtime:?} containers have no system console; open the Terminal or Logs instead\r\n"
        )))
        .await;
}

pub async fn handle_logs_ws(
    ws: WebSocketUpgrade,
    Path(ticket_id): Path<String>,
    State(ws_manager): State<Arc<ContainerWsManager>>,
) -> Response {
    upgrade_for_kind(ws, ticket_id, ContainerWsKind::Logs, ws_manager).await
}

pub async fn handle_exec_ws(
    ws: WebSocketUpgrade,
    Path(ticket_id): Path<String>,
    State(ws_manager): State<Arc<ContainerWsManager>>,
) -> Response {
    upgrade_for_kind(ws, ticket_id, ContainerWsKind::Exec, ws_manager).await
}

pub async fn handle_events_ws(
    ws: WebSocketUpgrade,
    Path(ticket_id): Path<String>,
    State(ws_manager): State<Arc<ContainerWsManager>>,
) -> Response {
    upgrade_for_kind(ws, ticket_id, ContainerWsKind::Events, ws_manager).await
}

// ---------------------------------------------------------------------
// Live logs
// ---------------------------------------------------------------------

async fn handle_logs(ws_socket: WebSocket, container_id: String, runtime: ContainerRuntime) {
    let (mut ws_sender, mut ws_receiver) = ws_socket.split();

    // Drain (and ignore) client->server frames so the socket closes cleanly
    // when the browser tab navigates away.
    tokio::spawn(async move { while ws_receiver.next().await.is_some() {} });

    #[cfg(any(feature = "lxd", feature = "incus"))]
    if let Some(backend) = instance_backend(&runtime) {
        stream_instance_console_log(&mut ws_sender, backend, &container_id).await;
        return;
    }
    match runtime {
        #[cfg(feature = "lxc")]
        ContainerRuntime::Lxc => {
            stream_lxc_console_log(&mut ws_sender, &container_id).await;
        }
        ContainerRuntime::Docker => {
            let docker = match Docker::connect_with_local_defaults() {
                Ok(d) => d,
                Err(e) => {
                    let _ = ws_sender
                        .send(Message::Text(format!("[error] docker connect: {e}\n")))
                        .await;
                    return;
                }
            };
            let options = Some(LogsOptions::<String> {
                follow: true,
                stdout: true,
                stderr: true,
                tail: "100".to_string(),
                timestamps: true,
                ..Default::default()
            });
            let mut stream = docker.logs(&container_id, options);
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(log) => {
                        if ws_sender
                            .send(Message::Text(log.to_string()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = ws_sender
                            .send(Message::Text(format!("[error] {e}\n")))
                            .await;
                        break;
                    }
                }
            }
        }
        _ => {
            // Podman (and Docker-without-API-access fallback): stream
            // `podman logs -f` / `docker logs -f` stdout line by line. This
            // is one-way only (no stdin), which is the documented CLI-path
            // limitation versus the bollard-attached Docker stream above.
            let bin = if runtime == ContainerRuntime::Podman {
                "podman"
            } else {
                "docker"
            };
            let mut child = match Command::new(bin)
                .args(["logs", "-f", "--tail", "100", &container_id])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    let _ = ws_sender
                        .send(Message::Text(format!("[error] spawn {bin} logs: {e}\n")))
                        .await;
                    return;
                }
            };
            let mut stdout = child.stdout.take().unwrap();
            let mut buf = vec![0u8; 4096];
            loop {
                match stdout.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let text = String::from_utf8_lossy(&buf[..n]).to_string();
                        if ws_sender.send(Message::Text(text)).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = child.kill().await;
        }
    }
    debug!("Container logs WebSocket closed for {}", container_id);
}

// ---------------------------------------------------------------------
// Interactive exec (the Portainer-defining feature: a real web terminal)
// ---------------------------------------------------------------------

async fn handle_exec(
    ws_socket: WebSocket,
    container_id: String,
    runtime: ContainerRuntime,
    exec_command: Vec<String>,
) {
    let cmd = if exec_command.is_empty() {
        vec!["/bin/sh".to_string()]
    } else {
        exec_command
    };

    #[cfg(any(feature = "lxd", feature = "incus"))]
    if let Some(backend) = instance_backend(&runtime) {
        return instance_session(ws_socket, backend, container_id, Some(cmd)).await;
    }
    match runtime {
        ContainerRuntime::Docker => handle_exec_docker(ws_socket, container_id, cmd).await,
        #[cfg(feature = "lxc")]
        ContainerRuntime::Lxc => handle_exec_lxc(ws_socket, container_id, cmd).await,
        _ => handle_exec_cli(ws_socket, container_id, cmd, runtime).await,
    }
}

/// LXC console log (`lxc.console.logfile`): the last 64 KiB, then follow.
#[cfg(feature = "lxc")]
async fn stream_lxc_console_log(
    ws_sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    container_id: &str,
) {
    use tokio::io::AsyncSeekExt;
    let mgr = super::lxc::LxcManager::new();
    let name = mgr.resolve_name(container_id);
    let path = mgr.console_log_path(&name);
    let mut pos: u64 = match tokio::fs::metadata(&path).await {
        Ok(m) => m.len().saturating_sub(64 * 1024),
        Err(_) => {
            let _ = ws_sender
                .send(Message::Text(format!(
                    "[no console output yet for {name}: {} does not exist]\n",
                    path.display()
                )))
                .await;
            0
        }
    };
    loop {
        if let Ok(mut f) = tokio::fs::File::open(&path).await {
            let len = f.metadata().await.map(|m| m.len()).unwrap_or(0);
            if len < pos {
                // Rotated/truncated (lxc.console.size): start over.
                pos = 0;
            }
            if len > pos && f.seek(std::io::SeekFrom::Start(pos)).await.is_ok() {
                let mut buf = Vec::new();
                if f.read_to_end(&mut buf).await.is_ok() {
                    pos += buf.len() as u64;
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if ws_sender.send(Message::Text(text)).await.is_err() {
                        return;
                    }
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        // A closed socket fails the ping; stop following.
        if ws_sender.send(Message::Ping(Vec::new())).await.is_err() {
            return;
        }
    }
}

/// Interactive shell in an LXC container (`pct enter`): `lxc-attach` on a
/// real PTY, so job control, prompts and curses programs work.
#[cfg(feature = "lxc")]
async fn handle_exec_lxc(ws_socket: WebSocket, container_id: String, cmd: Vec<String>) {
    let mgr = super::lxc::LxcManager::new();
    let name = mgr.resolve_name(&container_id);
    let argv = mgr.attach_argv(&name, &cmd);
    run_on_pty(ws_socket, argv, name, Vec::new()).await
}

/// Run `argv` (lxc-attach / lxc-console) on a fresh PTY bridged to the
/// browser; `history` is sent first.
#[cfg(feature = "lxc")]
async fn run_on_pty(ws_socket: WebSocket, argv: Vec<String>, name: String, history: Vec<u8>) {
    let (mut ws_sender, mut ws_receiver) = ws_socket.split();
    if !history.is_empty() && ws_sender.send(Message::Binary(history)).await.is_err() {
        return;
    }

    let pty = match nix::pty::openpty(
        Some(&nix::pty::Winsize {
            ws_row: 40,
            ws_col: 120,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }),
        None,
    ) {
        Ok(p) => p,
        Err(e) => {
            let _ = ws_sender
                .send(Message::Text(format!("[error] openpty: {e}\r\n")))
                .await;
            return;
        }
    };
    let slave = std::fs::File::from(pty.slave);
    let stdio = |f: &std::fs::File| f.try_clone().map(std::process::Stdio::from);
    let (sin, sout, serr) = match (stdio(&slave), stdio(&slave), stdio(&slave)) {
        (Ok(a), Ok(b), Ok(c)) => (a, b, c),
        _ => {
            let _ = ws_sender
                .send(Message::Text("[error] pty setup failed\r\n".into()))
                .await;
            return;
        }
    };
    let mut command = std::process::Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .stdin(sin)
        .stdout(sout)
        .stderr(serr);
    // New session with the PTY (stdin) as controlling terminal.
    // SAFETY: only async-signal-safe calls (setsid, ioctl) between fork and exec.
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| {
            nix::unistd::setsid().map_err(std::io::Error::from)?;
            if nix::libc::ioctl(0, nix::libc::TIOCSCTTY as _, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = match tokio::process::Command::from(command).spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = ws_sender
                .send(Message::Text(format!("[error] {}: {e}\r\n", argv[0])))
                .await;
            return;
        }
    };
    drop(slave);
    let master = std::fs::File::from(pty.master);
    let master_w = match master.try_clone() {
        Ok(m) => tokio::fs::File::from_std(m),
        Err(_) => return,
    };
    // Third handle: answers terminal queries the plain-text web terminal
    // can't (busybox/readline ask for the cursor position with ESC[6n and
    // stall line editing until they get a reply).
    let mut master_q = match master.try_clone() {
        Ok(m) => tokio::fs::File::from_std(m),
        Err(_) => return,
    };
    // Kept to find the shell's process group when the session ends.
    let master_ctl = match master.try_clone() {
        Ok(m) => m,
        Err(_) => return,
    };
    let mut master_r = tokio::fs::File::from_std(master);

    let stdin_task = tokio::spawn(async move {
        let mut w = master_w;
        while let Some(msg) = ws_receiver.next().await {
            let data = match msg {
                Ok(Message::Binary(d)) => d,
                Ok(Message::Text(t)) => t.into_bytes(),
                Ok(Message::Close(_)) | Err(_) => break,
                Ok(_) => continue,
            };
            if w.write_all(&data).await.is_err() || w.flush().await.is_err() {
                break;
            }
        }
    });
    let stdout_task = tokio::spawn(async move {
        let mut buf = vec![0u8; 4096];
        loop {
            match master_r.read(&mut buf).await {
                // EIO once the shell exits and the slave side closes.
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let chunk = &buf[..n];
                    if chunk.windows(4).any(|w| w == b"\x1b[6n") {
                        let _ = master_q.write_all(b"\x1b[1;1R").await;
                        let _ = master_q.flush().await;
                    }
                    if ws_sender
                        .send(Message::Binary(chunk.to_vec()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
        let _ = ws_sender.close().await;
    });
    let (stdin_abort, stdout_abort) = (stdin_task.abort_handle(), stdout_task.abort_handle());
    tokio::select! {
        _ = stdin_task => {}
        _ = stdout_task => {}
        _ = child.wait() => {}
    }
    // Hang up the shell (as closing a terminal does) so it exits and
    // lxc-attach, its parent, reaps it. SIGKILLing lxc-attach first would
    // orphan the shell, and an orphan nobody reaps keeps the container's
    // pid namespace alive (lxc-stop then never completes).
    stdin_abort.abort();
    stdout_abort.abort();
    {
        use std::os::fd::AsRawFd;
        let mut pgid: nix::libc::pid_t = 0;
        // SAFETY: valid fd; TIOCGPGRP writes one pid_t.
        let rc = unsafe {
            nix::libc::ioctl(master_ctl.as_raw_fd(), nix::libc::TIOCGPGRP as _, &mut pgid)
        };
        if rc == 0 && pgid > 1 {
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(pgid),
                nix::sys::signal::Signal::SIGHUP,
            );
        }
    }
    drop(master_ctl);
    if tokio::time::timeout(std::time::Duration::from_secs(5), child.wait())
        .await
        .is_err()
    {
        let _ = child.kill().await;
    }
    info!("LXC terminal session closed for {}", name);
}

async fn handle_exec_docker(ws_socket: WebSocket, container_id: String, cmd: Vec<String>) {
    let docker = match Docker::connect_with_local_defaults() {
        Ok(d) => d,
        Err(e) => {
            error!("Exec: docker connect failed: {}", e);
            return;
        }
    };

    let create = docker
        .create_exec(
            &container_id,
            CreateExecOptions {
                attach_stdin: Some(true),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                tty: Some(true),
                cmd: Some(cmd),
                ..Default::default()
            },
        )
        .await;
    let exec_id = match create {
        Ok(c) => c.id,
        Err(e) => {
            error!("docker create_exec failed: {}", e);
            return;
        }
    };

    let started = docker
        .start_exec(
            &exec_id,
            Some(StartExecOptions {
                tty: true,
                ..Default::default()
            }),
        )
        .await;

    let (mut output, mut input) = match started {
        Ok(StartExecResults::Attached { output, input }) => (output, input),
        Ok(StartExecResults::Detached) => {
            error!("docker start_exec returned Detached unexpectedly");
            return;
        }
        Err(e) => {
            error!("docker start_exec failed: {}", e);
            return;
        }
    };

    let (mut ws_sender, mut ws_receiver) = ws_socket.split();

    let stdin_task = tokio::spawn(async move {
        while let Some(msg) = ws_receiver.next().await {
            match msg {
                Ok(Message::Binary(data)) => {
                    if input.write_all(&data).await.is_err() {
                        break;
                    }
                }
                Ok(Message::Text(text)) => {
                    if input.write_all(text.as_bytes()).await.is_err() {
                        break;
                    }
                }
                Ok(Message::Close(_)) => break,
                Ok(_) => {}
                Err(_) => break,
            }
        }
    });

    let stdout_task = tokio::spawn(async move {
        while let Some(chunk) = output.next().await {
            match chunk {
                Ok(log) => {
                    let bytes = log.into_bytes();
                    if ws_sender
                        .send(Message::Binary(bytes.to_vec()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(e) => {
                    debug!("exec output stream ended: {}", e);
                    break;
                }
            }
        }
    });

    tokio::select! {
        _ = stdin_task => {}
        _ = stdout_task => {}
    }
    info!(
        "Interactive exec session closed for container {}",
        container_id
    );
}

/// Podman / CLI-fallback interactive exec. This uses `podman exec -i`
/// (stdin attached) over plain pipes rather than a real PTY - it is
/// functional for running commands and seeing their output, but does not
/// support terminal resize, job control signals, or raw-mode line editing
/// the way Docker's bollard-attached TTY exec above does. That gap is
/// intentional and documented in PORTAINER_COMPARISON.md rather than
/// silently pretending to be a full PTY.
async fn handle_exec_cli(
    ws_socket: WebSocket,
    container_id: String,
    cmd: Vec<String>,
    runtime: ContainerRuntime,
) {
    let bin = if runtime == ContainerRuntime::Podman {
        "podman"
    } else {
        "docker"
    };
    let mut command = Command::new(bin);
    command
        .arg("exec")
        .arg("-i")
        .arg(&container_id)
        .args(&cmd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to spawn {} exec: {}", bin, e);
            return;
        }
    };
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();

    let (mut ws_sender, mut ws_receiver) = ws_socket.split();

    let stdin_task = tokio::spawn(async move {
        while let Some(msg) = ws_receiver.next().await {
            match msg {
                Ok(Message::Binary(data)) => {
                    if stdin.write_all(&data).await.is_err() {
                        break;
                    }
                }
                Ok(Message::Text(text)) => {
                    if stdin.write_all(text.as_bytes()).await.is_err() {
                        break;
                    }
                }
                Ok(Message::Close(_)) => break,
                Ok(_) => {}
                Err(_) => break,
            }
        }
    });

    let stdout_task = tokio::spawn(async move {
        let mut buf = vec![0u8; 4096];
        loop {
            match stdout.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if ws_sender
                        .send(Message::Binary(buf[..n].to_vec()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    tokio::select! {
        _ = stdin_task => {}
        _ = stdout_task => {}
    }
    let _ = child.kill().await;
    info!(
        "CLI exec session closed for container {} ({})",
        container_id, bin
    );
}

// ---------------------------------------------------------------------
// Live events feed
// ---------------------------------------------------------------------

async fn handle_events(ws_socket: WebSocket, runtime: ContainerRuntime) {
    let (mut ws_sender, mut ws_receiver) = ws_socket.split();
    tokio::spawn(async move { while ws_receiver.next().await.is_some() {} });

    match runtime {
        ContainerRuntime::Docker => {
            let docker = match Docker::connect_with_local_defaults() {
                Ok(d) => d,
                Err(e) => {
                    let _ = ws_sender
                        .send(Message::Text(format!("[error] docker connect: {e}\n")))
                        .await;
                    return;
                }
            };
            let mut stream = docker.events(Some(EventsOptions::<String>::default()));
            while let Some(evt) = stream.next().await {
                match evt {
                    Ok(e) => {
                        let json = serde_json::to_string(&e).unwrap_or_default();
                        if ws_sender.send(Message::Text(json)).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = ws_sender
                            .send(Message::Text(format!("[error] {e}\n")))
                            .await;
                        break;
                    }
                }
            }
        }
        _ => {
            // Podman: `podman events --format json --stream` emits one JSON
            // object per line, forwarded verbatim as text frames.
            let mut child = match Command::new("podman")
                .args(["events", "--format", "json", "--stream"])
                .stdout(std::process::Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    let _ = ws_sender
                        .send(Message::Text(format!("[error] spawn podman events: {e}\n")))
                        .await;
                    return;
                }
            };
            let mut stdout = child.stdout.take().unwrap();
            let mut buf = vec![0u8; 4096];
            loop {
                match stdout.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let text = String::from_utf8_lossy(&buf[..n]).to_string();
                        if ws_sender.send(Message::Text(text)).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = child.kill().await;
        }
    }
}
