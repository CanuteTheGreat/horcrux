use leptos::*;
use crate::api::{
    ClusterNode, ClusterArchitecture, HaStatus, HaResource, HaGroup, MigrationJob,
    get_cluster_nodes, get_cluster_architecture, get_ha_status, get_ha_resources,
    get_ha_groups, HaClusterStatus, HaResourceState, HealthCheckStatus
};

#[component]
pub fn HaDashboard() -> impl IntoView {
    let (cluster_nodes, set_cluster_nodes) = create_signal(Vec::<ClusterNode>::new());
    let (cluster_architecture, set_cluster_architecture) = create_signal(None::<ClusterArchitecture>);
    let (ha_status, set_ha_status) = create_signal(None::<HaStatus>);
    let (ha_resources, set_ha_resources) = create_signal(Vec::<HaResource>::new());
    let (ha_groups, set_ha_groups) = create_signal(Vec::<HaGroup>::new());
    let (active_migrations, set_active_migrations) = create_signal(Vec::<MigrationJob>::new());
    let (loading, set_loading) = create_signal(false);
    let (error, set_error) = create_signal(None::<String>);

    let load_dashboard_data = move || {
        set_loading.set(true);
        set_error.set(None);

        spawn_local(async move {
            let mut errors = Vec::new();

            // Load cluster nodes
            match get_cluster_nodes().await {
                Ok(nodes) => set_cluster_nodes.set(nodes),
                Err(e) => errors.push(format!("Failed to load cluster nodes: {}", e)),
            }

            // Load cluster architecture
            match get_cluster_architecture().await {
                Ok(arch) => set_cluster_architecture.set(Some(arch)),
                Err(e) => errors.push(format!("Failed to load cluster architecture: {}", e)),
            }

            // Load HA status
            match get_ha_status().await {
                Ok(status) => set_ha_status.set(Some(status)),
                Err(e) => errors.push(format!("Failed to load HA status: {}", e)),
            }

            // Load HA resources
            match get_ha_resources().await {
                Ok(resources) => set_ha_resources.set(resources),
                Err(e) => errors.push(format!("Failed to load HA resources: {}", e)),
            }

            // Load HA groups
            match get_ha_groups().await {
                Ok(groups) => set_ha_groups.set(groups),
                Err(e) => errors.push(format!("Failed to load HA groups: {}", e)),
            }

            if !errors.is_empty() {
                set_error.set(Some(errors.join("; ")));
            }
            set_loading.set(false);
        });
    };

    // Load data on mount
    create_effect(move |_| {
        load_dashboard_data();
    });

    // Auto-refresh every 30 seconds
    use leptos::set_interval;
    set_interval(
        move || load_dashboard_data(),
        std::time::Duration::from_secs(30),
    );

    let get_node_status_color = move |status: &str| {
        match status.to_lowercase().as_str() {
            "online" => "bg-green-100 text-green-800",
            "offline" => "bg-red-100 text-red-800",
            "maintenance" => "bg-yellow-100 text-yellow-800",
            _ => "bg-gray-100 text-gray-800",
        }
    };

    let get_ha_status_color = move |status: &HaClusterStatus| {
        match status {
            HaClusterStatus::Active => "bg-green-100 text-green-800",
            HaClusterStatus::Degraded => "bg-yellow-100 text-yellow-800",
            HaClusterStatus::Failed => "bg-red-100 text-red-800",
            HaClusterStatus::Maintenance => "bg-blue-100 text-blue-800",
        }
    };

    let get_health_status_color = move |status: &HealthCheckStatus| {
        match status {
            HealthCheckStatus::Healthy => "bg-green-100 text-green-800",
            HealthCheckStatus::Warning => "bg-yellow-100 text-yellow-800",
            HealthCheckStatus::Critical => "bg-red-100 text-red-800",
            HealthCheckStatus::Unknown => "bg-gray-100 text-gray-800",
        }
    };

    let calculate_cluster_health = move || {
        let nodes = cluster_nodes.get();
        if nodes.is_empty() {
            return ("Unknown", "bg-gray-100 text-gray-800");
        }

        let total = nodes.len();
        let online = nodes.iter().filter(|n| n.status.to_lowercase() == "online").count();
        let health_percentage = (online as f64 / total as f64) * 100.0;

        match health_percentage {
            p if p >= 90.0 => ("Excellent", "bg-green-100 text-green-800"),
            p if p >= 75.0 => ("Good", "bg-blue-100 text-blue-800"),
            p if p >= 50.0 => ("Degraded", "bg-yellow-100 text-yellow-800"),
            _ => ("Critical", "bg-red-100 text-red-800"),
        }
    };

    view! {
        <div class="p-6">
            <div class="flex justify-between items-center mb-6">
                <div>
                    <h1 class="text-2xl font-bold">High Availability & Clustering</h1>
                    <p class="text-gray-600">
                        "Monitor and manage cluster health, HA resources, and VM migrations"
                    </p>
                </div>
                <div class="flex space-x-3">
                    <button
                        on:click=move |_| load_dashboard_data()
                        class="bg-gray-500 hover:bg-gray-600 text-white px-4 py-2 rounded-lg"
                    >
                        <i class="fas fa-sync mr-2"></i>
                        "Refresh"
                    </button>
                    <a href="/ha/cluster" class="bg-blue-500 hover:bg-blue-600 text-white px-4 py-2 rounded-lg">
                        "Manage Cluster"
                    </a>
                </div>
            </div>

            // Error display
            {move || error.get().map(|e| view! {
                <div class="bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded mb-6">
                    <i class="fas fa-exclamation-triangle mr-2"></i>
                    {e}
                </div>
            })}

            // Loading state
            {move || if loading.get() {
                view! {
                    <div class="bg-white rounded-lg shadow p-8 text-center">
                        <i class="fas fa-spinner fa-spin text-2xl text-gray-400 mb-2"></i>
                        <p class="text-gray-600">"Loading HA dashboard..."</p>
                    </div>
                }
            } else {
                view! {
                    <div class="space-y-6">
                        // Cluster Health Overview
                        <div class="grid grid-cols-1 md:grid-cols-4 gap-6">
                            // Cluster Health
                            <div class="bg-white rounded-lg shadow p-6">
                                <div class="flex items-center">
                                    <div class="flex-shrink-0">
                                        <i class="fas fa-heartbeat text-2xl text-red-500"></i>
                                    </div>
                                    <div class="ml-4">
                                        <p class="text-sm font-medium text-gray-500">"Cluster Health"</p>
                                        <div class="flex items-center">
                                            <span class=format!("px-2 py-1 text-xs rounded mr-2 {}", calculate_cluster_health().1)>
                                                {calculate_cluster_health().0}
                                            </span>
                                        </div>
                                    </div>
                                </div>
                            </div>

                            // Cluster Nodes
                            <div class="bg-white rounded-lg shadow p-6">
                                <div class="flex items-center">
                                    <div class="flex-shrink-0">
                                        <i class="fas fa-server text-2xl text-blue-500"></i>
                                    </div>
                                    <div class="ml-4">
                                        <p class="text-sm font-medium text-gray-500">"Cluster Nodes"</p>
                                        <p class="text-2xl font-semibold text-gray-900">
                                            {cluster_nodes.get().len()}
                                        </p>
                                        <p class="text-xs text-gray-500">
                                            {cluster_nodes.get().iter().filter(|n| n.status.to_lowercase() == "online").count()}
                                            " online"
                                        </p>
                                    </div>
                                </div>
                            </div>

                            // HA Resources
                            <div class="bg-white rounded-lg shadow p-6">
                                <div class="flex items-center">
                                    <div class="flex-shrink-0">
                                        <i class="fas fa-shield-alt text-2xl text-green-500"></i>
                                    </div>
                                    <div class="ml-4">
                                        <p class="text-sm font-medium text-gray-500">"HA Protected VMs"</p>
                                        <p class="text-2xl font-semibold text-gray-900">
                                            {ha_resources.get().len()}
                                        </p>
                                        <p class="text-xs text-gray-500">
                                            {ha_resources.get().iter().filter(|r| r.enabled).count()}
                                            " active"
                                        </p>
                                    </div>
                                </div>
                            </div>

                            // HA Groups
                            <div class="bg-white rounded-lg shadow p-6">
                                <div class="flex items-center">
                                    <div class="flex-shrink-0">
                                        <i class="fas fa-layer-group text-2xl text-purple-500"></i>
                                    </div>
                                    <div class="ml-4">
                                        <p class="text-sm font-medium text-gray-500">"HA Groups"</p>
                                        <p class="text-2xl font-semibold text-gray-900">
                                            {ha_groups.get().len()}
                                        </p>
                                        <p class="text-xs text-gray-500">
                                            {ha_groups.get().iter().map(|g| g.vm_ids.len()).sum::<usize>()}
                                            " total VMs"
                                        </p>
                                    </div>
                                </div>
                            </div>
                        </div>

                        // HA Status Panel
                        {move || ha_status.get().map(|status| view! {
                            <div class="bg-white rounded-lg shadow">
                                <div class="p-6 border-b border-gray-200">
                                    <div class="flex items-center justify-between">
                                        <h2 class="text-lg font-semibold">HA Cluster Status</h2>
                                        <span class=format!("px-3 py-1 text-sm rounded {}", get_ha_status_color(&status.cluster_status))>
                                            {format!("{:?}", status.cluster_status)}
                                        </span>
                                    </div>
                                </div>
                                <div class="p-6">
                                    <div class="grid grid-cols-1 md:grid-cols-3 gap-6 mb-6">
                                        <div class="text-center">
                                            <div class="text-2xl font-semibold text-gray-900">{status.resources.len()}</div>
                                            <div class="text-sm text-gray-500">"Protected Resources"</div>
                                        </div>
                                        <div class="text-center">
                                            <div class="text-2xl font-semibold text-blue-600">{status.total_failovers}</div>
                                            <div class="text-sm text-gray-500">"Total Failovers"</div>
                                        </div>
                                        <div class="text-center">
                                            <div class="text-2xl font-semibold text-green-600">
                                                {status.resources.iter()
                                                    .filter(|r| matches!(r.status, HaResourceState::Running))
                                                    .count()
                                                }
                                            </div>
                                            <div class="text-sm text-gray-500">"Running Resources"</div>
                                        </div>
                                    </div>

                                    {status.last_failover.as_ref().map(|last| view! {
                                        <div class="bg-yellow-50 border border-yellow-200 rounded-lg p-4">
                                            <div class="flex items-center">
                                                <i class="fas fa-exclamation-triangle text-yellow-600 mr-2"></i>
                                                <div>
                                                    <p class="font-medium text-yellow-800">"Last Failover Event"</p>
                                                    <p class="text-sm text-yellow-700">{last}</p>
                                                </div>
                                            </div>
                                        </div>
                                    })}
                                </div>
                            </div>
                        })}

                        // Cluster Nodes Overview
                        <div class="bg-white rounded-lg shadow">
                            <div class="p-6 border-b border-gray-200">
                                <div class="flex items-center justify-between">
                                    <h2 class="text-lg font-semibold">Cluster Nodes</h2>
                                    <a href="/ha/cluster" class="text-blue-600 hover:text-blue-800 text-sm font-medium">
                                        "View All Nodes "->""
                                    </a>
                                </div>
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
                                                "Resource Usage"
                                            </th>
                                            <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                                "VMs"
                                            </th>
                                            <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                                "Architecture"
                                            </th>
                                        </tr>
                                    </thead>
                                    <tbody class="bg-white divide-y divide-gray-200">
                                        {move || if cluster_nodes.get().is_empty() {
                                            view! {
                                                <tr>
                                                    <td colspan="5" class="px-6 py-8 text-center text-gray-500">
                                                        <div>
                                                            <i class="fas fa-server text-4xl mb-4"></i>
                                                            <p class="text-lg">"No cluster nodes configured"</p>
                                                            <p class="text-sm">"Add nodes to enable clustering and HA"</p>
                                                        </div>
                                                    </td>
                                                </tr>
                                            }.into_view()
                                        } else {
                                            cluster_nodes.get().into_iter().take(5).map(|node| {
                                                let status_color = get_node_status_color(&node.status);
                                                let node_name = node.name.clone();
                                                let node_address = node.address.clone();
                                                let status_str = node.status.clone();
                                                let cpu = format!("{:.1}%", node.cpu_usage);
                                                let mem = format!("{:.1}%", node.memory_usage);
                                                let disk = format!("{:.1}%", node.disk_usage);
                                                let vm_count = node.vm_count;
                                                let arch = node.architecture.clone();
                                                view! {
                                                    <tr>
                                                        <td class="px-6 py-4 whitespace-nowrap">
                                                            <div>
                                                                <div class="text-sm font-medium text-gray-900">{node_name}</div>
                                                                <div class="text-sm text-gray-500">{node_address}</div>
                                                            </div>
                                                        </td>
                                                        <td class="px-6 py-4 whitespace-nowrap">
                                                            <span class=format!("px-2 py-1 text-xs font-medium rounded {}", status_color)>
                                                                {status_str}
                                                            </span>
                                                        </td>
                                                        <td class="px-6 py-4 whitespace-nowrap">
                                                            <div class="text-sm text-gray-900">
                                                                <div>"CPU: " {cpu}</div>
                                                                <div>"Memory: " {mem}</div>
                                                                <div>"Disk: " {disk}</div>
                                                            </div>
                                                        </td>
                                                        <td class="px-6 py-4 whitespace-nowrap">
                                                            <div class="text-sm font-medium text-gray-900">{vm_count}</div>
                                                        </td>
                                                        <td class="px-6 py-4 whitespace-nowrap">
                                                            <div class="text-sm text-gray-900">{arch}</div>
                                                        </td>
                                                    </tr>
                                                }
                                            }).collect_view()
                                        }}
                                    </tbody>
                                </table>
                            </div>
                        </div>

                        // HA Resources Overview
                        {move || if !ha_resources.get().is_empty() {
                            view! {
                                <div class="bg-white rounded-lg shadow">
                                    <div class="p-6 border-b border-gray-200">
                                        <div class="flex items-center justify-between">
                                            <h2 class="text-lg font-semibold">HA Protected Resources</h2>
                                            <a href="/ha/groups" class="text-blue-600 hover:text-blue-800 text-sm font-medium">
                                                Manage HA Groups ->
                                            </a>
                                        </div>
                                    </div>
                                    <div class="p-6">
                                        {move || ha_status.get().map(|status| view! {
                                            <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
                                                {status.resources.clone().into_iter().take(6).map(|resource| {
                                                    let health_color = get_health_status_color(&resource.health_check);
                                                    let vm_id = resource.vm_id.clone();
                                                    let health_check_str = format!("{:?}", resource.health_check);
                                                    let current_node = resource.current_node.clone();
                                                    let status_str = format!("{:?}", resource.status);
                                                    let restart_count = resource.restart_count;
                                                    let last_migration = resource.last_migration.clone();
                                                    view! {
                                                        <div class="border border-gray-200 rounded-lg p-4">
                                                            <div class="flex items-center justify-between mb-2">
                                                                <h3 class="font-medium text-gray-900">{vm_id}</h3>
                                                                <span class=format!("px-2 py-1 text-xs rounded {}", health_color)>
                                                                    {health_check_str}
                                                                </span>
                                                            </div>
                                                            <div class="text-sm text-gray-600 space-y-1">
                                                                <div>
                                                                    <span class="font-medium">"Node: "</span>
                                                                    {current_node}
                                                                </div>
                                                                <div>
                                                                    <span class="font-medium">"Status: "</span>
                                                                    {status_str}
                                                                </div>
                                                                <div>
                                                                    <span class="font-medium">"Restarts: "</span>
                                                                    {restart_count}
                                                                </div>
                                                                {last_migration.map(|migration| view! {
                                                                    <div class="text-xs text-gray-500">
                                                                        "Last migration: " {migration}
                                                                    </div>
                                                                })}
                                                            </div>
                                                        </div>
                                                    }
                                                }).collect::<Vec<_>>()}
                                            </div>
                                        })}
                                    </div>
                                </div>
                            }
                        } else {
                            view! { <div></div> }
                        }}

                        // Quick Actions
                        <div class="bg-white rounded-lg shadow">
                            <div class="p-6 border-b border-gray-200">
                                <h2 class="text-lg font-semibold">Quick Actions</h2>
                            </div>
                            <div class="p-6">
                                <div class="grid grid-cols-1 md:grid-cols-3 gap-6">
                                    <a href="/ha/cluster" class="block p-6 border border-gray-200 rounded-lg hover:shadow-md transition-shadow">
                                        <div class="flex items-center">
                                            <i class="fas fa-server text-2xl text-blue-500 mr-4"></i>
                                            <div>
                                                <h3 class="font-medium text-gray-900">"Manage Cluster"</h3>
                                                <p class="text-sm text-gray-600">"Add/remove nodes and configure cluster settings"</p>
                                            </div>
                                        </div>
                                    </a>

                                    <a href="/ha/groups" class="block p-6 border border-gray-200 rounded-lg hover:shadow-md transition-shadow">
                                        <div class="flex items-center">
                                            <i class="fas fa-shield-alt text-2xl text-green-500 mr-4"></i>
                                            <div>
                                                <h3 class="font-medium text-gray-900">"Configure HA"</h3>
                                                <p class="text-sm text-gray-600">"Set up HA groups and protection policies"</p>
                                            </div>
                                        </div>
                                    </a>

                                    <a href="/ha/migration" class="block p-6 border border-gray-200 rounded-lg hover:shadow-md transition-shadow">
                                        <div class="flex items-center">
                                            <i class="fas fa-exchange-alt text-2xl text-purple-500 mr-4"></i>
                                            <div>
                                                <h3 class="font-medium text-gray-900">"Migration Center"</h3>
                                                <p class="text-sm text-gray-600">"Plan and execute VM migrations"</p>
                                            </div>
                                        </div>
                                    </a>
                                </div>
                            </div>
                        </div>
                    </div>
                }
            }}
        </div>
    }
}