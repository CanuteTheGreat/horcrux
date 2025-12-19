use leptos::*;
use crate::api::*;

#[component]
pub fn DiskManagementPage() -> impl IntoView {
    let (disks, set_disks) = create_signal(Vec::<DiskInfo>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);

    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);
            match get_disk_list().await {
                Ok(disk_list) => {
                    set_disks.set(disk_list);
                    set_error.set(None);
                }
                Err(e) => {
                    set_error.set(Some(format!("Failed to load disks: {}", e)));
                }
            }
            set_loading.set(false);
        });
    });

    view! {
        <div class="disk-management-page">
            <div class="page-header">
                <h1 class="page-title">Disk Management</h1>
                <p class="page-description">
                    Manage physical disks, partitions, and storage devices
                </p>
            </div>

            {move || if loading.get() {
                view! {
                    <div class="loading-container">
                        <div class="spinner"></div>
                        <p>Loading disk information...</p>
                    </div>
                }.into_view()
            } else if let Some(err) = error.get() {
                view! {
                    <div class="error-container">
                        <div class="error-message">
                            <h3>Error Loading Disks</h3>
                            <p>{err}</p>
                        </div>
                    </div>
                }.into_view()
            } else {
                view! {
                    <div class="disks-grid">
                        {disks.get().into_iter().map(|disk| view! {
                            <div class={format!("disk-card smart-{}", disk.smart_status)}>
                                <div class="disk-header">
                                    <h3>{disk.name.clone()}</h3>
                                    <span class={format!("smart-badge smart-{}", disk.smart_status)}>
                                        {disk.smart_status.to_uppercase()}
                                    </span>
                                </div>

                                <div class="disk-info">
                                    <div class="info-row">
                                        <span class="label">Model:</span>
                                        <span class="value">{disk.model.clone()}</span>
                                    </div>
                                    <div class="info-row">
                                        <span class="label">Type:</span>
                                        <span class="value">{disk.disk_type.to_uppercase()} ({disk.interface.clone()})</span>
                                    </div>
                                    <div class="info-row">
                                        <span class="label">Size:</span>
                                        <span class="value">{format_bytes(disk.size_bytes)}</span>
                                    </div>
                                    {disk.temperature.map(|temp| view! {
                                        <div class="info-row">
                                            <span class="label">"Temperature:"</span>
                                            <span class={format!("value {}", if temp > 50 { "hot" } else { "" })}>
                                                {format!("{}C", temp)}
                                            </span>
                                        </div>
                                    })}
                                </div>

                                <div class="disk-usage">
                                    <div class="usage-bar">
                                        <div
                                            class="usage-fill"
                                            style={format!("width: {}%", (disk.used_bytes as f64 / disk.size_bytes as f64 * 100.0))}
                                        ></div>
                                    </div>
                                    <span class="usage-text">
                                        {format_bytes(disk.used_bytes)} / {format_bytes(disk.size_bytes)} used
                                    </span>
                                </div>

                                <div class="partitions-section">
                                    <h4>Partitions ({disk.partitions.len()})</h4>
                                    <div class="partitions-list">
                                        {disk.partitions.iter().map(|part| view! {
                                            <div class="partition-item">
                                                <span class="partition-name">{part.name.clone()}</span>
                                                <span class="partition-fs">{part.filesystem.clone()}</span>
                                                <span class="partition-size">{format_bytes(part.size_bytes)}</span>
                                                {part.mount_point.as_ref().map(|mp| view! {
                                                    <span class="partition-mount">{mp.clone()}</span>
                                                })}
                                            </div>
                                        }).collect::<Vec<_>>()}
                                    </div>
                                </div>
                            </div>
                        }).collect::<Vec<_>>()}
                    </div>
                }.into_view()
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