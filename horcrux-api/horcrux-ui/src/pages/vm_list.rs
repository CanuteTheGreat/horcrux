use crate::api;
use horcrux_common::VmConfig;
use leptos::*;
use leptos_router::*;

#[component]
pub fn VmList() -> impl IntoView {
    let (vms, set_vms) = create_signal(Vec::<VmConfig>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);

    // Every action used to only log to the browser console and never
    // refreshed the list, so Start/Stop/Delete looked like they did nothing.
    let reload = move || {
        spawn_local(async move {
            match api::get_vms().await {
                Ok(vm_list) => {
                    set_vms.set(vm_list);
                    set_error.set(None);
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            set_loading.set(false);
        });
    };

    create_effect(move |_| {
        set_loading.set(true);
        reload();
    });

    let report = move |what: String, r: Result<(), api::ApiError>| {
        match r {
            Ok(()) => set_notice.set(Some(what)),
            Err(e) => set_notice.set(Some(format!("Failed: {}", e.message))),
        }
        reload();
    };

    let start_vm = move |vm_id: String| {
        spawn_local(async move {
            let r = api::start_vm(&vm_id).await;
            report(format!("VM {} started", vm_id), r);
        });
    };

    // Clean shutdown (ACPI power button), reboot.
    let shutdown_vm = move |vm_id: String| {
        set_notice.set(Some(format!("Shutting down VM {}...", vm_id)));
        spawn_local(async move {
            let r = api::shutdown_vm(&vm_id, 60, false).await.map(|_| ());
            report(format!("VM {} shut down", vm_id), r);
        });
    };
    let reboot_vm = move |vm_id: String| {
        set_notice.set(Some(format!("Rebooting VM {}...", vm_id)));
        spawn_local(async move {
            let r = api::reboot_vm(&vm_id).await.map(|_| ());
            report(format!("VM {} rebooted", vm_id), r);
        });
    };

    let stop_vm = move |vm_id: String| {
        spawn_local(async move {
            let r = api::stop_vm(&vm_id).await;
            report(format!("VM {} stopped", vm_id), r);
        });
    };

    // Clone dialog: name, full or linked, start afterwards. (It used to be
    // a browser prompt for the name that always sent mode "full".)
    let (clone_src, set_clone_src) = create_signal(None::<VmConfig>);
    let (clone_name, set_clone_name) = create_signal(String::new());
    let (clone_mode, set_clone_mode) = create_signal("full".to_string());
    let (clone_start, set_clone_start) = create_signal(false);
    let (clone_error, set_clone_error) = create_signal(None::<String>);
    let (cloning, set_cloning) = create_signal(false);

    let open_clone = move |vm: VmConfig| {
        set_clone_name.set(format!("{}-clone", vm.name));
        set_clone_mode.set("full".to_string());
        set_clone_start.set(false);
        set_clone_error.set(None);
        set_clone_src.set(Some(vm));
    };

    let submit_clone = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let Some(src) = clone_src.get() else { return };
        let name = clone_name.get();
        if name.trim().is_empty() {
            set_clone_error.set(Some("Enter a name for the clone".to_string()));
            return;
        }
        let (mode, start) = (clone_mode.get(), clone_start.get());
        set_cloning.set(true);
        spawn_local(async move {
            let r = api::clone_vm(&src.id, &name, &mode, start).await;
            set_cloning.set(false);
            match r {
                Ok(vm) => {
                    set_clone_src.set(None);
                    report(
                        format!("VM {} cloned ({}) as {} ({})", src.id, mode, vm.name, vm.id),
                        Ok(()),
                    );
                }
                // Keep the dialog open with the API's reason.
                Err(e) => set_clone_error.set(Some(e.message)),
            }
        });
    };

    let delete_vm = move |vm_id: String| {
        if web_sys::window()
            .and_then(|w| {
                w.confirm_with_message(&format!("Delete VM {}?", vm_id))
                    .ok()
            })
            .unwrap_or(false)
        {
            spawn_local(async move {
                let r = api::delete_vm(&vm_id).await;
                report(format!("VM {} deleted", vm_id), r);
            });
        }
    };

    view! {
        <div class="vm-list">
            <div class="page-header">
                <h1>"Virtual Machines"</h1>
                <A href="/vms/create" class="btn btn-primary">"Create VM"</A>
            </div>
            {move || notice.get().map(|n| view! { <div class="alert alert-info" id="vm-notice">{n}</div> })}

            {move || {
                if loading.get() {
                    view! { <p class="loading">"Loading VMs..."</p> }.into_view()
                } else if let Some(err) = error.get() {
                    view! { <p class="error">"Error: " {err}</p> }.into_view()
                } else {
                    let vm_list = vms.get();
                    if vm_list.is_empty() {
                        view! { <p class="no-data">"No virtual machines found. Create one to get started!"</p> }.into_view()
                    } else {
                        view! {
                            <table class="vm-table">
                                <thead>
                                    <tr>
                                        <th>"ID"</th>
                                        <th>"Name"</th>
                                        <th>"Status"</th>
                                        <th>"Hypervisor"</th>
                                        <th>"CPUs"</th>
                                        <th>"Memory"</th>
                                        <th>"Architecture"</th>
                                        <th>"Actions"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {vm_list.into_iter().map(|vm| {
                                        let console_href = format!("/vms/{}/console", vm.id);
                                        let vm_id_start = vm.id.clone();
                                        let vm_id_stop = vm.id.clone();
                                        let vm_id_shutdown = vm.id.clone();
                                        let vm_id_reboot = vm.id.clone();
                                        let hardware_href = format!("/vms/{}/hardware", vm.id);
                                        let is_qemu = vm.hypervisor == horcrux_common::VmHypervisor::Qemu;
                                        let vm_id_delete = vm.id.clone();
                                        let vm_for_clone = vm.clone();
                                        let status_class = match vm.status {
                                            horcrux_common::VmStatus::Running => "status-running",
                                            horcrux_common::VmStatus::Stopped => "status-stopped",
                                            horcrux_common::VmStatus::Paused => "status-paused",
                                            horcrux_common::VmStatus::Unknown => "status-unknown",
                                        };

                                        view! {
                                            <tr data-vm-id=vm.id.clone()>
                                                <td>{&vm.id}</td>
                                                <td><strong>{&vm.name}</strong></td>
                                                <td><span class={status_class}>{vm.status.to_string()}</span></td>
                                                <td>{format!("{:?}", vm.hypervisor)}</td>
                                                <td>{vm.cpus}</td>
                                                <td>{format!("{} MB", vm.memory)}</td>
                                                <td>{format!("{:?}", vm.architecture)}</td>
                                                <td class="actions">
                                                    <A href=console_href class="btn btn-sm btn-primary">"Console"</A>
                                                    <button
                                                        class="btn btn-sm btn-success"
                                                        on:click=move |_| start_vm(vm_id_start.clone())
                                                    >"Start"</button>
                                                    {is_qemu.then(|| view! {
                                                        <button
                                                            class="btn btn-sm btn-warning vm-shutdown"
                                                            title="ACPI shutdown (the guest powers itself off)"
                                                            on:click=move |_| shutdown_vm(vm_id_shutdown.clone())
                                                        >"Shutdown"</button>
                                                        <button
                                                            class="btn btn-sm btn-secondary vm-reboot"
                                                            on:click=move |_| reboot_vm(vm_id_reboot.clone())
                                                        >"Reboot"</button>
                                                    })}
                                                    <button
                                                        class="btn btn-sm btn-warning"
                                                        title="Stop immediately (like pulling the power)"
                                                        on:click=move |_| stop_vm(vm_id_stop.clone())
                                                    >"Stop"</button>
                                                    <A href=hardware_href class="btn btn-sm btn-secondary vm-hardware-link">"Hardware"</A>
                                                    <button
                                                        class="btn btn-sm btn-secondary"
                                                        on:click=move |_| open_clone(vm_for_clone.clone())
                                                    >"Clone"</button>
                                                    <button
                                                        class="btn btn-sm btn-danger"
                                                        on:click=move |_| delete_vm(vm_id_delete.clone())
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

            {move || clone_src.get().map(|src| view! {
                <div class="modal" id="clone-dialog">
                    <div class="modal-content">
                        <h2>"Clone " {src.name.clone()}</h2>
                        <form id="clone-vm-form" on:submit=submit_clone>
                            {move || clone_error.get().map(|e| view! { <div class="alert alert-error" id="clone-error">{e}</div> })}
                            <div class="form-group">
                                <label for="clone-name">"Name"</label>
                                <input id="clone-name" type="text" required prop:value=clone_name
                                    on:input=move |ev| set_clone_name.set(event_target_value(&ev))/>
                            </div>
                            <div class="form-group">
                                <label for="clone-mode">"Mode"</label>
                                <select id="clone-mode" prop:value=clone_mode
                                    on:change=move |ev| set_clone_mode.set(event_target_value(&ev))>
                                    <option value="full">"Full clone (independent copy of the disk)"</option>
                                    <option value="linked">"Linked clone (qcow2 overlay on this VM's disk)"</option>
                                </select>
                                {move || (clone_mode.get() == "linked").then(|| view! {
                                    <small class="form-help" id="clone-linked-help">
                                        "A linked clone only stores its own changes and reads the rest from this VM's disk, so this VM can't be deleted while the clone exists."
                                    </small>
                                })}
                            </div>
                            <div class="form-group">
                                <label>
                                    <input id="clone-start" type="checkbox" prop:checked=clone_start
                                        on:change=move |ev| set_clone_start.set(event_target_checked(&ev))/>
                                    " Start the clone afterwards"
                                </label>
                            </div>
                            <div class="form-actions">
                                <button type="button" class="btn btn-secondary"
                                    on:click=move |_| set_clone_src.set(None)>"Cancel"</button>
                                <button id="clone-submit" type="submit" class="btn btn-primary" disabled=cloning>
                                    {move || if cloning.get() { "Cloning..." } else { "Clone" }}
                                </button>
                            </div>
                        </form>
                    </div>
                </div>
            })}
        </div>
    }
}
