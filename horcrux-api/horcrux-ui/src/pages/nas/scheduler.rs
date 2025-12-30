//! NAS Scheduler Management Page

use leptos::*;

#[derive(Clone, Debug, serde::Deserialize)]
#[allow(dead_code)]
pub struct ScheduledJob {
    pub id: String,
    pub name: String,
    pub job_type: String,
    pub schedule: String,
    pub enabled: bool,
    pub last_run: Option<i64>,
    pub next_run: Option<i64>,
    pub last_status: Option<String>,
    pub created_at: i64,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct JobHistory {
    pub id: String,
    pub started_at: i64,
    pub completed_at: Option<i64>,
    pub status: String,
    pub error_message: Option<String>,
    pub output: Option<String>,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct SchedulerStatus {
    pub running: bool,
    pub total_jobs: i64,
    pub enabled_jobs: i64,
    pub running_jobs: i64,
    pub recent_failures_24h: i64,
    pub next_scheduled: Option<NextScheduled>,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct NextScheduled {
    pub job_id: String,
    pub job_name: String,
    pub next_run: i64,
}

#[component]
pub fn SchedulerPage() -> impl IntoView {
    let (status, set_status) = create_signal(None::<SchedulerStatus>);
    let (jobs, set_jobs) = create_signal(Vec::<ScheduledJob>::new());
    let (history, set_history) = create_signal(Vec::<JobHistory>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);
    let (active_tab, set_active_tab) = create_signal("jobs".to_string());
    let (show_create_job, set_show_create_job) = create_signal(false);
    let (selected_job_id, set_selected_job_id) = create_signal(None::<String>);

    // Form state for creating jobs
    let (new_job_name, set_new_job_name) = create_signal(String::new());
    let (new_job_type, set_new_job_type) = create_signal("snapshot".to_string());
    let (new_job_schedule, set_new_job_schedule) = create_signal("0 0 * * *".to_string());
    let (new_job_dataset, set_new_job_dataset) = create_signal(String::new());
    let (new_job_pool, set_new_job_pool) = create_signal(String::new());
    let (new_job_task_id, set_new_job_task_id) = create_signal(String::new());

    // Load scheduler data
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);

            // Fetch scheduler status
            match reqwasm::http::Request::get("/api/nas/scheduler/status")
                .send()
                .await
            {
                Ok(resp) => {
                    if resp.ok() {
                        if let Ok(data) = resp.json::<SchedulerStatus>().await {
                            set_status.set(Some(data));
                        }
                    }
                }
                Err(e) => set_error.set(Some(format!("Network error: {}", e))),
            }

            // Fetch jobs
            match reqwasm::http::Request::get("/api/nas/scheduler/jobs")
                .send()
                .await
            {
                Ok(resp) => {
                    if resp.ok() {
                        if let Ok(data) = resp.json::<Vec<ScheduledJob>>().await {
                            set_jobs.set(data);
                        }
                    }
                }
                Err(_) => {}
            }

            set_loading.set(false);
        });
    });

    // Load job history when a job is selected
    create_effect(move |_| {
        if let Some(job_id) = selected_job_id.get() {
            spawn_local(async move {
                match reqwasm::http::Request::get(&format!("/api/nas/scheduler/jobs/{}/history", job_id))
                    .send()
                    .await
                {
                    Ok(resp) => {
                        if resp.ok() {
                            if let Ok(data) = resp.json::<Vec<JobHistory>>().await {
                                set_history.set(data);
                            }
                        }
                    }
                    Err(_) => {}
                }
            });
        }
    });

    let run_job = move |job_id: String| {
        spawn_local(async move {
            let _ = reqwasm::http::Request::post(&format!("/api/nas/scheduler/jobs/{}/run", job_id))
                .send()
                .await;
        });
    };

    let pause_job = move |job_id: String| {
        spawn_local(async move {
            let _ = reqwasm::http::Request::post(&format!("/api/nas/scheduler/jobs/{}/pause", job_id))
                .send()
                .await;
        });
    };

    let resume_job = move |job_id: String| {
        spawn_local(async move {
            let _ = reqwasm::http::Request::post(&format!("/api/nas/scheduler/jobs/{}/resume", job_id))
                .send()
                .await;
        });
    };

    let delete_job = move |job_id: String, job_name: String| {
        if web_sys::window()
            .and_then(|w| w.confirm_with_message(&format!("Delete scheduled job '{}'?", job_name)).ok())
            .unwrap_or(false)
        {
            spawn_local(async move {
                let _ = reqwasm::http::Request::delete(&format!("/api/nas/scheduler/jobs/{}", job_id))
                    .send()
                    .await;
            });
        }
    };

    let submit_create_job = move |_| {
        let name = new_job_name.get();
        let job_type = new_job_type.get();
        let schedule = new_job_schedule.get();
        let dataset = new_job_dataset.get();
        let pool = new_job_pool.get();
        let task_id = new_job_task_id.get();

        spawn_local(async move {
            let mut body = serde_json::json!({
                "name": name,
                "job_type": job_type,
                "schedule": schedule,
            });

            // Add job-type specific fields
            match job_type.as_str() {
                "snapshot" | "retention" => {
                    body["dataset"] = serde_json::Value::String(dataset);
                }
                "scrub" => {
                    body["pool"] = serde_json::Value::String(pool);
                }
                "replication" => {
                    body["task_id"] = serde_json::Value::String(task_id);
                }
                _ => {}
            }

            let _ = reqwasm::http::Request::post("/api/nas/scheduler/jobs")
                .header("Content-Type", "application/json")
                .body(body.to_string())
                .send()
                .await;
        });
        set_show_create_job.set(false);
    };

    view! {
        <div class="scheduler-page">
            <div class="page-header">
                <h1>"Job Scheduler"</h1>
                <p class="subtitle">"Automated task scheduling for NAS operations"</p>
            </div>

            // Scheduler Status Dashboard
            {move || {
                if let Some(st) = status.get() {
                    view! {
                        <div class="status-dashboard">
                            <div class="stat-card">
                                <div class="stat-value">{st.total_jobs}</div>
                                <div class="stat-label">"Total Jobs"</div>
                            </div>
                            <div class="stat-card">
                                <div class="stat-value">{st.enabled_jobs}</div>
                                <div class="stat-label">"Enabled"</div>
                            </div>
                            <div class="stat-card">
                                <div class="stat-value">{st.running_jobs}</div>
                                <div class="stat-label">"Running"</div>
                            </div>
                            <div class="stat-card stat-warning">
                                <div class="stat-value">{st.recent_failures_24h}</div>
                                <div class="stat-label">"Failures (24h)"</div>
                            </div>
                            {st.next_scheduled.as_ref().map(|next| view! {
                                <div class="stat-card stat-info">
                                    <div class="stat-value">{&next.job_name}</div>
                                    <div class="stat-label">"Next Run: " {format_timestamp(next.next_run)}</div>
                                </div>
                            })}
                        </div>
                    }.into_view()
                } else {
                    view! {}.into_view()
                }
            }}

            <div class="tabs">
                <button
                    class={move || if active_tab.get() == "jobs" { "tab active" } else { "tab" }}
                    on:click=move |_| set_active_tab.set("jobs".to_string())
                >"Scheduled Jobs"</button>
                <button
                    class={move || if active_tab.get() == "history" { "tab active" } else { "tab" }}
                    on:click=move |_| set_active_tab.set("history".to_string())
                >"Execution History"</button>
            </div>

            {move || {
                if loading.get() {
                    view! { <p class="loading">"Loading scheduler data..."</p> }.into_view()
                } else if let Some(err) = error.get() {
                    view! { <p class="error">"Error: " {err}</p> }.into_view()
                } else if active_tab.get() == "jobs" {
                    let job_list = jobs.get();
                    view! {
                        <div class="tab-content">
                            <div class="tab-header">
                                <button class="btn btn-primary" on:click=move |_| set_show_create_job.set(true)>
                                    "Create Job"
                                </button>
                            </div>
                            {if job_list.is_empty() {
                                view! {
                                    <div class="no-data">
                                        <p>"No scheduled jobs configured."</p>
                                        <p class="hint">"Create jobs to automate snapshots, replication, scrubs, and more."</p>
                                    </div>
                                }.into_view()
                            } else {
                                view! {
                                    <table class="data-table">
                                        <thead>
                                            <tr>
                                                <th>"Name"</th>
                                                <th>"Type"</th>
                                                <th>"Schedule"</th>
                                                <th>"Status"</th>
                                                <th>"Last Run"</th>
                                                <th>"Next Run"</th>
                                                <th>"Actions"</th>
                                            </tr>
                                        </thead>
                                        <tbody>
                                            {job_list.into_iter().map(|job| {
                                                let job_id = job.id.clone();
                                                let job_id2 = job.id.clone();
                                                let job_id3 = job.id.clone();
                                                let job_id4 = job.id.clone();
                                                let job_id5 = job.id.clone();
                                                let job_name = job.name.clone();
                                                let status_class = if job.enabled { "status-active" } else { "status-inactive" };
                                                let last_status_class = match job.last_status.as_deref() {
                                                    Some("success") => "status-success",
                                                    Some("failed") => "status-error",
                                                    Some("running") => "status-running",
                                                    _ => "status-unknown",
                                                };
                                                view! {
                                                    <tr>
                                                        <td>
                                                            <strong>{&job.name}</strong>
                                                        </td>
                                                        <td>
                                                            <span class="job-type">{format_job_type(&job.job_type)}</span>
                                                        </td>
                                                        <td><code>{&job.schedule}</code></td>
                                                        <td>
                                                            <span class={status_class}>
                                                                {if job.enabled { "Enabled" } else { "Paused" }}
                                                            </span>
                                                            {job.last_status.as_ref().map(|s| view! {
                                                                <span class={format!("last-status {}", last_status_class)}>
                                                                    {s}
                                                                </span>
                                                            })}
                                                        </td>
                                                        <td>{job.last_run.map(format_timestamp).unwrap_or_else(|| "Never".to_string())}</td>
                                                        <td>{job.next_run.map(format_timestamp).unwrap_or_else(|| "-".to_string())}</td>
                                                        <td class="actions">
                                                            <button
                                                                class="btn btn-sm"
                                                                on:click=move |_| run_job(job_id.clone())
                                                                title="Run Now"
                                                            >"Run"</button>
                                                            {if job.enabled {
                                                                view! {
                                                                    <button
                                                                        class="btn btn-sm btn-warning"
                                                                        on:click=move |_| pause_job(job_id2.clone())
                                                                        title="Pause"
                                                                    >"Pause"</button>
                                                                }.into_view()
                                                            } else {
                                                                view! {
                                                                    <button
                                                                        class="btn btn-sm btn-success"
                                                                        on:click=move |_| resume_job(job_id3.clone())
                                                                        title="Resume"
                                                                    >"Resume"</button>
                                                                }.into_view()
                                                            }}
                                                            <button
                                                                class="btn btn-sm"
                                                                on:click=move |_| {
                                                                    set_selected_job_id.set(Some(job_id4.clone()));
                                                                    set_active_tab.set("history".to_string());
                                                                }
                                                                title="View History"
                                                            >"History"</button>
                                                            <button
                                                                class="btn btn-sm btn-danger"
                                                                on:click=move |_| delete_job(job_id5.clone(), job_name.clone())
                                                                title="Delete"
                                                            >"Delete"</button>
                                                        </td>
                                                    </tr>
                                                }
                                            }).collect_view()}
                                        </tbody>
                                    </table>
                                }.into_view()
                            }}
                        </div>
                    }.into_view()
                } else {
                    // History tab
                    let history_list = history.get();
                    view! {
                        <div class="tab-content">
                            <div class="tab-header">
                                <select
                                    on:change=move |e| {
                                        let value = event_target_value(&e);
                                        if value.is_empty() {
                                            set_selected_job_id.set(None);
                                        } else {
                                            set_selected_job_id.set(Some(value));
                                        }
                                    }
                                >
                                    <option value="">"All Jobs"</option>
                                    {jobs.get().into_iter().map(|j| {
                                        let is_selected = selected_job_id.get().as_ref() == Some(&j.id);
                                        view! {
                                            <option value={&j.id} selected=is_selected>{&j.name}</option>
                                        }
                                    }).collect_view()}
                                </select>
                            </div>
                            {if history_list.is_empty() {
                                view! {
                                    <div class="no-data">
                                        <p>"No execution history available."</p>
                                        {selected_job_id.get().is_some().then(|| view! {
                                            <p class="hint">"Run the job to see execution history."</p>
                                        })}
                                    </div>
                                }.into_view()
                            } else {
                                view! {
                                    <table class="data-table">
                                        <thead>
                                            <tr>
                                                <th>"Started"</th>
                                                <th>"Completed"</th>
                                                <th>"Duration"</th>
                                                <th>"Status"</th>
                                                <th>"Error"</th>
                                            </tr>
                                        </thead>
                                        <tbody>
                                            {history_list.into_iter().map(|h| {
                                                let status_class = match h.status.as_str() {
                                                    "success" => "status-success",
                                                    "failed" => "status-error",
                                                    "running" => "status-running",
                                                    "cancelled" => "status-warning",
                                                    _ => "status-unknown",
                                                };
                                                let duration = h.completed_at.map(|c| {
                                                    let secs = c - h.started_at;
                                                    format_duration(secs)
                                                }).unwrap_or_else(|| "-".to_string());
                                                view! {
                                                    <tr>
                                                        <td>{format_timestamp(h.started_at)}</td>
                                                        <td>{h.completed_at.map(format_timestamp).unwrap_or_else(|| "-".to_string())}</td>
                                                        <td>{duration}</td>
                                                        <td>
                                                            <span class={status_class}>{&h.status}</span>
                                                        </td>
                                                        <td class="error-cell">
                                                            {h.error_message.as_ref().map(|e| view! {
                                                                <span class="error-text" title={e.clone()}>{truncate_str(e, 50)}</span>
                                                            })}
                                                        </td>
                                                    </tr>
                                                }
                                            }).collect_view()}
                                        </tbody>
                                    </table>
                                }.into_view()
                            }}
                        </div>
                    }.into_view()
                }
            }}

            // Create Job Modal
            {move || {
                if show_create_job.get() {
                    let current_job_type = new_job_type.get();
                    view! {
                        <div class="modal-overlay" on:click=move |_| set_show_create_job.set(false)>
                            <div class="modal modal-lg" on:click=|e| e.stop_propagation()>
                                <h2>"Create Scheduled Job"</h2>
                                <form on:submit=move |e| {
                                    e.prevent_default();
                                    submit_create_job(());
                                }>
                                    <div class="form-row">
                                        <div class="form-group">
                                            <label>"Job Name"</label>
                                            <input
                                                type="text"
                                                placeholder="Daily Snapshot"
                                                required
                                                on:input=move |e| set_new_job_name.set(event_target_value(&e))
                                            />
                                        </div>
                                        <div class="form-group">
                                            <label>"Job Type"</label>
                                            <select on:change=move |e| set_new_job_type.set(event_target_value(&e))>
                                                <option value="snapshot">"Snapshot"</option>
                                                <option value="retention">"Retention Cleanup"</option>
                                                <option value="replication">"Replication"</option>
                                                <option value="scrub">"Pool Scrub"</option>
                                                <option value="health_check">"Health Check"</option>
                                                <option value="quota_check">"Quota Check"</option>
                                                <option value="smart_check">"SMART Check"</option>
                                            </select>
                                        </div>
                                    </div>

                                    <div class="form-group">
                                        <label>"Schedule (Cron Expression)"</label>
                                        <input
                                            type="text"
                                            placeholder="0 0 * * *"
                                            value=new_job_schedule.get()
                                            required
                                            on:input=move |e| set_new_job_schedule.set(event_target_value(&e))
                                        />
                                        <small>"Format: minute hour day month weekday"</small>
                                        <div class="cron-examples">
                                            <span class="example" on:click=move |_| set_new_job_schedule.set("0 0 * * *".to_string())>"Daily midnight"</span>
                                            <span class="example" on:click=move |_| set_new_job_schedule.set("0 */6 * * *".to_string())>"Every 6 hours"</span>
                                            <span class="example" on:click=move |_| set_new_job_schedule.set("0 2 * * 0".to_string())>"Weekly Sunday 2AM"</span>
                                            <span class="example" on:click=move |_| set_new_job_schedule.set("0 3 1 * *".to_string())>"Monthly 1st 3AM"</span>
                                        </div>
                                    </div>

                                    // Job-type specific fields
                                    {match current_job_type.as_str() {
                                        "snapshot" | "retention" => view! {
                                            <div class="form-group">
                                                <label>"Dataset"</label>
                                                <input
                                                    type="text"
                                                    placeholder="tank/data"
                                                    required
                                                    on:input=move |e| set_new_job_dataset.set(event_target_value(&e))
                                                />
                                            </div>
                                        }.into_view(),
                                        "scrub" => view! {
                                            <div class="form-group">
                                                <label>"Pool"</label>
                                                <input
                                                    type="text"
                                                    placeholder="tank"
                                                    required
                                                    on:input=move |e| set_new_job_pool.set(event_target_value(&e))
                                                />
                                            </div>
                                        }.into_view(),
                                        "replication" => view! {
                                            <div class="form-group">
                                                <label>"Replication Task ID"</label>
                                                <input
                                                    type="text"
                                                    placeholder="task-uuid"
                                                    required
                                                    on:input=move |e| set_new_job_task_id.set(event_target_value(&e))
                                                />
                                            </div>
                                        }.into_view(),
                                        _ => view! {}.into_view(),
                                    }}

                                    <div class="modal-actions">
                                        <button type="button" class="btn" on:click=move |_| set_show_create_job.set(false)>"Cancel"</button>
                                        <button type="submit" class="btn btn-primary">"Create Job"</button>
                                    </div>
                                </form>
                            </div>
                        </div>
                    }.into_view()
                } else {
                    view! {}.into_view()
                }
            }}
        </div>
    }
}

fn format_timestamp(ts: i64) -> String {
    // Simple timestamp formatting - in real app use chrono
    let now = js_sys::Date::now() as i64 / 1000;
    let diff = now - ts;

    if diff < 0 {
        // Future time
        let abs_diff = -diff;
        if abs_diff < 60 {
            format!("in {}s", abs_diff)
        } else if abs_diff < 3600 {
            format!("in {}m", abs_diff / 60)
        } else if abs_diff < 86400 {
            format!("in {}h", abs_diff / 3600)
        } else {
            format!("in {}d", abs_diff / 86400)
        }
    } else if diff < 60 {
        format!("{}s ago", diff)
    } else if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86400 {
        format!("{}h ago", diff / 3600)
    } else {
        format!("{}d ago", diff / 86400)
    }
}

fn format_duration(secs: i64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        let m = secs / 60;
        let s = secs % 60;
        format!("{}m {}s", m, s)
    } else {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        format!("{}h {}m", h, m)
    }
}

fn format_job_type(job_type: &str) -> String {
    // Parse JSON job type if present
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(job_type) {
        if let Some(obj) = parsed.as_object() {
            if obj.contains_key("Snapshot") {
                return "Snapshot".to_string();
            } else if obj.contains_key("RetentionCleanup") {
                return "Retention".to_string();
            } else if obj.contains_key("Replication") {
                return "Replication".to_string();
            } else if obj.contains_key("Scrub") {
                return "Scrub".to_string();
            } else if obj.contains_key("CustomScript") {
                return "Custom".to_string();
            }
        }
    }

    // Fallback to string matching
    match job_type.to_lowercase().as_str() {
        "snapshot" => "Snapshot",
        "retention" | "retention_cleanup" => "Retention",
        "replication" => "Replication",
        "scrub" => "Scrub",
        "health_check" => "Health",
        "quota_check" => "Quota",
        "smart_check" => "SMART",
        "custom" | "custom_script" => "Custom",
        _ => job_type,
    }.to_string()
}

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}
