//! Pod Management Page
//!
//! Provides comprehensive pod management interface including:
//! - Pod listing with real-time status
//! - Pod logs streaming and search
//! - Pod deletion and restart operations
//! - Container details and monitoring

use leptos::*;
use leptos_router::*;
use crate::api::{self, KubernetesPod, PodStatus};

#[component]
pub fn PodsPage() -> impl IntoView {
    let params = use_params_map();
    let cluster_id = move || params.with(|p| p.get("cluster_id").cloned().unwrap_or_default());
    let namespace = move || params.with(|p| p.get("namespace").cloned().unwrap_or("default".to_string()));

    let (pods, set_pods) = create_signal::<Vec<KubernetesPod>>(vec![]);
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal::<Option<String>>(None);
    let (selected_pod, set_selected_pod) = create_signal::<Option<KubernetesPod>>(None);
    let (show_logs, set_show_logs) = create_signal(false);
    let (pod_logs, set_pod_logs) = create_signal(String::new());
    let (logs_loading, set_logs_loading) = create_signal(false);
    let (search_filter, set_search_filter) = create_signal(String::new());
    let (status_filter, set_status_filter) = create_signal(String::new());
    let (auto_refresh, set_auto_refresh) = create_signal(true);

    // Load pods
    let load_pods = {
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
                match api::get_pods(&cluster_id, &namespace).await {
                    Ok(data) => {
                        set_pods.set(data);
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
            load_pods();
            set_interval_with_handle(
                move || {
                    if auto_refresh.get() {
                        load_pods();
                    }
                },
                std::time::Duration::from_secs(10),
            ).ok();
        }
    });

    // Initial load
    create_effect(move |_| {
        load_pods();
    });

    // Filter pods based on search and status
    let filtered_pods = move || {
        let search = search_filter.get().to_lowercase();
        let status = status_filter.get();

        pods.get()
            .into_iter()
            .filter(|pod| {
                let name_match = search.is_empty() || pod.name.to_lowercase().contains(&search);
                let status_match = status.is_empty() || pod.status.phase.eq_ignore_ascii_case(&status);
                name_match && status_match
            })
            .collect::<Vec<_>>()
    };

    // Delete pod action
    let delete_pod = {
        let cluster_id = cluster_id.clone();
        let namespace = namespace.clone();
        move |pod_name: String| {
            let cluster_id = cluster_id();
            let namespace = namespace();
            spawn_local(async move {
                if let Err(e) = api::delete_pod(&cluster_id, &namespace, &pod_name).await {
                    set_error.set(Some(format!("Failed to delete pod: {}", e.message)));
                } else {
                    load_pods();
                }
            });
        }
    };

    // View pod logs
    let view_logs = {
        let cluster_id = cluster_id.clone();
        let namespace = namespace.clone();
        move |pod: KubernetesPod, container: Option<String>| {
            let cluster_id = cluster_id();
            let namespace = namespace();
            set_selected_pod.set(Some(pod.clone()));
            set_show_logs.set(true);
            set_logs_loading.set(true);

            spawn_local(async move {
                let container_name = container.as_deref();
                match api::get_pod_logs(&cluster_id, &namespace, &pod.name, container_name).await {
                    Ok(logs) => {
                        set_pod_logs.set(logs);
                        set_error.set(None);
                    }
                    Err(e) => {
                        set_error.set(Some(format!("Failed to fetch logs: {}", e.message)));
                    }
                }
                set_logs_loading.set(false);
            });
        }
    };

    // Get status badge class
    let status_class = |status: &PodStatus| {
        match status.phase.as_str() {
            "Running" => "bg-green-100 text-green-800",
            "Pending" => "bg-yellow-100 text-yellow-800",
            "Succeeded" => "bg-blue-100 text-blue-800",
            "Failed" => "bg-red-100 text-red-800",
            "Unknown" => "bg-gray-100 text-gray-800",
            _ => "bg-gray-100 text-gray-800",
        }
    };

    view! {
        <div class="p-6 space-y-6">
            <div class="flex justify-between items-center">
                <div>
                    <h1 class="text-2xl font-bold text-gray-900">Pods</h1>
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
                        on:click=move |_| load_pods()
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
                            "Search Pods"
                        </label>
                        <input
                            type="text"
                            placeholder="Filter by pod name..."
                            class="w-full px-3 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                            prop:value=search_filter
                            on:input=move |ev| set_search_filter.set(event_target_value(&ev))
                        />
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-2">
                            "Status Filter"
                        </label>
                        <select
                            class="w-full px-3 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                            on:change=move |ev| set_status_filter.set(event_target_value(&ev))
                        >
                            <option value="">"All Statuses"</option>
                            <option value="Running">"Running"</option>
                            <option value="Pending">"Pending"</option>
                            <option value="Succeeded">"Succeeded"</option>
                            <option value="Failed">"Failed"</option>
                            <option value="Unknown">"Unknown"</option>
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

            // Pods table
            <div class="bg-white rounded-lg shadow overflow-hidden">
                <div class="overflow-x-auto">
                    <table class="min-w-full divide-y divide-gray-200">
                        <thead class="bg-gray-50">
                            <tr>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Name"
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Status"
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Ready"
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Restarts"
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Age"
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Node"
                                </th>
                                <th class="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Actions"
                                </th>
                            </tr>
                        </thead>
                        <tbody class="bg-white divide-y divide-gray-200">
                            {filtered_pods().into_iter().map(|pod| {
                                let pod_for_delete = pod.clone();
                                let pod_for_logs = pod.clone();
                                let pod_name = pod.name.clone();
                                let namespace = pod.namespace.clone();
                                let status_cls = status_class(&pod.status);
                                let phase = pod.status.phase.clone();
                                let reason = pod.status.reason.clone();
                                let ready = pod.ready.clone();
                                let restarts = pod.restarts;
                                let age = pod.age.clone();
                                let node = pod.node.clone().unwrap_or_else(|| "<none>".to_string());

                                view! {
                                    <tr class="hover:bg-gray-50">
                                        <td class="px-6 py-4 whitespace-nowrap">
                                            <div class="text-sm font-medium text-gray-900">{pod_name}</div>
                                            <div class="text-sm text-gray-500">{namespace}</div>
                                        </td>
                                        <td class="px-6 py-4 whitespace-nowrap">
                                            <span class={format!("inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium {}", status_cls)}>
                                                {phase}
                                            </span>
                                            {reason.map(|r| view! {
                                                <div class="text-xs text-gray-500 mt-1">{r}</div>
                                            })}
                                        </td>
                                        <td class="px-6 py-4 whitespace-nowrap text-sm text-gray-900">
                                            {ready}
                                        </td>
                                        <td class="px-6 py-4 whitespace-nowrap text-sm text-gray-900">
                                            {restarts}
                                        </td>
                                        <td class="px-6 py-4 whitespace-nowrap text-sm text-gray-900">
                                            {age}
                                        </td>
                                        <td class="px-6 py-4 whitespace-nowrap text-sm text-gray-900">
                                            {node}
                                        </td>
                                        <td class="px-6 py-4 whitespace-nowrap text-right text-sm font-medium">
                                            <div class="flex justify-end space-x-2">
                                                <button
                                                    on:click=move |_| view_logs(pod_for_logs.clone(), None)
                                                    class="text-indigo-600 hover:text-indigo-900"
                                                    title="View Logs"
                                                >
                                                    "Logs"
                                                </button>
                                                <button
                                                    on:click=move |_| delete_pod(pod_for_delete.name.clone())
                                                    class="text-red-600 hover:text-red-900"
                                                    title="Delete Pod"
                                                >
                                                    "Delete"
                                                </button>
                                            </div>
                                        </td>
                                    </tr>
                                }
                            }).collect::<Vec<_>>()}

                            {move || {
                                if !loading.get() && filtered_pods().is_empty() {
                                    view! {
                                        <tr>
                                            <td colspan="7" class="px-6 py-12 text-center text-sm text-gray-500">
                                                "No pods found matching the current filters."
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
                            "Loading pods..."
                        </div>
                    </div>
                })}
            </div>

            // Logs Modal
            {move || show_logs.get().then(|| {
                let pod = selected_pod.get();
                view! {
                    <div class="fixed inset-0 bg-gray-600 bg-opacity-50 overflow-y-auto h-full w-full z-50">
                        <div class="relative top-20 mx-auto p-5 border w-11/12 max-w-4xl shadow-lg rounded-md bg-white">
                            <div class="flex justify-between items-center mb-4">
                                <h3 class="text-lg font-bold text-gray-900">
                                    "Pod Logs: " {pod.as_ref().map(|p| p.name.clone()).unwrap_or_default()}
                                </h3>
                                <button
                                    on:click=move |_| set_show_logs.set(false)
                                    class="text-gray-400 hover:text-gray-600"
                                >
                                    <svg class="w-6 h-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"></path>
                                    </svg>
                                </button>
                            </div>

                            <div class="border rounded-lg p-4 bg-gray-900 text-gray-100 font-mono text-sm h-96 overflow-y-auto">
                                {move || {
                                    if logs_loading.get() {
                                        view! {
                                            <div class="text-center py-8">
                                                "Loading logs..."
                                            </div>
                                        }.into_view()
                                    } else if pod_logs.get().is_empty() {
                                        view! {
                                            <div class="text-center py-8 text-gray-400">
                                                "No logs available"
                                            </div>
                                        }.into_view()
                                    } else {
                                        view! {
                                            <pre class="whitespace-pre-wrap">{pod_logs.get()}</pre>
                                        }.into_view()
                                    }
                                }}
                            </div>

                            <div class="mt-4 flex justify-end space-x-2">
                                <button
                                    on:click=move |_| {
                                        if let Some(pod) = selected_pod.get() {
                                            view_logs(pod, None);
                                        }
                                    }
                                    class="px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700"
                                >
                                    "Refresh Logs"
                                </button>
                                <button
                                    on:click=move |_| set_show_logs.set(false)
                                    class="px-4 py-2 bg-gray-300 text-gray-700 rounded-lg hover:bg-gray-400"
                                >
                                    "Close"
                                </button>
                            </div>
                        </div>
                    </div>
                }
            })}

            // Statistics footer
            <div class="bg-white rounded-lg shadow p-4">
                <div class="grid grid-cols-2 md:grid-cols-4 gap-4 text-center">
                    <div>
                        <div class="text-2xl font-bold text-gray-900">{move || filtered_pods().len()}</div>
                        <div class="text-sm text-gray-500">"Total Pods"</div>
                    </div>
                    <div>
                        <div class="text-2xl font-bold text-green-600">
                            {move || filtered_pods().iter().filter(|p| p.status.phase == "Running").count()}
                        </div>
                        <div class="text-sm text-gray-500">"Running"</div>
                    </div>
                    <div>
                        <div class="text-2xl font-bold text-yellow-600">
                            {move || filtered_pods().iter().filter(|p| p.status.phase == "Pending").count()}
                        </div>
                        <div class="text-sm text-gray-500">"Pending"</div>
                    </div>
                    <div>
                        <div class="text-2xl font-bold text-red-600">
                            {move || filtered_pods().iter().filter(|p| p.status.phase == "Failed").count()}
                        </div>
                        <div class="text-sm text-gray-500">"Failed"</div>
                    </div>
                </div>
            </div>
        </div>
    }
}