//! Docker/Podman container-runtime network management page.
//!
//! This manages `docker network` / `podman network` bridge networks that
//! containers attach to - unrelated to Horcrux's own SDN/CNI networking
//! page (`pages::network::NetworkManagement`).
use crate::api;
use leptos::*;

#[component]
pub fn ContainerNetworksPage() -> impl IntoView {
    let (runtime, set_runtime) = create_signal("docker".to_string());
    let (networks, set_networks) = create_signal(Vec::<api::ContainerNetworkInfo>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);
    let (new_name, set_new_name) = create_signal(String::new());

    let reload = move || {
        let rt = runtime.get();
        spawn_local(async move {
            set_loading.set(true);
            match api::get_container_networks(&rt).await {
                Ok(list) => {
                    set_networks.set(list);
                    set_error.set(None);
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            set_loading.set(false);
        });
    };

    create_effect(move |_| {
        let _ = runtime.get();
        reload();
    });

    let create = move |_| {
        let rt = runtime.get();
        let name = new_name.get();
        if name.trim().is_empty() {
            return;
        }
        spawn_local(async move {
            if let Err(e) = api::create_container_network(&name, &rt).await {
                logging::log!("Error creating network: {}", e.message);
            }
            set_new_name.set(String::new());
        });
    };

    let remove = move |id: String| {
        let rt = runtime.get();
        if web_sys::window()
            .and_then(|w| w.confirm_with_message(&format!("Remove network {}?", id)).ok())
            .unwrap_or(false)
        {
            spawn_local(async move {
                if let Err(e) = api::remove_container_network(&id, &rt).await {
                    logging::log!("Error removing network: {}", e.message);
                }
            });
        }
    };

    view! {
        <div class="container-list">
            <div class="page-header">
                <h1>"Container Networks"</h1>
                <p class="subtitle">"Docker/Podman bridge networks (not Horcrux SDN - see Network page for that)."</p>
                <div class="header-actions">
                    <select on:change=move |ev| set_runtime.set(event_target_value(&ev))>
                        <option value="docker">"Docker"</option>
                        <option value="podman">"Podman"</option>
                    </select>
                    <input
                        type="text"
                        placeholder="network name"
                        prop:value=new_name
                        on:input=move |ev| set_new_name.set(event_target_value(&ev))
                    />
                    <button class="btn btn-primary" on:click=create>"Create Network"</button>
                </div>
            </div>

            {move || {
                if loading.get() {
                    view! { <p class="loading">"Loading networks..."</p> }.into_view()
                } else if let Some(err) = error.get() {
                    view! { <p class="error">"Error: " {err}</p> }.into_view()
                } else {
                    let list = networks.get();
                    if list.is_empty() {
                        view! { <div class="no-data"><p>"No container networks found."</p></div> }.into_view()
                    } else {
                        view! {
                            <table class="container-table">
                                <thead>
                                    <tr><th>"ID"</th><th>"Name"</th><th>"Driver"</th><th>"Scope"</th><th>"Actions"</th></tr>
                                </thead>
                                <tbody>
                                    {list.into_iter().map(|n| {
                                        let remove_id = n.id.clone();
                                        view! {
                                            <tr>
                                                <td><code>{n.id.chars().take(12).collect::<String>()}</code></td>
                                                <td><strong>{n.name}</strong></td>
                                                <td>{n.driver}</td>
                                                <td>{n.scope}</td>
                                                <td class="actions">
                                                    <button class="btn btn-sm btn-danger" on:click=move |_| remove(remove_id.clone())>"Remove"</button>
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
