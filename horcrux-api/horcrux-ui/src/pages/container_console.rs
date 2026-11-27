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
    /// System console (/dev/console): LXC, LXD and Incus containers.
    Console,
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

/// Interactive system console of an LXC/LXD/Incus container (getty, init
/// output; log history first). Route: `/containers/:id/console`.
#[component]
pub fn ContainerSystemConsolePage() -> impl IntoView {
    view! { <ContainerConsole mode=ContainerWsMode::Console/> }
}

/// What the page knows about the container before it connects.
#[derive(Debug, Clone, PartialEq)]
enum CtState {
    Loading,
    /// No such container on this node.
    Missing,
    /// Exists but is not running (status as the API reports it).
    NotRunning(String),
    Running,
    /// The list could not be read (e.g. a runtime is missing): just try.
    Unknown,
}

/// A terminal or console needs a running container; logs work either way.
fn needs_running(mode: ContainerWsMode) -> bool {
    mode != ContainerWsMode::Logs
}

#[component]
fn ContainerConsole(mode: ContainerWsMode) -> impl IntoView {
    let params = leptos_router::use_params_map();
    let container_id = move || params.get().get("id").cloned().unwrap_or_default();

    // The container list links here with ?runtime=<runtime>. Start from it
    // (not "docker") so the console page's first connect already uses the
    // right runtime instead of racing a refused docker ticket.
    let query = leptos_router::use_query_map();
    let (runtime, set_runtime) = create_signal(
        query
            .get_untracked()
            .get("runtime")
            .cloned()
            .unwrap_or_else(|| "podman".to_string()),
    );
    create_effect(move |_| {
        if let Some(rt) = query.get().get("runtime") {
            // Unchanged: don't re-trigger the console's auto-connect.
            if runtime.get_untracked() != *rt {
                set_runtime.set(rt.clone());
            }
        }
    });
    let (exec_command, set_exec_command) = create_signal("/bin/sh".to_string());
    let (output, set_output) = create_signal(String::new());
    let (connected, set_connected) = create_signal(false);
    let (input_line, set_input_line) = create_signal(String::new());
    let (ws_handle, set_ws_handle) = create_signal(None::<WebSocket>);
    let (ct_state, set_ct_state) = create_signal(CtState::Loading);
    let (error, set_error) = create_signal(None::<String>);
    let (starting, set_starting) = create_signal(false);

    // Look the container up first: a stopped container's console used to try
    // to connect anyway and print the raw refusal, with no way to start it.
    let lookup = move || {
        let id = container_id();
        spawn_local(async move {
            match api::get_containers().await {
                Ok(list) => match list
                    .into_iter()
                    .find(|c| c.id == id || c.vmid == id || c.name == id)
                {
                    Some(c) => {
                        if query.get_untracked().get("runtime").is_none() && !c.runtime.is_empty() {
                            set_runtime.set(c.runtime.to_lowercase());
                        }
                        set_ct_state.set(if c.status.eq_ignore_ascii_case("running") {
                            CtState::Running
                        } else {
                            CtState::NotRunning(c.status)
                        });
                    }
                    None => set_ct_state.set(CtState::Missing),
                },
                Err(_) => set_ct_state.set(CtState::Unknown),
            }
        });
    };
    lookup();

    let connect = move |_: ()| {
        let id = container_id();
        let rt = runtime.get();
        let cmd = exec_command.get();
        set_output.set(String::new());
        if let Some(old) = ws_handle.get_untracked() {
            old.set_onclose(None);
            old.set_onmessage(None);
            let _ = old.close();
        }

        spawn_local(async move {
            let ticket = match mode {
                ContainerWsMode::Logs => api::get_container_logs_ticket(&id, &rt).await,
                ContainerWsMode::Exec => api::get_container_exec_ticket(&id, &cmd, &rt).await,
                ContainerWsMode::Console => api::get_container_console_ticket(&id, &rt).await,
            };
            let ticket = match ticket {
                Ok(t) => {
                    set_error.set(None);
                    t
                }
                Err(e) => {
                    logging::log!("Failed to get ws ticket: {}", e.message);
                    set_error.set(Some(format!("Could not connect: {}", e.message)));
                    return;
                }
            };

            let url = api::resolve_ws_url(&ticket.ws_path);
            let ws = match WebSocket::new(&url) {
                Ok(ws) => ws,
                Err(e) => {
                    logging::log!("Failed to open container WebSocket: {:?}", e);
                    set_error.set(Some("Could not open the connection to the server.".into()));
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
            // A console tty wants Enter (CR); the exec PTY takes either.
            line.push(if mode == ContainerWsMode::Console {
                '\r'
            } else {
                '\n'
            });
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
        ContainerWsMode::Console => "Console",
    };
    // The console page connects on its own (again when the runtime changes);
    // Connect/Reconnect stays available.
    if mode == ContainerWsMode::Console {
        create_effect(move |_| {
            let _ = runtime.get();
            let st = ct_state.get();
            if !container_id().is_empty() && matches!(st, CtState::Running | CtState::Unknown) {
                connect(());
            }
        });
    }

    let can_connect = move || {
        !needs_running(mode) || !matches!(ct_state.get(), CtState::NotRunning(_) | CtState::Loading)
    };

    // Start from here, then connect once it runs (no trip back to the list).
    let start = move |_| {
        let id = container_id();
        set_starting.set(true);
        set_error.set(None);
        spawn_local(async move {
            if let Err(e) = api::start_container(&id).await {
                set_error.set(Some(format!(
                    "Could not start the container: {}",
                    e.message
                )));
                set_starting.set(false);
                return;
            }
            for _ in 0..30 {
                if let Ok(list) = api::get_containers().await {
                    if let Some(c) = list
                        .iter()
                        .find(|c| c.id == id || c.vmid == id || c.name == id)
                    {
                        if c.status.eq_ignore_ascii_case("running") {
                            set_ct_state.set(CtState::Running);
                            set_starting.set(false);
                            if mode == ContainerWsMode::Exec {
                                connect(());
                            }
                            return;
                        }
                    }
                }
                gloo_timers::future::TimeoutFuture::new(1000).await;
            }
            set_starting.set(false);
            set_error.set(Some(
                "The container did not reach the running state in 30 s.".into(),
            ));
            lookup();
        });
    };

    view! {
        <div class="container-list">
            <div class="page-header">
                <h1>{title} " — " {container_id}</h1>
                <div class="header-actions">
                    <select id="console-runtime" prop:value=runtime on:change=move |ev| set_runtime.set(event_target_value(&ev))>
                        <option value="podman" selected=move || runtime.get() == "podman">"Podman"</option>
                        <option value="docker" selected=move || runtime.get() == "docker">"Docker"</option>
                        <option value="lxc" selected=move || runtime.get() == "lxc">"LXC"</option>
                        <option value="lxd" selected=move || runtime.get() == "lxd">"LXD"</option>
                        <option value="incus" selected=move || runtime.get() == "incus">"Incus"</option>
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
                    <button id="console-connect" class="btn btn-primary" disabled=move || !can_connect() on:click=move |_| connect(())>
                        {move || if connected.get() { "Reconnect" } else { "Connect" }}
                    </button>
                    <span
                        id="console-status"
                        class=move || format!("console-status {}", if connected.get() { "connected" } else { "disconnected" })
                    >
                        {move || if connected.get() { "● connected" } else { "○ disconnected" }}
                    </span>
                </div>
            </div>

            {move || error.get().map(|e| view! { <div class="alert alert-error" id="console-error" role="alert">{e}</div> })}
            {move || match ct_state.get() {
                CtState::Missing => view! {
                    <div class="empty-state" id="console-missing">
                        <p>{format!("There is no container \"{}\" on this node.", container_id())}</p>
                        <a href="/containers" class="btn btn-primary">"Show all containers"</a>
                    </div>
                }.into_view(),
                CtState::NotRunning(st) if needs_running(mode) => view! {
                    <div class="empty-state" id="console-stopped">
                        <p>{format!("This container is {st}. Start it to open the {}.", if mode == ContainerWsMode::Exec { "terminal" } else { "console" })}</p>
                        <button id="console-start" class="btn btn-primary" disabled=starting on:click=start>
                            {move || if starting.get() { "Starting..." } else { "Start container" }}
                        </button>
                    </div>
                }.into_view(),
                _ => view! {}.into_view(),
            }}
            <pre id="console-output" class="console-output">
                {move || output.get()}
            </pre>

            {move || {
                if mode != ContainerWsMode::Logs {
                    view! {
                        <div class="form-section">
                            <input
                                id="console-input"
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
