use leptos::*;
use leptos_router::*;
use crate::api::*;

#[component]
pub fn DashboardEditorPage() -> impl IntoView {
    let params = use_params_map();
    let dashboard_id = move || params.with(|p| p.get("id").cloned().unwrap_or_default());

    let (dashboard, set_dashboard) = create_signal(None::<CustomDashboard>);
    let (widgets, set_widgets) = create_signal(Vec::<DashboardWidget>::new());
    let (available_metrics, set_available_metrics) = create_signal(Vec::<MetricDefinition>::new());
    let (loading, set_loading) = create_signal(false);
    let (error_message, set_error_message) = create_signal(None::<String>);
    let (success_message, set_success_message) = create_signal(None::<String>);

    // Editor state
    let (edit_mode, set_edit_mode) = create_signal(true);
    let (show_widget_library, set_show_widget_library) = create_signal(false);
    let (show_add_widget_modal, set_show_add_widget_modal) = create_signal(false);
    let (selected_widget, set_selected_widget) = create_signal(None::<DashboardWidget>);
    let (drag_widget, set_drag_widget) = create_signal(None::<String>);
    let (preview_mode, set_preview_mode) = create_signal(false);

    // Widget creation form state
    let (widget_title, set_widget_title) = create_signal(String::new());
    let (widget_type, set_widget_type) = create_signal("line_chart".to_string());
    let (widget_metric, set_widget_metric) = create_signal(String::new());
    let (widget_width, set_widget_width) = create_signal(6);
    let (widget_height, set_widget_height) = create_signal(4);
    let (widget_position_x, set_widget_position_x) = create_signal(0);
    let (widget_position_y, set_widget_position_y) = create_signal(0);

    // Helper function to clear widget form (defined early so actions can use it)
    let clear_widget_form = move || {
        set_widget_title.set(String::new());
        set_widget_type.set("line_chart".to_string());
        set_widget_metric.set(String::new());
        set_widget_width.set(6);
        set_widget_height.set(4);
        set_widget_position_x.set(0);
        set_widget_position_y.set(0);
    };

    // Load dashboard and widgets
    let load_dashboard = create_action(move |_: &()| async move {
        let id = dashboard_id();
        if id.is_empty() {
            return;
        }

        set_loading.set(true);
        set_error_message.set(None);

        // Load dashboard details
        match get_custom_dashboard(&id).await {
            Ok(dashboard_data) => set_dashboard.set(Some(dashboard_data)),
            Err(e) => set_error_message.set(Some(format!("Failed to load dashboard: {}", e))),
        }

        // Load dashboard widgets
        match get_dashboard_widgets(&id).await {
            Ok(widget_list) => set_widgets.set(widget_list),
            Err(e) => set_error_message.set(Some(format!("Failed to load widgets: {}", e))),
        }

        set_loading.set(false);
    });

    // Load available metrics
    let load_metrics = create_action(move |_: &()| async move {
        match get_available_metrics().await {
            Ok(metrics) => set_available_metrics.set(metrics),
            Err(e) => set_error_message.set(Some(format!("Failed to load metrics: {}", e))),
        }
    });

    // Add widget action
    let add_widget_action = create_action(move |_: &()| async move {
        let id = dashboard_id();
        if id.is_empty() || widget_title.get().is_empty() || widget_metric.get().is_empty() {
            return;
        }

        set_loading.set(true);
        set_error_message.set(None);

        let request = CreateWidgetRequest {
            title: widget_title.get(),
            widget_type: widget_type.get(),
            metric: widget_metric.get(),
            width: widget_width.get(),
            height: widget_height.get(),
            position_x: widget_position_x.get(),
            position_y: widget_position_y.get(),
            config: WidgetConfig::default(),
        };

        match add_dashboard_widget(&id, request).await {
            Ok(widget) => {
                set_success_message.set(Some(format!("Widget '{}' added successfully", widget.title)));
                set_show_add_widget_modal.set(false);
                clear_widget_form();
                load_dashboard.dispatch(());
            }
            Err(e) => set_error_message.set(Some(format!("Failed to add widget: {}", e))),
        }

        set_loading.set(false);
    });

    // Update widget position
    let update_widget_position = create_action(move |(widget_id, x, y): &(String, u32, u32)| {
        let widget_id = widget_id.clone();
        let x = *x;
        let y = *y;
        async move {
            let dashboard_id = dashboard_id();
            if dashboard_id.is_empty() {
                return;
            }

            match update_widget_position(&dashboard_id, &widget_id, x, y).await {
                Ok(_) => {
                    // Update widget in local state
                    set_widgets.update(|widgets| {
                        if let Some(widget) = widgets.iter_mut().find(|w| w.id == widget_id) {
                            widget.position_x = x;
                            widget.position_y = y;
                        }
                    });
                }
                Err(e) => set_error_message.set(Some(format!("Failed to update widget position: {}", e))),
            }
        }
    });

    // Delete widget action
    let delete_widget_action = create_action(move |widget_id: &String| {
        let widget_id = widget_id.clone();
        async move {
            let dashboard_id = dashboard_id();
            if dashboard_id.is_empty() {
                return;
            }

            set_loading.set(true);
            set_error_message.set(None);

            match remove_dashboard_widget(&dashboard_id, &widget_id).await {
                Ok(_) => {
                    set_success_message.set(Some("Widget removed successfully".to_string()));
                    load_dashboard.dispatch(());
                }
                Err(e) => set_error_message.set(Some(format!("Failed to remove widget: {}", e))),
            }

            set_loading.set(false);
        }
    });

    // Save dashboard layout
    let save_layout = create_action(move |_: &()| async move {
        let dashboard_id = dashboard_id();
        if dashboard_id.is_empty() {
            return;
        }

        set_loading.set(true);
        set_error_message.set(None);

        match save_dashboard_layout(&dashboard_id, widgets.get()).await {
            Ok(_) => {
                set_success_message.set(Some("Dashboard layout saved successfully".to_string()));
                set_edit_mode.set(false);
            }
            Err(e) => set_error_message.set(Some(format!("Failed to save layout: {}", e))),
        }

        set_loading.set(false);
    });

    // Helper functions
    let get_widget_component = move |widget: DashboardWidget| {
        match widget.widget_type.as_str() {
            "line_chart" => view! { <LineChartWidget widget=widget/> }.into_view(),
            "bar_chart" => view! { <BarChartWidget widget=widget/> }.into_view(),
            "pie_chart" => view! { <PieChartWidget widget=widget/> }.into_view(),
            "gauge" => view! { <GaugeWidget widget=widget/> }.into_view(),
            "table" => view! { <TableWidget widget=widget/> }.into_view(),
            "metric" => view! { <MetricWidget widget=widget/> }.into_view(),
            "heatmap" => view! { <HeatmapWidget widget=widget/> }.into_view(),
            _ => view! { <DefaultWidget widget=widget/> }.into_view(),
        }
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
        load_dashboard.dispatch(());
        load_metrics.dispatch(());
    });

    view! {
        <div class="dashboard-editor">
            // Editor header
            <div class="editor-header">
                <div class="header-left">
                    {move || if let Some(dash) = dashboard.get() {
                        view! {
                            <div class="dashboard-info">
                                <h1>{&dash.name}</h1>
                                <span class="dashboard-category">{&dash.category}</span>
                            </div>
                        }.into_view()
                    } else {
                        view! {
                            <div class="loading-header">"Loading dashboard..."</div>
                        }.into_view()
                    }}
                </div>

                <div class="header-actions">
                    <button
                        class="btn btn-secondary"
                        on:click=move |_| set_preview_mode.update(|p| *p = !*p)
                    >
                        {move || if preview_mode.get() { "Edit Mode" } else { "Preview" }}
                    </button>

                    <button
                        class="btn btn-secondary"
                        on:click=move |_| set_show_widget_library.set(true)
                        disabled=preview_mode
                    >
                        "Widget Library"
                    </button>

                    <button
                        class="btn btn-primary"
                        on:click=move |_| set_show_add_widget_modal.set(true)
                        disabled=preview_mode
                    >
                        "Add Widget"
                    </button>

                    {move || if edit_mode.get() {
                        view! {
                            <button
                                class="btn btn-success"
                                on:click=move |_| save_layout.dispatch(())
                                disabled=loading
                            >
                                "Save Layout"
                            </button>
                        }.into_view()
                    } else {
                        view! {
                            <button
                                class="btn btn-secondary"
                                on:click=move |_| set_edit_mode.set(true)
                            >
                                "Edit Layout"
                            </button>
                        }.into_view()
                    }}

                    <a href="/dashboard/builder" class="btn btn-outline">"Back to Builder"</a>
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

            // Dashboard canvas
            <div class={format!("dashboard-canvas {}", if edit_mode.get() && !preview_mode.get() { "edit-mode" } else { "view-mode" })}>
                {move || if loading.get() && widgets.get().is_empty() {
                    view! { <div class="loading">"Loading dashboard..."</div> }.into_view()
                } else if widgets.get().is_empty() {
                    view! {
                        <div class="empty-canvas">
                            <div class="empty-icon">"📊"</div>
                            <h3>"Empty Dashboard"</h3>
                            <p>"Add your first widget to get started"</p>
                            <button
                                class="btn btn-primary"
                                on:click=move |_| set_show_add_widget_modal.set(true)
                            >
                                "Add Widget"
                            </button>
                        </div>
                    }.into_view()
                } else {
                    view! {
                        <div class="widgets-grid">
                            {widgets.get().into_iter().map(|widget| {
                                let widget_clone1 = widget.clone();
                                let widget_clone2 = widget.clone();
                                let widget_clone3 = widget.clone();
                                let widget_id = widget.id.clone();
                                let widget_width = widget.width;
                                let widget_height = widget.height;
                                let widget_pos_x = widget.position_x;
                                let widget_pos_y = widget.position_y;

                                view! {
                                    <div
                                        class={format!("widget-container {}", if edit_mode.get() && !preview_mode.get() { "editable" } else { "" })}
                                        style=format!("grid-column: span {}; grid-row: span {};
                                                     transform: translate({}px, {}px);",
                                                     widget_width, widget_height,
                                                     widget_pos_x * 20, widget_pos_y * 20)
                                        draggable=edit_mode.get() && !preview_mode.get()
                                        on:dragstart=move |_| {
                                            set_drag_widget.set(Some(widget_clone1.id.clone()));
                                        }
                                        on:dragend=move |_| {
                                            set_drag_widget.set(None);
                                        }
                                    >
                                        {
                                            let widget_for_edit = widget_clone2.clone();
                                            let widget_for_delete = widget_clone3.clone();
                                            move || if edit_mode.get() && !preview_mode.get() {
                                                let widget_edit_inner = widget_for_edit.clone();
                                                let widget_delete_inner = widget_for_delete.clone();
                                                view! {
                                                    <div class="widget-controls">
                                                        <button
                                                            class="widget-control edit-btn"
                                                            on:click=move |_| set_selected_widget.set(Some(widget_edit_inner.clone()))
                                                            title="Edit Widget"
                                                        >
                                                            "✏️"
                                                        </button>
                                                        <button
                                                            class="widget-control delete-btn"
                                                            on:click=move |_| {
                                                                if web_sys::window()
                                                                    .unwrap()
                                                                    .confirm_with_message(&format!("Delete widget '{}'?", widget_delete_inner.title))
                                                                    .unwrap_or(false)
                                                                {
                                                                    delete_widget_action.dispatch(widget_delete_inner.id.clone());
                                                                }
                                                            }
                                                            title="Delete Widget"
                                                        >
                                                            "🗑️"
                                                        </button>
                                                        <div class="drag-handle" title="Drag to move">
                                                            "⋮⋮"
                                                        </div>
                                                    </div>
                                                }.into_view()
                                            } else {
                                                view! { <div></div> }.into_view()
                                            }
                                        }

                                        <div class="widget-content">
                                            {get_widget_component(widget)}
                                        </div>
                                    </div>
                                }
                            }).collect::<Vec<_>>()}
                        </div>
                    }.into_view()
                }}
            </div>

            // Add Widget Modal
            {move || if show_add_widget_modal.get() {
                view! {
                    <div class="modal-overlay">
                        <div class="modal modal-lg">
                            <div class="modal-header">
                                <h3>"Add New Widget"</h3>
                                <button
                                    class="modal-close"
                                    on:click=move |_| {
                                        set_show_add_widget_modal.set(false);
                                        clear_widget_form();
                                    }
                                >
                                    "x"
                                </button>
                            </div>
                            <div class="modal-body">
                                <form on:submit=move |ev| {
                                    ev.prevent_default();
                                    if !widget_title.get().is_empty() && !widget_metric.get().is_empty() {
                                        add_widget_action.dispatch(());
                                    }
                                }>
                                    <div class="form-grid">
                                        <div class="form-group">
                                            <label>"Widget Title *"</label>
                                            <input
                                                type="text"
                                                prop:value=widget_title
                                                on:input=move |ev| set_widget_title.set(event_target_value(&ev))
                                                placeholder="Enter widget title"
                                                required
                                                class="form-input"
                                            />
                                        </div>

                                        <div class="form-group">
                                            <label>"Widget Type"</label>
                                            <select
                                                prop:value=widget_type
                                                on:change=move |ev| set_widget_type.set(event_target_value(&ev))
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
                                            <label>"Metric *"</label>
                                            <select
                                                prop:value=widget_metric
                                                on:change=move |ev| set_widget_metric.set(event_target_value(&ev))
                                                required
                                                class="form-select"
                                            >
                                                <option value="">"Select a metric"</option>
                                                {available_metrics.get().into_iter().map(|metric| {
                                                    let metric_name = metric.name.clone();
                                                    let metric_name_val = metric.name.clone();
                                                    let metric_desc = metric.description.clone();
                                                    view! {
                                                        <option value={metric_name_val}>
                                                            {metric_name}" - "{metric_desc}
                                                        </option>
                                                    }
                                                }).collect::<Vec<_>>()}
                                            </select>
                                        </div>

                                        <div class="form-group">
                                            <label>"Width (grid columns)"</label>
                                            <input
                                                type="number"
                                                min=1
                                                max=12
                                                prop:value=move || widget_width.get().to_string()
                                                on:input=move |ev| {
                                                    if let Ok(width) = event_target_value(&ev).parse::<u32>() {
                                                        set_widget_width.set(width);
                                                    }
                                                }
                                                class="form-input"
                                            />
                                        </div>

                                        <div class="form-group">
                                            <label>"Height (grid rows)"</label>
                                            <input
                                                type="number"
                                                min=1
                                                max=20
                                                prop:value=move || widget_height.get().to_string()
                                                on:input=move |ev| {
                                                    if let Ok(height) = event_target_value(&ev).parse::<u32>() {
                                                        set_widget_height.set(height);
                                                    }
                                                }
                                                class="form-input"
                                            />
                                        </div>

                                        <div class="form-group">
                                            <label>"Position X"</label>
                                            <input
                                                type="number"
                                                min=0
                                                prop:value=move || widget_position_x.get().to_string()
                                                on:input=move |ev| {
                                                    if let Ok(x) = event_target_value(&ev).parse::<u32>() {
                                                        set_widget_position_x.set(x);
                                                    }
                                                }
                                                class="form-input"
                                            />
                                        </div>

                                        <div class="form-group">
                                            <label>"Position Y"</label>
                                            <input
                                                type="number"
                                                min=0
                                                prop:value=move || widget_position_y.get().to_string()
                                                on:input=move |ev| {
                                                    if let Ok(y) = event_target_value(&ev).parse::<u32>() {
                                                        set_widget_position_y.set(y);
                                                    }
                                                }
                                                class="form-input"
                                            />
                                        </div>
                                    </div>

                                    <div class="modal-actions">
                                        <button
                                            type="button"
                                            class="btn btn-secondary"
                                            on:click=move |_| {
                                                set_show_add_widget_modal.set(false);
                                                clear_widget_form();
                                            }
                                        >
                                            "Cancel"
                                        </button>
                                        <button
                                            type="submit"
                                            class="btn btn-primary"
                                            disabled=move || widget_title.get().is_empty() || widget_metric.get().is_empty() || loading.get()
                                        >
                                            "Add Widget"
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

// Widget Components
#[component]
fn LineChartWidget(widget: DashboardWidget) -> impl IntoView {
    view! {
        <div class="widget widget-line-chart">
            <div class="widget-header">
                <h4>{&widget.title}</h4>
            </div>
            <div class="widget-body">
                <div class="chart-container" id={format!("chart-{}", widget.id)}>
                    // Chart will be rendered here by JavaScript
                    <div class="chart-placeholder">"Line Chart - "{&widget.metric}</div>
                </div>
            </div>
        </div>
    }
}

#[component]
fn BarChartWidget(widget: DashboardWidget) -> impl IntoView {
    view! {
        <div class="widget widget-bar-chart">
            <div class="widget-header">
                <h4>{&widget.title}</h4>
            </div>
            <div class="widget-body">
                <div class="chart-container" id={format!("chart-{}", widget.id)}>
                    <div class="chart-placeholder">"Bar Chart - "{&widget.metric}</div>
                </div>
            </div>
        </div>
    }
}

#[component]
fn PieChartWidget(widget: DashboardWidget) -> impl IntoView {
    view! {
        <div class="widget widget-pie-chart">
            <div class="widget-header">
                <h4>{&widget.title}</h4>
            </div>
            <div class="widget-body">
                <div class="chart-container" id={format!("chart-{}", widget.id)}>
                    <div class="chart-placeholder">"Pie Chart - "{&widget.metric}</div>
                </div>
            </div>
        </div>
    }
}

#[component]
fn GaugeWidget(widget: DashboardWidget) -> impl IntoView {
    view! {
        <div class="widget widget-gauge">
            <div class="widget-header">
                <h4>{&widget.title}</h4>
            </div>
            <div class="widget-body">
                <div class="gauge-container" id={format!("gauge-{}", widget.id)}>
                    <div class="gauge-placeholder">"Gauge - "{&widget.metric}</div>
                </div>
            </div>
        </div>
    }
}

#[component]
fn TableWidget(widget: DashboardWidget) -> impl IntoView {
    view! {
        <div class="widget widget-table">
            <div class="widget-header">
                <h4>{&widget.title}</h4>
            </div>
            <div class="widget-body">
                <div class="table-container">
                    <div class="table-placeholder">"Table - "{&widget.metric}</div>
                </div>
            </div>
        </div>
    }
}

#[component]
fn MetricWidget(widget: DashboardWidget) -> impl IntoView {
    view! {
        <div class="widget widget-metric">
            <div class="widget-header">
                <h4>{&widget.title}</h4>
            </div>
            <div class="widget-body">
                <div class="metric-display">
                    <div class="metric-value">"--"</div>
                    <div class="metric-label">{&widget.metric}</div>
                </div>
            </div>
        </div>
    }
}

#[component]
fn HeatmapWidget(widget: DashboardWidget) -> impl IntoView {
    view! {
        <div class="widget widget-heatmap">
            <div class="widget-header">
                <h4>{&widget.title}</h4>
            </div>
            <div class="widget-body">
                <div class="heatmap-container" id={format!("heatmap-{}", widget.id)}>
                    <div class="heatmap-placeholder">"Heatmap - "{&widget.metric}</div>
                </div>
            </div>
        </div>
    }
}

#[component]
fn DefaultWidget(widget: DashboardWidget) -> impl IntoView {
    view! {
        <div class="widget widget-default">
            <div class="widget-header">
                <h4>{&widget.title}</h4>
            </div>
            <div class="widget-body">
                <div class="widget-placeholder">
                    <p>"Widget Type: "{&widget.widget_type}</p>
                    <p>"Metric: "{&widget.metric}</p>
                </div>
            </div>
        </div>
    }
}