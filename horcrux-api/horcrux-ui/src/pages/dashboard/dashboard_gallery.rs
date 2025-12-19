use leptos::*;
use crate::api::*;

#[component]
pub fn DashboardGalleryPage() -> impl IntoView {
    let (featured_dashboards, set_featured_dashboards) = create_signal(Vec::<CustomDashboard>::new());
    let (public_dashboards, set_public_dashboards) = create_signal(Vec::<CustomDashboard>::new());
    let (dashboard_categories, set_dashboard_categories) = create_signal(Vec::<DashboardCategory>::new());
    let (loading, set_loading) = create_signal(false);
    let (error_message, set_error_message) = create_signal(None::<String>);
    let (success_message, set_success_message) = create_signal(None::<String>);
    let (selected_category, set_selected_category) = create_signal("all".to_string());
    let (search_query, set_search_query) = create_signal(String::new());
    let (sort_by, set_sort_by) = create_signal("popular".to_string());
    let (view_mode, set_view_mode) = create_signal("grid".to_string());

    // Load dashboard gallery data
    let load_gallery = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);

        // Load featured dashboards
        match get_featured_dashboards().await {
            Ok(dashboards) => set_featured_dashboards.set(dashboards),
            Err(e) => set_error_message.set(Some(format!("Failed to load featured dashboards: {}", e))),
        }

        // Load public dashboards
        match get_public_dashboards().await {
            Ok(dashboards) => set_public_dashboards.set(dashboards),
            Err(e) => set_error_message.set(Some(format!("Failed to load public dashboards: {}", e))),
        }

        // Load categories
        match get_dashboard_categories().await {
            Ok(categories) => set_dashboard_categories.set(categories),
            Err(e) => set_error_message.set(Some(format!("Failed to load categories: {}", e))),
        }

        set_loading.set(false);
    });

    // Import dashboard action
    let import_dashboard_action = create_action(move |dashboard: &CustomDashboard| {
        let dashboard = dashboard.clone();
        async move {
            set_loading.set(true);
            set_error_message.set(None);

            let request = ImportDashboardRequest {
                source_dashboard_id: dashboard.id.clone(),
                name: format!("Imported - {}", dashboard.name),
                import_widgets: true,
            };

            match import_dashboard(request).await {
                Ok(imported_dashboard) => {
                    set_success_message.set(Some(format!("Dashboard imported as '{}'", imported_dashboard.name)));
                }
                Err(e) => set_error_message.set(Some(format!("Failed to import dashboard: {}", e))),
            }

            set_loading.set(false);
        }
    });

    // Preview dashboard action
    let preview_dashboard_action = create_action(move |dashboard_id: &String| {
        let dashboard_id = dashboard_id.clone();
        async move {
            // This would open a preview modal or navigate to preview page
            set_success_message.set(Some(format!("Opening preview for dashboard {}", dashboard_id)));
        }
    });

    // Filter and sort dashboards
    let filtered_public_dashboards = move || {
        let query = search_query.get().to_lowercase();
        let category = selected_category.get();
        let sort = sort_by.get();

        let mut filtered: Vec<CustomDashboard> = public_dashboards.get()
            .into_iter()
            .filter(|dashboard| {
                let matches_search = query.is_empty() ||
                    dashboard.name.to_lowercase().contains(&query) ||
                    dashboard.description.as_ref().map_or(false, |d| d.to_lowercase().contains(&query)) ||
                    dashboard.tags.iter().any(|tag| tag.to_lowercase().contains(&query));

                let matches_category = category == "all" || dashboard.category == category;

                matches_search && matches_category
            })
            .collect();

        // Sort dashboards
        match sort.as_str() {
            "popular" => filtered.sort_by(|a, b| b.usage_count.cmp(&a.usage_count)),
            "recent" => filtered.sort_by(|a, b| b.updated_at.cmp(&a.updated_at)),
            "name" => filtered.sort_by(|a, b| a.name.cmp(&b.name)),
            "widgets" => filtered.sort_by(|a, b| b.widget_count.cmp(&a.widget_count)),
            _ => {}
        }

        filtered
    };

    // Helper functions
    let get_dashboard_preview = move |dashboard: &CustomDashboard| -> String {
        if dashboard.widget_count > 0 {
            format!("📊 {} widgets", dashboard.widget_count)
        } else {
            "Empty dashboard".to_string()
        }
    };

    let get_popularity_score = move |dashboard: &CustomDashboard| -> String {
        let score = dashboard.usage_count as f64 * 0.7 + dashboard.rating.unwrap_or(0.0) * 0.3;
        format!("{:.1}", score)
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
        load_gallery.dispatch(());
    });

    view! {
        <div class="dashboard-gallery-page">
            <div class="page-header">
                <div class="header-title">
                    <h1>"Dashboard Gallery"</h1>
                    <p class="header-subtitle">"Discover and import dashboards from the community"</p>
                </div>
                <div class="header-actions">
                    <button
                        class="btn btn-secondary"
                        on:click=move |_| load_gallery.dispatch(())
                        disabled=loading
                    >
                        "Refresh"
                    </button>
                    <a href="/dashboard/builder" class="btn btn-primary">
                        "Create Dashboard"
                    </a>
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

            // Featured Dashboards Section
            {move || if !featured_dashboards.get().is_empty() {
                view! {
                    <div class="featured-section">
                        <div class="section-header">
                            <h2>"Featured Dashboards"</h2>
                            <p>"Curated dashboards for common monitoring scenarios"</p>
                        </div>
                        <div class="featured-carousel">
                            {featured_dashboards.get().into_iter().map(|dashboard| {
                                let dashboard_clone1 = dashboard.clone();
                                let dashboard_clone2 = dashboard.clone();
                                let preview_text = get_dashboard_preview(&dashboard);
                                let dash_name = dashboard.name.clone();
                                let dash_category = dashboard.category.clone();
                                let dash_description = dashboard.description.clone();
                                let dash_usage_count = dashboard.usage_count;
                                let dash_rating = dashboard.rating;

                                view! {
                                    <div class="featured-card">
                                        <div class="card-image">
                                            <div class="dashboard-preview">
                                                {preview_text}
                                            </div>
                                            <div class="card-overlay">
                                                <button
                                                    class="btn btn-sm btn-primary"
                                                    on:click=move |_| preview_dashboard_action.dispatch(dashboard_clone1.id.clone())
                                                >
                                                    "Preview"
                                                </button>
                                                <button
                                                    class="btn btn-sm btn-secondary"
                                                    on:click=move |_| import_dashboard_action.dispatch(dashboard_clone2.clone())
                                                >
                                                    "Import"
                                                </button>
                                            </div>
                                        </div>
                                        <div class="card-content">
                                            <h3>{dash_name}</h3>
                                            {dash_description.map(|desc| view! {
                                                <p class="dashboard-description">{desc}</p>
                                            })}
                                            <div class="dashboard-meta">
                                                <span class="category-badge">{dash_category}</span>
                                                <span class="usage-count">"{dash_usage_count} uses"</span>
                                                {dash_rating.map(|rating| view! {
                                                    <span class="rating">
                                                        {(0..5).map(|i| {
                                                            if i < rating as usize {
                                                                "⭐"
                                                            } else {
                                                                "☆"
                                                            }
                                                        }).collect::<String>()}
                                                        " "{format!("{:.1}", rating)}
                                                    </span>
                                                })}
                                            </div>
                                        </div>
                                    </div>
                                }
                            }).collect::<Vec<_>>()}
                        </div>
                    </div>
                }.into_view()
            } else {
                view! { <div></div> }.into_view()
            }}

            // Gallery Controls
            <div class="gallery-controls">
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
                        <label>"Category:"</label>
                        <select
                            prop:value=selected_category
                            on:change=move |ev| set_selected_category.set(event_target_value(&ev))
                        >
                            <option value="all">"All Categories"</option>
                            {dashboard_categories.get().into_iter().map(|category| {
                                let cat_name_val = category.name.clone();
                                let cat_name_display = category.name.clone();
                                let cat_count = category.dashboard_count;
                                view! {
                                    <option value={cat_name_val}>
                                        {cat_name_display}" ("{cat_count}")"
                                    </option>
                                }
                            }).collect::<Vec<_>>()}
                        </select>

                        <label>"Sort by:"</label>
                        <select
                            prop:value=sort_by
                            on:change=move |ev| set_sort_by.set(event_target_value(&ev))
                        >
                            <option value="popular">"Most Popular"</option>
                            <option value="recent">"Recently Updated"</option>
                            <option value="name">"Name"</option>
                            <option value="widgets">"Widget Count"</option>
                        </select>

                        <label>"View:"</label>
                        <div class="view-toggle">
                            <button
                                class={format!("btn btn-xs {}", if view_mode.get() == "grid" { "btn-primary" } else { "btn-secondary" })}
                                on:click=move |_| set_view_mode.set("grid".to_string())
                            >
                                "Grid"
                            </button>
                            <button
                                class={format!("btn btn-xs {}", if view_mode.get() == "list" { "btn-primary" } else { "btn-secondary" })}
                                on:click=move |_| set_view_mode.set("list".to_string())
                            >
                                "List"
                            </button>
                        </div>
                    </div>
                </div>
            </div>

            // Public Dashboards Gallery
            <div class="public-gallery-section">
                <div class="section-header">
                    <h2>"Public Dashboard Gallery"</h2>
                    <p>"Community-created dashboards available for import"</p>
                </div>

                {move || if loading.get() && public_dashboards.get().is_empty() {
                    view! { <div class="loading">"Loading gallery..."</div> }.into_view()
                } else {
                    let filtered = filtered_public_dashboards();
                    if filtered.is_empty() {
                        view! {
                            <div class="empty-state">
                                <div class="empty-icon">"🎨"</div>
                                <h3>"No dashboards found"</h3>
                                <p>"No dashboards match your current search and filters"</p>
                            </div>
                        }.into_view()
                    } else if view_mode.get() == "grid" {
                        view! {
                            <div class="gallery-grid">
                                {filtered_public_dashboards().into_iter().map(|dashboard| {
                                    let dashboard_clone1 = dashboard.clone();
                                    let dashboard_clone2 = dashboard.clone();
                                    let dash_name = dashboard.name.clone();
                                    let dash_category = dashboard.category.clone();
                                    let dash_description = dashboard.description.clone();
                                    let dash_preview = get_dashboard_preview(&dashboard);
                                    let dash_popularity = get_popularity_score(&dashboard);
                                    let dash_widget_count = dashboard.widget_count;
                                    let dash_usage_count = dashboard.usage_count;
                                    let dash_rating = dashboard.rating;
                                    let dash_tags = dashboard.tags.clone();
                                    let dash_created_by = dashboard.created_by.clone().unwrap_or("Unknown".to_string());
                                    let dash_updated = dashboard.updated_at[..10].to_string();

                                    view! {
                                        <div class="gallery-card">
                                            <div class="card-header">
                                                <div class="dashboard-info">
                                                    <h3>{dash_name}</h3>
                                                    <div class="dashboard-badges">
                                                        <span class="category-badge">{dash_category}</span>
                                                        <span class="popularity-badge">
                                                            "Score: "{dash_popularity}
                                                        </span>
                                                    </div>
                                                </div>
                                            </div>

                                            <div class="card-content">
                                                {dash_description.map(|desc| view! {
                                                    <p class="dashboard-description">{desc}</p>
                                                })}

                                                <div class="dashboard-preview-mini">
                                                    {dash_preview}
                                                </div>

                                                <div class="dashboard-stats">
                                                    <div class="stat-group">
                                                        <span class="stat-label">"Widgets:"</span>
                                                        <span class="stat-value">{dash_widget_count}</span>
                                                    </div>
                                                    <div class="stat-group">
                                                        <span class="stat-label">"Uses:"</span>
                                                        <span class="stat-value">{dash_usage_count}</span>
                                                    </div>
                                                    {dash_rating.map(|rating| view! {
                                                        <div class="stat-group">
                                                            <span class="stat-label">"Rating:"</span>
                                                            <span class="stat-value rating-value">{format!("{:.1}⭐", rating)}</span>
                                                        </div>
                                                    })}
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

                                            <div class="card-actions">
                                                <button
                                                    class="btn btn-sm btn-outline"
                                                    on:click=move |_| preview_dashboard_action.dispatch(dashboard_clone1.id.clone())
                                                >
                                                    "Preview"
                                                </button>
                                                <button
                                                    class="btn btn-sm btn-primary"
                                                    on:click=move |_| import_dashboard_action.dispatch(dashboard_clone2.clone())
                                                >
                                                    "Import"
                                                </button>
                                            </div>

                                            <div class="card-footer">
                                                <span class="created-by">
                                                    "by "{dash_created_by}
                                                </span>
                                                <span class="updated-date">
                                                    {dash_updated}
                                                </span>
                                            </div>
                                        </div>
                                    }
                                }).collect::<Vec<_>>()}
                            </div>
                        }.into_view()
                    } else {
                        view! {
                            <div class="gallery-list">
                                {filtered_public_dashboards().into_iter().map(|dashboard| {
                                    let dashboard_clone1 = dashboard.clone();
                                    let dashboard_clone2 = dashboard.clone();
                                    let dash_name = dashboard.name.clone();
                                    let dash_category = dashboard.category.clone();
                                    let dash_description = dashboard.description.clone();
                                    let dash_popularity = get_popularity_score(&dashboard);
                                    let dash_widget_count = dashboard.widget_count;
                                    let dash_usage_count = dashboard.usage_count;
                                    let dash_updated = dashboard.updated_at[..10].to_string();
                                    let dash_rating = dashboard.rating;
                                    let dash_tags = dashboard.tags.clone();

                                    view! {
                                        <div class="list-item">
                                            <div class="item-content">
                                                <div class="item-header">
                                                    <h3>{dash_name}</h3>
                                                    <div class="item-badges">
                                                        <span class="category-badge">{dash_category}</span>
                                                        <span class="popularity-badge">
                                                            "Score: "{dash_popularity}
                                                        </span>
                                                    </div>
                                                </div>

                                                {dash_description.map(|desc| view! {
                                                    <p class="item-description">{desc}</p>
                                                })}

                                                <div class="item-meta">
                                                    <span>{dash_widget_count}" widgets"</span>
                                                    <span>{dash_usage_count}" uses"</span>
                                                    <span>"Updated "{dash_updated}</span>
                                                    {dash_rating.map(|rating| view! {
                                                        <span class="rating">{format!("{:.1}⭐", rating)}</span>
                                                    })}
                                                </div>

                                                {if !dash_tags.is_empty() {
                                                    view! {
                                                        <div class="item-tags">
                                                            {dash_tags.into_iter().map(|tag| view! {
                                                                <span class="tag">{tag}</span>
                                                            }).collect::<Vec<_>>()}
                                                        </div>
                                                    }.into_view()
                                                } else {
                                                    view! { <div></div> }.into_view()
                                                }}
                                            </div>

                                            <div class="item-actions">
                                                <button
                                                    class="btn btn-sm btn-outline"
                                                    on:click=move |_| preview_dashboard_action.dispatch(dashboard_clone1.id.clone())
                                                >
                                                    "Preview"
                                                </button>
                                                <button
                                                    class="btn btn-sm btn-primary"
                                                    on:click=move |_| import_dashboard_action.dispatch(dashboard_clone2.clone())
                                                >
                                                    "Import"
                                                </button>
                                            </div>
                                        </div>
                                    }
                                }).collect::<Vec<_>>()}
                            </div>
                        }.into_view()
                    }
                }}

                // Gallery Statistics
                <div class="gallery-stats">
                    <div class="stats-container">
                        {
                            let total_public = public_dashboards.get().len();
                            let featured_count = featured_dashboards.get().len();
                            let filtered_count = filtered_public_dashboards().len();
                            let categories = dashboard_categories.get();

                            view! {
                                <>
                                    <div class="stat-item">
                                        <span class="stat-label">"Total Public:"</span>
                                        <span class="stat-value">{total_public}</span>
                                    </div>
                                    <div class="stat-item">
                                        <span class="stat-label">"Featured:"</span>
                                        <span class="stat-value text-blue-600">{featured_count}</span>
                                    </div>
                                    <div class="stat-item">
                                        <span class="stat-label">"Showing:"</span>
                                        <span class="stat-value text-green-600">{filtered_count}</span>
                                    </div>
                                    <div class="stat-item">
                                        <span class="stat-label">"Categories:"</span>
                                        <span class="stat-value text-purple-600">{categories.len()}</span>
                                    </div>
                                </>
                            }
                        }
                    </div>
                </div>
            </div>
        </div>
    }
}