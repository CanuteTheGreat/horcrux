use crate::api;
use crate::features::IfFeature;
use leptos::*;

#[component]
pub fn ContainerList() -> impl IntoView {
    let (containers, set_containers) = create_signal(Vec::<api::Container>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);

    let (action_error, set_action_error) = create_signal(None::<String>);

    // Load (and, after every action, re-load) the container list so the
    // table always shows the real post-action state.
    let reload = move || {
        spawn_local(async move {
            match api::get_containers().await {
                Ok(container_list) => {
                    set_containers.set(container_list);
                    set_error.set(None);
                }
                Err(e) => {
                    set_error.set(Some(e.message));
                }
            }
            set_loading.set(false);
        });
    };

    create_effect(move |_| reload());

    let start_container = move |container_id: String| {
        spawn_local(async move {
            match api::start_container(&container_id).await {
                Ok(()) => set_action_error.set(None),
                Err(e) => {
                    set_action_error.set(Some(format!("Start {container_id}: {}", e.message)))
                }
            }
            reload();
        });
    };

    let stop_container = move |container_id: String| {
        spawn_local(async move {
            match api::stop_container(&container_id).await {
                Ok(()) => set_action_error.set(None),
                Err(e) => set_action_error.set(Some(format!("Stop {container_id}: {}", e.message))),
            }
            reload();
        });
    };

    let delete_container = move |container_id: String| {
        if web_sys::window()
            .and_then(|w| {
                w.confirm_with_message(&format!("Delete container {}?", container_id))
                    .ok()
            })
            .unwrap_or(false)
        {
            spawn_local(async move {
                match api::delete_container(&container_id).await {
                    Ok(()) => set_action_error.set(None),
                    Err(e) => {
                        set_action_error.set(Some(format!("Delete {container_id}: {}", e.message)))
                    }
                }
                reload();
            });
        }
    };

    view! {
        <div class="container-list">
            <div class="page-header">
                <h1>"Containers"</h1>
                <div class="header-actions">
                    <a href="/containers/create" class="btn btn-primary" id="create-container-btn">"Create Container"</a>
                    <IfFeature feature="lxc">
                        <a href="/containers/lxc/templates" class="btn" id="lxc-templates-btn">"LXC Templates"</a>
                    </IfFeature>
                </div>
            </div>

            {move || action_error.get().map(|e| view! { <div class="alert alert-error">{e}</div> })}

            {move || {
                if loading.get() {
                    view! { <p class="loading">"Loading containers..."</p> }.into_view()
                } else if let Some(err) = error.get() {
                    view! { <p class="error">"Error: " {err}</p> }.into_view()
                } else {
                    let container_list = containers.get();
                    if container_list.is_empty() {
                        view! {
                            <div class="no-data">
                                <p>"No containers found."</p>
                                <p>"Create a container to get started with lightweight virtualization!"</p>
                            </div>
                        }.into_view()
                    } else {
                        view! {
                            <table class="container-table">
                                <thead>
                                    <tr>
                                        <th>"ID"</th>
                                        <th>"Name"</th>
                                        <th>"Runtime"</th>
                                        <th>"Image"</th>
                                        <th>"Status"</th>
                                        <th>"Created"</th>
                                        <th>"Actions"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {container_list.into_iter().map(|container| {
                                        let container_id_start = container.id.clone();
                                        let container_id_stop = container.id.clone();
                                        let container_id_delete = container.id.clone();
                                        let status_class = match container.status.as_str() {
                                            "running" => "status-running",
                                            "stopped" => "status-stopped",
                                            "paused" => "status-paused",
                                            _ => "status-unknown",
                                        };

                                        view! {
                                            <tr data-container-id=container.id.clone()>
                                                <td>{&container.id}</td>
                                                <td><strong>{&container.name}</strong></td>
                                                <td>
                                                    <span class="runtime-badge">{&container.runtime}</span>
                                                </td>
                                                <td><code>{&container.image}</code></td>
                                                <td><span class={status_class}>{&container.status}</span></td>
                                                <td>
                                                    {container.created_at.as_deref().map(crate::utils::format_rfc3339).unwrap_or_else(|| "N/A".to_string())}
                                                </td>
                                                <td class="actions">
                                                    <button
                                                        class="btn btn-sm btn-success"
                                                        on:click=move |_| start_container(container_id_start.clone())
                                                    >"Start"</button>
                                                    <button
                                                        class="btn btn-sm btn-warning"
                                                        on:click=move |_| stop_container(container_id_stop.clone())
                                                    >"Stop"</button>
                                                    {(container.runtime == "lxc").then(|| view! {
                                                        <a class="btn btn-sm btn-primary lxc-manage" href=format!("/containers/{}/lxc", container.id)>"Manage"</a>
                                                    })}
                                                    <a class="btn btn-sm" href=format!("/containers/{}/logs?runtime={}", container.id, container.runtime)>"Logs"</a>
                                                    <a class="btn btn-sm" href=format!("/containers/{}/exec?runtime={}", container.id, container.runtime)>"Terminal"</a>
                                                    <button
                                                        class="btn btn-sm btn-danger"
                                                        on:click=move |_| delete_container(container_id_delete.clone())
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
