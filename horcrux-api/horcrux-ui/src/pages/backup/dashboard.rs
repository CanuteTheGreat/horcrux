//! /backup: every backup in the catalog, "Back up now", and per-backup
//! verify / restore / protect / delete.
use super::common::{confirm, load_guests, non_empty, short, Guest};
use crate::api::backup as bk;
use leptos::*;

#[component]
pub fn BackupDashboard() -> impl IntoView {
    let (backups, set_backups) = create_signal(Vec::<bk::Backup>::new());
    let (jobs, set_jobs) = create_signal(Vec::<bk::BackupJob>::new());
    let (guests, set_guests) = create_signal(Vec::<Guest>::new());
    let (storages, set_storages) = create_signal(Vec::<bk::BackupStorage>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let (busy, set_busy) = create_signal(false);
    let (filter, set_filter) = create_signal(String::new());

    // "Back up now" form
    let (target, set_target) = create_signal(String::new());
    let (storage, set_storage) = create_signal("local".to_string());
    let (mode, set_mode) = create_signal("snapshot".to_string());
    let (compression, set_compression) = create_signal("zstd".to_string());
    let (notes, set_notes) = create_signal(String::new());
    let (protect, set_protect) = create_signal(false);

    // Restore dialog
    let (restore_of, set_restore_of) = create_signal(None::<bk::Backup>);
    let (r_target, set_r_target) = create_signal(String::new());
    let (r_name, set_r_name) = create_signal(String::new());
    let (r_force, set_r_force) = create_signal(false);
    let (r_start, set_r_start) = create_signal(false);

    let reload = move || {
        spawn_local(async move {
            match bk::list_backups().await {
                Ok(b) => {
                    set_backups.set(b);
                    set_error.set(None);
                }
                Err(e) => set_error.set(Some(format!("Loading backups: {}", e.message))),
            }
            if let Ok(j) = bk::list_backup_jobs().await {
                set_jobs.set(j);
            }
            set_loading.set(false);
        });
    };

    create_effect(move |_| {
        reload();
        spawn_local(async move {
            let g = load_guests().await;
            if let Some(first) = g.first() {
                set_target.set(first.id.clone());
            }
            set_guests.set(g);
            if let Ok(s) = bk::list_backup_storages().await {
                set_storages.set(s);
            }
        });
    });

    let backup_now = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let t = target.get();
        if t.is_empty() {
            set_error.set(Some("Choose a VM or container to back up".into()));
            return;
        }
        let kind = guests.get().into_iter().find(|g| g.id == t).map(|g| g.kind);
        let req = bk::CreateBackupRequest {
            target_id: t.clone(),
            target_type: kind,
            storage: storage.get(),
            mode: mode.get(),
            compression: compression.get(),
            notes: non_empty(&notes.get()),
            protected: protect.get(),
        };
        set_busy.set(true);
        set_notice.set(Some(format!("Backing up {t}...")));
        spawn_local(async move {
            match bk::create_backup(&req).await {
                Ok(b) => {
                    set_notice.set(Some(format!(
                        "Backup {} of {} written: {} ({})",
                        short(&b.id),
                        b.target_id,
                        bk::format_bytes(b.size),
                        b.path
                    )));
                    set_error.set(None);
                    set_notes.set(String::new());
                }
                Err(e) => {
                    set_notice.set(None);
                    set_error.set(Some(format!("Backup of {t} failed: {}", e.message)));
                }
            }
            set_busy.set(false);
            reload();
        });
    };

    let verify = move |id: String| {
        set_busy.set(true);
        spawn_local(async move {
            match bk::verify_backup(&id).await {
                Ok(v) if v.ok => {
                    set_notice.set(Some(format!(
                        "Backup {} verified OK ({} file(s) checked)",
                        short(&id),
                        v.files_checked
                    )));
                    set_error.set(None);
                }
                Ok(v) => set_error.set(Some(format!(
                    "Backup {} FAILED verification: {}",
                    short(&id),
                    v.error.unwrap_or_default()
                ))),
                Err(e) => set_error.set(Some(format!("Verify: {}", e.message))),
            }
            set_busy.set(false);
            reload();
        });
    };

    let toggle_protect = move |(id, protected): (String, bool)| {
        spawn_local(async move {
            let req = bk::UpdateBackupRequest {
                notes: None,
                protected: Some(!protected),
            };
            if let Err(e) = bk::update_backup(&id, &req).await {
                set_error.set(Some(format!("Update {}: {}", short(&id), e.message)));
            }
            reload();
        });
    };

    let delete = move |id: String| {
        if !confirm(&format!("Delete backup {} and its archive?", short(&id))) {
            return;
        }
        spawn_local(async move {
            match bk::delete_backup(&id).await {
                Ok(()) => set_notice.set(Some(format!("Backup {} deleted", short(&id)))),
                Err(e) => set_error.set(Some(format!("Delete {}: {}", short(&id), e.message))),
            }
            reload();
        });
    };

    let open_restore = move |b: bk::Backup| {
        set_r_target.set(b.target_id.clone());
        set_r_name.set(String::new());
        set_r_force.set(false);
        set_r_start.set(false);
        set_restore_of.set(Some(b));
    };

    let do_restore = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let Some(b) = restore_of.get() else { return };
        let req = bk::RestoreRequest {
            target_id: non_empty(&r_target.get()),
            name: non_empty(&r_name.get()),
            force: r_force.get(),
            start: r_start.get(),
        };
        set_busy.set(true);
        spawn_local(async move {
            match bk::restore_backup(&b.id, &req).await {
                Ok(r) => {
                    set_notice.set(Some(format!(
                        "Restored backup {} to {} {} ({}){}",
                        short(&b.id),
                        r.target_type,
                        r.target_id,
                        r.name,
                        if r.replaced { ", replacing it" } else { "" }
                    )));
                    set_error.set(None);
                    set_restore_of.set(None);
                }
                Err(e) => set_error.set(Some(format!("Restore failed: {}", e.message))),
            }
            set_busy.set(false);
            reload();
        });
    };

    let failed_jobs = move || {
        jobs.get()
            .into_iter()
            .filter(|j| j.last_status.as_deref() == Some("failed"))
            .count()
    };

    let has_failed = move || failed_jobs() != 0;

    view! {
        <div class="backup-dashboard">
            <div class="page-header">
                <h1>"Backups"</h1>
                <div class="header-actions">
                    <a href="/backup/jobs" class="btn btn-secondary">"Backup Jobs"</a>
                    " "
                    <a href="/backup/retention" class="btn btn-secondary">"Retention / Prune"</a>
                    " "
                    <a href="/backup/verify" class="btn btn-secondary">"Verify"</a>
                </div>
            </div>

            <div class="stats-grid">
                <div class="stat-card">
                    <h3>"Backups"</h3>
                    <div class="stat-value" id="backup-count">{move || backups.get().len()}</div>
                </div>
                <div class="stat-card">
                    <h3>"Total size"</h3>
                    <div class="stat-value">
                        {move || bk::format_bytes(backups.get().iter().map(|b| b.size).sum())}
                    </div>
                </div>
                <div class="stat-card">
                    <h3>"Jobs"</h3>
                    <div class="stat-value">
                        <a href="/backup/jobs">{move || jobs.get().len()}</a>
                    </div>
                </div>
                <div class="stat-card">
                    <h3>"Failed jobs"</h3>
                    <div class="stat-value">
                        <a href="/backup/jobs" class:error=has_failed>
                            {failed_jobs}
                        </a>
                    </div>
                </div>
            </div>

            {move || notice.get().map(|n| view! { <div class="alert alert-success" id="backup-notice">{n}</div> })}
            {move || error.get().map(|e| view! { <div class="error" id="backup-error">{e}</div> })}

            <div class="dashboard-section">
                <h2>"Back up now"</h2>
                <form on:submit=backup_now class="backup-form">
                    <div class="form-group">
                        <label for="backup-target">"VM / container"</label>
                        <select id="backup-target" on:change=move |ev| set_target.set(event_target_value(&ev))>
                            {move || guests.get().into_iter().map(|g| {
                                let selected = g.id == target.get();
                                view! { <option value=g.id.clone() selected=selected>{g.label}</option> }
                            }).collect_view()}
                        </select>
                    </div>
                    <div class="form-group">
                        <label for="backup-storage">"Storage"</label>
                        <select id="backup-storage" on:change=move |ev| set_storage.set(event_target_value(&ev))>
                            {move || storages.get().into_iter().map(|s| {
                                let selected = s.id == storage.get();
                                let label = format!("{} ({}, {})", s.id, s.path,
                                    s.available_gb.map(|a| format!("{a} GB free")).unwrap_or_else(|| s.kind.clone()));
                                view! { <option value=s.id.clone() selected=selected>{label}</option> }
                            }).collect_view()}
                        </select>
                    </div>
                    <div class="form-group">
                        <label for="backup-mode">"Mode"</label>
                        <select id="backup-mode" on:change=move |ev| set_mode.set(event_target_value(&ev))>
                            <option value="snapshot" selected=move || mode.get() == "snapshot">"snapshot (offline copy if stopped; containers frozen)"</option>
                            <option value="suspend" selected=move || mode.get() == "suspend">"suspend"</option>
                            <option value="stop" selected=move || mode.get() == "stop">"stop (stop, back up, restart)"</option>
                        </select>
                    </div>
                    <div class="form-group">
                        <label for="backup-compression">"Compression"</label>
                        <select id="backup-compression" on:change=move |ev| set_compression.set(event_target_value(&ev))>
                            <option value="zstd" selected=move || compression.get() == "zstd">"zstd"</option>
                            <option value="gzip" selected=move || compression.get() == "gzip">"gzip"</option>
                            <option value="lzo" selected=move || compression.get() == "lzo">"lzo"</option>
                            <option value="none" selected=move || compression.get() == "none">"none"</option>
                        </select>
                    </div>
                    <div class="form-group">
                        <label for="backup-notes">"Notes"</label>
                        <input id="backup-notes" type="text" prop:value=move || notes.get()
                            on:input=move |ev| set_notes.set(event_target_value(&ev))/>
                    </div>
                    <div class="form-group">
                        <label>
                            <input id="backup-protect" type="checkbox" prop:checked=move || protect.get()
                                on:change=move |ev| set_protect.set(event_target_checked(&ev))/>
                            " Protected (never pruned or deleted)"
                        </label>
                    </div>
                    <div class="form-actions">
                        <button type="submit" id="backup-now-btn" class="btn btn-primary" disabled=move || busy.get()>
                            {move || if busy.get() { "Working..." } else { "Back up now" }}
                        </button>
                    </div>
                </form>
            </div>

            {move || restore_of.get().map(|b| view! {
                <div class="dashboard-section" id="restore-dialog">
                    <h2>{format!("Restore backup {} of {} ({})", short(&b.id), b.target_id, bk::format_time(b.timestamp))}</h2>
                    <form on:submit=do_restore>
                        <div class="form-group">
                            <label for="restore-target">"Restore to id (original or a new id)"</label>
                            <input id="restore-target" type="text" prop:value=move || r_target.get()
                                on:input=move |ev| set_r_target.set(event_target_value(&ev))/>
                        </div>
                        <div class="form-group">
                            <label for="restore-name">"Name (optional)"</label>
                            <input id="restore-name" type="text" prop:value=move || r_name.get()
                                on:input=move |ev| set_r_name.set(event_target_value(&ev))/>
                        </div>
                        <div class="form-group">
                            <label>
                                <input id="restore-force" type="checkbox" prop:checked=move || r_force.get()
                                    on:change=move |ev| set_r_force.set(event_target_checked(&ev))/>
                                " Overwrite a different existing guest with this id"
                            </label>
                            <label>
                                <input id="restore-start" type="checkbox" prop:checked=move || r_start.get()
                                    on:change=move |ev| set_r_start.set(event_target_checked(&ev))/>
                                " Start after restore"
                            </label>
                        </div>
                        <p>"Restoring over the original replaces its disks/filesystem with the backup. The archive is verified first; a corrupt archive never touches the existing guest."</p>
                        <div class="form-actions">
                            <button type="submit" id="restore-submit" class="btn btn-warning" disabled=move || busy.get()>"Restore"</button>
                            " "
                            <button type="button" class="btn btn-secondary" on:click=move |_| set_restore_of.set(None)>"Cancel"</button>
                        </div>
                    </form>
                </div>
            })}

            <div class="dashboard-section">
                <h2>"Backup archives"</h2>
                <div class="form-group">
                    <input id="backup-filter" type="text" placeholder="Filter by guest id or name"
                        prop:value=move || filter.get() on:input=move |ev| set_filter.set(event_target_value(&ev))/>
                </div>
                {move || {
                    if loading.get() {
                        return view! { <p class="loading">"Loading backups..."</p> }.into_view();
                    }
                    let f = filter.get().to_lowercase();
                    let list: Vec<bk::Backup> = backups.get().into_iter()
                        .filter(|b| f.is_empty() || b.target_id.to_lowercase().contains(&f) || b.target_name.to_lowercase().contains(&f))
                        .collect();
                    if list.is_empty() {
                        return view! { <p class="no-data">"No backups yet. Use \"Back up now\" or create a backup job."</p> }.into_view();
                    }
                    view! {
                        <table class="vm-table" id="backup-table">
                            <thead>
                                <tr>
                                    <th>"Guest"</th>
                                    <th>"Time"</th>
                                    <th>"Size"</th>
                                    <th>"Mode / compression"</th>
                                    <th>"Storage"</th>
                                    <th>"Verified"</th>
                                    <th>"Notes"</th>
                                    <th>"Actions"</th>
                                </tr>
                            </thead>
                            <tbody>
                                {list.into_iter().map(|b| {
                                    let (vid, rb, pid, did) = (b.id.clone(), b.clone(), b.id.clone(), b.id.clone());
                                    let protected = b.protected;
                                    let verify_cell = match b.verify_state.as_deref() {
                                        Some("ok") => view! { <span class="status-running" title=b.verified_at.map(bk::format_time).unwrap_or_default()>"OK"</span> }.into_view(),
                                        Some(_) => view! { <span class="status-stopped" title=b.verify_error.clone().unwrap_or_default()>{format!("FAILED: {}", b.verify_error.clone().unwrap_or_default())}</span> }.into_view(),
                                        None => view! { <span class="status-unknown">"not verified"</span> }.into_view(),
                                    };
                                    let kind = if b.target_type == "vm" { "VM" } else { "CT" };
                                    view! {
                                        <tr class="backup-row" data-backup-id=b.id.clone() data-target-id=b.target_id.clone()>
                                            <td>{format!("{kind} {} ({})", b.target_id, b.target_name)}</td>
                                            <td title=b.path.clone()>{bk::format_time(b.timestamp)}</td>
                                            <td>{bk::format_bytes(b.size)}</td>
                                            <td>{format!("{} / {}", b.mode, b.compression)}</td>
                                            <td>{b.storage.clone()}</td>
                                            <td class="verify-state">{verify_cell}</td>
                                            <td>
                                                {if protected { "[protected] " } else { "" }}
                                                {b.job_id.clone().map(|j| format!("[job {j}] ")).unwrap_or_default()}
                                                {b.notes.clone().unwrap_or_default()}
                                            </td>
                                            <td class="actions">
                                                <button class="btn btn-sm btn-secondary verify-btn" on:click=move |_| verify(vid.clone())>"Verify"</button>
                                                <button class="btn btn-sm btn-primary restore-btn" on:click=move |_| open_restore(rb.clone())>"Restore"</button>
                                                <button class="btn btn-sm btn-secondary protect-btn" on:click=move |_| toggle_protect((pid.clone(), protected))>
                                                    {if protected { "Unprotect" } else { "Protect" }}
                                                </button>
                                                <button class="btn btn-sm btn-danger delete-btn" disabled=protected on:click=move |_| delete(did.clone())>"Delete"</button>
                                            </td>
                                        </tr>
                                    }
                                }).collect_view()}
                            </tbody>
                        </table>
                    }.into_view()
                }}
            </div>
        </div>
    }
}
