//! Live Docker/Podman events feed (container start/stop/die, image pull, ...).
use crate::api;
use leptos::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{MessageEvent, WebSocket};

/// One row of the events table, parsed from a daemon event JSON line
/// (bollard `EventMessage`: `Type`, `Action`, `Actor.ID`,
/// `Actor.Attributes.name`, `time`).
#[derive(Debug, Clone, PartialEq)]
pub struct EventRow {
    pub time: String,
    pub kind: String,
    pub action: String,
    pub name: String,
}

fn field<'a>(v: &'a serde_json::Value, names: &[&str]) -> Option<&'a serde_json::Value> {
    names.iter().find_map(|n| v.get(*n))
}

pub fn parse_event(line: &str) -> EventRow {
    let v: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => {
            return EventRow {
                time: String::new(),
                kind: String::new(),
                action: line.to_string(),
                name: String::new(),
            }
        }
    };
    let s =
        |x: Option<&serde_json::Value>| x.and_then(|x| x.as_str()).unwrap_or_default().to_string();
    // Docker: epoch seconds; `podman events --format json`: RFC 3339 text.
    let time = match field(&v, &["time", "Time"]) {
        Some(t) if t.is_i64() => crate::utils::format_unix_secs(t.as_i64().unwrap_or_default()),
        Some(t) => t
            .as_str()
            .map(crate::utils::format_rfc3339)
            .unwrap_or_default(),
        None => String::new(),
    };
    let actor = field(&v, &["Actor", "actor"]);
    let attrs = actor.and_then(|a| field(a, &["Attributes", "attributes"]));
    let mut name = s(attrs.and_then(|a| a.get("name")));
    if name.is_empty() {
        name = s(field(&v, &["Name"]));
    }
    if name.is_empty() {
        name = s(actor
            .and_then(|a| field(a, &["ID", "id"]))
            .or_else(|| field(&v, &["ID"])));
        name.truncate(12);
    }
    let mut action = s(field(&v, &["Action", "action"]));
    if action.is_empty() {
        action = s(field(&v, &["Status"]));
    }
    EventRow {
        time,
        kind: s(field(&v, &["Type", "type", "typ"])),
        action,
        name,
    }
}

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

            <table class="container-table" id="events-table">
                <thead>
                    <tr>
                        <th>"Time"</th>
                        <th>"Type"</th>
                        <th>"Action"</th>
                        <th>"Name / ID"</th>
                    </tr>
                </thead>
                <tbody>
                    {move || {
                        events
                            .get()
                            .iter()
                            .rev()
                            .map(|line| {
                                let r = parse_event(line);
                                view! {
                                    <tr title=line.clone()>
                                        <td>{r.time}</td>
                                        <td>{r.kind}</td>
                                        <td><strong>{r.action}</strong></td>
                                        <td><code>{r.name}</code></td>
                                    </tr>
                                }
                            })
                            .collect_view()
                    }}
                </tbody>
            </table>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_daemon_event_json_into_a_readable_row() {
        let line = r#"{"Type":"container","Action":"start","Actor":{"ID":"0123456789abcdef","Attributes":{"name":"qa-events-6","image":"nginx:alpine"}},"scope":"local","time":1792162846,"timeNano":1792162846000000000}"#;
        assert_eq!(
            parse_event(line),
            EventRow {
                time: "2026-10-16 15:00:46 UTC".into(),
                kind: "container".into(),
                action: "start".into(),
                name: "qa-events-6".into(),
            }
        );
    }

    #[test]
    fn falls_back_to_short_id_and_raw_text() {
        let r = parse_event(
            r#"{"Type":"network","Action":"connect","Actor":{"ID":"0123456789abcdef"}}"#,
        );
        assert_eq!(r.name, "0123456789ab");
        assert_eq!(parse_event("not json").action, "not json");
        // podman events --format json
        let p = parse_event(
            r#"{"ID":"abc","Image":"busybox","Name":"pod-ct","Status":"start","Time":"2026-10-16T15:00:46Z","Type":"container"}"#,
        );
        assert_eq!((p.action.as_str(), p.name.as_str()), ("start", "pod-ct"));
        assert_eq!(p.time, "2026-10-16 15:00:46 UTC");
    }
}
