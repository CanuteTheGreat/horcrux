//! NAS Shares Management Page

use leptos::*;

#[derive(Clone, Debug, serde::Deserialize)]
pub struct Share {
    pub id: String,
    pub name: String,
    pub path: String,
    pub protocols: Vec<String>,
    pub enabled: bool,
    pub description: Option<String>,
}

#[component]
pub fn SharesPage() -> impl IntoView {
    let (shares, set_shares) = create_signal(Vec::<Share>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);

    // Load shares
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);
            match reqwasm::http::Request::get("/api/nas/shares")
                .send()
                .await
            {
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
            .and_then(|w| w.confirm_with_message(&format!("Delete share {}?", share_id)).ok())
            .unwrap_or(false)
        {
            spawn_local(async move {
                let _ = reqwasm::http::Request::delete(&format!("/api/nas/shares/{}", share_id))
                    .send()
                    .await;
            });
        }
    };

    let toggle_share = move |share_id: String, enable: bool| {
        spawn_local(async move {
            let action = if enable { "enable" } else { "disable" };
            let _ = reqwasm::http::Request::post(&format!("/api/nas/shares/{}/{}", share_id, action))
                .send()
                .await;
        });
    };

    view! {
        <div class="shares-page">
            <div class="page-header">
                <h1>"NAS Shares"</h1>
                <div class="header-actions">
                    <a href="/nas/shares/create" class="btn btn-primary">"Create Share"</a>
                </div>
            </div>

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
                                        let is_enabled = share.enabled;
                                        let status_class = if share.enabled { "status-active" } else { "status-inactive" };
                                        let protocols = share.protocols.join(", ");

                                        view! {
                                            <tr>
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
                                                    <a href={format!("/nas/shares/{}", &share.id)} class="btn btn-sm">"Edit"</a>
                                                    <button
                                                        class={if is_enabled { "btn btn-sm btn-warning" } else { "btn btn-sm btn-success" }}
                                                        on:click=move |_| toggle_share(share_id_toggle.clone(), !is_enabled)
                                                    >
                                                        {if is_enabled { "Disable" } else { "Enable" }}
                                                    </button>
                                                    <button
                                                        class="btn btn-sm btn-danger"
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
