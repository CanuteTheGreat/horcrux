use leptos::*;
use crate::api;

#[component]
pub fn ReplicationList() -> impl IntoView {
    let (jobs, set_jobs) = create_signal(Vec::<api::ReplicationJob>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);

    // Load replication jobs
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);
            match api::get_replication_jobs().await {
                Ok(job_list) => {
                    set_jobs.set(job_list);
                    set_error.set(None);
                }
                Err(e) => {
                    set_error.set(Some(e.message));
                }
            }
            set_loading.set(false);
        });
    });

    let execute_job = move |job_id: String| {
        spawn_local(async move {
            if let Err(e) = api::execute_replication(&job_id).await {
                logging::log!("Error executing replication: {}", e.message);
            } else {
                logging::log!("Replication started successfully");
            }
        });
    };

    let delete_job = move |job_id: String| {
        if web_sys::window()
            .and_then(|w| w.confirm_with_message(&format!("Delete replication job {}?", job_id)).ok())
            .unwrap_or(false)
        {
            spawn_local(async move {
                if let Err(e) = api::delete_replication(&job_id).await {
                    logging::log!("Error deleting replication: {}", e.message);
                }
            });
        }
    };

    view! {
        <div class="replication-list">
            <div class="page-header">
                <h1>"Replication Jobs"</h1>
                <div class="header-actions">
                    <button class="btn btn-primary">"Create Replication"</button>
                </div>
            </div>

            {move || {
                if loading.get() {
                    view! { <p class="loading">"Loading replication jobs..."</p> }.into_view()
                } else if let Some(err) = error.get() {
                    view! { <p class="error">"Error: " {err}</p> }.into_view()
                } else {
                    let job_list = jobs.get();
                    if job_list.is_empty() {
                        view! {
                            <div class="no-data">
                                <p>"No replication jobs found."</p>
                                <p>"Create a replication job for disaster recovery!"</p>
                            </div>
                        }.into_view()
                    } else {
                        view! {
                            <table class="replication-table">
                                <thead>
                                    <tr>
                                        <th>"ID"</th>
                                        <th>"VM ID"</th>
                                        <th>"Source → Target"</th>
                                        <th>"Schedule"</th>
                                        <th>"Enabled"</th>
                                        <th>"Last Sync"</th>
                                        <th>"Status"</th>
                                        <th>"Actions"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {job_list.into_iter().map(|job| {
                                        let job_id_exec = job.id.clone();
                                        let job_id_delete = job.id.clone();
                                        let status_class = match job.status.as_str() {
                                            "ok" | "success" => "status-success",
                                            "running" | "syncing" => "status-running",
                                            "failed" | "error" => "status-error",
                                            _ => "status-unknown",
                                        };

                                        view! {
                                            <tr>
                                                <td><code>{&job.id}</code></td>
                                                <td>{&job.vm_id}</td>
                                                <td>{format!("{} → {}", job.source_node, job.target_node)}</td>
                                                <td><span class="schedule-badge">{&job.schedule}</span></td>
                                                <td>
                                                    {if job.enabled {
                                                        view! { <span class="badge badge-success">"Enabled"</span> }.into_view()
                                                    } else {
                                                        view! { <span class="badge badge-secondary">"Disabled"</span> }.into_view()
                                                    }}
                                                </td>
                                                <td>
                                                    {job.last_sync.as_ref().map(|t| t.to_string()).unwrap_or_else(|| "Never".to_string())}
                                                </td>
                                                <td><span class={status_class}>{&job.status}</span></td>
                                                <td class="actions">
                                                    <button
                                                        class="btn btn-sm btn-primary"
                                                        on:click=move |_| execute_job(job_id_exec.clone())
                                                    >"Execute Now"</button>
                                                    <button
                                                        class="btn btn-sm btn-danger"
                                                        on:click=move |_| delete_job(job_id_delete.clone())
                                                    >"Delete"</button>
                                                </td>
                                            </tr>
                                        }
                                    }).collect_view()}
                                </tbody>
                            </table>
                        }.into_view()
                    }
                }
            }}
        </div>
    }
}
