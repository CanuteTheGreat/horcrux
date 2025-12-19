use leptos::*;
use std::collections::HashMap;
use crate::api::{KubernetesConfigMap, CreateConfigMapRequest, get_kubernetes_configmaps, create_kubernetes_configmap, update_kubernetes_configmap, delete_kubernetes_configmap};

#[component]
pub fn ConfigMapsPage() -> impl IntoView {
    let (configmaps, set_configmaps) = create_signal(Vec::<KubernetesConfigMap>::new());
    let (loading, set_loading) = create_signal(false);
    let (error, set_error) = create_signal(None::<String>);
    let (show_create_modal, set_show_create_modal) = create_signal(false);
    let (show_edit_modal, set_show_edit_modal) = create_signal(false);
    let (selected_configmap, set_selected_configmap) = create_signal(None::<KubernetesConfigMap>);
    let (search_query, set_search_query) = create_signal(String::new());
    let (selected_namespace, set_selected_namespace) = create_signal("default".to_string());
    let (cluster_id, set_cluster_id) = create_signal("main".to_string());

    // Form state
    let (name, set_name) = create_signal(String::new());
    let (namespace, set_namespace) = create_signal("default".to_string());
    let (data_key, set_data_key) = create_signal(String::new());
    let (data_value, set_data_value) = create_signal(String::new());
    let (config_data, set_config_data) = create_signal(HashMap::<String, String>::new());
    let (labels, set_labels) = create_signal(HashMap::<String, String>::new());
    let (annotations, set_annotations) = create_signal(HashMap::<String, String>::new());

    let load_configmaps = move || {
        set_loading.set(true);
        set_error.set(None);

        spawn_local(async move {
            match get_kubernetes_configmaps(&cluster_id.get(), Some(&selected_namespace.get())).await {
                Ok(cms) => {
                    set_configmaps.set(cms);
                    set_error.set(None);
                }
                Err(e) => set_error.set(Some(format!("Failed to load ConfigMaps: {}", e))),
            }
            set_loading.set(false);
        });
    };

    // Load configmaps on mount and when dependencies change
    create_effect(move |_| {
        if !cluster_id.get().is_empty() && !selected_namespace.get().is_empty() {
            load_configmaps();
        }
    });

    // Auto-refresh every 30 seconds
    use leptos::set_interval;
    set_interval(
        move || load_configmaps(),
        std::time::Duration::from_secs(30),
    );

    let filtered_configmaps = move || {
        let query = search_query.get().to_lowercase();
        if query.is_empty() {
            configmaps.get()
        } else {
            configmaps
                .get()
                .into_iter()
                .filter(|cm| {
                    cm.name.to_lowercase().contains(&query) ||
                    cm.namespace.to_lowercase().contains(&query)
                })
                .collect()
        }
    };

    let add_data_pair = move || {
        let key = data_key.get().trim().to_string();
        let value = data_value.get();

        if !key.is_empty() {
            let mut current_data = config_data.get();
            current_data.insert(key, value);
            set_config_data.set(current_data);
            set_data_key.set(String::new());
            set_data_value.set(String::new());
        }
    };

    let remove_data_pair = move |key: String| {
        let mut current_data = config_data.get();
        current_data.remove(&key);
        set_config_data.set(current_data);
    };

    let reset_form = move || {
        set_name.set(String::new());
        set_namespace.set("default".to_string());
        set_config_data.set(HashMap::new());
        set_labels.set(HashMap::new());
        set_annotations.set(HashMap::new());
        set_data_key.set(String::new());
        set_data_value.set(String::new());
    };

    let create_configmap = move || {
        let request = CreateConfigMapRequest {
            name: name.get(),
            data: config_data.get(),
            binary_data: None,
            labels: if labels.get().is_empty() { None } else { Some(labels.get()) },
            annotations: if annotations.get().is_empty() { None } else { Some(annotations.get()) },
        };

        let ns = namespace.get();
        spawn_local(async move {
            match create_kubernetes_configmap(&cluster_id.get(), &ns, request).await {
                Ok(_) => {
                    set_show_create_modal.set(false);
                    reset_form();
                    load_configmaps();
                }
                Err(e) => set_error.set(Some(format!("Failed to create ConfigMap: {}", e))),
            }
        });
    };

    let edit_configmap = move |configmap: KubernetesConfigMap| {
        set_selected_configmap.set(Some(configmap.clone()));
        set_name.set(configmap.name);
        set_namespace.set(configmap.namespace);
        set_config_data.set(configmap.data);
        set_labels.set(configmap.labels);
        set_annotations.set(configmap.annotations);
        set_show_edit_modal.set(true);
    };

    let update_configmap = move || {
        if let Some(cm) = selected_configmap.get() {
            let request = CreateConfigMapRequest {
                name: name.get(),
                data: config_data.get(),
                binary_data: None,
                labels: if labels.get().is_empty() { None } else { Some(labels.get()) },
                annotations: if annotations.get().is_empty() { None } else { Some(annotations.get()) },
            };

            spawn_local(async move {
                match update_kubernetes_configmap(&cluster_id.get(), &cm.namespace, &cm.name, request).await {
                    Ok(_) => {
                        set_show_edit_modal.set(false);
                        reset_form();
                        load_configmaps();
                    }
                    Err(e) => set_error.set(Some(format!("Failed to update ConfigMap: {}", e))),
                }
            });
        }
    };

    let delete_configmap = move |configmap: KubernetesConfigMap| {
        if web_sys::window()
            .unwrap()
            .confirm_with_message(&format!("Are you sure you want to delete ConfigMap '{}'?", configmap.name))
            .unwrap()
        {
            spawn_local(async move {
                match delete_kubernetes_configmap(&cluster_id.get(), &configmap.namespace, &configmap.name).await {
                    Ok(_) => load_configmaps(),
                    Err(e) => set_error.set(Some(format!("Failed to delete ConfigMap: {}", e))),
                }
            });
        }
    };

    view! {
        <div class="p-6">
            <div class="flex justify-between items-center mb-6">
                <h1 class="text-2xl font-bold">ConfigMaps</h1>
                <button
                    on:click=move |_| {
                        reset_form();
                        set_show_create_modal.set(true);
                    }
                    class="bg-blue-500 hover:bg-blue-600 text-white px-4 py-2 rounded-lg flex items-center gap-2"
                >
                    <i class="fas fa-plus"></i>
                    "Create ConfigMap"
                </button>
            </div>

            // Controls
            <div class="bg-white rounded-lg shadow p-4 mb-6">
                <div class="grid grid-cols-1 md:grid-cols-3 gap-4">
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-1">
                            "Cluster"
                        </label>
                        <select
                            on:change=move |ev| set_cluster_id.set(event_target_value(&ev))
                            class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                        >
                            <option value="main">"Main Cluster"</option>
                            <option value="staging">"Staging Cluster"</option>
                        </select>
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-1">
                            "Namespace"
                        </label>
                        <select
                            on:change=move |ev| set_selected_namespace.set(event_target_value(&ev))
                            class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                        >
                            <option value="default">"default"</option>
                            <option value="kube-system">"kube-system"</option>
                            <option value="monitoring">"monitoring"</option>
                            <option value="logging">"logging"</option>
                        </select>
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-1">
                            "Search ConfigMaps"
                        </label>
                        <input
                            type="text"
                            placeholder="Search by name or namespace..."
                            on:input=move |ev| set_search_query.set(event_target_value(&ev))
                            class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                        />
                    </div>
                </div>
            </div>

            // Error display
            {move || error.get().map(|e| view! {
                <div class="bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded mb-4">
                    {e}
                </div>
            })}

            // Loading state
            {move || if loading.get() {
                view! {
                    <div class="bg-white rounded-lg shadow p-8 text-center">
                        <i class="fas fa-spinner fa-spin text-2xl text-gray-400 mb-2"></i>
                        <p class="text-gray-600">"Loading ConfigMaps..."</p>
                    </div>
                }
            } else {
                view! {
                    <div class="bg-white rounded-lg shadow overflow-hidden">
                        <div class="overflow-x-auto">
                            <table class="min-w-full divide-y divide-gray-200">
                                <thead class="bg-gray-50">
                                    <tr>
                                        <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                            "Name"
                                        </th>
                                        <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                            "Namespace"
                                        </th>
                                        <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                            "Data Keys"
                                        </th>
                                        <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                            "Age"
                                        </th>
                                        <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                            "Actions"
                                        </th>
                                    </tr>
                                </thead>
                                <tbody class="bg-white divide-y divide-gray-200">
                                    {filtered_configmaps().into_iter().map(|configmap| {
                                        let configmap_edit = configmap.clone();
                                        let configmap_delete = configmap.clone();
                                        let name = configmap.name.clone();
                                        let namespace = configmap.namespace.clone();
                                        let data_keys: Vec<String> = configmap.data.keys().cloned().collect();
                                        let keys_len = data_keys.len();
                                        let keys_display = if data_keys.len() > 3 {
                                            format!("{}, ... (+{})", data_keys[..3].join(", "), data_keys.len() - 3)
                                        } else {
                                            data_keys.join(", ")
                                        };
                                        let created_at = configmap.created_at.map(|dt| dt.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_else(|| "Unknown".to_string());

                                        view! {
                                            <tr>
                                                <td class="px-6 py-4 whitespace-nowrap">
                                                    <div class="text-sm font-medium text-gray-900">{name}</div>
                                                </td>
                                                <td class="px-6 py-4 whitespace-nowrap">
                                                    <span class="px-2 py-1 text-xs font-medium bg-blue-100 text-blue-800 rounded">
                                                        {namespace}
                                                    </span>
                                                </td>
                                                <td class="px-6 py-4 whitespace-nowrap">
                                                    <div class="text-sm text-gray-900">{keys_display}</div>
                                                    <div class="text-xs text-gray-500">{format!("{} keys total", keys_len)}</div>
                                                </td>
                                                <td class="px-6 py-4 whitespace-nowrap">
                                                    <div class="text-sm text-gray-900">
                                                        {created_at}
                                                    </div>
                                                </td>
                                                <td class="px-6 py-4 whitespace-nowrap text-sm font-medium">
                                                    <div class="flex space-x-2">
                                                        <button
                                                            on:click=move |_| edit_configmap(configmap_edit.clone())
                                                            class="text-blue-600 hover:text-blue-900 px-2 py-1 rounded hover:bg-blue-50"
                                                            title="Edit ConfigMap"
                                                        >
                                                            <i class="fas fa-edit"></i>
                                                        </button>
                                                        <button
                                                            on:click=move |_| delete_configmap(configmap_delete.clone())
                                                            class="text-red-600 hover:text-red-900 px-2 py-1 rounded hover:bg-red-50"
                                                            title="Delete ConfigMap"
                                                        >
                                                            <i class="fas fa-trash"></i>
                                                        </button>
                                                    </div>
                                                </td>
                                            </tr>
                                        }
                                    }).collect::<Vec<_>>()}
                                </tbody>
                            </table>
                        </div>
                    </div>
                }
            }}

            // Create Modal
            {move || if show_create_modal.get() {
                view! {
                    <div class="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50 p-4">
                        <div class="bg-white rounded-lg w-full max-w-4xl max-h-[90vh] overflow-y-auto">
                            <div class="p-6 border-b border-gray-200">
                                <h2 class="text-xl font-semibold">"Create ConfigMap"</h2>
                            </div>

                            <div class="p-6 space-y-6">
                                <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                                    <div>
                                        <label class="block text-sm font-medium text-gray-700 mb-1">
                                            "Name"
                                        </label>
                                        <input
                                            type="text"
                                            placeholder="my-config"
                                            prop:value=move || name.get()
                                            on:input=move |ev| set_name.set(event_target_value(&ev))
                                            class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                        />
                                    </div>
                                    <div>
                                        <label class="block text-sm font-medium text-gray-700 mb-1">
                                            "Namespace"
                                        </label>
                                        <input
                                            type="text"
                                            placeholder="default"
                                            prop:value=move || namespace.get()
                                            on:input=move |ev| set_namespace.set(event_target_value(&ev))
                                            class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                        />
                                    </div>
                                </div>

                                // Data Section
                                <div>
                                    <h3 class="text-lg font-medium mb-4">"Configuration Data"</h3>

                                    <div class="bg-gray-50 p-4 rounded-lg mb-4">
                                        <div class="grid grid-cols-1 md:grid-cols-3 gap-4 items-end">
                                            <div>
                                                <label class="block text-sm font-medium text-gray-700 mb-1">
                                                    "Key"
                                                </label>
                                                <input
                                                    type="text"
                                                    placeholder="config.properties"
                                                    prop:value=move || data_key.get()
                                                    on:input=move |ev| set_data_key.set(event_target_value(&ev))
                                                    class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                                />
                                            </div>
                                            <div>
                                                <label class="block text-sm font-medium text-gray-700 mb-1">
                                                    "Value"
                                                </label>
                                                <textarea
                                                    placeholder="key=value\nanother_key=another_value"
                                                    prop:value=move || data_value.get()
                                                    on:input=move |ev| set_data_value.set(event_target_value(&ev))
                                                    class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                                    rows="3"
                                                />
                                            </div>
                                            <div>
                                                <button
                                                    on:click=move |_| add_data_pair()
                                                    class="w-full bg-green-500 hover:bg-green-600 text-white px-4 py-2 rounded-lg"
                                                >
                                                    "Add Data"
                                                </button>
                                            </div>
                                        </div>
                                    </div>

                                    // Current Data
                                    <div class="space-y-2">
                                        {move || config_data.get().into_iter().map(|(key, value)| {
                                            let key_clone = key.clone();
                                            let key_display = key.clone();
                                            view! {
                                                <div class="bg-white border border-gray-200 rounded-lg p-3">
                                                    <div class="flex justify-between items-start">
                                                        <div class="flex-1 min-w-0">
                                                            <div class="text-sm font-medium text-gray-900 mb-1">{key_display}</div>
                                                            <div class="text-xs text-gray-600 font-mono bg-gray-50 p-2 rounded">
                                                                {if value.len() > 100 {
                                                                    format!("{}...", &value[..100])
                                                                } else {
                                                                    value.clone()
                                                                }}
                                                            </div>
                                                        </div>
                                                        <button
                                                            on:click=move |_| remove_data_pair(key_clone.clone())
                                                            class="ml-2 text-red-600 hover:text-red-900 p-1"
                                                        >
                                                            <i class="fas fa-times"></i>
                                                        </button>
                                                    </div>
                                                </div>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </div>
                                </div>
                            </div>

                            <div class="p-6 border-t border-gray-200 flex justify-end space-x-3">
                                <button
                                    on:click=move |_| set_show_create_modal.set(false)
                                    class="px-4 py-2 text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50"
                                >
                                    "Cancel"
                                </button>
                                <button
                                    on:click=move |_| create_configmap()
                                    class="px-4 py-2 bg-blue-500 text-white rounded-lg hover:bg-blue-600"
                                    disabled=move || name.get().is_empty() || config_data.get().is_empty()
                                >
                                    "Create ConfigMap"
                                </button>
                            </div>
                        </div>
                    </div>
                }
            } else {
                view! { <div></div> }
            }}

            // Edit Modal (similar structure to create modal)
            {move || if show_edit_modal.get() {
                view! {
                    <div class="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50 p-4">
                        <div class="bg-white rounded-lg w-full max-w-4xl max-h-[90vh] overflow-y-auto">
                            <div class="p-6 border-b border-gray-200">
                                <h2 class="text-xl font-semibold">"Edit ConfigMap"</h2>
                            </div>

                            <div class="p-6 space-y-6">
                                <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                                    <div>
                                        <label class="block text-sm font-medium text-gray-700 mb-1">
                                            "Name"
                                        </label>
                                        <input
                                            type="text"
                                            prop:value=move || name.get()
                                            on:input=move |ev| set_name.set(event_target_value(&ev))
                                            class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                            disabled=true
                                        />
                                    </div>
                                    <div>
                                        <label class="block text-sm font-medium text-gray-700 mb-1">
                                            "Namespace"
                                        </label>
                                        <input
                                            type="text"
                                            prop:value=move || namespace.get()
                                            on:input=move |ev| set_namespace.set(event_target_value(&ev))
                                            class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                            disabled=true
                                        />
                                    </div>
                                </div>

                                // Data Section (same as create modal)
                                <div>
                                    <h3 class="text-lg font-medium mb-4">"Configuration Data"</h3>

                                    <div class="bg-gray-50 p-4 rounded-lg mb-4">
                                        <div class="grid grid-cols-1 md:grid-cols-3 gap-4 items-end">
                                            <div>
                                                <label class="block text-sm font-medium text-gray-700 mb-1">
                                                    "Key"
                                                </label>
                                                <input
                                                    type="text"
                                                    placeholder="config.properties"
                                                    prop:value=move || data_key.get()
                                                    on:input=move |ev| set_data_key.set(event_target_value(&ev))
                                                    class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                                />
                                            </div>
                                            <div>
                                                <label class="block text-sm font-medium text-gray-700 mb-1">
                                                    "Value"
                                                </label>
                                                <textarea
                                                    placeholder="key=value\nanother_key=another_value"
                                                    prop:value=move || data_value.get()
                                                    on:input=move |ev| set_data_value.set(event_target_value(&ev))
                                                    class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                                    rows="3"
                                                />
                                            </div>
                                            <div>
                                                <button
                                                    on:click=move |_| add_data_pair()
                                                    class="w-full bg-green-500 hover:bg-green-600 text-white px-4 py-2 rounded-lg"
                                                >
                                                    "Add Data"
                                                </button>
                                            </div>
                                        </div>
                                    </div>

                                    // Current Data (same as create modal)
                                    <div class="space-y-2">
                                        {move || config_data.get().into_iter().map(|(key, value)| {
                                            let key_clone = key.clone();
                                            let key_display = key.clone();
                                            view! {
                                                <div class="bg-white border border-gray-200 rounded-lg p-3">
                                                    <div class="flex justify-between items-start">
                                                        <div class="flex-1 min-w-0">
                                                            <div class="text-sm font-medium text-gray-900 mb-1">{key_display}</div>
                                                            <div class="text-xs text-gray-600 font-mono bg-gray-50 p-2 rounded">
                                                                {if value.len() > 100 {
                                                                    format!("{}...", &value[..100])
                                                                } else {
                                                                    value.clone()
                                                                }}
                                                            </div>
                                                        </div>
                                                        <button
                                                            on:click=move |_| remove_data_pair(key_clone.clone())
                                                            class="ml-2 text-red-600 hover:text-red-900 p-1"
                                                        >
                                                            <i class="fas fa-times"></i>
                                                        </button>
                                                    </div>
                                                </div>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </div>
                                </div>
                            </div>

                            <div class="p-6 border-t border-gray-200 flex justify-end space-x-3">
                                <button
                                    on:click=move |_| set_show_edit_modal.set(false)
                                    class="px-4 py-2 text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50"
                                >
                                    "Cancel"
                                </button>
                                <button
                                    on:click=move |_| update_configmap()
                                    class="px-4 py-2 bg-blue-500 text-white rounded-lg hover:bg-blue-600"
                                >
                                    "Update ConfigMap"
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