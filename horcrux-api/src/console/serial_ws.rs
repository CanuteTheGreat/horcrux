//! WebSocket bridge for VM serial consoles
//!
//! Mirrors the noVNC WebSocket proxy (`console::novnc`), but bridges to the
//! Unix domain socket QEMU exposes for `-serial unix:...,server,nowait`
//! instead of a TCP VNC port.
use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, State, WebSocketUpgrade,
    },
    response::Response,
};
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tracing::{debug, error, info};

use super::ConsoleManager;

/// Handle WebSocket upgrade for a serial console proxy
pub async fn handle_serial_websocket(
    ws: WebSocketUpgrade,
    Path(ticket_id): Path<String>,
    State(console_manager): State<Arc<ConsoleManager>>,
) -> Response {
    info!("Serial WebSocket upgrade request for ticket: {}", ticket_id);

    // Verify the ticket
    let ticket = match console_manager.verify_ticket(&ticket_id).await {
        Ok(ticket) => ticket,
        Err(e) => {
            error!("Invalid serial ticket {}: {}", ticket_id, e);
            return axum::response::Response::builder()
                .status(axum::http::StatusCode::UNAUTHORIZED)
                .body(axum::body::Body::from("Invalid or expired ticket"))
                .unwrap();
        }
    };

    // Resolve the Unix socket path backing this VM's serial console
    let socket_path = match console_manager.get_serial_socket_path(&ticket.vm_id).await {
        Ok(path) => path,
        Err(e) => {
            error!(
                "Serial console not available for VM {}: {}",
                ticket.vm_id, e
            );
            return axum::response::Response::builder()
                .status(axum::http::StatusCode::NOT_FOUND)
                .body(axum::body::Body::from("Serial console not available"))
                .unwrap();
        }
    };

    info!(
        "Valid serial ticket for VM {} (socket {})",
        ticket.vm_id, socket_path
    );

    ws.on_upgrade(move |socket| handle_serial_connection(socket, socket_path))
}

/// Handle the WebSocket connection and proxy to the VM's serial Unix socket
async fn handle_serial_connection(ws_socket: WebSocket, socket_path: String) {
    info!("Establishing serial connection to {}", socket_path);

    let unix_stream = match UnixStream::connect(&socket_path).await {
        Ok(stream) => stream,
        Err(e) => {
            error!("Failed to connect to serial socket {}: {}", socket_path, e);
            return;
        }
    };

    info!("Connected to serial socket at {}", socket_path);

    let (mut ws_sender, mut ws_receiver) = ws_socket.split();
    let (mut unix_reader, mut unix_writer) = unix_stream.into_split();

    // Task 1: Forward WebSocket -> serial socket
    let ws_to_serial = tokio::spawn(async move {
        while let Some(msg) = ws_receiver.next().await {
            match msg {
                Ok(Message::Binary(data)) => {
                    if let Err(e) = unix_writer.write_all(&data).await {
                        debug!("Error writing to serial socket: {}", e);
                        break;
                    }
                }
                Ok(Message::Text(text)) => {
                    // Allow plain-text terminal input too (xterm.js style clients)
                    if let Err(e) = unix_writer.write_all(text.as_bytes()).await {
                        debug!("Error writing to serial socket: {}", e);
                        break;
                    }
                }
                Ok(Message::Close(_)) => {
                    debug!("WebSocket closed by client");
                    break;
                }
                Ok(_) => {}
                Err(e) => {
                    debug!("WebSocket error: {}", e);
                    break;
                }
            }
        }
        debug!("WebSocket -> serial forwarding stopped");
    });

    // Task 2: Forward serial socket -> WebSocket
    let serial_to_ws = tokio::spawn(async move {
        let mut buffer = vec![0u8; 8192];
        loop {
            match unix_reader.read(&mut buffer).await {
                Ok(0) => {
                    debug!("Serial socket closed");
                    break;
                }
                Ok(n) => {
                    if ws_sender
                        .send(Message::Binary(buffer[..n].to_vec()))
                        .await
                        .is_err()
                    {
                        debug!("Error sending to WebSocket");
                        break;
                    }
                }
                Err(e) => {
                    debug!("Error reading from serial socket: {}", e);
                    break;
                }
            }
        }
        debug!("Serial -> WebSocket forwarding stopped");
    });

    tokio::select! {
        _ = ws_to_serial => {
            debug!("WebSocket to serial task completed");
        }
        _ = serial_to_ws => {
            debug!("Serial to WebSocket task completed");
        }
    }

    info!(
        "Serial WebSocket proxy connection closed for {}",
        socket_path
    );
}
