use leptos::*;
use wasm_bindgen::JsCast;
use crate::api::*;
use web_sys::MouseEvent;

#[component]
pub fn AlertCenterPage() -> impl IntoView {
    let (alert_rules, set_alert_rules) = create_signal(Vec::<AlertRule>::new());
    let (active_alerts, set_active_alerts) = create_signal(Vec::<ActiveAlert>::new());
    let (notification_channels, set_notification_channels) = create_signal(Vec::<NotificationChannel>::new());
    let (selected_rule, set_selected_rule) = create_signal(None::<AlertRule>);
    let (show_create_modal, set_show_create_modal) = create_signal(false);
    let (show_edit_modal, set_show_edit_modal) = create_signal(false);
    let (show_test_modal, set_show_test_modal) = create_signal(false);
    let (loading, set_loading) = create_signal(false);
    let (error_message, set_error_message) = create_signal(None::<String>);
    let (active_tab, set_active_tab) = create_signal("rules".to_string());

    // Alert rule form
    let (form_name, set_form_name) = create_signal(String::new());
    let (form_description, set_form_description) = create_signal(String::new());
    let (form_metric, set_form_metric) = create_signal(String::new());
    let (form_condition, set_form_condition) = create_signal("greater_than".to_string());
    let (form_threshold, set_form_threshold) = create_signal(0.0);
    let (form_duration, set_form_duration) = create_signal(300); // 5 minutes
    let (form_severity, set_form_severity) = create_signal("warning".to_string());
    let (form_enabled, set_form_enabled) = create_signal(true);
    let (form_labels, set_form_labels) = create_signal(String::new());
    let (form_annotations, set_form_annotations) = create_signal(String::new());
    let (selected_channels, set_selected_channels) = create_signal(Vec::<String>::new());

    // Helper functions - defined early so actions can use them
    let clear_form = move || {
        set_form_name.set(String::new());
        set_form_description.set(String::new());
        set_form_metric.set(String::new());
        set_form_condition.set("greater_than".to_string());
        set_form_threshold.set(0.0);
        set_form_duration.set(300);
        set_form_severity.set("warning".to_string());
        set_form_enabled.set(true);
        set_form_labels.set(String::new());
        set_form_annotations.set(String::new());
        set_selected_channels.set(Vec::new());
    };

    let parse_key_value_pairs = |text: &str| -> std::collections::HashMap<String, String> {
        text.lines()
            .filter_map(|line| {
                let parts: Vec<&str> = line.split('=').collect();
                if parts.len() == 2 {
                    Some((parts[0].trim().to_string(), parts[1].trim().to_string()))
                } else {
                    None
                }
            })
            .collect()
    };

    let format_key_value_pairs = |map: &std::collections::HashMap<String, String>| -> String {
        map.iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("\n")
    };

    // Load data
    let load_data = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);

        match get_alert_rules().await {
            Ok(rules) => set_alert_rules.set(rules),
            Err(e) => set_error_message.set(Some(format!("Failed to load alert rules: {}", e))),
        }

        match get_active_alerts().await {
            Ok(alerts) => set_active_alerts.set(alerts),
            Err(e) => set_error_message.set(Some(format!("Failed to load active alerts: {}", e))),
        }

        match get_notification_channels().await {
            Ok(channels) => set_notification_channels.set(channels),
            Err(_) => {}
        }

        set_loading.set(false);
    });

    // Create alert rule
    let create_rule = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);

        let new_rule = AlertRule {
            id: format!("rule-{}", chrono::Utc::now().timestamp()),
            name: form_name.get(),
            description: if form_description.get().is_empty() { None } else { Some(form_description.get()) },
            metric: form_metric.get(),
            condition: form_condition.get(),
            threshold: form_threshold.get(),
            duration_seconds: form_duration.get(),
            severity: form_severity.get(),
            enabled: form_enabled.get(),
            labels: parse_key_value_pairs(&form_labels.get()),
            annotations: parse_key_value_pairs(&form_annotations.get()),
            notification_channels: selected_channels.get(),
            created_at: chrono::Utc::now(),
            last_triggered: None,
            trigger_count: 0,
        };

        match create_alert_rule(new_rule).await {
            Ok(_) => {
                set_show_create_modal.set(false);
                clear_form();
                load_data.dispatch(());
            }
            Err(e) => set_error_message.set(Some(format!("Failed to create alert rule: {}", e))),
        }

        set_loading.set(false);
    });

    // Edit alert rule
    let edit_rule = create_action(move |_: &()| async move {
        if let Some(rule) = selected_rule.get() {
            set_loading.set(true);
            set_error_message.set(None);

            let updated_rule = AlertRule {
                id: rule.id.clone(),
                name: form_name.get(),
                description: if form_description.get().is_empty() { None } else { Some(form_description.get()) },
                metric: form_metric.get(),
                condition: form_condition.get(),
                threshold: form_threshold.get(),
                duration_seconds: form_duration.get(),
                severity: form_severity.get(),
                enabled: form_enabled.get(),
                labels: parse_key_value_pairs(&form_labels.get()),
                annotations: parse_key_value_pairs(&form_annotations.get()),
                notification_channels: selected_channels.get(),
                created_at: rule.created_at,
                last_triggered: rule.last_triggered,
                trigger_count: rule.trigger_count,
            };

            match update_alert_rule(updated_rule).await {
                Ok(_) => {
                    set_show_edit_modal.set(false);
                    set_selected_rule.set(None);
                    clear_form();
                    load_data.dispatch(());
                }
                Err(e) => set_error_message.set(Some(format!("Failed to update alert rule: {}", e))),
            }

            set_loading.set(false);
        }
    });

    // Delete alert rule
    let delete_rule = create_action(move |rule_id: &String| {
        let rule_id = rule_id.clone();
        async move {
            set_loading.set(true);
            set_error_message.set(None);

            match delete_alert_rule(rule_id).await {
                Ok(_) => load_data.dispatch(()),
                Err(e) => set_error_message.set(Some(format!("Failed to delete alert rule: {}", e))),
            }

            set_loading.set(false);
        }
    });

    // Test alert rule
    let test_rule = create_action(move |rule_id: &String| {
        let rule_id = rule_id.clone();
        async move {
            set_loading.set(true);
            set_error_message.set(None);

            match test_alert_rule(rule_id).await {
                Ok(_) => {
                    set_show_test_modal.set(false);
                    // Show success message
                }
                Err(e) => set_error_message.set(Some(format!("Failed to test alert rule: {}", e))),
            }

            set_loading.set(false);
        }
    });

    // Acknowledge alert
    let acknowledge_alert = create_action(move |alert_id: &String| {
        let alert_id = alert_id.clone();
        async move {
            match acknowledge_alert(alert_id).await {
                Ok(_) => load_data.dispatch(()),
                Err(e) => set_error_message.set(Some(format!("Failed to acknowledge alert: {}", e))),
            }
        }
    });

    // Silence alert
    let silence_alert = create_action(move |(alert_id, duration): &(String, u32)| {
        let alert_id = alert_id.clone();
        let duration = *duration;
        async move {
            match silence_alert(alert_id, duration).await {
                Ok(_) => load_data.dispatch(()),
                Err(e) => set_error_message.set(Some(format!("Failed to silence alert: {}", e))),
            }
        }
    });

    // Helper functions
    let init_form_with_rule = move |rule: &AlertRule| {
        set_form_name.set(rule.name.clone());
        set_form_description.set(rule.description.clone().unwrap_or_default());
        set_form_metric.set(rule.metric.clone());
        set_form_condition.set(rule.condition.clone());
        set_form_threshold.set(rule.threshold);
        set_form_duration.set(rule.duration_seconds);
        set_form_severity.set(rule.severity.clone());
        set_form_enabled.set(rule.enabled);
        set_form_labels.set(format_key_value_pairs(&rule.labels));
        set_form_annotations.set(format_key_value_pairs(&rule.annotations));
        set_selected_channels.set(rule.notification_channels.clone());
    };

    let get_severity_color = |severity: &str| match severity {
        "critical" => "text-red-600 bg-red-100",
        "warning" => "text-yellow-600 bg-yellow-100",
        "info" => "text-blue-600 bg-blue-100",
        _ => "text-gray-600 bg-gray-100",
    };

    let get_alert_status_color = |status: &str| match status {
        "firing" => "text-red-600 bg-red-100",
        "pending" => "text-yellow-600 bg-yellow-100",
        "resolved" => "text-green-600 bg-green-100",
        "silenced" => "text-gray-600 bg-gray-100",
        _ => "text-gray-600 bg-gray-100",
    };

    // Initial load
    create_effect(move |_| {
        load_data.dispatch(());
    });

    view! {
        <div class="alert-center-page">
            <div class="page-header">
                <h1>"Alert Center"</h1>
                <div class="header-actions">
                    <button
                        class="btn btn-primary"
                        on:click=move |_| set_show_create_modal.set(true)
                        disabled=loading
                    >
                        "Create Alert Rule"
                    </button>
                    <button
                        class="btn btn-secondary"
                        on:click=move |_| load_data.dispatch(())
                        disabled=loading
                    >
                        "Refresh"
                    </button>
                </div>
            </div>

            {move || error_message.get().map(|msg| view! {
                <div class="alert alert-error">{msg}</div>
            })}

            <div class="alert-tabs">
                <div class="tab-buttons">
                    <button
                        class={move || if active_tab.get() == "rules" { "tab-button active" } else { "tab-button" }}
                        on:click=move |_| set_active_tab.set("rules".to_string())
                    >
                        "Alert Rules ("{alert_rules.get().len()}")"
                    </button>
                    <button
                        class={move || if active_tab.get() == "active" { "tab-button active" } else { "tab-button" }}
                        on:click=move |_| set_active_tab.set("active".to_string())
                    >
                        "Active Alerts ("{active_alerts.get().len()}")"
                    </button>
                    <button
                        class={move || if active_tab.get() == "history" { "tab-button active" } else { "tab-button" }}
                        on:click=move |_| set_active_tab.set("history".to_string())
                    >
                        "Alert History"
                    </button>
                </div>

                <div class="tab-content">
                    {move || match active_tab.get().as_str() {
                        "rules" => view! {
                            <div class="alert-rules-section">
                                {move || if loading.get() {
                                    view! { <div class="loading">"Loading alert rules..."</div> }.into_view()
                                } else if alert_rules.get().is_empty() {
                                    view! { <div class="empty-state">"No alert rules configured"</div> }.into_view()
                                } else {
                                    view! {
                                        <div class="alert-rules-grid">
                                            {alert_rules.get().into_iter().map(|rule| {
                                                let rule_clone = rule.clone();
                                                let rule_clone2 = rule.clone();
                                                let rule_clone3 = rule.clone();
                                                let name = rule.name.clone();
                                                let severity = rule.severity.clone();
                                                let severity_color = get_severity_color(&rule.severity);
                                                let enabled = rule.enabled;
                                                let enabled_text = if enabled { "Enabled" } else { "Disabled" };
                                                let enabled_class = if enabled { "status-enabled" } else { "status-disabled" };
                                                let description = rule.description.clone();
                                                let metric = rule.metric.clone();
                                                let condition = rule.condition.clone();
                                                let threshold = rule.threshold;
                                                let duration_seconds = rule.duration_seconds;
                                                let notification_count = rule.notification_channels.len();
                                                let trigger_count = rule.trigger_count;
                                                let labels = rule.labels.clone();

                                                view! {
                                                    <div class="alert-rule-card">
                                                        <div class="card-header">
                                                            <div class="rule-info">
                                                                <h3>{name}</h3>
                                                                <span class={format!("severity-badge {}", severity_color)}>
                                                                    {severity}
                                                                </span>
                                                                <span class=enabled_class>
                                                                    {enabled_text}
                                                                </span>
                                                            </div>
                                                            <div class="card-actions">
                                                                <button
                                                                    class="btn btn-sm btn-secondary"
                                                                    on:click=move |_| {
                                                                        set_selected_rule.set(Some(rule_clone.clone()));
                                                                        init_form_with_rule(&rule_clone);
                                                                        set_show_edit_modal.set(true);
                                                                    }
                                                                >
                                                                    "Edit"
                                                                </button>
                                                                <button
                                                                    class="btn btn-sm btn-primary"
                                                                    on:click=move |_| {
                                                                        set_selected_rule.set(Some(rule_clone2.clone()));
                                                                        set_show_test_modal.set(true);
                                                                    }
                                                                >
                                                                    "Test"
                                                                </button>
                                                                <button
                                                                    class="btn btn-sm btn-danger"
                                                                    on:click=move |_| {
                                                                        if web_sys::window()
                                                                            .unwrap()
                                                                            .confirm_with_message(&format!("Delete alert rule '{}'?", rule_clone3.name))
                                                                            .unwrap_or(false)
                                                                        {
                                                                            delete_rule.dispatch(rule_clone3.id.clone());
                                                                        }
                                                                    }
                                                                >
                                                                    "Delete"
                                                                </button>
                                                            </div>
                                                        </div>
                                                        <div class="card-content">
                                                            {description.map(|desc| view! {
                                                                <div class="rule-description">{desc}</div>
                                                            })}

                                                            <div class="rule-details">
                                                                <div class="detail-row">
                                                                    <span class="label">"Metric:"</span>
                                                                    <code class="metric-name">{metric}</code>
                                                                </div>
                                                                <div class="detail-row">
                                                                    <span class="label">"Condition:"</span>
                                                                    <span class="value">{condition}" "{threshold}</span>
                                                                </div>
                                                                <div class="detail-row">
                                                                    <span class="label">"Duration:"</span>
                                                                    <span class="value">{duration_seconds}"s"</span>
                                                                </div>
                                                                <div class="detail-row">
                                                                    <span class="label">"Notifications:"</span>
                                                                    <span class="value">{notification_count}" channels"</span>
                                                                </div>
                                                                {if trigger_count > 0 {
                                                                    view! {
                                                                        <div class="detail-row">
                                                                            <span class="label">"Triggered:"</span>
                                                                            <span class="value">{trigger_count}" times"</span>
                                                                        </div>
                                                                    }.into_view()
                                                                } else {
                                                                    view! {}.into_view()
                                                                }}
                                                            </div>

                                                            {if !labels.is_empty() {
                                                                view! {
                                                                    <div class="rule-labels">
                                                                        <h4>"Labels:"</h4>
                                                                        <div class="label-tags">
                                                                            {labels.iter().map(|(key, value)| view! {
                                                                                <span class="label-tag">{format!("{}={}", key, value)}</span>
                                                                            }).collect::<Vec<_>>()}
                                                                        </div>
                                                                    </div>
                                                                }.into_view()
                                                            } else {
                                                                view! {}.into_view()
                                                            }}
                                                        </div>
                                                    </div>
                                                }
                                            }).collect::<Vec<_>>()}
                                        </div>
                                    }.into_view()
                                }}
                            </div>
                        }.into_view(),

                        "active" => view! {
                            <div class="active-alerts-section">
                                {move || if loading.get() {
                                    view! { <div class="loading">"Loading active alerts..."</div> }.into_view()
                                } else if active_alerts.get().is_empty() {
                                    view! { <div class="empty-state">"No active alerts"</div> }.into_view()
                                } else {
                                    view! {
                                        <div class="active-alerts-list">
                                            {active_alerts.get().into_iter().map(|alert| {
                                                let alert_clone = alert.clone();
                                                let alert_clone2 = alert.clone();
                                                let firing_duration = chrono::Utc::now()
                                                    .signed_duration_since(alert.started_at)
                                                    .num_minutes();
                                                let rule_name = alert.rule_name.clone();
                                                let severity = alert.severity.clone();
                                                let severity_color = get_severity_color(&alert.severity);
                                                let status = alert.status.clone();
                                                let status_color = get_alert_status_color(&alert.status);
                                                let is_firing = alert.status == "firing";
                                                let message = alert.message.clone();
                                                let current_value = alert.current_value;
                                                let threshold = alert.threshold;
                                                let started_at = alert.started_at.format("%Y-%m-%d %H:%M UTC").to_string();
                                                let labels = alert.labels.clone();

                                                view! {
                                                    <div class="active-alert-card">
                                                        <div class="alert-header">
                                                            <div class="alert-info">
                                                                <h3>{rule_name}</h3>
                                                                <span class={format!("severity-badge {}", severity_color)}>
                                                                    {severity}
                                                                </span>
                                                                <span class={format!("status-badge {}", status_color)}>
                                                                    {status}
                                                                </span>
                                                            </div>
                                                            <div class="alert-actions">
                                                                {if is_firing {
                                                                    view! {
                                                                        <>
                                                                            <button
                                                                                class="btn btn-sm btn-secondary"
                                                                                on:click=move |_| acknowledge_alert.dispatch(alert_clone.id.clone())
                                                                            >
                                                                                "Acknowledge"
                                                                            </button>
                                                                            <button
                                                                                class="btn btn-sm btn-warning"
                                                                                on:click=move |_| silence_alert.dispatch((alert_clone2.id.clone(), 3600)) // 1 hour
                                                                            >
                                                                                "Silence 1h"
                                                                            </button>
                                                                        </>
                                                                    }.into_view()
                                                                } else {
                                                                    view! {}.into_view()
                                                                }}
                                                            </div>
                                                        </div>
                                                        <div class="alert-content">
                                                            <div class="alert-message">{message}</div>
                                                            <div class="alert-details">
                                                                <div class="detail-row">
                                                                    <span class="label">"Value:"</span>
                                                                    <span class="value">{current_value}</span>
                                                                </div>
                                                                <div class="detail-row">
                                                                    <span class="label">"Threshold:"</span>
                                                                    <span class="value">{threshold}</span>
                                                                </div>
                                                                <div class="detail-row">
                                                                    <span class="label">"Firing for:"</span>
                                                                    <span class="value">{firing_duration}" minutes"</span>
                                                                </div>
                                                                <div class="detail-row">
                                                                    <span class="label">"Started:"</span>
                                                                    <span class="value">{started_at}</span>
                                                                </div>
                                                            </div>

                                                            {if !labels.is_empty() {
                                                                view! {
                                                                    <div class="alert-labels">
                                                                        <h4>"Labels:"</h4>
                                                                        <div class="label-tags">
                                                                            {labels.iter().map(|(key, value)| view! {
                                                                                <span class="label-tag">{format!("{}={}", key, value)}</span>
                                                                            }).collect::<Vec<_>>()}
                                                                        </div>
                                                                    </div>
                                                                }.into_view()
                                                            } else {
                                                                view! {}.into_view()
                                                            }}
                                                        </div>
                                                    </div>
                                                }
                                            }).collect::<Vec<_>>()}
                                        </div>
                                    }.into_view()
                                }}
                            </div>
                        }.into_view(),

                        _ => view! {
                            <div class="alert-history-section">
                                <div class="coming-soon">
                                    "Alert history feature coming soon"
                                </div>
                            </div>
                        }.into_view(),
                    }}
                </div>
            </div>

            // Create Alert Rule Modal
            {move || if show_create_modal.get() {
                view! {
                    <div class="modal-overlay" on:click=move |e: MouseEvent| {
                        if e.target().and_then(|t| t.dyn_ref::<web_sys::Element>().map(|el| el.class_name())).unwrap_or_default() == "modal-overlay" {
                            set_show_create_modal.set(false);
                            clear_form();
                        }
                    }>
                        <div class="modal-content large">
                            <div class="modal-header">
                                <h2>"Create Alert Rule"</h2>
                                <button
                                    class="modal-close"
                                    on:click=move |_| {
                                        set_show_create_modal.set(false);
                                        clear_form();
                                    }
                                >"x"</button>
                            </div>
                            <div class="modal-body">
                                <div class="form-group">
                                    <label>"Rule Name"</label>
                                    <input
                                        type="text"
                                        prop:value=form_name
                                        on:input=move |ev| set_form_name.set(event_target_value(&ev))
                                        placeholder="Enter alert rule name"
                                    />
                                </div>

                                <div class="form-group">
                                    <label>"Description (Optional)"</label>
                                    <textarea
                                        prop:value=form_description
                                        on:input=move |ev| set_form_description.set(event_target_value(&ev))
                                        placeholder="Describe what this alert monitors"
                                        rows="3"
                                    ></textarea>
                                </div>

                                <div class="form-row">
                                    <div class="form-group">
                                        <label>"Metric"</label>
                                        <select
                                            prop:value=form_metric
                                            on:change=move |ev| set_form_metric.set(event_target_value(&ev))
                                        >
                                            <option value="">"Select metric"</option>
                                            <option value="cpu_usage">"CPU Usage %"</option>
                                            <option value="memory_usage">"Memory Usage %"</option>
                                            <option value="disk_usage">"Disk Usage %"</option>
                                            <option value="network_rx_bytes">"Network Received"</option>
                                            <option value="network_tx_bytes">"Network Transmitted"</option>
                                            <option value="vm_count">"VM Count"</option>
                                            <option value="load_average">"Load Average"</option>
                                            <option value="disk_io_read">"Disk Read IOPS"</option>
                                            <option value="disk_io_write">"Disk Write IOPS"</option>
                                        </select>
                                    </div>
                                    <div class="form-group">
                                        <label>"Condition"</label>
                                        <select
                                            prop:value=form_condition
                                            on:change=move |ev| set_form_condition.set(event_target_value(&ev))
                                        >
                                            <option value="greater_than">"Greater Than"</option>
                                            <option value="less_than">"Less Than"</option>
                                            <option value="equal_to">"Equal To"</option>
                                            <option value="not_equal_to">"Not Equal To"</option>
                                        </select>
                                    </div>
                                    <div class="form-group">
                                        <label>"Threshold"</label>
                                        <input
                                            type="number"
                                            step="0.1"
                                            prop:value=form_threshold
                                            on:input=move |ev| set_form_threshold.set(event_target_value(&ev).parse().unwrap_or(0.0))
                                            placeholder="0.0"
                                        />
                                    </div>
                                </div>

                                <div class="form-row">
                                    <div class="form-group">
                                        <label>"Duration (seconds)"</label>
                                        <input
                                            type="number"
                                            prop:value=form_duration
                                            on:input=move |ev| set_form_duration.set(event_target_value(&ev).parse().unwrap_or(300))
                                            min="60"
                                            placeholder="300"
                                        />
                                        <small>"How long the condition must be true before firing"</small>
                                    </div>
                                    <div class="form-group">
                                        <label>"Severity"</label>
                                        <select
                                            prop:value=form_severity
                                            on:change=move |ev| set_form_severity.set(event_target_value(&ev))
                                        >
                                            <option value="info">"Info"</option>
                                            <option value="warning">"Warning"</option>
                                            <option value="critical">"Critical"</option>
                                        </select>
                                    </div>
                                </div>

                                <div class="form-group">
                                    <label>"Notification Channels"</label>
                                    <div class="checkbox-group">
                                        {notification_channels.get().into_iter().map(|channel| {
                                            let channel_id = channel.id.clone();
                                            let channel_id2 = channel.id.clone();
                                            let channel_name = channel.name.clone();
                                            let channel_type = channel.channel_type.clone();
                                            view! {
                                                <label class="checkbox-label">
                                                    <input
                                                        type="checkbox"
                                                        prop:checked=move || selected_channels.get().contains(&channel_id)
                                                        on:input=move |ev| {
                                                            let mut channels = selected_channels.get();
                                                            if event_target_checked(&ev) {
                                                                if !channels.contains(&channel_id2) {
                                                                    channels.push(channel_id2.clone());
                                                                }
                                                            } else {
                                                                channels.retain(|id| id != &channel_id2);
                                                            }
                                                            set_selected_channels.set(channels);
                                                        }
                                                    />
                                                    {channel_name}" ("{channel_type}")"
                                                </label>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </div>
                                </div>

                                <div class="form-group">
                                    <label class="checkbox-label">
                                        <input
                                            type="checkbox"
                                            prop:checked=form_enabled
                                            on:input=move |ev| set_form_enabled.set(event_target_checked(&ev))
                                        />
                                        "Enable this rule"
                                    </label>
                                </div>

                                <div class="form-row">
                                    <div class="form-group">
                                        <label>"Labels (one per line, key=value)"</label>
                                        <textarea
                                            prop:value=form_labels
                                            on:input=move |ev| set_form_labels.set(event_target_value(&ev))
                                            placeholder="environment=production\nteam=infrastructure"
                                            rows="3"
                                        ></textarea>
                                    </div>
                                    <div class="form-group">
                                        <label>"Annotations (one per line, key=value)"</label>
                                        <textarea
                                            prop:value=form_annotations
                                            on:input=move |ev| set_form_annotations.set(event_target_value(&ev))
                                            placeholder="summary=High CPU usage detected\nrunbook_url=https://wiki.example.com/high-cpu"
                                            rows="3"
                                        ></textarea>
                                    </div>
                                </div>
                            </div>
                            <div class="modal-footer">
                                <button
                                    class="btn btn-secondary"
                                    on:click=move |_| {
                                        set_show_create_modal.set(false);
                                        clear_form();
                                    }
                                >"Cancel"</button>
                                <button
                                    class="btn btn-primary"
                                    on:click=move |_| create_rule.dispatch(())
                                    disabled=move || form_name.get().is_empty() || form_metric.get().is_empty() || loading.get()
                                >"Create Alert Rule"</button>
                            </div>
                        </div>
                    </div>
                }.into_view()
            } else {
                view! {}.into_view()
            }}

            // Edit Alert Rule Modal
            {move || if show_edit_modal.get() {
                view! {
                    <div class="modal-overlay" on:click=move |e: MouseEvent| {
                        if e.target().and_then(|t| t.dyn_ref::<web_sys::Element>().map(|el| el.class_name())).unwrap_or_default() == "modal-overlay" {
                            set_show_edit_modal.set(false);
                            set_selected_rule.set(None);
                            clear_form();
                        }
                    }>
                        <div class="modal-content large">
                            <div class="modal-header">
                                <h2>"Edit Alert Rule"</h2>
                                <button
                                    class="modal-close"
                                    on:click=move |_| {
                                        set_show_edit_modal.set(false);
                                        set_selected_rule.set(None);
                                        clear_form();
                                    }
                                >"x"</button>
                            </div>
                            <div class="modal-body">
                                // Same form as create modal
                                <div class="form-group">
                                    <label>"Rule Name"</label>
                                    <input
                                        type="text"
                                        prop:value=form_name
                                        on:input=move |ev| set_form_name.set(event_target_value(&ev))
                                        placeholder="Enter alert rule name"
                                    />
                                </div>
                                // ... (rest of form fields same as create modal)
                            </div>
                            <div class="modal-footer">
                                <button
                                    class="btn btn-secondary"
                                    on:click=move |_| {
                                        set_show_edit_modal.set(false);
                                        set_selected_rule.set(None);
                                        clear_form();
                                    }
                                >"Cancel"</button>
                                <button
                                    class="btn btn-primary"
                                    on:click=move |_| edit_rule.dispatch(())
                                    disabled=move || form_name.get().is_empty() || form_metric.get().is_empty() || loading.get()
                                >"Update Alert Rule"</button>
                            </div>
                        </div>
                    </div>
                }.into_view()
            } else {
                view! {}.into_view()
            }}

            // Test Alert Modal
            {move || if show_test_modal.get() {
                view! {
                    <div class="modal-overlay" on:click=move |e: MouseEvent| {
                        if e.target().and_then(|t| t.dyn_ref::<web_sys::Element>().map(|el| el.class_name())).unwrap_or_default() == "modal-overlay" {
                            set_show_test_modal.set(false);
                            set_selected_rule.set(None);
                        }
                    }>
                        <div class="modal-content">
                            <div class="modal-header">
                                <h2>"Test Alert Rule"</h2>
                                <button
                                    class="modal-close"
                                    on:click=move |_| {
                                        set_show_test_modal.set(false);
                                        set_selected_rule.set(None);
                                    }
                                >"x"</button>
                            </div>
                            <div class="modal-body">
                                {selected_rule.get().map(|rule| view! {
                                    <div class="test-alert-info">
                                        <h3>"Testing: "{&rule.name}</h3>
                                        <p>"This will send a test notification to all configured channels for this alert rule."</p>
                                        <div class="rule-summary">
                                            <div class="summary-row">
                                                <span class="label">"Metric:"</span>
                                                <span class="value">{&rule.metric}</span>
                                            </div>
                                            <div class="summary-row">
                                                <span class="label">"Channels:"</span>
                                                <span class="value">{rule.notification_channels.len()}" configured"</span>
                                            </div>
                                        </div>
                                    </div>
                                })}
                            </div>
                            <div class="modal-footer">
                                <button
                                    class="btn btn-secondary"
                                    on:click=move |_| {
                                        set_show_test_modal.set(false);
                                        set_selected_rule.set(None);
                                    }
                                >"Cancel"</button>
                                <button
                                    class="btn btn-warning"
                                    on:click=move |_| {
                                        if let Some(rule) = selected_rule.get() {
                                            test_rule.dispatch(rule.id);
                                        }
                                    }
                                    disabled=loading
                                >"Send Test Alert"</button>
                            </div>
                        </div>
                    </div>
                }.into_view()
            } else {
                view! {}.into_view()
            }}
        </div>
    }
}