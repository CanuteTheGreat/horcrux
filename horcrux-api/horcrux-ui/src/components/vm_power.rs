//! One power toolbar for a VM, shown wherever a VM page can be in a state the
//! user may want to change (Summary, Hardware): every action the current state
//! allows, nothing it doesn't, and the result or error right next to the
//! buttons. Proxmox shows the same toolbar on every guest view; before this,
//! the Summary page had no Start at all and the Hardware page offered only
//! Shutdown/Reboot/Reset (all disabled for a stopped VM), so the user had to go
//! back to the VM list to start it.
use crate::api;
use horcrux_common::{VmConfig, VmStatus};
use leptos::*;

/// Which actions a VM in `status` offers (in toolbar order).
pub fn power_actions(status: Option<&VmStatus>) -> &'static [&'static str] {
    match status {
        Some(VmStatus::Stopped) => &["Start"],
        Some(VmStatus::Running) => &["Shutdown", "Reboot", "Pause", "Stop", "Reset"],
        Some(VmStatus::Paused) => &["Resume", "Stop"],
        // Unknown: let the user try to get it to a known state
        Some(VmStatus::Unknown) => &["Start", "Stop"],
        None => &[],
    }
}

fn button_id(action: &str) -> &'static str {
    match action {
        "Start" => "vm-start",
        "Shutdown" => "vm-shutdown",
        "Reboot" => "vm-reboot",
        "Pause" => "vm-pause",
        "Resume" => "vm-resume",
        "Stop" => "vm-stop",
        _ => "vm-reset",
    }
}

fn button_class(action: &str) -> &'static str {
    match action {
        "Start" | "Resume" => "btn btn-primary",
        "Stop" | "Reset" => "btn btn-danger",
        "Shutdown" => "btn btn-warning",
        _ => "btn btn-secondary",
    }
}

/// Confirmation text for actions that lose guest state, `None` for the rest.
pub fn confirm_text(action: &str, name: &str) -> Option<String> {
    match action {
        "Stop" => Some(format!(
            "Stop {name} now? This is like pulling the power cord: unsaved work in the guest is lost."
        )),
        "Reset" => Some(format!(
            "Reset {name} now? This is like pressing the reset button: unsaved work in the guest is lost."
        )),
        _ => None,
    }
}

#[component]
pub fn VmPowerBar(
    /// The VM id.
    #[prop(into)]
    vm_id: Signal<String>,
    /// The VM as last loaded (`None` while loading).
    #[prop(into)]
    vm: Signal<Option<VmConfig>>,
    /// Called with the VM after an action changed it.
    #[prop(into)]
    on_change: Callback<VmConfig>,
) -> impl IntoView {
    let (busy, set_busy) = create_signal(None::<&'static str>);
    let (msg, set_msg) = create_signal(None::<(bool, String)>); // (is_error, text)

    let run = move |action: &'static str| {
        let id = vm_id.get_untracked();
        let name = vm
            .get_untracked()
            .map(|v| v.name)
            .unwrap_or_else(|| id.clone());
        if let Some(q) = confirm_text(action, &name) {
            let ok = web_sys::window()
                .and_then(|w| w.confirm_with_message(&q).ok())
                .unwrap_or(false);
            if !ok {
                return;
            }
        }
        set_busy.set(Some(action));
        set_msg.set(None);
        spawn_local(async move {
            let r: Result<Option<VmConfig>, api::ApiError> = match action {
                "Start" => api::start_vm(&id).await.map(|_| None),
                "Stop" => api::stop_vm(&id).await.map(|_| None),
                "Shutdown" => api::shutdown_vm(&id, 60, false).await.map(Some),
                "Reboot" => api::reboot_vm(&id).await.map(Some),
                "Pause" => api::pause_vm(&id).await.map(Some),
                "Resume" => api::resume_vm(&id).await.map(Some),
                _ => api::reset_vm(&id).await.map(Some),
            };
            let r = match r {
                Ok(Some(v)) => Ok(v),
                Ok(None) => api::get_vm(&id).await,
                Err(e) => Err(e),
            };
            match r {
                Ok(v) => {
                    set_msg.set(Some((
                        false,
                        format!("{action}: the VM is now {}.", v.status),
                    )));
                    on_change.call(v);
                }
                Err(e) => set_msg.set(Some((true, format!("{action} failed: {}", e.message)))),
            }
            set_busy.set(None);
        });
    };

    view! {
        <div class="form-actions vm-power-bar" id="vm-power" role="toolbar" aria-label="VM power">
            {move || {
                let v = vm.get();
                let actions = power_actions(v.as_ref().map(|v| &v.status));
                actions
                    .iter()
                    .map(|&a| {
                        let label = move || match busy.get() {
                            Some(b) if b == a => format!("{a}..."),
                            _ => a.to_string(),
                        };
                        view! {
                            <button type="button" class=button_class(a) id=button_id(a)
                                disabled=move || busy.get().is_some()
                                on:click=move |_| run(a)>{label}</button>
                        }
                    })
                    .collect_view()
            }}
            {move || msg.get().map(|(err, t)| {
                let cls = if err { "alert alert-error" } else { "form-help" };
                view! { <span class=cls id="vm-power-msg" role="status">{t}</span> }
            })}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_state_offers_a_way_out_and_nothing_impossible() {
        assert_eq!(power_actions(Some(&VmStatus::Stopped)), &["Start"]);
        let run = power_actions(Some(&VmStatus::Running));
        assert!(!run.contains(&"Start") && run.contains(&"Stop") && run.contains(&"Shutdown"));
        let paused = power_actions(Some(&VmStatus::Paused));
        assert!(paused.contains(&"Resume") && !paused.contains(&"Start"));
        assert!(power_actions(None).is_empty());
        assert!(confirm_text("Stop", "web").unwrap().contains("web"));
        assert!(confirm_text("Start", "web").is_none());
    }
}
