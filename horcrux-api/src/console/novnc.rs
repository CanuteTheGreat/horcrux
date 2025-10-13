///! noVNC WebSocket proxy implementation
///! Provides WebSocket proxy for VNC connections using Axum WebSocket support

use axum::{
    extract::{ws::{Message, WebSocket}, Path, State, WebSocketUpgrade},
    response::Response,
};
use futures::{SinkExt, StreamExt}; // For split(), send(), and recv() methods
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::{debug, error, info};

use super::ConsoleManager;

/// Handle WebSocket upgrade for VNC proxy
pub async fn handle_vnc_websocket(
    ws: WebSocketUpgrade,
    Path(ticket_id): Path<String>,
    State(console_manager): State<Arc<ConsoleManager>>,
) -> Response {
    info!("WebSocket upgrade request for ticket: {}", ticket_id);

    // Verify the ticket
    let ticket = match console_manager.verify_ticket(&ticket_id).await {
        Ok(ticket) => ticket,
        Err(e) => {
            error!("Invalid ticket {}: {}", ticket_id, e);
            return axum::response::Response::builder()
                .status(axum::http::StatusCode::UNAUTHORIZED)
                .body(axum::body::Body::from("Invalid or expired ticket"))
                .unwrap();
        }
    };

    info!(
        "Valid ticket for VM {} (port {})",
        ticket.vm_id, ticket.vnc_port
    );

    // Upgrade the WebSocket connection
    ws.on_upgrade(move |socket| handle_vnc_connection(socket, ticket.vnc_port))
}

/// Handle the WebSocket connection and proxy to VNC
async fn handle_vnc_connection(ws_socket: WebSocket, vnc_port: u16) {
    info!("Establishing VNC connection to port {}", vnc_port);

    // Connect to the VNC server
    let vnc_stream = match TcpStream::connect(format!("127.0.0.1:{}", vnc_port)).await {
        Ok(stream) => stream,
        Err(e) => {
            error!("Failed to connect to VNC server on port {}: {}", vnc_port, e);
            return;
        }
    };

    info!("Connected to VNC server on port {}", vnc_port);

    // Split the streams
    let (mut ws_sender, mut ws_receiver) = ws_socket.split();
    let (mut vnc_reader, mut vnc_writer) = vnc_stream.into_split();

    // Create tasks for bidirectional forwarding

    // Task 1: Forward WebSocket -> VNC
    let ws_to_vnc = tokio::spawn(async move {
        while let Some(msg) = ws_receiver.next().await {
            match msg {
                Ok(Message::Binary(data)) => {
                    // noVNC sends binary data for VNC protocol
                    if let Err(e) = vnc_writer.write_all(&data).await {
                        debug!("Error writing to VNC: {}", e);
                        break;
                    }
                }
                Ok(Message::Close(_)) => {
                    debug!("WebSocket closed by client");
                    break;
                }
                Ok(_) => {
                    // Ignore text and other message types
                }
                Err(e) => {
                    debug!("WebSocket error: {}", e);
                    break;
                }
            }
        }
        debug!("WebSocket -> VNC forwarding stopped");
    });

    // Task 2: Forward VNC -> WebSocket
    let vnc_to_ws = tokio::spawn(async move {
        let mut buffer = vec![0u8; 8192];
        loop {
            match vnc_reader.read(&mut buffer).await {
                Ok(0) => {
                    // Connection closed
                    debug!("VNC connection closed");
                    break;
                }
                Ok(n) => {
                    // Send VNC data to WebSocket as binary message
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
                    debug!("Error reading from VNC: {}", e);
                    break;
                }
            }
        }
        debug!("VNC -> WebSocket forwarding stopped");
    });

    // Wait for either direction to complete
    tokio::select! {
        _ = ws_to_vnc => {
            debug!("WebSocket to VNC task completed");
        }
        _ = vnc_to_ws => {
            debug!("VNC to WebSocket task completed");
        }
    }

    info!("VNC WebSocket proxy connection closed for port {}", vnc_port);
}

/// Get noVNC client HTML page
pub fn get_novnc_html(_ticket_id: &str, ws_url: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html>
<head>
    <title>Horcrux Console</title>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <style>
        body {{
            margin: 0;
            padding: 0;
            background-color: #000;
            overflow: hidden;
            font-family: Arial, sans-serif;
        }}
        #console {{
            width: 100vw;
            height: 100vh;
        }}
        #status {{
            position: absolute;
            top: 10px;
            right: 10px;
            padding: 10px;
            background: rgba(0, 0, 0, 0.7);
            color: #fff;
            border-radius: 5px;
            font-size: 14px;
        }}
        .connecting {{ color: #ff9800; }}
        .connected {{ color: #4caf50; }}
        .disconnected {{ color: #f44336; }}
    </style>
</head>
<body>
    <div id="status" class="connecting">Connecting...</div>
    <canvas id="console"></canvas>

    <script>
        // Simple VNC client implementation
        // In production, you'd use noVNC library: https://github.com/novnc/noVNC

        const canvas = document.getElementById('console');
        const ctx = canvas.getContext('2d');
        const status = document.getElementById('status');

        // Set canvas to full screen
        canvas.width = window.innerWidth;
        canvas.height = window.innerHeight;

        window.addEventListener('resize', () => {{
            canvas.width = window.innerWidth;
            canvas.height = window.innerHeight;
        }});

        // Connect to VNC WebSocket proxy
        const ws = new WebSocket('{ws_url}');
        ws.binaryType = 'arraybuffer';

        ws.onopen = () => {{
            status.textContent = 'Connected';
            status.className = 'connected';
            console.log('WebSocket connected to VNC server');

            // TODO: Implement RFB (Remote Framebuffer) protocol
            // For now, this is a placeholder
            // Full implementation would include:
            // 1. RFB handshake
            // 2. VNC authentication (if required)
            // 3. FramebufferUpdate handling
            // 4. Keyboard/mouse event encoding
        }};

        ws.onmessage = (event) => {{
            // Handle VNC protocol messages
            console.log('Received VNC data:', event.data.byteLength, 'bytes');
            // TODO: Parse and render VNC framebuffer updates
        }};

        ws.onerror = (error) => {{
            status.textContent = 'Connection Error';
            status.className = 'disconnected';
            console.error('WebSocket error:', error);
        }};

        ws.onclose = () => {{
            status.textContent = 'Disconnected';
            status.className = 'disconnected';
            console.log('WebSocket connection closed');
        }};

        // Handle keyboard input
        window.addEventListener('keydown', (e) => {{
            if (ws.readyState === WebSocket.OPEN) {{
                // TODO: Encode keyboard event to VNC protocol
                // e.preventDefault();
            }}
        }});

        // Handle mouse input
        canvas.addEventListener('mousemove', (e) => {{
            if (ws.readyState === WebSocket.OPEN) {{
                // TODO: Encode mouse move to VNC protocol
            }}
        }});

        canvas.addEventListener('mousedown', (e) => {{
            if (ws.readyState === WebSocket.OPEN) {{
                // TODO: Encode mouse button press to VNC protocol
            }}
        }});

        canvas.addEventListener('mouseup', (e) => {{
            if (ws.readyState === WebSocket.OPEN) {{
                // TODO: Encode mouse button release to VNC protocol
            }}
        }});
    </script>

    <!-- Production implementation would use noVNC library -->
    <!--
    <script src="https://cdn.jsdelivr.net/npm/@novnc/novnc@1.4.0/core/rfb.js"></script>
    <script>
        const rfb = new NoVNC.RFB(canvas, '{ws_url}');
        rfb.scaleViewport = true;
        rfb.resizeSession = true;
    </script>
    -->
</body>
</html>"#,
        ws_url = ws_url
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_novnc_html_generation() {
        let html = get_novnc_html("test-ticket", "ws://localhost:6080/test-ticket");
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("ws://localhost:6080/test-ticket"));
        assert!(html.contains("Horcrux Console"));
    }
}
