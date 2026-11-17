//! Infrastructure > ISO Library (`/storage/isos`): the ISO library VM CD-ROM
//! drives pick from - list, upload, download from a URL, delete.
use crate::api::{self, IsoFile};
use leptos::*;

#[component]
pub fn IsoLibraryPage() -> impl IntoView {
    let (isos, set_isos) = create_signal(Vec::<IsoFile>::new());
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let (iso_dir, set_iso_dir) = create_signal(String::new());
    let (storages, set_storages) = create_signal(Vec::<api::IsoStorage>::new());
    let (default_storage, set_default_storage) = create_signal(String::new());

    let reload = move || {
        spawn_local(async move {
            match api::list_isos().await {
                Ok(l) => {
                    set_isos.set(l);
                    set_error.set(None);
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            if let Ok(o) = api::get_vm_host_options().await {
                set_iso_dir.set(o.iso_dir);
                set_storages.set(o.iso_storages);
                set_default_storage.set(o.default_iso_storage);
            }
        });
    };
    create_effect(move |_| reload());

    // While something downloads, refresh every 2 s (stops on its own, and
    // when the page is left).
    let handle = store_value(None::<leptos::leptos_dom::helpers::IntervalHandle>);
    create_effect(move |_| {
        let active = isos.get().iter().any(|i| i.downloading);
        match (active, handle.get_value()) {
            (true, None) => {
                if let Ok(h) = set_interval_with_handle(reload, std::time::Duration::from_secs(2)) {
                    handle.set_value(Some(h));
                }
            }
            (false, Some(h)) => {
                h.clear();
                handle.set_value(None);
            }
            _ => {}
        }
    });
    on_cleanup(move || {
        if let Some(h) = handle.get_value() {
            h.clear();
        }
    });

    let added = Callback::new(move |iso: api::IsoFile| {
        set_notice.set(Some(format!("{} is in the ISO library", iso.name)));
        set_error.set(None);
        reload();
    });

    let remove = move |(name, storage): (String, String)| {
        if !web_sys::window()
            .and_then(|w| w.confirm_with_message(&format!("Delete {name}?")).ok())
            .unwrap_or(false)
        {
            return;
        }
        spawn_local(async move {
            match api::delete_iso(&name, &storage).await {
                Ok(()) => set_notice.set(Some(format!("{name} deleted"))),
                Err(e) => set_error.set(Some(e.message)),
            }
            reload();
        });
    };

    view! {
        <div class="iso-library">
            <div class="page-header">
                <h1>"ISO Library"</h1>
            </div>
            <p class="form-help">
                "Installer and live ISOs for VM CD-ROM drives. Add one by uploading it or downloading it from a URL, into any storage; the image library is "
                <code>{move || iso_dir.get()}</code>
                "."
            </p>
            {move || notice.get().map(|n| view! { <div class="alert alert-info" id="iso-notice">{n}</div> })}
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="iso-error">{e}</div> })}

            <crate::components::IsoAddPanel storages=storages default_storage=default_storage on_added=added prefix="iso"/>

            <table class="vm-table" id="iso-table">
                <thead>
                    <tr><th>"Name"</th><th>"Storage"</th><th>"Size"</th><th>"Status"</th><th>"Path"</th><th>"Actions"</th></tr>
                </thead>
                <tbody>
                    {move || {
                        let list = isos.get();
                        if list.is_empty() {
                            // Plain text links into the form above (no extra button).
                            let jump = |id: &'static str| move |ev: ev::MouseEvent| {
                                ev.prevent_default();
                                crate::components::focus::focus_id(id);
                            };
                            view! { <tr><td colspan="6" class="no-data" id="iso-empty">
                                "No ISO images yet. "
                                <a href="#iso-file" on:click=jump("iso-file")>"Upload one"</a>
                                " or "
                                <a href="#iso-url" on:click=jump("iso-url")>"enter a download URL"</a>
                                " in the form above."
                            </td></tr> }.into_view()
                        } else {
                            list.into_iter().map(|iso| {
                                let status = if let Some(e) = &iso.error {
                                    format!("failed: {e}")
                                } else if iso.downloading {
                                    "downloading".to_string()
                                } else {
                                    "ready".to_string()
                                };
                                let deletable = !iso.downloading;
                                let name = (iso.name.clone(), iso.storage.clone());
                                let storage_name = if iso.storage_name.is_empty() { "Image library".to_string() } else { iso.storage_name.clone() };
                                view! {
                                    <tr class="iso-row" data-iso=iso.name.clone()>
                                        <td>{iso.name.clone()}</td>
                                        <td>{storage_name}</td>
                                        <td>{api::human_bytes(iso.size)}</td>
                                        <td class="iso-status">{status}</td>
                                        <td><code>{iso.path.clone()}</code></td>
                                        <td>
                                            <button class="btn btn-sm btn-danger iso-delete" disabled=!deletable
                                                on:click=move |_| remove(name.clone())>"Delete"</button>
                                        </td>
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
