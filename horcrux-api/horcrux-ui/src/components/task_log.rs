//! Live task output next to the form that started it (Proxmox shows the task
//! viewer the moment an action starts): status badge, the log tail that
//! follows new lines, and a link to the task on the Tasks page.
use crate::api::{fetch_json, ApiError, TaskLog, TaskRecord};
use leptos::*;

async fn task(upid: &str) -> Result<TaskRecord, ApiError> {
    fetch_json(&format!("/tasks/{upid}")).await
}

async fn log_from(upid: &str, start: usize) -> Result<TaskLog, ApiError> {
    fetch_json(&format!("/tasks/{upid}/log?start={start}")).await
}

/// Badge class + text for a task status (`running`, `ok`, `error`, `stopped`).
pub fn status_badge(status: &str) -> (&'static str, String) {
    match status {
        "running" => ("hx-badge hx-badge-info", "Running".into()),
        "ok" => ("hx-badge hx-badge-ok", "Finished".into()),
        "error" => ("hx-badge hx-badge-bad", "Failed".into()),
        "stopped" => ("hx-badge hx-badge-warn", "Stopped".into()),
        s => ("hx-badge", s.to_string()),
    }
}

#[component]
pub fn TaskLogPanel(
    /// Task to follow.
    upid: String,
    /// Heading above the log.
    #[prop(into)]
    title: String,
    /// Called once with the final status when the task ends.
    #[prop(optional, into)]
    on_done: Option<Callback<String>>,
    /// Poll interval in milliseconds.
    #[prop(default = 2000)]
    poll_ms: u64,
) -> impl IntoView {
    let (lines, set_lines) = create_signal(Vec::<String>::new());
    let (status, set_status) = create_signal("running".to_string());
    let (err, set_err) = create_signal(None::<String>);
    let next = store_value(0usize);
    let finished = store_value(false);
    let busy = store_value(false);
    let pre = create_node_ref::<html::Pre>();
    let u = upid.clone();
    let poll = move || {
        if finished.get_value() || busy.get_value() {
            return;
        }
        busy.set_value(true);
        let u = u.clone();
        spawn_local(async move {
            match log_from(&u, next.get_value()).await {
                Ok(l) => {
                    set_err.set(None);
                    if !l.lines.is_empty() {
                        set_lines.update(|v| v.extend(l.lines));
                    }
                    next.set_value(l.total);
                    if !l.running {
                        let st = task(&u)
                            .await
                            .map(|t| t.status)
                            .unwrap_or_else(|_| "ok".into());
                        finished.set_value(true);
                        set_status.set(st.clone());
                        if let Some(cb) = on_done {
                            cb.call(st);
                        }
                    }
                }
                // transport errors (a node restart, a changed network) are retried
                Err(e) => set_err.set(Some(e.message)),
            }
            busy.set_value(false);
        });
    };
    poll();
    let p2 = poll.clone();
    let h = set_interval_with_handle(p2, std::time::Duration::from_millis(poll_ms)).ok();
    on_cleanup(move || {
        if let Some(h) = h {
            h.clear();
        }
    });
    // follow the tail
    create_effect(move |_| {
        lines.track();
        if let Some(el) = pre.get() {
            el.set_scroll_top(el.scroll_height());
        }
    });
    let href = format!("/system/tasks?upid={upid}");
    view! {
        <section class="hx-tasklog" aria-live="polite">
            <header class="hx-tasklog-head">
                <strong>{title}</strong>
                {move || {
                    let (c, t) = status_badge(&status.get());
                    view! { <span class=c data-status=status.get()>{t}</span> }
                }}
                <a class="hx-tasklog-link" href=href>"Open in Tasks"</a>
            </header>
            {move || err.get().map(|e| view! {
                <p class="help-text">{format!("Reconnecting to the task log: {e}")}</p>
            })}
            <pre class="hx-tasklog-body" node_ref=pre>
                {move || {
                    let l = lines.get();
                    if l.is_empty() { "Waiting for output...".to_string() } else { l.join("\n") }
                }}
            </pre>
        </section>
    }
}
