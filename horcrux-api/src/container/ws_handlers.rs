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
            handle_exec(socket, ticket.container_id, ticket.runtime, ticket.exec_command)
        }),
        ContainerWsKind::Events => {
            ws.on_upgrade(move |socket| handle_events(socket, ticket.runtime))
        }
    }
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

    match runtime {
        ContainerRuntime::Docker => handle_exec_docker(ws_socket, container_id, cmd).await,
        _ => handle_exec_cli(ws_socket, container_id, cmd, runtime).await,
    }
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
                    if ws_sender.send(Message::Binary(bytes.to_vec())).await.is_err() {
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
    info!("Interactive exec session closed for container {}", container_id);
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
