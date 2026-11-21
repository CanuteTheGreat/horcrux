//! Where the web UI / API listens (GET/PUT/DELETE /api/system/web).
//! Route: `/system/web`. Docs: docs/WEB-LISTENERS.md.
use crate::api;
use leptos::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Web {
    host: String,
    port: u16,
    http_redirect_port: Option<u16>,
    #[serde(default)]
    extra_https_ports: Vec<u16>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct View {
    running: Web,
    configured: Web,
    defaults: Web,
    stored: bool,
    restart_required: bool,
    #[serde(default)]
    notes: Vec<String>,
}

fn ports_text(v: &[u16]) -> String {
    v.iter()
        .map(|p| p.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn parse_ports(s: &str) -> Vec<u16> {
    s.split(',')
        .filter_map(|p| p.trim().parse::<u16>().ok())
        .collect()
}

#[component]
pub fn WebListenersPage() -> impl IntoView {
    let view_sig = create_rw_signal(View::default());
    let form = create_rw_signal(Web::default());
    let (loaded, set_loaded) = create_signal(false);
    let (notice, set_notice) = create_signal(None::<String>);
    let load = move || {
        spawn_local(async move {
            match api::fetch_json::<View>("/system/web").await {
                Ok(v) => {
                    form.set(v.configured.clone());
                    view_sig.set(v);
                    set_loaded.set(true);
                }
                Err(e) => set_notice.set(Some(format!("Could not load: {}", e.message))),
            }
        });
    };
    create_effect(move |_| load());
    let save = move |_| {
        let w = form.get();
        spawn_local(async move {
            match api::put_json::<View, Web>("/system/web", &w).await {
                Ok(v) => {
                    let mut msg =
                        "Saved. The listeners are re-bound at the next restart of horcrux-api."
                            .to_string();
                    for n in &v.notes {
                        msg.push(' ');
                        msg.push_str(n);
                    }
                    set_notice.set(Some(msg));
                    form.set(v.configured.clone());
                    view_sig.set(v);
                }
                Err(e) => set_notice.set(Some(format!("Not saved: {}", e.message))),
            }
        });
    };
    let use_defaults = move |_| {
        form.set(view_sig.get().defaults);
    };
    view! {
        <div class="page system-web-listeners">
            <div class="page-header"><h1>"Web listeners"</h1></div>
            <p class="help">
                "Where this node serves the web UI and API. The standard ports are the default: "
                "HTTPS on 443, and HTTP on 80 that only redirects to HTTPS. Extra HTTPS ports "
                "(for example 8006 for existing API clients and cluster peers) are optional. "
                "Changes are stored on the node and applied when horcrux-api restarts; the firewall "
                "rules for this node follow the ports configured here."
            </p>
            {move || notice.get().map(|n| view! { <p id="web-listeners-notice" class="notice">{n}</p> })}
            {move || loaded.get().then(|| view! {
                <section class="card" id="web-listeners-running">
                    <h2>"Listening now"</h2>
                    <p>
                        {move || {
                            let r = view_sig.get().running;
                            format!(
                                "{} - HTTPS {}, HTTP redirect {}",
                                r.host,
                                ports_text(&[vec![r.port], r.extra_https_ports.clone()].concat()),
                                r.http_redirect_port.map(|p| p.to_string()).unwrap_or_else(|| "off".into())
                            )
                        }}
                    </p>
                    {move || view_sig.get().restart_required.then(|| view! {
                        <p class="notice" id="web-listeners-restart">"The saved settings differ from what is running: restart horcrux-api to apply them."</p>
                    })}
                    {move || view_sig.get().stored.then(|| view! {
                        <p class="help">"These settings were saved through the API, UI or a declarative spec and override config.toml and the environment."</p>
                    })}
                </section>
                <form class="settings-form" id="web-listeners-form" on:submit=|ev| ev.prevent_default()>
                    <label>"Bind address"
                        <input type="text" name="host" prop:value=move || form.get().host
                            on:change=move |ev| { let v = event_target_value(&ev).trim().to_string(); form.update(|f| f.host = v) }/>
                    </label>
                    <label>"HTTPS port"
                        <input type="number" min="1" max="65535" name="port" prop:value=move || form.get().port.to_string()
                            on:change=move |ev| { let v = event_target_value(&ev).trim().parse().unwrap_or(form.get().port); form.update(|f| f.port = v) }/>
                    </label>
                    <label>"HTTP redirect port (empty = off)"
                        <input type="text" name="http_redirect_port"
                            prop:value=move || form.get().http_redirect_port.map(|p| p.to_string()).unwrap_or_default()
                            on:change=move |ev| { let v = event_target_value(&ev).trim().parse::<u16>().ok(); form.update(|f| f.http_redirect_port = v) }/>
                    </label>
                    <label>"Extra HTTPS ports (comma separated, empty = none)"
                        <input type="text" name="extra_https_ports" placeholder="8006"
                            prop:value=move || ports_text(&form.get().extra_https_ports)
                            on:change=move |ev| { let v = parse_ports(&event_target_value(&ev)); form.update(|f| f.extra_https_ports = v) }/>
                    </label>
                    <div class="btn-row">
                        <button type="button" class="btn btn-primary" id="web-listeners-save" on:click=save>"Save"</button>
                        <button type="button" class="btn btn-secondary" id="web-listeners-defaults" on:click=use_defaults>"Fill in the standard ports"</button>
                    </div>
                </form>
            })}
        </div>
    }
}
