//! iSCSI Target Management Page

use leptos::*;

#[derive(Clone, Debug, serde::Deserialize)]
pub struct IscsiTarget {
    pub id: String,
    pub iqn: String,
    pub alias: Option<String>,
    pub enabled: bool,
    pub auth_method: String,
    pub luns: Vec<IscsiLun>,
    pub initiators: Vec<String>,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct IscsiLun {
    pub lun_id: u32,
    pub backing_store: String,
    pub size_bytes: u64,
    pub readonly: bool,
}

#[component]
pub fn IscsiPage() -> impl IntoView {
    let (targets, set_targets) = create_signal(Vec::<IscsiTarget>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);
    let (show_create, set_show_create) = create_signal(false);

    // Load targets
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);
            match reqwasm::http::Request::get("/api/nas/iscsi/targets")
                .send()
                .await
            {
                Ok(resp) => {
                    if resp.ok() {
                        if let Ok(data) = resp.json::<Vec<IscsiTarget>>().await {
                            set_targets.set(data);
                            set_error.set(None);
                        }
                    } else {
                        set_error.set(Some("Failed to load iSCSI targets".to_string()));
                    }
                }
                Err(e) => set_error.set(Some(format!("Network error: {}", e))),
            }
            set_loading.set(false);
        });
    });

    let toggle_target = move |target_id: String, enable: bool| {
        spawn_local(async move {
            let action = if enable { "enable" } else { "disable" };
            let _ = reqwasm::http::Request::post(&format!("/api/nas/iscsi/targets/{}/{}", target_id, action))
                .send()
                .await;
        });
    };

    let delete_target = move |target_id: String| {
        if web_sys::window()
            .and_then(|w| w.confirm_with_message("Delete this iSCSI target?").ok())
            .unwrap_or(false)
        {
            spawn_local(async move {
                let _ = reqwasm::http::Request::delete(&format!("/api/nas/iscsi/targets/{}", target_id))
                    .send()
                    .await;
            });
        }
    };

    view! {
        <div class="iscsi-page">
            <div class="page-header">
                <h1>"iSCSI Targets"</h1>
                <div class="header-actions">
                    <button class="btn btn-primary" on:click=move |_| set_show_create.set(true)>
                        "Create Target"
                    </button>
                </div>
            </div>

            {move || {
                if loading.get() {
                    view! { <p class="loading">"Loading iSCSI targets..."</p> }.into_view()
                } else if let Some(err) = error.get() {
                    view! { <p class="error">"Error: " {err}</p> }.into_view()
                } else {
                    let target_list = targets.get();
                    if target_list.is_empty() {
                        view! {
                            <div class="no-data">
                                <p>"No iSCSI targets configured."</p>
                                <p>"Create a target to expose block storage over the network."</p>
                            </div>
                        }.into_view()
                    } else {
                        view! {
                            <div class="target-list">
                                {target_list.into_iter().map(|target| {
                                    let target_id_toggle = target.id.clone();
                                    let target_id_del = target.id.clone();
                                    let is_enabled = target.enabled;
                                    let status_class = if target.enabled { "status-active" } else { "status-inactive" };
                                    let total_luns = target.luns.len();
                                    let total_size: u64 = target.luns.iter().map(|l| l.size_bytes).sum();

                                    view! {
                                        <div class="target-card">
                                            <div class="target-header">
                                                <div class="target-iqn">
                                                    <h3>{&target.iqn}</h3>
                                                    {target.alias.as_ref().map(|a| view! { <span class="alias">"(" {a} ")"</span> })}
                                                </div>
                                                <span class={status_class}>
                                                    {if target.enabled { "Enabled" } else { "Disabled" }}
                                                </span>
                                            </div>

                                            <div class="target-info">
                                                <div class="info-item">
                                                    <span class="label">"Auth:"</span>
                                                    <span class="value">{&target.auth_method}</span>
                                                </div>
                                                <div class="info-item">
                                                    <span class="label">"LUNs:"</span>
                                                    <span class="value">{total_luns}</span>
                                                </div>
                                                <div class="info-item">
                                                    <span class="label">"Total Size:"</span>
                                                    <span class="value">{format_bytes(total_size)}</span>
                                                </div>
                                                <div class="info-item">
                                                    <span class="label">"Initiators:"</span>
                                                    <span class="value">{target.initiators.len()}</span>
                                                </div>
                                            </div>

                                            {if !target.luns.is_empty() {
                                                view! {
                                                    <div class="lun-list">
                                                        <h4>"LUNs"</h4>
                                                        <table class="lun-table">
                                                            <thead>
                                                                <tr>
                                                                    <th>"LUN"</th>
                                                                    <th>"Backing Store"</th>
                                                                    <th>"Size"</th>
                                                                    <th>"Mode"</th>
                                                                </tr>
                                                            </thead>
                                                            <tbody>
                                                                {target.luns.iter().map(|lun| {
                                                                    view! {
                                                                        <tr>
                                                                            <td>{lun.lun_id}</td>
                                                                            <td><code>{&lun.backing_store}</code></td>
                                                                            <td>{format_bytes(lun.size_bytes)}</td>
                                                                            <td>{if lun.readonly { "RO" } else { "RW" }}</td>
                                                                        </tr>
                                                                    }
                                                                }).collect_view()}
                                                            </tbody>
                                                        </table>
                                                    </div>
                                                }.into_view()
                                            } else {
                                                view! {}.into_view()
                                            }}

                                            <div class="target-actions">
                                                <a href={format!("/nas/iscsi/{}", &target.id)} class="btn btn-sm">"Manage"</a>
                                                <a href={format!("/nas/iscsi/{}/luns", &target.id)} class="btn btn-sm">"Add LUN"</a>
                                                <button
                                                    class={if is_enabled { "btn btn-sm btn-warning" } else { "btn btn-sm btn-success" }}
                                                    on:click=move |_| toggle_target(target_id_toggle.clone(), !is_enabled)
                                                >
                                                    {if is_enabled { "Disable" } else { "Enable" }}
                                                </button>
                                                <button
                                                    class="btn btn-sm btn-danger"
                                                    on:click=move |_| delete_target(target_id_del.clone())
                                                >"Delete"</button>
                                            </div>
                                        </div>
                                    }
                                }).collect_view()}
                            </div>
                        }.into_view()
                    }
                }
            }}

            // Create Target Modal
            {move || {
                if show_create.get() {
                    view! {
                        <div class="modal-overlay" on:click=move |_| set_show_create.set(false)>
                            <div class="modal" on:click=|e| e.stop_propagation()>
                                <h2>"Create iSCSI Target"</h2>
                                <form>
                                    <div class="form-group">
                                        <label>"Target Alias"</label>
                                        <input type="text" name="alias" placeholder="my-storage-target" />
                                        <small>"A friendly name for this target"</small>
                                    </div>
                                    <div class="form-group">
                                        <label>"Authentication"</label>
                                        <select name="auth_method">
                                            <option value="none">"None"</option>
                                            <option value="chap">"CHAP"</option>
                                            <option value="mutual_chap">"Mutual CHAP"</option>
                                        </select>
                                    </div>
                                    <div class="form-group">
                                        <label>"Allowed Initiators"</label>
                                        <input type="text" name="initiators" placeholder="iqn.2024-01.com.example:initiator" />
                                        <small>"Comma-separated list of initiator IQNs (leave empty for all)"</small>
                                    </div>
                                    <div class="modal-actions">
                                        <button type="button" class="btn" on:click=move |_| set_show_create.set(false)>"Cancel"</button>
                                        <button type="submit" class="btn btn-primary">"Create Target"</button>
                                    </div>
                                </form>
                            </div>
                        </div>
                    }.into_view()
                } else {
                    view! {}.into_view()
                }
            }}
        </div>
    }
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.2} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}
