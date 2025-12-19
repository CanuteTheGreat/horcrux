//! Helm Repository Management Page
//!
//! Provides comprehensive Helm repository management including:
//! - Repository listing and status monitoring
//! - Repository addition with authentication
//! - Repository updates and synchronization
//! - Repository removal and cleanup

use leptos::*;
use crate::api::{self, HelmRepository, AddHelmRepoRequest};

#[component]
pub fn HelmRepositoriesPage() -> impl IntoView {
    let (repositories, set_repositories) = create_signal::<Vec<HelmRepository>>(vec![]);
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal::<Option<String>>(None);
    let (show_add_modal, set_show_add_modal) = create_signal(false);
    let (adding, set_adding) = create_signal(false);
    let (updating, set_updating) = create_signal::<Option<String>>(None);
    let (search_filter, set_search_filter) = create_signal(String::new());
    let (auto_refresh, set_auto_refresh) = create_signal(true);

    // Add repository form fields
    let (repo_name, set_repo_name) = create_signal(String::new());
    let (repo_url, set_repo_url) = create_signal(String::new());
    let (username, set_username) = create_signal(String::new());
    let (password, set_password) = create_signal(String::new());
    let (force_update, set_force_update) = create_signal(false);

    // Reset form helper
    let reset_add_form = move || {
        set_repo_name.set(String::new());
        set_repo_url.set(String::new());
        set_username.set(String::new());
        set_password.set(String::new());
        set_force_update.set(false);
    };

    // Load repositories
    let load_repositories = move || {
        set_loading.set(true);
        spawn_local(async move {
            match api::get_helm_repositories().await {
                Ok(data) => {
                    set_repositories.set(data);
                    set_error.set(None);
                }
                Err(e) => {
                    set_error.set(Some(e.message));
                }
            }
            set_loading.set(false);
        });
    };

    // Auto-refresh effect
    create_effect(move |_| {
        if auto_refresh.get() {
            load_repositories();
            set_interval_with_handle(
                move || {
                    if auto_refresh.get() {
                        load_repositories();
                    }
                },
                std::time::Duration::from_secs(30),
            ).ok();
        }
    });

    // Initial load
    create_effect(move |_| {
        load_repositories();
    });

    // Filter repositories
    let filtered_repositories = move || {
        let search = search_filter.get().to_lowercase();

        repositories.get()
            .into_iter()
            .filter(|repo| {
                search.is_empty()
                    || repo.name.to_lowercase().contains(&search)
                    || repo.url.to_lowercase().contains(&search)
                    || repo.description.as_ref().map_or(false, |d| d.to_lowercase().contains(&search))
            })
            .collect::<Vec<_>>()
    };

    // Add repository
    let add_repository = move || {
        let name = repo_name.get();
        let url = repo_url.get();
        let user = if username.get().is_empty() { None } else { Some(username.get()) };
        let pass = if password.get().is_empty() { None } else { Some(password.get()) };
        let force = if force_update.get() { Some(true) } else { None };

        if name.is_empty() {
            set_error.set(Some("Repository name is required".to_string()));
            return;
        }

        if url.is_empty() {
            set_error.set(Some("Repository URL is required".to_string()));
            return;
        }

        set_adding.set(true);
        spawn_local(async move {
            let request = AddHelmRepoRequest {
                name,
                url,
                username: user,
                password: pass,
                force_update: force,
            };

            match api::add_helm_repository(request).await {
                Ok(_) => {
                    set_show_add_modal.set(false);
                    load_repositories();
                    reset_add_form();
                    set_error.set(None);
                }
                Err(e) => {
                    set_error.set(Some(format!("Failed to add repository: {}", e.message)));
                }
            }
            set_adding.set(false);
        });
    };

    // Update repository
    let update_repository = move |repo_name: String| {
        set_updating.set(Some(repo_name.clone()));
        spawn_local(async move {
            match api::update_helm_repository(&repo_name).await {
                Ok(()) => {
                    load_repositories();
                    set_error.set(None);
                }
                Err(e) => {
                    set_error.set(Some(format!("Failed to update repository: {}", e.message)));
                }
            }
            set_updating.set(None);
        });
    };

    // Remove repository
    let remove_repository = move |repo_name: String| {
        spawn_local(async move {
            match api::remove_helm_repository(&repo_name).await {
                Ok(()) => {
                    load_repositories();
                    set_error.set(None);
                }
                Err(e) => {
                    set_error.set(Some(format!("Failed to remove repository: {}", e.message)));
                }
            }
        });
    };

    // Reset add form
    let reset_add_form = move || {
        set_repo_name.set(String::new());
        set_repo_url.set(String::new());
        set_username.set(String::new());
        set_password.set(String::new());
        set_force_update.set(false);
    };

    // Get status badge class
    let status_class = |status: &str| {
        match status {
            "ready" | "active" => "bg-green-100 text-green-800",
            "updating" | "syncing" => "bg-yellow-100 text-yellow-800",
            "error" | "failed" => "bg-red-100 text-red-800",
            _ => "bg-gray-100 text-gray-800",
        }
    };

    view! {
        <div class="p-6 space-y-6">
            <div class="flex justify-between items-center">
                <div>
                    <h1 class="text-2xl font-bold text-gray-900">Helm Repositories</h1>
                    <p class="mt-1 text-sm text-gray-500">
                        "Manage Helm chart repositories for application deployment"
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
                            reset_add_form();
                            set_show_add_modal.set(true);
                        }
                        class="px-4 py-2 bg-green-600 text-white rounded-lg hover:bg-green-700"
                    >
                        "Add Repository"
                    </button>
                    <button
                        on:click=move |_| load_repositories()
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
                        "Search Repositories"
                    </label>
                    <input
                        type="text"
                        placeholder="Filter by name, URL, or description..."
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

            // Repositories table
            <div class="bg-white rounded-lg shadow overflow-hidden">
                <div class="overflow-x-auto">
                    <table class="min-w-full divide-y divide-gray-200">
                        <thead class="bg-gray-50">
                            <tr>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Name"
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "URL"
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Status"
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Last Update"
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Added"
                                </th>
                                <th class="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    "Actions"
                                </th>
                            </tr>
                        </thead>
                        <tbody class="bg-white divide-y divide-gray-200">
                            {filtered_repositories().into_iter().map(|repo| {
                                let repo_name = repo.name.clone();
                                let repo_name_check = repo.name.clone();
                                let repo_name_check_2 = repo.name.clone();
                                let repo_for_update = repo.clone();
                                let repo_for_delete = repo.clone();
                                let description = repo.description.clone();
                                let url = repo.url.clone();
                                let url_href = repo.url.clone();
                                let status = repo.status.clone();
                                let status_cls = status_class(&repo.status);
                                let last_update = repo.last_update.clone().unwrap_or_else(|| "Never".to_string());
                                let added_at = repo.added_at.clone();

                                view! {
                                    <tr class="hover:bg-gray-50">
                                        <td class="px-6 py-4 whitespace-nowrap">
                                            <div class="text-sm font-medium text-gray-900">{repo_name}</div>
                                            {description.map(|desc| view! {
                                                <div class="text-sm text-gray-500">{desc}</div>
                                            })}
                                        </td>
                                        <td class="px-6 py-4 whitespace-nowrap text-sm text-gray-900">
                                            <a href={url_href} target="_blank" class="text-blue-600 hover:text-blue-800">
                                                {url}
                                            </a>
                                        </td>
                                        <td class="px-6 py-4 whitespace-nowrap">
                                            <span class={format!("inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium {}", status_cls)}>
                                                {status}
                                            </span>
                                        </td>
                                        <td class="px-6 py-4 whitespace-nowrap text-sm text-gray-900">
                                            {last_update}
                                        </td>
                                        <td class="px-6 py-4 whitespace-nowrap text-sm text-gray-900">
                                            {added_at}
                                        </td>
                                        <td class="px-6 py-4 whitespace-nowrap text-right text-sm font-medium">
                                            <div class="flex justify-end space-x-2">
                                                <button
                                                    on:click=move |_| update_repository(repo_for_update.name.clone())
                                                    disabled=move || updating.get().as_ref() == Some(&repo_name_check)
                                                    class="text-indigo-600 hover:text-indigo-900 disabled:opacity-50"
                                                    title="Update Repository"
                                                >
                                                    {move || if updating.get().as_ref() == Some(&repo_name_check_2) { "Updating..." } else { "Update" }}
                                                </button>
                                                <button
                                                    on:click=move |_| {
                                                        if window().confirm_with_message(&format!("Are you sure you want to remove repository '{}'?", repo_for_delete.name)).unwrap_or(false) {
                                                            remove_repository(repo_for_delete.name.clone());
                                                        }
                                                    }
                                                    class="text-red-600 hover:text-red-900"
                                                    title="Remove Repository"
                                                >
                                                    "Remove"
                                                </button>
                                            </div>
                                        </td>
                                    </tr>
                                }
                            }).collect::<Vec<_>>()}

                            {move || {
                                if !loading.get() && filtered_repositories().is_empty() {
                                    view! {
                                        <tr>
                                            <td colspan="6" class="px-6 py-12 text-center text-sm text-gray-500">
                                                "No repositories found matching the current filters."
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
                            "Loading repositories..."
                        </div>
                    </div>
                })}
            </div>

            // Add Repository Modal
            {move || show_add_modal.get().then(|| view! {
                <div class="fixed inset-0 bg-gray-600 bg-opacity-50 overflow-y-auto h-full w-full z-50">
                    <div class="relative top-20 mx-auto p-5 border w-96 shadow-lg rounded-md bg-white">
                        <div class="flex justify-between items-center mb-4">
                            <h3 class="text-lg font-bold text-gray-900">"Add Helm Repository"</h3>
                            <button
                                on:click=move |_| set_show_add_modal.set(false)
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
                                    "Repository Name"
                                </label>
                                <input
                                    type="text"
                                    required
                                    placeholder="e.g., stable, bitnami, etc."
                                    class="w-full px-3 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                                    prop:value=repo_name
                                    on:input=move |ev| set_repo_name.set(event_target_value(&ev))
                                />
                            </div>

                            <div>
                                <label class="block text-sm font-medium text-gray-700 mb-2">
                                    "Repository URL"
                                </label>
                                <input
                                    type="url"
                                    required
                                    placeholder="https://charts.helm.sh/stable"
                                    class="w-full px-3 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                                    prop:value=repo_url
                                    on:input=move |ev| set_repo_url.set(event_target_value(&ev))
                                />
                            </div>

                            <div>
                                <label class="block text-sm font-medium text-gray-700 mb-2">
                                    "Username (Optional)"
                                </label>
                                <input
                                    type="text"
                                    placeholder="For private repositories"
                                    class="w-full px-3 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                                    prop:value=username
                                    on:input=move |ev| set_username.set(event_target_value(&ev))
                                />
                            </div>

                            <div>
                                <label class="block text-sm font-medium text-gray-700 mb-2">
                                    "Password (Optional)"
                                </label>
                                <input
                                    type="password"
                                    placeholder="For private repositories"
                                    class="w-full px-3 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                                    prop:value=password
                                    on:input=move |ev| set_password.set(event_target_value(&ev))
                                />
                            </div>

                            <div>
                                <label class="flex items-center">
                                    <input
                                        type="checkbox"
                                        class="rounded border-gray-300"
                                        prop:checked=force_update
                                        on:change=move |ev| set_force_update.set(event_target_checked(&ev))
                                    />
                                    <span class="ml-2 text-sm text-gray-700">"Force update if repository exists"</span>
                                </label>
                            </div>
                        </div>

                        <div class="flex justify-end space-x-2 pt-4 mt-4 border-t">
                            <button
                                on:click=move |_| set_show_add_modal.set(false)
                                disabled=adding
                                class="px-4 py-2 bg-gray-300 text-gray-700 rounded-lg hover:bg-gray-400 disabled:opacity-50"
                            >
                                "Cancel"
                            </button>
                            <button
                                on:click=move |_| add_repository()
                                disabled=adding
                                class="px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 disabled:opacity-50"
                            >
                                {move || if adding.get() { "Adding..." } else { "Add Repository" }}
                            </button>
                        </div>
                    </div>
                </div>
            })}

            // Statistics and popular repositories
            <div class="grid grid-cols-1 lg:grid-cols-2 gap-6">
                // Repository statistics
                <div class="bg-white rounded-lg shadow p-4">
                    <h3 class="text-lg font-medium text-gray-900 mb-4">"Repository Statistics"</h3>
                    <div class="grid grid-cols-2 gap-4 text-center">
                        <div>
                            <div class="text-2xl font-bold text-gray-900">{move || filtered_repositories().len()}</div>
                            <div class="text-sm text-gray-500">"Total Repositories"</div>
                        </div>
                        <div>
                            <div class="text-2xl font-bold text-green-600">
                                {move || filtered_repositories().iter().filter(|r| r.status == "ready" || r.status == "active").count()}
                            </div>
                            <div class="text-sm text-gray-500">"Active"</div>
                        </div>
                    </div>
                </div>

                // Popular repositories to add
                <div class="bg-white rounded-lg shadow p-4">
                    <h3 class="text-lg font-medium text-gray-900 mb-4">"Popular Repositories"</h3>
                    <div class="space-y-2">
                        <div class="flex justify-between items-center p-2 bg-gray-50 rounded">
                            <div>
                                <div class="font-medium">"Bitnami"</div>
                                <div class="text-sm text-gray-500">"https://charts.bitnami.com/bitnami"</div>
                            </div>
                        </div>
                        <div class="flex justify-between items-center p-2 bg-gray-50 rounded">
                            <div>
                                <div class="font-medium">"Stable"</div>
                                <div class="text-sm text-gray-500">"https://charts.helm.sh/stable"</div>
                            </div>
                        </div>
                        <div class="flex justify-between items-center p-2 bg-gray-50 rounded">
                            <div>
                                <div class="font-medium">"Ingress NGINX"</div>
                                <div class="text-sm text-gray-500">"https://kubernetes.github.io/ingress-nginx"</div>
                            </div>
                        </div>
                    </div>
                </div>
            </div>
        </div>
    }
}