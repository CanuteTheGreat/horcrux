use leptos::*;
use crate::api::*;

// All types imported from crate::api::* (SecurityEvent, SecurityIndicator, SecurityThreat, SecurityStats)

#[component]
pub fn SecurityEventsPage() -> impl IntoView {
    let (security_events, set_security_events) = create_signal(Vec::<SecurityEvent>::new());
    let (active_threats, set_active_threats) = create_signal(Vec::<SecurityThreat>::new());
    let (stats, set_stats) = create_signal(None::<SecurityStats>);
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);

    // Filter states
    let (search_term, set_search_term) = create_signal(String::new());
    let (filter_severity, set_filter_severity) = create_signal("all".to_string());
    let (filter_status, set_filter_status) = create_signal("all".to_string());
    let (filter_event_type, set_filter_event_type) = create_signal("all".to_string());
    let (time_range, set_time_range) = create_signal("24h".to_string());

    // Modal states
    let (selected_event, set_selected_event) = create_signal(None::<SecurityEvent>);
    let (show_event_detail, set_show_event_detail) = create_signal(false);
    let (show_block_ip_modal, set_show_block_ip_modal) = create_signal(false);
    let (ip_to_block, set_ip_to_block) = create_signal(String::new());

    // Current view tab
    let (current_tab, set_current_tab) = create_signal("events".to_string());

    // Load data on mount
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);

            let events_result = get_security_events(time_range.get()).await;
            let threats_result = get_active_threats().await;
            let stats_result = get_security_stats().await;

            match (events_result, threats_result, stats_result) {
                (Ok(events), Ok(threats), Ok(statistics)) => {
                    set_security_events.set(events);
                    set_active_threats.set(threats);
                    set_stats.set(Some(statistics));
                    set_error.set(None);
                }
                (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
                    set_error.set(Some(format!("Failed to load security data: {}", e)));
                }
            }
            set_loading.set(false);
        });
    });

    // Filtered events
    let filtered_events = create_memo(move |_| {
        security_events.get()
            .into_iter()
            .filter(|event| {
                let search_match = if search_term.get().is_empty() {
                    true
                } else {
                    let term = search_term.get().to_lowercase();
                    event.description.to_lowercase().contains(&term) ||
                    event.source_ip.to_lowercase().contains(&term) ||
                    event.target_user.as_ref().map(|u| u.to_lowercase().contains(&term)).unwrap_or(false)
                };

                let severity_match = filter_severity.get() == "all" || event.severity == filter_severity.get();
                let status_match = filter_status.get() == "all" || event.status == filter_status.get();
                let type_match = filter_event_type.get() == "all" || event.event_type == filter_event_type.get();

                search_match && severity_match && status_match && type_match
            })
            .collect::<Vec<_>>()
    });

    // Update event status
    let update_event_status = move |event_id: String, new_status: String| {
        spawn_local(async move {
            if let Ok(_) = update_security_event_status(event_id, new_status).await {
                // Reload events
                if let Ok(events) = get_security_events(time_range.get()).await {
                    set_security_events.set(events);
                }
            }
        });
    };

    // Block IP address
    let block_ip = move || {
        let ip = ip_to_block.get();
        spawn_local(async move {
            if let Ok(_) = block_ip_address(ip).await {
                set_show_block_ip_modal.set(false);
                set_ip_to_block.set(String::new());
                // Reload stats
                if let Ok(statistics) = get_security_stats().await {
                    set_stats.set(Some(statistics));
                }
            }
        });
    };

    view! {
        <div class="security-events-page">
            <div class="page-header">
                <h1 class="page-title">Security Events</h1>
                <p class="page-description">
                    Monitor and investigate security threats and anomalies across your infrastructure
                </p>

                <div class="page-tabs">
                    <button
                        class={move || if current_tab.get() == "events" { "tab-btn active" } else { "tab-btn" }}
                        on:click=move |_| set_current_tab.set("events".to_string())
                    >
                        Events
                    </button>
                    <button
                        class={move || if current_tab.get() == "threats" { "tab-btn active" } else { "tab-btn" }}
                        on:click=move |_| set_current_tab.set("threats".to_string())
                    >
                        Active Threats ({move || active_threats.get().len()})
                    </button>
                    <button
                        class={move || if current_tab.get() == "dashboard" { "tab-btn active" } else { "tab-btn" }}
                        on:click=move |_| set_current_tab.set("dashboard".to_string())
                    >
                        Dashboard
                    </button>
                </div>
            </div>

            // Security Stats Summary (always visible)
            {move || stats.get().map(|s| view! {
                <div class="security-stats-summary">
                    <div class="stat-card critical">
                        <span class="stat-value">{s.critical_events_24h.to_string()}</span>
                        <span class="stat-label">Critical Events (24h)</span>
                    </div>
                    <div class="stat-card warning">
                        <span class="stat-value">{s.total_events_24h.to_string()}</span>
                        <span class="stat-label">Total Events (24h)</span>
                    </div>
                    <div class="stat-card info">
                        <span class="stat-value">{s.failed_logins_24h.to_string()}</span>
                        <span class="stat-label">Failed Logins (24h)</span>
                    </div>
                    <div class="stat-card danger">
                        <span class="stat-value">{s.blocked_ips_24h.to_string()}</span>
                        <span class="stat-label">Blocked IPs (24h)</span>
                    </div>
                    <div class="stat-card alert">
                        <span class="stat-value">{s.active_threats.to_string()}</span>
                        <span class="stat-label">Active Threats</span>
                    </div>
                </div>
            })}

            {move || match current_tab.get().as_str() {
                "events" => view! {
                    <div class="events-view">
                        // Filters
                        <div class="security-filters">
                            <div class="filter-row">
                                <div class="search-box">
                                    <input
                                        type="text"
                                        placeholder="Search events..."
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
                                                if let Ok(events) = get_security_events(range).await {
                                                    set_security_events.set(events);
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
                                        prop:value=filter_severity
                                        on:change=move |ev| {
                                            set_filter_severity.set(event_target_value(&ev));
                                        }
                                    >
                                        <option value="all">All Severities</option>
                                        <option value="critical">Critical</option>
                                        <option value="high">High</option>
                                        <option value="medium">Medium</option>
                                        <option value="low">Low</option>
                                    </select>

                                    <select
                                        class="filter-select"
                                        prop:value=filter_status
                                        on:change=move |ev| {
                                            set_filter_status.set(event_target_value(&ev));
                                        }
                                    >
                                        <option value="all">All Status</option>
                                        <option value="new">New</option>
                                        <option value="investigating">Investigating</option>
                                        <option value="resolved">Resolved</option>
                                        <option value="false_positive">False Positive</option>
                                    </select>

                                    <select
                                        class="filter-select"
                                        prop:value=filter_event_type
                                        on:change=move |ev| {
                                            set_filter_event_type.set(event_target_value(&ev));
                                        }
                                    >
                                        <option value="all">All Types</option>
                                        <option value="brute_force">Brute Force</option>
                                        <option value="unauthorized_access">Unauthorized Access</option>
                                        <option value="privilege_escalation">Privilege Escalation</option>
                                        <option value="suspicious_activity">Suspicious Activity</option>
                                        <option value="malware">Malware</option>
                                        <option value="data_exfiltration">Data Exfiltration</option>
                                    </select>
                                </div>
                            </div>
                        </div>

                        // Events List
                        {move || if loading.get() {
                            view! {
                                <div class="loading-container">
                                    <div class="spinner"></div>
                                    <p>Loading security events...</p>
                                </div>
                            }.into_view()
                        } else if let Some(err) = error.get() {
                            view! {
                                <div class="error-container">
                                    <div class="error-message">
                                        <h3>Error Loading Security Events</h3>
                                        <p>{err}</p>
                                    </div>
                                </div>
                            }.into_view()
                        } else {
                            view! {
                                <div class="events-list">
                                    {move || if filtered_events.get().is_empty() {
                                        view! {
                                            <div class="empty-state">
                                                <h3>No security events found</h3>
                                                <p>No events match your current filters</p>
                                            </div>
                                        }.into_view()
                                    } else {
                                        filtered_events.get().into_iter().map(|event| {
                                            let event_clone = event.clone();
                                            let event_clone2 = event.clone();
                                            let event_id_investigate = event.id.clone();
                                            let event_id_resolve = event.id.clone();
                                            let event_id_false_positive = event.id.clone();
                                            view! {
                                                <div class={format!("security-event-card severity-{}", event.severity)}>
                                                    <div class="event-header">
                                                        <div class="event-type">
                                                            <span class={format!("severity-badge severity-{}", event.severity)}>
                                                                {event.severity.to_uppercase()}
                                                            </span>
                                                            <span class="event-type-badge">
                                                                {event.event_type.replace("_", " ")}
                                                            </span>
                                                        </div>
                                                        <div class="event-meta">
                                                            <span class="event-time">{event.timestamp.clone()}</span>
                                                            <span class={format!("status-badge status-{}", event.status)}>
                                                                {event.status.replace("_", " ")}
                                                            </span>
                                                        </div>
                                                    </div>

                                                    <div class="event-description">
                                                        {event.description.clone()}
                                                    </div>

                                                    <div class="event-details">
                                                        <div class="detail-item">
                                                            <span class="label">Source IP:</span>
                                                            <span class="value ip-address">{event.source_ip.clone()}</span>
                                                            <button
                                                                class="btn btn-sm btn-danger"
                                                                on:click=move |_| {
                                                                    set_ip_to_block.set(event_clone.source_ip.clone());
                                                                    set_show_block_ip_modal.set(true);
                                                                }
                                                            >
                                                                Block IP
                                                            </button>
                                                        </div>
                                                        {event.target_user.as_ref().map(|user| view! {
                                                            <div class="detail-item">
                                                                <span class="label">Target User:</span>
                                                                <span class="value">{user.clone()}</span>
                                                            </div>
                                                        })}
                                                        {event.target_resource.as_ref().map(|resource| view! {
                                                            <div class="detail-item">
                                                                <span class="label">Target Resource:</span>
                                                                <span class="value">{resource.clone()}</span>
                                                            </div>
                                                        })}
                                                    </div>

                                                    <div class="event-actions">
                                                        <button
                                                            class="btn btn-secondary"
                                                            on:click=move |_| {
                                                                set_selected_event.set(Some(event_clone2.clone()));
                                                                set_show_event_detail.set(true);
                                                            }
                                                        >
                                                            View Details
                                                        </button>

                                                        {if event.status == "new" {
                                                            view! {
                                                                <button
                                                                    class="btn btn-primary"
                                                                    on:click=move |_| {
                                                                        update_event_status(event_id_investigate.clone(), "investigating".to_string());
                                                                    }
                                                                >
                                                                    Investigate
                                                                </button>
                                                            }.into_view()
                                                        } else if event.status == "investigating" {
                                                            view! {
                                                                <div class="status-buttons">
                                                                    <button
                                                                        class="btn btn-success"
                                                                        on:click=move |_| {
                                                                            update_event_status(event_id_resolve.clone(), "resolved".to_string());
                                                                        }
                                                                    >
                                                                        Resolve
                                                                    </button>
                                                                    <button
                                                                        class="btn btn-outline"
                                                                        on:click=move |_| {
                                                                            update_event_status(event_id_false_positive.clone(), "false_positive".to_string());
                                                                        }
                                                                    >
                                                                        False Positive
                                                                    </button>
                                                                </div>
                                                            }.into_view()
                                                        } else {
                                                            view! { <div></div> }.into_view()
                                                        }}
                                                    </div>
                                                </div>
                                            }
                                        }).collect::<Vec<_>>().into_view()
                                    }}
                                </div>
                            }.into_view()
                        }}
                    </div>
                }.into_view(),

                "threats" => view! {
                    <div class="threats-view">
                        <h2>Active Threats</h2>
                        {move || if active_threats.get().is_empty() {
                            view! {
                                <div class="empty-state success">
                                    <h3>No Active Threats</h3>
                                    <p>All detected threats have been resolved</p>
                                </div>
                            }.into_view()
                        } else {
                            view! {
                                <div class="threats-list">
                                    {active_threats.get().into_iter().map(|threat| {
                                        view! {
                                            <div class={format!("threat-card severity-{}", threat.severity)}>
                                                <div class="threat-header">
                                                    <h3>{threat.name.clone()}</h3>
                                                    <span class={format!("severity-badge severity-{}", threat.severity)}>
                                                        {threat.severity.to_uppercase()}
                                                    </span>
                                                </div>

                                                <p class="threat-description">{threat.description.clone()}</p>

                                                <div class="threat-details">
                                                    <div class="detail-item">
                                                        <span class="label">Detection Time:</span>
                                                        <span class="value">{threat.detection_time.clone()}</span>
                                                    </div>
                                                    <div class="detail-item">
                                                        <span class="label">Affected Events:</span>
                                                        <span class="value">{threat.affected_events.len().to_string()} events</span>
                                                    </div>
                                                    <div class="detail-item">
                                                        <span class="label">Status:</span>
                                                        <span class={format!("status-badge status-{}", threat.status)}>
                                                            {threat.status.clone()}
                                                        </span>
                                                    </div>
                                                </div>

                                                {if !threat.mitigations.is_empty() {
                                                    view! {
                                                        <div class="threat-mitigations">
                                                            <h4>Recommended Mitigations</h4>
                                                            <ul>
                                                                {threat.mitigations.iter().map(|mitigation| view! {
                                                                    <li>{mitigation.clone()}</li>
                                                                }).collect::<Vec<_>>()}
                                                            </ul>
                                                        </div>
                                                    }.into_view()
                                                } else {
                                                    view! { <div></div> }.into_view()
                                                }}
                                            </div>
                                        }
                                    }).collect::<Vec<_>>()}
                                </div>
                            }.into_view()
                        }}
                    </div>
                }.into_view(),

                "dashboard" => view! {
                    <div class="security-dashboard-view">
                        {move || stats.get().map(|s| view! {
                            <div class="dashboard-grid">
                                <div class="dashboard-section">
                                    <h3>Events by Type</h3>
                                    <div class="chart-container">
                                        {s.events_by_type.iter().map(|(event_type, count)| view! {
                                            <div class="chart-bar">
                                                <span class="bar-label">{event_type.replace("_", " ")}</span>
                                                <div class="bar-container">
                                                    <div
                                                        class="bar-fill"
                                                        style={format!("width: {}%", (*count as f64 / s.total_events_24h as f64 * 100.0).min(100.0))}
                                                    ></div>
                                                </div>
                                                <span class="bar-value">{count.to_string()}</span>
                                            </div>
                                        }).collect::<Vec<_>>()}
                                    </div>
                                </div>

                                <div class="dashboard-section">
                                    <h3>Events by Severity</h3>
                                    <div class="severity-breakdown">
                                        {s.events_by_severity.iter().map(|(severity, count)| view! {
                                            <div class={format!("severity-item severity-{}", severity)}>
                                                <span class="severity-label">{severity.to_uppercase()}</span>
                                                <span class="severity-count">{count.to_string()}</span>
                                            </div>
                                        }).collect::<Vec<_>>()}
                                    </div>
                                </div>

                                <div class="dashboard-section">
                                    <h3>Top Source IPs</h3>
                                    <table class="ip-table">
                                        <thead>
                                            <tr>
                                                <th>IP Address</th>
                                                <th>Events</th>
                                                <th>Action</th>
                                            </tr>
                                        </thead>
                                        <tbody>
                                            {s.top_source_ips.iter().map(|(ip, count)| {
                                                let ip_clone = ip.clone();
                                                view! {
                                                    <tr>
                                                        <td class="ip-address">{ip.clone()}</td>
                                                        <td>{count.to_string()}</td>
                                                        <td>
                                                            <button
                                                                class="btn btn-sm btn-danger"
                                                                on:click=move |_| {
                                                                    set_ip_to_block.set(ip_clone.clone());
                                                                    set_show_block_ip_modal.set(true);
                                                                }
                                                            >
                                                                Block
                                                            </button>
                                                        </td>
                                                    </tr>
                                                }
                                            }).collect::<Vec<_>>()}
                                        </tbody>
                                    </table>
                                </div>
                            </div>
                        })}
                    </div>
                }.into_view(),

                _ => view! { <div></div> }.into_view()
            }}

            // Event Detail Modal
            {move || if show_event_detail.get() {
                selected_event.get().map(|event| view! {
                    <div class="modal-overlay" on:click=move |_| set_show_event_detail.set(false)>
                        <div class="modal-content event-detail-modal" on:click=|e| e.stop_propagation()>
                            <div class="modal-header">
                                <h2>Security Event Details</h2>
                                <button
                                    class="modal-close"
                                    on:click=move |_| set_show_event_detail.set(false)
                                >
                                    "x"
                                </button>
                            </div>

                            <div class="modal-body">
                                <div class="event-full-details">
                                    <div class="detail-section">
                                        <h3>Event Information</h3>
                                        <div class="detail-grid">
                                            <div class="detail-item">
                                                <span class="label">Event ID:</span>
                                                <span class="value">{event.id.clone()}</span>
                                            </div>
                                            <div class="detail-item">
                                                <span class="label">Type:</span>
                                                <span class="value">{event.event_type.replace("_", " ")}</span>
                                            </div>
                                            <div class="detail-item">
                                                <span class="label">Severity:</span>
                                                <span class={format!("value severity-badge severity-{}", event.severity)}>
                                                    {event.severity.to_uppercase()}
                                                </span>
                                            </div>
                                            <div class="detail-item">
                                                <span class="label">Status:</span>
                                                <span class={format!("value status-badge status-{}", event.status)}>
                                                    {event.status.replace("_", " ")}
                                                </span>
                                            </div>
                                            <div class="detail-item">
                                                <span class="label">Timestamp:</span>
                                                <span class="value">{event.timestamp.clone()}</span>
                                            </div>
                                        </div>
                                    </div>

                                    <div class="detail-section">
                                        <h3>Description</h3>
                                        <p>{event.description.clone()}</p>
                                    </div>

                                    {if !event.indicators.is_empty() {
                                        view! {
                                            <div class="detail-section">
                                                <h3>Security Indicators</h3>
                                                <div class="indicators-list">
                                                    {event.indicators.iter().map(|indicator| view! {
                                                        <div class="indicator-item">
                                                            <div class="indicator-header">
                                                                <span class="indicator-type">{indicator.indicator_type.clone()}</span>
                                                                <span class="indicator-confidence">
                                                                    {format!("{:.0}% confidence", indicator.confidence * 100.0)}
                                                                </span>
                                                            </div>
                                                            <div class="indicator-value">{indicator.value.clone()}</div>
                                                            <div class="indicator-description">{indicator.description.clone()}</div>
                                                        </div>
                                                    }).collect::<Vec<_>>()}
                                                </div>
                                            </div>
                                        }.into_view()
                                    } else {
                                        view! { <div></div> }.into_view()
                                    }}

                                    <div class="detail-section">
                                        <h3>Raw Details</h3>
                                        <pre class="event-json">
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

            // Block IP Modal
            {move || if show_block_ip_modal.get() {
                view! {
                    <div class="modal-overlay" on:click=move |_| set_show_block_ip_modal.set(false)>
                        <div class="modal-content block-ip-modal" on:click=|e| e.stop_propagation()>
                            <div class="modal-header">
                                <h2>Block IP Address</h2>
                                <button
                                    class="modal-close"
                                    on:click=move |_| set_show_block_ip_modal.set(false)
                                >
                                    "x"
                                </button>
                            </div>

                            <div class="modal-body">
                                <p>Are you sure you want to block the following IP address?</p>
                                <div class="ip-display">
                                    <code>{ip_to_block.get()}</code>
                                </div>
                                <p class="warning-text">
                                    This will immediately block all traffic from this IP address.
                                    This action can be undone from the firewall settings.
                                </p>
                            </div>

                            <div class="modal-footer">
                                <button
                                    class="btn btn-secondary"
                                    on:click=move |_| set_show_block_ip_modal.set(false)
                                >
                                    Cancel
                                </button>
                                <button
                                    class="btn btn-danger"
                                    on:click=move |_| block_ip()
                                >
                                    Block IP
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