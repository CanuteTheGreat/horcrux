//! Storage > Ceph: install Ceph with the release of your choice, create or
//! join a cluster, and upgrade to a newer release in Ceph's order.
use crate::api::platform as p;
use leptos::*;

#[component]
pub fn CephInstallCard() -> impl IntoView {
    let (rel, set_rel) = create_signal(None::<p::CephReleases>);
    let (up, set_up) = create_signal(None::<p::CephUpgradeStatus>);
    let (unavail, set_unavail) = create_signal(None::<crate::api::Unavailable>);
    let (msg, set_msg) = create_signal(None::<(bool, String)>);
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

    let done = move |r: Result<p::TaskStarted, crate::api::ApiError>, what: String| match r {
        Ok(t) => {
            set_unavail.set(None);
            set_msg.set(Some((
                true,
                format!(
                    "{what} started as task {} (follow it on the Tasks page)",
                    t.upid
                ),
            )));
            load();
        }
        Err(e) => match e.unavailable() {
            Some(u) => set_unavail.set(Some(u)),
            None => set_msg.set(Some((false, e.message))),
        },
    };

    let install = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
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
        spawn_local(async move {
            let r = p::ceph_install(major, b).await;
            peer_pw.set(String::new());
            done(r, format!("Ceph {major} install"));
        });
    };
    let upgrade = move |steps: Vec<String>| {
        let Ok(major) = up_target.get().parse::<u64>() else {
            return;
        };
        spawn_local(async move {
            let what = if steps.is_empty() {
                "Upgrade".to_string()
            } else {
                format!("Upgrade step {}", steps.join(", "))
            };
            done(p::ceph_upgrade(major, steps).await, what);
        });
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
                let names: Vec<(u64, String)> = r.releases.iter().map(|x| (x.major, p::release_label(x.major, &x.name))).collect();
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
                match r.installed.clone() {
                    None => view! {
                        <p class="help-text">"Ceph is not installed on this node. Pick the release, then create a new cluster here or join an existing one."</p>
                        <form id="ceph-install-form" class="fields" on:submit=install>
                            <div class="form-group">
                                <label for="ceph-release">"Release"</label>
                                <select id="ceph-release" prop:value=choice on:change=move |ev| choice.set(event_target_value(&ev))>
                                    {r.releases.iter().map(|x| {
                                        let label = format!("{} - {} ({}{})", p::release_label(x.major, &x.name), x.newest,
                                            if x.keyword == "testing" { "testing, " } else { "" }, x.repo);
                                        view! { <option value=x.major.to_string()>{label}</option> }
                                    }).collect_view()}
                                </select>
                            </div>
                            <div class="form-group">
                                <label for="ceph-mode">"Then"</label>
                                <select id="ceph-mode" prop:value=mode on:change=move |ev| mode.set(event_target_value(&ev))>
                                    <option value="new">"Create a new cluster (first monitor and manager here)"</option>
                                    <option value="join">"Join an existing cluster"</option>
                                    <option value="none">"Install the packages only"</option>
                                </select>
                            </div>
                            <Show when=move || mode.get() == "new">
                                <div class="form-group"><label for="ceph-public-net">"Public network (CIDR)"</label>
                                    <input id="ceph-public-net" type="text" required placeholder="10.0.0.0/24" prop:value=public_net on:input=move |ev| public_net.set(event_target_value(&ev))/></div>
                                <div class="form-group"><label for="ceph-cluster-net">"Cluster network (optional)"</label>
                                    <input id="ceph-cluster-net" type="text" placeholder="10.1.0.0/24" prop:value=cluster_net on:input=move |ev| cluster_net.set(event_target_value(&ev))/></div>
                            </Show>
                            <Show when=move || mode.get() == "join">
                                <div class="form-group"><label for="ceph-peer">"A cluster member (host or host:port)"</label>
                                    <input id="ceph-peer" type="text" required placeholder="node1.example.net" prop:value=peer on:input=move |ev| peer.set(event_target_value(&ev))/></div>
                                <div class="form-group"><label for="ceph-peer-user">"Administrator there"</label>
                                    <input id="ceph-peer-user" type="text" required prop:value=peer_user on:input=move |ev| peer_user.set(event_target_value(&ev))/></div>
                                <div class="form-group"><label for="ceph-peer-pw">"Password"</label>
                                    <input id="ceph-peer-pw" type="password" required autocomplete="off" prop:value=peer_pw on:input=move |ev| peer_pw.set(event_target_value(&ev))/></div>
                            </Show>
                            <Show when=move || mode.get() != "none">
                                <div class="form-group"><label for="ceph-mon-ip">"This node's monitor address (optional)"</label>
                                    <input id="ceph-mon-ip" type="text" placeholder="its address in the public network" prop:value=mon_ip on:input=move |ev| mon_ip.set(event_target_value(&ev))/></div>
                            </Show>
                            <button id="ceph-install-submit" type="submit" class="btn btn-primary">"Install Ceph"</button>
                        </form>
                    }.into_view(),
                    Some(v) => {
                        let label = r.installed_major.map(|m| {
                            names.iter().find(|(x, _)| *x == m).map(|(_, l)| l.clone()).unwrap_or_else(|| format!("Ceph {m}"))
                        }).unwrap_or_default();
                        view! {
                            <p id="ceph-installed">{format!("Installed: {label}, version {v}")}
                                {r.pinned.map(|m| format!(" - updates stay on release {m}"))}</p>
                            {move || up.get().map(|u| {
                                if u.targets.is_empty() {
                                    return view! { <p class="help-text" id="ceph-upgrade-none">"No newer release is available in this node's repositories."</p> }.into_view();
                                }
                                let next = u.next_step.clone().unwrap_or_else(|| "packages".into());
                                let daemons = u.daemons.as_ref().map(|d| d.by_type.iter().map(|(k, l)| {
                                    format!("{k}: {}", l.iter().map(|(m, n)| format!("{n} on {m}")).collect::<Vec<_>>().join(", "))
                                }).collect::<Vec<_>>().join(" | ")).unwrap_or_default();
                                let next2 = next.clone();
                                view! {
                                    <div id="ceph-upgrade">
                                        <h3>"Upgrade"</h3>
                                        <p class="help-text">"Ceph upgrades in a fixed order: the packages on every node, then the monitors on every node, then managers, OSDs, MDS and RGW, and a final step once every daemon runs the new release. Run each step on every node before the next one. The noout flag is set for the upgrade and cleared at the end."</p>
                                        <p id="ceph-daemon-versions">{daemons}</p>
                                        <div class="form-row fields">
                                            <div class="form-group"><label for="ceph-upgrade-target">"To release"</label>
                                                <select id="ceph-upgrade-target" prop:value=up_target on:change=move |ev| up_target.set(event_target_value(&ev))>
                                                    {u.targets.iter().map(|t| {
                                                        let l = names.iter().find(|(x, _)| x == t).map(|(_, l)| l.clone()).unwrap_or_else(|| format!("Ceph {t}"));
                                                        view! { <option value=t.to_string()>{l}</option> }
                                                    }).collect_view()}
                                                </select></div>
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
