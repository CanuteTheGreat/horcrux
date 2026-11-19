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

/// The console JavaScript that types pasted text as key events (also
/// exercised in a browser by `horcrux-ui/e2e/console_paste_e2e.py`).
pub const PASTE_JS: &str = include_str!("paste.js");

/// How the console's Paste button behaves for one VM: the node's `[console]`
/// settings plus the VM's keyboard layout.
#[derive(Debug, Clone, PartialEq)]
pub struct PasteSettings {
    /// `type`, `clipboard` or `both`.
    pub mode: String,
    pub delay_ms: u32,
    pub line_delay_ms: u32,
    pub max_chars: u32,
    pub confirm_chars: u32,
    pub unicode_keysyms: bool,
    /// QEMU keymap name of the VM (`en-us` when the VM sets none).
    pub layout: String,
}

impl Default for PasteSettings {
    fn default() -> Self {
        Self::new(&horcrux_core::config::ConsoleConfig::default(), None)
    }
}

impl PasteSettings {
    pub fn new(cfg: &horcrux_core::config::ConsoleConfig, layout: Option<&str>) -> Self {
        let mode = match cfg.paste_mode.as_str() {
            m @ ("type" | "clipboard" | "both") => m.to_string(),
            _ => "type".to_string(),
        };
        Self {
            mode,
            delay_ms: cfg.paste_delay_ms,
            line_delay_ms: cfg.paste_line_delay_ms,
            max_chars: cfg.paste_max_chars,
            confirm_chars: cfg.paste_confirm_chars,
            unicode_keysyms: cfg.paste_unicode_keysyms,
            layout: layout.unwrap_or("en-us").to_string(),
        }
    }

    /// JSON object for the page script. `<` is escaped so no value can end
    /// the surrounding `<script>` element.
    pub fn to_script_json(&self) -> String {
        serde_json::json!({
            "mode": self.mode,
            "delay": self.delay_ms,
            "lineDelay": self.line_delay_ms,
            "maxChars": self.max_chars,
            "confirmChars": self.confirm_chars,
            "unicode": self.unicode_keysyms,
            "layout": self.layout,
        })
        .to_string()
        .replace('<', "\\u003c")
    }
}

/// Get noVNC client HTML page
/// `password` is this VM run's VNC password (QEMU runs VNC with
/// `password=on`); it reaches the browser only inside this ticketed page.
pub fn get_novnc_html(
    _ticket_id: &str,
    ws_url: &str,
    password: Option<&str>,
    paste: &PasteSettings,
) -> String {
    let paste_cfg = paste.to_script_json();
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
            width: 100%;
            height: 100%;
            overflow: hidden;
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
        <button class="control-btn" id="btn-paste" title="Type the clipboard text into the guest as key presses">Paste</button>
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
        const pasteCfg = {paste_cfg};
        {paste_js}
        // Paste = type the text into the guest as key presses (works in BIOS
        // menus, boot loaders, installers and disk-unlock prompts, which have
        // no clipboard agent). Mode `clipboard` / `both` also (or only) sets
        // the guest clipboard, which needs a clipboard agent in the guest.
        // The text itself is never logged or stored: the audit log gets only
        // the character count.
        let pasteJob = null;
        const pasteBtn = document.getElementById('btn-paste');
        function pasteReport(typing, done, total, note) {{
            if (!embedded) return;
            window.parent.postMessage({{
                horcruxConsole: 'paste-state', typing: typing,
                done: done, total: total, note: note || ''
            }}, window.location.origin);
        }}
        function pasteAudit(chars, mode) {{
            const m = location.pathname.match(/\/console\/([^/]+)\/novnc/);
            if (!m) return;
            fetch('/api/console/' + m[1] + '/paste-audit', {{
                method: 'POST', credentials: 'same-origin',
                headers: {{ 'Content-Type': 'application/json' }},
                body: JSON.stringify({{ chars: chars, mode: mode }})
            }}).catch(() => {{}});
        }}
        function cancelPaste() {{
            if (pasteJob) pasteJob.cancelled = true;
        }}
        async function pasteText(text, modeOverride) {{
            if (!rfb || !text || pasteJob) return;
            const mode = ['type', 'clipboard', 'both'].includes(modeOverride)
                ? modeOverride : pasteCfg.mode;
            const chars = Array.from(text).length;
            if (chars > pasteCfg.maxChars) {{
                alert('Too much text to paste (' + chars + ' characters; the limit is '
                    + pasteCfg.maxChars + ').');
                return;
            }}
            if (mode !== 'clipboard' && pasteCfg.confirmChars > 0 && chars > pasteCfg.confirmChars
                && !confirm('Type ' + chars + ' characters into the guest?')) {{
                return;
            }}
            pasteAudit(chars, mode);
            if (mode === 'clipboard' || mode === 'both') rfb.clipboardPasteFrom(text);
            if (mode === 'clipboard') return;
            const plan = HorcruxPaste.plan(text, pasteCfg.layout, {{ unicode: pasteCfg.unicode }});
            const notes = [];
            if (plan.nonAscii > 0) {{
                notes.push(plan.nonAscii + ' character(s) outside ASCII are typed only if the VM\'s'
                    + ' keyboard layout (' + pasteCfg.layout + ') has them');
            }}
            if (plan.skipped > 0) notes.push(plan.skipped + ' skipped');
            const connectedText = status.textContent;
            pasteJob = {{ cancelled: false }};
            pasteBtn.textContent = 'Cancel paste';
            pasteBtn.onclick = cancelPaste;
            pasteReport(true, 0, plan.steps.length, notes.join('; '));
            let done = 0;
            try {{
                done = await HorcruxPaste.type(rfb, plan.steps, pasteJob, {{
                    delay: pasteCfg.delay, lineDelay: pasteCfg.lineDelay,
                    onProgress: (d, t) => {{
                        status.textContent = 'Typing ' + d + '/' + t;
                        pasteReport(true, d, t, '');
                    }}
                }});
            }} finally {{
                const cancelled = pasteJob.cancelled;
                pasteJob = null;
                pasteBtn.textContent = 'Paste';
                pasteBtn.onclick = null;
                status.textContent = cancelled ? 'Paste cancelled' : connectedText;
                pasteReport(false, done, plan.steps.length,
                    cancelled ? 'cancelled' : notes.join('; '));
                if (cancelled) setTimeout(() => {{ status.textContent = connectedText; }}, 2500);
            }}
        }}
        window.horcruxPaste = pasteText; // resolves when typing has finished (e2e tests)
        window.horcruxPasteCfg = pasteCfg;
        // Reads the browser clipboard (asks permission the first time); a
        // plain prompt is the fallback where that is not allowed.
        async function pasteFromBrowser(modeOverride) {{
            let text = '';
            try {{ text = await navigator.clipboard.readText(); }} catch (_) {{}}
            if (!text) text = prompt('Text to type into the guest:') || '';
            if (text) pasteText(text, modeOverride);
        }}
        function toggleFullscreen() {{
            if (document.fullscreenElement) {{
                document.exitFullscreen();
            }} else {{
                document.documentElement.requestFullscreen();
            }}
        }}
        document.getElementById('btn-cad').addEventListener('click', sendCtrlAltDel);
        pasteBtn.addEventListener('click', () => {{ if (pasteJob) cancelPaste(); else pasteFromBrowser(); }});
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
        // Always fit the guest to the window (scale-to-fit, aspect kept). The
        // fullscreen size must never stick: leaving fullscreen through F11,
        // Esc, the browser UI or the button resizes the window late and
        // sometimes without noVNC noticing, so every size/fullscreen signal
        // re-runs the scaling, now and again once the transition settled.
        function refit() {{
            if (!rfb) return;
            rfb.scaleViewport = false;
            rfb.scaleViewport = true;
        }}
        let refitTimers = [];
        function scheduleRefit() {{
            refitTimers.forEach(clearTimeout);
            refit();
            requestAnimationFrame(refit);
            refitTimers = [60, 250, 700].map((ms) => setTimeout(refit, ms));
        }}
        window.horcruxRefit = scheduleRefit;
        function onModeChange() {{
            syncMode();
            scheduleRefit();
        }}
        document.addEventListener('fullscreenchange', onModeChange);
        window.addEventListener('resize', onModeChange);
        if (window.ResizeObserver) {{
            new ResizeObserver(scheduleRefit).observe(screen);
        }}
        syncMode();

        // Commands from the parent page's toolbar.
        window.addEventListener('message', (e) => {{
            if (e.origin !== window.location.origin || !e.data) return;
            if (e.data.horcruxConsole === 'ctrl-alt-del') sendCtrlAltDel();
            else if (e.data.horcruxConsole === 'paste') pasteText(e.data.text, e.data.mode);
            else if (e.data.horcruxConsole === 'paste-cancel') cancelPaste();
            else if (e.data.horcruxConsole === 'refit') scheduleRefit();
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
            scheduleRefit();

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
        paste_cfg = paste_cfg,
        paste_js = PASTE_JS,
        password = password
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_novnc_html_generation() {
        let html = get_novnc_html(
            "test-ticket",
            "ws://localhost:6080/test-ticket",
            None,
            &PasteSettings::default(),
        );
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
        let html = get_novnc_html("t", "/api/console/ws/t", None, &PasteSettings::default());
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
    /// NOVNC_DUMP_WS=<ws://host:port> points the page at a real VNC
    /// websocket (QEMU `-vnc :N,websocket=PORT`) for the paste e2e.
    #[test]
    fn novnc_page_dump() {
        if let Ok(path) = std::env::var("NOVNC_DUMP") {
            let ws = std::env::var("NOVNC_DUMP_WS").unwrap_or_else(|_| "/api/console/ws/t".into());
            let html = get_novnc_html("t", &ws, Some("ab12"), &PasteSettings::default());
            std::fs::write(path, html).unwrap();
        }
    }

    #[test]
    fn test_novnc_html_relative_ws_path_resolved_client_side() {
        // The API passes a path-only URL; the page must resolve it against
        // window.location rather than embedding a hardcoded host.
        let html = get_novnc_html(
            "t",
            "/api/console/ws/t",
            Some("ab12'cd"),
            &PasteSettings::default(),
        );
        assert!(html.contains("password: 'ab12cd'"));
        assert!(html.contains("const wsTarget = '/api/console/ws/t';"));
        assert!(html.contains("window.location.host + wsTarget"));
        assert!(html.contains("new RFB(screen, wsUrl,"));
        assert!(!html.contains("localhost"));
    }

    /// Paste types the text as key presses by default (a clipboard paste is
    /// dropped by every guest without a clipboard agent: BIOS, installers,
    /// disk-unlock prompts), carries the node's pacing/limits, and the audit
    /// call reports a count, never the text.
    #[test]
    fn paste_types_keys_and_carries_settings() {
        let html = get_novnc_html("t", "/api/console/ws/t", None, &PasteSettings::default());
        assert!(html.contains("const HorcruxPaste"));
        assert!(html.contains("rfb.sendKey("));
        assert!(html.contains(
            r#"const pasteCfg = {"confirmChars":500,"delay":20,"layout":"en-us","lineDelay":50,"maxChars":20000,"mode":"type","unicode":true};"#
        ));
        assert!(html.contains("/paste-audit"));
        assert!(html.contains("body: JSON.stringify({ chars: chars, mode: mode })"));
        assert!(html.contains("paste-cancel"));
        // the pasted text is never passed to console.* or the audit call
        assert!(!html.contains("console.log(text"));
    }

    #[test]
    fn paste_settings_follow_config_and_layout() {
        let cfg = horcrux_core::config::ConsoleConfig {
            paste_mode: "both".into(),
            paste_delay_ms: 20,
            paste_unicode_keysyms: false,
            ..Default::default()
        };
        let p = PasteSettings::new(&cfg, Some("de"));
        let j = p.to_script_json();
        assert!(j.contains(r#""mode":"both""#) && j.contains(r#""layout":"de""#));
        assert!(j.contains(r#""delay":20"#) && j.contains(r#""unicode":false"#));
        // an unknown mode falls back to typing; nothing can close the script tag
        let bad = horcrux_core::config::ConsoleConfig {
            paste_mode: "</script>".into(),
            ..Default::default()
        };
        assert_eq!(PasteSettings::new(&bad, Some("</script>")).mode, "type");
        assert!(!PasteSettings::new(&bad, Some("</script>"))
            .to_script_json()
            .contains('<'));
    }
}
