use leptos::*;
use leptos_router::*;
use crate::api;
use horcrux_common::VmConfig;

#[component]
pub fn VmList() -> impl IntoView {
    let (vms, set_vms) = create_signal(Vec::<VmConfig>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);

    // Load VMs
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);
            match api::get_vms().await {
                Ok(vm_list) => {
                    set_vms.set(vm_list);
                    set_error.set(None);
                }
                Err(e) => {
                    set_error.set(Some(e.message));
                }
            }
            set_loading.set(false);
        });
    });

    let start_vm = move |vm_id: String| {
        spawn_local(async move {
            if let Err(e) = api::start_vm(&vm_id).await {
                logging::log!("Error starting VM: {}", e.message);
            }
        });
    };

    let stop_vm = move |vm_id: String| {
        spawn_local(async move {
            if let Err(e) = api::stop_vm(&vm_id).await {
                logging::log!("Error stopping VM: {}", e.message);
            }
        });
    };

    let delete_vm = move |vm_id: String| {
        if web_sys::window()
            .and_then(|w| w.confirm_with_message(&format!("Delete VM {}?", vm_id)).ok())
            .unwrap_or(false)
        {
            spawn_local(async move {
                if let Err(e) = api::delete_vm(&vm_id).await {
                    logging::log!("Error deleting VM: {}", e.message);
                }
            });
        }
    };

    view! {
        <div class="vm-list">
            <div class="page-header">
                <h1>"Virtual Machines"</h1>
                <A href="/vms/create" class="btn btn-primary">"Create VM"</A>
            </div>

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
                                        let vm_id = vm.id.clone();
                                        let vm_id_start = vm.id.clone();
                                        let vm_id_stop = vm.id.clone();
                                        let vm_id_delete = vm.id.clone();
                                        let status_class = match vm.status {
                                            horcrux_common::VmStatus::Running => "status-running",
                                            horcrux_common::VmStatus::Stopped => "status-stopped",
                                            horcrux_common::VmStatus::Paused => "status-paused",
                                            horcrux_common::VmStatus::Unknown => "status-unknown",
                                        };

                                        view! {
                                            <tr>
                                                <td>{&vm.id}</td>
                                                <td><strong>{&vm.name}</strong></td>
                                                <td><span class={status_class}>{format!("{:?}", vm.status)}</span></td>
                                                <td>{format!("{:?}", vm.hypervisor)}</td>
                                                <td>{vm.cpus}</td>
                                                <td>{format!("{} MB", vm.memory)}</td>
                                                <td>{format!("{:?}", vm.architecture)}</td>
                                                <td class="actions">
                                                    <button
                                                        class="btn btn-sm btn-success"
                                                        on:click=move |_| start_vm(vm_id_start.clone())
                                                    >"Start"</button>
                                                    <button
                                                        class="btn btn-sm btn-warning"
                                                        on:click=move |_| stop_vm(vm_id_stop.clone())
                                                    >"Stop"</button>
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
        </div>
    }
}
