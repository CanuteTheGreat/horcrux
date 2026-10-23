//! Task log: every long-running host operation (package installs, syncs,
//! service and configuration changes) with its status and log; a running
//! task can be stopped.
use crate::api::*;
use leptos::*;

fn fmt_time(t: i64) -> String {
    let d = web_sys::js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(t as f64 * 1000.0));
    let iso: String = d.to_iso_string().into();
    iso.replace('T', " ")
        .trim_end_matches('Z')
        .split('.')
        .next()
        .unwrap_or("")
        .to_string()
}

fn status_class(s: &str) -> &'static str {
    match s {
        "ok" => "text-green-600",
        "running" => "text-blue-600",
        "stopped" | "interrupted" => "text-yellow-600",
        _ => "text-red-600",
    }
}

#[component]
pub fn TasksPage() -> impl IntoView {
    let (tasks, set_tasks) = create_signal(Vec::<TaskRecord>::new());
    let (error, set_error) = create_signal(None::<String>);
    let (filter, set_filter) = create_signal(String::new());
    let (selected, set_selected) = create_signal(None::<String>);
    let (log, set_log) = create_signal(Vec::<String>::new());

    let reload = move || {
        spawn_local(async move {
            match list_tasks(&filter.get_untracked()).await {
                Ok(t) => {
                    set_error.set(None);
                    set_tasks.set(t);
                }
                Err(e) => set_error.set(Some(format!("Failed to load tasks: {}", e))),
            }
            if let Some(u) = selected.get_untracked() {
                if let Ok(l) = get_task_log(&u).await {
                    set_log.set(l.lines);
                }
            }
        });
    };
    create_effect(move |_| {
        filter.track();
        reload();
    });
    let interval = set_interval_with_handle(reload, std::time::Duration::from_secs(3)).ok();
    on_cleanup(move || {
        if let Some(h) = interval {
            h.clear();
        }
    });

    let stop = move |upid: String| {
        spawn_local(async move {
            if let Err(e) = stop_task(&upid).await {
                set_error.set(Some(format!("Cannot stop task: {}", e)));
            }
            reload();
        });
    };

    view! {
        <div class="tasks-page">
            <div class="page-header">
                <h1>"Tasks"</h1>
                <div class="header-actions">
                    <select
                        prop:value=filter
                        on:change=move |ev| set_filter.set(event_target_value(&ev))
                    >
                        <option value="">"All"</option>
                        <option value="running">"Running"</option>
                        <option value="ok">"Succeeded"</option>
                        <option value="error">"Failed"</option>
                        <option value="stopped">"Stopped"</option>
                    </select>
                    <button class="btn btn-secondary" on:click=move |_| reload()>"Refresh"</button>
                </div>
            </div>
            {move || error.get().map(|m| view! { <div class="alert alert-error">{m}</div> })}
            <table class="table">
                <thead>
                    <tr>
                        <th>"Started"</th><th>"Type"</th><th>"Target"</th>
                        <th>"User"</th><th>"Status"</th><th>"Duration"</th><th></th>
                    </tr>
                </thead>
                <tbody>
                    {move || {
                        let rows = tasks.get();
                        if rows.is_empty() {
                            return view! { <tr><td colspan="7">"No tasks yet."</td></tr> }.into_view();
                        }
                        rows.into_iter().map(|t| {
                            let upid = t.upid.clone();
                            let upid2 = t.upid.clone();
                            let upid3 = t.upid.clone();
                            let running = t.status == "running";
                            let dur = t.end_time.map(|e| format!("{} s", e - t.start_time)).unwrap_or_else(|| "-".into());
                            view! {
                                <tr class="task-row" id=upid.clone()>
                                    <td>{fmt_time(t.start_time)}</td>
                                    <td>{t.task_type.clone()}</td>
                                    <td>{t.target.clone().unwrap_or_default()}</td>
                                    <td>{t.user.clone()}</td>
                                    <td class=status_class(&t.status)>{t.status.clone()}</td>
                                    <td>{dur}</td>
                                    <td>
                                        <button class="btn btn-small" on:click=move |_| {
                                            set_selected.set(Some(upid2.clone()));
                                            set_log.set(vec![]);
                                            reload();
                                        }>"Log"</button>
                                        {running.then(|| view! {
                                            <button class="btn btn-small btn-danger" on:click=move |_| stop(upid3.clone())>"Stop"</button>
                                        })}
                                    </td>
                                </tr>
                            }
                        }).collect_view()
                    }}
                </tbody>
            </table>
            {move || selected.get().map(|u| view! {
                <div class="task-log">
                    <h3>"Log of " {u.clone()}</h3>
                    <pre class="log-output" style="max-height: 24em; overflow: auto">{move || log.get().join("\n")}</pre>
                    <button class="btn btn-secondary" on:click=move |_| set_selected.set(None)>"Close"</button>
                </div>
            })}
        </div>
    }
}
