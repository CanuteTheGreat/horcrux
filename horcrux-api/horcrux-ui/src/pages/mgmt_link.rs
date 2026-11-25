//! Infrastructure > Management Link (`/network/management`): the NICs, bond
//! (link aggregation, incl. 802.3ad LACP) and VLAN tag that carry this node's
//! own address, web UI and SSH, as the installer set them up. Shows the live
//! state (bond mode, each member's link, LACP partner, addresses) and edits
//! the layout: written for the node's init system and applied at the next
//! boot, or right away when the user ticks "apply now".
use crate::api::{get_mgmt_link, set_mgmt_link, MgmtBond, MgmtRecord, MgmtView};
use leptos::*;

fn split_words(s: &str) -> Vec<String> {
    s.split([',', ' '])
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(String::from)
        .collect()
}

#[component]
pub fn MgmtLinkPage() -> impl IntoView {
    let (view_data, set_view) = create_signal(None::<MgmtView>);
    let (unavailable, set_unavailable) = create_signal(None::<crate::api::Unavailable>);
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let (loading, set_loading) = create_signal(true);
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

    let bonded = create_memo(move |_| split_words(&ifaces.get()).len() > 1);

    let save = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let list = split_words(&ifaces.get());
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
                "The network link that carries this node's own address, web UI and SSH: one NIC, or several NICs bonded into one link (failover or LACP), optionally tagged with a VLAN. The installer set it up; changes are written for this node's init system and take effect at the next boot unless you apply them now."
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
                    view! {
                        <table class="vm-table" id="mgmt-state">
                            <tbody>
                                <tr><th>"Address on"</th><td id="mgmt-l3">{r.address_interface.clone()}" "{if l.address_interface_up { "(up)" } else { "(down)" }}</td></tr>
                                <tr><th>"Addresses"</th><td id="mgmt-addresses">{if l.addresses.is_empty() { "-".to_string() } else { l.addresses.join(", ") }}</td></tr>
                                <tr><th>"Bond"</th><td id="mgmt-bond">{r.bond.as_ref().map(|b| format!("{} ({})", b.name, l.bond_mode.clone().unwrap_or_else(|| b.mode.clone()))).unwrap_or_else(|| "none (one NIC)".into())}</td></tr>
                                <tr><th>"VLAN"</th><td id="mgmt-vlan">{r.vlan.map(|v| v.to_string()).unwrap_or_else(|| "untagged".into())}</td></tr>
                                {l.lacp_partner.clone().map(|p| view! { <tr><th>"LACP partner"</th><td id="mgmt-lacp">{if p == "00:00:00:00:00:00" { "no partner answered (check the switch's LACP group)".to_string() } else { p }}</td></tr> })}
                                <tr><th>"Members"</th><td id="mgmt-members">
                                    {l.members.iter().map(|m| view! {
                                        <span class=if m.link == "up" { "badge badge-success" } else { "badge badge-danger" }>{format!("{} {}", m.name, m.link)}</span>" "
                                    }).collect_view()}
                                </td></tr>
                            </tbody>
                        </table>

                        <form id="mgmt-form" class="fields" on:submit=save>
                            <div class="form-group"><label for="mgmt-ifaces">"NICs (two or more = a bond)"</label>
                                <input id="mgmt-ifaces" type="text" required placeholder="eth0 eth1" prop:value=ifaces on:input=move |ev| ifaces.set(event_target_value(&ev))/></div>
                            <Show when=move || bonded.get()>
                                <div class="form-group"><label for="mgmt-bond-name">"Bond name"</label>
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
                                    <div class="form-group"><label for="mgmt-lacp-rate">"LACP rate"</label>
                                        <select id="mgmt-lacp-rate" on:change=move |ev| lacp.set(event_target_value(&ev))>
                                            <option value="fast" selected=move || lacp.get() == "fast">"fast (1 s)"</option>
                                            <option value="slow" selected=move || lacp.get() == "slow">"slow (30 s)"</option>
                                        </select></div>
                                </Show>
                                <div class="form-group"><label for="mgmt-miimon">"Link check interval (ms)"</label>
                                    <input id="mgmt-miimon" type="number" min="1" max="60000" prop:value=miimon on:input=move |ev| miimon.set(event_target_value(&ev))/></div>
                            </Show>
                            <div class="form-group"><label for="mgmt-vlan-id">"VLAN tag (empty = untagged)"</label>
                                <input id="mgmt-vlan-id" type="number" min="1" max="4094" placeholder="42" prop:value=vlan on:input=move |ev| vlan.set(event_target_value(&ev))/></div>
                            <div class="form-group"><label for="mgmt-mode">"Address"</label>
                                <select id="mgmt-mode" on:change=move |ev| mode.set(event_target_value(&ev))>
                                    <option value="dhcp" selected=move || mode.get() == "dhcp">"DHCP"</option>
                                    <option value="static" selected=move || mode.get() == "static">"Static"</option>
                                </select></div>
                            <Show when=move || mode.get() == "static">
                                <div class="form-group"><label for="mgmt-address">"Address / prefix"</label>
                                    <input id="mgmt-address" type="text" placeholder="192.0.2.10/24" prop:value=address on:input=move |ev| address.set(event_target_value(&ev))/></div>
                                <div class="form-group"><label for="mgmt-gateway">"Gateway"</label>
                                    <input id="mgmt-gateway" type="text" prop:value=gateway on:input=move |ev| gateway.set(event_target_value(&ev))/></div>
                                <div class="form-group"><label for="mgmt-dns">"DNS servers"</label>
                                    <input id="mgmt-dns" type="text" prop:value=dns on:input=move |ev| dns.set(event_target_value(&ev))/></div>
                            </Show>
                            <label>
                                <input id="mgmt-apply-now" type="checkbox" prop:checked=apply_now on:change=move |ev| apply_now.set(event_target_checked(&ev))/>
                                " Apply now (otherwise at the next boot)"
                            </label>
                            <button id="mgmt-save" type="submit" class="btn btn-primary">"Save management link"</button>
                        </form>
                    }.into_view()
                } else {
                    view! { <p class="no-data">"No data."</p> }.into_view()
                }
            }}
        </div>
    }
}
