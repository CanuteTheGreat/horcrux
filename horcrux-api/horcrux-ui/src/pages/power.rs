//! Infrastructure > Guest Power (`/system/power`): Hyper-V-style automatic
//! stop/start actions (node default and per guest), CRIU/QEMU capability
//! warnings, and Save state / Resume / Discard for every VM and container.
use crate::api::{self, GuestPowerOverride, NodePower, PowerGuest, PowerOverview};
use leptos::*;

pub fn human_size(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

/// "Saved 12.3 MB, 2026-10-22 16:03" for the guest lists.
pub fn saved_label(s: &api::SavedState) -> String {
    let when = s
        .saved_at
        .get(..16)
        .unwrap_or(&s.saved_at)
        .replace('T', " ");
    match s.size_bytes {
        Some(b) => format!("Saved {} · {} UTC", human_size(b), when),
        None => format!("Saved · {} UTC", when),
    }
}

fn stop_label(a: &str) -> &'static str {
    match a {
        "save" => "Save state",
        "shutdown" => "Shut down",
        "poweroff" => "Power off",
        _ => "?",
    }
}

fn start_label(a: &str) -> &'static str {
    match a {
        "resume_if_running" => "Resume if it was running",
        "always" => "Always start",
        "nothing" => "Nothing",
        _ => "?",
    }
}

#[component]
pub fn GuestPowerPage() -> impl IntoView {
    let (ov, set_ov) = create_signal(None::<PowerOverview>);
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let node = create_rw_signal(None::<NodePower>);

    let reload = move || {
        spawn_local(async move {
            match api::power_overview().await {
                Ok(o) => {
                    node.set(Some(o.node.clone()));
                    set_ov.set(Some(o));
                    set_error.set(None);
                }
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };
    create_effect(move |_| reload());

    let save_node = move |_| {
        let Some(p) = node.get() else { return };
        spawn_local(async move {
            match api::put_node_power(&p).await {
                Ok(_) => set_notice.set(Some("Node defaults saved".into())),
                Err(e) => set_error.set(Some(e.message)),
            }
            reload();
        });
    };

    let boot_now = move |_| {
        set_notice.set(Some("Resuming saved guests...".into()));
        spawn_local(async move {
            match api::boot_saved_guests().await {
                Ok(r) => set_notice.set(Some(format!(
                    "Boot pass: {}",
                    r["message"].as_str().unwrap_or("done")
                ))),
                Err(e) => set_error.set(Some(e.message)),
            }
            reload();
        });
    };

    let upd = move |f: Box<dyn Fn(&mut NodePower)>| {
        node.update(|n| {
            if let Some(n) = n.as_mut() {
                f(n)
            }
        })
    };

    view! {
        <div class="guest-power">
            <div class="page-header"><h1>"Guest Power: save state on shutdown"</h1></div>
            <p class="form-help">
                "When this host shuts down or reboots, Horcrux saves each running guest's memory and device state to disk (like Hyper-V's \"Save state\") and resumes it exactly where it was when the host boots again. Where state cannot be saved (e.g. CRIU is missing for a container) the guest is shut down cleanly and started again instead; the reason is shown next to the guest."
            </p>
            {move || notice.get().map(|n| view! { <div class="alert alert-info" id="power-notice">{n}</div> })}
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="power-error">{e}</div> })}

            {move || ov.get().map(|o| {
                let c = o.capabilities;
                let criu = if c.criu.available {
                    format!("CRIU {} is available: containers can be saved.", c.criu.version.clone().unwrap_or_default())
                } else {
                    format!("Containers cannot save state on this node: {}", c.criu.error.clone().unwrap_or_default())
                };
                view! {
                    <div class=if c.criu.available { "alert alert-info" } else { "alert alert-warning" } id="power-criu">{criu}</div>
                    <p class="form-help" id="power-accel">
                        {if c.kvm { "VMs run with KVM acceleration; their state is saved with QEMU." } else { "No /dev/kvm: VMs run under TCG emulation; their state is saved with QEMU all the same." }}
                    </p>
                }
            })}

            <h2>"Node defaults"</h2>
            {move || node.get().map(|n| view! {
                <form id="power-node-form" class="form-row" on:submit=move |ev| ev.prevent_default()>
                    <label>"Automatic stop action "
                        <select id="power-stop" prop:value=n.stop_action.clone()
                            on:change=move |ev| { let v = event_target_value(&ev); upd(Box::new(move |n| n.stop_action = v.clone())) }>
                            <option value="save">"Save state"</option>
                            <option value="shutdown">"Shut down"</option>
                            <option value="poweroff">"Power off"</option>
                        </select></label>
                    <label>"Automatic start action "
                        <select id="power-start" prop:value=n.start_action.clone()
                            on:change=move |ev| { let v = event_target_value(&ev); upd(Box::new(move |n| n.start_action = v.clone())) }>
                            <option value="resume_if_running">"Resume if it was running"</option>
                            <option value="always">"Always start"</option>
                            <option value="nothing">"Nothing"</option>
                        </select></label>
                    <label>"Start delay (s) "
                        <input id="power-delay" type="number" min="0" max="86400" prop:value=n.start_delay_secs
                            on:input=move |ev| { let v = event_target_value(&ev).parse().unwrap_or(0); upd(Box::new(move |n| n.start_delay_secs = v)) }/></label>
                    <label>"Save states to "
                        <input id="power-dir" type="text" placeholder="default: <data dir>/saved-state/data"
                            prop:value=n.save_dir.clone().unwrap_or_default()
                            on:input=move |ev| { let v = event_target_value(&ev); upd(Box::new(move |n| n.save_dir = if v.trim().is_empty() { None } else { Some(v.clone()) })) }/></label>
                    <label>"Save timeout (s) "
                        <input id="power-save-timeout" type="number" min="5" prop:value=n.save_timeout_secs
                            on:input=move |ev| { let v = event_target_value(&ev).parse().unwrap_or(300); upd(Box::new(move |n| n.save_timeout_secs = v)) }/></label>
                    <label>"Shutdown timeout (s) "
                        <input id="power-shutdown-timeout" type="number" min="5" prop:value=n.shutdown_timeout_secs
                            on:input=move |ev| { let v = event_target_value(&ev).parse().unwrap_or(120); upd(Box::new(move |n| n.shutdown_timeout_secs = v)) }/></label>
                    <label>"Guests at once "
                        <input id="power-parallel" type="number" min="1" max="64" prop:value=n.max_parallel
                            on:input=move |ev| { let v = event_target_value(&ev).parse().unwrap_or(4); upd(Box::new(move |n| n.max_parallel = v)) }/></label>
                    <button id="power-node-save" type="button" class="btn btn-primary" on:click=save_node>"Save defaults"</button>
                    <button id="power-boot-now" type="button" class="btn btn-secondary" on:click=boot_now
                        title="Run the boot pass now: resume saved guests per their start actions">"Resume saved guests now"</button>
                </form>
            })}

            <h2>"Guests"</h2>
            <table class="vm-table" id="power-table">
                <thead><tr>
                    <th>"Guest"</th><th>"Status"</th><th>"Saved state"</th>
                    <th>"Stop action"</th><th>"Start action"</th><th>"Delay (s)"</th><th>"Order"</th>
                    <th>"Actions"</th><th>"Notes"</th>
                </tr></thead>
                <tbody>
                    {move || match ov.get() {
                        None => view! { <tr><td colspan="9" class="no-data">"Loading..."</td></tr> }.into_view(),
                        Some(o) if o.guests.is_empty() => view! { <tr><td colspan="9" class="no-data">"No guests yet."</td></tr> }.into_view(),
                        Some(o) => o.guests.into_iter().map(|g| view! {
                            <GuestPowerRow g=g set_notice set_error reload/>
                        }).collect_view(),
                    }}
                </tbody>
            </table>
        </div>
    }
}

#[component]
fn GuestPowerRow(
    g: PowerGuest,
    set_notice: WriteSignal<Option<String>>,
    set_error: WriteSignal<Option<String>>,
    reload: impl Fn() + Copy + 'static,
) -> impl IntoView {
    let stop = create_rw_signal(g.overrides.stop_action.clone().unwrap_or_default());
    let start = create_rw_signal(g.overrides.start_action.clone().unwrap_or_default());
    let delay = create_rw_signal(
        g.overrides
            .start_delay_secs
            .map(|d| d.to_string())
            .unwrap_or_default(),
    );
    let order = create_rw_signal(
        g.overrides
            .start_order
            .map(|d| d.to_string())
            .unwrap_or_default(),
    );
    let up = g.status == "running" || g.status == "paused";
    let kind = g.kind.clone();
    let id = g.id.clone();
    let name = g.name.clone();

    let apply = {
        let (kind, id) = (kind.clone(), id.clone());
        move |_| {
            let o = GuestPowerOverride {
                stop_action: Some(stop.get()).filter(|s| !s.is_empty()),
                start_action: Some(start.get()).filter(|s| !s.is_empty()),
                start_delay_secs: delay.get().trim().parse().ok(),
                start_order: order.get().trim().parse().ok(),
                save_dir: None,
            };
            let (kind, id) = (kind.clone(), id.clone());
            spawn_local(async move {
                match api::put_guest_power(&kind, &id, &o).await {
                    Ok(_) => set_notice.set(Some(format!("Power settings of {id} saved"))),
                    Err(e) => set_error.set(Some(e.message)),
                }
                reload();
            });
        }
    };
    let act = move |what: &'static str, kind: String, id: String| {
        move |_| {
            let (kind, id) = (kind.clone(), id.clone());
            set_notice.set(Some(format!("{what} {id}...")));
            spawn_local(async move {
                let r = match what {
                    "Saving state of" => api::save_guest_state(&kind, &id).await.map(|_| ()),
                    "Resuming" => api::resume_guest_state(&kind, &id).await.map(|_| ()),
                    _ => api::discard_guest_state(&kind, &id).await,
                };
                match r {
                    Ok(()) => {
                        set_error.set(None);
                        set_notice.set(Some(format!("{what} {id}: done")))
                    }
                    Err(e) => set_error.set(Some(format!("{what} {id} failed: {}", e.message))),
                }
                reload();
            });
        }
    };
    let saved = g.saved.clone();
    let notes = {
        let mut n = Vec::new();
        if let Some(b) = &g.save_blocker {
            n.push(format!("State cannot be saved: {b}"));
        }
        if let Some(r) = &g.last_result {
            if let Some(m) = &r.message {
                n.push(format!("Last {} ({}): {m}", r.phase, r.action));
            }
        }
        n.join(" | ")
    };
    let eff = g.policy.clone();

    view! {
        <tr class="power-row" data-guest-id=g.id.clone() data-kind=g.kind.clone()>
            <td><strong>{name}</strong>" "<small>{format!("({} {})", g.kind, g.runtime)}</small></td>
            <td>{g.status.clone()}</td>
            <td>{match &saved {
                Some(s) => view! { <span class="badge badge-info saved-badge" title=s.mechanism.clone()>{saved_label(s)}</span> }.into_view(),
                None => view! { <span>"-"</span> }.into_view(),
            }}</td>
            <td><select class="power-guest-stop" prop:value=stop on:change=move |ev| stop.set(event_target_value(&ev))>
                <option value="">{format!("Node default ({})", stop_label(&eff.stop_action))}</option>
                <option value="save">"Save state"</option>
                <option value="shutdown">"Shut down"</option>
                <option value="poweroff">"Power off"</option>
            </select></td>
            <td><select class="power-guest-start" prop:value=start on:change=move |ev| start.set(event_target_value(&ev))>
                <option value="">{format!("Node default ({})", start_label(&eff.start_action))}</option>
                <option value="resume_if_running">"Resume if it was running"</option>
                <option value="always">"Always start"</option>
                <option value="nothing">"Nothing"</option>
            </select></td>
            <td><input class="power-guest-delay" type="number" min="0" style="width:5em" placeholder=eff.start_delay_secs.to_string()
                prop:value=delay on:input=move |ev| delay.set(event_target_value(&ev))/></td>
            <td><input class="power-guest-order" type="number" style="width:5em" placeholder=eff.start_order.to_string()
                prop:value=order on:input=move |ev| order.set(event_target_value(&ev))/></td>
            <td class="actions">
                <button class="btn btn-sm btn-primary power-apply" on:click=apply>"Apply"</button>
                <button class="btn btn-sm btn-warning power-save-state" disabled=!(up && g.can_save && saved.is_none())
                    title=if g.can_save { "Save RAM and device state to disk and stop" } else { "State cannot be saved for this guest (see notes)" }
                    on:click=act("Saving state of", kind.clone(), id.clone())>"Save state"</button>
                <button class="btn btn-sm btn-success power-resume" disabled=!(saved.is_some() && !up)
                    on:click=act("Resuming", kind.clone(), id.clone())>"Resume"</button>
                <button class="btn btn-sm btn-secondary power-discard" disabled=saved.is_none()
                    title="Delete the saved state; the guest then starts normally"
                    on:click=act("Discarding state of", kind.clone(), id.clone())>"Discard"</button>
            </td>
            <td class="power-notes">{notes}</td>
        </tr>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_human() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn saved_label_shows_size_and_time() {
        let s = api::SavedState {
            kind: "vm".into(),
            id: "1".into(),
            name: "n".into(),
            runtime: "qemu".into(),
            saved_at: "2026-10-22T16:03:11.5+00:00".into(),
            mechanism: "m".into(),
            size_bytes: Some(2 * 1024 * 1024),
            state_dir: "/x".into(),
            note: None,
        };
        assert_eq!(saved_label(&s), "Saved 2.0 MB · 2026-10-22 16:03 UTC");
        let t = api::SavedState {
            size_bytes: None,
            ..s
        };
        assert!(saved_label(&t).starts_with("Saved · 2026-10-22"));
    }
}
