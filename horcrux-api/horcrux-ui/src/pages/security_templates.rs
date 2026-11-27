//! Pentest VM templates: Kali, Parrot Security and Pentoo as first-class
//! templates (download + verify the image once, then create VMs from it).
//! API: horcrux-api/src/api/security/pentest.rs; design
//! docs/design/security-suite.md phase 2.
use crate::api::*;
use horcrux_common::pentest::{CatalogueEntry, CatalogueInfo, CreateOptions};
use leptos::*;
use leptos_router::*;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PentestJob {
    pub phase: String,
    pub done_bytes: u64,
    pub total_bytes: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PentestReady {
    pub verified: String,
    pub completed_at: String,
    pub size_bytes: u64,
    pub virtual_gb: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PentestTemplate {
    #[serde(flatten)]
    pub entry: CatalogueEntry,
    pub status: String,
    pub job: Option<PentestJob>,
    pub ready: Option<PentestReady>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PentestCreated {
    pub vm: horcrux_common::VmConfig,
    pub network: String,
    pub network_note: String,
    pub login: String,
    pub next_steps: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PentestLab {
    pub mode: String,
    pub vnet: String,
    pub bridge_present: bool,
    pub lan_interface: String,
    pub lan_problem: Option<String>,
}

async fn get_pentest_templates() -> Result<Vec<PentestTemplate>, ApiError> {
    fetch_json("/security/pentest/templates").await
}
async fn get_pentest_catalogue() -> Result<CatalogueInfo, ApiError> {
    fetch_json("/security/pentest/catalogue").await
}
async fn get_pentest_lab() -> Result<PentestLab, ApiError> {
    fetch_json("/security/pentest/lab").await
}

fn gib(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / (1u64 << 30) as f64)
}

fn phase_label(p: &str) -> &'static str {
    match p {
        "queued" => "Waiting to start",
        "verifying-vendor" => "Checking the vendor's checksum and signature",
        "downloading" => "Downloading",
        "verifying" => "Verifying the download",
        "unpacking" => "Unpacking",
        "converting" => "Preparing the disk image",
        _ => "Working",
    }
}

#[component]
pub fn SecurityTemplatesPage() -> impl IntoView {
    let query = use_query_map();
    let (items, set_items) = create_signal(None::<Vec<PentestTemplate>>);
    let (cat, set_cat) = create_signal(None::<CatalogueInfo>);
    let (lab, set_lab) = create_signal(None::<PentestLab>);
    let (err, set_err) = create_signal(None::<String>);
    let (ok, set_ok) = create_signal(None::<String>);
    let (created, set_created) = create_signal(None::<PentestCreated>);
    let (unavailable, set_unavailable) = create_signal(None::<Unavailable>);
    // Template whose create form is open (from ?create=<id> too).
    let open = create_rw_signal(query.with_untracked(|q| q.get("create").cloned()));
    let seq = store_value(0u32);
    let load = create_action(move |_: &()| async move {
        seq.update_value(|n| *n += 1);
        let mine = seq.get_value();
        let t = get_pentest_templates().await;
        let c = get_pentest_catalogue().await;
        let l = get_pentest_lab().await;
        if mine != seq.get_value() {
            return; // a newer load answered first
        }
        match t {
            Ok(t) => set_items.set(Some(t)),
            Err(e) => set_err.set(Some(format!("Could not load the templates: {e}"))),
        }
        if let Ok(c) = c {
            set_cat.set(Some(c));
        }
        if let Ok(l) = l {
            set_lab.set(Some(l));
        }
    });
    load.dispatch(());
    // Poll while a download runs (reads the memo only, so an open form keeps
    // its state).
    let downloading = create_memo(move |_| {
        items.with(|v| {
            v.as_ref()
                .is_some_and(|v| v.iter().any(|t| t.status == "downloading"))
        })
    });
    let poll = set_interval_with_handle(
        move || {
            if downloading.get_untracked() {
                load.dispatch(());
            }
        },
        std::time::Duration::from_secs(3),
    )
    .ok();
    on_cleanup(move || {
        if let Some(p) = poll {
            p.clear();
        }
    });
    let act = create_action(move |(path, what, del): &(String, String, bool)| {
        let (path, what, del) = (path.clone(), what.clone(), *del);
        async move {
            set_err.set(None);
            set_ok.set(None);
            let r: Result<(), ApiError> = if del {
                delete_json(&path).await
            } else {
                post_json_no_response(&path, &serde_json::json!({})).await
            };
            match r {
                Ok(()) => {
                    set_unavailable.set(None);
                    set_ok.set(Some(what))
                }
                Err(e) => match e.unavailable() {
                    Some(u) => set_unavailable.set(Some(u)),
                    None => set_err.set(Some(format!("{e}"))),
                },
            }
            load.dispatch(());
        }
    });
    let refresh = create_action(move |_: &()| async move {
        set_err.set(None);
        set_ok.set(None);
        let r: Result<CatalogueInfo, ApiError> = post_json(
            "/security/pentest/catalogue/refresh",
            &serde_json::json!({}),
        )
        .await;
        match r {
            Ok(c) => {
                set_ok.set(Some(format!("Catalogue refreshed (serial {}).", c.serial)));
                set_cat.set(Some(c));
                load.dispatch(());
            }
            Err(e) => match e.unavailable() {
                Some(u) => set_unavailable.set(Some(u)),
                None => set_err.set(Some(format!("{e}"))),
            },
        }
    });
    view! {
        <div class="security-page pentest-page">
            <div class="page-header">
                <h1>"Pentest VM templates"</h1>
                <div class="header-actions">
                    <button class="btn btn-secondary" disabled=move || refresh.pending().get()
                        on:click=move |_| refresh.dispatch(())>"Refresh catalogue"</button>
                </div>
            </div>
            {super::security::security_tabs("templates")}
            <p class="help-text">
                "Kali, Parrot Security and Pentoo as ready-made VMs. Download an image once (its checksum and the vendor's signature are verified), then create as many VMs from it as you like. \
                 New VMs go on an isolated lab network with no uplink unless you pick the LAN. Use these only on systems you are authorised to test."
            </p>
            {move || cat.get().map(|c| view! {
                <p class="help-text mono pentest-catalogue">
                    {format!("Catalogue: {} (serial {}, {})", c.source, c.serial, c.generated)}
                    {c.last_error.map(|e| format!(" · last refresh failed: {e}"))}
                </p>
            })}
            {move || lab.get().map(|l| view! {
                <p class="help-text pentest-lab">
                    {if l.mode == "isolated" {
                        format!("Default network: isolated lab bridge {} ({}).", l.vnet, if l.bridge_present { "present" } else { "created with the first VM" })
                    } else {
                        format!("Default network: the LAN{}.", if l.lan_interface.is_empty() { String::new() } else { format!(" through {}", l.lan_interface) })
                    }}
                    {l.lan_problem.map(|p| format!(" LAN mode is not possible here: {p}"))}
                </p>
            })}
            {move || err.get().map(|m| view! { <div class="alert alert-error">{m}</div> })}
            {move || ok.get().map(|m| view! { <div class="alert alert-success">{m}</div> })}
            {move || unavailable.get().map(|u| view! {
                <crate::components::UnavailableState info=u on_retry=Callback::new(move |_| { set_unavailable.set(None); load.dispatch(()) })/>
            })}
            {move || created.get().map(|c| {
                let href = format!("/vms/{}", c.vm.id);
                let console = format!("/vms/{}/console", c.vm.id);
                view! {
                    <div class="card pentest-created">
                        <h2>{format!("{} is ready", c.vm.name)}</h2>
                        <p>{c.next_steps.clone()}</p>
                        <p class="help-text">{c.network_note.clone()}</p>
                        <p>"Login: "<span class="mono">{c.login.clone()}</span></p>
                        <p>
                            <a class="btn btn-primary" href=href>"Open the VM"</a>" "
                            <a class="btn btn-secondary" href=console>"Console"</a>
                        </p>
                    </div>
                }
            })}
            {move || match items.get() {
                None => view! { <div class="loading">"Loading..."</div> }.into_view(),
                Some(v) => v.into_iter().map(|t| template_card(t, open, act, set_created, set_err, load)).collect_view(),
            }}
        </div>
    }
}

fn template_card(
    t: PentestTemplate,
    open: RwSignal<Option<String>>,
    act: Action<(String, String, bool), ()>,
    set_created: WriteSignal<Option<PentestCreated>>,
    set_err: WriteSignal<Option<String>>,
    load: Action<(), ()>,
) -> impl IntoView {
    let e = t.entry.clone();
    let id = e.id.clone();
    let hw = e.hardware.clone();
    let status = t.status.clone();
    let id_dl = id.clone();
    let id_cancel = id.clone();
    let id_del = id.clone();
    let id_open = id.clone();
    let name_dl = e.name.clone();
    let progress = t.job.clone().filter(|_| status == "downloading").map(|j| {
        let pct = if j.total_bytes > 0 {
            ((j.done_bytes as f64 / j.total_bytes as f64) * 100.0).min(100.0) as u32
        } else {
            0
        };
        let label = if j.phase == "downloading" || j.phase == "unpacking" {
            format!(
                "{} · {} of {}",
                phase_label(&j.phase),
                gib(j.done_bytes),
                gib(j.total_bytes)
            )
        } else {
            phase_label(&j.phase).to_string()
        };
        view! {
            <div class="pentest-progress">
                <div class="progress-bar"><div class="progress-fill" style=format!("width: {pct}%")></div></div>
                <span class="progress-text">{label}</span>
            </div>
        }
    });
    let failed = t
        .job
        .clone()
        .filter(|j| j.phase == "failed")
        .and_then(|j| j.error)
        .map(|m| view! { <div class="alert alert-error">{format!("Download failed: {m}")}</div> });
    let ready_info = t.ready.clone().map(|r| {
        view! {
            <p class="help-text">
                <span class="sev-badge sev-none">"image ready"</span>" "
                {format!("{} · downloaded {}", r.verified, r.completed_at.chars().take(16).collect::<String>().replace('T', " "))}
            </p>
        }
    });
    let is_ready = status == "ready";
    let is_dl = status == "downloading";
    let pretty_status = match status.as_str() {
        "ready" => "Ready",
        "downloading" => "Downloading",
        "failed" => "Download failed",
        _ => "Not downloaded",
    };
    let create_form = move || {
        (open.get().as_deref() == Some(id_open.as_str()) && is_ready)
            .then(|| create_form_view(e.clone(), set_created, set_err, load, open))
    };
    let created_open = {
        let id = id.clone();
        move |_| open.set(Some(id.clone()))
    };
    view! {
        <div class="card pentest-template" id=format!("tpl-{id}")>
            <div class="pentest-head">
                <h2>{t.entry.name.clone()}" "<span class="help-text">{t.entry.version.clone()}</span></h2>
                <span class=format!("sev-badge {}", if is_ready { "sev-none" } else { "sev-low" })>{pretty_status}</span>
            </div>
            <p>{t.entry.description.clone()}</p>
            <p class="help-text mono">
                {format!("{} vCPU · {} MB RAM · {} GB disk · {} · {}",
                    hw.cpus, hw.memory_mb, hw.disk_gb,
                    if hw.firmware == "uefi" { "UEFI" } else { "BIOS" },
                    if hw.display == "spice" { "SPICE" } else { "VNC" })}
            </p>
            <p class="help-text">{t.entry.notes.clone()}</p>
            <p class="help-text">
                {format!("Download {}", gib(t.entry.image.size_bytes))}
                " · "
                <a href=t.entry.homepage.clone() target="_blank" rel="noopener noreferrer">"project site"</a>
            </p>
            {ready_info}
            {progress}
            {failed}
            <div class="form-actions">
                {(!is_ready && !is_dl).then(|| {
                    let id = id_dl.clone();
                    let name = name_dl.clone();
                    view! {
                        <button class="btn btn-primary" on:click=move |_| act.dispatch((format!("/security/pentest/templates/{id}/download"), format!("Download of {name} started."), false))>"Download image"</button>
                    }
                })}
                {is_dl.then(|| {
                    let id = id_cancel.clone();
                    view! {
                        <button class="btn btn-secondary" on:click=move |_| act.dispatch((format!("/security/pentest/templates/{id}/cancel"), "Download cancelled.".to_string(), false))>"Cancel"</button>
                    }
                })}
                {is_ready.then(|| view! {
                    <button class="btn btn-primary" on:click=created_open>"Create VM"</button>
                })}
                {is_ready.then(|| {
                    let id = id_del.clone();
                    view! {
                        <button class="btn btn-danger" on:click=move |_| {
                            if super::security::confirm("Delete the stored image? VMs already created from it keep working.") {
                                act.dispatch((format!("/security/pentest/templates/{id}/image"), "Image deleted.".to_string(), true))
                            }
                        }>"Delete image"</button>
                    }
                })}
            </div>
            {create_form}
        </div>
    }
}

fn create_form_view(
    e: CatalogueEntry,
    set_created: WriteSignal<Option<PentestCreated>>,
    set_err: WriteSignal<Option<String>>,
    load: Action<(), ()>,
    open: RwSignal<Option<String>>,
) -> impl IntoView {
    let name = create_rw_signal(format!("{}-lab", e.id));
    let network = create_rw_signal("isolated".to_string());
    let cpus = create_rw_signal(e.hardware.cpus.to_string());
    let mem = create_rw_signal(e.hardware.memory_mb.to_string());
    let disk = create_rw_signal(e.hardware.disk_gb.to_string());
    let start = create_rw_signal(false);
    let id = e.id.clone();
    let submit = create_action(move |_: &()| {
        let id = id.clone();
        async move {
            set_err.set(None);
            let o = CreateOptions {
                name: name.get_untracked(),
                id: String::new(),
                network: network.get_untracked(),
                cpus: cpus.get_untracked().trim().parse().unwrap_or(0),
                memory_mb: mem.get_untracked().trim().parse().unwrap_or(0),
                disk_gb: disk.get_untracked().trim().parse().unwrap_or(0),
                storage: String::new(),
                start: start.get_untracked(),
            };
            let r: Result<PentestCreated, ApiError> =
                post_json(&format!("/security/pentest/templates/{id}/create"), &o).await;
            match r {
                Ok(c) => {
                    set_created.set(Some(c));
                    open.set(None);
                }
                Err(e) => set_err.set(Some(format!("Could not create the VM: {e}"))),
            }
            load.dispatch(());
        }
    });
    view! {
        <form class="settings-form pentest-create" on:submit=move |ev| { ev.prevent_default(); submit.dispatch(()) }>
            <h3>{format!("New VM from {}", e.name)}</h3>
            <div class="form-row">
                <div class="form-group">
                    <label for=format!("pt-name-{}", e.id)>"VM name"</label>
                    <input id=format!("pt-name-{}", e.id) type="text" prop:value=move || name.get()
                        on:input=move |ev| name.set(event_target_value(&ev))/>
                </div>
                <div class="form-group">
                    <label for=format!("pt-net-{}", e.id)>"Network"</label>
                    <select id=format!("pt-net-{}", e.id) on:change=move |ev| network.set(event_target_value(&ev))>
                        <option value="isolated" selected=move || network.get() == "isolated">"Isolated lab (no uplink)"</option>
                        <option value="lan" selected=move || network.get() == "lan">"LAN (reaches your network)"</option>
                    </select>
                    <small class="form-help">"The isolated lab reaches other lab VMs only. Choose the LAN only for systems you are allowed to test."</small>
                </div>
            </div>
            <div class="form-row">
                <div class="form-group">
                    <label for=format!("pt-cpu-{}", e.id)>"vCPUs"</label>
                    <input id=format!("pt-cpu-{}", e.id) type="number" min="1" prop:value=move || cpus.get()
                        on:input=move |ev| cpus.set(event_target_value(&ev))/>
                </div>
                <div class="form-group">
                    <label for=format!("pt-mem-{}", e.id)>"Memory (MB)"</label>
                    <input id=format!("pt-mem-{}", e.id) type="number" min="512" prop:value=move || mem.get()
                        on:input=move |ev| mem.set(event_target_value(&ev))/>
                </div>
                <div class="form-group">
                    <label for=format!("pt-disk-{}", e.id)>"Disk (GB)"</label>
                    <input id=format!("pt-disk-{}", e.id) type="number" min="1" prop:value=move || disk.get()
                        on:input=move |ev| disk.set(event_target_value(&ev))/>
                    <small class="form-help">"Never smaller than the image's own disk."</small>
                </div>
            </div>
            <div class="form-group">
                <label for=format!("pt-start-{}", e.id)><input id=format!("pt-start-{}", e.id) type="checkbox" prop:checked=move || start.get()
                    on:change=move |ev| start.set(event_target_checked(&ev))/>" Start it when it is ready"</label>
            </div>
            <div class="form-actions">
                <button type="submit" class="btn btn-primary" disabled=move || submit.pending().get()>
                    {move || if submit.pending().get() { "Creating (copying the disk)..." } else { "Create" }}
                </button>
                <button type="button" class="btn btn-secondary" on:click=move |_| open.set(None)>"Cancel"</button>
            </div>
        </form>
    }
}
