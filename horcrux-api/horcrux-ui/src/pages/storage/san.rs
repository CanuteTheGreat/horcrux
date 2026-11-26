//! Storage > SAN: the cluster's SAN targets (iSCSI, NVMe over Fabrics),
//! shared LVM storages on SAN LUNs, Fibre Channel HBAs, and the per-node
//! path view (every LUN with each path's address and state). The "Add SAN
//! storage" wizard walks portal/NQN -> discovered paths -> LUN -> LVM
//! storage -> review.
use crate::api::san as s;
use crate::api::{get_cluster_nodes, ApiError, Unavailable};
use crate::components::UnavailableState;
use leptos::*;
use leptos_router::*;

fn state_class(st: &str) -> &'static str {
    match st {
        "active" => "badge badge-success",
        "standby" => "badge badge-info",
        "failed" => "badge badge-danger",
        _ => "badge badge-secondary",
    }
}

/// `Storage > SAN` (`/storage/san?node=<name>`).
#[component]
pub fn SanPage() -> impl IntoView {
    let query = use_query_map();
    let node = move || {
        query
            .with(|q| q.get("node").cloned())
            .filter(|n| !n.is_empty())
    };
    let (nodes, set_nodes) = create_signal(Vec::<String>::new());
    let (paths, set_paths) = create_signal(None::<s::NodePaths>);
    let (status, set_status) = create_signal(None::<s::SanStatus>);
    let (targets, set_targets) = create_signal(Vec::<s::IscsiTarget>::new());
    let (nvme, set_nvme) = create_signal(Vec::<s::NvmeTarget>::new());
    let (lvms, set_lvms) = create_signal(Vec::<s::SharedLvm>::new());
    let (fc, set_fc) = create_signal(s::FcView::default());
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let (unavail, set_unavail) = create_signal(None::<Unavailable>);
    let seq = store_value(0u64);

    let reload = move || {
        seq.update_value(|n| *n += 1);
        let mine = seq.get_value();
        let n = node();
        spawn_local(async move {
            let p = s::get_paths(n.as_deref()).await;
            if seq.get_value() != mine {
                return;
            }
            match p {
                Ok(p) => {
                    set_paths.set(Some(p));
                    set_error.set(None);
                }
                Err(e) if !crate::api::is_transport_error(&e.message) => {
                    set_error.set(Some(e.message))
                }
                Err(_) => {}
            }
            if let Ok(x) = s::get_status().await {
                set_status.set(Some(x));
            }
            if let Ok(x) = s::get_targets().await {
                set_targets.set(x);
            }
            if let Ok(x) = s::get_nvme_targets().await {
                set_nvme.set(x);
            }
            if let Ok(x) = s::get_lvm().await {
                set_lvms.set(x);
            }
            if let Ok(x) = s::get_fc().await {
                set_fc.set(x);
            }
            if let Ok(c) = get_cluster_nodes().await {
                set_nodes.set(c.into_iter().map(|c| c.name).collect());
            }
        });
    };
    create_effect(move |_| {
        let _ = node();
        reload();
    });
    let handle = set_interval_with_handle(reload, std::time::Duration::from_secs(5)).ok();
    on_cleanup(move || {
        if let Some(h) = handle {
            h.clear();
        }
    });

    let act = move |what: &'static str,
                    f: std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), ApiError>>>,
    >| {
        spawn_local(async move {
            match f.await {
                Ok(()) => {
                    set_error.set(None);
                    set_unavail.set(None);
                    set_notice.set(Some(what.to_string()));
                }
                Err(e) => match e.unavailable() {
                    Some(u) => set_unavail.set(Some(u)),
                    None => set_error.set(Some(e.message)),
                },
            }
            reload();
        });
    };
    let apply_now = move |_| {
        act(
            "Applied the SAN configuration on this node",
            Box::pin(async { s::apply().await.map(|_| ()) }),
        )
    };
    let rescan = move |_| {
        act(
            "Rescanned the Fibre Channel HBAs",
            Box::pin(async { s::fc_rescan().await.map(|_| ()) }),
        )
    };

    let nav = use_navigate();
    let node_label =
        move || node().unwrap_or_else(|| status.get().map(|s| s.node).unwrap_or_default());

    view! {
        <div class="page san-page">
            <div class="page-header">
                <div class="page-header-text">
                    <h1 class="page-title">"SAN"</h1>
                    <p class="page-description">"iSCSI, NVMe over Fabrics and Fibre Channel LUNs with every path, shared LVM on them. Targets and storages apply to every cluster node."</p>
                </div>
                <div class="page-actions">
                    <a class="btn btn-primary" href="/storage/san/new">"Add SAN storage"</a>
                    <button class="btn btn-secondary" type="button" on:click=apply_now>"Apply now"</button>
                </div>
            </div>
            {move || notice.get().map(|m| view! { <div class="alert alert-success" role="status">{m}</div> })}
            {move || error.get().map(|m| view! { <div class="alert alert-error" role="alert">{m}</div> })}
            {move || unavail.get().map(|u| view! { <UnavailableState info=u on_retry=move |_| { set_unavail.set(None); reload(); }/> })}

            <section class="card san-node">
                <div class="card-header">
                    <h2>"Paths on " {node_label}</h2>
                    <label class="inline-field">
                        <span>"Node"</span>
                        <select aria-label="Node" on:change=move |ev| {
                            let v = event_target_value(&ev);
                            nav(&if v.is_empty() { "/storage/san".to_string() } else { format!("/storage/san?node={v}") }, Default::default());
                        }>
                            <option value="" selected={move || node().is_none()}>"This node"</option>
                            {move || nodes.get().into_iter().map(|n| {
                                let sel = node().as_deref() == Some(n.as_str());
                                view! { <option value=n.clone() selected=sel>{n.clone()}</option> }
                            }).collect_view()}
                        </select>
                    </label>
                </div>
                {move || match paths.get() {
                    None => view! { <p class="muted">"Loading paths..."</p> }.into_view(),
                    Some(p) if p.luns.is_empty() => view! {
                        <div class="empty-state">
                            <h3 class="empty-title">"No SAN LUNs on this node"</h3>
                            <p class="empty-todo">"Add an iSCSI or NVMe over Fabrics target (or zone an FC LUN to this node's HBA); its LUNs show up here with every path."</p>
                        </div>
                    }.into_view(),
                    Some(p) => view! {
                        <div class="san-meta">
                            <span>{format!("multipathd {}", if p.multipathd_running { "running" } else { "not running" })}</span>
                            {p.native_nvme_multipath.map(|on| view! { <span>{format!("native NVMe multipath {}", if on { "on" } else { "off" })}</span> })}
                        </div>
                        {p.errors.iter().map(|e| view! { <div class="alert alert-warning">{e.clone()}</div> }).collect_view()}
                        <div class="table-wrap">
                        <table class="table san-luns">
                            <thead><tr>
                                <th>"LUN"</th><th>"Target"</th><th>"Device"</th><th>"Size"</th>
                                <th>"Paths"</th><th>"Path detail"</th>
                            </tr></thead>
                            <tbody>
                                {p.luns.into_iter().map(|l| {
                                    let failed = l.failed_paths;
                                    view! {
                                        <tr data-wwid=l.wwid.clone()>
                                            <td><code>{l.wwid.clone()}</code>
                                                {l.lun.map(|n| view! { <div class="muted">{format!("LUN {n}")}</div> })}
                                                {l.vendor.clone().map(|v| view! { <div class="muted">{format!("{v} {}", l.product.clone().unwrap_or_default())}</div> })}
                                            </td>
                                            <td>{l.target.clone().unwrap_or_default()}</td>
                                            <td><code>{l.device.clone()}</code></td>
                                            <td>{l.size_bytes.map(s::human_bytes).unwrap_or_default()}</td>
                                            <td>
                                                <span class="badge badge-success">{format!("{} active", l.active_paths)}</span>
                                                " "
                                                <span class={if failed > 0 { "badge badge-danger" } else { "badge badge-secondary" }}>{format!("{failed} failed")}</span>
                                            </td>
                                            <td>
                                                <ul class="path-list">
                                                    {l.paths.into_iter().map(|x| view! {
                                                        <li>
                                                            <span class={state_class(&x.state)}>{x.state.clone()}</span>
                                                            " " <code>{x.dev.clone()}</code>
                                                            " " {x.transport.clone()}
                                                            " " {x.portal.clone().unwrap_or_default()}
                                                            {x.local_address.clone().map(|a| view! { <span class="muted">{format!(" via {a}")}</span> })}
                                                        </li>
                                                    }).collect_view()}
                                                </ul>
                                            </td>
                                        </tr>
                                    }
                                }).collect_view()}
                            </tbody>
                        </table>
                        </div>
                    }.into_view(),
                }}
            </section>

            <section class="card">
                <div class="card-header"><h2>"Targets"</h2></div>
                {move || {
                    let t = targets.get();
                    let n = nvme.get();
                    if t.is_empty() && n.is_empty() {
                        view! {
                            <div class="empty-state">
                                <h3 class="empty-title">"No SAN targets yet"</h3>
                                <p class="empty-todo">"Add one with the wizard: give it a portal or discovery address, it finds every path and logs in on every node."</p>
                                <div class="empty-actions"><a class="btn btn-primary" href="/storage/san/new">"Add SAN storage"</a></div>
                            </div>
                        }.into_view()
                    } else {
                        view! {
                            <div class="table-wrap">
                            <table class="table">
                                <thead><tr><th>"ID"</th><th>"Type"</th><th>"Name"</th><th>"Portals / addresses"</th><th>"Nodes"</th><th>"Actions"</th></tr></thead>
                                <tbody>
                                    {t.into_iter().map(|x| {
                                        let id = x.id.clone();
                                        view! { <tr>
                                            <td>{x.id.clone()}</td><td>"iSCSI"</td><td><code>{x.iqn.clone()}</code></td>
                                            <td>{x.portals.join(", ")}{x.use_advertised_portals.then_some(" + advertised")}</td>
                                            <td>{if x.nodes.is_empty() { "all".to_string() } else { x.nodes.join(", ") }}</td>
                                            <td><button class="btn btn-sm btn-danger" type="button" on:click=move |_| {
                                                let id = id.clone();
                                                act("Deleted the target; every node logs out", Box::pin(async move { s::delete_target(&id).await }))
                                            }>"Delete"</button></td>
                                        </tr> }
                                    }).collect_view()}
                                    {n.into_iter().map(|x| {
                                        let id = x.id.clone();
                                        view! { <tr>
                                            <td>{x.id.clone()}</td><td>{format!("NVMe/{}", x.transport.to_uppercase())}</td><td><code>{x.subnqn.clone()}</code></td>
                                            <td>{x.addresses.join(", ")}</td>
                                            <td>{if x.nodes.is_empty() { "all".to_string() } else { x.nodes.join(", ") }}</td>
                                            <td><button class="btn btn-sm btn-danger" type="button" on:click=move |_| {
                                                let id = id.clone();
                                                act("Deleted the subsystem; every node disconnects", Box::pin(async move { s::delete_nvme_target(&id).await }))
                                            }>"Delete"</button></td>
                                        </tr> }
                                    }).collect_view()}
                                </tbody>
                            </table>
                            </div>
                        }.into_view()
                    }
                }}
            </section>

            <section class="card">
                <div class="card-header"><h2>"Shared LVM storages"</h2></div>
                {move || {
                    let l = lvms.get();
                    if l.is_empty() {
                        view! {
                            <div class="empty-state">
                                <h3 class="empty-title">"No shared LVM storage yet"</h3>
                                <p class="empty-todo">"The wizard's last steps put a shared volume group on a LUN and add it as storage on every node."</p>
                            </div>
                        }.into_view()
                    } else {
                        view! {
                            <div class="table-wrap">
                            <table class="table">
                                <thead><tr><th>"Storage"</th><th>"Volume group"</th><th>"LUN"</th><th>"Snapshots"</th><th>"Removed volumes"</th><th>"Actions"</th></tr></thead>
                                <tbody>
                                    {l.into_iter().map(|x| {
                                        let id = x.id.clone();
                                        let removed = match (x.wipe_removed, x.discard_removed) {
                                            (true, true) => "zeroed + discarded",
                                            (true, false) => "zeroed",
                                            (false, true) => "discarded",
                                            _ => "removed only",
                                        };
                                        view! { <tr>
                                            <td><a href="/storage">{x.id.clone()}</a></td>
                                            <td><code>{x.vg_name.clone()}</code></td>
                                            <td><code>{x.device.clone()}</code></td>
                                            <td>{if x.snapshot_as_volume_chain { "volume chain (qcow2)" } else { "none" }}</td>
                                            <td>{removed}</td>
                                            <td><button class="btn btn-sm btn-danger" type="button" on:click=move |_| {
                                                let id = id.clone();
                                                act("Removed the storage (the volume group stays on the LUN)", Box::pin(async move { s::delete_lvm(&id).await }))
                                            }>"Remove"</button></td>
                                        </tr> }
                                    }).collect_view()}
                                </tbody>
                            </table>
                            </div>
                        }.into_view()
                    }
                }}
            </section>

            <section class="card">
                <div class="card-header">
                    <h2>"Fibre Channel"</h2>
                    <button class="btn btn-secondary" type="button" disabled={move || fc.get().hosts.is_empty()} on:click=rescan>"Rescan HBAs"</button>
                </div>
                {move || {
                    let f = fc.get();
                    if f.hosts.is_empty() {
                        view! {
                            <div class="empty-state">
                                <h3 class="empty-title">"No Fibre Channel HBA on this node"</h3>
                                <p class="empty-todo">"This node has no Fibre Channel HBA. FC LUNs need no setup here: once the fabric zones a LUN to an HBA it appears in the path view with every path."</p>
                            </div>
                        }.into_view()
                    } else {
                        view! {
                            <div class="table-wrap">
                            <table class="table">
                                <thead><tr><th>"HBA"</th><th>"WWPN"</th><th>"State"</th><th>"Speed"</th><th>"Remote target ports"</th></tr></thead>
                                <tbody>
                                    {f.hosts.iter().map(|h| {
                                        let rports: Vec<s::FcRemotePort> = f.remote_ports.iter().filter(|r| r.host == h.host).cloned().collect();
                                        let st = h.port_state.clone().unwrap_or_default();
                                        view! { <tr>
                                            <td>{h.name.clone()}</td>
                                            <td><code>{h.wwpn.clone().unwrap_or_default()}</code></td>
                                            <td><span class={if st == "Online" { "badge badge-success" } else { "badge badge-danger" }}>{st.clone()}</span></td>
                                            <td>{h.speed.clone().unwrap_or_default()}</td>
                                            <td><ul class="path-list">{rports.into_iter().map(|r| view! {
                                                <li><code>{r.wwpn.clone().unwrap_or_default()}</code>" "{r.roles.clone().unwrap_or_default()}" "{r.port_state.clone().unwrap_or_default()}{format!(" ({} LUNs)", r.luns.len())}</li>
                                            }).collect_view()}</ul></td>
                                        </tr> }
                                    }).collect_view()}
                                </tbody>
                            </table>
                            </div>
                        }.into_view()
                    }
                }}
            </section>
        </div>
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Iscsi,
    Nvme,
}

/// `Storage > SAN > Add SAN storage` (`/storage/san/new`).
#[component]
pub fn SanWizard() -> impl IntoView {
    let step = create_rw_signal(1u8);
    let kind = create_rw_signal(Kind::Iscsi);
    let addrs = create_rw_signal(String::new());
    let chap_user = create_rw_signal(String::new());
    let chap_secret = create_rw_signal(String::new());
    let found_iscsi = create_rw_signal(Vec::<s::DiscoveredTarget>::new());
    let found_nvme = create_rw_signal(Vec::<s::DiscoveredSubsystem>::new());
    let picked = create_rw_signal(String::new());
    let target_id = create_rw_signal(String::new());
    let luns = create_rw_signal(Vec::<s::LunView>::new());
    let lun = create_rw_signal(String::new());
    let storage_id = create_rw_signal(String::new());
    let vg_name = create_rw_signal(String::new());
    let chain = create_rw_signal(true);
    let discard = create_rw_signal(true);
    let wipe = create_rw_signal(false);
    let create_vg = create_rw_signal(true);
    let busy = create_rw_signal(false);
    let error = create_rw_signal(None::<String>);
    let unavail = create_rw_signal(None::<Unavailable>);
    let done = create_rw_signal(None::<s::ApplyReport>);

    let list = move || {
        addrs
            .get()
            .split([',', ' ', '\n'])
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    let fail = move |e: ApiError| {
        busy.set(false);
        match e.unavailable() {
            Some(u) => unavail.set(Some(u)),
            None => error.set(Some(e.message)),
        }
    };

    let discover = move |_| {
        error.set(None);
        unavail.set(None);
        let a = list();
        if a.is_empty() {
            error.set(Some(
                "Enter at least one portal or discovery address".into(),
            ));
            return;
        }
        busy.set(true);
        spawn_local(async move {
            match kind.get_untracked() {
                Kind::Iscsi => match s::discover_iscsi(a).await {
                    Ok(r) => {
                        busy.set(false);
                        if let Some(t) = r.targets.first() {
                            picked.set(t.iqn.clone());
                        }
                        found_iscsi.set(r.targets);
                        step.set(2);
                    }
                    Err(e) => fail(e),
                },
                Kind::Nvme => match s::discover_nvme(a).await {
                    Ok(r) => {
                        busy.set(false);
                        if let Some(t) = r.subsystems.first() {
                            picked.set(t.subnqn.clone());
                        }
                        found_nvme.set(r.subsystems);
                        step.set(2);
                    }
                    Err(e) => fail(e),
                },
            }
        });
    };

    // Step 2 -> 3: store the target (every node logs in / connects on every
    // path), then list the LUNs it brought.
    let add_target = move |_| {
        error.set(None);
        let id = target_id.get_untracked().trim().to_string();
        let name = picked.get_untracked();
        if id.is_empty() || name.is_empty() {
            error.set(Some("Pick a target and give it an ID".into()));
            return;
        }
        busy.set(true);
        let a = list();
        spawn_local(async move {
            let r = match kind.get_untracked() {
                Kind::Iscsi => {
                    let u = chap_user.get_untracked();
                    let chap = (!u.trim().is_empty()).then(|| (u, chap_secret.get_untracked()));
                    s::create_iscsi_target(&id, &name, a, chap).await
                }
                Kind::Nvme => s::create_nvme_target(&id, &name, a).await,
            };
            if let Err(e) = r {
                return fail(e);
            }
            let _ = s::apply().await;
            match s::get_paths(None).await {
                Ok(p) => {
                    let mine: Vec<s::LunView> = p
                        .luns
                        .into_iter()
                        .filter(|l| l.target.as_deref() == Some(name.as_str()))
                        .collect();
                    if let Some(l) = mine.first() {
                        lun.set(l.wwid.clone());
                    }
                    luns.set(mine);
                    busy.set(false);
                    step.set(3);
                }
                Err(e) => fail(e),
            }
        });
    };

    let finish = move |_| {
        error.set(None);
        let req = s::CreateLvm {
            id: storage_id.get_untracked().trim().to_string(),
            vg_name: vg_name.get_untracked().trim().to_string(),
            device: lun.get_untracked(),
            wipe_removed: wipe.get_untracked(),
            discard_removed: discard.get_untracked(),
            snapshot_as_volume_chain: chain.get_untracked(),
            create_vg: create_vg.get_untracked(),
        };
        busy.set(true);
        spawn_local(async move {
            match s::create_lvm(&req).await {
                Ok(r) => {
                    busy.set(false);
                    done.set(Some(r));
                    step.set(6);
                }
                Err(e) => fail(e),
            }
        });
    };

    let steps = ["Target", "Paths", "LUN", "Storage", "Review"];
    view! {
        <div class="page san-wizard">
            <div class="page-header">
                <div class="page-header-text">
                    <h1 class="page-title">"Add SAN storage"</h1>
                    <p class="page-description">"Every cluster node logs in on every path the array advertises; the LUN gets one multipath device and a shared volume group."</p>
                </div>
            </div>
            <ol class="wizard-steps" aria-label="Steps">
                {steps.iter().enumerate().map(|(i, name)| {
                    let n = (i + 1) as u8;
                    view! { <li class={move || if step.get() == n { "current" } else if step.get() > n { "done" } else { "" }}>{format!("{n}. {name}")}</li> }
                }).collect_view()}
            </ol>
            {move || error.get().map(|m| view! { <div class="alert alert-error" role="alert">{m}</div> })}
            {move || unavail.get().map(|u| view! { <UnavailableState info=u on_retry=move |_| unavail.set(None)/> })}

            <Show when=move || step.get() == 1>
                <section class="card wizard-panel">
                    <fieldset>
                        <legend>"Transport"</legend>
                        <label><input type="radio" name="kind" checked={move || kind.get() == Kind::Iscsi} on:change=move |_| kind.set(Kind::Iscsi)/>" iSCSI"</label>
                        <label><input type="radio" name="kind" checked={move || kind.get() == Kind::Nvme} on:change=move |_| kind.set(Kind::Nvme)/>" NVMe over TCP"</label>
                    </fieldset>
                    <label class="field">
                        <span>{move || if kind.get() == Kind::Iscsi { "Portals (host or host:port, comma separated)" } else { "Discovery controllers (host or host:port, default port 8009)" }}</span>
                        <input type="text" placeholder="10.0.0.10" prop:value=move || addrs.get() on:input=move |ev| addrs.set(event_target_value(&ev))/>
                    </label>
                    <Show when=move || kind.get() == Kind::Iscsi>
                        <label class="field"><span>"CHAP user (optional)"</span>
                            <input type="text" autocomplete="off" prop:value=move || chap_user.get() on:input=move |ev| chap_user.set(event_target_value(&ev))/></label>
                        <label class="field"><span>"CHAP secret (optional)"</span>
                            <input type="password" autocomplete="new-password" prop:value=move || chap_secret.get() on:input=move |ev| chap_secret.set(event_target_value(&ev))/></label>
                    </Show>
                    <div class="wizard-actions">
                        <a class="btn btn-secondary" href="/storage/san">"Cancel"</a>
                        <button class="btn btn-primary" type="button" disabled={move || busy.get()} on:click=discover>{move || if busy.get() { "Discovering..." } else { "Discover" }}</button>
                    </div>
                </section>
            </Show>

            <Show when=move || step.get() == 2>
                <section class="card wizard-panel">
                    <h2>"Discovered paths"</h2>
                    {move || {
                        let rows: Vec<(String, Vec<String>)> = match kind.get() {
                            Kind::Iscsi => found_iscsi.get().into_iter().map(|t| (t.iqn, t.portals)).collect(),
                            Kind::Nvme => found_nvme.get().into_iter().map(|t| (t.subnqn, t.addresses)).collect(),
                        };
                        if rows.is_empty() {
                            view! { <p class="muted">"The array answered but offers no target to this node. Check its ACLs (initiator name / host NQN), then go back and discover again."</p> }.into_view()
                        } else {
                            rows.into_iter().map(|(name, ps)| {
                                let n2 = name.clone();
                                let n3 = name.clone();
                                view! {
                                    <label class="pick-row">
                                        <input type="radio" name="picked" checked={move || picked.get() == n2} on:change=move |_| picked.set(n3.clone())/>
                                        <code>{name.clone()}</code>
                                        <span class="muted">{format!(" {} paths: {}", ps.len(), ps.join(", "))}</span>
                                    </label>
                                }
                            }).collect_view()
                        }
                    }}
                    <label class="field"><span>"Target ID (letters, digits, - and _)"</span>
                        <input type="text" placeholder="array1" prop:value=move || target_id.get() on:input=move |ev| target_id.set(event_target_value(&ev))/></label>
                    <div class="wizard-actions">
                        <button class="btn btn-secondary" type="button" on:click=move |_| step.set(1)>"Back"</button>
                        <button class="btn btn-primary" type="button" disabled={move || busy.get()} on:click=add_target>{move || if busy.get() { "Logging in on every path..." } else { "Add target and log in" }}</button>
                    </div>
                </section>
            </Show>

            <Show when=move || step.get() == 3>
                <section class="card wizard-panel">
                    <h2>"Pick the LUN"</h2>
                    {move || {
                        let l = luns.get();
                        if l.is_empty() {
                            view! { <p class="muted">"No LUN from this target yet. Map a LUN to this node on the array, then press Refresh."</p> }.into_view()
                        } else {
                            l.into_iter().map(|x| {
                                let w2 = x.wwid.clone();
                                let w3 = x.wwid.clone();
                                view! {
                                    <label class="pick-row">
                                        <input type="radio" name="lun" checked={move || lun.get() == w2} on:change=move |_| lun.set(w3.clone())/>
                                        <code>{x.wwid.clone()}</code>
                                        <span class="muted">{format!(" {} - {} active / {} paths", x.size_bytes.map(s::human_bytes).unwrap_or_default(), x.active_paths, x.paths.len())}</span>
                                    </label>
                                }
                            }).collect_view()
                        }
                    }}
                    <div class="wizard-actions">
                        <button class="btn btn-secondary" type="button" on:click=move |_| {
                            let name = picked.get_untracked();
                            spawn_local(async move {
                                let _ = s::apply().await;
                                if let Ok(p) = s::get_paths(None).await {
                                    luns.set(p.luns.into_iter().filter(|l| l.target.as_deref() == Some(name.as_str())).collect());
                                }
                            });
                        }>"Refresh"</button>
                        <button class="btn btn-primary" type="button" disabled={move || lun.get().is_empty()} on:click=move |_| step.set(4)>"Next"</button>
                    </div>
                </section>
            </Show>

            <Show when=move || step.get() == 4>
                <section class="card wizard-panel">
                    <h2>"LVM storage on the LUN"</h2>
                    <label class="field"><span>"Storage ID"</span>
                        <input type="text" placeholder="san-lvm" prop:value=move || storage_id.get() on:input=move |ev| storage_id.set(event_target_value(&ev))/></label>
                    <label class="field"><span>"Volume group name"</span>
                        <input type="text" placeholder="vg_san" prop:value=move || vg_name.get() on:input=move |ev| vg_name.set(event_target_value(&ev))/></label>
                    <label class="check"><input type="checkbox" prop:checked={move || create_vg.get()} on:change=move |ev| create_vg.set(event_target_checked(&ev))/>" Create the volume group on the LUN (off: it exists already)"</label>
                    <label class="check"><input type="checkbox" prop:checked={move || chain.get()} on:change=move |ev| chain.set(event_target_checked(&ev))/>" Snapshots as volume chain (qcow2 volumes on thick LVs)"</label>
                    <label class="check"><input type="checkbox" prop:checked={move || discard.get()} on:change=move |ev| discard.set(event_target_checked(&ev))/>" Discard removed volumes (gives the space back to a thin array)"</label>
                    <label class="check"><input type="checkbox" prop:checked={move || wipe.get()} on:change=move |ev| wipe.set(event_target_checked(&ev))/>" Zero removed volumes"</label>
                    <div class="wizard-actions">
                        <button class="btn btn-secondary" type="button" on:click=move |_| step.set(3)>"Back"</button>
                        <button class="btn btn-primary" type="button" disabled={move || storage_id.get().trim().is_empty() || vg_name.get().trim().is_empty()} on:click=move |_| step.set(5)>"Next"</button>
                    </div>
                </section>
            </Show>

            <Show when=move || step.get() == 5>
                <section class="card wizard-panel">
                    <h2>"Review"</h2>
                    <dl class="review">
                        <dt>"Target"</dt><dd><code>{move || picked.get()}</code>{move || format!(" ({})", target_id.get())}</dd>
                        <dt>"LUN"</dt><dd><code>{move || lun.get()}</code></dd>
                        <dt>"Storage"</dt><dd>{move || format!("{} on volume group {}", storage_id.get(), vg_name.get())}</dd>
                        <dt>"Snapshots"</dt><dd>{move || if chain.get() { "volume chain" } else { "none" }}</dd>
                        <dt>"Removed volumes"</dt><dd>{move || match (wipe.get(), discard.get()) { (true, true) => "zeroed, then discarded", (true, false) => "zeroed", (false, true) => "discarded", _ => "removed only" }}</dd>
                    </dl>
                    <div class="wizard-actions">
                        <button class="btn btn-secondary" type="button" on:click=move |_| step.set(4)>"Back"</button>
                        <button class="btn btn-primary" type="button" disabled={move || busy.get()} on:click=finish>{move || if busy.get() { "Creating..." } else { "Finish" }}</button>
                    </div>
                </section>
            </Show>

            <Show when=move || step.get() == 6>
                <section class="card wizard-panel" data-done="1">
                    <h2>"SAN storage added"</h2>
                    {move || done.get().map(|r| view! {
                        <ul>{r.actions.into_iter().map(|a| view! { <li>{a}</li> }).collect_view()}</ul>
                        {(!r.errors.is_empty()).then(|| view! { <div class="alert alert-warning">{r.errors.join("; ")}</div> })}
                    })}
                    <div class="wizard-actions">
                        <a class="btn btn-secondary" href="/storage/san">"Open the SAN page"</a>
                        <a class="btn btn-primary" href="/storage">"Open Storage"</a>
                    </div>
                </section>
            </Show>
        </div>
    }
}
