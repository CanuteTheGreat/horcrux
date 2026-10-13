//! Live Docker/Podman events feed (container start/stop/die, image pull, ...).
use crate::api;
use leptos::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{MessageEvent, WebSocket};

#[component]
pub fn ContainerEventsPage() -> impl IntoView {
    let (runtime, set_runtime) = create_signal("docker".to_string());
    let (events, set_events) = create_signal(Vec::<String>::new());
    let (connected, set_connected) = create_signal(false);

    let connect = move |_| {
        let rt = runtime.get();
        set_events.set(Vec::new());
        spawn_local(async move {
            let ticket = match api::get_container_events_ticket(&rt).await {
                Ok(t) => t,
                Err(e) => {
                    logging::log!("Failed to get events ticket: {}", e.message);
                    return;
                }
            };
            let url = api::resolve_ws_url(&ticket.ws_path);
            let ws = match WebSocket::new(&url) {
                Ok(ws) => ws,
                Err(e) => {
                    logging::log!("Failed to open events WebSocket: {:?}", e);
                    return;
                }
            };

            let onopen = Closure::wrap(Box::new(move |_: JsValue| {
                set_connected.set(true);
            }) as Box<dyn FnMut(JsValue)>);
            ws.set_onopen(Some(onopen.as_ref().unchecked_ref()));
            onopen.forget();

            let onmessage = Closure::wrap(Box::new(move |e: MessageEvent| {
                if let Ok(txt) = e.data().dyn_into::<js_sys::JsString>() {
                    let line = String::from(txt);
                    set_events.update(|list| {
                        list.push(line);
                        if list.len() > 500 {
                            list.remove(0);
                        }
                    });
                }
            }) as Box<dyn FnMut(MessageEvent)>);
            ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
            onmessage.forget();

            let onclose = Closure::wrap(Box::new(move |_: JsValue| {
                set_connected.set(false);
            }) as Box<dyn FnMut(JsValue)>);
            ws.set_onclose(Some(onclose.as_ref().unchecked_ref()));
            onclose.forget();
        });
    };

    view! {
        <div class="container-list">
            <div class="page-header">
                <h1>"Events"</h1>
                <div class="header-actions">
                    <select on:change=move |ev| set_runtime.set(event_target_value(&ev))>
                        <option value="docker">"Docker"</option>
                        <option value="podman">"Podman"</option>
                    </select>
                    <button class="btn btn-primary" on:click=connect>"Connect"</button>
                    <span>{move || if connected.get() { "● live" } else { "○ disconnected" }}</span>
                </div>
            </div>

            <pre style="background:#111;color:#9cf;padding:1em;height:500px;overflow:auto;white-space:pre-wrap;">
                {move || events.get().join("\n")}
            </pre>
        </div>
    }
}
