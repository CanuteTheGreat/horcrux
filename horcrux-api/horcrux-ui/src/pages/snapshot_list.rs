use leptos::*;
use crate::api;

#[component]
pub fn SnapshotList() -> impl IntoView {
    let (selected_vm, set_selected_vm) = create_signal(String::new());
    let (snapshots, set_snapshots) = create_signal(Vec::<api::VmSnapshot>::new());
    let (vms, set_vms) = create_signal(Vec::<horcrux_common::VmConfig>::new());
    let (loading, set_loading) = create_signal(false);
    let (error, set_error) = create_signal(None::<String>);
    let (show_create_dialog, set_show_create_dialog) = create_signal(false);

    // Load VMs for dropdown
    create_effect(move |_| {
        spawn_local(async move {
            match api::get_vms().await {
                Ok(vm_list) => {
                    set_vms.set(vm_list);
                }
                Err(e) => {
                    logging::log!("Error loading VMs: {}", e.message);
                }
            }
        });
    });

    // Load snapshots when VM is selected
    create_effect(move |_| {
        let vm_id = selected_vm.get();
        if !vm_id.is_empty() {
            spawn_local(async move {
                set_loading.set(true);
                match api::get_vm_snapshots(&vm_id).await {
                    Ok(snapshot_list) => {
                        set_snapshots.set(snapshot_list);
                        set_error.set(None);
                    }
                    Err(e) => {
                        set_error.set(Some(e.message));
                    }
                }
                set_loading.set(false);
            });
        }
    });

    let restore_snapshot = move |vm_id: String, snapshot_id: String| {
        if web_sys::window()
            .and_then(|w| w.confirm_with_message(&format!("Restore snapshot {}? Current state will be lost.", snapshot_id)).ok())
            .unwrap_or(false)
        {
            spawn_local(async move {
                if let Err(e) = api::restore_snapshot(&vm_id, &snapshot_id).await {
                    logging::log!("Error restoring snapshot: {}", e.message);
                } else {
                    logging::log!("Snapshot restored successfully");
                }
            });
        }
    };

    let delete_snapshot = move |vm_id: String, snapshot_id: String| {
        if web_sys::window()
            .and_then(|w| w.confirm_with_message(&format!("Delete snapshot {}?", snapshot_id)).ok())
            .unwrap_or(false)
        {
            spawn_local(async move {
                if let Err(e) = api::delete_snapshot(&vm_id, &snapshot_id).await {
                    logging::log!("Error deleting snapshot: {}", e.message);
                }
            });
        }
    };

    view! {
        <div class="snapshot-list">
            <div class="page-header">
                <h1>"VM Snapshots"</h1>
                <div class="header-actions">
                    <button
                        class="btn btn-primary"
                        on:click=move |_| set_show_create_dialog.set(true)
                        disabled=move || selected_vm.get().is_empty()
                    >"Create Snapshot"</button>
                </div>
            </div>

            <div class="vm-selector">
                <label>"Select VM:"</label>
                <select
                    on:change=move |ev| {
                        let value = event_target_value(&ev);
                        set_selected_vm.set(value);
                    }
                >
                    <option value="">"-- Select a VM --"</option>
                    {move || vms.get().into_iter().map(|vm| {
                        view! {
                            <option value={vm.id.clone()}>{format!("{} ({})", vm.name, vm.id)}</option>
                        }
                    }).collect_view()}
                </select>
            </div>

            {move || {
                if selected_vm.get().is_empty() {
                    view! {
                        <div class="no-data">
                            <p>"Please select a VM to view its snapshots."</p>
                        </div>
                    }.into_view()
                } else if loading.get() {
                    view! { <p class="loading">"Loading snapshots..."</p> }.into_view()
                } else if let Some(err) = error.get() {
                    view! { <p class="error">"Error: " {err}</p> }.into_view()
                } else {
                    let snapshot_list = snapshots.get();
                    if snapshot_list.is_empty() {
                        view! {
                            <div class="no-data">
                                <p>"No snapshots found for this VM."</p>
                                <p>"Create a snapshot to save the current state!"</p>
                            </div>
                        }.into_view()
                    } else {
                        view! {
                            <table class="snapshot-table">
                                <thead>
                                    <tr>
                                        <th>"ID"</th>
                                        <th>"Name"</th>
                                        <th>"Description"</th>
                                        <th>"Created"</th>
                                        <th>"Size"</th>
                                        <th>"Memory"</th>
                                        <th>"Actions"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {snapshot_list.into_iter().map(|snapshot| {
                                        let vm_id_restore = snapshot.vm_id.clone();
                                        let snapshot_id_restore = snapshot.id.clone();
                                        let vm_id_delete = snapshot.vm_id.clone();
                                        let snapshot_id_delete = snapshot.id.clone();

                                        view! {
                                            <tr>
                                                <td><code>{&snapshot.id}</code></td>
                                                <td><strong>{&snapshot.name}</strong></td>
                                                <td>
                                                    {snapshot.description.as_ref().map(|d| d.to_string()).unwrap_or_else(|| "-".to_string())}
                                                </td>
                                                <td>{&snapshot.created_at}</td>
                                                <td>{format!("{} MB", snapshot.size_mb)}</td>
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
                }
            }}

            // Create Snapshot Dialog (simplified)
            {move || {
                if show_create_dialog.get() {
                    view! {
                        <div class="modal">
                            <div class="modal-content">
                                <h2>"Create Snapshot"</h2>
                                <p>"Snapshot creation form would go here"</p>
                                <button
                                    class="btn btn-secondary"
                                    on:click=move |_| set_show_create_dialog.set(false)
                                >"Close"</button>
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
