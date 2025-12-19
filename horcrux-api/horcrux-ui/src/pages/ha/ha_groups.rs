use leptos::*;
use wasm_bindgen::JsCast;
use crate::api::*;
use web_sys::MouseEvent;

#[component]
pub fn HaGroupsPage() -> impl IntoView {
    let (ha_groups, set_ha_groups) = create_signal(Vec::<HaGroup>::new());
    let (vms, set_vms) = create_signal(Vec::<VirtualMachine>::new());
    let (containers, set_containers) = create_signal(Vec::<Container>::new());
    let (cluster_nodes, set_cluster_nodes) = create_signal(Vec::<ClusterNode>::new());
    let (selected_group, set_selected_group) = create_signal(None::<HaGroup>);
    let (show_create_modal, set_show_create_modal) = create_signal(false);
    let (show_edit_modal, set_show_edit_modal) = create_signal(false);
    let (show_assign_modal, set_show_assign_modal) = create_signal(false);
    let (loading, set_loading) = create_signal(false);
    let (error_message, set_error_message) = create_signal(None::<String>);

    // Form fields for HA group creation/editing
    let (form_name, set_form_name) = create_signal(String::new());
    let (form_priority, set_form_priority) = create_signal(1);
    let (form_max_restart, set_form_max_restart) = create_signal(3);
    let (form_max_relocate, set_form_relocate) = create_signal(1);
    let (form_enabled, set_form_enabled) = create_signal(true);
    let (form_comment, set_form_comment) = create_signal(String::new());

    // Assignment form
    let (selected_resources, set_selected_resources) = create_signal(Vec::<String>::new());
    let (assignment_priority, set_assignment_priority) = create_signal(100);

    // Helper functions - defined early so actions can use them
    let clear_form = move || {
        set_form_name.set(String::new());
        set_form_priority.set(1);
        set_form_max_restart.set(3);
        set_form_relocate.set(1);
        set_form_enabled.set(true);
        set_form_comment.set(String::new());
        set_selected_resources.set(Vec::new());
        set_assignment_priority.set(100);
    };

    // Load data
    let load_data = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);

        match get_ha_groups().await {
            Ok(groups) => set_ha_groups.set(groups),
            Err(e) => set_error_message.set(Some(format!("Failed to load HA groups: {}", e))),
        }

        match get_virtual_machines().await {
            Ok(vm_list) => set_vms.set(vm_list),
            Err(_) => {}
        }

        match get_containers().await {
            Ok(container_list) => set_containers.set(container_list),
            Err(_) => {}
        }

        match get_cluster_nodes().await {
            Ok(nodes) => set_cluster_nodes.set(nodes),
            Err(_) => {}
        }

        set_loading.set(false);
    });

    // Create HA group
    let create_group = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);

        let new_group = HaGroup {
            id: format!("ha-{}", chrono::Utc::now().timestamp()),
            name: form_name.get(),
            priority: form_priority.get(),
            max_restart: form_max_restart.get(),
            max_relocate: form_max_relocate.get(),
            enabled: form_enabled.get(),
            comment: if form_comment.get().is_empty() { None } else { Some(form_comment.get()) },
            resources: Vec::new(),
            nodes: cluster_nodes.get().into_iter().map(|n| n.name).collect(),
            state: "active".to_string(),
            restricted: false,
            nofailback: false,
            vm_ids: Vec::new(),
        };

        match create_ha_group(new_group).await {
            Ok(_) => {
                set_show_create_modal.set(false);
                clear_form();
                load_data.dispatch(());
            }
            Err(e) => set_error_message.set(Some(format!("Failed to create HA group: {}", e))),
        }

        set_loading.set(false);
    });

    // Edit HA group
    let edit_group = create_action(move |_: &()| async move {
        if let Some(group) = selected_group.get() {
            set_loading.set(true);
            set_error_message.set(None);

            let updated_group = HaGroup {
                id: group.id.clone(),
                name: form_name.get(),
                priority: form_priority.get(),
                max_restart: form_max_restart.get(),
                max_relocate: form_max_relocate.get(),
                enabled: form_enabled.get(),
                comment: if form_comment.get().is_empty() { None } else { Some(form_comment.get()) },
                resources: group.resources,
                nodes: group.nodes,
                state: group.state,
                restricted: group.restricted,
                nofailback: group.nofailback,
                vm_ids: group.vm_ids,
            };

            match update_ha_group(updated_group).await {
                Ok(_) => {
                    set_show_edit_modal.set(false);
                    set_selected_group.set(None);
                    clear_form();
                    load_data.dispatch(());
                }
                Err(e) => set_error_message.set(Some(format!("Failed to update HA group: {}", e))),
            }

            set_loading.set(false);
        }
    });

    // Assign resources to HA group
    let assign_resources = create_action(move |_: &()| async move {
        if let Some(group) = selected_group.get() {
            set_loading.set(true);
            set_error_message.set(None);

            for resource_id in selected_resources.get().iter() {
                let assignment = HaResourceAssignment {
                    group_id: group.id.clone(),
                    resource_id: resource_id.clone(),
                    resource_type: if vms.get().iter().any(|vm| vm.vmid.to_string() == *resource_id) {
                        "vm".to_string()
                    } else {
                        "container".to_string()
                    },
                    priority: assignment_priority.get(),
                    state: "started".to_string(),
                };

                match assign_resource_to_ha_group(assignment).await {
                    Ok(_) => {}
                    Err(e) => {
                        set_error_message.set(Some(format!("Failed to assign resource {}: {}", resource_id, e)));
                        break;
                    }
                }
            }

            set_show_assign_modal.set(false);
            set_selected_group.set(None);
            set_selected_resources.set(Vec::new());
            load_data.dispatch(());
            set_loading.set(false);
        }
    });

    // Delete HA group
    let delete_group = create_action(move |group_id: &String| {
        let group_id = group_id.clone();
        async move {
            set_loading.set(true);
            set_error_message.set(None);

            match delete_ha_group(group_id).await {
                Ok(_) => load_data.dispatch(()),
                Err(e) => set_error_message.set(Some(format!("Failed to delete HA group: {}", e))),
            }

            set_loading.set(false);
        }
    });

    // Helper function to clear form
    let clear_form = move || {
        set_form_name.set(String::new());
        set_form_priority.set(1);
        set_form_max_restart.set(3);
        set_form_relocate.set(1);
        set_form_enabled.set(true);
        set_form_comment.set(String::new());
    };

    // Initialize form with selected group data
    let init_form_with_group = move |group: &HaGroup| {
        set_form_name.set(group.name.clone());
        set_form_priority.set(group.priority);
        set_form_max_restart.set(group.max_restart);
        set_form_relocate.set(group.max_relocate);
        set_form_enabled.set(group.enabled);
        set_form_comment.set(group.comment.clone().unwrap_or_default());
    };

    // Load initial data
    create_effect(move |_| {
        load_data.dispatch(());
    });

    view! {
        <div class="ha-groups-page">
            <div class="page-header">
                <h1>"High Availability Groups"</h1>
                <div class="header-actions">
                    <button
                        class="btn btn-primary"
                        on:click=move |_| set_show_create_modal.set(true)
                        disabled=loading
                    >
                        "Create HA Group"
                    </button>
                    <button
                        class="btn btn-secondary"
                        on:click=move |_| load_data.dispatch(())
                        disabled=loading
                    >
                        "Refresh"
                    </button>
                </div>
            </div>

            {move || error_message.get().map(|msg| view! {
                <div class="alert alert-error">{msg}</div>
            })}

            {move || if loading.get() {
                view! { <div class="loading">"Loading HA groups..."</div> }.into_view()
            } else {
                view! {
                    <div class="ha-groups-grid">
                        {ha_groups.get().into_iter().map(|group| {
                            let group_clone = group.clone();
                            let group_clone2 = group.clone();
                            let group_clone3 = group.clone();
                            let group_name = group.name.clone();
                            let priority = group.priority;
                            let max_restart = group.max_restart;
                            let max_relocate = group.max_relocate;
                            let state = group.state.clone();
                            let state_lower = group.state.to_lowercase();
                            let enabled = group.enabled;
                            let comment = group.comment.clone();
                            let resources = group.resources.clone();
                            let resources_len = group.resources.len();
                            let nodes = group.nodes.clone();

                            view! {
                                <div class="ha-group-card">
                                    <div class="card-header">
                                        <h3>{group_name}</h3>
                                        <div class="card-actions">
                                            <button
                                                class="btn btn-sm btn-secondary"
                                                on:click=move |_| {
                                                    set_selected_group.set(Some(group_clone.clone()));
                                                    init_form_with_group(&group_clone);
                                                    set_show_edit_modal.set(true);
                                                }
                                            >
                                                "Edit"
                                            </button>
                                            <button
                                                class="btn btn-sm btn-primary"
                                                on:click=move |_| {
                                                    set_selected_group.set(Some(group_clone2.clone()));
                                                    set_show_assign_modal.set(true);
                                                }
                                            >
                                                "Assign Resources"
                                            </button>
                                            <button
                                                class="btn btn-sm btn-danger"
                                                on:click=move |_| {
                                                    if web_sys::window()
                                                        .unwrap()
                                                        .confirm_with_message(&format!("Delete HA group '{}'?", group_clone3.name))
                                                        .unwrap_or(false)
                                                    {
                                                        delete_group.dispatch(group_clone3.id.clone());
                                                    }
                                                }
                                            >
                                                "Delete"
                                            </button>
                                        </div>
                                    </div>
                                    <div class="card-content">
                                        <div class="ha-group-info">
                                            <div class="info-row">
                                                <span class="label">"Priority:"</span>
                                                <span class="value">{priority}</span>
                                            </div>
                                            <div class="info-row">
                                                <span class="label">"Max Restart:"</span>
                                                <span class="value">{max_restart}</span>
                                            </div>
                                            <div class="info-row">
                                                <span class="label">"Max Relocate:"</span>
                                                <span class="value">{max_relocate}</span>
                                            </div>
                                            <div class="info-row">
                                                <span class="label">"State:"</span>
                                                <span class={format!("status-badge status-{}", state_lower)}>
                                                    {state}
                                                </span>
                                            </div>
                                            <div class="info-row">
                                                <span class="label">"Enabled:"</span>
                                                <span class={if enabled { "text-success" } else { "text-danger" }}>
                                                    {if enabled { "Yes" } else { "No" }}
                                                </span>
                                            </div>
                                        </div>

                                        {comment.map(|c| view! {
                                            <div class="ha-group-comment">
                                                <strong>"Comment: "</strong>{c}
                                            </div>
                                        })}

                                        <div class="ha-group-resources">
                                            <h4>"Protected Resources ({}):"</h4> {resources_len}
                                            <div class="resource-list">
                                                {resources.into_iter().map(|resource| {
                                                    let res_type = resource.resource_type.clone();
                                                    let res_id = resource.resource_id.clone();
                                                    let res_priority = resource.priority;
                                                    let res_state = resource.state.clone();
                                                    let res_state_lower = resource.state.to_lowercase();
                                                    view! {
                                                        <div class="resource-item">
                                                            <span class="resource-type">{res_type}</span>
                                                            <span class="resource-id">{res_id}</span>
                                                            <span class="resource-priority">"Priority: "{res_priority}</span>
                                                            <span class={format!("status-badge status-{}", res_state_lower)}>
                                                                {res_state}
                                                            </span>
                                                        </div>
                                                    }
                                                }).collect::<Vec<_>>()}
                                            </div>
                                        </div>

                                        <div class="ha-group-nodes">
                                            <h4>"Target Nodes:"</h4>
                                            <div class="node-list">
                                                {nodes.into_iter().map(|node| view! {
                                                    <span class="node-tag">{node}</span>
                                                }).collect::<Vec<_>>()}
                                            </div>
                                        </div>
                                    </div>
                                </div>
                            }
                        }).collect::<Vec<_>>()}
                    </div>
                }.into_view()
            }}

            // Create HA Group Modal
            {move || if show_create_modal.get() {
                view! {
                    <div class="modal-overlay" on:click=move |e: MouseEvent| {
                        if e.target().and_then(|t| t.dyn_ref::<web_sys::Element>().map(|el| el.class_name())).unwrap_or_default() == "modal-overlay" {
                            set_show_create_modal.set(false);
                            clear_form();
                        }
                    }>
                        <div class="modal-content">
                            <div class="modal-header">
                                <h2>"Create HA Group"</h2>
                                <button
                                    class="modal-close"
                                    on:click=move |_| {
                                        set_show_create_modal.set(false);
                                        clear_form();
                                    }
                                >"x"</button>
                            </div>
                            <div class="modal-body">
                                <div class="form-group">
                                    <label>"Group Name"</label>
                                    <input
                                        type="text"
                                        prop:value=form_name
                                        on:input=move |ev| set_form_name.set(event_target_value(&ev))
                                        placeholder="Enter HA group name"
                                    />
                                </div>
                                <div class="form-row">
                                    <div class="form-group">
                                        <label>"Priority"</label>
                                        <input
                                            type="number"
                                            prop:value=form_priority
                                            on:input=move |ev| set_form_priority.set(event_target_value(&ev).parse().unwrap_or(1))
                                            min="1"
                                        />
                                    </div>
                                    <div class="form-group">
                                        <label>"Max Restart"</label>
                                        <input
                                            type="number"
                                            prop:value=form_max_restart
                                            on:input=move |ev| set_form_max_restart.set(event_target_value(&ev).parse().unwrap_or(3))
                                            min="0"
                                        />
                                    </div>
                                    <div class="form-group">
                                        <label>"Max Relocate"</label>
                                        <input
                                            type="number"
                                            prop:value=form_max_relocate
                                            on:input=move |ev| set_form_relocate.set(event_target_value(&ev).parse().unwrap_or(1))
                                            min="0"
                                        />
                                    </div>
                                </div>
                                <div class="form-group">
                                    <label class="checkbox-label">
                                        <input
                                            type="checkbox"
                                            prop:checked=form_enabled
                                            on:input=move |ev| set_form_enabled.set(event_target_checked(&ev))
                                        />
                                        "Enabled"
                                    </label>
                                </div>
                                <div class="form-group">
                                    <label>"Comment (Optional)"</label>
                                    <textarea
                                        prop:value=form_comment
                                        on:input=move |ev| set_form_comment.set(event_target_value(&ev))
                                        placeholder="Optional description or notes"
                                        rows="3"
                                    ></textarea>
                                </div>
                            </div>
                            <div class="modal-footer">
                                <button
                                    class="btn btn-secondary"
                                    on:click=move |_| {
                                        set_show_create_modal.set(false);
                                        clear_form();
                                    }
                                >"Cancel"</button>
                                <button
                                    class="btn btn-primary"
                                    on:click=move |_| create_group.dispatch(())
                                    disabled=move || form_name.get().is_empty() || loading.get()
                                >"Create Group"</button>
                            </div>
                        </div>
                    </div>
                }.into_view()
            } else {
                view! {}.into_view()
            }}

            // Edit HA Group Modal
            {move || if show_edit_modal.get() {
                view! {
                    <div class="modal-overlay" on:click=move |e: MouseEvent| {
                        if e.target().and_then(|t| t.dyn_ref::<web_sys::Element>().map(|el| el.class_name())).unwrap_or_default() == "modal-overlay" {
                            set_show_edit_modal.set(false);
                            set_selected_group.set(None);
                            clear_form();
                        }
                    }>
                        <div class="modal-content">
                            <div class="modal-header">
                                <h2>"Edit HA Group"</h2>
                                <button
                                    class="modal-close"
                                    on:click=move |_| {
                                        set_show_edit_modal.set(false);
                                        set_selected_group.set(None);
                                        clear_form();
                                    }
                                >"x"</button>
                            </div>
                            <div class="modal-body">
                                <div class="form-group">
                                    <label>"Group Name"</label>
                                    <input
                                        type="text"
                                        prop:value=form_name
                                        on:input=move |ev| set_form_name.set(event_target_value(&ev))
                                        placeholder="Enter HA group name"
                                    />
                                </div>
                                <div class="form-row">
                                    <div class="form-group">
                                        <label>"Priority"</label>
                                        <input
                                            type="number"
                                            prop:value=form_priority
                                            on:input=move |ev| set_form_priority.set(event_target_value(&ev).parse().unwrap_or(1))
                                            min="1"
                                        />
                                    </div>
                                    <div class="form-group">
                                        <label>"Max Restart"</label>
                                        <input
                                            type="number"
                                            prop:value=form_max_restart
                                            on:input=move |ev| set_form_max_restart.set(event_target_value(&ev).parse().unwrap_or(3))
                                            min="0"
                                        />
                                    </div>
                                    <div class="form-group">
                                        <label>"Max Relocate"</label>
                                        <input
                                            type="number"
                                            prop:value=form_max_relocate
                                            on:input=move |ev| set_form_relocate.set(event_target_value(&ev).parse().unwrap_or(1))
                                            min="0"
                                        />
                                    </div>
                                </div>
                                <div class="form-group">
                                    <label class="checkbox-label">
                                        <input
                                            type="checkbox"
                                            prop:checked=form_enabled
                                            on:input=move |ev| set_form_enabled.set(event_target_checked(&ev))
                                        />
                                        "Enabled"
                                    </label>
                                </div>
                                <div class="form-group">
                                    <label>"Comment (Optional)"</label>
                                    <textarea
                                        prop:value=form_comment
                                        on:input=move |ev| set_form_comment.set(event_target_value(&ev))
                                        placeholder="Optional description or notes"
                                        rows="3"
                                    ></textarea>
                                </div>
                            </div>
                            <div class="modal-footer">
                                <button
                                    class="btn btn-secondary"
                                    on:click=move |_| {
                                        set_show_edit_modal.set(false);
                                        set_selected_group.set(None);
                                        clear_form();
                                    }
                                >"Cancel"</button>
                                <button
                                    class="btn btn-primary"
                                    on:click=move |_| edit_group.dispatch(())
                                    disabled=move || form_name.get().is_empty() || loading.get()
                                >"Update Group"</button>
                            </div>
                        </div>
                    </div>
                }.into_view()
            } else {
                view! {}.into_view()
            }}

            // Assign Resources Modal
            {move || if show_assign_modal.get() {
                view! {
                    <div class="modal-overlay" on:click=move |e: MouseEvent| {
                        if e.target().and_then(|t| t.dyn_ref::<web_sys::Element>().map(|el| el.class_name())).unwrap_or_default() == "modal-overlay" {
                            set_show_assign_modal.set(false);
                            set_selected_group.set(None);
                            set_selected_resources.set(Vec::new());
                        }
                    }>
                        <div class="modal-content large">
                            <div class="modal-header">
                                <h2>"Assign Resources to HA Group"</h2>
                                <button
                                    class="modal-close"
                                    on:click=move |_| {
                                        set_show_assign_modal.set(false);
                                        set_selected_group.set(None);
                                        set_selected_resources.set(Vec::new());
                                    }
                                >"x"</button>
                            </div>
                            <div class="modal-body">
                                <div class="form-group">
                                    <label>"Resource Priority"</label>
                                    <input
                                        type="number"
                                        prop:value=assignment_priority
                                        on:input=move |ev| set_assignment_priority.set(event_target_value(&ev).parse().unwrap_or(100))
                                        min="1"
                                        max="1000"
                                        placeholder="100"
                                    />
                                    <small>"Lower numbers have higher priority"</small>
                                </div>

                                <div class="resource-tabs">
                                    <div class="tab-content">
                                        <h3>"Virtual Machines"</h3>
                                        <div class="resource-selection">
                                            {vms.get().into_iter().map(|vm| {
                                                let vm_id = vm.vmid.to_string();
                                                let vm_id_clone = vm_id.clone();
                                                let vm_id_check = vm_id.clone();
                                                let vm_name = vm.name.clone();
                                                let vmid = vm.vmid;
                                                let status = vm.status.clone();
                                                let status_lower = vm.status.to_lowercase();
                                                view! {
                                                    <label class="resource-checkbox">
                                                        <input
                                                            type="checkbox"
                                                            prop:checked=move || selected_resources.get().contains(&vm_id_check)
                                                            on:input=move |ev| {
                                                                let mut resources = selected_resources.get();
                                                                if event_target_checked(&ev) {
                                                                    if !resources.contains(&vm_id_clone) {
                                                                        resources.push(vm_id_clone.clone());
                                                                    }
                                                                } else {
                                                                    resources.retain(|id| id != &vm_id_clone);
                                                                }
                                                                set_selected_resources.set(resources);
                                                            }
                                                        />
                                                        <div class="resource-info">
                                                            <span class="resource-name">{vm_name}</span>
                                                            <span class="resource-id">"VM " {vmid}</span>
                                                            <span class={format!("status-badge status-{}", status_lower)}>
                                                                {status}
                                                            </span>
                                                        </div>
                                                    </label>
                                                }
                                            }).collect::<Vec<_>>()}
                                        </div>

                                        <h3>"Containers"</h3>
                                        <div class="resource-selection">
                                            {containers.get().into_iter().map(|container| {
                                                let container_id = container.vmid.to_string();
                                                let container_id_clone = container_id.clone();
                                                let container_id_check = container_id.clone();
                                                let hostname = container.hostname.clone();
                                                let vmid = container.vmid;
                                                let status = container.status.clone();
                                                let status_lower = container.status.to_lowercase();
                                                view! {
                                                    <label class="resource-checkbox">
                                                        <input
                                                            type="checkbox"
                                                            prop:checked=move || selected_resources.get().contains(&container_id_check)
                                                            on:input=move |ev| {
                                                                let mut resources = selected_resources.get();
                                                                if event_target_checked(&ev) {
                                                                    if !resources.contains(&container_id_clone) {
                                                                        resources.push(container_id_clone.clone());
                                                                    }
                                                                } else {
                                                                    resources.retain(|id| id != &container_id_clone);
                                                                }
                                                                set_selected_resources.set(resources);
                                                            }
                                                        />
                                                        <div class="resource-info">
                                                            <span class="resource-name">{hostname}</span>
                                                            <span class="resource-id">"CT " {vmid}</span>
                                                            <span class={format!("status-badge status-{}", status_lower)}>
                                                                {status}
                                                            </span>
                                                        </div>
                                                    </label>
                                                }
                                            }).collect::<Vec<_>>()}
                                        </div>
                                    </div>
                                </div>
                            </div>
                            <div class="modal-footer">
                                <button
                                    class="btn btn-secondary"
                                    on:click=move |_| {
                                        set_show_assign_modal.set(false);
                                        set_selected_group.set(None);
                                        set_selected_resources.set(Vec::new());
                                    }
                                >"Cancel"</button>
                                <button
                                    class="btn btn-primary"
                                    on:click=move |_| assign_resources.dispatch(())
                                    disabled=move || selected_resources.get().is_empty() || loading.get()
                                >{format!("Assign {} Resources", selected_resources.get().len())}</button>
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