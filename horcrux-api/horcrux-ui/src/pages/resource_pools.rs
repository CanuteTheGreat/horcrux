//! Resource pools (`/pools`): group VMs, containers and storages; grant a
//! role on `/pool/<id>` (Access > Permissions) to delegate the whole group.
use crate::api::{self, ResourcePool};
use leptos::*;

fn toggle(list: &mut Vec<String>, id: &str, on: bool) {
    list.retain(|m| m != id);
    if on {
        list.push(id.to_string());
    }
}

#[component]
pub fn ResourcePoolsPage() -> impl IntoView {
    let (pools, set_pools) = create_signal(Vec::<ResourcePool>::new());
    let (vms, set_vms) = create_signal(Vec::<(String, String)>::new());
    let (cts, set_cts) = create_signal(Vec::<(String, String)>::new());
    let (stores, set_stores) = create_signal(Vec::<(String, String)>::new());
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let (new_id, set_new_id) = create_signal(String::new());
    let (new_comment, set_new_comment) = create_signal(String::new());

    let reload = move || {
        spawn_local(async move {
            match api::list_resource_pools().await {
                Ok(p) => {
                    set_pools.set(p);
                    set_error.set(None);
                }
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };
    create_effect(move |_| {
        reload();
        spawn_local(async move {
            if let Ok(v) = api::get_vms().await {
                set_vms.set(v.into_iter().map(|v| (v.id, v.name)).collect());
            }
            if let Ok(c) = api::get_containers().await {
                set_cts.set(c.into_iter().map(|c| (c.id, c.name)).collect());
            }
            if let Ok(s) = api::get_storage_pools().await {
                set_stores.set(s.into_iter().map(|s| (s.id, s.name)).collect());
            }
        });
    });

    let create = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let p = ResourcePool {
            id: new_id.get().trim().to_string(),
            comment: new_comment.get(),
            ..Default::default()
        };
        spawn_local(async move {
            match api::create_resource_pool(&p).await {
                Ok(p) => {
                    set_notice.set(Some(format!("Pool {} created", p.id)));
                    set_new_id.set(String::new());
                    set_new_comment.set(String::new());
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            reload();
        });
    };
    let save = move |p: ResourcePool| {
        spawn_local(async move {
            match api::update_resource_pool(&p).await {
                Ok(_) => set_notice.set(Some(format!("Pool {} saved", p.id))),
                Err(e) => set_error.set(Some(e.message)),
            }
            reload();
        });
    };
    let remove = move |id: String| {
        if !web_sys::window()
            .and_then(|w| {
                w.confirm_with_message(&format!("Delete pool {id}? Grants on it are removed."))
                    .ok()
            })
            .unwrap_or(false)
        {
            return;
        }
        spawn_local(async move {
            match api::delete_resource_pool(&id).await {
                Ok(()) => set_notice.set(Some(format!("Pool {id} deleted"))),
                Err(e) => set_error.set(Some(e.message)),
            }
            reload();
        });
    };

    // one checkbox group: (label, all candidates, selected, apply)
    let members = move |title: &'static str,
                        all: ReadSignal<Vec<(String, String)>>,
                        pool: ResourcePool,
                        pick: fn(&mut ResourcePool) -> &mut Vec<String>| {
        let selected = {
            let mut p = pool.clone();
            pick(&mut p).clone()
        };
        view! {
            <fieldset class="pool-members">
                <legend>{title}</legend>
                {move || all.get().into_iter().map(|(id, name)| {
                    let checked = selected.contains(&id);
                    let pool = pool.clone();
                    let id2 = id.clone();
                    view! {
                        <label class="checkbox-label">
                            <input type="checkbox" class="pool-member" prop:checked=checked
                                on:change=move |ev| {
                                    let mut p = pool.clone();
                                    toggle(pick(&mut p), &id2, event_target_checked(&ev));
                                    save(p);
                                }/>
                            {format!("{name} ({id})")}
                        </label>
                    }
                }).collect_view()}
            </fieldset>
        }
    };

    view! {
        <div class="resource-pools">
            <div class="page-header"><h1>"Resource Pools"</h1></div>
            <p class="form-help">
                "Group VMs, containers and storages. Grant a role on the path "
                <code>"/pool/<id>"</code>
                " (Access > Permissions) to delegate everything in the pool; a guest or storage can be in one pool."
            </p>
            {move || notice.get().map(|n| view! { <div class="alert alert-info" id="pool-notice">{n}</div> })}
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="pool-error">{e}</div> })}
            <form id="pool-create-form" class="form-row" on:submit=create>
                <input id="pool-id" type="text" required placeholder="pool id" prop:value=new_id
                    on:input=move |ev| set_new_id.set(event_target_value(&ev))/>
                <input id="pool-comment" type="text" placeholder="comment" style="flex: 2" prop:value=new_comment
                    on:input=move |ev| set_new_comment.set(event_target_value(&ev))/>
                <button id="pool-create" type="submit" class="btn btn-primary">"Create pool"</button>
            </form>
            {move || {
                let list = pools.get();
                if list.is_empty() {
                    view! { <p class="no-data">"No pools yet."</p> }.into_view()
                } else {
                    list.into_iter().map(|p| {
                        let id = p.id.clone();
                        view! {
                            <div class="card pool-card" data-pool=p.id.clone()>
                                <h3>{p.id.clone()} " " <small>{p.comment.clone()}</small></h3>
                                {members("VMs", vms, p.clone(), |p| &mut p.vms)}
                                {members("Containers", cts, p.clone(), |p| &mut p.containers)}
                                {members("Storages", stores, p.clone(), |p| &mut p.storages)}
                                <button class="btn btn-sm btn-danger pool-delete" on:click=move |_| remove(id.clone())>"Delete pool"</button>
                            </div>
                        }
                    }).collect_view()
                }
            }}
        </div>
    }
}
