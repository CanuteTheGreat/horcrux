//! Docker Compose / Podman "stack" deployment page.
use crate::api;
use leptos::*;

#[component]
pub fn StacksPage() -> impl IntoView {
    let (runtime, set_runtime) = create_signal("docker".to_string());
    let (stacks, set_stacks) = create_signal(Vec::<api::StackSummary>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);
    let (name, set_name) = create_signal(String::new());
    let (action_error, set_action_error) = create_signal(None::<String>);
    let (compose, set_compose) = create_signal(
        "services:\n  web:\n    image: nginx:latest\n    ports:\n      - \"8080:80\"\n".to_string(),
    );

    let reload = move || {
        spawn_local(async move {
            set_loading.set(true);
            match api::get_stacks().await {
                Ok(list) => {
                    set_stacks.set(list);
                    set_error.set(None);
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            set_loading.set(false);
        });
    };

    create_effect(move |_| reload());

    let deploy = move |_| {
        let n = name.get();
        let yaml = compose.get();
        let rt = runtime.get();
        if n.trim().is_empty() || yaml.trim().is_empty() {
            return;
        }
        spawn_local(async move {
            match api::deploy_stack(&n, &yaml, &rt).await {
                Ok(_) => {
                    set_name.set(String::new());
                    set_action_error.set(None);
                    reload();
                }
                Err(e) => set_action_error.set(Some(format!("Deploy failed: {}", e.message))),
            }
        });
    };

    let start = move |id: String| {
        spawn_local(async move {
            if let Err(e) = api::start_stack(&id).await {
                set_action_error.set(Some(format!("Start failed: {}", e.message)));
            }
            reload();
        });
    };
    let stop = move |id: String| {
        spawn_local(async move {
            if let Err(e) = api::stop_stack(&id).await {
                set_action_error.set(Some(format!("Stop failed: {}", e.message)));
            }
            reload();
        });
    };
    let remove = move |id: String| {
        if web_sys::window()
            .and_then(|w| {
                w.confirm_with_message(&format!(
                    "Remove stack {}? This stops and deletes every container in it.",
                    id
                ))
                .ok()
            })
            .unwrap_or(false)
        {
            spawn_local(async move {
                if let Err(e) = api::remove_stack(&id).await {
                    set_action_error.set(Some(format!("Remove failed: {}", e.message)));
                }
                // Re-read the list so the removed stack disappears at once
                // (it used to linger until a manual page refresh).
                reload();
            });
        }
    };

    view! {
        <div class="container-list">
            <div class="page-header">
                <h1>"Stacks (Compose)"</h1>
                <p class="subtitle">
                    "Deploys common docker-compose.yml services as one named group of containers. "
                    "Services share a per-stack network and reach each other by service name; ports, environment, volumes and command are applied. Swarm `deploy:` keys and `build:` contexts are not applied."
                </p>
            </div>

            {move || action_error.get().map(|e| view! { <div class="alert alert-error">{e}</div> })}

            <div class="form-section">
                <select on:change=move |ev| set_runtime.set(event_target_value(&ev))>
                    <option value="docker">"Docker"</option>
                    <option value="podman">"Podman"</option>
                </select>
                <input
                    type="text"
                    placeholder="stack name"
                    prop:value=name
                    on:input=move |ev| set_name.set(event_target_value(&ev))
                />
                <textarea
                    rows="10"
                    style="width: 100%; font-family: monospace;"
                    prop:value=compose
                    on:input=move |ev| set_compose.set(event_target_value(&ev))
                ></textarea>
                <button class="btn btn-primary" on:click=deploy>"Deploy Stack"</button>
            </div>

            {move || {
                if loading.get() {
                    view! { <p class="loading">"Loading stacks..."</p> }.into_view()
                } else if let Some(err) = error.get() {
                    view! { <p class="error">"Error: " {err}</p> }.into_view()
                } else {
                    let list = stacks.get();
                    if list.is_empty() {
                        view! { <div class="no-data"><p>"No stacks deployed yet."</p></div> }.into_view()
                    } else {
                        view! {
                            <table class="container-table">
                                <thead>
                                    <tr><th>"Name"</th><th>"Runtime"</th><th>"Services"</th><th>"Actions"</th></tr>
                                </thead>
                                <tbody>
                                    {list.into_iter().map(|s| {
                                        let start_id = s.id.clone();
                                        let stop_id = s.id.clone();
                                        let remove_id = s.id.clone();
                                        let service_names: Vec<String> = s.services.iter().map(|sv| sv.service_name.clone()).collect();
                                        view! {
                                            <tr>
                                                <td><strong>{s.name}</strong></td>
                                                <td><span class="runtime-badge">{s.runtime}</span></td>
                                                <td>{service_names.join(", ")}</td>
                                                <td class="actions">
                                                    <button class="btn btn-sm btn-success" on:click=move |_| start(start_id.clone())>"Start"</button>
                                                    <button class="btn btn-sm btn-warning" on:click=move |_| stop(stop_id.clone())>"Stop"</button>
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
