//! Docker/Podman volume management page.
use crate::api;
use leptos::*;

#[component]
pub fn VolumesPage() -> impl IntoView {
    let (runtime, set_runtime) = create_signal("docker".to_string());
    let (volumes, set_volumes) = create_signal(Vec::<api::VolumeInfo>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);
    let (new_name, set_new_name) = create_signal(String::new());

    let reload = move || {
        let rt = runtime.get();
        spawn_local(async move {
            set_loading.set(true);
            match api::get_volumes(&rt).await {
                Ok(list) => {
                    set_volumes.set(list);
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
            if let Err(e) = api::create_volume(&name, &rt).await {
                logging::log!("Error creating volume: {}", e.message);
            }
            set_new_name.set(String::new());
        });
    };

    let remove = move |name: String| {
        let rt = runtime.get();
        if web_sys::window()
            .and_then(|w| w.confirm_with_message(&format!("Remove volume {}?", name)).ok())
            .unwrap_or(false)
        {
            spawn_local(async move {
                if let Err(e) = api::remove_volume(&name, &rt).await {
                    logging::log!("Error removing volume: {}", e.message);
                }
            });
        }
    };

    view! {
        <div class="container-list">
            <div class="page-header">
                <h1>"Volumes"</h1>
                <div class="header-actions">
                    <select on:change=move |ev| set_runtime.set(event_target_value(&ev))>
                        <option value="docker">"Docker"</option>
                        <option value="podman">"Podman"</option>
                    </select>
                    <input
                        type="text"
                        placeholder="volume name"
                        prop:value=new_name
                        on:input=move |ev| set_new_name.set(event_target_value(&ev))
                    />
                    <button class="btn btn-primary" on:click=create>"Create Volume"</button>
                </div>
            </div>

            {move || {
                if loading.get() {
                    view! { <p class="loading">"Loading volumes..."</p> }.into_view()
                } else if let Some(err) = error.get() {
                    view! { <p class="error">"Error: " {err}</p> }.into_view()
                } else {
                    let list = volumes.get();
                    if list.is_empty() {
                        view! { <div class="no-data"><p>"No volumes found."</p></div> }.into_view()
                    } else {
                        view! {
                            <table class="container-table">
                                <thead>
                                    <tr><th>"Name"</th><th>"Driver"</th><th>"Mountpoint"</th><th>"Actions"</th></tr>
                                </thead>
                                <tbody>
                                    {list.into_iter().map(|v| {
                                        let remove_name = v.name.clone();
                                        view! {
                                            <tr>
                                                <td><strong>{v.name.clone()}</strong></td>
                                                <td>{v.driver}</td>
                                                <td><code>{v.mountpoint}</code></td>
                                                <td class="actions">
                                                    <button class="btn btn-sm btn-danger" on:click=move |_| remove(remove_name.clone())>"Remove"</button>
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
