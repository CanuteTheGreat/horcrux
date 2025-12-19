use leptos::*;
use crate::api::*;

#[component]
pub fn ServicesPage() -> impl IntoView {
    let (services, set_services) = create_signal(Vec::<ServiceStatus>::new());
    let (loading, set_loading) = create_signal(false);
    let (error_message, set_error_message) = create_signal(None::<String>);
    let (success_message, set_success_message) = create_signal(None::<String>);
    let (filter_status, set_filter_status) = create_signal("all".to_string());
    let (search_query, set_search_query) = create_signal(String::new());

    // Load services
    let load_services = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);

        match get_services_status().await {
            Ok(service_list) => set_services.set(service_list),
            Err(e) => set_error_message.set(Some(format!("Failed to load services: {}", e))),
        }

        set_loading.set(false);
    });

    // Control service action
    let control_service_action = create_action(move |(service_name, action): &(String, String)| {
        let service_name = service_name.clone();
        let action = action.clone();
        async move {
            set_loading.set(true);
            set_error_message.set(None);

            match control_service(&service_name, &action).await {
                Ok(_) => {
                    set_success_message.set(Some(format!("Successfully {} service '{}'",
                        match action.as_str() {
                            "start" => "started",
                            "stop" => "stopped",
                            "restart" => "restarted",
                            "enable" => "enabled",
                            "disable" => "disabled",
                            _ => "controlled",
                        }, service_name)));
                    // Reload services after action
                    load_services.dispatch(());
                }
                Err(e) => set_error_message.set(Some(format!("Failed to {} service '{}': {}", action, service_name, e))),
            }

            set_loading.set(false);
        }
    });

    // Filter services based on status and search query
    let filtered_services = move || {
        let query = search_query.get().to_lowercase();
        let status_filter = filter_status.get();

        services.get()
            .into_iter()
            .filter(|service| {
                let matches_search = query.is_empty() ||
                    service.name.to_lowercase().contains(&query) ||
                    service.description.to_lowercase().contains(&query);

                let matches_status = status_filter == "all" ||
                    service.status == status_filter;

                matches_search && matches_status
            })
            .collect::<Vec<_>>()
    };

    // Helper functions
    let get_status_color = |status: &str| match status {
        "active" => "text-green-600",
        "inactive" => "text-gray-500",
        "failed" => "text-red-600",
        _ => "text-yellow-600",
    };

    let get_status_icon = |status: &str| match status {
        "active" => "🟢",
        "inactive" => "⚫",
        "failed" => "🔴",
        _ => "🟡",
    };

    let format_memory = |bytes: Option<u64>| -> String {
        match bytes {
            Some(b) => {
                if b >= 1024 * 1024 * 1024 {
                    format!("{:.1} GB", b as f64 / (1024.0 * 1024.0 * 1024.0))
                } else if b >= 1024 * 1024 {
                    format!("{:.1} MB", b as f64 / (1024.0 * 1024.0))
                } else if b >= 1024 {
                    format!("{:.1} KB", b as f64 / 1024.0)
                } else {
                    format!("{} B", b)
                }
            }
            None => "-".to_string(),
        }
    };

    let can_start = |service: &ServiceStatus| -> bool {
        service.status == "inactive" && service.enabled
    };

    let can_stop = |service: &ServiceStatus| -> bool {
        service.status == "active"
    };

    // Clear messages after delay
    let clear_messages = move || {
        set_timeout(
            move || {
                set_success_message.set(None);
                set_error_message.set(None);
            },
            std::time::Duration::from_secs(5),
        );
    };

    // Auto-refresh every 30 seconds
    create_effect(move |_| {
        use wasm_bindgen::prelude::*;
        use wasm_bindgen::JsCast;

        // Initial load
        load_services.dispatch(());

        // Set up auto-refresh
        let closure = Closure::wrap(Box::new(move || {
            load_services.dispatch(());
        }) as Box<dyn Fn()>);

        web_sys::window()
            .unwrap()
            .set_interval_with_callback_and_timeout_and_arguments_0(
                closure.as_ref().unchecked_ref(),
                30000, // 30 seconds
            )
            .unwrap();

        closure.forget();
    });

    view! {
        <div class="services-page">
            <div class="page-header">
                <h1>"System Services"</h1>
                <div class="header-actions">
                    <button
                        class="btn btn-secondary"
                        on:click=move |_| load_services.dispatch(())
                        disabled=loading
                    >
                        "Refresh"
                    </button>
                </div>
            </div>

            {move || error_message.get().map(|msg| {
                clear_messages();
                view! {
                    <div class="alert alert-error">{msg}</div>
                }
            })}

            {move || success_message.get().map(|msg| {
                clear_messages();
                view! {
                    <div class="alert alert-success">{msg}</div>
                }
            })}

            <div class="services-controls">
                <div class="controls-row">
                    <div class="search-box">
                        <input
                            type="text"
                            prop:value=search_query
                            on:input=move |ev| set_search_query.set(event_target_value(&ev))
                            placeholder="Search services..."
                            class="search-input"
                        />
                    </div>
                    <div class="filter-controls">
                        <label>"Filter by status:"</label>
                        <select
                            prop:value=filter_status
                            on:change=move |ev| set_filter_status.set(event_target_value(&ev))
                        >
                            <option value="all">"All"</option>
                            <option value="active">"Active"</option>
                            <option value="inactive">"Inactive"</option>
                            <option value="failed">"Failed"</option>
                        </select>
                    </div>
                </div>
            </div>

            {move || if loading.get() && services.get().is_empty() {
                view! { <div class="loading">"Loading services..."</div> }.into_view()
            } else {
                let filtered = filtered_services();
                if filtered.is_empty() {
                    view! { <div class="empty-state">"No services found matching the current filters"</div> }.into_view()
                } else {
                    view! {
                        <div class="services-table-container">
                            <table class="services-table">
                                <thead>
                                    <tr>
                                        <th>"Service"</th>
                                        <th>"Status"</th>
                                        <th>"Enabled"</th>
                                        <th>"PID"</th>
                                        <th>"Memory"</th>
                                        <th>"CPU %"</th>
                                        <th>"Description"</th>
                                        <th>"Actions"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {filtered_services().into_iter().map(|service| {
                                        // Clones for disabled checks (need separate clones from on:click handlers)
                                        let service_for_start_disabled = service.clone();
                                        let service_for_start_click = service.clone();
                                        let service_for_stop_disabled = service.clone();
                                        let service_for_stop_click = service.clone();
                                        let service_for_restart_disabled = service.clone();
                                        let service_for_restart_click = service.clone();
                                        let service_clone4 = service.clone();
                                        let service_clone5 = service.clone();
                                        let status_color = get_status_color(&service.status);
                                        let status_icon = get_status_icon(&service.status);
                                        let service_name = service.name.clone();
                                        let status = service.status.clone();
                                        let enabled = service.enabled;
                                        let pid = service.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".to_string());
                                        let memory = format_memory(service.memory_usage);
                                        let cpu = service.cpu_usage.map(|c| format!("{:.1}%", c)).unwrap_or_else(|| "-".to_string());
                                        let description = service.description.clone();
                                        let enabled_class = if enabled { "text-green-600" } else { "text-gray-500" };
                                        let enabled_text = if enabled { "[OK] Yes" } else { "[X] No" };

                                        view! {
                                            <tr class="service-row">
                                                <td class="service-name">
                                                    <strong>{service_name}</strong>
                                                </td>
                                                <td class="service-status">
                                                    <span class={format!("status-indicator {}", status_color)}>
                                                        {status_icon}" "{status}
                                                    </span>
                                                </td>
                                                <td class="service-enabled">
                                                    <span class=enabled_class>
                                                        {enabled_text}
                                                    </span>
                                                </td>
                                                <td class="service-pid">
                                                    {pid}
                                                </td>
                                                <td class="service-memory">
                                                    {memory}
                                                </td>
                                                <td class="service-cpu">
                                                    {cpu}
                                                </td>
                                                <td class="service-description">
                                                    <span class="description-text">{description}</span>
                                                </td>
                                                <td class="service-actions">
                                                    <div class="action-buttons">
                                                        <button
                                                            class="btn btn-xs btn-success"
                                                            disabled=move || !can_start(&service_for_start_disabled) || loading.get()
                                                            on:click=move |_| {
                                                                if web_sys::window()
                                                                    .unwrap()
                                                                    .confirm_with_message(&format!("Start service '{}'?", service_for_start_click.name))
                                                                    .unwrap_or(false)
                                                                {
                                                                    control_service_action.dispatch((service_for_start_click.name.clone(), "start".to_string()));
                                                                }
                                                            }
                                                        >
                                                            "Start"
                                                        </button>
                                                        <button
                                                            class="btn btn-xs btn-warning"
                                                            disabled=move || !can_stop(&service_for_stop_disabled) || loading.get()
                                                            on:click=move |_| {
                                                                if web_sys::window()
                                                                    .unwrap()
                                                                    .confirm_with_message(&format!("Stop service '{}'?", service_for_stop_click.name))
                                                                    .unwrap_or(false)
                                                                {
                                                                    control_service_action.dispatch((service_for_stop_click.name.clone(), "stop".to_string()));
                                                                }
                                                            }
                                                        >
                                                            "Stop"
                                                        </button>
                                                        <button
                                                            class="btn btn-xs btn-primary"
                                                            disabled=move || service_for_restart_disabled.status != "active" || loading.get()
                                                            on:click=move |_| {
                                                                if web_sys::window()
                                                                    .unwrap()
                                                                    .confirm_with_message(&format!("Restart service '{}'?", service_for_restart_click.name))
                                                                    .unwrap_or(false)
                                                                {
                                                                    control_service_action.dispatch((service_for_restart_click.name.clone(), "restart".to_string()));
                                                                }
                                                            }
                                                        >
                                                            "Restart"
                                                        </button>
                                                        {if enabled {
                                                            view! {
                                                                <button
                                                                    class="btn btn-xs btn-danger"
                                                                    disabled=loading
                                                                    on:click=move |_| {
                                                                        if web_sys::window()
                                                                            .unwrap()
                                                                            .confirm_with_message(&format!("Disable service '{}'?", service_clone4.name))
                                                                            .unwrap_or(false)
                                                                        {
                                                                            control_service_action.dispatch((service_clone4.name.clone(), "disable".to_string()));
                                                                        }
                                                                    }
                                                                >
                                                                    "Disable"
                                                                </button>
                                                            }.into_view()
                                                        } else {
                                                            view! {
                                                                <button
                                                                    class="btn btn-xs btn-secondary"
                                                                    disabled=loading
                                                                    on:click=move |_| {
                                                                        if web_sys::window()
                                                                            .unwrap()
                                                                            .confirm_with_message(&format!("Enable service '{}'?", service_clone5.name))
                                                                            .unwrap_or(false)
                                                                        {
                                                                            control_service_action.dispatch((service_clone5.name.clone(), "enable".to_string()));
                                                                        }
                                                                    }
                                                                >
                                                                    "Enable"
                                                                </button>
                                                            }.into_view()
                                                        }}
                                                    </div>
                                                </td>
                                            </tr>
                                        }
                                    }).collect::<Vec<_>>()}
                                </tbody>
                            </table>
                        </div>

                        <div class="services-summary">
                            <div class="summary-stats">
                                {
                                    let total = services.get().len();
                                    let active = services.get().iter().filter(|s| s.status == "active").count();
                                    let inactive = services.get().iter().filter(|s| s.status == "inactive").count();
                                    let failed = services.get().iter().filter(|s| s.status == "failed").count();
                                    let enabled = services.get().iter().filter(|s| s.enabled).count();

                                    view! {
                                        <>
                                            <div class="stat-item">
                                                <span class="stat-label">"Total Services:"</span>
                                                <span class="stat-value">{total}</span>
                                            </div>
                                            <div class="stat-item">
                                                <span class="stat-label">"Active:"</span>
                                                <span class="stat-value text-green-600">{active}</span>
                                            </div>
                                            <div class="stat-item">
                                                <span class="stat-label">"Inactive:"</span>
                                                <span class="stat-value text-gray-500">{inactive}</span>
                                            </div>
                                            <div class="stat-item">
                                                <span class="stat-label">"Failed:"</span>
                                                <span class="stat-value text-red-600">{failed}</span>
                                            </div>
                                            <div class="stat-item">
                                                <span class="stat-label">"Enabled:"</span>
                                                <span class="stat-value text-blue-600">{enabled}</span>
                                            </div>
                                        </>
                                    }
                                }
                            </div>
                        </div>
                    }.into_view()
                }
            }}
        </div>
    }
}