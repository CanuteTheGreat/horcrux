//! Monitoring > Metric Servers: send this node's metrics to InfluxDB
//! (1.x / 2.x, HTTP(S) or UDP) or Graphite (TCP / UDP), like Proxmox's
//! external metric server. Secrets are never shown; leaving them untouched
//! when editing keeps the stored ones.
use crate::api::metric_servers as ms;
use leptos::*;

fn blank(kind: &str) -> ms::MetricServer {
    ms::MetricServer {
        id: String::new(),
        name: String::new(),
        kind: kind.into(),
        enabled: true,
        host: String::new(),
        port: 0,
        protocol: String::new(),
        api_version: if kind == "influxdb" {
            "v2".into()
        } else {
            String::new()
        },
        organization: String::new(),
        bucket: String::new(),
        token: String::new(),
        database: String::new(),
        username: String::new(),
        password: String::new(),
        verify_tls: true,
        prefix: String::new(),
        interval_secs: 30,
        status: None,
    }
}

fn when(ts: Option<i64>) -> String {
    ts.map(|t| {
        let ago = (js_sys::Date::now() / 1000.0) as i64 - t;
        format!("{ago} s ago")
    })
    .unwrap_or_else(|| "never".into())
}

#[component]
pub fn MetricServersPage() -> impl IntoView {
    let (servers, set_servers) = create_signal(Vec::<ms::MetricServer>::new());
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let form = create_rw_signal(None::<ms::MetricServer>);

    let reload = move || {
        spawn_local(async move {
            match ms::list().await {
                Ok(l) => set_servers.set(l),
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };
    reload();

    let save = move |_| {
        let Some(s) = form.get_untracked() else {
            return;
        };
        spawn_local(async move {
            set_error.set(None);
            let r = if s.id.is_empty() {
                ms::create(&s).await
            } else {
                ms::update(&s).await
            };
            match r {
                Ok(saved) => {
                    set_notice.set(Some(format!("Saved metric server {}", saved.name)));
                    form.set(None);
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            reload();
        });
    };
    let test = move |id: String, name: String| {
        spawn_local(async move {
            set_error.set(None);
            match ms::test(&id).await {
                Ok(t) => set_notice.set(Some(format!("{name}: sent {} data points", t.points))),
                Err(e) => set_error.set(Some(e.message)),
            }
            reload();
        });
    };
    let remove = move |id: String, name: String| {
        spawn_local(async move {
            set_error.set(None);
            match ms::delete(&id).await {
                Ok(()) => set_notice.set(Some(format!("Removed {name}"))),
                Err(e) => set_error.set(Some(e.message)),
            }
            reload();
        });
    };

    // field helpers on the open form
    let get =
        move |f: fn(&ms::MetricServer) -> String| form.get().map(|s| f(&s)).unwrap_or_default();
    let set = move |f: fn(&mut ms::MetricServer, String), v: String| {
        form.update(|s| {
            if let Some(s) = s {
                f(s, v)
            }
        })
    };
    let is_influx = move || form.get().is_some_and(|s| s.kind == "influxdb");
    let is_v2 = move || {
        form.get()
            .is_some_and(|s| s.kind == "influxdb" && s.api_version != "v1")
    };

    view! {
        <div class="page metric-servers">
            <header class="page-header">
                <h2>"Metric Servers"</h2>
                <button class="btn btn-primary" id="ms-add-influx" on:click=move |_| form.set(Some(blank("influxdb")))>"Add InfluxDB"</button>
                <button class="btn btn-primary" id="ms-add-graphite" on:click=move |_| form.set(Some(blank("graphite")))>"Add Graphite"</button>
            </header>
            <p class="form-help">"Every enabled server receives this node's CPU, memory, per-VM and storage metrics at its interval."</p>
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="ms-error">{e}</div> })}
            {move || notice.get().map(|n| view! { <div class="alert alert-success" id="ms-notice">{n}</div> })}

            <table class="data-table" id="ms-table">
                <thead><tr><th>"Name"</th><th>"Type"</th><th>"Target"</th><th>"Every"</th><th>"Enabled"</th><th>"Last send"</th><th>"Actions"</th></tr></thead>
                <tbody>
                    {move || servers.get().into_iter().map(|s| {
                        let (id1, n1) = (s.id.clone(), s.name.clone());
                        let (id2, n2) = (s.id.clone(), s.name.clone());
                        let edit = s.clone();
                        let st = s.status.clone().unwrap_or_default();
                        view! {
                            <tr class="ms-row" data-name=s.name.clone()>
                                <td>{s.name.clone()}</td>
                                <td>{s.kind.clone()}</td>
                                <td>{format!("{}://{}:{}", s.protocol, s.host, s.port)}</td>
                                <td>{format!("{} s", s.interval_secs)}</td>
                                <td>{if s.enabled { "yes" } else { "no" }}</td>
                                <td class="ms-status">
                                    {match st.last_error {
                                        Some(e) => format!("failed: {e}"),
                                        None => format!("{} ({} points)", when(st.last_success), st.points_sent),
                                    }}
                                </td>
                                <td>
                                    <button class="btn btn-sm ms-test" on:click=move |_| test(id1.clone(), n1.clone())>"Send test"</button>
                                    <button class="btn btn-sm ms-edit" on:click=move |_| form.set(Some(edit.clone()))>"Edit"</button>
                                    <button class="btn btn-sm btn-danger ms-delete" on:click=move |_| remove(id2.clone(), n2.clone())>"Delete"</button>
                                </td>
                            </tr>
                        }
                    }).collect_view()}
                </tbody>
            </table>
            {move || servers.get().is_empty().then(|| view! { <p class="form-help" id="ms-empty">"No metric server configured."</p> })}

            {move || form.get().map(|s| view! {
                <fieldset class="form-section" id="ms-form">
                    <legend>{if s.id.is_empty() { format!("New {} server", s.kind) } else { format!("Edit {}", s.name) }}</legend>
                    <div class="form-row">
                        <label>"Name"</label>
                        <input id="ms-name" type="text" prop:value=move || get(|s| s.name.clone()) on:input=move |ev| set(|s, v| s.name = v, event_target_value(&ev))/>
                        <label>"Host"</label>
                        <input id="ms-host" type="text" prop:value=move || get(|s| s.host.clone()) on:input=move |ev| set(|s, v| s.host = v, event_target_value(&ev))/>
                        <label>"Port (0 = default)"</label>
                        <input id="ms-port" type="number" min="0" max="65535" prop:value=move || get(|s| s.port.to_string())
                            on:input=move |ev| set(|s, v| s.port = v.parse().unwrap_or(0), event_target_value(&ev))/>
                        <label>"Protocol"</label>
                        <select id="ms-protocol" prop:value=move || get(|s| s.protocol.clone()) on:change=move |ev| set(|s, v| s.protocol = v, event_target_value(&ev))>
                            <option value="">"default"</option>
                            {move || if is_influx() {
                                view! { <option value="http">"http"</option><option value="https">"https"</option><option value="udp">"udp (1.x)"</option> }.into_view()
                            } else {
                                view! { <option value="tcp">"tcp"</option><option value="udp">"udp"</option> }.into_view()
                            }}
                        </select>
                    </div>
                    <Show when=is_influx>
                        <div class="form-row">
                            <label>"API"</label>
                            <select id="ms-api" prop:value=move || get(|s| s.api_version.clone()) on:change=move |ev| set(|s, v| s.api_version = v, event_target_value(&ev))>
                                <option value="v2">"InfluxDB 2.x"</option><option value="v1">"InfluxDB 1.x"</option>
                            </select>
                            <Show when=is_v2 fallback=move || view! {
                                <label>"Database"</label>
                                <input id="ms-database" type="text" prop:value=move || get(|s| s.database.clone()) on:input=move |ev| set(|s, v| s.database = v, event_target_value(&ev))/>
                                <label>"User"</label>
                                <input id="ms-username" type="text" prop:value=move || get(|s| s.username.clone()) on:input=move |ev| set(|s, v| s.username = v, event_target_value(&ev))/>
                                <label>"Password"</label>
                                <input id="ms-password" type="password" prop:value=move || get(|s| s.password.clone()) on:input=move |ev| set(|s, v| s.password = v, event_target_value(&ev))/>
                            }>
                                <label>"Organization"</label>
                                <input id="ms-org" type="text" prop:value=move || get(|s| s.organization.clone()) on:input=move |ev| set(|s, v| s.organization = v, event_target_value(&ev))/>
                                <label>"Bucket"</label>
                                <input id="ms-bucket" type="text" prop:value=move || get(|s| s.bucket.clone()) on:input=move |ev| set(|s, v| s.bucket = v, event_target_value(&ev))/>
                                <label>"Token"</label>
                                <input id="ms-token" type="password" prop:value=move || get(|s| s.token.clone()) on:input=move |ev| set(|s, v| s.token = v, event_target_value(&ev))/>
                            </Show>
                        </div>
                    </Show>
                    <Show when=move || !is_influx()>
                        <div class="form-row">
                            <label>"Path prefix"</label>
                            <input id="ms-prefix" type="text" placeholder="horcrux" prop:value=move || get(|s| s.prefix.clone()) on:input=move |ev| set(|s, v| s.prefix = v, event_target_value(&ev))/>
                        </div>
                    </Show>
                    <div class="form-row">
                        <label>"Interval (s)"</label>
                        <input id="ms-interval" type="number" min="5" max="3600" prop:value=move || get(|s| s.interval_secs.to_string())
                            on:input=move |ev| set(|s, v| s.interval_secs = v.parse().unwrap_or(30), event_target_value(&ev))/>
                        <label><input id="ms-enabled" type="checkbox" prop:checked=move || form.get().is_some_and(|s| s.enabled)
                            on:change=move |ev| { let on = event_target_checked(&ev); form.update(|s| if let Some(s) = s { s.enabled = on }) }/>" Enabled"</label>
                        <Show when=is_influx>
                            <label><input id="ms-verify" type="checkbox" prop:checked=move || form.get().is_some_and(|s| s.verify_tls)
                                on:change=move |ev| { let on = event_target_checked(&ev); form.update(|s| if let Some(s) = s { s.verify_tls = on }) }/>" Verify HTTPS certificate"</label>
                        </Show>
                    </div>
                    <div class="form-actions">
                        <button class="btn btn-primary" id="ms-save" on:click=save>"Save"</button>
                        <button class="btn" id="ms-cancel" on:click=move |_| form.set(None)>"Cancel"</button>
                    </div>
                </fieldset>
            })}
        </div>
    }
}
