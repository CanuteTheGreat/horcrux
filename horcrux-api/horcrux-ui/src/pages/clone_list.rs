use leptos::*;
use crate::api;

#[component]
pub fn CloneList() -> impl IntoView {
    let (jobs, set_jobs) = create_signal(Vec::<api::CloneJob>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);

    // Load clone jobs
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);
            match api::get_clone_jobs().await {
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

    view! {
        <div class="clone-list">
            <div class="page-header">
                <h1>"Clone Jobs"</h1>
            </div>

            {move || {
                if loading.get() {
                    view! { <p class="loading">"Loading clone jobs..."</p> }.into_view()
                } else if let Some(err) = error.get() {
                    view! { <p class="error">"Error: " {err}</p> }.into_view()
                } else {
                    let job_list = jobs.get();
                    if job_list.is_empty() {
                        view! {
                            <div class="no-data">
                                <p>"No clone jobs found."</p>
                            </div>
                        }.into_view()
                    } else {
                        view! {
                            <table class="clone-table">
                                <thead>
                                    <tr>
                                        <th>"Job ID"</th>
                                        <th>"Source VM"</th>
                                        <th>"Target Name"</th>
                                        <th>"Type"</th>
                                        <th>"Status"</th>
                                        <th>"Progress"</th>
                                        <th>"Created"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {job_list.into_iter().map(|job| {
                                        let progress_pct = (job.progress * 100.0) as u32;
                                        let status_class = match job.status.as_str() {
                                            "completed" => "status-success",
                                            "running" => "status-running",
                                            "failed" => "status-error",
                                            _ => "status-unknown",
                                        };

                                        view! {
                                            <tr>
                                                <td><code>{&job.job_id}</code></td>
                                                <td>{&job.source_vm_id}</td>
                                                <td><strong>{&job.target_vm_name}</strong></td>
                                                <td>{&job.clone_type}</td>
                                                <td><span class={status_class}>{&job.status}</span></td>
                                                <td>
                                                    <div class="progress-bar">
                                                        <div
                                                            class="progress-fill"
                                                            style=format!("width: {}%", progress_pct)
                                                        ></div>
                                                    </div>
                                                    <span class="progress-text">{format!("{}%", progress_pct)}</span>
                                                </td>
                                                <td>{&job.created_at}</td>
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
