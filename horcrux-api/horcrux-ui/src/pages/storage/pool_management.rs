//! Storage Pool Management Page

use leptos::*;
use serde::{Deserialize, Serialize};
use crate::api::{fetch_json, delete_json};

/// Storage pool information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoragePool {
    pub id: String,
    pub name: String,
    pub pool_type: String,
    pub path: Option<String>,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub status: String,
}

/// Storage Management Page Component
#[component]
pub fn StorageManagement() -> impl IntoView {
    let (pools, set_pools) = create_signal(Vec::<StoragePool>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);
    let (show_create_modal, set_show_create_modal) = create_signal(false);

    // Fetch storage pools on mount
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);
            match fetch_json::<Vec<StoragePool>>("/api/storage").await {
                Ok(data) => {
                    set_pools.set(data);
                    set_loading.set(false);
                }
                Err(e) => {
                    set_error.set(Some(format!("Failed to load storage pools: {}", e.message)));
                    set_loading.set(false);
                }
            }
        });
    });

    // Helper function to format bytes
    let format_bytes = |bytes: u64| -> String {
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
    };

    // Delete storage pool
    let delete_pool = move |pool_id: String| {
        spawn_local(async move {
            let url = format!("/api/storage/{}", pool_id);
            match delete_json(&url).await {
                Ok(()) => {
                    if let Ok(data) = fetch_json::<Vec<StoragePool>>("/api/storage").await {
                        set_pools.set(data);
                    }
                }
                Err(e) => {
                    set_error.set(Some(format!("Delete failed: {}", e.message)));
                }
            }
        });
    };

    view! {
        <div class="page storage-management">
            <header class="page-header">
                <h2>"Storage Pools"</h2>
                <button class="btn btn-primary" on:click=move |_| set_show_create_modal.set(true)>
                    "+ Create Pool"
                </button>
            </header>

            // Error display
            {move || error.get().map(|e| view! {
                <div class="alert alert-error">
                    <span>{e}</span>
                    <button class="btn-close" on:click=move |_| set_error.set(None)>"x"</button>
                </div>
            })}

            // Loading state
            {move || loading.get().then(|| view! {
                <div class="loading">
                    <div class="spinner"></div>
                    <span>"Loading storage pools..."</span>
                </div>
            })}

            // Storage overview
            {move || if !loading.get() && !pools.get().is_empty() {
                let total_capacity: u64 = pools.get().iter().map(|p| p.total_bytes).sum();
                let total_used: u64 = pools.get().iter().map(|p| p.used_bytes).sum();
                let usage_percent = if total_capacity > 0 {
                    (total_used as f64 / total_capacity as f64) * 100.0
                } else {
                    0.0
                };

                view! {
                    <div class="storage-overview">
                        <div class="overview-card">
                            <h4>"Total Capacity"</h4>
                            <span class="value">{format_bytes(total_capacity)}</span>
                        </div>
                        <div class="overview-card">
                            <h4>"Used"</h4>
                            <span class="value">{format_bytes(total_used)}</span>
                        </div>
                        <div class="overview-card">
                            <h4>"Available"</h4>
                            <span class="value">{format_bytes(total_capacity - total_used)}</span>
                        </div>
                        <div class="overview-card">
                            <h4>"Usage"</h4>
                            <span class="value">{format!("{:.1}%", usage_percent)}</span>
                        </div>
                    </div>
                }.into_view()
            } else {
                view! { <div></div> }.into_view()
            }}

            // Storage Pool List
            {move || if !loading.get() {
                view! {
                    <div class="storage-grid">
                        {move || pools.get().into_iter().map(|pool| {
                            let pool_id = pool.id.clone();
                            let usage_percent = if pool.total_bytes > 0 {
                                (pool.used_bytes as f64 / pool.total_bytes as f64) * 100.0
                            } else {
                                0.0
                            };
                            let usage_class = if usage_percent > 90.0 {
                                "critical"
                            } else if usage_percent > 75.0 {
                                "warning"
                            } else {
                                "healthy"
                            };

                            view! {
                                <div class={format!("storage-card status-{}", pool.status.to_lowercase())}>
                                    <div class="storage-header">
                                        <div class="storage-info">
                                            <h3>{&pool.name}</h3>
                                            <span class="type-badge">{&pool.pool_type}</span>
                                        </div>
                                        <span class={format!("status-badge {}", pool.status.to_lowercase())}>
                                            {&pool.status}
                                        </span>
                                    </div>

                                    <div class="storage-usage">
                                        <div class="usage-bar-container">
                                            <div class={format!("usage-bar {}", usage_class)}
                                                 style={format!("width: {}%", usage_percent)}></div>
                                        </div>
                                        <div class="usage-labels">
                                            <span>{format_bytes(pool.used_bytes)}" used"</span>
                                            <span>{format!("{:.1}%", usage_percent)}</span>
                                            <span>{format_bytes(pool.total_bytes)}" total"</span>
                                        </div>
                                    </div>

                                    <div class="storage-details">
                                        {pool.path.clone().map(|p| view! {
                                            <div class="detail-row">
                                                <span class="label">"Path:"</span>
                                                <code>{p}</code>
                                            </div>
                                        })}
                                        <div class="detail-row">
                                            <span class="label">"Available:"</span>
                                            <span>{format_bytes(pool.available_bytes)}</span>
                                        </div>
                                    </div>

                                    <div class="storage-actions">
                                        <button class="btn btn-secondary">
                                            "Browse"
                                        </button>
                                        <button class="btn btn-danger"
                                                on:click=move |_| delete_pool(pool_id.clone())>
                                            "Delete"
                                        </button>
                                    </div>
                                </div>
                            }
                        }).collect_view()}
                    </div>
                }.into_view()
            } else {
                view! { <div></div> }.into_view()
            }}

            // Empty state
            {move || if !loading.get() && pools.get().is_empty() {
                view! {
                    <div class="empty-state">
                        <div class="icon">"💾"</div>
                        <h3>"No Storage Pools"</h3>
                        <p>"Create a storage pool to store VM images and container data."</p>
                        <button class="btn btn-primary" on:click=move |_| set_show_create_modal.set(true)>
                            "Create Pool"
                        </button>
                    </div>
                }.into_view()
            } else {
                view! { <div></div> }.into_view()
            }}

            // Create Pool Modal
            {move || show_create_modal.get().then(|| view! {
                <div class="modal-overlay" on:click=move |_| set_show_create_modal.set(false)>
                    <div class="modal" on:click=|e| e.stop_propagation()>
                        <div class="modal-header">
                            <h3>"Create Storage Pool"</h3>
                            <button class="btn-close" on:click=move |_| set_show_create_modal.set(false)>
                                "x"
                            </button>
                        </div>
                        <div class="modal-body">
                            <div class="form-group">
                                <label>"Pool ID"</label>
                                <input type="text" class="form-control" placeholder="local-zfs"/>
                            </div>
                            <div class="form-group">
                                <label>"Display Name"</label>
                                <input type="text" class="form-control" placeholder="Local ZFS Storage"/>
                            </div>
                            <div class="form-group">
                                <label>"Type"</label>
                                <select class="form-control">
                                    <option value="Directory">"Directory"</option>
                                    <option value="Zfs">"ZFS"</option>
                                    <option value="Lvm">"LVM"</option>
                                    <option value="Ceph">"Ceph RBD"</option>
                                    <option value="Nfs">"NFS"</option>
                                    <option value="Iscsi">"iSCSI"</option>
                                </select>
                            </div>
                            <div class="form-group">
                                <label>"Path / Pool Name"</label>
                                <input type="text" class="form-control" placeholder="/var/lib/horcrux/images"/>
                            </div>
                        </div>
                        <div class="modal-footer">
                            <button class="btn btn-secondary" on:click=move |_| set_show_create_modal.set(false)>
                                "Cancel"
                            </button>
                            <button class="btn btn-primary">
                                "Create"
                            </button>
                        </div>
                    </div>
                </div>
            })}

            // Supported backends info
            <div class="info-box">
                <h4>"Supported Storage Backends"</h4>
                <div class="backend-grid">
                    <div class="backend">
                        <strong>"ZFS"</strong>
                        <span>"Snapshots, clones, compression"</span>
                    </div>
                    <div class="backend">
                        <strong>"Ceph RBD"</strong>
                        <span>"Distributed, HA storage"</span>
                    </div>
                    <div class="backend">
                        <strong>"LVM"</strong>
                        <span>"Logical volume management"</span>
                    </div>
                    <div class="backend">
                        <strong>"NFS"</strong>
                        <span>"Network file storage"</span>
                    </div>
                    <div class="backend">
                        <strong>"Directory"</strong>
                        <span>"Simple file-based storage"</span>
                    </div>
                    <div class="backend">
                        <strong>"iSCSI"</strong>
                        <span>"Block-level network storage"</span>
                    </div>
                </div>
            </div>
        </div>
    }
}
