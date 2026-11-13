//! NAS Shares Management Page

use leptos::*;

#[derive(Clone, Debug, serde::Deserialize)]
pub struct Share {
    pub id: String,
    pub name: String,
    pub path: String,
    // GET /api/nas/shares has no protocols field: without the default every share failed to
    // deserialize and the page always said "No shares configured".
    #[serde(default)]
    pub protocols: Vec<String>,
    pub enabled: bool,
    pub description: Option<String>,
}

#[component]
pub fn SharesPage() -> impl IntoView {
    let (shares, set_shares) = create_signal(Vec::<Share>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);
    // Bumped after every create/edit/toggle/delete so the list reloads.
    let (reload, set_reload) = create_signal(0u32);
    // The create/edit form: open on ?create=1 (the NAS dashboard's "Create Share" links here).
    let query = leptos_router::use_query_map();
    let (show_form, set_show_form) = create_signal(
        query
            .get_untracked()
            .get("create")
            .is_some_and(|v| v == "1"),
    );
    let (edit_id, set_edit_id) = create_signal(None::<String>);
    let (f_name, set_f_name) = create_signal(String::new());
    let (f_path, set_f_path) = create_signal(String::new());
    let (f_desc, set_f_desc) = create_signal(String::new());
    let (notice, set_notice) = create_signal(None::<(bool, String)>);

    // Load shares
    create_effect(move |_| {
        reload.track();
        spawn_local(async move {
            set_loading.set(true);
            match crate::api::authed::get("/api/nas/shares").send().await {
                Ok(resp) => {
                    if resp.ok() {
                        if let Ok(data) = resp.json::<Vec<Share>>().await {
                            set_shares.set(data);
                            set_error.set(None);
                        }
                    } else {
                        set_error.set(Some("Failed to load shares".to_string()));
                    }
                }
                Err(e) => {
                    set_error.set(Some(format!("Network error: {}", e)));
                }
            }
            set_loading.set(false);
        });
    });

    let delete_share = move |share_id: String| {
        if web_sys::window()
            .and_then(|w| {
                w.confirm_with_message(&format!("Delete share {}?", share_id))
                    .ok()
            })
            .unwrap_or(false)
        {
            spawn_local(async move {
                let _ = crate::api::authed::delete(&format!("/api/nas/shares/{}", share_id))
                    .send()
                    .await;
                set_reload.update(|n| *n += 1);
            });
        }
    };

    let toggle_share = move |share_id: String, enable: bool| {
        spawn_local(async move {
            let action = if enable { "enable" } else { "disable" };
            let _ = crate::api::authed::post(&format!("/api/nas/shares/{}/{}", share_id, action))
                .send()
                .await;
            set_reload.update(|n| *n += 1);
        });
    };

    let open_create = move |_| {
        set_edit_id.set(None);
        set_f_name.set(String::new());
        set_f_path.set(String::new());
        set_f_desc.set(String::new());
        set_show_form.set(true);
    };
    let save_share = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let editing = edit_id.get();
        let name = f_name.get().trim().to_string();
        let body = serde_json::json!({
            "name": name,
            "path": f_path.get().trim(),
            "description": f_desc.get().trim(),
        });
        spawn_local(async move {
            let res = match &editing {
                Some(id) => crate::api::put_json::<serde_json::Value, _>(
                    &format!("/nas/shares/{id}"),
                    &body,
                )
                .await
                .map(|_| ()),
                None => crate::api::post_json_no_response("/nas/shares", &body).await,
            };
            match res {
                Ok(()) => {
                    let what = if editing.is_some() {
                        "saved"
                    } else {
                        "created"
                    };
                    set_notice.set(Some((true, format!("Share {name} {what}"))));
                    set_show_form.set(false);
                    set_reload.update(|n| *n += 1);
                }
                Err(e) => set_notice.set(Some((false, format!("Failed: {}", e.message)))),
            }
        });
    };

    view! {
        <div class="shares-page">
            <div class="page-header">
                <h1>"NAS Shares"</h1>
                <div class="header-actions">
                    <button class="btn btn-primary" id="nas-share-create" on:click=open_create>"Create Share"</button>
                </div>
            </div>
            {move || notice.get().map(|(ok, m)| view! {
                <div class=if ok { "alert-success" } else { "alert-error" } id="nas-share-notice">{m}</div>
            })}
            <Show when=move || show_form.get()>
                <form class="card nas-share-form" id="nas-share-form" on:submit=save_share>
                    <h2>{move || if edit_id.get().is_some() { "Edit Share" } else { "Create Share" }}</h2>
                    <div class="form-group">
                        <label for="nas-share-name">"Name"</label>
                        <input id="nas-share-name" required prop:value=f_name
                            on:input=move |e| set_f_name.set(event_target_value(&e))/>
                    </div>
                    <div class="form-group">
                        <label for="nas-share-path">"Path"</label>
                        // the path of an existing share is fixed (the API updates name and description only)
                        <input id="nas-share-path" required placeholder="/srv/nas/share" prop:value=f_path
                            prop:disabled=move || edit_id.get().is_some()
                            on:input=move |e| set_f_path.set(event_target_value(&e))/>
                    </div>
                    <div class="form-group">
                        <label for="nas-share-desc">"Description"</label>
                        <input id="nas-share-desc" prop:value=f_desc
                            on:input=move |e| set_f_desc.set(event_target_value(&e))/>
                    </div>
                    <div class="form-actions">
                        <button type="submit" class="btn btn-primary" id="nas-share-save">"Save"</button>
                        <button type="button" class="btn" on:click=move |_| set_show_form.set(false)>"Cancel"</button>
                    </div>
                </form>
            </Show>

            {move || {
                if loading.get() {
                    view! { <p class="loading">"Loading shares..."</p> }.into_view()
                } else if let Some(err) = error.get() {
                    view! { <p class="error">"Error: " {err}</p> }.into_view()
                } else {
                    let share_list = shares.get();
                    if share_list.is_empty() {
                        view! {
                            <div class="no-data">
                                <p>"No shares configured."</p>
                                <p>"Create a share to enable file sharing over SMB, NFS, AFP, or other protocols."</p>
                                <button class="btn btn-primary" on:click=open_create>"Create Share"</button>
                            </div>
                        }.into_view()
                    } else {
                        view! {
                            <table class="data-table">
                                <thead>
                                    <tr>
                                        <th>"Name"</th>
                                        <th>"Path"</th>
                                        <th>"Protocols"</th>
                                        <th>"Status"</th>
                                        <th>"Description"</th>
                                        <th>"Actions"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {share_list.into_iter().map(|share| {
                                        let share_id_del = share.id.clone();
                                        let share_id_toggle = share.id.clone();
                                        let edit = (share.id.clone(), share.name.clone(), share.path.clone(), share.description.clone().unwrap_or_default());
                                        let is_enabled = share.enabled;
                                        let status_class = if share.enabled { "status-active" } else { "status-inactive" };
                                        let protocols = share.protocols.join(", ");

                                        view! {
                                            <tr class="nas-share-row" data-share=share.name.clone()>
                                                <td><strong>{&share.name}</strong></td>
                                                <td><code>{&share.path}</code></td>
                                                <td>{protocols}</td>
                                                <td>
                                                    <span class={status_class}>
                                                        {if share.enabled { "Enabled" } else { "Disabled" }}
                                                    </span>
                                                </td>
                                                <td>{share.description.unwrap_or_default()}</td>
                                                <td class="actions">
                                                    <button class="btn btn-sm nas-share-edit" on:click=move |_| {
                                                        let (id, name, path, desc) = edit.clone();
                                                        set_edit_id.set(Some(id));
                                                        set_f_name.set(name);
                                                        set_f_path.set(path);
                                                        set_f_desc.set(desc);
                                                        set_show_form.set(true);
                                                    }>"Edit"</button>
                                                    <button
                                                        class={if is_enabled { "btn btn-sm btn-warning" } else { "btn btn-sm btn-success" }}
                                                        on:click=move |_| toggle_share(share_id_toggle.clone(), !is_enabled)
                                                    >
                                                        {if is_enabled { "Disable" } else { "Enable" }}
                                                    </button>
                                                    <button
                                                        class="btn btn-sm btn-danger nas-share-delete"
                                                        on:click=move |_| delete_share(share_id_del.clone())
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
        </div>
    }
}
