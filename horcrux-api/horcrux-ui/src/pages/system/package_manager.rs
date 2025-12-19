use leptos::*;
use crate::api::*;

#[component]
pub fn PackageManagerPage() -> impl IntoView {
    let (packages, set_packages) = create_signal(Vec::<PackageInfo>::new());
    let (loading, set_loading) = create_signal(false);
    let (error_message, set_error_message) = create_signal(None::<String>);
    let (success_message, set_success_message) = create_signal(None::<String>);
    let (package_filter, set_package_filter) = create_signal("all".to_string());
    let (search_query, set_search_query) = create_signal(String::new());
    let (show_updates_only, set_show_updates_only) = create_signal(false);
    let (selected_packages, set_selected_packages) = create_signal(std::collections::HashSet::<String>::new());
    let (package_manager_type, set_package_manager_type) = create_signal("auto".to_string());

    // Package operations
    let refresh_packages = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);

        match get_installed_packages().await {
            Ok(package_list) => set_packages.set(package_list),
            Err(e) => set_error_message.set(Some(format!("Failed to load packages: {}", e))),
        }

        set_loading.set(false);
    });

    let update_package_lists = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);

        match update_package_database().await {
            Ok(_) => {
                set_success_message.set(Some("Package database updated successfully".to_string()));
                refresh_packages.dispatch(());
            }
            Err(e) => set_error_message.set(Some(format!("Failed to update package database: {}", e))),
        }

        set_loading.set(false);
    });

    let install_package_action = create_action(move |package_name: &String| {
        let package_name = package_name.clone();
        async move {
            set_loading.set(true);
            set_error_message.set(None);

            match install_package(&package_name).await {
                Ok(_) => {
                    set_success_message.set(Some(format!("Package '{}' installed successfully", package_name)));
                    refresh_packages.dispatch(());
                }
                Err(e) => set_error_message.set(Some(format!("Failed to install package '{}': {}", package_name, e))),
            }

            set_loading.set(false);
        }
    });

    let remove_package_action = create_action(move |package_name: &String| {
        let package_name = package_name.clone();
        async move {
            set_loading.set(true);
            set_error_message.set(None);

            match remove_package(&package_name).await {
                Ok(_) => {
                    set_success_message.set(Some(format!("Package '{}' removed successfully", package_name)));
                    refresh_packages.dispatch(());
                }
                Err(e) => set_error_message.set(Some(format!("Failed to remove package '{}': {}", package_name, e))),
            }

            set_loading.set(false);
        }
    });

    let upgrade_package_action = create_action(move |package_name: &String| {
        let package_name = package_name.clone();
        async move {
            set_loading.set(true);
            set_error_message.set(None);

            match upgrade_package(&package_name).await {
                Ok(_) => {
                    set_success_message.set(Some(format!("Package '{}' upgraded successfully", package_name)));
                    refresh_packages.dispatch(());
                }
                Err(e) => set_error_message.set(Some(format!("Failed to upgrade package '{}': {}", package_name, e))),
            }

            set_loading.set(false);
        }
    });

    let upgrade_all_action = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);

        match upgrade_all_packages().await {
            Ok(upgraded_count) => {
                set_success_message.set(Some(format!("Successfully upgraded {} packages", upgraded_count)));
                refresh_packages.dispatch(());
            }
            Err(e) => set_error_message.set(Some(format!("Failed to upgrade packages: {}", e))),
        }

        set_loading.set(false);
    });

    let bulk_install_action = create_action(move |_: &()| async move {
        let packages_to_install: Vec<String> = selected_packages.get().into_iter().collect();
        if packages_to_install.is_empty() {
            return;
        }

        set_loading.set(true);
        set_error_message.set(None);

        match bulk_install_packages(packages_to_install.clone()).await {
            Ok(_) => {
                set_success_message.set(Some(format!("Successfully installed {} packages", packages_to_install.len())));
                set_selected_packages.set(std::collections::HashSet::new());
                refresh_packages.dispatch(());
            }
            Err(e) => set_error_message.set(Some(format!("Failed to install packages: {}", e))),
        }

        set_loading.set(false);
    });

    // Filter packages based on criteria
    let filtered_packages = move || {
        let query = search_query.get().to_lowercase();
        let filter = package_filter.get();
        let updates_only = show_updates_only.get();

        packages.get()
            .into_iter()
            .filter(|package| {
                let matches_search = query.is_empty() ||
                    package.name.to_lowercase().contains(&query) ||
                    package.description.to_lowercase().contains(&query);

                let matches_filter = match filter.as_str() {
                    "installed" => package.status == "installed",
                    "upgradeable" => package.status == "upgradeable",
                    "not-installed" => package.status == "not-installed",
                    _ => true,
                };

                let matches_updates = !updates_only || package.status == "upgradeable";

                matches_search && matches_filter && matches_updates
            })
            .collect::<Vec<_>>()
    };

    // Helper functions
    let get_status_color = |status: &str| match status {
        "installed" => "text-green-600 bg-green-50",
        "upgradeable" => "text-blue-600 bg-blue-50",
        "not-installed" => "text-gray-600 bg-gray-50",
        _ => "text-yellow-600 bg-yellow-50",
    };

    let get_status_icon = |status: &str| match status {
        "installed" => "[OK]",
        "upgradeable" => "[UP]",
        "not-installed" => "[ ]",
        _ => "[?]",
    };

    let format_size = |size_bytes: Option<u64>| -> String {
        match size_bytes {
            Some(size) => {
                if size >= 1024 * 1024 * 1024 {
                    format!("{:.1} GB", size as f64 / (1024.0 * 1024.0 * 1024.0))
                } else if size >= 1024 * 1024 {
                    format!("{:.1} MB", size as f64 / (1024.0 * 1024.0))
                } else if size >= 1024 {
                    format!("{:.1} KB", size as f64 / 1024.0)
                } else {
                    format!("{} B", size)
                }
            }
            None => "-".to_string(),
        }
    };

    let toggle_package_selection = move |package_name: String| {
        set_selected_packages.update(|selected| {
            if selected.contains(&package_name) {
                selected.remove(&package_name);
            } else {
                selected.insert(package_name);
            }
        });
    };

    // Clear messages after delay
    let clear_messages = move || {
        set_timeout(
            move || {
                set_success_message.set(None);
                set_error_message.set(None);
            },
            std::time::Duration::from_secs(5),
        );
    };

    // Initial load
    create_effect(move |_| {
        refresh_packages.dispatch(());
    });

    view! {
        <div class="package-manager-page">
            <div class="page-header">
                <h1>"Package Manager"</h1>
                <div class="header-actions">
                    <button
                        class="btn btn-secondary"
                        on:click=move |_| refresh_packages.dispatch(())
                        disabled=loading
                    >
                        "Refresh"
                    </button>
                    <button
                        class="btn btn-primary"
                        on:click=move |_| update_package_lists.dispatch(())
                        disabled=loading
                    >
                        "Update Lists"
                    </button>
                    <button
                        class="btn btn-warning"
                        on:click=move |_| {
                            if web_sys::window()
                                .unwrap()
                                .confirm_with_message("Upgrade all packages? This may take some time.")
                                .unwrap_or(false)
                            {
                                upgrade_all_action.dispatch(());
                            }
                        }
                        disabled=loading
                    >
                        "Upgrade All"
                    </button>
                </div>
            </div>

            {move || error_message.get().map(|msg| {
                clear_messages();
                view! {
                    <div class="alert alert-error">{msg}</div>
                }
            })}

            {move || success_message.get().map(|msg| {
                clear_messages();
                view! {
                    <div class="alert alert-success">{msg}</div>
                }
            })}

            <div class="package-controls">
                <div class="controls-row">
                    <div class="search-box">
                        <input
                            type="text"
                            prop:value=search_query
                            on:input=move |ev| set_search_query.set(event_target_value(&ev))
                            placeholder="Search packages..."
                            class="search-input"
                        />
                    </div>

                    <div class="filter-controls">
                        <label>"Package Manager:"</label>
                        <select
                            prop:value=package_manager_type
                            on:change=move |ev| {
                                set_package_manager_type.set(event_target_value(&ev));
                                refresh_packages.dispatch(());
                            }
                        >
                            <option value="auto">"Auto-detect"</option>
                            <option value="apt">"APT (Debian/Ubuntu)"</option>
                            <option value="yum">"YUM (RHEL/CentOS)"</option>
                            <option value="dnf">"DNF (Fedora)"</option>
                            <option value="pacman">"Pacman (Arch)"</option>
                            <option value="portage">"Portage (Gentoo)"</option>
                            <option value="zypper">"Zypper (openSUSE)"</option>
                        </select>

                        <label>"Status Filter:"</label>
                        <select
                            prop:value=package_filter
                            on:change=move |ev| set_package_filter.set(event_target_value(&ev))
                        >
                            <option value="all">"All Packages"</option>
                            <option value="installed">"Installed"</option>
                            <option value="upgradeable">"Upgradeable"</option>
                            <option value="not-installed">"Available"</option>
                        </select>
                    </div>
                </div>

                <div class="controls-row">
                    <div class="checkbox-controls">
                        <label class="checkbox-label">
                            <input
                                type="checkbox"
                                prop:checked=show_updates_only
                                on:input=move |ev| set_show_updates_only.set(event_target_checked(&ev))
                            />
                            " Show updates only"
                        </label>
                    </div>

                    {move || if !selected_packages.get().is_empty() {
                        view! {
                            <div class="bulk-actions">
                                <span class="selected-count">
                                    {selected_packages.get().len()}" packages selected"
                                </span>
                                <button
                                    class="btn btn-xs btn-primary"
                                    on:click=move |_| bulk_install_action.dispatch(())
                                    disabled=loading
                                >
                                    "Install Selected"
                                </button>
                                <button
                                    class="btn btn-xs btn-secondary"
                                    on:click=move |_| set_selected_packages.set(std::collections::HashSet::new())
                                >
                                    "Clear Selection"
                                </button>
                            </div>
                        }.into_view()
                    } else {
                        view! { <div></div> }.into_view()
                    }}
                </div>
            </div>

            {move || if loading.get() && packages.get().is_empty() {
                view! { <div class="loading">"Loading packages..."</div> }.into_view()
            } else {
                let filtered = filtered_packages();
                if filtered.is_empty() {
                    view! { <div class="empty-state">"No packages found matching the current filters"</div> }.into_view()
                } else {
                    view! {
                        <div class="packages-table-container">
                            <table class="packages-table">
                                <thead>
                                    <tr>
                                        <th class="select-col">
                                            <input
                                                type="checkbox"
                                                on:change=move |ev| {
                                                    if event_target_checked(&ev) {
                                                        let all_package_names: std::collections::HashSet<String> =
                                                            filtered_packages().iter().map(|p| p.name.clone()).collect();
                                                        set_selected_packages.set(all_package_names);
                                                    } else {
                                                        set_selected_packages.set(std::collections::HashSet::new());
                                                    }
                                                }
                                            />
                                        </th>
                                        <th>"Package"</th>
                                        <th>"Status"</th>
                                        <th>"Installed Version"</th>
                                        <th>"Available Version"</th>
                                        <th>"Size"</th>
                                        <th>"Description"</th>
                                        <th>"Actions"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {filtered_packages().into_iter().map(|package| {
                                        let package_clone1 = package.clone();
                                        let package_clone2 = package.clone();
                                        let package_clone3 = package.clone();
                                        let package_clone4 = package.clone();
                                        let package_name = package.name.clone();
                                        let package_name_check = package.name.clone();
                                        let status_class = get_status_color(&package.status);
                                        let status_icon = get_status_icon(&package.status);
                                        let status = package.status.clone();
                                        let installed_version = package.installed_version.clone().unwrap_or_else(|| "-".to_string());
                                        let available_version = package.available_version.clone().unwrap_or_else(|| "-".to_string());
                                        let size = format_size(package.size);
                                        let description = package.description.clone();

                                        view! {
                                            <tr class="package-row">
                                                <td class="package-select">
                                                    <input
                                                        type="checkbox"
                                                        prop:checked=move || selected_packages.get().contains(&package_name_check)
                                                        on:change=move |_| toggle_package_selection(package_clone4.name.clone())
                                                    />
                                                </td>
                                                <td class="package-name">
                                                    <strong>{package_name}</strong>
                                                </td>
                                                <td class="package-status">
                                                    <span class={format!("status-badge {}", status_class)}>
                                                        {status_icon}" "{status.clone()}
                                                    </span>
                                                </td>
                                                <td class="package-version-installed">
                                                    {installed_version}
                                                </td>
                                                <td class="package-version-available">
                                                    {available_version}
                                                </td>
                                                <td class="package-size">
                                                    {size}
                                                </td>
                                                <td class="package-description">
                                                    <span class="description-text">{description}</span>
                                                </td>
                                                <td class="package-actions">
                                                    <div class="action-buttons">
                                                        {match status.as_str() {
                                                            "not-installed" => view! {
                                                                <button
                                                                    class="btn btn-xs btn-primary"
                                                                    disabled=loading
                                                                    on:click=move |_| {
                                                                        install_package_action.dispatch(package_clone1.name.clone());
                                                                    }
                                                                >
                                                                    "Install"
                                                                </button>
                                                            }.into_view(),
                                                            "installed" => view! {
                                                                <button
                                                                    class="btn btn-xs btn-danger"
                                                                    disabled=loading
                                                                    on:click=move |_| {
                                                                        if web_sys::window()
                                                                            .unwrap()
                                                                            .confirm_with_message(&format!("Remove package '{}'?", package_clone2.name))
                                                                            .unwrap_or(false)
                                                                        {
                                                                            remove_package_action.dispatch(package_clone2.name.clone());
                                                                        }
                                                                    }
                                                                >
                                                                    "Remove"
                                                                </button>
                                                            }.into_view(),
                                                            "upgradeable" => view! {
                                                                <>
                                                                    <button
                                                                        class="btn btn-xs btn-success"
                                                                        disabled=loading
                                                                        on:click=move |_| {
                                                                            upgrade_package_action.dispatch(package_clone3.name.clone());
                                                                        }
                                                                    >
                                                                        "Upgrade"
                                                                    </button>
                                                                    <button
                                                                        class="btn btn-xs btn-danger"
                                                                        disabled=loading
                                                                        on:click=move |_| {
                                                                            if web_sys::window()
                                                                                .unwrap()
                                                                                .confirm_with_message(&format!("Remove package '{}'?", package_clone1.name))
                                                                                .unwrap_or(false)
                                                                            {
                                                                                remove_package_action.dispatch(package_clone1.name.clone());
                                                                            }
                                                                        }
                                                                    >
                                                                        "Remove"
                                                                    </button>
                                                                </>
                                                            }.into_view(),
                                                            _ => view! {
                                                                <span class="text-gray-500">"No actions"</span>
                                                            }.into_view(),
                                                        }}
                                                    </div>
                                                </td>
                                            </tr>
                                        }
                                    }).collect::<Vec<_>>()}
                                </tbody>
                            </table>
                        </div>

                        <div class="packages-summary">
                            <div class="summary-stats">
                                {
                                    let total = packages.get().len();
                                    let installed = packages.get().iter().filter(|p| p.status == "installed").count();
                                    let upgradeable = packages.get().iter().filter(|p| p.status == "upgradeable").count();
                                    let available = packages.get().iter().filter(|p| p.status == "not-installed").count();
                                    let filtered_count = filtered.len();

                                    view! {
                                        <>
                                            <div class="stat-item">
                                                <span class="stat-label">"Total Packages:"</span>
                                                <span class="stat-value">{total}</span>
                                            </div>
                                            <div class="stat-item">
                                                <span class="stat-label">"Installed:"</span>
                                                <span class="stat-value text-green-600">{installed}</span>
                                            </div>
                                            <div class="stat-item">
                                                <span class="stat-label">"Upgradeable:"</span>
                                                <span class="stat-value text-blue-600">{upgradeable}</span>
                                            </div>
                                            <div class="stat-item">
                                                <span class="stat-label">"Available:"</span>
                                                <span class="stat-value text-gray-600">{available}</span>
                                            </div>
                                            <div class="stat-item">
                                                <span class="stat-label">"Showing:"</span>
                                                <span class="stat-value text-purple-600">{filtered_count}</span>
                                            </div>
                                        </>
                                    }
                                }
                            </div>
                        </div>
                    }.into_view()
                }
            }}
        </div>
    }
}