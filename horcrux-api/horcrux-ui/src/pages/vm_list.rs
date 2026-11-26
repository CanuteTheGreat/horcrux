use crate::api;
use crate::components::ui;
use horcrux_common::VmConfig;
use leptos::*;
use leptos_router::*;

/// Does a VM status pass the `?state=` filter of the list? The dashboard's
/// "Running VMs" / "Stopped VMs" cards link here; an empty or unknown
/// filter shows everything.
pub fn vm_state_matches(filter: &str, status: &horcrux_common::VmStatus) -> bool {
    use horcrux_common::VmStatus::*;
    match filter {
        "running" => matches!(status, Running),
        "stopped" => matches!(status, Stopped),
        "paused" => matches!(status, Paused),
        _ => true,
    }
}

/// Display name of a hypervisor enum value ("Qemu" -> "QEMU").
fn hypervisor_label(h: &str) -> String {
    match h {
        "Qemu" => "QEMU".into(),
        "Lxd" => "LXD".into(),
        "Incus" => "Incus".into(),
        o => o.to_string(),
    }
}

/// "vm-12" sorts after "vm-9": compare the numeric tail, then the text.
fn natural_id(id: &str) -> (String, u64) {
    let digits: String = id
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let head = id[..id.len() - digits.len()].to_string();
    (head, digits.parse().unwrap_or(0))
}

#[component]
pub fn VmList() -> impl IntoView {
    let query = use_query_map();
    let state_filter = move || query.with(|q| q.get("state").cloned().unwrap_or_default());
    let (vms, set_vms) = create_signal(Vec::<VmConfig>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    // Saved states (Hyper-V-style "Save state"), by VM id.
    let (saved, set_saved) =
        create_signal(std::collections::HashMap::<String, api::SavedState>::new());
    let (blockers, set_blockers) =
        create_signal(std::collections::HashMap::<String, String>::new());

    // Every action used to only log to the browser console and never
    // refreshed the list, so Start/Stop/Delete looked like they did nothing.
    let reload = move || {
        spawn_local(async move {
            if let Ok(o) = api::power_overview().await {
                set_saved.set(
                    o.guests
                        .iter()
                        .filter(|g| g.kind == "vm")
                        .filter_map(|g| g.saved.clone().map(|s| (g.id.clone(), s)))
                        .collect(),
                );
                set_blockers.set(
                    o.guests
                        .iter()
                        .filter(|g| g.kind == "vm" && !g.can_save)
                        .filter_map(|g| g.save_blocker.clone().map(|b| (g.id.clone(), b)))
                        .collect(),
                );
            }
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

    let save_state = move |vm_id: String| {
        set_notice.set(Some(format!("Saving state of VM {}...", vm_id)));
        spawn_local(async move {
            let r = api::save_guest_state("vm", &vm_id).await.map(|_| ());
            report(format!("VM {} saved to disk and stopped", vm_id), r);
        });
    };
    let resume_state = move |vm_id: String| {
        set_notice.set(Some(format!(
            "Resuming VM {} from its saved state...",
            vm_id
        )));
        spawn_local(async move {
            let r = api::resume_guest_state("vm", &vm_id).await.map(|_| ());
            report(format!("VM {} resumed", vm_id), r);
        });
    };
    let discard_state = move |vm_id: String| {
        spawn_local(async move {
            let r = api::discard_guest_state("vm", &vm_id).await;
            report(format!("Saved state of VM {} discarded", vm_id), r);
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

    // Grid state: text filter, sort, hidden columns (kept per browser).
    let (text, set_text) = create_signal(String::new());
    let (sort, set_sort) = create_signal(("id".to_string(), true));
    let hidden = ui::hidden_columns("vm-grid");
    let rows = move || {
        let filter = state_filter();
        let t = text.get().to_lowercase();
        let mut list: Vec<VmConfig> = vms
            .get()
            .into_iter()
            .filter(|v| vm_state_matches(&filter, &v.status))
            .filter(|v| {
                t.is_empty()
                    || v.name.to_lowercase().contains(&t)
                    || v.id.to_lowercase().contains(&t)
            })
            .collect();
        let (key, asc) = sort.get();
        list.sort_by(|x, y| {
            let o = match key.as_str() {
                "name" => x.name.to_lowercase().cmp(&y.name.to_lowercase()),
                "status" => x.status.to_string().cmp(&y.status.to_string()),
                "cpus" => x.cpus.cmp(&y.cpus),
                "memory" => x.memory.cmp(&y.memory),
                "hypervisor" => format!("{:?}", x.hypervisor).cmp(&format!("{:?}", y.hypervisor)),
                "arch" => format!("{:?}", x.architecture).cmp(&format!("{:?}", y.architecture)),
                _ => natural_id(&x.id).cmp(&natural_id(&y.id)),
            };
            if asc {
                o
            } else {
                o.reverse()
            }
        });
        list
    };
    let filter_link = move |state: &'static str, label: &'static str, id: &'static str| {
        let href = if state.is_empty() {
            "/vms".to_string()
        } else {
            format!("/vms?state={state}")
        };
        view! {
            <a href=href id=id class="filter-link" class:active=move || state_filter() == state
                aria-current=move || (state_filter() == state).then_some("page")>{label}</a>
        }
    };

    view! {
        <div class="vm-list">
            <div class="page-header">
                <div>
                    <h1>"Virtual Machines"</h1>
                    <p class="page-subtitle">{move || {
                        let v = vms.get();
                        let run = v.iter().filter(|x| matches!(x.status, horcrux_common::VmStatus::Running)).count();
                        format!("{} VMs \u{b7} {} running", v.len(), run)
                    }}</p>
                </div>
                <div class="page-actions">
                    <A href="/vms/create" class="btn btn-primary">"Create VM"</A>
                    <A href="/vms/gaming/new" class="btn" id="vm-new-gaming">"New Gaming VM"</A>
                    <details class="row-more">
                        <summary class="btn">"Import"<span class="caret-dn" aria-hidden="true"></span></summary>
                        <div class="menu">
                            <A href="/vms/import-ovf" class="btn">"Import OVF / OVA"</A>
                            <A href="/vms/import-esxi" class="btn">"Import from ESXi"</A>
                            <A href="/vms/import-libvirt" class="btn">"Import from libvirt"</A>
                        </div>
                    </details>
                </div>
            </div>
            <crate::pages::UnmanagedLibvirtBanner/>
            {move || notice.get().map(|n| view! { <div class="alert alert-info" id="vm-notice">{n}</div> })}

            <div class="toolbar">
                <div class="toolbar-start">
                    <nav class="seg-links filter-links" id="vm-state-filter" aria-label="Filter by state">
                        {filter_link("", "All", "vm-filter-all")}
                        {filter_link("running", "Running", "vm-filter-running")}
                        {filter_link("stopped", "Stopped", "vm-filter-stopped")}
                        {filter_link("paused", "Paused", "vm-filter-paused")}
                    </nav>
                    <input type="search" class="grid-filter" id="vm-grid-filter" placeholder="Filter by name or ID"
                        aria-label="Filter VMs by name or ID"
                        on:input=move |ev| set_text.set(event_target_value(&ev))/>
                </div>
                <div class="toolbar-end">
                    <ui::ColumnChooser grid="vm-grid" hidden=hidden columns=vec![
                        ("status", "Status"), ("hypervisor", "Hypervisor"), ("cpus", "CPUs"), ("memory", "Memory"), ("arch", "Architecture"),
                    ]/>
                </div>
            </div>
            <div class="grid-wrap" id="vm-grid">
            {move || {
                if loading.get() {
                    view! { <p class="loading">"Loading VMs..."</p> }.into_view()
                } else if let Some(err) = error.get() {
                    view! { <p class="error">"Error: " {err}</p> }.into_view()
                } else {
                    let filter = state_filter();
                    let total = vms.get().len();
                    let vm_list = rows();
                    if vm_list.is_empty() && total > 0 {
                        view! { <p class="no-data grid-empty" id="vm-filter-empty">{if filter.is_empty() { "No VMs match the filter. ".to_string() } else { format!("No {} VMs. ", filter) }}<A href="/vms">"Show all VMs"</A></p> }.into_view()
                    } else if vm_list.is_empty() {
                        view! {
                            <div class="empty-state">
                                <div class="empty-title">"No virtual machines yet"</div>
                                <p class="empty-todo">"Create one from an ISO, or import an existing VM from libvirt, an OVF/OVA file or ESXi."</p>
                                <div class="empty-actions">
                                    <A href="/vms/create" class="btn btn-primary">"Create VM"</A>
                                    <A href="/vms/import-libvirt" class="btn">"Import from libvirt"</A>
                                </div>
                            </div>
                        }.into_view()
                    } else {
                        let shown = vm_list.len();
                        view! {
                            <table class="vm-table">
                                <thead>
                                    <tr>
                                        <ui::SortTh label="ID" key="id" sort=sort set_sort=set_sort/>
                                        <ui::SortTh label="Name" key="name" sort=sort set_sort=set_sort/>
                                        <ui::SortTh label="Status" key="status" sort=sort set_sort=set_sort/>
                                        <ui::SortTh label="Hypervisor" key="hypervisor" sort=sort set_sort=set_sort/>
                                        <ui::SortTh label="CPUs" key="cpus" sort=sort set_sort=set_sort num=true/>
                                        <ui::SortTh label="Memory" key="memory" sort=sort set_sort=set_sort num=true/>
                                        <ui::SortTh label="Architecture" key="arch" sort=sort set_sort=set_sort/>
                                        <th class="cell-actions">"Actions"</th>
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
                                        let detail_href = format!("/vms/{}", vm.id);
                                        let is_qemu = vm.hypervisor == horcrux_common::VmHypervisor::Qemu;
                                        let vm_id_delete = vm.id.clone();
                                        let vm_id_save = vm.id.clone();
                                        let vm_id_resume = vm.id.clone();
                                        let vm_id_discard = vm.id.clone();
                                        let saved_state = saved.get().get(&vm.id).cloned();
                                        let blocker = blockers.get().get(&vm.id).cloned();
                                        let running = matches!(vm.status, horcrux_common::VmStatus::Running | horcrux_common::VmStatus::Paused);
                                        // Power buttons follow the state: a stopped VM cannot be shut down,
                                        // rebooted or stopped, a running one cannot be started again.
                                        let is_running = vm.status == horcrux_common::VmStatus::Running;
                                        let is_stopped = vm.status == horcrux_common::VmStatus::Stopped;
                                        let vm_for_clone = vm.clone();
                                        let status_class = match vm.status {
                                            horcrux_common::VmStatus::Running => "status-running",
                                            horcrux_common::VmStatus::Stopped => "status-stopped",
                                            horcrux_common::VmStatus::Paused => "status-paused",
                                            horcrux_common::VmStatus::Unknown => "status-unknown",
                                        };
                                        let tone = ui::state_tone(&vm.status.to_string());

                                        view! {
                                            <tr data-vm-id=vm.id.clone()>
                                                <td class="mono" data-col="id">{&vm.id}</td>
                                                <td class="cell-name" data-col="name"><A href=detail_href.clone()>{vm.name.clone()}</A></td>
                                                <td data-col="status"><span class=format!("state-badge {tone} {status_class}")><span class=format!("dot {tone}") aria-hidden="true"></span>{vm.status.to_string()}</span>
                                                    {saved_state.clone().map(|s| view! {
                                                        " " <span class="badge badge-info saved-badge" title=s.mechanism.clone()>{crate::pages::saved_label(&s)}</span>
                                                    })}
                                                    {blocker.map(|b| view! { " " <span class="badge warn save-blocker" title=format!("State can't be saved: {b}")>"no save"</span> })}
                                                </td>
                                                <td data-col="hypervisor">{hypervisor_label(&format!("{:?}", vm.hypervisor))}</td>
                                                <td class="num" data-col="cpus">{vm.cpus}</td>
                                                <td class="num" data-col="memory">{ui::fmt_bytes(vm.memory * 1024 * 1024)}</td>
                                                <td data-col="arch" class="mono">{format!("{:?}", vm.architecture).to_lowercase()}</td>
                                                <td class="cell-actions">
                                                  <div class="actions">
                                                    <A href=console_href class="btn btn-sm">"Console"</A>
                                                    <button
                                                        class="btn btn-sm btn-success vm-start"
                                                        disabled=is_running
                                                        on:click=move |_| start_vm(vm_id_start.clone())
                                                    >{if saved_state.is_some() { "Start (resume)" } else { "Start" }}</button>
                                                    <button
                                                        class="btn btn-sm btn-warning vm-stop"
                                                        disabled=is_stopped
                                                        title="Stop immediately (like pulling the power)"
                                                        on:click=move |_| stop_vm(vm_id_stop.clone())
                                                    >"Stop"</button>
                                                    <ui::RowMenu>
                                                        {is_qemu.then(|| view! {
                                                            <button
                                                                class="btn btn-sm vm-shutdown"
                                                                disabled=is_stopped
                                                                title="ACPI shutdown (the guest powers itself off)"
                                                                on:click=move |_| shutdown_vm(vm_id_shutdown.clone())
                                                            >"Shutdown"</button>
                                                            <button
                                                                class="btn btn-sm vm-reboot"
                                                                disabled=is_stopped
                                                                on:click=move |_| reboot_vm(vm_id_reboot.clone())
                                                            >"Reboot"</button>
                                                        })}
                                                        {saved_state.is_some().then(|| view! {
                                                            <button class="btn btn-sm vm-resume-state"
                                                                on:click=move |_| resume_state(vm_id_resume.clone())>"Resume"</button>
                                                            <button class="btn btn-sm vm-discard-state"
                                                                title="Delete the saved state; the VM then boots normally"
                                                                on:click=move |_| discard_state(vm_id_discard.clone())>"Discard state"</button>
                                                        })}
                                                        {(running && saved_state.is_none()).then(|| view! {
                                                            <button class="btn btn-sm vm-save-state"
                                                                title="Save RAM and device state to disk and stop the VM"
                                                                on:click=move |_| save_state(vm_id_save.clone())>"Save state"</button>
                                                        })}
                                                        <hr/>
                                                        <A href=detail_href class="btn btn-sm vm-detail-link">"Details"</A>
                                                        <A href=hardware_href class="btn btn-sm vm-hardware-link">"Hardware"</A>
                                                        <button
                                                            class="btn btn-sm"
                                                            on:click=move |_| open_clone(vm_for_clone.clone())
                                                        >"Clone"</button>
                                                        <hr/>
                                                        <button
                                                            class="btn btn-sm btn-danger"
                                                            on:click=move |_| delete_vm(vm_id_delete.clone())
                                                        >"Delete"</button>
                                                    </ui::RowMenu>
                                                  </div>
                                                </td>
                                            </tr>
                                        }
                                    }).collect_view()}
                                </tbody>
                            </table>
                            <div class="grid-footer"><span>{format!("{shown} of {total} VMs")}</span></div>
                        }.into_view()
                    }
                }
            }}
            </div>

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

#[cfg(test)]
mod filter_tests {
    use super::vm_state_matches;
    use horcrux_common::VmStatus::*;

    #[test]
    fn state_filter_selects_the_right_subset() {
        assert!(vm_state_matches("running", &Running));
        assert!(!vm_state_matches("running", &Stopped));
        assert!(vm_state_matches("stopped", &Stopped));
        assert!(!vm_state_matches("stopped", &Paused));
        assert!(vm_state_matches("paused", &Paused));
        for s in [Running, Stopped, Paused, Unknown] {
            assert!(vm_state_matches("", &s));
            assert!(vm_state_matches("bogus", &s));
        }
    }
}
