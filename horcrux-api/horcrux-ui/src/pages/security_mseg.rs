//! Security > Microsegmentation: per-guest default-deny profiles, templates,
//! "allow from group" rules, per-rule hit counters and the flow view.
//! API: horcrux-api/src/api/security/microseg.rs; design
//! docs/design/security-suite.md.
use super::security::SecurityTabs;
use crate::api::*;
use leptos::*;
use leptos_router::*;

fn count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out.chars().rev().collect()
}

fn bytes(n: u64) -> String {
    const U: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

// ------------------------------------------------------------ guest list

#[component]
pub fn MicrosegPage() -> impl IntoView {
    let (rows, set_rows) = create_signal(None::<Vec<MsegGuestRow>>);
    let (err, set_err) = create_signal(None::<String>);
    let load = create_action(move |_: &()| async move {
        match list_microseg_guests().await {
            Ok(v) => set_rows.set(Some(v)),
            Err(e) => set_err.set(Some(format!("Could not load the guests: {e}"))),
        }
    });
    load.dispatch(());
    let poll = set_interval_with_handle(
        move || load.dispatch(()),
        std::time::Duration::from_secs(10),
    )
    .ok();
    on_cleanup(move || {
        if let Some(h) = poll {
            h.clear();
        }
    });
    view! {
        <div class="security-page">
            <div class="page-header">
                <h1>"Microsegmentation"</h1>
                <div class="header-actions">
                    <button class="btn btn-secondary" on:click=move |_| load.dispatch(())>"Refresh"</button>
                </div>
            </div>
            <SecurityTabs active="mseg"/>
            <p class="help-text">
                "A profile puts a default-deny firewall around one guest, on every network card it has: bridges, SDN networks and macvtap. Pick a template, then allow what the guest needs. Rules count the packets they match."
            </p>
            {move || err.get().map(|m| view! { <div class="alert alert-error">{m}</div> })}
            {move || match rows.get() {
                None => view! { <div class="loading">"Loading..."</div> }.into_view(),
                Some(v) if v.is_empty() => view! {
                    <div class="empty-state card">
                        <p>"There are no VMs or containers on this node yet."</p>
                        <A href="/vms" class="btn btn-primary">"Go to VMs"</A>
                    </div>
                }.into_view(),
                Some(v) => view! {
                    <div class="card table-container">
                        <table class="data-table">
                            <thead><tr>
                                <th>"Guest"</th><th>"Type"</th><th>"State"</th><th>"Profile"</th>
                                <th>"Enforced"</th><th class="num">"Dropped packets"</th><th></th>
                            </tr></thead>
                            <tbody>
                                {v.into_iter().map(|g| {
                                    let href = format!("/security/microsegmentation/{}", g.key);
                                    let tmpl = g.template.clone();
                                    view! {
                                        <tr>
                                            <td><a href=href.clone()>{g.name.clone()}</a>" "<span class="mono muted">{g.id.clone()}</span></td>
                                            <td>{g.kind.clone()}</td>
                                            <td>{if g.running { "running" } else { "stopped" }}</td>
                                            <td>{match tmpl { Some(t) => t.replace('_', " "), None => "none".into() }}</td>
                                            <td>{if g.enforced { "yes" } else { "no" }}</td>
                                            <td class="num">{count(g.dropped_packets)}</td>
                                            <td><a href=href class="btn btn-secondary btn-sm">{if g.template.is_some() { "Edit" } else { "Add profile" }}</a></td>
                                        </tr>
                                    }
                                }).collect_view()}
                            </tbody>
                        </table>
                    </div>
                }.into_view(),
            }}
        </div>
    }
}

// ------------------------------------------------------------ one guest

fn peer_rows(
    list: Signal<Vec<MsegPeer>>,
    set: impl Fn(Box<dyn Fn(&mut GuestProfile)>) + Copy + 'static,
    inbound: bool,
) -> impl IntoView {
    let id = if inbound { "from" } else { "to" };
    view! {
        <div class="peer-list">
            {move || list.get().into_iter().enumerate().map(|(i, p)| {
                view! {
                    <div class="form-row peer-row">
                        <div class="form-group">
                            <label for=format!("mseg-{id}-g{i}")>"Group"</label>
                            <input id=format!("mseg-{id}-g{i}") type="text" placeholder="+ipset, alias, CIDR"
                                prop:value=p.group.clone()
                                on:change=move |e| { let c = event_target_value(&e); set(Box::new(move |x| { let l = if inbound { &mut x.allow_from } else { &mut x.allow_to }; if let Some(r) = l.get_mut(i) { r.group = c.clone() } })) }/>
                        </div>
                        <div class="form-group">
                            <label for=format!("mseg-{id}-p{i}")>"Protocol"</label>
                            <select id=format!("mseg-{id}-p{i}")
                                on:change=move |e| { let c = event_target_value(&e); set(Box::new(move |x| { let l = if inbound { &mut x.allow_from } else { &mut x.allow_to }; if let Some(r) = l.get_mut(i) { r.protocol = c.clone() } })) }>
                                {["any", "tcp", "udp", "icmp"].into_iter().map(|o| view! { <option value=o selected=p.protocol == o>{o}</option> }).collect_view()}
                            </select>
                        </div>
                        <div class="form-group">
                            <label for=format!("mseg-{id}-d{i}")>"Ports"</label>
                            <input id=format!("mseg-{id}-d{i}") type="text" placeholder="any, 22 or 80,443"
                                prop:value=p.dport.clone()
                                on:change=move |e| { let c = event_target_value(&e); set(Box::new(move |x| { let l = if inbound { &mut x.allow_from } else { &mut x.allow_to }; if let Some(r) = l.get_mut(i) { r.dport = c.clone() } })) }/>
                        </div>
                        <div class="form-group">
                            <label for=format!("mseg-{id}-c{i}")>"Note"</label>
                            <input id=format!("mseg-{id}-c{i}") type="text" prop:value=p.comment.clone()
                                on:change=move |e| { let c = event_target_value(&e); set(Box::new(move |x| { let l = if inbound { &mut x.allow_from } else { &mut x.allow_to }; if let Some(r) = l.get_mut(i) { r.comment = c.clone() } })) }/>
                        </div>
                        <div class="form-group">
                            <button type="button" class="btn btn-secondary btn-sm" aria-label="Remove this rule"
                                on:click=move |_| set(Box::new(move |x| { let l = if inbound { &mut x.allow_from } else { &mut x.allow_to }; if i < l.len() { l.remove(i); } }))>"Remove"</button>
                        </div>
                    </div>
                }
            }).collect_view()}
            <button type="button" class="btn btn-secondary btn-sm"
                on:click=move |_| set(Box::new(move |x| { let l = if inbound { &mut x.allow_from } else { &mut x.allow_to }; l.push(MsegPeer::default()) }))>
                {if inbound { "Allow from a group" } else { "Allow to a group" }}
            </button>
        </div>
    }
}

#[component]
pub fn MicrosegGuestPage() -> impl IntoView {
    let params = use_params_map();
    let key = move || params.with(|p| p.get("key").cloned().unwrap_or_default());
    let (templates, set_templates) = create_signal(Vec::<MsegTemplateInfo>::new());
    let (profile, set_profile) = create_signal(None::<GuestProfile>);
    let (existing, set_existing) = create_signal(false);
    let (row, set_row) = create_signal(None::<MsegGuestRow>);
    let (hits, set_hits) = create_signal(Vec::<MsegRuleHit>::new());
    let (flows, set_flows) = create_signal(None::<MsegFlowView>);
    let (ip, set_ip) = create_signal(String::new());
    let (err, set_err) = create_signal(None::<String>);
    let (ok, set_ok) = create_signal(None::<String>);
    let (warn, set_warn) = create_signal(Vec::<String>::new());

    let refresh_live = create_action(move |k: &String| {
        let k = k.clone();
        async move {
            if let Ok(h) = get_microseg_rules(&k).await {
                set_hits.set(h);
            }
            if let Ok(f) = get_microseg_flows(&k, &ip.get_untracked()).await {
                set_flows.set(Some(f));
            }
        }
    });
    let load = create_action(move |k: &String| {
        let k = k.clone();
        async move {
            if let Ok(t) = get_microseg_templates().await {
                set_templates.set(t);
            }
            match list_microseg_guests().await {
                Ok(v) => {
                    let r = v.into_iter().find(|g| g.key == k);
                    set_existing.set(r.as_ref().map(|g| g.template.is_some()).unwrap_or(false));
                    if r.is_none() {
                        set_err.set(Some(format!("{k} is not a guest of this node.")));
                    }
                    set_row.set(r);
                }
                Err(e) => set_err.set(Some(format!("Could not load the guest: {e}"))),
            }
            if existing.get_untracked() {
                match get_microseg_profile(&k).await {
                    Ok(p) => set_profile.set(Some(p)),
                    Err(e) => set_err.set(Some(format!("Could not load the profile: {e}"))),
                }
            } else {
                // A new profile starts from the safe default: deny both ways.
                set_profile.set(Some(GuestProfile::default()));
            }
            refresh_live.dispatch(k);
        }
    });
    create_effect(move |_| load.dispatch(key()));
    let poll = set_interval_with_handle(
        move || refresh_live.dispatch(key()),
        std::time::Duration::from_secs(5),
    )
    .ok();
    on_cleanup(move || {
        if let Some(h) = poll {
            h.clear();
        }
    });

    let save = create_action(move |p: &GuestProfile| {
        let p = p.clone();
        async move {
            set_err.set(None);
            set_ok.set(None);
            match put_microseg_profile(&key(), &p).await {
                Ok(a) => {
                    set_profile.set(Some(a.profile));
                    set_existing.set(true);
                    set_warn.set(a.warnings);
                    set_ok.set(Some("Saved and installed.".into()));
                    refresh_live.dispatch(key());
                }
                Err(e) => set_err.set(Some(format!("Not saved: {e}"))),
            }
        }
    });
    let remove = create_action(move |_: &()| async move {
        set_err.set(None);
        set_ok.set(None);
        match delete_microseg_profile(&key()).await {
            Ok(()) => {
                set_existing.set(false);
                set_profile.set(Some(GuestProfile::default()));
                set_hits.set(Vec::new());
                set_ok.set(Some(
                    "Profile removed: the guest is unrestricted again.".into(),
                ));
            }
            Err(e) => set_err.set(Some(format!("Not removed: {e}"))),
        }
    });
    let reapply = create_action(move |_: &()| async move {
        set_err.set(None);
        match apply_microseg_profile(&key()).await {
            Ok(w) => {
                set_warn.set(w);
                set_ok.set(Some("Installed again.".into()));
                refresh_live.dispatch(key());
            }
            Err(e) => set_err.set(Some(format!("Not installed: {e}"))),
        }
    });
    let upd = move |f: Box<dyn Fn(&mut GuestProfile)>| {
        set_profile.update(|o| {
            if let Some(x) = o.as_mut() {
                f(x)
            }
        })
    };
    let peers_in = Signal::derive(move || profile.get().map(|p| p.allow_from).unwrap_or_default());
    let peers_out = Signal::derive(move || profile.get().map(|p| p.allow_to).unwrap_or_default());

    view! {
        <div class="security-page">
            <div class="page-header">
                <h1>{move || row.get().map(|r| format!("{} · Microsegmentation", r.name)).unwrap_or_else(|| "Microsegmentation".into())}</h1>
                <div class="header-actions">
                    <A href="/security/microsegmentation" class="btn btn-secondary">"All guests"</A>
                    <button class="btn btn-secondary" disabled=move || !existing.get() on:click=move |_| reapply.dispatch(())>"Install again"</button>
                </div>
            </div>
            <SecurityTabs active="mseg"/>
            {move || err.get().map(|m| view! { <div class="alert alert-error">{m}</div> })}
            {move || ok.get().map(|m| view! { <div class="alert alert-success">{m}</div> })}
            {move || warn.get().into_iter().map(|m| view! { <div class="alert alert-warning">{m}</div> }).collect_view()}
            {move || row.get().and_then(|r| (!r.macvtap_devices.is_empty()).then(|| view! {
                <p class="help-text">"Macvtap cards (" <span class="mono">{r.macvtap_devices.join(", ")}</span> ") are filtered on the device itself, without connection tracking: replies to allowed TCP, ICMP and UDP flows pass, new flows follow the rules."</p>
            }))}
            {move || profile.get().map(|cur| {
                let tinfo = templates.get().into_iter().find(|t| t.id == cur.template.id());
                view! {
                    <form class="card settings-form" on:submit=move |e| { e.prevent_default(); if let Some(x) = profile.get_untracked() { save.dispatch(x) } }>
                        <h2>"Profile"</h2>
                        <div class="form-group">
                            <label for="mseg-on"><input id="mseg-on" type="checkbox" prop:checked=cur.enabled
                                on:change=move |e| { let c = event_target_checked(&e); upd(Box::new(move |x| x.enabled = c)) }/>" Profile on"</label>
                        </div>
                        <div class="form-group">
                            <label for="mseg-tpl">"Template"</label>
                            <select id="mseg-tpl" on:change=move |e| {
                                let c = event_target_value(&e);
                                let t = MsegTemplate::ALL.iter().copied().find(|t| t.id() == c).unwrap_or_default();
                                upd(Box::new(move |x| x.template = t))
                            }>
                                {templates.get().into_iter().map(|t| {
                                    let sel = t.id == cur.template.id();
                                    view! { <option value=t.id.clone() selected=sel>{t.title.clone()}</option> }
                                }).collect_view()}
                            </select>
                            {tinfo.clone().map(|t| view! {
                                <small class="form-help">{t.description.clone()}</small>
                                {(!t.allows.is_empty()).then(|| view! { <ul class="mono small">{t.allows.iter().map(|l| view! { <li>{l.clone()}</li> }).collect_view()}</ul> })}
                            })}
                        </div>
                        {tinfo.clone().filter(|t| t.needs_lab_network).map(|_| view! {
                            <div class="form-group">
                                <label for="mseg-lab">"Lab network"</label>
                                <input id="mseg-lab" type="text" placeholder="10.99.0.0/24, an alias or +ipset" prop:value=cur.lab_network.clone()
                                    on:change=move |e| { let c = event_target_value(&e); upd(Box::new(move |x| x.lab_network = c.clone())) }/>
                                <small class="form-help">"The only network this guest may talk to."</small>
                            </div>
                        })}
                        {tinfo.clone().filter(|t| !t.default_ports.is_empty() || cur.template == MsegTemplate::PentestBox).map(|t| view! {
                            <div class="form-group">
                                <label for="mseg-ports">"Ports"</label>
                                <input id="mseg-ports" type="text" placeholder=if t.default_ports.is_empty() { "any".to_string() } else { t.default_ports.clone() } prop:value=cur.ports.clone()
                                    on:change=move |e| { let c = event_target_value(&e); upd(Box::new(move |x| x.ports = c.clone())) }/>
                                <small class="form-help">"Empty keeps the template's ports."</small>
                            </div>
                        })}
                        <div class="form-row">
                            <div class="form-group">
                                <label for="mseg-din">"When nothing matches, traffic into the guest is"</label>
                                <select id="mseg-din" on:change=move |e| { let c = event_target_value(&e) == "allow"; upd(Box::new(move |x| x.default_in = if c { MsegDefault::Allow } else { MsegDefault::Deny })) }>
                                    <option value="deny" selected=cur.default_in == MsegDefault::Deny>"dropped"</option>
                                    <option value="allow" selected=cur.default_in == MsegDefault::Allow>"allowed"</option>
                                </select>
                            </div>
                            <div class="form-group">
                                <label for="mseg-dout">"When nothing matches, traffic out of the guest is"</label>
                                <select id="mseg-dout" on:change=move |e| { let c = event_target_value(&e) == "allow"; upd(Box::new(move |x| x.default_out = if c { MsegDefault::Allow } else { MsegDefault::Deny })) }>
                                    <option value="deny" selected=cur.default_out == MsegDefault::Deny>"dropped"</option>
                                    <option value="allow" selected=cur.default_out == MsegDefault::Allow>"allowed"</option>
                                </select>
                            </div>
                        </div>
                        <h3>"Allow from groups"</h3>
                        <p class="help-text">"A group is an IP set (written +name), an alias or an address range from Firewall > Objects."</p>
                        {peer_rows(peers_in, upd, true)}
                        <h3>"Allow to groups"</h3>
                        {peer_rows(peers_out, upd, false)}
                        <div class="form-actions">
                            <button type="submit" class="btn btn-primary" disabled=move || save.pending().get()>"Save and install"</button>
                            {move || existing.get().then(|| view! {
                                <button type="button" class="btn btn-danger" on:click=move |_| {
                                    if web_sys::window().and_then(|w| w.confirm_with_message("Remove this profile? The guest is unrestricted again.").ok()).unwrap_or(false) { remove.dispatch(()) }
                                }>"Remove profile"</button>
                            })}
                        </div>
                    </form>
                }
            })}
            <div class="card">
                <h2>"Rules and hit counters"</h2>
                {move || if hits.get().is_empty() {
                    view! {
                        <div class="empty-state">
                            <p class="help-text">"Nothing installed yet: the rules of a saved profile show up here and count the packets they match."</p>
                            <button type="button" class="btn btn-secondary btn-sm" on:click=move |_| {
                                if let Some(el) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id("mseg-tpl")) {
                                    let _ = wasm_bindgen::JsCast::dyn_into::<web_sys::HtmlElement>(el).map(|e| e.focus());
                                }
                            }>"Pick a template"</button>
                        </div>
                    }.into_view()
                } else {
                    view! {
                        <div class="table-container"><table class="data-table">
                            <thead><tr><th>"Direction"</th><th>"Rule"</th><th>"Action"</th><th class="num">"Packets"</th><th class="num">"Bytes"</th></tr></thead>
                            <tbody>
                                {hits.get().into_iter().map(|h| view! {
                                    <tr>
                                        <td>{if h.direction == "in" { "into the guest" } else { "out of the guest" }}</td>
                                        <td><span class="mono">{h.rule_id.clone()}</span>{(!h.rule.is_empty()).then(|| format!(" · {}", h.rule))}</td>
                                        <td>{h.action.clone()}</td>
                                        <td class="num">{count(h.packets)}</td>
                                        <td class="num">{bytes(h.bytes)}</td>
                                    </tr>
                                }).collect_view()}
                            </tbody>
                        </table></div>
                    }.into_view()
                }}
            </div>
            <div class="card">
                <h2>"Flows"</h2>
                <div class="form-row">
                    <div class="form-group">
                        <label for="mseg-ip">"Only flows of this address"</label>
                        <input id="mseg-ip" type="text" placeholder="the guest's IP address" prop:value=move || ip.get()
                            on:change=move |e| { set_ip.set(event_target_value(&e)); refresh_live.dispatch(key()) }/>
                    </div>
                </div>
                {move || flows.get().map(|f| view! {
                    {f.notes.iter().map(|n| view! { <p class="help-text">{n.clone()}</p> }).collect_view()}
                    <h3>"Allowed (tracked)"</h3>
                    {if f.flows.is_empty() { view! { <p class="help-text">"No tracked flows."</p> }.into_view() } else { view! {
                        <div class="table-container"><table class="data-table">
                            <thead><tr><th>"Protocol"</th><th>"From"</th><th>"To"</th><th>"State"</th><th class="num">"Packets"</th></tr></thead>
                            <tbody>{f.flows.iter().map(|x| view! {
                                <tr><td>{x.proto.clone()}</td><td class="mono">{format!("{}:{}", x.src, x.sport)}</td><td class="mono">{format!("{}:{}", x.dst, x.dport)}</td><td>{x.state.clone()}</td><td class="num">{count(x.packets)}</td></tr>
                            }).collect_view()}</tbody>
                        </table></div> }.into_view() }}
                    <h3>"Dropped"</h3>
                    {if f.denied.is_empty() { view! { <p class="help-text">"No dropped flows logged."</p> }.into_view() } else { view! {
                        <div class="table-container"><table class="data-table">
                            <thead><tr><th>"Time"</th><th>"Protocol"</th><th>"From"</th><th>"To"</th></tr></thead>
                            <tbody>{f.denied.iter().map(|x| view! {
                                <tr><td>{x.time.clone()}</td><td>{x.proto.clone()}</td><td class="mono">{format!("{}:{}", x.src, x.sport)}</td><td class="mono">{format!("{}:{}", x.dst, x.dport)}</td></tr>
                            }).collect_view()}</tbody>
                        </table></div> }.into_view() }}
                })}
            </div>
        </div>
    }
}
