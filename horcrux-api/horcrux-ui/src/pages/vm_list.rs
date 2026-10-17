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

    let stop_vm = move |vm_id: String| {
        spawn_local(async move {
            let r = api::stop_vm(&vm_id).await;
            report(format!("VM {} stopped", vm_id), r);
        });
    };

    let clone_vm = move |vm_id: String| {
        let name = web_sys::window()
            .and_then(|w| {
                w.prompt_with_message_and_default(
                    &format!("Name for the clone of {}:", vm_id),
                    &format!("{}-clone", vm_id),
                )
                .ok()
                .flatten()
            })
            .unwrap_or_default();
        if name.trim().is_empty() {
            return;
        }
        spawn_local(async move {
            let r = api::clone_vm(&vm_id, &name, "full").await;
            match r {
                Ok(vm) => report(format!("VM {} cloned as {}", vm_id, vm.id), Ok(())),
                Err(e) => report(String::new(), Err(e)),
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
                                        let vm_id_delete = vm.id.clone();
                                        let vm_id_clone = vm.id.clone();
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
                                                    <button
                                                        class="btn btn-sm btn-warning"
                                                        on:click=move |_| stop_vm(vm_id_stop.clone())
                                                    >"Stop"</button>
                                                    <button
                                                        class="btn btn-sm btn-secondary"
                                                        on:click=move |_| clone_vm(vm_id_clone.clone())
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
        </div>
    }
}
