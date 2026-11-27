//! Infrastructure > Host Bridges (`/network/bridges`): the Linux bridges VM
//! network cards attach to; create and delete Horcrux-managed ones.
use crate::api::{
    create_bond, create_bridge, create_vlan, delete_bond, delete_bridge, delete_vlan, list_bonds,
    list_bridges, list_host_interfaces, list_vlans, set_bridge_trunk_vlans, update_bridge,
    BridgeInfo, HostInterface, ManagedBond, ManagedVlan, NewBridge,
};
use leptos::*;

#[component]
pub fn HostBridgesPage() -> impl IntoView {
    let (bridges, set_bridges) = create_signal(Vec::<BridgeInfo>::new());
    let (ifaces, set_ifaces) = create_signal(Vec::<HostInterface>::new());
    let (vlans, set_vlans) = create_signal(Vec::<ManagedVlan>::new());
    let (bonds, set_bonds) = create_signal(Vec::<ManagedBond>::new());
    // Inline edit of a managed bridge: Some(name) while its row is open.
    let editing = create_rw_signal(None::<String>);
    let ed_addr = create_rw_signal(String::new());
    let ed_ports = create_rw_signal(String::new());
    let ed_comment = create_rw_signal(String::new());
    let bond_name = create_rw_signal(String::new());
    let bond_mode = create_rw_signal("active-backup".to_string());
    let bond_slaves = create_rw_signal(String::new());
    let bond_addr = create_rw_signal(String::new());
    let vlan_parent = create_rw_signal(String::new());
    let vlan_id = create_rw_signal(String::new());
    let vlan_name = create_rw_signal(String::new());
    let vlan_addr = create_rw_signal(String::new());
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let name = create_rw_signal(String::new());
    let vlan_aware = create_rw_signal(true);
    let address = create_rw_signal(String::new());
    let ports = create_rw_signal(String::new());
    let trunk = create_rw_signal(String::new());

    let reload = move || {
        spawn_local(async move {
            match list_bridges().await {
                Ok(l) => set_bridges.set(l),
                Err(e) => set_error.set(Some(e.message)),
            }
            // Older nodes lack the route: the table just stays empty.
            if let Ok(l) = list_host_interfaces().await {
                set_ifaces.set(l);
            }
            if let Ok(l) = list_vlans().await {
                set_vlans.set(l);
            }
            if let Ok(l) = list_bonds().await {
                set_bonds.set(l);
            }
        });
    };
    create_effect(move |_| reload());

    let create = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let b = NewBridge {
            name: name.get().trim().to_string(),
            vlan_aware: vlan_aware.get(),
            address: Some(address.get().trim().to_string()).filter(|a| !a.is_empty()),
            ports: ports
                .get()
                .split([',', ' '])
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(String::from)
                .collect(),
            trunk_vlans: split_list(&trunk.get()),
            comment: None,
        };
        spawn_local(async move {
            match create_bridge(&b).await {
                Ok(b) => {
                    set_notice.set(Some(format!("Bridge {} created", b.name)));
                    set_error.set(None);
                    name.set(String::new());
                    address.set(String::new());
                    ports.set(String::new());
                    trunk.set(String::new());
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            reload();
        });
    };

    let edit_trunk = move |n: String, current: String| {
        let Some(v) = web_sys::window().and_then(|w| {
            w.prompt_with_message_and_default(
                &format!("Trunk VLANs on the ports of {n} (comma separated ids or ranges, e.g. 20,30,100-110; empty = none)"),
                &current,
            )
            .ok()
            .flatten()
        }) else {
            return;
        };
        spawn_local(async move {
            match set_bridge_trunk_vlans(&n, &split_list(&v)).await {
                Ok(_) => {
                    set_notice.set(Some(format!("Trunk VLANs of {n} updated")));
                    set_error.set(None);
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            reload();
        });
    };

    let create_vlan_ev = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let Ok(id) = vlan_id.get().trim().parse::<u16>() else {
            set_error.set(Some("VLAN id must be a number from 1 to 4094".into()));
            return;
        };
        let v = ManagedVlan {
            name: vlan_name.get().trim().to_string(),
            parent: vlan_parent.get(),
            id,
            address: Some(vlan_addr.get().trim().to_string()).filter(|a| !a.is_empty()),
            comment: None,
        };
        spawn_local(async move {
            match create_vlan(&v).await {
                Ok(v) => {
                    set_notice.set(Some(format!("VLAN interface {} created", v.name)));
                    set_error.set(None);
                    vlan_id.set(String::new());
                    vlan_name.set(String::new());
                    vlan_addr.set(String::new());
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            reload();
        });
    };

    let create_bond_ev = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let b = ManagedBond {
            name: bond_name.get().trim().to_string(),
            mode: bond_mode.get(),
            slaves: split_list(&bond_slaves.get()),
            miimon: 100,
            xmit_hash_policy: None,
            address: Some(bond_addr.get().trim().to_string()).filter(|a| !a.is_empty()),
            comment: None,
        };
        spawn_local(async move {
            match create_bond(&b).await {
                Ok(b) => {
                    set_notice.set(Some(format!(
                        "Bond {} created ({})",
                        b.name,
                        b.slaves.join(", ")
                    )));
                    set_error.set(None);
                    bond_name.set(String::new());
                    bond_slaves.set(String::new());
                    bond_addr.set(String::new());
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            reload();
        });
    };

    let remove_bond = move |n: String| {
        if !web_sys::window()
            .and_then(|w| {
                w.confirm_with_message(&format!(
                    "Delete bond {n}? Its member interfaces are released."
                ))
                .ok()
            })
            .unwrap_or(false)
        {
            return;
        }
        spawn_local(async move {
            match delete_bond(&n).await {
                Ok(()) => set_notice.set(Some(format!("Bond {n} deleted"))),
                Err(e) => set_error.set(Some(e.message)),
            }
            reload();
        });
    };

    let save_edit = move |_| {
        let Some(n) = editing.get() else { return };
        let (a, p, c) = (ed_addr.get(), split_list(&ed_ports.get()), ed_comment.get());
        spawn_local(async move {
            match update_bridge(&n, a.trim(), &p, c.trim()).await {
                Ok(_) => {
                    set_notice.set(Some(format!("Bridge {n} updated")));
                    set_error.set(None);
                    editing.set(None);
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            reload();
        });
    };

    let remove_vlan = move |n: String| {
        if !web_sys::window()
            .and_then(|w| {
                w.confirm_with_message(&format!("Delete VLAN interface {n}?"))
                    .ok()
            })
            .unwrap_or(false)
        {
            return;
        }
        spawn_local(async move {
            match delete_vlan(&n).await {
                Ok(()) => set_notice.set(Some(format!("VLAN interface {n} deleted"))),
                Err(e) => set_error.set(Some(e.message)),
            }
            reload();
        });
    };

    let remove = move |n: String| {
        if !web_sys::window()
            .and_then(|w| w.confirm_with_message(&format!("Delete bridge {n}?")).ok())
            .unwrap_or(false)
        {
            return;
        }
        spawn_local(async move {
            match delete_bridge(&n).await {
                Ok(()) => set_notice.set(Some(format!("Bridge {n} deleted"))),
                Err(e) => set_error.set(Some(e.message)),
            }
            reload();
        });
    };

    view! {
        <div class="host-bridges">
            <div class="page-header"><h1>"Host Bridges"</h1></div>
            <p class="form-help">
                "VM network cards are plugged into a Linux bridge on this host. A bridge with an address gives VMs a network with the host; add a port (a NIC without an IP address) to connect it to a physical network. Bridges created here are re-created when Horcrux starts."
            </p>
            {move || notice.get().map(|n| view! { <div class="alert alert-info" id="bridge-notice">{n}</div> })}
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="bridge-error">{e}</div> })}

            <form id="bridge-create-form" class="form-row fields" on:submit=create>
                <div class="form-group"><label for="bridge-name">"Bridge name"</label><input id="bridge-name" type="text" required placeholder="vmbr0" maxlength="15"
                    prop:value=name on:input=move |ev| name.set(event_target_value(&ev))/></div>
                <div class="form-group"><label for="bridge-address">"Host address (optional)"</label><input id="bridge-address" type="text" placeholder="10.10.0.1/24"
                    prop:value=address on:input=move |ev| address.set(event_target_value(&ev))/></div>
                <div class="form-group"><label for="bridge-ports">"Ports (optional)"</label><input id="bridge-ports" type="text" placeholder="eth1"
                    prop:value=ports on:input=move |ev| ports.set(event_target_value(&ev))/>
                    {move || {
                        let free: Vec<String> = ifaces.get().into_iter().filter(|i| i.bridge_port_ok).map(|i| i.name).collect();
                        if free.is_empty() {
                            view! { <small class="form-help" id="bridge-ports-none">"No free wired interface (each one carries a host address or is already in a bridge): leave empty for a host-only bridge."</small> }.into_view()
                        } else {
                            view! { <div class="bridge-port-picks" id="bridge-port-picks">
                                <small class="form-help">"Free interfaces: "</small>
                                {free.into_iter().map(|n| {
                                    let n2 = n.clone();
                                    let on = move || split_list(&ports.get()).contains(&n2);
                                    let n3 = n.clone();
                                    view! { <button type="button" class="btn btn-sm bridge-port-pick" class:btn-primary=on data-port=n.clone()
                                        on:click=move |_| ports.set(toggle_port(&ports.get(), &n3))>{n.clone()}</button> }
                                }).collect_view()}
                            </div> }.into_view()
                        }
                    }}
                </div>
                <div class="form-group"><label for="bridge-trunk">"Trunk VLANs (optional)"</label><input id="bridge-trunk" type="text" placeholder="20,30,100-110"
                    prop:value=trunk on:input=move |ev| trunk.set(event_target_value(&ev))/></div>
                <label>
                    <input id="bridge-vlan-aware" type="checkbox" prop:checked=vlan_aware
                        on:change=move |ev| vlan_aware.set(event_target_checked(&ev))/>
                    " VLAN-aware"
                </label>
                <button id="bridge-create" type="submit" class="btn btn-primary">"Create bridge"</button>
            </form>

            <table class="vm-table" id="bridge-table">
                <thead>
                    <tr><th>"Name"</th><th>"State"</th><th>"VLAN-aware"</th><th>"Ports"</th><th>"Trunk VLANs"</th><th>"Addresses"</th><th>"Managed"</th><th>"Actions"</th></tr>
                </thead>
                <tbody>
                    {move || {
                        let list = bridges.get();
                        if list.is_empty() {
                            // One way in: a text link into the form above (a second button repeated it).
                            view! { <tr><td colspan="8" class="no-data" id="bridges-empty">"No bridges on this host yet. "
                                <a href="#bridge-name" on:click=move |ev: ev::MouseEvent| {
                                    ev.prevent_default();
                                    crate::components::focus::focus_id("bridge-name");
                                }>"Create one"</a>" with the form above."</td></tr> }.into_view()
                        } else {
                            list.into_iter().map(|b| {
                                let n = b.name.clone();
                                let managed = b.managed;
                                let vlans_text = b
                                    .port_vlans
                                    .iter()
                                    .map(|(p, v)| format!("{p}: {}", v.join(",")))
                                    .collect::<Vec<_>>()
                                    .join("; ");
                                let first_ports = b
                                    .port_vlans
                                    .values()
                                    .next()
                                    .map(|v| v.join(","))
                                    .unwrap_or_default();
                                let n2 = b.name.clone();
                                let n3 = b.name.clone();
                                let n4 = b.name.clone();
                                let cur_addr = b.addresses.first().cloned().unwrap_or_default();
                                let cur_ports = b.ports.iter().filter(|p| !p.starts_with("tap") && !p.starts_with("veth")).cloned().collect::<Vec<_>>().join(", ");
                                let cur_comment = b.comment.clone().unwrap_or_default();
                                let aware = b.vlan_aware;
                                view! {
                                    <tr class="bridge-row" data-bridge=b.name.clone()>
                                        <td>{b.name.clone()}</td>
                                        <td>{if b.up { "up" } else { "down" }}</td>
                                        <td>{if b.vlan_aware { "yes" } else { "no" }}</td>
                                        <td>{b.ports.join(", ")}</td>
                                        <td class="bridge-vlans">{vlans_text}</td>
                                        <td>{b.addresses.join(", ")}</td>
                                        <td>{if managed { "Horcrux" } else { "host config" }}</td>
                                        <td>
                                            <button class="btn btn-sm bridge-edit" disabled=!managed
                                                title=if managed { "Change the address, ports or comment" } else { "Part of the host's own network configuration" }
                                                on:click=move |_| {
                                                    ed_addr.set(cur_addr.clone());
                                                    ed_ports.set(cur_ports.clone());
                                                    ed_comment.set(cur_comment.clone());
                                                    editing.set(Some(n3.clone()));
                                                }>"Edit"</button>
                                            <button class="btn btn-sm bridge-trunk-edit" disabled=!(managed && aware)
                                                title="Set the VLAN ids carried on this bridge's uplink ports"
                                                on:click=move |_| edit_trunk(n2.clone(), first_ports.clone())>"Trunk VLANs"</button>
                                            <button class="btn btn-sm btn-danger bridge-delete" disabled=!managed
                                                title=if managed { "" } else { "Part of the host's own network configuration" }
                                                on:click=move |_| remove(n.clone())>"Delete"</button>
                                        </td>
                                    </tr>
                                    {move || (editing.get().as_deref() == Some(n4.as_str())).then(|| view! {
                                        <tr class="bridge-edit-row"><td colspan="8">
                                            <div class="form-row fields">
                                                <div class="form-group"><label for="bridge-edit-address">"Host address"</label><input id="bridge-edit-address" type="text" placeholder="none"
                                                    prop:value=ed_addr on:input=move |ev| ed_addr.set(event_target_value(&ev))/></div>
                                                <div class="form-group"><label for="bridge-edit-ports">"Ports"</label><input id="bridge-edit-ports" type="text" placeholder="none"
                                                    prop:value=ed_ports on:input=move |ev| ed_ports.set(event_target_value(&ev))/></div>
                                                <div class="form-group"><label for="bridge-edit-comment">"Comment"</label><input id="bridge-edit-comment" type="text"
                                                    prop:value=ed_comment on:input=move |ev| ed_comment.set(event_target_value(&ev))/></div>
                                                <button id="bridge-edit-save" class="btn btn-primary" on:click=save_edit>"Save"</button>
                                                <button class="btn" on:click=move |_| editing.set(None)>"Cancel"</button>
                                            </div>
                                            <small class="form-help">"Changes apply to the running bridge at once; the new address is added before the old one is removed."</small>
                                        </td></tr>
                                    })}
                                }
                            }).collect_view()
                        }
                    }}
                </tbody>
            </table>

            <h2>"Network interfaces"</h2>
            <form id="bond-create-form" class="form-row fields" on:submit=create_bond_ev>
                <div class="form-group"><label for="bond-name">"Bond name"</label><input id="bond-name" type="text" required maxlength="15" placeholder="bond0"
                    prop:value=bond_name on:input=move |ev| bond_name.set(event_target_value(&ev))/></div>
                <div class="form-group"><label for="bond-mode">"Mode"</label>
                    <select id="bond-mode" prop:value=bond_mode on:change=move |ev| bond_mode.set(event_target_value(&ev))>
                        {BOND_MODES.iter().map(|(v, l)| view! { <option value=*v selected=move || bond_mode.get() == *v>{*l}</option> }).collect_view()}
                    </select></div>
                <div class="form-group"><label for="bond-slaves">"Members"</label><input id="bond-slaves" type="text" required placeholder="eno1, eno2"
                    prop:value=bond_slaves on:input=move |ev| bond_slaves.set(event_target_value(&ev))/>
                    <div class="bridge-port-picks">
                        {move || ifaces.get().into_iter().filter(|i| i.bridge_port_ok && i.kind == "physical").map(|i| {
                            let n = i.name.clone();
                            let n2 = n.clone();
                            let on = move || split_list(&bond_slaves.get()).contains(&n2);
                            view! { <button type="button" class="btn btn-sm bond-slave-pick" class:btn-primary=on
                                on:click=move |_| bond_slaves.set(toggle_port(&bond_slaves.get(), &n))>{i.name.clone()}</button> }
                        }).collect_view()}
                    </div></div>
                <div class="form-group"><label for="bond-address">"Host address (optional)"</label><input id="bond-address" type="text" placeholder="10.30.0.1/24"
                    prop:value=bond_addr on:input=move |ev| bond_addr.set(event_target_value(&ev))/></div>
                <button id="bond-create" type="submit" class="btn btn-primary">"Create bond"</button>
            </form>
            <form id="vlan-create-form" class="form-row fields" on:submit=create_vlan_ev>
                <div class="form-group"><label for="vlan-parent">"VLAN on"</label>
                    <select id="vlan-parent" required prop:value=vlan_parent on:change=move |ev| vlan_parent.set(event_target_value(&ev))>
                        <option value="">"Choose an interface..."</option>
                        {move || ifaces.get().into_iter()
                            .filter(|i| matches!(i.kind.as_str(), "physical" | "bond" | "bridge"))
                            .map(|i| view! { <option value=i.name.clone()>{i.name.clone()}</option> })
                            .collect_view()}
                    </select></div>
                <div class="form-group"><label for="vlan-id">"VLAN id"</label><input id="vlan-id" type="number" min="1" max="4094" required placeholder="20"
                    prop:value=vlan_id on:input=move |ev| vlan_id.set(event_target_value(&ev))/></div>
                <div class="form-group"><label for="vlan-name">"Name (optional)"</label><input id="vlan-name" type="text" maxlength="15"
                    placeholder=move || if vlan_parent.get().is_empty() { "eno1.20".to_string() } else { format!("{}.{}", vlan_parent.get(), if vlan_id.get().is_empty() { "20".to_string() } else { vlan_id.get() }) }
                    prop:value=vlan_name on:input=move |ev| vlan_name.set(event_target_value(&ev))/></div>
                <div class="form-group"><label for="vlan-address">"Host address (optional)"</label><input id="vlan-address" type="text" placeholder="10.20.0.1/24"
                    prop:value=vlan_addr on:input=move |ev| vlan_addr.set(event_target_value(&ev))/></div>
                <button id="vlan-create" type="submit" class="btn btn-primary">"Create VLAN interface"</button>
            </form>
            <p class="form-help">"Every interface of this host. Wired NICs, bonds and VLANs without an address can be a bridge port (pick them in the form above); guest ports (tap/veth) are not listed."</p>
            <table class="vm-table" id="iface-table">
                <thead>
                    <tr><th>"Name"</th><th>"Type"</th><th>"State"</th><th>"MAC"</th><th>"Speed"</th><th>"MTU"</th><th>"Addresses"</th><th>"Member of"</th><th>"Bridge port"</th><th>"Actions"</th></tr>
                </thead>
                <tbody>
                    {move || {
                        let list = ifaces.get();
                        if list.is_empty() {
                            view! { <tr><td colspan="10" class="no-data">"Loading interfaces..."</td></tr> }.into_view()
                        } else {
                            let managed: Vec<String> = vlans.get().into_iter().map(|v| v.name).collect();
                            let managed_bonds: Vec<String> = bonds.get().into_iter().map(|b| b.name).collect();
                            list.into_iter().map(|i| {
                                let is_managed = managed.contains(&i.name);
                                let is_bond = managed_bonds.contains(&i.name);
                                let bond_del = i.name.clone();
                                let del_name = i.name.clone();
                                let state = iface_state(&i);
                                let state_cls = if i.state == "up" && i.carrier { "status-running" } else if i.state == "down" { "status-stopped" } else { "status-unknown" };
                                view! {
                                    <tr class="iface-row" data-iface=i.name.clone()>
                                        <td><strong>{i.name.clone()}</strong></td>
                                        <td>{iface_kind_label(&i)}</td>
                                        <td><span class=state_cls>{state}</span></td>
                                        <td><code>{i.mac.clone().unwrap_or_default()}</code></td>
                                        <td>{i.speed_mbps.map(speed_label).unwrap_or_default()}</td>
                                        <td>{i.mtu}</td>
                                        <td>{i.addresses.join(", ")}</td>
                                        <td>{i.master.clone().unwrap_or_default()}</td>
                                        <td title=i.bridge_port_note.clone().unwrap_or_default()>
                                            {if i.bridge_port_ok { "available".to_string() } else { i.bridge_port_note.clone().unwrap_or_else(|| "no".into()) }}
                                        </td>
                                        <td>{is_managed.then(|| view! {
                                            <button class="btn btn-sm btn-danger vlan-delete" on:click=move |_| remove_vlan(del_name.clone())>"Delete"</button>
                                        })}{is_bond.then(|| view! {
                                            <button class="btn btn-sm btn-danger bond-delete" on:click=move |_| remove_bond(bond_del.clone())>"Delete"</button>
                                        })}</td>
                                    </tr>
                                }
                            }).collect_view()
                        }
                    }}
                </tbody>
            </table>
        </div>
    }
}

/// Bond modes (value, label) in the order Proxmox lists them.
const BOND_MODES: [(&str, &str); 7] = [
    ("balance-rr", "balance-rr (round robin)"),
    ("active-backup", "active-backup (any switch)"),
    ("balance-xor", "balance-xor"),
    ("broadcast", "broadcast"),
    ("802.3ad", "802.3ad (LACP, switch support needed)"),
    ("balance-tlb", "balance-tlb"),
    ("balance-alb", "balance-alb"),
];

/// Add `port` to the comma separated `list`, or remove it if present.
fn toggle_port(list: &str, port: &str) -> String {
    let mut v = split_list(list);
    if let Some(i) = v.iter().position(|p| p == port) {
        v.remove(i);
    } else {
        v.push(port.to_string());
    }
    v.join(", ")
}

fn iface_kind_label(i: &HostInterface) -> String {
    match i.kind.as_str() {
        "physical" => "Network card".into(),
        "wireless" => "Wi-Fi".into(),
        "bond" => "Bond".into(),
        "vlan" => match (&i.vlan_parent, i.vlan_id) {
            (Some(p), Some(id)) => format!("VLAN {id} on {p}"),
            _ => "VLAN".into(),
        },
        "bridge" => "Bridge".into(),
        "wireguard" => "WireGuard".into(),
        other => other.to_string(),
    }
}

fn iface_state(i: &HostInterface) -> String {
    match (i.state.as_str(), i.carrier) {
        ("up", true) | ("unknown", true) => "up".into(),
        ("down", _) => "down".into(),
        (_, false) => "no link".into(),
        (s, _) => s.to_string(),
    }
}

fn speed_label(mbps: u32) -> String {
    if mbps >= 1000 && mbps.is_multiple_of(1000) {
        format!("{} Gb/s", mbps / 1000)
    } else {
        format!("{mbps} Mb/s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_picks_toggle_and_labels() {
        assert_eq!(toggle_port("", "eth1"), "eth1");
        assert_eq!(toggle_port("eth1, eth2", "eth1"), "eth2");
        assert_eq!(toggle_port("eth2", "eth1"), "eth2, eth1");
        assert_eq!(speed_label(2500), "2500 Mb/s");
        assert_eq!(speed_label(10000), "10 Gb/s");
        let mut i = HostInterface {
            kind: "vlan".into(),
            vlan_parent: Some("eth0".into()),
            vlan_id: Some(20),
            ..Default::default()
        };
        assert_eq!(iface_kind_label(&i), "VLAN 20 on eth0");
        i.state = "lowerlayerdown".into();
        assert_eq!(iface_state(&i), "no link");
    }
}

fn split_list(s: &str) -> Vec<String> {
    s.split([',', ' '])
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect()
}
