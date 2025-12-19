//! Ingress Management Page
//!
//! Provides comprehensive ingress management interface including:
//! - Ingress listing with host and path details
//! - Ingress creation with rule configuration
//! - TLS certificate management
//! - Annotation and label management

use leptos::*;
use leptos_router::*;
use crate::api::{self, KubernetesIngress, CreateIngressRequest, IngressRule, IngressPath, IngressTLS};
use std::collections::HashMap;

#[component]
pub fn IngressesPage() -> impl IntoView {
    let params = use_params_map();
    let cluster_id = move || params.with(|p| p.get("cluster_id").cloned().unwrap_or_default());
    let namespace = move || params.with(|p| p.get("namespace").cloned().unwrap_or("default".to_string()));

    let (ingresses, set_ingresses) = create_signal::<Vec<KubernetesIngress>>(vec![]);
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal::<Option<String>>(None);
    let (show_create_modal, set_show_create_modal) = create_signal(false);
    let (creating, set_creating) = create_signal(false);
    let (search_filter, set_search_filter) = create_signal(String::new());
    let (auto_refresh, set_auto_refresh) = create_signal(true);

    // Create ingress form fields
    let (ingress_name, set_ingress_name) = create_signal(String::new());
    let (ingress_class, set_ingress_class) = create_signal(String::new());
    let (ingress_rules, set_ingress_rules) = create_signal::<Vec<IngressRule>>(vec![IngressRule {
        host: Some("example.com".to_string()),
        paths: vec![IngressPath {
            path: "/".to_string(),
            path_type: "Prefix".to_string(),
            service_name: "".to_string(),
            service_port: 80,
        }],
    }]);
    let (tls_config, set_tls_config) = create_signal::<Vec<IngressTLS>>(vec![]);
    let (annotations, set_annotations) = create_signal::<Vec<(String, String)>>(vec![]);

    // Reset form helper
    let reset_create_form = move || {
        set_ingress_name.set(String::new());
        set_ingress_class.set(String::new());
        set_ingress_rules.set(vec![IngressRule {
            host: Some("example.com".to_string()),
            paths: vec![IngressPath {
                path: "/".to_string(),
                path_type: "Prefix".to_string(),
                service_name: "".to_string(),
                service_port: 80,
            }],
        }]);
        set_tls_config.set(vec![]);
        set_annotations.set(vec![]);
    };

    // Load ingresses
    let load_ingresses = {
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
                match api::get_ingresses(&cluster_id, &namespace).await {
                    Ok(data) => {
                        set_ingresses.set(data);
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
            load_ingresses();
            set_interval_with_handle(
                move || {
                    if auto_refresh.get() {
                        load_ingresses();
                    }
                },
                std::time::Duration::from_secs(15),
            ).ok();
        }
    });

    // Initial load
    create_effect(move |_| {
        load_ingresses();
    });

    // Filter ingresses based on search
    let filtered_ingresses = move || {
        let search = search_filter.get().to_lowercase();

        ingresses.get()
            .into_iter()
            .filter(|ingress| {
                search.is_empty()
                    || ingress.name.to_lowercase().contains(&search)
                    || ingress.hosts.iter().any(|host| host.to_lowercase().contains(&search))
            })
            .collect::<Vec<_>>()
    };

    // Create ingress
    let create_ingress = {
        let cluster_id = cluster_id.clone();
        let namespace = namespace.clone();
        move || {
            let cluster_id = cluster_id();
            let namespace = namespace();
            let name = ingress_name.get();
            let class = if ingress_class.get().is_empty() { None } else { Some(ingress_class.get()) };
            let rules = ingress_rules.get();
            let tls = if tls_config.get().is_empty() { None } else { Some(tls_config.get()) };
            let annotations_map: HashMap<String, String> = annotations.get()
                .into_iter()
                .filter(|(k, v)| !k.is_empty() && !v.is_empty())
                .collect();

            if name.is_empty() {
                set_error.set(Some("Ingress name is required".to_string()));
                return;
            }

            if rules.is_empty() {
                set_error.set(Some("At least one rule is required".to_string()));
                return;
            }

            set_creating.set(true);
            spawn_local(async move {
                let request = CreateIngressRequest {
                    name,
                    ingress_class: class,
                    rules,
                    tls,
                    labels: None,
                    annotations: if annotations_map.is_empty() { None } else { Some(annotations_map) },
                };

                match api::create_ingress(&cluster_id, &namespace, request).await {
                    Ok(_) => {
                        set_show_create_modal.set(false);
                        load_ingresses();
                        reset_create_form();
                        set_error.set(None);
                    }
                    Err(e) => {
                        set_error.set(Some(format!("Failed to create ingress: {}", e.message)));
                    }
                }
                set_creating.set(false);
            });
        }
    };

    // Delete ingress
    let delete_ingress = {
        let cluster_id = cluster_id.clone();
        let namespace = namespace.clone();
        move |ingress_name: String| {
            let cluster_id = cluster_id();
            let namespace = namespace();
            spawn_local(async move {
                match api::delete_ingress(&cluster_id, &namespace, &ingress_name).await {
                    Ok(()) => {
                        load_ingresses();
                        set_error.set(None);
                    }
                    Err(e) => {
                        set_error.set(Some(format!("Failed to delete ingress: {}", e.message)));
                    }
                }
            });
        }
    };

    // Reset create form
    let reset_create_form = move || {
        set_ingress_name.set(String::new());
        set_ingress_class.set(String::new());
        set_ingress_rules.set(vec![IngressRule {
            host: Some("example.com".to_string()),
            paths: vec![IngressPath {
                path: "/".to_string(),
                path_type: "Prefix".to_string(),
                service_name: "".to_string(),
                service_port: 80,
            }],
        }]);
        set_tls_config.set(vec![]);
        set_annotations.set(vec![]);
    };

    // Add rule
    let add_rule = move || {
        let mut rules = ingress_rules.get();
        rules.push(IngressRule {
            host: Some("".to_string()),
            paths: vec![IngressPath {
                path: "/".to_string(),
                path_type: "Prefix".to_string(),
                service_name: "".to_string(),
                service_port: 80,
            }],
        });
        set_ingress_rules.set(rules);
    };

    // Remove rule
    let remove_rule = move |index: usize| {
        let mut rules = ingress_rules.get();
        if rules.len() > 1 {
            rules.remove(index);
            set_ingress_rules.set(rules);
        }
    };

    // Add path to rule
    let add_path_to_rule = move |rule_index: usize| {
        let mut rules = ingress_rules.get();
        if let Some(rule) = rules.get_mut(rule_index) {
            rule.paths.push(IngressPath {
                path: "/".to_string(),
                path_type: "Prefix".to_string(),
                service_name: "".to_string(),
                service_port: 80,
            });
            set_ingress_rules.set(rules);
        }
    };

    // Remove path from rule
    let remove_path_from_rule = move |rule_index: usize, path_index: usize| {
        let mut rules = ingress_rules.get();
        if let Some(rule) = rules.get_mut(rule_index) {
            if rule.paths.len() > 1 {
                rule.paths.remove(path_index);
                set_ingress_rules.set(rules);
            }
        }
    };

    // Add TLS config
    let add_tls = move || {
        let mut tls = tls_config.get();
        tls.push(IngressTLS {
            hosts: vec!["example.com".to_string()],
            secret_name: "".to_string(),
        });
        set_tls_config.set(tls);
    };

    // Remove TLS config
    let remove_tls = move |index: usize| {
        let mut tls = tls_config.get();
        tls.remove(index);
        set_tls_config.set(tls);
    };

    // Add annotation
    let add_annotation = move || {
        let mut ann = annotations.get();
        ann.push(("".to_string(), "".to_string()));
        set_annotations.set(ann);
    };

    // Remove annotation
    let remove_annotation = move |index: usize| {
        let mut ann = annotations.get();
        ann.remove(index);
        set_annotations.set(ann);
    };

    // Format hosts display
    let format_hosts = |hosts: &Vec<String>| {
        if hosts.is_empty() {
            "*".to_string()
        } else {
            hosts.join(", ")
        }
    };

    // Format rules display
    let format_rules_summary = |rules: &Vec<IngressRule>| {
        rules
            .iter()
            .map(|rule| {
                let host = rule.host.clone().unwrap_or_else(|| "*".to_string());
                let paths_count = rule.paths.len();
                format!("{} ({} paths)", host, paths_count)
            })
            .collect::<Vec<_>>()
            .join(", ")
    };

    view! {
        <div class="p-6 space-y-6">
            <div class="flex justify-between items-center">
                <div>
                    <h1 class="text-2xl font-bold text-gray-900">Ingresses</h1>
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
                        "Create Ingress"
                    </button>
                    <button
                        on:click=move |_| load_ingresses()
                        disabled=loading
                        class="px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed"
                    >
                        {move || if loading.get() { "Loading..." } else { "Refresh" }}
                    </button>
                </div>
            </div>

            // Search filter
            <div class="bg-white p-4 rounded-lg shadow">
                <div class="max-w-md">
                    <label class="block text-sm font-medium text-gray-700 mb-2">
                        "Search Ingresses"
                    </label>
                    <input
                        type="text"
                        placeholder="Filter by name or host..."
                        class="w-full px-3 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                        prop:value=search_filter
                        on:input=move |ev| set_search_filter.set(event_target_value(&ev))
                    />
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

            // Ingresses table
            <div class="bg-white rounded-lg shadow overflow-hidden">
                <div class="overflow-x-auto">
                    <table class="min-w-full divide-y divide-gray-200">
                        <thead class="bg-gray-50">
                            <tr>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Name"
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Class"
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Hosts"
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Rules"
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "TLS"
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
                            {filtered_ingresses().into_iter().map(|ingress| {
                                let ingress_for_delete = ingress.clone();
                                let ingress_name = ingress.name.clone();
                                let ingress_namespace = ingress.namespace.clone();
                                let ingress_class = ingress.class.clone().unwrap_or_else(|| "<none>".to_string());
                                let hosts_display = format_hosts(&ingress.hosts);
                                let hosts_title = hosts_display.clone();
                                let rules_display = format_rules_summary(&ingress.rules);
                                let rules_title = rules_display.clone();
                                let tls_len = ingress.tls.len();
                                let has_tls = !ingress.tls.is_empty();
                                let ingress_age = ingress.age.clone();

                                view! {
                                    <tr class="hover:bg-gray-50">
                                        <td class="px-6 py-4 whitespace-nowrap">
                                            <div class="text-sm font-medium text-gray-900">{ingress_name}</div>
                                            <div class="text-sm text-gray-500">{ingress_namespace}</div>
                                        </td>
                                        <td class="px-6 py-4 whitespace-nowrap text-sm text-gray-900">
                                            {ingress_class}
                                        </td>
                                        <td class="px-6 py-4 whitespace-nowrap text-sm text-gray-900">
                                            <div class="max-w-xs truncate" title={hosts_title}>
                                                {hosts_display}
                                            </div>
                                        </td>
                                        <td class="px-6 py-4 whitespace-nowrap text-sm text-gray-900">
                                            <div class="max-w-xs truncate" title={rules_title}>
                                                {rules_display}
                                            </div>
                                        </td>
                                        <td class="px-6 py-4 whitespace-nowrap">
                                            {if has_tls {
                                                view! {
                                                    <span class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-green-100 text-green-800">
                                                        {format!("{} TLS", tls_len)}
                                                    </span>
                                                }.into_view()
                                            } else {
                                                view! {
                                                    <span class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-gray-100 text-gray-800">
                                                        "No TLS"
                                                    </span>
                                                }.into_view()
                                            }}
                                        </td>
                                        <td class="px-6 py-4 whitespace-nowrap text-sm text-gray-900">
                                            {ingress_age}
                                        </td>
                                        <td class="px-6 py-4 whitespace-nowrap text-right text-sm font-medium">
                                            <button
                                                on:click=move |_| {
                                                    if window().confirm_with_message(&format!("Are you sure you want to delete ingress '{}'?", ingress_for_delete.name)).unwrap_or(false) {
                                                        delete_ingress(ingress_for_delete.name.clone());
                                                    }
                                                }
                                                class="text-red-600 hover:text-red-900"
                                                title="Delete Ingress"
                                            >
                                                "Delete"
                                            </button>
                                        </td>
                                    </tr>
                                }
                            }).collect::<Vec<_>>()}

                            {move || {
                                if !loading.get() && filtered_ingresses().is_empty() {
                                    view! {
                                        <tr>
                                            <td colspan="7" class="px-6 py-12 text-center text-sm text-gray-500">
                                                "No ingresses found matching the current filters."
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
                            "Loading ingresses..."
                        </div>
                    </div>
                })}
            </div>

            // Create Ingress Modal
            {move || show_create_modal.get().then(|| view! {
                <div class="fixed inset-0 bg-gray-600 bg-opacity-50 overflow-y-auto h-full w-full z-50">
                    <div class="relative top-5 mx-auto p-5 border w-11/12 max-w-4xl shadow-lg rounded-md bg-white">
                        <div class="flex justify-between items-center mb-4">
                            <h3 class="text-lg font-bold text-gray-900">"Create Ingress"</h3>
                            <button
                                on:click=move |_| set_show_create_modal.set(false)
                                class="text-gray-400 hover:text-gray-600"
                            >
                                <svg class="w-6 h-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"></path>
                                </svg>
                            </button>
                        </div>

                        <div class="space-y-6 max-h-96 overflow-y-auto">
                            // Basic ingress info
                            <div class="grid grid-cols-2 gap-4">
                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-2">
                                        "Ingress Name"
                                    </label>
                                    <input
                                        type="text"
                                        required
                                        class="w-full px-3 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                                        prop:value=ingress_name
                                        on:input=move |ev| set_ingress_name.set(event_target_value(&ev))
                                    />
                                </div>
                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-2">
                                        "Ingress Class"
                                    </label>
                                    <input
                                        type="text"
                                        placeholder="nginx, traefik, etc."
                                        class="w-full px-3 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                                        prop:value=ingress_class
                                        on:input=move |ev| set_ingress_class.set(event_target_value(&ev))
                                    />
                                </div>
                            </div>

                            // Rules section
                            <div>
                                <div class="flex justify-between items-center mb-4">
                                    <label class="text-sm font-medium text-gray-700">"Ingress Rules"</label>
                                    <button
                                        on:click=move |_| add_rule()
                                        class="px-3 py-1 bg-blue-600 text-white text-sm rounded hover:bg-blue-700"
                                    >
                                        "Add Rule"
                                    </button>
                                </div>

                                <div class="space-y-4">
                                    {ingress_rules.get().into_iter().enumerate().map(|(rule_index, rule)| {
                                        let rule_host = rule.host.clone().unwrap_or_default();
                                        let rule_paths = rule.paths.clone();
                                        let paths_len = rule.paths.len();
                                        view! {
                                            <div class="p-4 border border-gray-200 rounded-lg">
                                                <div class="flex justify-between items-center mb-3">
                                                    <h4 class="font-medium text-gray-900">{format!("Rule {}", rule_index + 1)}</h4>
                                                    {move || (ingress_rules.get().len() > 1).then(|| view! {
                                                        <button
                                                            on:click=move |_| remove_rule(rule_index)
                                                            class="px-2 py-1 bg-red-600 text-white text-xs rounded hover:bg-red-700"
                                                        >
                                                            "Remove Rule"
                                                        </button>
                                                    })}
                                                </div>

                                                // Host field
                                                <div class="mb-3">
                                                    <label class="block text-sm font-medium text-gray-700 mb-1">
                                                        "Host"
                                                    </label>
                                                    <input
                                                        type="text"
                                                        placeholder="example.com or leave empty for wildcard"
                                                        class="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm"
                                                        prop:value=rule_host
                                                        on:input=move |ev| {
                                                            let value = event_target_value(&ev);
                                                            let mut rules = ingress_rules.get();
                                                            if let Some(r) = rules.get_mut(rule_index) {
                                                                r.host = if value.is_empty() { None } else { Some(value) };
                                                                set_ingress_rules.set(rules);
                                                            }
                                                        }
                                                    />
                                                </div>

                                                // Paths section
                                                <div>
                                                    <div class="flex justify-between items-center mb-2">
                                                        <label class="text-sm font-medium text-gray-700">"Paths"</label>
                                                        <button
                                                            on:click=move |_| add_path_to_rule(rule_index)
                                                            class="px-2 py-1 bg-green-600 text-white text-xs rounded hover:bg-green-700"
                                                        >
                                                            "Add Path"
                                                        </button>
                                                    </div>

                                                    <div class="space-y-2">
                                                        {rule_paths.into_iter().enumerate().map(|(path_index, path)| {
                                                            let path_value = path.path.clone();
                                                            let path_type = path.path_type.clone();
                                                            let is_prefix = path.path_type == "Prefix";
                                                            let is_exact = path.path_type == "Exact";
                                                            let service_name = path.service_name.clone();
                                                            let service_port = path.service_port;
                                                            view! {
                                                                <div class="grid grid-cols-4 gap-2 items-end">
                                                                    <div>
                                                                        <label class="block text-xs text-gray-600 mb-1">"Path"</label>
                                                                        <input
                                                                            type="text"
                                                                            placeholder="/api"
                                                                            class="w-full px-2 py-1 border border-gray-300 rounded text-sm"
                                                                            prop:value=path_value
                                                                            on:input=move |ev| {
                                                                                let value = event_target_value(&ev);
                                                                                let mut rules = ingress_rules.get();
                                                                                if let Some(rule) = rules.get_mut(rule_index) {
                                                                                    if let Some(p) = rule.paths.get_mut(path_index) {
                                                                                        p.path = value;
                                                                                        set_ingress_rules.set(rules);
                                                                                    }
                                                                                }
                                                                            }
                                                                        />
                                                                    </div>
                                                                    <div>
                                                                        <label class="block text-xs text-gray-600 mb-1">"Type"</label>
                                                                        <select
                                                                            class="w-full px-2 py-1 border border-gray-300 rounded text-sm"
                                                                            on:change=move |ev| {
                                                                                let value = event_target_value(&ev);
                                                                                let mut rules = ingress_rules.get();
                                                                                if let Some(rule) = rules.get_mut(rule_index) {
                                                                                    if let Some(p) = rule.paths.get_mut(path_index) {
                                                                                        p.path_type = value;
                                                                                        set_ingress_rules.set(rules);
                                                                                    }
                                                                                }
                                                                            }
                                                                        >
                                                                            <option value="Prefix" selected=is_prefix>"Prefix"</option>
                                                                            <option value="Exact" selected=is_exact>"Exact"</option>
                                                                        </select>
                                                                    </div>
                                                                    <div>
                                                                        <label class="block text-xs text-gray-600 mb-1">"Service"</label>
                                                                        <input
                                                                            type="text"
                                                                            placeholder="service-name"
                                                                            class="w-full px-2 py-1 border border-gray-300 rounded text-sm"
                                                                            prop:value=service_name
                                                                            on:input=move |ev| {
                                                                                let value = event_target_value(&ev);
                                                                                let mut rules = ingress_rules.get();
                                                                                if let Some(rule) = rules.get_mut(rule_index) {
                                                                                    if let Some(p) = rule.paths.get_mut(path_index) {
                                                                                        p.service_name = value;
                                                                                        set_ingress_rules.set(rules);
                                                                                    }
                                                                                }
                                                                            }
                                                                        />
                                                                    </div>
                                                                    <div class="flex items-center space-x-1">
                                                                        <input
                                                                            type="number"
                                                                            placeholder="80"
                                                                            min="1"
                                                                            max="65535"
                                                                            class="flex-1 px-2 py-1 border border-gray-300 rounded text-sm"
                                                                            prop:value=service_port
                                                                            on:input=move |ev| {
                                                                                if let Ok(value) = event_target_value(&ev).parse::<u16>() {
                                                                                    let mut rules = ingress_rules.get();
                                                                                    if let Some(rule) = rules.get_mut(rule_index) {
                                                                                        if let Some(p) = rule.paths.get_mut(path_index) {
                                                                                            p.service_port = value;
                                                                                            set_ingress_rules.set(rules);
                                                                                        }
                                                                                    }
                                                                                }
                                                                            }
                                                                        />
                                                                        {(paths_len > 1).then(|| view! {
                                                                            <button
                                                                                on:click=move |_| remove_path_from_rule(rule_index, path_index)
                                                                                class="px-1 py-1 bg-red-600 text-white text-xs rounded hover:bg-red-700"
                                                                            >
                                                                                "x"
                                                                            </button>
                                                                        })}
                                                                    </div>
                                                                </div>
                                                            }
                                                        }).collect::<Vec<_>>()}
                                                    </div>
                                                </div>
                                            </div>
                                        }
                                    }).collect::<Vec<_>>()}
                                </div>
                            </div>

                            // TLS section
                            <div>
                                <div class="flex justify-between items-center mb-2">
                                    <label class="text-sm font-medium text-gray-700">"TLS Configuration"</label>
                                    <button
                                        on:click=move |_| add_tls()
                                        class="px-2 py-1 bg-blue-600 text-white text-xs rounded hover:bg-blue-700"
                                    >
                                        "Add TLS"
                                    </button>
                                </div>

                                <div class="space-y-2">
                                    {tls_config.get().into_iter().enumerate().map(|(index, tls)| {
                                        let hosts_value = tls.hosts.join(",");
                                        let secret_name = tls.secret_name.clone();
                                        view! {
                                            <div class="flex items-center space-x-2 p-2 border border-gray-200 rounded">
                                                <input
                                                    type="text"
                                                    placeholder="Hosts (comma-separated)"
                                                    class="flex-1 px-2 py-1 border border-gray-300 rounded text-sm"
                                                    prop:value=hosts_value
                                                    on:input=move |ev| {
                                                        let value = event_target_value(&ev);
                                                        let hosts: Vec<String> = value.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                                                        let mut tls_configs = tls_config.get();
                                                        if let Some(t) = tls_configs.get_mut(index) {
                                                            t.hosts = hosts;
                                                            set_tls_config.set(tls_configs);
                                                        }
                                                    }
                                                />
                                                <input
                                                    type="text"
                                                    placeholder="Secret name"
                                                    class="flex-1 px-2 py-1 border border-gray-300 rounded text-sm"
                                                    prop:value=secret_name
                                                    on:input=move |ev| {
                                                        let value = event_target_value(&ev);
                                                        let mut tls_configs = tls_config.get();
                                                        if let Some(t) = tls_configs.get_mut(index) {
                                                            t.secret_name = value;
                                                            set_tls_config.set(tls_configs);
                                                        }
                                                    }
                                                />
                                                <button
                                                    on:click=move |_| remove_tls(index)
                                                    class="px-2 py-1 bg-red-600 text-white text-xs rounded hover:bg-red-700"
                                                >
                                                    "Remove"
                                                </button>
                                            </div>
                                        }
                                    }).collect::<Vec<_>>()}
                                </div>
                            </div>

                            // Annotations section
                            <div>
                                <div class="flex justify-between items-center mb-2">
                                    <label class="text-sm font-medium text-gray-700">"Annotations"</label>
                                    <button
                                        on:click=move |_| add_annotation()
                                        class="px-2 py-1 bg-blue-600 text-white text-xs rounded hover:bg-blue-700"
                                    >
                                        "Add Annotation"
                                    </button>
                                </div>

                                <div class="space-y-2">
                                    {annotations.get().into_iter().enumerate().map(|(index, (key, value))| {
                                        let key_value = key.clone();
                                        let val_value = value.clone();
                                        view! {
                                            <div class="flex items-center space-x-2">
                                                <input
                                                    type="text"
                                                    placeholder="Annotation key"
                                                    class="flex-1 px-2 py-1 border border-gray-300 rounded text-sm"
                                                    prop:value=key_value
                                                    on:input=move |ev| {
                                                        let new_key = event_target_value(&ev);
                                                        let mut anns = annotations.get();
                                                        if let Some((k, _)) = anns.get_mut(index) {
                                                            *k = new_key;
                                                            set_annotations.set(anns);
                                                        }
                                                    }
                                                />
                                                <input
                                                    type="text"
                                                    placeholder="Annotation value"
                                                    class="flex-1 px-2 py-1 border border-gray-300 rounded text-sm"
                                                    prop:value=val_value
                                                    on:input=move |ev| {
                                                        let new_value = event_target_value(&ev);
                                                        let mut anns = annotations.get();
                                                        if let Some((_, v)) = anns.get_mut(index) {
                                                            *v = new_value;
                                                            set_annotations.set(anns);
                                                        }
                                                    }
                                                />
                                                <button
                                                    on:click=move |_| remove_annotation(index)
                                                    class="px-2 py-1 bg-red-600 text-white text-xs rounded hover:bg-red-700"
                                                >
                                                    "Remove"
                                                </button>
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
                                on:click=move |_| create_ingress()
                                disabled=creating
                                class="px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 disabled:opacity-50"
                            >
                                {move || if creating.get() { "Creating..." } else { "Create Ingress" }}
                            </button>
                        </div>
                    </div>
                </div>
            })}

            // Statistics footer
            <div class="bg-white rounded-lg shadow p-4">
                <div class="grid grid-cols-2 md:grid-cols-4 gap-4 text-center">
                    <div>
                        <div class="text-2xl font-bold text-gray-900">{move || filtered_ingresses().len()}</div>
                        <div class="text-sm text-gray-500">"Total Ingresses"</div>
                    </div>
                    <div>
                        <div class="text-2xl font-bold text-green-600">
                            {move || filtered_ingresses().iter().filter(|i| !i.tls.is_empty()).count()}
                        </div>
                        <div class="text-sm text-gray-500">"With TLS"</div>
                    </div>
                    <div>
                        <div class="text-2xl font-bold text-blue-600">
                            {move || filtered_ingresses().iter().map(|i| i.rules.len()).sum::<usize>()}
                        </div>
                        <div class="text-sm text-gray-500">"Total Rules"</div>
                    </div>
                    <div>
                        <div class="text-2xl font-bold text-purple-600">
                            {move || filtered_ingresses().iter().map(|i| i.hosts.len()).sum::<usize>()}
                        </div>
                        <div class="text-sm text-gray-500">"Total Hosts"</div>
                    </div>
                </div>
            </div>
        </div>
    }
}