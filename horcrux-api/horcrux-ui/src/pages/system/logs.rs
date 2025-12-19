use leptos::*;
use wasm_bindgen::JsCast;
use crate::api::*;

#[component]
pub fn SystemLogsPage() -> impl IntoView {
    let (logs, set_logs) = create_signal(Vec::<LogEntry>::new());
    let (loading, set_loading) = create_signal(false);
    let (error_message, set_error_message) = create_signal(None::<String>);
    let (auto_refresh, set_auto_refresh) = create_signal(true);
    let (log_level, set_log_level) = create_signal("all".to_string());
    let (service_filter, set_service_filter) = create_signal("all".to_string());
    let (search_query, set_search_query) = create_signal(String::new());
    let (lines_limit, set_lines_limit) = create_signal(100);
    let (follow_logs, set_follow_logs) = create_signal(false);

    // Load system logs
    let load_logs = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);

        let filters = LogFilters {
            level: if log_level.get() == "all" { None } else { Some(log_level.get()) },
            service: if service_filter.get() == "all" { None } else { Some(service_filter.get()) },
            search: if search_query.get().is_empty() { None } else { Some(search_query.get()) },
            limit: Some(lines_limit.get()),
        };

        match get_system_logs(filters).await {
            Ok(log_entries) => set_logs.set(log_entries),
            Err(e) => set_error_message.set(Some(format!("Failed to load system logs: {}", e))),
        }

        set_loading.set(false);
    });

    // Clear logs action
    let clear_logs_action = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);

        match clear_system_logs().await {
            Ok(_) => {
                set_logs.set(Vec::new());
                load_logs.dispatch(());
            }
            Err(e) => set_error_message.set(Some(format!("Failed to clear logs: {}", e))),
        }

        set_loading.set(false);
    });

    // Export logs action
    let export_logs_action = create_action(move |format: &String| {
        let format = format.clone();
        async move {
            set_loading.set(true);
            set_error_message.set(None);

            let filters = LogFilters {
                level: if log_level.get() == "all" { None } else { Some(log_level.get()) },
                service: if service_filter.get() == "all" { None } else { Some(service_filter.get()) },
                search: if search_query.get().is_empty() { None } else { Some(search_query.get()) },
                limit: None, // Export all matching logs
            };

            let format_extension = if format == "csv" { "csv" } else { "json" };
            match export_system_logs(filters, format).await {
                Ok(export_data) => {
                    // Trigger download
                    use wasm_bindgen::prelude::*;
                    let window = web_sys::window().unwrap();
                    let document = window.document().unwrap();

                    let element = document.create_element("a").unwrap();
                    let element = element.dyn_into::<web_sys::HtmlAnchorElement>().unwrap();

                    let blob_parts = js_sys::Array::new();
                    blob_parts.push(&JsValue::from_str(&export_data));

                    let blob = web_sys::Blob::new_with_str_sequence(&blob_parts).unwrap();
                    let url = web_sys::Url::create_object_url_with_blob(&blob).unwrap();

                    element.set_href(&url);
                    element.set_download(&format!("system-logs.{}", format_extension));
                    element.click();

                    web_sys::Url::revoke_object_url(&url).unwrap();
                }
                Err(e) => set_error_message.set(Some(format!("Failed to export logs: {}", e))),
            }

            set_loading.set(false);
        }
    });

    // Helper functions
    let get_level_color = |level: &str| match level.to_lowercase().as_str() {
        "error" => "text-red-600 bg-red-50",
        "warn" | "warning" => "text-yellow-600 bg-yellow-50",
        "info" => "text-blue-600 bg-blue-50",
        "debug" => "text-green-600 bg-green-50",
        "trace" => "text-purple-600 bg-purple-50",
        _ => "text-gray-600 bg-gray-50",
    };

    let format_timestamp = |timestamp: &str| -> String {
        // Format the timestamp for display (assuming ISO format)
        timestamp.chars().take(19).collect::<String>().replace("T", " ")
    };

    let get_service_list = move || -> Vec<String> {
        let mut services: Vec<String> = logs.get()
            .iter()
            .filter_map(|log| log.service.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        services.sort();
        services
    };

    // Auto-refresh setup
    create_effect(move |_| {
        use wasm_bindgen::prelude::*;
        use wasm_bindgen::JsCast;

        // Initial load
        load_logs.dispatch(());

        if auto_refresh.get() {
            let closure = Closure::wrap(Box::new(move || {
                if auto_refresh.get() {
                    load_logs.dispatch(());
                }
            }) as Box<dyn Fn()>);

            web_sys::window()
                .unwrap()
                .set_interval_with_callback_and_timeout_and_arguments_0(
                    closure.as_ref().unchecked_ref(),
                    5000, // 5 seconds for logs
                )
                .unwrap();

            closure.forget();
        }
    });

    view! {
        <div class="system-logs-page">
            <div class="page-header">
                <h1>"System Logs"</h1>
                <div class="header-actions">
                    <button
                        class="btn btn-secondary"
                        on:click=move |_| load_logs.dispatch(())
                        disabled=loading
                    >
                        "Refresh"
                    </button>
                    <div class="dropdown">
                        <button class="btn btn-secondary dropdown-toggle">"Export"</button>
                        <div class="dropdown-menu">
                            <button
                                class="dropdown-item"
                                on:click=move |_| export_logs_action.dispatch("json".to_string())
                                disabled=loading
                            >
                                "Export as JSON"
                            </button>
                            <button
                                class="dropdown-item"
                                on:click=move |_| export_logs_action.dispatch("csv".to_string())
                                disabled=loading
                            >
                                "Export as CSV"
                            </button>
                        </div>
                    </div>
                    <button
                        class="btn btn-danger"
                        on:click=move |_| {
                            if web_sys::window()
                                .unwrap()
                                .confirm_with_message("Clear all system logs? This action cannot be undone.")
                                .unwrap_or(false)
                            {
                                clear_logs_action.dispatch(());
                            }
                        }
                        disabled=loading
                    >
                        "Clear Logs"
                    </button>
                </div>
            </div>

            {move || error_message.get().map(|msg| view! {
                <div class="alert alert-error">{msg}</div>
            })}

            <div class="logs-controls">
                <div class="controls-row">
                    <div class="control-group">
                        <label class="control-label">
                            <input
                                type="checkbox"
                                prop:checked=auto_refresh
                                on:input=move |ev| set_auto_refresh.set(event_target_checked(&ev))
                            />
                            " Auto-refresh (5s)"
                        </label>
                        <label class="control-label">
                            <input
                                type="checkbox"
                                prop:checked=follow_logs
                                on:input=move |ev| set_follow_logs.set(event_target_checked(&ev))
                            />
                            " Follow logs"
                        </label>
                    </div>

                    <div class="search-box">
                        <input
                            type="text"
                            prop:value=search_query
                            on:input=move |ev| set_search_query.set(event_target_value(&ev))
                            placeholder="Search logs..."
                            class="search-input"
                        />
                    </div>
                </div>

                <div class="controls-row">
                    <div class="filter-group">
                        <label>"Log Level:"</label>
                        <select
                            prop:value=log_level
                            on:change=move |ev| {
                                set_log_level.set(event_target_value(&ev));
                                load_logs.dispatch(());
                            }
                        >
                            <option value="all">"All Levels"</option>
                            <option value="error">"Error"</option>
                            <option value="warn">"Warning"</option>
                            <option value="info">"Info"</option>
                            <option value="debug">"Debug"</option>
                            <option value="trace">"Trace"</option>
                        </select>
                    </div>

                    <div class="filter-group">
                        <label>"Service:"</label>
                        <select
                            prop:value=service_filter
                            on:change=move |ev| {
                                set_service_filter.set(event_target_value(&ev));
                                load_logs.dispatch(());
                            }
                        >
                            <option value="all">"All Services"</option>
                            {get_service_list().into_iter().map(|service| {
                                let svc = service.clone();
                                let svc_val = service.clone();
                                view! {
                                    <option value={svc_val}>{svc}</option>
                                }
                            }).collect::<Vec<_>>()}
                        </select>
                    </div>

                    <div class="filter-group">
                        <label>"Lines:"</label>
                        <select
                            prop:value=move || lines_limit.get().to_string()
                            on:change=move |ev| {
                                if let Ok(limit) = event_target_value(&ev).parse::<u32>() {
                                    set_lines_limit.set(limit);
                                    load_logs.dispatch(());
                                }
                            }
                        >
                            <option value="50">"50"</option>
                            <option value="100">"100"</option>
                            <option value="500">"500"</option>
                            <option value="1000">"1000"</option>
                            <option value="5000">"5000"</option>
                        </select>
                    </div>
                </div>
            </div>

            {move || if loading.get() && logs.get().is_empty() {
                view! { <div class="loading">"Loading system logs..."</div> }.into_view()
            } else if logs.get().is_empty() {
                view! { <div class="empty-state">"No log entries found matching the current filters"</div> }.into_view()
            } else {
                view! {
                    <div class="logs-container">
                        <div class="logs-header">
                            <div class="log-stats">
                                <span class="stat-item">
                                    "Showing "{logs.get().len()}" entries"
                                </span>
                                {move || if follow_logs.get() {
                                    view! { <span class="stat-item follow-indicator">"📡 Following"</span> }.into_view()
                                } else {
                                    view! { <span></span> }.into_view()
                                }}
                            </div>
                        </div>

                        <div class="logs-table-container">
                            <table class="logs-table">
                                <thead>
                                    <tr>
                                        <th class="timestamp-col">"Timestamp"</th>
                                        <th class="level-col">"Level"</th>
                                        <th class="service-col">"Service"</th>
                                        <th class="message-col">"Message"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {logs.get().into_iter().map(|log| {
                                        let level_class = get_level_color(&log.level);
                                        let timestamp = format_timestamp(&log.timestamp);
                                        let level = log.level.to_uppercase();
                                        let service = log.service.clone().unwrap_or_else(|| "-".to_string());
                                        let message = log.message.clone();
                                        let details = log.details.clone();

                                        view! {
                                            <tr class="log-row">
                                                <td class="log-timestamp">
                                                    {timestamp}
                                                </td>
                                                <td class="log-level">
                                                    <span class={format!("level-badge {}", level_class)}>
                                                        {level}
                                                    </span>
                                                </td>
                                                <td class="log-service">
                                                    {service}
                                                </td>
                                                <td class="log-message">
                                                    <span class="message-text">{message}</span>
                                                    {details.map(|d| view! {
                                                        <div class="log-details">{d}</div>
                                                    })}
                                                </td>
                                            </tr>
                                        }
                                    }).collect::<Vec<_>>()}
                                </tbody>
                            </table>
                        </div>

                        {move || if follow_logs.get() {
                            // Auto-scroll to bottom when following logs
                            create_effect(move |_| {
                                if let Some(window) = web_sys::window() {
                                    if let Some(document) = window.document() {
                                        if let Some(container) = document.query_selector(".logs-table-container").ok().flatten() {
                                            let container = container.dyn_into::<web_sys::HtmlElement>().unwrap();
                                            container.set_scroll_top(container.scroll_height());
                                        }
                                    }
                                }
                            });
                            view! { <div class="follow-indicator">"Auto-scrolling to latest entries..."</div> }.into_view()
                        } else {
                            view! { <div></div> }.into_view()
                        }}
                    </div>
                }.into_view()
            }}
        </div>
    }
}