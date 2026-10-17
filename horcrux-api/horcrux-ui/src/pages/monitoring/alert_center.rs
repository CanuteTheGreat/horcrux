//! Monitoring > Alert Center (`/monitoring/alerts`): alert rules (create,
//! edit, enable/disable, test, delete), active alerts (acknowledge) and the
//! alert history. Every type is the API's own shape (see `api::AlertRule`,
//! `api::ActiveAlert`); `tests/api_contract.rs` checks them against a real
//! server with a rule that really fires.

use crate::api::*;
use leptos::*;
use wasm_bindgen::JsCast;
use web_sys::MouseEvent;

/// Metric types the server evaluates against the node every 15 s. Disk I/O
/// and network I/O rules are accepted by the API but have no node-level
/// source yet, so the form doesn't offer them.
const FORM_METRICS: &[&str] = &[
    "cpu_usage",
    "memory_usage",
    "disk_usage",
    "node_load",
    "vm_count",
];

fn severity_class(severity: &str) -> &'static str {
    match severity {
        "critical" => "text-red-600 bg-red-100",
        "warning" => "text-yellow-600 bg-yellow-100",
        "info" => "text-blue-600 bg-blue-100",
        _ => "text-gray-600 bg-gray-100",
    }
}

fn status_class(status: &str) -> &'static str {
    match status {
        "firing" => "text-red-600 bg-red-100",
        "acknowledged" => "text-yellow-600 bg-yellow-100",
        "resolved" => "text-green-600 bg-green-100",
        _ => "text-gray-600 bg-gray-100",
    }
}

fn describe_condition(c: &AlertCondition) -> String {
    format!(
        "{} {} {}",
        alert_metric_label(&c.metric_type),
        alert_operator_symbol(&c.operator),
        c.threshold
    )
}

#[component]
pub fn AlertCenterPage() -> impl IntoView {
    let (alert_rules, set_alert_rules) = create_signal(Vec::<AlertRule>::new());
    let (active_alerts, set_active_alerts) = create_signal(Vec::<ActiveAlert>::new());
    let (alert_history, set_alert_history) = create_signal(Vec::<ActiveAlert>::new());
    let (channel_count, set_channel_count) = create_signal(0usize);
    let (selected_rule, set_selected_rule) = create_signal(None::<AlertRule>);
    // None = closed, Some(None) = create, Some(Some(id)) = edit
    let (form_open, set_form_open) = create_signal(None::<Option<String>>);
    let (show_test_modal, set_show_test_modal) = create_signal(false);
    let (loading, set_loading) = create_signal(false);
    let (error_message, set_error_message) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let (active_tab, set_active_tab) = create_signal("rules".to_string());

    // Rule form
    let (form_name, set_form_name) = create_signal(String::new());
    let (form_description, set_form_description) = create_signal(String::new());
    let (form_metric, set_form_metric) = create_signal("cpu_usage".to_string());
    let (form_operator, set_form_operator) = create_signal("greaterthan".to_string());
    let (form_threshold, set_form_threshold) = create_signal(80.0f64);
    let (form_target, set_form_target) = create_signal("*".to_string());
    let (form_duration, set_form_duration) = create_signal(300u64);
    let (form_severity, set_form_severity) = create_signal("warning".to_string());
    let (form_enabled, set_form_enabled) = create_signal(true);

    let clear_form = move || {
        set_form_name.set(String::new());
        set_form_description.set(String::new());
        set_form_metric.set("cpu_usage".to_string());
        set_form_operator.set("greaterthan".to_string());
        set_form_threshold.set(80.0);
        set_form_target.set("*".to_string());
        set_form_duration.set(300);
        set_form_severity.set("warning".to_string());
        set_form_enabled.set(true);
    };

    let fill_form = move |rule: &AlertRule| {
        set_form_name.set(rule.name.clone());
        set_form_description.set(rule.description.clone());
        set_form_metric.set(rule.condition.metric_type.clone());
        set_form_operator.set(rule.condition.operator.clone());
        set_form_threshold.set(rule.condition.threshold);
        set_form_target.set(rule.condition.target_pattern.clone());
        set_form_duration.set(rule.condition.duration_seconds);
        set_form_severity.set(rule.severity.clone());
        set_form_enabled.set(rule.enabled);
    };

    let form_rule = move |id: String| AlertRule {
        id,
        name: form_name.get().trim().to_string(),
        description: form_description.get(),
        severity: form_severity.get(),
        enabled: form_enabled.get(),
        condition: AlertCondition {
            metric_type: form_metric.get(),
            operator: form_operator.get(),
            threshold: form_threshold.get(),
            target_pattern: form_target.get().trim().to_string(),
            duration_seconds: form_duration.get(),
        },
    };

    let load_data = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);
        let mut errors = Vec::new();
        match get_alert_rules().await {
            Ok(mut rules) => {
                rules.sort_by(|a, b| a.name.cmp(&b.name));
                set_alert_rules.set(rules)
            }
            Err(e) => errors.push(format!("Failed to load alert rules: {}", e)),
        }
        match get_active_alerts().await {
            Ok(alerts) => set_active_alerts.set(alerts),
            Err(e) => errors.push(format!("Failed to load active alerts: {}", e)),
        }
        match get_alert_history(200).await {
            Ok(history) => set_alert_history.set(history),
            Err(e) => errors.push(format!("Failed to load alert history: {}", e)),
        }
        if let Ok(channels) = get_notification_channels().await {
            set_channel_count.set(channels.iter().filter(|c| c.enabled).count())
        }
        if !errors.is_empty() {
            set_error_message.set(Some(errors.join("; ")));
        }
        set_loading.set(false);
    });

    let save_rule = create_action(move |_: &()| async move {
        let Some(mode) = form_open.get_untracked() else {
            return;
        };
        set_loading.set(true);
        set_error_message.set(None);
        let result = match mode {
            None => create_alert_rule(form_rule(format!("rule-{}", js_sys::Date::now() as u64)))
                .await
                .map(|_| "Alert rule created".to_string()),
            Some(id) => update_alert_rule(form_rule(id))
                .await
                .map(|r| format!("Alert rule '{}' updated", r.name)),
        };
        match result {
            Ok(msg) => {
                set_form_open.set(None);
                clear_form();
                set_notice.set(Some(msg));
                load_data.dispatch(());
            }
            Err(e) => set_error_message.set(Some(format!("Failed to save alert rule: {}", e))),
        }
        set_loading.set(false);
    });

    let delete_rule = create_action(move |rule_id: &String| {
        let rule_id = rule_id.clone();
        async move {
            match delete_alert_rule(rule_id).await {
                Ok(_) => {
                    set_notice.set(Some("Alert rule deleted".into()));
                    load_data.dispatch(())
                }
                Err(e) => {
                    set_error_message.set(Some(format!("Failed to delete alert rule: {}", e)))
                }
            }
        }
    });

    let toggle_rule = create_action(move |(rule_id, enabled): &(String, bool)| {
        let (rule_id, enabled) = (rule_id.clone(), *enabled);
        async move {
            match toggle_alert_rule(rule_id, enabled).await {
                Ok(r) => {
                    set_notice.set(Some(format!(
                        "Alert rule '{}' {}",
                        r.name,
                        if r.enabled { "enabled" } else { "disabled" }
                    )));
                    load_data.dispatch(())
                }
                Err(e) => {
                    set_error_message.set(Some(format!("Failed to update alert rule: {}", e)))
                }
            }
        }
    });

    let test_rule = create_action(move |rule_id: &String| {
        let rule_id = rule_id.clone();
        async move {
            set_loading.set(true);
            set_error_message.set(None);
            match test_alert_rule(rule_id).await {
                Ok(r) => {
                    set_show_test_modal.set(false);
                    set_notice.set(Some(if r.channels == 0 {
                        "No enabled notification channel - add one under Monitoring > Notifications"
                            .to_string()
                    } else {
                        format!(
                            "Test alert sent to {} channel(s): {} delivered, {} failed",
                            r.channels, r.delivered, r.failed
                        )
                    }));
                    load_data.dispatch(());
                }
                Err(e) => set_error_message.set(Some(format!("Failed to test alert rule: {}", e))),
            }
            set_loading.set(false);
        }
    });

    let acknowledge = create_action(move |alert: &ActiveAlert| {
        let alert = alert.clone();
        async move {
            let user = get_me()
                .await
                .map(|me| me.user.username)
                .unwrap_or_else(|_| "unknown".to_string());
            match acknowledge_alert(&alert, &user).await {
                Ok(_) => {
                    set_notice.set(Some(format!("Alert '{}' acknowledged", alert.rule_name)));
                    load_data.dispatch(())
                }
                Err(e) => {
                    set_error_message.set(Some(format!("Failed to acknowledge alert: {}", e)))
                }
            }
        }
    });

    create_effect(move |_| {
        load_data.dispatch(());
    });

    let tab_button = move |key: &'static str, label: Signal<String>| {
        view! {
            <button
                class=move || if active_tab.get() == key { "tab-button active" } else { "tab-button" }
                on:click=move |_| set_active_tab.set(key.to_string())
            >
                {label}
            </button>
        }
    };

    let rules_view = move || {
        let rules = alert_rules.get();
        if rules.is_empty() {
            return view! {
                <div class="empty-state">
                    {move || if loading.get() { "Loading alert rules..." } else { "No alert rules configured" }}
                </div>
            }
            .into_view();
        }
        view! {
            <div class="alert-rules-grid">
                {rules.into_iter().map(|rule| {
                    let for_edit = rule.clone();
                    let for_test = rule.clone();
                    let id_toggle = rule.id.clone();
                    let id_delete = rule.id.clone();
                    let name_delete = rule.name.clone();
                    let enabled = rule.enabled;
                    let condition = describe_condition(&rule.condition);
                    let description = rule.description.clone();
                    view! {
                        <div class="alert-rule-card" data-rule-name={rule.name.clone()}>
                            <div class="card-header">
                                <div class="rule-info">
                                    <h3>{rule.name.clone()}</h3>
                                    <span class={format!("severity-badge {}", severity_class(&rule.severity))}>
                                        {rule.severity.clone()}
                                    </span>
                                    <span class={if enabled { "status-enabled" } else { "status-disabled" }}>
                                        {if enabled { "Enabled" } else { "Disabled" }}
                                    </span>
                                </div>
                                <div class="card-actions">
                                    <button class="btn btn-sm btn-secondary" on:click=move |_| {
                                        fill_form(&for_edit);
                                        set_form_open.set(Some(Some(for_edit.id.clone())));
                                    }>"Edit"</button>
                                    <button class="btn btn-sm btn-secondary" on:click=move |_| {
                                        toggle_rule.dispatch((id_toggle.clone(), !enabled));
                                    }>{if enabled { "Disable" } else { "Enable" }}</button>
                                    <button class="btn btn-sm btn-primary" on:click=move |_| {
                                        set_selected_rule.set(Some(for_test.clone()));
                                        set_show_test_modal.set(true);
                                    }>"Test"</button>
                                    <button class="btn btn-sm btn-danger" on:click=move |_| {
                                        if web_sys::window()
                                            .and_then(|w| w.confirm_with_message(&format!("Delete alert rule '{}'?", name_delete)).ok())
                                            .unwrap_or(false)
                                        {
                                            delete_rule.dispatch(id_delete.clone());
                                        }
                                    }>"Delete"</button>
                                </div>
                            </div>
                            <div class="card-content">
                                {(!description.is_empty()).then(|| view! {
                                    <div class="rule-description">{description.clone()}</div>
                                })}
                                <div class="rule-details">
                                    <div class="detail-row">
                                        <span class="label">"Condition:"</span>
                                        <code class="metric-name">{condition}</code>
                                    </div>
                                    <div class="detail-row">
                                        <span class="label">"Targets:"</span>
                                        <code>{rule.condition.target_pattern.clone()}</code>
                                    </div>
                                    <div class="detail-row">
                                        <span class="label">"For at least:"</span>
                                        <span class="value">{rule.condition.duration_seconds}" s"</span>
                                    </div>
                                </div>
                            </div>
                        </div>
                    }
                }).collect_view()}
            </div>
        }
        .into_view()
    };

    let active_view = move || {
        let alerts = active_alerts.get();
        if alerts.is_empty() {
            return view! { <div class="empty-state">"No active alerts"</div> }.into_view();
        }
        view! {
            <div class="active-alerts-list">
                {alerts.into_iter().map(|alert| {
                    let is_firing = alert.status == "firing";
                    let for_ack = alert.clone();
                    let minutes = (js_sys::Date::now() as i64 / 1000 - alert.fired_at).max(0) / 60;
                    view! {
                        <div class="active-alert-card" data-rule-name={alert.rule_name.clone()}>
                            <div class="alert-header">
                                <div class="alert-info">
                                    <h3>{alert.rule_name.clone()}</h3>
                                    <span class={format!("severity-badge {}", severity_class(&alert.severity))}>
                                        {alert.severity.clone()}
                                    </span>
                                    <span class={format!("status-badge {}", status_class(&alert.status))}>
                                        {alert.status.clone()}
                                    </span>
                                </div>
                                <div class="alert-actions">
                                    {is_firing.then(|| view! {
                                        <button class="btn btn-sm btn-secondary"
                                            on:click=move |_| acknowledge.dispatch(for_ack.clone())
                                        >"Acknowledge"</button>
                                    })}
                                </div>
                            </div>
                            <div class="alert-content">
                                <div class="alert-message">{alert.message.clone()}</div>
                                <div class="alert-details">
                                    <div class="detail-row">
                                        <span class="label">"Target:"</span>
                                        <code>{alert.target.clone()}</code>
                                    </div>
                                    <div class="detail-row">
                                        <span class="label">"Value:"</span>
                                        <span class="value">{format!("{:.2}", alert.metric_value)}</span>
                                    </div>
                                    <div class="detail-row">
                                        <span class="label">"Threshold:"</span>
                                        <span class="value">{alert.threshold}</span>
                                    </div>
                                    <div class="detail-row">
                                        <span class="label">"Firing since:"</span>
                                        <span class="value">{format_unix_time(alert.fired_at)}" ("{minutes}" min)"</span>
                                    </div>
                                    {alert.acknowledged_by.clone().map(|by| view! {
                                        <div class="detail-row">
                                            <span class="label">"Acknowledged by:"</span>
                                            <span class="value">{by}</span>
                                        </div>
                                    })}
                                </div>
                            </div>
                        </div>
                    }
                }).collect_view()}
            </div>
        }
        .into_view()
    };

    let history_view = move || {
        let history = alert_history.get();
        if history.is_empty() {
            return view! { <div class="empty-state">"No alerts have fired yet"</div> }.into_view();
        }
        view! {
            <table class="data-table alert-history-table">
                <thead>
                    <tr>
                        <th>"Rule"</th>
                        <th>"Severity"</th>
                        <th>"Status"</th>
                        <th>"Target"</th>
                        <th>"Value"</th>
                        <th>"Fired"</th>
                        <th>"Resolved"</th>
                    </tr>
                </thead>
                <tbody>
                    {history.into_iter().map(|a| view! {
                        <tr>
                            <td>{a.rule_name.clone()}</td>
                            <td><span class={format!("severity-badge {}", severity_class(&a.severity))}>{a.severity.clone()}</span></td>
                            <td><span class={format!("status-badge {}", status_class(&a.status))}>{a.status.clone()}</span></td>
                            <td><code>{a.target.clone()}</code></td>
                            <td>{format!("{:.2}", a.metric_value)}</td>
                            <td>{format_unix_time(a.fired_at)}</td>
                            <td>{a.resolved_at.map(format_unix_time).unwrap_or_else(|| "-".into())}</td>
                        </tr>
                    }).collect_view()}
                </tbody>
            </table>
        }
        .into_view()
    };

    let close_form = move || {
        set_form_open.set(None);
        clear_form();
    };

    let form_modal = move || {
        let mode = form_open.get()?;
        let editing = mode.is_some();
        Some(view! {
            <div class="modal-overlay" on:click=move |e: MouseEvent| {
                if e.target().and_then(|t| t.dyn_ref::<web_sys::Element>().map(|el| el.class_name())).unwrap_or_default() == "modal-overlay" {
                    close_form();
                }
            }>
                <div class="modal-content large">
                    <div class="modal-header">
                        <h2>{if editing { "Edit Alert Rule" } else { "Create Alert Rule" }}</h2>
                        <button class="modal-close" on:click=move |_| close_form()>"x"</button>
                    </div>
                    <div class="modal-body">
                        <div class="form-group">
                            <label>"Rule Name"</label>
                            <input id="rule-name" type="text" prop:value=form_name
                                on:input=move |ev| set_form_name.set(event_target_value(&ev))
                                placeholder="High CPU on node" />
                        </div>
                        <div class="form-group">
                            <label>"Description (optional)"</label>
                            <textarea prop:value=form_description rows="2"
                                on:input=move |ev| set_form_description.set(event_target_value(&ev))></textarea>
                        </div>
                        <div class="form-row">
                            <div class="form-group">
                                <label>"Metric"</label>
                                <select id="rule-metric" on:change=move |ev| set_form_metric.set(event_target_value(&ev))>
                                    {FORM_METRICS.iter().map(|m| view! {
                                        <option value={*m} selected=move || form_metric.get() == *m>
                                            {alert_metric_label(m)}
                                        </option>
                                    }).collect_view()}
                                </select>
                            </div>
                            <div class="form-group">
                                <label>"Condition"</label>
                                <select id="rule-operator" on:change=move |ev| set_form_operator.set(event_target_value(&ev))>
                                    {ALERT_OPERATORS.iter().map(|(v, sym)| view! {
                                        <option value={*v} selected=move || form_operator.get() == *v>{*sym}</option>
                                    }).collect_view()}
                                </select>
                            </div>
                            <div class="form-group">
                                <label>"Threshold"</label>
                                <input id="rule-threshold" type="number" step="0.1" prop:value=move || form_threshold.get().to_string()
                                    on:input=move |ev| {
                                        if let Ok(v) = event_target_value(&ev).parse::<f64>() {
                                            set_form_threshold.set(v)
                                        }
                                    } />
                            </div>
                        </div>
                        <div class="form-row">
                            <div class="form-group">
                                <label>"Targets"</label>
                                <input id="rule-target" type="text" prop:value=form_target
                                    on:input=move |ev| set_form_target.set(event_target_value(&ev))
                                    placeholder="*" />
                                <small>"Node name to watch; * for every node, or a prefix like node-*"</small>
                            </div>
                            <div class="form-group">
                                <label>"For at least (seconds)"</label>
                                <input id="rule-duration" type="number" min="0" prop:value=move || form_duration.get().to_string()
                                    on:input=move |ev| set_form_duration.set(event_target_value(&ev).parse().unwrap_or(0)) />
                                <small>"How long the condition must hold before the alert fires (checked every 15 s)"</small>
                            </div>
                            <div class="form-group">
                                <label>"Severity"</label>
                                <select id="rule-severity" on:change=move |ev| set_form_severity.set(event_target_value(&ev))>
                                    {["info", "warning", "critical"].into_iter().map(|s| view! {
                                        <option value={s} selected=move || form_severity.get() == s>{s}</option>
                                    }).collect_view()}
                                </select>
                            </div>
                        </div>
                        <div class="form-group">
                            <label class="checkbox-label">
                                <input type="checkbox" prop:checked=form_enabled
                                    on:change=move |ev| set_form_enabled.set(event_target_checked(&ev)) />
                                "Enable this rule"
                            </label>
                        </div>
                        <p class="form-hint">
                            "Fired alerts are sent to every enabled channel under Monitoring > Notifications ("
                            {channel_count}" enabled)."
                        </p>
                    </div>
                    <div class="modal-footer">
                        <button class="btn btn-secondary" on:click=move |_| close_form()>"Cancel"</button>
                        <button id="rule-submit" class="btn btn-primary"
                            on:click=move |_| save_rule.dispatch(())
                            disabled=move || form_name.get().trim().is_empty() || form_target.get().trim().is_empty() || loading.get()
                        >{if editing { "Save Changes" } else { "Create Alert Rule" }}</button>
                    </div>
                </div>
            </div>
        })
    };

    let close_test = move || {
        set_show_test_modal.set(false);
        set_selected_rule.set(None);
    };

    let test_modal = move || {
        if !show_test_modal.get() {
            return None;
        }
        let rule = selected_rule.get()?;
        let rule_id = rule.id.clone();
        Some(view! {
            <div class="modal-overlay" on:click=move |e: MouseEvent| {
                if e.target().and_then(|t| t.dyn_ref::<web_sys::Element>().map(|el| el.class_name())).unwrap_or_default() == "modal-overlay" {
                    close_test();
                }
            }>
                <div class="modal-content">
                    <div class="modal-header">
                        <h2>"Test Alert Rule"</h2>
                        <button class="modal-close" on:click=move |_| close_test()>"x"</button>
                    </div>
                    <div class="modal-body">
                        <h3>"Testing: "{rule.name.clone()}</h3>
                        <p>"Sends a test alert for this rule to every enabled notification channel."</p>
                        <div class="rule-summary">
                            <div class="summary-row">
                                <span class="label">"Condition:"</span>
                                <span class="value">{describe_condition(&rule.condition)}</span>
                            </div>
                            <div class="summary-row">
                                <span class="label">"Channels:"</span>
                                <span class="value">{channel_count}" enabled"</span>
                            </div>
                        </div>
                    </div>
                    <div class="modal-footer">
                        <button class="btn btn-secondary" on:click=move |_| close_test()>"Cancel"</button>
                        <button class="btn btn-warning"
                            on:click=move |_| test_rule.dispatch(rule_id.clone())
                            disabled=loading
                        >"Send Test Alert"</button>
                    </div>
                </div>
            </div>
        })
    };

    view! {
        <div class="alert-center-page">
            <div class="page-header">
                <h1>"Alert Center"</h1>
                <div class="header-actions">
                    <button class="btn btn-primary" disabled=loading on:click=move |_| {
                        clear_form();
                        set_form_open.set(Some(None));
                    }>"Create Alert Rule"</button>
                    <button class="btn btn-secondary" disabled=loading
                        on:click=move |_| load_data.dispatch(())
                    >"Refresh"</button>
                </div>
            </div>

            {move || error_message.get().map(|msg| view! { <div id="alert-center-error" class="alert alert-error">{msg}</div> })}
            {move || notice.get().map(|msg| view! { <div id="alert-center-notice" class="alert alert-success">{msg}</div> })}

            <div class="alert-tabs">
                <div class="tab-buttons">
                    {tab_button("rules", Signal::derive(move || format!("Alert Rules ({})", alert_rules.get().len())))}
                    {tab_button("active", Signal::derive(move || format!("Active Alerts ({})", active_alerts.get().len())))}
                    {tab_button("history", Signal::derive(|| "Alert History".to_string()))}
                </div>
                <div class="tab-content">
                    {move || match active_tab.get().as_str() {
                        "rules" => rules_view(),
                        "active" => active_view(),
                        _ => history_view(),
                    }}
                </div>
            </div>

            {form_modal}
            {test_modal}
        </div>
    }
}
