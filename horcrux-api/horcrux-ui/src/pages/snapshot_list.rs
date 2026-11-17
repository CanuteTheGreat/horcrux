use crate::api;
use leptos::*;
use leptos_router::*;

/// localStorage key: the VM the last snapshot was taken of (the Create
/// dialog's default when the page shows every VM).
const LAST_VM_KEY: &str = "hx-snapshots-last-vm";

fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

/// Newest first; ties broken by VM and name so the order is stable.
pub fn sort_snapshots(v: &mut [api::VmSnapshot]) {
    v.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| a.vm_id.cmp(&b.vm_id))
            .then_with(|| a.name.cmp(&b.name))
    });
}

/// The VM the Create dialog starts on: the filtered VM, else the VM of the
/// last snapshot taken here (if it still exists), else the first VM.
pub fn default_snapshot_vm(
    filter: &str,
    last_used: Option<&str>,
    vm_ids: &[String],
) -> Option<String> {
    if !filter.is_empty() && vm_ids.iter().any(|v| v == filter) {
        return Some(filter.to_string());
    }
    last_used
        .filter(|l| vm_ids.iter().any(|v| v == l))
        .map(str::to_string)
        .or_else(|| vm_ids.first().cloned())
}

#[component]
pub fn SnapshotList() -> impl IntoView {
    // "" = every VM (the default: a snapshot just taken is always visible).
    let (selected_vm, set_selected_vm) = create_signal(String::new());
    let (snapshots, set_snapshots) = create_signal(Vec::<api::VmSnapshot>::new());
    let (vms, set_vms) = create_signal(Vec::<horcrux_common::VmConfig>::new());
    let (vms_loaded, set_vms_loaded) = create_signal(false);
    let (loading, set_loading) = create_signal(false);
    let (error, set_error) = create_signal(None::<String>);
    let (show_create_dialog, set_show_create_dialog) = create_signal(false);
    let (notice, set_notice) = create_signal(None::<String>);
    let (refresh, set_refresh) = create_signal(0u32);
    let (snap_vm, set_snap_vm) = create_signal(String::new());
    let (snap_name, set_snap_name) = create_signal(String::new());
    let (snap_desc, set_snap_desc) = create_signal(String::new());
    let (snap_memory, set_snap_memory) = create_signal(false);
    let (creating, set_creating) = create_signal(false);
    let (create_error, set_create_error) = create_signal(None::<String>);
    let load_seq = store_value(0u64);

    // `?vm=<id>` (VM detail page, links) preselects one VM.
    let query = use_query_map();
    create_effect(move |_| {
        if let Some(v) = query.get().get("vm") {
            set_selected_vm.set(v.clone());
        }
    });

    // Load VMs for the filter and the Create dialog
    spawn_local(async move {
        match api::get_vms().await {
            Ok(vm_list) => set_vms.set(vm_list),
            Err(e) => set_error.set(Some(format!("Could not load VMs: {}", e.message))),
        }
        set_vms_loaded.set(true);
    });

    // Load the snapshots of the selected VM, or of every VM.
    create_effect(move |_| {
        let vm_id = selected_vm.get();
        let _ = refresh.get();
        let ids: Vec<String> = if vm_id.is_empty() {
            vms.get().into_iter().map(|v| v.id).collect()
        } else {
            vec![vm_id]
        };
        load_seq.update_value(|n| *n += 1);
        let seq = load_seq.get_value();
        set_loading.set(true);
        spawn_local(async move {
            let mut all = Vec::new();
            let mut failed = Vec::new();
            for id in &ids {
                match api::get_vm_snapshots(id).await {
                    Ok(list) => all.extend(list),
                    Err(e) => failed.push(format!("{id}: {}", e.message)),
                }
            }
            if load_seq.get_value() != seq {
                return; // a newer load (filter changed) owns the table
            }
            sort_snapshots(&mut all);
            set_snapshots.set(all);
            set_error.set(
                (!failed.is_empty())
                    .then(|| format!("Could not load snapshots of {}", failed.join("; "))),
            );
            set_loading.set(false);
        });
    });

    let vm_name = move |id: &str| {
        vms.with(|v| {
            v.iter()
                .find(|vm| vm.id == id)
                .map(|vm| vm.name.clone())
                .unwrap_or_else(|| id.to_string())
        })
    };

    let restore_snapshot = move |vm_id: String, snapshot_id: String| {
        if web_sys::window()
            .and_then(|w| {
                w.confirm_with_message(&format!(
                    "Restore snapshot {}? Current state will be lost.",
                    snapshot_id
                ))
                .ok()
            })
            .unwrap_or(false)
        {
            spawn_local(async move {
                match api::restore_snapshot(&vm_id, &snapshot_id).await {
                    Ok(_) => set_notice.set(Some(format!("Snapshot {} restored", snapshot_id))),
                    Err(e) => set_error.set(Some(format!("Restore failed: {}", e.message))),
                }
                set_refresh.update(|n| *n += 1);
            });
        }
    };

    let delete_snapshot = move |vm_id: String, snapshot_id: String| {
        if web_sys::window()
            .and_then(|w| {
                w.confirm_with_message(&format!("Delete snapshot {}?", snapshot_id))
                    .ok()
            })
            .unwrap_or(false)
        {
            spawn_local(async move {
                if let Err(e) = api::delete_snapshot(&vm_id, &snapshot_id).await {
                    set_error.set(Some(format!("Delete failed: {}", e.message)));
                }
                set_refresh.update(|n| *n += 1);
            });
        }
    };

    let open_create = move |_: ev::MouseEvent| {
        let ids: Vec<String> = vms.with(|v| v.iter().map(|vm| vm.id.clone()).collect());
        let last = local_storage().and_then(|s| s.get_item(LAST_VM_KEY).ok().flatten());
        set_snap_vm.set(
            default_snapshot_vm(&selected_vm.get_untracked(), last.as_deref(), &ids)
                .unwrap_or_default(),
        );
        set_create_error.set(None);
        set_show_create_dialog.set(true);
    };

    let submit_create = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let vm_id = snap_vm.get();
        if vm_id.is_empty() {
            set_create_error.set(Some("Choose the VM to snapshot".to_string()));
            return;
        }
        let name = snap_name.get().trim().to_string();
        if name.is_empty() {
            set_create_error.set(Some("Snapshot name is required".to_string()));
            return;
        }
        let desc = snap_desc.get();
        let desc = (!desc.trim().is_empty()).then_some(desc);
        let memory = snap_memory.get();
        set_creating.set(true);
        spawn_local(async move {
            match api::create_snapshot(&vm_id, &name, desc, memory).await {
                Ok(_) => {
                    set_show_create_dialog.set(false);
                    set_create_error.set(None);
                    set_snap_name.set(String::new());
                    set_snap_desc.set(String::new());
                    if let Some(s) = local_storage() {
                        let _ = s.set_item(LAST_VM_KEY, &vm_id);
                    }
                    // keep the new snapshot in view: a filter on another VM
                    // switches to the snapshotted one
                    let f = selected_vm.get_untracked();
                    if !f.is_empty() && f != vm_id {
                        set_selected_vm.set(vm_id.clone());
                    }
                    set_notice.set(Some(format!("Snapshot {} created", name)));
                    set_refresh.update(|n| *n += 1);
                }
                Err(e) => set_create_error.set(Some(e.message)),
            }
            set_creating.set(false);
        });
    };

    view! {
        <div class="snapshot-list">
            {move || notice.get().map(|n| view! { <div class="alert alert-success" id="snapshot-notice">{n}</div> })}
            <div class="page-header">
                <h1>"Snapshots"</h1>
                <div class="header-actions">
                    <button
                        id="snapshot-create-btn"
                        class="btn btn-primary"
                        on:click=open_create
                        disabled=move || vms.with(|v| v.is_empty())
                    >"Create Snapshot"</button>
                </div>
            </div>
            <crate::pages::snapshot_policies::SnapshotTabs active="snapshots"/>

            <div class="form-group vm-selector" style="max-width:32rem">
                <label for="snapshot-vm-filter">"Show snapshots of"</label>
                <select
                    id="snapshot-vm-filter"
                    on:change=move |ev| set_selected_vm.set(event_target_value(&ev))
                >
                    <option value="" selected=move || selected_vm.get().is_empty()>"All VMs"</option>
                    {move || vms.get().into_iter().map(|vm| {
                        let id = vm.id.clone();
                        view! {
                            <option value={vm.id.clone()} selected=move || selected_vm.get() == id>{format!("{} ({})", vm.name, vm.id)}</option>
                        }
                    }).collect_view()}
                </select>
            </div>

            {move || error.get().map(|err| view! { <p class="error" id="snapshot-error">"Error: " {err}</p> })}

            {move || {
                let list = snapshots.get();
                if vms_loaded.get() && vms.with(|v| v.is_empty()) {
                    view! {
                        <div class="no-data empty-state" id="snapshots-no-vms">
                            <p>"There are no VMs yet, so there is nothing to snapshot."</p>
                            <a class="btn btn-primary" href="/vms/create">"Create VM"</a>
                        </div>
                    }.into_view()
                } else if (loading.get() || !vms_loaded.get()) && list.is_empty() {
                    view! { <p class="loading">"Loading snapshots..."</p> }.into_view()
                } else if list.is_empty() {
                    view! {
                        <div class="no-data" id="snapshots-empty">
                            <p>{move || if selected_vm.get().is_empty() {
                                "No VM has a snapshot yet.".to_string()
                            } else {
                                format!("{} has no snapshots yet.", vm_name(&selected_vm.get()))
                            }}</p>
                            <p>"Create Snapshot saves a VM's current disk (and optionally memory) state."</p>
                        </div>
                    }.into_view()
                } else {
                    view! {
                        <table class="snapshot-table" id="snapshot-table">
                            <thead>
                                <tr>
                                    <th>"VM"</th>
                                    <th>"Name"</th>
                                    <th>"Description"</th>
                                    <th>"Created"</th>
                                    <th>"Size"</th>
                                    <th>"Memory"</th>
                                    <th>"Actions"</th>
                                </tr>
                            </thead>
                            <tbody>
                                {list.into_iter().map(|snapshot| {
                                    let vm_id_restore = snapshot.vm_id.clone();
                                    let snapshot_id_restore = snapshot.id.clone();
                                    let vm_id_delete = snapshot.vm_id.clone();
                                    let snapshot_id_delete = snapshot.id.clone();
                                    let vm_href = format!("/vms/{}", snapshot.vm_id);
                                    let vm_label = vm_name(&snapshot.vm_id);

                                    view! {
                                        <tr data-snapshot-name=snapshot.name.clone() data-vm-id=snapshot.vm_id.clone()>
                                            <td><a href=vm_href>{vm_label}</a></td>
                                            <td><strong>{&snapshot.name}</strong>" "<code class="snapshot-id">{&snapshot.id}</code></td>
                                            <td>
                                                {snapshot.description.as_ref().map(|d| d.to_string()).unwrap_or_else(|| "-".to_string())}
                                            </td>
                                            <td>{&snapshot.created_at}</td>
                                            <td class="snapshot-size">{snapshot.size_label()}</td>
                                            <td>
                                                {if snapshot.memory_included {
                                                    view! { <span class="badge badge-success">"Yes"</span> }.into_view()
                                                } else {
                                                    view! { <span class="badge badge-secondary">"No"</span> }.into_view()
                                                }}
                                            </td>
                                            <td class="actions">
                                                <button
                                                    class="btn btn-sm btn-primary"
                                                    on:click=move |_| restore_snapshot(vm_id_restore.clone(), snapshot_id_restore.clone())
                                                >"Restore"</button>
                                                <button
                                                    class="btn btn-sm btn-danger"
                                                    on:click=move |_| delete_snapshot(vm_id_delete.clone(), snapshot_id_delete.clone())
                                                >"Delete"</button>
                                            </td>
                                        </tr>
                                    }
                                }).collect_view()}
                            </tbody>
                        </table>
                    }.into_view()
                }
            }}

            // Create Snapshot dialog
            {move || {
                if show_create_dialog.get() {
                    view! {
                        <div class="modal">
                            <div class="modal-content">
                                <h2>"Create Snapshot"</h2>
                                <form id="create-snapshot-form" on:submit=submit_create>
                                    {move || create_error.get().map(|e| view! { <div class="alert alert-error" id="create-snapshot-error">{e}</div> })}
                                    <div class="form-group">
                                        <label for="snap-vm">"VM"</label>
                                        <select id="snap-vm" on:change=move |ev| set_snap_vm.set(event_target_value(&ev))>
                                            {move || vms.get().into_iter().map(|vm| {
                                                let id = vm.id.clone();
                                                view! { <option value=vm.id.clone() selected=move || snap_vm.get() == id>{format!("{} ({})", vm.name, vm.id)}</option> }
                                            }).collect_view()}
                                        </select>
                                    </div>
                                    <div class="form-group">
                                        <label for="snap-name">"Name"</label>
                                        <input id="snap-name" type="text" required prop:value=snap_name
                                            on:input=move |ev| set_snap_name.set(event_target_value(&ev))/>
                                    </div>
                                    <div class="form-group">
                                        <label for="snap-desc">"Description"</label>
                                        <input id="snap-desc" type="text" prop:value=snap_desc
                                            on:input=move |ev| set_snap_desc.set(event_target_value(&ev))/>
                                    </div>
                                    <div class="form-group">
                                        <label>
                                            <input id="snap-memory" type="checkbox" prop:checked=snap_memory
                                                on:change=move |ev| set_snap_memory.set(event_target_checked(&ev))/>
                                            " Include memory (running VMs only)"
                                        </label>
                                    </div>
                                    <div class="form-actions">
                                        <button type="button" class="btn btn-secondary"
                                            on:click=move |_| set_show_create_dialog.set(false)>"Cancel"</button>
                                        <button id="snap-submit" type="submit" class="btn btn-primary" disabled=creating>
                                            {move || if creating.get() { "Creating..." } else { "Create" }}
                                        </button>
                                    </div>
                                </form>
                            </div>
                        </div>
                    }.into_view()
                } else {
                    view! { <div></div> }.into_view()
                }
            }}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(vm: &str, name: &str, at: i64) -> api::VmSnapshot {
        serde_json::from_value(serde_json::json!({
            "id": format!("{vm}-{name}"), "vm_id": vm, "name": name, "created_at": at
        }))
        .unwrap()
    }

    #[test]
    fn every_vms_snapshots_are_listed_newest_first() {
        let mut v = vec![
            snap("a", "old", 1_762_000_000),
            snap("b", "new", 1_763_400_000),
            snap("a", "mid", 1_762_800_000),
        ];
        sort_snapshots(&mut v);
        let names: Vec<_> = v.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["new", "mid", "old"]);
    }

    #[test]
    fn the_create_dialog_starts_on_a_useful_vm() {
        let ids = vec!["vm1".to_string(), "vm2".to_string()];
        assert_eq!(
            default_snapshot_vm("vm2", Some("vm1"), &ids).as_deref(),
            Some("vm2")
        );
        // all VMs shown: the VM snapshotted last, if it still exists
        assert_eq!(
            default_snapshot_vm("", Some("vm2"), &ids).as_deref(),
            Some("vm2")
        );
        assert_eq!(
            default_snapshot_vm("", Some("gone"), &ids).as_deref(),
            Some("vm1")
        );
        assert_eq!(default_snapshot_vm("", None, &[]), None);
    }
}
