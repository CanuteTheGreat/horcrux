use leptos::*;
use crate::api::{
    VmBackup, BackupJob, QuotaSummary, VmTemplate,
    get_backups, get_backup_jobs, get_snapshot_quota_summary,
    get_templates, BackupStatus
};

#[component]
pub fn BackupDashboard() -> impl IntoView {
    let (backups, set_backups) = create_signal(Vec::<VmBackup>::new());
    let (backup_jobs, set_backup_jobs) = create_signal(Vec::<BackupJob>::new());
    let (quota_summary, set_quota_summary) = create_signal(None::<QuotaSummary>);
    let (templates, set_templates) = create_signal(Vec::<VmTemplate>::new());
    let (loading, set_loading) = create_signal(false);
    let (error, set_error) = create_signal(None::<String>);

    let load_dashboard_data = move || {
        set_loading.set(true);
        set_error.set(None);

        spawn_local(async move {
            let mut errors = Vec::new();

            // Load recent backups
            match get_backups().await {
                Ok(mut backups_list) => {
                    // Sort by created_at descending and take first 10
                    backups_list.sort_by(|a, b| b.created_at.cmp(&a.created_at));
                    backups_list.truncate(10);
                    set_backups.set(backups_list);
                }
                Err(e) => errors.push(format!("Failed to load backups: {}", e)),
            }

            // Load backup jobs
            match get_backup_jobs().await {
                Ok(jobs) => set_backup_jobs.set(jobs),
                Err(e) => errors.push(format!("Failed to load backup jobs: {}", e)),
            }

            // Load quota summary
            match get_snapshot_quota_summary().await {
                Ok(summary) => set_quota_summary.set(Some(summary)),
                Err(e) => errors.push(format!("Failed to load quota summary: {}", e)),
            }

            // Load templates
            match get_templates().await {
                Ok(template_list) => set_templates.set(template_list),
                Err(e) => errors.push(format!("Failed to load templates: {}", e)),
            }

            if !errors.is_empty() {
                set_error.set(Some(errors.join("; ")));
            }
            set_loading.set(false);
        });
    };

    // Load data on mount
    create_effect(move |_| {
        load_dashboard_data();
    });

    // Auto-refresh every 30 seconds
    use leptos::set_interval;
    set_interval(
        move || load_dashboard_data(),
        std::time::Duration::from_secs(30),
    );

    let get_status_color = move |status: &BackupStatus| {
        match status {
            BackupStatus::Completed => "bg-green-100 text-green-800",
            BackupStatus::Running => "bg-blue-100 text-blue-800",
            BackupStatus::Pending => "bg-yellow-100 text-yellow-800",
            BackupStatus::Failed => "bg-red-100 text-red-800",
            BackupStatus::Cancelled => "bg-gray-100 text-gray-800",
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
                <h1 class="text-2xl font-bold">Backup & Data Protection</h1>
                <div class="flex space-x-3">
                    <a href="/backup/jobs" class="bg-blue-500 hover:bg-blue-600 text-white px-4 py-2 rounded-lg">
                        "Manage Jobs"
                    </a>
                    <a href="/backup/snapshots" class="bg-green-500 hover:bg-green-600 text-white px-4 py-2 rounded-lg">
                        "Snapshots"
                    </a>
                    <a href="/backup/templates" class="bg-purple-500 hover:bg-purple-600 text-white px-4 py-2 rounded-lg">
                        "Templates"
                    </a>
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
                        <p class="text-gray-600">"Loading dashboard data..."</p>
                    </div>
                }
            } else {
                view! {
                    <div class="space-y-6">
                        // Statistics Overview
                        <div class="grid grid-cols-1 md:grid-cols-4 gap-6">
                            // Recent Backups Count
                            <div class="bg-white rounded-lg shadow p-6">
                                <div class="flex items-center">
                                    <div class="flex-shrink-0">
                                        <i class="fas fa-archive text-2xl text-blue-500"></i>
                                    </div>
                                    <div class="ml-4">
                                        <p class="text-sm font-medium text-gray-500">"Recent Backups"</p>
                                        <p class="text-2xl font-semibold text-gray-900">{backups.get().len()}</p>
                                    </div>
                                </div>
                            </div>

                            // Active Backup Jobs
                            <div class="bg-white rounded-lg shadow p-6">
                                <div class="flex items-center">
                                    <div class="flex-shrink-0">
                                        <i class="fas fa-clock text-2xl text-green-500"></i>
                                    </div>
                                    <div class="ml-4">
                                        <p class="text-sm font-medium text-gray-500">"Active Jobs"</p>
                                        <p class="text-2xl font-semibold text-gray-900">
                                            {backup_jobs.get().iter().filter(|job| job.enabled).count()}
                                        </p>
                                    </div>
                                </div>
                            </div>

                            // Quota Summary
                            {move || if let Some(summary) = quota_summary.get() {
                                view! {
                                    <div class="bg-white rounded-lg shadow p-6">
                                        <div class="flex items-center">
                                            <div class="flex-shrink-0">
                                                <i class="fas fa-chart-pie text-2xl text-yellow-500"></i>
                                            </div>
                                            <div class="ml-4">
                                                <p class="text-sm font-medium text-gray-500">"Total Snapshots"</p>
                                                <p class="text-2xl font-semibold text-gray-900">{summary.total_snapshots}</p>
                                                <p class="text-xs text-gray-500">{format_bytes(summary.total_size)}</p>
                                            </div>
                                        </div>
                                    </div>
                                }
                            } else {
                                view! {
                                    <div class="bg-white rounded-lg shadow p-6">
                                        <div class="text-gray-500">"Loading quota data..."</div>
                                    </div>
                                }
                            }}

                            // Templates Count
                            <div class="bg-white rounded-lg shadow p-6">
                                <div class="flex items-center">
                                    <div class="flex-shrink-0">
                                        <i class="fas fa-file-archive text-2xl text-purple-500"></i>
                                    </div>
                                    <div class="ml-4">
                                        <p class="text-sm font-medium text-gray-500">"Templates"</p>
                                        <p class="text-2xl font-semibold text-gray-900">{templates.get().len()}</p>
                                    </div>
                                </div>
                            </div>
                        </div>

                        // Recent Backup Activity
                        <div class="bg-white rounded-lg shadow">
                            <div class="px-6 py-4 border-b border-gray-200">
                                <h2 class="text-lg font-semibold">Recent Backup Activity</h2>
                            </div>
                            <div class="overflow-x-auto">
                                <table class="min-w-full divide-y divide-gray-200">
                                    <thead class="bg-gray-50">
                                        <tr>
                                            <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                                "VM"
                                            </th>
                                            <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                                "Backup Name"
                                            </th>
                                            <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                                "Status"
                                            </th>
                                            <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                                "Size"
                                            </th>
                                            <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                                "Created"
                                            </th>
                                        </tr>
                                    </thead>
                                    <tbody class="bg-white divide-y divide-gray-200">
                                        {move || if backups.get().is_empty() {
                                            view! {
                                                <tr>
                                                    <td colspan="5" class="px-6 py-4 text-center text-gray-500">
                                                        "No recent backups"
                                                    </td>
                                                </tr>
                                            }.into_view()
                                        } else {
                                            backups.get().into_iter().map(|backup| {
                                                let status_color = get_status_color(&backup.status);
                                                let vm_id = backup.vm_id.clone();
                                                let name = backup.name.clone().unwrap_or_else(|| backup.id.clone());
                                                let desc = backup.description.clone();
                                                let status_str = format!("{:?}", backup.status);
                                                let size_str = if let Some(size) = backup.size {
                                                    format_bytes(size)
                                                } else {
                                                    "Unknown".to_string()
                                                };
                                                let compressed = backup.compressed_size;
                                                let created = backup.created_at.clone();
                                                view! {
                                                    <tr>
                                                        <td class="px-6 py-4 whitespace-nowrap">
                                                            <div class="text-sm font-medium text-gray-900">{vm_id}</div>
                                                        </td>
                                                        <td class="px-6 py-4 whitespace-nowrap">
                                                            <div class="text-sm text-gray-900">{name}</div>
                                                            {desc.map(|d| view! {
                                                                <div class="text-xs text-gray-500">{d}</div>
                                                            })}
                                                        </td>
                                                        <td class="px-6 py-4 whitespace-nowrap">
                                                            <span class=format!("px-2 py-1 text-xs font-medium rounded {}", status_color)>
                                                                {status_str}
                                                            </span>
                                                        </td>
                                                        <td class="px-6 py-4 whitespace-nowrap">
                                                            <div class="text-sm text-gray-900">{size_str}</div>
                                                            {compressed.map(|size| view! {
                                                                <div class="text-xs text-gray-500">
                                                                    "Compressed: " {format_bytes(size)}
                                                                </div>
                                                            })}
                                                        </td>
                                                        <td class="px-6 py-4 whitespace-nowrap">
                                                            <div class="text-sm text-gray-900">{created}</div>
                                                        </td>
                                                    </tr>
                                                }
                                            }).collect_view()
                                        }}
                                    </tbody>
                                </table>
                            </div>
                            <div class="px-6 py-4 border-t border-gray-200">
                                <a href="/backup/history" class="text-blue-600 hover:text-blue-800 text-sm font-medium">
                                    "View All Backups ->"
                                </a>
                            </div>
                        </div>

                        // Active Backup Jobs
                        <div class="bg-white rounded-lg shadow">
                            <div class="px-6 py-4 border-b border-gray-200">
                                <div class="flex justify-between items-center">
                                    <h2 class="text-lg font-semibold">Active Backup Jobs</h2>
                                    <a href="/backup/jobs" class="text-blue-600 hover:text-blue-800 text-sm font-medium">
                                        "Manage Jobs"
                                    </a>
                                </div>
                            </div>
                            <div class="p-6">
                                {move || if backup_jobs.get().is_empty() {
                                    view! {
                                        <div class="text-center text-gray-500">
                                            <i class="fas fa-plus-circle text-4xl mb-4"></i>
                                            <p class="text-lg mb-2">"No backup jobs configured"</p>
                                            <p class="text-sm mb-4">"Create a backup job to automate your backup process"</p>
                                            <a href="/backup/jobs" class="bg-blue-500 hover:bg-blue-600 text-white px-4 py-2 rounded-lg">
                                                "Create Backup Job"
                                            </a>
                                        </div>
                                    }
                                } else {
                                    view! {
                                        <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
                                            {backup_jobs.get().into_iter().filter(|job| job.enabled).take(6).map(|job| {
                                                let job_name = job.name.clone();
                                                let job_enabled = job.enabled;
                                                let vm_count = job.vm_ids.len();
                                                let schedule = job.schedule.clone();
                                                let next_run = job.next_run.clone();
                                                let last_status = job.last_status.clone();
                                                view! {
                                                    <div class="border border-gray-200 rounded-lg p-4">
                                                        <div class="flex items-center justify-between mb-2">
                                                            <h3 class="font-medium text-gray-900">{job_name}</h3>
                                                            <span class=format!("px-2 py-1 text-xs rounded {}",
                                                                if job_enabled { "bg-green-100 text-green-800" } else { "bg-gray-100 text-gray-800" })>
                                                                {if job_enabled { "Active" } else { "Disabled" }}
                                                            </span>
                                                        </div>
                                                        <div class="text-sm text-gray-600 mb-2">
                                                            {format!("{} VMs", vm_count)}
                                                        </div>
                                                        <div class="text-xs text-gray-500">
                                                            "Schedule: " {schedule}
                                                        </div>
                                                        {next_run.map(|next| view! {
                                                            <div class="text-xs text-gray-500">
                                                                "Next run: " {next}
                                                            </div>
                                                        })}
                                                        {last_status.map(|status| {
                                                            let status_color = get_status_color(&status);
                                                            view! {
                                                                <div class="mt-2">
                                                                    <span class=format!("px-2 py-1 text-xs rounded {}", status_color)>
                                                                        "Last: " {format!("{:?}", status)}
                                                                    </span>
                                                                </div>
                                                            }
                                                        })}
                                                    </div>
                                                }
                                            }).collect::<Vec<_>>()}
                                        </div>
                                    }
                                }}
                            </div>
                        </div>

                        // Quota Status
                        {move || quota_summary.get().map(|summary| view! {
                            <div class="bg-white rounded-lg shadow">
                                <div class="px-6 py-4 border-b border-gray-200">
                                    <div class="flex justify-between items-center">
                                        <h2 class="text-lg font-semibold">Quota Overview</h2>
                                        <a href="/backup/quotas" class="text-blue-600 hover:text-blue-800 text-sm font-medium">
                                            "Manage Quotas"
                                        </a>
                                    </div>
                                </div>
                                <div class="p-6">
                                    <div class="grid grid-cols-1 md:grid-cols-3 gap-6">
                                        <div class="text-center">
                                            <div class="text-2xl font-semibold text-gray-900">{summary.total_quotas}</div>
                                            <div class="text-sm text-gray-500">"Total Quotas"</div>
                                        </div>
                                        <div class="text-center">
                                            <div class="text-2xl font-semibold text-green-600">{summary.active_quotas}</div>
                                            <div class="text-sm text-gray-500">"Active Quotas"</div>
                                        </div>
                                        <div class="text-center">
                                            <div class=format!("text-2xl font-semibold {}",
                                                if summary.quotas_exceeded > 0 { "text-red-600" } else { "text-green-600" })>
                                                {summary.quotas_exceeded}
                                            </div>
                                            <div class="text-sm text-gray-500">"Quotas Exceeded"</div>
                                        </div>
                                    </div>
                                    {if summary.quotas_exceeded > 0 {
                                        view! {
                                            <div class="mt-4 p-3 bg-red-50 border border-red-200 rounded-lg">
                                                <div class="flex items-center">
                                                    <i class="fas fa-exclamation-triangle text-red-500 mr-2"></i>
                                                    <span class="text-red-700 text-sm">
                                                        "Some quotas are exceeded. Consider reviewing your retention policies."
                                                    </span>
                                                </div>
                                            </div>
                                        }
                                    } else {
                                        view! { <div></div> }
                                    }}
                                </div>
                            </div>
                        })}

                        // Templates Overview
                        {move || if !templates.get().is_empty() {
                            view! {
                                <div class="bg-white rounded-lg shadow">
                                    <div class="px-6 py-4 border-b border-gray-200">
                                        <div class="flex justify-between items-center">
                                            <h2 class="text-lg font-semibold">VM Templates</h2>
                                            <a href="/backup/templates" class="text-blue-600 hover:text-blue-800 text-sm font-medium">
                                                "Manage Templates"
                                            </a>
                                        </div>
                                    </div>
                                    <div class="p-6">
                                        <div class="grid grid-cols-1 md:grid-cols-3 lg:grid-cols-4 gap-4">
                                            {templates.get().into_iter().take(8).map(|template| {
                                                let name = template.name.clone();
                                                let desc = template.description.clone();
                                                let size = format_bytes(template.size_mb * 1024 * 1024);
                                                let created = template.created_at.clone();
                                                view! {
                                                    <div class="border border-gray-200 rounded-lg p-4 hover:shadow-md transition-shadow">
                                                        <div class="flex items-center mb-2">
                                                            <i class="fas fa-file-archive text-purple-500 mr-2"></i>
                                                            <h3 class="font-medium text-gray-900 truncate">{name}</h3>
                                                        </div>
                                                        {desc.map(|d| view! {
                                                            <p class="text-sm text-gray-600 mb-2 line-clamp-2">{d}</p>
                                                        })}
                                                        <div class="text-xs text-gray-500">
                                                            "Size: " {size}
                                                        </div>
                                                        <div class="text-xs text-gray-500">
                                                            "Created: " {created}
                                                        </div>
                                                    </div>
                                                }
                                            }).collect::<Vec<_>>()}
                                        </div>
                                    </div>
                                </div>
                            }
                        } else {
                            view! { <div></div> }
                        }}
                    </div>
                }
            }}
        </div>
    }
}