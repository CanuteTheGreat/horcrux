//! Node shell (`/system/shell`): a login shell on this node over a WebSocket
//! (needs Sys.Modify). Line-oriented: type a command and press Enter; the
//! control buttons send Ctrl-C / Ctrl-D / Tab for interrupting and completing.
use crate::api;
use leptos::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{MessageEvent, WebSocket};

#[component]
pub fn NodeShellPage() -> impl IntoView {
    let (output, set_output) = create_signal(String::new());
    let (connected, set_connected) = create_signal(false);
    let (input_line, set_input_line) = create_signal(String::new());
    let (ws_handle, set_ws_handle) = create_signal(None::<WebSocket>);

    let connect = move |_: ()| {
        set_output.set(String::new());
        if let Some(old) = ws_handle.get_untracked() {
            old.set_onclose(None);
            old.set_onmessage(None);
            let _ = old.close();
        }
        spawn_local(async move {
            let ticket = match api::get_node_shell_ticket().await {
                Ok(t) => t,
                Err(e) => {
                    set_output.set(format!("[error] {}\n", e.message));
                    return;
                }
            };
            let ws = match WebSocket::new(&api::resolve_ws_url(&ticket.ws_path)) {
                Ok(ws) => ws,
                Err(e) => {
                    set_output.set(format!("[cannot open the WebSocket: {:?}]\n", e));
                    return;
                }
            };
            ws.set_binary_type(web_sys::BinaryType::Arraybuffer);
            let ws_open = ws.clone();
            let onopen = Closure::wrap(Box::new(move |_: JsValue| {
                set_connected.set(true);
                let _ = ws_open.send_with_str("{\"resize\":[120,32]}");
            }) as Box<dyn FnMut(JsValue)>);
            ws.set_onopen(Some(onopen.as_ref().unchecked_ref()));
            onopen.forget();
            let onmessage = Closure::wrap(Box::new(move |e: MessageEvent| {
                let data = e.data();
                let chunk = if let Some(txt) = data.as_string() {
                    txt
                } else if let Ok(buf) = data.dyn_into::<js_sys::ArrayBuffer>() {
                    String::from_utf8_lossy(&js_sys::Uint8Array::new(&buf).to_vec()).to_string()
                } else {
                    return;
                };
                let chunk = crate::pages::lxc::spec::strip_ansi(&chunk).replace('\r', "");
                set_output.update(|o| {
                    o.push_str(&chunk);
                    // keep the page light: the last ~200 KB
                    if o.len() > 200_000 {
                        let cut = o.len() - 150_000;
                        let cut = (cut..o.len()).find(|i| o.is_char_boundary(*i)).unwrap_or(0);
                        o.drain(..cut);
                    }
                });
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
    create_effect(move |_| connect(()));

    let send_raw = move |s: &str| {
        if let Some(ws) = ws_handle.get_untracked() {
            let _ = ws.send_with_str(s);
        }
    };
    let do_send = move || {
        let mut line = input_line.get_untracked();
        line.push('\r');
        send_raw(&line);
        set_input_line.set(String::new());
    };

    on_cleanup(move || {
        if let Some(ws) = ws_handle.get_untracked() {
            let _ = ws.close();
        }
    });

    view! {
        <div class="node-shell">
            <div class="page-header">
                <h1>"Node Shell"</h1>
                <div class="header-actions">
                    <button id="shell-connect" class="btn btn-primary" on:click=move |_| connect(())>
                        {move || if connected.get() { "Reconnect" } else { "Connect" }}
                    </button>
                    <span>{move || if connected.get() { "● connected" } else { "○ disconnected" }}</span>
                </div>
            </div>
            <p class="form-help">
                "A login shell on this node, run as the Horcrux service user. Sessions are recorded in the Tasks log. Full-screen programs (vim, top) need a real terminal: use SSH for those."
            </p>
            <pre id="shell-output" style="background:#111;color:#0f0;padding:1em;height:420px;overflow:auto;white-space:pre-wrap;">
                {move || output.get()}
            </pre>
            <div class="form-section">
                <input
                    id="shell-input"
                    type="text"
                    style="width:70%"
                    placeholder="command, then Enter"
                    prop:value=input_line
                    on:input=move |ev| set_input_line.set(event_target_value(&ev))
                    on:keydown=move |ev| {
                        if ev.key() == "Enter" {
                            do_send();
                        }
                    }
                />
                <button class="btn btn-sm btn-primary" on:click=move |_| do_send()>"Send"</button>
                <button class="btn btn-sm" on:click=move |_| send_raw("\u{3}")>"Ctrl-C"</button>
                <button class="btn btn-sm" on:click=move |_| send_raw("\u{4}")>"Ctrl-D"</button>
                <button class="btn btn-sm" on:click=move |_| send_raw("\t")>"Tab"</button>
                <button class="btn btn-sm" on:click=move |_| set_output.set(String::new())>"Clear"</button>
            </div>
        </div>
    }
}
