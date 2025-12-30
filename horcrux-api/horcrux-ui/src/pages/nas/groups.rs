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

    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);
            match reqwasm::http::Request::get("/api/nas/groups")
                .send()
                .await
            {
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
    });

    let delete_group = move |group_id: String| {
        if web_sys::window()
            .and_then(|w| w.confirm_with_message("Delete this group?").ok())
            .unwrap_or(false)
        {
            spawn_local(async move {
                let _ = reqwasm::http::Request::delete(&format!("/api/nas/groups/{}", group_id))
                    .send()
                    .await;
            });
        }
    };

    view! {
        <div class="groups-page">
            <div class="page-header">
                <h1>"NAS Groups"</h1>
                <div class="header-actions">
                    <button class="btn btn-primary">"Create Group"</button>
                </div>
            </div>

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
                                                <td>{member_count} " members"</td>
                                                <td>{group.description.clone().unwrap_or_else(|| "-".to_string())}</td>
                                                <td class="actions">
                                                    <a href={format!("/nas/groups/{}", &group.id)} class="btn btn-sm">"Manage"</a>
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
