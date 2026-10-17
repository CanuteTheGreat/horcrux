use crate::api;
use leptos::*;

#[component]
pub fn ReplicationList() -> impl IntoView {
    let (jobs, set_jobs) = create_signal(Vec::<api::ReplicationJob>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let (show_create, set_show_create) = create_signal(false);
    let (vms, set_vms) = create_signal(Vec::<horcrux_common::VmConfig>::new());
    let (f_vm, set_f_vm) = create_signal(String::new());
    let (f_snapshot, set_f_snapshot) = create_signal(String::new());
    let (f_node, set_f_node) = create_signal(String::new());
    let (f_pool, set_f_pool) = create_signal(String::new());
    let (f_schedule, set_f_schedule) = create_signal("daily".to_string());
    let (f_error, set_f_error) = create_signal(None::<String>);

    let reload = move || {
        spawn_local(async move {
            match api::get_replication_jobs().await {
                Ok(job_list) => {
                    set_jobs.set(job_list);
                    set_error.set(None);
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            set_loading.set(false);
        });
    };

    // Load replication jobs (and VMs for the create form)
    create_effect(move |_| {
        set_loading.set(true);
        reload();
        spawn_local(async move {
            if let Ok(list) = api::get_vms().await {
                set_vms.set(list);
            }
        });
    });

    let execute_job = move |job_id: String| {
        spawn_local(async move {
            match api::execute_replication(&job_id).await {
                Ok(()) => set_notice.set(Some(format!("Replication {} started", job_id))),
                Err(e) => set_error.set(Some(format!("Run failed: {}", e.message))),
            }
            reload();
        });
    };

    let delete_job = move |job_id: String| {
        if web_sys::window()
            .and_then(|w| {
                w.confirm_with_message(&format!("Delete replication job {}?", job_id))
                    .ok()
            })
            .unwrap_or(false)
        {
            spawn_local(async move {
                if let Err(e) = api::delete_replication(&job_id).await {
                    set_error.set(Some(format!("Delete failed: {}", e.message)));
                }
                reload();
            });
        }
    };

    let submit_create = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let req = api::CreateReplicationRequest {
            source_vm_id: f_vm.get(),
            source_snapshot: f_snapshot.get().trim().to_string(),
            target_node: f_node.get().trim().to_string(),
            target_pool: f_pool.get().trim().to_string(),
            schedule: f_schedule.get(),
            retention_count: 7,
            enabled: true,
        };
        if req.source_vm_id.is_empty() || req.target_node.is_empty() || req.target_pool.is_empty() {
            set_f_error.set(Some(
                "VM, target node and target pool are required".to_string(),
            ));
            return;
        }
        spawn_local(async move {
            match api::create_replication(&req).await {
                Ok(_) => {
                    set_show_create.set(false);
                    set_f_error.set(None);
                    reload();
                }
                Err(e) => set_f_error.set(Some(e.message)),
            }
        });
    };

    view! {
        <div class="replication-list">
            <div class="page-header">
                <h1>"Replication Jobs"</h1>
                <div class="header-actions">
                    <button class="btn btn-primary" id="create-replication-btn"
                        on:click=move |_| set_show_create.update(|v| *v = !*v)>"Create Replication"</button>
                </div>
            </div>

            {move || notice.get().map(|n| view! { <div class="alert alert-success">{n}</div> })}

            <Show when=move || show_create.get()>
                <form class="form form-section" id="create-replication-form" on:submit=submit_create>
                    {move || f_error.get().map(|e| view! { <div class="alert alert-error" id="create-replication-error">{e}</div> })}
                    <div class="form-group">
                        <label for="rep-vm">"Source VM"</label>
                        <select id="rep-vm" on:change=move |ev| set_f_vm.set(event_target_value(&ev))>
                            <option value="">"-- select a VM --"</option>
                            {move || vms.get().into_iter().map(|vm| {
                                let label = format!("{} ({})", vm.name, vm.id);
                                view! { <option value=vm.id.clone()>{label}</option> }
                            }).collect_view()}
                        </select>
                    </div>
                    <div class="form-group">
                        <label for="rep-snapshot">"Source snapshot (optional)"</label>
                        <input id="rep-snapshot" type="text" prop:value=f_snapshot
                            on:input=move |ev| set_f_snapshot.set(event_target_value(&ev))/>
                    </div>
                    <div class="form-row">
                        <div class="form-group">
                            <label for="rep-node">"Target node"</label>
                            <input id="rep-node" type="text" placeholder="node-b" prop:value=f_node
                                on:input=move |ev| set_f_node.set(event_target_value(&ev))/>
                        </div>
                        <div class="form-group">
                            <label for="rep-pool">"Target pool"</label>
                            <input id="rep-pool" type="text" placeholder="tank" prop:value=f_pool
                                on:input=move |ev| set_f_pool.set(event_target_value(&ev))/>
                        </div>
                    </div>
                    <div class="form-group">
                        <label for="rep-schedule">"Schedule"</label>
                        <select id="rep-schedule" on:change=move |ev| set_f_schedule.set(event_target_value(&ev))>
                            <option value="hourly">"Hourly"</option>
                            <option value="daily" selected=true>"Daily"</option>
                            <option value="weekly">"Weekly"</option>
                            <option value="manual">"Manual"</option>
                        </select>
                    </div>
                    <button type="submit" class="btn btn-primary" id="rep-submit">"Create"</button>
                </form>
            </Show>

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
                                        <th>"Source "->" Target"</th>
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
                                                <td>{format!("{} -> {}", job.source_node, job.target_node)}</td>
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
