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
            match reqwasm::http::Request::get("/api/nas/users")
                .send()
                .await
            {
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

    let delete_user = move |user_id: String| {
        if web_sys::window()
            .and_then(|w| w.confirm_with_message("Delete this user?").ok())
            .unwrap_or(false)
        {
            spawn_local(async move {
                let _ = reqwasm::http::Request::delete(&format!("/api/nas/users/{}", user_id))
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
                                                    <a href={format!("/nas/users/{}", &user.id)} class="btn btn-sm">"Edit"</a>
                                                    <button class="btn btn-sm">"Password"</button>
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
