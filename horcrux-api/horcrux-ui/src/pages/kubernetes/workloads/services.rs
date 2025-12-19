//! Service Management Page
//!
//! Provides comprehensive service management interface including:
//! - Service listing with endpoint details
//! - Service creation and configuration
//! - Port and selector management
//! - Load balancer configuration

use leptos::*;
use leptos_router::*;
use crate::api::{self, KubernetesService, CreateServiceRequest, ServicePort};
use std::collections::HashMap;

#[component]
pub fn ServicesPage() -> impl IntoView {
    let params = use_params_map();
    let cluster_id = move || params.with(|p| p.get("cluster_id").cloned().unwrap_or_default());
    let namespace = move || params.with(|p| p.get("namespace").cloned().unwrap_or("default".to_string()));

    let (services, set_services) = create_signal::<Vec<KubernetesService>>(vec![]);
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal::<Option<String>>(None);
    let (show_create_modal, set_show_create_modal) = create_signal(false);
    let (creating, set_creating) = create_signal(false);
    let (search_filter, set_search_filter) = create_signal(String::new());
    let (service_type_filter, set_service_type_filter) = create_signal(String::new());
    let (auto_refresh, set_auto_refresh) = create_signal(true);

    // Create service form fields
    let (service_name, set_service_name) = create_signal(String::new());
    let (service_type, set_service_type) = create_signal("ClusterIP".to_string());
    let (service_ports, set_service_ports) = create_signal::<Vec<ServicePort>>(vec![ServicePort {
        name: None,
        protocol: "TCP".to_string(),
        port: 80,
        target_port: Some("80".to_string()),
        node_port: None,
    }]);
    let (service_selectors, set_service_selectors) = create_signal::<Vec<(String, String)>>(vec![("app".to_string(), "".to_string())]);

    // Reset form helper
    let reset_create_form = move || {
        set_service_name.set(String::new());
        set_service_type.set("ClusterIP".to_string());
        set_service_ports.set(vec![ServicePort {
            name: None,
            protocol: "TCP".to_string(),
            port: 80,
            target_port: Some("80".to_string()),
            node_port: None,
        }]);
        set_service_selectors.set(vec![("app".to_string(), "".to_string())]);
    };

    // Load services
    let load_services = {
        let cluster_id = cluster_id.clone();
        let namespace = namespace.clone();
        move || {
            let cluster_id = cluster_id();
            let namespace = namespace();
            if cluster_id.is_empty() {
                return;
            }

            set_loading.set(true);
            spawn_local(async move {
                match api::get_services(&cluster_id, &namespace).await {
                    Ok(data) => {
                        set_services.set(data);
                        set_error.set(None);
                    }
                    Err(e) => {
                        set_error.set(Some(e.message));
                    }
                }
                set_loading.set(false);
            });
        }
    };

    // Auto-refresh effect
    create_effect(move |_| {
        if auto_refresh.get() {
            load_services();
            set_interval_with_handle(
                move || {
                    if auto_refresh.get() {
                        load_services();
                    }
                },
                std::time::Duration::from_secs(15),
            ).ok();
        }
    });

    // Initial load
    create_effect(move |_| {
        load_services();
    });

    // Filter services
    let filtered_services = move || {
        let search = search_filter.get().to_lowercase();
        let type_filter = service_type_filter.get();

        services.get()
            .into_iter()
            .filter(|service| {
                let name_match = search.is_empty() || service.name.to_lowercase().contains(&search);
                let type_match = type_filter.is_empty() || service.service_type.eq_ignore_ascii_case(&type_filter);
                name_match && type_match
            })
            .collect::<Vec<_>>()
    };

    // Create service
    let create_service = {
        let cluster_id = cluster_id.clone();
        let namespace = namespace.clone();
        move || {
            let cluster_id = cluster_id();
            let namespace = namespace();
            let name = service_name.get();
            let svc_type = service_type.get();
            let ports = service_ports.get();
            let selectors: HashMap<String, String> = service_selectors.get()
                .into_iter()
                .filter(|(k, v)| !k.is_empty() && !v.is_empty())
                .collect();

            if name.is_empty() {
                set_error.set(Some("Service name is required".to_string()));
                return;
            }

            if selectors.is_empty() {
                set_error.set(Some("At least one selector is required".to_string()));
                return;
            }

            set_creating.set(true);
            spawn_local(async move {
                let request = CreateServiceRequest {
                    name,
                    service_type: svc_type,
                    ports,
                    selector: selectors,
                    labels: None,
                };

                match api::create_service(&cluster_id, &namespace, request).await {
                    Ok(_) => {
                        set_show_create_modal.set(false);
                        load_services();
                        reset_create_form();
                        set_error.set(None);
                    }
                    Err(e) => {
                        set_error.set(Some(format!("Failed to create service: {}", e.message)));
                    }
                }
                set_creating.set(false);
            });
        }
    };

    // Delete service
    let delete_service = {
        let cluster_id = cluster_id.clone();
        let namespace = namespace.clone();
        move |service_name: String| {
            let cluster_id = cluster_id();
            let namespace = namespace();
            spawn_local(async move {
                match api::delete_service(&cluster_id, &namespace, &service_name).await {
                    Ok(()) => {
                        load_services();
                        set_error.set(None);
                    }
                    Err(e) => {
                        set_error.set(Some(format!("Failed to delete service: {}", e.message)));
                    }
                }
            });
        }
    };

    // Reset create form
    let reset_create_form = move || {
        set_service_name.set(String::new());
        set_service_type.set("ClusterIP".to_string());
        set_service_ports.set(vec![ServicePort {
            name: None,
            protocol: "TCP".to_string(),
            port: 80,
            target_port: Some("80".to_string()),
            node_port: None,
        }]);
        set_service_selectors.set(vec![("app".to_string(), "".to_string())]);
    };

    // Add port to service
    let add_service_port = move || {
        let mut ports = service_ports.get();
        ports.push(ServicePort {
            name: None,
            protocol: "TCP".to_string(),
            port: 80,
            target_port: Some("80".to_string()),
            node_port: None,
        });
        set_service_ports.set(ports);
    };

    // Remove port from service
    let remove_service_port = move |index: usize| {
        let mut ports = service_ports.get();
        if ports.len() > 1 {
            ports.remove(index);
            set_service_ports.set(ports);
        }
    };

    // Add selector
    let add_selector = move || {
        let mut selectors = service_selectors.get();
        selectors.push(("".to_string(), "".to_string()));
        set_service_selectors.set(selectors);
    };

    // Remove selector
    let remove_selector = move |index: usize| {
        let mut selectors = service_selectors.get();
        if selectors.len() > 1 {
            selectors.remove(index);
            set_service_selectors.set(selectors);
        }
    };

    // Format external IPs display
    let format_external_ips = |external_ips: &Vec<String>| {
        if external_ips.is_empty() {
            "<none>".to_string()
        } else {
            external_ips.join(", ")
        }
    };

    // Format ports display
    let format_ports = |ports: &Vec<ServicePort>| {
        ports
            .iter()
            .map(|port| {
                let mut port_str = format!("{}:{}", port.port, port.target_port.as_ref().unwrap_or(&"<unknown>".to_string()));
                if let Some(node_port) = port.node_port {
                    port_str.push_str(&format!(":{}", node_port));
                }
                format!("{}/{}", port_str, port.protocol)
            })
            .collect::<Vec<_>>()
            .join(", ")
    };

    view! {
        <div class="p-6 space-y-6">
            <div class="flex justify-between items-center">
                <div>
                    <h1 class="text-2xl font-bold text-gray-900">Services</h1>
                    <p class="mt-1 text-sm text-gray-500">
                        "Cluster: " {cluster_id} " | Namespace: " {namespace}
                    </p>
                </div>
                <div class="flex items-center space-x-4">
                    <label class="flex items-center">
                        <input
                            type="checkbox"
                            class="rounded border-gray-300"
                            prop:checked=auto_refresh
                            on:change=move |ev| set_auto_refresh.set(event_target_checked(&ev))
                        />
                        <span class="ml-2 text-sm text-gray-700">"Auto Refresh"</span>
                    </label>
                    <button
                        on:click=move |_| {
                            reset_create_form();
                            set_show_create_modal.set(true);
                        }
                        class="px-4 py-2 bg-green-600 text-white rounded-lg hover:bg-green-700"
                    >
                        "Create Service"
                    </button>
                    <button
                        on:click=move |_| load_services()
                        disabled=loading
                        class="px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed"
                    >
                        {move || if loading.get() { "Loading..." } else { "Refresh" }}
                    </button>
                </div>
            </div>

            // Filters
            <div class="bg-white p-4 rounded-lg shadow space-y-4">
                <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-2">
                            "Search Services"
                        </label>
                        <input
                            type="text"
                            placeholder="Filter by service name..."
                            class="w-full px-3 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                            prop:value=search_filter
                            on:input=move |ev| set_search_filter.set(event_target_value(&ev))
                        />
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-2">
                            "Service Type"
                        </label>
                        <select
                            class="w-full px-3 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                            on:change=move |ev| set_service_type_filter.set(event_target_value(&ev))
                        >
                            <option value="">"All Types"</option>
                            <option value="ClusterIP">"ClusterIP"</option>
                            <option value="NodePort">"NodePort"</option>
                            <option value="LoadBalancer">"LoadBalancer"</option>
                            <option value="ExternalName">"ExternalName"</option>
                        </select>
                    </div>
                </div>
            </div>

            // Error display
            {move || error.get().map(|err| view! {
                <div class="bg-red-50 border border-red-200 rounded-lg p-4">
                    <div class="flex">
                        <div class="ml-3">
                            <h3 class="text-sm font-medium text-red-800">"Error"</h3>
                            <div class="mt-2 text-sm text-red-700">
                                <p>{err}</p>
                            </div>
                        </div>
                    </div>
                </div>
            })}

            // Services table
            <div class="bg-white rounded-lg shadow overflow-hidden">
                <div class="overflow-x-auto">
                    <table class="min-w-full divide-y divide-gray-200">
                        <thead class="bg-gray-50">
                            <tr>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Name"
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Type"
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Cluster IP"
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "External IP"
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Ports"
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Age"
                                </th>
                                <th class="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Actions"
                                </th>
                            </tr>
                        </thead>
                        <tbody class="bg-white divide-y divide-gray-200">
                            {filtered_services().into_iter().map(|service| {
                                let service_for_delete = service.clone();
                                let svc_name = service.name.clone();
                                let namespace = service.namespace.clone();
                                let svc_type = service.service_type.clone();
                                let cluster_ip = service.cluster_ip.clone();
                                let ext_ips = format_external_ips(&service.external_ips);
                                let ports_str = format_ports(&service.ports);
                                let ports_title = ports_str.clone();
                                let age = service.age.clone();

                                view! {
                                    <tr class="hover:bg-gray-50">
                                        <td class="px-6 py-4 whitespace-nowrap">
                                            <div class="text-sm font-medium text-gray-900">{svc_name}</div>
                                            <div class="text-sm text-gray-500">{namespace}</div>
                                        </td>
                                        <td class="px-6 py-4 whitespace-nowrap">
                                            <span class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-blue-100 text-blue-800">
                                                {svc_type}
                                            </span>
                                        </td>
                                        <td class="px-6 py-4 whitespace-nowrap text-sm text-gray-900">
                                            {cluster_ip}
                                        </td>
                                        <td class="px-6 py-4 whitespace-nowrap text-sm text-gray-900">
                                            {ext_ips}
                                        </td>
                                        <td class="px-6 py-4 whitespace-nowrap text-sm text-gray-900">
                                            <div class="max-w-xs truncate" title={ports_title}>
                                                {ports_str}
                                            </div>
                                        </td>
                                        <td class="px-6 py-4 whitespace-nowrap text-sm text-gray-900">
                                            {age}
                                        </td>
                                        <td class="px-6 py-4 whitespace-nowrap text-right text-sm font-medium">
                                            <button
                                                on:click=move |_| {
                                                    if window().confirm_with_message(&format!("Are you sure you want to delete service '{}'?", service_for_delete.name)).unwrap_or(false) {
                                                        delete_service(service_for_delete.name.clone());
                                                    }
                                                }
                                                class="text-red-600 hover:text-red-900"
                                                title="Delete Service"
                                            >
                                                "Delete"
                                            </button>
                                        </td>
                                    </tr>
                                }
                            }).collect::<Vec<_>>()}

                            {move || {
                                if !loading.get() && filtered_services().is_empty() {
                                    view! {
                                        <tr>
                                            <td colspan="7" class="px-6 py-12 text-center text-sm text-gray-500">
                                                "No services found matching the current filters."
                                            </td>
                                        </tr>
                                    }.into_view()
                                } else {
                                    view! {}.into_view()
                                }
                            }}
                        </tbody>
                    </table>
                </div>

                {move || loading.get().then(|| view! {
                    <div class="px-6 py-12 text-center">
                        <div class="inline-flex items-center">
                            <div class="animate-spin -ml-1 mr-3 h-5 w-5 text-blue-500">
                                <svg class="w-5 h-5" fill="none" viewBox="0 0 24 24">
                                    <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4"></circle>
                                    <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"></path>
                                </svg>
                            </div>
                            "Loading services..."
                        </div>
                    </div>
                })}
            </div>

            // Create Service Modal
            {move || show_create_modal.get().then(|| view! {
                <div class="fixed inset-0 bg-gray-600 bg-opacity-50 overflow-y-auto h-full w-full z-50">
                    <div class="relative top-10 mx-auto p-5 border w-11/12 max-w-2xl shadow-lg rounded-md bg-white">
                        <div class="flex justify-between items-center mb-4">
                            <h3 class="text-lg font-bold text-gray-900">"Create Service"</h3>
                            <button
                                on:click=move |_| set_show_create_modal.set(false)
                                class="text-gray-400 hover:text-gray-600"
                            >
                                <svg class="w-6 h-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"></path>
                                </svg>
                            </button>
                        </div>

                        <div class="space-y-4 max-h-96 overflow-y-auto">
                            // Basic service info
                            <div class="grid grid-cols-2 gap-4">
                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-2">
                                        "Service Name"
                                    </label>
                                    <input
                                        type="text"
                                        required
                                        class="w-full px-3 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                                        prop:value=service_name
                                        on:input=move |ev| set_service_name.set(event_target_value(&ev))
                                    />
                                </div>
                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-2">
                                        "Service Type"
                                    </label>
                                    <select
                                        class="w-full px-3 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                                        on:change=move |ev| set_service_type.set(event_target_value(&ev))
                                    >
                                        <option value="ClusterIP" selected={service_type.get() == "ClusterIP"}>"ClusterIP"</option>
                                        <option value="NodePort" selected={service_type.get() == "NodePort"}>"NodePort"</option>
                                        <option value="LoadBalancer" selected={service_type.get() == "LoadBalancer"}>"LoadBalancer"</option>
                                    </select>
                                </div>
                            </div>

                            // Ports section
                            <div>
                                <div class="flex justify-between items-center mb-2">
                                    <label class="text-sm font-medium text-gray-700">"Ports"</label>
                                    <button
                                        on:click=move |_| add_service_port()
                                        class="px-2 py-1 bg-blue-600 text-white text-xs rounded hover:bg-blue-700"
                                    >
                                        "Add Port"
                                    </button>
                                </div>
                                <div class="space-y-2">
                                    {service_ports.get().into_iter().enumerate().map(|(index, port)| {
                                        let port_val = port.port;
                                        let target_port_val = port.target_port.clone().unwrap_or_default();
                                        let protocol = port.protocol.clone();
                                        let is_tcp = protocol == "TCP";
                                        let is_udp = protocol == "UDP";
                                        view! {
                                            <div class="flex items-center space-x-2 p-2 border border-gray-200 rounded">
                                                <input
                                                    type="number"
                                                    placeholder="Port"
                                                    min="1"
                                                    max="65535"
                                                    class="flex-1 px-2 py-1 border border-gray-300 rounded text-sm"
                                                    prop:value=port_val
                                                    on:input=move |ev| {
                                                        if let Ok(value) = event_target_value(&ev).parse::<u16>() {
                                                            let mut ports = service_ports.get();
                                                            if let Some(p) = ports.get_mut(index) {
                                                                p.port = value;
                                                                set_service_ports.set(ports);
                                                            }
                                                        }
                                                    }
                                                />
                                                <input
                                                    type="text"
                                                    placeholder="Target Port"
                                                    class="flex-1 px-2 py-1 border border-gray-300 rounded text-sm"
                                                    prop:value=target_port_val
                                                    on:input=move |ev| {
                                                        let value = event_target_value(&ev);
                                                        let mut ports = service_ports.get();
                                                        if let Some(p) = ports.get_mut(index) {
                                                            p.target_port = if value.is_empty() { None } else { Some(value) };
                                                            set_service_ports.set(ports);
                                                        }
                                                    }
                                                />
                                                <select
                                                    class="px-2 py-1 border border-gray-300 rounded text-sm"
                                                    on:change=move |ev| {
                                                        let value = event_target_value(&ev);
                                                        let mut ports = service_ports.get();
                                                        if let Some(p) = ports.get_mut(index) {
                                                            p.protocol = value;
                                                            set_service_ports.set(ports);
                                                        }
                                                    }
                                                >
                                                    <option value="TCP" selected=is_tcp>"TCP"</option>
                                                    <option value="UDP" selected=is_udp>"UDP"</option>
                                                </select>
                                                {move || (service_ports.get().len() > 1).then(|| view! {
                                                    <button
                                                        on:click=move |_| remove_service_port(index)
                                                        class="px-2 py-1 bg-red-600 text-white text-xs rounded hover:bg-red-700"
                                                    >
                                                        "Remove"
                                                    </button>
                                                })}
                                            </div>
                                        }
                                    }).collect::<Vec<_>>()}
                                </div>
                            </div>

                            // Selectors section
                            <div>
                                <div class="flex justify-between items-center mb-2">
                                    <label class="text-sm font-medium text-gray-700">"Selectors"</label>
                                    <button
                                        on:click=move |_| add_selector()
                                        class="px-2 py-1 bg-blue-600 text-white text-xs rounded hover:bg-blue-700"
                                    >
                                        "Add Selector"
                                    </button>
                                </div>
                                <div class="space-y-2">
                                    {service_selectors.get().into_iter().enumerate().map(|(index, (key, value))| {
                                        let key_val = key.clone();
                                        let value_val = value.clone();
                                        view! {
                                            <div class="flex items-center space-x-2">
                                                <input
                                                    type="text"
                                                    placeholder="Key (e.g., app)"
                                                    class="flex-1 px-2 py-1 border border-gray-300 rounded text-sm"
                                                    prop:value=key_val
                                                    on:input=move |ev| {
                                                        let new_key = event_target_value(&ev);
                                                        let mut selectors = service_selectors.get();
                                                        if let Some((k, _)) = selectors.get_mut(index) {
                                                            *k = new_key;
                                                            set_service_selectors.set(selectors);
                                                        }
                                                    }
                                                />
                                                <input
                                                    type="text"
                                                    placeholder="Value (e.g., frontend)"
                                                    class="flex-1 px-2 py-1 border border-gray-300 rounded text-sm"
                                                    prop:value=value_val
                                                    on:input=move |ev| {
                                                        let new_value = event_target_value(&ev);
                                                        let mut selectors = service_selectors.get();
                                                        if let Some((_, v)) = selectors.get_mut(index) {
                                                            *v = new_value;
                                                            set_service_selectors.set(selectors);
                                                        }
                                                    }
                                                />
                                                {move || (service_selectors.get().len() > 1).then(|| view! {
                                                    <button
                                                        on:click=move |_| remove_selector(index)
                                                        class="px-2 py-1 bg-red-600 text-white text-xs rounded hover:bg-red-700"
                                                    >
                                                        "Remove"
                                                    </button>
                                                })}
                                            </div>
                                        }
                                    }).collect::<Vec<_>>()}
                                </div>
                            </div>
                        </div>

                        <div class="flex justify-end space-x-2 pt-4 mt-4 border-t">
                            <button
                                on:click=move |_| set_show_create_modal.set(false)
                                disabled=creating
                                class="px-4 py-2 bg-gray-300 text-gray-700 rounded-lg hover:bg-gray-400 disabled:opacity-50"
                            >
                                "Cancel"
                            </button>
                            <button
                                on:click=move |_| create_service()
                                disabled=creating
                                class="px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 disabled:opacity-50"
                            >
                                {move || if creating.get() { "Creating..." } else { "Create Service" }}
                            </button>
                        </div>
                    </div>
                </div>
            })}

            // Statistics footer
            <div class="bg-white rounded-lg shadow p-4">
                <div class="grid grid-cols-2 md:grid-cols-4 gap-4 text-center">
                    <div>
                        <div class="text-2xl font-bold text-gray-900">{move || filtered_services().len()}</div>
                        <div class="text-sm text-gray-500">"Total Services"</div>
                    </div>
                    <div>
                        <div class="text-2xl font-bold text-blue-600">
                            {move || filtered_services().iter().filter(|s| s.service_type == "ClusterIP").count()}
                        </div>
                        <div class="text-sm text-gray-500">"ClusterIP"</div>
                    </div>
                    <div>
                        <div class="text-2xl font-bold text-green-600">
                            {move || filtered_services().iter().filter(|s| s.service_type == "LoadBalancer").count()}
                        </div>
                        <div class="text-sm text-gray-500">"LoadBalancer"</div>
                    </div>
                    <div>
                        <div class="text-2xl font-bold text-purple-600">
                            {move || filtered_services().iter().filter(|s| s.service_type == "NodePort").count()}
                        </div>
                        <div class="text-sm text-gray-500">"NodePort"</div>
                    </div>
                </div>
            </div>
        </div>
    }
}