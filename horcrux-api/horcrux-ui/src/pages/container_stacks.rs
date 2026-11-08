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

    // Some(stack name) while the form edits an existing stack.
    let (editing, set_editing) = create_signal(None::<String>);
    let (env_text, set_env_text) = create_signal(String::new());
    let (notice, set_notice) = create_signal(None::<String>);

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
                Err(e) if api::is_transport_error(&e.message) => {
                    // The request was cut off before any answer (browser network
                    // change, proxy timeout on a long image pull). The server
                    // finishes the deploy on its own: wait for the stack to show
                    // up instead of reporting a failure for a deploy that works.
                    let mut found = false;
                    for _ in 0..45 {
                        if let Ok(list) = api::get_stacks().await {
                            if list.iter().any(|s| s.name == n) {
                                found = true;
                                break;
                            }
                        }
                        gloo_timers::future::TimeoutFuture::new(2_000).await;
                    }
                    if found {
                        set_name.set(String::new());
                        set_action_error.set(None);
                        reload();
                    } else {
                        set_action_error.set(Some(format!(
                            "Deploy failed: {} (the stack did not appear within 90 s)",
                            e.message
                        )));
                    }
                }
                Err(e) => set_action_error.set(Some(format!("Deploy failed: {}", e.message))),
            }
        });
    };

    let edit = move |id: String| {
        spawn_local(async move {
            match api::get_stack(&id).await {
                Ok(s) => {
                    set_compose.set(s.compose_yaml.unwrap_or_default());
                    set_name.set(s.name.clone());
                    set_runtime.set(s.runtime.clone());
                    set_env_text.set(String::new());
                    set_editing.set(Some(s.name));
                    set_action_error.set(None);
                    set_notice.set(None);
                }
                Err(e) => set_action_error.set(Some(format!("Load failed: {}", e.message))),
            }
        });
    };
    let cancel_edit = move |_| {
        set_editing.set(None);
        set_name.set(String::new());
        set_env_text.set(String::new());
    };
    let update = move |_| {
        let Some(id) = editing.get() else { return };
        let yaml = compose.get();
        let env: std::collections::BTreeMap<String, String> = env_text
            .get()
            .lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
            .filter(|(k, _)| !k.is_empty())
            .collect();
        spawn_local(async move {
            match api::update_stack(&id, Some(&yaml), &env).await {
                Ok(r) => {
                    set_notice.set(Some(format!(
                        "Stack {id} updated: recreated [{}], unchanged [{}], added [{}], removed [{}]",
                        r.recreated.join(", "),
                        r.unchanged.join(", "),
                        r.added.join(", "),
                        r.removed.join(", ")
                    )));
                    set_action_error.set(None);
                    set_editing.set(None);
                    set_name.set(String::new());
                    set_env_text.set(String::new());
                    reload();
                }
                Err(e) => set_action_error.set(Some(format!("Update failed: {}", e.message))),
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
            {move || notice.get().map(|n| view! { <div class="alert alert-info" id="stack-notice">{n}</div> })}

            <div class="form-section">
                <select on:change=move |ev| set_runtime.set(event_target_value(&ev))>
                    <option value="docker">"Docker"</option>
                    <option value="podman">"Podman"</option>
                </select>
                <input
                    type="text"
                    id="stack-name"
                    disabled=move || editing.get().is_some()
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
                {move || if editing.get().is_some() {
                    view! {
                        <textarea
                            id="stack-env"
                            rows="3"
                            style="width: 100%; font-family: monospace;"
                            placeholder="extra environment for every service, one KEY=value per line (optional)"
                            prop:value=env_text
                            on:input=move |ev| set_env_text.set(event_target_value(&ev))
                        ></textarea>
                        <button id="stack-update" class="btn btn-primary" on:click=update>"Update Stack"</button>
                        <button class="btn" on:click=cancel_edit>"Cancel"</button>
                        <p class="form-help">"Services whose configuration changed are pulled and recreated one at a time; unchanged services keep running and volumes are kept."</p>
                    }.into_view()
                } else {
                    view! { <button id="stack-deploy" class="btn btn-primary" on:click=deploy>"Deploy Stack"</button> }.into_view()
                }}
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
                                        let edit_id = s.id.clone();
                                        let service_names: Vec<String> = s.services.iter().map(|sv| sv.service_name.clone()).collect();
                                        view! {
                                            <tr>
                                                <td><strong>{s.name}</strong></td>
                                                <td><span class="runtime-badge">{s.runtime}</span></td>
                                                <td>{service_names.join(", ")}</td>
                                                <td class="actions">
                                                    <button class="btn btn-sm btn-success" on:click=move |_| start(start_id.clone())>"Start"</button>
                                                    <button class="btn btn-sm btn-warning" on:click=move |_| stop(stop_id.clone())>"Stop"</button>
                                                    <button class="btn btn-sm stack-edit" on:click=move |_| edit(edit_id.clone())>"Edit / Update"</button>
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
