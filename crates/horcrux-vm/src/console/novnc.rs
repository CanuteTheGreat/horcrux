//! noVNC WebSocket proxy implementation
//! Provides WebSocket proxy for VNC connections using Axum WebSocket support
use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, State, WebSocketUpgrade,
    },
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

    if ticket.console_type == super::ConsoleType::Serial {
        return axum::response::Response::builder()
            .status(axum::http::StatusCode::BAD_REQUEST)
            .body(axum::body::Body::from(
                "Serial console tickets use /api/console/ws/serial/:ticket",
            ))
            .unwrap();
    }

    info!(
        "Valid ticket for VM {} (port {})",
        ticket.vm_id, ticket.vnc_port
    );

    // Upgrade the WebSocket connection
    ws.on_upgrade(move |socket| handle_vnc_connection(socket, ticket.target_host, ticket.vnc_port))
}

/// Handle the WebSocket connection and proxy to VNC
async fn handle_vnc_connection(ws_socket: WebSocket, host: String, vnc_port: u16) {
    info!("Establishing VNC connection to port {}", vnc_port);

    // Connect to the VNC server
    let vnc_stream = match TcpStream::connect((host.as_str(), vnc_port)).await {
        Ok(stream) => stream,
        Err(e) => {
            error!(
                "Failed to connect to VNC server on port {}: {}",
                vnc_port, e
            );
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

    info!(
        "VNC WebSocket proxy connection closed for port {}",
        vnc_port
    );
}

/// Get noVNC client HTML page
/// `password` is this VM run's VNC password (QEMU runs VNC with
/// `password=on`); it reaches the browser only inside this ticketed page.
pub fn get_novnc_html(_ticket_id: &str, ws_url: &str, password: Option<&str>) -> String {
    // Hex-only (generated by vm::display), but never trust it into JS.
    let password: String = password
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
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

        /* The guest screen owns the whole viewport. Nothing is drawn over it:
           the controls live in the parent page's toolbar (?embed=1) or, in a
           pop-out window / fullscreen, in a bar that is translated out of
           view and only slides in while the pointer is at the top edge. */
        #screen {{
            position: fixed;
            top: 0;
            left: 0;
            right: 0;
            bottom: 0;
            width: 100vw;
            height: 100vh;
        }}

        #bar {{
            position: fixed;
            top: 0;
            left: 0;
            right: 0;
            height: 36px;
            box-sizing: border-box;
            display: flex;
            align-items: center;
            gap: 8px;
            padding: 0 10px;
            background: rgba(20, 20, 20, 0.96);
            border-bottom: 1px solid #444;
            color: #fff;
            font-size: 13px;
            z-index: 1000;
            transform: translateY(-100%);
            pointer-events: none;
            visibility: hidden;
            transition: transform 0.15s ease, visibility 0s linear 0.15s;
        }}
        body.autohide #bar.show {{
            transform: translateY(0);
            pointer-events: auto;
            visibility: visible;
            transition: transform 0.15s ease;
        }}
        /* embedded: the parent toolbar has the buttons and the indicator */
        body.embed #bar {{
            display: none;
        }}

        #status {{
            display: inline-flex;
            align-items: center;
            gap: 6px;
            margin-left: auto;
            font-weight: 500;
        }}
        #status::before {{
            content: '';
            width: 9px;
            height: 9px;
            border-radius: 50%;
            background: currentColor;
        }}

        .connecting {{
            color: #ff9800;
        }}
        .connected {{
            color: #4caf50;
        }}
        .disconnected {{
            color: #f44336;
        }}

        .control-btn {{
            padding: 4px 12px;
            background: #2a2a2a;
            color: #fff;
            border: 1px solid #555;
            border-radius: 5px;
            cursor: pointer;
            font-size: 13px;
        }}

        .control-btn:hover {{
            background: #3a3a3a;
            border-color: #777;
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
            pointer-events: none;
            z-index: 1;
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
    <div id="bar">
        <button class="control-btn" id="btn-cad" title="Send Ctrl+Alt+Del">Ctrl+Alt+Del</button>
        <button class="control-btn" id="btn-paste" title="Send text to the guest">Paste</button>
        <button class="control-btn" id="btn-fullscreen" title="Toggle fullscreen">Fullscreen</button>
        <span id="status" class="connecting">Connecting...</span>
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

        const embedded = new URLSearchParams(location.search).get('embed') === '1'
            && window.parent !== window;
        document.body.classList.add(embedded ? 'embed' : 'standalone');
        const bar = document.getElementById('bar');
        const status = document.getElementById('status');
        const loading = document.getElementById('loading');
        const screen = document.getElementById('screen');

        let rfb;

        // Tell the embedding page (same origin) about the connection state so
        // its toolbar can show the indicator.
        function reportStatus(state) {{
            if (!embedded) return;
            window.parent.postMessage({{
                horcruxConsole: 'status',
                state: state,
                text: status.textContent
            }}, window.location.origin);
        }}

        function sendCtrlAltDel() {{
            if (rfb) rfb.sendCtrlAltDel();
        }}
        function pasteText(text) {{
            if (rfb && text) {{
                rfb.clipboardPasteFrom(text);
            }}
        }}
        function toggleFullscreen() {{
            if (document.fullscreenElement) {{
                document.exitFullscreen();
            }} else {{
                document.documentElement.requestFullscreen();
            }}
        }}
        document.getElementById('btn-cad').addEventListener('click', sendCtrlAltDel);
        document.getElementById('btn-paste').addEventListener('click', () => pasteText(prompt('Paste text:')));
        document.getElementById('btn-fullscreen').addEventListener('click', toggleFullscreen);

        // Auto-hiding bar: pop-out windows and fullscreen only. It stays out
        // of view (and out of the screen's area) until the pointer touches the
        // top edge, then hides again once the pointer has left it.
        function autohideOn() {{
            return !embedded || !!document.fullscreenElement
                || window.innerHeight >= screen.height - 1;
        }}
        function syncMode() {{
            const on = autohideOn();
            document.body.classList.toggle('autohide', on);
            if (!on) bar.classList.remove('show');
        }}
        let hideTimer = null;
        function showBar() {{
            if (!document.body.classList.contains('autohide')) return;
            clearTimeout(hideTimer);
            bar.classList.add('show');
        }}
        function scheduleHide() {{
            clearTimeout(hideTimer);
            hideTimer = setTimeout(() => bar.classList.remove('show'), 700);
        }}
        window.addEventListener('mousemove', (e) => {{
            if (e.clientY <= 2) showBar();
            else if (e.clientY > 44 && bar.classList.contains('show')) scheduleHide();
            else if (e.clientY <= 44) clearTimeout(hideTimer);
        }}, true);
        window.addEventListener('touchstart', (e) => {{
            const t = e.touches[0];
            if (t && t.clientY <= 12) showBar();
        }}, true);
        document.addEventListener('mouseleave', scheduleHide);
        document.addEventListener('fullscreenchange', syncMode);
        window.addEventListener('resize', syncMode);
        syncMode();

        // Commands from the parent page's toolbar.
        window.addEventListener('message', (e) => {{
            if (e.origin !== window.location.origin || !e.data) return;
            if (e.data.horcruxConsole === 'ctrl-alt-del') sendCtrlAltDel();
            else if (e.data.horcruxConsole === 'paste') pasteText(e.data.text);
        }});

        // Connection handling
        function connectedToServer(e) {{
            status.textContent = 'Connected';
            status.className = 'connected';
            loading.classList.add('hidden');
            reportStatus('connected');
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
            reportStatus('disconnected');
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

        // Resolve the WebSocket URL. A path-only target ("/api/...") is made
        // absolute against the page's own origin (ws:// or wss:// to match
        // http/https), so the console works from any host/port/proxy the page
        // was actually loaded through instead of a fixed hardcoded host.
        const wsTarget = '{ws_url}';
        const wsUrl = wsTarget.startsWith('/')
            ? (window.location.protocol === 'https:' ? 'wss://' : 'ws://') + window.location.host + wsTarget
            : wsTarget;

        // Initialize noVNC
        try {{
            rfb = new RFB(screen, wsUrl, {{
                credentials: {{ password: '{password}' }}
            }});

            // Configure noVNC settings
            rfb.viewOnly = false;
            rfb.scaleViewport = true;
            rfb.resizeSession = false;
            rfb.showDotCursor = true;
            rfb.background = '#000000';

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
            reportStatus('disconnected');
            console.error('Failed to create RFB client:', err);
        }}

        // Keyboard shortcuts
        document.addEventListener('keydown', (e) => {{
            // Prevent browser shortcuts when focused on VNC
            if (e.ctrlKey && e.altKey) {{
                e.preventDefault();
            }}
        }});


    </script>
</body>
</html>"#,
        ws_url = ws_url,
        password = password
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_novnc_html_generation() {
        let html = get_novnc_html("test-ticket", "ws://localhost:6080/test-ticket", None);
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("ws://localhost:6080/test-ticket"));
        assert!(html.contains("Horcrux Console"));
    }

    /// Nothing may be drawn over the guest's screen: the controls and the
    /// status live in a bar that is translated out of view unless the pointer
    /// is at the top edge (the browser-level proof is
    /// `horcrux-ui/e2e/console_overlay_e2e.py`, run by the UI E2E job).
    #[test]
    fn test_novnc_html_has_no_overlay_over_the_screen() {
        let html = get_novnc_html("t", "/api/console/ws/t", None);
        // the old always-visible overlays are gone
        assert!(!html.contains("#controls"));
        assert!(!html.contains("top: 15px"));
        // the bar is hidden by default and only shown by the autohide class
        assert!(html.contains("transform: translateY(-100%)"));
        assert!(html.contains("body.autohide #bar.show"));
        assert!(html.contains("body.embed #bar"));
        // the screen fills the viewport
        assert!(html.contains("#screen {"));
        assert!(html.contains("bottom: 0;"));
        // status element + buttons the UI E2E and parent toolbar rely on
        assert!(html.contains("id=\"status\""));
        assert!(html.contains("id=\"btn-cad\""));
        assert!(html.contains("horcruxConsole"));
    }

    /// Dump the page for the Firefox overlay check (NOVNC_DUMP=<file>).
    #[test]
    fn novnc_page_dump() {
        if let Ok(path) = std::env::var("NOVNC_DUMP") {
            let html = get_novnc_html("t", "/api/console/ws/t", Some("ab12"));
            std::fs::write(path, html).unwrap();
        }
    }

    #[test]
    fn test_novnc_html_relative_ws_path_resolved_client_side() {
        // The API passes a path-only URL; the page must resolve it against
        // window.location rather than embedding a hardcoded host.
        let html = get_novnc_html("t", "/api/console/ws/t", Some("ab12'cd"));
        assert!(html.contains("password: 'ab12cd'"));
        assert!(html.contains("const wsTarget = '/api/console/ws/t';"));
        assert!(html.contains("window.location.host + wsTarget"));
        assert!(html.contains("new RFB(screen, wsUrl,"));
        assert!(!html.contains("localhost"));
    }
}
