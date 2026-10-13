//! Docker/Podman image management page (Portainer-parity).
use crate::api;
use leptos::*;

#[component]
pub fn ImagesPage() -> impl IntoView {
    let (runtime, set_runtime) = create_signal("docker".to_string());
    let (images, set_images) = create_signal(Vec::<api::ImageInfo>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);
    let (pull_name, set_pull_name) = create_signal(String::new());
    let (pull_tag, set_pull_tag) = create_signal("latest".to_string());

    let reload = move || {
        let rt = runtime.get();
        spawn_local(async move {
            set_loading.set(true);
            match api::get_images(&rt).await {
                Ok(list) => {
                    set_images.set(list);
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

    let do_pull = move |_| {
        let rt = runtime.get();
        let name = pull_name.get();
        let tag = pull_tag.get();
        if name.trim().is_empty() {
            return;
        }
        spawn_local(async move {
            if let Err(e) = api::pull_image(&name, &tag, &rt).await {
                logging::log!("Error pulling image: {}", e.message);
            }
            set_pull_name.set(String::new());
        });
    };

    let remove = move |id: String| {
        let rt = runtime.get();
        if web_sys::window()
            .and_then(|w| w.confirm_with_message(&format!("Remove image {}?", id)).ok())
            .unwrap_or(false)
        {
            spawn_local(async move {
                if let Err(e) = api::remove_image(&id, &rt).await {
                    logging::log!("Error removing image: {}", e.message);
                }
            });
        }
    };

    view! {
        <div class="container-list">
            <div class="page-header">
                <h1>"Images"</h1>
                <div class="header-actions">
                    <select on:change=move |ev| set_runtime.set(event_target_value(&ev))>
                        <option value="docker">"Docker"</option>
                        <option value="podman">"Podman"</option>
                    </select>
                    <input
                        type="text"
                        placeholder="image name (e.g. nginx)"
                        prop:value=pull_name
                        on:input=move |ev| set_pull_name.set(event_target_value(&ev))
                    />
                    <input
                        type="text"
                        placeholder="tag"
                        prop:value=pull_tag
                        on:input=move |ev| set_pull_tag.set(event_target_value(&ev))
                    />
                    <button class="btn btn-primary" on:click=do_pull>"Pull"</button>
                </div>
            </div>

            {move || {
                if loading.get() {
                    view! { <p class="loading">"Loading images..."</p> }.into_view()
                } else if let Some(err) = error.get() {
                    view! { <p class="error">"Error: " {err}</p> }.into_view()
                } else {
                    let list = images.get();
                    if list.is_empty() {
                        view! { <div class="no-data"><p>"No images found."</p></div> }.into_view()
                    } else {
                        view! {
                            <table class="container-table">
                                <thead>
                                    <tr><th>"ID"</th><th>"Repo Tags"</th><th>"Size"</th><th>"Actions"</th></tr>
                                </thead>
                                <tbody>
                                    {list.into_iter().map(|img| {
                                        let remove_id = img.id.clone();
                                        view! {
                                            <tr>
                                                <td><code>{img.id.chars().take(16).collect::<String>()}</code></td>
                                                <td>{img.repo_tags.join(", ")}</td>
                                                <td>{format!("{:.1} MB", img.size as f64 / 1_000_000.0)}</td>
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
