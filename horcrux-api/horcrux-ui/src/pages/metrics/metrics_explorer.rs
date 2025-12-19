use leptos::*;
use wasm_bindgen::JsCast;
use crate::api::*;

#[component]
pub fn MetricsExplorerPage() -> impl IntoView {
    let (available_metrics, set_available_metrics) = create_signal(Vec::<MetricDefinition>::new());
    let (query_results, set_query_results) = create_signal(Vec::<MetricResult>::new());
    let (loading, set_loading) = create_signal(false);
    let (error_message, set_error_message) = create_signal(None::<String>);
    let (success_message, set_success_message) = create_signal(None::<String>);

    // Query state
    let (current_query, set_current_query) = create_signal(String::new());
    let (time_range, set_time_range) = create_signal("1h".to_string());
    let (refresh_interval, set_refresh_interval) = create_signal(0);
    let (auto_refresh, set_auto_refresh) = create_signal(false);
    let (query_mode, set_query_mode) = create_signal("builder".to_string());

    // Query builder state
    let (selected_metric, set_selected_metric) = create_signal(String::new());
    let (metric_filters, set_metric_filters) = create_signal(std::collections::HashMap::<String, String>::new());
    let (aggregation_function, set_aggregation_function) = create_signal("avg".to_string());
    let (group_by_labels, set_group_by_labels) = create_signal(Vec::<String>::new());

    // UI state
    let (show_query_history, set_show_query_history) = create_signal(false);
    let (show_save_dialog, set_show_save_dialog) = create_signal(false);
    let (chart_type, set_chart_type) = create_signal("line".to_string());
    let (show_raw_data, set_show_raw_data) = create_signal(false);

    // Build PromQL query from builder components - defined first to be available
    let build_promql_query = move || -> String {
        if selected_metric.get().is_empty() {
            return String::new();
        }

        let mut query = selected_metric.get();

        // Add filters
        if !metric_filters.get().is_empty() {
            let filters: Vec<String> = metric_filters.get()
                .iter()
                .map(|(k, v)| format!("{}=\"{}\"", k, v))
                .collect();
            query = format!("{}{{ {} }}", query, filters.join(", "));
        }

        // Add aggregation
        if aggregation_function.get() != "none" {
            if group_by_labels.get().is_empty() {
                query = format!("{}({})", aggregation_function.get(), query);
            } else {
                query = format!("{}({}) by ({})",
                    aggregation_function.get(),
                    query,
                    group_by_labels.get().join(", "));
            }
        }

        query
    };

    // Parse time range string to start/end times - defined before use
    let parse_time_range = move |range: &str| -> (Option<chrono::DateTime<chrono::Utc>>, Option<chrono::DateTime<chrono::Utc>>) {
        let end_time = chrono::Utc::now();
        let start_time = match range {
            "5m" => end_time - chrono::Duration::minutes(5),
            "15m" => end_time - chrono::Duration::minutes(15),
            "30m" => end_time - chrono::Duration::minutes(30),
            "1h" => end_time - chrono::Duration::hours(1),
            "3h" => end_time - chrono::Duration::hours(3),
            "6h" => end_time - chrono::Duration::hours(6),
            "12h" => end_time - chrono::Duration::hours(12),
            "1d" => end_time - chrono::Duration::days(1),
            "3d" => end_time - chrono::Duration::days(3),
            "7d" => end_time - chrono::Duration::days(7),
            _ => end_time - chrono::Duration::hours(1),
        };
        (Some(start_time), Some(end_time))
    };

    // Load available metrics
    let load_metrics = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);

        match get_available_metrics().await {
            Ok(metrics) => set_available_metrics.set(metrics),
            Err(e) => set_error_message.set(Some(format!("Failed to load metrics: {}", e))),
        }

        set_loading.set(false);
    });

    // Execute query
    let execute_query = create_action(move |_: &()| async move {
        let query = if query_mode.get() == "builder" {
            build_promql_query()
        } else {
            current_query.get()
        };

        if query.is_empty() {
            set_error_message.set(Some("Please enter a query".to_string()));
            return;
        }

        set_loading.set(true);
        set_error_message.set(None);

        // Parse time range
        let (start_time, end_time) = parse_time_range(&time_range.get());

        let query_request = MetricQuery {
            metric: query.clone(),
            labels: std::collections::HashMap::new(),
            start_time,
            end_time,
            step: 60, // 1 minute step
        };

        match query_metrics(query_request).await {
            Ok(results) => {
                set_query_results.set(vec![results]);
                // Query history save removed - save_query_history not available in API
            }
            Err(e) => set_error_message.set(Some(format!("Query failed: {}", e))),
        }

        set_loading.set(false);
    });

    // Helper functions
    let add_filter = move |key: String, value: String| {
        set_metric_filters.update(|filters| {
            filters.insert(key, value);
        });
    };

    let remove_filter = move |key: String| {
        set_metric_filters.update(|filters| {
            filters.remove(&key);
        });
    };

    let add_group_by = move |label: String| {
        if !label.is_empty() && !group_by_labels.get().contains(&label) {
            set_group_by_labels.update(|labels| {
                labels.push(label);
            });
        }
    };

    let remove_group_by = move |label: String| {
        set_group_by_labels.update(|labels| {
            labels.retain(|l| l != &label);
        });
    };

    // Auto-refresh setup
    create_effect(move |_| {
        if auto_refresh.get() && refresh_interval.get() > 0 {
            use wasm_bindgen::prelude::*;
            use wasm_bindgen::JsCast;

            let closure = Closure::wrap(Box::new(move || {
                if auto_refresh.get() {
                    execute_query.dispatch(());
                }
            }) as Box<dyn Fn()>);

            web_sys::window()
                .unwrap()
                .set_interval_with_callback_and_timeout_and_arguments_0(
                    closure.as_ref().unchecked_ref(),
                    (refresh_interval.get() * 1000) as i32,
                )
                .unwrap();

            closure.forget();
        }
    });

    // Update current query when builder changes
    create_effect(move |_| {
        if query_mode.get() == "builder" {
            set_current_query.set(build_promql_query());
        }
    });

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
        load_metrics.dispatch(());
    });

    view! {
        <div class="metrics-explorer-page">
            <div class="page-header">
                <div class="header-title">
                    <h1>"Metrics Explorer"</h1>
                    <p class="header-subtitle">"Query and visualize metrics with advanced PromQL support"</p>
                </div>
                <div class="header-actions">
                    <button
                        class="btn btn-secondary"
                        on:click=move |_| load_metrics.dispatch(())
                        disabled=loading
                    >
                        "Refresh Metrics"
                    </button>
                    <button
                        class="btn btn-outline"
                        on:click=move |_| set_show_query_history.update(|h| *h = !*h)
                    >
                        "Query History"
                    </button>
                    <button
                        class="btn btn-primary"
                        on:click=move |_| execute_query.dispatch(())
                        disabled=loading.get() || current_query.get().is_empty()
                    >
                        "Execute Query"
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

            <div class="metrics-query-panel">
                <div class="query-controls">
                    <div class="query-mode-selector">
                        <label>"Query Mode:"</label>
                        <div class="mode-toggle">
                            <button
                                class={format!("btn btn-xs {}", if query_mode.get() == "builder" { "btn-primary" } else { "btn-secondary" })}
                                on:click=move |_| set_query_mode.set("builder".to_string())
                            >
                                "Query Builder"
                            </button>
                            <button
                                class={format!("btn btn-xs {}", if query_mode.get() == "raw" { "btn-primary" } else { "btn-secondary" })}
                                on:click=move |_| set_query_mode.set("raw".to_string())
                            >
                                "Raw PromQL"
                            </button>
                        </div>
                    </div>

                    <div class="time-controls">
                        <label>"Time Range:"</label>
                        <select
                            prop:value=time_range
                            on:change=move |ev| set_time_range.set(event_target_value(&ev))
                        >
                            <option value="5m">"Last 5 minutes"</option>
                            <option value="15m">"Last 15 minutes"</option>
                            <option value="30m">"Last 30 minutes"</option>
                            <option value="1h">"Last 1 hour"</option>
                            <option value="3h">"Last 3 hours"</option>
                            <option value="6h">"Last 6 hours"</option>
                            <option value="12h">"Last 12 hours"</option>
                            <option value="1d">"Last 1 day"</option>
                            <option value="3d">"Last 3 days"</option>
                            <option value="7d">"Last 7 days"</option>
                        </select>
                    </div>

                    <div class="refresh-controls">
                        <label class="checkbox-label">
                            <input
                                type="checkbox"
                                prop:checked=auto_refresh
                                on:input=move |ev| set_auto_refresh.set(event_target_checked(&ev))
                            />
                            " Auto-refresh"
                        </label>
                        {move || if auto_refresh.get() {
                            view! {
                                <select
                                    prop:value=move || refresh_interval.get().to_string()
                                    on:change=move |ev| {
                                        if let Ok(interval) = event_target_value(&ev).parse::<u32>() {
                                            set_refresh_interval.set(interval);
                                        }
                                    }
                                >
                                    <option value="0">"Select interval"</option>
                                    <option value="5">"5 seconds"</option>
                                    <option value="10">"10 seconds"</option>
                                    <option value="30">"30 seconds"</option>
                                    <option value="60">"1 minute"</option>
                                    <option value="300">"5 minutes"</option>
                                </select>
                            }.into_view()
                        } else {
                            view! { <div></div> }.into_view()
                        }}
                    </div>
                </div>

                {move || if query_mode.get() == "builder" {
                    view! {
                        <div class="query-builder">
                            <div class="builder-section">
                                <h3>"Metric Selection"</h3>
                                <div class="metric-selector">
                                    <label>"Metric:"</label>
                                    <select
                                        prop:value=selected_metric
                                        on:change=move |ev| set_selected_metric.set(event_target_value(&ev))
                                    >
                                        <option value="">"Select a metric"</option>
                                        {available_metrics.get().into_iter().map(|metric| {
                                            let metric_name = metric.name.clone();
                                            let metric_name_val = metric.name.clone();
                                            let metric_description = metric.description.clone();
                                            view! {
                                                <option value={metric_name_val}>
                                                    {metric_name}" - "{metric_description}
                                                </option>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </select>
                                </div>

                                {move || if !selected_metric.get().is_empty() {
                                    if let Some(metric) = available_metrics.get().iter().find(|m| m.name == selected_metric.get()) {
                                        view! {
                                            <div class="metric-info">
                                                <div class="metric-details">
                                                    <span class="metric-type">"Type: "{&metric.metric_type}</span>
                                                    <span class="metric-unit">"Unit: "{metric.unit.clone().unwrap_or_else(|| "N/A".to_string())}</span>
                                                </div>
                                                <div class="available-labels">
                                                    <label>"Available Labels:"</label>
                                                    <div class="labels-list">
                                                        {metric.labels.clone().into_iter().map(|label| view! {
                                                            <span class="label-tag">{label}</span>
                                                        }).collect::<Vec<_>>()}
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

                            <div class="builder-section">
                                <h3>"Filters"</h3>
                                <div class="filters-container">
                                    {metric_filters.get().into_iter().collect::<Vec<_>>().into_iter().map(|(key, value)| {
                                        let key_clone = key.clone();
                                        let key_display = key.clone();
                                        let value_display = value.clone();
                                        view! {
                                            <div class="filter-item">
                                                <span class="filter-key">{key_display}</span>
                                                <span class="filter-operator">"="</span>
                                                <span class="filter-value">"\"{value_display}\""</span>
                                                <button
                                                    class="btn btn-xs btn-danger"
                                                    on:click=move |_| remove_filter(key_clone.clone())
                                                >
                                                    "x"
                                                </button>
                                            </div>
                                        }
                                    }).collect::<Vec<_>>()}

                                    <div class="add-filter">
                                        <input
                                            type="text"
                                            placeholder="label"
                                            class="filter-key-input"
                                            id="filter-key-input"
                                        />
                                        <span>=</span>
                                        <input
                                            type="text"
                                            placeholder="value"
                                            class="filter-value-input"
                                            id="filter-value-input"
                                        />
                                        <button
                                            class="btn btn-xs btn-secondary"
                                            on:click=move |_| {
                                                if let Some(window) = web_sys::window() {
                                                    if let Some(document) = window.document() {
                                                        if let (Some(key_element), Some(value_element)) = (
                                                            document.get_element_by_id("filter-key-input"),
                                                            document.get_element_by_id("filter-value-input")
                                                        ) {
                                                            let key_input = key_element.dyn_into::<web_sys::HtmlInputElement>().unwrap();
                                                            let value_input = value_element.dyn_into::<web_sys::HtmlInputElement>().unwrap();
                                                            let key = key_input.value();
                                                            let value = value_input.value();
                                                            if !key.is_empty() && !value.is_empty() {
                                                                add_filter(key, value);
                                                                key_input.set_value("");
                                                                value_input.set_value("");
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        >
                                            "Add Filter"
                                        </button>
                                    </div>
                                </div>
                            </div>

                            <div class="builder-section">
                                <h3>"Aggregation"</h3>
                                <div class="aggregation-controls">
                                    <label>"Function:"</label>
                                    <select
                                        prop:value=aggregation_function
                                        on:change=move |ev| set_aggregation_function.set(event_target_value(&ev))
                                    >
                                        <option value="none">"None"</option>
                                        <option value="avg">"avg() - Average"</option>
                                        <option value="sum">"sum() - Sum"</option>
                                        <option value="min">"min() - Minimum"</option>
                                        <option value="max">"max() - Maximum"</option>
                                        <option value="count">"count() - Count"</option>
                                        <option value="rate">"rate() - Rate"</option>
                                        <option value="increase">"increase() - Increase"</option>
                                    </select>

                                    {move || if aggregation_function.get() != "none" {
                                        view! {
                                            <div class="group-by-controls">
                                                <label>"Group By:"</label>
                                                <div class="group-by-tags">
                                                    {group_by_labels.get().into_iter().map(|label| {
                                                        let label_clone = label.clone();
                                                        let label_display = label.clone();
                                                        view! {
                                                            <div class="group-by-tag">
                                                                {label_display}
                                                                <button
                                                                    class="tag-remove"
                                                                    on:click=move |_| remove_group_by(label_clone.clone())
                                                                >
                                                                    "x"
                                                                </button>
                                                            </div>
                                                        }
                                                    }).collect::<Vec<_>>()}
                                                    <input
                                                        type="text"
                                                        placeholder="Add label..."
                                                        class="group-by-input"
                                                        id="group-by-input"
                                                        on:keydown=move |ev| {
                                                            if ev.key() == "Enter" {
                                                                if let Some(window) = web_sys::window() {
                                                                    if let Some(document) = window.document() {
                                                                        if let Some(element) = document.get_element_by_id("group-by-input") {
                                                                            let input = element.dyn_into::<web_sys::HtmlInputElement>().unwrap();
                                                                            let value = input.value();
                                                                            if !value.is_empty() {
                                                                                add_group_by(value);
                                                                                input.set_value("");
                                                                            }
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    />
                                                </div>
                                            </div>
                                        }.into_view()
                                    } else {
                                        view! { <div></div> }.into_view()
                                    }}
                                </div>
                            </div>

                            <div class="generated-query">
                                <label>"Generated PromQL Query:"</label>
                                <div class="query-preview">
                                    <code>{move || build_promql_query()}</code>
                                </div>
                            </div>
                        </div>
                    }.into_view()
                } else {
                    view! {
                        <div class="raw-query-editor">
                            <label>"PromQL Query:"</label>
                            <textarea
                                prop:value=current_query
                                on:input=move |ev| set_current_query.set(event_target_value(&ev))
                                placeholder="Enter your PromQL query here..."
                                rows=6
                                class="query-textarea"
                            ></textarea>
                            <div class="query-help">
                                <span>"Examples: "</span>
                                <button class="query-example" on:click=move |_| set_current_query.set("up".to_string())>"up"</button>
                                <button class="query-example" on:click=move |_| set_current_query.set("rate(cpu_usage_total[5m])".to_string())>"rate(cpu_usage_total[5m])"</button>
                                <button class="query-example" on:click=move |_| set_current_query.set("avg(memory_usage) by (instance)".to_string())>"avg(memory_usage) by (instance)"</button>
                            </div>
                        </div>
                    }.into_view()
                }}
            </div>

            // Query Results
            {move || if !query_results.get().is_empty() || loading.get() {
                view! {
                    <div class="query-results">
                        <div class="results-header">
                            <h3>"Query Results"</h3>
                            <div class="results-controls">
                                <label>"Chart Type:"</label>
                                <select
                                    prop:value=chart_type
                                    on:change=move |ev| set_chart_type.set(event_target_value(&ev))
                                >
                                    <option value="line">"Line Chart"</option>
                                    <option value="area">"Area Chart"</option>
                                    <option value="bar">"Bar Chart"</option>
                                    <option value="table">"Table"</option>
                                </select>
                                <label class="checkbox-label">
                                    <input
                                        type="checkbox"
                                        prop:checked=show_raw_data
                                        on:input=move |ev| set_show_raw_data.set(event_target_checked(&ev))
                                    />
                                    " Show Raw Data"
                                </label>
                            </div>
                        </div>

                        {move || if loading.get() {
                            view! { <div class="loading">"Executing query..."</div> }.into_view()
                        } else if query_results.get().is_empty() {
                            view! { <div class="no-data">"No data returned"</div> }.into_view()
                        } else {
                            view! {
                                <div class="results-content">
                                    {move || {
                                        let results = query_results.get();
                                        if chart_type.get() == "table" || show_raw_data.get() {
                                            view! {
                                                <div class="results-table">
                                                    <table class="data-table">
                                                        <thead>
                                                            <tr>
                                                                <th>"Timestamp"</th>
                                                                <th>"Value"</th>
                                                                <th>"Labels"</th>
                                                            </tr>
                                                        </thead>
                                                        <tbody>
                                                            {results.iter().flat_map(|result| &result.values).cloned().collect::<Vec<_>>().into_iter().map(|point| {
                                                                let timestamp = point.timestamp.format("%Y-%m-%d %H:%M:%S").to_string();
                                                                let value_formatted = format!("{:.4}", point.value);
                                                                let labels = point.labels.clone();
                                                                view! {
                                                                    <tr>
                                                                        <td>{timestamp}</td>
                                                                        <td class="value-cell">{value_formatted}</td>
                                                                        <td class="labels-cell">
                                                                            {labels.into_iter().map(|(key, value)| {
                                                                                let k = key.clone();
                                                                                let v = value.clone();
                                                                                view! {
                                                                                    <span class="label-pair">{k}"="{v}</span>
                                                                                }
                                                                            }).collect::<Vec<_>>()}
                                                                        </td>
                                                                    </tr>
                                                                }
                                                            }).collect::<Vec<_>>()}
                                                        </tbody>
                                                    </table>
                                                </div>
                                            }.into_view()
                                        } else {
                                            view! {
                                                <div class="chart-container">
                                                    <div class="chart-placeholder" id="metrics-chart">
                                                        "Chart will be rendered here - "{chart_type.get()}" chart for "{results.len()}" series"
                                                    </div>
                                                </div>
                                            }.into_view()
                                        }
                                    }}

                                    {move || {
                                        let results = query_results.get();
                                        view! {
                                            <div class="results-summary">
                                                <div class="summary-stats">
                                                    <span class="stat">"Series: "{results.len()}</span>
                                                    <span class="stat">"Data Points: "{results.iter().map(|r| r.values.len()).sum::<usize>()}</span>
                                                    <span class="stat">"Query: "{current_query.get()}</span>
                                                </div>
                                            </div>
                                        }
                                    }}
                                </div>
                            }.into_view()
                        }}
                    </div>
                }.into_view()
            } else {
                view! {
                    <div class="empty-results">
                        <div class="empty-icon">"📊"</div>
                        <h3>"Ready to explore metrics"</h3>
                        <p>"Build a query using the query builder or write raw PromQL to get started"</p>
                    </div>
                }.into_view()
            }}
        </div>
    }
}