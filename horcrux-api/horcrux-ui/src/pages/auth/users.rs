//! User Management Dashboard
//!
//! Complete user management interface for Horcrux enterprise features.
//! Provides CRUD operations for users, role assignments, and bulk operations.

use leptos::*;
use crate::api;
use std::collections::HashSet;

/// User management page component
#[component]
pub fn UsersPage() -> impl IntoView {
    let (users, set_users) = create_signal(Vec::<api::User>::new());
    let (roles, set_roles) = create_signal(Vec::<api::Role>::new());
    let (loading, set_loading) = create_signal(true);
    let (error_message, set_error_message) = create_signal(None::<String>);
    let (show_create_form, set_show_create_form) = create_signal(false);
    let (selected_users, set_selected_users) = create_signal(HashSet::<String>::new());
    let (bulk_action_loading, set_bulk_action_loading) = create_signal(false);

    // Filter and search state
    let (search_term, set_search_term) = create_signal(String::new());
    let (realm_filter, set_realm_filter) = create_signal(String::new());
    let (role_filter, set_role_filter) = create_signal(String::new());
    let (status_filter, set_status_filter) = create_signal(String::new());

    // Load users and roles on mount
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);
            set_error_message.set(None);

            // Load users and roles in parallel
            let users_result = api::get_users().await;
            let roles_result = api::get_roles().await;

            match (users_result, roles_result) {
                (Ok(users_data), Ok(roles_data)) => {
                    set_users.set(users_data);
                    set_roles.set(roles_data);
                }
                (Err(e), _) | (_, Err(e)) => {
                    set_error_message.set(Some(format!("Failed to load data: {}", e)));
                }
            }

            set_loading.set(false);
        });
    });

    // Filtered users based on search and filters
    let filtered_users = move || {
        let search = search_term.get().to_lowercase();
        let realm_f = realm_filter.get();
        let role_f = role_filter.get();
        let status_f = status_filter.get();

        users.get()
            .into_iter()
            .filter(|user| {
                // Search filter
                if !search.is_empty() {
                    let matches_search = user.username.to_lowercase().contains(&search)
                        || user.email.to_lowercase().contains(&search)
                        || user.id.to_lowercase().contains(&search);
                    if !matches_search {
                        return false;
                    }
                }

                // Realm filter
                if !realm_f.is_empty() && realm_f != "all" {
                    if user.realm != realm_f {
                        return false;
                    }
                }

                // Role filter
                if !role_f.is_empty() && role_f != "all" {
                    if user.role != role_f && !user.roles.contains(&role_f) {
                        return false;
                    }
                }

                // Status filter
                if !status_f.is_empty() && status_f != "all" {
                    let enabled = user.enabled;
                    match status_f.as_str() {
                        "enabled" => if !enabled { return false; },
                        "disabled" => if enabled { return false; },
                        _ => {}
                    }
                }

                true
            })
            .collect::<Vec<_>>()
    };

    // Toggle user selection
    let toggle_user_selection = move |user_id: String| {
        set_selected_users.update(|selected| {
            if selected.contains(&user_id) {
                selected.remove(&user_id);
            } else {
                selected.insert(user_id);
            }
        });
    };

    // Select all filtered users
    let select_all_users = move |_| {
        let all_ids: HashSet<String> = filtered_users()
            .into_iter()
            .map(|user| user.id)
            .collect();
        set_selected_users.set(all_ids);
    };

    // Clear selection
    let clear_selection = move |_| {
        set_selected_users.set(HashSet::new());
    };

    // Delete user
    let delete_user = move |user_id: String| {
        spawn_local(async move {
            match api::delete_user(&user_id).await {
                Ok(()) => {
                    // Refresh users list
                    if let Ok(users_data) = api::get_users().await {
                        set_users.set(users_data);
                    }
                }
                Err(e) => {
                    set_error_message.set(Some(format!("Failed to delete user: {}", e)));
                }
            }
        });
    };

    // Toggle user enabled status
    let toggle_user_enabled = move |user_id: String, enabled: bool| {
        spawn_local(async move {
            match api::toggle_user(&user_id, !enabled).await {
                Ok(()) => {
                    // Refresh users list
                    if let Ok(users_data) = api::get_users().await {
                        set_users.set(users_data);
                    }
                }
                Err(e) => {
                    set_error_message.set(Some(format!("Failed to update user: {}", e)));
                }
            }
        });
    };

    // Bulk enable users
    let bulk_enable_users = move |_| {
        let selected = selected_users.get();
        if selected.is_empty() {
            return;
        }

        set_bulk_action_loading.set(true);
        spawn_local(async move {
            let mut success_count = 0;
            let mut error_count = 0;

            for user_id in selected.iter() {
                match api::toggle_user(user_id, true).await {
                    Ok(()) => success_count += 1,
                    Err(_) => error_count += 1,
                }
            }

            // Refresh users list
            if let Ok(users_data) = api::get_users().await {
                set_users.set(users_data);
            }

            set_selected_users.set(HashSet::new());
            set_bulk_action_loading.set(false);

            if error_count > 0 {
                set_error_message.set(Some(format!(
                    "Bulk enable completed: {} succeeded, {} failed",
                    success_count, error_count
                )));
            }
        });
    };

    // Bulk disable users
    let bulk_disable_users = move |_| {
        let selected = selected_users.get();
        if selected.is_empty() {
            return;
        }

        set_bulk_action_loading.set(true);
        spawn_local(async move {
            let mut success_count = 0;
            let mut error_count = 0;

            for user_id in selected.iter() {
                match api::toggle_user(user_id, false).await {
                    Ok(()) => success_count += 1,
                    Err(_) => error_count += 1,
                }
            }

            // Refresh users list
            if let Ok(users_data) = api::get_users().await {
                set_users.set(users_data);
            }

            set_selected_users.set(HashSet::new());
            set_bulk_action_loading.set(false);

            if error_count > 0 {
                set_error_message.set(Some(format!(
                    "Bulk disable completed: {} succeeded, {} failed",
                    success_count, error_count
                )));
            }
        });
    };

    // Get unique realms for filter
    let available_realms = move || {
        let mut realms: HashSet<String> = HashSet::new();
        for user in users.get() {
            realms.insert(user.realm.clone());
        }
        realms.into_iter().collect::<Vec<_>>()
    };

    // Get unique roles for filter
    let available_roles = move || {
        roles.get().into_iter().map(|r| r.name).collect::<Vec<_>>()
    };

    view! {
        <div class="users-management-page">
            <div class="page-header">
                <div class="header-content">
                    <h1>"User Management"</h1>
                    <p class="description">
                        "Manage user accounts, roles, and permissions across all authentication realms"
                    </p>
                </div>

                <div class="header-actions">
                    <button
                        class="btn btn-primary"
                        on:click=move |_| set_show_create_form.set(!show_create_form.get())
                    >
                        <span class="icon">"+"</span>
                        {move || if show_create_form.get() { "Cancel" } else { "Add User" }}
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

            {move || if show_create_form.get() {
                view! {
                    <CreateUserForm
                        roles=roles.get()
                        on_success={
                            let set_show_create_form = set_show_create_form.clone();
                            let set_users = set_users.clone();
                            move || {
                                set_show_create_form.set(false);
                                // Refresh users list
                                spawn_local(async move {
                                    if let Ok(users_data) = api::get_users().await {
                                        set_users.set(users_data);
                                    }
                                });
                            }
                        }
                        on_error={
                            let set_error_message = set_error_message.clone();
                            move |msg| set_error_message.set(Some(msg))
                        }
                    />
                }.into_view()
            } else {
                view! { <div></div> }.into_view()
            }}

            <div class="filters-section">
                <div class="search-filter">
                    <input
                        type="text"
                        placeholder="Search users..."
                        prop:value=move || search_term.get()
                        on:input=move |ev| set_search_term.set(event_target_value(&ev))
                        class="search-input"
                    />
                    <span class="search-icon">"🔍"</span>
                </div>

                <div class="dropdown-filters">
                    <select
                        on:change=move |ev| set_realm_filter.set(event_target_value(&ev))
                        class="filter-select"
                    >
                        <option value="">"All Realms"</option>
                        {move || available_realms().into_iter().map(|realm| {
                            view! {
                                <option value={realm.clone()}>{realm}</option>
                            }
                        }).collect_view()}
                    </select>

                    <select
                        on:change=move |ev| set_role_filter.set(event_target_value(&ev))
                        class="filter-select"
                    >
                        <option value="">"All Roles"</option>
                        {move || available_roles().into_iter().map(|role| {
                            view! {
                                <option value={role.clone()}>{role}</option>
                            }
                        }).collect_view()}
                    </select>

                    <select
                        on:change=move |ev| set_status_filter.set(event_target_value(&ev))
                        class="filter-select"
                    >
                        <option value="">"All Status"</option>
                        <option value="enabled">"Enabled"</option>
                        <option value="disabled">"Disabled"</option>
                    </select>
                </div>
            </div>

            <div class="bulk-actions" style:display=move || {
                if selected_users.get().is_empty() { "none" } else { "flex" }
            }>
                <div class="bulk-selection-info">
                    <span>{move || format!("{} users selected", selected_users.get().len())}</span>
                    <button class="btn-link" on:click=clear_selection>"Clear"</button>
                </div>

                <div class="bulk-action-buttons">
                    <button
                        class="btn btn-secondary"
                        on:click=bulk_enable_users
                        disabled=move || bulk_action_loading.get()
                    >
                        "Enable Selected"
                    </button>
                    <button
                        class="btn btn-secondary"
                        on:click=bulk_disable_users
                        disabled=move || bulk_action_loading.get()
                    >
                        "Disable Selected"
                    </button>
                </div>
            </div>

            <div class="users-table-container">
                {move || if loading.get() {
                    view! {
                        <div class="loading-container">
                            <div class="spinner"></div>
                            <p>"Loading users..."</p>
                        </div>
                    }.into_view()
                } else {
                    let filtered = filtered_users();
                    if filtered.is_empty() {
                        view! {
                            <div class="empty-state">
                                <div class="empty-icon">"👤"</div>
                                <h3>"No users found"</h3>
                                <p>"No users match your current filters"</p>
                                <button
                                    class="btn btn-primary"
                                    on:click=move |_| {
                                        set_search_term.set(String::new());
                                        set_realm_filter.set(String::new());
                                        set_role_filter.set(String::new());
                                        set_status_filter.set(String::new());
                                    }
                                >"Clear Filters"</button>
                            </div>
                        }.into_view()
                    } else {
                        view! {
                            <div class="table-container">
                                <table class="users-table">
                                    <thead>
                                        <tr>
                                            <th class="checkbox-column">
                                                <input
                                                    type="checkbox"
                                                    on:change=select_all_users
                                                    prop:checked=move || {
                                                        let selected = selected_users.get();
                                                        let filtered = filtered_users();
                                                        !filtered.is_empty() && filtered.iter().all(|u| selected.contains(&u.id))
                                                    }
                                                />
                                            </th>
                                            <th>"Username"</th>
                                            <th>"Email"</th>
                                            <th>"Role"</th>
                                            <th>"Realm"</th>
                                            <th>"Status"</th>
                                            <th>"Last Login"</th>
                                            <th class="actions-column">"Actions"</th>
                                        </tr>
                                    </thead>
                                    <tbody>
                                        {move || filtered_users().into_iter().map(|user| {
                                            let user_id = user.id.clone();
                                            let user_id_delete = user.id.clone();
                                            let user_id_toggle = user.id.clone();
                                            let user_id_selected = user.id.clone();
                                            let user_id_selected_2 = user.id.clone();
                                            let is_selected_row = move || selected_users.get().contains(&user_id_selected);
                                            let is_selected_checkbox = move || selected_users.get().contains(&user_id_selected_2);
                                            let user_enabled = user.enabled;

                                            view! {
                                                <tr class:selected=is_selected_row>
                                                    <td>
                                                        <input
                                                            type="checkbox"
                                                            prop:checked=is_selected_checkbox
                                                            on:change=move |_| toggle_user_selection(user_id.clone())
                                                        />
                                                    </td>
                                                    <td>
                                                        <div class="user-info">
                                                            <strong>{&user.username}</strong>
                                                            {user.comment.as_ref().map(|comment| view! {
                                                                <small>{comment}</small>
                                                            })}
                                                        </div>
                                                    </td>
                                                    <td>{&user.email}</td>
                                                    <td>
                                                        <span class="role-badge">{&user.role}</span>
                                                        {if user.roles.len() > 1 {
                                                            view! {
                                                                <small class="additional-roles">
                                                                    {format!("+ {} more", user.roles.len() - 1)}
                                                                </small>
                                                            }.into_view()
                                                        } else {
                                                            view! {}.into_view()
                                                        }}
                                                    </td>
                                                    <td>
                                                        <span class="realm-badge">{&user.realm}</span>
                                                    </td>
                                                    <td>
                                                        <span class={format!("status-badge {}", if user.enabled { "enabled" } else { "disabled" })}>
                                                            {if user.enabled { "Enabled" } else { "Disabled" }}
                                                        </span>
                                                    </td>
                                                    <td>
                                                        {user.last_login.as_ref().map(|login| login.clone()).unwrap_or_else(|| "Never".to_string())}
                                                    </td>
                                                    <td class="actions-cell">
                                                        <div class="action-buttons">
                                                            <button
                                                                class="btn-icon"
                                                                title={if user_enabled { "Disable User" } else { "Enable User" }}
                                                                on:click=move |_| toggle_user_enabled(user_id_toggle.clone(), user_enabled)
                                                            >
                                                                {if user_enabled { "⏸" } else { "▶" }}
                                                            </button>
                                                            <button
                                                                class="btn-icon edit-btn"
                                                                title="Edit User"
                                                            >
                                                                "✏️"
                                                            </button>
                                                            <button
                                                                class="btn-icon delete-btn"
                                                                title="Delete User"
                                                                on:click=move |_| {
                                                                    if web_sys::window()
                                                                        .unwrap()
                                                                        .confirm_with_message(&format!("Delete user '{}'?", &user.username))
                                                                        .unwrap_or(false)
                                                                    {
                                                                        delete_user(user_id_delete.clone());
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

                                <div class="table-footer">
                                    <div class="table-info">
                                        {move || {
                                            let total = users.get().len();
                                            let filtered_count = filtered_users().len();
                                            if filtered_count != total {
                                                format!("Showing {} of {} users", filtered_count, total)
                                            } else {
                                                format!("{} users total", total)
                                            }
                                        }}
                                    </div>
                                </div>
                            </div>
                        }.into_view()
                    }
                }}
            </div>
        </div>
    }
}

/// Create user form component
#[component]
pub fn CreateUserForm<F, G>(
    roles: Vec<api::Role>,
    on_success: F,
    on_error: G,
) -> impl IntoView
where
    F: Fn() + 'static,
    G: Fn(String) + 'static,
{
    let (username, set_username) = create_signal(String::new());
    let (password, set_password) = create_signal(String::new());
    let (confirm_password, set_confirm_password) = create_signal(String::new());
    let (email, set_email) = create_signal(String::new());
    let (selected_role, set_selected_role) = create_signal(String::new());
    let (realm, set_realm) = create_signal("pam".to_string());
    let (comment, set_comment) = create_signal(String::new());
    let (creating, set_creating) = create_signal(false);

    // Wrap callbacks in Rc early so they can be cloned into closures
    let on_success_rc = std::rc::Rc::new(on_success);
    let on_error_rc = std::rc::Rc::new(on_error);

    let is_valid = move || {
        !username.get().trim().is_empty()
            && !password.get().is_empty()
            && password.get() == confirm_password.get()
            && !email.get().trim().is_empty()
            && !selected_role.get().is_empty()
    };

    let on_success_clone = on_success_rc.clone();
    let on_error_clone = on_error_rc.clone();
    let submit_form = move |ev: web_sys::SubmitEvent| {
        ev.prevent_default();

        if !is_valid() {
            return;
        }

        let request = api::CreateUserRequest {
            username: username.get().trim().to_string(),
            password: password.get(),
            email: email.get().trim().to_string(),
            role: selected_role.get(),
            realm: realm.get(),
            enabled: true,
            comment: if comment.get().trim().is_empty() {
                None
            } else {
                Some(comment.get().trim().to_string())
            },
        };

        set_creating.set(true);

        let on_success_inner = on_success_clone.clone();
        let on_error_inner = on_error_clone.clone();

        spawn_local(async move {
            match api::create_user(request).await {
                Ok(_) => {
                    on_success_inner();
                }
                Err(e) => {
                    on_error_inner(format!("Failed to create user: {}", e));
                }
            }
            set_creating.set(false);
        });
    };

    view! {
        <div class="create-user-form-container">
            <form class="create-user-form" on:submit=submit_form>
                <h2>"Create New User"</h2>

                <div class="form-row">
                    <div class="form-group">
                        <label for="username">"Username *"</label>
                        <input
                            type="text"
                            id="username"
                            prop:value=move || username.get()
                            on:input=move |ev| set_username.set(event_target_value(&ev))
                            placeholder="Enter username"
                            required
                        />
                    </div>

                    <div class="form-group">
                        <label for="email">"Email *"</label>
                        <input
                            type="email"
                            id="email"
                            prop:value=move || email.get()
                            on:input=move |ev| set_email.set(event_target_value(&ev))
                            placeholder="user@example.com"
                            required
                        />
                    </div>
                </div>

                <div class="form-row">
                    <div class="form-group">
                        <label for="password">"Password *"</label>
                        <input
                            type="password"
                            id="password"
                            prop:value=move || password.get()
                            on:input=move |ev| set_password.set(event_target_value(&ev))
                            placeholder="Enter password"
                            required
                        />
                    </div>

                    <div class="form-group">
                        <label for="confirm-password">"Confirm Password *"</label>
                        <input
                            type="password"
                            id="confirm-password"
                            prop:value=move || confirm_password.get()
                            on:input=move |ev| set_confirm_password.set(event_target_value(&ev))
                            placeholder="Confirm password"
                            required
                            class:error=move || !password.get().is_empty() && !confirm_password.get().is_empty() && password.get() != confirm_password.get()
                        />
                        {move || if !password.get().is_empty() && !confirm_password.get().is_empty() && password.get() != confirm_password.get() {
                            view! {
                                <small class="error-text">"Passwords do not match"</small>
                            }.into_view()
                        } else {
                            view! {}.into_view()
                        }}
                    </div>
                </div>

                <div class="form-row">
                    <div class="form-group">
                        <label for="role">"Role *"</label>
                        <select
                            id="role"
                            prop:value=move || selected_role.get()
                            on:change=move |ev| set_selected_role.set(event_target_value(&ev))
                            required
                        >
                            <option value="">"Select a role"</option>
                            {roles.into_iter().map(|role| {
                                view! {
                                    <option value={role.name.clone()}>
                                        {format!("{} - {}", role.name, role.description)}
                                    </option>
                                }
                            }).collect_view()}
                        </select>
                    </div>

                    <div class="form-group">
                        <label for="realm">"Authentication Realm"</label>
                        <select
                            id="realm"
                            prop:value=move || realm.get()
                            on:change=move |ev| set_realm.set(event_target_value(&ev))
                        >
                            <option value="pam">"PAM (System Users)"</option>
                            <option value="ldap">"LDAP"</option>
                            <option value="ad">"Active Directory"</option>
                            <option value="oidc">"OpenID Connect"</option>
                        </select>
                    </div>
                </div>

                <div class="form-group">
                    <label for="comment">"Comment"</label>
                    <textarea
                        id="comment"
                        prop:value=move || comment.get()
                        on:input=move |ev| set_comment.set(event_target_value(&ev))
                        placeholder="Optional user description or notes"
                        rows="3"
                    ></textarea>
                </div>

                <div class="form-actions">
                    <button
                        type="submit"
                        class="btn btn-primary"
                        disabled=move || creating.get() || !is_valid()
                    >
                        {move || if creating.get() { "Creating..." } else { "Create User" }}
                    </button>
                </div>
            </form>
        </div>
    }
}