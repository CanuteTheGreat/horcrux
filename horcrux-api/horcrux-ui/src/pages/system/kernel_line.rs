//! System > Packages > Kernel line: the stable kernel (default) or the newest
//! kernel the repositories carry (opt-in), capped by the ZFS module. The
//! kernel arrives with the normal update; the reboot is the administrator's.
use crate::api::platform as p;
use crate::components::{Choice, ChoiceCards, TaskLogPanel};
use leptos::*;

#[component]
pub fn KernelLineCard() -> impl IntoView {
    let (st, set_st) = create_signal(None::<p::KernelLineStatus>);
    let (unavail, set_unavail) = create_signal(None::<crate::api::Unavailable>);
    let (msg, set_msg) = create_signal(None::<(bool, String)>);
    let (task, set_task) = create_signal(None::<String>);
    let line = create_rw_signal(String::new());
    let gen = store_value(0u64);
    let load = move || {
        gen.update_value(|g| *g += 1);
        let mine = gen.get_value();
        spawn_local(async move {
            let r = p::kernel_line().await;
            if gen.get_value() != mine {
                return;
            }
            match r {
                Ok(s) => {
                    line.set(s.line.clone());
                    set_st.set(Some(s));
                }
                Err(e) => match e.unavailable() {
                    Some(u) => set_unavail.set(Some(u)),
                    None => set_msg.set(Some((false, e.message))),
                },
            }
        })
    };
    create_effect(move |_| load());
    let fail = move |e: crate::api::ApiError| match e.unavailable() {
        Some(u) => set_unavail.set(Some(u)),
        None => set_msg.set(Some((false, e.message))),
    };
    let set_line = Callback::new(move |l: String| {
        spawn_local(async move {
            match p::set_kernel_line(&l).await {
                Ok(s) => {
                    let what = if l == "newest" { "Newest" } else { "Default" };
                    set_msg.set(Some((true, format!("Kernel line set to {what}."))));
                    line.set(s.line.clone());
                    set_st.set(Some(s));
                }
                Err(e) => {
                    // put the radio back on the saved value
                    if let Some(s) = st.get_untracked() {
                        line.set(s.line);
                    }
                    fail(e)
                }
            }
        })
    });
    let apply = move |_| {
        spawn_local(async move {
            match p::apply_kernel_line().await {
                Ok(t) => {
                    set_msg.set(None);
                    set_task.set(Some(t.upid));
                }
                Err(e) => fail(e),
            }
        })
    };
    let handled = store_value(String::new());
    let task_done = Callback::new(move |s: String| {
        let u = task.get_untracked().unwrap_or_default();
        if handled.get_value() == u {
            return;
        }
        handled.set_value(u);
        set_msg.set(Some(if s == "ok" {
            (
                true,
                "Update finished. Reboot this node when convenient to run the new kernel."
                    .to_string(),
            )
        } else {
            (
                false,
                "The update did not finish: read its log above.".to_string(),
            )
        }));
        load();
    });
    let v = |o: &Option<String>| {
        o.clone()
            .unwrap_or_else(|| "none in the repositories".into())
    };
    view! {
        <div class="card" id="kernel-line">
            <h2>"Kernel line"</h2>
            {move || msg.get().map(|(ok, m)| view! {
                <div class=if ok { "alert alert-info" } else { "alert alert-error" } id="kernel-line-msg">{m}</div>
            })}
            {move || unavail.get().map(|u| view! {
                <crate::components::UnavailableState info=u on_retry=Callback::new(move |_| load())/>
            })}
            {move || st.get().map(|s| {
                let mut newest_tags = vec![("hx-badge hx-badge-warn", "Opt-in".to_string())];
                if s.held_back.is_some() {
                    newest_tags.push(("hx-badge hx-badge-info", "Capped by OpenZFS".to_string()));
                }
                let choices = vec![
                    Choice { value: "default".into(), title: "Default (stable)".into(),
                        detail: format!("{} - the distribution's stable kernel", v(&s.default_version)),
                        tags: vec![("hx-badge hx-badge-ok", "Recommended".to_string())] },
                    Choice { value: "newest".into(), title: "Newest".into(),
                        detail: format!("{} - the newest kernel the installed OpenZFS module supports", v(&s.newest_version)),
                        tags: newest_tags },
                ];
                let mut kernels = s.installed.clone();
                if !kernels.contains(&s.running) && !s.running.is_empty() {
                    kernels.push(s.running.clone());
                }
                let running = s.running.clone();
                view! {
                    <p class="help-text">"The new kernel comes with the next update (nightly, or Update now). The previous kernel stays in the boot menu and the boot environment snapshot taken before the update can be booted. Nothing reboots by itself."</p>
                    <ChoiceCards name="kernel-line" choices=choices selected=line on_change=set_line/>
                    {s.held_back.clone().map(|h| view! { <p class="help-text" id="kernel-line-held">{h}</p> })}
                    <table class="data-table" id="kernel-line-installed">
                        <thead><tr><th>"Installed kernel"</th><th>"State"</th></tr></thead>
                        <tbody>
                            {kernels.into_iter().map(|k| {
                                let (c, t) = if k == running { ("hx-badge hx-badge-ok", "Running") } else { ("hx-badge", "Installed") };
                                view! { <tr><td>{k}</td><td><span class=c>{t}</span></td></tr> }
                            }).collect_view()}
                        </tbody>
                    </table>
                    {s.update_pending.clone().map(|u| view! {
                        <div class="hx-actions" id="kernel-line-pending">
                            {move || if task.get().is_some_and(|t| handled.get_value() != t) {
                                view! { <span>{format!("Installing {u}: follow the log below.")}</span> }.into_view()
                            } else {
                                let u = u.clone();
                                view! {
                                    <span>{format!("{u} is selected but not installed yet.")}</span>
                                    <button id="kernel-line-apply" class="btn btn-primary btn-sm" type="button" on:click=apply>"Update now"</button>
                                }.into_view()
                            }}
                        </div>
                    })}
                    {s.reboot_required.then(|| view! { <p class="alert alert-info" id="kernel-line-reboot">"A newer kernel is installed: reboot this node when convenient."</p> })}
                    {(!s.pins_in_sync).then(|| view! { <p class="alert alert-error" id="kernel-line-drift">"The Portage files for this setting were changed outside Horcrux: choose the line again to rewrite them."</p> })}
                }
            })}
            {move || task.get().map(|u| view! { <TaskLogPanel upid=u title="Kernel update" on_done=task_done/> })}
        </div>
    }
}
