//! NAS Users Management Page

use leptos::*;

#[derive(Clone, Debug, serde::Deserialize)]
pub struct NasUser {
    pub id: String,
    pub username: String,
    pub uid: u32,
    pub primary_group: String,
    pub home_directory: Option<String>,
    pub enabled: bool,
    pub smb_enabled: bool,
    pub ssh_enabled: bool,
}

#[component]
pub fn UsersPage() -> impl IntoView {
    let (users, set_users) = create_signal(Vec::<NasUser>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);
    let (show_create, set_show_create) = create_signal(false);

    // Load users
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);
            match crate::api::authed::get("/api/nas/users").send().await {
                Ok(resp) => {
                    if resp.ok() {
                        if let Ok(data) = resp.json::<Vec<NasUser>>().await {
                            set_users.set(data);
                            set_error.set(None);
                        }
                    } else {
                        set_error.set(Some("Failed to load users".to_string()));
                    }
                }
                Err(e) => {
                    set_error.set(Some(format!("Network error: {}", e)));
                }
            }
            set_loading.set(false);
        });
    });

    // Password: inline form (the button used to have no handler).
    let (pw_user, set_pw_user) = create_signal(None::<(String, String)>);
    let (pw_value, set_pw_value) = create_signal(String::new());
    let (notice, set_notice) = create_signal(None::<String>);
    let save_password = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let Some((id, name)) = pw_user.get() else {
            return;
        };
        let pw = pw_value.get();
        spawn_local(async move {
            match crate::api::post_json_no_response(
                &format!("/nas/users/{}/password", id),
                &serde_json::json!({ "password": pw }),
            )
            .await
            {
                Ok(()) => {
                    set_notice.set(Some(format!("Password of {name} changed")));
                    set_pw_user.set(None);
                }
                Err(e) => set_notice.set(Some(format!("Failed: {}", e.message))),
            }
            set_pw_value.set(String::new());
        });
    };
    let toggle_user = move |id: String, enable: bool| {
        spawn_local(async move {
            let action = if enable { "enable" } else { "disable" };
            match crate::api::post_empty(&format!("/nas/users/{}/{}", id, action)).await {
                Ok(()) => set_notice.set(Some(format!("User {action}d"))),
                Err(e) => set_notice.set(Some(format!("Failed: {}", e.message))),
            }
            if let Ok(resp) = crate::api::authed::get("/api/nas/users").send().await {
                if let Ok(data) = resp.json::<Vec<NasUser>>().await {
                    set_users.set(data);
                }
            }
        });
    };

    let delete_user = move |user_id: String| {
        if web_sys::window()
            .and_then(|w| w.confirm_with_message("Delete this user?").ok())
            .unwrap_or(false)
        {
            spawn_local(async move {
                let _ = crate::api::authed::delete(&format!("/api/nas/users/{}", user_id))
                    .send()
                    .await;
            });
        }
    };

    view! {
        <div class="users-page">
            <div class="page-header">
                <h1>"NAS Users"</h1>
                <div class="header-actions">
                    <button class="btn btn-primary" on:click=move |_| set_show_create.set(true)>
                        "Create User"
                    </button>
                </div>
            </div>

            {move || notice.get().map(|n| view! { <div class="alert alert-info" id="nas-user-notice">{n}</div> })}
            {move || pw_user.get().map(|(_, name)| view! {
                <form class="form-row" id="nas-password-form" on:submit=save_password>
                    <label>"New password for " <strong>{name}</strong></label>
                    <input id="nas-password" type="password" required minlength="8" autocomplete="new-password"
                        prop:value=pw_value on:input=move |ev| set_pw_value.set(event_target_value(&ev))/>
                    <button type="submit" class="btn btn-primary">"Set password"</button>
                    <button type="button" class="btn btn-secondary" on:click=move |_| set_pw_user.set(None)>"Cancel"</button>
                </form>
            })}
            {move || {
                if loading.get() {
                    view! { <p class="loading">"Loading users..."</p> }.into_view()
                } else if let Some(err) = error.get() {
                    view! { <p class="error">"Error: " {err}</p> }.into_view()
                } else {
                    let user_list = users.get();
                    if user_list.is_empty() {
                        view! {
                            <div class="no-data">
                                <p>"No NAS users configured."</p>
                                <p>"Create a user to enable access to NAS shares."</p>
                            </div>
                        }.into_view()
                    } else {
                        view! {
                            <table class="data-table">
                                <thead>
                                    <tr>
                                        <th>"Username"</th>
                                        <th>"UID"</th>
                                        <th>"Primary Group"</th>
                                        <th>"Home Directory"</th>
                                        <th>"SMB"</th>
                                        <th>"SSH"</th>
                                        <th>"Status"</th>
                                        <th>"Actions"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {user_list.into_iter().map(|user| {
                                        let user_id = user.id.clone();
                                        let status_class = if user.enabled { "status-active" } else { "status-inactive" };

                                        view! {
                                            <tr>
                                                <td><strong>{&user.username}</strong></td>
                                                <td>{user.uid}</td>
                                                <td>{&user.primary_group}</td>
                                                <td><code>{user.home_directory.clone().unwrap_or_else(|| "-".to_string())}</code></td>
                                                <td>{if user.smb_enabled { "Yes" } else { "No" }}</td>
                                                <td>{if user.ssh_enabled { "Yes" } else { "No" }}</td>
                                                <td>
                                                    <span class={status_class}>
                                                        {if user.enabled { "Enabled" } else { "Disabled" }}
                                                    </span>
                                                </td>
                                                <td class="actions">
                                                    <button class="btn btn-sm nas-user-password"
                                                        on:click={
                                                            let (id, name) = (user.id.clone(), user.username.clone());
                                                            move |_| set_pw_user.set(Some((id.clone(), name.clone())))
                                                        }>"Password"</button>
                                                    <button class="btn btn-sm nas-user-toggle"
                                                        on:click={
                                                            let (id, enabled) = (user.id.clone(), user.enabled);
                                                            move |_| toggle_user(id.clone(), !enabled)
                                                        }>{if user.enabled { "Disable" } else { "Enable" }}</button>
                                                    <button
                                                        class="btn btn-sm btn-danger"
                                                        on:click=move |_| delete_user(user_id.clone())
                                                    >"Delete"</button>
                                                </td>
                                            </tr>
                                        }
                                    }).collect_view()}
                                </tbody>
                            </table>
                        }.into_view()
                    }
                }
            }}

            // Create User Modal
            {move || {
                if show_create.get() {
                    view! {
                        <div class="modal-overlay" on:click=move |_| set_show_create.set(false)>
                            <div class="modal" on:click=|e| e.stop_propagation()>
                                <h2>"Create NAS User"</h2>
                                <form>
                                    <div class="form-group">
                                        <label>"Username"</label>
                                        <input type="text" name="username" required />
                                    </div>
                                    <div class="form-group">
                                        <label>"Password"</label>
                                        <input type="password" name="password" required />
                                    </div>
                                    <div class="form-group">
                                        <label>"Primary Group"</label>
                                        <input type="text" name="primary_group" value="users" />
                                    </div>
                                    <div class="form-group">
                                        <label>"Home Directory"</label>
                                        <input type="text" name="home_directory" placeholder="/home/username" />
                                    </div>
                                    <div class="form-check">
                                        <input type="checkbox" name="smb_enabled" checked />
                                        <label>"Enable SMB access"</label>
                                    </div>
                                    <div class="form-check">
                                        <input type="checkbox" name="ssh_enabled" />
                                        <label>"Enable SSH/SFTP access"</label>
                                    </div>
                                    <div class="modal-actions">
                                        <button type="button" class="btn" on:click=move |_| set_show_create.set(false)>"Cancel"</button>
                                        <button type="submit" class="btn btn-primary">"Create User"</button>
                                    </div>
                                </form>
                            </div>
                        </div>
                    }.into_view()
                } else {
                    view! {}.into_view()
                }
            }}
        </div>
    }
}
