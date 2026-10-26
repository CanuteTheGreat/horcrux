//! Storage Pool Management Page

use crate::api::{self, delete_json, fetch_json, post_json, PoolVolume};
use leptos::*;
use serde::Serialize;

/// Storage pool (adapter over the API's `storage::StoragePool`).
pub use crate::api::StoragePoolInfo as StoragePool;

#[derive(Debug, Clone, Serialize)]
struct AddStoragePoolReq {
    name: String,
    storage_type: String,
    path: String,
}

/// Storage Management Page Component
#[component]
pub fn StorageManagement() -> impl IntoView {
    let (pools, set_pools) = create_signal(Vec::<StoragePool>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);
    let (show_create_modal, set_show_create_modal) = create_signal(false);

    let name_ref: NodeRef<html::Input> = create_node_ref();
    let type_ref: NodeRef<html::Select> = create_node_ref();
    let path_ref: NodeRef<html::Input> = create_node_ref();

    // Fetch storage pools on mount
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);
            // NOTE: the real backend route is /api/storage/pools (see
            // horcrux-api/src/main.rs), not /api/storage — this page was
            // silently 404ing on every load until fixed here.
            match fetch_json::<Vec<StoragePool>>("/api/storage/pools").await {
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
            let url = format!("/api/storage/pools/{}", pool_id);
            match delete_json(&url).await {
                Ok(()) => {
                    if let Ok(data) = fetch_json::<Vec<StoragePool>>("/api/storage/pools").await {
                        set_pools.set(data);
                    }
                }
                Err(e) => {
                    set_error.set(Some(format!("Delete failed: {}", e.message)));
                }
            }
        });
    };

    // Create storage pool (previously a dead button — the modal opened but
    // its "Create" submit never called the API at all, so a Horcrux admin
    // could never actually create a storage pool through the UI)
    let create_pool = move |_| {
        let name = name_ref.get().map(|el| el.value()).unwrap_or_default();
        if name.is_empty() {
            set_error.set(Some("Pool ID is required".to_string()));
            return;
        }
        let storage_type = type_ref
            .get()
            .map(|el| el.value())
            .unwrap_or_else(|| "Directory".to_string());
        let path = path_ref.get().map(|el| el.value()).unwrap_or_default();
        if path.is_empty() {
            set_error.set(Some("Path / Pool name is required".to_string()));
            return;
        }

        let req = AddStoragePoolReq {
            name,
            storage_type,
            path,
        };

        spawn_local(async move {
            match post_json::<StoragePool, AddStoragePoolReq>("/api/storage/pools", &req).await {
                Ok(_) => {
                    set_show_create_modal.set(false);
                    if let Ok(data) = fetch_json::<Vec<StoragePool>>("/api/storage/pools").await {
                        set_pools.set(data);
                    }
                }
                Err(e) => {
                    set_error.set(Some(format!("Create pool failed: {}", e.message)));
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
                            let volumes_pool_id = pool.id.clone();
                            let show_volumes = create_rw_signal(false);
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
                                        <button class="btn btn-secondary pool-volumes-toggle"
                                                on:click=move |_| show_volumes.update(|v| *v = !*v)>
                                            {move || if show_volumes.get() { "Hide volumes" } else { "Volumes" }}
                                        </button>
                                        <button class="btn btn-danger"
                                                on:click=move |_| delete_pool(pool_id.clone())>
                                            "Delete"
                                        </button>
                                    </div>
                                    {move || show_volumes.get().then(|| view! {
                                        <PoolVolumes pool_id=volumes_pool_id.clone()/>
                                    })}
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
                                <input node_ref=name_ref type="text" class="form-control" placeholder="local-zfs"/>
                            </div>
                            <div class="form-group">
                                <label>"Type"</label>
                                <select node_ref=type_ref class="form-control">
                                    <option value="Directory">"Directory"</option>
                                    <option value="Zfs">"ZFS"</option>
                                    <option value="Lvm">"LVM"</option>
                                    <option value="Ceph">"Ceph RBD"</option>
                                    <option value="Nfs">"NFS"</option>
                                    <option value="Iscsi">"iSCSI"</option>
                                    <option value="CephFs">"CephFS (mounted)"</option>
                                    <option value="Cifs">"CIFS / SMB"</option>
                                    <option value="GlusterFs">"GlusterFS"</option>
                                    <option value="BtrFs">"BtrFS"</option>
                                    <option value="S3">"S3 (backups)"</option>
                                </select>
                            </div>
                            <div class="form-group">
                                <label>"Path / Pool Name"</label>
                                <input node_ref=path_ref type="text" class="form-control" placeholder="/var/lib/horcrux/images"/>
                            </div>
                        </div>
                        <div class="modal-footer">
                            <button class="btn btn-secondary" on:click=move |_| set_show_create_modal.set(false)>
                                "Cancel"
                            </button>
                            <button class="btn btn-primary" on:click=create_pool>
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

fn human_size(bytes: u64) -> String {
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else {
        format!("{:.1} MB", b / MB)
    }
}

/// The volumes of one pool: list, create (name + GB), grow, delete.
/// Volumes that are a VM's disk can't be deleted (the API answers 409
/// naming the VM).
#[component]
fn PoolVolumes(pool_id: String) -> impl IntoView {
    let pool_id = store_value(pool_id);
    let (volumes, set_volumes) = create_signal(None::<Vec<PoolVolume>>);
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let name = create_rw_signal(String::new());
    let size = create_rw_signal(1u64);

    let reload = move || {
        spawn_local(async move {
            match api::list_pool_volumes(&pool_id.get_value()).await {
                Ok(v) => set_volumes.set(Some(v)),
                Err(e) => {
                    set_volumes.set(Some(Vec::new()));
                    set_error.set(Some(format!("Could not list volumes: {}", e.message)))
                }
            }
        });
    };
    reload();
    let report = move |r: Result<String, api::ApiError>| match r {
        Ok(msg) => {
            set_error.set(None);
            set_notice.set(Some(msg));
            reload();
        }
        Err(e) => {
            set_notice.set(None);
            set_error.set(Some(e.message));
        }
    };

    let create = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let n = name.get_untracked().trim().to_string();
        let gb = size.get_untracked();
        spawn_local(async move {
            let r = api::create_pool_volume(&pool_id.get_value(), &n, gb)
                .await
                .map(|v| format!("Created {} ({})", v.name, human_size(v.size_bytes)));
            if r.is_ok() {
                name.set(String::new());
            }
            report(r);
        });
    };

    view! {
        <div class="pool-volumes" data-pool=pool_id.get_value()>
            <h4>"Volumes"</h4>
            {move || notice.get().map(|n| view! { <div class="alert alert-info pool-volume-notice">{n}</div> })}
            {move || error.get().map(|e| view! { <div class="alert alert-error pool-volume-error">{e}</div> })}
            {move || match volumes.get() {
                None => view! { <p class="loading">"Loading volumes..."</p> }.into_view(),
                Some(v) if v.is_empty() => view! { <p class="empty-volumes">"No volumes in this pool."</p> }.into_view(),
                Some(v) => view! {
                    <table class="table pool-volume-table">
                        <thead><tr><th>"Name"</th><th>"Format"</th><th>"Size"</th><th>"Allocated"</th><th>"Path"</th><th></th></tr></thead>
                        <tbody>
                            {v.into_iter().map(|vol| {
                                let grow_to = create_rw_signal(vol.size_bytes.div_ceil(1024 * 1024 * 1024) + 1);
                                let n_resize = vol.name.clone();
                                let n_delete = vol.name.clone();
                                let grow = move |_| {
                                    let n = n_resize.clone();
                                    let gb = grow_to.get_untracked();
                                    spawn_local(async move {
                                        report(api::resize_pool_volume(&pool_id.get_value(), &n, gb)
                                            .await
                                            .map(|v| format!("{} is now {}", v.name, human_size(v.size_bytes))));
                                    });
                                };
                                let remove = move |_| {
                                    let n = n_delete.clone();
                                    let ok = web_sys::window()
                                        .and_then(|w| w.confirm_with_message(&format!("Delete volume {n}? Its data is lost.")).ok())
                                        .unwrap_or(false);
                                    if !ok {
                                        return;
                                    }
                                    spawn_local(async move {
                                        report(api::delete_pool_volume(&pool_id.get_value(), &n)
                                            .await
                                            .map(|_| format!("Deleted {n}")));
                                    });
                                };
                                view! {
                                    <tr class="pool-volume" data-volume=vol.name.clone()>
                                        <td>{vol.name.clone()}</td>
                                        <td>{vol.format.clone()}</td>
                                        <td class="pool-volume-size">{human_size(vol.size_bytes)}</td>
                                        <td>{human_size(vol.allocated_bytes)}</td>
                                        <td><code>{vol.path.clone()}</code></td>
                                        <td>
                                            <input type="number" min="1" class="pool-volume-grow-size" prop:value=grow_to
                                                on:input=move |ev| grow_to.set(event_target_value(&ev).parse().unwrap_or(0))/>
                                            " GB "
                                            <button type="button" class="btn btn-sm btn-secondary pool-volume-grow" on:click=grow>"Grow"</button>
                                            <button type="button" class="btn btn-sm btn-danger pool-volume-delete" on:click=remove>"Delete"</button>
                                        </td>
                                    </tr>
                                }
                            }).collect_view()}
                        </tbody>
                    </table>
                }.into_view(),
            }}
            <form class="form-row pool-volume-create" on:submit=create>
                <input type="text" class="pool-volume-name" placeholder="volume name (letters, digits, . _ -)"
                    pattern="[A-Za-z0-9_][A-Za-z0-9._\\-]{0,63}" required=true
                    prop:value=name on:input=move |ev| name.set(event_target_value(&ev))/>
                <input type="number" min="1" class="pool-volume-size-input" prop:value=size
                    on:input=move |ev| size.set(event_target_value(&ev).parse().unwrap_or(1))/>
                " GB "
                <button type="submit" class="btn btn-sm btn-primary pool-volume-create-btn">"Create volume"</button>
            </form>
        </div>
    }
}
