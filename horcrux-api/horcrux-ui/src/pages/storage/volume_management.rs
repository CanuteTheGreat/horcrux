use leptos::*;
use crate::api::*;

#[component]
pub fn VolumeManagementPage() -> impl IntoView {
    let (volumes, set_volumes) = create_signal(Vec::<VolumeInfo>::new());
    let (pools, set_pools) = create_signal(Vec::<StoragePoolInfo>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);
    let (filters, set_filters) = create_signal(VolumeFilters::default());
    let (selected_volume, set_selected_volume) = create_signal(None::<VolumeInfo>);
    let (show_create_modal, set_show_create_modal) = create_signal(false);
    let (show_resize_modal, set_show_resize_modal) = create_signal(false);
    let (show_snapshots_modal, set_show_snapshots_modal) = create_signal(false);
    let (create_form, set_create_form) = create_signal(CreateVolumeForm::default());
    let (new_size_gb, set_new_size_gb) = create_signal(0u64);
    let (volume_snapshots, set_volume_snapshots) = create_signal(Vec::<VolumeSnapshot>::new());

    // Load volumes and pools
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);

            // Load pools for filtering
            match get_storage_pools().await {
                Ok(pool_list) => set_pools.set(pool_list),
                Err(_) => {}
            }

            // Load volumes
            match get_volumes(filters.get()).await {
                Ok(volume_list) => {
                    set_volumes.set(volume_list);
                    set_error.set(None);
                }
                Err(e) => {
                    set_error.set(Some(format!("Failed to load volumes: {}", e)));
                }
            }
            set_loading.set(false);
        });
    });

    let filtered_volumes = move || {
        let f = filters.get();
        volumes.get().into_iter().filter(|v| {
            let search_match = f.search.is_empty() ||
                v.name.to_lowercase().contains(&f.search.to_lowercase()) ||
                v.pool_name.to_lowercase().contains(&f.search.to_lowercase());
            let pool_match = f.pool_id.as_ref().map_or(true, |p| p.is_empty() || &v.pool_id == p);
            let type_match = f.volume_type.as_ref().map_or(true, |t| t.is_empty() || &v.volume_type == t);
            let status_match = f.status.as_ref().map_or(true, |s| s.is_empty() || &v.status == s);
            let attached_match = !f.attached_only || v.attached_to.is_some();

            search_match && pool_match && type_match && status_match && attached_match
        }).collect::<Vec<_>>()
    };

    let total_size = move || {
        filtered_volumes().iter().map(|v| v.size_bytes).sum::<u64>()
    };

    let total_used = move || {
        filtered_volumes().iter().map(|v| v.used_bytes).sum::<u64>()
    };

    let create_volume = move |_| {
        let form = create_form.get();
        spawn_local(async move {
            match create_new_volume(form).await {
                Ok(_) => {
                    set_show_create_modal.set(false);
                    set_create_form.set(CreateVolumeForm::default());
                    // Refresh volumes
                    if let Ok(volume_list) = get_volumes(filters.get()).await {
                        set_volumes.set(volume_list);
                    }
                }
                Err(e) => {
                    set_error.set(Some(format!("Failed to create volume: {}", e)));
                }
            }
        });
    };

    let resize_volume = move |_| {
        if let Some(vol) = selected_volume.get() {
            let volume_id = vol.id.clone();
            let size = new_size_gb.get();
            spawn_local(async move {
                match resize_volume_api(&volume_id, size).await {
                    Ok(_) => {
                        set_show_resize_modal.set(false);
                        if let Ok(volume_list) = get_volumes(filters.get()).await {
                            set_volumes.set(volume_list);
                        }
                    }
                    Err(e) => {
                        set_error.set(Some(format!("Failed to resize volume: {}", e)));
                    }
                }
            });
        }
    };

    let delete_volume = move |volume_id: String| {
        spawn_local(async move {
            match delete_volume_api(&volume_id).await {
                Ok(_) => {
                    if let Ok(volume_list) = get_volumes(filters.get()).await {
                        set_volumes.set(volume_list);
                    }
                }
                Err(e) => {
                    set_error.set(Some(format!("Failed to delete volume: {}", e)));
                }
            }
        });
    };

    let load_snapshots = move |volume_id: String| {
        spawn_local(async move {
            match get_volume_snapshots(&volume_id).await {
                Ok(snaps) => {
                    set_volume_snapshots.set(snaps);
                    set_show_snapshots_modal.set(true);
                }
                Err(e) => {
                    set_error.set(Some(format!("Failed to load snapshots: {}", e)));
                }
            }
        });
    };

    view! {
        <div class="volume-management-page">
            <div class="page-header">
                <h1 class="page-title">Volume Management</h1>
                <p class="page-description">
                    Create, manage, and monitor storage volumes across all pools
                </p>
                <button
                    class="btn btn-primary"
                    on:click=move |_| set_show_create_modal.set(true)
                >
                    Create Volume
                </button>
            </div>

            // Statistics Overview
            <div class="volume-stats">
                <div class="stat-card">
                    <span class="stat-label">Total Volumes</span>
                    <span class="stat-value">{move || filtered_volumes().len()}</span>
                </div>
                <div class="stat-card">
                    <span class="stat-label">Total Capacity</span>
                    <span class="stat-value">{move || format_bytes(total_size())}</span>
                </div>
                <div class="stat-card">
                    <span class="stat-label">Used Space</span>
                    <span class="stat-value">{move || format_bytes(total_used())}</span>
                </div>
                <div class="stat-card">
                    <span class="stat-label">Attached</span>
                    <span class="stat-value">
                        {move || filtered_volumes().iter().filter(|v| v.attached_to.is_some()).count()}
                    </span>
                </div>
            </div>

            // Filters
            <div class="filters-section">
                <div class="filter-row">
                    <input
                        type="text"
                        class="filter-input"
                        placeholder="Search volumes..."
                        on:input=move |ev| {
                            let mut f = filters.get();
                            f.search = event_target_value(&ev);
                            set_filters.set(f);
                        }
                    />
                    <select
                        class="filter-select"
                        on:change=move |ev| {
                            let mut f = filters.get();
                            let val = event_target_value(&ev);
                            f.pool_id = if val.is_empty() { None } else { Some(val) };
                            set_filters.set(f);
                        }
                    >
                        <option value="">All Pools</option>
                        {move || pools.get().into_iter().map(|p| view! {
                            <option value={p.id.clone()}>{p.name}</option>
                        }).collect::<Vec<_>>()}
                    </select>
                    <select
                        class="filter-select"
                        on:change=move |ev| {
                            let mut f = filters.get();
                            let val = event_target_value(&ev);
                            f.volume_type = if val.is_empty() { None } else { Some(val) };
                            set_filters.set(f);
                        }
                    >
                        <option value="">All Types</option>
                        <option value="raw">Raw</option>
                        <option value="qcow2">QCOW2</option>
                        <option value="lvm">LVM</option>
                        <option value="zvol">ZFS Volume</option>
                    </select>
                    <select
                        class="filter-select"
                        on:change=move |ev| {
                            let mut f = filters.get();
                            let val = event_target_value(&ev);
                            f.status = if val.is_empty() { None } else { Some(val) };
                            set_filters.set(f);
                        }
                    >
                        <option value="">All Status</option>
                        <option value="available">Available</option>
                        <option value="in_use">In Use</option>
                        <option value="error">Error</option>
                    </select>
                    <label class="checkbox-label">
                        <input
                            type="checkbox"
                            on:change=move |ev| {
                                let mut f = filters.get();
                                f.attached_only = event_target_checked(&ev);
                                set_filters.set(f);
                            }
                        />
                        Attached Only
                    </label>
                </div>
            </div>

            {move || if loading.get() {
                view! {
                    <div class="loading-container">
                        <div class="spinner"></div>
                        <p>Loading volumes...</p>
                    </div>
                }.into_view()
            } else if let Some(err) = error.get() {
                view! {
                    <div class="error-container">
                        <div class="error-message">
                            <h3>Error</h3>
                            <p>{err}</p>
                        </div>
                    </div>
                }.into_view()
            } else {
                view! {
                    <div class="volumes-table-container">
                        <table class="volumes-table">
                            <thead>
                                <tr>
                                    <th>Name</th>
                                    <th>Pool</th>
                                    <th>Type</th>
                                    <th>Size</th>
                                    <th>Used</th>
                                    <th>Attached To</th>
                                    <th>Snapshots</th>
                                    <th>Status</th>
                                    <th>Actions</th>
                                </tr>
                            </thead>
                            <tbody>
                                {move || filtered_volumes().into_iter().map(|volume| {
                                    let vol_id = volume.id.clone();
                                    let vol_id_2 = volume.id.clone();
                                    let vol_id_3 = volume.id.clone();
                                    let vol_clone = volume.clone();
                                    let vol_clone_2 = volume.clone();
                                    let usage_pct = if volume.size_bytes > 0 {
                                        (volume.used_bytes as f64 / volume.size_bytes as f64 * 100.0) as u32
                                    } else { 0 };

                                    view! {
                                        <tr class={format!("volume-row status-{}", volume.status)}>
                                            <td>
                                                <div class="volume-name">
                                                    <span class="name">{volume.name.clone()}</span>
                                                    <span class="format-badge">{volume.format.clone()}</span>
                                                </div>
                                            </td>
                                            <td>{volume.pool_name.clone()}</td>
                                            <td>
                                                <span class="type-badge">{volume.volume_type.to_uppercase()}</span>
                                            </td>
                                            <td>{format_bytes(volume.size_bytes)}</td>
                                            <td>
                                                <div class="usage-cell">
                                                    <div class="mini-usage-bar">
                                                        <div
                                                            class="mini-usage-fill"
                                                            style={format!("width: {}%", usage_pct)}
                                                        ></div>
                                                    </div>
                                                    <span>{format_bytes(volume.used_bytes)}</span>
                                                </div>
                                            </td>
                                            <td>
                                                {if let Some(ref name) = volume.attached_name {
                                                    view! {
                                                        <span class="attached-badge">{name.clone()}</span>
                                                    }.into_view()
                                                } else {
                                                    view! {
                                                        <span class="not-attached">-</span>
                                                    }.into_view()
                                                }}
                                            </td>
                                            <td>
                                                <button
                                                    class="snapshots-link"
                                                    on:click=move |_| load_snapshots(vol_id_3.clone())
                                                >
                                                    {volume.snapshots}
                                                </button>
                                            </td>
                                            <td>
                                                <span class={format!("status-badge status-{}", volume.status)}>
                                                    {volume.status.clone()}
                                                </span>
                                            </td>
                                            <td>
                                                <div class="action-buttons">
                                                    <button
                                                        class="btn btn-sm btn-secondary"
                                                        title="Resize"
                                                        disabled={volume.attached_to.is_some()}
                                                        on:click=move |_| {
                                                            set_selected_volume.set(Some(vol_clone.clone()));
                                                            set_new_size_gb.set(vol_clone.size_bytes / (1024 * 1024 * 1024));
                                                            set_show_resize_modal.set(true);
                                                        }
                                                    >
                                                        Resize
                                                    </button>
                                                    <button
                                                        class="btn btn-sm btn-danger"
                                                        title="Delete"
                                                        disabled={volume.attached_to.is_some()}
                                                        on:click=move |_| {
                                                            if web_sys::window()
                                                                .unwrap()
                                                                .confirm_with_message(&format!("Delete volume {}?", vol_clone_2.name))
                                                                .unwrap_or(false)
                                                            {
                                                                delete_volume(vol_id.clone());
                                                            }
                                                        }
                                                    >
                                                        Delete
                                                    </button>
                                                </div>
                                            </td>
                                        </tr>
                                    }
                                }).collect::<Vec<_>>()}
                            </tbody>
                        </table>
                    </div>
                }.into_view()
            }}

            // Create Volume Modal
            {move || if show_create_modal.get() {
                view! {
                    <div class="modal-overlay">
                        <div class="modal modal-lg">
                            <div class="modal-header">
                                <h2>Create New Volume</h2>
                                <button
                                    class="modal-close"
                                    on:click=move |_| set_show_create_modal.set(false)
                                >
                                    x
                                </button>
                            </div>
                            <div class="modal-body">
                                <div class="form-grid">
                                    <div class="form-group">
                                        <label>Volume Name</label>
                                        <input
                                            type="text"
                                            class="form-input"
                                            placeholder="my-volume"
                                            prop:value=move || create_form.get().name
                                            on:input=move |ev| {
                                                let mut form = create_form.get();
                                                form.name = event_target_value(&ev);
                                                set_create_form.set(form);
                                            }
                                        />
                                    </div>
                                    <div class="form-group">
                                        <label>Storage Pool</label>
                                        <select
                                            class="form-select"
                                            on:change=move |ev| {
                                                let mut form = create_form.get();
                                                form.pool_id = event_target_value(&ev);
                                                set_create_form.set(form);
                                            }
                                        >
                                            <option value="">Select Pool</option>
                                            {move || pools.get().into_iter().map(|p| view! {
                                                <option value={p.id.clone()}>{p.name} ({format_bytes(p.available_bytes)} free)</option>
                                            }).collect::<Vec<_>>()}
                                        </select>
                                    </div>
                                    <div class="form-group">
                                        <label>Size (GB)</label>
                                        <input
                                            type="number"
                                            class="form-input"
                                            min="1"
                                            prop:value=move || create_form.get().size_gb
                                            on:input=move |ev| {
                                                let mut form = create_form.get();
                                                form.size_gb = event_target_value(&ev).parse().unwrap_or(0);
                                                set_create_form.set(form);
                                            }
                                        />
                                    </div>
                                    <div class="form-group">
                                        <label>Volume Type</label>
                                        <select
                                            class="form-select"
                                            on:change=move |ev| {
                                                let mut form = create_form.get();
                                                form.volume_type = event_target_value(&ev);
                                                set_create_form.set(form);
                                            }
                                        >
                                            <option value="qcow2">QCOW2 (Recommended)</option>
                                            <option value="raw">Raw</option>
                                            <option value="lvm">LVM</option>
                                            <option value="zvol">ZFS Volume</option>
                                        </select>
                                    </div>
                                    <div class="form-group">
                                        <label>Format</label>
                                        <select
                                            class="form-select"
                                            on:change=move |ev| {
                                                let mut form = create_form.get();
                                                form.format = event_target_value(&ev);
                                                set_create_form.set(form);
                                            }
                                        >
                                            <option value="ext4">ext4</option>
                                            <option value="xfs">XFS</option>
                                            <option value="btrfs">Btrfs</option>
                                            <option value="none">None (raw block)</option>
                                        </select>
                                    </div>
                                    <div class="form-group checkbox-group">
                                        <label class="checkbox-label">
                                            <input
                                                type="checkbox"
                                                prop:checked=move || create_form.get().thin_provisioned
                                                on:change=move |ev| {
                                                    let mut form = create_form.get();
                                                    form.thin_provisioned = event_target_checked(&ev);
                                                    set_create_form.set(form);
                                                }
                                            />
                                            Thin Provisioned
                                        </label>
                                        <p class="form-hint">Allocate storage on demand rather than upfront</p>
                                    </div>
                                    <div class="form-group full-width">
                                        <label>Description (optional)</label>
                                        <textarea
                                            class="form-textarea"
                                            rows="2"
                                            prop:value=move || create_form.get().description
                                            on:input=move |ev| {
                                                let mut form = create_form.get();
                                                form.description = event_target_value(&ev);
                                                set_create_form.set(form);
                                            }
                                        ></textarea>
                                    </div>
                                </div>
                            </div>
                            <div class="modal-footer">
                                <button
                                    class="btn btn-secondary"
                                    on:click=move |_| set_show_create_modal.set(false)
                                >
                                    Cancel
                                </button>
                                <button
                                    class="btn btn-primary"
                                    disabled=move || {
                                        let form = create_form.get();
                                        form.name.is_empty() || form.pool_id.is_empty() || form.size_gb == 0
                                    }
                                    on:click=create_volume
                                >
                                    Create Volume
                                </button>
                            </div>
                        </div>
                    </div>
                }.into_view()
            } else {
                view! {}.into_view()
            }}

            // Resize Volume Modal
            {move || if show_resize_modal.get() {
                let vol = selected_volume.get();
                view! {
                    <div class="modal-overlay">
                        <div class="modal">
                            <div class="modal-header">
                                <h2>Resize Volume</h2>
                                <button
                                    class="modal-close"
                                    on:click=move |_| set_show_resize_modal.set(false)
                                >
                                    x
                                </button>
                            </div>
                            <div class="modal-body">
                                {if let Some(v) = vol {
                                    view! {
                                        <div class="resize-info">
                                            <p><strong>Volume:</strong> {v.name}</p>
                                            <p><strong>Current Size:</strong> {format_bytes(v.size_bytes)}</p>
                                        </div>
                                        <div class="form-group">
                                            <label>New Size (GB)</label>
                                            <input
                                                type="number"
                                                class="form-input"
                                                min={v.size_bytes / (1024 * 1024 * 1024)}
                                                prop:value=move || new_size_gb.get()
                                                on:input=move |ev| {
                                                    set_new_size_gb.set(event_target_value(&ev).parse().unwrap_or(0));
                                                }
                                            />
                                            <p class="form-hint">Note: Volumes can only be increased in size</p>
                                        </div>
                                    }.into_view()
                                } else {
                                    view! {}.into_view()
                                }}
                            </div>
                            <div class="modal-footer">
                                <button
                                    class="btn btn-secondary"
                                    on:click=move |_| set_show_resize_modal.set(false)
                                >
                                    Cancel
                                </button>
                                <button
                                    class="btn btn-primary"
                                    on:click=resize_volume
                                >
                                    Resize
                                </button>
                            </div>
                        </div>
                    </div>
                }.into_view()
            } else {
                view! {}.into_view()
            }}

            // Snapshots Modal
            {move || if show_snapshots_modal.get() {
                view! {
                    <div class="modal-overlay">
                        <div class="modal modal-lg">
                            <div class="modal-header">
                                <h2>Volume Snapshots</h2>
                                <button
                                    class="modal-close"
                                    on:click=move |_| set_show_snapshots_modal.set(false)
                                >
                                    x
                                </button>
                            </div>
                            <div class="modal-body">
                                {if volume_snapshots.get().is_empty() {
                                    view! {
                                        <div class="empty-state">
                                            <p>No snapshots for this volume</p>
                                        </div>
                                    }.into_view()
                                } else {
                                    view! {
                                        <table class="snapshots-table">
                                            <thead>
                                                <tr>
                                                    <th>Name</th>
                                                    <th>Size</th>
                                                    <th>Created</th>
                                                    <th>Description</th>
                                                </tr>
                                            </thead>
                                            <tbody>
                                                {volume_snapshots.get().into_iter().map(|snap| view! {
                                                    <tr>
                                                        <td>{snap.name}</td>
                                                        <td>{format_bytes(snap.size_bytes)}</td>
                                                        <td>{snap.created_at}</td>
                                                        <td>{snap.description.unwrap_or_default()}</td>
                                                    </tr>
                                                }).collect::<Vec<_>>()}
                                            </tbody>
                                        </table>
                                    }.into_view()
                                }}
                            </div>
                            <div class="modal-footer">
                                <button
                                    class="btn btn-secondary"
                                    on:click=move |_| set_show_snapshots_modal.set(false)
                                >
                                    Close
                                </button>
                            </div>
                        </div>
                    </div>
                }.into_view()
            } else {
                view! {}.into_view()
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
