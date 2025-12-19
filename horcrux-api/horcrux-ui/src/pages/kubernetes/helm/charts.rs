//! Helm Charts Browsing and Installation Page
//!
//! Provides comprehensive Helm chart management including:
//! - Chart search and browsing across repositories
//! - Chart details and version comparison
//! - Chart installation with custom values
//! - Release management and history

use leptos::*;
use leptos_router::*;
use crate::api::{self, HelmChart, HelmRelease, HelmInstallRequest};

#[component]
pub fn HelmChartsPage() -> impl IntoView {
    let params = use_params_map();
    let cluster_id = move || params.with(|p| p.get("cluster_id").cloned().unwrap_or_default());

    let (charts, set_charts) = create_signal::<Vec<HelmChart>>(vec![]);
    let (releases, set_releases) = create_signal::<Vec<HelmRelease>>(vec![]);
    let (loading, set_loading) = create_signal(true);
    let (releases_loading, set_releases_loading) = create_signal(true);
    let (error, set_error) = create_signal::<Option<String>>(None);
    let (search_query, set_search_query) = create_signal(String::new());
    let (selected_repo, set_selected_repo) = create_signal(String::new());
    let (selected_chart, set_selected_chart) = create_signal::<Option<HelmChart>>(None);
    let (show_install_modal, set_show_install_modal) = create_signal(false);
    let (installing, set_installing) = create_signal(false);
    let (chart_versions, set_chart_versions) = create_signal::<Vec<String>>(vec![]);
    let (chart_values, set_chart_values) = create_signal::<Option<serde_json::Value>>(None);

    // Install form fields
    let (release_name, set_release_name) = create_signal(String::new());
    let (target_namespace, set_target_namespace) = create_signal("default".to_string());
    let (selected_version, set_selected_version) = create_signal(String::new());
    let (custom_values, set_custom_values) = create_signal(String::new());
    let (create_namespace, set_create_namespace) = create_signal(false);
    let (wait_for_install, set_wait_for_install) = create_signal(true);

    // Reset install form helper
    let reset_install_form = move || {
        set_release_name.set(String::new());
        set_target_namespace.set("default".to_string());
        set_selected_version.set(String::new());
        set_custom_values.set(String::new());
        set_create_namespace.set(false);
        set_wait_for_install.set(true);
    };

    // Search charts
    let search_charts = move || {
        let query = search_query.get();
        let repo = if selected_repo.get().is_empty() { None } else { Some(selected_repo.get()) };

        if query.is_empty() {
            set_charts.set(vec![]);
            return;
        }

        if query.len() < 2 {
            return;
        }

        set_loading.set(true);
        spawn_local(async move {
            match api::search_helm_charts(&query, repo.as_deref()).await {
                Ok(data) => {
                    set_charts.set(data);
                    set_error.set(None);
                }
                Err(e) => {
                    set_error.set(Some(e.message));
                    set_charts.set(vec![]);
                }
            }
            set_loading.set(false);
        });
    };

    // Load releases
    let load_releases = {
        let cluster_id = cluster_id.clone();
        move || {
            let cluster_id = cluster_id();
            if cluster_id.is_empty() {
                return;
            }

            set_releases_loading.set(true);
            spawn_local(async move {
                match api::get_helm_releases(&cluster_id, None).await {
                    Ok(data) => {
                        set_releases.set(data);
                    }
                    Err(e) => {
                        set_error.set(Some(e.message));
                    }
                }
                set_releases_loading.set(false);
            });
        }
    };

    // Initial load
    create_effect(move |_| {
        load_releases();
    });

    // Load chart details when selected
    let load_chart_details = move |chart: HelmChart| {
        set_selected_chart.set(Some(chart.clone()));
        set_release_name.set(chart.name.clone());
        set_selected_version.set(chart.version.clone());

        // Clone values for async closures
        let chart_repo = chart.repository.clone();
        let chart_name = chart.name.clone();
        let chart_version = chart.version.clone();
        let chart_repo_2 = chart.repository.clone();
        let chart_name_2 = chart.name.clone();
        let chart_version_2 = chart.version.clone();

        // Load versions
        spawn_local(async move {
            match api::get_helm_chart_versions(&chart_repo, &chart_name).await {
                Ok(versions) => {
                    set_chart_versions.set(versions);
                }
                Err(e) => {
                    set_error.set(Some(format!("Failed to load chart versions: {}", e.message)));
                }
            }
        });

        // Load default values
        spawn_local(async move {
            match api::get_helm_chart_values(&chart_repo_2, &chart_name_2, Some(&chart_version_2)).await {
                Ok(values) => {
                    set_chart_values.set(Some(values.clone()));
                    set_custom_values.set(serde_json::to_string_pretty(&values).unwrap_or_default());
                }
                Err(e) => {
                    set_error.set(Some(format!("Failed to load chart values: {}", e.message)));
                }
            }
        });

        set_show_install_modal.set(true);
    };

    // Install chart
    let install_chart = {
        let cluster_id = cluster_id.clone();
        move || {
            let cluster_id = cluster_id();
            let chart = match selected_chart.get() {
                Some(chart) => chart,
                None => return,
            };

            let name = release_name.get();
            let namespace = target_namespace.get();
            let version = if selected_version.get().is_empty() { None } else { Some(selected_version.get()) };
            let values_text = custom_values.get();

            if name.is_empty() {
                set_error.set(Some("Release name is required".to_string()));
                return;
            }

            if namespace.is_empty() {
                set_error.set(Some("Namespace is required".to_string()));
                return;
            }

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

            set_installing.set(true);
            spawn_local(async move {
                let request = HelmInstallRequest {
                    name,
                    chart: format!("{}/{}", chart.repository, chart.name),
                    version,
                    values,
                    create_namespace: if create_namespace.get() { Some(true) } else { None },
                    wait: if wait_for_install.get() { Some(true) } else { None },
                    timeout: Some("300s".to_string()),
                };

                match api::install_helm_chart(&cluster_id, &namespace, request).await {
                    Ok(_) => {
                        set_show_install_modal.set(false);
                        load_releases();
                        reset_install_form();
                        set_error.set(None);
                    }
                    Err(e) => {
                        set_error.set(Some(format!("Failed to install chart: {}", e.message)));
                    }
                }
                set_installing.set(false);
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

    // Reset install form
    let reset_install_form = move || {
        set_release_name.set(String::new());
        set_target_namespace.set("default".to_string());
        set_selected_version.set(String::new());
        set_custom_values.set(String::new());
        set_create_namespace.set(false);
        set_wait_for_install.set(true);
        set_selected_chart.set(None);
        set_chart_versions.set(vec![]);
        set_chart_values.set(None);
    };

    // Get release status class
    let release_status_class = |status: &str| {
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
                    <h1 class="text-2xl font-bold text-gray-900">Helm Charts</h1>
                    <p class="mt-1 text-sm text-gray-500">
                        "Browse and install Helm charts to deploy applications"
                    </p>
                </div>
            </div>

            // Search and filters
            <div class="bg-white p-4 rounded-lg shadow space-y-4">
                <div class="grid grid-cols-1 md:grid-cols-3 gap-4">
                    <div class="md:col-span-2">
                        <label class="block text-sm font-medium text-gray-700 mb-2">
                            "Search Charts"
                        </label>
                        <input
                            type="text"
                            placeholder="Search for charts (e.g., nginx, wordpress, mysql)..."
                            class="w-full px-3 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                            prop:value=search_query
                            on:input=move |ev| {
                                set_search_query.set(event_target_value(&ev));
                                search_charts();
                            }
                        />
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-2">
                            "Repository Filter"
                        </label>
                        <select
                            class="w-full px-3 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                            on:change=move |ev| {
                                set_selected_repo.set(event_target_value(&ev));
                                search_charts();
                            }
                        >
                            <option value="">"All Repositories"</option>
                            <option value="stable">"Stable"</option>
                            <option value="bitnami">"Bitnami"</option>
                            <option value="ingress-nginx">"Ingress NGINX"</option>
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

            <div class="grid grid-cols-1 lg:grid-cols-2 gap-6">
                // Charts search results
                <div class="space-y-4">
                    <h2 class="text-lg font-medium text-gray-900">"Available Charts"</h2>

                    {move || {
                        if loading.get() {
                            view! {
                                <div class="bg-white rounded-lg shadow p-6 text-center">
                                    <div class="inline-flex items-center">
                                        <div class="animate-spin -ml-1 mr-3 h-5 w-5 text-blue-500">
                                            <svg class="w-5 h-5" fill="none" viewBox="0 0 24 24">
                                                <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4"></circle>
                                                <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"></path>
                                            </svg>
                                        </div>
                                        "Searching charts..."
                                    </div>
                                </div>
                            }.into_view()
                        } else if search_query.get().is_empty() {
                            view! {
                                <div class="bg-white rounded-lg shadow p-6 text-center">
                                    <svg class="mx-auto h-12 w-12 text-gray-400" fill="none" stroke="currentColor" viewBox="0 0 48 48">
                                        <path stroke-linecap="round" stroke-linejoin="round" stroke_width="2" d="M21 21l-6-6m2-5a7 7 0 1114 0 7 7 0 01-14 0zM10 14l6-6"></path>
                                    </svg>
                                    <h3 class="mt-2 text-sm font-medium text-gray-900">"Search for charts"</h3>
                                    <p class="mt-1 text-sm text-gray-500">"Enter a chart name or keyword to get started"</p>
                                </div>
                            }.into_view()
                        } else if charts.get().is_empty() {
                            view! {
                                <div class="bg-white rounded-lg shadow p-6 text-center">
                                    <svg class="mx-auto h-12 w-12 text-gray-400" fill="none" stroke="currentColor" viewBox="0 0 48 48">
                                        <path stroke-linecap="round" stroke-linejoin="round" stroke_width="2" d="M20 13V6a2 2 0 00-2-2H6a2 2 0 00-2 2v7h16zM4 20v10a2 2 0 002 2h12a2 2 0 002-2V20H4z"></path>
                                    </svg>
                                    <h3 class="mt-2 text-sm font-medium text-gray-900">"No charts found"</h3>
                                    <p class="mt-1 text-sm text-gray-500">"Try adjusting your search terms or repository filter"</p>
                                </div>
                            }.into_view()
                        } else {
                            view! {
                                <div class="space-y-4 max-h-96 overflow-y-auto">
                                    {charts.get().into_iter().map(|chart| {
                                        let chart_for_install = chart.clone();
                                        let chart_name = chart.name.clone();
                                        let chart_repository = chart.repository.clone();
                                        let chart_description = chart.description.clone();
                                        let chart_version = chart.version.clone();
                                        let chart_app_version = chart.app_version.clone();
                                        let chart_created = chart.created.clone();
                                        let chart_keywords = chart.keywords.clone();
                                        view! {
                                            <div class="bg-white rounded-lg shadow p-4 hover:shadow-md transition-shadow">
                                                <div class="flex justify-between items-start">
                                                    <div class="flex-1">
                                                        <div class="flex items-center space-x-2">
                                                            <h3 class="text-lg font-medium text-gray-900">{chart_name}</h3>
                                                            <span class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-blue-100 text-blue-800">
                                                                {chart_repository}
                                                            </span>
                                                        </div>
                                                        <p class="text-sm text-gray-500 mt-1">{chart_description}</p>
                                                        <div class="mt-2 flex items-center space-x-4 text-xs text-gray-500">
                                                            <span>"Version: " {chart_version}</span>
                                                            {chart_app_version.map(|v| view! {
                                                                <span>"App: " {v}</span>
                                                            })}
                                                            <span>"Updated: " {chart_created}</span>
                                                        </div>
                                                        {if !chart_keywords.is_empty() {
                                                            view! {
                                                                <div class="mt-2 flex flex-wrap gap-1">
                                                                    {chart_keywords.iter().take(3).map(|keyword| view! {
                                                                        <span class="inline-flex items-center px-2 py-0.5 rounded text-xs bg-gray-100 text-gray-800">
                                                                            {keyword}
                                                                        </span>
                                                                    }).collect_view()}
                                                                </div>
                                                            }.into_view()
                                                        } else {
                                                            view! {}.into_view()
                                                        }}
                                                    </div>
                                                    <button
                                                        on:click=move |_| load_chart_details(chart_for_install.clone())
                                                        class="ml-4 px-4 py-2 bg-blue-600 text-white text-sm rounded-lg hover:bg-blue-700"
                                                    >
                                                        "Install"
                                                    </button>
                                                </div>
                                            </div>
                                        }
                                    }).collect::<Vec<_>>()}
                                </div>
                            }.into_view()
                        }
                    }}
                </div>

                // Installed releases
                <div class="space-y-4">
                    <div class="flex justify-between items-center">
                        <h2 class="text-lg font-medium text-gray-900">"Installed Releases"</h2>
                        <button
                            on:click=move |_| load_releases()
                            disabled=releases_loading
                            class="px-3 py-1 bg-blue-600 text-white text-sm rounded hover:bg-blue-700 disabled:opacity-50"
                        >
                            {move || if releases_loading.get() { "Loading..." } else { "Refresh" }}
                        </button>
                    </div>

                    {move || {
                        if releases_loading.get() {
                            view! {
                                <div class="bg-white rounded-lg shadow p-6 text-center">
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
                            }.into_view()
                        } else if releases.get().is_empty() {
                            view! {
                                <div class="bg-white rounded-lg shadow p-6 text-center">
                                    <svg class="mx-auto h-12 w-12 text-gray-400" fill="none" stroke="currentColor" viewBox="0 0 48 48">
                                        <path stroke-linecap="round" stroke-linejoin="round" stroke_width="2" d="M19 11H5a2 2 0 00-2 2v8a2 2 0 002 2h14m15-5v2a2 2 0 01-2 2H9"></path>
                                    </svg>
                                    <h3 class="mt-2 text-sm font-medium text-gray-900">"No releases installed"</h3>
                                    <p class="mt-1 text-sm text-gray-500">"Install some charts to get started"</p>
                                </div>
                            }.into_view()
                        } else {
                            view! {
                                <div class="space-y-3 max-h-96 overflow-y-auto">
                                    {releases.get().into_iter().map(|release| {
                                        let release_for_uninstall = release.clone();
                                        let release_name = release.name.clone();
                                        let release_status = release.status.clone();
                                        let status_class = release_status_class(&release.status);
                                        let release_chart = release.chart.clone();
                                        let release_namespace = release.namespace.clone();
                                        let release_revision = release.revision;
                                        view! {
                                            <div class="bg-white rounded-lg shadow p-3">
                                                <div class="flex justify-between items-start">
                                                    <div class="flex-1">
                                                        <div class="flex items-center space-x-2">
                                                            <h4 class="font-medium text-gray-900">{release_name}</h4>
                                                            <span class={format!("inline-flex items-center px-2 py-0.5 rounded-full text-xs font-medium {}", status_class)}>
                                                                {release_status}
                                                            </span>
                                                        </div>
                                                        <div class="text-sm text-gray-500">
                                                            <p>"Chart: " {release_chart}</p>
                                                            <p>"Namespace: " {release_namespace}</p>
                                                            <p>"Revision: " {release_revision}</p>
                                                        </div>
                                                    </div>
                                                    <button
                                                        on:click=move |_| {
                                                            if window().confirm_with_message(&format!("Are you sure you want to uninstall release '{}'?", release_for_uninstall.name)).unwrap_or(false) {
                                                                uninstall_release(release_for_uninstall.clone());
                                                            }
                                                        }
                                                        class="ml-2 px-2 py-1 bg-red-600 text-white text-xs rounded hover:bg-red-700"
                                                    >
                                                        "Uninstall"
                                                    </button>
                                                </div>
                                            </div>
                                        }
                                    }).collect::<Vec<_>>()}
                                </div>
                            }.into_view()
                        }
                    }}
                </div>
            </div>

            // Install Chart Modal
            {move || show_install_modal.get().then(|| {
                let chart = selected_chart.get();
                view! {
                    <div class="fixed inset-0 bg-gray-600 bg-opacity-50 overflow-y-auto h-full w-full z-50">
                        <div class="relative top-5 mx-auto p-5 border w-11/12 max-w-4xl shadow-lg rounded-md bg-white">
                            <div class="flex justify-between items-center mb-4">
                                <h3 class="text-lg font-bold text-gray-900">
                                    "Install Chart: " {chart.as_ref().map(|c| c.name.clone()).unwrap_or_default()}
                                </h3>
                                <button
                                    on:click=move |_| {
                                        set_show_install_modal.set(false);
                                        reset_install_form();
                                    }
                                    class="text-gray-400 hover:text-gray-600"
                                >
                                    <svg class="w-6 h-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"></path>
                                    </svg>
                                </button>
                            </div>

                            <div class="grid grid-cols-1 lg:grid-cols-2 gap-6 max-h-96 overflow-y-auto">
                                // Installation configuration
                                <div class="space-y-4">
                                    <h4 class="font-medium text-gray-900">"Installation Configuration"</h4>

                                    <div>
                                        <label class="block text-sm font-medium text-gray-700 mb-2">
                                            "Release Name"
                                        </label>
                                        <input
                                            type="text"
                                            required
                                            class="w-full px-3 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                                            prop:value=release_name
                                            on:input=move |ev| set_release_name.set(event_target_value(&ev))
                                        />
                                    </div>

                                    <div>
                                        <label class="block text-sm font-medium text-gray-700 mb-2">
                                            "Target Namespace"
                                        </label>
                                        <input
                                            type="text"
                                            required
                                            class="w-full px-3 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                                            prop:value=target_namespace
                                            on:input=move |ev| set_target_namespace.set(event_target_value(&ev))
                                        />
                                    </div>

                                    <div>
                                        <label class="block text-sm font-medium text-gray-700 mb-2">
                                            "Chart Version"
                                        </label>
                                        <select
                                            class="w-full px-3 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                                            on:change=move |ev| set_selected_version.set(event_target_value(&ev))
                                        >
                                            {chart_versions.get().into_iter().map(|version| {
                                                let version_value = version.clone();
                                                let version_display = version.clone();
                                                let is_selected = selected_version.get() == version;
                                                view! {
                                                    <option value={version_value} selected=is_selected>
                                                        {version_display}
                                                    </option>
                                                }
                                            }).collect::<Vec<_>>()}
                                        </select>
                                    </div>

                                    <div class="space-y-2">
                                        <label class="flex items-center">
                                            <input
                                                type="checkbox"
                                                class="rounded border-gray-300"
                                                prop:checked=create_namespace
                                                on:change=move |ev| set_create_namespace.set(event_target_checked(&ev))
                                            />
                                            <span class="ml-2 text-sm text-gray-700">"Create namespace if it doesn't exist"</span>
                                        </label>
                                        <label class="flex items-center">
                                            <input
                                                type="checkbox"
                                                class="rounded border-gray-300"
                                                prop:checked=wait_for_install
                                                on:change=move |ev| set_wait_for_install.set(event_target_checked(&ev))
                                            />
                                            <span class="ml-2 text-sm text-gray-700">"Wait for installation to complete"</span>
                                        </label>
                                    </div>
                                </div>

                                // Values configuration
                                <div class="space-y-4">
                                    <h4 class="font-medium text-gray-900">"Custom Values (YAML/JSON)"</h4>
                                    <div>
                                        <textarea
                                            rows="12"
                                            placeholder="# Enter custom values here in YAML or JSON format"
                                            class="w-full px-3 py-2 border border-gray-300 rounded-lg font-mono text-sm focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                                            prop:value=custom_values
                                            on:input=move |ev| set_custom_values.set(event_target_value(&ev))
                                        ></textarea>
                                        <p class="mt-1 text-xs text-gray-500">
                                            "Modify the values above to customize the chart installation"
                                        </p>
                                    </div>
                                </div>
                            </div>

                            <div class="flex justify-end space-x-2 pt-4 mt-4 border-t">
                                <button
                                    on:click=move |_| {
                                        set_show_install_modal.set(false);
                                        reset_install_form();
                                    }
                                    disabled=installing
                                    class="px-4 py-2 bg-gray-300 text-gray-700 rounded-lg hover:bg-gray-400 disabled:opacity-50"
                                >
                                    "Cancel"
                                </button>
                                <button
                                    on:click=move |_| install_chart()
                                    disabled=installing
                                    class="px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 disabled:opacity-50"
                                >
                                    {move || if installing.get() { "Installing..." } else { "Install Chart" }}
                                </button>
                            </div>
                        </div>
                    </div>
                }
            })}
        </div>
    }
}