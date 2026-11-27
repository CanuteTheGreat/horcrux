//! Docker/Podman volume management page.
use crate::api;
use leptos::*;

#[component]
pub fn VolumesPage() -> impl IntoView {
    let (runtime, set_runtime, rt_probes) = crate::components::oci_runtime::use_oci_runtime();
    let (action_error, set_action_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let (volumes, set_volumes) = create_signal(Vec::<api::ContainerVolumeInfo>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);
    let (unavailable, set_unavailable) = create_signal(None::<api::Unavailable>);
    let (new_name, set_new_name) = create_signal(String::new());

    let reload = move || {
        let rt = runtime.get();
        if rt.is_empty() {
            return;
        }
        spawn_local(async move {
            set_loading.set(true);
            match api::get_container_volumes(&rt).await {
                Ok(list) => {
                    set_volumes.set(list);
                    set_error.set(None);
                    set_unavailable.set(None);
                }
                Err(e) => {
                    set_unavailable.set(e.unavailable());
                    set_error.set(Some(e.message));
                }
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
            match api::create_volume(&name, &rt).await {
                Ok(_) => {
                    set_action_error.set(None);
                    set_notice.set(Some(format!("Volume {name} created")));
                    reload();
                }
                Err(e) => {
                    set_notice.set(None);
                    set_action_error
                        .set(Some(format!("Could not create the volume: {}", e.message)));
                }
            }
            set_new_name.set(String::new());
        });
    };

    let remove = move |name: String| {
        let rt = runtime.get();
        if web_sys::window()
            .and_then(|w| {
                w.confirm_with_message(&format!("Remove volume {}?", name))
                    .ok()
            })
            .unwrap_or(false)
        {
            spawn_local(async move {
                match api::remove_volume(&name, &rt).await {
                    Ok(_) => {
                        set_action_error.set(None);
                        set_notice.set(Some(format!("Volume {name} removed")));
                        reload();
                    }
                    Err(e) => {
                        set_notice.set(None);
                        set_action_error
                            .set(Some(format!("Could not remove the volume: {}", e.message)));
                    }
                }
            });
        }
    };

    view! {
        <div class="container-list">
            <div class="page-header">
                <h1>"Volumes"</h1>
                <div class="header-actions">
                    <crate::components::oci_runtime::OciRuntimeSelect runtime=runtime set_runtime=set_runtime probes=rt_probes/>
                    <input
                        type="text"
                        aria-label="Volume name" placeholder="my-volume"
                        prop:value=new_name
                        on:input=move |ev| set_new_name.set(event_target_value(&ev))
                    />
                    <button class="btn btn-primary" on:click=create>"Create Volume"</button>
                </div>
            </div>

            {move || action_error.get().map(|e| view! { <div class="alert alert-error" id="oci-action-error">{e}</div> })}
            {move || notice.get().map(|n| view! { <div class="alert alert-info" id="oci-action-notice">{n}</div> })}
            {move || {
                if loading.get() {
                    view! { <p class="loading">"Loading volumes..."</p> }.into_view()
                } else if let Some(u) = unavailable.get() {
                    view! { <crate::components::UnavailableState info=u on_retry=Callback::new(move |_| reload())/> }.into_view()
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
