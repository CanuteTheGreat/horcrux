//! Storage > Ceph: install Ceph with the release of your choice in a short
//! wizard (release, cluster, review, live install log), and upgrade to a newer
//! release in Ceph's order with each step's task output shown in place.
use crate::api::platform as p;
use crate::components::{Choice, ChoiceCards, StepHeader, TaskLogPanel};
use leptos::*;

const STEPS: [&str; 4] = ["Release", "Cluster", "Review", "Install"];

fn mode_label(m: &str) -> &'static str {
    match m {
        "new" => "Create a new cluster",
        "join" => "Join an existing cluster",
        _ => "Install the packages only",
    }
}

#[component]
pub fn CephInstallCard() -> impl IntoView {
    let (rel, set_rel) = create_signal(None::<p::CephReleases>);
    let (up, set_up) = create_signal(None::<p::CephUpgradeStatus>);
    let (unavail, set_unavail) = create_signal(None::<crate::api::Unavailable>);
    let (msg, set_msg) = create_signal(None::<(bool, String)>);
    let (task, set_task) = create_signal(None::<(String, String)>);
    let step = create_rw_signal(0usize);
    let choice = create_rw_signal(String::new());
    let mode = create_rw_signal("new".to_string());
    let public_net = create_rw_signal(String::new());
    let cluster_net = create_rw_signal(String::new());
    let mon_ip = create_rw_signal(String::new());
    let peer = create_rw_signal(String::new());
    let peer_user = create_rw_signal("root".to_string());
    let peer_pw = create_rw_signal(String::new());
    let up_target = create_rw_signal(String::new());
    // drop answers that a newer load superseded
    let gen = store_value(0u64);

    let load = move || {
        gen.update_value(|g| *g += 1);
        let mine = gen.get_value();
        spawn_local(async move {
            let r = p::ceph_releases().await;
            if gen.get_value() != mine {
                return;
            }
            match r {
                Ok(r) => {
                    if choice.get_untracked().is_empty() {
                        if let Some(m) = r.installed_major.or(r.recommended) {
                            choice.set(m.to_string());
                        }
                    }
                    let installed = r.installed_major.is_some();
                    set_rel.set(Some(r));
                    if installed {
                        if let Ok(u) = p::ceph_upgrade_status().await {
                            if gen.get_value() != mine {
                                return;
                            }
                            if up_target.get_untracked().is_empty() {
                                if let Some(t) = u.targets.first() {
                                    up_target.set(t.to_string());
                                }
                            }
                            set_up.set(Some(u));
                        }
                    }
                }
                Err(e) => set_msg.set(Some((false, e.message))),
            }
        });
    };
    create_effect(move |_| load());

    let started = move |r: Result<p::TaskStarted, crate::api::ApiError>, what: String| match r {
        Ok(t) => {
            set_unavail.set(None);
            set_msg.set(None);
            set_task.set(Some((t.upid, what)));
        }
        Err(e) => match e.unavailable() {
            Some(u) => set_unavail.set(Some(u)),
            None => set_msg.set(Some((false, e.message))),
        },
    };
    // a re-rendered log panel reports the same finished task again: act once
    let handled = store_value(String::new());
    let task_done = Callback::new(move |st: String| {
        let upid = task.get_untracked().map(|(u, _)| u).unwrap_or_default();
        if handled.get_value() == upid {
            return;
        }
        handled.set_value(upid);
        if st == "ok" {
            set_msg.set(Some((true, "Done.".into())));
        } else {
            set_msg.set(Some((
                false,
                "The task did not finish: read its log above, fix the cause and run it again."
                    .into(),
            )));
        }
        load();
    });

    let label_of = move |m: u64| {
        rel.get_untracked()
            .and_then(|r| {
                r.releases
                    .iter()
                    .find(|x| x.major == m)
                    .map(|x| p::release_label(x.major, &x.name))
            })
            .unwrap_or_else(|| format!("Ceph {m}"))
    };
    // what the Cluster step still lacks, if anything
    let cluster_missing = move || -> Option<&'static str> {
        match mode.get().as_str() {
            "new" if public_net.get().trim().is_empty() => Some("Enter the public network."),
            "join" if peer.get().trim().is_empty() => Some("Enter a cluster member."),
            "join" if peer_pw.get().is_empty() => Some("Enter the administrator's password."),
            _ => None,
        }
    };

    let install = move |_| {
        let Ok(major) = choice.get().parse::<u64>() else {
            return;
        };
        let opt = |s: String| (!s.trim().is_empty()).then(|| s.trim().to_string());
        let b = match mode.get().as_str() {
            "new" => serde_json::json!({"mode": "new", "public_network": public_net.get().trim(),
                "cluster_network": opt(cluster_net.get()), "mon_ip": opt(mon_ip.get())}),
            "join" => serde_json::json!({"mode": "join", "peer": peer.get().trim(),
                "username": peer_user.get().trim(), "password": peer_pw.get(), "mon_ip": opt(mon_ip.get())}),
            _ => serde_json::json!({"mode": "none"}),
        };
        let what = format!("Install {}", label_of(major));
        spawn_local(async move {
            let r = p::ceph_install(major, b).await;
            peer_pw.set(String::new());
            let ok = r.is_ok();
            started(r, what);
            if ok {
                step.set(3);
            }
        });
    };
    let upgrade = move |steps: Vec<String>| {
        let Ok(major) = up_target.get().parse::<u64>() else {
            return;
        };
        spawn_local(async move {
            let what = if steps.is_empty() {
                format!("Upgrade to {}", label_of(major))
            } else {
                format!("Upgrade step: {}", steps.join(", "))
            };
            started(p::ceph_upgrade(major, steps).await, what);
        });
    };

    let fields_new = move || {
        view! {
            <div class="form-group"><label for="ceph-public-net">"Public network (CIDR)"</label>
                <input id="ceph-public-net" type="text" required placeholder="10.0.0.0/24" prop:value=public_net on:input=move |ev| public_net.set(event_target_value(&ev))/>
                <p class="form-help">"Clients and monitors talk on this network."</p></div>
            <div class="form-group"><label for="ceph-cluster-net">"Cluster network (optional)"</label>
                <input id="ceph-cluster-net" type="text" placeholder="10.1.0.0/24" prop:value=cluster_net on:input=move |ev| cluster_net.set(event_target_value(&ev))/>
                <p class="form-help">"A separate network for OSD replication; empty uses the public network."</p></div>
        }
    };
    let fields_join = move || {
        view! {
            <div class="form-group"><label for="ceph-peer">"A cluster member (host or host:port)"</label>
                <input id="ceph-peer" type="text" required placeholder="node1.example.net" prop:value=peer on:input=move |ev| peer.set(event_target_value(&ev))/></div>
            <div class="form-group"><label for="ceph-peer-user">"Administrator there"</label>
                <input id="ceph-peer-user" type="text" required prop:value=peer_user on:input=move |ev| peer_user.set(event_target_value(&ev))/></div>
            <div class="form-group"><label for="ceph-peer-pw">"Password"</label>
                <input id="ceph-peer-pw" type="password" required autocomplete="off" prop:value=peer_pw on:input=move |ev| peer_pw.set(event_target_value(&ev))/>
                <p class="form-help">"Used once to fetch the cluster's join material; it is not stored."</p></div>
        }
    };

    let wizard = move |r: p::CephReleases| {
        let choices: Vec<Choice> = r
            .releases
            .iter()
            .map(|x| {
                let mut tags = vec![];
                if r.recommended == Some(x.major) {
                    tags.push(("hx-badge hx-badge-ok", "Recommended".to_string()));
                }
                if x.keyword == "testing" {
                    tags.push(("hx-badge hx-badge-warn", "Testing".to_string()));
                }
                Choice {
                    value: x.major.to_string(),
                    title: p::release_label(x.major, &x.name),
                    detail: format!("Version {} from {}", x.newest, x.repo),
                    tags,
                }
            })
            .collect();
        let modes = vec![
            Choice {
                value: "new".into(),
                title: mode_label("new").into(),
                detail: "The first monitor and manager run on this node.".into(),
                tags: vec![],
            },
            Choice {
                value: "join".into(),
                title: mode_label("join").into(),
                detail: "This node joins a cluster that another Horcrux node runs.".into(),
                tags: vec![],
            },
            Choice {
                value: "none".into(),
                title: mode_label("none").into(),
                detail: "Install the release; create or join a cluster later.".into(),
                tags: vec![],
            },
        ];
        view! {
            <div class="hx-wizard" id="ceph-install-form">
                <StepHeader steps=STEPS.to_vec() current=step on_select=Callback::new(move |i: usize| { if step.get_untracked() < 3 { step.set(i) } })/>
                <div class="hx-wizard-body" data-step=move || step.get().to_string()>
                    <Show when=move || step.get() == 0>
                        <p class="help-text">"Ceph is not installed on this node. Pick the release; updates then stay on it until you upgrade."</p>
                        <ChoiceCards name="ceph-release" choices=choices.clone() selected=choice/>
                    </Show>
                    <Show when=move || step.get() == 1>
                        <ChoiceCards name="ceph-mode" choices=modes.clone() selected=mode/>
                        <div class="fields">
                            <Show when=move || mode.get() == "new">{fields_new()}</Show>
                            <Show when=move || mode.get() == "join">{fields_join()}</Show>
                            <Show when=move || mode.get() != "none">
                                <div class="form-group"><label for="ceph-mon-ip">"This node's monitor address (optional)"</label>
                                    <input id="ceph-mon-ip" type="text" placeholder="its address in the public network" prop:value=mon_ip on:input=move |ev| mon_ip.set(event_target_value(&ev))/></div>
                            </Show>
                        </div>
                    </Show>
                    <Show when=move || step.get() == 2>
                        <table class="hx-summary" id="ceph-install-summary">
                            <tbody>
                                <tr><th>"Release"</th><td>{move || choice.get().parse::<u64>().map(label_of).unwrap_or_default()}</td></tr>
                                <tr><th>"Then"</th><td>{move || mode_label(&mode.get())}</td></tr>
                                <Show when=move || mode.get() == "new">
                                    <tr><th>"Public network"</th><td>{move || public_net.get()}</td></tr>
                                    <tr><th>"Cluster network"</th><td>{move || { let c = cluster_net.get(); if c.trim().is_empty() { "same as public".to_string() } else { c } }}</td></tr>
                                </Show>
                                <Show when=move || mode.get() == "join">
                                    <tr><th>"Cluster member"</th><td>{move || format!("{} as {}", peer.get(), peer_user.get())}</td></tr>
                                </Show>
                                <Show when=move || mode.get() != "none" && !mon_ip.get().trim().is_empty()>
                                    <tr><th>"Monitor address"</th><td>{move || mon_ip.get()}</td></tr>
                                </Show>
                            </tbody>
                        </table>
                        <p class="help-text">"The install runs as a task; its output appears here and on the Tasks page."</p>
                    </Show>
                    <Show when=move || step.get() == 3>
                        {move || task.get().map(|(u, t)| view! { <TaskLogPanel upid=u title=t on_done=task_done/> })}
                    </Show>
                </div>
                <div class="form-actions hx-wizard-actions">
                    <Show when=move || (1..3).contains(&step.get())>
                        <button type="button" class="btn btn-secondary" id="ceph-install-back" on:click=move |_| step.update(|s| *s -= 1)>"Back"</button>
                    </Show>
                    <Show when=move || step.get() == 1>
                        {move || cluster_missing().map(|m| view! { <span class="form-help" id="ceph-install-missing">{m}</span> })}
                    </Show>
                    <Show when=move || step.get() < 2>
                        <button type="button" class="btn btn-primary" id="ceph-install-next"
                            disabled=move || (step.get() == 0 && choice.get().is_empty()) || (step.get() == 1 && cluster_missing().is_some())
                            on:click=move |_| step.update(|s| *s += 1)>"Next"</button>
                    </Show>
                    <Show when=move || step.get() == 2>
                        <button id="ceph-install-submit" type="button" class="btn btn-primary" on:click=install>"Install Ceph"</button>
                    </Show>
                </div>
            </div>
        }
    };

    view! {
        <div class="card" id="ceph-install">
            <h2>"Ceph release"</h2>
            {move || msg.get().map(|(ok, m)| view! {
                <div class=if ok { "alert alert-info" } else { "alert alert-error" } id="ceph-install-msg">{m}</div>
            })}
            {move || unavail.get().map(|u| view! {
                <crate::components::UnavailableState info=u on_retry=Callback::new(move |_| load())/>
            })}
            {move || rel.get().map(|r| {
                if !r.package_manager && r.installed.is_none() {
                    return view! {
                        <div class="empty-state" id="ceph-no-portage">
                            <p class="empty-title">"Ceph is installed through Portage, which this node does not have."</p>
                            <p class="empty-todo">"Install Ceph with the distribution's own packages; Horcrux then manages the cluster on this page."</p>
                        </div>
                    }.into_view();
                }
                if r.releases.is_empty() && r.installed.is_none() {
                    return view! {
                        <div class="empty-state" id="ceph-no-release">
                            <p class="empty-title">{format!("The package repositories of this node carry no Ceph release for {}.", r.arch)}</p>
                            <p class="empty-todo">"Sync the repositories, then check again."</p>
                            <a class="btn btn-secondary" href="/system/packages">"Open Packages"</a>
                        </div>
                    }.into_view();
                }
                // an install that just started keeps the wizard (and its log) on screen
                let install_running = step.get_untracked() == 3
                    && task.get_untracked().is_some_and(|(u, _)| handled.get_value() != u);
                match r.installed.clone().filter(|_| !install_running) {
                    None => wizard(r).into_view(),
                    Some(v) => {
                        let label = r.installed_major.map(label_of).unwrap_or_default();
                        view! {
                            <table class="hx-summary" id="ceph-installed">
                                <tbody>
                                    <tr><th>"Installed"</th><td>{format!("{label}, version {v}")}</td></tr>
                                    <tr><th>"Updates"</th><td>{r.pinned.map(|m| format!("stay on release {m}")).unwrap_or_else(|| "follow the repositories".into())}</td></tr>
                                </tbody>
                            </table>
                            {move || task.get().map(|(u, t)| view! { <TaskLogPanel upid=u title=t on_done=task_done/> })}
                            {move || up.get().map(|u| {
                                if u.targets.is_empty() {
                                    return view! { <p class="help-text" id="ceph-upgrade-none">"No newer release is available in this node's repositories."</p> }.into_view();
                                }
                                let next = u.next_step.clone().unwrap_or_else(|| "packages".into());
                                let next2 = next.clone();
                                let rows = u.daemons.as_ref().map(|d| d.by_type.clone()).unwrap_or_default();
                                let targets: Vec<Choice> = u.targets.iter().map(|t| Choice {
                                    value: t.to_string(), title: label_of(*t), detail: String::new(), tags: vec![],
                                }).collect();
                                view! {
                                    <div id="ceph-upgrade">
                                        <h3>"Upgrade"</h3>
                                        <p class="help-text">"Ceph upgrades in a fixed order: the packages on every node, then the monitors on every node, then managers, OSDs, MDS and RGW, and a final step once every daemon runs the new release. Run each step on every node before the next one. The noout flag is set for the upgrade and cleared at the end."</p>
                                        <table class="data-table" id="ceph-daemon-versions">
                                            <thead><tr><th>"Daemon"</th><th>"Release"</th><th>"Count"</th></tr></thead>
                                            <tbody>
                                                {rows.into_iter().flat_map(|(k, l)| l.into_iter().map(move |(m, n)| (k.clone(), m, n))).map(|(k, m, n)| view! {
                                                    <tr><td>{k}</td><td>{label_of(m)}</td><td>{n.to_string()}</td></tr>
                                                }).collect_view()}
                                            </tbody>
                                        </table>
                                        <p class="form-help">"To release"</p>
                                        <ChoiceCards name="ceph-upgrade-target" choices=targets selected=up_target/>
                                        <div class="form-actions">
                                            <button id="ceph-upgrade-next" class="btn btn-primary" type="button" on:click=move |_| upgrade(vec![next2.clone()])>{format!("Run next step: {next}")}</button>
                                            <button id="ceph-upgrade-all" class="btn btn-secondary" type="button" on:click=move |_| upgrade(vec![])>"All steps (single-node cluster)"</button>
                                        </div>
                                    </div>
                                }.into_view()
                            })}
                        }.into_view()
                    }
                }
            })}
        </div>
    }
}
