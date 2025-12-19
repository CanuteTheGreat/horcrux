//! Helm Releases Management Page
//!
//! Provides comprehensive Helm release management including:
//! - Release listing with status and history
//! - Release upgrade and rollback operations
//! - Values inspection and modification
//! - Release lifecycle management

use leptos::*;
use leptos_router::*;
use crate::api::{self, HelmRelease, HelmValues, HelmInstallRequest};

#[component]
pub fn HelmReleasesPage() -> impl IntoView {
    let params = use_params_map();
    let cluster_id = move || params.with(|p| p.get("cluster_id").cloned().unwrap_or_default());
    let namespace = move || params.with(|p| p.get("namespace").cloned().unwrap_or_default());

    let (releases, set_releases) = create_signal::<Vec<HelmRelease>>(vec![]);
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal::<Option<String>>(None);
    let (selected_release, set_selected_release) = create_signal::<Option<HelmRelease>>(None);
    let (show_values_modal, set_show_values_modal) = create_signal(false);
    let (show_history_modal, set_show_history_modal) = create_signal(false);
    let (show_upgrade_modal, set_show_upgrade_modal) = create_signal(false);
    let (release_values, set_release_values) = create_signal::<Option<HelmValues>>(None);
    let (release_history, set_release_history) = create_signal::<Vec<HelmRelease>>(vec![]);
    let (upgrading, set_upgrading) = create_signal(false);
    let (search_filter, set_search_filter) = create_signal(String::new());
    let (status_filter, set_status_filter) = create_signal(String::new());
    let (auto_refresh, set_auto_refresh) = create_signal(true);

    // Upgrade form fields
    let (new_chart_version, set_new_chart_version) = create_signal(String::new());
    let (upgrade_values, set_upgrade_values) = create_signal(String::new());

    // Load releases
    let load_releases = {
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
                let ns = if namespace.is_empty() { None } else { Some(namespace) };
                match api::get_helm_releases(&cluster_id, ns.as_deref()).await {
                    Ok(data) => {
                        set_releases.set(data);
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
            load_releases();
            set_interval_with_handle(
                move || {
                    if auto_refresh.get() {
                        load_releases();
                    }
                },
                std::time::Duration::from_secs(15),
            ).ok();
        }
    });

    // Initial load
    create_effect(move |_| {
        load_releases();
    });

    // Filter releases
    let filtered_releases = move || {
        let search = search_filter.get().to_lowercase();
        let status = status_filter.get();

        releases.get()
            .into_iter()
            .filter(|release| {
                let name_match = search.is_empty() || release.name.to_lowercase().contains(&search);
                let namespace_match = search.is_empty() || release.namespace.to_lowercase().contains(&search);
                let chart_match = search.is_empty() || release.chart.to_lowercase().contains(&search);
                let search_match = name_match || namespace_match || chart_match;

                let status_match = status.is_empty() || release.status.eq_ignore_ascii_case(&status);

                search_match && status_match
            })
            .collect::<Vec<_>>()
    };

    // View release values
    let view_values = {
        let cluster_id = cluster_id.clone();
        move |release: HelmRelease| {
            let cluster_id = cluster_id();
            set_selected_release.set(Some(release.clone()));
            set_show_values_modal.set(true);

            spawn_local(async move {
                match api::get_helm_release_values(&cluster_id, &release.namespace, &release.name).await {
                    Ok(values) => {
                        set_release_values.set(Some(values));
                        set_error.set(None);
                    }
                    Err(e) => {
                        set_error.set(Some(format!("Failed to load release values: {}", e.message)));
                    }
                }
            });
        }
    };

    // View release history
    let view_history = {
        let cluster_id = cluster_id.clone();
        move |release: HelmRelease| {
            let cluster_id = cluster_id();
            set_selected_release.set(Some(release.clone()));
            set_show_history_modal.set(true);

            spawn_local(async move {
                match api::get_helm_release_history(&cluster_id, &release.namespace, &release.name).await {
                    Ok(history) => {
                        set_release_history.set(history);
                        set_error.set(None);
                    }
                    Err(e) => {
                        set_error.set(Some(format!("Failed to load release history: {}", e.message)));
                    }
                }
            });
        }
    };

    // Prepare upgrade
    let prepare_upgrade = move |release: HelmRelease| {
        set_selected_release.set(Some(release.clone()));
        set_new_chart_version.set(String::new());

        // Load current values for editing
        spawn_local(async move {
            match api::get_helm_release_values(&cluster_id(), &release.namespace, &release.name).await {
                Ok(values) => {
                    set_upgrade_values.set(serde_json::to_string_pretty(&values.user_supplied_values).unwrap_or_default());
                }
                Err(e) => {
                    set_error.set(Some(format!("Failed to load current values: {}", e.message)));
                }
            }
        });

        set_show_upgrade_modal.set(true);
    };

    // Upgrade release
    let upgrade_release = {
        let cluster_id = cluster_id.clone();
        move || {
            let cluster_id = cluster_id();
            let release = match selected_release.get() {
                Some(release) => release,
                None => return,
            };

            let version = if new_chart_version.get().is_empty() { None } else { Some(new_chart_version.get()) };
            let values_text = upgrade_values.get();

            let values = if values_text.trim().is_empty() {
                None
            } else {
                match serde_json::from_str(&values_text) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        set_error.set(Some(format!("Invalid YAML/JSON values: {}", e)));
                        return;
                    }
                }
            };

            set_upgrading.set(true);
            spawn_local(async move {
                let request = HelmInstallRequest {
                    name: release.name.clone(),
                    chart: release.chart.clone(),
                    version,
                    values,
                    create_namespace: None,
                    wait: Some(true),
                    timeout: Some("300s".to_string()),
                };

                match api::upgrade_helm_release(&cluster_id, &release.namespace, &release.name, request).await {
                    Ok(_) => {
                        set_show_upgrade_modal.set(false);
                        load_releases();
                        set_error.set(None);
                    }
                    Err(e) => {
                        set_error.set(Some(format!("Failed to upgrade release: {}", e.message)));
                    }
                }
                set_upgrading.set(false);
            });
        }
    };

    // Rollback release
    let rollback_release = {
        let cluster_id = cluster_id.clone();
        move |release: HelmRelease, revision: u32| {
            let cluster_id = cluster_id();
            spawn_local(async move {
                match api::rollback_helm_release(&cluster_id, &release.namespace, &release.name, revision).await {
                    Ok(_) => {
                        load_releases();
                        set_show_history_modal.set(false);
                        set_error.set(None);
                    }
                    Err(e) => {
                        set_error.set(Some(format!("Failed to rollback release: {}", e.message)));
                    }
                }
            });
        }
    };

    // Uninstall release
    let uninstall_release = {
        let cluster_id = cluster_id.clone();
        move |release: HelmRelease| {
            let cluster_id = cluster_id();
            spawn_local(async move {
                match api::uninstall_helm_release(&cluster_id, &release.namespace, &release.name).await {
                    Ok(()) => {
                        load_releases();
                        set_error.set(None);
                    }
                    Err(e) => {
                        set_error.set(Some(format!("Failed to uninstall release: {}", e.message)));
                    }
                }
            });
        }
    };

    // Get status badge class
    let status_class = |status: &str| {
        match status {
            "deployed" => "bg-green-100 text-green-800",
            "pending-install" | "pending-upgrade" => "bg-yellow-100 text-yellow-800",
            "failed" => "bg-red-100 text-red-800",
            "uninstalled" => "bg-gray-100 text-gray-800",
            _ => "bg-blue-100 text-blue-800",
        }
    };

    view! {
        <div class="p-6 space-y-6">
            <div class="flex justify-between items-center">
                <div>
                    <h1 class="text-2xl font-bold text-gray-900">Helm Releases</h1>
                    <p class="mt-1 text-sm text-gray-500">
                        "Manage deployed Helm releases and their lifecycle"
                        {if !namespace().is_empty() {
                            format!(" in namespace: {}", namespace())
                        } else {
                            " across all namespaces".to_string()
                        }}
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
                        on:click=move |_| load_releases()
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
                            "Search Releases"
                        </label>
                        <input
                            type="text"
                            placeholder="Filter by release name, namespace, or chart..."
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
                            <option value="deployed">"Deployed"</option>
                            <option value="pending-install">"Pending Install"</option>
                            <option value="pending-upgrade">"Pending Upgrade"</option>
                            <option value="failed">"Failed"</option>
                            <option value="uninstalled">"Uninstalled"</option>
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

            // Releases table
            <div class="bg-white rounded-lg shadow overflow-hidden">
                <div class="overflow-x-auto">
                    <table class="min-w-full divide-y divide-gray-200">
                        <thead class="bg-gray-50">
                            <tr>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Release"
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Chart"
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Status"
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Revision"
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Updated"
                                </th>
                                <th class="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Actions"
                                </th>
                            </tr>
                        </thead>
                        <tbody class="bg-white divide-y divide-gray-200">
                            {filtered_releases().into_iter().map(|release| {
                                let release_for_values = release.clone();
                                let release_for_history = release.clone();
                                let release_for_upgrade = release.clone();
                                let release_for_uninstall = release.clone();
                                let rel_name = release.name.clone();
                                let namespace = release.namespace.clone();
                                let chart = release.chart.clone();
                                let app_version = release.app_version.clone();
                                let status = release.status.clone();
                                let status_cls = status_class(&release.status);
                                let revision = release.revision;
                                let updated = release.updated.clone();

                                view! {
                                    <tr class="hover:bg-gray-50">
                                        <td class="px-6 py-4 whitespace-nowrap">
                                            <div class="text-sm font-medium text-gray-900">{rel_name}</div>
                                            <div class="text-sm text-gray-500">{namespace}</div>
                                        </td>
                                        <td class="px-6 py-4 whitespace-nowrap">
                                            <div class="text-sm text-gray-900">{chart}</div>
                                            {app_version.map(|version| view! {
                                                <div class="text-sm text-gray-500">"App: " {version}</div>
                                            })}
                                        </td>
                                        <td class="px-6 py-4 whitespace-nowrap">
                                            <span class={format!("inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium {}", status_cls)}>
                                                {status}
                                            </span>
                                        </td>
                                        <td class="px-6 py-4 whitespace-nowrap text-sm text-gray-900">
                                            {revision}
                                        </td>
                                        <td class="px-6 py-4 whitespace-nowrap text-sm text-gray-900">
                                            {updated}
                                        </td>
                                        <td class="px-6 py-4 whitespace-nowrap text-right text-sm font-medium">
                                            <div class="flex justify-end space-x-2">
                                                <button
                                                    on:click=move |_| view_values(release_for_values.clone())
                                                    class="text-indigo-600 hover:text-indigo-900"
                                                    title="View Values"
                                                >
                                                    "Values"
                                                </button>
                                                <button
                                                    on:click=move |_| view_history(release_for_history.clone())
                                                    class="text-blue-600 hover:text-blue-900"
                                                    title="View History"
                                                >
                                                    "History"
                                                </button>
                                                <button
                                                    on:click=move |_| prepare_upgrade(release_for_upgrade.clone())
                                                    class="text-green-600 hover:text-green-900"
                                                    title="Upgrade Release"
                                                >
                                                    "Upgrade"
                                                </button>
                                                <button
                                                    on:click=move |_| {
                                                        if window().confirm_with_message(&format!("Are you sure you want to uninstall release '{}'?", release_for_uninstall.name)).unwrap_or(false) {
                                                            uninstall_release(release_for_uninstall.clone());
                                                        }
                                                    }
                                                    class="text-red-600 hover:text-red-900"
                                                    title="Uninstall Release"
                                                >
                                                    "Uninstall"
                                                </button>
                                            </div>
                                        </td>
                                    </tr>
                                }
                            }).collect::<Vec<_>>()}

                            {move || {
                                if !loading.get() && filtered_releases().is_empty() {
                                    view! {
                                        <tr>
                                            <td colspan="6" class="px-6 py-12 text-center text-sm text-gray-500">
                                                "No releases found matching the current filters."
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
                            "Loading releases..."
                        </div>
                    </div>
                })}
            </div>

            // Values Modal
            {move || show_values_modal.get().then(|| {
                let release = selected_release.get();
                let values = release_values.get();
                view! {
                    <div class="fixed inset-0 bg-gray-600 bg-opacity-50 overflow-y-auto h-full w-full z-50">
                        <div class="relative top-10 mx-auto p-5 border w-11/12 max-w-4xl shadow-lg rounded-md bg-white">
                            <div class="flex justify-between items-center mb-4">
                                <h3 class="text-lg font-bold text-gray-900">
                                    "Values: " {release.as_ref().map(|r| r.name.clone()).unwrap_or_default()}
                                </h3>
                                <button
                                    on:click=move |_| set_show_values_modal.set(false)
                                    class="text-gray-400 hover:text-gray-600"
                                >
                                    <svg class="w-6 h-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"></path>
                                    </svg>
                                </button>
                            </div>

                            <div class="max-h-96 overflow-y-auto">
                                {values.map(|v| view! {
                                    <div class="space-y-4">
                                        <div>
                                            <h4 class="font-medium text-gray-900 mb-2">"User Supplied Values"</h4>
                                            <pre class="bg-gray-100 p-4 rounded-lg text-sm font-mono overflow-x-auto">
                                                {serde_json::to_string_pretty(&v.user_supplied_values).unwrap_or_default()}
                                            </pre>
                                        </div>
                                        <div>
                                            <h4 class="font-medium text-gray-900 mb-2">"Computed Values"</h4>
                                            <pre class="bg-gray-100 p-4 rounded-lg text-sm font-mono overflow-x-auto max-h-64 overflow-y-auto">
                                                {serde_json::to_string_pretty(&v.computed_values).unwrap_or_default()}
                                            </pre>
                                        </div>
                                    </div>
                                })}
                            </div>

                            <div class="flex justify-end pt-4 mt-4 border-t">
                                <button
                                    on:click=move |_| set_show_values_modal.set(false)
                                    class="px-4 py-2 bg-gray-300 text-gray-700 rounded-lg hover:bg-gray-400"
                                >
                                    "Close"
                                </button>
                            </div>
                        </div>
                    </div>
                }
            })}

            // History Modal
            {move || show_history_modal.get().then(|| {
                let release = selected_release.get();
                let history = release_history.get();
                view! {
                    <div class="fixed inset-0 bg-gray-600 bg-opacity-50 overflow-y-auto h-full w-full z-50">
                        <div class="relative top-10 mx-auto p-5 border w-11/12 max-w-3xl shadow-lg rounded-md bg-white">
                            <div class="flex justify-between items-center mb-4">
                                <h3 class="text-lg font-bold text-gray-900">
                                    "Release History: " {release.as_ref().map(|r| r.name.clone()).unwrap_or_default()}
                                </h3>
                                <button
                                    on:click=move |_| set_show_history_modal.set(false)
                                    class="text-gray-400 hover:text-gray-600"
                                >
                                    <svg class="w-6 h-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"></path>
                                    </svg>
                                </button>
                            </div>

                            <div class="max-h-96 overflow-y-auto">
                                <table class="min-w-full divide-y divide-gray-200">
                                    <thead class="bg-gray-50">
                                        <tr>
                                            <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase">
                                                "Revision"
                                            </th>
                                            <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase">
                                                "Updated"
                                            </th>
                                            <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase">
                                                "Status"
                                            </th>
                                            <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase">
                                                "Chart"
                                            </th>
                                            <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase">
                                                "Description"
                                            </th>
                                            <th class="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase">
                                                "Actions"
                                            </th>
                                        </tr>
                                    </thead>
                                    <tbody class="bg-white divide-y divide-gray-200">
                                        {history.clone().into_iter().map(|hist| {
                                            let hist_for_rollback = hist.clone();
                                            let is_current = release.as_ref().map(|r| r.revision == hist.revision).unwrap_or(false);
                                            let revision = hist.revision;
                                            let updated = hist.updated.clone();
                                            let status = hist.status.clone();
                                            let status_cls = status_class(&hist.status);
                                            let chart = hist.chart.clone();
                                            let description = hist.description.clone();
                                            let release_for_rollback = release.clone();
                                            let row_class = if is_current { "bg-blue-50" } else { "" };
                                            view! {
                                                <tr class=row_class>
                                                    <td class="px-6 py-4 whitespace-nowrap text-sm font-medium text-gray-900">
                                                        {revision}
                                                        {if is_current {
                                                            view! { <span class="ml-2 text-blue-600">"(current)"</span> }.into_view()
                                                        } else {
                                                            view! {}.into_view()
                                                        }}
                                                    </td>
                                                    <td class="px-6 py-4 whitespace-nowrap text-sm text-gray-500">
                                                        {updated}
                                                    </td>
                                                    <td class="px-6 py-4 whitespace-nowrap">
                                                        <span class={format!("inline-flex items-center px-2 py-1 rounded-full text-xs font-medium {}", status_cls)}>
                                                            {status}
                                                        </span>
                                                    </td>
                                                    <td class="px-6 py-4 whitespace-nowrap text-sm text-gray-500">
                                                        {chart}
                                                    </td>
                                                    <td class="px-6 py-4 text-sm text-gray-500">
                                                        {description}
                                                    </td>
                                                    <td class="px-6 py-4 whitespace-nowrap text-right text-sm font-medium">
                                                        {if !is_current {
                                                            view! {
                                                                <button
                                                                    on:click=move |_| {
                                                                        if let Some(rel) = release_for_rollback.clone() {
                                                                            if window().confirm_with_message(&format!("Are you sure you want to rollback to revision {}?", hist_for_rollback.revision)).unwrap_or(false) {
                                                                                rollback_release(rel, hist_for_rollback.revision);
                                                                            }
                                                                        }
                                                                    }
                                                                    class="text-blue-600 hover:text-blue-900"
                                                                >
                                                                    "Rollback"
                                                                </button>
                                                            }.into_view()
                                                        } else {
                                                            view! {}.into_view()
                                                        }}
                                                    </td>
                                                </tr>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </tbody>
                                </table>
                            </div>

                            <div class="flex justify-end pt-4 mt-4 border-t">
                                <button
                                    on:click=move |_| set_show_history_modal.set(false)
                                    class="px-4 py-2 bg-gray-300 text-gray-700 rounded-lg hover:bg-gray-400"
                                >
                                    "Close"
                                </button>
                            </div>
                        </div>
                    </div>
                }
            })}

            // Upgrade Modal
            {move || show_upgrade_modal.get().then(|| {
                let release = selected_release.get();
                view! {
                    <div class="fixed inset-0 bg-gray-600 bg-opacity-50 overflow-y-auto h-full w-full z-50">
                        <div class="relative top-10 mx-auto p-5 border w-11/12 max-w-3xl shadow-lg rounded-md bg-white">
                            <div class="flex justify-between items-center mb-4">
                                <h3 class="text-lg font-bold text-gray-900">
                                    "Upgrade Release: " {release.as_ref().map(|r| r.name.clone()).unwrap_or_default()}
                                </h3>
                                <button
                                    on:click=move |_| set_show_upgrade_modal.set(false)
                                    class="text-gray-400 hover:text-gray-600"
                                >
                                    <svg class="w-6 h-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"></path>
                                    </svg>
                                </button>
                            </div>

                            <div class="space-y-4">
                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-2">
                                        "New Chart Version (optional)"
                                    </label>
                                    <input
                                        type="text"
                                        placeholder="Leave empty to use latest version"
                                        class="w-full px-3 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                                        prop:value=new_chart_version
                                        on:input=move |ev| set_new_chart_version.set(event_target_value(&ev))
                                    />
                                </div>

                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-2">
                                        "Values (YAML/JSON)"
                                    </label>
                                    <textarea
                                        rows="12"
                                        class="w-full px-3 py-2 border border-gray-300 rounded-lg font-mono text-sm focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                                        prop:value=upgrade_values
                                        on:input=move |ev| set_upgrade_values.set(event_target_value(&ev))
                                    ></textarea>
                                </div>
                            </div>

                            <div class="flex justify-end space-x-2 pt-4 mt-4 border-t">
                                <button
                                    on:click=move |_| set_show_upgrade_modal.set(false)
                                    disabled=upgrading
                                    class="px-4 py-2 bg-gray-300 text-gray-700 rounded-lg hover:bg-gray-400 disabled:opacity-50"
                                >
                                    "Cancel"
                                </button>
                                <button
                                    on:click=move |_| upgrade_release()
                                    disabled=upgrading
                                    class="px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 disabled:opacity-50"
                                >
                                    {move || if upgrading.get() { "Upgrading..." } else { "Upgrade Release" }}
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
                        <div class="text-2xl font-bold text-gray-900">{move || filtered_releases().len()}</div>
                        <div class="text-sm text-gray-500">"Total Releases"</div>
                    </div>
                    <div>
                        <div class="text-2xl font-bold text-green-600">
                            {move || filtered_releases().iter().filter(|r| r.status == "deployed").count()}
                        </div>
                        <div class="text-sm text-gray-500">"Deployed"</div>
                    </div>
                    <div>
                        <div class="text-2xl font-bold text-yellow-600">
                            {move || filtered_releases().iter().filter(|r| r.status.starts_with("pending")).count()}
                        </div>
                        <div class="text-sm text-gray-500">"Pending"</div>
                    </div>
                    <div>
                        <div class="text-2xl font-bold text-red-600">
                            {move || filtered_releases().iter().filter(|r| r.status == "failed").count()}
                        </div>
                        <div class="text-sm text-gray-500">"Failed"</div>
                    </div>
                </div>
            </div>
        </div>
    }
}