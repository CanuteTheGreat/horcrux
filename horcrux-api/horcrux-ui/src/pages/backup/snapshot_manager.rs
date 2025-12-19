use leptos::*;
use crate::api::{
    VmSnapshot, CreateSnapshotRequest, RestoreSnapshotRequest, SnapshotTreeNode,
    SnapshotSchedule, CreateSnapshotScheduleRequest, SnapshotQuota, CreateSnapshotQuotaRequest, QuotaSummary, RetentionPolicy, QuotaType, CleanupPolicy,
    get_vm_snapshots, create_vm_snapshot, delete_vm_snapshot, restore_vm_snapshot, get_vm_snapshot_tree,
    get_snapshot_schedules, create_snapshot_schedule, delete_snapshot_schedule,
    get_snapshot_quotas, create_snapshot_quota, delete_snapshot_quota,
    get_snapshot_quota_summary,
    get_vms
};
use horcrux_common::VmConfig;

fn format_bytes_static(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

fn render_snapshot_tree_node(node: &SnapshotTreeNode, depth: usize) -> impl IntoView {
    let indent = "  ".repeat(depth);
    let snapshot = &node.snapshot;
    let children = node.children.clone();

    let snapshot_name = snapshot.name.clone();
    let snapshot_size = format_bytes_static(snapshot.size_mb * 1024 * 1024);
    let snapshot_created = snapshot.created_at.clone();

    view! {
        <div class="font-mono text-sm">
            <div class="flex items-center py-1">
                <span class="text-gray-500">{indent}</span>
                <i class="fas fa-camera text-blue-500 mr-2"></i>
                <span class="font-medium">{snapshot_name}</span>
                <span class="text-xs text-gray-500 ml-2">
                    "(" {snapshot_size} ")"
                </span>
                <span class="text-xs text-gray-400 ml-2">{snapshot_created}</span>
            </div>
            {children.into_iter().map(|child| {
                render_snapshot_tree_node(&child, depth + 1)
            }).collect_view()}
        </div>
    }
}

#[component]
pub fn SnapshotManagerPage() -> impl IntoView {
    let (vms, set_vms) = create_signal(Vec::<VmConfig>::new());
    let (selected_vm_id, set_selected_vm_id) = create_signal(None::<String>);
    let (snapshots, set_snapshots) = create_signal(Vec::<VmSnapshot>::new());
    let (snapshot_tree, set_snapshot_tree) = create_signal(Vec::<SnapshotTreeNode>::new());
    let (schedules, set_schedules) = create_signal(Vec::<SnapshotSchedule>::new());
    let (quotas, set_quotas) = create_signal(Vec::<SnapshotQuota>::new());
    let (quota_summary, set_quota_summary) = create_signal(None::<QuotaSummary>);
    let (loading, set_loading) = create_signal(false);
    let (error, set_error) = create_signal(None::<String>);
    let (active_tab, set_active_tab) = create_signal("snapshots".to_string());

    // Modal states
    let (show_create_snapshot_modal, set_show_create_snapshot_modal) = create_signal(false);
    let (show_create_schedule_modal, set_show_create_schedule_modal) = create_signal(false);
    let (show_create_quota_modal, set_show_create_quota_modal) = create_signal(false);
    let (show_tree_view, set_show_tree_view) = create_signal(false);

    // Form states
    let (snapshot_name, set_snapshot_name) = create_signal(String::new());
    let (snapshot_description, set_snapshot_description) = create_signal(String::new());
    let (include_memory, set_include_memory) = create_signal(true);

    // Schedule form state
    let (schedule_name, set_schedule_name) = create_signal(String::new());
    let (schedule_description, set_schedule_description) = create_signal(String::new());
    let (schedule_cron, set_schedule_cron) = create_signal("0 2 * * *".to_string());
    let (schedule_enabled, set_schedule_enabled) = create_signal(true);
    let (schedule_include_memory, set_schedule_include_memory) = create_signal(false);

    // Quota form state
    let (quota_name, set_quota_name) = create_signal(String::new());
    let (quota_description, set_quota_description) = create_signal(String::new());
    let (quota_type, set_quota_type) = create_signal(QuotaType::MaxCount);
    let (quota_limit, set_quota_limit) = create_signal(10u64);
    let (quota_storage_path, set_quota_storage_path) = create_signal("/var/lib/libvirt/images".to_string());
    let (quota_cleanup_policy, set_quota_cleanup_policy) = create_signal(CleanupPolicy::OldestFirst);
    let (quota_enabled, set_quota_enabled) = create_signal(true);

    let load_vms = move || {
        spawn_local(async move {
            match get_vms().await {
                Ok(vm_list) => set_vms.set(vm_list),
                Err(e) => set_error.set(Some(format!("Failed to load VMs: {}", e))),
            }
        });
    };

    let load_snapshots = move || {
        if let Some(vm_id) = selected_vm_id.get() {
            set_loading.set(true);
            spawn_local(async move {
                match get_vm_snapshots(&vm_id).await {
                    Ok(snapshot_list) => set_snapshots.set(snapshot_list),
                    Err(e) => set_error.set(Some(format!("Failed to load snapshots: {}", e))),
                }
                set_loading.set(false);
            });
        }
    };

    let load_snapshot_tree = move || {
        if let Some(vm_id) = selected_vm_id.get() {
            spawn_local(async move {
                match get_vm_snapshot_tree(&vm_id).await {
                    Ok(tree) => set_snapshot_tree.set(tree),
                    Err(e) => set_error.set(Some(format!("Failed to load snapshot tree: {}", e))),
                }
            });
        }
    };

    let load_schedules = move || {
        spawn_local(async move {
            match get_snapshot_schedules().await {
                Ok(schedule_list) => set_schedules.set(schedule_list),
                Err(e) => set_error.set(Some(format!("Failed to load schedules: {}", e))),
            }
        });
    };

    let load_quotas = move || {
        spawn_local(async move {
            match get_snapshot_quotas().await {
                Ok(quota_list) => set_quotas.set(quota_list),
                Err(e) => set_error.set(Some(format!("Failed to load quotas: {}", e))),
            }

            match get_snapshot_quota_summary().await {
                Ok(summary) => set_quota_summary.set(Some(summary)),
                Err(e) => set_error.set(Some(format!("Failed to load quota summary: {}", e))),
            }
        });
    };

    let load_all_data = move || {
        load_vms();
        load_schedules();
        load_quotas();
        if selected_vm_id.get().is_some() {
            load_snapshots();
        }
    };

    // Load data on mount
    create_effect(move |_| {
        load_all_data();
    });

    // Load snapshots when VM selection changes
    create_effect(move |_| {
        let _ = selected_vm_id.get();
        load_snapshots();
        if show_tree_view.get() {
            load_snapshot_tree();
        }
    });

    let create_snapshot = move || {
        if let Some(vm_id) = selected_vm_id.get() {
            let request = CreateSnapshotRequest {
                name: snapshot_name.get(),
                description: if snapshot_description.get().is_empty() {
                    None
                } else {
                    Some(snapshot_description.get())
                },
                include_memory: include_memory.get(),
            };

            spawn_local(async move {
                match create_vm_snapshot(&vm_id, request).await {
                    Ok(_) => {
                        set_show_create_snapshot_modal.set(false);
                        set_snapshot_name.set(String::new());
                        set_snapshot_description.set(String::new());
                        set_include_memory.set(true);
                        load_snapshots();
                    }
                    Err(e) => set_error.set(Some(format!("Failed to create snapshot: {}", e))),
                }
            });
        }
    };

    let delete_snapshot = move |snapshot_id: String| {
        if let Some(vm_id) = selected_vm_id.get() {
            if web_sys::window()
                .unwrap()
                .confirm_with_message(&format!("Are you sure you want to delete snapshot '{}'?", snapshot_id))
                .unwrap()
            {
                spawn_local(async move {
                    match delete_vm_snapshot(&vm_id, &snapshot_id).await {
                        Ok(_) => load_snapshots(),
                        Err(e) => set_error.set(Some(format!("Failed to delete snapshot: {}", e))),
                    }
                });
            }
        }
    };

    let restore_snapshot = move |snapshot_id: String| {
        if let Some(vm_id) = selected_vm_id.get() {
            if web_sys::window()
                .unwrap()
                .confirm_with_message(&format!("Are you sure you want to restore to snapshot '{}'? This will replace the current VM state.", snapshot_id))
                .unwrap()
            {
                let request = RestoreSnapshotRequest {
                    restore_memory: Some(true),
                };

                spawn_local(async move {
                    match restore_vm_snapshot(&vm_id, &snapshot_id, request).await {
                        Ok(_) => load_snapshots(),
                        Err(e) => set_error.set(Some(format!("Failed to restore snapshot: {}", e))),
                    }
                });
            }
        }
    };

    let create_schedule = move || {
        if let Some(vm_id) = selected_vm_id.get() {
            let retention = RetentionPolicy {
                keep_hourly: None,
                keep_daily: Some(7),
                keep_weekly: Some(4),
                keep_monthly: Some(12),
                keep_yearly: None,
                max_age_days: Some(90),
            };

            let request = CreateSnapshotScheduleRequest {
                vm_id,
                name: schedule_name.get(),
                description: if schedule_description.get().is_empty() {
                    None
                } else {
                    Some(schedule_description.get())
                },
                schedule: schedule_cron.get(),
                retention_policy: retention,
                include_memory: schedule_include_memory.get(),
                enabled: schedule_enabled.get(),
            };

            spawn_local(async move {
                match create_snapshot_schedule(request).await {
                    Ok(_) => {
                        set_show_create_schedule_modal.set(false);
                        set_schedule_name.set(String::new());
                        set_schedule_description.set(String::new());
                        set_schedule_cron.set("0 2 * * *".to_string());
                        set_schedule_enabled.set(true);
                        set_schedule_include_memory.set(false);
                        load_schedules();
                    }
                    Err(e) => set_error.set(Some(format!("Failed to create schedule: {}", e))),
                }
            });
        }
    };

    let delete_schedule = move |schedule_id: String| {
        if web_sys::window()
            .unwrap()
            .confirm_with_message("Are you sure you want to delete this snapshot schedule?")
            .unwrap()
        {
            spawn_local(async move {
                match delete_snapshot_schedule(&schedule_id).await {
                    Ok(_) => load_schedules(),
                    Err(e) => set_error.set(Some(format!("Failed to delete schedule: {}", e))),
                }
            });
        }
    };

    let create_quota = move || {
        let request = CreateSnapshotQuotaRequest {
            name: quota_name.get(),
            description: if quota_description.get().is_empty() {
                None
            } else {
                Some(quota_description.get())
            },
            quota_type: quota_type.get(),
            limit_value: quota_limit.get(),
            storage_path: quota_storage_path.get(),
            cleanup_policy: quota_cleanup_policy.get(),
            enabled: quota_enabled.get(),
        };

        spawn_local(async move {
            match create_snapshot_quota(request).await {
                Ok(_) => {
                    set_show_create_quota_modal.set(false);
                    set_quota_name.set(String::new());
                    set_quota_description.set(String::new());
                    set_quota_type.set(QuotaType::MaxCount);
                    set_quota_limit.set(10);
                    set_quota_storage_path.set("/var/lib/libvirt/images".to_string());
                    set_quota_cleanup_policy.set(CleanupPolicy::OldestFirst);
                    set_quota_enabled.set(true);
                    load_quotas();
                }
                Err(e) => set_error.set(Some(format!("Failed to create quota: {}", e))),
            }
        });
    };

    let delete_quota = move |quota_id: String| {
        if web_sys::window()
            .unwrap()
            .confirm_with_message("Are you sure you want to delete this snapshot quota?")
            .unwrap()
        {
            spawn_local(async move {
                match delete_snapshot_quota(&quota_id).await {
                    Ok(_) => load_quotas(),
                    Err(e) => set_error.set(Some(format!("Failed to delete quota: {}", e))),
                }
            });
        }
    };

    let format_bytes = move |bytes: u64| {
        if bytes < 1024 {
            format!("{} B", bytes)
        } else if bytes < 1024 * 1024 {
            format!("{:.1} KB", bytes as f64 / 1024.0)
        } else if bytes < 1024 * 1024 * 1024 {
            format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
        } else {
            format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
        }
    };


    view! {
        <div class="p-6">
            <div class="flex justify-between items-center mb-6">
                <h1 class="text-2xl font-bold">Snapshot Management</h1>
                <div class="flex space-x-3">
                    <button
                        on:click=move |_| load_all_data()
                        class="bg-gray-500 hover:bg-gray-600 text-white px-4 py-2 rounded-lg"
                    >
                        <i class="fas fa-sync mr-2"></i>
                        "Refresh"
                    </button>
                </div>
            </div>

            // Error display
            {move || error.get().map(|e| view! {
                <div class="bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded mb-6">
                    {e}
                </div>
            })}

            // VM Selection
            <div class="bg-white rounded-lg shadow p-4 mb-6">
                <label class="block text-sm font-medium text-gray-700 mb-2">
                    "Select Virtual Machine"
                </label>
                <select
                    on:change=move |ev| {
                        let value = event_target_value(&ev);
                        if value.is_empty() {
                            set_selected_vm_id.set(None);
                        } else {
                            set_selected_vm_id.set(Some(value));
                        }
                    }
                    class="w-full md:w-1/3 px-3 py-2 border border-gray-300 rounded-lg"
                >
                    <option value="">"Select a VM..."</option>
                    {move || vms.get().into_iter().map(|vm| {
                        let vm_id = vm.id.clone();
                        let vm_display = format!("{} ({})", vm.name, vm.id);
                        view! {
                            <option value={vm_id}>{vm_display}</option>
                        }
                    }).collect::<Vec<_>>()}
                </select>
            </div>

            // Tab Navigation
            <div class="bg-white rounded-lg shadow mb-6">
                <div class="border-b border-gray-200">
                    <nav class="-mb-px flex">
                        <button
                            on:click=move |_| set_active_tab.set("snapshots".to_string())
                            class=format!("py-2 px-4 border-b-2 font-medium text-sm {}",
                                if active_tab.get() == "snapshots" {
                                    "border-blue-500 text-blue-600"
                                } else {
                                    "border-transparent text-gray-500 hover:text-gray-700 hover:border-gray-300"
                                })
                        >
                            "Snapshots"
                        </button>
                        <button
                            on:click=move |_| set_active_tab.set("schedules".to_string())
                            class=format!("py-2 px-4 border-b-2 font-medium text-sm {}",
                                if active_tab.get() == "schedules" {
                                    "border-blue-500 text-blue-600"
                                } else {
                                    "border-transparent text-gray-500 hover:text-gray-700 hover:border-gray-300"
                                })
                        >
                            "Schedules"
                        </button>
                        <button
                            on:click=move |_| set_active_tab.set("quotas".to_string())
                            class=format!("py-2 px-4 border-b-2 font-medium text-sm {}",
                                if active_tab.get() == "quotas" {
                                    "border-blue-500 text-blue-600"
                                } else {
                                    "border-transparent text-gray-500 hover:text-gray-700 hover:border-gray-300"
                                })
                        >
                            "Quotas"
                        </button>
                    </nav>
                </div>

                <div class="p-6">
                    {move || match active_tab.get().as_str() {
                        "snapshots" => view! {
                            <div>
                                <div class="flex justify-between items-center mb-4">
                                    <h2 class="text-lg font-semibold">VM Snapshots</h2>
                                    <div class="flex space-x-2">
                                        {move || if selected_vm_id.get().is_some() {
                                            view! {
                                                <div class="flex space-x-2">
                                                    <button
                                                        on:click=move |_| {
                                                            set_show_tree_view.set(!show_tree_view.get());
                                                            if show_tree_view.get() {
                                                                load_snapshot_tree();
                                                            }
                                                        }
                                                        class=format!("px-4 py-2 rounded-lg {}",
                                                            if show_tree_view.get() {
                                                                "bg-blue-500 text-white"
                                                            } else {
                                                                "bg-gray-200 text-gray-700 hover:bg-gray-300"
                                                            })
                                                    >
                                                        <i class="fas fa-sitemap mr-2"></i>
                                                        "Tree View"
                                                    </button>
                                                    <button
                                                        on:click=move |_| set_show_create_snapshot_modal.set(true)
                                                        class="bg-green-500 hover:bg-green-600 text-white px-4 py-2 rounded-lg"
                                                    >
                                                        <i class="fas fa-camera mr-2"></i>
                                                        "Create Snapshot"
                                                    </button>
                                                </div>
                                            }
                                        } else {
                                            view! {
                                                <div class="text-gray-500">
                                                    "Select a VM to manage snapshots"
                                                </div>
                                            }
                                        }}
                                    </div>
                                </div>

                                {move || if show_tree_view.get() {
                                    view! {
                                        <div class="bg-gray-50 p-4 rounded-lg">
                                            <h3 class="font-medium mb-3">"Snapshot Hierarchy"</h3>
                                            {move || if snapshot_tree.get().is_empty() {
                                                view! {
                                                    <div class="text-gray-500 text-center py-4">
                                                        "No snapshots found"
                                                    </div>
                                                }
                                            } else {
                                                view! {
                                                    <div>
                                                        {snapshot_tree.get().into_iter().map(|node| {
                                                            render_snapshot_tree_node(&node, 0)
                                                        }).collect::<Vec<_>>()}
                                                    </div>
                                                }
                                            }}
                                        </div>
                                    }
                                } else {
                                    view! {
                                        <div>
                                            {move || if loading.get() {
                                                view! {
                                                    <div class="text-center py-8">
                                                        <i class="fas fa-spinner fa-spin text-2xl text-gray-400 mb-2"></i>
                                                        <p class="text-gray-600">"Loading snapshots..."</p>
                                                    </div>
                                                }
                                            } else if selected_vm_id.get().is_none() {
                                                view! {
                                                    <div class="text-center py-8 text-gray-500">
                                                        <i class="fas fa-hdd text-4xl mb-4"></i>
                                                        <p class="text-lg">"No VM Selected"</p>
                                                        <p class="text-sm">"Please select a virtual machine to view its snapshots"</p>
                                                    </div>
                                                }
                                            } else if snapshots.get().is_empty() {
                                                view! {
                                                    <div class="text-center py-8 text-gray-500">
                                                        <i class="fas fa-camera text-4xl mb-4"></i>
                                                        <p class="text-lg">"No Snapshots Found"</p>
                                                        <p class="text-sm mb-4">"Create your first snapshot to preserve the VM state"</p>
                                                        <button
                                                            on:click=move |_| set_show_create_snapshot_modal.set(true)
                                                            class="bg-green-500 hover:bg-green-600 text-white px-4 py-2 rounded-lg"
                                                        >
                                                            "Create First Snapshot"
                                                        </button>
                                                    </div>
                                                }
                                            } else {
                                                view! {
                                                    <div class="overflow-x-auto">
                                                        <table class="min-w-full divide-y divide-gray-200">
                                                            <thead class="bg-gray-50">
                                                                <tr>
                                                                    <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                                                        "Name"
                                                                    </th>
                                                                    <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                                                        "Size"
                                                                    </th>
                                                                    <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                                                        "Memory"
                                                                    </th>
                                                                    <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                                                        "Created"
                                                                    </th>
                                                                    <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                                                        "Actions"
                                                                    </th>
                                                                </tr>
                                                            </thead>
                                                            <tbody class="bg-white divide-y divide-gray-200">
                                                                {snapshots.get().into_iter().map(|snapshot| {
                                                                    let snapshot_id = snapshot.id.clone();
                                                                    let snapshot_id_delete = snapshot.id.clone();
                                                                    let snapshot_name = snapshot.name.clone();
                                                                    let snapshot_desc = snapshot.description.clone();
                                                                    let snapshot_size = format_bytes(snapshot.size_mb * 1024 * 1024);
                                                                    let memory_included = snapshot.memory_included;
                                                                    let created_at = snapshot.created_at.clone();
                                                                    view! {
                                                                        <tr>
                                                                            <td class="px-6 py-4 whitespace-nowrap">
                                                                                <div class="text-sm font-medium text-gray-900">{snapshot_name}</div>
                                                                                {snapshot_desc.map(|desc| view! {
                                                                                    <div class="text-xs text-gray-500">{desc}</div>
                                                                                })}
                                                                            </td>
                                                                            <td class="px-6 py-4 whitespace-nowrap">
                                                                                <div class="text-sm text-gray-900">{snapshot_size}</div>
                                                                            </td>
                                                                            <td class="px-6 py-4 whitespace-nowrap">
                                                                                <span class=format!("px-2 py-1 text-xs font-medium rounded {}",
                                                                                    if memory_included {
                                                                                        "bg-green-100 text-green-800"
                                                                                    } else {
                                                                                        "bg-gray-100 text-gray-800"
                                                                                    })>
                                                                                    {if memory_included { "Yes" } else { "No" }}
                                                                                </span>
                                                                            </td>
                                                                            <td class="px-6 py-4 whitespace-nowrap">
                                                                                <div class="text-sm text-gray-900">{created_at}</div>
                                                                            </td>
                                                                            <td class="px-6 py-4 whitespace-nowrap text-sm font-medium">
                                                                                <div class="flex space-x-2">
                                                                                    <button
                                                                                        on:click=move |_| restore_snapshot(snapshot_id.clone())
                                                                                        class="text-blue-600 hover:text-blue-900 px-2 py-1 rounded hover:bg-blue-50"
                                                                                        title="Restore Snapshot"
                                                                                    >
                                                                                        <i class="fas fa-undo"></i>
                                                                                    </button>
                                                                                    <button
                                                                                        on:click=move |_| delete_snapshot(snapshot_id_delete.clone())
                                                                                        class="text-red-600 hover:text-red-900 px-2 py-1 rounded hover:bg-red-50"
                                                                                        title="Delete Snapshot"
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
                                                }
                                            }}
                                        </div>
                                    }
                                }}
                            </div>
                        },
                        "schedules" => view! {
                            <div>
                                <div class="flex justify-between items-center mb-4">
                                    <h2 class="text-lg font-semibold">Snapshot Schedules</h2>
                                    <button
                                        on:click=move |_| set_show_create_schedule_modal.set(true)
                                        class="bg-blue-500 hover:bg-blue-600 text-white px-4 py-2 rounded-lg"
                                        disabled=move || selected_vm_id.get().is_none()
                                    >
                                        <i class="fas fa-clock mr-2"></i>
                                        "Create Schedule"
                                    </button>
                                </div>

                                {move || if schedules.get().is_empty() {
                                    view! {
                                        <div class="text-center py-8 text-gray-500">
                                            <i class="fas fa-calendar-alt text-4xl mb-4"></i>
                                            <p class="text-lg">"No Snapshot Schedules"</p>
                                            <p class="text-sm">"Create automated snapshot schedules for your VMs"</p>
                                        </div>
                                    }
                                } else {
                                    view! {
                                        <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
                                            {schedules.get().into_iter().map(|schedule| {
                                                let schedule_id = schedule.id.clone();
                                                let schedule_name = schedule.name.clone();
                                                let schedule_enabled = schedule.enabled;
                                                let schedule_desc = schedule.description.clone();
                                                let vm_id = schedule.vm_id.clone();
                                                let schedule_cron = schedule.schedule.clone();
                                                let next_run = schedule.next_run.clone();
                                                let include_memory = schedule.include_memory;
                                                view! {
                                                    <div class="border border-gray-200 rounded-lg p-4">
                                                        <div class="flex items-center justify-between mb-2">
                                                            <h3 class="font-medium text-gray-900">{schedule_name}</h3>
                                                            <span class=format!("px-2 py-1 text-xs rounded {}",
                                                                if schedule_enabled {
                                                                    "bg-green-100 text-green-800"
                                                                } else {
                                                                    "bg-gray-100 text-gray-800"
                                                                })>
                                                                {if schedule_enabled { "Active" } else { "Disabled" }}
                                                            </span>
                                                        </div>
                                                        {schedule_desc.map(|desc| view! {
                                                            <p class="text-sm text-gray-600 mb-2">{desc}</p>
                                                        })}
                                                        <div class="text-xs text-gray-500 space-y-1">
                                                            <div>
                                                                <i class="fas fa-server mr-1"></i>
                                                                "VM: " {vm_id}
                                                            </div>
                                                            <div>
                                                                <i class="fas fa-clock mr-1"></i>
                                                                "Schedule: " {schedule_cron}
                                                            </div>
                                                            {next_run.map(|next| view! {
                                                                <div>
                                                                    <i class="fas fa-arrow-right mr-1"></i>
                                                                    "Next: " {next}
                                                                </div>
                                                            })}
                                                            <div>
                                                                <i class="fas fa-memory mr-1"></i>
                                                                "Memory: " {if include_memory { "Yes" } else { "No" }}
                                                            </div>
                                                        </div>
                                                        <div class="mt-3 flex justify-end">
                                                            <button
                                                                on:click=move |_| delete_schedule(schedule_id.clone())
                                                                class="text-red-600 hover:text-red-900 text-sm"
                                                            >
                                                                "Delete"
                                                            </button>
                                                        </div>
                                                    </div>
                                                }
                                            }).collect::<Vec<_>>()}
                                        </div>
                                    }
                                }}
                            </div>
                        },
                        "quotas" => view! {
                            <div>
                                <div class="flex justify-between items-center mb-4">
                                    <h2 class="text-lg font-semibold">Snapshot Quotas</h2>
                                    <button
                                        on:click=move |_| set_show_create_quota_modal.set(true)
                                        class="bg-purple-500 hover:bg-purple-600 text-white px-4 py-2 rounded-lg"
                                    >
                                        <i class="fas fa-chart-pie mr-2"></i>
                                        "Create Quota"
                                    </button>
                                </div>

                                // Quota Summary
                                {move || quota_summary.get().map(|summary| view! {
                                    <div class="bg-gray-50 rounded-lg p-4 mb-6">
                                        <h3 class="font-medium mb-3">"Quota Overview"</h3>
                                        <div class="grid grid-cols-2 md:grid-cols-5 gap-4 text-center">
                                            <div>
                                                <div class="text-2xl font-semibold text-gray-900">{summary.total_quotas}</div>
                                                <div class="text-xs text-gray-500">"Total Quotas"</div>
                                            </div>
                                            <div>
                                                <div class="text-2xl font-semibold text-green-600">{summary.active_quotas}</div>
                                                <div class="text-xs text-gray-500">"Active"</div>
                                            </div>
                                            <div>
                                                <div class="text-2xl font-semibold text-blue-600">{summary.total_snapshots}</div>
                                                <div class="text-xs text-gray-500">"Snapshots"</div>
                                            </div>
                                            <div>
                                                <div class="text-2xl font-semibold text-purple-600">{format_bytes(summary.total_size)}</div>
                                                <div class="text-xs text-gray-500">"Total Size"</div>
                                            </div>
                                            <div>
                                                <div class=format!("text-2xl font-semibold {}",
                                                    if summary.quotas_exceeded > 0 { "text-red-600" } else { "text-green-600" })>
                                                    {summary.quotas_exceeded}
                                                </div>
                                                <div class="text-xs text-gray-500">"Exceeded"</div>
                                            </div>
                                        </div>
                                    </div>
                                })}

                                {move || if quotas.get().is_empty() {
                                    view! {
                                        <div class="text-center py-8 text-gray-500">
                                            <i class="fas fa-chart-pie text-4xl mb-4"></i>
                                            <p class="text-lg">"No Snapshot Quotas"</p>
                                            <p class="text-sm">"Create quotas to limit snapshot storage usage"</p>
                                        </div>
                                    }
                                } else {
                                    view! {
                                        <div class="space-y-4">
                                            {quotas.get().into_iter().map(|quota| {
                                                let quota_id = quota.id.clone();
                                                let quota_name = quota.name.clone();
                                                let quota_enabled = quota.enabled;
                                                let quota_desc = quota.description.clone();
                                                let quota_type = format!("{:?}", quota.quota_type);
                                                let limit_value = quota.limit_value;
                                                let cleanup_policy = format!("{:?}", quota.cleanup_policy);
                                                let storage_path = quota.storage_path.clone();
                                                view! {
                                                    <div class="border border-gray-200 rounded-lg p-4">
                                                        <div class="flex items-center justify-between">
                                                            <div class="flex-1">
                                                                <div class="flex items-center mb-2">
                                                                    <h3 class="font-medium text-gray-900 mr-3">{quota_name}</h3>
                                                                    <span class=format!("px-2 py-1 text-xs rounded {}",
                                                                        if quota_enabled {
                                                                            "bg-green-100 text-green-800"
                                                                        } else {
                                                                            "bg-gray-100 text-gray-800"
                                                                        })>
                                                                        {if quota_enabled { "Active" } else { "Disabled" }}
                                                                    </span>
                                                                </div>
                                                                {quota_desc.map(|desc| view! {
                                                                    <p class="text-sm text-gray-600 mb-2">{desc}</p>
                                                                })}
                                                                <div class="grid grid-cols-2 md:grid-cols-4 gap-4 text-xs text-gray-500">
                                                                    <div>
                                                                        <span class="font-medium">"Type: "</span>
                                                                        {quota_type}
                                                                    </div>
                                                                    <div>
                                                                        <span class="font-medium">"Limit: "</span>
                                                                        {limit_value}
                                                                    </div>
                                                                    <div>
                                                                        <span class="font-medium">"Policy: "</span>
                                                                        {cleanup_policy}
                                                                    </div>
                                                                    <div>
                                                                        <span class="font-medium">"Path: "</span>
                                                                        {storage_path}
                                                                    </div>
                                                                </div>
                                                            </div>
                                                            <div class="flex items-center space-x-2">
                                                                <button
                                                                    class="text-blue-600 hover:text-blue-900 text-sm"
                                                                >
                                                                    "Usage"
                                                                </button>
                                                                <button
                                                                    on:click=move |_| delete_quota(quota_id.clone())
                                                                    class="text-red-600 hover:text-red-900 text-sm"
                                                                >
                                                                    "Delete"
                                                                </button>
                                                            </div>
                                                        </div>
                                                    </div>
                                                }
                                            }).collect::<Vec<_>>()}
                                        </div>
                                    }
                                }}
                            </div>
                        },
                        _ => view! { <div></div> }
                    }}
                </div>
            </div>

            // Create Snapshot Modal
            {move || if show_create_snapshot_modal.get() {
                view! {
                    <div class="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50 p-4">
                        <div class="bg-white rounded-lg w-full max-w-md">
                            <div class="p-6 border-b border-gray-200">
                                <h2 class="text-xl font-semibold">"Create Snapshot"</h2>
                            </div>

                            <div class="p-6 space-y-4">
                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-1">
                                        "Snapshot Name"
                                    </label>
                                    <input
                                        type="text"
                                        placeholder="snapshot-$(date +%Y%m%d-%H%M%S)"
                                        prop:value=move || snapshot_name.get()
                                        on:input=move |ev| set_snapshot_name.set(event_target_value(&ev))
                                        class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                    />
                                </div>
                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-1">
                                        "Description"
                                    </label>
                                    <textarea
                                        placeholder="Optional description for this snapshot"
                                        prop:value=move || snapshot_description.get()
                                        on:input=move |ev| set_snapshot_description.set(event_target_value(&ev))
                                        class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                        rows="3"
                                    />
                                </div>
                                <div>
                                    <label class="flex items-center">
                                        <input
                                            type="checkbox"
                                            checked=move || include_memory.get()
                                            on:change=move |ev| set_include_memory.set(event_target_checked(&ev))
                                            class="mr-2"
                                        />
                                        "Include memory state (allows full restore)"
                                    </label>
                                    <p class="text-xs text-gray-500 mt-1">
                                        "Including memory allows restoring the exact running state but increases snapshot size"
                                    </p>
                                </div>
                            </div>

                            <div class="p-6 border-t border-gray-200 flex justify-end space-x-3">
                                <button
                                    on:click=move |_| set_show_create_snapshot_modal.set(false)
                                    class="px-4 py-2 text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50"
                                >
                                    "Cancel"
                                </button>
                                <button
                                    on:click=move |_| create_snapshot()
                                    class="px-4 py-2 bg-green-500 text-white rounded-lg hover:bg-green-600"
                                    disabled=move || snapshot_name.get().is_empty()
                                >
                                    "Create Snapshot"
                                </button>
                            </div>
                        </div>
                    </div>
                }
            } else {
                view! { <div></div> }
            }}

            // Create Schedule Modal
            {move || if show_create_schedule_modal.get() {
                view! {
                    <div class="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50 p-4">
                        <div class="bg-white rounded-lg w-full max-w-lg">
                            <div class="p-6 border-b border-gray-200">
                                <h2 class="text-xl font-semibold">"Create Snapshot Schedule"</h2>
                            </div>

                            <div class="p-6 space-y-4">
                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-1">
                                        "Schedule Name"
                                    </label>
                                    <input
                                        type="text"
                                        placeholder="Daily Snapshots"
                                        prop:value=move || schedule_name.get()
                                        on:input=move |ev| set_schedule_name.set(event_target_value(&ev))
                                        class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                    />
                                </div>
                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-1">
                                        "Description"
                                    </label>
                                    <input
                                        type="text"
                                        placeholder="Optional description"
                                        prop:value=move || schedule_description.get()
                                        on:input=move |ev| set_schedule_description.set(event_target_value(&ev))
                                        class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                    />
                                </div>
                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-1">
                                        "Schedule"
                                    </label>
                                    <select
                                        on:change=move |ev| set_schedule_cron.set(event_target_value(&ev))
                                        class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                    >
                                        <option value="0 2 * * *" selected=move || schedule_cron.get() == "0 2 * * *">"Daily at 2 AM"</option>
                                        <option value="0 2 * * 0" selected=move || schedule_cron.get() == "0 2 * * 0">"Weekly on Sunday"</option>
                                        <option value="0 2 1 * *" selected=move || schedule_cron.get() == "0 2 1 * *">"Monthly on 1st"</option>
                                        <option value="0 */6 * * *" selected=move || schedule_cron.get() == "0 */6 * * *">"Every 6 hours"</option>
                                        <option value="0 */12 * * *" selected=move || schedule_cron.get() == "0 */12 * * *">"Every 12 hours"</option>
                                    </select>
                                </div>
                                <div class="space-y-2">
                                    <label class="flex items-center">
                                        <input
                                            type="checkbox"
                                            checked=move || schedule_include_memory.get()
                                            on:change=move |ev| set_schedule_include_memory.set(event_target_checked(&ev))
                                            class="mr-2"
                                        />
                                        "Include memory state in snapshots"
                                    </label>
                                    <label class="flex items-center">
                                        <input
                                            type="checkbox"
                                            checked=move || schedule_enabled.get()
                                            on:change=move |ev| set_schedule_enabled.set(event_target_checked(&ev))
                                            class="mr-2"
                                        />
                                        "Enable schedule immediately"
                                    </label>
                                </div>
                            </div>

                            <div class="p-6 border-t border-gray-200 flex justify-end space-x-3">
                                <button
                                    on:click=move |_| set_show_create_schedule_modal.set(false)
                                    class="px-4 py-2 text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50"
                                >
                                    "Cancel"
                                </button>
                                <button
                                    on:click=move |_| create_schedule()
                                    class="px-4 py-2 bg-blue-500 text-white rounded-lg hover:bg-blue-600"
                                    disabled=move || schedule_name.get().is_empty()
                                >
                                    "Create Schedule"
                                </button>
                            </div>
                        </div>
                    </div>
                }
            } else {
                view! { <div></div> }
            }}

            // Create Quota Modal
            {move || if show_create_quota_modal.get() {
                view! {
                    <div class="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50 p-4">
                        <div class="bg-white rounded-lg w-full max-w-lg">
                            <div class="p-6 border-b border-gray-200">
                                <h2 class="text-xl font-semibold">"Create Snapshot Quota"</h2>
                            </div>

                            <div class="p-6 space-y-4">
                                <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                                    <div>
                                        <label class="block text-sm font-medium text-gray-700 mb-1">
                                            "Quota Name"
                                        </label>
                                        <input
                                            type="text"
                                            placeholder="Default Quota"
                                            prop:value=move || quota_name.get()
                                            on:input=move |ev| set_quota_name.set(event_target_value(&ev))
                                            class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                        />
                                    </div>
                                    <div>
                                        <label class="block text-sm font-medium text-gray-700 mb-1">
                                            "Quota Type"
                                        </label>
                                        <select
                                            on:change=move |ev| {
                                                let value = event_target_value(&ev);
                                                let quota_type = match value.as_str() {
                                                    "MaxSize" => QuotaType::MaxSize,
                                                    "MaxAge" => QuotaType::MaxAge,
                                                    _ => QuotaType::MaxCount,
                                                };
                                                set_quota_type.set(quota_type);
                                            }
                                            class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                        >
                                            <option value="MaxCount">"Max Count"</option>
                                            <option value="MaxSize">"Max Size"</option>
                                            <option value="MaxAge">"Max Age"</option>
                                        </select>
                                    </div>
                                </div>
                                <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                                    <div>
                                        <label class="block text-sm font-medium text-gray-700 mb-1">
                                            {move || match quota_type.get() {
                                                QuotaType::MaxCount => "Max Snapshots",
                                                QuotaType::MaxSize => "Max Size (GB)",
                                                QuotaType::MaxAge => "Max Age (days)",
                                            }}
                                        </label>
                                        <input
                                            type="number"
                                            min="1"
                                            prop:value=move || quota_limit.get().to_string()
                                            on:input=move |ev| set_quota_limit.set(event_target_value(&ev).parse().unwrap_or(10))
                                            class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                        />
                                    </div>
                                    <div>
                                        <label class="block text-sm font-medium text-gray-700 mb-1">
                                            "Cleanup Policy"
                                        </label>
                                        <select
                                            on:change=move |ev| {
                                                let value = event_target_value(&ev);
                                                let policy = match value.as_str() {
                                                    "LargestFirst" => CleanupPolicy::LargestFirst,
                                                    "Manual" => CleanupPolicy::Manual,
                                                    _ => CleanupPolicy::OldestFirst,
                                                };
                                                set_quota_cleanup_policy.set(policy);
                                            }
                                            class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                        >
                                            <option value="OldestFirst">"Oldest First"</option>
                                            <option value="LargestFirst">"Largest First"</option>
                                            <option value="Manual">"Manual"</option>
                                        </select>
                                    </div>
                                </div>
                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-1">
                                        "Storage Path"
                                    </label>
                                    <input
                                        type="text"
                                        prop:value=move || quota_storage_path.get()
                                        on:input=move |ev| set_quota_storage_path.set(event_target_value(&ev))
                                        class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                    />
                                </div>
                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-1">
                                        "Description"
                                    </label>
                                    <textarea
                                        placeholder="Optional description"
                                        prop:value=move || quota_description.get()
                                        on:input=move |ev| set_quota_description.set(event_target_value(&ev))
                                        class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                        rows="2"
                                    />
                                </div>
                                <div>
                                    <label class="flex items-center">
                                        <input
                                            type="checkbox"
                                            checked=move || quota_enabled.get()
                                            on:change=move |ev| set_quota_enabled.set(event_target_checked(&ev))
                                            class="mr-2"
                                        />
                                        "Enable quota immediately"
                                    </label>
                                </div>
                            </div>

                            <div class="p-6 border-t border-gray-200 flex justify-end space-x-3">
                                <button
                                    on:click=move |_| set_show_create_quota_modal.set(false)
                                    class="px-4 py-2 text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50"
                                >
                                    "Cancel"
                                </button>
                                <button
                                    on:click=move |_| create_quota()
                                    class="px-4 py-2 bg-purple-500 text-white rounded-lg hover:bg-purple-600"
                                    disabled=move || quota_name.get().is_empty()
                                >
                                    "Create Quota"
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