//! /backup/jobs: scheduled backup jobs (create, edit, enable/disable, run
//! now, delete) with their last/next run and last error.
use super::common::{confirm, load_guests, non_empty, Guest};
use crate::api::backup as bk;
use leptos::*;

const SCHEDULE_PRESETS: [(&str, &str); 5] = [
    ("0 2 * * *", "daily at 02:00"),
    ("0 */6 * * *", "every 6 hours"),
    ("0 3 * * 0", "Sundays at 03:00"),
    ("0 4 1 * *", "monthly, 1st at 04:00"),
    ("*/15 * * * *", "every 15 minutes"),
];

/// Build the job from the raw form values (kept browser-free for tests).
#[allow(clippy::too_many_arguments)]
pub fn build_job(
    id: &str,
    comment: &str,
    enabled: bool,
    schedule: &str,
    targets: &[String],
    storage: &str,
    mode: &str,
    compression: &str,
    keep: [&str; 6],
    notify: &str,
) -> Result<bk::BackupJob, String> {
    if schedule.trim().is_empty() {
        return Err("Schedule is required".into());
    }
    if targets.is_empty() {
        return Err("Pick at least one VM or container".into());
    }
    Ok(bk::BackupJob {
        id: id.trim().to_string(),
        comment: non_empty(comment),
        enabled,
        schedule: schedule.trim().to_string(),
        targets: targets.to_vec(),
        storage: storage.to_string(),
        mode: mode.to_string(),
        compression: compression.to_string(),
        retention: bk::Retention::from_fields(keep)?,
        notify: Some(notify.to_string()).filter(|n| n != "never"),
        ..Default::default()
    })
}

#[component]
pub fn BackupJobsPage() -> impl IntoView {
    let (jobs, set_jobs) = create_signal(Vec::<bk::BackupJob>::new());
    let (guests, set_guests) = create_signal(Vec::<Guest>::new());
    let (storages, set_storages) = create_signal(Vec::<bk::BackupStorage>::new());
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let (loading, set_loading) = create_signal(true);

    // Form state. `editing` = Some(id) while editing an existing job.
    let (show_form, set_show_form) = create_signal(false);
    let (editing, set_editing) = create_signal(None::<String>);
    let (f_id, set_f_id) = create_signal(String::new());
    let (f_comment, set_f_comment) = create_signal(String::new());
    let (f_enabled, set_f_enabled) = create_signal(true);
    let (f_schedule, set_f_schedule) = create_signal("0 2 * * *".to_string());
    let (f_targets, set_f_targets) = create_signal(Vec::<String>::new());
    let (f_storage, set_f_storage) = create_signal("local".to_string());
    let (f_mode, set_f_mode) = create_signal("snapshot".to_string());
    let (f_comp, set_f_comp) = create_signal("zstd".to_string());
    let (f_notify, set_f_notify) = create_signal("failure".to_string());
    let keep: [(ReadSignal<String>, WriteSignal<String>); 6] =
        std::array::from_fn(|_| create_signal(String::new()));

    let reload = move || {
        spawn_local(async move {
            match bk::list_backup_jobs().await {
                Ok(j) => {
                    set_jobs.set(j);
                    set_error.set(None);
                }
                Err(e) => set_error.set(Some(format!("Loading jobs: {}", e.message))),
            }
            set_loading.set(false);
        });
    };
    create_effect(move |_| {
        reload();
        spawn_local(async move {
            set_guests.set(load_guests().await);
            if let Ok(s) = bk::list_backup_storages().await {
                set_storages.set(s);
            }
        });
    });
    // Keep "last run" fresh while a job runs in the background.
    let interval = set_interval_with_handle(reload, std::time::Duration::from_secs(5)).ok();
    on_cleanup(move || {
        if let Some(h) = interval {
            h.clear();
        }
    });

    let open_new = move |_| {
        set_editing.set(None);
        set_f_id.set(String::new());
        set_f_comment.set(String::new());
        set_f_enabled.set(true);
        set_f_schedule.set("0 2 * * *".into());
        set_f_targets.set(Vec::new());
        set_f_storage.set("local".into());
        set_f_mode.set("snapshot".into());
        set_f_comp.set("zstd".into());
        set_f_notify.set("failure".into());
        for (_, w) in keep.iter() {
            w.set(String::new());
        }
        keep[0].1.set("7".into());
        set_show_form.set(true);
    };

    let open_edit = move |j: bk::BackupJob| {
        set_editing.set(Some(j.id.clone()));
        set_f_id.set(j.id.clone());
        set_f_comment.set(j.comment.clone().unwrap_or_default());
        set_f_enabled.set(j.enabled);
        set_f_schedule.set(j.schedule.clone());
        set_f_targets.set(j.targets.clone());
        set_f_storage.set(if j.storage.is_empty() {
            "local".into()
        } else {
            j.storage.clone()
        });
        set_f_mode.set(j.mode.clone());
        set_f_comp.set(j.compression.clone());
        set_f_notify.set(j.notify.clone().unwrap_or_else(|| "never".into()));
        let r = &j.retention;
        let vals = [
            r.keep_last,
            r.keep_hourly,
            r.keep_daily,
            r.keep_weekly,
            r.keep_monthly,
            r.keep_yearly,
        ];
        for (i, v) in vals.iter().enumerate() {
            keep[i].1.set(v.map(|v| v.to_string()).unwrap_or_default());
        }
        set_show_form.set(true);
    };

    let save = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let k: Vec<String> = keep.iter().map(|(r, _)| r.get()).collect();
        let job = match build_job(
            &f_id.get(),
            &f_comment.get(),
            f_enabled.get(),
            &f_schedule.get(),
            &f_targets.get(),
            &f_storage.get(),
            &f_mode.get(),
            &f_comp.get(),
            [&k[0], &k[1], &k[2], &k[3], &k[4], &k[5]],
            &f_notify.get(),
        ) {
            Ok(j) => j,
            Err(e) => {
                set_error.set(Some(e));
                return;
            }
        };
        let edit = editing.get();
        spawn_local(async move {
            let r = match &edit {
                Some(id) => bk::update_backup_job(id, &job).await,
                None => bk::create_backup_job(&job).await,
            };
            match r {
                Ok(saved) => {
                    set_notice.set(Some(format!(
                        "Job {} saved; next run {}",
                        saved.id,
                        saved
                            .next_run
                            .map(bk::format_time)
                            .unwrap_or_else(|| "-".into())
                    )));
                    set_error.set(None);
                    set_show_form.set(false);
                }
                Err(e) => set_error.set(Some(format!("Saving job: {}", e.message))),
            }
            reload();
        });
    };

    let toggle = move |j: bk::BackupJob| {
        spawn_local(async move {
            let mut j2 = j.clone();
            j2.enabled = !j.enabled;
            if let Err(e) = bk::update_backup_job(&j.id, &j2).await {
                set_error.set(Some(format!("Job {}: {}", j.id, e.message)));
            }
            reload();
        });
    };

    let run_now = move |id: String| {
        spawn_local(async move {
            match bk::run_backup_job(&id).await {
                Ok(_) => set_notice.set(Some(format!(
                    "Job {id} started; its result appears under Last run when it finishes"
                ))),
                Err(e) => set_error.set(Some(format!("Run {id}: {}", e.message))),
            }
            reload();
        });
    };

    let remove = move |id: String| {
        if !confirm(&format!("Delete backup job {id}? Its backups are kept.")) {
            return;
        }
        spawn_local(async move {
            if let Err(e) = bk::delete_backup_job(&id).await {
                set_error.set(Some(format!("Delete {id}: {}", e.message)));
            }
            reload();
        });
    };

    let keep_labels = [
        "Keep last",
        "Keep hourly",
        "Keep daily",
        "Keep weekly",
        "Keep monthly",
        "Keep yearly",
    ];

    view! {
        <div class="backup-jobs">
            <div class="page-header">
                <h1>"Backup Jobs"</h1>
                <div class="header-actions">
                    <a href="/backup" class="btn btn-secondary">"Backups"</a>
                    " "
                    <button id="new-job-btn" class="btn btn-primary" on:click=open_new>"New job"</button>
                </div>
            </div>

            {move || notice.get().map(|n| view! { <div class="alert alert-success" id="job-notice">{n}</div> })}
            {move || error.get().map(|e| view! { <div class="error" id="job-error">{e}</div> })}

            {move || show_form.get().then(|| view! {
                <div class="dashboard-section" id="job-form">
                    <h2>{move || match editing.get() { Some(id) => format!("Edit job {id}"), None => "New backup job".to_string() }}</h2>
                    <form on:submit=save>
                        <div class="form-group">
                            <label for="job-id">"Job id (blank = generated)"</label>
                            <input id="job-id" type="text" prop:value=move || f_id.get()
                                disabled=move || editing.get().is_some()
                                on:input=move |ev| set_f_id.set(event_target_value(&ev))/>
                        </div>
                        <div class="form-group">
                            <label for="job-schedule">"Schedule (cron: minute hour day month weekday, or daily/weekly/...)"</label>
                            <input id="job-schedule" type="text" prop:value=move || f_schedule.get()
                                on:input=move |ev| set_f_schedule.set(event_target_value(&ev))/>
                            <select on:change=move |ev| { let v = event_target_value(&ev); if !v.is_empty() { set_f_schedule.set(v) } }>
                                <option value="">"presets..."</option>
                                {SCHEDULE_PRESETS.iter().map(|(v, l)| view! { <option value=*v>{format!("{l} ({v})")}</option> }).collect_view()}
                            </select>
                        </div>
                        <div class="form-group" id="job-targets">
                            <label>"VMs / containers"</label>
                            {move || guests.get().into_iter().map(|g| {
                                let id = g.id.clone();
                                let id2 = g.id.clone();
                                view! {
                                    <label class="job-target">
                                        <input type="checkbox" value=g.id.clone()
                                            prop:checked=move || f_targets.get().contains(&id)
                                            on:change=move |ev| {
                                                let on = event_target_checked(&ev);
                                                set_f_targets.update(|t| {
                                                    t.retain(|x| x != &id2);
                                                    if on { t.push(id2.clone()); }
                                                });
                                            }/>
                                        {format!(" {}", g.label)}
                                    </label>
                                }
                            }).collect_view()}
                        </div>
                        <div class="form-group">
                            <label for="job-storage">"Storage"</label>
                            <select id="job-storage" on:change=move |ev| set_f_storage.set(event_target_value(&ev))>
                                {move || storages.get().into_iter().map(|s| {
                                    let sel = s.id == f_storage.get();
                                    view! { <option value=s.id.clone() selected=sel>{format!("{} ({})", s.id, s.path)}</option> }
                                }).collect_view()}
                            </select>
                        </div>
                        <div class="form-group">
                            <label for="job-mode">"Mode"</label>
                            <select id="job-mode" on:change=move |ev| set_f_mode.set(event_target_value(&ev))>
                                {["snapshot", "suspend", "stop"].into_iter().map(|m| view! {
                                    <option value=m selected=move || f_mode.get() == m>{m}</option>
                                }).collect_view()}
                            </select>
                        </div>
                        <div class="form-group">
                            <label for="job-compression">"Compression"</label>
                            <select id="job-compression" on:change=move |ev| set_f_comp.set(event_target_value(&ev))>
                                {["zstd", "gzip", "lzo", "none"].into_iter().map(|m| view! {
                                    <option value=m selected=move || f_comp.get() == m>{m}</option>
                                }).collect_view()}
                            </select>
                        </div>
                        <div class="form-group">
                            <label>"Retention (blank = unset; all blank keeps everything)"</label>
                            {keep.iter().enumerate().map(|(i, (r, w))| {
                                let (r, w) = (*r, *w);
                                view! {
                                    <label>{keep_labels[i]}
                                        <input id=format!("job-keep-{i}") type="number" min="0" prop:value=move || r.get()
                                            on:input=move |ev| w.set(event_target_value(&ev))/>
                                    </label>
                                }
                            }).collect_view()}
                        </div>
                        <div class="form-group">
                            <label for="job-notify">"Notify (configured notification channels)"</label>
                            <select id="job-notify" on:change=move |ev| set_f_notify.set(event_target_value(&ev))>
                                {["failure", "always", "never"].into_iter().map(|m| view! {
                                    <option value=m selected=move || f_notify.get() == m>{m}</option>
                                }).collect_view()}
                            </select>
                        </div>
                        <div class="form-group">
                            <label for="job-comment">"Comment"</label>
                            <input id="job-comment" type="text" prop:value=move || f_comment.get()
                                on:input=move |ev| set_f_comment.set(event_target_value(&ev))/>
                        </div>
                        <div class="form-group">
                            <label>
                                <input id="job-enabled" type="checkbox" prop:checked=move || f_enabled.get()
                                    on:change=move |ev| set_f_enabled.set(event_target_checked(&ev))/>
                                " Enabled"
                            </label>
                        </div>
                        <div class="form-actions">
                            <button type="submit" id="job-submit" class="btn btn-primary">"Save job"</button>
                            " "
                            <button type="button" class="btn btn-secondary" on:click=move |_| set_show_form.set(false)>"Cancel"</button>
                        </div>
                    </form>
                </div>
            })}

            {move || {
                if loading.get() {
                    return view! { <p class="loading">"Loading jobs..."</p> }.into_view();
                }
                let list = jobs.get();
                if list.is_empty() {
                    return view! { <p class="no-data">"No backup jobs. Create one to back guests up on a schedule."</p> }.into_view();
                }
                view! {
                    <table class="vm-table" id="job-table">
                        <thead>
                            <tr>
                                <th>"Job"</th>
                                <th>"Schedule"</th>
                                <th>"Targets"</th>
                                <th>"Storage / mode"</th>
                                <th>"Retention"</th>
                                <th>"Last run"</th>
                                <th>"Next run"</th>
                                <th>"Actions"</th>
                            </tr>
                        </thead>
                        <tbody>
                            {list.into_iter().map(|j| {
                                let (ej, tj, rid, did) = (j.clone(), j.clone(), j.id.clone(), j.id.clone());
                                let last = match (j.last_run, j.last_status.as_deref()) {
                                    (Some(t), Some("ok")) => view! { <span class="status-running">{format!("OK {}", bk::format_time(t))}</span> }.into_view(),
                                    (Some(t), _) => view! {
                                        <span class="status-stopped" title=j.last_error.clone().unwrap_or_default()>
                                            {format!("FAILED {}: {}", bk::format_time(t), j.last_error.clone().unwrap_or_default())}
                                        </span>
                                    }.into_view(),
                                    _ => view! { <span class="status-unknown">"never"</span> }.into_view(),
                                };
                                view! {
                                    <tr class="job-row" data-job-id=j.id.clone()>
                                        <td>{j.id.clone()}{j.comment.clone().map(|c| format!(" - {c}")).unwrap_or_default()}
                                            {if j.enabled { "" } else { " (disabled)" }}</td>
                                        <td>{j.schedule.clone()}</td>
                                        <td>{j.targets.join(", ")}</td>
                                        <td>{format!("{} / {} / {}", if j.storage.is_empty() { "local" } else { &j.storage }, j.mode, j.compression)}</td>
                                        <td>{j.retention.summary()}</td>
                                        <td class="job-last-run">{last}</td>
                                        <td>{j.next_run.map(bk::format_time).unwrap_or_else(|| "-".into())}</td>
                                        <td class="actions">
                                            <button class="btn btn-sm btn-primary run-btn" on:click=move |_| run_now(rid.clone())>"Run now"</button>
                                            <button class="btn btn-sm btn-secondary" on:click=move |_| open_edit(ej.clone())>"Edit"</button>
                                            <button class="btn btn-sm btn-secondary" on:click=move |_| toggle(tj.clone())>
                                                {if j.enabled { "Disable" } else { "Enable" }}
                                            </button>
                                            <button class="btn btn-sm btn-danger" on:click=move |_| remove(did.clone())>"Delete"</button>
                                        </td>
                                    </tr>
                                }
                            }).collect_view()}
                        </tbody>
                    </table>
                }.into_view()
            }}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_job_from_form() {
        let j = build_job(
            "",
            " nightly ",
            true,
            "0 2 * * *",
            &["vm1".to_string()],
            "local",
            "stop",
            "zstd",
            ["3", "", "7", "", "", ""],
            "never",
        )
        .unwrap();
        assert_eq!(j.comment.as_deref(), Some("nightly"));
        assert_eq!(j.retention.keep_last, Some(3));
        assert_eq!(j.notify, None);
        let v = serde_json::to_value(&j).unwrap();
        assert!(
            v.get("last_run").is_none(),
            "server-owned fields are not sent"
        );
        assert!(build_job(
            "",
            "",
            true,
            "",
            &["a".into()],
            "",
            "",
            "",
            [""; 6],
            "never"
        )
        .is_err());
        assert!(build_job("", "", true, "daily", &[], "", "", "", [""; 6], "never").is_err());
    }
}
