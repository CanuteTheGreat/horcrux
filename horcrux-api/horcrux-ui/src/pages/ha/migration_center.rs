use leptos::*;
use crate::api::*;
use web_sys::MouseEvent;
use wasm_bindgen::JsCast;

#[component]
pub fn MigrationCenterPage() -> impl IntoView {
    let (migration_jobs, set_migration_jobs) = create_signal(Vec::<MigrationJob>::new());
    let (vms, set_vms) = create_signal(Vec::<VirtualMachine>::new());
    let (containers, set_containers) = create_signal(Vec::<Container>::new());
    let (cluster_nodes, set_cluster_nodes) = create_signal(Vec::<ClusterNode>::new());
    let (show_migration_modal, set_show_migration_modal) = create_signal(false);
    let (show_bulk_modal, set_show_bulk_modal) = create_signal(false);
    let (selected_resource, set_selected_resource) = create_signal(None::<(String, String)>); // (type, id)
    let (loading, set_loading) = create_signal(false);
    let (error_message, set_error_message) = create_signal(None::<String>);

    // Migration form fields
    let (target_node, set_target_node) = create_signal(String::new());
    let (migration_type, set_migration_type) = create_signal("online".to_string());
    let (force_migration, set_force_migration) = create_signal(false);
    let (migration_timeout, set_migration_timeout) = create_signal(300);
    let (bandwidth_limit, set_bandwidth_limit) = create_signal(None::<u64>);
    let (with_local_disks, set_with_local_disks) = create_signal(false);

    // Bulk migration
    let (selected_resources, set_selected_resources) = create_signal(Vec::<(String, String)>::new());
    let (bulk_target_node, set_bulk_target_node) = create_signal(String::new());
    let (bulk_max_workers, set_bulk_max_workers) = create_signal(2);

    // Auto-refresh timer
    let refresh_interval = create_signal(5000); // 5 seconds

    // Clear migration form
    let clear_migration_form = move || {
        set_target_node.set(String::new());
        set_migration_type.set("online".to_string());
        set_force_migration.set(false);
        set_migration_timeout.set(300);
        set_bandwidth_limit.set(None);
        set_with_local_disks.set(false);
    };

    // Load data
    let load_data = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);

        match get_migration_jobs().await {
            Ok(jobs) => set_migration_jobs.set(jobs),
            Err(e) => set_error_message.set(Some(format!("Failed to load migration jobs: {}", e))),
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

    // Create migration job
    let create_migration = create_action(move |_: &()| async move {
        if let Some((resource_type, resource_id)) = selected_resource.get() {
            set_loading.set(true);
            set_error_message.set(None);

            let migration_job = MigrationJob {
                job_id: format!("migration-{}", chrono::Utc::now().timestamp()),
                resource_type: resource_type.clone(),
                resource_id: resource_id.clone(),
                source_node: "".to_string(), // Will be determined by backend
                target_node: target_node.get(),
                migration_type: migration_type.get(),
                status: "pending".to_string(),
                progress: 0,
                start_time: chrono::Utc::now(),
                end_time: None,
                error: None,
                bandwidth_limit: bandwidth_limit.get(),
                timeout: migration_timeout.get(),
                force: force_migration.get(),
                with_local_disks: with_local_disks.get(),
                estimated_duration: None,
                transferred_bytes: 0,
                remaining_bytes: None,
            };

            match create_migration_job(migration_job).await {
                Ok(_) => {
                    set_show_migration_modal.set(false);
                    set_selected_resource.set(None);
                    clear_migration_form();
                    load_data.dispatch(());
                }
                Err(e) => set_error_message.set(Some(format!("Failed to create migration: {}", e))),
            }

            set_loading.set(false);
        }
    });

    // Bulk migration
    let create_bulk_migration = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);

        let bulk_job = BulkMigrationJob {
            job_id: format!("bulk-migration-{}", chrono::Utc::now().timestamp()),
            target_node: bulk_target_node.get(),
            resources: selected_resources.get(),
            max_workers: bulk_max_workers.get(),
            migration_type: "online".to_string(),
            status: "pending".to_string(),
            start_time: chrono::Utc::now(),
            end_time: None,
            completed_count: 0,
            failed_count: 0,
            total_count: selected_resources.get().len(),
        };

        match create_bulk_migration_job(bulk_job).await {
            Ok(_) => {
                set_show_bulk_modal.set(false);
                set_selected_resources.set(Vec::new());
                load_data.dispatch(());
            }
            Err(e) => set_error_message.set(Some(format!("Failed to create bulk migration: {}", e))),
        }

        set_loading.set(false);
    });

    // Cancel migration
    let cancel_migration = create_action(move |job_id: &String| {
        let job_id = job_id.clone();
        async move {
            set_loading.set(true);
            set_error_message.set(None);

            match cancel_migration_job(job_id).await {
                Ok(_) => load_data.dispatch(()),
                Err(e) => set_error_message.set(Some(format!("Failed to cancel migration: {}", e))),
            }

            set_loading.set(false);
        }
    });

    // Helper functions
    let clear_migration_form = move || {
        set_target_node.set(String::new());
        set_migration_type.set("online".to_string());
        set_force_migration.set(false);
        set_migration_timeout.set(300);
        set_bandwidth_limit.set(None);
        set_with_local_disks.set(false);
    };

    let get_resource_name = move |resource_type: &str, resource_id: &str| {
        if resource_type == "vm" {
            vms.get()
                .iter()
                .find(|vm| vm.vmid.to_string() == resource_id)
                .map(|vm| vm.name.clone())
                .unwrap_or_else(|| format!("VM {}", resource_id))
        } else {
            containers.get()
                .iter()
                .find(|ct| ct.vmid.to_string() == resource_id)
                .map(|ct| ct.hostname.clone())
                .unwrap_or_else(|| format!("CT {}", resource_id))
        }
    };

    let get_optimal_target_node = move |resource_id: &str| {
        // Simple logic to find best node based on resource usage
        cluster_nodes.get()
            .iter()
            .filter(|node| node.status == "online")
            .min_by_key(|node| node.memory_used + node.cpu_usage as u64)
            .map(|node| node.name.clone())
            .unwrap_or_default()
    };

    // Auto-refresh with setInterval from web APIs
    create_effect(move |_| {
        use wasm_bindgen::prelude::*;
        use wasm_bindgen::JsCast;

        let interval = refresh_interval.0.get();
        if interval > 0 {
            let closure = Closure::wrap(Box::new(move || {
                load_data.dispatch(());
            }) as Box<dyn Fn()>);

            web_sys::window()
                .unwrap()
                .set_interval_with_callback_and_timeout_and_arguments_0(
                    closure.as_ref().unchecked_ref(),
                    interval as i32,
                )
                .unwrap();

            closure.forget(); // Keep the closure alive
        }
    });

    // Initial load
    create_effect(move |_| {
        load_data.dispatch(());
    });

    view! {
        <div class="migration-center-page">
            <div class="page-header">
                <h1>"Migration Center"</h1>
                <div class="header-actions">
                    <button
                        class="btn btn-secondary"
                        on:click=move |_| set_show_bulk_modal.set(true)
                        disabled=loading
                    >
                        "Bulk Migration"
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

            <div class="migration-dashboard">
                // Active Migrations
                <div class="migration-section">
                    <h2>"Active Migrations"</h2>
                    {move || if loading.get() {
                        view! { <div class="loading">"Loading migrations..."</div> }.into_view()
                    } else if migration_jobs.get().is_empty() {
                        view! { <div class="empty-state">"No active migrations"</div> }.into_view()
                    } else {
                        view! {
                            <div class="migration-jobs">
                                {migration_jobs.get().into_iter().map(|job| {
                                    let job_clone = job.clone();
                                    let progress_width = format!("{}%", job.progress);
                                    let is_active = job.status == "running" || job.status == "pending";
                                    let duration = job.end_time.map(|end| {
                                        let duration = end.signed_duration_since(job.start_time);
                                        format!("{}s", duration.num_seconds())
                                    }).unwrap_or_else(|| {
                                        let duration = chrono::Utc::now().signed_duration_since(job.start_time);
                                        format!("{}s", duration.num_seconds())
                                    });
                                    let resource_name = get_resource_name(&job.resource_type, &job.resource_id);
                                    let resource_type_upper = job.resource_type.to_uppercase();
                                    let resource_id = job.resource_id.clone();
                                    let source_node = job.source_node.clone();
                                    let target_node = job.target_node.clone();
                                    let migration_type = job.migration_type.clone();
                                    let status = job.status.clone();
                                    let status_lower = job.status.to_lowercase();
                                    let error = job.error.clone();
                                    let progress = job.progress;
                                    let estimated_duration = job.estimated_duration;
                                    let transferred_bytes = job.transferred_bytes;
                                    let remaining_bytes = job.remaining_bytes;

                                    view! {
                                        <div class="migration-job-card">
                                            <div class="job-header">
                                                <div class="resource-info">
                                                    <h3>{resource_name}</h3>
                                                    <span class="resource-id">
                                                        {resource_type_upper}" "{resource_id}
                                                    </span>
                                                </div>
                                                <div class="migration-route">
                                                    <span class="source-node">{source_node}</span>
                                                    <span class="arrow">"->"</span>
                                                    <span class="target-node">{target_node}</span>
                                                </div>
                                                <div class="job-actions">
                                                    {if is_active {
                                                        view! {
                                                            <button
                                                                class="btn btn-sm btn-danger"
                                                                on:click=move |_| {
                                                                    if web_sys::window()
                                                                        .unwrap()
                                                                        .confirm_with_message("Cancel this migration?")
                                                                        .unwrap_or(false)
                                                                    {
                                                                        cancel_migration.dispatch(job_clone.job_id.clone());
                                                                    }
                                                                }
                                                            >
                                                                "Cancel"
                                                            </button>
                                                        }.into_view()
                                                    } else {
                                                        view! {}.into_view()
                                                    }}
                                                </div>
                                            </div>

                                            <div class="job-details">
                                                <div class="detail-row">
                                                    <span class="label">"Type:"</span>
                                                    <span class="value">{migration_type}</span>
                                                    <span class="label">"Status:"</span>
                                                    <span class={format!("status-badge status-{}", status_lower)}>
                                                        {status}
                                                    </span>
                                                    <span class="label">"Duration:"</span>
                                                    <span class="value">{duration}</span>
                                                </div>

                                                {error.map(|e| view! {
                                                    <div class="error-message">{e}</div>
                                                })}
                                            </div>

                                            <div class="progress-section">
                                                <div class="progress-info">
                                                    <span class="progress-text">{format!("{}%", progress)}</span>
                                                    {estimated_duration.map(|est| view! {
                                                        <span class="estimated-time">
                                                            "ETA: "{est}" min"
                                                        </span>
                                                    })}
                                                </div>
                                                <div class="progress-bar">
                                                    <div
                                                        class="progress-fill"
                                                        style=format!("width: {}", progress_width)
                                                    ></div>
                                                </div>
                                                {if transferred_bytes > 0 {
                                                    view! {
                                                        <div class="transfer-info">
                                                            "Transferred: "{format!("{:.1} GB", transferred_bytes as f64 / 1024.0 / 1024.0 / 1024.0)}
                                                            {remaining_bytes.map(|remaining| view! {
                                                                <span>" / Remaining: "{format!("{:.1} GB", remaining as f64 / 1024.0 / 1024.0 / 1024.0)}</span>
                                                            })}
                                                        </div>
                                                    }.into_view()
                                                } else {
                                                    view! {}.into_view()
                                                }}
                                            </div>
                                        </div>
                                    }
                                }).collect::<Vec<_>>()}
                            </div>
                        }.into_view()
                    }}
                </div>

                // Resource Selection for Migration
                <div class="resource-section">
                    <h2>"Available Resources"</h2>

                    <div class="resource-tabs">
                        <div class="tab-content">
                            <h3>"Virtual Machines"</h3>
                            <div class="resource-grid">
                                {vms.get().into_iter().map(|vm| {
                                    let vm_clone = vm.clone();
                                    let vm_clone2 = vm.clone();
                                    let vmid = vm.vmid;
                                    let vmid_str = vmid.to_string();
                                    let vmid_str2 = vmid_str.clone();
                                    let vmid_str3 = vmid_str.clone();
                                    let vm_name = vm.name.clone();
                                    let status = vm.status.clone();
                                    let status_lower = vm.status.to_lowercase();
                                    let node = vm.node.clone();
                                    let maxcpu = vm.maxcpu;
                                    let memory_gb = format!("{:.1} GB", vm.maxmem as f64 / 1024.0 / 1024.0 / 1024.0);
                                    let can_migrate = vm.status == "running" && !migration_jobs.get()
                                        .iter().any(|job| job.resource_id == vmid_str &&
                                                  (job.status == "running" || job.status == "pending"));

                                    view! {
                                        <div class="resource-card">
                                            <div class="resource-header">
                                                <h4>{vm_name}</h4>
                                                <span class="resource-id">"VM " {vmid}</span>
                                            </div>
                                            <div class="resource-info">
                                                <div class="info-row">
                                                    <span class="label">"Status:"</span>
                                                    <span class={format!("status-badge status-{}", status_lower)}>
                                                        {status}
                                                    </span>
                                                </div>
                                                <div class="info-row">
                                                    <span class="label">"Node:"</span>
                                                    <span class="value">{node}</span>
                                                </div>
                                                <div class="info-row">
                                                    <span class="label">"CPU:"</span>
                                                    <span class="value">{maxcpu}" cores"</span>
                                                </div>
                                                <div class="info-row">
                                                    <span class="label">"Memory:"</span>
                                                    <span class="value">{memory_gb}</span>
                                                </div>
                                            </div>
                                            <div class="resource-actions">
                                                <button
                                                    class="btn btn-sm btn-primary"
                                                    disabled=!can_migrate
                                                    on:click=move |_| {
                                                        set_selected_resource.set(Some(("vm".to_string(), vm_clone.vmid.to_string())));
                                                        set_target_node.set(get_optimal_target_node(&vm_clone.vmid.to_string()));
                                                        set_show_migration_modal.set(true);
                                                    }
                                                >
                                                    {if can_migrate { "Migrate" } else { "Unavailable" }}
                                                </button>
                                                <button
                                                    class="btn btn-sm btn-secondary"
                                                    disabled=!can_migrate
                                                    on:click=move |_| {
                                                        let mut resources = selected_resources.get();
                                                        let resource = ("vm".to_string(), vm_clone2.vmid.to_string());
                                                        if resources.contains(&resource) {
                                                            resources.retain(|r| r != &resource);
                                                        } else {
                                                            resources.push(resource);
                                                        }
                                                        set_selected_resources.set(resources);
                                                    }
                                                >
                                                    {if selected_resources.get().contains(&("vm".to_string(), vmid_str2)) {
                                                        "[OK] Selected"
                                                    } else {
                                                        "Select"
                                                    }}
                                                </button>
                                            </div>
                                        </div>
                                    }
                                }).collect::<Vec<_>>()}
                            </div>

                            <h3>"Containers"</h3>
                            <div class="resource-grid">
                                {containers.get().into_iter().map(|container| {
                                    let container_clone = container.clone();
                                    let container_clone2 = container.clone();
                                    let vmid = container.vmid;
                                    let vmid_str = vmid.to_string();
                                    let vmid_str2 = vmid_str.clone();
                                    let hostname = container.hostname.clone();
                                    let status = container.status.clone();
                                    let status_lower = container.status.to_lowercase();
                                    let node = container.node.clone();
                                    let cpus = container.cpus;
                                    let memory_gb = format!("{:.1} GB", container.maxmem as f64 / 1024.0 / 1024.0 / 1024.0);
                                    let can_migrate = container.status == "running" && !migration_jobs.get()
                                        .iter().any(|job| job.resource_id == vmid_str &&
                                                  (job.status == "running" || job.status == "pending"));

                                    view! {
                                        <div class="resource-card">
                                            <div class="resource-header">
                                                <h4>{hostname}</h4>
                                                <span class="resource-id">"CT " {vmid}</span>
                                            </div>
                                            <div class="resource-info">
                                                <div class="info-row">
                                                    <span class="label">"Status:"</span>
                                                    <span class={format!("status-badge status-{}", status_lower)}>
                                                        {status}
                                                    </span>
                                                </div>
                                                <div class="info-row">
                                                    <span class="label">"Node:"</span>
                                                    <span class="value">{node}</span>
                                                </div>
                                                <div class="info-row">
                                                    <span class="label">"CPU:"</span>
                                                    <span class="value">{cpus}" cores"</span>
                                                </div>
                                                <div class="info-row">
                                                    <span class="label">"Memory:"</span>
                                                    <span class="value">{memory_gb}</span>
                                                </div>
                                            </div>
                                            <div class="resource-actions">
                                                <button
                                                    class="btn btn-sm btn-primary"
                                                    disabled=!can_migrate
                                                    on:click=move |_| {
                                                        set_selected_resource.set(Some(("container".to_string(), container_clone.vmid.to_string())));
                                                        set_target_node.set(get_optimal_target_node(&container_clone.vmid.to_string()));
                                                        set_show_migration_modal.set(true);
                                                    }
                                                >
                                                    {if can_migrate { "Migrate" } else { "Unavailable" }}
                                                </button>
                                                <button
                                                    class="btn btn-sm btn-secondary"
                                                    disabled=!can_migrate
                                                    on:click=move |_| {
                                                        let mut resources = selected_resources.get();
                                                        let resource = ("container".to_string(), container_clone2.vmid.to_string());
                                                        if resources.contains(&resource) {
                                                            resources.retain(|r| r != &resource);
                                                        } else {
                                                            resources.push(resource);
                                                        }
                                                        set_selected_resources.set(resources);
                                                    }
                                                >
                                                    {if selected_resources.get().contains(&("container".to_string(), vmid_str2)) {
                                                        "[OK] Selected"
                                                    } else {
                                                        "Select"
                                                    }}
                                                </button>
                                            </div>
                                        </div>
                                    }
                                }).collect::<Vec<_>>()}
                            </div>
                        </div>
                    </div>
                </div>
            </div>

            // Migration Modal
            {move || if show_migration_modal.get() {
                view! {
                    <div class="modal-overlay" on:click=move |e: MouseEvent| {
                        if e.target().and_then(|t| t.dyn_ref::<web_sys::Element>().map(|el| el.class_name())).unwrap_or_default() == "modal-overlay" {
                            set_show_migration_modal.set(false);
                            set_selected_resource.set(None);
                            clear_migration_form();
                        }
                    }>
                        <div class="modal-content">
                            <div class="modal-header">
                                <h2>"Migrate Resource"</h2>
                                <button
                                    class="modal-close"
                                    on:click=move |_| {
                                        set_show_migration_modal.set(false);
                                        set_selected_resource.set(None);
                                        clear_migration_form();
                                    }
                                >"x"</button>
                            </div>
                            <div class="modal-body">
                                {selected_resource.get().map(|(resource_type, resource_id)| view! {
                                    <div class="migration-summary">
                                        <h3>"Migrating: "{get_resource_name(&resource_type, &resource_id)}</h3>
                                        <p class="resource-detail">{resource_type.to_uppercase()}" "{resource_id}</p>
                                    </div>
                                })}

                                <div class="form-group">
                                    <label>"Target Node"</label>
                                    <select
                                        prop:value=target_node
                                        on:change=move |ev| set_target_node.set(event_target_value(&ev))
                                    >
                                        <option value="">"Select target node"</option>
                                        {cluster_nodes.get().into_iter().filter(|n| n.status == "online").map(|node| {
                                            let node_name = node.name.clone();
                                            let node_name_val = node.name.clone();
                                            let cpu_usage = node.cpu_usage;
                                            let memory_gb = format!("{:.1}", node.memory_used as f64 / 1024.0 / 1024.0 / 1024.0);
                                            view! {
                                                <option value={node_name_val}>
                                                    {node_name}" ("{cpu_usage}"% CPU, "
                                                    {memory_gb}"GB used)"
                                                </option>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </select>
                                </div>

                                <div class="form-group">
                                    <label>"Migration Type"</label>
                                    <select
                                        prop:value=migration_type
                                        on:change=move |ev| set_migration_type.set(event_target_value(&ev))
                                    >
                                        <option value="online">"Online (Live Migration)"</option>
                                        <option value="offline">"Offline (Stop & Start)"</option>
                                    </select>
                                </div>

                                <div class="form-row">
                                    <div class="form-group">
                                        <label>"Timeout (seconds)"</label>
                                        <input
                                            type="number"
                                            prop:value=migration_timeout
                                            on:input=move |ev| set_migration_timeout.set(event_target_value(&ev).parse().unwrap_or(300))
                                            min="60"
                                            max="3600"
                                        />
                                    </div>
                                    <div class="form-group">
                                        <label>"Bandwidth Limit (MB/s)"</label>
                                        <input
                                            type="number"
                                            prop:value=move || bandwidth_limit.get().unwrap_or(0)
                                            on:input=move |ev| {
                                                let value = event_target_value(&ev).parse().ok();
                                                set_bandwidth_limit.set(value.filter(|&v| v > 0));
                                            }
                                            placeholder="No limit"
                                            min="0"
                                        />
                                    </div>
                                </div>

                                <div class="form-group">
                                    <label class="checkbox-label">
                                        <input
                                            type="checkbox"
                                            prop:checked=force_migration
                                            on:input=move |ev| set_force_migration.set(event_target_checked(&ev))
                                        />
                                        "Force migration (ignore safety checks)"
                                    </label>
                                </div>

                                <div class="form-group">
                                    <label class="checkbox-label">
                                        <input
                                            type="checkbox"
                                            prop:checked=with_local_disks
                                            on:input=move |ev| set_with_local_disks.set(event_target_checked(&ev))
                                        />
                                        "Migrate with local disks"
                                    </label>
                                </div>
                            </div>
                            <div class="modal-footer">
                                <button
                                    class="btn btn-secondary"
                                    on:click=move |_| {
                                        set_show_migration_modal.set(false);
                                        set_selected_resource.set(None);
                                        clear_migration_form();
                                    }
                                >"Cancel"</button>
                                <button
                                    class="btn btn-primary"
                                    on:click=move |_| create_migration.dispatch(())
                                    disabled=move || target_node.get().is_empty() || loading.get()
                                >"Start Migration"</button>
                            </div>
                        </div>
                    </div>
                }.into_view()
            } else {
                view! {}.into_view()
            }}

            // Bulk Migration Modal
            {move || if show_bulk_modal.get() {
                view! {
                    <div class="modal-overlay" on:click=move |e: MouseEvent| {
                        if e.target().and_then(|t| t.dyn_ref::<web_sys::Element>().map(|el| el.class_name())).unwrap_or_default() == "modal-overlay" {
                            set_show_bulk_modal.set(false);
                            set_selected_resources.set(Vec::new());
                        }
                    }>
                        <div class="modal-content large">
                            <div class="modal-header">
                                <h2>"Bulk Migration"</h2>
                                <button
                                    class="modal-close"
                                    on:click=move |_| {
                                        set_show_bulk_modal.set(false);
                                        set_selected_resources.set(Vec::new());
                                    }
                                >"x"</button>
                            </div>
                            <div class="modal-body">
                                <div class="bulk-summary">
                                    <p>"Selected "{selected_resources.get().len()}" resources for bulk migration"</p>
                                </div>

                                <div class="form-group">
                                    <label>"Target Node"</label>
                                    <select
                                        prop:value=bulk_target_node
                                        on:change=move |ev| set_bulk_target_node.set(event_target_value(&ev))
                                    >
                                        <option value="">"Select target node"</option>
                                        {cluster_nodes.get().into_iter().filter(|n| n.status == "online").map(|node| {
                                            let node_name = node.name.clone();
                                            let node_name_val = node.name.clone();
                                            let cpu_usage = node.cpu_usage;
                                            view! {
                                                <option value={node_name_val}>
                                                    {node_name}" ("{cpu_usage}"% CPU)"
                                                </option>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </select>
                                </div>

                                <div class="form-group">
                                    <label>"Max Concurrent Migrations"</label>
                                    <input
                                        type="number"
                                        prop:value=bulk_max_workers
                                        on:input=move |ev| set_bulk_max_workers.set(event_target_value(&ev).parse().unwrap_or(2))
                                        min="1"
                                        max="10"
                                    />
                                    <small>"Recommended: 1-3 concurrent migrations"</small>
                                </div>

                                <div class="selected-resources">
                                    <h3>"Selected Resources:"</h3>
                                    <div class="resource-list">
                                        {selected_resources.get().into_iter().map(|(resource_type, resource_id)| {
                                            let res_name = get_resource_name(&resource_type, &resource_id);
                                            let res_type_upper = resource_type.to_uppercase();
                                            view! {
                                                <div class="bulk-resource-item">
                                                    <span class="resource-name">{res_name}</span>
                                                    <span class="resource-id">{res_type_upper}" "{resource_id}</span>
                                                </div>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </div>
                                </div>
                            </div>
                            <div class="modal-footer">
                                <button
                                    class="btn btn-secondary"
                                    on:click=move |_| {
                                        set_show_bulk_modal.set(false);
                                        set_selected_resources.set(Vec::new());
                                    }
                                >"Cancel"</button>
                                <button
                                    class="btn btn-primary"
                                    on:click=move |_| create_bulk_migration.dispatch(())
                                    disabled=move || bulk_target_node.get().is_empty() || selected_resources.get().is_empty() || loading.get()
                                >{format!("Migrate {} Resources", selected_resources.get().len())}</button>
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