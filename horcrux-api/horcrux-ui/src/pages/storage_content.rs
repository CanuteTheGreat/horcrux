//! Infrastructure > Storage Content (`/storage/content`): the files a storage
//! pool holds per content type (ISO images, container templates, snippets,
//! backups, disk images), with upload from the browser and delete.
use crate::api::{self, ContentItem, ContentTypeInfo};
use leptos::*;
use wasm_bindgen::JsCast;

#[component]
pub fn StorageContentPage() -> impl IntoView {
    let (pools, set_pools) = create_signal(Vec::<(String, String)>::new());
    let (types, set_types) = create_signal(Vec::<ContentTypeInfo>::new());
    let (pool, set_pool) = create_signal(String::new());
    let (ctype, set_ctype) = create_signal("iso".to_string());
    let (items, set_items) = create_signal(Vec::<ContentItem>::new());
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let (busy, set_busy) = create_signal(false);

    let reload = move || {
        let (p, c) = (pool.get_untracked(), ctype.get_untracked());
        if p.is_empty() {
            return;
        }
        spawn_local(async move {
            match api::list_pool_content(&p, &c).await {
                Ok(l) => {
                    set_items.set(l);
                    set_error.set(None);
                }
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };

    create_effect(move |_| {
        spawn_local(async move {
            if let Ok(t) = api::get_content_types().await {
                set_types.set(t);
            }
            match api::get_storage_pools().await {
                Ok(p) => {
                    let v: Vec<(String, String)> = p.into_iter().map(|p| (p.id, p.name)).collect();
                    if let Some((id, _)) = v.first() {
                        set_pool.set(id.clone());
                    }
                    set_pools.set(v);
                }
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    });
    create_effect(move |_| {
        pool.track();
        ctype.track();
        reload();
    });

    let uploadable = move || {
        types
            .get()
            .iter()
            .find(|t| t.id == ctype.get())
            .map(|t| t.uploadable)
            .unwrap_or(false)
    };

    let upload = move |_| {
        let input = document()
            .get_element_by_id("content-file")
            .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok());
        let Some(file) = input.and_then(|i| i.files()).and_then(|f| f.get(0)) else {
            set_error.set(Some("choose a file first".into()));
            return;
        };
        let (p, c) = (pool.get_untracked(), ctype.get_untracked());
        set_busy.set(true);
        spawn_local(async move {
            match api::upload_pool_content(&p, &c, file).await {
                Ok(i) => {
                    set_notice.set(Some(format!(
                        "Uploaded {} ({})",
                        i.name,
                        api::human_bytes(i.size)
                    )));
                    set_error.set(None);
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            set_busy.set(false);
            reload();
        });
    };

    let remove = move |name: String| {
        if !web_sys::window()
            .and_then(|w| w.confirm_with_message(&format!("Delete {name}?")).ok())
            .unwrap_or(false)
        {
            return;
        }
        let (p, c) = (pool.get_untracked(), ctype.get_untracked());
        spawn_local(async move {
            match api::delete_pool_content(&p, &c, &name).await {
                Ok(()) => set_notice.set(Some(format!("{name} deleted"))),
                Err(e) => set_error.set(Some(e.message)),
            }
            reload();
        });
    };

    view! {
        <div class="storage-content">
            <div class="page-header"><h1>"Storage Content"</h1></div>
            <p class="form-help">
                "Files a storage pool holds, by content type. Upload goes straight to the pool directory."
            </p>
            {move || notice.get().map(|n| view! { <div class="alert alert-info" id="content-notice">{n}</div> })}
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="content-error">{e}</div> })}
            <div class="form-row">
                <select id="content-pool" on:change=move |ev| set_pool.set(event_target_value(&ev))>
                    {move || pools.get().into_iter().map(|(id, name)| view! { <option value=id.clone()>{name}</option> }).collect_view()}
                </select>
                <select id="content-type" on:change=move |ev| set_ctype.set(event_target_value(&ev))>
                    {move || types.get().into_iter().map(|t| {
                        let sel = t.id == "iso";
                        view! { <option value=t.id.clone() selected=sel>{t.label.clone()}</option> }
                    }).collect_view()}
                </select>
            </div>
            <div class="form-row" style:display=move || if uploadable() { "flex" } else { "none" }>
                <input id="content-file" type="file"/>
                <button id="content-upload" class="btn btn-primary" disabled=move || busy.get() on:click=upload>
                    {move || if busy.get() { "Uploading..." } else { "Upload" }}
                </button>
            </div>
            <table class="vm-table" id="content-table">
                <thead><tr><th>"Name"</th><th>"Size"</th><th>"Path"</th><th>"Actions"</th></tr></thead>
                <tbody>
                    {move || {
                        let list = items.get();
                        if list.is_empty() {
                            view! { <tr><td colspan="4" class="no-data">"Nothing of this type in the pool."</td></tr> }.into_view()
                        } else {
                            list.into_iter().map(|i| {
                                let name = i.name.clone();
                                view! {
                                    <tr class="content-row" data-name=i.name.clone()>
                                        <td>{i.name.clone()}</td>
                                        <td>{api::human_bytes(i.size)}</td>
                                        <td><code>{i.path.clone()}</code></td>
                                        <td><button class="btn btn-sm btn-danger content-delete" on:click=move |_| remove(name.clone())>"Delete"</button></td>
                                    </tr>
                                }
                            }).collect_view()
                        }
                    }}
                </tbody>
            </table>
        </div>
    }
}
