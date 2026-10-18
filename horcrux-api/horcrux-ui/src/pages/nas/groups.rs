//! NAS Groups Management Page

use leptos::*;

#[derive(Clone, Debug, serde::Deserialize)]
pub struct NasGroup {
    pub id: String,
    pub name: String,
    pub gid: u32,
    pub members: Vec<String>,
    pub description: Option<String>,
}

#[component]
pub fn GroupsPage() -> impl IntoView {
    let (groups, set_groups) = create_signal(Vec::<NasGroup>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);

    let reload = move || {
        spawn_local(async move {
            match crate::api::authed::get("/api/nas/groups").send().await {
                Ok(resp) => {
                    if resp.ok() {
                        if let Ok(data) = resp.json::<Vec<NasGroup>>().await {
                            set_groups.set(data);
                            set_error.set(None);
                        }
                    } else {
                        set_error.set(Some("Failed to load groups".to_string()));
                    }
                }
                Err(e) => set_error.set(Some(format!("Network error: {}", e))),
            }
            set_loading.set(false);
        });
    };
    create_effect(move |_| reload());

    // "Create Group" used to be a button without a handler.
    let (show_create, set_show_create) = create_signal(false);
    let (new_name, set_new_name) = create_signal(String::new());
    let (notice, set_notice) = create_signal(None::<String>);
    let create_group = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let name = new_name.get().trim().to_string();
        spawn_local(async move {
            match crate::api::post_json::<serde_json::Value, _>(
                "/nas/groups",
                &serde_json::json!({ "name": name }),
            )
            .await
            {
                Ok(_) => {
                    set_notice.set(Some(format!("Group {name} created")));
                    set_show_create.set(false);
                    set_new_name.set(String::new());
                }
                Err(e) => set_notice.set(Some(format!("Failed: {}", e.message))),
            }
            reload();
        });
    };

    let delete_group = move |group_id: String| {
        if web_sys::window()
            .and_then(|w| w.confirm_with_message("Delete this group?").ok())
            .unwrap_or(false)
        {
            spawn_local(async move {
                match crate::api::delete_json(&format!("/nas/groups/{}", group_id)).await {
                    Ok(()) => set_notice.set(Some("Group deleted".to_string())),
                    Err(e) => set_notice.set(Some(format!("Failed: {}", e.message))),
                }
                reload();
            });
        }
    };

    view! {
        <div class="groups-page">
            <div class="page-header">
                <h1>"NAS Groups"</h1>
                <div class="header-actions">
                    <button class="btn btn-primary" id="nas-group-create"
                        on:click=move |_| set_show_create.update(|v| *v = !*v)>"Create Group"</button>
                </div>
            </div>
            {move || notice.get().map(|n| view! { <div class="alert alert-info" id="nas-group-notice">{n}</div> })}
            {move || show_create.get().then(|| view! {
                <form class="form-row" id="nas-group-form" on:submit=create_group>
                    <input id="nas-group-name" type="text" required placeholder="group name"
                        prop:value=new_name on:input=move |ev| set_new_name.set(event_target_value(&ev))/>
                    <button type="submit" class="btn btn-primary">"Create"</button>
                    <small class="form-help">"The next free GID is assigned."</small>
                </form>
            })}

            {move || {
                if loading.get() {
                    view! { <p class="loading">"Loading groups..."</p> }.into_view()
                } else if let Some(err) = error.get() {
                    view! { <p class="error">"Error: " {err}</p> }.into_view()
                } else {
                    let group_list = groups.get();
                    if group_list.is_empty() {
                        view! {
                            <div class="no-data">
                                <p>"No NAS groups configured."</p>
                            </div>
                        }.into_view()
                    } else {
                        view! {
                            <table class="data-table">
                                <thead>
                                    <tr>
                                        <th>"Name"</th>
                                        <th>"GID"</th>
                                        <th>"Members"</th>
                                        <th>"Description"</th>
                                        <th>"Actions"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {group_list.into_iter().map(|group| {
                                        let group_id = group.id.clone();
                                        let member_count = group.members.len();

                                        view! {
                                            <tr>
                                                <td><strong>{&group.name}</strong></td>
                                                <td>{group.gid}</td>
                                                <td title=group.members.join(", ")>{member_count} " members"
                                                    {(!group.members.is_empty()).then(|| format!(": {}", group.members.join(", ")))}</td>
                                                <td>{group.description.clone().unwrap_or_else(|| "-".to_string())}</td>
                                                <td class="actions">
                                                    <button
                                                        class="btn btn-sm btn-danger"
                                                        on:click=move |_| delete_group(group_id.clone())
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
