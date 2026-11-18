//! Add an ISO from a form: upload from this computer, or download from a
//! URL (optionally verified against a SHA-256), into any ISO-capable
//! storage. Shared by the ISO Library page and the Create VM / VM hardware
//! forms, so they behave identically.
//!
//! Element ids are `<prefix>-storage`, `-file`, `-upload`, `-url`,
//! `-filename`, `-sha256`, `-download`, `-status` and `-error`. Nothing here
//! is a `<form>`, so it nests inside one (Enter in the URL field downloads).
use crate::api::{self, IsoFile, IsoStorage};
use leptos::*;

/// Poll a download until it finishes (or fails); `None` on failure/timeout.
async fn wait_for_download(name: &str, storage: &str) -> Result<IsoFile, String> {
    for _ in 0..1800 {
        match api::list_isos().await {
            Ok(list) => {
                if let Some(i) = list.iter().find(|i| i.name == name && i.storage == storage) {
                    if let Some(e) = &i.error {
                        return Err(format!("download of {name} failed: {e}"));
                    }
                    if !i.downloading {
                        return Ok(i.clone());
                    }
                }
            }
            Err(e) => return Err(e.message),
        }
        gloo_timers::future::TimeoutFuture::new(2000).await;
    }
    Err(format!(
        "download of {name} is still running; check the ISO Library"
    ))
}

/// `storages`: where ISOs can go; `default_storage`: pre-selected one;
/// `on_added`: called with the ISO once it is usable (upload done, or
/// download finished).
#[component]
pub fn IsoAddPanel(
    #[prop(into)] storages: Signal<Vec<IsoStorage>>,
    #[prop(into)] default_storage: Signal<String>,
    #[prop(into)] on_added: Callback<IsoFile>,
    #[prop(into, default = "iso".to_string())] prefix: String,
) -> impl IntoView {
    let id = |s: &str| format!("{prefix}-{s}");
    let (id_status, id_error) = (id("status"), id("error"));
    let (storage, set_storage) = create_signal(None::<String>);
    let chosen = move || storage.get().unwrap_or_else(|| default_storage.get());
    let (url, set_url) = create_signal(String::new());
    let (filename, set_filename) = create_signal(String::new());
    let (sha256, set_sha256) = create_signal(String::new());
    let (busy, set_busy) = create_signal(false);
    let (status, set_status) = create_signal(None::<String>);
    let (error, set_error) = create_signal(None::<String>);
    let file_input = create_node_ref::<html::Input>();

    let sum = move || {
        let s = sha256.get().trim().to_string();
        (!s.is_empty()).then_some(s)
    };

    let upload = move |_| {
        let Some(file) = file_input
            .get()
            .and_then(|i| i.files())
            .and_then(|f| f.get(0))
        else {
            set_error.set(Some("Choose an ISO file to upload first".into()));
            return;
        };
        let (st, sum) = (chosen(), sum());
        set_busy.set(true);
        set_error.set(None);
        set_status.set(Some(format!("Uploading {}...", file.name())));
        spawn_local(async move {
            match api::upload_iso(&st, file, sum.as_deref()).await {
                Ok(iso) => {
                    set_status.set(Some(format!("Uploaded {}", iso.name)));
                    if let Some(i) = file_input.get() {
                        i.set_value("");
                    }
                    on_added.call(iso);
                }
                Err(e) => {
                    set_status.set(None);
                    set_error.set(Some(e.message));
                }
            }
            set_busy.set(false);
        });
    };

    let download = move || {
        let (u, f, st, sum) = (
            url.get_untracked(),
            filename.get_untracked(),
            chosen(),
            sum(),
        );
        if u.trim().is_empty() {
            set_error.set(Some("Enter the ISO's URL first".into()));
            return;
        }
        set_busy.set(true);
        set_error.set(None);
        set_status.set(Some("Starting download...".into()));
        spawn_local(async move {
            let f = f.trim().to_string();
            match api::download_iso(
                u.trim(),
                (!f.is_empty()).then_some(f.as_str()),
                &st,
                sum.as_deref(),
            )
            .await
            {
                Ok(started) => {
                    set_status.set(Some(format!("Downloading {}...", started.name)));
                    match wait_for_download(&started.name, &st).await {
                        Ok(iso) => {
                            // keep the fields on failure so it can be retried
                            set_url.set(String::new());
                            set_filename.set(String::new());
                            set_sha256.set(String::new());
                            set_status.set(Some(format!("Downloaded {}", iso.name)));
                            on_added.call(iso);
                        }
                        Err(e) => {
                            set_status.set(None);
                            set_error.set(Some(e));
                        }
                    }
                }
                Err(e) => {
                    set_status.set(None);
                    set_error.set(Some(e.message));
                }
            }
            set_busy.set(false);
        });
    };

    view! {
        <div class="iso-add" id=id("panel")>
            <div class="form-row fields">
                <div class="form-group"><label for=id("storage")>"Store in"</label>
                <select id=id("storage") on:change=move |ev| set_storage.set(Some(event_target_value(&ev)))>
                    {move || {
                        let cur = chosen();
                        storages.get().into_iter().map(|s| {
                            let label = format!("{} ({} GB free)", s.name, s.available);
                            let sel = s.id == cur;
                            view! { <option value=s.id.clone() selected=sel>{label}</option> }
                        }).collect_view()
                    }}
                </select></div>
                <div class="form-group wide"><label for=id("sha256")>"SHA-256 (optional)"</label>
                <input id=id("sha256") type="text" placeholder="checksum of the file"
                    prop:value=sha256 on:input=move |ev| set_sha256.set(event_target_value(&ev))/></div>
            </div>
            <div class="form-row fields">
                <div class="form-group"><label for=id("file")>"ISO file"</label>
                <input id=id("file") type="file" accept=".iso,.img" node_ref=file_input/></div>
                <button id=id("upload") type="button" class="btn btn-secondary" disabled=move || busy.get() on:click=upload>
                    "Upload from this computer"
                </button>
            </div>
            <div class="form-row fields">
                <div class="form-group wide"><label for=id("url")>"Download URL"</label>
                <input id=id("url") type="url" placeholder="https://example.org/installer.iso"
                    prop:value=url on:input=move |ev| set_url.set(event_target_value(&ev))
                    on:keydown=move |ev| if ev.key() == "Enter" { ev.prevent_default(); download() }/></div>
                <div class="form-group"><label for=id("filename")>"File name (optional)"</label>
                <input id=id("filename") type="text" placeholder="installer.iso"
                    prop:value=filename on:input=move |ev| set_filename.set(event_target_value(&ev))/></div>
                <button id=id("download") type="button" class="btn btn-primary" disabled=move || busy.get() on:click=move |_| download()>
                    "Download from URL"
                </button>
            </div>
            {move || status.get().map(|s| view! { <div class="alert alert-info" id=id_status.clone()>{s}</div> })}
            {move || error.get().map(|e| view! { <div class="alert alert-error" id=id_error.clone()>{e}</div> })}
        </div>
    }
}
