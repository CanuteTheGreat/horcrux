//! HA > Maintenance & Rebalance (`/ha/maintenance`): put this node in
//! maintenance (drain: running VMs are live-migrated to other nodes, what
//! cannot move stays and is saved on shutdown), see the plan first, rebalance
//! the cluster (settings, preview, "Rebalance now"), set placement rules per
//! VM, and read the guests' location history.
use crate::api::{self, MoveOutcome, MoveRun, PlacementRules, RebalanceSettings};
use leptos::*;

fn plan_table(plan: &api::MovePlan, id: &'static str) -> impl IntoView {
    let moves = plan.moves.clone();
    let unplaced = plan.unplaced.clone();
    view! {
        <table class="vm-table" id=id>
            <thead><tr><th>"Guest"</th><th>"From"</th><th>"To"</th><th>"Why"</th></tr></thead>
            <tbody>
                {if moves.is_empty() && unplaced.is_empty() {
                    view! { <tr><td colspan="4" class="no-data">"Nothing to move."</td></tr> }.into_view()
                } else {
                    moves.into_iter().map(|m| view! {
                        <tr class="plan-move"><td>{format!("{} ({})", m.name, m.guest_id)}</td><td>{m.from}</td><td>{m.to}</td><td>{m.reason}</td></tr>
                    }).collect_view()
                }}
                {unplaced.into_iter().map(|u| view! {
                    <tr class="plan-unplaced"><td>{format!("{} ({})", u.name, u.guest_id)}</td><td colspan="2">"stays here: state is saved instead"</td><td>{u.reason}</td></tr>
                }).collect_view()}
            </tbody>
        </table>
    }
}

fn results_table(results: Vec<MoveOutcome>, id: &'static str) -> impl IntoView {
    view! {
        <table class="vm-table" id=id>
            <thead><tr><th>"Guest"</th><th>"From → To"</th><th>"Status"</th><th>"Progress"</th><th>"Error"</th></tr></thead>
            <tbody>
                {results.into_iter().map(|r| view! {
                    <tr class="move-result" data-status=r.status.clone()>
                        <td>{format!("{} ({})", r.name, r.guest_id)}</td>
                        <td>{format!("{} → {}", r.from, r.to)}</td>
                        <td>{r.status.clone()}</td>
                        <td><progress max="100" value=r.progress></progress>{format!(" {}%", r.progress)}</td>
                        <td>{r.error.clone().unwrap_or_default()}</td>
                    </tr>
                }).collect_view()}
            </tbody>
        </table>
    }
}

fn run_view(run: Option<MoveRun>, id: &'static str) -> View {
    match run {
        None => view! { <p class="form-help">"No run yet."</p> }.into_view(),
        Some(r) => view! {
            <p class="form-help" id=format!("{id}-status")>
                {format!("{} started {} - {}", r.kind, r.started_at.get(..19).unwrap_or(&r.started_at).replace('T', " "), r.status)}
            </p>
            {results_table(r.results.clone(), id)}
            {(!r.plan.unplaced.is_empty()).then(|| plan_table(&api::MovePlan { moves: vec![], unplaced: r.plan.unplaced.clone() }, "drain-unplaced"))}
        }
        .into_view(),
    }
}

#[component]
pub fn MaintenancePage() -> impl IntoView {
    let (maint, set_maint) = create_signal(api::MaintenanceState::default());
    let (reb, set_reb) = create_signal(None::<api::RebalanceState>);
    let (history, set_history) = create_signal(Vec::<api::MoveHistoryEntry>::new());
    let (drain_preview, set_drain_preview) = create_signal(None::<api::MovePlan>);
    let (vms, set_vms) = create_signal(Vec::<(String, String, PlacementRules)>::new());
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let settings = create_rw_signal(None::<RebalanceSettings>);

    let reload = move || {
        spawn_local(async move {
            if let Ok(m) = api::get_maintenance().await {
                set_maint.set(m);
            }
            if let Ok(r) = api::get_rebalance().await {
                if settings.get_untracked().is_none() {
                    settings.set(Some(r.settings.clone()));
                }
                set_reb.set(Some(r));
            }
            if let Ok(h) = api::list_moves().await {
                set_history.set(h);
            }
        });
    };
    let reload_vms = move || {
        spawn_local(async move {
            if let Ok(list) = api::get_vms().await {
                let mut out = Vec::new();
                for v in list {
                    let r = api::get_placement(&v.id).await.unwrap_or_default();
                    out.push((v.id.clone(), v.name.clone(), r));
                }
                set_vms.set(out);
            }
        });
    };
    create_effect(move |_| {
        reload();
        reload_vms();
    });
    // Progress of a running drain / rebalance.
    let interval = set_interval_with_handle(reload, std::time::Duration::from_secs(3)).ok();
    on_cleanup(move || {
        if let Some(h) = interval {
            h.clear();
        }
    });

    let enter = move |drain: bool| {
        move |_| {
            set_error.set(None);
            spawn_local(async move {
                match api::set_maintenance(true, drain).await {
                    Ok(_) => set_notice.set(Some(if drain {
                        "Maintenance on: moving running VMs to other nodes...".into()
                    } else {
                        "Maintenance on (nothing moved)".into()
                    })),
                    Err(e) => set_error.set(Some(e.message)),
                }
                reload();
            });
        }
    };
    let leave = move |_| {
        spawn_local(async move {
            match api::set_maintenance(false, false).await {
                Ok(_) => {
                    set_notice.set(Some("Maintenance off: this node takes guests again".into()))
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            reload();
        });
    };
    let preview_drain = move |_| {
        spawn_local(async move {
            match api::drain_plan().await {
                Ok(p) => set_drain_preview.set(Some(p)),
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };
    let save_settings = move |_| {
        let Some(s) = settings.get() else { return };
        spawn_local(async move {
            match api::put_rebalance(&s).await {
                Ok(_) => set_notice.set(Some("Rebalance settings saved".into())),
                Err(e) => set_error.set(Some(e.message)),
            }
            reload();
        });
    };
    let rebalance_now = move |_| {
        set_notice.set(Some("Rebalancing...".into()));
        spawn_local(async move {
            match api::run_rebalance(false).await {
                Ok(r) => set_notice.set(Some(format!(
                    "Rebalance: {} move(s) planned",
                    r.plan.moves.len()
                ))),
                Err(e) => set_error.set(Some(e.message)),
            }
            reload();
        });
    };

    view! {
        <div class="maintenance-page">
            <div class="page-header"><h1>"Maintenance & Rebalance"</h1></div>
            {move || notice.get().map(|n| view! { <div class="alert alert-info" id="maint-notice">{n}</div> })}
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="maint-error">{e}</div> })}

            <h2>"This node"</h2>
            <p class="form-help">
                "In maintenance a node takes no new guests. \"Drain\" live-migrates its running VMs to other nodes (placed by free RAM, architecture and the placement rules below, without a restart). Guests that cannot move stay and are saved to disk when the host shuts down. Hosts that shut down drain automatically first when \"Drain on shutdown\" is on (Guest Power)."
            </p>
            <p id="maint-state">
                {move || {
                    let m = maint.get();
                    if !m.clustered {
                        "This node is not in a cluster: there is nowhere to move guests to.".to_string()
                    } else if m.enabled {
                        format!("In maintenance since {} ({})", m.since.clone().unwrap_or_default().get(..19).unwrap_or("").replace('T', " "), m.reason.clone().unwrap_or_default())
                    } else {
                        "Active: taking guests".to_string()
                    }
                }}
            </p>
            <div class="form-row">
                <button id="maint-enter-drain" class="btn btn-warning" on:click=enter(true)
                    disabled=move || maint.get().enabled || !maint.get().clustered>"Enter maintenance (drain)"</button>
                <button id="maint-enter-nodrain" class="btn btn-secondary" on:click=enter(false)
                    disabled=move || maint.get().enabled || !maint.get().clustered>"Enter maintenance (don't move guests)"</button>
                <button id="maint-leave" class="btn btn-success" on:click=leave disabled=move || !maint.get().enabled>"Leave maintenance"</button>
                <button id="maint-preview" class="btn btn-secondary" on:click=preview_drain>"Preview drain plan"</button>
            </div>
            {move || drain_preview.get().map(|p| view! { <h3>"Drain plan (not executed)"</h3> {plan_table(&p, "drain-plan")} })}
            <h3>"Drain progress"</h3>
            {move || run_view(maint.get().drain, "drain-results")}

            <h2>"Rebalance"</h2>
            <p class="form-help">
                "Evens out RAM load with live migrations only (never more than the limit at once), honouring the placement rules. It runs on demand, and - depending on the mode - when a node returns from maintenance or a new node joins (\"On return\") or whenever the spread exceeds the threshold (\"Automatic\")."
            </p>
            {move || settings.get().map(|s| view! {
                <div class="form-row" id="rebalance-settings">
                    <label>"Mode "
                        <select id="rebalance-mode" prop:value=s.mode.clone()
                            on:change=move |ev| { let v = event_target_value(&ev); settings.update(|s| if let Some(s) = s { s.mode = v }) }>
                            <option value="off">"Off (only Rebalance now)"</option>
                            <option value="on_return">"On return / new node"</option>
                            <option value="auto">"Automatic"</option>
                        </select></label>
                    <label>"Moves at once "
                        <input id="rebalance-max" type="number" min="1" max="32" prop:value=s.max_moves
                            on:input=move |ev| { let v = event_target_value(&ev).parse().unwrap_or(2); settings.update(|s| if let Some(s) = s { s.max_moves = v }) }/></label>
                    <label>"Spread threshold (points) "
                        <input id="rebalance-threshold" type="number" min="0.1" max="100" step="0.1" prop:value=s.threshold_pct
                            on:input=move |ev| { let v = event_target_value(&ev).parse().unwrap_or(20.0); settings.update(|s| if let Some(s) = s { s.threshold_pct = v }) }/></label>
                    <label>"Least seconds between automatic runs "
                        <input id="rebalance-interval" type="number" min="0" prop:value=s.min_interval_secs
                            on:input=move |ev| { let v = event_target_value(&ev).parse().unwrap_or(300); settings.update(|s| if let Some(s) = s { s.min_interval_secs = v }) }/></label>
                    <button id="rebalance-save" class="btn btn-primary" on:click=save_settings>"Save"</button>
                    <button id="rebalance-now" class="btn btn-warning" on:click=rebalance_now>"Rebalance now"</button>
                </div>
            })}
            {move || reb.get().map(|r| view! {
                <h3>"Nodes"</h3>
                <table class="vm-table" id="rebalance-nodes">
                    <thead><tr><th>"Node"</th><th>"State"</th><th>"RAM (MB)"</th></tr></thead>
                    <tbody>{r.nodes.iter().map(|n| view! {
                        <tr><td>{n.name.clone()}</td>
                            <td>{if !n.online { "offline" } else if n.maintenance { "maintenance" } else { "active" }}</td>
                            <td>{n.mem_total_mb}</td></tr>
                    }).collect_view()}</tbody>
                </table>
                <h3>"Plan if rebalanced now"</h3>
                {plan_table(&r.plan, "rebalance-plan")}
                <h3>"Last rebalance"</h3>
                {run_view(r.last.clone(), "rebalance-results")}
            })}

            <h2>"Placement rules"</h2>
            <table class="vm-table" id="placement-table">
                <thead><tr><th>"VM"</th><th>"Pin to node"</th><th>"Affinity group (stay together)"</th><th>"Anti-affinity group (spread)"</th><th>"Never migrate"</th><th></th></tr></thead>
                <tbody>
                    {move || vms.get().into_iter().map(|(id, name, rules)| view! {
                        <PlacementRow id name rules set_notice set_error/>
                    }).collect_view()}
                </tbody>
            </table>

            <h2>"Guest location history"</h2>
            <table class="vm-table" id="moves-table">
                <thead><tr><th>"When"</th><th>"Guest"</th><th>"From → To"</th><th>"Why"</th><th>"Result"</th></tr></thead>
                <tbody>
                    {move || {
                        let h = history.get();
                        if h.is_empty() {
                            view! { <tr><td colspan="5" class="no-data">"No guest has been moved yet."</td></tr> }.into_view()
                        } else {
                            h.into_iter().map(|m| view! {
                                <tr class="move-history">
                                    <td>{m.at.get(..19).unwrap_or(&m.at).replace('T', " ")}</td>
                                    <td>{format!("{} ({})", m.name, m.guest_id)}</td>
                                    <td>{format!("{} → {}", m.from, m.to)}</td>
                                    <td>{m.reason.clone()}</td>
                                    <td>{if m.ok { "moved".to_string() } else { format!("failed: {}", m.error.clone().unwrap_or_default()) }}</td>
                                </tr>
                            }).collect_view()
                        }
                    }}
                </tbody>
            </table>
        </div>
    }
}

#[component]
fn PlacementRow(
    id: String,
    name: String,
    rules: PlacementRules,
    set_notice: WriteSignal<Option<String>>,
    set_error: WriteSignal<Option<String>>,
) -> impl IntoView {
    let pin = create_rw_signal(rules.pin.clone().unwrap_or_default());
    let aff = create_rw_signal(rules.affinity_group.clone().unwrap_or_default());
    let anti = create_rw_signal(rules.anti_affinity_group.clone().unwrap_or_default());
    let nomig = create_rw_signal(rules.no_migrate);
    let vm = id.clone();
    let apply = move |_| {
        let r = PlacementRules {
            pin: Some(pin.get().trim().to_string()).filter(|s| !s.is_empty()),
            affinity_group: Some(aff.get().trim().to_string()).filter(|s| !s.is_empty()),
            anti_affinity_group: Some(anti.get().trim().to_string()).filter(|s| !s.is_empty()),
            no_migrate: nomig.get(),
        };
        let vm = vm.clone();
        spawn_local(async move {
            match api::put_placement(&vm, &r).await {
                Ok(_) => set_notice.set(Some(format!("Placement rules of {vm} saved"))),
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };
    view! {
        <tr class="placement-row" data-vm=id.clone()>
            <td>{format!("{name} ({id})")}</td>
            <td><input class="pl-pin" type="text" placeholder="any node" prop:value=pin on:input=move |ev| pin.set(event_target_value(&ev))/></td>
            <td><input class="pl-aff" type="text" prop:value=aff on:input=move |ev| aff.set(event_target_value(&ev))/></td>
            <td><input class="pl-anti" type="text" prop:value=anti on:input=move |ev| anti.set(event_target_value(&ev))/></td>
            <td><input class="pl-nomig" type="checkbox" prop:checked=nomig on:change=move |ev| nomig.set(event_target_checked(&ev))/></td>
            <td><button class="btn btn-sm btn-primary pl-apply" on:click=apply>"Apply"</button></td>
        </tr>
    }
}
