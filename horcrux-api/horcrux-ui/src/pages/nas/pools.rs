//! NAS Storage Pools Management Page

use leptos::*;

#[derive(Clone, Debug, serde::Deserialize)]
#[allow(dead_code)]
pub struct StoragePool {
    pub id: String,
    pub name: String,
    pub pool_type: String, // zfs, btrfs, mdraid, lvm
    pub status: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
    pub health: String,
    pub disks: Vec<String>,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct Dataset {
    pub id: String,
    pub name: String,
    pub pool_id: String,
    pub mountpoint: String,
    pub used_bytes: u64,
    pub quota_bytes: Option<u64>,
    pub compression: Option<String>,
    pub snapshots: u32,
}

/// A 424 NOT_AVAILABLE answer of `resp` (the node lacks the tool this page needs), if it is one.
async fn unavailable_of(resp: reqwasm::http::Response) -> Option<crate::api::Unavailable> {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    crate::api::parse_unavailable(&crate::api::error_text(status, &body))
}

#[component]
pub fn PoolsPage() -> impl IntoView {
    let (pools, set_pools) = create_signal(Vec::<StoragePool>::new());
    let (datasets, set_datasets) = create_signal(Vec::<Dataset>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);
    let (active_tab, set_active_tab) = create_signal("pools".to_string());
    // Bumped after a create/delete so the lists reload.
    let (reload, set_reload) = create_signal(0u32);
    // Create Pool form (the button used to link to a /nas/pools/create route that does not exist).
    let query = leptos_router::use_query_map();
    let (show_form, set_show_form) = create_signal(
        query
            .get_untracked()
            .get("create")
            .is_some_and(|v| v == "1"),
    );
    let (f_name, set_f_name) = create_signal(String::new());
    let (f_raid, set_f_raid) = create_signal("mirror".to_string());
    let (f_devices, set_f_devices) = create_signal(String::new());
    let (notice, set_notice) = create_signal(None::<(bool, String)>);
    let (unavailable, set_unavailable) = create_signal(None::<crate::api::Unavailable>);

    // Load pools
    create_effect(move |_| {
        reload.track();
        spawn_local(async move {
            set_loading.set(true);

            // Fetch pools
            match crate::api::authed::get("/api/nas/pools").send().await {
                Ok(resp) => {
                    if resp.ok() {
                        set_unavailable.set(None);
                        if let Ok(data) = resp.json::<Vec<StoragePool>>().await {
                            set_pools.set(data);
                        }
                    } else if resp.status() == 424 {
                        set_unavailable.set(unavailable_of(resp).await);
                    } else {
                        set_error.set(Some(format!(
                            "Failed to load pools (HTTP {})",
                            resp.status()
                        )));
                    }
                }
                Err(e) => set_error.set(Some(format!("Network error: {}", e))),
            }

            // Fetch datasets
            if let Ok(resp) = crate::api::authed::get("/api/nas/datasets").send().await {
                if resp.ok() {
                    if let Ok(data) = resp.json::<Vec<Dataset>>().await {
                        set_datasets.set(data);
                    }
                }
            }

            set_loading.set(false);
        });
    });

    let scrub_pool = move |pool_id: String| {
        spawn_local(async move {
            let _ = crate::api::authed::post(&format!("/api/nas/pools/{}/scrub", pool_id))
                .send()
                .await;
        });
    };

    let delete_pool = move |pool_id: String| {
        if web_sys::window()
            .and_then(|w| {
                w.confirm_with_message("Delete this pool? All data will be lost!")
                    .ok()
            })
            .unwrap_or(false)
        {
            spawn_local(async move {
                let _ = crate::api::authed::delete(&format!("/api/nas/pools/{}", pool_id))
                    .send()
                    .await;
                set_reload.update(|n| *n += 1);
            });
        }
    };

    let create_pool = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let name = f_name.get().trim().to_string();
        let devices: Vec<String> = f_devices
            .get()
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|d| !d.is_empty())
            .map(str::to_string)
            .collect();
        let confirmed = web_sys::window()
            .and_then(|w| {
                w.confirm_with_message(&format!(
                    "Create pool {name} on {}? Everything on these disks is erased.",
                    devices.join(", ")
                ))
                .ok()
            })
            .unwrap_or(false);
        if !confirmed {
            return;
        }
        let body =
            serde_json::json!({ "name": name, "raid_level": f_raid.get(), "devices": devices });
        spawn_local(async move {
            match crate::api::post_json_no_response("/nas/pools", &body).await {
                Ok(()) => {
                    set_notice.set(Some((true, format!("Pool {name} created"))));
                    set_show_form.set(false);
                    set_reload.update(|n| *n += 1);
                }
                Err(e) => set_notice.set(Some((false, format!("Failed: {}", e.message)))),
            }
        });
    };

    view! {
        <div class="pools-page">
            <div class="page-header">
                <h1>"Storage Pools"</h1>
                <div class="header-actions">
                    <button class="btn btn-primary" id="nas-pool-create" on:click=move |_| set_show_form.set(true)>"Create Pool"</button>
                </div>
            </div>
            {move || notice.get().map(|(ok, m)| view! {
                <div class=if ok { "alert-success" } else { "alert-error" } id="nas-pool-notice">{m}</div>
            })}
            <Show when=move || show_form.get()>
                <form class="card nas-pool-form" id="nas-pool-form" on:submit=create_pool>
                    <h2>"Create ZFS Pool"</h2>
                    <div class="form-group">
                        <label for="nas-pool-name">"Name"</label>
                        <input id="nas-pool-name" required prop:value=f_name
                            on:input=move |e| set_f_name.set(event_target_value(&e))/>
                    </div>
                    <div class="form-group">
                        <label for="nas-pool-raid">"Layout"</label>
                        <select id="nas-pool-raid" on:change=move |e| set_f_raid.set(event_target_value(&e))>
                            <option value="mirror" selected=true>"Mirror"</option>
                            <option value="raidz1">"RAID-Z1"</option>
                            <option value="raidz2">"RAID-Z2"</option>
                            <option value="raidz3">"RAID-Z3"</option>
                            <option value="stripe">"Stripe (no redundancy)"</option>
                        </select>
                    </div>
                    <div class="form-group">
                        <label for="nas-pool-devices">"Disks"</label>
                        <input id="nas-pool-devices" required placeholder="/dev/disk/by-id/ata-A /dev/disk/by-id/ata-B"
                            prop:value=f_devices on:input=move |e| set_f_devices.set(event_target_value(&e))/>
                        <p class="hint">"Separate disks with spaces or commas. "<a href="/storage/disks">"See the node's disks"</a></p>
                    </div>
                    <div class="form-actions">
                        <button type="submit" class="btn btn-primary">"Create"</button>
                        <button type="button" class="btn" on:click=move |_| set_show_form.set(false)>"Cancel"</button>
                    </div>
                </form>
            </Show>

            <div class="tabs">
                <button
                    class={move || if active_tab.get() == "pools" { "tab active" } else { "tab" }}
                    on:click=move |_| set_active_tab.set("pools".to_string())
                >"Pools"</button>
                <button
                    class={move || if active_tab.get() == "datasets" { "tab active" } else { "tab" }}
                    on:click=move |_| set_active_tab.set("datasets".to_string())
                >"Datasets"</button>
            </div>

            {move || {
                if let Some(u) = unavailable.get() {
                    view! { <crate::components::UnavailableState info=u on_retry=Callback::new(move |_| set_reload.update(|n| *n += 1))/> }.into_view()
                } else if loading.get() {
                    view! { <p class="loading">"Loading storage..."</p> }.into_view()
                } else if let Some(err) = error.get() {
                    view! { <p class="error">"Error: " {err}</p> }.into_view()
                } else if active_tab.get() == "pools" {
                    let pool_list = pools.get();
                    if pool_list.is_empty() {
                        view! {
                            <div class="no-data">
                                <p>"No storage pools configured."</p>
                                <p>"Create a pool to start using NAS storage features."</p>
                                <button class="btn btn-primary" on:click=move |_| set_show_form.set(true)>"Create Pool"</button>
                            </div>
                        }.into_view()
                    } else {
                        view! {
                            <div class="pool-cards">
                                {pool_list.into_iter().map(|pool| {
                                    let pool_id_scrub = pool.id.clone();
                                    let pool_id_del = pool.id.clone();
                                    let usage_pct = if pool.total_bytes > 0 {
                                        (pool.used_bytes as f64 / pool.total_bytes as f64 * 100.0) as u32
                                    } else { 0 };
                                    let health_class = match pool.health.as_str() {
                                        "ONLINE" | "healthy" => "health-good",
                                        "DEGRADED" => "health-warning",
                                        _ => "health-error",
                                    };

                                    view! {
                                        <div class="pool-card">
                                            <div class="pool-header">
                                                <h3>{&pool.name}</h3>
                                                <span class={format!("pool-type {}", pool.pool_type)}>{&pool.pool_type}</span>
                                            </div>
                                            <div class="pool-health">
                                                <span class={health_class}>{&pool.health}</span>
                                            </div>
                                            <div class="pool-usage">
                                                <div class="usage-bar">
                                                    <div class="usage-fill" style={format!("width: {}%", usage_pct)}></div>
                                                </div>
                                                <div class="usage-text">
                                                    {format_bytes(pool.used_bytes)} " / " {format_bytes(pool.total_bytes)}
                                                    " (" {usage_pct} "%)"
                                                </div>
                                            </div>
                                            <div class="pool-disks">
                                                <strong>"Disks: "</strong>
                                                {pool.disks.join(", ")}
                                            </div>
                                            <div class="pool-actions">
                                                <a href={format!("/nas/pools/{}", pool.id)} class="btn btn-sm">"Manage"</a>
                                                <button
                                                    class="btn btn-sm"
                                                    on:click=move |_| scrub_pool(pool_id_scrub.clone())
                                                >"Scrub"</button>
                                                <button
                                                    class="btn btn-sm btn-danger"
                                                    on:click=move |_| delete_pool(pool_id_del.clone())
                                                >"Delete"</button>
                                            </div>
                                        </div>
                                    }
                                }).collect_view()}
                            </div>
                        }.into_view()
                    }
                } else {
                    // Datasets tab
                    let dataset_list = datasets.get();
                    if dataset_list.is_empty() {
                        view! {
                            <div class="no-data">
                                <p>"No datasets configured."</p>
                            </div>
                        }.into_view()
                    } else {
                        view! {
                            <table class="data-table">
                                <thead>
                                    <tr>
                                        <th>"Name"</th>
                                        <th>"Pool"</th>
                                        <th>"Mountpoint"</th>
                                        <th>"Used"</th>
                                        <th>"Quota"</th>
                                        <th>"Compression"</th>
                                        <th>"Snapshots"</th>
                                        <th>"Actions"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {dataset_list.into_iter().map(|ds| {
                                        view! {
                                            <tr>
                                                <td><strong>{&ds.name}</strong></td>
                                                <td>{&ds.pool_id}</td>
                                                <td><code>{&ds.mountpoint}</code></td>
                                                <td>{format_bytes(ds.used_bytes)}</td>
                                                <td>{ds.quota_bytes.map(format_bytes).unwrap_or_else(|| "-".to_string())}</td>
                                                <td>{ds.compression.clone().unwrap_or_else(|| "-".to_string())}</td>
                                                <td>{ds.snapshots}</td>
                                                <td class="actions">
                                                    <a href={format!("/nas/datasets/{}", ds.id)} class="btn btn-sm">"Edit"</a>
                                                    <a href={format!("/nas/datasets/{}/snapshots", ds.id)} class="btn btn-sm">"Snapshots"</a>
                                                </td>
                                            </tr>
                                        }
                                    }).collect_view()}
                                </tbody>
                            </table>
                        }.into_view()
                    }
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
