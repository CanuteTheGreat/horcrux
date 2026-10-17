//! Live container logs, interactive exec terminal, and the Docker/Podman
//! events feed - Portainer's defining "web terminal into a container"
//! feature plus live log tailing and an events stream, all driven by the
//! same ticket-authenticated WebSocket pattern as the VM console page.
use crate::api;
use leptos::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{MessageEvent, WebSocket};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ContainerWsMode {
    Logs,
    Exec,
}

/// Live log viewer for one container. Route: `/containers/:id/logs`.
#[component]
pub fn ContainerLogsPage() -> impl IntoView {
    view! { <ContainerConsole mode=ContainerWsMode::Logs/> }
}

/// Interactive exec terminal for one container. Route: `/containers/:id/exec`.
#[component]
pub fn ContainerExecPage() -> impl IntoView {
    view! { <ContainerConsole mode=ContainerWsMode::Exec/> }
}

#[component]
fn ContainerConsole(mode: ContainerWsMode) -> impl IntoView {
    let params = leptos_router::use_params_map();
    let container_id = move || params.get().get("id").cloned().unwrap_or_default();

    let (runtime, set_runtime) = create_signal("docker".to_string());
    // The container list links here with ?runtime=<runtime>.
    let query = leptos_router::use_query_map();
    create_effect(move |_| {
        if let Some(rt) = query.get().get("runtime") {
            set_runtime.set(rt.clone());
        }
    });
    let (exec_command, set_exec_command) = create_signal("/bin/sh".to_string());
    let (output, set_output) = create_signal(String::new());
    let (connected, set_connected) = create_signal(false);
    let (input_line, set_input_line) = create_signal(String::new());
    let (ws_handle, set_ws_handle) = create_signal(None::<WebSocket>);

    let connect = move |_| {
        let id = container_id();
        let rt = runtime.get();
        let cmd = exec_command.get();
        set_output.set(String::new());

        spawn_local(async move {
            let ticket = match mode {
                ContainerWsMode::Logs => api::get_container_logs_ticket(&id, &rt).await,
                ContainerWsMode::Exec => api::get_container_exec_ticket(&id, &cmd, &rt).await,
            };
            let ticket = match ticket {
                Ok(t) => t,
                Err(e) => {
                    logging::log!("Failed to get ws ticket: {}", e.message);
                    return;
                }
            };

            let url = api::resolve_ws_url(&ticket.ws_path);
            let ws = match WebSocket::new(&url) {
                Ok(ws) => ws,
                Err(e) => {
                    logging::log!("Failed to open container WebSocket: {:?}", e);
                    return;
                }
            };

            let onopen = Closure::wrap(Box::new(move |_: JsValue| {
                set_connected.set(true);
            }) as Box<dyn FnMut(JsValue)>);
            ws.set_onopen(Some(onopen.as_ref().unchecked_ref()));
            onopen.forget();

            // Exec output arrives as binary frames (PTY / exec stream bytes);
            // they used to be dropped, so the terminal never showed output.
            ws.set_binary_type(web_sys::BinaryType::Arraybuffer);
            let onmessage = Closure::wrap(Box::new(move |e: MessageEvent| {
                let data = e.data();
                let chunk = if let Some(txt) = data.as_string() {
                    txt
                } else if let Ok(buf) = data.dyn_into::<js_sys::ArrayBuffer>() {
                    String::from_utf8_lossy(&js_sys::Uint8Array::new(&buf).to_vec()).to_string()
                } else {
                    return;
                };
                let chunk = crate::pages::lxc::spec::strip_ansi(&chunk);
                set_output.update(|o| o.push_str(&chunk));
            }) as Box<dyn FnMut(MessageEvent)>);
            ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
            onmessage.forget();

            let onclose = Closure::wrap(Box::new(move |_: JsValue| {
                set_connected.set(false);
            }) as Box<dyn FnMut(JsValue)>);
            ws.set_onclose(Some(onclose.as_ref().unchecked_ref()));
            onclose.forget();

            set_ws_handle.set(Some(ws));
        });
    };

    let do_send = move || {
        if let Some(ws) = ws_handle.get() {
            let mut line = input_line.get();
            line.push('\n');
            let _ = ws.send_with_str(&line);
            set_input_line.set(String::new());
        }
    };

    on_cleanup(move || {
        if let Some(ws) = ws_handle.get_untracked() {
            let _ = ws.close();
        }
    });

    let title = match mode {
        ContainerWsMode::Logs => "Live Logs",
        ContainerWsMode::Exec => "Interactive Terminal",
    };

    view! {
        <div class="container-list">
            <div class="page-header">
                <h1>{title} " — " {container_id}</h1>
                <div class="header-actions">
                    <select id="console-runtime" prop:value=runtime on:change=move |ev| set_runtime.set(event_target_value(&ev))>
                        <option value="docker" selected=move || runtime.get() == "docker">"Docker"</option>
                        <option value="podman" selected=move || runtime.get() == "podman">"Podman"</option>
                        <option value="lxc" selected=move || runtime.get() == "lxc">"LXC"</option>
                    </select>
                    {move || {
                        if mode == ContainerWsMode::Exec {
                            view! {
                                <input
                                    type="text"
                                    placeholder="command (default /bin/sh)"
                                    prop:value=exec_command
                                    on:input=move |ev| set_exec_command.set(event_target_value(&ev))
                                />
                            }.into_view()
                        } else {
                            view! {}.into_view()
                        }
                    }}
                    <button class="btn btn-primary" on:click=connect>
                        {move || if connected.get() { "Reconnect" } else { "Connect" }}
                    </button>
                    <span>{move || if connected.get() { "● connected" } else { "○ disconnected" }}</span>
                </div>
            </div>

            <pre style="background:#111;color:#0f0;padding:1em;height:400px;overflow:auto;white-space:pre-wrap;">
                {move || output.get()}
            </pre>

            {move || {
                if mode == ContainerWsMode::Exec {
                    view! {
                        <div class="form-section">
                            <input
                                type="text"
                                style="width:80%"
                                placeholder="type a command, press Send"
                                prop:value=input_line
                                on:input=move |ev| set_input_line.set(event_target_value(&ev))
                                on:keydown=move |ev| {
                                    if ev.key() == "Enter" {
                                        do_send();
                                    }
                                }
                            />
                            <button class="btn btn-sm btn-primary" on:click=move |_| do_send()>"Send"</button>
                        </div>
                    }.into_view()
                } else {
                    view! {}.into_view()
                }
            }}
        </div>
    }
}
