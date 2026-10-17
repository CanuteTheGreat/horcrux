//! VM console page. Route: `/vms/:id/console`.
//!
//! - **VNC** embeds the server-rendered noVNC page at
//!   `/api/console/:vm_id/novnc`, which issues its own console ticket and
//!   derives ws/wss + host:port from wherever it was loaded, so it works
//!   behind TLS and from any client host (no `localhost:8006` assumptions).
//!   It is also linked directly so it can be popped out into its own tab.
//! - **Serial** gets a ticket from `POST /api/console/:vm_id/serial` and
//!   talks to `/api/console/ws/serial/:ticket`, the WebSocket bridge to the
//!   VM's QEMU serial socket.
use crate::api;
use horcrux_common::VmConfig;
use leptos::*;
use leptos_router::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{BinaryType, MessageEvent, WebSocket};

/// Console access types
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConsoleType {
    Vnc,
    Spice,
    Serial,
}

/// Console page for one VM.
#[component]
pub fn VmConsole() -> impl IntoView {
    let params = use_params_map();
    let vm_id = move || params.get().get("id").cloned().unwrap_or_default();

    let (console_type, set_console_type) = create_signal(ConsoleType::Vnc);
    let (vm, set_vm) = create_signal(None::<VmConfig>);
    let (error_message, set_error_message) = create_signal(None::<String>);
    let display_edit = create_rw_signal(horcrux_common::VmDisplay::default());
    let (display_notice, set_display_notice) = create_signal(None::<String>);
    let save_display = move |_| {
        let id = vm_id();
        let d = display_edit.get();
        spawn_local(async move {
            match api::set_vm_display(&id, &d).await {
                Ok(cfg) => {
                    set_vm.set(Some(cfg));
                    set_display_notice.set(Some("Saved: applies at the next start".to_string()));
                }
                Err(e) => set_display_notice.set(Some(format!("Not saved: {}", e.message))),
            }
        });
    };

    create_effect(move |_| {
        let id = vm_id();
        if id.is_empty() {
            return;
        }
        spawn_local(async move {
            match api::fetch_json::<VmConfig>(&format!("/vms/{}", urlencoding::encode(&id))).await {
                Ok(config) => {
                    // Open the console the VM is actually configured for.
                    set_console_type.set(match config.display.protocol {
                        horcrux_common::DisplayProtocol::Spice => ConsoleType::Spice,
                        horcrux_common::DisplayProtocol::None => ConsoleType::Serial,
                        horcrux_common::DisplayProtocol::Vnc => ConsoleType::Vnc,
                    });
                    display_edit.set(config.display);
                    set_vm.set(Some(config));
                    set_error_message.set(None);
                }
                Err(e) => set_error_message.set(Some(format!("Failed to load VM: {}", e))),
            }
        });
    });

    let toggle_fullscreen = move |_| {
        if let Some(elem) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id("console-frame"))
        {
            let _ = elem.request_fullscreen();
        }
    };

    view! {
        <div class="console-page">
            <div class="page-header console-header">
                <h1>
                    "Console: "
                    {move || vm.get().map(|v| v.name).unwrap_or_else(vm_id)}
                </h1>
                <div class="header-actions console-controls">
                    <A href="/vms" class="btn btn-secondary">"Back to VMs"</A>
                    <select on:change=move |ev| {
                        match event_target_value(&ev).as_str() {
                            "serial" => set_console_type.set(ConsoleType::Serial),
                            "spice" => set_console_type.set(ConsoleType::Spice),
                            _ => set_console_type.set(ConsoleType::Vnc),
                        }
                    }>
                        <option value="vnc" selected=move || console_type.get() == ConsoleType::Vnc>
                            "VNC (graphical)"
                        </option>
                        <option value="spice" selected=move || console_type.get() == ConsoleType::Spice>
                            "SPICE (remote-viewer)"
                        </option>
                        <option value="serial" selected=move || console_type.get() == ConsoleType::Serial>
                            "Serial"
                        </option>
                    </select>
                    {move || (console_type.get() == ConsoleType::Vnc).then(|| view! {
                        <button class="btn btn-secondary" on:click=toggle_fullscreen>
                            "Fullscreen"
                        </button>
                        <a
                            class="btn btn-secondary"
                            href=api::novnc_page_url(&vm_id())
                            target="_blank"
                            rel="noopener"
                        >
                            "Open in new window"
                        </a>
                    })}
                </div>
            </div>

            {move || error_message.get().map(|msg| view! {
                <div class="error-banner">{msg}</div>
            })}

            {move || vm.get().and_then(|v| {
                (v.status != horcrux_common::VmStatus::Running).then(|| view! {
                    <div class="error-banner">
                        "This VM is " {v.status.to_string()}
                        " - start it to get a live console."
                    </div>
                    <details class="display-settings" open=true>
                        <summary>"Display settings"</summary>
                        <super::vm_display::DisplaySettingsFields display=display_edit/>
                        <button id="save-display" class="btn btn-primary" on:click=save_display>"Save display settings"</button>
                        {move || display_notice.get().map(|n| view! { <p id="display-notice">{n}</p> })}
                    </details>
                })
            })}

            <div class="console-container">
                {move || {
                    let id = vm_id();
                    if id.is_empty() {
                        return view! { <p>"No VM selected"</p> }.into_view();
                    }
                    match console_type.get() {
                        ConsoleType::Vnc => view! {
                            <iframe
                                id="console-frame"
                                class="console-iframe"
                                style="width:100%;height:75vh;border:0;background:#000;"
                                src=api::novnc_page_url(&id)
                                allowfullscreen=true
                            />
                        }
                        .into_view(),
                        ConsoleType::Serial => view! { <SerialConsole vm_id=id/> }.into_view(),
                        ConsoleType::Spice => view! { <SpiceConsole vm_id=id/> }.into_view(),
                    }
                }}
            </div>
        </div>
    }
}

/// SPICE: connection details + `.vv` file for remote-viewer/virt-viewer.
///
/// There is no in-browser SPICE client here: the only one (spice-html5) is
/// unmaintained, has no audio-recording or USB-redirection support and
/// would have to be vendored; remote-viewer gives the full channel set
/// (display, audio playback/record, clipboard agent, USB redirection).
#[component]
fn SpiceConsole(vm_id: String) -> impl IntoView {
    let (info, set_info) = create_signal(None::<api::ConsoleInfo>);
    let (err, set_err) = create_signal(None::<String>);
    let id = vm_id.clone();
    create_effect(move |_| {
        let id = id.clone();
        spawn_local(async move {
            match api::create_spice_console(&id).await {
                Ok(i) => set_info.set(Some(i)),
                Err(e) => set_err.set(Some(e.message)),
            }
        });
    });
    let vv = api::spice_vv_url(&vm_id);
    view! {
        <div class="spice-console">
            {move || err.get().map(|e| view! { <div class="error-banner" id="spice-error">{e}</div> })}
            {move || info.get().map(|i| view! {
                <p id="spice-endpoint">"SPICE is listening on " <code>{format!("{}:{}", i.host, i.port)}</code>
                    " (password changes at every VM start)."</p>
            })}
            <a id="spice-vv" class="btn btn-primary" href=vv.clone() download="">"Download .vv for remote-viewer"</a>
            <p class="help">
                "Open the file with remote-viewer / virt-viewer (virt-viewer package on Linux, "
                "virt-viewer MSI on Windows). It carries display, audio (playback and microphone), "
                "clipboard sharing (install spice-vdagent in the guest) and USB redirection. "
                "SPICE listens on 127.0.0.1 unless the server sets HORCRUX_SPICE_LISTEN; for a "
                "localhost-only server use an SSH tunnel (ssh -L <port>:127.0.0.1:<port> <host>) "
                "and edit host= in the file to 127.0.0.1. A browser SPICE client is not offered: "
                "spice-html5 is unmaintained and lacks audio input and USB redirection."
            </p>
        </div>
    }
}

/// Serial console over the ticket-authenticated WebSocket bridge.
#[component]
fn SerialConsole(vm_id: String) -> impl IntoView {
    let (output, set_output) = create_signal(String::new());
    let (input, set_input) = create_signal(String::new());
    let (connected, set_connected) = create_signal(false);
    let (status, set_status) = create_signal("Connecting...".to_string());
    let (ws_handle, set_ws_handle) = create_signal(None::<WebSocket>);

    let connect = move |vm: String| {
        if let Some(old) = ws_handle.get_untracked() {
            let _ = old.close();
        }
        set_status.set("Connecting...".to_string());
        spawn_local(async move {
            let info = match api::create_serial_console(&vm).await {
                Ok(info) => info,
                Err(e) => {
                    set_status.set(format!("Failed to open serial console: {}", e));
                    return;
                }
            };
            let url = api::resolve_ws_url(&api::serial_console_ws_path(&info.ticket));
            let ws = match WebSocket::new(&url) {
                Ok(ws) => ws,
                Err(e) => {
                    set_status.set(format!("Failed to open WebSocket: {:?}", e));
                    return;
                }
            };
            // The bridge sends raw serial bytes as binary frames.
            ws.set_binary_type(BinaryType::Arraybuffer);

            let onopen = Closure::wrap(Box::new(move |_: JsValue| {
                set_connected.set(true);
                set_status.set("Connected".to_string());
            }) as Box<dyn FnMut(JsValue)>);
            ws.set_onopen(Some(onopen.as_ref().unchecked_ref()));
            onopen.forget();

            let onmessage = Closure::wrap(Box::new(move |e: MessageEvent| {
                let data = e.data();
                let chunk = if let Ok(buf) = data.clone().dyn_into::<js_sys::ArrayBuffer>() {
                    String::from_utf8_lossy(&js_sys::Uint8Array::new(&buf).to_vec()).into_owned()
                } else if let Some(text) = data.as_string() {
                    text
                } else {
                    return;
                };
                set_output.update(|o| o.push_str(&chunk));
            }) as Box<dyn FnMut(MessageEvent)>);
            ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
            onmessage.forget();

            let onclose = Closure::wrap(Box::new(move |_: JsValue| {
                set_connected.set(false);
                set_status.set("Disconnected".to_string());
            }) as Box<dyn FnMut(JsValue)>);
            ws.set_onclose(Some(onclose.as_ref().unchecked_ref()));
            onclose.forget();

            set_ws_handle.set(Some(ws));
        });
    };

    connect(vm_id.clone());

    let send_line = move || {
        if let Some(ws) = ws_handle.get_untracked() {
            let mut line = input.get_untracked();
            line.push('\r');
            let _ = ws.send_with_str(&line);
            set_input.set(String::new());
        }
    };

    on_cleanup(move || {
        if let Some(ws) = ws_handle.get_untracked() {
            let _ = ws.close();
        }
    });

    view! {
        <div class="serial-container">
            <div class="console-controls">
                <span>{move || status.get()}</span>
                <button class="btn btn-sm btn-secondary" on:click=move |_| connect(vm_id.clone())>
                    "Reconnect"
                </button>
            </div>
            <pre
                class="serial-output"
                style="background:#111;color:#0f0;padding:1em;height:60vh;overflow:auto;white-space:pre-wrap;"
            >
                {move || output.get()}
            </pre>
            <div class="serial-input-container">
                <input
                    type="text"
                    class="serial-input"
                    style="width:80%"
                    prop:value=move || input.get()
                    on:input=move |ev| set_input.set(event_target_value(&ev))
                    on:keydown=move |ev| {
                        if ev.key() == "Enter" {
                            send_line();
                        }
                    }
                    disabled=move || !connected.get()
                    placeholder="Type a line and press Enter"
                />
                <button class="btn btn-sm btn-primary" on:click=move |_| send_line()>"Send"</button>
            </div>
        </div>
    }
}
