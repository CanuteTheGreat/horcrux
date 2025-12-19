use leptos::*;
use crate::api::{MetricDefinition, MetricSeries, get_metrics_catalog, get_metric_samples};

#[component]
pub fn MetricsCatalogPage() -> impl IntoView {
    let (metrics, set_metrics) = create_signal(Vec::<MetricDefinition>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);

    // Filter states
    let (search_term, set_search_term) = create_signal(String::new());
    let (selected_category, set_selected_category) = create_signal("all".to_string());
    let (selected_type, set_selected_type) = create_signal("all".to_string());
    let (selected_source, set_selected_source) = create_signal("all".to_string());
    let (sort_by, set_sort_by) = create_signal("name".to_string());
    let (sort_order, set_sort_order) = create_signal("asc".to_string());

    // Modal states
    let (selected_metric, set_selected_metric) = create_signal(None::<MetricDefinition>);
    let (show_metric_detail, set_show_metric_detail) = create_signal(false);
    let (metric_samples, set_metric_samples) = create_signal(None::<MetricSeries>);
    let (show_add_to_query, set_show_add_to_query) = create_signal(false);

    // Load metrics on mount
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);
            match get_metrics_catalog().await {
                Ok(data) => {
                    set_metrics.set(data);
                    set_error.set(None);
                }
                Err(e) => {
                    set_error.set(Some(format!("Failed to load metrics: {}", e)));
                }
            }
            set_loading.set(false);
        });
    });

    // Filtered and sorted metrics
    let filtered_metrics = create_memo(move |_| {
        let mut filtered: Vec<MetricDefinition> = metrics.get()
            .into_iter()
            .filter(|metric| {
                let search_match = if search_term.get().is_empty() {
                    true
                } else {
                    let term = search_term.get().to_lowercase();
                    metric.name.to_lowercase().contains(&term) ||
                    metric.description.to_lowercase().contains(&term) ||
                    metric.help.to_lowercase().contains(&term)
                };

                let category_match = selected_category.get() == "all" || metric.category == selected_category.get();
                let type_match = selected_type.get() == "all" || metric.metric_type == selected_type.get();
                let source_match = selected_source.get() == "all" || metric.source == selected_source.get();

                search_match && category_match && type_match && source_match
            })
            .collect();

        // Sort metrics
        match sort_by.get().as_str() {
            "name" => filtered.sort_by(|a, b| a.name.cmp(&b.name)),
            "type" => filtered.sort_by(|a, b| a.metric_type.cmp(&b.metric_type)),
            "category" => filtered.sort_by(|a, b| a.category.cmp(&b.category)),
            "cardinality" => filtered.sort_by(|a, b| a.cardinality.cmp(&b.cardinality)),
            "last_scraped" => filtered.sort_by(|a, b| a.last_scraped.cmp(&b.last_scraped)),
            _ => {}
        }

        if sort_order.get() == "desc" {
            filtered.reverse();
        }

        filtered
    });

    // Get unique categories, types, and sources for filters
    let categories = create_memo(move |_| {
        let mut cats: Vec<String> = metrics.get()
            .iter()
            .map(|m| m.category.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        cats.sort();
        cats
    });

    let metric_types = create_memo(move |_| {
        let mut types: Vec<String> = metrics.get()
            .iter()
            .map(|m| m.metric_type.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        types.sort();
        types
    });

    let sources = create_memo(move |_| {
        let mut srcs: Vec<String> = metrics.get()
            .iter()
            .map(|m| m.source.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        srcs.sort();
        srcs
    });

    // Load metric samples when detail modal opens
    let load_metric_samples = move |metric: MetricDefinition| {
        let metric_name = metric.name.clone();
        set_selected_metric.set(Some(metric));
        set_show_metric_detail.set(true);

        spawn_local(async move {
            match get_metric_samples(metric_name, "1h".to_string()).await {
                Ok(samples) => set_metric_samples.set(Some(samples)),
                Err(_) => set_metric_samples.set(None),
            }
        });
    };

    // Add metric to query builder
    let add_metric_to_query = move |metric_name: String| {
        // This would integrate with the PromQL builder or metrics explorer
        set_show_add_to_query.set(false);
        web_sys::window()
            .unwrap()
            .location()
            .set_href(&format!("/metrics/explorer?metric={}", metric_name))
            .unwrap();
    };

    view! {
        <div class="metrics-catalog-page">
            <div class="page-header">
                <h1 class="page-title">Metrics Catalog</h1>
                <p class="page-description">
                    Browse and explore available metrics across your infrastructure
                </p>
            </div>

            // Filters and Search
            <div class="metrics-filters">
                <div class="filter-row">
                    <div class="search-box">
                        <input
                            type="text"
                            placeholder="Search metrics..."
                            class="search-input"
                            prop:value=search_term
                            on:input=move |ev| {
                                set_search_term.set(event_target_value(&ev));
                            }
                        />
                    </div>

                    <div class="filter-selects">
                        <select
                            class="filter-select"
                            prop:value=selected_category
                            on:change=move |ev| {
                                set_selected_category.set(event_target_value(&ev));
                            }
                        >
                            <option value="all">All Categories</option>
                            {move || categories.get().into_iter().map(|cat| view! {
                                <option value={cat.clone()}>{cat}</option>
                            }).collect::<Vec<_>>()}
                        </select>

                        <select
                            class="filter-select"
                            prop:value=selected_type
                            on:change=move |ev| {
                                set_selected_type.set(event_target_value(&ev));
                            }
                        >
                            <option value="all">All Types</option>
                            {move || metric_types.get().into_iter().map(|typ| view! {
                                <option value={typ.clone()}>{typ}</option>
                            }).collect::<Vec<_>>()}
                        </select>

                        <select
                            class="filter-select"
                            prop:value=selected_source
                            on:change=move |ev| {
                                set_selected_source.set(event_target_value(&ev));
                            }
                        >
                            <option value="all">All Sources</option>
                            {move || sources.get().into_iter().map(|src| view! {
                                <option value={src.clone()}>{src}</option>
                            }).collect::<Vec<_>>()}
                        </select>
                    </div>

                    <div class="sort-controls">
                        <select
                            class="sort-select"
                            prop:value=sort_by
                            on:change=move |ev| {
                                set_sort_by.set(event_target_value(&ev));
                            }
                        >
                            <option value="name">Name</option>
                            <option value="type">Type</option>
                            <option value="category">Category</option>
                            <option value="cardinality">Cardinality</option>
                            <option value="last_scraped">Last Scraped</option>
                        </select>

                        <button
                            class="sort-direction-btn"
                            on:click=move |_| {
                                set_sort_order.set(if sort_order.get() == "asc" { "desc".to_string() } else { "asc".to_string() });
                            }
                        >
                            {move || if sort_order.get() == "asc" { "↑" } else { "↓" }}
                        </button>
                    </div>
                </div>

                <div class="filter-summary">
                    Showing {move || filtered_metrics.get().len()} of {move || metrics.get().len()} metrics
                </div>
            </div>

            // Loading state
            {move || if loading.get() {
                view! {
                    <div class="loading-container">
                        <div class="spinner"></div>
                        <p>Loading metrics catalog...</p>
                    </div>
                }.into_view()
            } else if let Some(err) = error.get() {
                view! {
                    <div class="error-container">
                        <div class="error-message">
                            <h3>Error Loading Metrics</h3>
                            <p>{err}</p>
                            <button
                                class="retry-btn"
                                on:click=move |_| {
                                    spawn_local(async move {
                                        set_loading.set(true);
                                        match get_metrics_catalog().await {
                                            Ok(data) => {
                                                set_metrics.set(data);
                                                set_error.set(None);
                                            }
                                            Err(e) => {
                                                set_error.set(Some(format!("Failed to load metrics: {}", e)));
                                            }
                                        }
                                        set_loading.set(false);
                                    });
                                }
                            >
                                Retry
                            </button>
                        </div>
                    </div>
                }.into_view()
            } else {
                view! {
                    <div class="metrics-grid">
                        {move || filtered_metrics.get().into_iter().map(|metric| {
                            let metric_clone = metric.clone();
                            let metric_clone2 = metric.clone();
                            view! {
                                <div class="metric-card">
                                    <div class="metric-header">
                                        <h3 class="metric-name">{metric.name.clone()}</h3>
                                        <div class="metric-badges">
                                            <span class={format!("metric-type-badge metric-type-{}", metric.metric_type)}>
                                                {metric.metric_type.clone()}
                                            </span>
                                            <span class="metric-category-badge">
                                                {metric.category.clone()}
                                            </span>
                                        </div>
                                    </div>

                                    <div class="metric-description">
                                        {metric.description.clone()}
                                    </div>

                                    <div class="metric-details">
                                        <div class="metric-detail-item">
                                            <span class="label">Source:</span>
                                            <span class="value">{metric.source.clone()}</span>
                                        </div>
                                        <div class="metric-detail-item">
                                            <span class="label">Cardinality:</span>
                                            <span class="value">{metric.cardinality.to_string()}</span>
                                        </div>
                                        <div class="metric-detail-item">
                                            <span class="label">Labels:</span>
                                            <span class="value">{metric.labels.join(", ")}</span>
                                        </div>
                                        {metric.unit.as_ref().map(|unit| view! {
                                            <div class="metric-detail-item">
                                                <span class="label">Unit:</span>
                                                <span class="value">{unit.clone()}</span>
                                            </div>
                                        })}
                                    </div>

                                    <div class="metric-actions">
                                        <button
                                            class="btn btn-secondary"
                                            on:click=move |_| {
                                                load_metric_samples(metric_clone.clone());
                                            }
                                        >
                                            View Details
                                        </button>
                                        <button
                                            class="btn btn-primary"
                                            on:click=move |_| {
                                                add_metric_to_query(metric_clone2.name.clone());
                                            }
                                        >
                                            Add to Query
                                        </button>
                                    </div>
                                </div>
                            }
                        }).collect::<Vec<_>>()}
                    </div>
                }.into_view()
            }}

            // Metric Detail Modal
            {move || if show_metric_detail.get() {
                selected_metric.get().map(|metric| view! {
                    <div class="modal-overlay" on:click=move |_| set_show_metric_detail.set(false)>
                        <div class="modal-content metric-detail-modal" on:click=|e| e.stop_propagation()>
                            <div class="modal-header">
                                <h2>{metric.name.clone()}</h2>
                                <button
                                    class="modal-close"
                                    on:click=move |_| set_show_metric_detail.set(false)
                                >
                                    "x"
                                </button>
                            </div>

                            <div class="modal-body">
                                <div class="metric-full-details">
                                    <div class="detail-section">
                                        <h3>Description</h3>
                                        <p>{metric.description.clone()}</p>
                                        <p>{metric.help.clone()}</p>
                                    </div>

                                    <div class="detail-section">
                                        <h3>Properties</h3>
                                        <div class="property-grid">
                                            <div class="property-item">
                                                <span class="property-label">Type:</span>
                                                <span class="property-value">{metric.metric_type.clone()}</span>
                                            </div>
                                            <div class="property-item">
                                                <span class="property-label">Category:</span>
                                                <span class="property-value">{metric.category.clone()}</span>
                                            </div>
                                            <div class="property-item">
                                                <span class="property-label">Source:</span>
                                                <span class="property-value">{metric.source.clone()}</span>
                                            </div>
                                            <div class="property-item">
                                                <span class="property-label">Cardinality:</span>
                                                <span class="property-value">{metric.cardinality.to_string()}</span>
                                            </div>
                                            <div class="property-item">
                                                <span class="property-label">Scrape Interval:</span>
                                                <span class="property-value">{metric.scrape_interval.clone()}</span>
                                            </div>
                                            <div class="property-item">
                                                <span class="property-label">Retention:</span>
                                                <span class="property-value">{metric.retention.clone()}</span>
                                            </div>
                                            <div class="property-item">
                                                <span class="property-label">Last Scraped:</span>
                                                <span class="property-value">{metric.last_scraped.clone()}</span>
                                            </div>
                                        </div>
                                    </div>

                                    <div class="detail-section">
                                        <h3>Labels</h3>
                                        <div class="labels-list">
                                            {metric.labels.iter().map(|label| view! {
                                                <span class="label-tag">{label.clone()}</span>
                                            }).collect::<Vec<_>>()}
                                        </div>
                                    </div>

                                    {metric_samples.get().map(|samples| view! {
                                        <div class="detail-section">
                                            <h3>Recent Samples ({samples.sample_count} samples)</h3>
                                            <div class="samples-stats">
                                                <div class="stat-item">
                                                    <span class="stat-label">Min:</span>
                                                    <span class="stat-value">{samples.min_value.to_string()}</span>
                                                </div>
                                                <div class="stat-item">
                                                    <span class="stat-label">Max:</span>
                                                    <span class="stat-value">{samples.max_value.to_string()}</span>
                                                </div>
                                                <div class="stat-item">
                                                    <span class="stat-label">Avg:</span>
                                                    <span class="stat-value">{format!("{:.2}", samples.avg_value)}</span>
                                                </div>
                                            </div>
                                            // Here you could add a mini chart of recent samples
                                        </div>
                                    })}
                                </div>
                            </div>

                            <div class="modal-footer">
                                <button
                                    class="btn btn-secondary"
                                    on:click=move |_| set_show_metric_detail.set(false)
                                >
                                    Close
                                </button>
                                <button
                                    class="btn btn-primary"
                                    on:click=move |_| {
                                        add_metric_to_query(metric.name.clone());
                                    }
                                >
                                    Add to Query
                                </button>
                            </div>
                        </div>
                    </div>
                })
            } else {
                None
            }}
        </div>
    }
}