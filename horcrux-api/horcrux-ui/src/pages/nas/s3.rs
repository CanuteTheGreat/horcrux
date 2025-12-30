//! S3 Gateway Management Page

use leptos::*;

#[derive(Clone, Debug, serde::Deserialize)]
#[allow(dead_code)]
pub struct S3Bucket {
    pub name: String,
    pub created_at: String,
    pub size_bytes: u64,
    pub object_count: u64,
    pub versioning: bool,
    pub policy: Option<String>,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct S3AccessKey {
    pub access_key_id: String,
    pub user: String,
    pub created_at: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct S3GatewayStatus {
    pub running: bool,
    pub endpoint: String,
    pub version: Option<String>,
    pub uptime_secs: Option<u64>,
}

#[component]
pub fn S3Page() -> impl IntoView {
    let (status, set_status) = create_signal(None::<S3GatewayStatus>);
    let (buckets, set_buckets) = create_signal(Vec::<S3Bucket>::new());
    let (access_keys, set_access_keys) = create_signal(Vec::<S3AccessKey>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);
    let (active_tab, set_active_tab) = create_signal("buckets".to_string());
    let (show_create_bucket, set_show_create_bucket) = create_signal(false);
    let (show_create_key, set_show_create_key) = create_signal(false);

    // Load S3 data
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);

            // Fetch gateway status
            match reqwasm::http::Request::get("/api/nas/s3-gateway/status")
                .send()
                .await
            {
                Ok(resp) => {
                    if resp.ok() {
                        if let Ok(data) = resp.json::<S3GatewayStatus>().await {
                            set_status.set(Some(data));
                        }
                    }
                }
                Err(e) => set_error.set(Some(format!("Network error: {}", e))),
            }

            // Fetch buckets
            match reqwasm::http::Request::get("/api/nas/s3-gateway/buckets")
                .send()
                .await
            {
                Ok(resp) => {
                    if resp.ok() {
                        if let Ok(data) = resp.json::<Vec<S3Bucket>>().await {
                            set_buckets.set(data);
                        }
                    }
                }
                Err(_) => {}
            }

            // Fetch access keys
            match reqwasm::http::Request::get("/api/nas/s3-gateway/keys")
                .send()
                .await
            {
                Ok(resp) => {
                    if resp.ok() {
                        if let Ok(data) = resp.json::<Vec<S3AccessKey>>().await {
                            set_access_keys.set(data);
                        }
                    }
                }
                Err(_) => {}
            }

            set_loading.set(false);
        });
    });

    let start_gateway = move |_| {
        spawn_local(async move {
            let _ = reqwasm::http::Request::post("/api/nas/s3-gateway/start")
                .send()
                .await;
        });
    };

    let stop_gateway = move |_| {
        spawn_local(async move {
            let _ = reqwasm::http::Request::post("/api/nas/s3-gateway/stop")
                .send()
                .await;
        });
    };

    let delete_bucket = move |bucket_name: String| {
        if web_sys::window()
            .and_then(|w| w.confirm_with_message(&format!("Delete bucket '{}'?", bucket_name)).ok())
            .unwrap_or(false)
        {
            spawn_local(async move {
                let _ = reqwasm::http::Request::delete(&format!("/api/nas/s3-gateway/buckets/{}", bucket_name))
                    .send()
                    .await;
            });
        }
    };

    let delete_key = move |key_id: String| {
        if web_sys::window()
            .and_then(|w| w.confirm_with_message("Delete this access key?").ok())
            .unwrap_or(false)
        {
            spawn_local(async move {
                let _ = reqwasm::http::Request::delete(&format!("/api/nas/s3-gateway/keys/{}", key_id))
                    .send()
                    .await;
            });
        }
    };

    view! {
        <div class="s3-page">
            <div class="page-header">
                <h1>"S3 Gateway"</h1>
                <p class="subtitle">"S3-compatible object storage API"</p>
            </div>

            // Gateway Status Card
            {move || {
                if let Some(st) = status.get() {
                    let status_class = if st.running { "status-active" } else { "status-inactive" };
                    view! {
                        <div class="status-card">
                            <div class="status-header">
                                <h3>"Gateway Status"</h3>
                                <span class={status_class}>
                                    {if st.running { "Running" } else { "Stopped" }}
                                </span>
                            </div>
                            <div class="status-info">
                                <div class="info-item">
                                    <span class="label">"Endpoint:"</span>
                                    <code>{&st.endpoint}</code>
                                </div>
                                {st.version.as_ref().map(|v| view! {
                                    <div class="info-item">
                                        <span class="label">"Version:"</span>
                                        <span>{v}</span>
                                    </div>
                                })}
                                {st.uptime_secs.map(|u| view! {
                                    <div class="info-item">
                                        <span class="label">"Uptime:"</span>
                                        <span>{format_uptime(u)}</span>
                                    </div>
                                })}
                            </div>
                            <div class="status-actions">
                                {if st.running {
                                    view! {
                                        <button class="btn btn-danger" on:click=stop_gateway>"Stop Gateway"</button>
                                    }.into_view()
                                } else {
                                    view! {
                                        <button class="btn btn-success" on:click=start_gateway>"Start Gateway"</button>
                                    }.into_view()
                                }}
                            </div>
                        </div>
                    }.into_view()
                } else {
                    view! {}.into_view()
                }
            }}

            <div class="tabs">
                <button
                    class={move || if active_tab.get() == "buckets" { "tab active" } else { "tab" }}
                    on:click=move |_| set_active_tab.set("buckets".to_string())
                >"Buckets"</button>
                <button
                    class={move || if active_tab.get() == "keys" { "tab active" } else { "tab" }}
                    on:click=move |_| set_active_tab.set("keys".to_string())
                >"Access Keys"</button>
            </div>

            {move || {
                if loading.get() {
                    view! { <p class="loading">"Loading S3 data..."</p> }.into_view()
                } else if let Some(err) = error.get() {
                    view! { <p class="error">"Error: " {err}</p> }.into_view()
                } else if active_tab.get() == "buckets" {
                    let bucket_list = buckets.get();
                    view! {
                        <div class="tab-content">
                            <div class="tab-header">
                                <button class="btn btn-primary" on:click=move |_| set_show_create_bucket.set(true)>
                                    "Create Bucket"
                                </button>
                            </div>
                            {if bucket_list.is_empty() {
                                view! {
                                    <div class="no-data">
                                        <p>"No S3 buckets configured."</p>
                                    </div>
                                }.into_view()
                            } else {
                                view! {
                                    <table class="data-table">
                                        <thead>
                                            <tr>
                                                <th>"Bucket Name"</th>
                                                <th>"Objects"</th>
                                                <th>"Size"</th>
                                                <th>"Versioning"</th>
                                                <th>"Created"</th>
                                                <th>"Actions"</th>
                                            </tr>
                                        </thead>
                                        <tbody>
                                            {bucket_list.into_iter().map(|bucket| {
                                                let bucket_name = bucket.name.clone();
                                                view! {
                                                    <tr>
                                                        <td><strong>{&bucket.name}</strong></td>
                                                        <td>{bucket.object_count}</td>
                                                        <td>{format_bytes(bucket.size_bytes)}</td>
                                                        <td>{if bucket.versioning { "Enabled" } else { "Disabled" }}</td>
                                                        <td>{&bucket.created_at}</td>
                                                        <td class="actions">
                                                            <a href={format!("/nas/s3/buckets/{}", &bucket.name)} class="btn btn-sm">"Browse"</a>
                                                            <button
                                                                class="btn btn-sm btn-danger"
                                                                on:click=move |_| delete_bucket(bucket_name.clone())
                                                            >"Delete"</button>
                                                        </td>
                                                    </tr>
                                                }
                                            }).collect_view()}
                                        </tbody>
                                    </table>
                                }.into_view()
                            }}
                        </div>
                    }.into_view()
                } else {
                    // Access Keys tab
                    let key_list = access_keys.get();
                    view! {
                        <div class="tab-content">
                            <div class="tab-header">
                                <button class="btn btn-primary" on:click=move |_| set_show_create_key.set(true)>
                                    "Create Access Key"
                                </button>
                            </div>
                            {if key_list.is_empty() {
                                view! {
                                    <div class="no-data">
                                        <p>"No access keys configured."</p>
                                    </div>
                                }.into_view()
                            } else {
                                view! {
                                    <table class="data-table">
                                        <thead>
                                            <tr>
                                                <th>"Access Key ID"</th>
                                                <th>"User"</th>
                                                <th>"Status"</th>
                                                <th>"Created"</th>
                                                <th>"Actions"</th>
                                            </tr>
                                        </thead>
                                        <tbody>
                                            {key_list.into_iter().map(|key| {
                                                let key_id = key.access_key_id.clone();
                                                let status_class = if key.enabled { "status-active" } else { "status-inactive" };
                                                view! {
                                                    <tr>
                                                        <td><code>{&key.access_key_id}</code></td>
                                                        <td>{&key.user}</td>
                                                        <td>
                                                            <span class={status_class}>
                                                                {if key.enabled { "Active" } else { "Disabled" }}
                                                            </span>
                                                        </td>
                                                        <td>{&key.created_at}</td>
                                                        <td class="actions">
                                                            <button class="btn btn-sm">"Rotate"</button>
                                                            <button
                                                                class="btn btn-sm btn-danger"
                                                                on:click=move |_| delete_key(key_id.clone())
                                                            >"Delete"</button>
                                                        </td>
                                                    </tr>
                                                }
                                            }).collect_view()}
                                        </tbody>
                                    </table>
                                }.into_view()
                            }}
                        </div>
                    }.into_view()
                }
            }}

            // Create Bucket Modal
            {move || {
                if show_create_bucket.get() {
                    view! {
                        <div class="modal-overlay" on:click=move |_| set_show_create_bucket.set(false)>
                            <div class="modal" on:click=|e| e.stop_propagation()>
                                <h2>"Create S3 Bucket"</h2>
                                <form>
                                    <div class="form-group">
                                        <label>"Bucket Name"</label>
                                        <input type="text" name="name" placeholder="my-bucket" required />
                                        <small>"Lowercase letters, numbers, and hyphens only"</small>
                                    </div>
                                    <div class="form-check">
                                        <input type="checkbox" name="versioning" />
                                        <label>"Enable Versioning"</label>
                                    </div>
                                    <div class="modal-actions">
                                        <button type="button" class="btn" on:click=move |_| set_show_create_bucket.set(false)>"Cancel"</button>
                                        <button type="submit" class="btn btn-primary">"Create Bucket"</button>
                                    </div>
                                </form>
                            </div>
                        </div>
                    }.into_view()
                } else {
                    view! {}.into_view()
                }
            }}

            // Create Access Key Modal
            {move || {
                if show_create_key.get() {
                    view! {
                        <div class="modal-overlay" on:click=move |_| set_show_create_key.set(false)>
                            <div class="modal" on:click=|e| e.stop_propagation()>
                                <h2>"Create Access Key"</h2>
                                <form>
                                    <div class="form-group">
                                        <label>"User"</label>
                                        <input type="text" name="user" placeholder="username" required />
                                    </div>
                                    <div class="form-group">
                                        <label>"Description"</label>
                                        <input type="text" name="description" placeholder="API access for backup script" />
                                    </div>
                                    <div class="modal-actions">
                                        <button type="button" class="btn" on:click=move |_| set_show_create_key.set(false)>"Cancel"</button>
                                        <button type="submit" class="btn btn-primary">"Create Key"</button>
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

fn format_uptime(secs: u64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;

    if days > 0 {
        format!("{}d {}h {}m", days, hours, mins)
    } else if hours > 0 {
        format!("{}h {}m", hours, mins)
    } else {
        format!("{}m", mins)
    }
}
