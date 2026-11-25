//! System Log (`/system/logs`): journald or syslog-file entries of this node
//! or another cluster node, colour-coded by priority, with a time span,
//! minimum priority, unit, syslog identifier (`*` wildcard), kernel-only and
//! boot filters, reboot separators and a live mode over a WebSocket.
//!
//! The filters can be preset in the address: `/system/logs?unit=ssh&priority=3`.
use crate::api::{self, LogBoot, LogEntry, LogFilters, LogInfo};
use leptos::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{MessageEvent, WebSocket};

/// Time spans of the picker: (value, label, relative `since`).
const SPANS: [(&str, &str, &str); 7] = [
    ("all", "Everything", ""),
    ("15m", "Last 15 minutes", "15m"),
    ("1h", "Last hour", "1h"),
    ("6h", "Last 6 hours", "6h"),
    ("24h", "Last 24 hours", "24h"),
    ("7d", "Last 7 days", "7d"),
    ("custom", "Custom range...", ""),
];

/// Minimum-priority choices: (value, label).
const PRIORITIES: [(&str, &str); 9] = [
    ("", "Any priority"),
    ("7", "Debug (7) and worse"),
    ("6", "Info (6) and worse"),
    ("5", "Notice (5) and worse"),
    ("4", "Warning (4) and worse"),
    ("3", "Error (3) and worse"),
    ("2", "Critical (2) and worse"),
    ("1", "Alert (1) and worse"),
    ("0", "Emergency (0)"),
];

/// Everything the filter bar holds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bar {
    pub span: String,
    pub since: String,
    pub until: String,
    pub priority: String,
    pub unit: String,
    pub identifier: String,
    pub kernel: bool,
    pub boot: String,
    pub node: String,
    pub search: String,
    pub limit: u32,
}

impl Bar {
    pub fn filters(&self, with_limit: bool) -> LogFilters {
        let opt = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());
        let (since, until) = if self.span == "custom" {
            (opt(&self.since), opt(&self.until))
        } else {
            let rel = SPANS
                .iter()
                .find(|(v, _, _)| *v == self.span)
                .map(|(_, _, r)| *r)
                .unwrap_or("");
            (opt(rel), None)
        };
        LogFilters {
            search: opt(&self.search),
            limit: with_limit.then_some(self.limit),
            since,
            until,
            priority: self.priority.trim().parse::<u8>().ok(),
            unit: opt(&self.unit),
            identifier: opt(&self.identifier),
            kernel: self.kernel,
            boot: opt(&self.boot),
            node: opt(&self.node),
            ..Default::default()
        }
    }
}

/// CSS class of a priority (0 emerg .. 7 debug).
pub fn priority_class(e: &LogEntry) -> &'static str {
    match e.priority {
        Some(0) => "prio-0",
        Some(1) => "prio-1",
        Some(2) => "prio-2",
        Some(3) => "prio-3",
        Some(4) => "prio-4",
        Some(5) => "prio-5",
        Some(7) => "prio-7",
        Some(_) => "prio-6",
        None => match e.level.as_str() {
            "error" => "prio-3",
            "warn" => "prio-4",
            "debug" => "prio-7",
            _ => "prio-6",
        },
    }
}

pub fn priority_label(e: &LogEntry) -> String {
    let name = e.priority_name.clone().unwrap_or_else(|| e.level.clone());
    // a guessed priority is marked, so nobody mistakes it for a logged one
    if e.priority_inferred {
        format!("{name}?")
    } else {
        name
    }
}

/// `2026-11-25T10:00:01.123Z` -> `2026-11-25 10:00:01`
pub fn short_time(ts: &str) -> String {
    ts.chars().take(19).collect::<String>().replace('T', " ")
}

/// Rows of the table: entries with a separator wherever the boot changes
/// between two neighbours (the list is newest first, so the separator
/// stands between the newer boot's first line and the older boot's last).
#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    Entry(LogEntry),
    Reboot(String),
}

pub fn with_separators(entries: &[LogEntry], boots: &[LogBoot]) -> Vec<Row> {
    let mut rows = Vec::with_capacity(entries.len() + 4);
    for (i, e) in entries.iter().enumerate() {
        if i > 0 {
            let newer = &entries[i - 1];
            if let (Some(a), Some(b)) = (&newer.boot_id, &e.boot_id) {
                if a != b {
                    let started = boots
                        .iter()
                        .find(|x| &x.boot_id == a)
                        .and_then(|x| x.first_entry.clone())
                        .map(|t| short_time(&t))
                        .unwrap_or_else(|| short_time(&newer.timestamp));
                    rows.push(Row::Reboot(format!(
                        "Rebooted - boot started {started} (older boot below)"
                    )));
                }
            }
        }
        rows.push(Row::Entry(e.clone()));
    }
    rows
}

fn boot_label(b: &LogBoot) -> String {
    let when = b
        .first_entry
        .as_deref()
        .map(short_time)
        .unwrap_or_else(|| b.boot_id.clone());
    match b.index {
        0 => format!("Current boot ({when})"),
        -1 => format!("Previous boot ({when})"),
        n => format!("Boot {n} ({when})"),
    }
}

fn load_query_into(bar: &mut Bar, q: &leptos_router::ParamsMap) {
    let get = |k: &str| q.get(k).cloned().unwrap_or_default();
    bar.unit = get("unit");
    bar.identifier = get("identifier");
    bar.search = get("search");
    bar.node = get("node");
    bar.boot = get("boot");
    bar.priority = get("priority");
    bar.kernel = matches!(get("kernel").as_str(), "1" | "true");
    if !get("since").is_empty() {
        bar.span = "custom".into();
        bar.since = get("since");
        bar.until = get("until");
    }
}

#[component]
pub fn SystemLogsPage() -> impl IntoView {
    let mut initial = Bar {
        span: "all".into(),
        limit: 200,
        ..Default::default()
    };
    load_query_into(
        &mut initial,
        &leptos_router::use_query_map().get_untracked(),
    );
    let bar = create_rw_signal(initial);
    let (logs, set_logs) = create_signal(Vec::<LogEntry>::new());
    let (boots, set_boots) = create_signal(Vec::<LogBoot>::new());
    let (units, set_units) = create_signal(Vec::<String>::new());
    let (info, set_info) = create_signal(None::<LogInfo>);
    let (nodes, set_nodes) = create_signal(Vec::<String>::new());
    let (loading, set_loading) = create_signal(false);
    let (error_message, set_error_message) = create_signal(None::<String>);
    let (unavailable, set_unavailable) = create_signal(None::<api::Unavailable>);
    let (live, set_live) = create_signal(false);
    let (live_note, set_live_note) = create_signal(String::new());
    let ws_handle = create_rw_signal(None::<WebSocket>);
    let settings = create_rw_signal(None::<horcrux_common::system_logs::LogSettings>);
    let (settings_msg, set_settings_msg) = create_signal(String::new());

    let node_arg = move || Some(bar.get_untracked().node).filter(|n| !n.is_empty());

    // The pickers that depend on the node: boots, units, what the source can do.
    let load_side = create_action(move |_: &()| async move {
        let n = node_arg();
        if let Ok(b) = api::get_log_boots(n.as_deref()).await {
            set_boots.set(b);
        }
        if let Ok(u) = api::get_log_units(n.as_deref()).await {
            set_units.set(u.units);
        }
        set_info.set(api::get_log_info(n.as_deref()).await.ok());
    });

    let load_logs = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);
        let filters = bar.get_untracked().filters(true);
        match api::get_system_logs(filters).await {
            Ok(entries) => {
                set_unavailable.set(None);
                set_logs.set(entries)
            }
            Err(e) => match e.unavailable() {
                Some(u) => set_unavailable.set(Some(u)),
                None => set_error_message.set(Some(format!("Failed to load system logs: {}", e))),
            },
        }
        set_loading.set(false);
    });

    let stop_live = move || {
        if let Some(ws) = ws_handle.get_untracked() {
            ws.set_onclose(None);
            ws.set_onmessage(None);
            let _ = ws.close();
        }
        ws_handle.set(None);
        set_live.set(false);
    };

    let start_live = move || {
        if let Some(ws) = ws_handle.get_untracked() {
            ws.set_onclose(None);
            ws.set_onmessage(None);
            let _ = ws.close();
        }
        set_live_note.set("Connecting...".into());
        set_live.set(true);
        spawn_local(async move {
            let ticket = match api::get_log_stream_ticket().await {
                Ok(t) => t,
                Err(e) => {
                    set_live_note.set(format!("Live mode unavailable: {}", e.message));
                    set_live.set(false);
                    return;
                }
            };
            // the stream wants the filters, not the span or the line count
            let mut f = bar.get_untracked().filters(false);
            f.since = None;
            f.until = None;
            f.limit = None;
            let q = f.query();
            let path = if q.is_empty() {
                ticket.ws_path.clone()
            } else {
                format!("{}?{}", ticket.ws_path, q)
            };
            let ws = match WebSocket::new(&api::resolve_ws_url(&path)) {
                Ok(ws) => ws,
                Err(_) => {
                    set_live_note.set("Live mode: cannot open the connection".into());
                    set_live.set(false);
                    return;
                }
            };
            let onmessage = Closure::wrap(Box::new(move |e: MessageEvent| {
                let Some(text) = e.data().as_string() else {
                    return;
                };
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
                    return;
                };
                match v.get("type").and_then(|t| t.as_str()) {
                    Some("hello") => set_live_note.set("Live: waiting for new messages".into()),
                    Some("entries") => {
                        if let Some(arr) = v.get("entries") {
                            if let Ok(new) = serde_json::from_value::<Vec<LogEntry>>(arr.clone()) {
                                let cap = 5000usize;
                                set_logs.update(|l| {
                                    // newest first: the new ones go on top
                                    let mut merged: Vec<LogEntry> = new.into_iter().rev().collect();
                                    merged.append(l);
                                    merged.truncate(cap);
                                    *l = merged;
                                });
                                set_live_note.set("Live: receiving".into());
                            }
                        }
                    }
                    Some("closed") => {
                        let why = match v.get("reason").and_then(|r| r.as_str()) {
                            Some("idle") => "no new messages for a while".to_string(),
                            Some("max-time") => "the stream reached its time limit".to_string(),
                            Some("max-entries") => {
                                "the stream reached its message limit".to_string()
                            }
                            _ => v
                                .get("message")
                                .and_then(|m| m.as_str())
                                .unwrap_or("the server ended it")
                                .to_string(),
                        };
                        set_live_note
                            .set(format!("Live stopped: {why}. Switch Live on to resume."));
                        set_live.set(false);
                    }
                    _ => {}
                }
            }) as Box<dyn FnMut(MessageEvent)>);
            ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
            onmessage.forget();
            let onclose = Closure::wrap(Box::new(move |_: JsValue| {
                if live.get_untracked() {
                    set_live_note.set("Live stopped: the connection closed.".into());
                    set_live.set(false);
                }
            }) as Box<dyn FnMut(JsValue)>);
            ws.set_onclose(Some(onclose.as_ref().unchecked_ref()));
            onclose.forget();
            ws_handle.set(Some(ws));
        });
    };
    on_cleanup(move || {
        if let Some(ws) = ws_handle.get_untracked() {
            ws.set_onclose(None);
            let _ = ws.close();
        }
    });

    // A changed filter reloads the list and restarts the live stream.
    let apply = move || {
        load_logs.dispatch(());
        if live.get_untracked() {
            start_live();
        }
    };
    let node_changed = move || {
        bar.update(|b| {
            b.boot.clear();
            b.unit.clear();
        });
        load_side.dispatch(());
        apply();
    };

    create_effect(move |_| {
        load_side.dispatch(());
        load_logs.dispatch(());
        spawn_local(async move {
            if let Ok(n) = api::get_cluster_nodes().await {
                set_nodes.set(n.into_iter().map(|x| x.name).collect());
            }
            if let Ok(s) = api::get_log_settings().await {
                settings.set(Some(s));
            }
        });
    });

    let clear_logs_action = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);
        match api::clear_system_logs().await {
            Ok(_) => {
                set_logs.set(Vec::new());
                load_logs.dispatch(());
            }
            Err(e) => set_error_message.set(Some(format!("Failed to clear logs: {}", e))),
        }
        set_loading.set(false);
    });

    let export_logs_action = create_action(move |format: &String| {
        let format = format.clone();
        async move {
            set_loading.set(true);
            set_error_message.set(None);
            let filters = bar.get_untracked().filters(false);
            let ext = if format == "csv" { "csv" } else { "json" };
            match api::export_system_logs(filters, format).await {
                Ok(data) => {
                    let window = web_sys::window().unwrap();
                    let document = window.document().unwrap();
                    let a = document
                        .create_element("a")
                        .unwrap()
                        .dyn_into::<web_sys::HtmlAnchorElement>()
                        .unwrap();
                    let parts = js_sys::Array::new();
                    parts.push(&JsValue::from_str(&data));
                    let blob = web_sys::Blob::new_with_str_sequence(&parts).unwrap();
                    let url = web_sys::Url::create_object_url_with_blob(&blob).unwrap();
                    a.set_href(&url);
                    a.set_download(&format!("system-logs.{ext}"));
                    a.click();
                    web_sys::Url::revoke_object_url(&url).unwrap();
                }
                Err(e) => set_error_message.set(Some(format!("Failed to export logs: {}", e))),
            }
            set_loading.set(false);
        }
    });

    let save_settings = create_action(move |_: &()| async move {
        let Some(s) = settings.get_untracked() else {
            return;
        };
        match api::put_log_settings(&s).await {
            Ok(saved) => {
                settings.set(Some(saved));
                set_settings_msg.set("Saved.".into());
                load_side.dispatch(());
            }
            Err(e) => set_settings_msg.set(format!("Not saved: {}", e.message)),
        }
    });

    let set_num = move |f: fn(&mut horcrux_common::system_logs::LogSettings, u64), v: String| {
        if let Ok(n) = v.trim().parse::<u64>() {
            settings.update(|s| {
                if let Some(s) = s {
                    f(s, n)
                }
            });
        }
    };

    view! {
        <div class="system-logs-page">
            <div class="page-header">
                <h1>"System Log"</h1>
                <div class="header-actions">
                    <button class="btn btn-secondary" on:click=move |_| apply() disabled=loading>"Refresh"</button>
                    <button class="btn btn-secondary" on:click=move |_| export_logs_action.dispatch("json".to_string()) disabled=loading>"Export JSON"</button>
                    <button class="btn btn-secondary" on:click=move |_| export_logs_action.dispatch("csv".to_string()) disabled=loading>"Export CSV"</button>
                    <button
                        class="btn btn-danger"
                        on:click=move |_| {
                            if web_sys::window()
                                .unwrap()
                                .confirm_with_message("Clear the system log on this node? This cannot be undone.")
                                .unwrap_or(false)
                            {
                                clear_logs_action.dispatch(());
                            }
                        }
                        disabled=loading
                    >"Clear Log"</button>
                </div>
            </div>

            {move || error_message.get().map(|msg| view! { <div class="alert alert-error">{msg}</div> })}
            {move || unavailable.get().map(|u| view! {
                <crate::components::UnavailableState info=u on_retry=Callback::new(move |_| apply())/>
            })}

            {move || info.get().filter(|i| !i.limitations.is_empty()).map(|i| view! {
                <div class="note log-limits" role="note">
                    <strong>{format!("Log source on {}: {}", i.node, i.source)}</strong>
                    <ul>{i.limitations.iter().map(|l| view! { <li>{l.clone()}</li> }).collect::<Vec<_>>()}</ul>
                </div>
            })}

            <div class="logs-controls" style:display=move || if unavailable.get().is_some() { "none" } else { "" }>
                <div class="controls-row log-filters">
                    {move || (nodes.get().len() > 1).then(|| view! {
                        <label class="filter-group">"Node"
                            <select id="log-node" prop:value=move || bar.get().node
                                on:change=move |ev| { bar.update(|b| b.node = event_target_value(&ev)); node_changed(); }>
                                <option value="">"This node"</option>
                                {nodes.get().into_iter().map(|n| view! { <option value=n.clone()>{n.clone()}</option> }).collect::<Vec<_>>()}
                            </select>
                        </label>
                    })}
                    <label class="filter-group">"Time span"
                        <select id="log-span" prop:value=move || bar.get().span
                            on:change=move |ev| { bar.update(|b| b.span = event_target_value(&ev)); apply(); }>
                            {SPANS.iter().map(|(v, l, _)| view! { <option value=*v>{*l}</option> }).collect::<Vec<_>>()}
                        </select>
                    </label>
                    {move || (bar.get().span == "custom").then(|| view! {
                        <label class="filter-group">"Since"
                            <input id="log-since" type="text" placeholder="2026-11-25 10:00 or 2h"
                                prop:value=move || bar.get().since
                                on:change=move |ev| { bar.update(|b| b.since = event_target_value(&ev)); apply(); }/>
                        </label>
                        <label class="filter-group">"Until"
                            <input id="log-until" type="text" placeholder="now"
                                prop:value=move || bar.get().until
                                on:change=move |ev| { bar.update(|b| b.until = event_target_value(&ev)); apply(); }/>
                        </label>
                    })}
                    <label class="filter-group">"Minimum priority"
                        <select id="log-priority" prop:value=move || bar.get().priority
                            on:change=move |ev| { bar.update(|b| b.priority = event_target_value(&ev)); apply(); }>
                            {PRIORITIES.iter().map(|(v, l)| view! { <option value=*v>{*l}</option> }).collect::<Vec<_>>()}
                        </select>
                    </label>
                    <label class="filter-group">"Unit / service"
                        <input id="log-unit" type="text" list="log-units" placeholder="ssh"
                            prop:value=move || bar.get().unit
                            on:change=move |ev| { bar.update(|b| b.unit = event_target_value(&ev)); apply(); }/>
                        <datalist id="log-units">
                            {move || units.get().into_iter().map(|u| view! { <option value=u.clone()></option> }).collect::<Vec<_>>()}
                        </datalist>
                    </label>
                    <label class="filter-group">"Syslog identifier"
                        <input id="log-identifier" type="text" placeholder="sshd or ssh*"
                            title="* matches any run of characters, e.g. ssh* finds sshd and sshd-session"
                            prop:value=move || bar.get().identifier
                            on:change=move |ev| { bar.update(|b| b.identifier = event_target_value(&ev)); apply(); }/>
                        <small class="field-hint">"* matches any text"</small>
                    </label>
                    <label class="filter-group">"Boot"
                        <select id="log-boot" prop:value=move || bar.get().boot
                            on:change=move |ev| { bar.update(|b| b.boot = event_target_value(&ev)); apply(); }>
                            <option value="">"All boots"</option>
                            {move || boots.get().into_iter().rev().map(|b| {
                                let v = b.index.to_string();
                                view! { <option value=v>{boot_label(&b)}</option> }
                            }).collect::<Vec<_>>()}
                        </select>
                    </label>
                    <label class="control-label">
                        <input id="log-kernel" type="checkbox" prop:checked=move || bar.get().kernel
                            on:change=move |ev| { bar.update(|b| b.kernel = event_target_checked(&ev)); apply(); }/>
                        " Kernel messages only"
                    </label>
                </div>
                <div class="controls-row">
                    <label class="filter-group">"Search"
                        <input id="log-search" type="text" class="search-input" placeholder="text in the message"
                            prop:value=move || bar.get().search
                            on:change=move |ev| { bar.update(|b| b.search = event_target_value(&ev)); apply(); }/>
                    </label>
                    <label class="filter-group">"Lines"
                        <select id="log-lines" prop:value=move || bar.get().limit.to_string()
                            on:change=move |ev| {
                                if let Ok(n) = event_target_value(&ev).parse::<u32>() {
                                    bar.update(|b| b.limit = n);
                                    apply();
                                }
                            }>
                            <option value="50">"50"</option>
                            <option value="200">"200"</option>
                            <option value="500">"500"</option>
                            <option value="1000">"1000"</option>
                            <option value="5000">"5000"</option>
                        </select>
                    </label>
                    <label class="control-label">
                        <input id="log-live" type="checkbox" prop:checked=live
                            on:change=move |ev| { if event_target_checked(&ev) { start_live() } else { stop_live() } }/>
                        " Live"
                    </label>
                    <span id="log-live-note" class="live-note" role="status">{move || live_note.get()}</span>
                </div>
            </div>

            {move || if unavailable.get().is_some() {
                ().into_view()
            } else if loading.get() && logs.get().is_empty() {
                view! { <div class="loading">"Loading system log..."</div> }.into_view()
            } else if logs.get().is_empty() {
                view! { <div class="empty-state">
                    <p>"No log entries match the current filters."</p>
                    <p class="empty-todo">"Widen the time span, lower the minimum priority or clear the unit and identifier."</p>
                </div> }.into_view()
            } else {
                let rows = with_separators(&logs.get(), &boots.get());
                let n = logs.get().len();
                let multi = nodes.get().len() > 1;
                view! {
                    <div class="logs-container">
                        <div class="logs-header"><span class="stat-item">{format!("{n} entries, newest first")}</span></div>
                        <div class="logs-table-container">
                            <table class="logs-table">
                                <thead>
                                    <tr>
                                        <th class="timestamp-col">"Time"</th>
                                        <th class="level-col">"Priority"</th>
                                        {multi.then(|| view! { <th>"Node"</th> })}
                                        <th class="service-col">"Unit / identifier"</th>
                                        <th class="message-col">"Message"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {rows.into_iter().map(|row| match row {
                                        Row::Reboot(text) => view! {
                                            <tr class="boot-sep"><td colspan=if multi { "5" } else { "4" }>{text}</td></tr>
                                        }.into_view(),
                                        Row::Entry(e) => {
                                            let class = format!("log-row {}", priority_class(&e));
                                            let who = match (&e.identifier, e.pid) {
                                                (Some(i), Some(p)) => format!("{i}[{p}]"),
                                                (Some(i), None) => i.clone(),
                                                _ => e.service.clone().unwrap_or_else(|| "-".into()),
                                            };
                                            let unit = e.unit.clone().or(e.details.clone());
                                            let title = e.hostname.clone().unwrap_or_default();
                                            let node = e.node.clone().unwrap_or_default();
                                            view! {
                                                <tr class=class title=title>
                                                    <td class="log-timestamp">{short_time(&e.timestamp)}</td>
                                                    <td class="log-level"><span class="level-badge">{priority_label(&e)}</span></td>
                                                    {multi.then(|| view! { <td>{node.clone()}</td> })}
                                                    <td class="log-service">{who}{unit.map(|u| view! { <div class="log-details">{u}</div> })}</td>
                                                    <td class="log-message"><span class="message-text">{e.message.clone()}</span></td>
                                                </tr>
                                            }.into_view()
                                        }
                                    }).collect::<Vec<_>>()}
                                </tbody>
                            </table>
                        </div>
                    </div>
                }.into_view()
            }}

            <details class="log-settings">
                <summary>"Log viewer limits"</summary>
                {move || settings.get().map(|s| view! {
                    <div class="controls-row">
                        <label class="filter-group">"Most lines per request"
                            <input id="set-max-lines" type="number" min="100" prop:value=s.max_lines.to_string()
                                on:change=move |ev| set_num(|s, n| s.max_lines = n as usize, event_target_value(&ev))/></label>
                        <label class="filter-group">"Default lines"
                            <input id="set-default-lines" type="number" min="1" prop:value=s.default_lines.to_string()
                                on:change=move |ev| set_num(|s, n| s.default_lines = n as usize, event_target_value(&ev))/></label>
                        <label class="filter-group">"Live: close after idle (s)"
                            <input id="set-follow-idle" type="number" min="10" prop:value=s.follow_idle_secs.to_string()
                                on:change=move |ev| set_num(|s, n| s.follow_idle_secs = n, event_target_value(&ev))/></label>
                        <label class="filter-group">"Live: longest stream (s)"
                            <input id="set-follow-max" type="number" min="30" prop:value=s.follow_max_secs.to_string()
                                on:change=move |ev| set_num(|s, n| s.follow_max_secs = n, event_target_value(&ev))/></label>
                        <label class="filter-group">"Live: streams at once"
                            <input id="set-follow-streams" type="number" min="1" prop:value=s.follow_max_streams.to_string()
                                on:change=move |ev| set_num(|s, n| s.follow_max_streams = n as usize, event_target_value(&ev))/></label>
                        <label class="filter-group">"Syslog bytes searched"
                            <input id="set-tail-bytes" type="number" min="65536" prop:value=s.syslog_tail_bytes.to_string()
                                on:change=move |ev| set_num(|s, n| s.syslog_tail_bytes = n, event_target_value(&ev))/></label>
                        <button class="btn btn-secondary" id="set-save" on:click=move |_| save_settings.dispatch(())>"Save limits"</button>
                        <span id="set-msg" role="status">{move || settings_msg.get()}</span>
                    </div>
                })}
            </details>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(boot: &str, ts: &str) -> LogEntry {
        LogEntry {
            timestamp: ts.into(),
            level: "info".into(),
            message: "m".into(),
            boot_id: Some(boot.into()),
            ..Default::default()
        }
    }

    #[test]
    fn a_separator_stands_where_the_boot_changes() {
        let v = vec![
            e("b2", "2026-11-25T10:05:00Z"),
            e("b2", "2026-11-25T10:00:30Z"),
            e("b1", "2026-11-25T09:00:00Z"),
        ];
        let boots = vec![LogBoot {
            index: 0,
            boot_id: "b2".into(),
            first_us: None,
            last_us: None,
            first_entry: Some("2026-11-25T10:00:00Z".into()),
            last_entry: None,
            source: String::new(),
        }];
        let rows = with_separators(&v, &boots);
        assert_eq!(rows.len(), 4);
        assert!(matches!(&rows[2], Row::Reboot(t) if t.contains("2026-11-25 10:00:00")));
        assert!(with_separators(&v[..2], &boots).len() == 2);
        // entries without a boot id never get a separator
        let mut n = v.clone();
        n[2].boot_id = None;
        assert_eq!(with_separators(&n, &boots).len(), 3);
    }

    #[test]
    fn the_filter_bar_becomes_api_filters() {
        let b = Bar {
            span: "1h".into(),
            priority: "3".into(),
            unit: " ssh ".into(),
            identifier: "ssh*".into(),
            kernel: true,
            boot: "-1".into(),
            node: "n2".into(),
            limit: 200,
            ..Default::default()
        };
        let f = b.filters(true);
        assert_eq!(f.since.as_deref(), Some("1h"));
        assert_eq!(f.priority, Some(3));
        assert_eq!(f.unit.as_deref(), Some("ssh"));
        let q = f.query();
        for want in [
            "since=1h",
            "priority=3",
            "unit=ssh",
            "identifier=ssh%2A",
            "kernel=true",
            "boot=-1",
            "node=n2",
            "limit=200",
        ] {
            assert!(q.contains(want), "{want} not in {q}");
        }
        // custom span uses the typed times; "everything" sends none
        let c = Bar {
            span: "custom".into(),
            since: "2026-11-25 10:00".into(),
            until: "2h".into(),
            ..Default::default()
        };
        let q = c.filters(false).query();
        assert!(q.contains("since=2026-11-25%2010%3A00") && q.contains("until=2h"));
        assert!(!q.contains("limit"));
        assert_eq!(
            Bar {
                span: "all".into(),
                ..Default::default()
            }
            .filters(false)
            .query(),
            ""
        );
    }

    #[test]
    fn priorities_get_a_class_and_a_guessed_one_is_marked() {
        let mut x = e("b", "t");
        x.priority = Some(3);
        x.priority_name = Some("err".into());
        assert_eq!(priority_class(&x), "prio-3");
        assert_eq!(priority_label(&x), "err");
        x.priority_inferred = true;
        assert_eq!(priority_label(&x), "err?");
        x.priority = None;
        x.level = "warn".into();
        assert_eq!(priority_class(&x), "prio-4");
        assert_eq!(
            short_time("2026-11-25T10:00:01.123Z"),
            "2026-11-25 10:00:01"
        );
    }
}
