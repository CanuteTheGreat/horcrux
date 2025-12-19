use leptos::*;
use crate::api::*;

#[component]
pub fn WidgetManagerPage() -> impl IntoView {
    let (widgets, set_widgets) = create_signal(Vec::<DashboardWidget>::new());
    let (loading, set_loading) = create_signal(false);
    let (error_message, set_error_message) = create_signal(None::<String>);
    let (success_message, set_success_message) = create_signal(None::<String>);
    let (search_query, set_search_query) = create_signal(String::new());
    let (filter_type, set_filter_type) = create_signal("all".to_string());
    let (filter_dashboard, set_filter_dashboard) = create_signal("all".to_string());
    let (show_bulk_actions, set_show_bulk_actions) = create_signal(false);
    let (selected_widgets, set_selected_widgets) = create_signal(std::collections::HashSet::<String>::new());

    // Load all widgets across dashboards
    let load_widgets = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);

        match get_all_widgets().await {
            Ok(widget_list) => set_widgets.set(widget_list),
            Err(e) => set_error_message.set(Some(format!("Failed to load widgets: {}", e))),
        }

        set_loading.set(false);
    });

    // Bulk delete action
    let bulk_delete_action = create_action(move |_: &()| async move {
        let widget_ids: Vec<String> = selected_widgets.get().into_iter().collect();
        if widget_ids.is_empty() {
            return;
        }

        set_loading.set(true);
        set_error_message.set(None);

        match bulk_delete_widgets(widget_ids.clone()).await {
            Ok(_) => {
                set_success_message.set(Some(format!("Successfully deleted {} widgets", widget_ids.len())));
                set_selected_widgets.set(std::collections::HashSet::new());
                set_show_bulk_actions.set(false);
                load_widgets.dispatch(());
            }
            Err(e) => set_error_message.set(Some(format!("Failed to delete widgets: {}", e))),
        }

        set_loading.set(false);
    });

    // Bulk move action
    let bulk_move_action = create_action(move |target_dashboard_id: &String| {
        let target_dashboard_id = target_dashboard_id.clone();
        let widget_ids: Vec<String> = selected_widgets.get().into_iter().collect();
        async move {
            if widget_ids.is_empty() {
                return;
            }

            set_loading.set(true);
            set_error_message.set(None);

            match bulk_move_widgets(widget_ids.clone(), target_dashboard_id).await {
                Ok(_) => {
                    set_success_message.set(Some(format!("Successfully moved {} widgets", widget_ids.len())));
                    set_selected_widgets.set(std::collections::HashSet::new());
                    set_show_bulk_actions.set(false);
                    load_widgets.dispatch(());
                }
                Err(e) => set_error_message.set(Some(format!("Failed to move widgets: {}", e))),
            }

            set_loading.set(false);
        }
    });

    // Duplicate widget action
    let duplicate_widget_action = create_action(move |widget: &DashboardWidget| {
        let widget = widget.clone();
        async move {
            set_loading.set(true);
            set_error_message.set(None);

            let request = CreateWidgetRequest {
                title: format!("{} (Copy)", widget.title),
                widget_type: widget.widget_type.clone(),
                metric: widget.metric.clone(),
                width: widget.width,
                height: widget.height,
                position_x: widget.position_x + 1,
                position_y: widget.position_y + 1,
                config: widget.config.clone(),
            };

            match add_dashboard_widget(&widget.dashboard_id, request).await {
                Ok(new_widget) => {
                    set_success_message.set(Some(format!("Widget duplicated as '{}'", new_widget.title)));
                    load_widgets.dispatch(());
                }
                Err(e) => set_error_message.set(Some(format!("Failed to duplicate widget: {}", e))),
            }

            set_loading.set(false);
        }
    });

    // Export widget config
    let export_widget_action = create_action(move |widget: &DashboardWidget| {
        let widget = widget.clone();
        async move {
            let export_data = serde_json::to_string_pretty(&widget.config).unwrap_or_else(|_| "{}".to_string());

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
            element.set_download(&format!("widget-{}.json", widget.title.replace(" ", "_").to_lowercase()));
            element.click();

            web_sys::Url::revoke_object_url(&url).unwrap();
            set_success_message.set(Some("Widget configuration exported".to_string()));
        }
    });

    // Helper functions
    let filtered_widgets = move || {
        let query = search_query.get().to_lowercase();
        let widget_type = filter_type.get();
        let dashboard = filter_dashboard.get();

        widgets.get()
            .into_iter()
            .filter(|widget| {
                let matches_search = query.is_empty() ||
                    widget.title.to_lowercase().contains(&query) ||
                    widget.metric.to_lowercase().contains(&query) ||
                    widget.dashboard_name.to_lowercase().contains(&query);

                let matches_type = widget_type == "all" || widget.widget_type == widget_type;
                let matches_dashboard = dashboard == "all" || widget.dashboard_id == dashboard;

                matches_search && matches_type && matches_dashboard
            })
            .collect::<Vec<_>>()
    };

    let get_dashboard_list = move || -> Vec<(String, String)> {
        let mut dashboards: Vec<(String, String)> = widgets.get()
            .iter()
            .map(|w| (w.dashboard_id.clone(), w.dashboard_name.clone()))
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        dashboards.sort_by(|a, b| a.1.cmp(&b.1));
        dashboards
    };

    let get_widget_type_list = move || -> Vec<String> {
        let mut types: Vec<String> = widgets.get()
            .iter()
            .map(|w| w.widget_type.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        types.sort();
        types
    };

    let toggle_widget_selection = move |widget_id: String| {
        set_selected_widgets.update(|selected| {
            if selected.contains(&widget_id) {
                selected.remove(&widget_id);
            } else {
                selected.insert(widget_id);
            }
        });
    };

    let select_all_widgets = move || {
        let filtered = filtered_widgets();
        set_selected_widgets.update(|selected| {
            if selected.len() == filtered.len() {
                selected.clear();
            } else {
                *selected = filtered.iter().map(|w| w.id.clone()).collect();
            }
        });
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

    // Initial load
    create_effect(move |_| {
        load_widgets.dispatch(());
    });

    view! {
        <div class="widget-manager-page">
            <div class="page-header">
                <div class="header-title">
                    <h1>"Widget Manager"</h1>
                    <p class="header-subtitle">"Manage all widgets across your dashboards"</p>
                </div>
                <div class="header-actions">
                    <button
                        class="btn btn-secondary"
                        on:click=move |_| load_widgets.dispatch(())
                        disabled=loading
                    >
                        "Refresh"
                    </button>
                    {move || if !selected_widgets.get().is_empty() {
                        view! {
                            <button
                                class="btn btn-warning"
                                on:click=move |_| set_show_bulk_actions.update(|s| *s = !*s)
                            >
                                "Bulk Actions ("{selected_widgets.get().len()}")"
                            </button>
                        }.into_view()
                    } else {
                        view! { <div></div> }.into_view()
                    }}
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

            // Bulk actions bar
            {move || if show_bulk_actions.get() && !selected_widgets.get().is_empty() {
                view! {
                    <div class="bulk-actions-bar">
                        <div class="actions-info">
                            <span>{selected_widgets.get().len()}" widgets selected"</span>
                        </div>
                        <div class="actions-buttons">
                            <button
                                class="btn btn-sm btn-secondary"
                                on:click=move |_| {
                                    set_selected_widgets.set(std::collections::HashSet::new());
                                    set_show_bulk_actions.set(false);
                                }
                            >
                                "Clear Selection"
                            </button>
                            <div class="dropdown">
                                <button class="btn btn-sm btn-primary dropdown-toggle">"Move To"</button>
                                <div class="dropdown-menu">
                                    {get_dashboard_list().into_iter().map(|(dashboard_id, dashboard_name)| {
                                        let dash_id = dashboard_id.clone();
                                        view! {
                                            <button
                                                class="dropdown-item"
                                                on:click=move |_| bulk_move_action.dispatch(dash_id.clone())
                                            >
                                                {dashboard_name}
                                            </button>
                                        }
                                    }).collect::<Vec<_>>()}
                                </div>
                            </div>
                            <button
                                class="btn btn-sm btn-danger"
                                on:click=move |_| {
                                    if web_sys::window()
                                        .unwrap()
                                        .confirm_with_message(&format!("Delete {} selected widgets? This action cannot be undone.", selected_widgets.get().len()))
                                        .unwrap_or(false)
                                    {
                                        bulk_delete_action.dispatch(());
                                    }
                                }
                            >
                                "Delete Selected"
                            </button>
                        </div>
                    </div>
                }.into_view()
            } else {
                view! { <div></div> }.into_view()
            }}

            <div class="widget-controls">
                <div class="controls-row">
                    <div class="search-box">
                        <input
                            type="text"
                            prop:value=search_query
                            on:input=move |ev| set_search_query.set(event_target_value(&ev))
                            placeholder="Search widgets..."
                            class="search-input"
                        />
                    </div>

                    <div class="filter-controls">
                        <label>"Type:"</label>
                        <select
                            prop:value=filter_type
                            on:change=move |ev| set_filter_type.set(event_target_value(&ev))
                        >
                            <option value="all">"All Types"</option>
                            {get_widget_type_list().into_iter().map(|widget_type| {
                                let wt_val = widget_type.clone();
                                let wt_display = widget_type.clone();
                                view! {
                                    <option value={wt_val}>{wt_display}</option>
                                }
                            }).collect::<Vec<_>>()}
                        </select>

                        <label>"Dashboard:"</label>
                        <select
                            prop:value=filter_dashboard
                            on:change=move |ev| set_filter_dashboard.set(event_target_value(&ev))
                        >
                            <option value="all">"All Dashboards"</option>
                            {get_dashboard_list().into_iter().map(|(dashboard_id, dashboard_name)| {
                                let dash_id = dashboard_id.clone();
                                view! {
                                    <option value={dash_id}>{dashboard_name}</option>
                                }
                            }).collect::<Vec<_>>()}
                        </select>
                    </div>
                </div>
            </div>

            {move || if loading.get() && widgets.get().is_empty() {
                view! { <div class="loading">"Loading widgets..."</div> }.into_view()
            } else {
                let filtered = filtered_widgets();
                if filtered.is_empty() {
                    view! {
                        <div class="empty-state">
                            <div class="empty-icon">"🧩"</div>
                            <h3>"No widgets found"</h3>
                            <p>"No widgets match the current filters"</p>
                        </div>
                    }.into_view()
                } else {
                    view! {
                        <div class="widgets-table-container">
                            <table class="widgets-table">
                                <thead>
                                    <tr>
                                        <th class="select-col">
                                            <input
                                                type="checkbox"
                                                prop:checked=move || {
                                                    let filtered = filtered_widgets();
                                                    !filtered.is_empty() && selected_widgets.get().len() == filtered.len()
                                                }
                                                on:change=move |_| select_all_widgets()
                                            />
                                        </th>
                                        <th>"Widget"</th>
                                        <th>"Type"</th>
                                        <th>"Metric"</th>
                                        <th>"Dashboard"</th>
                                        <th>"Size"</th>
                                        <th>"Position"</th>
                                        <th>"Created"</th>
                                        <th>"Actions"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {filtered_widgets().into_iter().map(|widget| {
                                        let widget_clone1 = widget.clone();
                                        let widget_clone2 = widget.clone();
                                        let widget_clone3 = widget.clone();
                                        let widget_clone4 = widget.clone();
                                        let widget_id = widget.id.clone();
                                        let widget_title = widget.title.clone();
                                        let widget_id_display = widget.id.clone();
                                        let widget_type = widget.widget_type.clone();
                                        let widget_metric = widget.metric.clone();
                                        let widget_dashboard_id = widget.dashboard_id.clone();
                                        let widget_dashboard_name = widget.dashboard_name.clone();
                                        let widget_width = widget.width;
                                        let widget_height = widget.height;
                                        let widget_pos_x = widget.position_x;
                                        let widget_pos_y = widget.position_y;
                                        let widget_created = widget.created_at[..10].to_string();
                                        let editor_link = format!("/dashboard/editor/{}", widget.dashboard_id);

                                        view! {
                                            <tr class="widget-row">
                                                <td class="widget-select">
                                                    <input
                                                        type="checkbox"
                                                        prop:checked=move || selected_widgets.get().contains(&widget_id)
                                                        on:change=move |_| toggle_widget_selection(widget_clone4.id.clone())
                                                    />
                                                </td>
                                                <td class="widget-title">
                                                    <div class="widget-info">
                                                        <strong>{widget_title}</strong>
                                                        <span class="widget-id">"{widget_id_display}"</span>
                                                    </div>
                                                </td>
                                                <td class="widget-type">
                                                    <span class="type-badge">{widget_type}</span>
                                                </td>
                                                <td class="widget-metric">
                                                    <code>{widget_metric}</code>
                                                </td>
                                                <td class="widget-dashboard">
                                                    <a href={format!("/dashboard/view/{}", widget_dashboard_id)} class="dashboard-link">
                                                        {widget_dashboard_name}
                                                    </a>
                                                </td>
                                                <td class="widget-size">
                                                    {widget_width}"x"{widget_height}
                                                </td>
                                                <td class="widget-position">
                                                    "("{widget_pos_x}", "{widget_pos_y}")"
                                                </td>
                                                <td class="widget-created">
                                                    {widget_created}
                                                </td>
                                                <td class="widget-actions">
                                                    <div class="action-buttons">
                                                        <a
                                                            href={editor_link}
                                                            class="btn btn-xs btn-primary"
                                                            title="Edit in Dashboard"
                                                        >
                                                            "Edit"
                                                        </a>

                                                        <div class="dropdown">
                                                            <button class="btn btn-xs btn-secondary dropdown-toggle">"More"</button>
                                                            <div class="dropdown-menu">
                                                                <button
                                                                    class="dropdown-item"
                                                                    on:click=move |_| duplicate_widget_action.dispatch(widget_clone1.clone())
                                                                >
                                                                    "Duplicate"
                                                                </button>
                                                                <button
                                                                    class="dropdown-item"
                                                                    on:click=move |_| export_widget_action.dispatch(widget_clone2.clone())
                                                                >
                                                                    "Export Config"
                                                                </button>
                                                                <hr class="dropdown-divider"/>
                                                                <button
                                                                    class="dropdown-item text-danger"
                                                                    on:click=move |_| {
                                                                        if web_sys::window()
                                                                            .unwrap()
                                                                            .confirm_with_message(&format!("Delete widget '{}'? This action cannot be undone.", widget_clone3.title))
                                                                            .unwrap_or(false)
                                                                        {
                                                                            let mut to_delete = std::collections::HashSet::new();
                                                                            to_delete.insert(widget_clone3.id.clone());
                                                                            set_selected_widgets.set(to_delete);
                                                                            bulk_delete_action.dispatch(());
                                                                        }
                                                                    }
                                                                >
                                                                    "Delete"
                                                                </button>
                                                            </div>
                                                        </div>
                                                    </div>
                                                </td>
                                            </tr>
                                        }
                                    }).collect::<Vec<_>>()}
                                </tbody>
                            </table>
                        </div>

                        <div class="widgets-summary">
                            <div class="summary-stats">
                                {
                                    let total = widgets.get().len();
                                    let filtered_count = filtered.len();
                                    let selected_count = selected_widgets.get().len();
                                    let widget_types = get_widget_type_list();
                                    let dashboards = get_dashboard_list();

                                    view! {
                                        <>
                                            <div class="stat-item">
                                                <span class="stat-label">"Total Widgets:"</span>
                                                <span class="stat-value">{total}</span>
                                            </div>
                                            <div class="stat-item">
                                                <span class="stat-label">"Showing:"</span>
                                                <span class="stat-value text-blue-600">{filtered_count}</span>
                                            </div>
                                            <div class="stat-item">
                                                <span class="stat-label">"Selected:"</span>
                                                <span class="stat-value text-purple-600">{selected_count}</span>
                                            </div>
                                            <div class="stat-item">
                                                <span class="stat-label">"Widget Types:"</span>
                                                <span class="stat-value text-green-600">{widget_types.len()}</span>
                                            </div>
                                            <div class="stat-item">
                                                <span class="stat-label">"Dashboards:"</span>
                                                <span class="stat-value text-orange-600">{dashboards.len()}</span>
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