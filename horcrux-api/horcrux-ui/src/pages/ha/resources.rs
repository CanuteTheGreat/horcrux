//! HA Resources (Proxmox: Datacenter > HA > Resources): which guests HA keeps
//! running, where they run, their requested state and restart/relocate
//! limits, with Add / Edit / Remove.
use crate::api::{
    get_cluster_nodes, get_ha_groups, get_ha_resources, get_vms, ha_resource_add,
    ha_resource_update, remove_ha_resource, HaGroup, HaResource, HaResourceSettings,
};
use crate::components::ui;
use leptos::*;
use leptos_router::A;

/// The form's state: `None` = closed, `Some(None)` = adding, `Some(Some(id))` = editing `id`.
type FormMode = Option<Option<String>>;

fn confirm(msg: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.confirm_with_message(msg).ok())
        .unwrap_or(false)
}

/// Parse a 0-10 limit field; `None` when it is not a number in range.
pub fn parse_limit(s: &str) -> Option<u32> {
    s.trim().parse::<u32>().ok().filter(|n| *n <= 10)
}

/// What the Group column says: a resource without a group may run on any node.
pub fn group_label(group: &str) -> String {
    if group.is_empty() {
        "any node".to_string()
    } else {
        group.to_string()
    }
}

#[component]
pub fn HaResourcesPage() -> impl IntoView {
    let (resources, set_resources) = create_signal(Vec::<HaResource>::new());
    let (groups, set_groups) = create_signal(Vec::<HaGroup>::new());
    let (vms, set_vms) = create_signal(Vec::<(String, String)>::new());
    let (node_count, set_node_count) = create_signal(1usize);
    let (loaded, set_loaded) = create_signal(false);
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let (filter, set_filter) = create_signal(String::new());

    let (mode, set_mode) = create_signal::<FormMode>(None);
    let (f_vm, set_f_vm) = create_signal(String::new());
    let (f_state, set_f_state) = create_signal("started".to_string());
    let (f_group, set_f_group) = create_signal(String::new());
    let (f_restart, set_f_restart) = create_signal("1".to_string());
    let (f_relocate, set_f_relocate) = create_signal("1".to_string());
    let (busy, set_busy) = create_signal(false);

    let reload = move || {
        spawn_local(async move {
            match get_ha_resources().await {
                Ok(r) => {
                    set_resources.set(r);
                    set_error.set(None);
                }
                Err(e) => set_error.set(Some(format!("Cannot load HA resources: {}", e.message))),
            }
            if let Ok(g) = get_ha_groups().await {
                set_groups.set(g);
            }
            if let Ok(v) = get_vms().await {
                set_vms.set(v.into_iter().map(|vm| (vm.id, vm.name)).collect());
            }
            if let Ok(n) = get_cluster_nodes().await {
                set_node_count.set(n.len().max(1));
            }
            set_loaded.set(true);
        });
    };
    reload();
    let interval = set_interval_with_handle(reload, std::time::Duration::from_secs(10)).ok();
    on_cleanup(move || {
        if let Some(h) = interval {
            h.clear();
        }
    });

    let open_add = move |_| {
        set_notice.set(None);
        set_f_vm.set(String::new());
        set_f_state.set("started".into());
        set_f_group.set(String::new());
        set_f_restart.set("1".into());
        set_f_relocate.set("1".into());
        set_mode.set(Some(None));
    };
    let open_edit = move |r: HaResource| {
        set_notice.set(None);
        set_f_vm.set(r.resource_id.clone());
        set_f_state.set(
            if r.state == "stopped" {
                "stopped"
            } else {
                "started"
            }
            .into(),
        );
        set_f_group.set(r.group.clone());
        set_f_restart.set(r.max_restart.to_string());
        set_f_relocate.set(r.max_relocate.to_string());
        set_mode.set(Some(Some(r.resource_id)));
    };

    let form_problem = move || -> Option<&'static str> {
        if mode.get().is_some_and(|m| m.is_none()) && f_vm.get().is_empty() {
            return Some("Choose the virtual machine to protect.");
        }
        if parse_limit(&f_restart.get()).is_none() || parse_limit(&f_relocate.get()).is_none() {
            return Some("Max restart and max relocate are whole numbers from 0 to 10.");
        }
        None
    };

    let submit = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        if form_problem().is_some() {
            return;
        }
        let settings = HaResourceSettings {
            group: f_group.get(),
            max_restart: parse_limit(&f_restart.get()).unwrap_or(1),
            max_relocate: parse_limit(&f_relocate.get()).unwrap_or(1),
            state: f_state.get(),
        };
        let m = mode.get();
        let vm = f_vm.get();
        set_busy.set(true);
        spawn_local(async move {
            let r = match m {
                Some(Some(id)) => ha_resource_update(&id, &settings).await,
                _ => ha_resource_add(&vm, &settings).await,
            };
            set_busy.set(false);
            match r {
                Ok(()) => {
                    set_mode.set(None);
                    set_error.set(None);
                    set_notice.set(Some(format!("HA settings saved for {}.", vm)));
                    reload();
                }
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };

    let remove = move |id: String, name: String| {
        if !confirm(&format!(
            "Stop HA protection for {name}? The guest keeps running; it is just no longer restarted or moved by HA."
        )) {
            return;
        }
        spawn_local(async move {
            match remove_ha_resource(&id).await {
                Ok(()) => {
                    set_notice.set(Some(format!("{name} is no longer HA-protected.")));
                    reload();
                }
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };

    let vm_name = move |id: &str, fallback: &str| -> String {
        vms.with(|v| {
            v.iter()
                .find(|(i, _)| i == id)
                .map(|(_, n)| n.clone())
                .unwrap_or_else(|| fallback.to_string())
        })
    };
    let rows = move || {
        let q = filter.get().to_lowercase();
        let mut r: Vec<HaResource> = resources
            .get()
            .into_iter()
            .filter(|x| {
                q.is_empty()
                    || x.resource_id.to_lowercase().contains(&q)
                    || vm_name(&x.resource_id, &x.vm_name)
                        .to_lowercase()
                        .contains(&q)
                    || x.group.to_lowercase().contains(&q)
            })
            .collect();
        r.sort_by(|a, b| a.resource_id.cmp(&b.resource_id));
        r
    };
    let unprotected = move || {
        let taken: Vec<String> = resources.get().into_iter().map(|r| r.resource_id).collect();
        vms.get()
            .into_iter()
            .filter(|(id, _)| !taken.contains(id))
            .collect::<Vec<_>>()
    };

    view! {
        <div class="page ha-resources-page">
            <ui::PageHeader title="HA Resources" subtitle="Guests that HA keeps running: restarted when they stop unexpectedly, moved to another node when their node fails.">
                <button class="btn btn-primary" id="ha-res-add" on:click=open_add>"Add"</button>
            </ui::PageHeader>
            {move || (loaded.get() && node_count.get() < 2).then(|| view! {
                <div class="alert alert-info" id="ha-res-single-node">
                    "This node is not in a cluster yet. HA restarts a protected guest here when it stops unexpectedly; "
                    "moving it to another node needs more nodes. "
                    <A href="/ha/failover">"Create or join a cluster"</A>
                </div>
            })}
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="ha-res-error" role="alert">{e}</div> })}
            {move || notice.get().map(|n| view! { <div class="alert alert-success" id="ha-res-notice">{n}</div> })}

            {move || mode.get().map(|m| {
                let adding = m.is_none();
                view! {
                    <section class="panel" id="ha-res-form-panel">
                        <div class="panel-head">
                            <h2>{if adding { "Add HA resource".to_string() } else { format!("Edit HA resource {}", vm_name(&f_vm.get_untracked(), &f_vm.get_untracked())) }}</h2>
                        </div>
                        <form class="panel-body" id="ha-res-form" on:submit=submit>
                            {adding.then(|| view! {
                                <div class="form-group">
                                    <label for="ha-res-vm">"Virtual machine"</label>
                                    <select id="ha-res-vm" required on:change=move |ev| set_f_vm.set(event_target_value(&ev))>
                                        <option value="" selected=true>"-- choose --"</option>
                                        {unprotected().into_iter().map(|(id, name)| view! {
                                            <option value=id.clone()>{format!("{name} ({id})")}</option>
                                        }).collect_view()}
                                    </select>
                                    <p class="form-help">"Only guests that are not HA-protected yet are listed."</p>
                                </div>
                            })}
                            <div class="form-group">
                                <label for="ha-res-state">"Requested state"</label>
                                <select id="ha-res-state" on:change=move |ev| set_f_state.set(event_target_value(&ev))>
                                    <option value="started" selected=move || f_state.get() == "started">"started"</option>
                                    <option value="stopped" selected=move || f_state.get() == "stopped">"stopped"</option>
                                </select>
                                <p class="form-help">"started: HA keeps the guest running. stopped: HA keeps it stopped and does not restart it."</p>
                            </div>
                            <div class="form-group">
                                <label for="ha-res-group">"Group"</label>
                                <select id="ha-res-group" on:change=move |ev| set_f_group.set(event_target_value(&ev))>
                                    <option value="" selected=move || f_group.get().is_empty()>"any node"</option>
                                    {move || {
                                        let cur = f_group.get();
                                        groups.get().into_iter().map(|g| {
                                            let sel = g.name == cur;
                                            view! { <option value=g.name.clone() selected=sel>{g.name.clone()}</option> }
                                        }).collect_view()
                                    }}
                                </select>
                                <p class="form-help">"A group limits the nodes the guest may run on. "<A href="/ha/groups">"Manage groups"</A></p>
                            </div>
                            <div class="form-group">
                                <label for="ha-res-max-restart">"Max restart"</label>
                                <input type="number" min="0" max="10" id="ha-res-max-restart" prop:value=f_restart
                                    on:input=move |ev| set_f_restart.set(event_target_value(&ev))/>
                                <p class="form-help">"How often HA restarts a failed guest on the same node before it tries another node (0-10)."</p>
                            </div>
                            <div class="form-group">
                                <label for="ha-res-max-relocate">"Max relocate"</label>
                                <input type="number" min="0" max="10" id="ha-res-max-relocate" prop:value=f_relocate
                                    on:input=move |ev| set_f_relocate.set(event_target_value(&ev))/>
                                <p class="form-help">"How often HA moves the guest to another node before it gives up and marks it as error (0-10)."</p>
                            </div>
                            {move || form_problem().map(|p| view! { <p class="form-error" id="ha-res-form-problem">{p}</p> })}
                            <div class="form-actions">
                                <button type="submit" class="btn btn-primary" id="ha-res-submit"
                                    disabled=move || busy.get() || form_problem().is_some()>
                                    {move || if busy.get() { "Saving..." } else if adding { "Add" } else { "Save" }}
                                </button>
                                <button type="button" class="btn" id="ha-res-cancel" on:click=move |_| set_mode.set(None)>"Cancel"</button>
                            </div>
                        </form>
                    </section>
                }
            })}

            <div class="toolbar">
                <div class="toolbar-start">
                    <input type="search" class="grid-filter" id="ha-res-filter" placeholder="Filter by guest or group"
                        aria-label="Filter HA resources by guest or group"
                        on:input=move |ev| set_filter.set(event_target_value(&ev))/>
                </div>
            </div>
            <div class="grid-wrap" id="ha-res-grid">
                {move || {
                    let list = rows();
                    let total = resources.get().len();
                    if !loaded.get() {
                        view! { <p class="loading">"Loading HA resources..."</p> }.into_view()
                    } else if total == 0 {
                        view! {
                            <ui::EmptyState title="No HA-protected guests"
                                hint="Add a virtual machine so HA restarts it when it stops unexpectedly and moves it when its node fails.">
                                <button class="btn btn-primary" id="ha-res-empty-add" on:click=open_add>"Add"</button>
                            </ui::EmptyState>
                        }.into_view()
                    } else if list.is_empty() {
                        view! { <p class="no-data grid-empty">"No HA resource matches the filter."</p> }.into_view()
                    } else {
                        let shown = list.len();
                        view! {
                            <table class="table ha-res-table">
                                <thead><tr>
                                    <th>"Guest"</th><th>"Requested state"</th><th>"Node"</th><th>"Group"</th>
                                    <th class="num">"Max restart"</th><th class="num">"Max relocate"</th><th class="cell-actions">"Actions"</th>
                                </tr></thead>
                                <tbody>
                                    {list.into_iter().map(|r| {
                                        let id = r.resource_id.clone();
                                        let name = vm_name(&r.resource_id, &r.vm_name);
                                        let (edit_r, rm_id, rm_name) = (r.clone(), id.clone(), name.clone());
                                        view! {
                                            <tr data-vm=id.clone()>
                                                <td><A href=format!("/vms/{id}")>{name.clone()}</A>" "<span class="muted">{format!("({id})")}</span></td>
                                                <td><ui::StateBadge state=r.state.clone()/></td>
                                                <td>{r.current_node.clone().unwrap_or_else(|| "-".into())}</td>
                                                <td>{group_label(&r.group)}</td>
                                                <td class="num">{r.max_restart}</td>
                                                <td class="num">{r.max_relocate}</td>
                                                <td class="cell-actions">
                                                    <button class="btn btn-sm ha-res-edit" on:click=move |_| open_edit(edit_r.clone())>"Edit"</button>
                                                    " "
                                                    <button class="btn btn-sm btn-danger ha-res-remove" on:click=move |_| remove(rm_id.clone(), rm_name.clone())>"Remove"</button>
                                                </td>
                                            </tr>
                                        }
                                    }).collect_view()}
                                </tbody>
                            </table>
                            <div class="grid-footer"><span>{format!("{shown} of {total} HA resources")}</span></div>
                        }.into_view()
                    }
                }}
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_are_whole_numbers_up_to_ten() {
        assert_eq!(parse_limit("0"), Some(0));
        assert_eq!(parse_limit(" 10 "), Some(10));
        assert_eq!(parse_limit("11"), None);
        assert_eq!(parse_limit("-1"), None);
        assert_eq!(parse_limit("two"), None);
    }

    #[test]
    fn a_resource_without_a_group_runs_on_any_node() {
        assert_eq!(group_label(""), "any node");
        assert_eq!(group_label("rack-a"), "rack-a");
    }
}
