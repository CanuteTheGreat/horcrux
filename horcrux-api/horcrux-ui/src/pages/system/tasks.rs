//! Task log: every long-running host operation (package installs, syncs,
//! service and configuration changes) with its status and log; a running
//! task can be stopped.
use crate::api::*;
use leptos::*;
use leptos_router::use_query_map;

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

/// "node-shell" -> "Node shell", "net-dhcp" -> "Network DHCP", "pkg-install" -> "Package install".
pub fn task_type_label(t: &str) -> String {
    let words: Vec<String> = t
        .split(['-', '_'])
        .filter(|w| !w.is_empty())
        .enumerate()
        .map(|(i, w)| {
            let known = match w.to_ascii_lowercase().as_str() {
                "vm" | "qm" => Some("VM"),
                "ct" => Some("container"),
                "lxc" => Some("LXC"),
                "dhcp" => Some("DHCP"),
                "dns" => Some("DNS"),
                "iso" => Some("ISO"),
                "ha" => Some("HA"),
                "zfs" => Some("ZFS"),
                "nfs" => Some("NFS"),
                "lvm" => Some("LVM"),
                "acme" => Some("ACME"),
                "net" => Some("network"),
                "pkg" => Some("package"),
                "ceph" => Some("Ceph"),
                _ => None,
            };
            let w = known
                .map(str::to_string)
                .unwrap_or_else(|| w.to_ascii_lowercase());
            if i == 0 {
                let mut c = w.chars();
                c.next()
                    .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                    .unwrap_or_default()
            } else {
                w
            }
        })
        .collect();
    if words.is_empty() {
        "Task".into()
    } else {
        words.join(" ")
    }
}

/// What a person calls a task: "Node shell · admin" (type, then what it acted on, else who ran it),
/// never the raw UPID.
pub fn task_label(task_type: &str, target: Option<&str>, user: &str) -> String {
    let what = target
        .filter(|t| !t.trim().is_empty())
        .unwrap_or(user)
        .trim();
    if what.is_empty() {
        task_type_label(task_type)
    } else {
        format!("{} · {}", task_type_label(task_type), what)
    }
}

/// Name from a UPID alone ("UPID:node:serial:type:target:user:"), before the list has loaded.
pub fn upid_label(upid: &str) -> Option<String> {
    let f: Vec<&str> = upid.split(':').collect();
    if f.len() < 6 || f[0] != "UPID" {
        return None;
    }
    Some(task_label(f[3], Some(f[4]), f[5]))
}

#[component]
pub fn TasksPage() -> impl IntoView {
    let (tasks, set_tasks) = create_signal(Vec::<TaskRecord>::new());
    let (error, set_error) = create_signal(None::<String>);
    // `?status=running` / `?upid=...`: the dashboard links to a filtered list
    // and to one task's log.
    let query = use_query_map();
    let init_status = query
        .get_untracked()
        .get("status")
        .cloned()
        .filter(|s| matches!(s.as_str(), "running" | "ok" | "error" | "stopped"))
        .unwrap_or_default();
    let (filter, set_filter) = create_signal(init_status);
    let (selected, set_selected) = create_signal(query.get_untracked().get("upid").cloned());
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
                        <option value="" selected=move || filter.get() == "">"All"</option>
                        <option value="running" selected=move || filter.get() == "running">"Running"</option>
                        <option value="ok" selected=move || filter.get() == "ok">"Succeeded"</option>
                        <option value="error" selected=move || filter.get() == "error">"Failed"</option>
                        <option value="stopped" selected=move || filter.get() == "stopped">"Stopped"</option>
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
                            // Empty state with the way forward: tasks are created by guest and storage
                            // operations; a filtered empty list offers to show every task instead.
                            return if filter.get().is_empty() {
                                view! { <tr class="empty-tasks"><td colspan="7">
                                    "No tasks yet. Starting, stopping, migrating or backing up a guest records a task here. "
                                    <a href="/vms">"Go to Virtual Machines"</a>
                                </td></tr> }.into_view()
                            } else {
                                view! { <tr class="empty-tasks"><td colspan="7">
                                    "No tasks with this status. "
                                    <a href="/tasks" on:click=move |_| { set_filter.set(String::new()); reload(); }>"Show all tasks"</a>
                                </td></tr> }.into_view()
                            };
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
                                    <td title=t.task_type.clone()>{task_type_label(&t.task_type)}</td>
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
            {move || selected.get().map(|u| {
                // the open task by name (heading and page title); the UPID stays in the URL and a tooltip only
                let name = tasks.with(|ts| ts.iter().find(|t| t.upid == u).map(|t| task_label(&t.task_type, t.target.as_deref(), &t.user)))
                    .or_else(|| upid_label(&u))
                    .unwrap_or_else(|| "Task".into());
                crate::titles::set_subject(&name);
                view! {
                <div class="task-log card" id="task-log">
                    <h3 id="task-log-title" title=u.clone()>"Log: " {name}</h3>
                    <pre class="log-output" style="max-height: 24em; overflow: auto">{move || log.get().join("\n")}</pre>
                    <button class="btn btn-secondary" on:click=move |_| set_selected.set(None)>"Close"</button>
                </div>
            }})}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tasks_are_named_for_people_not_by_upid() {
        assert_eq!(task_type_label("node-shell"), "Node shell");
        assert_eq!(task_type_label("net-dhcp"), "Network DHCP");
        assert_eq!(task_type_label("pkg-install"), "Package install");
        assert_eq!(task_type_label("ceph-pool"), "Ceph pool");
        assert_eq!(task_type_label(""), "Task");
        assert_eq!(
            task_label("node-shell", None, "admin"),
            "Node shell · admin"
        );
        assert_eq!(
            task_label("service", Some("sshd:restart"), "admin"),
            "Service · sshd:restart"
        );
        assert_eq!(
            upid_label("UPID:node1:18A3F00000AB:node-shell:admin:admin:").as_deref(),
            Some("Node shell · admin")
        );
        assert_eq!(upid_label("not-a-upid"), None);
    }
}
