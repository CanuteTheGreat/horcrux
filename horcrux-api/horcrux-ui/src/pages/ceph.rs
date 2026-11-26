//! Infrastructure > Ceph (`/storage/ceph`): cluster health, monitors, OSDs
//! (in / out), pools (create / delete), CRUSH rules and maintenance flags,
//! through this node's `ceph` command line.
use crate::api::ceph as c;
use leptos::*;

fn gb(b: u64) -> String {
    format!("{:.1} GB", b as f64 / 1_073_741_824.0)
}

#[component]
pub fn CephPage() -> impl IntoView {
    let (status, set_status) = create_signal(None::<c::CephStatus>);
    let (mons, set_mons) = create_signal(Vec::<c::Monitor>::new());
    let (osds, set_osds) = create_signal(Vec::<c::Osd>::new());
    let (pools, set_pools) = create_signal(Vec::<c::Pool>::new());
    let (rules, set_rules) = create_signal(Vec::<c::CrushRule>::new());
    let (flags, set_flags) = create_signal(Vec::<c::CephFlag>::new());
    let (placement, set_placement) = create_signal(None::<c::CephPlacement>);
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let name = create_rw_signal(String::new());
    let pg_num = create_rw_signal("32".to_string());
    let size = create_rw_signal("3".to_string());
    let app = create_rw_signal("rbd".to_string());
    let pool_rule = create_rw_signal(String::new());
    let rule_name = create_rw_signal(String::new());
    let rule_domain = create_rw_signal("host".to_string());
    let rule_class = create_rw_signal("nvme".to_string());

    let reload = move || {
        spawn_local(async move {
            if let Ok(p) = c::ceph_placement().await {
                set_placement.set(Some(p));
            }
            if let Ok(s) = c::ceph_status().await {
                let up = s.reachable;
                set_status.set(Some(s));
                if up {
                    if let Ok(m) = c::ceph_monitors().await {
                        set_mons.set(m);
                    }
                    if let Ok(o) = c::ceph_osds().await {
                        set_osds.set(o);
                    }
                    if let Ok(p) = c::ceph_pools().await {
                        set_pools.set(p);
                    }
                    if let Ok(r) = c::ceph_crush_rules().await {
                        set_rules.set(r);
                    }
                    if let Ok(f) = c::ceph_flags().await {
                        set_flags.set(f);
                    }
                }
            }
        });
    };
    create_effect(move |_| reload());
    let handle = set_interval_with_handle(reload, std::time::Duration::from_secs(10)).ok();
    on_cleanup(move || {
        if let Some(h) = handle {
            h.clear();
        }
    });

    let fail = move |e: crate::api::ApiError| {
        set_notice.set(None);
        set_error.set(Some(e.message));
    };
    let ok = move |m: String| {
        set_error.set(None);
        set_notice.set(Some(m));
    };
    let osd_do = move |id: u64, action: &'static str| {
        spawn_local(async move {
            match c::ceph_osd_action(id, action).await {
                Ok(()) => ok(format!("osd.{id} marked {action}")),
                Err(e) => fail(e),
            }
            reload();
        });
    };
    let osd_device = create_rw_signal(String::new());
    let create_osd = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let dev = osd_device.get().trim().to_string();
        if dev.is_empty() {
            return;
        }
        let typed = web_sys::window().and_then(|w| {
            w.prompt_with_message(&format!(
                "Creating an OSD on {dev} ERASES the whole disk. Type the device path to confirm:"
            ))
            .ok()
            .flatten()
        });
        if typed.as_deref() != Some(dev.as_str()) {
            return;
        }
        spawn_local(async move {
            match c::ceph_create_osd(&dev).await {
                Ok(()) => {
                    osd_device.set(String::new());
                    ok(format!("OSD creation on {dev} started"));
                }
                Err(e) => fail(e),
            }
            reload();
        });
    };
    let create = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let (n, pg, sz, a, cr) = (
            name.get().trim().to_string(),
            pg_num.get().trim().parse().unwrap_or(0),
            size.get().trim().parse().unwrap_or(0),
            app.get(),
            pool_rule.get(),
        );
        spawn_local(async move {
            match c::ceph_create_pool(&n, pg, sz, &a, &cr).await {
                Ok(()) => ok(format!("Pool {n} created")),
                Err(e) => fail(e),
            }
            reload();
        });
    };
    let create_rule = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let (n, fd, dc) = (
            rule_name.get().trim().to_string(),
            rule_domain.get(),
            rule_class.get(),
        );
        spawn_local(async move {
            match c::ceph_create_crush_rule(&n, &fd, &dc).await {
                Ok(()) => {
                    rule_name.set(String::new());
                    ok(format!("CRUSH rule {n} created"));
                }
                Err(e) => fail(e),
            }
            reload();
        });
    };
    let delete = move |n: String| {
        let typed = web_sys::window().and_then(|w| {
            w.prompt_with_message(&format!(
                "Deleting pool {n} destroys all its data. Type the pool name to confirm:"
            ))
            .ok()
            .flatten()
        });
        if typed.as_deref() != Some(n.as_str()) {
            return;
        }
        spawn_local(async move {
            match c::ceph_delete_pool(&n).await {
                Ok(()) => ok(format!("Pool {n} deleted")),
                Err(e) => fail(e),
            }
            reload();
        });
    };
    let flag = move |f: String, on: bool| {
        spawn_local(async move {
            match c::ceph_set_flag(&f, on).await {
                Ok(l) => {
                    set_flags.set(l);
                    ok(format!("{f} {}", if on { "set" } else { "cleared" }));
                }
                Err(e) => fail(e),
            }
        });
    };

    view! {
        <div class="ceph-page">
            <div class="page-header"><h1>"Ceph"</h1></div>
            {move || notice.get().map(|n| view! { <div class="alert alert-info" id="ceph-notice">{n}</div> })}
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="ceph-error">{e}</div> })}
            {move || status.get().map(|s| {
                if !s.reachable {
                    return view! {
                        <div class="alert alert-info" id="ceph-unavailable">
                            {s.error.clone().unwrap_or_else(|| "No Ceph cluster answers on this node.".into())}
                        </div>
                    }.into_view();
                }
                view! {
                    <div id="ceph-status">
                        <h2>"Health: " <span id="ceph-health">{s.health.clone()}</span></h2>
                        <ul>{s.health_checks.iter().map(|h| view! { <li>{format!("{} ({}): {}", h.name, h.severity, h.summary)}</li> }).collect_view()}</ul>
                        <p>{format!("Monitors {} of {} in quorum ({}); manager {}, {} standby; OSDs {} up / {} in of {}; {} PGs in {} pools.",
                            s.mons_in_quorum.len(), s.mons_total, s.mons_in_quorum.join(", "),
                            if s.mgr_available { "active" } else { "NOT available" }, s.mgr_standbys,
                            s.osds_up, s.osds_in, s.osds_total, s.pgs_total, s.pools)}</p>
                        <p>{format!("Capacity: {} used of {} ({} free)", gb(s.bytes_used), gb(s.bytes_total), gb(s.bytes_avail))}</p>
                        <p>{s.pg_states.iter().map(|(n, c)| format!("{n}: {c}")).collect::<Vec<_>>().join(" | ")}</p>
                    </div>
                }.into_view()
            })}

            <h2>"Placement"</h2>
            <div id="ceph-placement">
                {move || placement.get().map(|p| {
                    let list = |v: &[String]| if v.is_empty() { "none".to_string() } else { v.join(", ") };
                    view! {
                        <p id="ceph-placement-osd">{format!("OSD hosts: {}", list(&p.osd_nodes))}</p>
                        <p id="ceph-placement-mon">{format!("Monitor hosts: {}", list(&p.mon_nodes))}</p>
                        <ul id="ceph-placement-refused">
                            {p.osd_refused.iter().map(|(n, why)| view! { <li>{format!("{n} cannot host OSDs: {why}")}</li> }).collect_view()}
                        </ul>
                        <ul id="ceph-placement-warnings">
                            {p.warnings.iter().map(|w| view! { <li>{w.clone()}</li> }).collect_view()}
                        </ul>
                    }
                })}
            </div>

            <h2>"Maintenance flags"</h2>
            <div id="ceph-flags">
                {move || flags.get().into_iter().map(|f| {
                    let n = f.name.clone();
                    let on = f.set;
                    view! {
                        <label>
                            <input type="checkbox" class="ceph-flag" prop:checked=on on:change=move |ev| flag(n.clone(), event_target_checked(&ev))/>
                            {format!(" {} ", f.name)}
                        </label>
                    }
                }).collect_view()}
            </div>

            <h2>"OSDs"</h2>
            <table class="vm-table" id="ceph-osd-table">
                <thead><tr><th>"OSD"</th><th>"Host"</th><th>"Class"</th><th>"Status"</th><th>"In"</th><th>"CRUSH weight"</th><th></th></tr></thead>
                <tbody>
                    {move || osds.get().into_iter().map(|o| {
                        let id = o.id;
                        view! {
                            <tr class="ceph-osd-row">
                                <td>{o.name.clone()}</td><td>{o.host.clone()}</td>
                                <td>{o.device_class.clone().unwrap_or_default()}</td>
                                <td>{o.status.clone()}</td><td>{if o.is_in { "in" } else { "out" }}</td>
                                <td>{format!("{:.2}", o.crush_weight)}</td>
                                <td>
                                    {if o.is_in {
                                        view! { <button class="btn btn-sm osd-out" on:click=move |_| osd_do(id, "out")>"Out"</button> }.into_view()
                                    } else {
                                        view! { <button class="btn btn-sm osd-in" on:click=move |_| osd_do(id, "in")>"In"</button> }.into_view()
                                    }}
                                </td>
                            </tr>
                        }
                    }).collect_view()}
                </tbody>
            </table>

            <form id="ceph-osd-form" class="form-row fields" on:submit=create_osd>
                <div class="form-group"><label for="ceph-osd-device">"Blank disk on this node"</label><input id="ceph-osd-device" type="text" required placeholder="/dev/nvme1n1" prop:value=osd_device on:input=move |ev| osd_device.set(event_target_value(&ev))/></div>
                <button id="ceph-osd-create" type="submit" class="btn btn-danger">"Create OSD"</button>
            </form>

            <h2>"Pools"</h2>
            <form id="ceph-pool-form" class="form-row fields" on:submit=create>
                <div class="form-group"><label for="ceph-pool-name">"Pool name"</label><input id="ceph-pool-name" type="text" required placeholder="rbd-fast" prop:value=name on:input=move |ev| name.set(event_target_value(&ev))/></div>
                <div class="form-group narrow"><label for="ceph-pool-pg">"PGs (power of 2)"</label><input id="ceph-pool-pg" type="text" placeholder="32" prop:value=pg_num on:input=move |ev| pg_num.set(event_target_value(&ev))/></div>
                <div class="form-group narrow"><label for="ceph-pool-size">"Replicas"</label><input id="ceph-pool-size" type="text" placeholder="3" prop:value=size on:input=move |ev| size.set(event_target_value(&ev))/></div>
                <div class="form-group"><label for="ceph-pool-app">"Application"</label><select id="ceph-pool-app" prop:value=app on:change=move |ev| app.set(event_target_value(&ev))>
                    <option value="rbd">"rbd (VM disks)"</option><option value="cephfs">"cephfs"</option>
                    <option value="rgw">"rgw"</option><option value="">"none"</option>
                </select></div>
                <div class="form-group"><label for="ceph-pool-rule">"CRUSH rule"</label><select id="ceph-pool-rule" prop:value=pool_rule on:change=move |ev| pool_rule.set(event_target_value(&ev))>
                    <option value="">"cluster default"</option>
                    {move || rules.get().into_iter().map(|r| view! { <option value=r.name.clone()>{r.name.clone()}</option> }).collect_view()}
                </select></div>
                <button id="ceph-pool-create" type="submit" class="btn btn-primary">"Create pool"</button>
            </form>
            <table class="vm-table" id="ceph-pool-table">
                <thead><tr><th>"Pool"</th><th>"Size/min"</th><th>"PGs"</th><th>"Apps"</th><th>"Stored"</th><th>"Objects"</th><th></th></tr></thead>
                <tbody>
                    {move || pools.get().into_iter().map(|p| {
                        let n = p.name.clone();
                        view! {
                            <tr class="ceph-pool-row">
                                <td>{p.name.clone()}</td><td>{format!("{}/{}", p.size, p.min_size)}</td>
                                <td>{p.pg_num}</td><td>{p.applications.join(", ")}</td>
                                <td>{gb(p.stored_bytes)}</td><td>{p.objects}</td>
                                <td><button class="btn btn-sm btn-danger pool-delete" on:click=move |_| delete(n.clone())>"Delete"</button></td>
                            </tr>
                        }
                    }).collect_view()}
                </tbody>
            </table>

            <h2>"Monitors"</h2>
            <ul id="ceph-mons">{move || mons.get().into_iter().map(|m| view! { <li>{format!("{} (rank {}) {} {}", m.name, m.rank, m.address, if m.in_quorum { "in quorum" } else { "OUT of quorum" })}</li> }).collect_view()}</ul>

            <h2>"CRUSH rules"</h2>
            <form id="ceph-rule-form" class="form-row fields" on:submit=create_rule>
                <div class="form-group"><label for="ceph-rule-name">"Rule name"</label><input id="ceph-rule-name" type="text" required placeholder="nvme-host" prop:value=rule_name on:input=move |ev| rule_name.set(event_target_value(&ev))/></div>
                <div class="form-group"><label for="ceph-rule-class">"Disks"</label><select id="ceph-rule-class" prop:value=rule_class on:change=move |ev| rule_class.set(event_target_value(&ev))>
                    <option value="nvme">"NVMe only"</option><option value="ssd">"SSD only"</option>
                    <option value="hdd">"HDD only"</option><option value="">"any"</option>
                </select></div>
                <div class="form-group"><label for="ceph-rule-domain">"Replicas on different"</label><select id="ceph-rule-domain" prop:value=rule_domain on:change=move |ev| rule_domain.set(event_target_value(&ev))>
                    <option value="host">"hosts"</option><option value="osd">"disks (single node)"</option>
                    <option value="chassis">"chassis"</option><option value="rack">"racks"</option>
                </select></div>
                <button id="ceph-rule-create" type="submit" class="btn btn-primary">"Create rule"</button>
            </form>
            <ul id="ceph-rules">{move || rules.get().into_iter().map(|r| view! { <li>{format!("{} (id {}): from {} spread over {}", r.name, r.id, r.root.clone().unwrap_or_default(), r.failure_domain.clone().unwrap_or_default())}</li> }).collect_view()}</ul>
        </div>
    }
}
