use leptos::*;
use serde::{Deserialize, Serialize};
use crate::api::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertRule {
    pub id: String,
    pub name: String,
    pub description: String,
    pub query: String,
    pub condition: String, // >, <, >=, <=, ==, !=
    pub threshold: f64,
    pub duration: String,
    pub severity: String, // critical, warning, info
    pub enabled: bool,
    pub labels: std::collections::HashMap<String, String>,
    pub annotations: std::collections::HashMap<String, String>,
    pub created_at: String,
    pub updated_at: String,
    pub last_triggered: Option<String>,
    pub notification_channels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateAlertRuleRequest {
    pub name: String,
    pub description: String,
    pub query: String,
    pub condition: String,
    pub threshold: f64,
    pub duration: String,
    pub severity: String,
    pub labels: std::collections::HashMap<String, String>,
    pub annotations: std::collections::HashMap<String, String>,
    pub notification_channels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertPreview {
    pub query_valid: bool,
    pub current_value: Option<f64>,
    pub would_trigger: bool,
    pub sample_data: Vec<MetricSample>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationChannel {
    pub id: String,
    pub name: String,
    pub channel_type: String, // email, slack, webhook, pagerduty
    pub enabled: bool,
    pub config: std::collections::HashMap<String, String>,
}

#[component]
pub fn AlertsFromMetricsPage() -> impl IntoView {
    let (alert_rules, set_alert_rules) = create_signal(Vec::<AlertRule>::new());
    let (notification_channels, set_notification_channels) = create_signal(Vec::<NotificationChannel>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);

    // Form states
    let (show_create_form, set_show_create_form) = create_signal(false);
    let (edit_rule_id, set_edit_rule_id) = create_signal(None::<String>);
    let (alert_name, set_alert_name) = create_signal(String::new());
    let (alert_description, set_alert_description) = create_signal(String::new());
    let (alert_query, set_alert_query) = create_signal(String::new());
    let (alert_condition, set_alert_condition) = create_signal(">".to_string());
    let (alert_threshold, set_alert_threshold) = create_signal(0.0);
    let (alert_duration, set_alert_duration) = create_signal("5m".to_string());
    let (alert_severity, set_alert_severity) = create_signal("warning".to_string());
    let (selected_channels, set_selected_channels) = create_signal(Vec::<String>::new());

    // Preview states
    let (show_preview, set_show_preview) = create_signal(false);
    let (preview_data, set_preview_data) = create_signal(None::<AlertPreview>);
    let (preview_loading, set_preview_loading) = create_signal(false);

    // Filter states
    let (search_term, set_search_term) = create_signal(String::new());
    let (filter_severity, set_filter_severity) = create_signal("all".to_string());
    let (filter_enabled, set_filter_enabled) = create_signal("all".to_string());

    // Load data on mount
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);

            let rules_result = get_alert_rules().await;
            let channels_result = get_notification_channels().await;

            match (rules_result, channels_result) {
                (Ok(rules), Ok(channels)) => {
                    set_alert_rules.set(rules);
                    set_notification_channels.set(channels);
                    set_error.set(None);
                }
                (Err(e), _) | (_, Err(e)) => {
                    set_error.set(Some(format!("Failed to load data: {}", e)));
                }
            }
            set_loading.set(false);
        });
    });

    // Filtered alert rules
    let filtered_rules = create_memo(move |_| {
        let mut filtered: Vec<AlertRule> = alert_rules.get()
            .into_iter()
            .filter(|rule| {
                let search_match = if search_term.get().is_empty() {
                    true
                } else {
                    let term = search_term.get().to_lowercase();
                    rule.name.to_lowercase().contains(&term) ||
                    rule.description.to_lowercase().contains(&term) ||
                    rule.query.to_lowercase().contains(&term)
                };

                let severity_match = filter_severity.get() == "all" || rule.severity == filter_severity.get();
                let enabled_match = match filter_enabled.get().as_str() {
                    "enabled" => rule.enabled,
                    "disabled" => !rule.enabled,
                    _ => true,
                };

                search_match && severity_match && enabled_match
            })
            .collect();

        // Sort by severity (critical first) and name
        filtered.sort_by(|a, b| {
            let severity_order = |s: &str| match s {
                "critical" => 0,
                "warning" => 1,
                "info" => 2,
                _ => 3,
            };
            severity_order(&a.severity).cmp(&severity_order(&b.severity))
                .then(a.name.cmp(&b.name))
        });

        filtered
    });

    // Preview alert rule
    let preview_alert = move || {
        set_preview_loading.set(true);
        set_show_preview.set(true);

        let query = alert_query.get();
        let condition = alert_condition.get();
        let threshold = alert_threshold.get();

        spawn_local(async move {
            match preview_alert_rule(query, condition, threshold).await {
                Ok(preview) => set_preview_data.set(Some(preview)),
                Err(e) => {
                    set_preview_data.set(Some(AlertPreview {
                        query_valid: false,
                        current_value: None,
                        would_trigger: false,
                        sample_data: vec![],
                        error_message: Some(e.to_string()),
                    }));
                }
            }
            set_preview_loading.set(false);
        });
    };

    // Create/update alert rule
    let save_alert_action = create_action(move |_: &()| async move {
        let request = CreateAlertRuleRequest {
            name: alert_name.get(),
            description: alert_description.get(),
            query: alert_query.get(),
            condition: alert_condition.get(),
            threshold: alert_threshold.get(),
            duration: alert_duration.get(),
            severity: alert_severity.get(),
            labels: std::collections::HashMap::new(),
            annotations: std::collections::HashMap::new(),
            notification_channels: selected_channels.get(),
        };

        let result = if let Some(rule_id) = edit_rule_id.get() {
            update_alert_rule(rule_id, request).await
        } else {
            create_alert_rule(request).await
        };

        match result {
            Ok(_) => {
                // Reset form
                set_alert_name.set(String::new());
                set_alert_description.set(String::new());
                set_alert_query.set(String::new());
                set_alert_condition.set(">".to_string());
                set_alert_threshold.set(0.0);
                set_alert_duration.set("5m".to_string());
                set_alert_severity.set("warning".to_string());
                set_selected_channels.set(Vec::new());
                set_show_create_form.set(false);
                set_edit_rule_id.set(None);

                // Reload rules
                match get_alert_rules().await {
                    Ok(rules) => set_alert_rules.set(rules),
                    Err(_) => {}
                }
                true
            }
            Err(_) => false
        }
    });

    // Edit alert rule
    let edit_rule = move |rule: AlertRule| {
        set_alert_name.set(rule.name);
        set_alert_description.set(rule.description);
        set_alert_query.set(rule.query);
        set_alert_condition.set(rule.condition);
        set_alert_threshold.set(rule.threshold);
        set_alert_duration.set(rule.duration);
        set_alert_severity.set(rule.severity);
        set_selected_channels.set(rule.notification_channels);
        set_edit_rule_id.set(Some(rule.id));
        set_show_create_form.set(true);
    };

    // Delete alert rule
    let delete_rule = move |rule_id: String| {
        spawn_local(async move {
            if let Ok(_) = delete_alert_rule(rule_id).await {
                // Reload rules
                match get_alert_rules().await {
                    Ok(rules) => set_alert_rules.set(rules),
                    Err(_) => {}
                }
            }
        });
    };

    // Toggle alert rule
    let toggle_rule = move |rule_id: String, enabled: bool| {
        spawn_local(async move {
            if let Ok(_) = toggle_alert_rule(rule_id, enabled).await {
                // Reload rules
                match get_alert_rules().await {
                    Ok(rules) => set_alert_rules.set(rules),
                    Err(_) => {}
                }
            }
        });
    };

    view! {
        <div class="alerts-from-metrics-page">
            <div class="page-header">
                <h1 class="page-title">Metric-Based Alerts</h1>
                <p class="page-description">
                    Create and manage alert rules based on metrics queries
                </p>

                <div class="page-actions">
                    <button
                        class="btn btn-primary"
                        on:click=move |_| {
                            set_edit_rule_id.set(None);
                            set_show_create_form.set(true);
                        }
                    >
                        Create Alert Rule
                    </button>
                </div>
            </div>

            // Filters
            <div class="alerts-filters">
                <div class="filter-row">
                    <div class="search-box">
                        <input
                            type="text"
                            placeholder="Search alert rules..."
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
                            prop:value=filter_severity
                            on:change=move |ev| {
                                set_filter_severity.set(event_target_value(&ev));
                            }
                        >
                            <option value="all">All Severities</option>
                            <option value="critical">Critical</option>
                            <option value="warning">Warning</option>
                            <option value="info">Info</option>
                        </select>

                        <select
                            class="filter-select"
                            prop:value=filter_enabled
                            on:change=move |ev| {
                                set_filter_enabled.set(event_target_value(&ev));
                            }
                        >
                            <option value="all">All Rules</option>
                            <option value="enabled">Enabled</option>
                            <option value="disabled">Disabled</option>
                        </select>
                    </div>
                </div>
            </div>

            // Alert Rules List
            {move || if loading.get() {
                view! {
                    <div class="loading-container">
                        <div class="spinner"></div>
                        <p>Loading alert rules...</p>
                    </div>
                }.into_view()
            } else if let Some(err) = error.get() {
                view! {
                    <div class="error-container">
                        <div class="error-message">
                            <h3>Error Loading Alert Rules</h3>
                            <p>{err}</p>
                        </div>
                    </div>
                }.into_view()
            } else {
                view! {
                    <div class="alert-rules-list">
                        {move || if filtered_rules.get().is_empty() {
                            view! {
                                <div class="empty-state">
                                    <h3>No alert rules found</h3>
                                    <p>Create your first alert rule to monitor your metrics</p>
                                    <button
                                        class="btn btn-primary"
                                        on:click=move |_| {
                                            set_edit_rule_id.set(None);
                                            set_show_create_form.set(true);
                                        }
                                    >
                                        Create Alert Rule
                                    </button>
                                </div>
                            }.into_view()
                        } else {
                            filtered_rules.get().into_iter().map(|rule| {
                                let rule_clone = rule.clone();
                                let rule_clone2 = rule.clone();
                                let rule_clone3 = rule.clone();
                                view! {
                                    <div class={format!("alert-rule-card severity-{}", rule.severity)}>
                                        <div class="rule-header">
                                            <div class="rule-title">
                                                <h3>{rule.name.clone()}</h3>
                                                <div class="rule-badges">
                                                    <span class={format!("severity-badge severity-{}", rule.severity)}>
                                                        {rule.severity.to_uppercase()}
                                                    </span>
                                                    <span class={format!("status-badge {}", if rule.enabled { "enabled" } else { "disabled" })}>
                                                        {if rule.enabled { "Enabled" } else { "Disabled" }}
                                                    </span>
                                                </div>
                                            </div>
                                            <div class="rule-actions">
                                                <button
                                                    class="action-btn toggle-btn"
                                                    on:click=move |_| {
                                                        toggle_rule(rule_clone.id.clone(), !rule_clone.enabled);
                                                    }
                                                    title={if rule.enabled { "Disable rule" } else { "Enable rule" }}
                                                >
                                                    {if rule.enabled { "⏸" } else { "▶" }}
                                                </button>
                                                <button
                                                    class="action-btn edit-btn"
                                                    on:click=move |_| {
                                                        edit_rule(rule_clone2.clone());
                                                    }
                                                    title="Edit rule"
                                                >
                                                    ✏️
                                                </button>
                                                <button
                                                    class="action-btn delete-btn"
                                                    on:click=move |_| {
                                                        if web_sys::window().unwrap().confirm_with_message(&format!("Delete alert rule '{}'?", rule_clone3.name)).unwrap() {
                                                            delete_rule(rule_clone3.id.clone());
                                                        }
                                                    }
                                                    title="Delete rule"
                                                >
                                                    🗑️
                                                </button>
                                            </div>
                                        </div>

                                        <div class="rule-description">
                                            {rule.description.clone()}
                                        </div>

                                        <div class="rule-condition">
                                            <div class="condition-display">
                                                <code class="query-code">{rule.query.clone()}</code>
                                                <span class="condition-operator">{rule.condition.clone()}</span>
                                                <span class="condition-threshold">{rule.threshold.to_string()}</span>
                                                <span class="condition-duration">for {rule.duration.clone()}</span>
                                            </div>
                                        </div>

                                        <div class="rule-details">
                                            <div class="detail-item">
                                                <span class="detail-label">Channels:</span>
                                                <span class="detail-value">
                                                    {if rule.notification_channels.is_empty() {
                                                        "None".to_string()
                                                    } else {
                                                        rule.notification_channels.join(", ")
                                                    }}
                                                </span>
                                            </div>
                                            {rule.last_triggered.map(|triggered| view! {
                                                <div class="detail-item">
                                                    <span class="detail-label">Last Triggered:</span>
                                                    <span class="detail-value">{triggered}</span>
                                                </div>
                                            })}
                                            <div class="detail-item">
                                                <span class="detail-label">Updated:</span>
                                                <span class="detail-value">{rule.updated_at.clone()}</span>
                                            </div>
                                        </div>
                                    </div>
                                }
                            }).collect::<Vec<_>>().into_view()
                        }}
                    </div>
                }.into_view()
            }}

            // Create/Edit Alert Rule Modal
            {move || if show_create_form.get() {
                view! {
                    <div class="modal-overlay" on:click=move |_| set_show_create_form.set(false)>
                        <div class="modal-content alert-form-modal" on:click=|e| e.stop_propagation()>
                            <div class="modal-header">
                                <h2>
                                    {if edit_rule_id.get().is_some() { "Edit Alert Rule" } else { "Create Alert Rule" }}
                                </h2>
                                <button
                                    class="modal-close"
                                    on:click=move |_| set_show_create_form.set(false)
                                >
                                    "x"
                                </button>
                            </div>

                            <div class="modal-body">
                                <form
                                    on:submit=move |ev| {
                                        ev.prevent_default();
                                        save_alert_action.dispatch(());
                                    }
                                >
                                    <div class="form-group">
                                        <label for="alert-name">Rule Name</label>
                                        <input
                                            type="text"
                                            id="alert-name"
                                            class="form-control"
                                            prop:value=alert_name
                                            on:input=move |ev| {
                                                set_alert_name.set(event_target_value(&ev));
                                            }
                                            required
                                        />
                                    </div>

                                    <div class="form-group">
                                        <label for="alert-description">Description</label>
                                        <textarea
                                            id="alert-description"
                                            class="form-control"
                                            rows="3"
                                            prop:value=alert_description
                                            on:input=move |ev| {
                                                set_alert_description.set(event_target_value(&ev));
                                            }
                                        ></textarea>
                                    </div>

                                    <div class="form-group">
                                        <label for="alert-query">PromQL Query</label>
                                        <textarea
                                            id="alert-query"
                                            class="form-control query-textarea"
                                            rows="4"
                                            prop:value=alert_query
                                            on:input=move |ev| {
                                                set_alert_query.set(event_target_value(&ev));
                                            }
                                            placeholder="up == 0"
                                            required
                                        ></textarea>
                                        <small class="form-text">
                                            Enter a PromQL expression that returns a numeric value
                                        </small>
                                    </div>

                                    <div class="form-row">
                                        <div class="form-group">
                                            <label for="alert-condition">Condition</label>
                                            <select
                                                id="alert-condition"
                                                class="form-control"
                                                prop:value=alert_condition
                                                on:change=move |ev| {
                                                    set_alert_condition.set(event_target_value(&ev));
                                                }
                                            >
                                                <option value=">">Greater than (>)</option>
                                                <option value="<">Less than (<)</option>
                                                <option value=">=">Greater than or equal (>=)</option>
                                                <option value="<=">Less than or equal (<=)</option>
                                                <option value="==">Equal to (==)</option>
                                                <option value="!=">Not equal to (!=)</option>
                                            </select>
                                        </div>

                                        <div class="form-group">
                                            <label for="alert-threshold">Threshold</label>
                                            <input
                                                type="number"
                                                step="0.01"
                                                id="alert-threshold"
                                                class="form-control"
                                                prop:value=alert_threshold
                                                on:input=move |ev| {
                                                    if let Ok(val) = event_target_value(&ev).parse::<f64>() {
                                                        set_alert_threshold.set(val);
                                                    }
                                                }
                                                required
                                            />
                                        </div>
                                    </div>

                                    <div class="form-row">
                                        <div class="form-group">
                                            <label for="alert-duration">Duration</label>
                                            <select
                                                id="alert-duration"
                                                class="form-control"
                                                prop:value=alert_duration
                                                on:change=move |ev| {
                                                    set_alert_duration.set(event_target_value(&ev));
                                                }
                                            >
                                                <option value="1m">1 minute</option>
                                                <option value="5m">5 minutes</option>
                                                <option value="10m">10 minutes</option>
                                                <option value="15m">15 minutes</option>
                                                <option value="30m">30 minutes</option>
                                                <option value="1h">1 hour</option>
                                            </select>
                                            <small class="form-text">
                                                How long the condition must be true before alerting
                                            </small>
                                        </div>

                                        <div class="form-group">
                                            <label for="alert-severity">Severity</label>
                                            <select
                                                id="alert-severity"
                                                class="form-control"
                                                prop:value=alert_severity
                                                on:change=move |ev| {
                                                    set_alert_severity.set(event_target_value(&ev));
                                                }
                                            >
                                                <option value="info">Info</option>
                                                <option value="warning">Warning</option>
                                                <option value="critical">Critical</option>
                                            </select>
                                        </div>
                                    </div>

                                    <div class="form-group">
                                        <label>Notification Channels</label>
                                        <div class="channels-list">
                                            {move || notification_channels.get().into_iter().map(|channel| {
                                                let channel_id = channel.id.clone();
                                                let is_selected = move || selected_channels.get().contains(&channel_id);
                                                view! {
                                                    <label class="channel-checkbox">
                                                        <input
                                                            type="checkbox"
                                                            prop:checked=is_selected
                                                            on:change=move |ev| {
                                                                let mut channels = selected_channels.get();
                                                                if event_target_checked(&ev) {
                                                                    if !channels.contains(&channel_id) {
                                                                        channels.push(channel_id.clone());
                                                                    }
                                                                } else {
                                                                    channels.retain(|id| id != &channel_id);
                                                                }
                                                                set_selected_channels.set(channels);
                                                            }
                                                        />
                                                        <span class="channel-name">{channel.name.clone()}</span>
                                                        <span class="channel-type">({channel.channel_type.clone()})</span>
                                                    </label>
                                                }
                                            }).collect::<Vec<_>>()}
                                        </div>
                                    </div>

                                    <div class="form-actions">
                                        <button
                                            type="button"
                                            class="btn btn-secondary"
                                            on:click=move |_| {
                                                preview_alert();
                                            }
                                        >
                                            Preview
                                        </button>
                                        <button
                                            type="button"
                                            class="btn btn-secondary"
                                            on:click=move |_| set_show_create_form.set(false)
                                        >
                                            Cancel
                                        </button>
                                        <button
                                            type="submit"
                                            class="btn btn-primary"
                                            disabled=move || save_alert_action.pending().get()
                                        >
                                            {move || if save_alert_action.pending().get() {
                                                "Saving..."
                                            } else if edit_rule_id.get().is_some() {
                                                "Update Rule"
                                            } else {
                                                "Create Rule"
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

            // Alert Preview Modal
            {move || if show_preview.get() {
                view! {
                    <div class="modal-overlay" on:click=move |_| set_show_preview.set(false)>
                        <div class="modal-content preview-modal" on:click=|e| e.stop_propagation()>
                            <div class="modal-header">
                                <h2>Alert Rule Preview</h2>
                                <button
                                    class="modal-close"
                                    on:click=move |_| set_show_preview.set(false)
                                >
                                    "x"
                                </button>
                            </div>

                            <div class="modal-body">
                                {move || if preview_loading.get() {
                                    view! {
                                        <div class="loading-container">
                                            <div class="spinner"></div>
                                            <p>Evaluating query...</p>
                                        </div>
                                    }.into_view()
                                } else if let Some(preview) = preview_data.get() {
                                    view! {
                                        <div class="preview-results">
                                            <div class={format!("preview-status {}", if preview.query_valid { "valid" } else { "invalid" })}>
                                                <h3>
                                                    {if preview.query_valid { "✅ Query Valid" } else { "❌ Query Invalid" }}
                                                </h3>
                                                {preview.error_message.map(|err| view! {
                                                    <p class="error-text">{err}</p>
                                                })}
                                            </div>

                                            {if preview.query_valid {
                                                view! {
                                                    <div class="preview-evaluation">
                                                        <div class="current-value">
                                                            <h4>Current Value</h4>
                                                            <span class="value-display">
                                                                {preview.current_value.map(|v| v.to_string()).unwrap_or("N/A".to_string())}
                                                            </span>
                                                        </div>

                                                        <div class="trigger-status">
                                                            <h4>Would Trigger</h4>
                                                            <span class={format!("trigger-indicator {}", if preview.would_trigger { "yes" } else { "no" })}>
                                                                {if preview.would_trigger { "YES" } else { "NO" }}
                                                            </span>
                                                        </div>

                                                        <div class="condition-summary">
                                                            <code>
                                                                {format!("{} {} {} for {}",
                                                                    alert_query.get(),
                                                                    alert_condition.get(),
                                                                    alert_threshold.get(),
                                                                    alert_duration.get()
                                                                )}
                                                            </code>
                                                        </div>
                                                    </div>
                                                }.into_view()
                                            } else {
                                                view! { <div></div> }.into_view()
                                            }}
                                        </div>
                                    }.into_view()
                                } else {
                                    view! { <div></div> }.into_view()
                                }}
                            </div>

                            <div class="modal-footer">
                                <button
                                    class="btn btn-secondary"
                                    on:click=move |_| set_show_preview.set(false)
                                >
                                    Close
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