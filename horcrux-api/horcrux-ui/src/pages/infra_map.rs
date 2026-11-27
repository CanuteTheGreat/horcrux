//! Infrastructure map (`/map`): this node's VMs, containers, storage and networks, merged with the
//! configured sources (a Patronus router's network map, a read-only Proxmox import), drawn by the
//! shared `ui-kit/map` component (served by the API at /ui-map/). docs/design/infra-map.md.
use crate::api;
use leptos::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen::{JsCast, JsValue};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct MapSource {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub kind: String,
    pub url: String,
    #[serde(default)]
    pub secret: String,
    #[serde(default = "yes")]
    pub verify_tls: bool,
    #[serde(default = "thirty")]
    pub refresh_secs: u64,
    #[serde(default = "ten")]
    pub timeout_secs: u64,
}
fn yes() -> bool {
    true
}
fn thirty() -> u64 {
    30
}
fn ten() -> u64 {
    10
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct MapKey {
    pub id: String,
    pub name: String,
    pub scope: String,
    pub detail: String,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub last_used: Option<i64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct NewKey {
    key: String,
    live_path: String,
}

/// Mount the shared map component on `el` (index.html's `hxMountMap` loads it on first use).
fn mount_map(el: &web_sys::HtmlElement) {
    let Some(w) = web_sys::window() else { return };
    let token = api::get_auth_token().unwrap_or_default();
    if let Ok(f) = js_sys::Reflect::get(&w, &JsValue::from_str("hxMountMap")) {
        if let Some(f) = f.dyn_ref::<js_sys::Function>() {
            let _ = f.call2(&JsValue::NULL, el, &JsValue::from_str(&token));
        }
    }
}

fn ts(t: Option<i64>) -> String {
    t.and_then(|t| chrono::DateTime::from_timestamp(t, 0))
        .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "-".into())
}

#[component]
pub fn InfraMapPage() -> impl IntoView {
    let map_ref = create_node_ref::<html::Div>();
    let (sources, set_sources) = create_signal(Vec::<MapSource>::new());
    let (keys, set_keys) = create_signal(Vec::<MapKey>::new());
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let (new_key, set_new_key) = create_signal(None::<NewKey>);
    let (s_name, set_s_name) = create_signal(String::new());
    let (s_kind, set_s_kind) = create_signal("patronus".to_string());
    let (s_url, set_s_url) = create_signal(String::new());
    let (s_secret, set_s_secret) = create_signal(String::new());
    let (s_verify, set_s_verify) = create_signal(true);
    let (k_name, set_k_name) = create_signal(String::new());
    let (k_scope, set_k_scope) = create_signal("view".to_string());
    let (k_detail, set_k_detail) = create_signal("basic".to_string());

    let reload = move || {
        spawn_local(async move {
            match api::fetch_json::<Vec<MapSource>>("/api/infra-map/sources").await {
                Ok(v) => set_sources.set(v),
                Err(e) => set_error.set(Some(e.message)),
            }
            if let Ok(v) = api::fetch_json::<Vec<MapKey>>("/api/infra-map/keys").await {
                set_keys.set(v);
            }
        });
    };
    create_effect(move |_| reload());
    map_ref.on_load(move |el| {
        let el: &web_sys::HtmlElement = &el;
        mount_map(el);
    });

    let add_source = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let s = MapSource {
            name: s_name.get_untracked().trim().to_string(),
            kind: s_kind.get_untracked(),
            url: s_url.get_untracked().trim().to_string(),
            secret: s_secret.get_untracked().trim().to_string(),
            verify_tls: s_verify.get_untracked(),
            refresh_secs: 30,
            timeout_secs: 10,
            ..Default::default()
        };
        spawn_local(async move {
            match api::post_json::<serde_json::Value, _>("/api/infra-map/sources", &s).await {
                Ok(_) => {
                    set_notice.set(Some(format!(
                        "Source {} saved; it appears on the map at the next refresh.",
                        s.name
                    )));
                    set_error.set(None);
                    set_s_secret.set(String::new());
                    reload();
                }
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };
    let add_key = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let body = serde_json::json!({"name": k_name.get_untracked().trim(), "scope": k_scope.get_untracked(), "detail": k_detail.get_untracked()});
        spawn_local(async move {
            match api::post_json::<NewKey, _>("/api/infra-map/keys", &body).await {
                Ok(k) => {
                    set_new_key.set(Some(k));
                    set_error.set(None);
                    set_k_name.set(String::new());
                    reload();
                }
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };
    let origin = web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_default();

    view! {
        <div class="page infra-map-page">
            <div class="page-header"><h1>"Infrastructure map"</h1></div>
            <p class="form-help">"Everything this node runs, built from what Horcrux manages: no diagram to keep up to date. Add a Patronus router to see the network around it, or a Proxmox cluster to compare before a migration."</p>
            {move || error.get().map(|e| view! { <div class="alert alert-error" role="alert">{e}</div> })}
            {move || notice.get().map(|n| view! { <div class="alert alert-success" role="status">{n}</div> })}
            <div id="hx-map" node_ref=map_ref></div>

            <section class="card" aria-labelledby="map-sources">
                <h2 id="map-sources">"Sources"</h2>
                <p class="form-help">"A Patronus source needs a share key from the router (Network map > Sharing; detail \"full\" links VMs to the devices the router sees). A Proxmox source needs a read-only API token (user@realm!tokenid=secret, role PVEAuditor). Secrets are stored, never shown again. UniFi and Unraid imports are planned."</p>
                <div class="table-scroll" tabindex="0" role="region" aria-label="Map sources">
                <table class="table">
                    <thead><tr><th>"Name"</th><th>"Kind"</th><th>"URL"</th><th>"TLS check"</th><th></th></tr></thead>
                    <tbody>
                        {move || {
                            let v = sources.get();
                            if v.is_empty() {
                                view! { <tr><td colspan="5" class="muted">"Only this node is drawn until a source is added."</td></tr> }.into_view()
                            } else {
                                v.into_iter().map(|s| {
                                    let id = s.id.clone();
                                    view! { <tr><td>{s.name.clone()}</td><td>{s.kind.clone()}</td><td>{s.url.clone()}</td>
                                        <td>{if s.verify_tls { "on" } else { "off" }}</td>
                                        <td><button class="btn btn-sm" on:click=move |_| {
                                            let id = id.clone();
                                            spawn_local(async move {
                                                if let Err(e) = api::delete_json(&format!("/api/infra-map/sources/{id}")).await { set_error.set(Some(e.message)); }
                                                reload();
                                            });
                                        }>"Remove"</button></td></tr> }
                                }).collect_view()
                            }
                        }}
                    </tbody>
                </table>
                </div>
                <form id="map-source-form" class="form-row fields" on:submit=add_source>
                    <div class="form-group"><label for="src-name">"Name"</label><input id="src-name" required pattern="[a-z0-9_\\-]{1,32}" placeholder="home-router" prop:value=s_name on:input=move |e| set_s_name.set(event_target_value(&e))/></div>
                    <div class="form-group"><label for="src-kind">"Kind"</label><select id="src-kind" on:change=move |e| set_s_kind.set(event_target_value(&e))>
                        <option value="patronus" selected>"Patronus router"</option>
                        <option value="proxmox">"Proxmox VE (read-only)"</option>
                    </select></div>
                    <div class="form-group wide"><label for="src-url">"URL"</label><input id="src-url" required placeholder="https://192.168.1.1" prop:value=s_url on:input=move |e| set_s_url.set(event_target_value(&e))/></div>
                    <div class="form-group"><label for="src-secret">"Key or token"</label><input id="src-secret" required type="password" autocomplete="off" placeholder="pmk_... or API token" prop:value=s_secret on:input=move |e| set_s_secret.set(event_target_value(&e))/></div>
                    <div class="form-group"><label for="src-tls">"Check TLS certificate"</label><input id="src-tls" type="checkbox" prop:checked=s_verify on:change=move |e| set_s_verify.set(event_target_checked(&e))/></div>
                    <button class="btn btn-primary" type="submit">"Add source"</button>
                </form>
            </section>

            <section class="card" aria-labelledby="map-keys">
                <h2 id="map-keys">"Read-only sharing"</h2>
                <p class="form-help">"A share key opens the live map without a login (scope view) or only the counts for a dashboard widget such as Homepage (scope stats). Only a hash is stored."</p>
                {move || new_key.get().map(|k| {
                    let link = format!("{}{}", origin, k.live_path);
                    view! { <div class="alert alert-success"><strong>"Copy this key now, it is not shown again: "</strong><code>{k.key.clone()}</code>
                        <br/>"Live view: "<a href=link.clone() target="_blank" rel="noopener">{link.clone()}</a></div> }
                })}
                <div class="table-scroll" tabindex="0" role="region" aria-label="Share keys">
                <table class="table">
                    <thead><tr><th>"Name"</th><th>"Scope"</th><th>"Detail"</th><th>"Created"</th><th>"Last used"</th><th></th></tr></thead>
                    <tbody>
                        {move || {
                            let v = keys.get();
                            if v.is_empty() {
                                view! { <tr><td colspan="6" class="muted">"No share keys."</td></tr> }.into_view()
                            } else {
                                v.into_iter().map(|k| {
                                    let id = k.id.clone();
                                    view! { <tr><td>{k.name.clone()}</td><td>{k.scope.clone()}</td><td>{k.detail.clone()}</td>
                                        <td>{ts(Some(k.created_at))}</td><td>{ts(k.last_used)}</td>
                                        <td><button class="btn btn-sm" on:click=move |_| {
                                            let id = id.clone();
                                            spawn_local(async move {
                                                if let Err(e) = api::delete_json(&format!("/api/infra-map/keys/{id}")).await { set_error.set(Some(e.message)); }
                                                reload();
                                            });
                                        }>"Revoke"</button></td></tr> }
                                }).collect_view()
                            }
                        }}
                    </tbody>
                </table>
                </div>
                <form id="map-key-form" class="form-row fields" on:submit=add_key>
                    <div class="form-group"><label for="key-name">"Name"</label><input id="key-name" required pattern="[a-z0-9_\\-]{1,32}" placeholder="wall-tablet" prop:value=k_name on:input=move |e| set_k_name.set(event_target_value(&e))/></div>
                    <div class="form-group"><label for="key-scope">"Scope"</label><select id="key-scope" on:change=move |e| set_k_scope.set(event_target_value(&e))>
                        <option value="view" selected>"view (live map)"</option><option value="stats">"stats (widget)"</option></select></div>
                    <div class="form-group"><label for="key-detail">"Detail"</label><select id="key-detail" on:change=move |e| set_k_detail.set(event_target_value(&e))>
                        <option value="basic" selected>"basic"</option><option value="full">"full (MACs, details)"</option></select></div>
                    <button class="btn btn-primary" type="submit">"Create key"</button>
                </form>
            </section>
        </div>
    }
}
