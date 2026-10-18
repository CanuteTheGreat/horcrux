//! Infrastructure > ISO Images (`/storage/isos`): the ISO library VM CD-ROM
//! drives pick from - list, download from a URL, delete.
use crate::api::{self, IsoFile};
use leptos::*;

#[component]
pub fn IsoLibraryPage() -> impl IntoView {
    let (isos, set_isos) = create_signal(Vec::<IsoFile>::new());
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let (url, set_url) = create_signal(String::new());
    let (filename, set_filename) = create_signal(String::new());
    let (iso_dir, set_iso_dir) = create_signal(String::new());

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

    let download = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let (u, f) = (url.get(), filename.get());
        spawn_local(async move {
            let f = f.trim().to_string();
            match api::download_iso(u.trim(), (!f.is_empty()).then_some(f.as_str())).await {
                Ok(iso) => {
                    set_notice.set(Some(format!("Downloading {}", iso.name)));
                    set_url.set(String::new());
                    set_filename.set(String::new());
                    set_error.set(None);
                }
                Err(e) => set_error.set(Some(e.message)),
            }
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
        spawn_local(async move {
            match api::delete_iso(&name).await {
                Ok(()) => set_notice.set(Some(format!("{name} deleted"))),
                Err(e) => set_error.set(Some(e.message)),
            }
            reload();
        });
    };

    view! {
        <div class="iso-library">
            <div class="page-header">
                <h1>"ISO Images"</h1>
            </div>
            <p class="form-help">
                "Installer and live ISOs for VM CD-ROM drives. Stored in "
                <code>{move || iso_dir.get()}</code>
                "; ISOs in storage pool directories are listed too."
            </p>
            {move || notice.get().map(|n| view! { <div class="alert alert-info" id="iso-notice">{n}</div> })}
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="iso-error">{e}</div> })}

            <form id="iso-download-form" class="form-row" on:submit=download>
                <input id="iso-url" type="url" required placeholder="https://.../installer.iso" style="flex: 3"
                    prop:value=url on:input=move |ev| set_url.set(event_target_value(&ev))/>
                <input id="iso-filename" type="text" placeholder="file name (optional)" style="flex: 1"
                    prop:value=filename on:input=move |ev| set_filename.set(event_target_value(&ev))/>
                <button id="iso-download" type="submit" class="btn btn-primary">"Download from URL"</button>
            </form>

            <table class="vm-table" id="iso-table">
                <thead>
                    <tr><th>"Name"</th><th>"Size"</th><th>"Status"</th><th>"Path"</th><th>"Actions"</th></tr>
                </thead>
                <tbody>
                    {move || {
                        let list = isos.get();
                        if list.is_empty() {
                            view! { <tr><td colspan="5" class="no-data">"No ISO images yet. Download one above."</td></tr> }.into_view()
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
                                let name = iso.name.clone();
                                view! {
                                    <tr class="iso-row" data-iso=iso.name.clone()>
                                        <td>{iso.name.clone()}</td>
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
