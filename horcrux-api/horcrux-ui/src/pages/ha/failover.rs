//! Cluster membership, fencing and HA events (`/ha/failover`).
//!
//! - `GET  /api/cluster/status`          -> cluster name, quorum
//! - `POST /api/cluster/create`          <- name + this node's address
//! - `POST /api/cluster/join`            <- member address + admin login
//! - `GET/POST /api/ha/fencing/devices`, `DELETE .../:id`
//! - `GET  /api/ha/events`               -> what HA did (recoveries, failures)
use crate::api::*;
use leptos::*;

#[component]
pub fn HaFailoverPage() -> impl IntoView {
    let (status, set_status) = create_signal(None::<ClusterStatusView>);
    let (devices, set_devices) = create_signal(Vec::<FencingDeviceView>::new());
    let (events, set_events) = create_signal(Vec::<HaEventView>::new());
    let (error, set_error) = create_signal(None::<String>);
    let (info, set_info) = create_signal(None::<String>);

    let (c_name, set_c_name) = create_signal(String::new());
    let (c_addr, set_c_addr) = create_signal(String::new());
    let (j_peer, set_j_peer) = create_signal(String::new());
    let (j_user, set_j_user) = create_signal("admin".to_string());
    let (j_pass, set_j_pass) = create_signal(String::new());
    let (j_addr, set_j_addr) = create_signal(String::new());

    let (f_id, set_f_id) = create_signal(String::new());
    let (f_node, set_f_node) = create_signal(String::new());
    let (f_agent, set_f_agent) = create_signal("Ipmi".to_string());
    let (f_addr, set_f_addr) = create_signal(String::new());
    let (f_user, set_f_user) = create_signal(String::new());
    let (f_pass, set_f_pass) = create_signal(String::new());

    let load = move || {
        spawn_local(async move {
            match get_cluster_status_view().await {
                Ok(s) => set_status.set(Some(s)),
                Err(e) => set_error.set(Some(format!("Cluster status: {}", e))),
            }
            match list_fencing_devices().await {
                Ok(d) => set_devices.set(d),
                Err(e) => set_error.set(Some(format!("Fencing devices: {}", e))),
            }
            match list_ha_events().await {
                Ok(ev) => set_events.set(ev),
                Err(e) => set_error.set(Some(format!("HA events: {}", e))),
            }
        });
    };
    create_effect(move |_| load());
    set_interval(load, std::time::Duration::from_secs(10));

    let do_create = move |_| {
        let req = CreateClusterRequest {
            name: c_name.get().trim().to_string(),
            address: c_addr.get().trim().to_string(),
        };
        spawn_local(async move {
            match create_cluster(&req).await {
                Ok(()) => {
                    set_error.set(None);
                    set_info.set(Some(format!("Cluster '{}' created", req.name)));
                    load();
                }
                Err(e) => set_error.set(Some(format!("Create cluster failed: {}", e))),
            }
        });
    };
    let do_join = move |_| {
        let req = JoinClusterRequest {
            peer: j_peer.get().trim().to_string(),
            username: j_user.get().trim().to_string(),
            password: j_pass.get(),
            address: j_addr.get().trim().to_string(),
        };
        spawn_local(async move {
            match join_cluster(&req).await {
                Ok(()) => {
                    set_error.set(None);
                    set_j_pass.set(String::new());
                    set_info.set(Some(format!("Joined the cluster of {}", req.peer)));
                    load();
                }
                Err(e) => set_error.set(Some(format!("Join failed: {}", e))),
            }
        });
    };
    let do_add_device = move |_| {
        let nz = |s: String| {
            if s.trim().is_empty() {
                None
            } else {
                Some(s.trim().to_string())
            }
        };
        let dev = NewFencingDevice {
            id: f_id.get().trim().to_string(),
            node: f_node.get().trim().to_string(),
            agent_type: f_agent.get(),
            address: f_addr.get().trim().to_string(),
            username: nz(f_user.get()),
            password: if f_pass.get().is_empty() {
                None
            } else {
                Some(f_pass.get())
            },
            port: None,
            options: Default::default(),
            priority: 1,
        };
        spawn_local(async move {
            match add_fencing_device(&dev).await {
                Ok(()) => {
                    set_error.set(None);
                    set_f_pass.set(String::new());
                    load();
                }
                Err(e) => set_error.set(Some(format!("Add fencing device failed: {}", e))),
            }
        });
    };

    view! {
        <div class="p-6 space-y-6" data-testid="ha-failover-page">
            <h1 class="text-2xl font-bold">"Cluster, fencing and HA events"</h1>
            {move || error.get().map(|e| view! { <div class="bg-red-100 text-red-800 p-3 rounded" data-testid="ha-error">{e}</div> })}
            {move || info.get().map(|e| view! { <div class="bg-green-100 text-green-800 p-3 rounded">{e}</div> })}

            <section class="bg-white p-4 rounded shadow">
                <h2 class="text-lg font-semibold mb-2">"Cluster"</h2>
                {move || match status.get() {
                    Some(s) if s.name.is_some() => view! {
                        <p data-testid="cluster-name">
                            "Cluster " <b>{s.name.clone().unwrap_or_default()}</b>
                            {format!(": {} of {} nodes online, quorum: {}", s.online_nodes, s.total_nodes,
                                match s.has_quorum { Some(true) => "yes", Some(false) => "NO", None => "unknown" })}
                        </p>
                    }.into_view(),
                    _ => view! {
                        <div class="grid md:grid-cols-2 gap-6">
                            <div class="space-y-2">
                                <h3 class="font-medium">"Create a cluster on this node"</h3>
                                <input class="border p-2 w-full" placeholder="cluster name" data-testid="cluster-create-name"
                                    prop:value=c_name on:input=move |e| set_c_name.set(event_target_value(&e))/>
                                <input class="border p-2 w-full" placeholder="address the other nodes reach this node on"
                                    data-testid="cluster-create-address"
                                    prop:value=c_addr on:input=move |e| set_c_addr.set(event_target_value(&e))/>
                                <button class="bg-blue-600 text-white px-4 py-2 rounded" data-testid="cluster-create" on:click=do_create>"Create cluster"</button>
                            </div>
                            <div class="space-y-2">
                                <h3 class="font-medium">"Join an existing cluster"</h3>
                                <input class="border p-2 w-full" placeholder="address of a cluster member (host or host:port)"
                                    data-testid="cluster-join-peer"
                                    prop:value=j_peer on:input=move |e| set_j_peer.set(event_target_value(&e))/>
                                <input class="border p-2 w-full" placeholder="administrator user on that member"
                                    prop:value=j_user on:input=move |e| set_j_user.set(event_target_value(&e))/>
                                <input class="border p-2 w-full" type="password" placeholder="its password"
                                    data-testid="cluster-join-password"
                                    prop:value=j_pass on:input=move |e| set_j_pass.set(event_target_value(&e))/>
                                <input class="border p-2 w-full" placeholder="address the members reach THIS node on"
                                    data-testid="cluster-join-address"
                                    prop:value=j_addr on:input=move |e| set_j_addr.set(event_target_value(&e))/>
                                <button class="bg-blue-600 text-white px-4 py-2 rounded" data-testid="cluster-join" on:click=do_join>"Join cluster"</button>
                            </div>
                        </div>
                    }.into_view(),
                }}
            </section>

            <section class="bg-white p-4 rounded shadow">
                <h2 class="text-lg font-semibold mb-2">"Fencing devices"</h2>
                <p class="text-sm text-gray-600 mb-2">
                    "A node that stops answering is powered off through its fencing device before its HA VMs restart elsewhere. "
                    "Nodes without a device are only recovered in clusters of 3+ nodes, after they stop themselves."
                </p>
                <table class="w-full text-sm mb-3" data-testid="fencing-table">
                    <thead><tr><th class="text-left">"ID"</th><th class="text-left">"Node"</th><th class="text-left">"Agent"</th><th class="text-left">"Address"</th><th></th></tr></thead>
                    <tbody>
                        <For each=move || devices.get() key=|d| d.id.clone() children=move |d| {
                            let id = d.id.clone();
                            view! { <tr>
                                <td>{d.id.clone()}</td><td>{d.node.clone()}</td><td>{d.agent_type.clone()}</td><td>{d.address.clone()}</td>
                                <td><button class="text-red-600" on:click=move |_| {
                                    let id = id.clone();
                                    spawn_local(async move {
                                        match delete_fencing_device(&id).await {
                                            Ok(()) => load(),
                                            Err(e) => set_error.set(Some(format!("Remove failed: {}", e))),
                                        }
                                    });
                                }>"Remove"</button></td>
                            </tr> }
                        }/>
                    </tbody>
                </table>
                <div class="grid md:grid-cols-6 gap-2">
                    <input class="border p-2" placeholder="id" data-testid="fence-id" prop:value=f_id on:input=move |e| set_f_id.set(event_target_value(&e))/>
                    <input class="border p-2" placeholder="node to fence" data-testid="fence-node" prop:value=f_node on:input=move |e| set_f_node.set(event_target_value(&e))/>
                    <select class="border p-2" data-testid="fence-agent" on:change=move |e| set_f_agent.set(event_target_value(&e))>
                        <option value="Ipmi">"IPMI"</option><option value="Ilo">"HP iLO"</option>
                        <option value="Drac">"Dell iDRAC"</option><option value="Ssh">"SSH"</option>
                        <option value="Libvirt">"libvirt"</option><option value="SnmpPdu">"SNMP PDU"</option>
                        <option value="Manual">"Manual"</option>
                    </select>
                    <input class="border p-2" placeholder="BMC / PDU address" data-testid="fence-address" prop:value=f_addr on:input=move |e| set_f_addr.set(event_target_value(&e))/>
                    <input class="border p-2" placeholder="user" prop:value=f_user on:input=move |e| set_f_user.set(event_target_value(&e))/>
                    <input class="border p-2" type="password" placeholder="password" prop:value=f_pass on:input=move |e| set_f_pass.set(event_target_value(&e))/>
                </div>
                <button class="mt-2 bg-blue-600 text-white px-4 py-2 rounded" data-testid="fence-add" on:click=do_add_device>"Add fencing device"</button>
            </section>

            <section class="bg-white p-4 rounded shadow">
                <h2 class="text-lg font-semibold mb-2">"Recent HA events"</h2>
                <table class="w-full text-sm" data-testid="ha-events-table">
                    <thead><tr><th class="text-left">"When"</th><th class="text-left">"VM"</th><th class="text-left">"Node"</th><th class="text-left">"What happened"</th></tr></thead>
                    <tbody>
                        <For each=move || events.get() key=|e| format!("{}{}{}", e.timestamp, e.vm_id, e.message) children=move |e| view! {
                            <tr><td>{e.timestamp.clone()}</td><td>{e.vm_id.clone()}</td><td>{e.node.clone()}</td><td>{e.message.clone()}</td></tr>
                        }/>
                    </tbody>
                </table>
            </section>
        </div>
    }
}
