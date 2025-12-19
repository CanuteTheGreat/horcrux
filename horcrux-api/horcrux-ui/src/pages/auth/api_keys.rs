//! API Key Management
//!
//! Manage API tokens for programmatic access to Horcrux.
//! Provides token generation, usage tracking, and security monitoring.

use leptos::*;
use crate::api;
use std::collections::HashMap;

/// API key management page component
#[component]
pub fn ApiKeysPage() -> impl IntoView {
    let (users, set_users) = create_signal(Vec::<api::User>::new());
    let (user_tokens, set_user_tokens) = create_signal(HashMap::<String, Vec<api::ApiToken>>::new());
    let (selected_user, set_selected_user) = create_signal(None::<String>);
    let (loading, set_loading) = create_signal(true);
    let (error_message, set_error_message) = create_signal(None::<String>);
    let (success_message, set_success_message) = create_signal(None::<String>);
    let (show_create_form, set_show_create_form) = create_signal(false);

    // Load users on mount
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);
            set_error_message.set(None);

            match api::get_users().await {
                Ok(users_data) => {
                    set_users.set(users_data.clone());

                    // Load tokens for each user
                    let mut tokens_map = HashMap::new();
                    for user in users_data {
                        match api::get_user_api_tokens(&user.id).await {
                            Ok(tokens) => {
                                tokens_map.insert(user.id.clone(), tokens);
                            }
                            Err(_) => {
                                // User might not have tokens, that's okay
                                tokens_map.insert(user.id.clone(), Vec::new());
                            }
                        }
                    }
                    set_user_tokens.set(tokens_map);
                }
                Err(e) => {
                    set_error_message.set(Some(format!("Failed to load users: {}", e)));
                }
            }

            set_loading.set(false);
        });
    });

    // Refresh tokens for a specific user
    let refresh_user_tokens = move |user_id: String| {
        spawn_local(async move {
            match api::get_user_api_tokens(&user_id).await {
                Ok(tokens) => {
                    set_user_tokens.update(|map| {
                        map.insert(user_id, tokens);
                    });
                }
                Err(e) => {
                    set_error_message.set(Some(format!("Failed to refresh tokens: {}", e)));
                }
            }
        });
    };

    // Delete API token
    let delete_token = move |user_id: String, token_id: String| {
        let user_id_clone = user_id.clone();
        spawn_local(async move {
            match api::delete_api_token(&user_id, &token_id).await {
                Ok(()) => {
                    refresh_user_tokens(user_id_clone);
                    set_success_message.set(Some("API token deleted successfully".to_string()));
                }
                Err(e) => {
                    set_error_message.set(Some(format!("Failed to delete token: {}", e)));
                }
            }
        });
    };

    // Toggle API token enabled status
    let toggle_token = move |user_id: String, token_id: String, enabled: bool| {
        let user_id_clone = user_id.clone();
        spawn_local(async move {
            match api::toggle_api_token(&user_id, &token_id, !enabled).await {
                Ok(()) => {
                    refresh_user_tokens(user_id_clone);
                    set_success_message.set(Some(format!(
                        "API token {} successfully",
                        if enabled { "disabled" } else { "enabled" }
                    )));
                }
                Err(e) => {
                    set_error_message.set(Some(format!("Failed to update token: {}", e)));
                }
            }
        });
    };

    // Calculate token statistics
    let token_stats = move || {
        let tokens_map = user_tokens.get();
        let mut total_tokens = 0;
        let mut active_tokens = 0;
        let mut expired_tokens = 0;

        let now = chrono::Utc::now().timestamp();

        for tokens in tokens_map.values() {
            for token in tokens {
                total_tokens += 1;
                if token.enabled {
                    if let Some(expire) = token.expire {
                        if expire <= now {
                            expired_tokens += 1;
                        } else {
                            active_tokens += 1;
                        }
                    } else {
                        active_tokens += 1;
                    }
                }
            }
        }

        (total_tokens, active_tokens, expired_tokens)
    };

    // Format timestamp
    let format_timestamp = |timestamp: i64| {
        let dt = chrono::DateTime::from_timestamp(timestamp, 0);
        match dt {
            Some(dt) => dt.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
            None => "Invalid timestamp".to_string(),
        }
    };

    // Check if token is expired
    fn is_token_expired(expire: Option<i64>) -> bool {
        if let Some(exp) = expire {
            exp <= chrono::Utc::now().timestamp()
        } else {
            false
        }
    }

    // Get token status description
    fn get_token_status(token: &api::ApiToken) -> (&'static str, &'static str) {
        if !token.enabled {
            ("Disabled", "disabled")
        } else if is_token_expired(token.expire) {
            ("Expired", "expired")
        } else {
            ("Active", "active")
        }
    }

    view! {
        <div class="api-keys-management-page">
            <div class="page-header">
                <div class="header-content">
                    <h1>"API Key Management"</h1>
                    <p class="description">
                        "Manage API tokens for programmatic access to Horcrux resources and services"
                    </p>
                </div>

                <div class="header-actions">
                    <select
                        class="user-selector"
                        on:change=move |ev| {
                            let value = event_target_value(&ev);
                            if value.is_empty() {
                                set_selected_user.set(None);
                            } else {
                                set_selected_user.set(Some(value));
                            }
                            set_show_create_form.set(false);
                        }
                    >
                        <option value="">"Select a user to manage tokens..."</option>
                        {move || users.get().into_iter().map(|user| {
                            let token_count = user_tokens.get()
                                .get(&user.id)
                                .map(|tokens| tokens.len())
                                .unwrap_or(0);

                            view! {
                                <option value={user.id.clone()}>
                                    {format!("{} ({} token{})", user.username, token_count, if token_count == 1 { "" } else { "s" })}
                                </option>
                            }
                        }).collect_view()}
                    </select>

                    {move || if selected_user.get().is_some() {
                        view! {
                            <button
                                class="btn btn-primary"
                                on:click=move |_| set_show_create_form.set(!show_create_form.get())
                            >
                                <span class="icon">"+"</span>
                                {move || if show_create_form.get() { "Cancel" } else { "Create Token" }}
                            </button>
                        }.into_view()
                    } else {
                        view! {}.into_view()
                    }}
                </div>
            </div>

            {move || error_message.get().map(|msg| view! {
                <div class="alert alert-error">
                    <span class="alert-icon">"!"</span>
                    <span class="alert-message">{msg}</span>
                    <button
                        class="alert-close"
                        on:click=move |_| set_error_message.set(None)
                    >"x"</button>
                </div>
            })}

            {move || success_message.get().map(|msg| view! {
                <div class="alert alert-success">
                    <span class="alert-icon">"[OK]"</span>
                    <span class="alert-message">{msg}</span>
                    <button
                        class="alert-close"
                        on:click=move |_| set_success_message.set(None)
                    >"x"</button>
                </div>
            })}

            <div class="token-stats">
                {move || {
                    let (total, active, expired) = token_stats();
                    view! {
                        <div class="stats-grid">
                            <div class="stat-card">
                                <div class="stat-value">{total}</div>
                                <div class="stat-label">"Total Tokens"</div>
                            </div>
                            <div class="stat-card">
                                <div class="stat-value">{active}</div>
                                <div class="stat-label">"Active Tokens"</div>
                            </div>
                            <div class="stat-card" class:warning=move || { expired > 0 }>
                                <div class="stat-value">{expired}</div>
                                <div class="stat-label">"Expired Tokens"</div>
                            </div>
                        </div>
                    }
                }}
            </div>

            {move || if show_create_form.get() && selected_user.get().is_some() {
                let user_id = selected_user.get().unwrap();
                let selected_username = users.get()
                    .into_iter()
                    .find(|u| u.id == user_id)
                    .map(|u| u.username)
                    .unwrap_or_else(|| "Unknown".to_string());

                view! {
                    <CreateApiTokenForm
                        user_id=user_id.clone()
                        username=selected_username
                        on_success={
                            let set_show_create_form = set_show_create_form.clone();
                            let user_id_clone = user_id.clone();
                            let set_success_message = set_success_message.clone();
                            move || {
                                set_show_create_form.set(false);
                                refresh_user_tokens(user_id_clone.clone());
                                set_success_message.set(Some("API token created successfully".to_string()));
                            }
                        }
                        on_error={
                            let set_error_message = set_error_message.clone();
                            move |msg| set_error_message.set(Some(msg))
                        }
                    />
                }.into_view()
            } else {
                view! {}.into_view()
            }}

            <div class="tokens-container">
                {move || if loading.get() {
                    view! {
                        <div class="loading-container">
                            <div class="spinner"></div>
                            <p>"Loading API tokens..."</p>
                        </div>
                    }.into_view()
                } else if selected_user.get().is_none() {
                    view! {
                        <div class="select-user-prompt">
                            <div class="prompt-icon">"🔑"</div>
                            <h3>"Select a User"</h3>
                            <p>"Choose a user from the dropdown above to view and manage their API tokens"</p>
                        </div>
                    }.into_view()
                } else {
                    let user_id = selected_user.get().unwrap();
                    let tokens = user_tokens.get().get(&user_id).cloned().unwrap_or_default();
                    let selected_username = users.get()
                        .into_iter()
                        .find(|u| u.id == user_id)
                        .map(|u| u.username)
                        .unwrap_or_else(|| "Unknown".to_string());

                    if tokens.is_empty() {
                        view! {
                            <div class="empty-state">
                                <div class="empty-icon">"🔑"</div>
                                <h3>{format!("No API tokens for {}", selected_username)}</h3>
                                <p>"Create the first API token to enable programmatic access"</p>
                                <button
                                    class="btn btn-primary"
                                    on:click=move |_| set_show_create_form.set(true)
                                >
                                    "Create First Token"
                                </button>
                            </div>
                        }.into_view()
                    } else {
                        view! {
                            <div class="tokens-section">
                                <div class="section-header">
                                    <h3>{format!("API Tokens for {}", selected_username)}</h3>
                                    <p class="token-count">
                                        {format!("{} token{}", tokens.len(), if tokens.len() == 1 { "" } else { "s" })}
                                    </p>
                                </div>

                                <div class="tokens-table">
                                    <table>
                                        <thead>
                                            <tr>
                                                <th>"Token ID"</th>
                                                <th>"Comment"</th>
                                                <th>"Status"</th>
                                                <th>"Created"</th>
                                                <th>"Expires"</th>
                                                <th>"Last Used"</th>
                                                <th>"Actions"</th>
                                            </tr>
                                        </thead>
                                        <tbody>
                                            {tokens.into_iter().map(|token| {
                                                let token_id = token.id.clone();
                                                let token_id_delete = token.id.clone();
                                                let token_id_toggle = token.id.clone();
                                                let user_id_delete = user_id.clone();
                                                let user_id_toggle = user_id.clone();
                                                let (status_text, status_class) = get_token_status(&token);
                                                let is_expired = is_token_expired(token.expire);

                                                view! {
                                                    <tr class={format!("token-row {}", status_class)}>
                                                        <td>
                                                            <code class="token-id">
                                                                {token_id.chars().take(12).collect::<String>()}...
                                                            </code>
                                                        </td>
                                                        <td>
                                                            <span class="token-comment">
                                                                {token.comment.unwrap_or_else(|| "No comment".to_string())}
                                                            </span>
                                                        </td>
                                                        <td>
                                                            <span class={format!("status-badge {}", status_class)}>
                                                                {status_text}
                                                            </span>
                                                        </td>
                                                        <td>
                                                            {token.created_at.as_ref().map(|created| format_timestamp(created.parse().unwrap_or(0))).unwrap_or_else(|| "Unknown".to_string())}
                                                        </td>
                                                        <td>
                                                            {match token.expire {
                                                                Some(exp) => format_timestamp(exp),
                                                                None => "Never".to_string(),
                                                            }}
                                                        </td>
                                                        <td>
                                                            {token.last_used.as_ref().map(|used| format_timestamp(used.parse().unwrap_or(0))).unwrap_or_else(|| "Never".to_string())}
                                                        </td>
                                                        <td class="actions-cell">
                                                            <div class="action-buttons">
                                                                {if !is_expired {
                                                                    view! {
                                                                        <button
                                                                            class="btn-icon"
                                                                            title={if token.enabled { "Disable Token" } else { "Enable Token" }}
                                                                            on:click=move |_| toggle_token(user_id_toggle.clone(), token_id_toggle.clone(), token.enabled)
                                                                        >
                                                                            {if token.enabled { "⏸" } else { "▶" }}
                                                                        </button>
                                                                    }.into_view()
                                                                } else {
                                                                    view! {}.into_view()
                                                                }}

                                                                <button
                                                                    class="btn-icon delete-btn"
                                                                    title="Delete Token"
                                                                    on:click=move |_| {
                                                                        if web_sys::window()
                                                                            .unwrap()
                                                                            .confirm_with_message("Delete this API token? This action cannot be undone.")
                                                                            .unwrap_or(false)
                                                                        {
                                                                            delete_token(user_id_delete.clone(), token_id_delete.clone());
                                                                        }
                                                                    }
                                                                >
                                                                    "🗑"
                                                                </button>
                                                            </div>
                                                        </td>
                                                    </tr>
                                                }
                                            }).collect_view()}
                                        </tbody>
                                    </table>
                                </div>
                            </div>
                        }.into_view()
                    }
                }}
            </div>
        </div>
    }
}

/// Create API token form component
#[component]
pub fn CreateApiTokenForm<F, G>(
    user_id: String,
    username: String,
    on_success: F,
    on_error: G,
) -> impl IntoView
where
    F: Fn() + 'static,
    G: Fn(String) + 'static,
{
    let (comment, set_comment) = create_signal(String::new());
    let (expire_option, set_expire_option) = create_signal("never".to_string());
    let (custom_days, set_custom_days) = create_signal(String::from("90"));
    let (creating, set_creating) = create_signal(false);
    let (generated_token, set_generated_token) = create_signal(None::<String>);

    // Wrap callbacks in Rc early
    let on_success_rc = std::rc::Rc::new(on_success);
    let on_error_rc = std::rc::Rc::new(on_error);

    let calculate_expiry = move || -> Option<i64> {
        match expire_option.get().as_str() {
            "never" => None,
            "30days" => Some(chrono::Utc::now().timestamp() + 30 * 24 * 3600),
            "90days" => Some(chrono::Utc::now().timestamp() + 90 * 24 * 3600),
            "365days" => Some(chrono::Utc::now().timestamp() + 365 * 24 * 3600),
            "custom" => {
                if let Ok(days) = custom_days.get().parse::<i64>() {
                    Some(chrono::Utc::now().timestamp() + days * 24 * 3600)
                } else {
                    None
                }
            }
            _ => None,
        }
    };

    let on_success_clone = on_success_rc.clone();
    let on_error_clone = on_error_rc.clone();
    let user_id_for_submit = user_id.clone();
    let submit_form = {
        let on_success_for_submit = on_success_clone.clone();
        let on_error_for_submit = on_error_clone.clone();
        let user_id_for_submit = user_id_for_submit.clone();
        move |ev: web_sys::SubmitEvent| {
            ev.prevent_default();

            let request = api::CreateApiTokenRequest {
                comment: if comment.get().trim().is_empty() {
                    None
                } else {
                    Some(comment.get().trim().to_string())
                },
                expire: calculate_expiry(),
                permissions: None, // Will inherit user permissions
            };

            set_creating.set(true);

            let user_id_clone = user_id_for_submit.clone();
            let on_success_inner = on_success_for_submit.clone();
            let on_error_inner = on_error_for_submit.clone();

            spawn_local(async move {
                match api::create_api_token(&user_id_clone, request).await {
                    Ok(token) => {
                        set_generated_token.set(Some(token.id));
                        on_success_inner();
                    }
                    Err(e) => {
                        on_error_inner(format!("Failed to create API token: {}", e));
                    }
                }
                set_creating.set(false);
            });
        }
    };

    let on_success_for_submit = on_success_clone.clone();
    let on_error_for_submit = on_error_clone.clone();
    let user_id_for_submit2 = user_id_for_submit.clone();

    view! {
        <div class="create-token-form-container">
            {move || if let Some(token_id) = generated_token.get() {
                let token_for_input = token_id.clone();
                let token_for_example = token_id.clone();
                view! {
                    <div class="token-generated">
                        <div class="success-header">
                            <span class="success-icon">"✅"</span>
                            <h3>"API Token Created Successfully"</h3>
                        </div>

                        <div class="token-display">
                            <label>"Your new API token:"</label>
                            <div class="token-value">
                                <input
                                    type="text"
                                    value={token_for_input}
                                    readonly=true
                                    class="token-input"
                                />
                                <button
                                    class="btn btn-secondary"
                                    on:click=move |_| {
                                        // Simple notification instead of clipboard API
                                        web_sys::console::log_1(&"Please copy the token manually".into());
                                    }
                                >
                                    "Copy"
                                </button>
                            </div>

                            <div class="security-warning">
                                <strong>"[!] Security Notice:"</strong>
                                " Please copy this token now. You won't be able to see it again for security reasons."
                            </div>
                        </div>

                        <div class="token-usage-info">
                            <h4>"Using Your API Token"</h4>
                            <p>"Include this token in your API requests:"</p>
                            <pre class="code-example">
{format!(r#"curl -H "Authorization: Bearer {}" \
     -H "Content-Type: application/json" \
     http://localhost:8006/api/vms"#, token_for_example)}
                            </pre>
                        </div>
                    </div>
                }.into_view()
            } else {
                let on_success_inner2 = on_success_for_submit.clone();
                let on_error_inner2 = on_error_for_submit.clone();
                let user_id_inner = user_id_for_submit2.clone();
                view! {
                    <form class="create-token-form" on:submit=move |ev: web_sys::SubmitEvent| {
                        ev.prevent_default();

                        let request = api::CreateApiTokenRequest {
                            comment: if comment.get().trim().is_empty() {
                                None
                            } else {
                                Some(comment.get().trim().to_string())
                            },
                            expire: calculate_expiry(),
                            permissions: None,
                        };

                        set_creating.set(true);

                        let user_id_clone = user_id_inner.clone();
                        let on_success_inner = on_success_inner2.clone();
                        let on_error_inner = on_error_inner2.clone();

                        spawn_local(async move {
                            match api::create_api_token(&user_id_clone, request).await {
                                Ok(token) => {
                                    set_generated_token.set(Some(token.id));
                                    on_success_inner();
                                }
                                Err(e) => {
                                    on_error_inner(format!("Failed to create API token: {}", e));
                                }
                            }
                            set_creating.set(false);
                        });
                    }>
                        <h3>{format!("Create API Token for {}", username)}</h3>

                        <div class="form-group">
                            <label for="token-comment">"Comment (Optional)"</label>
                            <input
                                type="text"
                                id="token-comment"
                                prop:value=move || comment.get()
                                on:input=move |ev| set_comment.set(event_target_value(&ev))
                                placeholder="e.g., Production monitoring, CI/CD pipeline"
                                maxlength="200"
                            />
                            <small>"A description to help identify this token's purpose"</small>
                        </div>

                        <div class="form-group">
                            <label>"Token Expiration"</label>
                            <div class="expiry-options">
                                <label class="radio-option">
                                    <input
                                        type="radio"
                                        name="expiry"
                                        value="never"
                                        checked=move || expire_option.get() == "never"
                                        on:change=move |_| set_expire_option.set("never".to_string())
                                    />
                                    <span>"Never expires"</span>
                                </label>

                                <label class="radio-option">
                                    <input
                                        type="radio"
                                        name="expiry"
                                        value="30days"
                                        checked=move || expire_option.get() == "30days"
                                        on:change=move |_| set_expire_option.set("30days".to_string())
                                    />
                                    <span>"30 days"</span>
                                </label>

                                <label class="radio-option">
                                    <input
                                        type="radio"
                                        name="expiry"
                                        value="90days"
                                        checked=move || expire_option.get() == "90days"
                                        on:change=move |_| set_expire_option.set("90days".to_string())
                                    />
                                    <span>"90 days (recommended)"</span>
                                </label>

                                <label class="radio-option">
                                    <input
                                        type="radio"
                                        name="expiry"
                                        value="365days"
                                        checked=move || expire_option.get() == "365days"
                                        on:change=move |_| set_expire_option.set("365days".to_string())
                                    />
                                    <span>"1 year"</span>
                                </label>

                                <label class="radio-option">
                                    <input
                                        type="radio"
                                        name="expiry"
                                        value="custom"
                                        checked=move || expire_option.get() == "custom"
                                        on:change=move |_| set_expire_option.set("custom".to_string())
                                    />
                                    <span>"Custom:"</span>
                                    <input
                                        type="number"
                                        prop:value=move || custom_days.get()
                                        on:input=move |ev| set_custom_days.set(event_target_value(&ev))
                                        min="1"
                                        max="3650"
                                        class="custom-days-input"
                                        disabled=move || expire_option.get() != "custom"
                                    />
                                    <span>"days"</span>
                                </label>
                            </div>
                        </div>

                        <div class="security-notice">
                            <h4>"🔒 Security Best Practices"</h4>
                            <ul>
                                <li>"Store the token securely and never share it publicly"</li>
                                <li>"Use environment variables instead of hardcoding in code"</li>
                                <li>"Rotate tokens regularly for enhanced security"</li>
                                <li>"Set appropriate expiration dates for automated systems"</li>
                                <li>"Revoke unused or compromised tokens immediately"</li>
                            </ul>
                        </div>

                        <div class="form-actions">
                            <button
                                type="submit"
                                class="btn btn-primary"
                                disabled=move || creating.get()
                            >
                                {move || if creating.get() { "Creating Token..." } else { "Create API Token" }}
                            </button>
                        </div>
                    </form>
                }.into_view()
            }}
        </div>
    }
}