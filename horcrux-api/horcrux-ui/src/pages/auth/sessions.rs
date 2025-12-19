//! Session Management Dashboard
//!
//! Monitor active user sessions and provide session termination capabilities.
//! Shows real-time session information with security event monitoring.

use leptos::*;
use crate::api;
use std::collections::HashMap;

/// Session management page component
#[component]
pub fn SessionsPage() -> impl IntoView {
    let (sessions, set_sessions) = create_signal(Vec::<api::UserSession>::new());
    let (loading, set_loading) = create_signal(true);
    let (error_message, set_error_message) = create_signal(None::<String>);
    let (auto_refresh, set_auto_refresh) = create_signal(true);
    let (filter_user, set_filter_user) = create_signal(String::new());
    let (filter_realm, set_filter_realm) = create_signal(String::new());

    // Auto-refresh sessions every 30 seconds
    let refresh_sessions = move || {
        spawn_local(async move {
            set_loading.set(true);
            match api::get_active_sessions().await {
                Ok(sessions_data) => {
                    set_sessions.set(sessions_data);
                    set_error_message.set(None);
                }
                Err(e) => {
                    set_error_message.set(Some(format!("Failed to load sessions: {}", e)));
                }
            }
            set_loading.set(false);
        });
    };

    // Initial load
    create_effect(move |_| {
        refresh_sessions();
    });

    // Auto-refresh timer
    create_effect(move |_| {
        if auto_refresh.get() {
            let timer = set_interval_with_handle(
                move || {
                    if auto_refresh.get() {
                        refresh_sessions();
                    }
                },
                std::time::Duration::from_secs(30),
            ).expect("Failed to set interval");

            on_cleanup(move || {
                timer.clear();
            });
        }
    });

    // Terminate session
    let terminate_session = move |session_id: String| {
        spawn_local(async move {
            match api::terminate_session(&session_id).await {
                Ok(()) => {
                    refresh_sessions();
                }
                Err(e) => {
                    set_error_message.set(Some(format!("Failed to terminate session: {}", e)));
                }
            }
        });
    };

    // Terminate all sessions for a user
    let terminate_user_sessions = move |username: String, realm: String| {
        let user_id = format!("{}@{}", username, realm);
        spawn_local(async move {
            match api::terminate_user_sessions(&user_id).await {
                Ok(()) => {
                    refresh_sessions();
                }
                Err(e) => {
                    set_error_message.set(Some(format!("Failed to terminate user sessions: {}", e)));
                }
            }
        });
    };

    // Filter sessions
    let filtered_sessions = move || {
        let user_filter = filter_user.get().to_lowercase();
        let realm_filter = filter_realm.get();

        sessions.get()
            .into_iter()
            .filter(|session| {
                if !user_filter.is_empty() {
                    if !session.username.to_lowercase().contains(&user_filter) {
                        return false;
                    }
                }

                if !realm_filter.is_empty() && realm_filter != "all" {
                    if session.realm != realm_filter {
                        return false;
                    }
                }

                true
            })
            .collect::<Vec<_>>()
    };

    // Get unique realms for filter
    let available_realms = move || {
        let mut realms = std::collections::HashSet::new();
        for session in sessions.get() {
            realms.insert(session.realm.clone());
        }
        realms.into_iter().collect::<Vec<_>>()
    };

    // Group sessions by user
    let sessions_by_user = move || {
        let mut grouped: HashMap<String, Vec<api::UserSession>> = HashMap::new();
        for session in filtered_sessions() {
            let user_key = format!("{}@{}", session.username, session.realm);
            grouped.entry(user_key).or_insert_with(Vec::new).push(session);
        }
        grouped
    };

    // Calculate session statistics
    let session_stats = move || {
        let all_sessions = sessions.get();
        let total_sessions = all_sessions.len();
        let unique_users = all_sessions.iter()
            .map(|s| format!("{}@{}", s.username, s.realm))
            .collect::<std::collections::HashSet<_>>()
            .len();

        let now = chrono::Utc::now().timestamp();
        let expiring_soon = all_sessions.iter()
            .filter(|s| s.expires - now < 3600) // Expiring within 1 hour
            .count();

        (total_sessions, unique_users, expiring_soon)
    };

    // Format timestamp
    let format_timestamp = |timestamp: i64| {
        let dt = chrono::DateTime::from_timestamp(timestamp, 0);
        match dt {
            Some(dt) => dt.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
            None => "Invalid timestamp".to_string(),
        }
    };

    // Calculate remaining time
    let remaining_time = move |expires: i64| {
        let now = chrono::Utc::now().timestamp();
        let remaining = expires - now;

        if remaining <= 0 {
            "Expired".to_string()
        } else if remaining < 60 {
            format!("{}s", remaining)
        } else if remaining < 3600 {
            format!("{}m", remaining / 60)
        } else if remaining < 86400 {
            format!("{}h {}m", remaining / 3600, (remaining % 3600) / 60)
        } else {
            format!("{}d {}h", remaining / 86400, (remaining % 86400) / 3600)
        }
    };

    view! {
        <div class="sessions-management-page">
            <div class="page-header">
                <div class="header-content">
                    <h1>"Active Sessions"</h1>
                    <p class="description">
                        "Monitor and manage active user sessions across all authentication realms"
                    </p>
                </div>

                <div class="header-actions">
                    <label class="auto-refresh-toggle">
                        <input
                            type="checkbox"
                            checked=move || auto_refresh.get()
                            on:change=move |_| set_auto_refresh.update(|r| *r = !*r)
                        />
                        <span class="toggle-label">"Auto-refresh"</span>
                    </label>

                    <button
                        class="btn btn-secondary"
                        on:click=move |_| refresh_sessions()
                        disabled=move || loading.get()
                    >
                        <span class="icon">"🔄"</span>
                        "Refresh"
                    </button>
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

            <div class="session-stats">
                {move || {
                    let (total, unique_users, expiring) = session_stats();
                    view! {
                        <div class="stats-grid">
                            <div class="stat-card">
                                <div class="stat-value">{total}</div>
                                <div class="stat-label">"Active Sessions"</div>
                            </div>
                            <div class="stat-card">
                                <div class="stat-value">{unique_users}</div>
                                <div class="stat-label">"Unique Users"</div>
                            </div>
                            <div class="stat-card" class:warning=move || { expiring > 0 }>
                                <div class="stat-value">{expiring}</div>
                                <div class="stat-label">"Expiring Soon"</div>
                            </div>
                        </div>
                    }
                }}
            </div>

            <div class="filters-section">
                <div class="search-filter">
                    <input
                        type="text"
                        placeholder="Filter by username..."
                        prop:value=move || filter_user.get()
                        on:input=move |ev| set_filter_user.set(event_target_value(&ev))
                        class="search-input"
                    />
                    <span class="search-icon">"🔍"</span>
                </div>

                <div class="dropdown-filters">
                    <select
                        on:change=move |ev| set_filter_realm.set(event_target_value(&ev))
                        class="filter-select"
                    >
                        <option value="">"All Realms"</option>
                        {move || available_realms().into_iter().map(|realm| {
                            view! {
                                <option value={realm.clone()}>{realm}</option>
                            }
                        }).collect_view()}
                    </select>
                </div>
            </div>

            <div class="sessions-container">
                {move || if loading.get() && sessions.get().is_empty() {
                    view! {
                        <div class="loading-container">
                            <div class="spinner"></div>
                            <p>"Loading sessions..."</p>
                        </div>
                    }.into_view()
                } else {
                    let grouped_sessions = sessions_by_user();
                    if grouped_sessions.is_empty() {
                        view! {
                            <div class="empty-state">
                                <div class="empty-icon">"🔒"</div>
                                <h3>"No active sessions"</h3>
                                <p>"No sessions match your current filters"</p>
                            </div>
                        }.into_view()
                    } else {
                        view! {
                            <div class="sessions-list">
                                {grouped_sessions.into_iter().map(|(user_key, user_sessions)| {
                                    let parts: Vec<String> = user_key.split('@').map(|s| s.to_string()).collect();
                                    let username = parts.get(0).cloned().unwrap_or_else(|| "unknown".to_string());
                                    let realm = parts.get(1).cloned().unwrap_or_else(|| "unknown".to_string());
                                    let username_for_display = username.clone();
                                    let realm_for_display = realm.clone();
                                    let username_for_action = username.clone();
                                    let realm_for_action = realm.clone();
                                    let session_count = user_sessions.len();

                                    view! {
                                        <div class="user-sessions-group">
                                            <div class="user-header">
                                                <div class="user-info">
                                                    <h3>{username_for_display}</h3>
                                                    <span class="realm-badge">{realm_for_display}</span>
                                                    <span class="session-count">
                                                        {format!("{} session{}", session_count, if session_count == 1 { "" } else { "s" })}
                                                    </span>
                                                </div>
                                                <div class="user-actions">
                                                    <button
                                                        class="btn btn-danger btn-sm"
                                                        title="Terminate all sessions for this user"
                                                        on:click=move |_| {
                                                            if web_sys::window()
                                                                .unwrap()
                                                                .confirm_with_message(&format!("Terminate all sessions for user '{}'?", username_for_action))
                                                                .unwrap_or(false)
                                                            {
                                                                terminate_user_sessions(username_for_action.clone(), realm_for_action.clone());
                                                            }
                                                        }
                                                    >
                                                        "Terminate All"
                                                    </button>
                                                </div>
                                            </div>

                                            <div class="sessions-table">
                                                <table>
                                                    <thead>
                                                        <tr>
                                                            <th>"Session ID"</th>
                                                            <th>"Created"</th>
                                                            <th>"Expires"</th>
                                                            <th>"Remaining"</th>
                                                            <th>"IP Address"</th>
                                                            <th>"User Agent"</th>
                                                            <th>"Actions"</th>
                                                        </tr>
                                                    </thead>
                                                    <tbody>
                                                        {user_sessions.into_iter().map(|session| {
                                                            let session_id = session.session_id.clone();
                                                            let session_id_terminate = session.session_id.clone();
                                                            let is_expired = session.expires <= chrono::Utc::now().timestamp();
                                                            let expires_soon = (session.expires - chrono::Utc::now().timestamp()) < 3600;

                                                            view! {
                                                                <tr class:expired=is_expired class:expires-soon=expires_soon>
                                                                    <td>
                                                                        <code class="session-id">{session_id.chars().take(12).collect::<String>()}...</code>
                                                                    </td>
                                                                    <td>{format_timestamp(session.created)}</td>
                                                                    <td>{format_timestamp(session.expires)}</td>
                                                                    <td>
                                                                        <span class={format!("remaining-time {}",
                                                                            if is_expired { "expired" } else if expires_soon { "warning" } else { "normal" }
                                                                        )}>
                                                                            {remaining_time(session.expires)}
                                                                        </span>
                                                                    </td>
                                                                    <td>
                                                                        <span class="ip-address">
                                                                            {session.ip_address.unwrap_or_else(|| "Unknown".to_string())}
                                                                        </span>
                                                                    </td>
                                                                    <td>
                                                                        <span class="user-agent" title={session.user_agent.clone().unwrap_or_else(|| "Unknown".to_string())}>
                                                                            {session.user_agent.as_ref()
                                                                                .map(|ua| {
                                                                                    if ua.len() > 50 {
                                                                                        format!("{}...", &ua[..47])
                                                                                    } else {
                                                                                        ua.clone()
                                                                                    }
                                                                                })
                                                                                .unwrap_or_else(|| "Unknown".to_string())
                                                                            }
                                                                        </span>
                                                                    </td>
                                                                    <td>
                                                                        <button
                                                                            class="btn btn-danger btn-sm"
                                                                            title="Terminate this session"
                                                                            on:click=move |_| {
                                                                                if web_sys::window()
                                                                                    .unwrap()
                                                                                    .confirm_with_message("Terminate this session?")
                                                                                    .unwrap_or(false)
                                                                                {
                                                                                    terminate_session(session_id_terminate.clone());
                                                                                }
                                                                            }
                                                                        >
                                                                            "Terminate"
                                                                        </button>
                                                                    </td>
                                                                </tr>
                                                            }
                                                        }).collect_view()}
                                                    </tbody>
                                                </table>
                                            </div>
                                        </div>
                                    }
                                }).collect_view()}
                            </div>
                        }.into_view()
                    }
                }}
            </div>
        </div>
    }
}

// Utility function to set interval with cleanup
fn set_interval_with_handle<F>(f: F, duration: std::time::Duration) -> Result<IntervalHandle, wasm_bindgen::JsValue>
where
    F: Fn() + 'static,
{
    use wasm_bindgen::{closure::Closure, JsCast};

    let callback = Closure::wrap(Box::new(f) as Box<dyn Fn()>);
    let handle = web_sys::window()
        .unwrap()
        .set_interval_with_callback_and_timeout_and_arguments_0(
            callback.as_ref().unchecked_ref(),
            duration.as_millis() as i32,
        )?;

    callback.forget(); // Prevent the closure from being dropped
    Ok(IntervalHandle { handle })
}

// Handle for managing intervals
struct IntervalHandle {
    handle: i32,
}

impl IntervalHandle {
    fn clear(self) {
        web_sys::window().unwrap().clear_interval_with_handle(self.handle);
    }
}