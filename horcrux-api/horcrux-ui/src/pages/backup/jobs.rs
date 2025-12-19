use leptos::*;
use crate::api::{
    BackupJob, CreateBackupJobRequest, RetentionPolicy, BackupTarget, BackupEncryption,
    get_backup_jobs, create_backup_job, run_backup_job_now,
    get_vms, BackupStatus
};
use horcrux_common::VmConfig;

#[component]
pub fn BackupJobsPage() -> impl IntoView {
    let (backup_jobs, set_backup_jobs) = create_signal(Vec::<BackupJob>::new());
    let (vms, set_vms) = create_signal(Vec::<VmConfig>::new());
    let (loading, set_loading) = create_signal(false);
    let (error, set_error) = create_signal(None::<String>);
    let (show_create_modal, set_show_create_modal) = create_signal(false);
    let (search_query, set_search_query) = create_signal(String::new());

    // Form state
    let (name, set_name) = create_signal(String::new());
    let (description, set_description) = create_signal(String::new());
    let (selected_vm_ids, set_selected_vm_ids) = create_signal(Vec::<String>::new());
    let (schedule, set_schedule) = create_signal("0 2 * * *".to_string()); // Daily at 2 AM
    let (storage_id, set_storage_id) = create_signal("local".to_string());
    let (backup_path, set_backup_path) = create_signal("/var/backups".to_string());
    let (enabled, set_enabled) = create_signal(true);

    // Retention policy form state
    let (keep_hourly, set_keep_hourly) = create_signal(0u32);
    let (keep_daily, set_keep_daily) = create_signal(7u32);
    let (keep_weekly, set_keep_weekly) = create_signal(4u32);
    let (keep_monthly, set_keep_monthly) = create_signal(12u32);
    let (keep_yearly, set_keep_yearly) = create_signal(0u32);
    let (max_age_days, set_max_age_days) = create_signal(90u32);

    // Encryption settings
    let (use_encryption, set_use_encryption) = create_signal(false);
    let (encryption_method, set_encryption_method) = create_signal("AES-256-GCM".to_string());
    let (encryption_key_id, set_encryption_key_id) = create_signal(String::new());

    let load_backup_jobs = move || {
        set_loading.set(true);
        set_error.set(None);

        spawn_local(async move {
            match get_backup_jobs().await {
                Ok(jobs) => set_backup_jobs.set(jobs),
                Err(e) => set_error.set(Some(format!("Failed to load backup jobs: {}", e))),
            }

            // Also load VMs for the creation form
            match get_vms().await {
                Ok(vm_list) => set_vms.set(vm_list),
                Err(e) => set_error.set(Some(format!("Failed to load VMs: {}", e))),
            }

            set_loading.set(false);
        });
    };

    // Load data on mount
    create_effect(move |_| {
        load_backup_jobs();
    });

    // Auto-refresh every 60 seconds
    use leptos::set_interval;
    set_interval(
        move || load_backup_jobs(),
        std::time::Duration::from_secs(60),
    );

    let filtered_jobs = move || {
        let query = search_query.get().to_lowercase();
        if query.is_empty() {
            backup_jobs.get()
        } else {
            backup_jobs
                .get()
                .into_iter()
                .filter(|job| {
                    job.name.to_lowercase().contains(&query) ||
                    job.description.as_ref().map_or(false, |d| d.to_lowercase().contains(&query))
                })
                .collect()
        }
    };

    let toggle_vm_selection = move |vm_id: String| {
        let mut current = selected_vm_ids.get();
        if current.contains(&vm_id) {
            current.retain(|id| id != &vm_id);
        } else {
            current.push(vm_id);
        }
        set_selected_vm_ids.set(current);
    };

    let reset_form = move || {
        set_name.set(String::new());
        set_description.set(String::new());
        set_selected_vm_ids.set(Vec::new());
        set_schedule.set("0 2 * * *".to_string());
        set_storage_id.set("local".to_string());
        set_backup_path.set("/var/backups".to_string());
        set_enabled.set(true);
        set_keep_hourly.set(0);
        set_keep_daily.set(7);
        set_keep_weekly.set(4);
        set_keep_monthly.set(12);
        set_keep_yearly.set(0);
        set_max_age_days.set(90);
        set_use_encryption.set(false);
        set_encryption_method.set("AES-256-GCM".to_string());
        set_encryption_key_id.set(String::new());
    };

    let create_backup_job = move || {
        let retention = RetentionPolicy {
            keep_hourly: if keep_hourly.get() > 0 { Some(keep_hourly.get()) } else { None },
            keep_daily: if keep_daily.get() > 0 { Some(keep_daily.get()) } else { None },
            keep_weekly: if keep_weekly.get() > 0 { Some(keep_weekly.get()) } else { None },
            keep_monthly: if keep_monthly.get() > 0 { Some(keep_monthly.get()) } else { None },
            keep_yearly: if keep_yearly.get() > 0 { Some(keep_yearly.get()) } else { None },
            max_age_days: if max_age_days.get() > 0 { Some(max_age_days.get()) } else { None },
        };

        let target = BackupTarget {
            storage_id: storage_id.get(),
            path: backup_path.get(),
            encryption: if use_encryption.get() {
                Some(BackupEncryption {
                    method: encryption_method.get(),
                    key_id: encryption_key_id.get(),
                })
            } else {
                None
            },
        };

        let request = CreateBackupJobRequest {
            name: name.get(),
            description: if description.get().is_empty() { None } else { Some(description.get()) },
            vm_ids: selected_vm_ids.get(),
            schedule: schedule.get(),
            retention,
            target,
            enabled: enabled.get(),
        };

        spawn_local(async move {
            match create_backup_job(request).await {
                Ok(_) => {
                    set_show_create_modal.set(false);
                    reset_form();
                    load_backup_jobs();
                }
                Err(e) => set_error.set(Some(format!("Failed to create backup job: {}", e))),
            }
        });
    };

    let run_job_now = move |job_id: String| {
        spawn_local(async move {
            match run_backup_job_now(&job_id).await {
                Ok(_) => {
                    // Reload jobs to see updated status
                    load_backup_jobs();
                }
                Err(e) => set_error.set(Some(format!("Failed to run backup job: {}", e))),
            }
        });
    };

    let get_status_color = move |status: &Option<BackupStatus>| {
        match status {
            Some(BackupStatus::Completed) => "bg-green-100 text-green-800",
            Some(BackupStatus::Running) => "bg-blue-100 text-blue-800",
            Some(BackupStatus::Pending) => "bg-yellow-100 text-yellow-800",
            Some(BackupStatus::Failed) => "bg-red-100 text-red-800",
            Some(BackupStatus::Cancelled) => "bg-gray-100 text-gray-800",
            None => "bg-gray-100 text-gray-800",
        }
    };

    let get_cron_description = move |cron: &str| -> String {
        match cron {
            "0 2 * * *" => "Daily at 2:00 AM".to_string(),
            "0 2 * * 0" => "Weekly on Sunday at 2:00 AM".to_string(),
            "0 2 1 * *" => "Monthly on 1st at 2:00 AM".to_string(),
            "0 */6 * * *" => "Every 6 hours".to_string(),
            "0 */12 * * *" => "Every 12 hours".to_string(),
            _ => cron.to_string(),
        }
    };

    view! {
        <div class="p-6">
            <div class="flex justify-between items-center mb-6">
                <h1 class="text-2xl font-bold">Backup Jobs</h1>
                <button
                    on:click=move |_| {
                        reset_form();
                        set_show_create_modal.set(true);
                    }
                    class="bg-blue-500 hover:bg-blue-600 text-white px-4 py-2 rounded-lg flex items-center gap-2"
                >
                    <i class="fas fa-plus"></i>
                    "Create Backup Job"
                </button>
            </div>

            // Controls
            <div class="bg-white rounded-lg shadow p-4 mb-6">
                <div class="flex items-center space-x-4">
                    <div class="flex-1">
                        <label class="block text-sm font-medium text-gray-700 mb-1">
                            "Search Jobs"
                        </label>
                        <input
                            type="text"
                            placeholder="Search by name or description..."
                            on:input=move |ev| set_search_query.set(event_target_value(&ev))
                            class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                        />
                    </div>
                    <div class="flex space-x-2">
                        <button
                            on:click=move |_| load_backup_jobs()
                            class="bg-gray-500 hover:bg-gray-600 text-white px-4 py-2 rounded-lg flex items-center gap-2"
                        >
                            <i class="fas fa-sync"></i>
                            "Refresh"
                        </button>
                    </div>
                </div>
            </div>

            // Error display
            {move || error.get().map(|e| view! {
                <div class="bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded mb-6">
                    {e}
                </div>
            })}

            // Loading state
            {move || if loading.get() {
                view! {
                    <div class="bg-white rounded-lg shadow p-8 text-center">
                        <i class="fas fa-spinner fa-spin text-2xl text-gray-400 mb-2"></i>
                        <p class="text-gray-600">"Loading backup jobs..."</p>
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
                                            "VMs"
                                        </th>
                                        <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                            "Schedule"
                                        </th>
                                        <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                            "Last Status"
                                        </th>
                                        <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                            "Next Run"
                                        </th>
                                        <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                            "Actions"
                                        </th>
                                    </tr>
                                </thead>
                                <tbody class="bg-white divide-y divide-gray-200">
                                    {move || if filtered_jobs().is_empty() {
                                        view! {
                                            <tr>
                                                <td colspan="6" class="px-6 py-8 text-center text-gray-500">
                                                    <div>
                                                        <i class="fas fa-calendar-plus text-4xl mb-4"></i>
                                                        <p class="text-lg mb-2">"No backup jobs found"</p>
                                                        <p class="text-sm">"Create your first backup job to get started"</p>
                                                    </div>
                                                </td>
                                            </tr>
                                        }.into_view()
                                    } else {
                                        filtered_jobs().into_iter().map(|job| {
                                                let job_id = job.id.clone();
                                                let job_id2 = job.id.clone();
                                                let status_color = get_status_color(&job.last_status);
                                                let job_name = job.name.clone();
                                                let job_desc = job.description.clone();
                                                let job_enabled = job.enabled;
                                                let vm_count = job.vm_ids.len();
                                                let vm_display = if job.vm_ids.len() <= 3 {
                                                    job.vm_ids.join(", ")
                                                } else {
                                                    format!("{}, +{} more", job.vm_ids[..3].join(", "), job.vm_ids.len() - 3)
                                                };
                                                let cron_desc = get_cron_description(&job.schedule);
                                                let schedule = job.schedule.clone();
                                                let last_status = job.last_status.clone();
                                                let last_run = job.last_run.clone();
                                                let next_run = job.next_run.clone().unwrap_or_else(|| "Unknown".to_string());

                                                view! {
                                                    <tr>
                                                        <td class="px-6 py-4 whitespace-nowrap">
                                                            <div class="flex items-center">
                                                                <div class=format!("w-3 h-3 rounded-full mr-3 {}",
                                                                    if job_enabled { "bg-green-500" } else { "bg-gray-400" })></div>
                                                                <div>
                                                                    <div class="text-sm font-medium text-gray-900">{job_name}</div>
                                                                    {job_desc.map(|desc| view! {
                                                                        <div class="text-xs text-gray-500">{desc}</div>
                                                                    })}
                                                                </div>
                                                            </div>
                                                        </td>
                                                        <td class="px-6 py-4 whitespace-nowrap">
                                                            <div class="text-sm text-gray-900">
                                                                {format!("{} VMs", vm_count)}
                                                            </div>
                                                            <div class="text-xs text-gray-500">{vm_display}</div>
                                                        </td>
                                                        <td class="px-6 py-4 whitespace-nowrap">
                                                            <div class="text-sm text-gray-900">{cron_desc}</div>
                                                            <div class="text-xs text-gray-500 font-mono">{schedule}</div>
                                                        </td>
                                                        <td class="px-6 py-4 whitespace-nowrap">
                                                            {match last_status {
                                                                Some(status) => view! {
                                                                    <span class=format!("px-2 py-1 text-xs font-medium rounded {}", status_color)>
                                                                        {format!("{:?}", status)}
                                                                    </span>
                                                                }.into_view(),
                                                                None => view! {
                                                                    <span class="text-gray-500 text-sm">"Never run"</span>
                                                                }.into_view()
                                                            }}
                                                            {last_run.map(|last| view! {
                                                                <div class="text-xs text-gray-500 mt-1">{last}</div>
                                                            })}
                                                        </td>
                                                        <td class="px-6 py-4 whitespace-nowrap">
                                                            <div class="text-sm text-gray-900">{next_run}</div>
                                                        </td>
                                                        <td class="px-6 py-4 whitespace-nowrap text-sm font-medium">
                                                            <div class="flex space-x-2">
                                                                <button
                                                                    on:click=move |_| run_job_now(job_id.clone())
                                                                    class="text-blue-600 hover:text-blue-900 px-2 py-1 rounded hover:bg-blue-50"
                                                                    title="Run Now"
                                                                    disabled=move || !job_enabled
                                                                >
                                                                    <i class="fas fa-play"></i>
                                                                </button>
                                                                <button
                                                                    class="text-gray-600 hover:text-gray-900 px-2 py-1 rounded hover:bg-gray-50"
                                                                    title="Edit Job"
                                                                >
                                                                    <i class="fas fa-edit"></i>
                                                                </button>
                                                                <button
                                                                    class="text-red-600 hover:text-red-900 px-2 py-1 rounded hover:bg-red-50"
                                                                    title="Delete Job"
                                                                >
                                                                    <i class="fas fa-trash"></i>
                                                                </button>
                                                            </div>
                                                        </td>
                                                    </tr>
                                                }
                                        }).collect_view()
                                    }}
                                </tbody>
                            </table>
                        </div>
                    </div>
                }
            }}

            // Create Job Modal
            {move || if show_create_modal.get() {
                view! {
                    <div class="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50 p-4">
                        <div class="bg-white rounded-lg w-full max-w-6xl max-h-[90vh] overflow-y-auto">
                            <div class="p-6 border-b border-gray-200">
                                <h2 class="text-xl font-semibold">"Create Backup Job"</h2>
                            </div>

                            <div class="p-6 space-y-6">
                                // Basic Information
                                <div>
                                    <h3 class="text-lg font-medium mb-4">"Basic Information"</h3>
                                    <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                                        <div>
                                            <label class="block text-sm font-medium text-gray-700 mb-1">
                                                "Job Name"
                                            </label>
                                            <input
                                                type="text"
                                                placeholder="Daily VM Backup"
                                                prop:value=move || name.get()
                                                on:input=move |ev| set_name.set(event_target_value(&ev))
                                                class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                            />
                                        </div>
                                        <div>
                                            <label class="block text-sm font-medium text-gray-700 mb-1">
                                                "Schedule"
                                            </label>
                                            <select
                                                on:change=move |ev| set_schedule.set(event_target_value(&ev))
                                                class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                            >
                                                <option value="0 2 * * *" selected=move || schedule.get() == "0 2 * * *">"Daily at 2 AM"</option>
                                                <option value="0 2 * * 0" selected=move || schedule.get() == "0 2 * * 0">"Weekly on Sunday"</option>
                                                <option value="0 2 1 * *" selected=move || schedule.get() == "0 2 1 * *">"Monthly on 1st"</option>
                                                <option value="0 */6 * * *" selected=move || schedule.get() == "0 */6 * * *">"Every 6 hours"</option>
                                                <option value="0 */12 * * *" selected=move || schedule.get() == "0 */12 * * *">"Every 12 hours"</option>
                                            </select>
                                        </div>
                                    </div>
                                    <div class="mt-4">
                                        <label class="block text-sm font-medium text-gray-700 mb-1">
                                            "Description"
                                        </label>
                                        <textarea
                                            placeholder="Optional description for this backup job"
                                            prop:value=move || description.get()
                                            on:input=move |ev| set_description.set(event_target_value(&ev))
                                            class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                            rows="3"
                                        />
                                    </div>
                                </div>

                                // VM Selection
                                <div>
                                    <h3 class="text-lg font-medium mb-4">"Select VMs to Backup"</h3>
                                    <div class="max-h-48 overflow-y-auto border border-gray-200 rounded-lg">
                                        {move || vms.get().into_iter().map(|vm| {
                                            let vm_id = vm.id.clone();
                                            let vm_id2 = vm.id.clone();
                                            let vm_id3 = vm.id.clone();
                                            let vm_name = vm.name.clone();
                                            let vm_status = format!("{:?}", vm.status).to_lowercase();
                                            let state_class = match vm.status {
                                                horcrux_common::VmStatus::Running => "bg-green-100 text-green-800",
                                                horcrux_common::VmStatus::Stopped => "bg-gray-100 text-gray-800",
                                                _ => "bg-yellow-100 text-yellow-800"
                                            };

                                            view! {
                                                <label class="flex items-center p-3 hover:bg-gray-50 border-b border-gray-100 cursor-pointer">
                                                    <input
                                                        type="checkbox"
                                                        checked=move || selected_vm_ids.get().contains(&vm_id)
                                                        on:change=move |_| toggle_vm_selection(vm_id2.clone())
                                                        class="mr-3"
                                                    />
                                                    <div class="flex-1">
                                                        <div class="text-sm font-medium text-gray-900">{vm_name}</div>
                                                        <div class="text-xs text-gray-500">{format!("ID: {}", vm_id3)}</div>
                                                    </div>
                                                    <div class=format!("px-2 py-1 text-xs rounded {}", state_class)>
                                                        {vm_status}
                                                    </div>
                                                </label>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </div>
                                    <div class="text-sm text-gray-600 mt-2">
                                        {move || format!("{} VMs selected", selected_vm_ids.get().len())}
                                    </div>
                                </div>

                                // Backup Target
                                <div>
                                    <h3 class="text-lg font-medium mb-4">"Backup Target"</h3>
                                    <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                                        <div>
                                            <label class="block text-sm font-medium text-gray-700 mb-1">
                                                "Storage"
                                            </label>
                                            <select
                                                on:change=move |ev| set_storage_id.set(event_target_value(&ev))
                                                class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                            >
                                                <option value="local">"Local Storage"</option>
                                                <option value="nas">"NAS Storage"</option>
                                                <option value="s3">"S3 Compatible"</option>
                                            </select>
                                        </div>
                                        <div>
                                            <label class="block text-sm font-medium text-gray-700 mb-1">
                                                "Backup Path"
                                            </label>
                                            <input
                                                type="text"
                                                placeholder="/var/backups"
                                                prop:value=move || backup_path.get()
                                                on:input=move |ev| set_backup_path.set(event_target_value(&ev))
                                                class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                            />
                                        </div>
                                    </div>
                                </div>

                                // Retention Policy
                                <div>
                                    <h3 class="text-lg font-medium mb-4">"Retention Policy"</h3>
                                    <div class="grid grid-cols-2 md:grid-cols-3 gap-4">
                                        <div>
                                            <label class="block text-sm font-medium text-gray-700 mb-1">
                                                "Keep Hourly"
                                            </label>
                                            <input
                                                type="number"
                                                min="0"
                                                max="24"
                                                prop:value=move || keep_hourly.get().to_string()
                                                on:input=move |ev| set_keep_hourly.set(event_target_value(&ev).parse().unwrap_or(0))
                                                class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                            />
                                        </div>
                                        <div>
                                            <label class="block text-sm font-medium text-gray-700 mb-1">
                                                "Keep Daily"
                                            </label>
                                            <input
                                                type="number"
                                                min="0"
                                                max="365"
                                                prop:value=move || keep_daily.get().to_string()
                                                on:input=move |ev| set_keep_daily.set(event_target_value(&ev).parse().unwrap_or(7))
                                                class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                            />
                                        </div>
                                        <div>
                                            <label class="block text-sm font-medium text-gray-700 mb-1">
                                                "Keep Weekly"
                                            </label>
                                            <input
                                                type="number"
                                                min="0"
                                                max="52"
                                                prop:value=move || keep_weekly.get().to_string()
                                                on:input=move |ev| set_keep_weekly.set(event_target_value(&ev).parse().unwrap_or(4))
                                                class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                            />
                                        </div>
                                        <div>
                                            <label class="block text-sm font-medium text-gray-700 mb-1">
                                                "Keep Monthly"
                                            </label>
                                            <input
                                                type="number"
                                                min="0"
                                                max="120"
                                                prop:value=move || keep_monthly.get().to_string()
                                                on:input=move |ev| set_keep_monthly.set(event_target_value(&ev).parse().unwrap_or(12))
                                                class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                            />
                                        </div>
                                        <div>
                                            <label class="block text-sm font-medium text-gray-700 mb-1">
                                                "Keep Yearly"
                                            </label>
                                            <input
                                                type="number"
                                                min="0"
                                                max="100"
                                                prop:value=move || keep_yearly.get().to_string()
                                                on:input=move |ev| set_keep_yearly.set(event_target_value(&ev).parse().unwrap_or(0))
                                                class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                            />
                                        </div>
                                        <div>
                                            <label class="block text-sm font-medium text-gray-700 mb-1">
                                                "Max Age (days)"
                                            </label>
                                            <input
                                                type="number"
                                                min="0"
                                                max="3650"
                                                prop:value=move || max_age_days.get().to_string()
                                                on:input=move |ev| set_max_age_days.set(event_target_value(&ev).parse().unwrap_or(90))
                                                class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                            />
                                        </div>
                                    </div>
                                </div>

                                // Options
                                <div>
                                    <h3 class="text-lg font-medium mb-4">"Options"</h3>
                                    <div class="space-y-4">
                                        <label class="flex items-center">
                                            <input
                                                type="checkbox"
                                                checked=move || enabled.get()
                                                on:change=move |ev| set_enabled.set(event_target_checked(&ev))
                                                class="mr-2"
                                            />
                                            "Enable job immediately"
                                        </label>
                                        <label class="flex items-center">
                                            <input
                                                type="checkbox"
                                                checked=move || use_encryption.get()
                                                on:change=move |ev| set_use_encryption.set(event_target_checked(&ev))
                                                class="mr-2"
                                            />
                                            "Enable encryption"
                                        </label>
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
                                    on:click=move |_| create_backup_job()
                                    class="px-4 py-2 bg-blue-500 text-white rounded-lg hover:bg-blue-600"
                                    disabled=move || name.get().is_empty() || selected_vm_ids.get().is_empty()
                                >
                                    "Create Job"
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