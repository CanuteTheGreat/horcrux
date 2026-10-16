//! Cluster management (`/ha/cluster`).
//!
//! Everything here reads/writes exactly what horcrux-api's cluster
//! endpoints send and accept:
//! - `GET  /api/cluster/nodes`               -> `Vec<ClusterNode>`
//! - `GET  /api/cluster/architecture`        -> `ClusterArchitecture`
//! - `POST /api/cluster/nodes/:name`         <- `AddNodeRequest` (201, no body)
//! - `PATCH /api/cluster/nodes/:name/priority` <- `{priority}`
//! - `POST /api/cluster/find-node`           <- `FindNodeRequest` -> node name
use crate::api::{
    add_cluster_node, find_best_node_for_vm, get_cluster_architecture, get_cluster_nodes, get_vms,
    set_node_priority, AddNodeRequest, ClusterArchitecture, ClusterNode, FindNodeRequest,
};
use horcrux_common::VmConfig;
use leptos::*;

/// Highest placement/HA weight the API/CLI convention allows.
const MAX_NODE_WEIGHT: u32 = 1000;

/// Human-readable size for a byte count (the node API reports RAM in bytes).
pub(crate) fn format_memory(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let b = bytes as f64;
    if b >= GIB {
        format!("{:.1} GB", b / GIB)
    } else {
        format!("{:.0} MB", b / MIB)
    }
}

/// Parse a weight typed into the UI, enforcing the documented 0-1000 range.
pub(crate) fn parse_weight(input: &str) -> Result<u32, String> {
    match input.trim().parse::<u32>() {
        Ok(w) if w <= MAX_NODE_WEIGHT => Ok(w),
        _ => Err(format!(
            "Weight must be a whole number from 0 to {}",
            MAX_NODE_WEIGHT
        )),
    }
}

/// `VmArchitecture` -> the architecture string the find-node API expects
/// ("x86_64", "aarch64", ...). Uses the type's own serde names.
fn vm_arch_string(vm: &VmConfig) -> String {
    serde_json::to_value(&vm.architecture)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "x86_64".to_string())
}

#[component]
pub fn ClusterManagementPage() -> impl IntoView {
    let (cluster_nodes, set_cluster_nodes) = create_signal(Vec::<ClusterNode>::new());
    let (cluster_architecture, set_cluster_architecture) =
        create_signal(None::<ClusterArchitecture>);
    let (vms, set_vms) = create_signal(Vec::<VmConfig>::new());
    let (loading, set_loading) = create_signal(false);
    let (error, set_error) = create_signal(None::<String>);
    let (success_message, set_success_message) = create_signal(None::<String>);
    let (show_add_node_modal, set_show_add_node_modal) = create_signal(false);
    let (show_node_finder_modal, set_show_node_finder_modal) = create_signal(false);
    let (search_query, set_search_query) = create_signal(String::new());

    // Add node form state
    let (node_name, set_node_name) = create_signal(String::new());
    let (node_address, set_node_address) = create_signal(String::new());
    let (node_architecture, set_node_architecture) = create_signal(String::new());
    let (node_weight, set_node_weight) = create_signal("100".to_string());

    // Node finder state
    let (selected_vm_id, set_selected_vm_id) = create_signal(String::new());
    let (min_weight, set_min_weight) = create_signal(String::new());
    let (recommended_node, set_recommended_node) = create_signal(None::<String>);

    let load_cluster_data = move || {
        set_loading.set(true);
        set_error.set(None);

        spawn_local(async move {
            let mut errors = Vec::new();

            match get_cluster_nodes().await {
                Ok(nodes) => set_cluster_nodes.set(nodes),
                Err(e) => errors.push(format!("Failed to load cluster nodes: {}", e)),
            }

            match get_cluster_architecture().await {
                Ok(arch) => set_cluster_architecture.set(Some(arch)),
                Err(e) => errors.push(format!("Failed to load cluster architecture: {}", e)),
            }

            match get_vms().await {
                Ok(vm_list) => set_vms.set(vm_list),
                Err(e) => errors.push(format!("Failed to load VMs: {}", e)),
            }

            if !errors.is_empty() {
                set_error.set(Some(errors.join("; ")));
            }
            set_loading.set(false);
        });
    };

    let flash = move |msg: String| {
        set_success_message.set(Some(msg));
        set_timeout(
            move || set_success_message.set(None),
            std::time::Duration::from_secs(3),
        );
    };

    // Load data on mount
    create_effect(move |_| {
        load_cluster_data();
    });

    // Auto-refresh every 60 seconds
    set_interval(load_cluster_data, std::time::Duration::from_secs(60));

    let filtered_nodes = move || {
        let query = search_query.get().to_lowercase();
        let nodes = cluster_nodes.get();
        if query.is_empty() {
            nodes
        } else {
            nodes
                .into_iter()
                .filter(|node| {
                    node.name.to_lowercase().contains(&query)
                        || node.ip.to_lowercase().contains(&query)
                        || node.architecture.to_lowercase().contains(&query)
                })
                .collect()
        }
    };

    let reset_add_node_form = move || {
        set_node_name.set(String::new());
        set_node_address.set(String::new());
        set_node_architecture.set(String::new());
        set_node_weight.set("100".to_string());
    };

    let add_node = move || {
        let priority = match parse_weight(&node_weight.get()) {
            Ok(w) => w,
            Err(e) => {
                set_error.set(Some(e));
                return;
            }
        };
        let arch = node_architecture.get();
        let request = AddNodeRequest {
            address: node_address.get().trim().to_string(),
            architecture: if arch.is_empty() { None } else { Some(arch) },
            priority: Some(priority),
        };
        let name = node_name.get().trim().to_string();

        spawn_local(async move {
            match add_cluster_node(&name, &request).await {
                Ok(()) => {
                    set_show_add_node_modal.set(false);
                    reset_add_node_form();
                    load_cluster_data();
                    flash(format!("Node '{}' added", name));
                }
                Err(e) => set_error.set(Some(format!("Failed to add node: {}", e))),
            }
        });
    };

    let save_weight = move |name: String, raw: String| match parse_weight(&raw) {
        Err(e) => set_error.set(Some(e)),
        Ok(weight) => spawn_local(async move {
            match set_node_priority(&name, weight).await {
                Ok(()) => {
                    load_cluster_data();
                    flash(format!("Node '{}' weight set to {}", name, weight));
                }
                Err(e) => set_error.set(Some(format!("Failed to set weight: {}", e))),
            }
        }),
    };

    let find_best_node = move || {
        let Some(vm) = vms
            .get()
            .into_iter()
            .find(|vm| vm.id == selected_vm_id.get())
        else {
            return;
        };
        let min_priority = match min_weight.get().trim() {
            "" => None,
            raw => match parse_weight(raw) {
                Ok(w) => Some(w),
                Err(e) => {
                    set_error.set(Some(e));
                    return;
                }
            },
        };
        let request = FindNodeRequest {
            architecture: vm_arch_string(&vm),
            memory_mb: vm.memory.max(1),
            cpu_cores: vm.cpus.max(1),
            min_priority,
        };
        set_recommended_node.set(None);
        spawn_local(async move {
            match find_best_node_for_vm(&request).await {
                Ok(node) => set_recommended_node.set(Some(node)),
                Err(e) => set_error.set(Some(format!("Failed to find best node: {}", e))),
            }
        });
    };

    let node_status_color = |status: &str| match status.to_lowercase().as_str() {
        "online" => "bg-green-100 text-green-800",
        "offline" => "bg-red-100 text-red-800",
        _ => "bg-gray-100 text-gray-800",
    };

    view! {
        <div class="p-6">
            <div class="flex justify-between items-center mb-6">
                <div>
                    <h1 class="text-2xl font-bold">"Cluster Management"</h1>
                    <p class="text-gray-600">
                        "Manage cluster nodes, placement weights, and VM placement"
                    </p>
                </div>
                <div class="flex space-x-3">
                    <button
                        on:click=move |_| load_cluster_data()
                        class="bg-gray-500 hover:bg-gray-600 text-white px-4 py-2 rounded-lg"
                    >
                        "Refresh"
                    </button>
                    <button
                        on:click=move |_| set_show_node_finder_modal.set(true)
                        class="bg-purple-500 hover:bg-purple-600 text-white px-4 py-2 rounded-lg"
                    >
                        "Find Best Node"
                    </button>
                    <button
                        on:click=move |_| {
                            reset_add_node_form();
                            set_show_add_node_modal.set(true);
                        }
                        class="bg-blue-500 hover:bg-blue-600 text-white px-4 py-2 rounded-lg"
                    >
                        "Add Node"
                    </button>
                </div>
            </div>

            {move || success_message.get().map(|msg| view! {
                <div class="bg-green-50 border border-green-200 text-green-700 px-4 py-3 rounded mb-6">
                    {msg}
                </div>
            })}

            {move || error.get().map(|e| view! {
                <div class="bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded mb-6">
                    {e}
                </div>
            })}

            // Cluster overview: architecture summary from the API plus real
            // node/VM counts from the lists this page already loads.
            {move || cluster_architecture.get().map(|arch| {
                let online = cluster_nodes
                    .get()
                    .iter()
                    .filter(|n| n.status.eq_ignore_ascii_case("online"))
                    .count();
                let total_vms = vms.get().len();
                view! {
                    <div class="bg-white rounded-lg shadow mb-6">
                        <div class="p-6 border-b border-gray-200">
                            <h2 class="text-lg font-semibold">"Cluster Architecture Overview"</h2>
                        </div>
                        <div class="p-6">
                            <div class="grid grid-cols-2 md:grid-cols-4 gap-6 mb-6">
                                <div class="text-center">
                                    <div class="text-2xl font-semibold text-blue-600">{arch.total_nodes()}</div>
                                    <div class="text-sm text-gray-500">"Total Nodes"</div>
                                </div>
                                <div class="text-center">
                                    <div class="text-2xl font-semibold text-green-600">{online}</div>
                                    <div class="text-sm text-gray-500">"Online Nodes"</div>
                                </div>
                                <div class="text-center">
                                    <div class="text-2xl font-semibold text-purple-600">{total_vms}</div>
                                    <div class="text-sm text-gray-500">"Total VMs"</div>
                                </div>
                                <div class="text-center">
                                    <div class="text-2xl font-semibold text-gray-600">
                                        {if arch.is_mixed { "Mixed" } else { "Uniform" }}
                                    </div>
                                    <div class="text-sm text-gray-500">"Architecture Mix"</div>
                                </div>
                            </div>
                            <h3 class="font-medium text-gray-900 mb-3">"Architecture Distribution"</h3>
                            <div class="space-y-2">
                                <div class="flex justify-between items-center text-sm">
                                    <span class="font-medium">"x86_64"</span>
                                    <span class="text-gray-600">{arch.x86_64_nodes} " nodes"</span>
                                </div>
                                <div class="flex justify-between items-center text-sm">
                                    <span class="font-medium">"aarch64"</span>
                                    <span class="text-gray-600">{arch.aarch64_nodes} " nodes"</span>
                                </div>
                                <div class="flex justify-between items-center text-sm">
                                    <span class="font-medium">"other"</span>
                                    <span class="text-gray-600">{arch.other_nodes} " nodes"</span>
                                </div>
                            </div>
                        </div>
                    </div>
                }
            })}

            // Search
            <div class="bg-white rounded-lg shadow p-4 mb-6">
                <div class="flex items-center space-x-4">
                    <div class="flex-1">
                        <label class="block text-sm font-medium text-gray-700 mb-1">
                            "Search Nodes"
                        </label>
                        <input
                            type="text"
                            placeholder="Search by name, address, or architecture..."
                            on:input=move |ev| set_search_query.set(event_target_value(&ev))
                            class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                        />
                    </div>
                    <div class="text-sm text-gray-600">
                        {move || {
                            let filtered = filtered_nodes().len();
                            let total = cluster_nodes.get().len();
                            if filtered == total {
                                format!("{} nodes", total)
                            } else {
                                format!("{} of {} nodes", filtered, total)
                            }
                        }}
                    </div>
                </div>
            </div>

            // Cluster Nodes Table
            {move || if loading.get() && cluster_nodes.get().is_empty() {
                view! {
                    <div class="bg-white rounded-lg shadow p-8 text-center">
                        <p class="text-gray-600">"Loading cluster nodes..."</p>
                    </div>
                }.into_view()
            } else {
                view! {
                    <div class="bg-white rounded-lg shadow overflow-hidden">
                        <div class="p-6 border-b border-gray-200">
                            <h2 class="text-lg font-semibold">"Cluster Nodes"</h2>
                            <p class="text-sm text-gray-500">
                                "Weight (0-1000, default 100): higher-weighted nodes are preferred for VM placement and HA failover."
                            </p>
                        </div>
                        <div class="overflow-x-auto">
                            <table class="min-w-full divide-y divide-gray-200">
                                <thead class="bg-gray-50">
                                    <tr>
                                        <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">"Node"</th>
                                        <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">"Status"</th>
                                        <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">"Resources"</th>
                                        <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">"Weight / Priority"</th>
                                    </tr>
                                </thead>
                                <tbody class="bg-white divide-y divide-gray-200">
                                    {move || {
                                        let nodes = filtered_nodes();
                                        if nodes.is_empty() {
                                            let msg = if cluster_nodes.get().is_empty() {
                                                "No cluster nodes configured"
                                            } else {
                                                "No nodes match your search"
                                            };
                                            view! {
                                                <tr>
                                                    <td colspan="4" class="px-6 py-8 text-center text-gray-500">{msg}</td>
                                                </tr>
                                            }.into_view()
                                        } else {
                                            nodes.into_iter().map(|node| {
                                                let status_color = node_status_color(&node.status);
                                                let (weight_input, set_weight_input) =
                                                    create_signal(node.priority.to_string());
                                                let name_for_save = node.name.clone();
                                                view! {
                                                    <tr>
                                                        <td class="px-6 py-4 whitespace-nowrap">
                                                            <div class="text-sm font-medium text-gray-900">
                                                                {node.name.clone()}
                                                                {node.is_local.then(|| view! {
                                                                    <span class="ml-2 text-xs text-blue-600">"(this node)"</span>
                                                                })}
                                                            </div>
                                                            <div class="text-sm text-gray-500">{node.ip.clone()}</div>
                                                        </td>
                                                        <td class="px-6 py-4 whitespace-nowrap">
                                                            <span class=format!("px-2 py-1 text-xs font-medium rounded {}", status_color)>
                                                                {node.status.clone()}
                                                            </span>
                                                        </td>
                                                        <td class="px-6 py-4 whitespace-nowrap">
                                                            <div class="text-sm text-gray-900">
                                                                <div>{node.cpu_cores} " CPU cores"</div>
                                                                <div>{format_memory(node.memory_total)} " RAM"</div>
                                                            </div>
                                                            <div class="text-xs text-gray-500">{node.architecture.clone()}</div>
                                                        </td>
                                                        <td class="px-6 py-4 whitespace-nowrap">
                                                            <div class="flex items-center space-x-2">
                                                                <span class="text-sm font-semibold">{node.priority}</span>
                                                                <input
                                                                    type="number"
                                                                    min="0"
                                                                    max=MAX_NODE_WEIGHT.to_string()
                                                                    class="w-24 px-2 py-1 border border-gray-300 rounded"
                                                                    prop:value=move || weight_input.get()
                                                                    on:input=move |ev| set_weight_input.set(event_target_value(&ev))
                                                                />
                                                                <button
                                                                    class="text-blue-600 hover:text-blue-900 px-2 py-1 rounded hover:bg-blue-50"
                                                                    on:click=move |_| save_weight(name_for_save.clone(), weight_input.get())
                                                                >
                                                                    "Set"
                                                                </button>
                                                            </div>
                                                        </td>
                                                    </tr>
                                                }
                                            }).collect_view()
                                        }
                                    }}
                                </tbody>
                            </table>
                        </div>
                    </div>
                }.into_view()
            }}

            // Add Node Modal
            {move || show_add_node_modal.get().then(|| view! {
                <div class="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50 p-4">
                    <div class="bg-white rounded-lg w-full max-w-lg">
                        <div class="p-6 border-b border-gray-200">
                            <h2 class="text-xl font-semibold">"Add Cluster Node"</h2>
                        </div>
                        <div class="p-6 space-y-4">
                            <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-1">"Node Name"</label>
                                    <input
                                        type="text"
                                        placeholder="node-02"
                                        prop:value=move || node_name.get()
                                        on:input=move |ev| set_node_name.set(event_target_value(&ev))
                                        class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                    />
                                </div>
                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-1">"IP Address"</label>
                                    <input
                                        type="text"
                                        placeholder="192.168.1.100"
                                        prop:value=move || node_address.get()
                                        on:input=move |ev| set_node_address.set(event_target_value(&ev))
                                        class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                    />
                                </div>
                            </div>
                            <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-1">"Architecture"</label>
                                    <select
                                        on:change=move |ev| set_node_architecture.set(event_target_value(&ev))
                                        class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                    >
                                        <option value="" selected=move || node_architecture.get().is_empty()>"Unknown"</option>
                                        <option value="x86_64">"x86_64"</option>
                                        <option value="aarch64">"ARM64 (aarch64)"</option>
                                        <option value="riscv64">"RISC-V 64"</option>
                                        <option value="ppc64le">"POWER (ppc64le)"</option>
                                    </select>
                                </div>
                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-1">"Weight (0-1000)"</label>
                                    <input
                                        type="number"
                                        min="0"
                                        max=MAX_NODE_WEIGHT.to_string()
                                        prop:value=move || node_weight.get()
                                        on:input=move |ev| set_node_weight.set(event_target_value(&ev))
                                        class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                    />
                                </div>
                            </div>
                        </div>
                        <div class="p-6 border-t border-gray-200 flex justify-end space-x-3">
                            <button
                                on:click=move |_| set_show_add_node_modal.set(false)
                                class="px-4 py-2 text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50"
                            >
                                "Cancel"
                            </button>
                            <button
                                on:click=move |_| add_node()
                                class="px-4 py-2 bg-blue-500 text-white rounded-lg hover:bg-blue-600"
                                disabled=move || node_name.get().trim().is_empty() || node_address.get().trim().is_empty()
                            >
                                "Add Node"
                            </button>
                        </div>
                    </div>
                </div>
            })}

            // Node Finder Modal
            {move || show_node_finder_modal.get().then(|| view! {
                <div class="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50 p-4">
                    <div class="bg-white rounded-lg w-full max-w-lg">
                        <div class="p-6 border-b border-gray-200">
                            <h2 class="text-xl font-semibold">"Find Best Node for VM"</h2>
                        </div>
                        <div class="p-6 space-y-4">
                            <div>
                                <label class="block text-sm font-medium text-gray-700 mb-1">"Select VM"</label>
                                <select
                                    on:change=move |ev| set_selected_vm_id.set(event_target_value(&ev))
                                    class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                >
                                    <option value="">"Select a VM..."</option>
                                    {vms.get().into_iter().map(|vm| {
                                        let display = format!(
                                            "{} ({}, {} vCPU, {} MB)",
                                            vm.name,
                                            vm_arch_string(&vm),
                                            vm.cpus,
                                            vm.memory
                                        );
                                        view! { <option value=vm.id.clone()>{display}</option> }
                                    }).collect_view()}
                                </select>
                            </div>
                            <div>
                                <label class="block text-sm font-medium text-gray-700 mb-1">
                                    "Minimum node weight (optional)"
                                </label>
                                <input
                                    type="number"
                                    min="0"
                                    max=MAX_NODE_WEIGHT.to_string()
                                    placeholder="any"
                                    prop:value=move || min_weight.get()
                                    on:input=move |ev| set_min_weight.set(event_target_value(&ev))
                                    class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                />
                            </div>
                            <button
                                on:click=move |_| find_best_node()
                                class="w-full bg-purple-500 hover:bg-purple-600 text-white px-4 py-2 rounded-lg"
                                disabled=move || selected_vm_id.get().is_empty()
                            >
                                "Find Best Node"
                            </button>
                            {move || recommended_node.get().map(|node| view! {
                                <div class="border border-gray-200 rounded-lg p-4 bg-green-50">
                                    <p class="text-sm text-gray-700">"Recommended node:"</p>
                                    <h4 class="font-medium text-gray-900">{node}</h4>
                                </div>
                            })}
                        </div>
                        <div class="p-6 border-t border-gray-200 flex justify-end space-x-3">
                            <button
                                on:click=move |_| {
                                    set_show_node_finder_modal.set(false);
                                    set_recommended_node.set(None);
                                    set_selected_vm_id.set(String::new());
                                    set_min_weight.set(String::new());
                                }
                                class="px-4 py-2 text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50"
                            >
                                "Close"
                            </button>
                        </div>
                    </div>
                </div>
            })}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_memory_treats_api_value_as_bytes() {
        assert_eq!(format_memory(8 * 1024 * 1024 * 1024), "8.0 GB");
        assert_eq!(format_memory(512 * 1024 * 1024), "512 MB");
        assert_eq!(format_memory(0), "0 MB");
    }

    #[test]
    fn parse_weight_enforces_range() {
        assert_eq!(parse_weight("0"), Ok(0));
        assert_eq!(parse_weight(" 300 "), Ok(300));
        assert_eq!(parse_weight("1000"), Ok(1000));
        assert!(parse_weight("1001").is_err());
        assert!(parse_weight("-1").is_err());
        assert!(parse_weight("abc").is_err());
    }
}
