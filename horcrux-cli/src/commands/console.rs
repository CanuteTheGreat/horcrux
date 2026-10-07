use crate::api::ApiClient;
use crate::output;
use anyhow::Result;
use serde::Deserialize;

/// Mirrors `horcrux_api::console::ConsoleInfo` for CLI-side deserialization
/// without pulling in the whole API crate as a dependency.
#[derive(Debug, Deserialize)]
struct ConsoleInfo {
    vm_id: String,
    #[serde(rename = "console_type")]
    #[allow(dead_code)]
    console_type: String,
    host: String,
    #[allow(dead_code)]
    port: u16,
    ticket: String,
    ws_port: u16,
}

pub async fn handle_console_command(vm_id: &str, serial: bool, api: &ApiClient) -> Result<()> {
    let endpoint = if serial { "serial" } else { "vnc" };

    output::print_info(&format!(
        "Opening {} console for VM {}...",
        if serial { "serial" } else { "VNC" },
        vm_id
    ));

    let info: ConsoleInfo = api
        .post(&format!("/api/console/{}/{}", vm_id, endpoint), &())
        .await?;

    let ws_path = if serial {
        format!("/api/console/ws/serial/{}", info.ticket)
    } else {
        format!("/api/console/ws/{}", info.ticket)
    };

    output::print_success(&format!(
        "Console ready for VM {} (ticket {})",
        info.vm_id, info.ticket
    ));
    output::print_info(&format!(
        "WebSocket bridge: ws://{}:{}{}",
        info.host, info.ws_port, ws_path
    ));

    if serial {
        output::print_info(
            "Connect a terminal client (e.g. websocat/xterm.js) to the WebSocket URL above to interact with the serial console.",
        );
    } else {
        output::print_info(&format!(
            "Open the noVNC page: {}/api/console/{}/novnc",
            api.base_url(),
            vm_id
        ));
    }

    Ok(())
}
