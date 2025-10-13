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
            background-color: #1a1a1a;
            font-family: 'Segoe UI', Tahoma, Geneva, Verdana, sans-serif;
            overflow: hidden;
        }}

        #screen {{
            width: 100vw;
            height: 100vh;
            display: flex;
            align-items: center;
            justify-content: center;
        }}

        #status {{
            position: absolute;
            top: 15px;
            right: 15px;
            padding: 12px 20px;
            background: rgba(0, 0, 0, 0.85);
            color: #fff;
            border-radius: 8px;
            font-size: 14px;
            font-weight: 500;
            box-shadow: 0 4px 12px rgba(0, 0, 0, 0.3);
            z-index: 1000;
            transition: all 0.3s ease;
        }}

        .connecting {{
            color: #ff9800;
            border-left: 3px solid #ff9800;
        }}
        .connected {{
            color: #4caf50;
            border-left: 3px solid #4caf50;
        }}
        .disconnected {{
            color: #f44336;
            border-left: 3px solid #f44336;
        }}

        #controls {{
            position: absolute;
            top: 15px;
            left: 15px;
            display: flex;
            gap: 10px;
            z-index: 1000;
        }}

        .control-btn {{
            padding: 8px 16px;
            background: rgba(0, 0, 0, 0.85);
            color: #fff;
            border: 1px solid #444;
            border-radius: 6px;
            cursor: pointer;
            font-size: 13px;
            transition: all 0.2s ease;
        }}

        .control-btn:hover {{
            background: rgba(30, 30, 30, 0.95);
            border-color: #666;
        }}

        .control-btn:active {{
            transform: scale(0.95);
        }}

        #loading {{
            position: absolute;
            top: 50%;
            left: 50%;
            transform: translate(-50%, -50%);
            text-align: center;
            color: #fff;
        }}

        .spinner {{
            border: 4px solid rgba(255, 255, 255, 0.1);
            border-top: 4px solid #4caf50;
            border-radius: 50%;
            width: 40px;
            height: 40px;
            animation: spin 1s linear infinite;
            margin: 0 auto 20px;
        }}

        @keyframes spin {{
            0% {{ transform: rotate(0deg); }}
            100% {{ transform: rotate(360deg); }}
        }}

        .hidden {{
            display: none !important;
        }}
    </style>
</head>
<body>
    <div id="status" class="connecting">Connecting...</div>

    <div id="controls">
        <button class="control-btn" onclick="sendCtrlAltDel()" title="Send Ctrl+Alt+Del">
            Ctrl+Alt+Del
        </button>
        <button class="control-btn" onclick="toggleFullscreen()" title="Toggle Fullscreen">
            Fullscreen
        </button>
        <button class="control-btn" onclick="rfb && rfb.clipboardPasteFrom(prompt('Paste text:'))" title="Send Clipboard">
            Paste
        </button>
    </div>

    <div id="loading">
        <div class="spinner"></div>
        <div>Connecting to console...</div>
    </div>

    <div id="screen"></div>

    <!-- noVNC library from CDN -->
    <script type="module">
        // Import noVNC
        import RFB from 'https://cdn.jsdelivr.net/npm/@novnc/novnc@1.4.0/core/rfb.js';

        const status = document.getElementById('status');
        const loading = document.getElementById('loading');
        const screen = document.getElementById('screen');

        let rfb;

        // Connection handling
        function connectedToServer(e) {{
            status.textContent = 'Connected';
            status.className = 'connected';
            loading.classList.add('hidden');
            console.log('Connected to VNC server');
        }}

        function disconnectedFromServer(e) {{
            if (e.detail.clean) {{
                status.textContent = 'Disconnected';
            }} else {{
                status.textContent = 'Connection Failed';
            }}
            status.className = 'disconnected';
            loading.classList.remove('hidden');
            console.log('Disconnected from VNC server:', e.detail);
        }}

        function credentialsRequired(e) {{
            const password = prompt('VNC Password:');
            if (password) {{
                rfb.sendCredentials({{ password: password }});
            }}
        }}

        // Control functions
        window.sendCtrlAltDel = function() {{
            if (rfb) {{
                rfb.sendCtrlAltDel();
            }}
        }};

        window.toggleFullscreen = function() {{
            if (!document.fullscreenElement) {{
                document.documentElement.requestFullscreen();
            }} else {{
                document.exitFullscreen();
            }}
        }};

        // Initialize noVNC
        try {{
            rfb = new RFB(screen, '{ws_url}', {{
                credentials: {{}}  // No credentials by default
            }});

            // Configure noVNC settings
            rfb.viewOnly = false;
            rfb.scaleViewport = true;
            rfb.resizeSession = false;
            rfb.showDotCursor = true;
            rfb.background = '#1a1a1a';

            // Event handlers
            rfb.addEventListener('connect', connectedToServer);
            rfb.addEventListener('disconnect', disconnectedFromServer);
            rfb.addEventListener('credentialsrequired', credentialsRequired);

            // Clipboard handling
            rfb.addEventListener('clipboard', (e) => {{
                console.log('Clipboard data received from VM');
            }});

            // Make rfb global for control buttons
            window.rfb = rfb;

        }} catch (err) {{
            status.textContent = 'Error: ' + err.message;
            status.className = 'disconnected';
            console.error('Failed to create RFB client:', err);
        }}

        // Keyboard shortcuts
        document.addEventListener('keydown', (e) => {{
            // Prevent browser shortcuts when focused on VNC
            if (e.ctrlKey && e.altKey) {{
                e.preventDefault();
            }}
        }});

        // Handle window resize
        window.addEventListener('resize', () => {{
            if (rfb) {{
                // noVNC handles resizing automatically with scaleViewport
            }}
        }});

    </script>
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
