use leptos::*;
use crate::api::*;

#[component]
pub fn DashboardBuilderPage() -> impl IntoView {
    let (dashboards, set_dashboards) = create_signal(Vec::<CustomDashboard>::new());
    let (loading, set_loading) = create_signal(false);
    let (error_message, set_error_message) = create_signal(None::<String>);
    let (success_message, set_success_message) = create_signal(None::<String>);
    let (show_create_modal, set_show_create_modal) = create_signal(false);
    let (show_edit_modal, set_show_edit_modal) = create_signal(false);
    let (selected_dashboard, set_selected_dashboard) = create_signal(None::<CustomDashboard>);
    let (search_query, set_search_query) = create_signal(String::new());
    let (filter_category, set_filter_category) = create_signal("all".to_string());

    // Form state for dashboard creation/editing
    let (dashboard_name, set_dashboard_name) = create_signal(String::new());
    let (dashboard_description, set_dashboard_description) = create_signal(String::new());
    let (dashboard_category, set_dashboard_category) = create_signal("monitoring".to_string());
    let (dashboard_layout, set_dashboard_layout) = create_signal("grid".to_string());
    let (dashboard_refresh_interval, set_dashboard_refresh_interval) = create_signal(30);
    let (dashboard_public, set_dashboard_public) = create_signal(false);
    let (dashboard_tags, set_dashboard_tags) = create_signal(String::new());

    // Helper function to clear form (defined early so actions can use it)
    let clear_form = move || {
        set_dashboard_name.set(String::new());
        set_dashboard_description.set(String::new());
        set_dashboard_category.set("monitoring".to_string());
        set_dashboard_layout.set("grid".to_string());
        set_dashboard_refresh_interval.set(30);
        set_dashboard_public.set(false);
        set_dashboard_tags.set(String::new());
        set_selected_dashboard.set(None);
    };

    // Load dashboards
    let load_dashboards = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);

        match get_custom_dashboards().await {
            Ok(dashboard_list) => set_dashboards.set(dashboard_list),
            Err(e) => set_error_message.set(Some(format!("Failed to load dashboards: {}", e))),
        }

        set_loading.set(false);
    });

    // Create dashboard action
    let create_dashboard_action = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);

        let tags: Vec<String> = dashboard_tags.get()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let request = CreateDashboardRequest {
            name: dashboard_name.get(),
            description: if dashboard_description.get().is_empty() {
                None
            } else {
                Some(dashboard_description.get())
            },
            category: dashboard_category.get(),
            layout: dashboard_layout.get(),
            refresh_interval: dashboard_refresh_interval.get(),
            public: dashboard_public.get(),
            tags,
        };

        match create_custom_dashboard(request).await {
            Ok(dashboard) => {
                set_success_message.set(Some(format!("Dashboard '{}' created successfully", dashboard.name)));
                set_show_create_modal.set(false);
                clear_form();
                load_dashboards.dispatch(());
            }
            Err(e) => set_error_message.set(Some(format!("Failed to create dashboard: {}", e))),
        }

        set_loading.set(false);
    });

    // Update dashboard action
    let update_dashboard_action = create_action(move |_: &()| async move {
        if let Some(dashboard) = selected_dashboard.get() {
            set_loading.set(true);
            set_error_message.set(None);

            let tags: Vec<String> = dashboard_tags.get()
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();

            let request = UpdateDashboardRequest {
                name: Some(dashboard_name.get()),
                description: if dashboard_description.get().is_empty() {
                    None
                } else {
                    Some(dashboard_description.get())
                },
                category: Some(dashboard_category.get()),
                layout: Some(dashboard_layout.get()),
                refresh_interval: Some(dashboard_refresh_interval.get()),
                public: Some(dashboard_public.get()),
                tags: Some(tags),
            };

            match update_custom_dashboard(&dashboard.id, request).await {
                Ok(updated_dashboard) => {
                    set_success_message.set(Some(format!("Dashboard '{}' updated successfully", updated_dashboard.name)));
                    set_show_edit_modal.set(false);
                    clear_form();
                    load_dashboards.dispatch(());
                }
                Err(e) => set_error_message.set(Some(format!("Failed to update dashboard: {}", e))),
            }

            set_loading.set(false);
        }
    });

    // Delete dashboard action
    let delete_dashboard_action = create_action(move |dashboard_id: &String| {
        let dashboard_id = dashboard_id.clone();
        async move {
            set_loading.set(true);
            set_error_message.set(None);

            match delete_custom_dashboard(&dashboard_id).await {
                Ok(_) => {
                    set_success_message.set(Some("Dashboard deleted successfully".to_string()));
                    load_dashboards.dispatch(());
                }
                Err(e) => set_error_message.set(Some(format!("Failed to delete dashboard: {}", e))),
            }

            set_loading.set(false);
        }
    });

    // Clone dashboard action
    let clone_dashboard_action = create_action(move |dashboard: &CustomDashboard| {
        let dashboard = dashboard.clone();
        async move {
            set_loading.set(true);
            set_error_message.set(None);

            let request = CloneDashboardRequest {
                name: format!("{} (Copy)", dashboard.name),
                copy_widgets: true,
            };

            match clone_custom_dashboard(&dashboard.id, request).await {
                Ok(cloned_dashboard) => {
                    set_success_message.set(Some(format!("Dashboard cloned as '{}'", cloned_dashboard.name)));
                    load_dashboards.dispatch(());
                }
                Err(e) => set_error_message.set(Some(format!("Failed to clone dashboard: {}", e))),
            }

            set_loading.set(false);
        }
    });

    // Export dashboard action
    let export_dashboard_action = create_action(move |dashboard: &CustomDashboard| {
        let dashboard = dashboard.clone();
        async move {
            set_loading.set(true);
            set_error_message.set(None);

            match export_dashboard(&dashboard.id).await {
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
                    element.set_download(&format!("dashboard-{}.json", dashboard.name.replace(" ", "_").to_lowercase()));
                    element.click();

                    web_sys::Url::revoke_object_url(&url).unwrap();
                    set_success_message.set(Some("Dashboard exported successfully".to_string()));
                }
                Err(e) => set_error_message.set(Some(format!("Failed to export dashboard: {}", e))),
            }

            set_loading.set(false);
        }
    });

    // Helper functions
    let open_edit_modal = move |dashboard: CustomDashboard| {
        set_dashboard_name.set(dashboard.name.clone());
        set_dashboard_description.set(dashboard.description.clone().unwrap_or_default());
        set_dashboard_category.set(dashboard.category.clone());
        set_dashboard_layout.set(dashboard.layout.clone());
        set_dashboard_refresh_interval.set(dashboard.refresh_interval);
        set_dashboard_public.set(dashboard.public);
        set_dashboard_tags.set(dashboard.tags.join(", "));
        set_selected_dashboard.set(Some(dashboard));
        set_show_edit_modal.set(true);
    };

    let filtered_dashboards = move || {
        let query = search_query.get().to_lowercase();
        let category = filter_category.get();

        dashboards.get()
            .into_iter()
            .filter(|dashboard| {
                let matches_search = query.is_empty() ||
                    dashboard.name.to_lowercase().contains(&query) ||
                    dashboard.description.as_ref().map_or(false, |d| d.to_lowercase().contains(&query)) ||
                    dashboard.tags.iter().any(|tag| tag.to_lowercase().contains(&query));

                let matches_category = category == "all" || dashboard.category == category;

                matches_search && matches_category
            })
            .collect::<Vec<_>>()
    };

    let get_category_list = move || -> Vec<String> {
        let mut categories: Vec<String> = dashboards.get()
            .iter()
            .map(|d| d.category.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        categories.sort();
        categories
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
        load_dashboards.dispatch(());
    });

    view! {
        <div class="dashboard-builder-page">
            <div class="page-header">
                <div class="header-title">
                    <h1>"Dashboard Builder"</h1>
                    <p class="header-subtitle">"Create and manage custom monitoring dashboards"</p>
                </div>
                <div class="header-actions">
                    <button
                        class="btn btn-secondary"
                        on:click=move |_| load_dashboards.dispatch(())
                        disabled=loading
                    >
                        "Refresh"
                    </button>
                    <button
                        class="btn btn-primary"
                        on:click=move |_| set_show_create_modal.set(true)
                        disabled=loading
                    >
                        "Create Dashboard"
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

            <div class="dashboard-controls">
                <div class="controls-row">
                    <div class="search-box">
                        <input
                            type="text"
                            prop:value=search_query
                            on:input=move |ev| set_search_query.set(event_target_value(&ev))
                            placeholder="Search dashboards..."
                            class="search-input"
                        />
                    </div>
                    <div class="filter-controls">
                        <label>"Filter by category:"</label>
                        <select
                            prop:value=filter_category
                            on:change=move |ev| set_filter_category.set(event_target_value(&ev))
                        >
                            <option value="all">"All Categories"</option>
                            {get_category_list().into_iter().map(|category| {
                                let cat_val = category.clone();
                                let cat_display = category.clone();
                                view! {
                                    <option value={cat_val}>{cat_display}</option>
                                }
                            }).collect::<Vec<_>>()}
                        </select>
                    </div>
                </div>
            </div>

            {move || if loading.get() && dashboards.get().is_empty() {
                view! { <div class="loading">"Loading dashboards..."</div> }.into_view()
            } else {
                let filtered = filtered_dashboards();
                if filtered.is_empty() {
                    view! {
                        <div class="empty-state">
                            <div class="empty-icon">"📊"</div>
                            <h3>"No dashboards found"</h3>
                            <p>"Create your first custom dashboard to get started"</p>
                            <button
                                class="btn btn-primary"
                                on:click=move |_| set_show_create_modal.set(true)
                            >
                                "Create Dashboard"
                            </button>
                        </div>
                    }.into_view()
                } else {
                    view! {
                        <div class="dashboards-grid">
                            {filtered_dashboards().into_iter().map(|dashboard| {
                                let dashboard_clone1 = dashboard.clone();
                                let dashboard_clone2 = dashboard.clone();
                                let dashboard_clone3 = dashboard.clone();
                                let dashboard_clone4 = dashboard.clone();
                                let dash_id = dashboard.id.clone();
                                let dash_name = dashboard.name.clone();
                                let dash_category = dashboard.category.clone();
                                let dash_description = dashboard.description.clone();
                                let dash_layout = dashboard.layout.clone();
                                let dash_widget_count = dashboard.widget_count;
                                let dash_refresh = dashboard.refresh_interval;
                                let dash_public = dashboard.public;
                                let dash_tags = dashboard.tags.clone();
                                let dash_created = dashboard.created_at[..10].to_string();
                                let dash_updated = dashboard.updated_at[..10].to_string();
                                let view_link = format!("/dashboard/view/{}", dashboard.id);
                                let editor_link = format!("/dashboard/editor/{}", dashboard.id);
                                let visibility_class = if dashboard.public { "text-green-600" } else { "text-gray-600" };

                                view! {
                                    <div class="dashboard-card">
                                        <div class="card-header">
                                            <div class="card-title">
                                                <h3>{dash_name}</h3>
                                                <span class="category-badge">{dash_category}</span>
                                            </div>
                                            <div class="card-actions">
                                                <div class="dropdown">
                                                    <button class="btn btn-xs btn-secondary dropdown-toggle">"Actions"</button>
                                                    <div class="dropdown-menu">
                                                        <a href={editor_link} class="dropdown-item">"Edit Dashboard"</a>
                                                        <button
                                                            class="dropdown-item"
                                                            on:click=move |_| open_edit_modal(dashboard_clone1.clone())
                                                        >
                                                            "Edit Settings"
                                                        </button>
                                                        <button
                                                            class="dropdown-item"
                                                            on:click=move |_| clone_dashboard_action.dispatch(dashboard_clone2.clone())
                                                        >
                                                            "Clone"
                                                        </button>
                                                        <button
                                                            class="dropdown-item"
                                                            on:click=move |_| export_dashboard_action.dispatch(dashboard_clone3.clone())
                                                        >
                                                            "Export"
                                                        </button>
                                                        <hr class="dropdown-divider"/>
                                                        <button
                                                            class="dropdown-item text-danger"
                                                            on:click=move |_| {
                                                                if web_sys::window()
                                                                    .unwrap()
                                                                    .confirm_with_message(&format!("Delete dashboard '{}'? This action cannot be undone.", dashboard_clone4.name))
                                                                    .unwrap_or(false)
                                                                {
                                                                    delete_dashboard_action.dispatch(dashboard_clone4.id.clone());
                                                                }
                                                            }
                                                        >
                                                            "Delete"
                                                        </button>
                                                    </div>
                                                </div>
                                            </div>
                                        </div>

                                        <div class="card-content">
                                            {dash_description.map(|desc| view! {
                                                <p class="dashboard-description">{desc}</p>
                                            })}

                                            <div class="dashboard-meta">
                                                <div class="meta-row">
                                                    <span class="meta-label">"Layout:"</span>
                                                    <span class="meta-value">{dash_layout}</span>
                                                </div>
                                                <div class="meta-row">
                                                    <span class="meta-label">"Widgets:"</span>
                                                    <span class="meta-value">{dash_widget_count}" widgets"</span>
                                                </div>
                                                <div class="meta-row">
                                                    <span class="meta-label">"Refresh:"</span>
                                                    <span class="meta-value">{dash_refresh}"s"</span>
                                                </div>
                                                <div class="meta-row">
                                                    <span class="meta-label">"Visibility:"</span>
                                                    <span class={format!("meta-value {}", visibility_class)}>
                                                        {if dash_public { "Public" } else { "Private" }}
                                                    </span>
                                                </div>
                                            </div>

                                            {if !dash_tags.is_empty() {
                                                view! {
                                                    <div class="dashboard-tags">
                                                        {dash_tags.into_iter().map(|tag| view! {
                                                            <span class="tag">{tag}</span>
                                                        }).collect::<Vec<_>>()}
                                                    </div>
                                                }.into_view()
                                            } else {
                                                view! { <div></div> }.into_view()
                                            }}
                                        </div>

                                        <div class="card-footer">
                                            <div class="dashboard-stats">
                                                <span class="stat">
                                                    "Created: "{dash_created}
                                                </span>
                                                <span class="stat">
                                                    "Updated: "{dash_updated}
                                                </span>
                                            </div>
                                            <a
                                                href={view_link}
                                                class="btn btn-sm btn-primary"
                                            >
                                                "View Dashboard"
                                            </a>
                                        </div>
                                    </div>
                                }
                            }).collect::<Vec<_>>()}
                        </div>
                    }.into_view()
                }
            }}

            // Create Dashboard Modal
            {move || if show_create_modal.get() {
                view! {
                    <div class="modal-overlay">
                        <div class="modal modal-lg">
                            <div class="modal-header">
                                <h3>"Create New Dashboard"</h3>
                                <button
                                    class="modal-close"
                                    on:click=move |_| {
                                        set_show_create_modal.set(false);
                                        clear_form();
                                    }
                                >
                                    "x"
                                </button>
                            </div>
                            <div class="modal-body">
                                <form on:submit=move |ev| {
                                    ev.prevent_default();
                                    if !dashboard_name.get().is_empty() {
                                        create_dashboard_action.dispatch(());
                                    }
                                }>
                                    <div class="form-grid">
                                        <div class="form-group">
                                            <label>"Dashboard Name *"</label>
                                            <input
                                                type="text"
                                                prop:value=dashboard_name
                                                on:input=move |ev| set_dashboard_name.set(event_target_value(&ev))
                                                placeholder="Enter dashboard name"
                                                required
                                                class="form-input"
                                            />
                                        </div>

                                        <div class="form-group">
                                            <label>"Category"</label>
                                            <select
                                                prop:value=dashboard_category
                                                on:change=move |ev| set_dashboard_category.set(event_target_value(&ev))
                                                class="form-select"
                                            >
                                                <option value="monitoring">"Monitoring"</option>
                                                <option value="infrastructure">"Infrastructure"</option>
                                                <option value="application">"Application"</option>
                                                <option value="security">"Security"</option>
                                                <option value="business">"Business"</option>
                                                <option value="custom">"Custom"</option>
                                            </select>
                                        </div>

                                        <div class="form-group full-width">
                                            <label>"Description"</label>
                                            <textarea
                                                prop:value=dashboard_description
                                                on:input=move |ev| set_dashboard_description.set(event_target_value(&ev))
                                                placeholder="Describe your dashboard"
                                                rows=3
                                                class="form-textarea"
                                            ></textarea>
                                        </div>

                                        <div class="form-group">
                                            <label>"Layout Type"</label>
                                            <select
                                                prop:value=dashboard_layout
                                                on:change=move |ev| set_dashboard_layout.set(event_target_value(&ev))
                                                class="form-select"
                                            >
                                                <option value="grid">"Grid Layout"</option>
                                                <option value="masonry">"Masonry Layout"</option>
                                                <option value="responsive">"Responsive Layout"</option>
                                                <option value="fixed">"Fixed Layout"</option>
                                            </select>
                                        </div>

                                        <div class="form-group">
                                            <label>"Refresh Interval (seconds)"</label>
                                            <input
                                                type="number"
                                                min=5
                                                max=3600
                                                prop:value=move || dashboard_refresh_interval.get().to_string()
                                                on:input=move |ev| {
                                                    if let Ok(interval) = event_target_value(&ev).parse::<u32>() {
                                                        set_dashboard_refresh_interval.set(interval);
                                                    }
                                                }
                                                class="form-input"
                                            />
                                        </div>

                                        <div class="form-group full-width">
                                            <label>"Tags (comma-separated)"</label>
                                            <input
                                                type="text"
                                                prop:value=dashboard_tags
                                                on:input=move |ev| set_dashboard_tags.set(event_target_value(&ev))
                                                placeholder="monitoring, performance, alerts"
                                                class="form-input"
                                            />
                                        </div>

                                        <div class="form-group full-width">
                                            <label class="checkbox-label">
                                                <input
                                                    type="checkbox"
                                                    prop:checked=dashboard_public
                                                    on:input=move |ev| set_dashboard_public.set(event_target_checked(&ev))
                                                />
                                                " Make this dashboard public (visible to all users)"
                                            </label>
                                        </div>
                                    </div>

                                    <div class="modal-actions">
                                        <button
                                            type="button"
                                            class="btn btn-secondary"
                                            on:click=move |_| {
                                                set_show_create_modal.set(false);
                                                clear_form();
                                            }
                                        >
                                            "Cancel"
                                        </button>
                                        <button
                                            type="submit"
                                            class="btn btn-primary"
                                            disabled=move || dashboard_name.get().is_empty() || loading.get()
                                        >
                                            "Create Dashboard"
                                        </button>
                                    </div>
                                </form>
                            </div>
                        </div>
                    </div>
                }.into_view()
            } else {
                view! { <div></div> }.into_view()
            }}

            // Edit Dashboard Modal (similar structure)
            {move || if show_edit_modal.get() {
                view! {
                    <div class="modal-overlay">
                        <div class="modal modal-lg">
                            <div class="modal-header">
                                <h3>"Edit Dashboard Settings"</h3>
                                <button
                                    class="modal-close"
                                    on:click=move |_| {
                                        set_show_edit_modal.set(false);
                                        clear_form();
                                    }
                                >
                                    "x"
                                </button>
                            </div>
                            <div class="modal-body">
                                <form on:submit=move |ev| {
                                    ev.prevent_default();
                                    if !dashboard_name.get().is_empty() {
                                        update_dashboard_action.dispatch(());
                                    }
                                }>
                                    // Same form structure as create modal
                                    <div class="form-grid">
                                        <div class="form-group">
                                            <label>"Dashboard Name *"</label>
                                            <input
                                                type="text"
                                                prop:value=dashboard_name
                                                on:input=move |ev| set_dashboard_name.set(event_target_value(&ev))
                                                required
                                                class="form-input"
                                            />
                                        </div>

                                        <div class="form-group">
                                            <label>"Category"</label>
                                            <select
                                                prop:value=dashboard_category
                                                on:change=move |ev| set_dashboard_category.set(event_target_value(&ev))
                                                class="form-select"
                                            >
                                                <option value="monitoring">"Monitoring"</option>
                                                <option value="infrastructure">"Infrastructure"</option>
                                                <option value="application">"Application"</option>
                                                <option value="security">"Security"</option>
                                                <option value="business">"Business"</option>
                                                <option value="custom">"Custom"</option>
                                            </select>
                                        </div>

                                        <div class="form-group full-width">
                                            <label>"Description"</label>
                                            <textarea
                                                prop:value=dashboard_description
                                                on:input=move |ev| set_dashboard_description.set(event_target_value(&ev))
                                                rows=3
                                                class="form-textarea"
                                            ></textarea>
                                        </div>

                                        <div class="form-group">
                                            <label>"Refresh Interval (seconds)"</label>
                                            <input
                                                type="number"
                                                min=5
                                                max=3600
                                                prop:value=move || dashboard_refresh_interval.get().to_string()
                                                on:input=move |ev| {
                                                    if let Ok(interval) = event_target_value(&ev).parse::<u32>() {
                                                        set_dashboard_refresh_interval.set(interval);
                                                    }
                                                }
                                                class="form-input"
                                            />
                                        </div>

                                        <div class="form-group full-width">
                                            <label>"Tags (comma-separated)"</label>
                                            <input
                                                type="text"
                                                prop:value=dashboard_tags
                                                on:input=move |ev| set_dashboard_tags.set(event_target_value(&ev))
                                                class="form-input"
                                            />
                                        </div>

                                        <div class="form-group full-width">
                                            <label class="checkbox-label">
                                                <input
                                                    type="checkbox"
                                                    prop:checked=dashboard_public
                                                    on:input=move |ev| set_dashboard_public.set(event_target_checked(&ev))
                                                />
                                                " Make this dashboard public"
                                            </label>
                                        </div>
                                    </div>

                                    <div class="modal-actions">
                                        <button
                                            type="button"
                                            class="btn btn-secondary"
                                            on:click=move |_| {
                                                set_show_edit_modal.set(false);
                                                clear_form();
                                            }
                                        >
                                            "Cancel"
                                        </button>
                                        <button
                                            type="submit"
                                            class="btn btn-primary"
                                            disabled=move || dashboard_name.get().is_empty() || loading.get()
                                        >
                                            "Update Dashboard"
                                        </button>
                                    </div>
                                </form>
                            </div>
                        </div>
                    </div>
                }.into_view()
            } else {
                view! { <div></div> }.into_view()
            }}
        </div>
    }
}