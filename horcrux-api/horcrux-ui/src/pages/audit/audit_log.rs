//! Audit log (`/audit`): who did what on this node, searchable, with the
//! security views (denied / suspicious, failed logins, brute-force suspects)
//! and CSV / JSON export. Backed by the audit logger, whose events are also
//! kept in the audit log file and reloaded when the API restarts.
use crate::api::*;
use leptos::*;
use wasm_bindgen::JsCast;

/// Offer `text` as a download named `name`.
fn download(name: &str, mime: &str, text: &str) {
    let parts = web_sys::js_sys::Array::new();
    parts.push(&wasm_bindgen::JsValue::from_str(text));
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type(mime);
    let Ok(blob) = web_sys::Blob::new_with_str_sequence_and_options(&parts, &opts) else {
        return;
    };
    let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob) else {
        return;
    };
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        if let Ok(a) = doc.create_element("a") {
            if let Ok(a) = a.dyn_into::<web_sys::HtmlAnchorElement>() {
                a.set_href(&url);
                a.set_download(name);
                a.click();
            }
        }
    }
    let _ = web_sys::Url::revoke_object_url(&url);
}

#[component]
pub fn AuditLogPage() -> impl IntoView {
    let (events, set_events) = create_signal(Vec::<AuditEvent>::new());
    let (total, set_total) = create_signal(0usize);
    let (options, set_options) = create_signal(None::<AuditFilterOptions>);
    let (error, set_error) = create_signal(None::<String>);
    let (suspects, set_suspects) = create_signal(Vec::<String>::new());
    let view_mode = create_rw_signal("all".to_string());
    let search = create_rw_signal(String::new());
    let user = create_rw_signal(String::new());
    let event_type = create_rw_signal(String::new());
    let severity = create_rw_signal(String::new());
    let result = create_rw_signal(String::new());
    let page = create_rw_signal(0usize);
    const PAGE: usize = 50;

    let query = move || AuditQuery {
        event_type: event_type.get_untracked(),
        user: user.get_untracked(),
        severity: severity.get_untracked(),
        result: result.get_untracked(),
        search: search.get_untracked(),
        limit: PAGE,
        offset: page.get_untracked() * PAGE,
    };

    let reload = move || {
        spawn_local(async move {
            let mode = view_mode.get_untracked();
            let r = match mode.as_str() {
                "security" => get_audit_security_events().await.map(|e| (e.len(), e)),
                "failed" => get_audit_failed_logins().await.map(|e| (e.len(), e)),
                _ => search_audit(&query()).await.map(|p| (p.total, p.events)),
            };
            match r {
                Ok((t, e)) => {
                    set_error.set(None);
                    set_total.set(t);
                    set_events.set(e);
                }
                Err(e) => set_error.set(Some(format!("Failed to load the audit log: {}", e))),
            }
            if let Ok(s) = get_audit_brute_force().await {
                set_suspects.set(s);
            }
        });
    };
    create_effect(move |_| {
        spawn_local(async move {
            if let Ok(o) = get_audit_filter_options().await {
                set_options.set(Some(o));
            }
        });
        reload();
    });

    let export = move |format: &'static str| {
        spawn_local(async move {
            match export_audit(&query(), format).await {
                Ok(text) => {
                    let (name, mime) = if format == "csv" {
                        ("audit-log.csv", "text/csv")
                    } else {
                        ("audit-log.json", "application/json")
                    };
                    download(name, mime, &text);
                }
                Err(e) => set_error.set(Some(format!("Export failed: {}", e))),
            }
        });
    };

    let opts = move |f: fn(&AuditFilterOptions) -> Vec<String>| {
        options
            .get()
            .map(|o| f(&o))
            .unwrap_or_default()
            .into_iter()
            .map(|v| view! { <option value=v.clone()>{v.clone()}</option> })
            .collect_view()
    };

    view! {
        <div class="audit-log">
            <div class="page-header">
                <h1>"Audit Log"</h1>
                <div class="header-actions">
                    <button id="audit-export-csv" class="btn btn-secondary" on:click=move |_| export("csv")>"Export CSV"</button>
                    <button id="audit-export-json" class="btn btn-secondary" on:click=move |_| export("json")>"Export JSON"</button>
                    <button class="btn btn-secondary" on:click=move |_| reload()>"Refresh"</button>
                </div>
            </div>
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="audit-error">{e}</div> })}
            {move || {
                let s = suspects.get();
                (!s.is_empty()).then(|| view! {
                    <div class="alert alert-error" id="audit-brute-force">
                        "Possible brute-force: 5+ failed logins in 10 minutes for " {s.join(", ")}
                    </div>
                })
            }}
            <div class="form-row">
                <select id="audit-view" prop:value=view_mode on:change=move |ev| { view_mode.set(event_target_value(&ev)); page.set(0); reload(); }>
                    <option value="all">"All events"</option>
                    <option value="security">"Security (denied / suspicious)"</option>
                    <option value="failed">"Failed logins"</option>
                </select>
                <input id="audit-search" type="text" placeholder="search action, user, resource, details, IP"
                    prop:value=search on:input=move |ev| search.set(event_target_value(&ev))
                    on:keydown=move |ev| { if ev.key() == "Enter" { page.set(0); reload(); } }/>
                <select id="audit-user" prop:value=user on:change=move |ev| { user.set(event_target_value(&ev)); page.set(0); reload(); }>
                    <option value="">"any user"</option>
                    {move || opts(|o| o.users.clone())}
                </select>
                <select id="audit-type" prop:value=event_type on:change=move |ev| { event_type.set(event_target_value(&ev)); page.set(0); reload(); }>
                    <option value="">"any event"</option>
                    {move || opts(|o| o.event_types.clone())}
                </select>
                <select id="audit-severity" prop:value=severity on:change=move |ev| { severity.set(event_target_value(&ev)); page.set(0); reload(); }>
                    <option value="">"any severity"</option>
                    {move || opts(|o| o.severities.clone())}
                </select>
                <select id="audit-result" prop:value=result on:change=move |ev| { result.set(event_target_value(&ev)); page.set(0); reload(); }>
                    <option value="">"any result"</option>
                    {move || opts(|o| o.results.clone())}
                </select>
                <button id="audit-apply" class="btn btn-primary" on:click=move |_| { page.set(0); reload(); }>"Search"</button>
            </div>
            <p>{move || format!("{} matching events", total.get())}</p>
            <table class="vm-table" id="audit-table">
                <thead>
                    <tr><th>"Time"</th><th>"Event"</th><th>"Severity"</th><th>"User"</th><th>"From"</th><th>"Resource"</th><th>"Action"</th><th>"Result"</th></tr>
                </thead>
                <tbody>
                    {move || {
                        let rows = events.get();
                        if rows.is_empty() {
                            return view! { <tr><td colspan="8" class="no-data">"No events match."</td></tr> }.into_view();
                        }
                        rows.into_iter().map(|e| view! {
                            <tr class="audit-row" title=e.details.clone().unwrap_or_default()>
                                <td>{e.timestamp.replace('T', " ").split('.').next().unwrap_or("").to_string()}</td>
                                <td>{e.event_type.clone()}</td>
                                <td>{e.severity.clone()}</td>
                                <td>{e.user.clone().unwrap_or_default()}</td>
                                <td>{e.source_ip.clone().unwrap_or_default()}</td>
                                <td>{e.resource.clone().unwrap_or_default()}</td>
                                <td>{e.action.clone()}</td>
                                <td>{e.result.clone()}</td>
                            </tr>
                        }).collect_view()
                    }}
                </tbody>
            </table>
            <div class="form-row">
                <button class="btn btn-sm" disabled=move || page.get() == 0 || view_mode.get() != "all"
                    on:click=move |_| { page.update(|p| *p = p.saturating_sub(1)); reload(); }>"Newer"</button>
                <button class="btn btn-sm" disabled=move || (page.get() + 1) * PAGE >= total.get() || view_mode.get() != "all"
                    on:click=move |_| { page.update(|p| *p += 1); reload(); }>"Older"</button>
            </div>
        </div>
    }
}
