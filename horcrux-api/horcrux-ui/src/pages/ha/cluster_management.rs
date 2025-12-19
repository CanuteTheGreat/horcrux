use leptos::*;
use horcrux_common::VmConfig;
use crate::api::{
    ClusterNode, ClusterArchitecture, AddNodeRequest, FindNodeRequest, NodeRecommendation,
    get_cluster_nodes, get_cluster_architecture, add_cluster_node, find_best_node_for_vm, get_vms
};

#[component]
pub fn ClusterManagementPage() -> impl IntoView {
    let (cluster_nodes, set_cluster_nodes) = create_signal(Vec::<ClusterNode>::new());
    let (cluster_architecture, set_cluster_architecture) = create_signal(None::<ClusterArchitecture>);
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
    let (node_ssh_key, set_node_ssh_key) = create_signal(String::new());
    let (node_architecture, set_node_architecture) = create_signal("x86_64".to_string());

    // Node finder state
    let (selected_vm_id, set_selected_vm_id) = create_signal(String::new());
    let (preferred_arch, set_preferred_arch) = create_signal(String::new());
    let (node_recommendations, set_node_recommendations) = create_signal(Vec::<NodeRecommendation>::new());

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

    // Load data on mount
    create_effect(move |_| {
        load_cluster_data();
    });

    // Auto-refresh every 60 seconds
    use leptos::set_interval;
    set_interval(
        move || load_cluster_data(),
        std::time::Duration::from_secs(60),
    );

    let filtered_nodes = move || {
        let query = search_query.get().to_lowercase();
        if query.is_empty() {
            cluster_nodes.get()
        } else {
            cluster_nodes
                .get()
                .into_iter()
                .filter(|node| {
                    node.name.to_lowercase().contains(&query) ||
                    node.address.to_lowercase().contains(&query) ||
                    node.architecture.to_lowercase().contains(&query)
                })
                .collect()
        }
    };

    let reset_add_node_form = move || {
        set_node_name.set(String::new());
        set_node_address.set(String::new());
        set_node_ssh_key.set(String::new());
        set_node_architecture.set("x86_64".to_string());
    };

    let add_node = move || {
        let request = AddNodeRequest {
            name: node_name.get(),
            address: node_address.get(),
            ssh_key: if node_ssh_key.get().is_empty() { None } else { Some(node_ssh_key.get()) },
            architecture: node_architecture.get(),
        };

        let name = node_name.get();

        spawn_local(async move {
            match add_cluster_node(name.clone(), request).await {
                Ok(_) => {
                    set_show_add_node_modal.set(false);
                    reset_add_node_form();
                    load_cluster_data();
                    set_success_message.set(Some(format!("Node '{}' added successfully", name)));
                    set_timeout(
                        move || set_success_message.set(None),
                        std::time::Duration::from_secs(3),
                    );
                }
                Err(e) => set_error.set(Some(format!("Failed to add node: {}", e))),
            }
        });
    };

    let find_best_node = move || {
        if let Some(selected_vm) = vms.get().iter().find(|vm| vm.id == selected_vm_id.get()) {
            let request = FindNodeRequest {
                vm_config: serde_json::json!({
                    "memory": 4096,
                    "vcpus": 2,
                    "disk_size": 20
                }),
                preferred_architecture: if preferred_arch.get().is_empty() { None } else { Some(preferred_arch.get()) },
                exclude_nodes: Vec::new(),
            };

            spawn_local(async move {
                match find_best_node_for_vm(request).await {
                    Ok(recommendation) => {
                        set_node_recommendations.set(vec![recommendation]);
                    }
                    Err(e) => set_error.set(Some(format!("Failed to find best node: {}", e))),
                }
            });
        }
    };

    let get_node_status_color = move |status: &str| {
        match status.to_lowercase().as_str() {
            "online" => "bg-green-100 text-green-800",
            "offline" => "bg-red-100 text-red-800",
            "maintenance" => "bg-yellow-100 text-yellow-800",
            _ => "bg-gray-100 text-gray-800",
        }
    };

    let get_usage_color = move |usage: f64| {
        if usage < 70.0 {
            "text-green-600"
        } else if usage < 85.0 {
            "text-yellow-600"
        } else {
            "text-red-600"
        }
    };

    let format_bytes = move |bytes: u64| {
        if bytes < 1024 {
            format!("{} MB", bytes)
        } else {
            format!("{:.1} GB", bytes as f64 / 1024.0)
        }
    };

    view! {
        <div class="p-6">
            <div class="flex justify-between items-center mb-6">
                <div>
                    <h1 class="text-2xl font-bold">Cluster Management</h1>
                    <p class="text-gray-600">
                        "Manage cluster nodes, monitor resources, and optimize VM placement"
                    </p>
                </div>
                <div class="flex space-x-3">
                    <button
                        on:click=move |_| load_cluster_data()
                        class="bg-gray-500 hover:bg-gray-600 text-white px-4 py-2 rounded-lg"
                    >
                        <i class="fas fa-sync mr-2"></i>
                        "Refresh"
                    </button>
                    <button
                        on:click=move |_| set_show_node_finder_modal.set(true)
                        class="bg-purple-500 hover:bg-purple-600 text-white px-4 py-2 rounded-lg"
                    >
                        <i class="fas fa-search mr-2"></i>
                        "Find Best Node"
                    </button>
                    <button
                        on:click=move |_| {
                            reset_add_node_form();
                            set_show_add_node_modal.set(true);
                        }
                        class="bg-blue-500 hover:bg-blue-600 text-white px-4 py-2 rounded-lg"
                    >
                        <i class="fas fa-plus mr-2"></i>
                        "Add Node"
                    </button>
                </div>
            </div>

            // Success message
            {move || success_message.get().map(|msg| view! {
                <div class="bg-green-50 border border-green-200 text-green-700 px-4 py-3 rounded mb-6">
                    <i class="fas fa-check-circle mr-2"></i>
                    {msg}
                </div>
            })}

            // Error display
            {move || error.get().map(|e| view! {
                <div class="bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded mb-6">
                    <i class="fas fa-exclamation-triangle mr-2"></i>
                    {e}
                </div>
            })}

            // Cluster Architecture Overview
            {move || cluster_architecture.get().map(|arch| view! {
                <div class="bg-white rounded-lg shadow mb-6">
                    <div class="p-6 border-b border-gray-200">
                        <h2 class="text-lg font-semibold">Cluster Architecture Overview</h2>
                    </div>
                    <div class="p-6">
                        <div class="grid grid-cols-2 md:grid-cols-4 gap-6 mb-6">
                            <div class="text-center">
                                <div class="text-2xl font-semibold text-blue-600">{arch.total_nodes}</div>
                                <div class="text-sm text-gray-500">"Total Nodes"</div>
                            </div>
                            <div class="text-center">
                                <div class="text-2xl font-semibold text-green-600">{arch.online_nodes}</div>
                                <div class="text-sm text-gray-500">"Online Nodes"</div>
                            </div>
                            <div class="text-center">
                                <div class="text-2xl font-semibold text-purple-600">{arch.total_vms}</div>
                                <div class="text-sm text-gray-500">"Total VMs"</div>
                            </div>
                            <div class="text-center">
                                <div class="text-2xl font-semibold text-gray-600">{arch.architectures.len()}</div>
                                <div class="text-sm text-gray-500">"Architectures"</div>
                            </div>
                        </div>

                        <div class="grid grid-cols-1 md:grid-cols-2 gap-6">
                            <div>
                                <h3 class="font-medium text-gray-900 mb-3">"Load Balance Mode"</h3>
                                <div class="bg-gray-50 p-3 rounded">
                                    <span class="text-sm font-mono">{&arch.load_balance_mode}</span>
                                </div>
                            </div>
                            <div>
                                <h3 class="font-medium text-gray-900 mb-3">"Architecture Distribution"</h3>
                                <div class="space-y-2">
                                    {arch.architectures.clone().into_iter().map(|arch_info| {
                                        let name = arch_info.name.clone();
                                        let node_count = arch_info.node_count;
                                        let vm_count = arch_info.vm_count;
                                        view! {
                                            <div class="flex justify-between items-center text-sm">
                                                <span class="font-medium">{name}</span>
                                                <span class="text-gray-600">
                                                    {node_count} " nodes, " {vm_count} " VMs"
                                                </span>
                                            </div>
                                        }
                                    }).collect::<Vec<_>>()}
                                </div>
                            </div>
                        </div>
                    </div>
                </div>
            })}

            // Search and Controls
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
                            let filtered = filtered_nodes();
                            let total = cluster_nodes.get().len();
                            if filtered.len() == total {
                                format!("{} nodes", total)
                            } else {
                                format!("{} of {} nodes", filtered.len(), total)
                            }
                        }}
                    </div>
                </div>
            </div>

            // Cluster Nodes Table
            {move || if loading.get() {
                view! {
                    <div class="bg-white rounded-lg shadow p-8 text-center">
                        <i class="fas fa-spinner fa-spin text-2xl text-gray-400 mb-2"></i>
                        <p class="text-gray-600">"Loading cluster nodes..."</p>
                    </div>
                }
            } else {
                view! {
                    <div class="bg-white rounded-lg shadow overflow-hidden">
                        <div class="p-6 border-b border-gray-200">
                            <h2 class="text-lg font-semibold">Cluster Nodes</h2>
                        </div>
                        <div class="overflow-x-auto">
                            <table class="min-w-full divide-y divide-gray-200">
                                <thead class="bg-gray-50">
                                    <tr>
                                        <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                            "Node"
                                        </th>
                                        <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                            "Status"
                                        </th>
                                        <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                            "Resources"
                                        </th>
                                        <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                            "Usage"
                                        </th>
                                        <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                            "VMs"
                                        </th>
                                        <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                            "Actions"
                                        </th>
                                    </tr>
                                </thead>
                                <tbody class="bg-white divide-y divide-gray-200">
                                    {move || if filtered_nodes().is_empty() {
                                        if cluster_nodes.get().is_empty() {
                                            view! {
                                                <tr>
                                                    <td colspan="6" class="px-6 py-8 text-center text-gray-500">
                                                        <div>
                                                            <i class="fas fa-server text-4xl mb-4"></i>
                                                            <p class="text-lg mb-2">"No cluster nodes configured"</p>
                                                            <p class="text-sm mb-4">"Add your first node to enable clustering and HA"</p>
                                                            <button
                                                                on:click=move |_| {
                                                                    reset_add_node_form();
                                                                    set_show_add_node_modal.set(true);
                                                                }
                                                                class="bg-blue-500 hover:bg-blue-600 text-white px-4 py-2 rounded-lg"
                                                            >
                                                                "Add First Node"
                                                            </button>
                                                        </div>
                                                    </td>
                                                </tr>
                                            }.into_view()
                                        } else {
                                            view! {
                                                <tr>
                                                    <td colspan="6" class="px-6 py-8 text-center text-gray-500">
                                                        <i class="fas fa-search text-4xl mb-4"></i>
                                                        <p class="text-lg">"No nodes match your search"</p>
                                                    </td>
                                                </tr>
                                            }.into_view()
                                        }
                                    } else {
                                        filtered_nodes().into_iter().map(|node| {
                                            let status_color = get_node_status_color(&node.status);
                                            let cpu_color = get_usage_color(node.cpu_usage);
                                            let memory_color = get_usage_color(node.memory_usage);
                                            let disk_color = get_usage_color(node.disk_usage);
                                            let node_name = node.name.clone();
                                            let node_address = node.address.clone();
                                            let node_version = node.version.clone();
                                            let status_str = node.status.clone();
                                                let last_seen = node.last_seen.map(|dt| dt.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_else(|| "Never".to_string());
                                                let cpu_cores = node.resources.cpu_cores;
                                                let memory_mb = node.resources.memory_mb;
                                                let disk_gb = node.resources.disk_gb;
                                                let architecture = node.architecture.clone();
                                                let cpu_usage = format!("{:.1}%", node.cpu_usage);
                                                let memory_usage = format!("{:.1}%", node.memory_usage);
                                                let disk_usage = format!("{:.1}%", node.disk_usage);
                                                let vm_count = node.vm_count;

                                                view! {
                                                    <tr>
                                                        <td class="px-6 py-4 whitespace-nowrap">
                                                            <div>
                                                                <div class="text-sm font-medium text-gray-900">{node_name}</div>
                                                                <div class="text-sm text-gray-500">{node_address}</div>
                                                                <div class="text-xs text-gray-400">"v" {node_version}</div>
                                                            </div>
                                                        </td>
                                                        <td class="px-6 py-4 whitespace-nowrap">
                                                            <div class="flex items-center">
                                                                <span class=format!("px-2 py-1 text-xs font-medium rounded {}", status_color)>
                                                                    {status_str}
                                                                </span>
                                                            </div>
                                                            <div class="text-xs text-gray-500 mt-1">{last_seen}</div>
                                                        </td>
                                                        <td class="px-6 py-4 whitespace-nowrap">
                                                            <div class="text-sm text-gray-900">
                                                                <div>{cpu_cores} " CPU cores"</div>
                                                                <div>{format_bytes(memory_mb)} " RAM"</div>
                                                                <div>{disk_gb} " GB disk"</div>
                                                            </div>
                                                            <div class="text-xs text-gray-500">
                                                                {architecture}
                                                            </div>
                                                        </td>
                                                        <td class="px-6 py-4 whitespace-nowrap">
                                                            <div class="text-sm">
                                                                <div class=format!("font-medium {}", cpu_color)>
                                                                    "CPU: " {cpu_usage}
                                                                </div>
                                                                <div class=format!("font-medium {}", memory_color)>
                                                                    "RAM: " {memory_usage}
                                                                </div>
                                                                <div class=format!("font-medium {}", disk_color)>
                                                                    "Disk: " {disk_usage}
                                                                </div>
                                                            </div>
                                                        </td>
                                                        <td class="px-6 py-4 whitespace-nowrap">
                                                            <div class="text-sm font-medium text-gray-900">{vm_count}</div>
                                                            <div class="text-xs text-gray-500">"VMs running"</div>
                                                        </td>
                                                        <td class="px-6 py-4 whitespace-nowrap text-sm font-medium">
                                                            <div class="flex space-x-2">
                                                                <button
                                                                    class="text-blue-600 hover:text-blue-900 px-2 py-1 rounded hover:bg-blue-50"
                                                                    title="Node Details"
                                                                >
                                                                    <i class="fas fa-eye"></i>
                                                                </button>
                                                                <button
                                                                    class="text-yellow-600 hover:text-yellow-900 px-2 py-1 rounded hover:bg-yellow-50"
                                                                    title="Maintenance Mode"
                                                                >
                                                                    <i class="fas fa-wrench"></i>
                                                                </button>
                                                                <button
                                                                    class="text-red-600 hover:text-red-900 px-2 py-1 rounded hover:bg-red-50"
                                                                    title="Remove Node"
                                                                >
                                                                    <i class="fas fa-times"></i>
                                                                </button>
                                                            </div>
                                                        </td>
                                                    </tr>
                                                }
                                        }).collect_view()
                                    }}
                                </tbody>
                            </table>
                        </div>
                    </div>
                }
            }}

            // Add Node Modal
            {move || if show_add_node_modal.get() {
                view! {
                    <div class="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50 p-4">
                        <div class="bg-white rounded-lg w-full max-w-lg">
                            <div class="p-6 border-b border-gray-200">
                                <h2 class="text-xl font-semibold">"Add Cluster Node"</h2>
                            </div>

                            <div class="p-6 space-y-4">
                                <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                                    <div>
                                        <label class="block text-sm font-medium text-gray-700 mb-1">
                                            "Node Name"
                                        </label>
                                        <input
                                            type="text"
                                            placeholder="node-02"
                                            prop:value=move || node_name.get()
                                            on:input=move |ev| set_node_name.set(event_target_value(&ev))
                                            class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                        />
                                    </div>
                                    <div>
                                        <label class="block text-sm font-medium text-gray-700 mb-1">
                                            "IP Address"
                                        </label>
                                        <input
                                            type="text"
                                            placeholder="192.168.1.100"
                                            prop:value=move || node_address.get()
                                            on:input=move |ev| set_node_address.set(event_target_value(&ev))
                                            class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                        />
                                    </div>
                                </div>

                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-1">
                                        "Architecture"
                                    </label>
                                    <select
                                        on:change=move |ev| set_node_architecture.set(event_target_value(&ev))
                                        class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                    >
                                        <option value="x86_64" selected=move || node_architecture.get() == "x86_64">"x86_64"</option>
                                        <option value="aarch64" selected=move || node_architecture.get() == "aarch64">"ARM64 (aarch64)"</option>
                                        <option value="riscv64" selected=move || node_architecture.get() == "riscv64">"RISC-V 64"</option>
                                    </select>
                                </div>

                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-1">
                                        "SSH Public Key (Optional)"
                                    </label>
                                    <textarea
                                        placeholder="ssh-rsa AAAAB3NzaC1yc2E..."
                                        prop:value=move || node_ssh_key.get()
                                        on:input=move |ev| set_node_ssh_key.set(event_target_value(&ev))
                                        class="w-full px-3 py-2 border border-gray-300 rounded-lg font-mono text-sm"
                                        rows="3"
                                    />
                                    <p class="text-xs text-gray-500 mt-1">
                                        "SSH key for secure communication with the node"
                                    </p>
                                </div>

                                <div class="bg-blue-50 border border-blue-200 rounded p-3">
                                    <div class="flex items-start">
                                        <i class="fas fa-info-circle text-blue-600 mt-1 mr-2"></i>
                                        <div class="text-sm text-blue-800">
                                            <p class="font-medium mb-1">"Prerequisites:"</p>
                                            <ul class="list-disc list-inside space-y-1">
                                                <li>"Node must have Horcrux agent installed"</li>
                                                <li>"SSH access configured for cluster communication"</li>
                                                <li>"Firewall ports opened for cluster traffic"</li>
                                                <li>"Sufficient resources for VM workloads"</li>
                                            </ul>
                                        </div>
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
                                    disabled=move || node_name.get().is_empty() || node_address.get().is_empty()
                                >
                                    "Add Node"
                                </button>
                            </div>
                        </div>
                    </div>
                }
            } else {
                view! { <div></div> }
            }}

            // Node Finder Modal
            {move || if show_node_finder_modal.get() {
                view! {
                    <div class="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50 p-4">
                        <div class="bg-white rounded-lg w-full max-w-lg">
                            <div class="p-6 border-b border-gray-200">
                                <h2 class="text-xl font-semibold">"Find Best Node for VM"</h2>
                            </div>

                            <div class="p-6 space-y-4">
                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-1">
                                        "Select VM"
                                    </label>
                                    <select
                                        on:change=move |ev| set_selected_vm_id.set(event_target_value(&ev))
                                        class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                    >
                                        <option value="">"Select a VM..."</option>
                                        {vms.get().into_iter().map(|vm| {
                                            let vm_id = vm.id.clone();
                                            let vm_status = format!("{:?}", vm.status);
                                            let display = format!("{} ({})", vm.name, vm_status);
                                            view! {
                                                <option value={vm_id}>
                                                    {display}
                                                </option>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </select>
                                </div>

                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-1">
                                        "Preferred Architecture (Optional)"
                                    </label>
                                    <select
                                        on:change=move |ev| set_preferred_arch.set(event_target_value(&ev))
                                        class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                    >
                                        <option value="">"Any Architecture"</option>
                                        <option value="x86_64">"x86_64"</option>
                                        <option value="aarch64">"ARM64 (aarch64)"</option>
                                        <option value="riscv64">"RISC-V 64"</option>
                                    </select>
                                </div>

                                <button
                                    on:click=move |_| find_best_node()
                                    class="w-full bg-purple-500 hover:bg-purple-600 text-white px-4 py-2 rounded-lg"
                                    disabled=move || selected_vm_id.get().is_empty()
                                >
                                    <i class="fas fa-search mr-2"></i>
                                    "Find Best Node"
                                </button>

                                // Recommendations
                                {move || if !node_recommendations.get().is_empty() {
                                    view! {
                                        <div class="mt-4 space-y-3">
                                            <h3 class="font-medium text-gray-900">"Recommendations:"</h3>
                                            {node_recommendations.get().into_iter().map(|rec| {
                                                let node_name = rec.node_name.clone();
                                                let score = format!("{:.1}", rec.score);
                                                let reason = rec.reason.clone();
                                                let cpu_cores = rec.resources_available.cpu_cores;
                                                let memory_mb = rec.resources_available.memory_mb;
                                                let disk_gb = rec.resources_available.disk_gb;
                                                view! {
                                                    <div class="border border-gray-200 rounded-lg p-4 bg-green-50">
                                                        <div class="flex items-center justify-between mb-2">
                                                            <h4 class="font-medium text-gray-900">{node_name}</h4>
                                                            <span class="text-sm font-medium text-green-600">
                                                                "Score: " {score}
                                                            </span>
                                                        </div>
                                                        <p class="text-sm text-gray-700 mb-2">{reason}</p>
                                                        <div class="text-xs text-gray-600">
                                                            "Available: " {cpu_cores} " CPU, "
                                                            {format_bytes(memory_mb)} " RAM, "
                                                            {disk_gb} " GB disk"
                                                        </div>
                                                    </div>
                                                }
                                            }).collect::<Vec<_>>()}
                                        </div>
                                    }
                                } else {
                                    view! { <div></div> }
                                }}
                            </div>

                            <div class="p-6 border-t border-gray-200 flex justify-end space-x-3">
                                <button
                                    on:click=move |_| {
                                        set_show_node_finder_modal.set(false);
                                        set_node_recommendations.set(Vec::new());
                                        set_selected_vm_id.set(String::new());
                                        set_preferred_arch.set(String::new());
                                    }
                                    class="px-4 py-2 text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50"
                                >
                                    "Close"
                                </button>
                            </div>
                        </div>
                    </div>
                }
            } else {
                view! { <div></div> }
            }}
        </div>
    }
}