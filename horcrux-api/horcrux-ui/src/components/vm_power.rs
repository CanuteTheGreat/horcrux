//! VM power controls shared by every VM page (list rows, the VM's own pages,
//! "imported" notices): Start / Shutdown / Reboot / Stop with the same
//! confirmations Proxmox asks for, a busy state while the action runs, the
//! VM's state polled until it really changed, and friendly errors.
//! A page that says "start it" always has this control next to the text.
use crate::api;
use horcrux_common::{VmConfig, VmStatus};
use leptos::*;

/// A power action on a VM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerAction {
    Start,
    Shutdown,
    Reboot,
    Stop,
    Pause,
    Resume,
    Reset,
}

impl PowerAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Start => "Start",
            Self::Shutdown => "Shutdown",
            Self::Reboot => "Reboot",
            Self::Stop => "Stop",
            Self::Pause => "Pause",
            Self::Resume => "Resume",
            Self::Reset => "Reset",
        }
    }

    /// Whether the action makes sense in `status` (the button is disabled otherwise).
    pub fn allowed(self, status: &VmStatus) -> bool {
        match self {
            Self::Start => matches!(status, VmStatus::Stopped | VmStatus::Unknown),
            Self::Shutdown | Self::Reboot | Self::Pause | Self::Reset => {
                matches!(status, VmStatus::Running)
            }
            Self::Resume => matches!(status, VmStatus::Paused),
            Self::Stop => matches!(status, VmStatus::Running | VmStatus::Paused),
        }
    }

    /// Text shown while the action runs ("Starting kali...").
    pub fn progress(self, name: &str) -> String {
        match self {
            Self::Start => format!("Starting {name}..."),
            Self::Shutdown => format!("Shutting down {name} (asking the guest OS)..."),
            Self::Reboot => format!("Rebooting {name}..."),
            Self::Stop => format!("Stopping {name}..."),
            Self::Pause => format!("Pausing {name}..."),
            Self::Resume => format!("Resuming {name}..."),
            Self::Reset => format!("Resetting {name}..."),
        }
    }

    /// Text once the VM reached the new state.
    pub fn done(self, name: &str) -> String {
        match self {
            Self::Start => format!("{name} is running"),
            Self::Shutdown => format!("{name} has shut down"),
            Self::Reboot => format!("{name} rebooted"),
            Self::Stop => format!("{name} is stopped"),
            Self::Pause => format!("{name} is paused"),
            Self::Resume => format!("{name} is running"),
            Self::Reset => format!("{name} was reset"),
        }
    }

    /// The state the VM ends up in, if the action changes it.
    pub fn target(self) -> Option<VmStatus> {
        match self {
            Self::Start | Self::Resume => Some(VmStatus::Running),
            Self::Shutdown | Self::Stop => Some(VmStatus::Stopped),
            Self::Pause => Some(VmStatus::Paused),
            Self::Reboot | Self::Reset => None,
        }
    }
}

/// The confirmation question for an action, or `None` when it needs none (Start).
/// Says what happens to the guest in plain words, like Proxmox's dialogs.
pub fn confirm_text(action: PowerAction, name: &str) -> Option<String> {
    match action {
        PowerAction::Start | PowerAction::Pause | PowerAction::Resume => None,
        PowerAction::Reset => Some(format!(
            "Reset VM {name}?\n\nLike pressing the reset button: the guest restarts at once without shutting down, and unsaved data in it is lost."
        )),
        PowerAction::Shutdown => Some(format!(
            "Shut down VM {name}?\n\nThe guest operating system is asked to shut down cleanly (like pressing the power button)."
        )),
        PowerAction::Reboot => Some(format!(
            "Reboot VM {name}?\n\nThe guest operating system is asked to restart."
        )),
        PowerAction::Stop => Some(format!(
            "Stop VM {name} now?\n\nThis is like pulling the power plug: the guest gets no chance to shut down and unsaved data in it is lost. Use Shutdown for a clean stop."
        )),
    }
}

/// Ask the user (browser dialog, as the rest of the UI does). `true` = go ahead.
pub fn ask(question: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.confirm_with_message(question).ok())
        .unwrap_or(false)
}

/// The question asked before deleting VM `id` (its disks go with it).
pub fn delete_question(id: &str, running: bool) -> String {
    format!(
        "Delete VM {id} permanently?\n\nIts configuration and disks are removed; this cannot be undone.{}\n\nType the VM ID ({id}) to confirm:",
        if running {
            " The VM is running: it is powered off first."
        } else {
            ""
        }
    )
}

/// Whether what the user typed confirms deleting `id`.
pub fn delete_confirmed(id: &str, typed: Option<&str>) -> bool {
    typed.map(str::trim) == Some(id)
}

/// Ask for the VM ID before a delete (Proxmox asks the same). `true` = delete.
pub fn confirm_delete(id: &str, running: bool) -> bool {
    let typed = web_sys::window()
        .and_then(|w| w.prompt_with_message(&delete_question(id, running)).ok())
        .flatten();
    delete_confirmed(id, typed.as_deref())
}

/// Run `action` on VM `id` and wait (up to ~60 s) until its state shows the
/// result. `Ok(true)` = reached, `Ok(false)` = still changing when we stopped waiting.
pub async fn run_power_action(id: &str, action: PowerAction) -> Result<bool, api::ApiError> {
    match action {
        PowerAction::Start => api::start_vm(id).await?,
        PowerAction::Shutdown => {
            api::shutdown_vm(id, 120, false).await?;
        }
        PowerAction::Reboot => {
            api::reboot_vm(id).await?;
        }
        PowerAction::Stop => api::stop_vm(id).await?,
        PowerAction::Pause => {
            api::pause_vm(id).await?;
        }
        PowerAction::Resume => {
            api::resume_vm(id).await?;
        }
        PowerAction::Reset => {
            api::reset_vm(id).await?;
        }
    }
    let Some(target) = action.target() else {
        return Ok(true);
    };
    for _ in 0..40 {
        if let Ok(v) = api::get_vm(id).await {
            if v.status == target {
                return Ok(true);
            }
        }
        gloo_timers::future::TimeoutFuture::new(1500).await;
    }
    Ok(false)
}

/// Next steps under a "VM created/imported" notice: power controls (start it
/// right here), Console, and its Hardware page.
#[component]
pub fn NewVmActions(vm: VmConfig) -> impl IntoView {
    let id = vm.id.clone();
    let (cur, set_cur) = create_signal(Some(vm));
    let refresh = {
        let id = id.clone();
        Callback::new(move |_| {
            let id = id.clone();
            spawn_local(async move {
                if let Ok(v) = api::get_vm(&id).await {
                    set_cur.set(Some(v));
                }
            });
        })
    };
    view! {
        <div class="new-vm-actions" id="new-vm-actions">
            <VmPowerBar vm=Signal::derive(move || cur.get()) on_changed=refresh/>
            <a class="btn btn-sm" href=format!("/vms/{}/hardware", id)>"Open its hardware"</a>
            <a class="btn btn-sm" href=format!("/vms/{}", id)>"VM details"</a>
        </div>
    }
}

/// Start / Shutdown / Reboot / Stop (+ Console) for one VM. `vm` is the page's
/// loaded VM; `on_changed` reloads the page's data after an action.
#[component]
pub fn VmPowerBar(
    #[prop(into)] vm: Signal<Option<VmConfig>>,
    #[prop(into)] on_changed: Callback<()>,
    /// Show the Console link (off on the console page itself).
    #[prop(default = true)]
    console_link: bool,
) -> impl IntoView {
    let (busy, set_busy) = create_signal(None::<PowerAction>);
    let (notice, set_notice) = create_signal(None::<String>);
    let (error, set_error) = create_signal(None::<String>);

    let act = move |action: PowerAction| {
        let Some(v) = vm.get_untracked() else { return };
        if let Some(q) = confirm_text(action, &v.name) {
            if !ask(&q) {
                return;
            }
        }
        set_error.set(None);
        set_busy.set(Some(action));
        set_notice.set(Some(action.progress(&v.name)));
        spawn_local(async move {
            match run_power_action(&v.id, action).await {
                Ok(true) => set_notice.set(Some(action.done(&v.name))),
                Ok(false) => set_notice.set(Some(format!(
                    "{} was asked to {}; it has not finished yet (the state refreshes on its own)",
                    v.name,
                    action.label().to_lowercase()
                ))),
                Err(e) => {
                    set_notice.set(None);
                    set_error.set(Some(format!(
                        "Could not {} {}: {}",
                        action.label().to_lowercase(),
                        v.name,
                        e.message
                    )));
                }
            }
            set_busy.set(None);
            on_changed.call(());
        });
    };

    let button = move |action: PowerAction, class: &'static str, title: &'static str| {
        let id = format!("vm-power-{}", action.label().to_lowercase());
        view! {
            <button id=id class=class title=title
                disabled=move || {
                    busy.get().is_some()
                        || !vm.get().map(|v| action.allowed(&v.status)).unwrap_or(false)
                }
                on:click=move |_| act(action)>
                {move || if busy.get() == Some(action) { format!("{}...", action.label()) } else { action.label().to_string() }}
            </button>
        }
    };

    view! {
        <div class="vm-power-bar" id="vm-power-bar" role="toolbar" aria-label="Power">
            <span class="vm-power-state" id="vm-power-state">
                {move || vm.get().map(|v| {
                    let s = v.status.to_string();
                    view! { <span class=format!("vm-status status-{s}")>{s.clone()}</span> }
                })}
            </span>
            {button(PowerAction::Start, "btn btn-success", "Power on the VM")}
            {button(PowerAction::Shutdown, "btn btn-warning", "Ask the guest OS to shut down cleanly")}
            {button(PowerAction::Reboot, "btn btn-secondary", "Ask the guest OS to restart")}
            {button(PowerAction::Stop, "btn btn-danger", "Stop immediately (like pulling the power)")}
            // only where they apply (a paused VM shows Resume, a running one Pause and Reset)
            {move || {
                let st = vm.get().map(|v| v.status);
                let shown = move |a: PowerAction| st.as_ref().map(|s| a.allowed(s)).unwrap_or(false);
                view! {
                    {shown(PowerAction::Resume).then(|| button(PowerAction::Resume, "btn btn-success", "Let the paused guest run again"))}
                    {shown(PowerAction::Pause).then(|| button(PowerAction::Pause, "btn btn-secondary", "Freeze the guest's CPUs (memory stays)"))}
                    {shown(PowerAction::Reset).then(|| button(PowerAction::Reset, "btn btn-secondary", "Hard restart (like the reset button)"))}
                }
            }}
            {console_link.then(|| view! {
                <a class="btn btn-primary" id="vm-power-console"
                    href=move || vm.get().map(|v| format!("/vms/{}/console", v.id)).unwrap_or_default()>"Console"</a>
            })}
        </div>
        {move || notice.get().map(|n| view! { <div class="alert alert-info" id="vm-power-notice" role="status">{n}</div> })}
        {move || error.get().map(|e| view! { <div class="alert alert-error" id="vm-power-error" role="alert">{e}</div> })}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_follow_the_vm_state() {
        use PowerAction::*;
        let running = VmStatus::Running;
        let stopped = VmStatus::Stopped;
        let paused = VmStatus::Paused;
        assert!(Start.allowed(&stopped) && !Start.allowed(&running) && !Start.allowed(&paused));
        assert!(Resume.allowed(&paused) && !Resume.allowed(&running) && !Resume.allowed(&stopped));
        assert!(Pause.allowed(&running) && !Pause.allowed(&paused) && !Pause.allowed(&stopped));
        assert!(Reset.allowed(&running) && !Reset.allowed(&stopped));
        assert!(Shutdown.allowed(&running) && !Shutdown.allowed(&stopped));
        assert!(Reboot.allowed(&running) && !Reboot.allowed(&stopped));
        assert!(Stop.allowed(&running) && Stop.allowed(&paused) && !Stop.allowed(&stopped));
    }

    #[test]
    fn delete_needs_the_exact_vm_id() {
        assert!(delete_confirmed("kali", Some("kali")));
        assert!(delete_confirmed("kali", Some(" kali ")));
        assert!(!delete_confirmed("kali", Some("Kali")));
        assert!(!delete_confirmed("kali", Some("")));
        assert!(!delete_confirmed("kali", None)); // Cancel
        assert!(delete_question("kali", true).contains("powered off first"));
        assert!(!delete_question("kali", false).contains("powered off"));
    }

    #[test]
    fn destructive_actions_ask_first_start_does_not() {
        assert!(confirm_text(PowerAction::Start, "kali").is_none());
        assert!(confirm_text(PowerAction::Resume, "kali").is_none());
        assert!(confirm_text(PowerAction::Reset, "kali")
            .unwrap()
            .contains("reset button"));
        let stop = confirm_text(PowerAction::Stop, "kali").unwrap();
        assert!(
            stop.contains("kali") && stop.contains("pulling the power plug"),
            "{stop}"
        );
        for a in [PowerAction::Shutdown, PowerAction::Reboot] {
            assert!(confirm_text(a, "kali").unwrap().contains("kali"));
        }
    }
}
