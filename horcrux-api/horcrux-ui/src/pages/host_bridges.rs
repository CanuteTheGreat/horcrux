//! Infrastructure > Host Bridges (`/network/bridges`): the Linux bridges VM
//! network cards attach to; create and delete Horcrux-managed ones.
use crate::api::{create_bridge, delete_bridge, list_bridges, BridgeInfo, NewBridge};
use leptos::*;

#[component]
pub fn HostBridgesPage() -> impl IntoView {
    let (bridges, set_bridges) = create_signal(Vec::<BridgeInfo>::new());
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let name = create_rw_signal(String::new());
    let vlan_aware = create_rw_signal(true);
    let address = create_rw_signal(String::new());
    let ports = create_rw_signal(String::new());

    let reload = move || {
        spawn_local(async move {
            match list_bridges().await {
                Ok(l) => set_bridges.set(l),
                Err(e) => set_error.set(Some(e.message)),
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
                }
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

            <form id="bridge-create-form" class="form-row" on:submit=create>
                <input id="bridge-name" type="text" required placeholder="vmbr0" maxlength="15"
                    prop:value=name on:input=move |ev| name.set(event_target_value(&ev))/>
                <input id="bridge-address" type="text" placeholder="host address, e.g. 10.10.0.1/24 (optional)"
                    prop:value=address on:input=move |ev| address.set(event_target_value(&ev))/>
                <input id="bridge-ports" type="text" placeholder="ports, e.g. eth1 (optional)"
                    prop:value=ports on:input=move |ev| ports.set(event_target_value(&ev))/>
                <label>
                    <input id="bridge-vlan-aware" type="checkbox" prop:checked=vlan_aware
                        on:change=move |ev| vlan_aware.set(event_target_checked(&ev))/>
                    " VLAN-aware"
                </label>
                <button id="bridge-create" type="submit" class="btn btn-primary">"Create bridge"</button>
            </form>

            <table class="vm-table" id="bridge-table">
                <thead>
                    <tr><th>"Name"</th><th>"State"</th><th>"VLAN-aware"</th><th>"Ports"</th><th>"Addresses"</th><th>"Managed"</th><th>"Actions"</th></tr>
                </thead>
                <tbody>
                    {move || {
                        let list = bridges.get();
                        if list.is_empty() {
                            view! { <tr><td colspan="7" class="no-data">"No bridges on this host yet. Create one above."</td></tr> }.into_view()
                        } else {
                            list.into_iter().map(|b| {
                                let n = b.name.clone();
                                let managed = b.managed;
                                view! {
                                    <tr class="bridge-row" data-bridge=b.name.clone()>
                                        <td>{b.name.clone()}</td>
                                        <td>{if b.up { "up" } else { "down" }}</td>
                                        <td>{if b.vlan_aware { "yes" } else { "no" }}</td>
                                        <td>{b.ports.join(", ")}</td>
                                        <td>{b.addresses.join(", ")}</td>
                                        <td>{if managed { "Horcrux" } else { "host config" }}</td>
                                        <td>
                                            <button class="btn btn-sm btn-danger bridge-delete" disabled=!managed
                                                title=if managed { "" } else { "Part of the host's own network configuration" }
                                                on:click=move |_| remove(n.clone())>"Delete"</button>
                                        </td>
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
