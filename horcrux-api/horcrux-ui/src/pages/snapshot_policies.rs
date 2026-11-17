//! Snapshots area, second and third tab: automatic snapshot schedules (per VM) and snapshot
//! quotas (storage limits with a cleanup policy). The first tab is the snapshot list itself
//! (`snapshot_list.rs`); all three share `SnapshotTabs`, so there is ONE Snapshots page.
//! (There used to be a second, differently styled "Snapshot Management" page at
//! /backup/snapshots that listed the same snapshots again; that path now redirects here.)
use crate::api::{
    create_snapshot_quota, create_snapshot_schedule, delete_snapshot_quota,
    delete_snapshot_schedule, get_snapshot_quota_summary, get_snapshot_quota_usage,
    get_snapshot_quotas, get_snapshot_schedules, get_vms, CleanupPolicy,
    CreateSnapshotQuotaRequest, CreateSnapshotScheduleRequest, QuotaSummary, QuotaType,
    RetentionPolicy, SnapshotQuota, SnapshotSchedule,
};
use horcrux_common::VmConfig;
use leptos::*;

/// The tab strip shared by /snapshots, /snapshots/schedules and /snapshots/quotas.
#[component]
pub fn SnapshotTabs(active: &'static str) -> impl IntoView {
    let tab = |href: &'static str, key: &'static str, text: &'static str| {
        let cls = if key == active { "tab active" } else { "tab" };
        view! { <a href=href class=cls aria-current=(key == active).then_some("page")>{text}</a> }
    };
    view! {
        <nav class="tabs snapshot-tabs" id="snapshot-tabs">
            {tab("/snapshots", "snapshots", "Snapshots")}
            {tab("/snapshots/schedules", "schedules", "Schedules")}
            {tab("/snapshots/quotas", "quotas", "Quotas")}
        </nav>
    }
}

pub fn quota_type_label(t: &QuotaType) -> &'static str {
    match t {
        QuotaType::MaxCount => "Max snapshots",
        QuotaType::MaxSize => "Max size (GB)",
        QuotaType::MaxAge => "Max age (days)",
    }
}

pub fn cleanup_policy_label(p: &CleanupPolicy) -> &'static str {
    match p {
        CleanupPolicy::OldestFirst => "Delete oldest first",
        CleanupPolicy::LargestFirst => "Delete largest first",
        CleanupPolicy::Manual => "Manual (warn only)",
    }
}

fn confirm(msg: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.confirm_with_message(msg).ok())
        .unwrap_or(false)
}

fn vm_name(vms: &[VmConfig], id: &str) -> String {
    vms.iter()
        .find(|v| v.id == id)
        .map(|v| v.name.clone())
        .unwrap_or_else(|| id.to_string())
}

#[component]
pub fn SnapshotSchedulesPage() -> impl IntoView {
    let (vms, set_vms) = create_signal(Vec::<VmConfig>::new());
    let (schedules, set_schedules) = create_signal(Vec::<SnapshotSchedule>::new());
    let (error, set_error) = create_signal(None::<String>);
    let (show_create, set_show_create) = create_signal(false);
    let (vm_id, set_vm_id) = create_signal(String::new());
    let (name, set_name) = create_signal(String::new());
    let (description, set_description) = create_signal(String::new());
    let (cron, set_cron) = create_signal("0 2 * * *".to_string());
    let (enabled, set_enabled) = create_signal(true);
    let (include_memory, set_include_memory) = create_signal(false);

    let load = move || {
        spawn_local(async move {
            match get_vms().await {
                Ok(v) => set_vms.set(v),
                Err(e) => set_error.set(Some(format!("Failed to load VMs: {}", e))),
            }
            match get_snapshot_schedules().await {
                Ok(s) => set_schedules.set(s),
                Err(e) => set_error.set(Some(format!("Failed to load schedules: {}", e))),
            }
        });
    };
    load();

    let open_create = move |_| {
        if vm_id.get_untracked().is_empty() {
            if let Some(first) = vms.with_untracked(|v| v.first().map(|v| v.id.clone())) {
                set_vm_id.set(first);
            }
        }
        set_show_create.set(true);
    };

    let create = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let request = CreateSnapshotScheduleRequest {
            vm_id: vm_id.get(),
            name: name.get().trim().to_string(),
            description: Some(description.get().trim().to_string()).filter(|d| !d.is_empty()),
            schedule: cron.get().trim().to_string(),
            retention_policy: RetentionPolicy {
                keep_hourly: None,
                keep_daily: Some(7),
                keep_weekly: Some(4),
                keep_monthly: Some(12),
                keep_yearly: None,
                max_age_days: Some(90),
            },
            include_memory: include_memory.get(),
            enabled: enabled.get(),
        };
        spawn_local(async move {
            match create_snapshot_schedule(request).await {
                Ok(_) => {
                    set_show_create.set(false);
                    set_name.set(String::new());
                    set_description.set(String::new());
                    set_error.set(None);
                    load();
                }
                Err(e) => set_error.set(Some(format!("Failed to create schedule: {}", e))),
            }
        });
    };

    let delete = move |id: String| {
        if confirm("Delete this snapshot schedule?") {
            spawn_local(async move {
                match delete_snapshot_schedule(&id).await {
                    Ok(_) => load(),
                    Err(e) => set_error.set(Some(format!("Failed to delete schedule: {}", e))),
                }
            });
        }
    };

    view! {
        <div class="snapshot-schedules-page">
            <div class="page-header">
                <h1>"Snapshots"</h1>
                <div class="header-actions">
                    <button id="schedule-create-btn" class="btn btn-primary" on:click=open_create
                        disabled=move || vms.with(|v| v.is_empty())>"Create Schedule"</button>
                </div>
            </div>
            <SnapshotTabs active="schedules"/>
            <p class="hint">"A schedule takes a snapshot of one VM automatically (cron syntax) and keeps 7 daily, 4 weekly and 12 monthly snapshots."</p>
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="schedule-error">{e}</div> })}
            {move || {
                let list = schedules.get();
                if list.is_empty() {
                    view! {
                        <div class="empty-state" id="schedules-empty">
                            <p class="empty-title">"No snapshot schedules"</p>
                            {move || if vms.with(|v| v.is_empty()) {
                                view! { <p class="empty-todo">"Create a VM first, then schedule its snapshots here. " <a href="/vms/create">"Create VM"</a></p> }.into_view()
                            } else {
                                view! { <p class="empty-todo">"Use Create Schedule above to snapshot a VM automatically."</p> }.into_view()
                            }}
                        </div>
                    }.into_view()
                } else {
                    let all_vms = vms.get();
                    view! {
                        <table class="data-table" id="schedule-table">
                            <thead><tr><th>"Name"</th><th>"VM"</th><th>"Schedule"</th><th>"Next run"</th><th>"Memory"</th><th>"Status"</th><th></th></tr></thead>
                            <tbody>
                                {list.into_iter().map(|s| {
                                    let id = s.id.clone();
                                    view! {
                                        <tr data-schedule-name=s.name.clone()>
                                            <td><strong>{s.name.clone()}</strong>
                                                {s.description.clone().map(|d| view! { <small class="hint" style="display:block">{d}</small> })}</td>
                                            <td><a href=format!("/vms/{}", s.vm_id)>{vm_name(&all_vms, &s.vm_id)}</a></td>
                                            <td><code>{s.schedule.clone()}</code></td>
                                            <td>{s.next_run.clone().unwrap_or_else(|| "-".into())}</td>
                                            <td>{if s.include_memory { "Yes" } else { "No" }}</td>
                                            <td>{if s.enabled { "Active" } else { "Disabled" }}</td>
                                            <td class="actions"><button class="btn btn-sm btn-danger" on:click=move |_| delete(id.clone())>"Delete"</button></td>
                                        </tr>
                                    }
                                }).collect_view()}
                            </tbody>
                        </table>
                    }.into_view()
                }
            }}
            <Show when=move || show_create.get()>
                <div class="modal-overlay" on:click=move |_| set_show_create.set(false)>
                    <div class="modal-content" on:click=|e| e.stop_propagation()>
                        <form id="create-schedule-form" on:submit=create>
                            <div class="modal-header"><h3>"Create Snapshot Schedule"</h3></div>
                            <div class="modal-body">
                                <div class="form-group">
                                    <label for="schedule-vm">"VM"</label>
                                    <select id="schedule-vm" required on:change=move |ev| set_vm_id.set(event_target_value(&ev))>
                                        {move || vms.get().into_iter().map(|v| {
                                            let id = v.id.clone();
                                            view! { <option value=v.id.clone() selected=move || vm_id.get() == id>{format!("{} ({})", v.name, v.id)}</option> }
                                        }).collect_view()}
                                    </select>
                                </div>
                                <div class="form-group">
                                    <label for="schedule-name">"Name"</label>
                                    <input id="schedule-name" type="text" required placeholder="Nightly"
                                        prop:value=name on:input=move |ev| set_name.set(event_target_value(&ev))/>
                                </div>
                                <div class="form-group">
                                    <label for="schedule-cron">"When (cron: minute hour day month weekday)"</label>
                                    <input id="schedule-cron" type="text" required placeholder="0 2 * * *"
                                        prop:value=cron on:input=move |ev| set_cron.set(event_target_value(&ev))/>
                                </div>
                                <div class="form-group">
                                    <label for="schedule-desc">"Description (optional)"</label>
                                    <input id="schedule-desc" type="text"
                                        prop:value=description on:input=move |ev| set_description.set(event_target_value(&ev))/>
                                </div>
                                <div class="form-group">
                                    <label class="checkbox-label"><input type="checkbox" id="schedule-memory" prop:checked=include_memory
                                        on:change=move |ev| set_include_memory.set(event_target_checked(&ev))/>"Include memory (running VMs)"</label>
                                </div>
                                <div class="form-group">
                                    <label class="checkbox-label"><input type="checkbox" id="schedule-enabled" prop:checked=enabled
                                        on:change=move |ev| set_enabled.set(event_target_checked(&ev))/>"Enabled"</label>
                                </div>
                            </div>
                            <div class="modal-footer">
                                <button type="button" class="btn btn-secondary" on:click=move |_| set_show_create.set(false)>"Cancel"</button>
                                <button type="submit" class="btn btn-primary" id="schedule-submit">"Create"</button>
                            </div>
                        </form>
                    </div>
                </div>
            </Show>
        </div>
    }
}

#[component]
pub fn SnapshotQuotasPage() -> impl IntoView {
    let (quotas, set_quotas) = create_signal(Vec::<SnapshotQuota>::new());
    let (summary, set_summary) = create_signal(None::<QuotaSummary>);
    let (usage, set_usage) = create_signal(std::collections::HashMap::<String, String>::new());
    let (error, set_error) = create_signal(None::<String>);
    let (show_create, set_show_create) = create_signal(false);
    let (name, set_name) = create_signal(String::new());
    let (description, set_description) = create_signal(String::new());
    let (quota_type, set_quota_type) = create_signal(QuotaType::MaxCount);
    let (limit, set_limit) = create_signal(10u64);
    let (path, set_path) = create_signal("/var/lib/libvirt/images".to_string());
    let (policy, set_policy) = create_signal(CleanupPolicy::OldestFirst);
    let (enabled, set_enabled) = create_signal(true);

    let load = move || {
        spawn_local(async move {
            match get_snapshot_quotas().await {
                Ok(q) => set_quotas.set(q),
                Err(e) => set_error.set(Some(format!("Failed to load quotas: {}", e))),
            }
            if let Ok(s) = get_snapshot_quota_summary().await {
                set_summary.set(Some(s));
            }
        });
    };
    load();

    let show_usage = move |id: String| {
        spawn_local(async move {
            let text = match get_snapshot_quota_usage(&id).await {
                Ok(u) => format!(
                    "{} snapshot(s), {:.1} MiB{} ({:.0}%){}",
                    u.current_count,
                    u.current_size_bytes as f64 / 1048576.0,
                    if u.max_size_bytes > 0 {
                        format!(" of {:.1} MiB", u.max_size_bytes as f64 / 1048576.0)
                    } else {
                        String::new()
                    },
                    u.usage_percent,
                    if u.is_exceeded {
                        " - over the limit"
                    } else if u.is_warning {
                        " - near the limit"
                    } else {
                        ""
                    }
                ),
                Err(e) => format!("Could not read usage: {}", e.message),
            };
            set_usage.update(|m| {
                m.insert(id, text);
            });
        });
    };

    let create = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let request = CreateSnapshotQuotaRequest {
            name: name.get().trim().to_string(),
            description: Some(description.get().trim().to_string()).filter(|d| !d.is_empty()),
            quota_type: quota_type.get(),
            limit_value: limit.get(),
            storage_path: path.get().trim().to_string(),
            cleanup_policy: policy.get(),
            enabled: enabled.get(),
        };
        spawn_local(async move {
            match create_snapshot_quota(request).await {
                Ok(_) => {
                    set_show_create.set(false);
                    set_name.set(String::new());
                    set_description.set(String::new());
                    set_error.set(None);
                    load();
                }
                Err(e) => set_error.set(Some(format!("Failed to create quota: {}", e))),
            }
        });
    };

    let delete = move |id: String| {
        if confirm("Delete this snapshot quota?") {
            spawn_local(async move {
                match delete_snapshot_quota(&id).await {
                    Ok(_) => load(),
                    Err(e) => set_error.set(Some(format!("Failed to delete quota: {}", e))),
                }
            });
        }
    };

    view! {
        <div class="snapshot-quotas-page">
            <div class="page-header">
                <h1>"Snapshots"</h1>
                <div class="header-actions">
                    <button id="quota-create-btn" class="btn btn-primary" on:click=move |_| set_show_create.set(true)>"Create Quota"</button>
                </div>
            </div>
            <SnapshotTabs active="quotas"/>
            <p class="hint">"A quota limits how many snapshots (or how much space, or how old) a storage path may hold, and what is cleaned up first."</p>
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="quota-error">{e}</div> })}
            {move || summary.get().filter(|s| s.total_quotas > 0).map(|s| view! {
                <div class="stats-grid" id="quota-summary">
                    <div class="stat-card"><div class="stat-value">{s.total_quotas}</div><div class="stat-label">"Quotas"</div></div>
                    <div class="stat-card"><div class="stat-value">{format!("{:.1}%", s.overall_usage_percent)}</div><div class="stat-label">"Used"</div></div>
                    <div class={if s.exceeded_quotas > 0 { "stat-card critical attention" } else { "stat-card" }}>
                        <div class="stat-value">{s.exceeded_quotas}</div><div class="stat-label">"Over the limit"</div></div>
                </div>
            })}
            {move || {
                let list = quotas.get();
                if list.is_empty() {
                    view! {
                        <div class="empty-state" id="quotas-empty">
                            <p class="empty-title">"No snapshot quotas"</p>
                            <p class="empty-todo">"Use Create Quota above to cap snapshot storage."</p>
                        </div>
                    }.into_view()
                } else {
                    view! {
                        <table class="data-table" id="quota-table">
                            <thead><tr><th>"Name"</th><th>"Limit"</th><th>"Cleanup"</th><th>"Path"</th><th>"Status"</th><th></th></tr></thead>
                            <tbody>
                                {list.into_iter().map(|q| {
                                    let (id_u, id_d, id_t) = (q.id.clone(), q.id.clone(), q.id.clone());
                                    view! {
                                        <tr data-quota-name=q.name.clone()>
                                            <td><strong>{q.name.clone()}</strong>
                                                {q.description.clone().map(|d| view! { <small class="hint" style="display:block">{d}</small> })}
                                                {move || usage.get().get(&id_t).cloned().map(|t| view! { <small class="quota-usage-text" style="display:block">{t}</small> })}</td>
                                            <td>{format!("{}: {}", quota_type_label(&q.quota_type), q.limit_value)}</td>
                                            <td>{cleanup_policy_label(&q.cleanup_policy)}</td>
                                            <td><code>{q.storage_path.clone()}</code></td>
                                            <td>{if q.enabled { "Active" } else { "Disabled" }}</td>
                                            <td class="actions">
                                                <button class="btn btn-sm btn-secondary quota-usage" on:click=move |_| show_usage(id_u.clone())>"Usage"</button>
                                                <button class="btn btn-sm btn-danger" on:click=move |_| delete(id_d.clone())>"Delete"</button>
                                            </td>
                                        </tr>
                                    }
                                }).collect_view()}
                            </tbody>
                        </table>
                    }.into_view()
                }
            }}
            <Show when=move || show_create.get()>
                <div class="modal-overlay" on:click=move |_| set_show_create.set(false)>
                    <div class="modal-content" on:click=|e| e.stop_propagation()>
                        <form id="create-quota-form" on:submit=create>
                            <div class="modal-header"><h3>"Create Snapshot Quota"</h3></div>
                            <div class="modal-body">
                                <div class="form-group">
                                    <label for="quota-name">"Name"</label>
                                    <input id="quota-name" type="text" required placeholder="VM images"
                                        prop:value=name on:input=move |ev| set_name.set(event_target_value(&ev))/>
                                </div>
                                <div class="form-row">
                                    <div class="form-group">
                                        <label for="quota-type">"Limit by"</label>
                                        <select id="quota-type" on:change=move |ev| set_quota_type.set(match event_target_value(&ev).as_str() {
                                            "MaxSize" => QuotaType::MaxSize,
                                            "MaxAge" => QuotaType::MaxAge,
                                            _ => QuotaType::MaxCount,
                                        })>
                                            <option value="MaxCount">"Number of snapshots"</option>
                                            <option value="MaxSize">"Total size (GB)"</option>
                                            <option value="MaxAge">"Age (days)"</option>
                                        </select>
                                    </div>
                                    <div class="form-group">
                                        <label for="quota-limit">{move || quota_type_label(&quota_type.get())}</label>
                                        <input id="quota-limit" type="number" min="1" required prop:value=move || limit.get().to_string()
                                            on:input=move |ev| set_limit.set(event_target_value(&ev).parse().unwrap_or(1))/>
                                    </div>
                                </div>
                                <div class="form-group">
                                    <label for="quota-cleanup">"When the limit is reached"</label>
                                    <select id="quota-cleanup" on:change=move |ev| set_policy.set(match event_target_value(&ev).as_str() {
                                        "LargestFirst" => CleanupPolicy::LargestFirst,
                                        "Manual" => CleanupPolicy::Manual,
                                        _ => CleanupPolicy::OldestFirst,
                                    })>
                                        <option value="OldestFirst">"Delete oldest first"</option>
                                        <option value="LargestFirst">"Delete largest first"</option>
                                        <option value="Manual">"Warn only"</option>
                                    </select>
                                </div>
                                <div class="form-group">
                                    <label for="quota-path">"Storage path"</label>
                                    <input id="quota-path" type="text" required prop:value=path on:input=move |ev| set_path.set(event_target_value(&ev))/>
                                </div>
                                <div class="form-group">
                                    <label for="quota-desc">"Description (optional)"</label>
                                    <input id="quota-desc" type="text" prop:value=description on:input=move |ev| set_description.set(event_target_value(&ev))/>
                                </div>
                                <div class="form-group">
                                    <label class="checkbox-label"><input type="checkbox" id="quota-enabled" prop:checked=enabled
                                        on:change=move |ev| set_enabled.set(event_target_checked(&ev))/>"Enabled"</label>
                                </div>
                            </div>
                            <div class="modal-footer">
                                <button type="button" class="btn btn-secondary" on:click=move |_| set_show_create.set(false)>"Cancel"</button>
                                <button type="submit" class="btn btn-primary" id="quota-submit">"Create"</button>
                            </div>
                        </form>
                    </div>
                </div>
            </Show>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_choices_read_as_plain_words() {
        assert_eq!(quota_type_label(&QuotaType::MaxCount), "Max snapshots");
        assert_eq!(quota_type_label(&QuotaType::MaxAge), "Max age (days)");
        assert_eq!(
            cleanup_policy_label(&CleanupPolicy::OldestFirst),
            "Delete oldest first"
        );
    }
}
