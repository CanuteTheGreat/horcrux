//! Network > SDN (`/network/sdn`): zones (simple / VLAN / VXLAN), VNets (each
//! one a bridge on this node), subnets with gateway + SNAT, and IPAM.
use crate::api::sdn_zones::{self as sdn, AllocateRequest, Subnet, VNet, Zone};
use leptos::*;

fn opt(s: String) -> Option<String> {
    let t = s.trim().to_string();
    (!t.is_empty()).then_some(t)
}

#[component]
pub fn SdnZonesPage() -> impl IntoView {
    let (zones, set_zones) = create_signal(Vec::<Zone>::new());
    let (vnets, set_vnets) = create_signal(Vec::<VNet>::new());
    let (subnets, set_subnets) = create_signal(Vec::<Subnet>::new());
    let (status, set_status) = create_signal(Vec::<sdn::VNetStatus>::new());
    let (ipam, set_ipam) = create_signal(None::<sdn::IpamView>);
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);

    let z_id = create_rw_signal(String::new());
    let z_type = create_rw_signal("simple".to_string());
    let z_uplink = create_rw_signal(String::new());
    let z_local = create_rw_signal(String::new());
    let z_peers = create_rw_signal(String::new());
    let z_mtu = create_rw_signal(String::new());

    let v_id = create_rw_signal(String::new());
    let v_zone = create_rw_signal(String::new());
    let v_tag = create_rw_signal(String::new());
    let v_aware = create_rw_signal(false);

    let s_vnet = create_rw_signal(String::new());
    let s_cidr = create_rw_signal(String::new());
    let s_gw = create_rw_signal(String::new());
    let s_snat = create_rw_signal(false);

    let a_host = create_rw_signal(String::new());
    let a_ip = create_rw_signal(String::new());
    let a_mac = create_rw_signal(String::new());

    let reload = move || {
        spawn_local(async move {
            if let Ok(z) = sdn::list_zones().await {
                set_zones.set(z);
            }
            if let Ok(v) = sdn::list_vnets().await {
                set_vnets.set(v);
            }
            if let Ok(s) = sdn::list_subnets().await {
                set_subnets.set(s);
            }
            if let Ok(s) = sdn::sdn_status().await {
                set_status.set(s);
            }
            if let Some(cur) = ipam.get_untracked() {
                if let Ok(v) = sdn::get_ipam(&cur.subnet).await {
                    set_ipam.set(Some(v));
                } else {
                    set_ipam.set(None);
                }
            }
        });
    };
    create_effect(move |_| reload());

    let fail = move |e: crate::api::ApiError| {
        set_notice.set(None);
        set_error.set(Some(e.message));
    };
    let ok = move |m: String| {
        set_error.set(None);
        set_notice.set(Some(m));
    };

    let add_zone = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let kind = z_type.get();
        let z = Zone {
            id: z_id.get().trim().to_string(),
            zone_type: kind.clone(),
            description: String::new(),
            uplink: if kind == "vlan" {
                opt(z_uplink.get())
            } else {
                None
            },
            local_ip: if kind == "vxlan" {
                opt(z_local.get())
            } else {
                None
            },
            peers: if kind == "vxlan" {
                z_peers
                    .get()
                    .split([',', ' '])
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .map(String::from)
                    .collect()
            } else {
                vec![]
            },
            mtu: z_mtu.get().trim().parse().ok(),
            dst_port: None,
        };
        spawn_local(async move {
            match sdn::create_zone(&z).await {
                Ok(z) => ok(format!("Zone {} created", z.id)),
                Err(e) => fail(e),
            }
            reload();
        });
    };
    let add_vnet = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let v = VNet {
            id: v_id.get().trim().to_string(),
            zone: v_zone.get(),
            alias: None,
            tag: v_tag.get().trim().parse().ok(),
            vlan_aware: v_aware.get(),
        };
        spawn_local(async move {
            match sdn::create_vnet(&v).await {
                Ok(v) => ok(format!(
                    "VNet {} created: bridge {} exists on this node",
                    v.id, v.id
                )),
                Err(e) => fail(e),
            }
            reload();
        });
    };
    let add_subnet = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let vnet = s_vnet.get();
        let s = Subnet {
            id: String::new(),
            vnet: vnet.clone(),
            cidr: s_cidr.get().trim().to_string(),
            gateway: opt(s_gw.get()),
            snat: s_snat.get(),
        };
        spawn_local(async move {
            match sdn::create_subnet(&vnet, &s).await {
                Ok(s) => ok(format!("Subnet {} added to {}", s.cidr, s.vnet)),
                Err(e) => fail(e),
            }
            reload();
        });
    };
    let del_zone = move |id: String| {
        spawn_local(async move {
            match sdn::delete_zone(&id).await {
                Ok(()) => ok(format!("Zone {id} deleted")),
                Err(e) => fail(e),
            }
            reload();
        });
    };
    let del_vnet = move |id: String| {
        spawn_local(async move {
            match sdn::delete_vnet(&id).await {
                Ok(()) => ok(format!("VNet {id} deleted (bridge removed)")),
                Err(e) => fail(e),
            }
            reload();
        });
    };
    let del_subnet = move |id: String| {
        spawn_local(async move {
            match sdn::delete_subnet(&id).await {
                Ok(()) => ok(format!("Subnet {id} deleted")),
                Err(e) => fail(e),
            }
            set_ipam.set(None);
            reload();
        });
    };
    let open_ipam = move |id: String| {
        spawn_local(async move {
            match sdn::get_ipam(&id).await {
                Ok(v) => set_ipam.set(Some(v)),
                Err(e) => fail(e),
            }
        });
    };
    let allocate = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let Some(cur) = ipam.get_untracked() else {
            return;
        };
        let r = AllocateRequest {
            ip: opt(a_ip.get()),
            hostname: opt(a_host.get()),
            mac: opt(a_mac.get()),
        };
        spawn_local(async move {
            match sdn::allocate_ip(&cur.subnet, &r).await {
                Ok(a) => ok(format!("{} allocated", a.ip)),
                Err(e) => fail(e),
            }
            reload();
        });
    };
    let release = move |ip: String| {
        let Some(cur) = ipam.get_untracked() else {
            return;
        };
        spawn_local(async move {
            match sdn::release_ip(&cur.subnet, &ip).await {
                Ok(()) => ok(format!("{ip} released")),
                Err(e) => fail(e),
            }
            reload();
        });
    };

    view! {
        <div class="sdn-zones">
            <div class="page-header"><h1>"SDN: Zones, VNets, Subnets, IPAM"</h1></div>
            <p class="form-help">
                "A zone says how a VNet reaches the wire (simple: isolated bridge, VLAN: a tagged sub-interface of an uplink, VXLAN: a tunnel to the zone's peer nodes). Each VNet becomes a Linux bridge on this node that VM and container network cards attach to; a subnet puts the gateway address on it and can masquerade (SNAT) traffic out. Configuration is per node and is re-applied when Horcrux starts."
            </p>
            {move || notice.get().map(|n| view! { <div class="alert alert-info" id="sdn-notice">{n}</div> })}
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="sdn-error">{e}</div> })}

            <h2>"Zones"</h2>
            <form id="zone-form" class="form-row" on:submit=add_zone>
                <input id="zone-id" type="text" required placeholder="zone id, e.g. lab" prop:value=z_id
                    on:input=move |ev| z_id.set(event_target_value(&ev))/>
                <select id="zone-type" prop:value=z_type on:change=move |ev| z_type.set(event_target_value(&ev))>
                    <option value="simple">"Simple (isolated bridge)"</option>
                    <option value="vlan">"VLAN"</option>
                    <option value="vxlan">"VXLAN"</option>
                </select>
                <input id="zone-uplink" type="text" placeholder="VLAN: uplink, e.g. eth0" prop:value=z_uplink
                    on:input=move |ev| z_uplink.set(event_target_value(&ev))/>
                <input id="zone-local" type="text" placeholder="VXLAN: this node's address" prop:value=z_local
                    on:input=move |ev| z_local.set(event_target_value(&ev))/>
                <input id="zone-peers" type="text" placeholder="VXLAN: peer addresses, comma separated" prop:value=z_peers
                    on:input=move |ev| z_peers.set(event_target_value(&ev))/>
                <input id="zone-mtu" type="text" placeholder="MTU (optional)" prop:value=z_mtu
                    on:input=move |ev| z_mtu.set(event_target_value(&ev))/>
                <button id="zone-create" type="submit" class="btn btn-primary">"Add zone"</button>
            </form>
            <table class="vm-table" id="zone-table">
                <thead><tr><th>"Zone"</th><th>"Type"</th><th>"Uplink"</th><th>"Local"</th><th>"Peers"</th><th>"MTU"</th><th></th></tr></thead>
                <tbody>
                    {move || zones.get().into_iter().map(|z| {
                        let id = z.id.clone();
                        view! {
                            <tr class="zone-row">
                                <td>{z.id.clone()}</td><td>{z.zone_type.clone()}</td>
                                <td>{z.uplink.clone().unwrap_or_default()}</td>
                                <td>{z.local_ip.clone().unwrap_or_default()}</td>
                                <td>{z.peers.join(", ")}</td>
                                <td>{z.mtu.map(|m| m.to_string()).unwrap_or_default()}</td>
                                <td><button class="btn btn-sm btn-danger zone-delete" on:click=move |_| del_zone(id.clone())>"Delete"</button></td>
                            </tr>
                        }
                    }).collect_view()}
                </tbody>
            </table>

            <h2>"VNets"</h2>
            <form id="vnet-form" class="form-row" on:submit=add_vnet>
                <input id="vnet-id" type="text" required maxlength="8" placeholder="vnet id (bridge name), e.g. web" prop:value=v_id
                    on:input=move |ev| v_id.set(event_target_value(&ev))/>
                <select id="vnet-zone" prop:value=v_zone on:change=move |ev| v_zone.set(event_target_value(&ev))>
                    <option value="">"zone..."</option>
                    {move || zones.get().into_iter().map(|z| view! { <option value=z.id.clone()>{z.id.clone()}</option> }).collect_view()}
                </select>
                <input id="vnet-tag" type="text" placeholder="VLAN id / VNI (VLAN and VXLAN zones)" prop:value=v_tag
                    on:input=move |ev| v_tag.set(event_target_value(&ev))/>
                <label><input id="vnet-aware" type="checkbox" prop:checked=v_aware
                    on:change=move |ev| v_aware.set(event_target_checked(&ev))/>" VLAN-aware"</label>
                <button id="vnet-create" type="submit" class="btn btn-primary">"Add VNet"</button>
            </form>
            <table class="vm-table" id="vnet-table">
                <thead><tr><th>"VNet"</th><th>"Zone"</th><th>"Tag"</th><th>"Bridge"</th><th>"Carrier"</th><th>"Addresses"</th><th>"Ports"</th><th></th></tr></thead>
                <tbody>
                    {move || vnets.get().into_iter().map(|v| {
                        let id = v.id.clone();
                        let st = status.get().into_iter().find(|s| s.vnet == v.id);
                        let (bridge, carrier, addrs, ports) = match st {
                            Some(s) => (
                                if s.bridge_present { if s.up { "up" } else { "down" } } else { "missing" }.to_string(),
                                match (&s.carrier, s.carrier_present) {
                                    (Some(c), true) => format!("{c} (ok)"),
                                    (Some(c), false) => format!("{c} (missing)"),
                                    _ => "-".into(),
                                },
                                s.addresses.join(", "),
                                s.ports.join(", "),
                            ),
                            None => ("?".into(), "-".into(), String::new(), String::new()),
                        };
                        view! {
                            <tr class="vnet-row">
                                <td>{v.id.clone()}</td><td>{v.zone.clone()}</td>
                                <td>{v.tag.map(|t| t.to_string()).unwrap_or_default()}</td>
                                <td>{bridge}</td><td>{carrier}</td><td>{addrs}</td><td>{ports}</td>
                                <td><button class="btn btn-sm btn-danger vnet-delete" on:click=move |_| del_vnet(id.clone())>"Delete"</button></td>
                            </tr>
                        }
                    }).collect_view()}
                </tbody>
            </table>

            <h2>"Subnets"</h2>
            <form id="subnet-form" class="form-row" on:submit=add_subnet>
                <select id="subnet-vnet" prop:value=s_vnet on:change=move |ev| s_vnet.set(event_target_value(&ev))>
                    <option value="">"vnet..."</option>
                    {move || vnets.get().into_iter().map(|v| view! { <option value=v.id.clone()>{v.id.clone()}</option> }).collect_view()}
                </select>
                <input id="subnet-cidr" type="text" required placeholder="10.20.0.0/24" prop:value=s_cidr
                    on:input=move |ev| s_cidr.set(event_target_value(&ev))/>
                <input id="subnet-gw" type="text" placeholder="gateway, e.g. 10.20.0.1" prop:value=s_gw
                    on:input=move |ev| s_gw.set(event_target_value(&ev))/>
                <label><input id="subnet-snat" type="checkbox" prop:checked=s_snat
                    on:change=move |ev| s_snat.set(event_target_checked(&ev))/>" SNAT (masquerade out)"</label>
                <button id="subnet-create" type="submit" class="btn btn-primary">"Add subnet"</button>
            </form>
            <table class="vm-table" id="subnet-table">
                <thead><tr><th>"Subnet"</th><th>"VNet"</th><th>"Gateway"</th><th>"SNAT"</th><th></th></tr></thead>
                <tbody>
                    {move || subnets.get().into_iter().map(|s| {
                        let id = s.id.clone();
                        let id2 = s.id.clone();
                        view! {
                            <tr class="subnet-row">
                                <td>{s.cidr.clone()}</td><td>{s.vnet.clone()}</td>
                                <td>{s.gateway.clone().unwrap_or_default()}</td>
                                <td>{if s.snat { "yes" } else { "no" }}</td>
                                <td>
                                    <button class="btn btn-sm subnet-ipam" on:click=move |_| open_ipam(id.clone())>"IPAM"</button>
                                    <button class="btn btn-sm btn-danger subnet-delete" on:click=move |_| del_subnet(id2.clone())>"Delete"</button>
                                </td>
                            </tr>
                        }
                    }).collect_view()}
                </tbody>
            </table>

            {move || ipam.get().map(|v| {
                view! {
                    <div id="ipam-panel">
                        <h2>"IPAM: " {v.subnet.clone()}</h2>
                        <p>{format!("{} of {} host addresses allocated, {} free", v.used, v.total, v.free)}</p>
                        <form id="ipam-form" class="form-row" on:submit=allocate>
                            <input id="ipam-host" type="text" placeholder="host name" prop:value=a_host
                                on:input=move |ev| a_host.set(event_target_value(&ev))/>
                            <input id="ipam-ip" type="text" placeholder="address (empty = next free)" prop:value=a_ip
                                on:input=move |ev| a_ip.set(event_target_value(&ev))/>
                            <input id="ipam-mac" type="text" placeholder="MAC (optional)" prop:value=a_mac
                                on:input=move |ev| a_mac.set(event_target_value(&ev))/>
                            <button id="ipam-allocate" type="submit" class="btn btn-primary">"Allocate"</button>
                        </form>
                        <table class="vm-table" id="ipam-table">
                            <thead><tr><th>"Address"</th><th>"Host"</th><th>"MAC"</th><th>"Kind"</th><th></th></tr></thead>
                            <tbody>
                                {v.allocations.into_iter().map(|a| {
                                    let ip = a.ip.clone();
                                    let gw = a.kind == "gateway";
                                    view! {
                                        <tr class="ipam-row">
                                            <td>{a.ip.clone()}</td>
                                            <td>{a.hostname.clone().unwrap_or_default()}</td>
                                            <td>{a.mac.clone().unwrap_or_default()}</td>
                                            <td>{a.kind.clone()}</td>
                                            <td><button class="btn btn-sm btn-danger ipam-release" disabled=gw on:click=move |_| release(ip.clone())>"Release"</button></td>
                                        </tr>
                                    }
                                }).collect_view()}
                            </tbody>
                        </table>
                    </div>
                }
            })}
        </div>
    }
}
