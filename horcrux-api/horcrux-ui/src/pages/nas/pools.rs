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

#[component]
pub fn PoolsPage() -> impl IntoView {
    let (pools, set_pools) = create_signal(Vec::<StoragePool>::new());
    let (datasets, set_datasets) = create_signal(Vec::<Dataset>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);
    let (active_tab, set_active_tab) = create_signal("pools".to_string());

    // Load pools
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);

            // Fetch pools
            match reqwasm::http::Request::get("/api/nas/pools")
                .send()
                .await
            {
                Ok(resp) => {
                    if resp.ok() {
                        if let Ok(data) = resp.json::<Vec<StoragePool>>().await {
                            set_pools.set(data);
                        }
                    }
                }
                Err(e) => set_error.set(Some(format!("Network error: {}", e))),
            }

            // Fetch datasets
            match reqwasm::http::Request::get("/api/nas/datasets")
                .send()
                .await
            {
                Ok(resp) => {
                    if resp.ok() {
                        if let Ok(data) = resp.json::<Vec<Dataset>>().await {
                            set_datasets.set(data);
                        }
                    }
                }
                Err(_) => {}
            }

            set_loading.set(false);
        });
    });

    let scrub_pool = move |pool_id: String| {
        spawn_local(async move {
            let _ = reqwasm::http::Request::post(&format!("/api/nas/pools/{}/scrub", pool_id))
                .send()
                .await;
        });
    };

    let delete_pool = move |pool_id: String| {
        if web_sys::window()
            .and_then(|w| w.confirm_with_message("Delete this pool? All data will be lost!").ok())
            .unwrap_or(false)
        {
            spawn_local(async move {
                let _ = reqwasm::http::Request::delete(&format!("/api/nas/pools/{}", pool_id))
                    .send()
                    .await;
            });
        }
    };

    view! {
        <div class="pools-page">
            <div class="page-header">
                <h1>"Storage Pools"</h1>
                <div class="header-actions">
                    <a href="/nas/pools/create" class="btn btn-primary">"Create Pool"</a>
                </div>
            </div>

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
                if loading.get() {
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
                                                <a href={format!("/nas/pools/{}", &pool.id)} class="btn btn-sm">"Manage"</a>
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
                                                    <a href={format!("/nas/datasets/{}", &ds.id)} class="btn btn-sm">"Edit"</a>
                                                    <a href={format!("/nas/datasets/{}/snapshots", &ds.id)} class="btn btn-sm">"Snapshots"</a>
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
