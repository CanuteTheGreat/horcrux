use leptos::*;
use wasm_bindgen::JsCast;
use crate::api::*;
use web_sys::MouseEvent;

#[component]
pub fn NotificationsPage() -> impl IntoView {
    let (notification_channels, set_notification_channels) = create_signal(Vec::<NotificationChannel>::new());
    let (webhook_configs, set_webhook_configs) = create_signal(Vec::<WebhookConfig>::new());
    let (selected_channel, set_selected_channel) = create_signal(None::<NotificationChannel>);
    let (show_create_modal, set_show_create_modal) = create_signal(false);
    let (show_edit_modal, set_show_edit_modal) = create_signal(false);
    let (show_test_modal, set_show_test_modal) = create_signal(false);
    let (loading, set_loading) = create_signal(false);
    let (error_message, set_error_message) = create_signal(None::<String>);
    let (active_tab, set_active_tab) = create_signal("channels".to_string());

    // Channel form
    let (form_name, set_form_name) = create_signal(String::new());
    let (form_description, set_form_description) = create_signal(String::new());
    let (form_type, set_form_type) = create_signal("email".to_string());
    let (form_enabled, set_form_enabled) = create_signal(true);

    // Email settings
    let (form_email_address, set_form_email_address) = create_signal(String::new());
    let (form_smtp_server, set_form_smtp_server) = create_signal(String::new());
    let (form_smtp_port, set_form_smtp_port) = create_signal(587);
    let (form_smtp_username, set_form_smtp_username) = create_signal(String::new());
    let (form_smtp_password, set_form_smtp_password) = create_signal(String::new());
    let (form_use_tls, set_form_use_tls) = create_signal(true);

    // Slack settings
    let (form_slack_webhook_url, set_form_slack_webhook_url) = create_signal(String::new());
    let (form_slack_channel, set_form_slack_channel) = create_signal(String::new());
    let (form_slack_username, set_form_slack_username) = create_signal(String::new());
    let (form_slack_icon_emoji, set_form_slack_icon_emoji) = create_signal(String::new());

    // Teams settings
    let (form_teams_webhook_url, set_form_teams_webhook_url) = create_signal(String::new());

    // Discord settings
    let (form_discord_webhook_url, set_form_discord_webhook_url) = create_signal(String::new());
    let (form_discord_username, set_form_discord_username) = create_signal(String::new());

    // PagerDuty settings
    let (form_pagerduty_routing_key, set_form_pagerduty_routing_key) = create_signal(String::new());
    let (form_pagerduty_severity, set_form_pagerduty_severity) = create_signal("error".to_string());

    // Webhook settings
    let (form_webhook_url, set_form_webhook_url) = create_signal(String::new());
    let (form_webhook_method, set_form_webhook_method) = create_signal("POST".to_string());
    let (form_webhook_headers, set_form_webhook_headers) = create_signal(String::new());

    // Helper functions - defined early so actions can use them
    let clear_form = move || {
        set_form_name.set(String::new());
        set_form_description.set(String::new());
        set_form_type.set("email".to_string());
        set_form_enabled.set(true);
        set_form_email_address.set(String::new());
        set_form_smtp_server.set(String::new());
        set_form_smtp_port.set(587);
        set_form_smtp_username.set(String::new());
        set_form_smtp_password.set(String::new());
        set_form_use_tls.set(true);
        set_form_slack_webhook_url.set(String::new());
        set_form_slack_channel.set(String::new());
        set_form_slack_username.set(String::new());
        set_form_slack_icon_emoji.set(String::new());
        set_form_teams_webhook_url.set(String::new());
        set_form_discord_webhook_url.set(String::new());
        set_form_discord_username.set(String::new());
        set_form_pagerduty_routing_key.set(String::new());
        set_form_pagerduty_severity.set("error".to_string());
        set_form_webhook_url.set(String::new());
        set_form_webhook_method.set("POST".to_string());
        set_form_webhook_headers.set(String::new());
    };

    let parse_headers = |text: &str| -> std::collections::HashMap<String, String> {
        text.lines()
            .filter_map(|line| {
                let parts: Vec<&str> = line.splitn(2, ':').collect();
                if parts.len() == 2 {
                    Some((parts[0].trim().to_string(), parts[1].trim().to_string()))
                } else {
                    None
                }
            })
            .collect()
    };

    let format_headers = |headers: &std::collections::HashMap<String, String>| -> String {
        headers
            .iter()
            .map(|(k, v)| format!("{}: {}", k, v))
            .collect::<Vec<_>>()
            .join("\n")
    };

    // Load data
    let load_data = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);

        match get_notification_channels().await {
            Ok(channels) => set_notification_channels.set(channels),
            Err(e) => set_error_message.set(Some(format!("Failed to load notification channels: {}", e))),
        }

        match get_webhook_configs().await {
            Ok(webhooks) => set_webhook_configs.set(webhooks),
            Err(_) => {}
        }

        set_loading.set(false);
    });

    // Create notification channel
    let create_channel = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);

        let config = match form_type.get().as_str() {
            "email" => NotificationChannelConfig::Email {
                address: form_email_address.get(),
                smtp_server: form_smtp_server.get(),
                smtp_port: form_smtp_port.get(),
                username: form_smtp_username.get(),
                password: form_smtp_password.get(),
                use_tls: form_use_tls.get(),
            },
            "slack" => NotificationChannelConfig::Slack {
                webhook_url: form_slack_webhook_url.get(),
                channel: Some(form_slack_channel.get()).filter(|s| !s.is_empty()),
                username: Some(form_slack_username.get()).filter(|s| !s.is_empty()),
                icon_emoji: Some(form_slack_icon_emoji.get()).filter(|s| !s.is_empty()),
            },
            "teams" => NotificationChannelConfig::Teams {
                webhook_url: form_teams_webhook_url.get(),
            },
            "discord" => NotificationChannelConfig::Discord {
                webhook_url: form_discord_webhook_url.get(),
                username: Some(form_discord_username.get()).filter(|s| !s.is_empty()),
                avatar_url: None,
            },
            "pagerduty" => NotificationChannelConfig::PagerDuty {
                integration_key: form_pagerduty_routing_key.get(),
                routing_key: form_pagerduty_routing_key.get(),
                severity: form_pagerduty_severity.get(),
            },
            "webhook" => NotificationChannelConfig::Webhook {
                url: form_webhook_url.get(),
                method: form_webhook_method.get(),
                headers: parse_headers(&form_webhook_headers.get()),
                auth: None,
            },
            _ => {
                set_error_message.set(Some("Unknown channel type".to_string()));
                set_loading.set(false);
                return;
            }
        };

        let new_channel = NotificationChannel {
            id: format!("channel-{}", chrono::Utc::now().timestamp()),
            name: form_name.get(),
            description: if form_description.get().is_empty() { None } else { Some(form_description.get()) },
            channel_type: form_type.get(),
            enabled: form_enabled.get(),
            config,
            created_at: chrono::Utc::now(),
            last_used: None,
            success_count: 0,
            failure_count: 0,
        };

        match create_notification_channel(new_channel).await {
            Ok(_) => {
                set_show_create_modal.set(false);
                clear_form();
                load_data.dispatch(());
            }
            Err(e) => set_error_message.set(Some(format!("Failed to create notification channel: {}", e))),
        }

        set_loading.set(false);
    });

    // Edit notification channel
    let edit_channel = create_action(move |_: &()| async move {
        if let Some(channel) = selected_channel.get() {
            set_loading.set(true);
            set_error_message.set(None);

            let config = match form_type.get().as_str() {
                "email" => NotificationChannelConfig::Email {
                    address: form_email_address.get(),
                    smtp_server: form_smtp_server.get(),
                    smtp_port: form_smtp_port.get(),
                    username: form_smtp_username.get(),
                    password: form_smtp_password.get(),
                    use_tls: form_use_tls.get(),
                },
                "slack" => NotificationChannelConfig::Slack {
                    webhook_url: form_slack_webhook_url.get(),
                    channel: Some(form_slack_channel.get()).filter(|s| !s.is_empty()),
                    username: Some(form_slack_username.get()).filter(|s| !s.is_empty()),
                    icon_emoji: Some(form_slack_icon_emoji.get()).filter(|s| !s.is_empty()),
                },
                "teams" => NotificationChannelConfig::Teams {
                    webhook_url: form_teams_webhook_url.get(),
                },
                "discord" => NotificationChannelConfig::Discord {
                    webhook_url: form_discord_webhook_url.get(),
                    username: Some(form_discord_username.get()).filter(|s| !s.is_empty()),
                    avatar_url: None,
                },
                "pagerduty" => NotificationChannelConfig::PagerDuty {
                    integration_key: form_pagerduty_routing_key.get(),
                    routing_key: form_pagerduty_routing_key.get(),
                    severity: form_pagerduty_severity.get(),
                },
                "webhook" => NotificationChannelConfig::Webhook {
                    url: form_webhook_url.get(),
                    method: form_webhook_method.get(),
                    headers: parse_headers(&form_webhook_headers.get()),
                    auth: None,
                },
                _ => {
                    set_error_message.set(Some("Unknown channel type".to_string()));
                    set_loading.set(false);
                    return;
                }
            };

            let updated_channel = NotificationChannel {
                id: channel.id.clone(),
                name: form_name.get(),
                description: if form_description.get().is_empty() { None } else { Some(form_description.get()) },
                channel_type: form_type.get(),
                enabled: form_enabled.get(),
                config,
                created_at: channel.created_at,
                last_used: channel.last_used,
                success_count: channel.success_count,
                failure_count: channel.failure_count,
            };

            match update_notification_channel(&channel.id, updated_channel).await {
                Ok(_) => {
                    set_show_edit_modal.set(false);
                    set_selected_channel.set(None);
                    clear_form();
                    load_data.dispatch(());
                }
                Err(e) => set_error_message.set(Some(format!("Failed to update notification channel: {}", e))),
            }

            set_loading.set(false);
        }
    });

    // Delete notification channel
    let delete_channel = create_action(move |channel_id: &String| {
        let channel_id = channel_id.clone();
        async move {
            set_loading.set(true);
            set_error_message.set(None);

            match delete_notification_channel(channel_id).await {
                Ok(_) => load_data.dispatch(()),
                Err(e) => set_error_message.set(Some(format!("Failed to delete notification channel: {}", e))),
            }

            set_loading.set(false);
        }
    });

    // Test notification channel
    let test_channel = create_action(move |channel_id: &String| {
        let channel_id = channel_id.clone();
        async move {
            set_loading.set(true);
            set_error_message.set(None);

            match test_notification_channel(channel_id).await {
                Ok(_) => {
                    set_show_test_modal.set(false);
                    // Show success message
                }
                Err(e) => set_error_message.set(Some(format!("Failed to test notification channel: {}", e))),
            }

            set_loading.set(false);
        }
    });

    // Helper functions
    let init_form_with_channel = move |channel: &NotificationChannel| {
        set_form_name.set(channel.name.clone());
        set_form_description.set(channel.description.clone().unwrap_or_default());
        set_form_type.set(channel.channel_type.clone());
        set_form_enabled.set(channel.enabled);

        match &channel.config {
            NotificationChannelConfig::Email { address, smtp_server, smtp_port, username, password, use_tls } => {
                set_form_email_address.set(address.clone());
                set_form_smtp_server.set(smtp_server.clone());
                set_form_smtp_port.set(*smtp_port);
                set_form_smtp_username.set(username.clone());
                set_form_smtp_password.set(password.clone());
                set_form_use_tls.set(*use_tls);
            },
            NotificationChannelConfig::Slack { webhook_url, channel, username, icon_emoji } => {
                set_form_slack_webhook_url.set(webhook_url.clone());
                set_form_slack_channel.set(channel.clone().unwrap_or_default());
                set_form_slack_username.set(username.clone().unwrap_or_default());
                set_form_slack_icon_emoji.set(icon_emoji.clone().unwrap_or_default());
            },
            NotificationChannelConfig::Teams { webhook_url } => {
                set_form_teams_webhook_url.set(webhook_url.clone());
            },
            NotificationChannelConfig::Discord { webhook_url, username, avatar_url: _ } => {
                set_form_discord_webhook_url.set(webhook_url.clone());
                set_form_discord_username.set(username.clone().unwrap_or_default());
            },
            NotificationChannelConfig::PagerDuty { integration_key, routing_key: _, severity } => {
                set_form_pagerduty_routing_key.set(integration_key.clone());
                set_form_pagerduty_severity.set(severity.clone());
            },
            NotificationChannelConfig::Webhook { url, method, headers, auth: _ } => {
                set_form_webhook_url.set(url.clone());
                set_form_webhook_method.set(method.clone());
                set_form_webhook_headers.set(format_headers(headers));
            },
        }
    };

    let get_channel_status_color = |channel: &NotificationChannel| {
        if !channel.enabled {
            "text-gray-500"
        } else if channel.failure_count > 0 && channel.success_count == 0 {
            "text-red-600"
        } else if channel.failure_count > channel.success_count {
            "text-yellow-600"
        } else {
            "text-green-600"
        }
    };

    let get_channel_icon = |channel_type: &str| match channel_type {
        "email" => "📧",
        "slack" => "📢",
        "teams" => "👥",
        "discord" => "💬",
        "pagerduty" => "🚨",
        "webhook" => "🔗",
        _ => "📡",
    };

    // Initial load
    create_effect(move |_| {
        load_data.dispatch(());
    });

    view! {
        <div class="notifications-page">
            <div class="page-header">
                <h1>"Notification Channels"</h1>
                <div class="header-actions">
                    <button
                        class="btn btn-primary"
                        on:click=move |_| set_show_create_modal.set(true)
                        disabled=loading
                    >
                        "Create Channel"
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

            <div class="notification-tabs">
                <div class="tab-buttons">
                    <button
                        class={move || if active_tab.get() == "channels" { "tab-button active" } else { "tab-button" }}
                        on:click=move |_| set_active_tab.set("channels".to_string())
                    >
                        "Notification Channels ("{notification_channels.get().len()}")"
                    </button>
                    <button
                        class={move || if active_tab.get() == "webhooks" { "tab-button active" } else { "tab-button" }}
                        on:click=move |_| set_active_tab.set("webhooks".to_string())
                    >
                        "Webhooks ("{webhook_configs.get().len()}")"
                    </button>
                </div>

                <div class="tab-content">
                    {move || match active_tab.get().as_str() {
                        "channels" => view! {
                            <div class="channels-section">
                                {move || if loading.get() {
                                    view! { <div class="loading">"Loading notification channels..."</div> }.into_view()
                                } else if notification_channels.get().is_empty() {
                                    view! { <div class="empty-state">"No notification channels configured"</div> }.into_view()
                                } else {
                                    view! {
                                        <div class="channels-grid">
                                            {notification_channels.get().into_iter().map(|channel| {
                                                let channel_clone = channel.clone();
                                                let channel_clone2 = channel.clone();
                                                let channel_clone3 = channel.clone();
                                                let status_color = get_channel_status_color(&channel);
                                                let icon = get_channel_icon(&channel.channel_type);
                                                let name = channel.name.clone();
                                                let channel_type = channel.channel_type.clone();
                                                let enabled = channel.enabled;
                                                let enabled_text = if enabled { "Enabled" } else { "Disabled" };
                                                let description = channel.description.clone();
                                                let success_count = channel.success_count;
                                                let failure_count = channel.failure_count;
                                                let last_used = channel.last_used;
                                                let config = channel.config.clone();

                                                view! {
                                                    <div class="channel-card">
                                                        <div class="card-header">
                                                            <div class="channel-info">
                                                                <div class="channel-title">
                                                                    <span class="channel-icon">{icon}</span>
                                                                    <h3>{name}</h3>
                                                                </div>
                                                                <span class="channel-type">{channel_type}</span>
                                                                <span class={format!("status-indicator {}", status_color)}>
                                                                    {enabled_text}
                                                                </span>
                                                            </div>
                                                            <div class="card-actions">
                                                                <button
                                                                    class="btn btn-sm btn-secondary"
                                                                    on:click=move |_| {
                                                                        set_selected_channel.set(Some(channel_clone.clone()));
                                                                        init_form_with_channel(&channel_clone);
                                                                        set_show_edit_modal.set(true);
                                                                    }
                                                                >
                                                                    "Edit"
                                                                </button>
                                                                <button
                                                                    class="btn btn-sm btn-primary"
                                                                    on:click=move |_| {
                                                                        set_selected_channel.set(Some(channel_clone2.clone()));
                                                                        set_show_test_modal.set(true);
                                                                    }
                                                                    disabled=!enabled
                                                                >
                                                                    "Test"
                                                                </button>
                                                                <button
                                                                    class="btn btn-sm btn-danger"
                                                                    on:click=move |_| {
                                                                        if web_sys::window()
                                                                            .unwrap()
                                                                            .confirm_with_message(&format!("Delete notification channel '{}'?", channel_clone3.name))
                                                                            .unwrap_or(false)
                                                                        {
                                                                            delete_channel.dispatch(channel_clone3.id.clone());
                                                                        }
                                                                    }
                                                                >
                                                                    "Delete"
                                                                </button>
                                                            </div>
                                                        </div>
                                                        <div class="card-content">
                                                            {description.map(|desc| view! {
                                                                <div class="channel-description">{desc}</div>
                                                            })}

                                                            <div class="channel-stats">
                                                                <div class="stat-item">
                                                                    <span class="stat-label">"Successful:"</span>
                                                                    <span class="stat-value success">{success_count}</span>
                                                                </div>
                                                                <div class="stat-item">
                                                                    <span class="stat-label">"Failed:"</span>
                                                                    <span class="stat-value error">{failure_count}</span>
                                                                </div>
                                                                {last_used.map(|lu| view! {
                                                                    <div class="stat-item">
                                                                        <span class="stat-label">"Last Used:"</span>
                                                                        <span class="stat-value">{lu.format("%Y-%m-%d %H:%M UTC").to_string()}</span>
                                                                    </div>
                                                                })}
                                                            </div>

                                                            <div class="channel-config">
                                                                {match config {
                                                                    NotificationChannelConfig::Email { address, smtp_server, .. } => view! {
                                                                        <div class="config-details">
                                                                            <div class="config-row">
                                                                                <span class="config-label">"Email:"</span>
                                                                                <span class="config-value">{address}</span>
                                                                            </div>
                                                                            <div class="config-row">
                                                                                <span class="config-label">"SMTP:"</span>
                                                                                <span class="config-value">{smtp_server}</span>
                                                                            </div>
                                                                        </div>
                                                                    }.into_view(),
                                                                    NotificationChannelConfig::Slack { channel: slack_channel, .. } => view! {
                                                                        <div class="config-details">
                                                                            {slack_channel.map(|ch| view! {
                                                                                <div class="config-row">
                                                                                    <span class="config-label">"Channel:"</span>
                                                                                    <span class="config-value">{"#"}{ch}</span>
                                                                                </div>
                                                                            })}
                                                                        </div>
                                                                    }.into_view(),
                                                                    NotificationChannelConfig::Teams { .. } => view! {
                                                                        <div class="config-details">
                                                                            <div class="config-row">
                                                                                <span class="config-label">"Type:"</span>
                                                                                <span class="config-value">"Microsoft Teams"</span>
                                                                            </div>
                                                                        </div>
                                                                    }.into_view(),
                                                                    NotificationChannelConfig::Discord { username, .. } => view! {
                                                                        <div class="config-details">
                                                                            {username.map(|usr| view! {
                                                                                <div class="config-row">
                                                                                    <span class="config-label">"Username:"</span>
                                                                                    <span class="config-value">{usr}</span>
                                                                                </div>
                                                                            })}
                                                                        </div>
                                                                    }.into_view(),
                                                                    NotificationChannelConfig::PagerDuty { severity, .. } => view! {
                                                                        <div class="config-details">
                                                                            <div class="config-row">
                                                                                <span class="config-label">"Severity:"</span>
                                                                                <span class="config-value">{severity}</span>
                                                                            </div>
                                                                        </div>
                                                                    }.into_view(),
                                                                    NotificationChannelConfig::Webhook { method, .. } => view! {
                                                                        <div class="config-details">
                                                                            <div class="config-row">
                                                                                <span class="config-label">"Method:"</span>
                                                                                <span class="config-value">{method}</span>
                                                                            </div>
                                                                        </div>
                                                                    }.into_view(),
                                                                }}
                                                            </div>
                                                        </div>
                                                    </div>
                                                }
                                            }).collect::<Vec<_>>()}
                                        </div>
                                    }.into_view()
                                }}
                            </div>
                        }.into_view(),

                        "webhooks" => view! {
                            <div class="webhooks-section">
                                <div class="coming-soon">
                                    "Advanced webhook configuration coming soon"
                                </div>
                            </div>
                        }.into_view(),

                        _ => view! {}.into_view(),
                    }}
                </div>
            </div>

            // Create Channel Modal
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
                                <h2>"Create Notification Channel"</h2>
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
                                    <label>"Channel Name"</label>
                                    <input
                                        type="text"
                                        prop:value=form_name
                                        on:input=move |ev| set_form_name.set(event_target_value(&ev))
                                        placeholder="Enter channel name"
                                    />
                                </div>

                                <div class="form-group">
                                    <label>"Description (Optional)"</label>
                                    <textarea
                                        prop:value=form_description
                                        on:input=move |ev| set_form_description.set(event_target_value(&ev))
                                        placeholder="Describe this notification channel"
                                        rows="2"
                                    ></textarea>
                                </div>

                                <div class="form-group">
                                    <label>"Channel Type"</label>
                                    <select
                                        prop:value=form_type
                                        on:change=move |ev| set_form_type.set(event_target_value(&ev))
                                    >
                                        <option value="email">"📧 Email"</option>
                                        <option value="slack">"📢 Slack"</option>
                                        <option value="teams">"👥 Microsoft Teams"</option>
                                        <option value="discord">"💬 Discord"</option>
                                        <option value="pagerduty">"🚨 PagerDuty"</option>
                                        <option value="webhook">"🔗 Custom Webhook"</option>
                                    </select>
                                </div>

                                // Channel-specific configuration
                                {move || match form_type.get().as_str() {
                                    "email" => view! {
                                        <div class="channel-config-section">
                                            <h3>"Email Configuration"</h3>
                                            <div class="form-group">
                                                <label>"Email Address"</label>
                                                <input
                                                    type="email"
                                                    prop:value=form_email_address
                                                    on:input=move |ev| set_form_email_address.set(event_target_value(&ev))
                                                    placeholder="alerts@example.com"
                                                />
                                            </div>
                                            <div class="form-row">
                                                <div class="form-group">
                                                    <label>"SMTP Server"</label>
                                                    <input
                                                        type="text"
                                                        prop:value=form_smtp_server
                                                        on:input=move |ev| set_form_smtp_server.set(event_target_value(&ev))
                                                        placeholder="smtp.example.com"
                                                    />
                                                </div>
                                                <div class="form-group">
                                                    <label>"SMTP Port"</label>
                                                    <input
                                                        type="number"
                                                        prop:value=form_smtp_port
                                                        on:input=move |ev| set_form_smtp_port.set(event_target_value(&ev).parse().unwrap_or(587))
                                                        placeholder="587"
                                                    />
                                                </div>
                                            </div>
                                            <div class="form-row">
                                                <div class="form-group">
                                                    <label>"Username"</label>
                                                    <input
                                                        type="text"
                                                        prop:value=form_smtp_username
                                                        on:input=move |ev| set_form_smtp_username.set(event_target_value(&ev))
                                                        placeholder="SMTP username"
                                                    />
                                                </div>
                                                <div class="form-group">
                                                    <label>"Password"</label>
                                                    <input
                                                        type="password"
                                                        prop:value=form_smtp_password
                                                        on:input=move |ev| set_form_smtp_password.set(event_target_value(&ev))
                                                        placeholder="SMTP password"
                                                    />
                                                </div>
                                            </div>
                                            <div class="form-group">
                                                <label class="checkbox-label">
                                                    <input
                                                        type="checkbox"
                                                        prop:checked=form_use_tls
                                                        on:input=move |ev| set_form_use_tls.set(event_target_checked(&ev))
                                                    />
                                                    "Use TLS encryption"
                                                </label>
                                            </div>
                                        </div>
                                    }.into_view(),

                                    "slack" => view! {
                                        <div class="channel-config-section">
                                            <h3>"Slack Configuration"</h3>
                                            <div class="form-group">
                                                <label>"Webhook URL"</label>
                                                <input
                                                    type="url"
                                                    prop:value=form_slack_webhook_url
                                                    on:input=move |ev| set_form_slack_webhook_url.set(event_target_value(&ev))
                                                    placeholder="https://hooks.slack.com/services/..."
                                                />
                                            </div>
                                            <div class="form-row">
                                                <div class="form-group">
                                                    <label>"Channel (Optional)"</label>
                                                    <input
                                                        type="text"
                                                        prop:value=form_slack_channel
                                                        on:input=move |ev| set_form_slack_channel.set(event_target_value(&ev))
                                                        placeholder="alerts"
                                                    />
                                                </div>
                                                <div class="form-group">
                                                    <label>"Username (Optional)"</label>
                                                    <input
                                                        type="text"
                                                        prop:value=form_slack_username
                                                        on:input=move |ev| set_form_slack_username.set(event_target_value(&ev))
                                                        placeholder="Horcrux Alerts"
                                                    />
                                                </div>
                                            </div>
                                            <div class="form-group">
                                                <label>"Icon Emoji (Optional)"</label>
                                                <input
                                                    type="text"
                                                    prop:value=form_slack_icon_emoji
                                                    on:input=move |ev| set_form_slack_icon_emoji.set(event_target_value(&ev))
                                                    placeholder=":warning:"
                                                />
                                            </div>
                                        </div>
                                    }.into_view(),

                                    "teams" => view! {
                                        <div class="channel-config-section">
                                            <h3>"Microsoft Teams Configuration"</h3>
                                            <div class="form-group">
                                                <label>"Webhook URL"</label>
                                                <input
                                                    type="url"
                                                    prop:value=form_teams_webhook_url
                                                    on:input=move |ev| set_form_teams_webhook_url.set(event_target_value(&ev))
                                                    placeholder="https://outlook.office.com/webhook/..."
                                                />
                                            </div>
                                        </div>
                                    }.into_view(),

                                    "discord" => view! {
                                        <div class="channel-config-section">
                                            <h3>"Discord Configuration"</h3>
                                            <div class="form-group">
                                                <label>"Webhook URL"</label>
                                                <input
                                                    type="url"
                                                    prop:value=form_discord_webhook_url
                                                    on:input=move |ev| set_form_discord_webhook_url.set(event_target_value(&ev))
                                                    placeholder="https://discord.com/api/webhooks/..."
                                                />
                                            </div>
                                            <div class="form-group">
                                                <label>"Username (Optional)"</label>
                                                <input
                                                    type="text"
                                                    prop:value=form_discord_username
                                                    on:input=move |ev| set_form_discord_username.set(event_target_value(&ev))
                                                    placeholder="Horcrux Alerts"
                                                />
                                            </div>
                                        </div>
                                    }.into_view(),

                                    "pagerduty" => view! {
                                        <div class="channel-config-section">
                                            <h3>"PagerDuty Configuration"</h3>
                                            <div class="form-group">
                                                <label>"Routing Key"</label>
                                                <input
                                                    type="text"
                                                    prop:value=form_pagerduty_routing_key
                                                    on:input=move |ev| set_form_pagerduty_routing_key.set(event_target_value(&ev))
                                                    placeholder="Integration routing key"
                                                />
                                            </div>
                                            <div class="form-group">
                                                <label>"Severity"</label>
                                                <select
                                                    prop:value=form_pagerduty_severity
                                                    on:change=move |ev| set_form_pagerduty_severity.set(event_target_value(&ev))
                                                >
                                                    <option value="info">"Info"</option>
                                                    <option value="warning">"Warning"</option>
                                                    <option value="error">"Error"</option>
                                                    <option value="critical">"Critical"</option>
                                                </select>
                                            </div>
                                        </div>
                                    }.into_view(),

                                    "webhook" => view! {
                                        <div class="channel-config-section">
                                            <h3>"Webhook Configuration"</h3>
                                            <div class="form-group">
                                                <label>"Webhook URL"</label>
                                                <input
                                                    type="url"
                                                    prop:value=form_webhook_url
                                                    on:input=move |ev| set_form_webhook_url.set(event_target_value(&ev))
                                                    placeholder="https://api.example.com/webhooks/alerts"
                                                />
                                            </div>
                                            <div class="form-group">
                                                <label>"HTTP Method"</label>
                                                <select
                                                    prop:value=form_webhook_method
                                                    on:change=move |ev| set_form_webhook_method.set(event_target_value(&ev))
                                                >
                                                    <option value="POST">"POST"</option>
                                                    <option value="PUT">"PUT"</option>
                                                    <option value="PATCH">"PATCH"</option>
                                                </select>
                                            </div>
                                            <div class="form-group">
                                                <label>"Headers (one per line, Header: Value)"</label>
                                                <textarea
                                                    prop:value=form_webhook_headers
                                                    on:input=move |ev| set_form_webhook_headers.set(event_target_value(&ev))
                                                    placeholder="Content-Type: application/json\nAuthorization: Bearer token123"
                                                    rows="3"
                                                ></textarea>
                                            </div>
                                        </div>
                                    }.into_view(),

                                    _ => view! {}.into_view(),
                                }}

                                <div class="form-group">
                                    <label class="checkbox-label">
                                        <input
                                            type="checkbox"
                                            prop:checked=form_enabled
                                            on:input=move |ev| set_form_enabled.set(event_target_checked(&ev))
                                        />
                                        "Enable this channel"
                                    </label>
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
                                    on:click=move |_| create_channel.dispatch(())
                                    disabled=move || form_name.get().is_empty() || loading.get()
                                >"Create Channel"</button>
                            </div>
                        </div>
                    </div>
                }.into_view()
            } else {
                view! {}.into_view()
            }}

            // Test Channel Modal
            {move || if show_test_modal.get() {
                view! {
                    <div class="modal-overlay" on:click=move |e: MouseEvent| {
                        if e.target().and_then(|t| t.dyn_ref::<web_sys::Element>().map(|el| el.class_name())).unwrap_or_default() == "modal-overlay" {
                            set_show_test_modal.set(false);
                            set_selected_channel.set(None);
                        }
                    }>
                        <div class="modal-content">
                            <div class="modal-header">
                                <h2>"Test Notification Channel"</h2>
                                <button
                                    class="modal-close"
                                    on:click=move |_| {
                                        set_show_test_modal.set(false);
                                        set_selected_channel.set(None);
                                    }
                                >"x"</button>
                            </div>
                            <div class="modal-body">
                                {selected_channel.get().map(|channel| view! {
                                    <div class="test-channel-info">
                                        <h3>"Testing: "{&channel.name}</h3>
                                        <p>"This will send a test notification to verify the channel configuration."</p>
                                        <div class="channel-summary">
                                            <div class="summary-row">
                                                <span class="label">"Type:"</span>
                                                <span class="value">{&channel.channel_type}</span>
                                            </div>
                                            <div class="summary-row">
                                                <span class="label">"Status:"</span>
                                                <span class="value">{if channel.enabled { "Enabled" } else { "Disabled" }}</span>
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
                                        set_selected_channel.set(None);
                                    }
                                >"Cancel"</button>
                                <button
                                    class="btn btn-primary"
                                    on:click=move |_| {
                                        if let Some(channel) = selected_channel.get() {
                                            test_channel.dispatch(channel.id);
                                        }
                                    }
                                    disabled=loading
                                >"Send Test Notification"</button>
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