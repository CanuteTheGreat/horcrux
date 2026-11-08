//! Infrastructure > DHCP & DNS (`/network/dhcp`): the dnsmasq Horcrux runs for
//! the guests of a bridge / VLAN / VNet - range, gateway, DNS, static leases,
//! and the node that serves each network.
use crate::api::{
    add_dhcp_static_lease, create_dhcp_network, delete_dhcp_network, delete_dhcp_static_lease,
    dhcp_leases, list_dhcp_networks, DhcpLease, DhcpNetwork, DhcpStaticLease,
};
use leptos::*;

fn split_list(s: &str) -> Vec<String> {
    s.split([',', ' '])
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect()
}

#[component]
pub fn DhcpNetworksPage() -> impl IntoView {
    let (nets, set_nets) = create_signal(Vec::<DhcpNetwork>::new());
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let (leases, set_leases) = create_signal(None::<(String, Vec<DhcpLease>)>);
    let bridge = create_rw_signal(String::new());
    let vlan = create_rw_signal(String::new());
    let address = create_rw_signal(String::new());
    let range_start = create_rw_signal(String::new());
    let range_end = create_rw_signal(String::new());
    let gateway = create_rw_signal(String::new());
    let dns = create_rw_signal(String::new());
    let upstream = create_rw_signal(String::new());
    let domain = create_rw_signal(String::new());
    let lease_time = create_rw_signal("12h".to_string());
    let dns_enabled = create_rw_signal(true);
    let serve_node = create_rw_signal(String::new());
    let sl_net = create_rw_signal(String::new());
    let sl_mac = create_rw_signal(String::new());
    let sl_ip = create_rw_signal(String::new());
    let sl_host = create_rw_signal(String::new());

    let reload = move || {
        spawn_local(async move {
            match list_dhcp_networks().await {
                Ok(l) => set_nets.set(l),
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };
    create_effect(move |_| reload());

    let create = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let n = DhcpNetwork {
            bridge: bridge.get().trim().to_string(),
            vlan: vlan.get().trim().parse().ok(),
            address: address.get().trim().to_string(),
            range_start: range_start.get().trim().to_string(),
            range_end: range_end.get().trim().to_string(),
            gateway: Some(gateway.get().trim().to_string()).filter(|g| !g.is_empty()),
            dns_servers: split_list(&dns.get()),
            upstream_dns: split_list(&upstream.get()),
            domain: Some(domain.get().trim().to_string()).filter(|d| !d.is_empty()),
            lease_time: lease_time.get(),
            dns_enabled: dns_enabled.get(),
            enabled: true,
            serve_node: serve_node.get().trim().to_string(),
            ..Default::default()
        };
        spawn_local(async move {
            match create_dhcp_network(&n).await {
                Ok(n) => {
                    set_notice.set(Some(format!("DHCP network {} created", n.id)));
                    set_error.set(None);
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            reload();
        });
    };

    let add_static = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let id = sl_net.get();
        let l = DhcpStaticLease {
            mac: sl_mac.get().trim().to_string(),
            ip: sl_ip.get().trim().to_string(),
            hostname: Some(sl_host.get().trim().to_string()).filter(|h| !h.is_empty()),
        };
        spawn_local(async move {
            match add_dhcp_static_lease(&id, &l).await {
                Ok(_) => {
                    set_notice.set(Some(format!("Static lease {} added to {id}", l.mac)));
                    set_error.set(None);
                    sl_mac.set(String::new());
                    sl_ip.set(String::new());
                    sl_host.set(String::new());
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            reload();
        });
    };

    let remove = move |id: String| {
        if !web_sys::window()
            .and_then(|w| {
                w.confirm_with_message(&format!("Delete the DHCP network {id}?"))
                    .ok()
            })
            .unwrap_or(false)
        {
            return;
        }
        spawn_local(async move {
            match delete_dhcp_network(&id).await {
                Ok(()) => set_notice.set(Some(format!("DHCP network {id} deleted"))),
                Err(e) => set_error.set(Some(e.message)),
            }
            reload();
        });
    };
    let remove_static = move |id: String, mac: String| {
        spawn_local(async move {
            if let Err(e) = delete_dhcp_static_lease(&id, &mac).await {
                set_error.set(Some(e.message));
            }
            reload();
        });
    };
    let show_leases = move |id: String| {
        spawn_local(async move {
            match dhcp_leases(&id).await {
                Ok(l) => set_leases.set(Some((id, l))),
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };

    view! {
        <div class="dhcp-networks">
            <div class="page-header"><h1>"DHCP & DNS"</h1></div>
            <p class="form-help">
                "Horcrux runs one dnsmasq per network (a bridge, a VLAN of a VLAN-aware bridge, or an SDN VNet) to give guests an address, a gateway, DNS servers and DNS names. In a cluster only the serving node runs it; the definition is kept on every node."
            </p>
            {move || notice.get().map(|n| view! { <div class="alert alert-info" id="dhcp-notice">{n}</div> })}
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="dhcp-error">{e}</div> })}

            <form id="dhcp-create-form" class="form-row" on:submit=create>
                <input id="dhcp-bridge" type="text" required placeholder="bridge / VNet, e.g. vmbr1"
                    prop:value=bridge on:input=move |ev| bridge.set(event_target_value(&ev))/>
                <input id="dhcp-vlan" type="text" placeholder="VLAN (optional)" size="10"
                    prop:value=vlan on:input=move |ev| vlan.set(event_target_value(&ev))/>
                <input id="dhcp-address" type="text" required placeholder="server address, e.g. 192.168.20.1/24"
                    prop:value=address on:input=move |ev| address.set(event_target_value(&ev))/>
                <input id="dhcp-range-start" type="text" required placeholder="range start"
                    prop:value=range_start on:input=move |ev| range_start.set(event_target_value(&ev))/>
                <input id="dhcp-range-end" type="text" required placeholder="range end"
                    prop:value=range_end on:input=move |ev| range_end.set(event_target_value(&ev))/>
                <input id="dhcp-gateway" type="text" placeholder="gateway (optional)"
                    prop:value=gateway on:input=move |ev| gateway.set(event_target_value(&ev))/>
                <input id="dhcp-dns" type="text" placeholder="DNS servers for clients (default: this server)"
                    prop:value=dns on:input=move |ev| dns.set(event_target_value(&ev))/>
                <input id="dhcp-upstream" type="text" placeholder="upstream resolvers (default: the host's)"
                    prop:value=upstream on:input=move |ev| upstream.set(event_target_value(&ev))/>
                <input id="dhcp-domain" type="text" placeholder="domain, e.g. lab.example"
                    prop:value=domain on:input=move |ev| domain.set(event_target_value(&ev))/>
                <input id="dhcp-lease-time" type="text" placeholder="lease time" size="8"
                    prop:value=lease_time on:input=move |ev| lease_time.set(event_target_value(&ev))/>
                <input id="dhcp-serve-node" type="text" placeholder="serving node (default: this one)"
                    prop:value=serve_node on:input=move |ev| serve_node.set(event_target_value(&ev))/>
                <label>
                    <input id="dhcp-dns-enabled" type="checkbox" prop:checked=dns_enabled
                        on:change=move |ev| dns_enabled.set(event_target_checked(&ev))/>
                    " DNS"
                </label>
                <button id="dhcp-create" type="submit" class="btn btn-primary">"Create network"</button>
            </form>

            <table class="vm-table" id="dhcp-table">
                <thead>
                    <tr><th>"Id"</th><th>"Interface"</th><th>"Server"</th><th>"Range"</th><th>"Gateway"</th><th>"Domain"</th><th>"Serving node"</th><th>"State"</th><th>"Static leases"</th><th>"Actions"</th></tr>
                </thead>
                <tbody>
                    {move || {
                        let list = nets.get();
                        if list.is_empty() {
                            view! { <tr><td colspan="10" class="no-data">"No DHCP networks yet. Create one above."</td></tr> }.into_view()
                        } else {
                            list.into_iter().map(|n| {
                                let id = n.id.clone();
                                let (id_l, id_d) = (n.id.clone(), n.id.clone());
                                let state = if !n.enabled { "disabled" } else if n.running { "running" } else if n.serving_here { "not running" } else { "on another node" };
                                let statics = n.static_leases.iter().map(|l| {
                                    let (i, m) = (id.clone(), l.mac.clone());
                                    let label = format!("{} = {} {}", l.mac, l.ip, l.hostname.clone().unwrap_or_default());
                                    view! { <div class="static-lease">{label}
                                        <button class="btn btn-sm btn-danger static-lease-delete" on:click=move |_| remove_static(i.clone(), m.clone())>"x"</button></div> }
                                }).collect_view();
                                view! {
                                    <tr class="dhcp-row" data-net=n.id.clone()>
                                        <td>{n.id.clone()}</td>
                                        <td>{n.iface.clone()}</td>
                                        <td>{n.address.clone()}</td>
                                        <td>{format!("{} - {}", n.range_start, n.range_end)}</td>
                                        <td>{n.gateway.clone().unwrap_or_default()}</td>
                                        <td>{n.domain.clone().unwrap_or_default()}</td>
                                        <td>{n.serve_node.clone()}</td>
                                        <td class="dhcp-state">{state}</td>
                                        <td>{statics}</td>
                                        <td>
                                            <button class="btn btn-sm dhcp-leases" on:click=move |_| show_leases(id_l.clone())>"Leases"</button>
                                            <button class="btn btn-sm btn-danger dhcp-delete" on:click=move |_| remove(id_d.clone())>"Delete"</button>
                                        </td>
                                    </tr>
                                }
                            }).collect_view()
                        }
                    }}
                </tbody>
            </table>

            <form id="dhcp-static-form" class="form-row" on:submit=add_static>
                <select id="dhcp-static-net" required on:change=move |ev| sl_net.set(event_target_value(&ev))>
                    <option value="">"network..."</option>
                    {move || nets.get().into_iter().map(|n| view! { <option value=n.id.clone()>{n.id.clone()}</option> }).collect_view()}
                </select>
                <input id="dhcp-static-mac" type="text" required placeholder="MAC aa:bb:cc:dd:ee:ff"
                    prop:value=sl_mac on:input=move |ev| sl_mac.set(event_target_value(&ev))/>
                <input id="dhcp-static-ip" type="text" required placeholder="IP address"
                    prop:value=sl_ip on:input=move |ev| sl_ip.set(event_target_value(&ev))/>
                <input id="dhcp-static-host" type="text" placeholder="hostname (optional)"
                    prop:value=sl_host on:input=move |ev| sl_host.set(event_target_value(&ev))/>
                <button id="dhcp-static-add" type="submit" class="btn btn-primary">"Add static lease"</button>
            </form>

            {move || leases.get().map(|(id, l)| view! {
                <div id="dhcp-lease-list">
                    <h3>{format!("Active leases of {id}")}</h3>
                    <table class="vm-table">
                        <thead><tr><th>"MAC"</th><th>"IP"</th><th>"Hostname"</th><th>"Expires (unix)"</th></tr></thead>
                        <tbody>
                            {l.into_iter().map(|x| view! { <tr><td>{x.mac}</td><td>{x.ip}</td><td>{x.hostname.unwrap_or_default()}</td><td>{x.expires}</td></tr> }).collect_view()}
                        </tbody>
                    </table>
                </div>
            })}
        </div>
    }
}
