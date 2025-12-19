use leptos::*;
use crate::api::{QueryHistoryEntry, QueryTemplate, CreateQueryTemplateRequest,
    get_query_history, get_query_templates, toggle_query_favorite, create_query_template};

#[component]
pub fn QueryHistoryPage() -> impl IntoView {
    let (history, set_history) = create_signal(Vec::<QueryHistoryEntry>::new());
    let (templates, set_templates) = create_signal(Vec::<QueryTemplate>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);

    // Filter states
    let (search_term, set_search_term) = create_signal(String::new());
    let (filter_status, set_filter_status) = create_signal("all".to_string());
    let (filter_source, set_filter_source) = create_signal("all".to_string());
    let (filter_user, set_filter_user) = create_signal("all".to_string());
    let (time_range, set_time_range) = create_signal("24h".to_string());
    let (show_favorites_only, set_show_favorites_only) = create_signal(false);

    // View states
    let (current_view, set_current_view) = create_signal("history".to_string()); // history, templates
    let (selected_entry, set_selected_entry) = create_signal(None::<QueryHistoryEntry>);
    let (show_query_detail, set_show_query_detail) = create_signal(false);
    let (show_template_form, set_show_template_form) = create_signal(false);

    // Template form states
    let (template_name, set_template_name) = create_signal(String::new());
    let (template_description, set_template_description) = create_signal(String::new());
    let (template_category, set_template_category) = create_signal("general".to_string());
    let (template_public, set_template_public) = create_signal(false);
    let (template_query, set_template_query) = create_signal(String::new());

    // Load data on mount
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);

            let history_result = get_query_history(time_range.get()).await;
            let templates_result = get_query_templates().await;

            match (history_result, templates_result) {
                (Ok(hist), Ok(tmpl)) => {
                    set_history.set(hist);
                    set_templates.set(tmpl);
                    set_error.set(None);
                }
                (Err(e), _) | (_, Err(e)) => {
                    set_error.set(Some(format!("Failed to load data: {}", e)));
                }
            }
            set_loading.set(false);
        });
    });

    // Filtered history
    let filtered_history = create_memo(move |_| {
        let mut filtered: Vec<QueryHistoryEntry> = history.get()
            .into_iter()
            .filter(|entry| {
                let search_match = if search_term.get().is_empty() {
                    true
                } else {
                    let term = search_term.get().to_lowercase();
                    entry.query.to_lowercase().contains(&term) ||
                    entry.user.to_lowercase().contains(&term) ||
                    entry.tags.iter().any(|tag| tag.to_lowercase().contains(&term))
                };

                let status_match = filter_status.get() == "all" || entry.status == filter_status.get();
                let source_match = filter_source.get() == "all" || entry.source == filter_source.get();
                let user_match = filter_user.get() == "all" || entry.user == filter_user.get();
                let favorites_match = !show_favorites_only.get() || entry.favorite;

                search_match && status_match && source_match && user_match && favorites_match
            })
            .collect();

        // Sort by timestamp (newest first)
        filtered.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        filtered
    });

    // Execute query from history
    let execute_query = move |query: String| {
        web_sys::window()
            .unwrap()
            .location()
            .set_href(&format!("/metrics/explorer?query={}", urlencoding::encode(&query)))
            .unwrap();
    };

    // Toggle favorite
    let toggle_favorite = move |entry_id: String| {
        spawn_local(async move {
            let _ = toggle_query_favorite(entry_id).await;
            // Reload history
            match get_query_history(time_range.get()).await {
                Ok(hist) => set_history.set(hist),
                Err(_) => {}
            }
        });
    };

    // Save query as template
    let save_as_template = move |query: String| {
        set_template_query.set(query);
        set_show_template_form.set(true);
    };

    // Create template action
    let create_template_action = create_action(move |_: &()| async move {
        let request = CreateQueryTemplateRequest {
            name: template_name.get(),
            description: template_description.get(),
            query: template_query.get(),
            category: template_category.get(),
            public: template_public.get(),
            variables: vec![], // Could be enhanced to parse variables from query
        };

        match create_query_template(request).await {
            Ok(_) => {
                set_show_template_form.set(false);
                // Reset form
                set_template_name.set(String::new());
                set_template_description.set(String::new());
                set_template_category.set("general".to_string());
                set_template_public.set(false);
                set_template_query.set(String::new());

                // Reload templates
                match get_query_templates().await {
                    Ok(tmpl) => set_templates.set(tmpl),
                    Err(_) => {}
                }
                true
            }
            Err(_) => false
        }
    });

    view! {
        <div class="query-history-page">
            <div class="page-header">
                <h1 class="page-title">Query History & Templates</h1>
                <p class="page-description">
                    Manage your query history and create reusable templates
                </p>

                <div class="page-tabs">
                    <button
                        class={move || if current_view.get() == "history" { "tab-btn active" } else { "tab-btn" }}
                        on:click=move |_| set_current_view.set("history".to_string())
                    >
                        Query History
                    </button>
                    <button
                        class={move || if current_view.get() == "templates" { "tab-btn active" } else { "tab-btn" }}
                        on:click=move |_| set_current_view.set("templates".to_string())
                    >
                        Templates
                    </button>
                </div>
            </div>

            // History View
            {move || if current_view.get() == "history" {
                view! {
                    <div class="history-view">
                        // Filters
                        <div class="history-filters">
                            <div class="filter-row">
                                <div class="search-box">
                                    <input
                                        type="text"
                                        placeholder="Search queries, users, tags..."
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
                                        prop:value=time_range
                                        on:change=move |ev| {
                                            let range = event_target_value(&ev);
                                            set_time_range.set(range.clone());
                                            spawn_local(async move {
                                                match get_query_history(range).await {
                                                    Ok(hist) => set_history.set(hist),
                                                    Err(_) => {}
                                                }
                                            });
                                        }
                                    >
                                        <option value="1h">Last Hour</option>
                                        <option value="24h">Last 24 Hours</option>
                                        <option value="7d">Last 7 Days</option>
                                        <option value="30d">Last 30 Days</option>
                                    </select>

                                    <select
                                        class="filter-select"
                                        prop:value=filter_status
                                        on:change=move |ev| {
                                            set_filter_status.set(event_target_value(&ev));
                                        }
                                    >
                                        <option value="all">All Status</option>
                                        <option value="success">Success</option>
                                        <option value="error">Error</option>
                                        <option value="timeout">Timeout</option>
                                    </select>

                                    <select
                                        class="filter-select"
                                        prop:value=filter_source
                                        on:change=move |ev| {
                                            set_filter_source.set(event_target_value(&ev));
                                        }
                                    >
                                        <option value="all">All Sources</option>
                                        <option value="metrics_explorer">Metrics Explorer</option>
                                        <option value="dashboard">Dashboard</option>
                                        <option value="alert_rule">Alert Rule</option>
                                        <option value="api">API</option>
                                    </select>
                                </div>

                                <div class="toggle-filters">
                                    <label class="toggle-filter">
                                        <input
                                            type="checkbox"
                                            prop:checked=show_favorites_only
                                            on:change=move |ev| {
                                                set_show_favorites_only.set(event_target_checked(&ev));
                                            }
                                        />
                                        Favorites Only
                                    </label>
                                </div>
                            </div>
                        </div>

                        // History List
                        {move || if loading.get() {
                            view! {
                                <div class="loading-container">
                                    <div class="spinner"></div>
                                    <p>Loading query history...</p>
                                </div>
                            }.into_view()
                        } else if let Some(err) = error.get() {
                            view! {
                                <div class="error-container">
                                    <div class="error-message">
                                        <h3>Error Loading History</h3>
                                        <p>{err}</p>
                                    </div>
                                </div>
                            }.into_view()
                        } else {
                            view! {
                                <div class="history-list">
                                    {move || if filtered_history.get().is_empty() {
                                        view! {
                                            <div class="empty-state">
                                                <h3>No queries found</h3>
                                                <p>Try adjusting your filters or time range</p>
                                            </div>
                                        }.into_view()
                                    } else {
                                        filtered_history.get().into_iter().map(|entry| {
                                            let entry_clone = entry.clone();
                                            let entry_clone2 = entry.clone();
                                            let entry_clone3 = entry.clone();
                                            view! {
                                                <div class="history-entry">
                                                    <div class="entry-header">
                                                        <div class="entry-meta">
                                                            <span class="entry-timestamp">{entry.timestamp.clone()}</span>
                                                            <span class={format!("entry-status status-{}", entry.status)}>
                                                                {entry.status.clone()}
                                                            </span>
                                                            <span class="entry-source">{entry.source.clone()}</span>
                                                            <span class="entry-user">{entry.user.clone()}</span>
                                                        </div>
                                                        <div class="entry-actions">
                                                            <button
                                                                class={move || if entry.favorite { "favorite-btn active" } else { "favorite-btn" }}
                                                                on:click=move |_| {
                                                                    toggle_favorite(entry_clone.id.clone());
                                                                }
                                                                title="Toggle favorite"
                                                            >
                                                                "*"
                                                            </button>
                                                            <button
                                                                class="action-btn"
                                                                on:click=move |_| {
                                                                    execute_query(entry_clone2.query.clone());
                                                                }
                                                                title="Execute query"
                                                            >
                                                                Run
                                                            </button>
                                                            <button
                                                                class="action-btn"
                                                                on:click=move |_| {
                                                                    save_as_template(entry_clone3.query.clone());
                                                                }
                                                                title="Save as template"
                                                            >
                                                                Save
                                                            </button>
                                                        </div>
                                                    </div>

                                                    <div class="entry-query">
                                                        <code>{entry.query.clone()}</code>
                                                    </div>

                                                    <div class="entry-details">
                                                        <span class="detail-item">
                                                            Duration: {format!("{:.2}ms", entry.execution_time_ms)}
                                                        </span>
                                                        <span class="detail-item">
                                                            Results: {entry.result_count.to_string()}
                                                        </span>
                                                        {entry.error_message.as_ref().map(|err| view! {
                                                            <span class="detail-item error-detail">
                                                                Error: {err.clone()}
                                                            </span>
                                                        })}
                                                    </div>

                                                    {if !entry.tags.is_empty() {
                                                        view! {
                                                            <div class="entry-tags">
                                                                {entry.tags.iter().map(|tag| view! {
                                                                    <span class="tag">{tag.clone()}</span>
                                                                }).collect::<Vec<_>>()}
                                                            </div>
                                                        }.into_view()
                                                    } else {
                                                        view! { <div></div> }.into_view()
                                                    }}
                                                </div>
                                            }
                                        }).collect::<Vec<_>>().into_view()
                                    }}
                                </div>
                            }.into_view()
                        }}
                    </div>
                }.into_view()
            } else {
                view! {
                    <div class="templates-view">
                        <div class="templates-header">
                            <h2>Query Templates</h2>
                            <button
                                class="btn btn-primary"
                                on:click=move |_| {
                                    set_template_query.set(String::new());
                                    set_show_template_form.set(true);
                                }
                            >
                                Create Template
                            </button>
                        </div>

                        <div class="templates-grid">
                            {move || templates.get().into_iter().map(|template| {
                                let template_clone = template.clone();
                                view! {
                                    <div class="template-card">
                                        <div class="template-header">
                                            <h3>{template.name.clone()}</h3>
                                            <div class="template-badges">
                                                <span class="category-badge">{template.category.clone()}</span>
                                                {if template.public {
                                                    view! { <span class="public-badge">Public</span> }.into_view()
                                                } else {
                                                    view! { <span class="private-badge">Private</span> }.into_view()
                                                }}
                                            </div>
                                        </div>

                                        <p class="template-description">{template.description.clone()}</p>

                                        <div class="template-query">
                                            <code>{template.query.clone()}</code>
                                        </div>

                                        <div class="template-meta">
                                            <span>By {template.created_by.clone()}</span>
                                            <span>Used {template.usage_count.to_string()} times</span>
                                        </div>

                                        <div class="template-actions">
                                            <button
                                                class="btn btn-secondary"
                                                on:click=move |_| {
                                                    execute_query(template_clone.query.clone());
                                                }
                                            >
                                                Use Template
                                            </button>
                                        </div>
                                    </div>
                                }
                            }).collect::<Vec<_>>()}
                        </div>
                    </div>
                }.into_view()
            }}

            // Template Creation Modal
            {move || if show_template_form.get() {
                view! {
                    <div class="modal-overlay" on:click=move |_| set_show_template_form.set(false)>
                        <div class="modal-content template-form-modal" on:click=|e| e.stop_propagation()>
                            <div class="modal-header">
                                <h2>Create Query Template</h2>
                                <button
                                    class="modal-close"
                                    on:click=move |_| set_show_template_form.set(false)
                                >
                                    "x"
                                </button>
                            </div>

                            <div class="modal-body">
                                <form
                                    on:submit=move |ev| {
                                        ev.prevent_default();
                                        create_template_action.dispatch(());
                                    }
                                >
                                    <div class="form-group">
                                        <label for="template-name">Template Name</label>
                                        <input
                                            type="text"
                                            id="template-name"
                                            class="form-control"
                                            prop:value=template_name
                                            on:input=move |ev| {
                                                set_template_name.set(event_target_value(&ev));
                                            }
                                            required
                                        />
                                    </div>

                                    <div class="form-group">
                                        <label for="template-description">Description</label>
                                        <textarea
                                            id="template-description"
                                            class="form-control"
                                            rows="3"
                                            prop:value=template_description
                                            on:input=move |ev| {
                                                set_template_description.set(event_target_value(&ev));
                                            }
                                        ></textarea>
                                    </div>

                                    <div class="form-group">
                                        <label for="template-category">Category</label>
                                        <select
                                            id="template-category"
                                            class="form-control"
                                            prop:value=template_category
                                            on:change=move |ev| {
                                                set_template_category.set(event_target_value(&ev));
                                            }
                                        >
                                            <option value="general">General</option>
                                            <option value="infrastructure">Infrastructure</option>
                                            <option value="application">Application</option>
                                            <option value="security">Security</option>
                                            <option value="performance">Performance</option>
                                        </select>
                                    </div>

                                    <div class="form-group">
                                        <label for="template-query">Query</label>
                                        <textarea
                                            id="template-query"
                                            class="form-control query-textarea"
                                            rows="6"
                                            prop:value=template_query
                                            on:input=move |ev| {
                                                set_template_query.set(event_target_value(&ev));
                                            }
                                            required
                                        ></textarea>
                                    </div>

                                    <div class="form-group">
                                        <label class="checkbox-label">
                                            <input
                                                type="checkbox"
                                                prop:checked=template_public
                                                on:change=move |ev| {
                                                    set_template_public.set(event_target_checked(&ev));
                                                }
                                            />
                                            Make this template public
                                        </label>
                                    </div>

                                    <div class="form-actions">
                                        <button
                                            type="button"
                                            class="btn btn-secondary"
                                            on:click=move |_| set_show_template_form.set(false)
                                        >
                                            Cancel
                                        </button>
                                        <button
                                            type="submit"
                                            class="btn btn-primary"
                                            disabled=move || create_template_action.pending().get()
                                        >
                                            {move || if create_template_action.pending().get() {
                                                "Creating..."
                                            } else {
                                                "Create Template"
                                            }}
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