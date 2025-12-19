use leptos::*;
use crate::api::{AuditEvent, AuditFilter, AuditExportRequest, get_audit_events, export_audit_events, get_audit_filter_options};

#[component]
pub fn AuditLogPage() -> impl IntoView {
    let (audit_events, set_audit_events) = create_signal(Vec::<AuditEvent>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);
    let (total_events, set_total_events) = create_signal(0u64);

    // Filter states
    let (search_term, set_search_term) = create_signal(String::new());
    let (start_date, set_start_date) = create_signal(String::new());
    let (end_date, set_end_date) = create_signal(String::new());
    let (selected_event_types, set_selected_event_types) = create_signal(Vec::<String>::new());
    let (selected_users, set_selected_users) = create_signal(Vec::<String>::new());
    let (selected_severity, set_selected_severity) = create_signal("all".to_string());
    let (success_filter, set_success_filter) = create_signal("all".to_string());
    let (source_ip_filter, set_source_ip_filter) = create_signal(String::new());
    let (correlation_id_filter, set_correlation_id_filter) = create_signal(String::new());

    // Pagination
    let (page, set_page) = create_signal(1);
    let (page_size, set_page_size) = create_signal(50);

    // View states
    let (selected_event, set_selected_event) = create_signal(None::<AuditEvent>);
    let (show_event_detail, set_show_event_detail) = create_signal(false);
    let (show_export_modal, set_show_export_modal) = create_signal(false);
    let (show_advanced_filters, set_show_advanced_filters) = create_signal(false);

    // Export states
    let (export_format, set_export_format) = create_signal("csv".to_string());
    let (export_fields, set_export_fields) = create_signal(vec![
        "timestamp".to_string(),
        "event_type".to_string(),
        "user".to_string(),
        "action".to_string(),
        "success".to_string()
    ]);
    let (include_details, set_include_details) = create_signal(false);

    // Available filter options
    let (available_event_types, set_available_event_types) = create_signal(Vec::<String>::new());
    let (available_users, set_available_users) = create_signal(Vec::<String>::new());

    // Real-time updates
    let (auto_refresh, set_auto_refresh) = create_signal(true);
    let (refresh_interval, set_refresh_interval) = create_signal(5000);

    // Build filter helper - must be defined before use
    let build_current_filter = move || {
        AuditFilter {
            start_time: if start_date.get().is_empty() { None } else { Some(start_date.get()) },
            end_time: if end_date.get().is_empty() { None } else { Some(end_date.get()) },
            event_types: selected_event_types.get(),
            users: selected_users.get(),
            resource_types: vec![],
            actions: vec![],
            severity_levels: if selected_severity.get() == "all" {
                vec![]
            } else {
                vec![selected_severity.get()]
            },
            success_filter: match success_filter.get().as_str() {
                "success" => Some(true),
                "failure" => Some(false),
                _ => None,
            },
            source_ips: if source_ip_filter.get().is_empty() {
                vec![]
            } else {
                vec![source_ip_filter.get()]
            },
            search_term: if search_term.get().is_empty() { None } else { Some(search_term.get()) },
            correlation_id: if correlation_id_filter.get().is_empty() {
                None
            } else {
                Some(correlation_id_filter.get())
            },
        }
    };

    // Load initial data and filter options
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);

            let filter = build_current_filter();

            match (
                get_audit_events(filter, page.get(), page_size.get()).await,
                get_audit_filter_options().await,
            ) {
                (Ok((events, total)), Ok(options)) => {
                    set_audit_events.set(events);
                    set_total_events.set(total);
                    set_available_event_types.set(options.event_types);
                    set_available_users.set(options.users);
                    set_error.set(None);
                }
                (Err(e), _) | (_, Err(e)) => {
                    set_error.set(Some(format!("Failed to load audit data: {}", e)));
                }
            }
            set_loading.set(false);
        });
    });

    // Auto-refresh effect
    create_effect(move |_| {
        if auto_refresh.get() {
            let interval = refresh_interval.get();
            spawn_local(async move {
                loop {
                    gloo_timers::future::TimeoutFuture::new(interval).await;
                    if !auto_refresh.get() { break; }

                    let filter = build_current_filter();
                    if let Ok((events, total)) = get_audit_events(filter, page.get(), page_size.get()).await {
                        set_audit_events.set(events);
                        set_total_events.set(total);
                    }
                }
            });
        }
    });

    // Export audit logs
    let export_audit_logs = move || {
        let request = AuditExportRequest {
            format: export_format.get(),
            filter: build_current_filter(),
            fields: export_fields.get(),
            include_details: include_details.get(),
        };

        spawn_local(async move {
            match export_audit_events(request).await {
                Ok(download_url) => {
                    web_sys::window()
                        .unwrap()
                        .location()
                        .set_href(&download_url)
                        .unwrap();
                    set_show_export_modal.set(false);
                }
                Err(_) => {
                    // Show error notification
                }
            }
        });
    };

    // Apply filters
    let apply_filters = move || {
        set_page.set(1); // Reset to first page when filters change
        spawn_local(async move {
            set_loading.set(true);
            let filter = build_current_filter();

            match get_audit_events(filter, 1, page_size.get()).await {
                Ok((events, total)) => {
                    set_audit_events.set(events);
                    set_total_events.set(total);
                    set_error.set(None);
                }
                Err(e) => {
                    set_error.set(Some(format!("Failed to apply filters: {}", e)));
                }
            }
            set_loading.set(false);
        });
    };

    // Clear filters
    let clear_filters = move || {
        set_search_term.set(String::new());
        set_start_date.set(String::new());
        set_end_date.set(String::new());
        set_selected_event_types.set(Vec::new());
        set_selected_users.set(Vec::new());
        set_selected_severity.set("all".to_string());
        set_success_filter.set("all".to_string());
        set_source_ip_filter.set(String::new());
        set_correlation_id_filter.set(String::new());
        apply_filters();
    };

    view! {
        <div class="audit-log-page">
            <div class="page-header">
                <h1 class="page-title">Audit Log</h1>
                <p class="page-description">
                    Monitor all system activities and security events for compliance and forensics
                </p>

                <div class="page-actions">
                    <div class="auto-refresh-controls">
                        <label class="auto-refresh-toggle">
                            <input
                                type="checkbox"
                                prop:checked=auto_refresh
                                on:change=move |ev| {
                                    set_auto_refresh.set(event_target_checked(&ev));
                                }
                            />
                            Auto-refresh
                        </label>
                        <select
                            class="refresh-interval-select"
                            prop:value=refresh_interval
                            on:change=move |ev| {
                                if let Ok(interval) = event_target_value(&ev).parse::<u32>() {
                                    set_refresh_interval.set(interval);
                                }
                            }
                            disabled=move || !auto_refresh.get()
                        >
                            <option value="5000">5s</option>
                            <option value="10000">10s</option>
                            <option value="30000">30s</option>
                            <option value="60000">1m</option>
                        </select>
                    </div>

                    <button
                        class="btn btn-secondary"
                        on:click=move |_| set_show_export_modal.set(true)
                    >
                        Export Logs
                    </button>

                    <button
                        class="btn btn-secondary"
                        on:click=move |_| apply_filters()
                    >
                        Refresh
                    </button>
                </div>
            </div>

            // Basic filters
            <div class="audit-filters">
                <div class="basic-filters">
                    <div class="filter-row">
                        <div class="search-box">
                            <input
                                type="text"
                                placeholder="Search events, users, actions..."
                                class="search-input"
                                prop:value=search_term
                                on:input=move |ev| {
                                    set_search_term.set(event_target_value(&ev));
                                }
                                on:keydown=move |ev| {
                                    if ev.key() == "Enter" {
                                        apply_filters();
                                    }
                                }
                            />
                        </div>

                        <div class="date-filters">
                            <input
                                type="datetime-local"
                                placeholder="Start time"
                                class="date-input"
                                prop:value=start_date
                                on:input=move |ev| {
                                    set_start_date.set(event_target_value(&ev));
                                }
                            />
                            <input
                                type="datetime-local"
                                placeholder="End time"
                                class="date-input"
                                prop:value=end_date
                                on:input=move |ev| {
                                    set_end_date.set(event_target_value(&ev));
                                }
                            />
                        </div>

                        <div class="quick-filters">
                            <select
                                class="filter-select"
                                prop:value=selected_severity
                                on:change=move |ev| {
                                    set_selected_severity.set(event_target_value(&ev));
                                }
                            >
                                <option value="all">All Severities</option>
                                <option value="low">Low</option>
                                <option value="medium">Medium</option>
                                <option value="high">High</option>
                                <option value="critical">Critical</option>
                            </select>

                            <select
                                class="filter-select"
                                prop:value=success_filter
                                on:change=move |ev| {
                                    set_success_filter.set(event_target_value(&ev));
                                }
                            >
                                <option value="all">All Results</option>
                                <option value="success">Success Only</option>
                                <option value="failure">Failures Only</option>
                            </select>
                        </div>

                        <div class="filter-actions">
                            <button
                                class="btn btn-secondary"
                                on:click=move |_| {
                                    set_show_advanced_filters.set(!show_advanced_filters.get());
                                }
                            >
                                {move || if show_advanced_filters.get() { "Hide Advanced" } else { "Advanced" }}
                            </button>
                            <button
                                class="btn btn-primary"
                                on:click=move |_| apply_filters()
                            >
                                Apply Filters
                            </button>
                            <button
                                class="btn btn-outline"
                                on:click=move |_| clear_filters()
                            >
                                Clear
                            </button>
                        </div>
                    </div>
                </div>

                // Advanced filters
                {move || if show_advanced_filters.get() {
                    view! {
                        <div class="advanced-filters">
                            <div class="filter-section">
                                <h4>Event Types</h4>
                                <div class="checkbox-group">
                                    {move || available_event_types.get().into_iter().map(|event_type| {
                                        let event_type_for_check = event_type.clone();
                                        let event_type_for_change = event_type.clone();
                                        let event_type_for_display = event_type.clone();
                                        let is_selected = move || selected_event_types.get().contains(&event_type_for_check);

                                        view! {
                                            <label class="filter-checkbox">
                                                <input
                                                    type="checkbox"
                                                    prop:checked=is_selected
                                                    on:change=move |ev| {
                                                        let mut types = selected_event_types.get();
                                                        let et = event_type_for_change.clone();
                                                        if event_target_checked(&ev) {
                                                            if !types.contains(&et) {
                                                                types.push(et);
                                                            }
                                                        } else {
                                                            types.retain(|t| t != &event_type_for_change);
                                                        }
                                                        set_selected_event_types.set(types);
                                                    }
                                                />
                                                {event_type_for_display}
                                            </label>
                                        }
                                    }).collect::<Vec<_>>()}
                                </div>
                            </div>

                            <div class="filter-section">
                                <h4>Additional Filters</h4>
                                <div class="form-row">
                                    <div class="form-group">
                                        <label>Source IP</label>
                                        <input
                                            type="text"
                                            placeholder="192.168.1.100"
                                            class="form-control"
                                            prop:value=source_ip_filter
                                            on:input=move |ev| {
                                                set_source_ip_filter.set(event_target_value(&ev));
                                            }
                                        />
                                    </div>
                                    <div class="form-group">
                                        <label>Correlation ID</label>
                                        <input
                                            type="text"
                                            placeholder="correlation-12345"
                                            class="form-control"
                                            prop:value=correlation_id_filter
                                            on:input=move |ev| {
                                                set_correlation_id_filter.set(event_target_value(&ev));
                                            }
                                        />
                                    </div>
                                </div>
                            </div>
                        </div>
                    }.into_view()
                } else {
                    view! { <div></div> }.into_view()
                }}
            </div>

            // Results summary
            <div class="results-summary">
                <div class="summary-info">
                    Showing {move || audit_events.get().len()} of {move || total_events.get()} events
                    {move || if page.get() > 1 {
                        format!(" (page {} of {})", page.get(), (total_events.get() as f64 / page_size.get() as f64).ceil() as u64)
                    } else {
                        String::new()
                    }}
                </div>

                <div class="page-size-selector">
                    <label>Events per page:</label>
                    <select
                        class="page-size-select"
                        prop:value=page_size
                        on:change=move |ev| {
                            if let Ok(size) = event_target_value(&ev).parse::<u64>() {
                                set_page_size.set(size);
                                set_page.set(1);
                                apply_filters();
                            }
                        }
                    >
                        <option value="25">25</option>
                        <option value="50">50</option>
                        <option value="100">100</option>
                        <option value="200">200</option>
                    </select>
                </div>
            </div>

            // Audit events table
            {move || if loading.get() {
                view! {
                    <div class="loading-container">
                        <div class="spinner"></div>
                        <p>Loading audit events...</p>
                    </div>
                }.into_view()
            } else if let Some(err) = error.get() {
                view! {
                    <div class="error-container">
                        <div class="error-message">
                            <h3>Error Loading Audit Events</h3>
                            <p>{err}</p>
                            <button
                                class="btn btn-primary"
                                on:click=move |_| apply_filters()
                            >
                                Retry
                            </button>
                        </div>
                    </div>
                }.into_view()
            } else {
                view! {
                    <div class="audit-table-container">
                        {move || if audit_events.get().is_empty() {
                            view! {
                                <div class="empty-state">
                                    <h3>No audit events found</h3>
                                    <p>Try adjusting your filters or time range</p>
                                </div>
                            }.into_view()
                        } else {
                            view! {
                                <table class="audit-table">
                                    <thead>
                                        <tr>
                                            <th class="timestamp-col">Timestamp</th>
                                            <th class="severity-col">Severity</th>
                                            <th class="event-type-col">Event Type</th>
                                            <th class="user-col">User</th>
                                            <th class="action-col">Action</th>
                                            <th class="resource-col">Resource</th>
                                            <th class="status-col">Status</th>
                                            <th class="source-col">Source IP</th>
                                            <th class="actions-col">Actions</th>
                                        </tr>
                                    </thead>
                                    <tbody>
                                        {move || audit_events.get().into_iter().map(|event| {
                                            let event_clone = event.clone();
                                            view! {
                                                <tr class={format!("audit-row severity-{} {}",
                                                    event.severity,
                                                    if event.success { "success" } else { "failure" }
                                                )}>
                                                    <td class="timestamp-cell">
                                                        {event.timestamp.clone()}
                                                    </td>
                                                    <td class="severity-cell">
                                                        <span class={format!("severity-badge severity-{}", event.severity)}>
                                                            {event.severity.to_uppercase()}
                                                        </span>
                                                    </td>
                                                    <td class="event-type-cell">
                                                        {event.event_type.clone()}
                                                    </td>
                                                    <td class="user-cell">
                                                        {event.user.clone().unwrap_or_else(|| "System".to_string())}
                                                    </td>
                                                    <td class="action-cell">
                                                        {event.action.clone()}
                                                    </td>
                                                    <td class="resource-cell">
                                                        {if let (Some(resource_type), Some(resource_id)) = (&event.resource_type, &event.resource_id) {
                                                            format!("{}/{}", resource_type, resource_id)
                                                        } else {
                                                            "-".to_string()
                                                        }}
                                                    </td>
                                                    <td class="status-cell">
                                                        <span class={format!("status-indicator {}", if event.success { "success" } else { "failure" })}>
                                                            {if event.success { "[OK]" } else { "[X]" }}
                                                        </span>
                                                    </td>
                                                    <td class="source-cell">
                                                        {event.source_ip.clone().unwrap_or_else(|| "-".to_string())}
                                                    </td>
                                                    <td class="actions-cell">
                                                        <button
                                                            class="btn btn-sm btn-secondary"
                                                            on:click=move |_| {
                                                                set_selected_event.set(Some(event_clone.clone()));
                                                                set_show_event_detail.set(true);
                                                            }
                                                        >
                                                            View Details
                                                        </button>
                                                    </td>
                                                </tr>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </tbody>
                                </table>
                            }.into_view()
                        }}
                    </div>
                }.into_view()
            }}

            // Pagination
            {move || if total_events.get() > page_size.get() {
                let total_pages = (total_events.get() as f64 / page_size.get() as f64).ceil() as u64;
                view! {
                    <div class="pagination">
                        <button
                            class="pagination-btn"
                            disabled=move || page.get() <= 1
                            on:click=move |_| {
                                if page.get() > 1 {
                                    set_page.set(page.get() - 1);
                                    apply_filters();
                                }
                            }
                        >
                            Previous
                        </button>

                        <span class="pagination-info">
                            Page {move || page.get()} of {total_pages}
                        </span>

                        <button
                            class="pagination-btn"
                            disabled=move || page.get() >= total_pages
                            on:click=move |_| {
                                if page.get() < total_pages {
                                    set_page.set(page.get() + 1);
                                    apply_filters();
                                }
                            }
                        >
                            Next
                        </button>
                    </div>
                }.into_view()
            } else {
                view! { <div></div> }.into_view()
            }}

            // Event Detail Modal
            {move || if show_event_detail.get() {
                selected_event.get().map(|event| view! {
                    <div class="modal-overlay" on:click=move |_| set_show_event_detail.set(false)>
                        <div class="modal-content event-detail-modal" on:click=|e| e.stop_propagation()>
                            <div class="modal-header">
                                <h2>"Audit Event Details"</h2>
                                <button
                                    class="modal-close"
                                    on:click=move |_| set_show_event_detail.set(false)
                                >
                                    "x"
                                </button>
                            </div>

                            <div class="modal-body">
                                <div class="event-details">
                                    <div class="detail-section">
                                        <h3>Event Information</h3>
                                        <div class="detail-grid">
                                            <div class="detail-item">
                                                <span class="detail-label">Event ID:</span>
                                                <span class="detail-value">{event.id.clone()}</span>
                                            </div>
                                            <div class="detail-item">
                                                <span class="detail-label">Timestamp:</span>
                                                <span class="detail-value">{event.timestamp.clone()}</span>
                                            </div>
                                            <div class="detail-item">
                                                <span class="detail-label">Event Type:</span>
                                                <span class="detail-value">{event.event_type.clone()}</span>
                                            </div>
                                            <div class="detail-item">
                                                <span class="detail-label">Severity:</span>
                                                <span class={format!("detail-value severity-badge severity-{}", event.severity)}>
                                                    {event.severity.to_uppercase()}
                                                </span>
                                            </div>
                                            <div class="detail-item">
                                                <span class="detail-label">Action:</span>
                                                <span class="detail-value">{event.action.clone()}</span>
                                            </div>
                                            <div class="detail-item">
                                                <span class="detail-label">Success:</span>
                                                <span class={format!("detail-value status-badge {}", if event.success { "success" } else { "failure" })}>
                                                    {if event.success { "Yes" } else { "No" }}
                                                </span>
                                            </div>
                                        </div>
                                    </div>

                                    <div class="detail-section">
                                        <h3>User & Session</h3>
                                        <div class="detail-grid">
                                            <div class="detail-item">
                                                <span class="detail-label">User:</span>
                                                <span class="detail-value">{event.user.clone().unwrap_or_else(|| "System".to_string())}</span>
                                            </div>
                                            <div class="detail-item">
                                                <span class="detail-label">Source IP:</span>
                                                <span class="detail-value">{event.source_ip.clone().unwrap_or_else(|| "-".to_string())}</span>
                                            </div>
                                            <div class="detail-item">
                                                <span class="detail-label">User Agent:</span>
                                                <span class="detail-value">{event.user_agent.clone().unwrap_or_else(|| "-".to_string())}</span>
                                            </div>
                                            <div class="detail-item">
                                                <span class="detail-label">Session ID:</span>
                                                <span class="detail-value">{event.session_id.clone().unwrap_or_else(|| "-".to_string())}</span>
                                            </div>
                                        </div>
                                    </div>

                                    {if event.resource_type.is_some() || event.resource_id.is_some() {
                                        view! {
                                            <div class="detail-section">
                                                <h3>Resource</h3>
                                                <div class="detail-grid">
                                                    <div class="detail-item">
                                                        <span class="detail-label">Resource Type:</span>
                                                        <span class="detail-value">{event.resource_type.clone().unwrap_or_else(|| "-".to_string())}</span>
                                                    </div>
                                                    <div class="detail-item">
                                                        <span class="detail-label">Resource ID:</span>
                                                        <span class="detail-value">{event.resource_id.clone().unwrap_or_else(|| "-".to_string())}</span>
                                                    </div>
                                                </div>
                                            </div>
                                        }.into_view()
                                    } else {
                                        view! { <div></div> }.into_view()
                                    }}

                                    <div class="detail-section">
                                        <h3>Additional Information</h3>
                                        {event.correlation_id.as_ref().map(|corr_id| view! {
                                            <div class="detail-item">
                                                <span class="detail-label">Correlation ID:</span>
                                                <span class="detail-value">{corr_id.clone()}</span>
                                            </div>
                                        })}
                                        {event.error_message.as_ref().map(|err| view! {
                                            <div class="detail-item">
                                                <span class="detail-label">Error Message:</span>
                                                <span class="detail-value error-text">{err.clone()}</span>
                                            </div>
                                        })}
                                        {if !event.tags.is_empty() {
                                            view! {
                                                <div class="detail-item">
                                                    <span class="detail-label">Tags:</span>
                                                    <div class="detail-tags">
                                                        {event.tags.iter().map(|tag| view! {
                                                            <span class="tag">{tag.clone()}</span>
                                                        }).collect::<Vec<_>>()}
                                                    </div>
                                                </div>
                                            }.into_view()
                                        } else {
                                            view! { <div></div> }.into_view()
                                        }}
                                    </div>

                                    <div class="detail-section">
                                        <h3>Event Details</h3>
                                        <pre class="event-details-json">
                                            {serde_json::to_string_pretty(&event.details).unwrap_or_else(|_| "{}".to_string())}
                                        </pre>
                                    </div>
                                </div>
                            </div>

                            <div class="modal-footer">
                                <button
                                    class="btn btn-secondary"
                                    on:click=move |_| set_show_event_detail.set(false)
                                >
                                    Close
                                </button>
                            </div>
                        </div>
                    </div>
                })
            } else {
                None
            }}

            // Export Modal
            {move || if show_export_modal.get() {
                view! {
                    <div class="modal-overlay" on:click=move |_| set_show_export_modal.set(false)>
                        <div class="modal-content export-modal" on:click=|e| e.stop_propagation()>
                            <div class="modal-header">
                                <h2>"Export Audit Logs"</h2>
                                <button
                                    class="modal-close"
                                    on:click=move |_| set_show_export_modal.set(false)
                                >
                                    "x"
                                </button>
                            </div>

                            <div class="modal-body">
                                <div class="export-options">
                                    <div class="form-group">
                                        <label>Export Format</label>
                                        <select
                                            class="form-control"
                                            prop:value=export_format
                                            on:change=move |ev| {
                                                set_export_format.set(event_target_value(&ev));
                                            }
                                        >
                                            <option value="csv">CSV</option>
                                            <option value="json">JSON</option>
                                            <option value="pdf">PDF Report</option>
                                        </select>
                                    </div>

                                    <div class="form-group">
                                        <label>Fields to Export</label>
                                        <div class="field-checkboxes">
                                            {["timestamp", "event_type", "severity", "user", "action", "resource_type", "resource_id", "success", "source_ip", "error_message"]
                                                .iter().map(|field| {
                                                    let field_name_for_check = field.to_string();
                                                    let field_name_for_change = field.to_string();
                                                    let field_display = field.replace("_", " ");
                                                    let is_selected = move || export_fields.get().contains(&field_name_for_check);

                                                    view! {
                                                        <label class="field-checkbox">
                                                            <input
                                                                type="checkbox"
                                                                prop:checked=is_selected
                                                                on:change=move |ev| {
                                                                    let mut fields = export_fields.get();
                                                                    let fname = field_name_for_change.clone();
                                                                    if event_target_checked(&ev) {
                                                                        if !fields.contains(&fname) {
                                                                            fields.push(fname);
                                                                        }
                                                                    } else {
                                                                        fields.retain(|f| f != &field_name_for_change);
                                                                    }
                                                                    set_export_fields.set(fields);
                                                                }
                                                            />
                                                            {field_display}
                                                        </label>
                                                    }
                                                }).collect::<Vec<_>>()}
                                        </div>
                                    </div>

                                    <div class="form-group">
                                        <label class="checkbox-label">
                                            <input
                                                type="checkbox"
                                                prop:checked=include_details
                                                on:change=move |ev| {
                                                    set_include_details.set(event_target_checked(&ev));
                                                }
                                            />
                                            Include detailed event data
                                        </label>
                                        <small class="form-text">
                                            This will include the full event details JSON in the export
                                        </small>
                                    </div>
                                </div>

                                <div class="export-summary">
                                    <p>
                                        <strong>Export Summary:</strong><br/>
                                        Format: {move || export_format.get().to_uppercase()}<br/>
                                        Records: {move || total_events.get()} events (based on current filters)<br/>
                                        Fields: {move || export_fields.get().len()} selected
                                    </p>
                                </div>
                            </div>

                            <div class="modal-footer">
                                <button
                                    class="btn btn-secondary"
                                    on:click=move |_| set_show_export_modal.set(false)
                                >
                                    Cancel
                                </button>
                                <button
                                    class="btn btn-primary"
                                    on:click=move |_| export_audit_logs()
                                    disabled=move || export_fields.get().is_empty()
                                >
                                    Export
                                </button>
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

// Helper trait for string title case conversion
#[allow(dead_code)]
trait ToTitleCase {
    fn to_title_case(&self) -> String;
}

impl ToTitleCase for str {
    fn to_title_case(&self) -> String {
        self.split_whitespace()
            .map(|word| {
                let mut chars = word.chars();
                match chars.next() {
                    None => String::new(),
                    Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}