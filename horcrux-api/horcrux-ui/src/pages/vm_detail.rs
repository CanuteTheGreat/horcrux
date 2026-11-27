//! VM detail page (`/vms/:id`) with tabs: Summary (state, resources, pool,
//! tags you can edit) and links to the Hardware, Console, Snapshots and
//! Firewall views of the same VM.
use crate::api::{self, InvResource};
use horcrux_common::VmConfig;
use leptos::*;
use leptos_router::*;

#[derive(Params, PartialEq, Clone)]
struct VmParams {
    id: Option<String>,
}

#[component]
pub fn VmDetailPage() -> impl IntoView {
    let params = use_params::<VmParams>();
    let id = move || params.get().ok().and_then(|p| p.id).unwrap_or_default();
    let (vm, set_vm) = create_signal(None::<VmConfig>);
    let (inv, set_inv) = create_signal(None::<InvResource>);
    let (tags_text, set_tags_text) = create_signal(String::new());
    let (error, set_error) = create_signal(None::<String>);
    // The VM id in the URL doesn't exist (deleted, mistyped link): show that,
    // not the settings forms of a VM that isn't there.
    let (missing, set_missing) = create_signal(false);
    let (notice, set_notice) = create_signal(None::<String>);
    let (power, set_power) = create_signal(None::<api::GuestPolicyView>);
    let stop_sel = create_rw_signal(String::new());
    let hib_sel = create_rw_signal(String::new());

    let load = move || {
        let vid = id();
        if vid.is_empty() {
            return;
        }
        spawn_local(async move {
            match api::get_vm(&vid).await {
                Ok(v) => {
                    crate::titles::set_subject(&v.name);
                    set_vm.set(Some(v));
                    set_missing.set(false);
                    set_error.set(None);
                }
                Err(e) if e.status == 404 => {
                    set_vm.set(None);
                    set_missing.set(true);
                    set_error.set(None);
                    return;
                }
                Err(e) => set_error.set(Some(format!("Could not load the VM: {}", e.message))),
            }
            if let Ok(p) = api::guest_power_policy("vm", &vid).await {
                stop_sel.set(p.overrides.stop_action.clone().unwrap_or_default());
                hib_sel.set(match p.overrides.guest_hibernate {
                    Some(true) => "on".to_string(),
                    Some(false) => "off".to_string(),
                    None => String::new(),
                });
                set_power.set(Some(p));
            }
            if let Ok(r) = api::search_resources(&vid, "vm", "", "").await {
                let me = r.into_iter().find(|x| x.id == vid);
                set_tags_text.set(me.as_ref().map(|m| m.tags.join(", ")).unwrap_or_default());
                set_inv.set(me);
            }
        });
    };
    create_effect(move |_| {
        id();
        load();
    });

    let save_tags = move |_| {
        let vid = id();
        let tags: Vec<String> = tags_text
            .get_untracked()
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect();
        spawn_local(async move {
            match api::put_guest_tags("vm", &vid, &tags).await {
                Ok(t) => {
                    set_notice.set(Some(format!("Tags saved: {}", t.join(", "))));
                    set_error.set(None);
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            load();
        });
    };

    let hibernate_now = move |_| {
        let vid = id();
        set_notice.set(Some("Asking the guest to hibernate...".into()));
        spawn_local(async move {
            match api::hibernate_vm(&vid).await {
                Ok(_) => {
                    set_notice.set(Some(
                        "The guest hibernated; start the VM to resume it".into(),
                    ));
                    set_error.set(None);
                }
                Err(e) => set_error.set(Some(format!("Hibernate failed: {}", e.message))),
            }
            load();
        });
    };

    let save_power = move |_| {
        let vid = id();
        let o = api::GuestPowerOverride {
            stop_action: Some(stop_sel.get_untracked()).filter(|s| !s.is_empty()),
            guest_hibernate: match hib_sel.get_untracked().as_str() {
                "on" => Some(true),
                "off" => Some(false),
                _ => None,
            },
            ..power
                .get_untracked()
                .map(|p| p.overrides)
                .unwrap_or_default()
        };
        spawn_local(async move {
            match api::put_guest_power("vm", &vid, &o).await {
                Ok(_) => {
                    set_notice.set(Some("Host shutdown behaviour saved".into()));
                    set_error.set(None);
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            load();
        });
    };

    view! {
        <div class="vm-detail">
            <div class="page-header"><h1>{move || vm.get().map(|v| format!("{} ({})", v.name, v.id)).unwrap_or_else(id)}</h1></div>
            <crate::components::VmTabs vm_id=Signal::derive(id) active="Summary"/>
            {move || notice.get().map(|n| view! { <div class="alert alert-info" id="vmd-notice">{n}</div> })}
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="vmd-error">{e}
                <button type="button" class="btn btn-secondary btn-sm" on:click=move |_| load()>"Retry"</button></div> })}
            <Show when=move || missing.get() fallback=|| ()>
                <div class="empty-state" id="vmd-missing">
                    <p>{move || format!("There is no VM \"{}\" on this node. It may have been deleted or moved.", id())}</p>
                    <A href="/vms" class="btn btn-primary">"Show all VMs"</A>
                </div>
            </Show>
            <Show when=move || vm.get().is_some() fallback=|| ()>
            <crate::components::VmPowerBar vm_id=Signal::derive(id) vm=Signal::derive(move || vm.get())
                on_change=Callback::new(move |v: VmConfig| set_vm.set(Some(v)))/>
            {move || vm.get().map(|v| view! {
                <table class="vm-table" id="vmd-summary">
                    <tr><th>"State"</th><td class="vmd-status">{format!("{:?}", v.status)}</td></tr>
                    <tr><th>"CPUs"</th><td>{v.cpus}</td></tr>
                    <tr><th>"Memory"</th><td>{format!("{} MB", v.memory)}</td></tr>
                    <tr><th>"Disks"</th><td>{v.disks.len()}</td></tr>
                    <tr><th>"Pool"</th><td class="vmd-pool">{inv.get().and_then(|i| i.pool).unwrap_or_else(|| "-".into())}</td></tr>
                </table>
            })}
            <h3>"When the host shuts down or restarts"</h3>
            <div class="form-row">
                <select id="vmd-stop" aria-label="When the host shuts down or restarts" prop:value=stop_sel
                    on:change=move |ev| stop_sel.set(event_target_value(&ev))>
                    <option value="">{move || format!("Node default ({})", power.get().map(|p| p.effective.stop_action).unwrap_or_default())}</option>
                    <option value="save">"Save state (resume where it was)"</option>
                    <option value="shutdown">"Shut down cleanly"</option>
                    <option value="poweroff">"Power off"</option>
                    <option value="none">"Do nothing (guest ends with the host)"</option>
                </select>
                <button id="vmd-stop-save" class="btn btn-primary" on:click=save_power>"Save"</button>
                <select id="vmd-hibernate-sel" title="Passthrough VMs: let the guest hibernate itself at host shutdown" prop:value=hib_sel
                    on:change=move |ev| hib_sel.set(event_target_value(&ev))>
                    <option value="">"Guest hibernate: node default"</option>
                    <option value="on">"Guest hibernate: on"</option>
                    <option value="off">"Guest hibernate: off"</option>
                </select>
                <button id="vmd-hibernate-now" class="btn btn-secondary" on:click=hibernate_now
                    disabled=move || !vm.get().map(|v| v.status == horcrux_common::VmStatus::Running).unwrap_or(false)
                    title=move || if vm.get().map(|v| v.status == horcrux_common::VmStatus::Running).unwrap_or(false) {
                        "Ask the guest to hibernate now; start the VM to resume"
                    } else {
                        "Only a running VM can hibernate"
                    }>"Hibernate now"</button>
                <a class="btn btn-secondary" href="/system/power">"Host power settings"</a>
            </div>
            {move || power.get().map(|p| {
                let a = p.host_stop_action.clone();
                let text = match (p.effective.stop_action.as_str(), a.as_str()) {
                    ("save", "save") => "At host shutdown this VM's memory is saved to encrypted disk and it resumes exactly where it was at the next boot.".to_string(),
                    ("save", "hibernate") => format!(
                        "This VM owns passthrough hardware, so its memory cannot be saved by the host. At host shutdown its guest will hibernate itself and resume at the next boot. {}",
                        p.host_stop_reason.clone().unwrap_or_default()
                    ),
                    ("save", other) => format!(
                        "This VM will {} at host shutdown, not save: {}",
                        if other == "poweroff" { "be powered off" } else { "be shut down cleanly" },
                        p.host_stop_reason.clone().unwrap_or_default()
                    ),
                    _ => format!("At host shutdown this VM will: {}.", match a.as_str() {
                        "shutdown" => "be shut down cleanly",
                        "poweroff" => "be powered off",
                        _ => "be left alone and end with the host",
                    }),
                };
                let cls = if p.effective.stop_action == "save" && a != "save" && a != "hibernate" { "alert alert-warning" } else { "form-help" };
                view! { <div class=cls id="vmd-stop-plan">{text}</div> }
            })}
            <h3>"Tags"</h3>
            <div class="form-row">
                <input id="vmd-tags" type="text" aria-label="Tags (comma-separated)" placeholder="prod, web" style="flex: 3" prop:value=tags_text
                    on:input=move |ev| set_tags_text.set(event_target_value(&ev))/>
                <button id="vmd-tags-save" class="btn btn-primary" on:click=save_tags>"Save tags"</button>
            </div>
            </Show>
        </div>
    }
}
