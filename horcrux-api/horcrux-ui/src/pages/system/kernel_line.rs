//! System > Packages > Kernel line: the stable kernel (default) or the newest
//! kernel the repositories carry (opt-in), capped by the ZFS module. The
//! kernel arrives with the normal update; the reboot is the administrator's.
use crate::api::platform as p;
use leptos::*;

#[component]
pub fn KernelLineCard() -> impl IntoView {
    let (st, set_st) = create_signal(None::<p::KernelLineStatus>);
    let (unavail, set_unavail) = create_signal(None::<crate::api::Unavailable>);
    let (msg, set_msg) = create_signal(None::<(bool, String)>);
    let load = move || {
        spawn_local(async move {
            match p::kernel_line().await {
                Ok(s) => set_st.set(Some(s)),
                Err(e) => set_msg.set(Some((false, e.message))),
            }
        })
    };
    create_effect(move |_| load());
    let fail = move |e: crate::api::ApiError| match e.unavailable() {
        Some(u) => set_unavail.set(Some(u)),
        None => set_msg.set(Some((false, e.message))),
    };
    let set_line = move |line: String| {
        spawn_local(async move {
            match p::set_kernel_line(&line).await {
                Ok(s) => {
                    set_msg.set(Some((true, format!("Kernel line set to {line}."))));
                    set_st.set(Some(s));
                }
                Err(e) => fail(e),
            }
        })
    };
    let apply = move |_| {
        spawn_local(async move {
            match p::apply_kernel_line().await {
                Ok(t) => set_msg.set(Some((
                    true,
                    format!("Update started as task {} (follow it on the Tasks page). Reboot when it is done.", t.upid),
                ))),
                Err(e) => fail(e),
            }
        })
    };
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
                let line = s.line.clone();
                view! {
                    <p class="help-text">"Default follows the distribution's stable kernel. Newest takes the newest kernel the repositories carry, as long as the installed OpenZFS module supports it. The new kernel comes with the next update (nightly, or Update now); the previous kernel stays in the boot menu and the boot environment snapshot taken before the update can be booted. Nothing reboots by itself."</p>
                    <div class="form-row fields">
                        <label><input type="radio" name="kernel-line" id="kernel-line-default" prop:checked=line == "default"
                            on:change=move |_| set_line("default".into())/>{format!(" Default (stable): {}", v(&s.default_version))}</label>
                        <label><input type="radio" name="kernel-line" id="kernel-line-newest" prop:checked=line == "newest"
                            on:change=move |_| set_line("newest".into())/>{format!(" Newest (opt-in): {}", v(&s.newest_version))}</label>
                    </div>
                    {s.held_back.clone().map(|h| view! { <p class="help-text" id="kernel-line-held">{h}</p> })}
                    <p id="kernel-line-running">{format!("Running: {}. Installed: {}.", s.running,
                        if s.installed.is_empty() { "-".to_string() } else { s.installed.join(", ") })}</p>
                    {s.update_pending.clone().map(|u| view! {
                        <p id="kernel-line-pending">{format!("{u} is selected but not installed yet.")}
                            " " <button id="kernel-line-apply" class="btn btn-primary btn-sm" type="button" on:click=apply>"Update now"</button></p>
                    })}
                    {s.reboot_required.then(|| view! { <p class="alert alert-info" id="kernel-line-reboot">"A newer kernel is installed: reboot this node when convenient."</p> })}
                    {(!s.pins_in_sync).then(|| view! { <p class="alert alert-error" id="kernel-line-drift">"The Portage files for this setting were changed outside Horcrux: choose the line again to rewrite them."</p> })}
                }
            })}
        </div>
    }
}
