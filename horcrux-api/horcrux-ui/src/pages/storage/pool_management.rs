//! Storage Pool Management Page

use crate::api::{self, delete_json, fetch_json, post_json, PoolVolume};
use leptos::*;
use leptos_router::use_query_map;
use serde::Serialize;

/// Storage pool (adapter over the API's `storage::StoragePool`).
pub use crate::api::StoragePoolInfo as StoragePool;

#[derive(Debug, Clone, Serialize)]
struct AddStoragePoolReq {
    name: String,
    storage_type: String,
    path: String,
}

/// Pool types the Add dialog offers: (API type, label, Some(reason) when the
/// type can't be added from here yet). CIFS, GlusterFS and S3 used to be
/// offered with one "Path" field and were accepted, but Horcrux doesn't keep
/// them mounted, so the new pool could hold nothing.
pub const POOL_TYPES: [(&str, &str, Option<&str>); 11] = [
    ("Directory", "Directory", None),
    ("Zfs", "ZFS", None),
    ("Lvm", "LVM", None),
    ("Ceph", "Ceph RBD", None),
    ("Nfs", "NFS", None),
    ("Iscsi", "iSCSI", None),
    ("CephFs", "CephFS (mounted)", None),
    ("BtrFs", "BtrFS", None),
    ("Cifs", "CIFS / SMB", Some("not yet: Horcrux doesn't keep SMB shares mounted; mount it on the host and add it as a Directory")),
    ("GlusterFs", "GlusterFS", Some("not yet: Horcrux doesn't keep GlusterFS volumes mounted; mount it on the host and add it as a Directory")),
    ("S3", "S3", Some("not VM storage: S3 holds backups")),
];

/// The two inputs the Add dialog shows for a pool type: (label, placeholder)
/// of the first, and of the second when the type needs one.
pub fn pool_fields(
    kind: &str,
) -> (
    (&'static str, &'static str),
    Option<(&'static str, &'static str)>,
) {
    match kind {
        "Zfs" => (("ZFS pool or dataset", "tank/vms"), None),
        "Lvm" => (("Volume group", "vg0"), None),
        "Ceph" => (("Ceph pool", "rbd"), None),
        "Nfs" => (
            ("NFS server", "nas.example.lan"),
            Some(("Export", "/export/vms")),
        ),
        "Iscsi" => (
            ("Portal (host:port)", "10.0.0.5:3260"),
            Some(("Target IQN", "iqn.2024-01.lan.example:vms")),
        ),
        "CephFs" => (("Mounted CephFS directory", "/mnt/cephfs/vms"), None),
        "BtrFs" => (("Btrfs directory", "/mnt/btrfs/vms"), None),
        _ => (("Directory", "/var/lib/horcrux/images"), None),
    }
}

/// The API's `path` for a pool from the dialog's inputs.
pub fn compose_pool_path(kind: &str, a: &str, b: &str) -> Result<String, String> {
    let (a, b) = (a.trim(), b.trim());
    let ((label_a, _), second) = pool_fields(kind);
    if a.is_empty() {
        return Err(format!("{label_a} is required"));
    }
    if let Some((label_b, _)) = second {
        if b.is_empty() {
            return Err(format!("{label_b} is required"));
        }
    }
    match kind {
        "Nfs" => Ok(format!("{a}:/{}", b.trim_start_matches('/'))),
        "Iscsi" => Ok(format!("iscsi://{a}/{b}")),
        "Directory" | "CephFs" | "BtrFs" if !a.starts_with('/') => Err(format!(
            "{label_a} must be an absolute path (starting with /)"
        )),
        _ => Ok(a.to_string()),
    }
}

/// A pool path for display: a password in `scheme://user:password@host`
/// is never shown.
pub fn display_path(p: &str) -> String {
    if let Some((scheme, rest)) = p.split_once("://") {
        if let Some((cred, host)) = rest.split_once('@') {
            if let Some((user, _)) = cred.split_once(':') {
                return format!("{scheme}://{user}:***@{host}");
            }
        }
    }
    p.to_string()
}

/// Storage Management Page Component
#[component]
pub fn StorageManagement() -> impl IntoView {
    // `?pool=<id>`: the dashboard links to one pool; its card is highlighted
    // and scrolled into view.
    let query = use_query_map();
    let focus_pool = move || query.with(|q| q.get("pool").cloned().unwrap_or_default());
    let (pools, set_pools) = create_signal(Vec::<StoragePool>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);
    let (pool_notice, set_notice) = create_signal(None::<String>);
    let (show_create_modal, set_show_create_modal) = create_signal(false);

    let name_ref: NodeRef<html::Input> = create_node_ref();
    let (new_type, set_new_type) = create_signal("Directory".to_string());
    let (field_a, set_field_a) = create_signal(String::new());
    let (field_b, set_field_b) = create_signal(String::new());
    let (dialog_error, set_dialog_error) = create_signal(None::<String>);
    let (creating, set_creating) = create_signal(false);
    let open_create = move || {
        set_new_type.set("Directory".to_string());
        set_field_a.set(String::new());
        set_field_b.set(String::new());
        set_dialog_error.set(None);
        set_show_create_modal.set(true);
    };

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
        // Removing a pool used to run on the first click, with no question.
        let name = pools
            .get_untracked()
            .iter()
            .find(|p| p.id == pool_id)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| pool_id.clone());
        let ok = web_sys::window()
            .and_then(|w| {
                w.confirm_with_message(&format!(
                    "Remove storage pool {name} from Horcrux? Its files stay on disk, but VM disks, ISOs and backups on it can no longer be used from Horcrux."
                ))
                .ok()
            })
            .unwrap_or(false);
        if !ok {
            return;
        }
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
        let name = name_ref
            .get()
            .map(|el| el.value())
            .unwrap_or_default()
            .trim()
            .to_string();
        if name.is_empty() {
            set_dialog_error.set(Some("Pool ID is required".to_string()));
            return;
        }
        let storage_type = new_type.get();
        let path = match compose_pool_path(&storage_type, &field_a.get(), &field_b.get()) {
            Ok(p) => p,
            Err(e) => {
                set_dialog_error.set(Some(e));
                return;
            }
        };
        set_creating.set(true);

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
                // Shown in the dialog (the page's banner is behind it).
                Err(e) => set_dialog_error.set(Some(e.message)),
            }
            set_creating.set(false);
        });
    };

    view! {
        <div class="page storage-management">
            <header class="page-header">
                <h2>"Storage Pools"</h2>
                <button class="btn btn-primary" on:click=move |_| open_create()>
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

            {move || pool_notice.get().map(|n| view! {
                <div class="alert alert-info" id="pool-notice">
                    <span>{n}</span>
                    <button class="btn-close" aria-label="Dismiss" on:click=move |_| set_notice.set(None)>"x"</button>
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
                // Pools on one filesystem / zpool share its space: count it once.
                let (total_capacity, total_used, _) = crate::api::storage_totals(&pools.get());
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
                            let show_edit = create_rw_signal(false);
                            let edit_pool_id = pool.id.clone();
                            let edit_name = create_rw_signal(pool.name.clone());
                            let edit_enabled = create_rw_signal(pool.status != "disabled");
                            let edit_error = create_rw_signal(None::<String>);
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
                                <div class={format!("storage-card status-{}{}", pool.status.to_lowercase(), if focus_pool() == pool.id { " pool-focus" } else { "" })}
                                    id={format!("pool-{}", pool.id)}>
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
                                                <code>{display_path(&p)}</code>
                                            </div>
                                        })}
                                        <div class="detail-row">
                                            <span class="label">"Available:"</span>
                                            <span>{format_bytes(pool.available_bytes)}</span>
                                        </div>
                                    </div>

                                    <div class="storage-actions">
                                        <button class="btn btn-sm btn-secondary pool-volumes-toggle"
                                                on:click=move |_| show_volumes.update(|v| *v = !*v)>
                                            {move || if show_volumes.get() { "Hide volumes" } else { "Volumes" }}
                                        </button>
                                        <button class="btn btn-sm btn-secondary pool-edit-toggle"
                                                on:click=move |_| show_edit.update(|v| *v = !*v)>
                                            "Edit"
                                        </button>
                                        <button class="btn btn-sm btn-danger"
                                                on:click=move |_| delete_pool(pool_id.clone())>
                                            "Delete"
                                        </button>
                                    </div>
                                    // Storage > Edit (Proxmox): name and enabled; the location
                                    // is what the pool is, so moving it = remove + add.
                                    {move || show_edit.get().then(|| {
                                        let id = edit_pool_id.clone();
                                        let save = move |_| {
                                            let id = id.clone();
                                            let n = edit_name.get_untracked();
                                            let en = edit_enabled.get_untracked();
                                            edit_error.set(None);
                                            spawn_local(async move {
                                                match api::update_storage_pool(&id, n.trim(), en).await {
                                                    Ok(p) => {
                                                        set_error.set(None);
                                                        set_notice.set(Some(format!(
                                                            "Storage pool {} saved ({}).",
                                                            p.name,
                                                            if en { "enabled" } else { "disabled: no new disks, ISOs or backups go to it" }
                                                        )));
                                                        if let Ok(data) = fetch_json::<Vec<StoragePool>>("/api/storage/pools").await {
                                                            set_pools.set(data);
                                                        }
                                                    }
                                                    Err(e) => edit_error.set(Some(e.message)),
                                                }
                                            });
                                        };
                                        view! {
                                            <div class="pool-edit-form">
                                                <p class="form-hint">"Name and state can be changed here. The location is fixed: remove the pool and add it again to move it."</p>
                                                <div class="form-group">
                                                    <label for=format!("pool-edit-name-{}", edit_pool_id)>"Name"</label>
                                                    <input type="text" class="form-control pool-edit-name"
                                                        id=format!("pool-edit-name-{}", edit_pool_id)
                                                        prop:value=move || edit_name.get()
                                                        on:input=move |ev| edit_name.set(event_target_value(&ev))/>
                                                </div>
                                                <label class="pool-edit-check">
                                                    <input type="checkbox" class="pool-edit-enabled"
                                                        prop:checked=move || edit_enabled.get()
                                                        on:change=move |ev| edit_enabled.set(event_target_checked(&ev))/>
                                                    <span>"Enabled: new VM disks, ISOs and backups may use this pool"</span>
                                                </label>
                                                {move || edit_error.get().map(|e| view! { <div class="alert alert-error pool-edit-error">{e}</div> })}
                                                <div class="form-actions">
                                                    <button type="button" class="btn btn-sm btn-secondary" on:click=move |_| show_edit.set(false)>"Cancel"</button>
                                                    <button type="button" class="btn btn-sm btn-primary pool-edit-save" on:click=save>"Save"</button>
                                                </div>
                                            </div>
                                        }
                                    })}
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
                        <button class="btn btn-primary" on:click=move |_| open_create()>
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
                    // .modal-content is the panel (.modal is itself a full-screen overlay:
                    // nested in .modal-overlay it laid header, form and buttons out side by side).
                    <div class="modal-content" id="pool-create-dialog" on:click=|e| e.stop_propagation()>
                        <div class="modal-header">
                            <h3>"Create Storage Pool"</h3>
                            <button class="modal-close" aria-label="Close" on:click=move |_| set_show_create_modal.set(false)>
                                "x"
                            </button>
                        </div>
                        <div class="modal-body">
                            {move || dialog_error.get().map(|e| view! { <div class="alert alert-error" id="pool-create-error">{e}</div> })}
                            <div class="form-group">
                                <label for="pool-create-id">"Pool ID"</label>
                                <input id="pool-create-id" node_ref=name_ref type="text" class="form-control" placeholder="local-zfs"/>
                            </div>
                            <div class="form-group">
                                <label for="pool-create-type">"Type"</label>
                                <select id="pool-create-type" class="form-control" on:change=move |ev| {
                                    set_new_type.set(event_target_value(&ev));
                                    set_dialog_error.set(None);
                                }>
                                    {POOL_TYPES.iter().map(|(v, label, why)| {
                                        let text = match why { Some(w) => format!("{label} ({w})"), None => label.to_string() };
                                        view! { <option value=*v disabled=why.is_some() selected=move || new_type.get() == *v>{text}</option> }
                                    }).collect_view()}
                                </select>
                            </div>
                            <div class="form-group">
                                <label for="pool-create-a">{move || pool_fields(&new_type.get()).0.0}</label>
                                <input id="pool-create-a" type="text" class="form-control"
                                    placeholder=move || pool_fields(&new_type.get()).0.1
                                    prop:value=field_a on:input=move |ev| set_field_a.set(event_target_value(&ev))/>
                            </div>
                            {move || pool_fields(&new_type.get()).1.map(|(label, ph)| view! {
                                <div class="form-group">
                                    <label for="pool-create-b">{label}</label>
                                    <input id="pool-create-b" type="text" class="form-control" placeholder=ph
                                        prop:value=field_b on:input=move |ev| set_field_b.set(event_target_value(&ev))/>
                                </div>
                            })}
                        </div>
                        <div class="modal-footer">
                            <button class="btn btn-secondary" on:click=move |_| set_show_create_modal.set(false)>
                                "Cancel"
                            </button>
                            <button id="pool-create-submit" class="btn btn-primary" on:click=create_pool disabled=creating>
                                {move || if creating.get() { "Creating..." } else { "Create" }}
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_paths_are_composed_per_type() {
        assert_eq!(
            compose_pool_path("Nfs", "nas", "/export/vms").unwrap(),
            "nas:/export/vms"
        );
        assert_eq!(
            compose_pool_path("Nfs", "nas", "export").unwrap(),
            "nas:/export"
        );
        assert!(compose_pool_path("Nfs", "nas", "")
            .unwrap_err()
            .contains("Export"));
        assert_eq!(
            compose_pool_path("Iscsi", "10.0.0.5:3260", "iqn.x:y").unwrap(),
            "iscsi://10.0.0.5:3260/iqn.x:y"
        );
        assert_eq!(
            compose_pool_path("Zfs", " tank/vms ", "").unwrap(),
            "tank/vms"
        );
        assert!(compose_pool_path("Directory", "relative/dir", "").is_err());
        assert_eq!(
            compose_pool_path("Directory", "/srv/vm", "").unwrap(),
            "/srv/vm"
        );
        assert!(compose_pool_path("Directory", " ", "").is_err());
    }

    #[test]
    fn unusable_types_are_offered_with_a_reason_and_passwords_never_shown() {
        for (t, _, why) in POOL_TYPES {
            if matches!(t, "Cifs" | "GlusterFs" | "S3") {
                assert!(why.is_some(), "{t}");
            } else {
                assert!(why.is_none(), "{t}");
            }
        }
        assert_eq!(
            display_path("cifs://bob:s3cret@srv/share"),
            "cifs://bob:***@srv/share"
        );
        assert_eq!(display_path("cifs://srv/share"), "cifs://srv/share");
        assert_eq!(display_path("/var/lib/x"), "/var/lib/x");
    }
}
