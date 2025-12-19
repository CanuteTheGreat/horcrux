use leptos::*;
use crate::api::{
    BackupJob, SnapshotSchedule, RetentionPolicy,
    get_backup_jobs, get_snapshot_schedules, apply_retention_policy
};

#[component]
pub fn RetentionPoliciesPage() -> impl IntoView {
    let (backup_jobs, set_backup_jobs) = create_signal(Vec::<BackupJob>::new());
    let (snapshot_schedules, set_snapshot_schedules) = create_signal(Vec::<SnapshotSchedule>::new());
    let (loading, set_loading) = create_signal(false);
    let (error, set_error) = create_signal(None::<String>);
    let (success_message, set_success_message) = create_signal(None::<String>);
    let (active_tab, set_active_tab) = create_signal("backup_jobs".to_string());

    let load_data = move || {
        set_loading.set(true);
        set_error.set(None);

        spawn_local(async move {
            let mut errors = Vec::new();

            match get_backup_jobs().await {
                Ok(jobs) => set_backup_jobs.set(jobs),
                Err(e) => errors.push(format!("Failed to load backup jobs: {}", e)),
            }

            match get_snapshot_schedules().await {
                Ok(schedules) => set_snapshot_schedules.set(schedules),
                Err(e) => errors.push(format!("Failed to load snapshot schedules: {}", e)),
            }

            if !errors.is_empty() {
                set_error.set(Some(errors.join("; ")));
            }
            set_loading.set(false);
        });
    };

    // Load data on mount
    create_effect(move |_| {
        load_data();
    });

    let apply_retention = move |target_id: String| {
        spawn_local(async move {
            match apply_retention_policy(&target_id).await {
                Ok(_) => {
                    set_success_message.set(Some("Retention policy applied successfully".to_string()));
                    // Clear success message after 3 seconds
                    set_timeout(
                        move || set_success_message.set(None),
                        std::time::Duration::from_secs(3),
                    );
                }
                Err(e) => set_error.set(Some(format!("Failed to apply retention policy: {}", e))),
            }
        });
    };

    let get_retention_description = move |policy: &RetentionPolicy| -> String {
        let mut parts = Vec::new();

        if let Some(hourly) = policy.keep_hourly {
            if hourly > 0 {
                parts.push(format!("{} hourly", hourly));
            }
        }

        if let Some(daily) = policy.keep_daily {
            if daily > 0 {
                parts.push(format!("{} daily", daily));
            }
        }

        if let Some(weekly) = policy.keep_weekly {
            if weekly > 0 {
                parts.push(format!("{} weekly", weekly));
            }
        }

        if let Some(monthly) = policy.keep_monthly {
            if monthly > 0 {
                parts.push(format!("{} monthly", monthly));
            }
        }

        if let Some(yearly) = policy.keep_yearly {
            if yearly > 0 {
                parts.push(format!("{} yearly", yearly));
            }
        }

        if let Some(max_age) = policy.max_age_days {
            if max_age > 0 {
                parts.push(format!("max {} days", max_age));
            }
        }

        if parts.is_empty() {
            "No retention policy".to_string()
        } else {
            format!("Keep: {}", parts.join(", "))
        }
    };

    let get_policy_health = move |policy: &RetentionPolicy| -> (&str, &str) {
        let has_short_term = policy.keep_hourly.unwrap_or(0) > 0 || policy.keep_daily.unwrap_or(0) > 0;
        let has_long_term = policy.keep_monthly.unwrap_or(0) > 0 || policy.keep_yearly.unwrap_or(0) > 0;
        let has_max_age = policy.max_age_days.is_some();

        match (has_short_term, has_long_term, has_max_age) {
            (true, true, true) => ("Excellent", "bg-green-100 text-green-800"),
            (true, true, false) => ("Good", "bg-blue-100 text-blue-800"),
            (true, false, _) => ("Basic", "bg-yellow-100 text-yellow-800"),
            (false, false, false) => ("None", "bg-red-100 text-red-800"),
            _ => ("Partial", "bg-orange-100 text-orange-800"),
        }
    };

    view! {
        <div class="p-6">
            <div class="flex justify-between items-center mb-6">
                <div>
                    <h1 class="text-2xl font-bold">Retention Policies</h1>
                    <p class="text-gray-600">
                        "Manage backup and snapshot retention policies to automatically clean up old data"
                    </p>
                </div>
                <button
                    on:click=move |_| load_data()
                    class="bg-gray-500 hover:bg-gray-600 text-white px-4 py-2 rounded-lg"
                >
                    <i class="fas fa-sync mr-2"></i>
                    "Refresh"
                </button>
            </div>

            // Success message
            {move || success_message.get().map(|msg| view! {
                <div class="bg-green-50 border border-green-200 text-green-700 px-4 py-3 rounded mb-6">
                    <i class="fas fa-check-circle mr-2"></i>
                    {msg}
                </div>
            })}

            // Error display
            {move || error.get().map(|e| view! {
                <div class="bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded mb-6">
                    <i class="fas fa-exclamation-triangle mr-2"></i>
                    {e}
                </div>
            })}

            // Retention Policy Guide
            <div class="bg-blue-50 border border-blue-200 rounded-lg p-4 mb-6">
                <h3 class="text-lg font-medium text-blue-900 mb-2">
                    <i class="fas fa-info-circle mr-2"></i>
                    "Retention Policy Guidelines"
                </h3>
                <div class="text-sm text-blue-800 space-y-2">
                    <p>
                        <strong>"Grandfather-Father-Son (GFS) Strategy:"</strong>
                        " Keep multiple generations of backups for different time periods:"
                    </p>
                    <ul class="list-disc list-inside ml-4 space-y-1">
                        <li><strong>"Hourly (Son):"</strong> " Recent backups for quick recovery (keep 24-48)"</li>
                        <li><strong>"Daily (Father):"</strong> " Regular backups for short-term recovery (keep 7-30)"</li>
                        <li><strong>"Weekly:"</strong> " Weekly backups for medium-term recovery (keep 4-12)"</li>
                        <li><strong>"Monthly (Grandfather):"</strong> " Long-term archival backups (keep 12-60)"</li>
                        <li><strong>"Max Age:"</strong> " Absolute limit for data retention (90-365+ days)"</li>
                    </ul>
                    <p class="mt-3">
                        <strong>"Best Practice:"</strong>
                        " Use a combination that provides both quick recovery and long-term retention while managing storage costs."
                    </p>
                </div>
            </div>

            // Tab Navigation
            <div class="bg-white rounded-lg shadow mb-6">
                <div class="border-b border-gray-200">
                    <nav class="-mb-px flex">
                        <button
                            on:click=move |_| set_active_tab.set("backup_jobs".to_string())
                            class=format!("py-2 px-4 border-b-2 font-medium text-sm {}",
                                if active_tab.get() == "backup_jobs" {
                                    "border-blue-500 text-blue-600"
                                } else {
                                    "border-transparent text-gray-500 hover:text-gray-700 hover:border-gray-300"
                                })
                        >
                            <i class="fas fa-briefcase mr-2"></i>
                            "Backup Jobs"
                        </button>
                        <button
                            on:click=move |_| set_active_tab.set("snapshot_schedules".to_string())
                            class=format!("py-2 px-4 border-b-2 font-medium text-sm {}",
                                if active_tab.get() == "snapshot_schedules" {
                                    "border-blue-500 text-blue-600"
                                } else {
                                    "border-transparent text-gray-500 hover:text-gray-700 hover:border-gray-300"
                                })
                        >
                            <i class="fas fa-camera mr-2"></i>
                            "Snapshot Schedules"
                        </button>
                    </nav>
                </div>

                <div class="p-6">
                    {move || if loading.get() {
                        view! {
                            <div class="text-center py-8">
                                <i class="fas fa-spinner fa-spin text-2xl text-gray-400 mb-2"></i>
                                <p class="text-gray-600">"Loading retention policies..."</p>
                            </div>
                        }
                    } else {
                        match active_tab.get().as_str() {
                            "backup_jobs" => view! {
                                <div>
                                    <div class="flex justify-between items-center mb-4">
                                        <h2 class="text-lg font-semibold">Backup Job Retention Policies</h2>
                                        <span class="text-sm text-gray-600">
                                            {backup_jobs.get().len()} " jobs configured"
                                        </span>
                                    </div>

                                    {move || if backup_jobs.get().is_empty() {
                                        view! {
                                            <div class="text-center py-8 text-gray-500">
                                                <i class="fas fa-briefcase text-4xl mb-4"></i>
                                                <p class="text-lg">"No Backup Jobs Found"</p>
                                                <p class="text-sm">"Create backup jobs to configure retention policies"</p>
                                                <a href="/backup/jobs" class="inline-block mt-4 bg-blue-500 hover:bg-blue-600 text-white px-4 py-2 rounded-lg">
                                                    "Create Backup Job"
                                                </a>
                                            </div>
                                        }
                                    } else {
                                        view! {
                                            <div class="space-y-4">
                                                {backup_jobs.get().into_iter().map(|job| {
                                                    let job_id = job.id.clone();
                                                    let job_name = job.name.clone();
                                                    let job_enabled = job.enabled;
                                                    let job_desc = job.description.clone();
                                                    let vm_count = job.vm_ids.len();
                                                    let retention_desc = get_retention_description(&job.retention);
                                                    let (health_text, health_color) = get_policy_health(&job.retention);
                                                    let keep_hourly = job.retention.keep_hourly.map_or("0".to_string(), |h| h.to_string());
                                                    let keep_daily = job.retention.keep_daily.map_or("0".to_string(), |d| d.to_string());
                                                    let keep_weekly = job.retention.keep_weekly.map_or("0".to_string(), |w| w.to_string());
                                                    let keep_monthly = job.retention.keep_monthly.map_or("0".to_string(), |m| m.to_string());
                                                    let keep_yearly = job.retention.keep_yearly.map_or("0".to_string(), |y| y.to_string());
                                                    let max_age = job.retention.max_age_days.map_or("Unlimited".to_string(), |age| format!("{}d", age));
                                                    let storage_id = job.target.storage_id.clone();

                                                    view! {
                                                        <div class="border border-gray-200 rounded-lg p-4">
                                                            <div class="flex items-center justify-between">
                                                                <div class="flex-1">
                                                                    <div class="flex items-center mb-2">
                                                                        <h3 class="font-medium text-gray-900 mr-3">{job_name}</h3>
                                                                        <span class=format!("px-2 py-1 text-xs rounded {}",
                                                                            if job_enabled {
                                                                                "bg-green-100 text-green-800"
                                                                            } else {
                                                                                "bg-gray-100 text-gray-800"
                                                                            })>
                                                                            {if job_enabled { "Active" } else { "Disabled" }}
                                                                        </span>
                                                                        <span class=format!("ml-2 px-2 py-1 text-xs rounded {}", health_color)>
                                                                            {health_text}
                                                                        </span>
                                                                    </div>
                                                                    {job_desc.map(|desc| view! {
                                                                        <p class="text-sm text-gray-600 mb-2">{desc}</p>
                                                                    })}
                                                                    <div class="text-sm text-gray-700 mb-2">
                                                                        <strong>"VMs: "</strong>
                                                                        {format!("{} virtual machines", vm_count)}
                                                                    </div>
                                                                    <div class="text-sm text-blue-600 mb-3">
                                                                        <i class="fas fa-clock mr-1"></i>
                                                                        {retention_desc}
                                                                    </div>

                                                                    // Detailed retention breakdown
                                                                    <div class="grid grid-cols-2 md:grid-cols-6 gap-2 text-xs">
                                                                        <div class="bg-gray-50 p-2 rounded text-center">
                                                                            <div class="font-medium text-gray-700">"Hourly"</div>
                                                                            <div class="text-gray-900">{keep_hourly}</div>
                                                                        </div>
                                                                        <div class="bg-gray-50 p-2 rounded text-center">
                                                                            <div class="font-medium text-gray-700">"Daily"</div>
                                                                            <div class="text-gray-900">{keep_daily}</div>
                                                                        </div>
                                                                        <div class="bg-gray-50 p-2 rounded text-center">
                                                                            <div class="font-medium text-gray-700">"Weekly"</div>
                                                                            <div class="text-gray-900">{keep_weekly}</div>
                                                                        </div>
                                                                        <div class="bg-gray-50 p-2 rounded text-center">
                                                                            <div class="font-medium text-gray-700">"Monthly"</div>
                                                                            <div class="text-gray-900">{keep_monthly}</div>
                                                                        </div>
                                                                        <div class="bg-gray-50 p-2 rounded text-center">
                                                                            <div class="font-medium text-gray-700">"Yearly"</div>
                                                                            <div class="text-gray-900">{keep_yearly}</div>
                                                                        </div>
                                                                        <div class="bg-gray-50 p-2 rounded text-center">
                                                                            <div class="font-medium text-gray-700">"Max Age"</div>
                                                                            <div class="text-gray-900">{max_age}</div>
                                                                        </div>
                                                                    </div>
                                                                </div>

                                                                <div class="flex flex-col items-end space-y-2">
                                                                    <button
                                                                        on:click=move |_| apply_retention(storage_id.clone())
                                                                        class="bg-orange-500 hover:bg-orange-600 text-white text-sm px-3 py-1 rounded"
                                                                        title="Apply retention policy now"
                                                                    >
                                                                        <i class="fas fa-broom mr-1"></i>
                                                                        "Clean Now"
                                                                    </button>
                                                                    <a
                                                                        href=format!("/backup/jobs?edit={}", job_id)
                                                                        class="text-blue-600 hover:text-blue-900 text-sm"
                                                                    >
                                                                        "Edit Policy"
                                                                    </a>
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
                            "snapshot_schedules" => view! {
                                <div>
                                    <div class="flex justify-between items-center mb-4">
                                        <h2 class="text-lg font-semibold">Snapshot Schedule Retention Policies</h2>
                                        <span class="text-sm text-gray-600">
                                            {snapshot_schedules.get().len()} " schedules configured"
                                        </span>
                                    </div>

                                    {move || if snapshot_schedules.get().is_empty() {
                                        view! {
                                            <div class="text-center py-8 text-gray-500">
                                                <i class="fas fa-camera text-4xl mb-4"></i>
                                                <p class="text-lg">"No Snapshot Schedules Found"</p>
                                                <p class="text-sm">"Create snapshot schedules to configure retention policies"</p>
                                                <a href="/backup/snapshots" class="inline-block mt-4 bg-blue-500 hover:bg-blue-600 text-white px-4 py-2 rounded-lg">
                                                    "Create Schedule"
                                                </a>
                                            </div>
                                        }
                                    } else {
                                        view! {
                                            <div class="space-y-4">
                                                {snapshot_schedules.get().into_iter().map(|schedule| {
                                                    let schedule_id = schedule.id.clone();
                                                    let schedule_name = schedule.name.clone();
                                                    let schedule_enabled = schedule.enabled;
                                                    let schedule_desc = schedule.description.clone();
                                                    let vm_id = schedule.vm_id.clone();
                                                    let schedule_cron = schedule.schedule.clone();
                                                    let retention_desc = get_retention_description(&schedule.retention_policy);
                                                    let (health_text, health_color) = get_policy_health(&schedule.retention_policy);
                                                    let keep_hourly = schedule.retention_policy.keep_hourly.map_or("0".to_string(), |h| h.to_string());
                                                    let keep_daily = schedule.retention_policy.keep_daily.map_or("0".to_string(), |d| d.to_string());
                                                    let keep_weekly = schedule.retention_policy.keep_weekly.map_or("0".to_string(), |w| w.to_string());
                                                    let keep_monthly = schedule.retention_policy.keep_monthly.map_or("0".to_string(), |m| m.to_string());
                                                    let keep_yearly = schedule.retention_policy.keep_yearly.map_or("0".to_string(), |y| y.to_string());
                                                    let max_age = schedule.retention_policy.max_age_days.map_or("Unlimited".to_string(), |age| format!("{}d", age));

                                                    view! {
                                                        <div class="border border-gray-200 rounded-lg p-4">
                                                            <div class="flex items-center justify-between">
                                                                <div class="flex-1">
                                                                    <div class="flex items-center mb-2">
                                                                        <h3 class="font-medium text-gray-900 mr-3">{schedule_name}</h3>
                                                                        <span class=format!("px-2 py-1 text-xs rounded {}",
                                                                            if schedule_enabled {
                                                                                "bg-green-100 text-green-800"
                                                                            } else {
                                                                                "bg-gray-100 text-gray-800"
                                                                            })>
                                                                            {if schedule_enabled { "Active" } else { "Disabled" }}
                                                                        </span>
                                                                        <span class=format!("ml-2 px-2 py-1 text-xs rounded {}", health_color)>
                                                                            {health_text}
                                                                        </span>
                                                                    </div>
                                                                    {schedule_desc.map(|desc| view! {
                                                                        <p class="text-sm text-gray-600 mb-2">{desc}</p>
                                                                    })}
                                                                    <div class="text-sm text-gray-700 mb-2">
                                                                        <strong>"VM: "</strong>
                                                                        {vm_id}
                                                                    </div>
                                                                    <div class="text-sm text-gray-700 mb-2">
                                                                        <strong>"Schedule: "</strong>
                                                                        {schedule_cron}
                                                                    </div>
                                                                    <div class="text-sm text-blue-600 mb-3">
                                                                        <i class="fas fa-clock mr-1"></i>
                                                                        {retention_desc}
                                                                    </div>

                                                                    // Detailed retention breakdown
                                                                    <div class="grid grid-cols-2 md:grid-cols-6 gap-2 text-xs">
                                                                        <div class="bg-gray-50 p-2 rounded text-center">
                                                                            <div class="font-medium text-gray-700">"Hourly"</div>
                                                                            <div class="text-gray-900">{keep_hourly}</div>
                                                                        </div>
                                                                        <div class="bg-gray-50 p-2 rounded text-center">
                                                                            <div class="font-medium text-gray-700">"Daily"</div>
                                                                            <div class="text-gray-900">{keep_daily}</div>
                                                                        </div>
                                                                        <div class="bg-gray-50 p-2 rounded text-center">
                                                                            <div class="font-medium text-gray-700">"Weekly"</div>
                                                                            <div class="text-gray-900">{keep_weekly}</div>
                                                                        </div>
                                                                        <div class="bg-gray-50 p-2 rounded text-center">
                                                                            <div class="font-medium text-gray-700">"Monthly"</div>
                                                                            <div class="text-gray-900">{keep_monthly}</div>
                                                                        </div>
                                                                        <div class="bg-gray-50 p-2 rounded text-center">
                                                                            <div class="font-medium text-gray-700">"Yearly"</div>
                                                                            <div class="text-gray-900">{keep_yearly}</div>
                                                                        </div>
                                                                        <div class="bg-gray-50 p-2 rounded text-center">
                                                                            <div class="font-medium text-gray-700">"Max Age"</div>
                                                                            <div class="text-gray-900">{max_age}</div>
                                                                        </div>
                                                                    </div>
                                                                </div>

                                                                <div class="flex flex-col items-end space-y-2">
                                                                    <a
                                                                        href=format!("/backup/snapshots?edit={}", schedule_id)
                                                                        class="text-blue-600 hover:text-blue-900 text-sm"
                                                                    >
                                                                        "Edit Policy"
                                                                    </a>
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
                        }
                    }}
                </div>
            </div>

            // Retention Best Practices
            <div class="bg-white rounded-lg shadow">
                <div class="p-6 border-b border-gray-200">
                    <h2 class="text-lg font-semibold">Retention Policy Best Practices</h2>
                </div>
                <div class="p-6">
                    <div class="grid grid-cols-1 md:grid-cols-3 gap-6">
                        <div class="text-center">
                            <div class="bg-green-100 rounded-full w-12 h-12 flex items-center justify-center mx-auto mb-3">
                                <i class="fas fa-trophy text-green-600 text-xl"></i>
                            </div>
                            <h3 class="font-medium text-gray-900 mb-2">"Recommended: GFS Strategy"</h3>
                            <p class="text-sm text-gray-600">
                                "7 daily, 4 weekly, 12 monthly backups with 365-day max age provides excellent coverage for most use cases."
                            </p>
                        </div>
                        <div class="text-center">
                            <div class="bg-blue-100 rounded-full w-12 h-12 flex items-center justify-center mx-auto mb-3">
                                <i class="fas fa-balance-scale text-blue-600 text-xl"></i>
                            </div>
                            <h3 class="font-medium text-gray-900 mb-2">"Balance Cost vs Recovery"</h3>
                            <p class="text-sm text-gray-600">
                                "Consider your RTO/RPO requirements against storage costs. More frequent backups = faster recovery but higher costs."
                            </p>
                        </div>
                        <div class="text-center">
                            <div class="bg-yellow-100 rounded-full w-12 h-12 flex items-center justify-center mx-auto mb-3">
                                <i class="fas fa-exclamation-triangle text-yellow-600 text-xl"></i>
                            </div>
                            <h3 class="font-medium text-gray-900 mb-2">"Monitor Compliance"</h3>
                            <p class="text-sm text-gray-600">
                                "Regularly review retention policies to ensure they meet your business and compliance requirements."
                            </p>
                        </div>
                    </div>

                    <div class="mt-8 bg-gray-50 rounded-lg p-4">
                        <h4 class="font-medium text-gray-900 mb-3">"Common Retention Strategies by Use Case:"</h4>
                        <div class="space-y-3 text-sm">
                            <div class="flex items-start">
                                <span class="font-medium text-gray-700 w-24 flex-shrink-0">"Development:"</span>
                                <span class="text-gray-600">"24 hourly, 7 daily (rapid iteration, short retention)"</span>
                            </div>
                            <div class="flex items-start">
                                <span class="font-medium text-gray-700 w-24 flex-shrink-0">"Production:"</span>
                                <span class="text-gray-600">"7 daily, 4 weekly, 12 monthly, 7 yearly (comprehensive protection)"</span>
                            </div>
                            <div class="flex items-start">
                                <span class="font-medium text-gray-700 w-24 flex-shrink-0">"Archival:"</span>
                                <span class="text-gray-600">"1 monthly, 10 yearly (long-term regulatory compliance)"</span>
                            </div>
                            <div class="flex items-start">
                                <span class="font-medium text-gray-700 w-24 flex-shrink-0">"Testing:"</span>
                                <span class="text-gray-600">"12 hourly, 3 daily (frequent changes, short lifecycle)"</span>
                            </div>
                        </div>
                    </div>
                </div>
            </div>
        </div>
    }
}