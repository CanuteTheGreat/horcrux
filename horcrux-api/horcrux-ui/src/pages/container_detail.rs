//! Container detail page (`/containers/:id`): what the container is, the
//! power actions that apply in its current state and every page about it
//! (logs, terminal, console, firewall, migrate), like a Proxmox CT summary.
//! LXC containers additionally have their settings/snapshots page
//! (`/containers/:id/lxc`), linked from here.
use crate::api;
use leptos::*;
use leptos_router::use_params_map;

/// Power actions a container offers in `status` (running, stopped, paused).
pub fn ct_actions(status: &str) -> (bool, bool) {
    // (start, stop)
    match status {
        "running" | "paused" => (false, true),
        "stopped" => (true, false),
        _ => (false, false),
    }
}

fn limit(v: u64, unit: &str) -> String {
    if v == 0 {
        "no limit".to_string()
    } else {
        format!("{v} {unit}")
    }
}

#[component]
pub fn ContainerDetailPage() -> impl IntoView {
    let params = use_params_map();
    let id = move || params.get().get("id").cloned().unwrap_or_default();
    let (ct, set_ct) = create_signal(None::<api::Container>);
    let (missing, set_missing) = create_signal(false);
    let (busy, set_busy) = create_signal(false);
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);

    let load = move || {
        let want = id();
        spawn_local(async move {
            match api::get_containers().await {
                Ok(list) => {
                    let found = list.into_iter().find(|c| c.id == want || c.name == want);
                    set_missing.set(found.is_none());
                    set_ct.set(found);
                }
                Err(e) => {
                    set_error.set(Some(format!("Could not load the container: {}", e.message)))
                }
            }
        });
    };
    create_effect(move |_| {
        let _ = id();
        load();
    });

    let power = move |start: bool| {
        let cid = id();
        set_busy.set(true);
        set_error.set(None);
        spawn_local(async move {
            let r = if start {
                api::start_container(&cid).await
            } else {
                api::stop_container(&cid).await
            };
            match r {
                Ok(()) => {
                    // The runtime may report the old state for a moment: poll.
                    let want = if start { "running" } else { "stopped" };
                    for _ in 0..30 {
                        if let Ok(list) = api::get_containers().await {
                            if let Some(c) = list.into_iter().find(|c| c.id == cid) {
                                let done = c.status == want;
                                set_ct.set(Some(c));
                                if done {
                                    break;
                                }
                            }
                        }
                        gloo_timers::future::TimeoutFuture::new(1000).await;
                    }
                    set_notice.set(Some(format!(
                        "{} {}",
                        if start { "Started" } else { "Stopped" },
                        cid
                    )));
                }
                Err(e) => set_error.set(Some(format!(
                    "Could not {} the container: {}",
                    if start { "start" } else { "stop" },
                    e.message
                ))),
            }
            set_busy.set(false);
        });
    };

    let delete = move |_| {
        let cid = id();
        if !crate::pages::backup::common::confirm(&format!(
            "Delete container {cid}? Its files are removed."
        )) {
            return;
        }
        set_busy.set(true);
        spawn_local(async move {
            match api::delete_container(&cid).await {
                Ok(()) => {
                    if let Some(w) = web_sys::window() {
                        let _ = w.location().set_href("/containers");
                    }
                }
                Err(e) => {
                    set_error.set(Some(format!(
                        "Could not delete the container: {}",
                        e.message
                    )));
                    set_busy.set(false);
                }
            }
        });
    };

    view! {
        <div class="container-list container-detail">
            <div class="page-header">
                <h1>{move || ct.get().map(|c| c.name).unwrap_or_else(id)}</h1>
                <div class="header-actions">
                    {move || ct.get().map(|c| {
                        let (can_start, can_stop) = ct_actions(&c.status);
                        let rt = c.runtime.clone();
                        let cid = c.id.clone();
                        let running = c.status == "running";
                        view! {
                            <span id="ctd-status" class=format!("status-{}", c.status)>{c.status.clone()}</span>
                            <button id="ctd-start" class="btn btn-sm btn-success" disabled=move || busy.get() || !can_start on:click=move |_| power(true)>"Start"</button>
                            <button id="ctd-stop" class="btn btn-sm btn-warning" disabled=move || busy.get() || !can_stop on:click=move |_| power(false)>"Stop"</button>
                            {if running {
                                view! { <a class="btn btn-sm" id="ctd-terminal" href=format!("/containers/{}/exec?runtime={}", cid, rt)>"Terminal"</a> }.into_view()
                            } else {
                                view! { <button class="btn btn-sm" id="ctd-terminal" disabled=true title="Start the container to open a terminal in it">"Terminal"</button> }.into_view()
                            }}
                            {matches!(rt.as_str(), "lxc" | "lxd" | "incus").then(|| view! {
                                <a class="btn btn-sm" href=format!("/containers/{}/console?runtime={}", cid, rt)>"Console"</a>
                            })}
                            <a class="btn btn-sm" href=format!("/containers/{}/logs?runtime={}", cid, rt)>"Logs"</a>
                            <a class="btn btn-sm" href=format!("/containers/{}/firewall", cid)>"Firewall"</a>
                            <crate::features::IfFeature feature="cluster">
                                <a class="btn btn-sm" id="ctd-migrate" href=format!("/ha/migration?guest={}&type=container", urlencoding::encode(&cid))>"Migrate"</a>
                            </crate::features::IfFeature>
                            <button id="ctd-delete" class="btn btn-sm btn-danger" disabled=move || busy.get() on:click=delete>"Delete"</button>
                        }
                    })}
                    <a class="btn btn-sm" href="/containers">"All containers"</a>
                </div>
            </div>
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="ctd-error">{e}</div> })}
            {move || notice.get().map(|n| view! { <div class="alert alert-info" id="ctd-notice">{n}</div> })}
            {move || missing.get().then(|| view! {
                <div class="empty-state" id="ctd-missing">
                    <p>{format!("There is no container \"{}\" on this node. It may have been deleted or moved to another node.", id())}</p>
                    <a href="/containers" class="btn btn-primary">"Show all containers"</a>
                </div>
            })}
            {move || ct.get().map(|c| view! {
                <table class="vm-table" id="ctd-summary">
                    <tbody>
                        <tr><th>"Name"</th><td>{c.name.clone()}</td></tr>
                        <tr><th>"ID"</th><td><code>{c.id.clone()}</code></td></tr>
                        <tr><th>"Runtime"</th><td>{c.runtime.clone()}</td></tr>
                        <tr><th>"Image / template"</th><td><code>{c.image.clone()}</code></td></tr>
                        <tr><th>"State"</th><td>{c.status.clone()}</td></tr>
                        <tr><th>"CPU limit"</th><td>{limit(c.cpus as u64, "vCPU")}</td></tr>
                        <tr><th>"Memory limit"</th><td>{limit(if c.memory > 0 { c.memory } else { c.maxmem / 1024 / 1024 }, "MB")}</td></tr>
                        <tr><th>"Created"</th><td>{c.created_at.as_deref().map(crate::utils::format_rfc3339).unwrap_or_else(|| "-".to_string())}</td></tr>
                    </tbody>
                </table>
                {(c.runtime == "lxc").then(|| view! {
                    <p class="form-help">"Settings, root disk, snapshots and clone: "
                        <a href=format!("/containers/{}/lxc", c.id) id="ctd-lxc-link">"LXC settings"</a></p>
                })}
            })}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_actions_follow_the_state() {
        assert_eq!(ct_actions("running"), (false, true));
        assert_eq!(ct_actions("paused"), (false, true));
        assert_eq!(ct_actions("stopped"), (true, false));
        assert_eq!(ct_actions("unknown"), (false, false));
        assert_eq!(limit(0, "MB"), "no limit");
        assert_eq!(limit(512, "MB"), "512 MB");
    }
}
