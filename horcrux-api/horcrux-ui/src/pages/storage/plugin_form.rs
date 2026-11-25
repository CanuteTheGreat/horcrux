//! Storage plugins in the web UI: the pool form generated from a plugin's
//! declared schema (no per-plugin code), and the plugin list / settings panel.
use crate::api::{self, PluginDescription, PluginField, PluginFieldType, StoragePluginSettings};
use leptos::*;
use std::collections::BTreeMap;

/// Form values, every field as text (what the inputs hold; bools are
/// "true"/"false"). The server and [`validate`] parse them by field type.
pub type PluginValues = BTreeMap<String, String>;

/// Initial values: each field's declared default.
pub fn default_values(desc: &PluginDescription) -> PluginValues {
    desc.schema
        .iter()
        .map(|f| {
            let v = match &f.default {
                Some(serde_json::Value::String(s)) => s.clone(),
                Some(serde_json::Value::Null) | None => match f.field_type {
                    PluginFieldType::Bool => "false".into(),
                    PluginFieldType::Enum => f
                        .options
                        .first()
                        .map(|o| o.value.clone())
                        .unwrap_or_default(),
                    _ => String::new(),
                },
                Some(v) => v.to_string(),
            };
            (f.key.clone(), v)
        })
        .collect()
}

/// The request `config` for these values (sent as typed by the user; the
/// server validates and normalises them again).
pub fn to_config(values: &PluginValues) -> BTreeMap<String, serde_json::Value> {
    values
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
        .collect()
}

/// Client-side check with the same rules the server applies (SDK
/// `validate_config`); field key -> message.
pub fn validate(desc: &PluginDescription, values: &PluginValues) -> BTreeMap<String, String> {
    match horcrux_storage_plugin_sdk::validate_config(&desc.schema, &to_config(values)) {
        Ok(_) => BTreeMap::new(),
        Err(errs) => errs.into_iter().map(|e| (e.key, e.message)).collect(),
    }
}

/// Whether field `f` applies given the current values (its `depends_on`).
fn active(f: &PluginField, schema: &[PluginField], values: &PluginValues) -> bool {
    let Some(d) = &f.depends_on else {
        return true;
    };
    // the dependency must itself be active
    if let Some(dep) = schema.iter().find(|x| x.key == d.key) {
        if !active(dep, schema, values) {
            return false;
        }
    }
    let cur = values.get(&d.key).cloned().unwrap_or_default();
    match &d.value {
        None => !cur.is_empty() && cur != "false" && cur != "0",
        Some(serde_json::Value::String(s)) => cur == *s,
        Some(v) => cur == v.to_string(),
    }
}

/// The generated pool form of one plugin. Every input carries
/// `data-key=<field key>`; hints show as the label's tooltip and below the
/// input; numbers carry min/max; secrets are password inputs.
#[component]
pub fn PluginConfigForm(
    desc: PluginDescription,
    values: RwSignal<PluginValues>,
    errors: RwSignal<BTreeMap<String, String>>,
) -> impl IntoView {
    let schema = store_value(desc.schema.clone());
    let fields = desc.schema.clone();
    view! {
        <div class="plugin-config-form" data-plugin=desc.id.clone()>
            {(!desc.description.is_empty()).then(|| view! {
                <p class="form-hint plugin-description">{desc.description.clone()}</p>
            })}
            {fields.into_iter().map(|f| {
                let key = f.key.clone();
                let fv = f.clone();
                let visible = create_memo(move |_| {
                    values.with(|v| schema.with_value(|s| active(&fv, s, v)))
                });
                let f = store_value(f);
                move || visible.get().then(|| field_view(f.get_value(), values, errors)).map(|v| {
                    view! { <div class="plugin-field-wrap" data-key=key.clone()>{v}</div> }
                })
            }).collect_view()}
        </div>
    }
}

fn field_view(
    f: PluginField,
    values: RwSignal<PluginValues>,
    errors: RwSignal<BTreeMap<String, String>>,
) -> impl IntoView {
    let key = store_value(f.key.clone());
    let id = format!("plugin-field-{}", f.key);
    let get = move || values.with(|v| v.get(&key.get_value()).cloned().unwrap_or_default());
    let set = move |s: String| {
        values.update(|v| {
            v.insert(key.get_value(), s);
        });
        errors.update(|e| {
            e.remove(&key.get_value());
        });
    };
    let hint = f.hint.clone();
    let required = f.required;
    let num = |n: Option<i64>| n.map(|x| x.to_string());
    let placeholder = (!f.placeholder.is_empty()).then(|| f.placeholder.clone());
    let input = match f.field_type {
        PluginFieldType::Int => view! {
            <input type="number" step="1" id=id.clone() class="form-control plugin-input"
                data-key=f.key.clone() min=num(f.min) max=num(f.max) required=required
                placeholder=placeholder
                prop:value=get on:input=move |ev| set(event_target_value(&ev))/>
        }
        .into_view(),
        PluginFieldType::Bool => view! {
            <input type="checkbox" id=id.clone() class="plugin-input" data-key=f.key.clone()
                prop:checked=move || get() == "true"
                on:change=move |ev| set(event_target_checked(&ev).to_string())/>
        }
        .into_view(),
        PluginFieldType::Enum => {
            let opts = f.options.clone();
            view! {
                <select id=id.clone() class="form-control plugin-input" data-key=f.key.clone()
                    prop:value=get on:change=move |ev| set(event_target_value(&ev))>
                    {opts.into_iter().map(|o| {
                        let label = if o.label.is_empty() { o.value.clone() } else { o.label.clone() };
                        let v = o.value.clone();
                        view! { <option value=o.value.clone() selected=move || get() == v>{label}</option> }
                    }).collect_view()}
                </select>
            }
            .into_view()
        }
        PluginFieldType::Secret => view! {
            <input type="password" autocomplete="new-password" id=id.clone()
                class="form-control plugin-input" data-key=f.key.clone() required=required
                minlength=num(f.min) maxlength=num(f.max)
                prop:value=get on:input=move |ev| set(event_target_value(&ev))/>
        }
        .into_view(),
        PluginFieldType::String | PluginFieldType::Path => view! {
            <input type="text" id=id.clone() class="form-control plugin-input"
                data-key=f.key.clone() required=required pattern=f.pattern.clone()
                minlength=num(f.min) maxlength=num(f.max) placeholder=placeholder
                prop:value=get on:input=move |ev| set(event_target_value(&ev))/>
        }
        .into_view(),
    };
    let tip = hint.clone();
    view! {
        <div class="form-group plugin-field">
            <label for=id>
                {f.label.clone()}
                {required.then(|| view! { <span class="required">" *"</span> })}
                {(!tip.is_empty()).then(|| view! {
                    <span class="field-hint-icon" title=tip.clone() aria-label=tip.clone()>" ⓘ"</span>
                })}
            </label>
            {input}
            {(!hint.is_empty()).then(|| view! { <small class="form-hint plugin-field-hint">{hint.clone()}</small> })}
            {move || errors.with(|e| e.get(&key.get_value()).cloned()).map(|m| view! {
                <div class="field-error plugin-field-error">{format!("{} {}", f.label, m)}</div>
            })}
        </div>
    }
}

/// Installed plugins (usable or with the reason they can't be used) and the
/// node's plugin settings. Shown on the Storage page.
#[component]
pub fn StoragePluginsPanel(
    plugins: ReadSignal<Option<Result<api::StoragePluginList, String>>>,
) -> impl IntoView {
    let (show_settings, set_show_settings) = create_signal(false);
    view! {
        <div class="info-box storage-plugins" id="storage-plugins">
            <h4>"Storage plugins"</h4>
            {move || match plugins.get() {
                None => view! { <p class="loading">"Loading storage plugins..."</p> }.into_view(),
                Some(Err(e)) => view! { <div class="alert alert-error">{e}</div> }.into_view(),
                Some(Ok(l)) if l.plugins.is_empty() => view! {
                    <div class="empty-state storage-plugins-empty">
                        <p>"No storage plugins are installed on this node."</p>
                        <p class="form-hint">
                            "A plugin adds a storage type to Create Pool. Install a plugin package \
                             (it puts an executable in "<code>{l.plugin_dir.clone()}</code>
                             "); see docs/storage-plugins.md to write one."
                        </p>
                    </div>
                }.into_view(),
                Some(Ok(l)) => view! {
                    <table class="table storage-plugin-table">
                        <thead><tr><th>"Plugin"</th><th>"Version"</th><th>"Can do"</th><th>"Status"</th></tr></thead>
                        <tbody>
                            {l.plugins.into_iter().map(|p| {
                                let (name, version, caps) = match &p.description {
                                    Some(d) => {
                                        let c = &d.capabilities;
                                        let mut can = vec![];
                                        if c.snapshots { can.push("snapshots"); }
                                        if c.resize { can.push("resize"); }
                                        if c.clone { can.push("clone"); }
                                        if c.shared { can.push("shared"); }
                                        if c.vm_disk_dir { can.push("VM disks"); }
                                        (d.name.clone(), d.version.clone(), can.join(", "))
                                    }
                                    None => (p.id.clone(), String::new(), String::new()),
                                };
                                view! {
                                    <tr class="storage-plugin" data-plugin=p.id.clone()>
                                        <td>{name}" "<code>{p.id.clone()}</code></td>
                                        <td>{version}</td>
                                        <td>{caps}</td>
                                        <td>{if p.usable {
                                            view! { <span class="status-badge active">"ready"</span> }.into_view()
                                        } else {
                                            view! { <span class="plugin-error">{p.error.clone().unwrap_or_default()}</span> }.into_view()
                                        }}</td>
                                    </tr>
                                }
                            }).collect_view()}
                        </tbody>
                    </table>
                }.into_view(),
            }}
            <button type="button" class="btn btn-secondary btn-sm storage-plugin-settings-toggle"
                on:click=move |_| set_show_settings.update(|v| *v = !*v)>
                {move || if show_settings.get() { "Hide plugin settings" } else { "Plugin settings" }}
            </button>
            {move || show_settings.get().then(|| view! { <PluginSettingsForm/> })}
        </div>
    }
}

/// Edit the node's storage plugin settings (plugin directory, timeouts).
#[component]
fn PluginSettingsForm() -> impl IntoView {
    let settings = create_rw_signal(None::<StoragePluginSettings>);
    let (msg, set_msg) = create_signal(None::<Result<String, String>>);
    spawn_local(async move {
        match api::get_storage_plugin_settings().await {
            Ok(s) => settings.set(Some(s)),
            Err(e) => set_msg.set(Some(Err(e.message))),
        }
    });
    let save = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let Some(s) = settings.get_untracked() else {
            return;
        };
        spawn_local(async move {
            match api::put_storage_plugin_settings(&s).await {
                Ok(s) => {
                    settings.set(Some(s));
                    set_msg.set(Some(Ok("Saved".into())));
                }
                Err(e) => set_msg.set(Some(Err(e.message))),
            }
        });
    };
    // (label, getter, setter) per timeout: one generic number row each
    type Get = fn(&StoragePluginSettings) -> u32;
    type Set = fn(&mut StoragePluginSettings, u32);
    let timeouts: Vec<(&'static str, &'static str, Get, Set)> = vec![
        (
            "describe",
            "Describe (list plugins, open the form)",
            |s| s.timeouts.describe_secs,
            |s, v| s.timeouts.describe_secs = v,
        ),
        (
            "activate",
            "Activate (mount at pool add and start)",
            |s| s.timeouts.activate_secs,
            |s, v| s.timeouts.activate_secs = v,
        ),
        (
            "deactivate",
            "Deactivate (pool removed)",
            |s| s.timeouts.deactivate_secs,
            |s, v| s.timeouts.deactivate_secs = v,
        ),
        (
            "status",
            "Status (capacity, on every pool list)",
            |s| s.timeouts.status_secs,
            |s, v| s.timeouts.status_secs = v,
        ),
        (
            "list",
            "List volumes / snapshots",
            |s| s.timeouts.list_secs,
            |s, v| s.timeouts.list_secs = v,
        ),
        (
            "create",
            "Create volume",
            |s| s.timeouts.create_secs,
            |s, v| s.timeouts.create_secs = v,
        ),
        (
            "delete",
            "Delete volume",
            |s| s.timeouts.delete_secs,
            |s, v| s.timeouts.delete_secs = v,
        ),
        (
            "resize",
            "Resize volume",
            |s| s.timeouts.resize_secs,
            |s, v| s.timeouts.resize_secs = v,
        ),
        (
            "snapshot",
            "Snapshot create / delete / rollback",
            |s| s.timeouts.snapshot_secs,
            |s, v| s.timeouts.snapshot_secs = v,
        ),
        (
            "path",
            "Volume path",
            |s| s.timeouts.path_secs,
            |s, v| s.timeouts.path_secs = v,
        ),
        (
            "map",
            "Map / unmap volume",
            |s| s.timeouts.map_secs,
            |s, v| s.timeouts.map_secs = v,
        ),
    ];
    view! {
        <form class="plugin-settings-form" on:submit=save>
            {move || msg.get().map(|m| match m {
                Ok(t) => view! { <div class="alert alert-info">{t}</div> }.into_view(),
                Err(t) => view! { <div class="alert alert-error">{t}</div> }.into_view(),
            })}
            <div class="form-group">
                <label for="plugin-dir" title="Every executable in this directory is a storage plugin (its file name is the plugin id).">
                    "Plugin directory"
                </label>
                <input id="plugin-dir" type="text" class="form-control" required=true pattern="/.*"
                    prop:value=move || settings.with(|s| s.as_ref().map(|s| s.plugin_dir.clone()).unwrap_or_default())
                    on:input=move |ev| settings.update(|s| if let Some(s) = s { s.plugin_dir = event_target_value(&ev) })/>
            </div>
            <div class="form-group">
                <label for="plugin-state-dir" title="Per-pool working directories (mount points). Empty = <data dir>/storage-plugins.">
                    "State directory"
                </label>
                <input id="plugin-state-dir" type="text" class="form-control" placeholder="<data dir>/storage-plugins"
                    prop:value=move || settings.with(|s| s.as_ref().map(|s| s.state_dir.clone()).unwrap_or_default())
                    on:input=move |ev| settings.update(|s| if let Some(s) = s { s.state_dir = event_target_value(&ev) })/>
            </div>
            <div class="form-group">
                <label title="Plugins run as root: only run executables owned by root and not writable by others.">
                    <input type="checkbox"
                        prop:checked=move || settings.with(|s| s.as_ref().map(|s| s.require_root_owned).unwrap_or(true))
                        on:change=move |ev| settings.update(|s| if let Some(s) = s { s.require_root_owned = event_target_checked(&ev) })/>
                    " Only run root-owned plugins"
                </label>
            </div>
            <div class="form-group">
                <label for="plugin-max-output" title="Largest answer read from a plugin (16-65536 KiB).">"Largest plugin answer (KiB)"</label>
                <input id="plugin-max-output" type="number" min="16" max="65536" class="form-control"
                    prop:value=move || settings.with(|s| s.as_ref().map(|s| s.max_output_kib.to_string()).unwrap_or_default())
                    on:input=move |ev| settings.update(|s| if let Some(s) = s { s.max_output_kib = event_target_value(&ev).parse().unwrap_or(0) })/>
            </div>
            <h5>"Time limits (seconds)"</h5>
            <div class="plugin-timeouts">
                {timeouts.into_iter().map(|(k, label, g, st)| view! {
                    <div class="form-group">
                        <label for=format!("plugin-timeout-{k}")>{label}</label>
                        <input id=format!("plugin-timeout-{k}") type="number" min="1" max="86400" class="form-control"
                            prop:value=move || settings.with(|s| s.as_ref().map(|s| g(s).to_string()).unwrap_or_default())
                            on:input=move |ev| settings.update(|s| if let Some(s) = s { st(s, event_target_value(&ev).parse().unwrap_or(0)) })/>
                    </div>
                }).collect_view()}
            </div>
            <button type="submit" class="btn btn-primary btn-sm">"Save plugin settings"</button>
        </form>
    }
}
