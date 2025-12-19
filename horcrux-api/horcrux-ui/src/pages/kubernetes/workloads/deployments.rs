//! Deployment Management Page
//!
//! Provides comprehensive deployment management interface including:
//! - Deployment listing with replica status
//! - Deployment scaling (manual)
//! - Restart and delete operations
//! - Deployment history and rollback

use leptos::*;
use leptos_router::*;
use crate::api::{self, KubernetesDeployment, DeploymentReplicas, DeploymentCondition};

#[component]
pub fn DeploymentsPage() -> impl IntoView {
    let params = use_params_map();
    let cluster_id = move || params.with(|p| p.get("cluster_id").cloned().unwrap_or_default());
    let namespace = move || params.with(|p| p.get("namespace").cloned().unwrap_or("default".to_string()));

    let (deployments, set_deployments) = create_signal::<Vec<KubernetesDeployment>>(vec![]);
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal::<Option<String>>(None);
    let (selected_deployment, set_selected_deployment) = create_signal::<Option<KubernetesDeployment>>(None);
    let (show_scale_modal, set_show_scale_modal) = create_signal(false);
    let (scale_replicas, set_scale_replicas) = create_signal(1u32);
    let (scaling, set_scaling) = create_signal(false);
    let (search_filter, set_search_filter) = create_signal(String::new());
    let (auto_refresh, set_auto_refresh) = create_signal(true);

    // Load deployments
    let load_deployments = {
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
                match api::get_deployments(&cluster_id, &namespace).await {
                    Ok(data) => {
                        set_deployments.set(data);
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
            load_deployments();
            set_interval_with_handle(
                move || {
                    if auto_refresh.get() {
                        load_deployments();
                    }
                },
                std::time::Duration::from_secs(15),
            ).ok();
        }
    });

    // Initial load
    create_effect(move |_| {
        load_deployments();
    });

    // Filter deployments based on search
    let filtered_deployments = move || {
        let search = search_filter.get().to_lowercase();

        deployments.get()
            .into_iter()
            .filter(|deployment| {
                search.is_empty() || deployment.name.to_lowercase().contains(&search)
            })
            .collect::<Vec<_>>()
    };

    // Scale deployment
    let scale_deployment = {
        let cluster_id = cluster_id.clone();
        let namespace = namespace.clone();
        move |deployment_name: String, replicas: u32| {
            let cluster_id = cluster_id();
            let namespace = namespace();
            set_scaling.set(true);

            spawn_local(async move {
                match api::scale_deployment(&cluster_id, &namespace, &deployment_name, replicas).await {
                    Ok(()) => {
                        set_show_scale_modal.set(false);
                        load_deployments();
                        set_error.set(None);
                    }
                    Err(e) => {
                        set_error.set(Some(format!("Failed to scale deployment: {}", e.message)));
                    }
                }
                set_scaling.set(false);
            });
        }
    };

    // Restart deployment
    let restart_deployment = {
        let cluster_id = cluster_id.clone();
        let namespace = namespace.clone();
        move |deployment_name: String| {
            let cluster_id = cluster_id();
            let namespace = namespace();
            spawn_local(async move {
                match api::restart_deployment(&cluster_id, &namespace, &deployment_name).await {
                    Ok(()) => {
                        load_deployments();
                        set_error.set(None);
                    }
                    Err(e) => {
                        set_error.set(Some(format!("Failed to restart deployment: {}", e.message)));
                    }
                }
            });
        }
    };

    // Delete deployment
    let delete_deployment = {
        let cluster_id = cluster_id.clone();
        let namespace = namespace.clone();
        move |deployment_name: String| {
            let cluster_id = cluster_id();
            let namespace = namespace();
            spawn_local(async move {
                if let Err(e) = api::delete_deployment(&cluster_id, &namespace, &deployment_name).await {
                    set_error.set(Some(format!("Failed to delete deployment: {}", e.message)));
                } else {
                    load_deployments();
                }
            });
        }
    };

    // Get replica status display
    let replica_status = |replicas: &DeploymentReplicas| {
        if replicas.ready == replicas.desired {
            format!("{}/{}", replicas.ready, replicas.desired)
        } else {
            format!("{}/{} ({})", replicas.ready, replicas.desired, replicas.current)
        }
    };

    // Get replica status class
    let replica_status_class = |replicas: &DeploymentReplicas| {
        if replicas.ready == replicas.desired && replicas.current == replicas.desired {
            "text-green-600"
        } else if replicas.ready > 0 {
            "text-yellow-600"
        } else {
            "text-red-600"
        }
    };

    // Get condition status
    let get_condition_status = |conditions: &[DeploymentCondition]| {
        for condition in conditions {
            if condition.condition_type == "Available" {
                return (condition.status == "True", condition.reason.clone());
            }
        }
        (false, Some("Unknown".to_string()))
    };

    view! {
        <div class="p-6 space-y-6">
            <div class="flex justify-between items-center">
                <div>
                    <h1 class="text-2xl font-bold text-gray-900">Deployments</h1>
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
                        on:click=move |_| load_deployments()
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
                        "Search Deployments"
                    </label>
                    <input
                        type="text"
                        placeholder="Filter by deployment name..."
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

            // Deployments table
            <div class="bg-white rounded-lg shadow overflow-hidden">
                <div class="overflow-x-auto">
                    <table class="min-w-full divide-y divide-gray-200">
                        <thead class="bg-gray-50">
                            <tr>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Name"
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Ready Replicas"
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Strategy"
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Age"
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Status"
                                </th>
                                <th class="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Actions"
                                </th>
                            </tr>
                        </thead>
                        <tbody class="bg-white divide-y divide-gray-200">
                            {filtered_deployments().into_iter().map(|deployment| {
                                let deployment_for_scale = deployment.clone();
                                let deployment_for_restart = deployment.clone();
                                let deployment_for_delete = deployment.clone();
                                let (available, reason) = get_condition_status(&deployment.conditions);
                                let dep_name = deployment.name.clone();
                                let namespace = deployment.namespace.clone();
                                let replica_cls = replica_status_class(&deployment.replicas);
                                let replica_stat = replica_status(&deployment.replicas);
                                let strategy = deployment.strategy.clone();
                                let age = deployment.age.clone();
                                let status_cls = if available { "bg-green-100 text-green-800" } else { "bg-red-100 text-red-800" };
                                let status_text = if available { "Available" } else { "Unavailable" };

                                view! {
                                    <tr class="hover:bg-gray-50">
                                        <td class="px-6 py-4 whitespace-nowrap">
                                            <div class="text-sm font-medium text-gray-900">{dep_name}</div>
                                            <div class="text-sm text-gray-500">{namespace}</div>
                                        </td>
                                        <td class="px-6 py-4 whitespace-nowrap">
                                            <span class={format!("text-sm font-medium {}", replica_cls)}>
                                                {replica_stat}
                                            </span>
                                        </td>
                                        <td class="px-6 py-4 whitespace-nowrap text-sm text-gray-900">
                                            {strategy}
                                        </td>
                                        <td class="px-6 py-4 whitespace-nowrap text-sm text-gray-900">
                                            {age}
                                        </td>
                                        <td class="px-6 py-4 whitespace-nowrap">
                                            <span class={format!("inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium {}", status_cls)}>
                                                {status_text}
                                            </span>
                                            {reason.map(|r| view! {
                                                <div class="text-xs text-gray-500 mt-1">{r}</div>
                                            })}
                                        </td>
                                        <td class="px-6 py-4 whitespace-nowrap text-right text-sm font-medium">
                                            <div class="flex justify-end space-x-2">
                                                <button
                                                    on:click=move |_| {
                                                        set_selected_deployment.set(Some(deployment_for_scale.clone()));
                                                        set_scale_replicas.set(deployment_for_scale.replicas.desired);
                                                        set_show_scale_modal.set(true);
                                                    }
                                                    class="text-indigo-600 hover:text-indigo-900"
                                                    title="Scale Deployment"
                                                >
                                                    "Scale"
                                                </button>
                                                <button
                                                    on:click=move |_| restart_deployment(deployment_for_restart.name.clone())
                                                    class="text-blue-600 hover:text-blue-900"
                                                    title="Restart Deployment"
                                                >
                                                    "Restart"
                                                </button>
                                                <button
                                                    on:click=move |_| {
                                                        if window().confirm_with_message(&format!("Are you sure you want to delete deployment '{}'?", deployment_for_delete.name)).unwrap_or(false) {
                                                            delete_deployment(deployment_for_delete.name.clone());
                                                        }
                                                    }
                                                    class="text-red-600 hover:text-red-900"
                                                    title="Delete Deployment"
                                                >
                                                    "Delete"
                                                </button>
                                            </div>
                                        </td>
                                    </tr>
                                }
                            }).collect::<Vec<_>>()}

                            {move || {
                                if !loading.get() && filtered_deployments().is_empty() {
                                    view! {
                                        <tr>
                                            <td colspan="6" class="px-6 py-12 text-center text-sm text-gray-500">
                                                "No deployments found matching the current filters."
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
                            "Loading deployments..."
                        </div>
                    </div>
                })}
            </div>

            // Scale Modal
            {move || show_scale_modal.get().then(|| {
                let deployment = selected_deployment.get();
                view! {
                    <div class="fixed inset-0 bg-gray-600 bg-opacity-50 overflow-y-auto h-full w-full z-50">
                        <div class="relative top-20 mx-auto p-5 border w-96 shadow-lg rounded-md bg-white">
                            <div class="flex justify-between items-center mb-4">
                                <h3 class="text-lg font-bold text-gray-900">
                                    "Scale Deployment"
                                </h3>
                                <button
                                    on:click=move |_| set_show_scale_modal.set(false)
                                    class="text-gray-400 hover:text-gray-600"
                                >
                                    <svg class="w-6 h-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"></path>
                                    </svg>
                                </button>
                            </div>

                            {deployment.as_ref().map(|dep| view! {
                                <div class="space-y-4">
                                    <div>
                                        <p class="text-sm text-gray-600">"Deployment: " <strong>{&dep.name}</strong></p>
                                        <p class="text-sm text-gray-600">
                                            "Current replicas: " <strong>{dep.replicas.current}</strong>
                                            " | Ready: " <strong>{dep.replicas.ready}</strong>
                                        </p>
                                    </div>

                                    <div>
                                        <label class="block text-sm font-medium text-gray-700 mb-2">
                                            "Target Replicas"
                                        </label>
                                        <input
                                            type="number"
                                            min="0"
                                            max="100"
                                            class="w-full px-3 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                                            prop:value=scale_replicas
                                            on:input=move |ev| {
                                                if let Ok(value) = event_target_value(&ev).parse::<u32>() {
                                                    set_scale_replicas.set(value);
                                                }
                                            }
                                        />
                                    </div>

                                    <div class="flex justify-end space-x-2 pt-4">
                                        <button
                                            on:click=move |_| set_show_scale_modal.set(false)
                                            disabled=scaling
                                            class="px-4 py-2 bg-gray-300 text-gray-700 rounded-lg hover:bg-gray-400 disabled:opacity-50"
                                        >
                                            "Cancel"
                                        </button>
                                        <button
                                            on:click=move |_| {
                                                if let Some(deployment) = selected_deployment.get() {
                                                    scale_deployment(deployment.name.clone(), scale_replicas.get());
                                                }
                                            }
                                            disabled=scaling
                                            class="px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 disabled:opacity-50"
                                        >
                                            {move || if scaling.get() { "Scaling..." } else { "Scale" }}
                                        </button>
                                    </div>
                                </div>
                            })}
                        </div>
                    </div>
                }
            })}

            // Statistics footer
            <div class="bg-white rounded-lg shadow p-4">
                <div class="grid grid-cols-2 md:grid-cols-4 gap-4 text-center">
                    <div>
                        <div class="text-2xl font-bold text-gray-900">{move || filtered_deployments().len()}</div>
                        <div class="text-sm text-gray-500">"Total Deployments"</div>
                    </div>
                    <div>
                        <div class="text-2xl font-bold text-green-600">
                            {move || filtered_deployments().iter().filter(|d| get_condition_status(&d.conditions).0).count()}
                        </div>
                        <div class="text-sm text-gray-500">"Available"</div>
                    </div>
                    <div>
                        <div class="text-2xl font-bold text-blue-600">
                            {move || filtered_deployments().iter().map(|d| d.replicas.ready).sum::<u32>()}
                        </div>
                        <div class="text-sm text-gray-500">"Ready Replicas"</div>
                    </div>
                    <div>
                        <div class="text-2xl font-bold text-gray-600">
                            {move || filtered_deployments().iter().map(|d| d.replicas.desired).sum::<u32>()}
                        </div>
                        <div class="text-sm text-gray-500">"Desired Replicas"</div>
                    </div>
                </div>
            </div>
        </div>
    }
}