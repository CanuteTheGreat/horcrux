use yew::prelude::*;
use serde::{Deserialize, Serialize};
use web_sys::HtmlInputElement;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BackupJob {
    pub id: String,
    pub name: String,
    pub target: String,
    pub schedule: String,  // Cron expression
    pub retention_days: u32,
    pub compression: String,
    pub enabled: bool,
    pub last_run: Option<String>,
    pub next_run: Option<String>,
}

#[derive(Properties, PartialEq)]
pub struct Props {}

pub struct BackupSchedulePage {
    jobs: Vec<BackupJob>,
    loading: bool,
    show_create: bool,
    // Form state
    job_name: String,
    job_target: String,
    job_schedule: String,
    job_retention: String,
    job_compression: String,
}

pub enum Msg {
    LoadJobs,
    JobsLoaded(Vec<BackupJob>),
    ToggleCreate,
    UpdateName(String),
    UpdateTarget(String),
    UpdateSchedule(String),
    UpdateRetention(String),
    UpdateCompression(String),
    CreateJob,
    JobCreated,
    ToggleJob(String),
    DeleteJob(String),
    Error(String),
}

impl Component for BackupSchedulePage {
    type Message = Msg;
    type Properties = Props;

    fn create(ctx: &Context<Self>) -> Self {
        ctx.link().send_message(Msg::LoadJobs);

        Self {
            jobs: Vec::new(),
            loading: true,
            show_create: false,
            job_name: String::new(),
            job_target: String::new(),
            job_schedule: "0 2 * * *".to_string(),  // Default: 2 AM daily
            job_retention: "30".to_string(),
            job_compression: "zstd".to_string(),
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            Msg::LoadJobs => {
                self.loading = true;
                let link = ctx.link().clone();

                wasm_bindgen_futures::spawn_local(async move {
                    // Simulate API call
                    let jobs = vec![
                        BackupJob {
                            id: "job1".to_string(),
                            name: "Daily Full Backup".to_string(),
                            target: "All VMs".to_string(),
                            schedule: "0 2 * * *".to_string(),
                            retention_days: 30,
                            compression: "zstd".to_string(),
                            enabled: true,
                            last_run: Some("2025-10-08 02:00".to_string()),
                            next_run: Some("2025-10-09 02:00".to_string()),
                        },
                        BackupJob {
                            id: "job2".to_string(),
                            name: "Weekly Archive".to_string(),
                            target: "Production VMs".to_string(),
                            schedule: "0 3 * * 0".to_string(),
                            retention_days: 90,
                            compression: "gzip".to_string(),
                            enabled: true,
                            last_run: Some("2025-10-06 03:00".to_string()),
                            next_run: Some("2025-10-13 03:00".to_string()),
                        },
                    ];

                    link.send_message(Msg::JobsLoaded(jobs));
                });

                true
            }

            Msg::JobsLoaded(jobs) => {
                self.jobs = jobs;
                self.loading = false;
                true
            }

            Msg::ToggleCreate => {
                self.show_create = !self.show_create;
                if !self.show_create {
                    // Reset form
                    self.job_name.clear();
                    self.job_target.clear();
                    self.job_schedule = "0 2 * * *".to_string();
                    self.job_retention = "30".to_string();
                    self.job_compression = "zstd".to_string();
                }
                true
            }

            Msg::UpdateName(name) => {
                self.job_name = name;
                true
            }

            Msg::UpdateTarget(target) => {
                self.job_target = target;
                true
            }

            Msg::UpdateSchedule(schedule) => {
                self.job_schedule = schedule;
                true
            }

            Msg::UpdateRetention(retention) => {
                self.job_retention = retention;
                true
            }

            Msg::UpdateCompression(compression) => {
                self.job_compression = compression;
                true
            }

            Msg::CreateJob => {
                let link = ctx.link().clone();
                let name = self.job_name.clone();
                let target = self.job_target.clone();
                let schedule = self.job_schedule.clone();
                let retention = self.job_retention.clone();
                let compression = self.job_compression.clone();

                wasm_bindgen_futures::spawn_local(async move {
                    // API call to create job
                    web_sys::console::log_1(&format!(
                        "Creating backup job: {} for {} with schedule {}",
                        name, target, schedule
                    ).into());

                    link.send_message(Msg::JobCreated);
                });

                true
            }

            Msg::JobCreated => {
                self.show_create = false;
                ctx.link().send_message(Msg::LoadJobs);
                true
            }

            Msg::ToggleJob(job_id) => {
                if let Some(job) = self.jobs.iter_mut().find(|j| j.id == job_id) {
                    job.enabled = !job.enabled;

                    let enabled = job.enabled;
                    wasm_bindgen_futures::spawn_local(async move {
                        web_sys::console::log_1(&format!(
                            "Toggled job {}: enabled={}",
                            job_id, enabled
                        ).into());
                    });
                }
                true
            }

            Msg::DeleteJob(job_id) => {
                self.jobs.retain(|j| j.id != job_id);

                wasm_bindgen_futures::spawn_local(async move {
                    web_sys::console::log_1(&format!("Deleted job {}", job_id).into());
                });

                true
            }

            Msg::Error(err) => {
                web_sys::console::error_1(&err.into());
                false
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        html! {
            <div class="container">
                <div class="header">
                    <h1>{ "Backup Scheduling" }</h1>
                    <button
                        class="btn-primary"
                        onclick={ctx.link().callback(|_| Msg::ToggleCreate)}
                    >
                        { if self.show_create { "Cancel" } else { "+ New Backup Job" } }
                    </button>
                </div>

                { self.view_create_form(ctx) }

                { if self.loading {
                    html! { <div class="loading">{ "Loading backup jobs..." }</div> }
                } else {
                    self.view_jobs_list(ctx)
                }}
            </div>
        }
    }
}

impl BackupSchedulePage {
    fn view_create_form(&self, ctx: &Context<Self>) -> Html {
        if !self.show_create {
            return html! {};
        }

        html! {
            <div class="create-form card">
                <h2>{ "Create Backup Job" }</h2>

                <div class="form-group">
                    <label>{ "Job Name" }</label>
                    <input
                        type="text"
                        value={self.job_name.clone()}
                        oninput={ctx.link().callback(|e: InputEvent| {
                            let input: HtmlInputElement = e.target_unchecked_into();
                            Msg::UpdateName(input.value())
                        })}
                        placeholder="e.g., Daily Full Backup"
                    />
                </div>

                <div class="form-group">
                    <label>{ "Target" }</label>
                    <input
                        type="text"
                        value={self.job_target.clone()}
                        oninput={ctx.link().callback(|e: InputEvent| {
                            let input: HtmlInputElement = e.target_unchecked_into();
                            Msg::UpdateTarget(input.value())
                        })}
                        placeholder="VM ID, tag, or 'all'"
                    />
                    <small>{ "VM ID (e.g., 100), tag (e.g., production), or 'all' for all VMs" }</small>
                </div>

                <div class="form-group">
                    <label>{ "Schedule (Cron)" }</label>
                    <input
                        type="text"
                        value={self.job_schedule.clone()}
                        oninput={ctx.link().callback(|e: InputEvent| {
                            let input: HtmlInputElement = e.target_unchecked_into();
                            Msg::UpdateSchedule(input.value())
                        })}
                        placeholder="0 2 * * *"
                    />
                    <small>
                        { "Common schedules: " }
                        <span class="schedule-hint" onclick={ctx.link().callback(|_| Msg::UpdateSchedule("0 2 * * *".to_string()))}>
                            { "Daily 2AM" }
                        </span>
                        { " | " }
                        <span class="schedule-hint" onclick={ctx.link().callback(|_| Msg::UpdateSchedule("0 3 * * 0".to_string()))}>
                            { "Weekly Sunday 3AM" }
                        </span>
                        { " | " }
                        <span class="schedule-hint" onclick={ctx.link().callback(|_| Msg::UpdateSchedule("0 */6 * * *".to_string()))}>
                            { "Every 6 hours" }
                        </span>
                    </small>
                </div>

                <div class="form-row">
                    <div class="form-group">
                        <label>{ "Retention (days)" }</label>
                        <input
                            type="number"
                            value={self.job_retention.clone()}
                            oninput={ctx.link().callback(|e: InputEvent| {
                                let input: HtmlInputElement = e.target_unchecked_into();
                                Msg::UpdateRetention(input.value())
                            })}
                        />
                    </div>

                    <div class="form-group">
                        <label>{ "Compression" }</label>
                        <select
                            value={self.job_compression.clone()}
                            onchange={ctx.link().callback(|e: Event| {
                                let select: HtmlInputElement = e.target_unchecked_into();
                                Msg::UpdateCompression(select.value())
                            })}
                        >
                            <option value="zstd">{ "ZSTD (recommended)" }</option>
                            <option value="gzip">{ "GZIP" }</option>
                            <option value="lz4">{ "LZ4 (fastest)" }</option>
                            <option value="none">{ "None" }</option>
                        </select>
                    </div>
                </div>

                <div class="form-actions">
                    <button
                        class="btn-primary"
                        onclick={ctx.link().callback(|_| Msg::CreateJob)}
                        disabled={self.job_name.is_empty() || self.job_target.is_empty()}
                    >
                        { "Create Job" }
                    </button>
                    <button
                        class="btn-secondary"
                        onclick={ctx.link().callback(|_| Msg::ToggleCreate)}
                    >
                        { "Cancel" }
                    </button>
                </div>
            </div>
        }
    }

    fn view_jobs_list(&self, ctx: &Context<Self>) -> Html {
        if self.jobs.is_empty() {
            return html! {
                <div class="empty-state">
                    <p>{ "No backup jobs configured" }</p>
                    <p>{ "Create your first backup job to automate VM backups" }</p>
                </div>
            };
        }

        html! {
            <div class="jobs-list">
                { for self.jobs.iter().map(|job| self.view_job(ctx, job)) }
            </div>
        }
    }

    fn view_job(&self, ctx: &Context<Self>, job: &BackupJob) -> Html {
        let job_id = job.id.clone();
        let delete_id = job.id.clone();

        html! {
            <div class={classes!("job-card", "card", if !job.enabled { Some("disabled") } else { None })}>
                <div class="job-header">
                    <div class="job-title">
                        <h3>{ &job.name }</h3>
                        <span class={classes!("status-badge", if job.enabled { "active" } else { "inactive" })}>
                            { if job.enabled { "Active" } else { "Disabled" } }
                        </span>
                    </div>
                    <div class="job-actions">
                        <button
                            class="btn-icon"
                            onclick={ctx.link().callback(move |_| Msg::ToggleJob(job_id.clone()))}
                            title={if job.enabled { "Disable" } else { "Enable" }}
                        >
                            { if job.enabled { "⏸" } else { "▶" } }
                        </button>
                        <button
                            class="btn-icon btn-danger"
                            onclick={ctx.link().callback(move |_| Msg::DeleteJob(delete_id.clone()))}
                            title="Delete"
                        >
                            { "🗑" }
                        </button>
                    </div>
                </div>

                <div class="job-details">
                    <div class="detail-row">
                        <span class="label">{ "Target:" }</span>
                        <span class="value">{ &job.target }</span>
                    </div>

                    <div class="detail-row">
                        <span class="label">{ "Schedule:" }</span>
                        <span class="value code">{ &job.schedule }</span>
                    </div>

                    <div class="detail-row">
                        <span class="label">{ "Retention:" }</span>
                        <span class="value">{ format!("{} days", job.retention_days) }</span>
                    </div>

                    <div class="detail-row">
                        <span class="label">{ "Compression:" }</span>
                        <span class="value">{ &job.compression }</span>
                    </div>

                    { if let Some(ref last_run) = job.last_run {
                        html! {
                            <div class="detail-row">
                                <span class="label">{ "Last run:" }</span>
                                <span class="value">{ last_run }</span>
                            </div>
                        }
                    } else { html! {} }}

                    { if let Some(ref next_run) = job.next_run {
                        html! {
                            <div class="detail-row">
                                <span class="label">{ "Next run:" }</span>
                                <span class="value">{ next_run }</span>
                            </div>
                        }
                    } else { html! {} }}
                </div>
            </div>
        }
    }
}
