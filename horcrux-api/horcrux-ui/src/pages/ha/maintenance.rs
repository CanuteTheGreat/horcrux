//! HA > Maintenance & Rebalance (`/ha/maintenance?node=<name>`): put any
//! cluster member in maintenance (drain: its HA guests and running guests
//! move to other members, what cannot move is refused unless forced) and take
//! it out again (return policy origin / rebalance / none), see the plan
//! first, follow the progress, rebalance
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

/// One line saying what a plan does, with counts that jump to the rows.
fn plan_summary(plan: &api::MovePlan, table: &'static str) -> impl IntoView {
    let (m, u) = (plan.moves.len(), plan.unplaced.len());
    view! {
        <p class="form-help plan-summary">
            <a href=format!("#{table}") class="plan-moves-count">{format!("{m} guest(s) move")}</a>
            ", "
            <a href=format!("#{table}") class="plan-stay-count">{format!("{u} cannot move")}</a>
        </p>
    }
}

/// Enter / leave maintenance for one cluster member (any member: the API
/// forwards to it), with the plan preview and the live progress. Used by
/// Cluster > Nodes and by HA > Maintenance.
#[component]
pub fn NodeMaintenancePanel(node: String) -> impl IntoView {
    let (state, set_state) = create_signal(None::<api::MaintenanceState>);
    let (plan, set_plan) = create_signal(None::<api::MovePlan>);
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    // "" = the node's/cluster's policy
    let policy = create_rw_signal(String::new());
    let force = create_rw_signal(false);
    let no_drain = create_rw_signal(false);
    let node = store_value(node);

    let reload_state = move || {
        let n = node.get_value();
        spawn_local(async move {
            match api::get_node_maintenance(&n).await {
                Ok(m) => set_state.set(Some(m)),
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };
    // The plan of the next action: the drain while active, the return while
    // in maintenance. Reloaded on demand, never by the poll.
    let reload_plan = move || {
        let n = node.get_value();
        let p = policy.get_untracked();
        spawn_local(async move {
            let enabled = api::get_node_maintenance(&n)
                .await
                .map(|m| m.enabled)
                .unwrap_or(false);
            let r = if enabled {
                api::node_maintenance_plan(&n, Some(&p)).await
            } else {
                api::node_maintenance_plan(&n, None).await
            };
            match r {
                Ok(pl) => set_plan.set(Some(pl)),
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };
    create_effect(move |_| {
        reload_state();
        reload_plan();
    });
    let interval = set_interval_with_handle(reload_state, std::time::Duration::from_secs(3)).ok();
    on_cleanup(move || {
        if let Some(h) = interval {
            h.clear();
        }
    });
    let enabled = create_memo(move |_| state.get().map(|m| m.enabled).unwrap_or(false));
    let clustered = create_memo(move |_| state.get().map(|m| m.clustered).unwrap_or(false));
    let running = create_memo(move |_| {
        state
            .get()
            .is_some_and(|m| api::run_busy(&m.drain) || api::run_busy(&m.exit))
    });

    let enter = move |_| {
        set_error.set(None);
        let n = node.get_value();
        let (p, f, nd) = (policy.get(), force.get(), no_drain.get());
        spawn_local(async move {
            match api::set_node_maintenance(&n, true, !nd, Some(&p), f).await {
                Ok(m) => {
                    set_notice.set(Some(if nd {
                        format!("{n} is in maintenance (nothing moved)")
                    } else {
                        format!("{n} is entering maintenance: its guests are moving to other members...")
                    }));
                    set_state.set(Some(m));
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            reload_plan();
        });
    };
    let exit = move |_| {
        set_error.set(None);
        let n = node.get_value();
        let p = policy.get();
        spawn_local(async move {
            match api::set_node_maintenance(&n, false, false, Some(&p), false).await {
                Ok(m) => {
                    set_notice.set(Some(format!(
                        "{n} left maintenance (return policy {})",
                        if p.is_empty() { m.effective_return_policy.clone() } else { p.clone() }
                    )));
                    set_state.set(Some(m));
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            reload_plan();
        });
    };

    view! {
        <div class="node-maint-panel" data-node=node.get_value()>
            <h3>{format!("Maintenance of {}", node.get_value())}</h3>
            {move || notice.get().map(|n| view! { <div class="alert alert-info" id="node-maint-notice">{n}</div> })}
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="node-maint-error">{e}</div> })}
            <p id="node-maint-state" data-enabled=move || enabled.get().to_string()>
                {move || match state.get() {
                    None => "Loading...".to_string(),
                    Some(m) if !m.clustered => "Not in a cluster: there is nowhere to move guests to.".to_string(),
                    Some(m) if m.enabled => format!(
                        "In maintenance since {} ({}); leaving will apply return policy \"{}\"",
                        m.since.clone().unwrap_or_default().get(..19).unwrap_or("").replace('T', " "),
                        m.reason.clone().unwrap_or_default(),
                        m.effective_return_policy
                    ),
                    Some(_) => "Active: takes guests".to_string(),
                }}
            </p>
            <p class="form-help">
                "Entering moves every HA-managed guest (running ones live, stopped ones offline) and every other running guest to other members, placed by free RAM, architecture and the placement rules; HA then never places a guest here. Leaving applies the return policy: \"origin\" moves those guests back here, \"rebalance\" lets the rebalancer even out the cluster, \"none\" leaves them where they are."
            </p>
            <div class="form-row">
                <label>"Return policy "
                    <select id="node-maint-return" prop:value=move || policy.get()
                        on:change=move |ev| { policy.set(event_target_value(&ev)); reload_plan(); }>
                        <option value="">"Default (node / cluster setting)"</option>
                        <option value="origin">"origin: bring the guests back"</option>
                        <option value="rebalance">"rebalance"</option>
                        <option value="none">"none: leave them where they are"</option>
                    </select>
                </label>
                <label><input id="node-maint-force" type="checkbox" prop:checked=move || force.get()
                    on:change=move |ev| force.set(event_target_checked(&ev))/>" Enter even if some guests cannot move"</label>
                <label><input id="node-maint-nodrain" type="checkbox" prop:checked=move || no_drain.get()
                    on:change=move |ev| no_drain.set(event_target_checked(&ev))/>" Don't move guests"</label>
            </div>
            <div class="form-row">
                <button id="node-maint-enter" class="btn btn-warning" on:click=enter
                    disabled=move || enabled.get() || !clustered.get() || running.get()>"Enter maintenance"</button>
                <button id="node-maint-exit" class="btn btn-success" on:click=exit
                    disabled=move || !enabled.get() || running.get()>"Exit maintenance"</button>
                <button id="node-maint-preview" class="btn btn-secondary" on:click=move |_| reload_plan()>"Refresh plan"</button>
            </div>
            <h4>{move || if enabled.get() { "What leaving would bring back" } else { "What entering would move" }}</h4>
            {move || plan.get().map(|p| view! { {plan_summary(&p, "node-maint-plan")} {plan_table(&p, "node-maint-plan")} })}
            <h4>"Drain progress"</h4>
            {move || run_view(state.get().and_then(|m| m.drain), "node-maint-drain")}
            <h4>"Last exit"</h4>
            {move || run_view(state.get().and_then(|m| m.exit), "node-maint-exit-run")}
        </div>
    }
}

#[component]
pub fn MaintenancePage() -> impl IntoView {
    let (reb, set_reb) = create_signal(None::<api::RebalanceState>);
    let (history, set_history) = create_signal(Vec::<api::MoveHistoryEntry>::new());
    let (vms, set_vms) = create_signal(Vec::<(String, String, PlacementRules)>::new());
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let settings = create_rw_signal(None::<RebalanceSettings>);
    let (members, set_members) = create_signal(None::<api::ClusterMaintenance>);
    let default_policy = create_rw_signal(None::<String>);
    let query = leptos_router::use_query_map();
    let picked = create_rw_signal(query.get_untracked().get("node").cloned());

    let reload = move || {
        spawn_local(async move {
            if let Ok(c) = api::get_cluster_maintenance().await {
                if picked.get_untracked().is_none() {
                    picked.set(c.nodes.iter().find(|n| n.local).map(|n| n.node.clone()));
                }
                if default_policy.get_untracked().is_none() {
                    default_policy.set(Some(c.settings.return_policy.clone()));
                }
                set_members.set(Some(c));
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
    // Progress of a running rebalance, members' badges.
    let interval = set_interval_with_handle(reload, std::time::Duration::from_secs(5)).ok();
    on_cleanup(move || {
        if let Some(h) = interval {
            h.clear();
        }
    });

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
    let save_default_policy = move |_| {
        let Some(p) = default_policy.get() else { return };
        spawn_local(async move {
            match api::put_maintenance_settings(&api::MaintenanceSettings { return_policy: p.clone() }).await {
                Ok(_) => set_notice.set(Some(format!("Default return policy: {p}"))),
                Err(e) => set_error.set(Some(e.message)),
            }
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
    let member_names = create_memo(move |_| {
        members
            .get()
            .map(|c| c.nodes.iter().map(|n| (n.node.clone(), n.local)).collect::<Vec<_>>())
            .unwrap_or_default()
    });

    view! {
        <div class="maintenance-page ha-page p-6">
            <div class="page-header"><h1>"Maintenance & Rebalance"</h1></div>
            {move || notice.get().map(|n| view! { <div class="alert alert-info" id="maint-notice">{n}</div> })}
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="maint-error">{e}</div> })}

            <h2>"Node maintenance"</h2>
            <p class="form-help">
                "Pick any cluster member: the request goes to that node. Hosts that shut down drain automatically first when \"Drain on shutdown\" is on (Guest Power)."
            </p>
            <div class="form-row">
                <label>"Node "
                    <select id="maint-node" prop:value=move || picked.get().unwrap_or_default()
                        on:change=move |ev| picked.set(Some(event_target_value(&ev)))>
                        {move || member_names.get().into_iter().map(|(n, local)| view! {
                            <option value=n.clone()>{if local { format!("{n} (this node)") } else { n }}</option>
                        }).collect_view()}
                    </select>
                </label>
                <label>"Default return policy (cluster) "
                    <select id="maint-default-return" prop:value=move || default_policy.get().unwrap_or_default()
                        on:change=move |ev| default_policy.set(Some(event_target_value(&ev)))>
                        <option value="origin">"origin"</option>
                        <option value="rebalance">"rebalance"</option>
                        <option value="none">"none"</option>
                    </select>
                </label>
                <button id="maint-default-save" class="btn btn-primary" on:click=save_default_policy>"Save"</button>
            </div>
            <table class="vm-table" id="maint-members">
                <thead><tr><th>"Node"</th><th>"State"</th><th>"Moved away"</th></tr></thead>
                <tbody>
                    {move || members.get().map(|c| c.nodes.into_iter().map(|row| {
                        let badge = api::maintenance_badge(&row).unwrap_or(if row.online { "active" } else { "offline" });
                        let moved = row.maintenance.as_ref().map(|m| m.origins.len()).unwrap_or(0);
                        let href = format!("/ha/maintenance?node={}", row.node);
                        let (n1, n2, n3) = (row.node.clone(), row.node.clone(), row.node.clone());
                        view! {
                            <tr class="maint-member" data-node=row.node.clone() data-state=badge>
                                <td><a href=href.clone() on:click=move |_| picked.set(Some(n1.clone()))>{row.node.clone()}</a></td>
                                <td><a href=href.clone() class="maint-badge" on:click=move |_| picked.set(Some(n2.clone()))>{badge}</a></td>
                                <td><a href=format!("{href}#node-maint-plan") class="maint-moved" on:click=move |_| picked.set(Some(n3.clone()))>{moved.to_string()}</a></td>
                            </tr>
                        }
                    }).collect_view())}
                </tbody>
            </table>
            {move || picked.get().map(|n| view! { <NodeMaintenancePanel node=n/> })}

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
                <thead><tr><th>"VM"</th><th>"Pin to node"</th><th>"Affinity group (stay together)"</th><th>"Anti-affinity group (spread)"</th><th>"Never migrate"</th><th>"Required node labels (comma-separated)"</th><th></th></tr></thead>
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
                            view! { <tr><td colspan="5" class="no-data">"No guest has been moved yet." " " <button class="btn btn-primary btn-sm" type="button" on:click=move |_| crate::components::focus::focus_id("rebalance-now")>"Rebalance now"</button></td></tr> }.into_view()
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
    let labels = create_rw_signal(rules.required_labels.join(", "));
    let vm = id.clone();
    let apply = move |_| {
        let r = PlacementRules {
            pin: Some(pin.get().trim().to_string()).filter(|s| !s.is_empty()),
            affinity_group: Some(aff.get().trim().to_string()).filter(|s| !s.is_empty()),
            anti_affinity_group: Some(anti.get().trim().to_string()).filter(|s| !s.is_empty()),
            no_migrate: nomig.get(),
            required_labels: labels
                .get()
                .split(',')
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect(),
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
            <td><input class="pl-labels" type="text" placeholder="none" prop:value=labels on:input=move |ev| labels.set(event_target_value(&ev))/></td>
            <td><button class="btn btn-sm btn-primary pl-apply" on:click=apply>"Apply"</button></td>
        </tr>
    }
}

#[cfg(test)]
mod tests {
    use crate::api::{self, MaintenanceState, MoveRun, NodeMaintenanceRow};

    fn row(m: Option<MaintenanceState>) -> NodeMaintenanceRow {
        NodeMaintenanceRow {
            node: "nb".into(),
            online: true,
            maintenance: m,
            ..Default::default()
        }
    }

    fn run(status: &str) -> Option<MoveRun> {
        Some(MoveRun {
            status: status.into(),
            ..Default::default()
        })
    }

    #[test]
    fn badge_follows_the_maintenance_lifecycle() {
        assert_eq!(api::maintenance_badge(&row(None)), None);
        let mut m = MaintenanceState::default();
        assert_eq!(api::maintenance_badge(&row(Some(m.clone()))), None);
        m.enabled = true;
        m.drain = run("running");
        assert_eq!(api::maintenance_badge(&row(Some(m.clone()))), Some("entering maintenance"));
        m.drain = run("done");
        assert_eq!(api::maintenance_badge(&row(Some(m.clone()))), Some("maintenance"));
        m.enabled = false;
        m.exit = run("running");
        assert_eq!(api::maintenance_badge(&row(Some(m.clone()))), Some("leaving maintenance"));
        m.exit = run("done");
        assert_eq!(api::maintenance_badge(&row(Some(m))), None);
    }

    #[test]
    fn request_body_carries_policy_only_when_chosen() {
        let b = api::node_maintenance_body(true, true, Some(""), false);
        assert_eq!(b, serde_json::json!({"enabled": true, "drain": true, "force": false}));
        let b = api::node_maintenance_body(false, false, Some("none"), false);
        assert_eq!(b["return"], "none");
        let b = api::node_maintenance_body(true, true, None, true);
        assert_eq!(b["force"], true);
    }

    #[test]
    fn maintenance_state_reads_old_and_new_servers() {
        let old: MaintenanceState =
            serde_json::from_str(r#"{"enabled":false,"since":null,"reason":null,"drain":null,"clustered":true}"#)
                .unwrap();
        assert!(old.origins.is_empty() && old.exit.is_none());
        let new: MaintenanceState = serde_json::from_str(
            r#"{"enabled":true,"since":"t","reason":"maintenance","drain":null,"clustered":true,
                "node":"nb","origins":[{"guest_id":"g","name":"g","origin":"nb","moved_to":"na","at":"t"}],
                "return_policy":"none","effective_return_policy":"none","exit":null}"#,
        )
        .unwrap();
        assert_eq!(new.origins[0].moved_to, "na");
        assert_eq!(new.effective_return_policy, "none");
    }
}
