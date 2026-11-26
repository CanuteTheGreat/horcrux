//! Infrastructure > Management Link (`/network/management`): the NICs, bond
//! (link aggregation, incl. 802.3ad LACP) and VLAN tag that carry this node's
//! own address, web UI and SSH, as the installer set them up. Shows the live
//! state (bond mode, each member's link, LACP partner, addresses) with a
//! warning that names every problem, and edits the layout: written for the
//! node's init system and applied at the next boot, or right away when the
//! user ticks "apply now".
use crate::api::{
    get_local_nics, get_mgmt_link, set_mgmt_link, MgmtBond, MgmtRecord, MgmtView, NicView,
};
use leptos::*;

fn split_words(s: &str) -> Vec<String> {
    s.split([',', ' '])
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(String::from)
        .collect()
}

/// Everything wrong with the live link, in words a user can act on (empty = healthy).
pub fn link_problems(v: &MgmtView) -> Vec<String> {
    let (r, l) = (&v.record, &v.live);
    let mut out = Vec::new();
    if !l.address_interface_up {
        out.push(format!("{} is down.", r.address_interface));
    } else if l.addresses.is_empty() {
        out.push(format!(
            "{} has no address{}.",
            r.address_interface,
            if r.mode == "dhcp" {
                match r.vlan {
                    Some(id) => format!(" (no DHCP answer on VLAN {id})"),
                    None => " (no DHCP answer)".to_string(),
                }
            } else {
                String::new()
            }
        ));
    }
    for m in &l.members {
        match m.link.as_str() {
            "up" => {}
            "missing" => out.push(format!(
                "{} does not exist on this node (renamed or removed NIC).",
                m.name
            )),
            other => out.push(format!("{} link is {other}.", m.name)),
        }
    }
    if r.bond.as_ref().is_some_and(|b| b.mode == "802.3ad")
        && l.lacp_partner
            .as_deref()
            .is_some_and(|p| p == "00:00:00:00:00:00")
    {
        out.push(
            "No LACP partner answered: the switch ports need an LACP (802.3ad) group, or use active-backup."
                .to_string(),
        );
    }
    out
}

/// NICs a management link can use: physical links of this node, not bonds, VLANs or loopback.
fn usable_nics(nics: &[NicView]) -> Vec<NicView> {
    nics.iter()
        .filter(|n| !n.is_bond && n.name != "lo" && !n.name.contains('.'))
        .cloned()
        .collect()
}

#[component]
pub fn MgmtLinkPage() -> impl IntoView {
    let (view_data, set_view) = create_signal(None::<MgmtView>);
    let (unavailable, set_unavailable) = create_signal(None::<crate::api::Unavailable>);
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let (loading, set_loading) = create_signal(true);
    let (nics, set_nics) = create_signal(Vec::<NicView>::new());
    let ifaces = create_rw_signal(String::new());
    let bond_name = create_rw_signal("bond0".to_string());
    let bond_mode = create_rw_signal("active-backup".to_string());
    let hash = create_rw_signal("layer3+4".to_string());
    let lacp = create_rw_signal("fast".to_string());
    let miimon = create_rw_signal("100".to_string());
    let vlan = create_rw_signal(String::new());
    let mode = create_rw_signal("dhcp".to_string());
    let address = create_rw_signal(String::new());
    let gateway = create_rw_signal(String::new());
    let dns = create_rw_signal(String::new());
    let apply_now = create_rw_signal(false);
    let gen = store_value(0u32);

    let fill = move |r: &MgmtRecord| {
        ifaces.set(r.interfaces.join(" "));
        if let Some(b) = &r.bond {
            bond_name.set(b.name.clone());
            bond_mode.set(b.mode.clone());
            hash.set(
                b.xmit_hash_policy
                    .clone()
                    .unwrap_or_else(|| "layer3+4".into()),
            );
            lacp.set(b.lacp_rate.clone().unwrap_or_else(|| "fast".into()));
            miimon.set(b.miimon.to_string());
        }
        vlan.set(r.vlan.map(|v| v.to_string()).unwrap_or_default());
        mode.set(r.mode.clone());
        address.set(r.address.clone().unwrap_or_default());
        gateway.set(r.gateway.clone().unwrap_or_default());
        dns.set(r.dns.join(" "));
    };

    let reload = move || {
        gen.update_value(|g| *g += 1);
        let mine = gen.get_value();
        set_loading.set(true);
        spawn_local(async move {
            let r = get_mgmt_link().await;
            if gen.get_value() != mine {
                return; // a newer load is in flight
            }
            set_loading.set(false);
            match r {
                Ok(v) => {
                    fill(&v.record);
                    set_unavailable.set(None);
                    set_error.set(None);
                    set_view.set(Some(v));
                }
                Err(e) => match e.unavailable() {
                    Some(u) => set_unavailable.set(Some(u)),
                    None => set_error.set(Some(e.message)),
                },
            }
        });
    };
    create_effect(move |_| reload());
    // the NIC picker; without the list (older node, no permission) the NICs are typed by name
    spawn_local(async move {
        if let Ok(v) = get_local_nics().await {
            set_nics.set(usable_nics(&v.nics));
        }
    });

    let chosen = create_memo(move |_| split_words(&ifaces.get()));
    let bonded = create_memo(move |_| chosen.get().len() > 1);
    let problems = create_memo(move |_| view_data.get().map(|v| link_problems(&v)));
    let toggle = move |name: String, on: bool| {
        let mut list = split_words(&ifaces.get_untracked());
        list.retain(|n| n != &name);
        if on {
            list.push(name);
        }
        ifaces.set(list.join(" "));
    };

    let save = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let list = split_words(&ifaces.get());
        if list.is_empty() {
            set_error.set(Some("Choose at least one NIC.".into()));
            return;
        }
        let bond = (list.len() > 1).then(|| MgmtBond {
            name: bond_name.get().trim().to_string(),
            mode: bond_mode.get(),
            miimon: miimon.get().trim().parse().unwrap_or(0),
            xmit_hash_policy: matches!(
                bond_mode.get().as_str(),
                "802.3ad" | "balance-xor" | "balance-tlb"
            )
            .then(|| hash.get()),
            lacp_rate: (bond_mode.get() == "802.3ad").then(|| lacp.get()),
            primary: None,
        });
        let vl = vlan.get().trim().to_string();
        let vlan_id = if vl.is_empty() || vl == "0" {
            None
        } else {
            match vl.parse::<u16>() {
                Ok(v) => Some(v),
                Err(_) => {
                    set_error.set(Some(
                        "VLAN tag: a number 1-4094, or empty for untagged".into(),
                    ));
                    return;
                }
            }
        };
        let st = mode.get() == "static";
        let rec = MgmtRecord {
            version: 1,
            interfaces: list,
            bond,
            vlan: vlan_id,
            address_interface: String::new(),
            mode: mode.get(),
            address: st.then(|| address.get().trim().to_string()),
            gateway: st
                .then(|| gateway.get().trim().to_string())
                .filter(|g| !g.is_empty()),
            dns: if st { split_words(&dns.get()) } else { vec![] },
            stack: None,
        };
        let now = apply_now.get();
        if now
            && !web_sys::window()
                .and_then(|w| {
                    w.confirm_with_message(
                        "Apply the new management link now? If it does not come up (wrong NICs, a switch without the VLAN or LACP group), this page and SSH lose the node until it is fixed on the console.",
                    )
                    .ok()
                })
                .unwrap_or(false)
        {
            return;
        }
        spawn_local(async move {
            match set_mgmt_link(&rec, now).await {
                Ok(r) => {
                    set_error.set(None);
                    set_notice.set(Some(if r.applied {
                        format!(
                            "Applied: the address is now on {}.",
                            r.record.address_interface
                        )
                    } else {
                        format!(
                            "Saved for the next boot (address on {}). {}",
                            r.record.address_interface,
                            r.log.join("; ")
                        )
                    }));
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            reload();
        });
    };

    view! {
        <div class="mgmt-link">
            <div class="page-header"><h1>"Management Link"</h1></div>
            <p class="form-help">
                "The network link that carries this node's own address, web UI and SSH: one NIC, or several NICs bonded into one link (failover or LACP), optionally tagged with a VLAN. The installer set it up. Changes are written for this node's init system and take effect at the next boot unless you apply them now."
            </p>
            {move || notice.get().map(|n| view! { <div class="alert alert-info" id="mgmt-notice">{n}</div> })}
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="mgmt-error">{e}</div> })}
            {move || {
                if loading.get() && view_data.get().is_none() {
                    view! { <p class="loading">"Loading the management link..."</p> }.into_view()
                } else if let Some(u) = unavailable.get() {
                    view! { <crate::components::UnavailableState info=u on_retry=Callback::new(move |_| reload())/> }.into_view()
                } else if let Some(v) = view_data.get() {
                    let r = v.record.clone();
                    let l = v.live.clone();
                    let modes = store_value(v.bond_modes.clone());
                    let hashes = store_value(v.hash_policies.clone());
                    let recorded = store_value(r.interfaces.clone());
                    let healthy = move || problems.get().is_some_and(|p| p.is_empty());
                    view! {
                        <section class="card" id="mgmt-state-card">
                            <div class="card-head">
                                <h2>"Current state"</h2>
                                {move || if healthy() {
                                    view! { <span class="badge badge-success" id="mgmt-health-badge">"Healthy"</span> }
                                } else {
                                    view! { <span class="badge badge-warning" id="mgmt-health-badge">"Needs attention"</span> }
                                }}
                            </div>
                            {move || problems.get().filter(|p| !p.is_empty()).map(|p| view! {
                                <div class="alert alert-warning" id="mgmt-health">
                                    <ul class="plain-list">{p.into_iter().map(|x| view! { <li>{x}</li> }).collect_view()}</ul>
                                </div>
                            })}
                            <table class="kv-table" id="mgmt-state">
                                <tbody>
                                    <tr><th>"Address on"</th><td id="mgmt-l3">{r.address_interface.clone()}" "
                                        <span class=if l.address_interface_up { "badge badge-success" } else { "badge badge-warning" }>{if l.address_interface_up { "up" } else { "down" }}</span></td></tr>
                                    <tr><th>"Addresses"</th><td id="mgmt-addresses">{if l.addresses.is_empty() { "none".to_string() } else { l.addresses.join(", ") }}{if r.mode == "dhcp" { " (DHCP)" } else { " (static)" }}</td></tr>
                                    <tr><th>"Bond"</th><td id="mgmt-bond">{r.bond.as_ref().map(|b| format!("{} ({})", b.name, l.bond_mode.clone().unwrap_or_else(|| b.mode.clone()))).unwrap_or_else(|| "none (one NIC)".into())}</td></tr>
                                    {l.active_member.clone().map(|a| view! { <tr><th>"Active member"</th><td id="mgmt-active">{a}</td></tr> })}
                                    <tr><th>"VLAN"</th><td id="mgmt-vlan">{r.vlan.map(|v| v.to_string()).unwrap_or_else(|| "untagged".into())}</td></tr>
                                    {l.lacp_partner.clone().map(|p| view! { <tr><th>"LACP partner"</th><td id="mgmt-lacp">{if p == "00:00:00:00:00:00" { "none answered".to_string() } else { p }}</td></tr> })}
                                    <tr><th>"Members"</th><td id="mgmt-members">
                                        {l.members.iter().map(|m| view! {
                                            <span class=if m.link == "up" { "badge badge-success" } else { "badge badge-warning" }>{format!("{} {}", m.name, m.link)}</span>" "
                                        }).collect_view()}
                                    </td></tr>
                                </tbody>
                            </table>
                        </section>

                        <section class="card" id="mgmt-config-card">
                            <h2>"Configuration"</h2>
                            <form id="mgmt-form" on:submit=save>
                                <fieldset class="form-group" id="mgmt-nics">
                                    <legend>"NICs (choose two or more for a bond)"</legend>
                                    {move || {
                                        let mut list = nics.get();
                                        for name in recorded.get_value() {
                                            if !list.iter().any(|n| n.name == name) {
                                                list.push(NicView { name, ..Default::default() });
                                            }
                                        }
                                        if nics.get().is_empty() {
                                            view! {
                                                <input id="mgmt-ifaces" type="text" required aria-label="NIC names" title="NIC names, separated by spaces"
                                                    prop:value=ifaces on:input=move |ev| ifaces.set(event_target_value(&ev))/>
                                            }.into_view()
                                        } else {
                                            view! {
                                                <div class="nic-pick">
                                                    {list.into_iter().map(|n| {
                                                        let name = n.name.clone();
                                                        let name2 = n.name.clone();
                                                        let meta = if !nics.get_untracked().iter().any(|x| x.name == n.name) {
                                                            "missing".to_string()
                                                        } else {
                                                            format!("{}{}", if n.up { "up" } else { "down" }, n.speed_mbps.map(|s| format!(", {} Mb/s", s)).unwrap_or_default())
                                                        };
                                                        view! {
                                                            <label class="nic-choice">
                                                                <input type="checkbox" class="mgmt-nic" value=n.name.clone()
                                                                    prop:checked=move || chosen.get().contains(&name)
                                                                    on:change=move |ev| toggle(name2.clone(), event_target_checked(&ev))/>
                                                                <span>{n.name.clone()}</span>
                                                                <span class="nic-meta">{meta}</span>
                                                            </label>
                                                        }
                                                    }).collect_view()}
                                                </div>
                                            }.into_view()
                                        }
                                    }}
                                </fieldset>
                                <div class="form-row fields">
                                    <Show when=move || bonded.get()>
                                        <div class="form-group narrow"><label for="mgmt-bond-name">"Bond name"</label>
                                            <input id="mgmt-bond-name" type="text" maxlength="15" prop:value=bond_name on:input=move |ev| bond_name.set(event_target_value(&ev))/></div>
                                        <div class="form-group"><label for="mgmt-bond-mode">"Bond mode"</label>
                                            <select id="mgmt-bond-mode" on:change=move |ev| bond_mode.set(event_target_value(&ev))>
                                                {move || modes.get_value().into_iter().map(|m| { let m2 = m.clone(); view! { <option value=m.clone() selected=move || bond_mode.get() == m2>{m}</option> } }).collect_view()}
                                            </select></div>
                                        <Show when=move || matches!(bond_mode.get().as_str(), "802.3ad" | "balance-xor" | "balance-tlb")>
                                            <div class="form-group"><label for="mgmt-hash">"Transmit hash policy"</label>
                                                <select id="mgmt-hash" on:change=move |ev| hash.set(event_target_value(&ev))>
                                                    {move || hashes.get_value().into_iter().map(|h| { let h2 = h.clone(); view! { <option value=h.clone() selected=move || hash.get() == h2>{h}</option> } }).collect_view()}
                                                </select></div>
                                        </Show>
                                        <Show when=move || bond_mode.get() == "802.3ad">
                                            <div class="form-group narrow"><label for="mgmt-lacp-rate">"LACP rate"</label>
                                                <select id="mgmt-lacp-rate" on:change=move |ev| lacp.set(event_target_value(&ev))>
                                                    <option value="fast" selected=move || lacp.get() == "fast">"fast (1 s)"</option>
                                                    <option value="slow" selected=move || lacp.get() == "slow">"slow (30 s)"</option>
                                                </select></div>
                                        </Show>
                                        <div class="form-group narrow"><label for="mgmt-miimon">"Link check (ms)"</label>
                                            <input id="mgmt-miimon" type="number" min="1" max="60000" prop:value=miimon on:input=move |ev| miimon.set(event_target_value(&ev))/></div>
                                    </Show>
                                    <div class="form-group narrow"><label for="mgmt-vlan-id">"VLAN tag"</label>
                                        <input id="mgmt-vlan-id" type="number" min="1" max="4094" title="1-4094; empty = untagged" prop:value=vlan on:input=move |ev| vlan.set(event_target_value(&ev))/></div>
                                    <div class="form-group narrow"><label for="mgmt-mode">"Address"</label>
                                        <select id="mgmt-mode" on:change=move |ev| mode.set(event_target_value(&ev))>
                                            <option value="dhcp" selected=move || mode.get() == "dhcp">"DHCP"</option>
                                            <option value="static" selected=move || mode.get() == "static">"Static"</option>
                                        </select></div>
                                </div>
                                <p class="form-help">"Leave the VLAN tag empty for an untagged link."</p>
                                <Show when=move || mode.get() == "static">
                                    <div class="form-row fields">
                                        <div class="form-group"><label for="mgmt-address">"Address / prefix"</label>
                                            <input id="mgmt-address" type="text" title="for example 192.0.2.10/24" prop:value=address on:input=move |ev| address.set(event_target_value(&ev))/></div>
                                        <div class="form-group"><label for="mgmt-gateway">"Gateway"</label>
                                            <input id="mgmt-gateway" type="text" prop:value=gateway on:input=move |ev| gateway.set(event_target_value(&ev))/></div>
                                        <div class="form-group wide"><label for="mgmt-dns">"DNS servers"</label>
                                            <input id="mgmt-dns" type="text" prop:value=dns on:input=move |ev| dns.set(event_target_value(&ev))/></div>
                                    </div>
                                </Show>
                                <div class="form-actions">
                                    <label class="check-inline" for="mgmt-apply-now">
                                        <input id="mgmt-apply-now" type="checkbox" prop:checked=apply_now on:change=move |ev| apply_now.set(event_target_checked(&ev))/>
                                        " Apply now (otherwise at the next boot)"
                                    </label>
                                    <button id="mgmt-save" type="submit" class="btn btn-primary">"Save management link"</button>
                                </div>
                            </form>
                        </section>
                    }.into_view()
                } else {
                    view! { <p class="no-data">"No data."</p> }.into_view()
                }
            }}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{MgmtLive, MgmtMember};

    fn view(up: bool, addrs: &[&str], members: &[(&str, &str)], partner: Option<&str>) -> MgmtView {
        MgmtView {
            record: MgmtRecord {
                version: 1,
                interfaces: members.iter().map(|m| m.0.to_string()).collect(),
                bond: Some(MgmtBond {
                    name: "bond0".into(),
                    mode: "802.3ad".into(),
                    miimon: 100,
                    xmit_hash_policy: None,
                    lacp_rate: None,
                    primary: None,
                }),
                vlan: Some(42),
                address_interface: "bond0.42".into(),
                mode: "dhcp".into(),
                address: None,
                gateway: None,
                dns: vec![],
                stack: None,
            },
            live: MgmtLive {
                lacp_partner: partner.map(String::from),
                members: members
                    .iter()
                    .map(|(n, l)| MgmtMember {
                        name: n.to_string(),
                        link: l.to_string(),
                        aggregator_id: None,
                    })
                    .collect(),
                addresses: addrs.iter().map(|a| a.to_string()).collect(),
                address_interface_up: up,
                ..Default::default()
            },
            bond_modes: vec![],
            hash_policies: vec![],
        }
    }

    #[test]
    fn healthy_link_has_no_problems() {
        let v = view(
            true,
            &["10.42.0.5/24"],
            &[("eth0", "up"), ("eth1", "up")],
            Some("52:54:00:00:00:01"),
        );
        assert!(link_problems(&v).is_empty());
    }

    #[test]
    fn every_problem_is_named() {
        let v = view(
            true,
            &[],
            &[("eth0", "up"), ("eth1", "missing"), ("eth2", "down")],
            Some("00:00:00:00:00:00"),
        );
        let p = link_problems(&v).join(" | ");
        assert!(p.contains("no DHCP answer on VLAN 42"), "{p}");
        assert!(p.contains("eth1 does not exist"), "{p}");
        assert!(p.contains("eth2 link is down"), "{p}");
        assert!(p.contains("No LACP partner"), "{p}");
        let down = view(false, &[], &[("eth0", "up")], None);
        assert_eq!(link_problems(&down), vec!["bond0.42 is down.".to_string()]);
    }
}
