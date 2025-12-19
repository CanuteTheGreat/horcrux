use leptos::*;
use crate::api::*;

#[component]
pub fn ChartLibraryPage() -> impl IntoView {
    let (chart_templates, set_chart_templates) = create_signal(Vec::<ChartTemplate>::new());
    let (loading, set_loading) = create_signal(false);
    let (error_message, set_error_message) = create_signal(None::<String>);
    let (success_message, set_success_message) = create_signal(None::<String>);
    let (search_query, set_search_query) = create_signal(String::new());
    let (filter_category, set_filter_category) = create_signal("all".to_string());
    let (filter_type, set_filter_type) = create_signal("all".to_string());
    let (selected_template, set_selected_template) = create_signal(None::<ChartTemplate>);
    let (show_preview_modal, set_show_preview_modal) = create_signal(false);
    let (show_create_modal, set_show_create_modal) = create_signal(false);

    // Form state for template creation
    let (template_name, set_template_name) = create_signal(String::new());
    let (template_description, set_template_description) = create_signal(String::new());
    let (template_category, set_template_category) = create_signal("monitoring".to_string());
    let (template_type, set_template_type) = create_signal("line_chart".to_string());
    let (template_config, set_template_config) = create_signal(String::new());

    // Helper function to clear form (defined early so actions can use it)
    let clear_form = move || {
        set_template_name.set(String::new());
        set_template_description.set(String::new());
        set_template_category.set("monitoring".to_string());
        set_template_type.set("line_chart".to_string());
        set_template_config.set(String::new());
    };

    // Load chart templates
    let load_templates = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);

        match get_chart_templates().await {
            Ok(templates) => set_chart_templates.set(templates),
            Err(e) => set_error_message.set(Some(format!("Failed to load chart templates: {}", e))),
        }

        set_loading.set(false);
    });

    // Create template action
    let create_template_action = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);

        let config = match serde_json::from_str::<serde_json::Value>(&template_config.get()) {
            Ok(config) => config,
            Err(_) => {
                set_error_message.set(Some("Invalid JSON configuration".to_string()));
                set_loading.set(false);
                return;
            }
        };

        let request = CreateChartTemplateRequest {
            name: template_name.get(),
            description: if template_description.get().is_empty() {
                None
            } else {
                Some(template_description.get())
            },
            category: template_category.get(),
            chart_type: template_type.get(),
            config,
        };

        match create_chart_template(request).await {
            Ok(template) => {
                set_success_message.set(Some(format!("Template '{}' created successfully", template.name)));
                set_show_create_modal.set(false);
                clear_form();
                load_templates.dispatch(());
            }
            Err(e) => set_error_message.set(Some(format!("Failed to create template: {}", e))),
        }

        set_loading.set(false);
    });

    // Use template action
    let use_template_action = create_action(move |template: &ChartTemplate| {
        let template = template.clone();
        async move {
            set_loading.set(true);
            set_error_message.set(None);

            // This would typically redirect to dashboard editor with template pre-filled
            set_success_message.set(Some(format!("Using template '{}' - redirect to dashboard editor", template.name)));

            set_loading.set(false);
        }
    });

    // Delete template action
    let delete_template_action = create_action(move |template_id: &String| {
        let template_id = template_id.clone();
        async move {
            set_loading.set(true);
            set_error_message.set(None);

            match delete_chart_template(&template_id).await {
                Ok(_) => {
                    set_success_message.set(Some("Template deleted successfully".to_string()));
                    load_templates.dispatch(());
                }
                Err(e) => set_error_message.set(Some(format!("Failed to delete template: {}", e))),
            }

            set_loading.set(false);
        }
    });

    // Export template action
    let export_template_action = create_action(move |template: &ChartTemplate| {
        let template = template.clone();
        async move {
            set_loading.set(true);
            set_error_message.set(None);

            match export_chart_template(&template.id).await {
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
                    element.set_download(&format!("chart-template-{}.json", template.name.replace(" ", "_").to_lowercase()));
                    element.click();

                    web_sys::Url::revoke_object_url(&url).unwrap();
                    set_success_message.set(Some("Template exported successfully".to_string()));
                }
                Err(e) => set_error_message.set(Some(format!("Failed to export template: {}", e))),
            }

            set_loading.set(false);
        }
    });

    // Helper functions
    let filtered_templates = move || {
        let query = search_query.get().to_lowercase();
        let category = filter_category.get();
        let chart_type = filter_type.get();

        chart_templates.get()
            .into_iter()
            .filter(|template| {
                let matches_search = query.is_empty() ||
                    template.name.to_lowercase().contains(&query) ||
                    template.description.as_ref().map_or(false, |d| d.to_lowercase().contains(&query));

                let matches_category = category == "all" || template.category == category;
                let matches_type = chart_type == "all" || template.chart_type == chart_type;

                matches_search && matches_category && matches_type
            })
            .collect::<Vec<_>>()
    };

    let get_category_list = move || -> Vec<String> {
        let mut categories: Vec<String> = chart_templates.get()
            .iter()
            .map(|t| t.category.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        categories.sort();
        categories
    };

    let get_type_list = move || -> Vec<String> {
        let mut types: Vec<String> = chart_templates.get()
            .iter()
            .map(|t| t.chart_type.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        types.sort();
        types
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
        load_templates.dispatch(());
    });

    view! {
        <div class="chart-library-page">
            <div class="page-header">
                <div class="header-title">
                    <h1>"Chart Library"</h1>
                    <p class="header-subtitle">"Browse and manage chart templates for your dashboards"</p>
                </div>
                <div class="header-actions">
                    <button
                        class="btn btn-secondary"
                        on:click=move |_| load_templates.dispatch(())
                        disabled=loading
                    >
                        "Refresh"
                    </button>
                    <button
                        class="btn btn-primary"
                        on:click=move |_| set_show_create_modal.set(true)
                        disabled=loading
                    >
                        "Create Template"
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

            <div class="chart-controls">
                <div class="controls-row">
                    <div class="search-box">
                        <input
                            type="text"
                            prop:value=search_query
                            on:input=move |ev| set_search_query.set(event_target_value(&ev))
                            placeholder="Search templates..."
                            class="search-input"
                        />
                    </div>

                    <div class="filter-controls">
                        <label>"Category:"</label>
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

                        <label>"Type:"</label>
                        <select
                            prop:value=filter_type
                            on:change=move |ev| set_filter_type.set(event_target_value(&ev))
                        >
                            <option value="all">"All Types"</option>
                            {get_type_list().into_iter().map(|chart_type| {
                                let ct_val = chart_type.clone();
                                let ct_display = chart_type.clone();
                                view! {
                                    <option value={ct_val}>{ct_display}</option>
                                }
                            }).collect::<Vec<_>>()}
                        </select>
                    </div>
                </div>
            </div>

            {move || if loading.get() && chart_templates.get().is_empty() {
                view! { <div class="loading">"Loading chart templates..."</div> }.into_view()
            } else {
                let filtered = filtered_templates();
                if filtered.is_empty() {
                    view! {
                        <div class="empty-state">
                            <div class="empty-icon">"📈"</div>
                            <h3>"No templates found"</h3>
                            <p>"Create your first chart template to get started"</p>
                            <button
                                class="btn btn-primary"
                                on:click=move |_| set_show_create_modal.set(true)
                            >
                                "Create Template"
                            </button>
                        </div>
                    }.into_view()
                } else {
                    view! {
                        <div class="templates-grid">
                            {filtered_templates().into_iter().map(|template| {
                                let template_clone1 = template.clone();
                                let template_clone2 = template.clone();
                                let template_clone3 = template.clone();
                                let template_clone4 = template.clone();
                                let chart_icon = get_chart_icon(&template.chart_type);
                                let tmpl_name = template.name.clone();
                                let tmpl_category = template.category.clone();
                                let tmpl_chart_type = template.chart_type.clone();
                                let tmpl_chart_type2 = template.chart_type.clone();
                                let tmpl_description = template.description.clone();
                                let tmpl_created = template.created_at[..10].to_string();
                                let tmpl_usage = template.usage_count;
                                let tmpl_custom = template.custom;

                                view! {
                                    <div class="template-card">
                                        <div class="card-header">
                                            <div class="template-icon">
                                                {chart_icon}
                                            </div>
                                            <div class="template-info">
                                                <h3>{tmpl_name}</h3>
                                                <div class="template-badges">
                                                    <span class="category-badge">{tmpl_category}</span>
                                                    <span class="type-badge">{tmpl_chart_type}</span>
                                                </div>
                                            </div>
                                        </div>

                                        <div class="card-content">
                                            {tmpl_description.map(|desc| view! {
                                                <p class="template-description">{desc}</p>
                                            })}

                                            <div class="template-preview">
                                                <div class="preview-placeholder">
                                                    "Preview - "{tmpl_chart_type2}
                                                </div>
                                            </div>

                                            <div class="template-meta">
                                                <div class="meta-row">
                                                    <span class="meta-label">"Created:"</span>
                                                    <span class="meta-value">{tmpl_created}</span>
                                                </div>
                                                <div class="meta-row">
                                                    <span class="meta-label">"Usage:"</span>
                                                    <span class="meta-value">{tmpl_usage}" times"</span>
                                                </div>
                                            </div>
                                        </div>

                                        <div class="card-actions">
                                            <button
                                                class="btn btn-sm btn-primary"
                                                on:click=move |_| use_template_action.dispatch(template_clone1.clone())
                                            >
                                                "Use Template"
                                            </button>

                                            <button
                                                class="btn btn-sm btn-secondary"
                                                on:click=move |_| {
                                                    set_selected_template.set(Some(template_clone2.clone()));
                                                    set_show_preview_modal.set(true);
                                                }
                                            >
                                                "Preview"
                                            </button>

                                            <div class="dropdown">
                                                <button class="btn btn-sm btn-outline dropdown-toggle">"More"</button>
                                                <div class="dropdown-menu">
                                                    <button
                                                        class="dropdown-item"
                                                        on:click=move |_| export_template_action.dispatch(template_clone3.clone())
                                                    >
                                                        "Export"
                                                    </button>
                                                    {if tmpl_custom {
                                                        view! {
                                                            <>
                                                                <hr class="dropdown-divider"/>
                                                                <button
                                                                    class="dropdown-item text-danger"
                                                                    on:click=move |_| {
                                                                        if web_sys::window()
                                                                            .unwrap()
                                                                            .confirm_with_message(&format!("Delete template '{}'? This action cannot be undone.", template_clone4.name))
                                                                            .unwrap_or(false)
                                                                        {
                                                                            delete_template_action.dispatch(template_clone4.id.clone());
                                                                        }
                                                                    }
                                                                >
                                                                    "Delete"
                                                                </button>
                                                            </>
                                                        }.into_view()
                                                    } else {
                                                        view! { <div></div> }.into_view()
                                                    }}
                                                </div>
                                            </div>
                                        </div>
                                    </div>
                                }
                            }).collect::<Vec<_>>()}
                        </div>
                    }.into_view()
                }
            }}

            // Create Template Modal
            {move || if show_create_modal.get() {
                view! {
                    <div class="modal-overlay">
                        <div class="modal modal-lg">
                            <div class="modal-header">
                                <h3>"Create Chart Template"</h3>
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
                                    if !template_name.get().is_empty() {
                                        create_template_action.dispatch(());
                                    }
                                }>
                                    <div class="form-grid">
                                        <div class="form-group">
                                            <label>"Template Name *"</label>
                                            <input
                                                type="text"
                                                prop:value=template_name
                                                on:input=move |ev| set_template_name.set(event_target_value(&ev))
                                                placeholder="Enter template name"
                                                required
                                                class="form-input"
                                            />
                                        </div>

                                        <div class="form-group">
                                            <label>"Chart Type"</label>
                                            <select
                                                prop:value=template_type
                                                on:change=move |ev| set_template_type.set(event_target_value(&ev))
                                                class="form-select"
                                            >
                                                <option value="line_chart">"Line Chart"</option>
                                                <option value="bar_chart">"Bar Chart"</option>
                                                <option value="pie_chart">"Pie Chart"</option>
                                                <option value="gauge">"Gauge"</option>
                                                <option value="table">"Table"</option>
                                                <option value="metric">"Single Metric"</option>
                                                <option value="heatmap">"Heatmap"</option>
                                            </select>
                                        </div>

                                        <div class="form-group">
                                            <label>"Category"</label>
                                            <select
                                                prop:value=template_category
                                                on:change=move |ev| set_template_category.set(event_target_value(&ev))
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
                                                prop:value=template_description
                                                on:input=move |ev| set_template_description.set(event_target_value(&ev))
                                                placeholder="Describe your template"
                                                rows=3
                                                class="form-textarea"
                                            ></textarea>
                                        </div>

                                        <div class="form-group full-width">
                                            <label>"Configuration (JSON)"</label>
                                            <textarea
                                                prop:value=template_config
                                                on:input=move |ev| set_template_config.set(event_target_value(&ev))
                                                placeholder="{\n  \"colors\": [\"#1f77b4\", \"#ff7f0e\"],\n  \"theme\": \"light\"\n}"
                                                rows=10
                                                class="form-textarea code"
                                            ></textarea>
                                            <small class="form-help">"Enter chart configuration in JSON format"</small>
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
                                            disabled=move || template_name.get().is_empty() || loading.get()
                                        >
                                            "Create Template"
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

            // Preview Modal
            {move || if show_preview_modal.get() {
                if let Some(template) = selected_template.get() {
                    view! {
                        <div class="modal-overlay">
                            <div class="modal modal-xl">
                                <div class="modal-header">
                                    <h3>"Template Preview: "{&template.name}</h3>
                                    <button
                                        class="modal-close"
                                        on:click=move |_| {
                                            set_show_preview_modal.set(false);
                                            set_selected_template.set(None);
                                        }
                                    >
                                        "x"
                                    </button>
                                </div>
                                <div class="modal-body">
                                    <div class="template-preview-large">
                                        <div class="preview-chart">
                                            "Chart Preview - "{&template.chart_type}
                                        </div>
                                        <div class="preview-config">
                                            <h4>"Configuration:"</h4>
                                            <pre class="config-display">
                                                {serde_json::to_string_pretty(&template.config).unwrap_or_else(|_| "Invalid JSON".to_string())}
                                            </pre>
                                        </div>
                                    </div>
                                </div>
                                <div class="modal-actions">
                                    <button
                                        class="btn btn-secondary"
                                        on:click=move |_| {
                                            set_show_preview_modal.set(false);
                                            set_selected_template.set(None);
                                        }
                                    >
                                        "Close"
                                    </button>
                                    <button
                                        class="btn btn-primary"
                                        on:click=move |_| {
                                            use_template_action.dispatch(template.clone());
                                            set_show_preview_modal.set(false);
                                            set_selected_template.set(None);
                                        }
                                    >
                                        "Use Template"
                                    </button>
                                </div>
                            </div>
                        </div>
                    }.into_view()
                } else {
                    view! { <div></div> }.into_view()
                }
            } else {
                view! { <div></div> }.into_view()
            }}
        </div>
    }
}

fn get_chart_icon(chart_type: &str) -> &'static str {
    match chart_type {
        "line_chart" => "📈",
        "bar_chart" => "📊",
        "pie_chart" => "🥧",
        "gauge" => "⏲️",
        "table" => "📋",
        "metric" => "🔢",
        "heatmap" => "🗺️",
        _ => "📊",
    }
}