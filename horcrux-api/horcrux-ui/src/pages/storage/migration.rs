use leptos::*;
use serde::{Deserialize, Serialize};
use crate::api::*;

// Local struct for migration plan preview (not in API)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationPlan {
    pub source_storage: String,
    pub destination_storage: String,
    pub resources: Vec<String>,
    pub total_size: u64,
    pub estimated_time: u64,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
    pub storage_compatibility: bool,
}

#[component]
pub fn StorageMigrationPage() -> impl IntoView {
    let (migration_jobs, set_migration_jobs) = create_signal(Vec::<StorageMigrationJob>::new());
    let (storage_pools, set_storage_pools) = create_signal(Vec::<MigrationStoragePool>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);

    // Wizard states
    let (current_tab, set_current_tab) = create_signal("jobs".to_string()); // jobs, create
    let (wizard_step, set_wizard_step) = create_signal(1);

    // Migration form states
    let (migration_name, set_migration_name) = create_signal(String::new());
    let (source_storage, set_source_storage) = create_signal(String::new());
    let (destination_storage, set_destination_storage) = create_signal(String::new());
    let (selected_resources, set_selected_resources) = create_signal(Vec::<String>::new());
    let (available_resources, set_available_resources) = create_signal(Vec::<MigratableResource>::new());
    let (migration_plan, set_migration_plan) = create_signal(None::<MigrationPlan>);

    // Options
    let (delete_source, set_delete_source) = create_signal(false);
    let (compress_transfer, set_compress_transfer) = create_signal(true);
    let (verify_checksum, set_verify_checksum) = create_signal(true);
    let (throttle_enabled, set_throttle_enabled) = create_signal(false);
    let (throttle_mbps, set_throttle_mbps) = create_signal(100u32);
    let (schedule_enabled, set_schedule_enabled) = create_signal(false);
    let (schedule_time, set_schedule_time) = create_signal(String::new());
    let (priority, set_priority) = create_signal("normal".to_string());

    // Modal states
    let (selected_job, set_selected_job) = create_signal(None::<StorageMigrationJob>);
    let (show_job_detail, set_show_job_detail) = create_signal(false);

    // Load data on mount
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);

            let jobs_result = get_storage_migrations().await;
            let pools_result = get_migration_storage_pools().await;

            match (jobs_result, pools_result) {
                (Ok(jobs), Ok(pools)) => {
                    set_migration_jobs.set(jobs);
                    set_storage_pools.set(pools);
                    set_error.set(None);
                }
                (Err(e), _) | (_, Err(e)) => {
                    set_error.set(Some(format!("Failed to load data: {}", e)));
                }
            }
            set_loading.set(false);
        });
    });

    // Load resources when source storage changes
    create_effect(move |_| {
        let source = source_storage.get();
        if !source.is_empty() {
            spawn_local(async move {
                if let Ok(resources) = get_migratable_resources(&source).await {
                    set_available_resources.set(resources);
                }
            });
        }
    });

    // Plan migration when resources are selected
    let plan_migration = move || {
        if source_storage.get().is_empty() || destination_storage.get().is_empty() || selected_resources.get().is_empty() {
            return;
        }

        // Create a preview of the migration plan locally
        let plan = MigrationPlan {
            source_storage: source_storage.get(),
            destination_storage: destination_storage.get(),
            resources: selected_resources.get(),
            total_size: 0, // Would need to calculate from resources
            estimated_time: 0,
            warnings: vec![],
            errors: vec![],
            storage_compatibility: true,
        };
        set_migration_plan.set(Some(plan));
    };

    // Start migration
    let start_migration = move || {
        let request = CreateMigrationRequest {
            source_pool_id: source_storage.get(),
            target_pool_id: destination_storage.get(),
            resource_ids: selected_resources.get(),
            options: MigrationOptions {
                live_migrate: false,
                verify_data: verify_checksum.get(),
                compress_transfer: compress_transfer.get(),
                bandwidth_limit_mbps: if throttle_enabled.get() { Some(throttle_mbps.get()) } else { None },
                schedule_time: if schedule_enabled.get() { Some(schedule_time.get()) } else { None },
                delete_source_after: delete_source.get(),
                priority: priority.get(),
            },
        };

        spawn_local(async move {
            match create_storage_migration(request).await {
                Ok(_) => {
                    // Reset wizard
                    set_wizard_step.set(1);
                    set_migration_name.set(String::new());
                    set_source_storage.set(String::new());
                    set_destination_storage.set(String::new());
                    set_selected_resources.set(Vec::new());
                    set_migration_plan.set(None);
                    set_current_tab.set("jobs".to_string());

                    // Reload jobs
                    if let Ok(jobs) = get_storage_migrations().await {
                        set_migration_jobs.set(jobs);
                    }
                }
                Err(_) => {
                    // Show error
                }
            }
        });
    };

    // Control migration job
    let control_job = move |job_id: String, action: String| {
        spawn_local(async move {
            let _ = match action.as_str() {
                "pause" => pause_storage_migration(&job_id).await,
                "resume" => resume_storage_migration(&job_id).await,
                "cancel" => cancel_storage_migration(&job_id).await,
                "retry" => retry_storage_migration(&job_id).await,
                _ => Ok(()),
            };

            // Reload jobs
            if let Ok(jobs) = get_storage_migrations().await {
                set_migration_jobs.set(jobs);
            }
        });
    };

    view! {
        <div class="storage-migration-page">
            <div class="page-header">
                <h1 class="page-title">Storage Migration</h1>
                <p class="page-description">
                    Migrate VMs, containers, and volumes between storage pools
                </p>

                <div class="page-tabs">
                    <button
                        class={move || if current_tab.get() == "jobs" { "tab-btn active" } else { "tab-btn" }}
                        on:click=move |_| set_current_tab.set("jobs".to_string())
                    >
                        Migration Jobs
                    </button>
                    <button
                        class={move || if current_tab.get() == "create" { "tab-btn active" } else { "tab-btn" }}
                        on:click=move |_| set_current_tab.set("create".to_string())
                    >
                        New Migration
                    </button>
                </div>
            </div>

            {move || if loading.get() {
                view! {
                    <div class="loading-container">
                        <div class="spinner"></div>
                        <p>Loading storage data...</p>
                    </div>
                }.into_view()
            } else if let Some(err) = error.get() {
                view! {
                    <div class="error-container">
                        <div class="error-message">
                            <h3>Error Loading Data</h3>
                            <p>{err}</p>
                        </div>
                    </div>
                }.into_view()
            } else {
                match current_tab.get().as_str() {
                    "jobs" => view! {
                        <div class="migration-jobs-view">
                            {move || if migration_jobs.get().is_empty() {
                                view! {
                                    <div class="empty-state">
                                        <h3>No Migration Jobs</h3>
                                        <p>Start a new migration to move data between storage pools</p>
                                        <button
                                            class="btn btn-primary"
                                            on:click=move |_| set_current_tab.set("create".to_string())
                                        >
                                            Create Migration
                                        </button>
                                    </div>
                                }.into_view()
                            } else {
                                view! {
                                    <div class="jobs-list">
                                        {migration_jobs.get().into_iter().map(|job| {
                                            let job_id = job.id.clone();
                                            let job_id2 = job.id.clone();
                                            let job_id3 = job.id.clone();
                                            let job_id4 = job.id.clone();
                                            let job_clone = job.clone();
                                            view! {
                                                <div class={format!("migration-job-card status-{}", job.status)}>
                                                    <div class="job-header">
                                                        <h3>{job.id.clone()}</h3>
                                                        <span class={format!("status-badge status-{}", job.status)}>
                                                            {job.status.to_uppercase()}
                                                        </span>
                                                    </div>

                                                    <div class="job-route">
                                                        <span class="source">{job.source_pool.clone()}</span>
                                                        <span class="arrow">"->"</span>
                                                        <span class="destination">{job.target_pool.clone()}</span>
                                                    </div>

                                                    <div class="job-progress">
                                                        <div class="progress-bar">
                                                            <div
                                                                class="progress-fill"
                                                                style={format!("width: {}%", job.progress)}
                                                            ></div>
                                                        </div>
                                                        <div class="progress-text">
                                                            <span>{format!("{:.1}%", job.progress)}</span>
                                                            <span>
                                                                {format_bytes(job.bytes_transferred)} / {format_bytes(job.bytes_total)}
                                                            </span>
                                                            {if let Some(rate) = job.transfer_rate_mbps {
                                                                view! {
                                                                    <span class="transfer-rate">
                                                                        {format!("{:.1} MB/s", rate)}
                                                                    </span>
                                                                }.into_view()
                                                            } else {
                                                                view! { <span></span> }.into_view()
                                                            }}
                                                        </div>
                                                    </div>

                                                    <div class="job-resources">
                                                        <span>
                                                            {job.resources.iter().filter(|r| r.status == "completed").count()} /
                                                            {job.resources.len()} resources migrated
                                                        </span>
                                                        {job.estimated_completion.as_ref().map(|eta| view! {
                                                            <span class="eta">ETA: {eta.clone()}</span>
                                                        })}
                                                    </div>

                                                    <div class="job-actions">
                                                        <button
                                                            class="btn btn-sm btn-secondary"
                                                            on:click=move |_| {
                                                                set_selected_job.set(Some(job_clone.clone()));
                                                                set_show_job_detail.set(true);
                                                            }
                                                        >
                                                            Details
                                                        </button>

                                                        {match job.status.as_str() {
                                                            "running" => view! {
                                                                <button
                                                                    class="btn btn-sm btn-warning"
                                                                    on:click=move |_| control_job(job_id.clone(), "pause".to_string())
                                                                >
                                                                    Pause
                                                                </button>
                                                            }.into_view(),
                                                            "paused" => view! {
                                                                <button
                                                                    class="btn btn-sm btn-primary"
                                                                    on:click=move |_| control_job(job_id2.clone(), "resume".to_string())
                                                                >
                                                                    Resume
                                                                </button>
                                                            }.into_view(),
                                                            "failed" => view! {
                                                                <button
                                                                    class="btn btn-sm btn-primary"
                                                                    on:click=move |_| control_job(job_id3.clone(), "retry".to_string())
                                                                >
                                                                    Retry
                                                                </button>
                                                            }.into_view(),
                                                            _ => view! { <span></span> }.into_view(),
                                                        }}

                                                        {if job.status == "running" || job.status == "paused" || job.status == "pending" {
                                                            view! {
                                                                <button
                                                                    class="btn btn-sm btn-danger"
                                                                    on:click=move |_| control_job(job_id4.clone(), "cancel".to_string())
                                                                >
                                                                    Cancel
                                                                </button>
                                                            }.into_view()
                                                        } else {
                                                            view! { <span></span> }.into_view()
                                                        }}
                                                    </div>
                                                </div>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </div>
                                }.into_view()
                            }}
                        </div>
                    }.into_view(),

                    "create" => view! {
                        <div class="create-migration-view">
                            <div class="wizard-container">
                                // Wizard steps indicator
                                <div class="wizard-steps">
                                    <div class={move || if wizard_step.get() >= 1 { "step active" } else { "step" }}>
                                        <span class="step-number">1</span>
                                        <span class="step-label">Select Storage</span>
                                    </div>
                                    <div class={move || if wizard_step.get() >= 2 { "step active" } else { "step" }}>
                                        <span class="step-number">2</span>
                                        <span class="step-label">Select Resources</span>
                                    </div>
                                    <div class={move || if wizard_step.get() >= 3 { "step active" } else { "step" }}>
                                        <span class="step-number">3</span>
                                        <span class="step-label">Options</span>
                                    </div>
                                    <div class={move || if wizard_step.get() >= 4 { "step active" } else { "step" }}>
                                        <span class="step-number">4</span>
                                        <span class="step-label">Review & Start</span>
                                    </div>
                                </div>

                                // Step 1: Select Storage
                                {move || if wizard_step.get() == 1 {
                                    view! {
                                        <div class="wizard-step-content">
                                            <h2>Select Source and Destination Storage</h2>

                                            <div class="form-group">
                                                <label for="migration-name">Migration Name</label>
                                                <input
                                                    type="text"
                                                    id="migration-name"
                                                    class="form-control"
                                                    placeholder="My Migration"
                                                    prop:value=migration_name
                                                    on:input=move |ev| {
                                                        set_migration_name.set(event_target_value(&ev));
                                                    }
                                                />
                                            </div>

                                            <div class="storage-selection">
                                                <div class="storage-select-group">
                                                    <label>Source Storage</label>
                                                    <div class="storage-cards">
                                                        {storage_pools.get().into_iter().map(|pool| {
                                                            let pool_id = pool.id.clone();
                                                            let pool_id_for_class = pool.id.clone();
                                                            let pool_id_for_click = pool.id.clone();
                                                            let used_pct = if pool.total_bytes > 0 {
                                                                (pool.total_bytes - pool.available_bytes) * 100 / pool.total_bytes
                                                            } else { 0 };
                                                            view! {
                                                                <div
                                                                    class={
                                                                        let pid = pool_id_for_class;
                                                                        move || format!("storage-card {} {}",
                                                                            if source_storage.get() == pid { "selected" } else { "" },
                                                                            if destination_storage.get() == pid { "disabled" } else { "" }
                                                                        )
                                                                    }
                                                                    on:click={
                                                                        let pid = pool_id_for_click;
                                                                        move |_| {
                                                                            if destination_storage.get() != pid {
                                                                                set_source_storage.set(pid.clone());
                                                                            }
                                                                        }
                                                                    }
                                                                >
                                                                    <h4>{pool.name.clone()}</h4>
                                                                    <span class="pool-type">{pool.pool_type.clone()}</span>
                                                                    <div class="pool-usage">
                                                                        <div class="usage-bar">
                                                                            <div
                                                                                class="usage-fill"
                                                                                style={format!("width: {}%", used_pct)}
                                                                            ></div>
                                                                        </div>
                                                                        <span>{format_bytes(pool.available_bytes)} free</span>
                                                                    </div>
                                                                </div>
                                                            }
                                                        }).collect::<Vec<_>>()}
                                                    </div>
                                                </div>

                                                <div class="storage-arrow">"->"</div>

                                                <div class="storage-select-group">
                                                    <label>Destination Storage</label>
                                                    <div class="storage-cards">
                                                        {storage_pools.get().into_iter().map(|pool| {
                                                            let pool_id_for_class = pool.id.clone();
                                                            let pool_id_for_click = pool.id.clone();
                                                            let used_pct = if pool.total_bytes > 0 {
                                                                (pool.total_bytes - pool.available_bytes) * 100 / pool.total_bytes
                                                            } else { 0 };
                                                            view! {
                                                                <div
                                                                    class={
                                                                        let pid = pool_id_for_class;
                                                                        move || format!("storage-card {} {}",
                                                                            if destination_storage.get() == pid { "selected" } else { "" },
                                                                            if source_storage.get() == pid { "disabled" } else { "" }
                                                                        )
                                                                    }
                                                                    on:click={
                                                                        let pid = pool_id_for_click;
                                                                        move |_| {
                                                                            if source_storage.get() != pid {
                                                                                set_destination_storage.set(pid.clone());
                                                                            }
                                                                        }
                                                                    }
                                                                >
                                                                    <h4>{pool.name.clone()}</h4>
                                                                    <span class="pool-type">{pool.pool_type.clone()}</span>
                                                                    <div class="pool-usage">
                                                                        <div class="usage-bar">
                                                                            <div
                                                                                class="usage-fill"
                                                                                style={format!("width: {}%", used_pct)}
                                                                            ></div>
                                                                        </div>
                                                                        <span>{format_bytes(pool.available_bytes)} free</span>
                                                                    </div>
                                                                </div>
                                                            }
                                                        }).collect::<Vec<_>>()}
                                                    </div>
                                                </div>
                                            </div>

                                            <div class="wizard-actions">
                                                <button
                                                    class="btn btn-primary"
                                                    on:click=move |_| set_wizard_step.set(2)
                                                    disabled=move || source_storage.get().is_empty() || destination_storage.get().is_empty()
                                                >
                                                    Next
                                                </button>
                                            </div>
                                        </div>
                                    }.into_view()
                                } else {
                                    view! { <div></div> }.into_view()
                                }}

                                // Step 2: Select Resources
                                {move || if wizard_step.get() == 2 {
                                    view! {
                                        <div class="wizard-step-content">
                                            <h2>Select Resources to Migrate</h2>

                                            <div class="resource-selection">
                                                <div class="selection-header">
                                                    <button
                                                        class="btn btn-sm btn-secondary"
                                                        on:click=move |_| {
                                                            let all_ids: Vec<String> = available_resources.get()
                                                                .iter()
                                                                .map(|r| r.id.clone())
                                                                .collect();
                                                            set_selected_resources.set(all_ids);
                                                        }
                                                    >
                                                        Select All
                                                    </button>
                                                    <button
                                                        class="btn btn-sm btn-secondary"
                                                        on:click=move |_| {
                                                            set_selected_resources.set(Vec::new());
                                                        }
                                                    >
                                                        Clear Selection
                                                    </button>
                                                    <span class="selection-count">
                                                        {move || selected_resources.get().len()} selected
                                                    </span>
                                                </div>

                                                <div class="resources-list">
                                                    {available_resources.get().into_iter().map(|resource| {
                                                        let resource_id = resource.id.clone();
                                                        let resource_id_for_selected = resource.id.clone();
                                                        let resource_id_for_click = resource.id.clone();
                                                        let resource_id_for_checkbox = resource.id.clone();
                                                        let is_selected = move || selected_resources.get().contains(&resource_id_for_selected);
                                                        let is_selected_class = move || selected_resources.get().contains(&resource_id);

                                                        view! {
                                                            <div
                                                                class={move || format!("resource-item {}", if is_selected_class() { "selected" } else { "" })}
                                                                on:click=move |_| {
                                                                    let mut current = selected_resources.get();
                                                                    if current.contains(&resource_id_for_click) {
                                                                        current.retain(|id| id != &resource_id_for_click);
                                                                    } else {
                                                                        current.push(resource_id_for_click.clone());
                                                                    }
                                                                    set_selected_resources.set(current);
                                                                }
                                                            >
                                                                <input
                                                                    type="checkbox"
                                                                    prop:checked=move || selected_resources.get().contains(&resource_id_for_checkbox)
                                                                    on:click=|e| e.stop_propagation()
                                                                />
                                                                <div class="resource-info">
                                                                    <span class="resource-name">{resource.name.clone()}</span>
                                                                    <span class="resource-type">{resource.resource_type.clone()}</span>
                                                                </div>
                                                                <span class="resource-size">{format_bytes(resource.size_bytes)}</span>
                                                            </div>
                                                        }
                                                    }).collect::<Vec<_>>()}
                                                </div>

                                                <div class="selection-summary">
                                                    <span>
                                                        Total size: {move || {
                                                            let total: u64 = available_resources.get()
                                                                .iter()
                                                                .filter(|r| selected_resources.get().contains(&r.id))
                                                                .map(|r| r.size_bytes)
                                                                .sum();
                                                            format_bytes(total)
                                                        }}
                                                    </span>
                                                </div>
                                            </div>

                                            <div class="wizard-actions">
                                                <button
                                                    class="btn btn-secondary"
                                                    on:click=move |_| set_wizard_step.set(1)
                                                >
                                                    Back
                                                </button>
                                                <button
                                                    class="btn btn-primary"
                                                    on:click=move |_| {
                                                        plan_migration();
                                                        set_wizard_step.set(3);
                                                    }
                                                    disabled=move || selected_resources.get().is_empty()
                                                >
                                                    Next
                                                </button>
                                            </div>
                                        </div>
                                    }.into_view()
                                } else {
                                    view! { <div></div> }.into_view()
                                }}

                                // Step 3: Options
                                {move || if wizard_step.get() == 3 {
                                    view! {
                                        <div class="wizard-step-content">
                                            <h2>Migration Options</h2>

                                            <div class="options-grid">
                                                <div class="option-group">
                                                    <h3>Transfer Options</h3>

                                                    <div class="form-group">
                                                        <label class="checkbox-label">
                                                            <input
                                                                type="checkbox"
                                                                prop:checked=compress_transfer
                                                                on:change=move |ev| {
                                                                    set_compress_transfer.set(event_target_checked(&ev));
                                                                }
                                                            />
                                                            Compress data during transfer
                                                        </label>
                                                        <small class="form-text">Reduces network bandwidth but increases CPU usage</small>
                                                    </div>

                                                    <div class="form-group">
                                                        <label class="checkbox-label">
                                                            <input
                                                                type="checkbox"
                                                                prop:checked=verify_checksum
                                                                on:change=move |ev| {
                                                                    set_verify_checksum.set(event_target_checked(&ev));
                                                                }
                                                            />
                                                            Verify data integrity (checksum)
                                                        </label>
                                                        <small class="form-text">Ensures data is transferred correctly</small>
                                                    </div>

                                                    <div class="form-group">
                                                        <label class="checkbox-label">
                                                            <input
                                                                type="checkbox"
                                                                prop:checked=throttle_enabled
                                                                on:change=move |ev| {
                                                                    set_throttle_enabled.set(event_target_checked(&ev));
                                                                }
                                                            />
                                                            Limit transfer speed
                                                        </label>
                                                        {move || if throttle_enabled.get() {
                                                            view! {
                                                                <div class="throttle-input">
                                                                    <input
                                                                        type="number"
                                                                        class="form-control"
                                                                        min="1"
                                                                        max="10000"
                                                                        prop:value=throttle_mbps
                                                                        on:input=move |ev| {
                                                                            if let Ok(val) = event_target_value(&ev).parse::<u32>() {
                                                                                set_throttle_mbps.set(val);
                                                                            }
                                                                        }
                                                                    />
                                                                    <span>MB/s</span>
                                                                </div>
                                                            }.into_view()
                                                        } else {
                                                            view! { <div></div> }.into_view()
                                                        }}
                                                    </div>
                                                </div>

                                                <div class="option-group">
                                                    <h3>Post-Migration</h3>

                                                    <div class="form-group">
                                                        <label class="checkbox-label">
                                                            <input
                                                                type="checkbox"
                                                                prop:checked=delete_source
                                                                on:change=move |ev| {
                                                                    set_delete_source.set(event_target_checked(&ev));
                                                                }
                                                            />
                                                            Delete source data after migration
                                                        </label>
                                                        <small class="form-text warning">
                                                            Warning: Source data will be permanently deleted after successful migration
                                                        </small>
                                                    </div>
                                                </div>

                                                <div class="option-group">
                                                    <h3>Scheduling</h3>

                                                    <div class="form-group">
                                                        <label class="checkbox-label">
                                                            <input
                                                                type="checkbox"
                                                                prop:checked=schedule_enabled
                                                                on:change=move |ev| {
                                                                    set_schedule_enabled.set(event_target_checked(&ev));
                                                                }
                                                            />
                                                            Schedule for later
                                                        </label>
                                                        {move || if schedule_enabled.get() {
                                                            view! {
                                                                <input
                                                                    type="datetime-local"
                                                                    class="form-control"
                                                                    prop:value=schedule_time
                                                                    on:input=move |ev| {
                                                                        set_schedule_time.set(event_target_value(&ev));
                                                                    }
                                                                />
                                                            }.into_view()
                                                        } else {
                                                            view! { <div></div> }.into_view()
                                                        }}
                                                    </div>

                                                    <div class="form-group">
                                                        <label>Priority</label>
                                                        <select
                                                            class="form-control"
                                                            prop:value=priority
                                                            on:change=move |ev| {
                                                                set_priority.set(event_target_value(&ev));
                                                            }
                                                        >
                                                            <option value="low">Low</option>
                                                            <option value="normal">Normal</option>
                                                            <option value="high">High</option>
                                                        </select>
                                                    </div>
                                                </div>
                                            </div>

                                            <div class="wizard-actions">
                                                <button
                                                    class="btn btn-secondary"
                                                    on:click=move |_| set_wizard_step.set(2)
                                                >
                                                    Back
                                                </button>
                                                <button
                                                    class="btn btn-primary"
                                                    on:click=move |_| set_wizard_step.set(4)
                                                >
                                                    Next
                                                </button>
                                            </div>
                                        </div>
                                    }.into_view()
                                } else {
                                    view! { <div></div> }.into_view()
                                }}

                                // Step 4: Review & Start
                                {move || if wizard_step.get() == 4 {
                                    view! {
                                        <div class="wizard-step-content">
                                            <h2>Review and Start Migration</h2>

                                            {migration_plan.get().map(|plan| view! {
                                                <div class="migration-review">
                                                    <div class="review-section">
                                                        <h3>Migration Summary</h3>
                                                        <div class="summary-grid">
                                                            <div class="summary-item">
                                                                <span class="label">Name:</span>
                                                                <span class="value">{migration_name.get()}</span>
                                                            </div>
                                                            <div class="summary-item">
                                                                <span class="label">Source:</span>
                                                                <span class="value">{plan.source_storage.clone()}</span>
                                                            </div>
                                                            <div class="summary-item">
                                                                <span class="label">Destination:</span>
                                                                <span class="value">{plan.destination_storage.clone()}</span>
                                                            </div>
                                                            <div class="summary-item">
                                                                <span class="label">Resources:</span>
                                                                <span class="value">{plan.resources.len().to_string()} items</span>
                                                            </div>
                                                            <div class="summary-item">
                                                                <span class="label">Total Size:</span>
                                                                <span class="value">{format_bytes(plan.total_size)}</span>
                                                            </div>
                                                            <div class="summary-item">
                                                                <span class="label">Estimated Time:</span>
                                                                <span class="value">{format_duration(plan.estimated_time)}</span>
                                                            </div>
                                                        </div>
                                                    </div>

                                                    {if !plan.warnings.is_empty() {
                                                        view! {
                                                            <div class="review-warnings">
                                                                <h4>Warnings</h4>
                                                                <ul>
                                                                    {plan.warnings.iter().map(|warning| view! {
                                                                        <li class="warning">{warning.clone()}</li>
                                                                    }).collect::<Vec<_>>()}
                                                                </ul>
                                                            </div>
                                                        }.into_view()
                                                    } else {
                                                        view! { <div></div> }.into_view()
                                                    }}

                                                    {if !plan.errors.is_empty() {
                                                        view! {
                                                            <div class="review-errors">
                                                                <h4>Errors</h4>
                                                                <ul>
                                                                    {plan.errors.iter().map(|error| view! {
                                                                        <li class="error">{error.clone()}</li>
                                                                    }).collect::<Vec<_>>()}
                                                                </ul>
                                                            </div>
                                                        }.into_view()
                                                    } else {
                                                        view! { <div></div> }.into_view()
                                                    }}

                                                    <div class="compatibility-check">
                                                        <span class={if plan.storage_compatibility { "compatible" } else { "incompatible" }}>
                                                            {if plan.storage_compatibility {
                                                                "[OK] Storage types are compatible"
                                                            } else {
                                                                "[X] Storage types may have compatibility issues"
                                                            }}
                                                        </span>
                                                    </div>
                                                </div>
                                            })}

                                            <div class="wizard-actions">
                                                <button
                                                    class="btn btn-secondary"
                                                    on:click=move |_| set_wizard_step.set(3)
                                                >
                                                    Back
                                                </button>
                                                <button
                                                    class="btn btn-primary"
                                                    on:click=move |_| start_migration()
                                                    disabled=move || migration_plan.get().map(|p| !p.errors.is_empty()).unwrap_or(true)
                                                >
                                                    {move || if schedule_enabled.get() {
                                                        "Schedule Migration"
                                                    } else {
                                                        "Start Migration"
                                                    }}
                                                </button>
                                            </div>
                                        </div>
                                    }.into_view()
                                } else {
                                    view! { <div></div> }.into_view()
                                }}
                            </div>
                        </div>
                    }.into_view(),

                    _ => view! { <div></div> }.into_view()
                }
            }}

            // Job Detail Modal
            {move || if show_job_detail.get() {
                selected_job.get().map(|job| view! {
                    <div class="modal-overlay" on:click=move |_| set_show_job_detail.set(false)>
                        <div class="modal-content job-detail-modal" on:click=|e| e.stop_propagation()>
                            <div class="modal-header">
                                <h2>Migration Job: {job.id.clone()}</h2>
                                <button
                                    class="modal-close"
                                    on:click=move |_| set_show_job_detail.set(false)
                                >
                                    "x"
                                </button>
                            </div>

                            <div class="modal-body">
                                <div class="job-detail-content">
                                    <div class="job-info-section">
                                        <h3>Overview</h3>
                                        <div class="info-grid">
                                            <div class="info-item">
                                                <span class="label">Status:</span>
                                                <span class={format!("value status-{}", job.status)}>
                                                    {job.status.to_uppercase()}
                                                </span>
                                            </div>
                                            <div class="info-item">
                                                <span class="label">Progress:</span>
                                                <span class="value">{format!("{:.1}%", job.progress)}</span>
                                            </div>
                                            <div class="info-item">
                                                <span class="label">Transferred:</span>
                                                <span class="value">
                                                    {format_bytes(job.bytes_transferred)} / {format_bytes(job.bytes_total)}
                                                </span>
                                            </div>
                                            {job.started_at.as_ref().map(|started| view! {
                                                <div class="info-item">
                                                    <span class="label">Started:</span>
                                                    <span class="value">{started.clone()}</span>
                                                </div>
                                            })}
                                        </div>
                                    </div>

                                    <div class="resources-section">
                                        <h3>Resources ({job.resources.len()})</h3>
                                        <table class="resources-table">
                                            <thead>
                                                <tr>
                                                    <th>ID</th>
                                                    <th>Name</th>
                                                    <th>Status</th>
                                                    <th>Progress</th>
                                                </tr>
                                            </thead>
                                            <tbody>
                                                {job.resources.iter().map(|resource| view! {
                                                    <tr>
                                                        <td>{resource.resource_id.clone()}</td>
                                                        <td>{resource.resource_name.clone()}</td>
                                                        <td>
                                                            <span class={format!("status-badge status-{}", resource.status)}>
                                                                {resource.status.to_uppercase()}
                                                            </span>
                                                        </td>
                                                        <td>{format!("{:.1}%", resource.progress)}</td>
                                                    </tr>
                                                }).collect::<Vec<_>>()}
                                            </tbody>
                                        </table>
                                    </div>

                                    {job.error_message.as_ref().map(|error| view! {
                                        <div class="error-section">
                                            <h3>Error</h3>
                                            <pre class="error-message">{error.clone()}</pre>
                                        </div>
                                    })}
                                </div>
                            </div>

                            <div class="modal-footer">
                                <button
                                    class="btn btn-secondary"
                                    on:click=move |_| set_show_job_detail.set(false)
                                >
                                    Close
                                </button>
                            </div>
                        </div>
                    </div>
                })
            } else {
                None
            }}
        </div>
    }
}

// Storage pool info for migration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoragePoolInfo {
    pub id: String,
    pub name: String,
    pub pool_type: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub used_percent: f64,
}

// Helper functions
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.2} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

fn format_duration(seconds: u64) -> String {
    if seconds >= 3600 {
        let hours = seconds / 3600;
        let minutes = (seconds % 3600) / 60;
        format!("{}h {}m", hours, minutes)
    } else if seconds >= 60 {
        let minutes = seconds / 60;
        let secs = seconds % 60;
        format!("{}m {}s", minutes, secs)
    } else {
        format!("{}s", seconds)
    }
}