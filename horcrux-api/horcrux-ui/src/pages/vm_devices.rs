//! VM hardware page, live devices: memory balloon, QEMU guest agent and USB
//! passthrough of a QEMU VM (the devices themselves are hardware settings).
use crate::api::vm_devices as dev;
use horcrux_common::{VmConfig, VmHardware};
use leptos::*;

/// Balloon / guest agent / USB controller switches (they take effect at the
/// VM's next start).
#[component]
pub fn DeviceSwitches(hardware: RwSignal<VmHardware>, running: Signal<bool>) -> impl IntoView {
    view! {
        <fieldset class="form-section" id="vm-devices-fieldset">
            <legend>"Balloon, guest agent, USB"</legend>
            <div class="form-row">
                <div class="form-group">
                    <label>
                        <input id="vm-balloon" type="checkbox" disabled={move || running.get()}
                            prop:checked={move || hardware.get().balloon}
                            on:change=move |ev| { let on = event_target_checked(&ev); hardware.update(|h| h.balloon = on); }/>
                        " Memory ballooning (virtio-balloon)"
                    </label>
                </div>
                <div class="form-group">
                    <label>
                        <input id="vm-guest-agent" type="checkbox" disabled={move || running.get()}
                            prop:checked={move || hardware.get().guest_agent}
                            on:change=move |ev| { let on = event_target_checked(&ev); hardware.update(|h| h.guest_agent = on); }/>
                        " QEMU guest agent channel"
                    </label>
                    <small class="form-help">"Needs qemu-guest-agent installed in the guest."</small>
                </div>
                <div class="form-group">
                    <label>
                        <input id="vm-usb-controller" type="checkbox" disabled={move || running.get()}
                            prop:checked={move || hardware.get().usb_controller || !hardware.get().usb.is_empty()}
                            on:change=move |ev| { let on = event_target_checked(&ev); hardware.update(|h| h.usb_controller = on); }/>
                        " USB controller (for passthrough)"
                    </label>
                </div>
            </div>
        </fieldset>
    }
}

fn mb(v: Option<u64>) -> String {
    v.map(|m| format!("{m} MB")).unwrap_or_else(|| "-".into())
}

#[component]
pub fn VmLiveDevices(
    #[prop(into)] vm_id: Signal<String>,
    vm: ReadSignal<Option<VmConfig>>,
    set_vm: WriteSignal<Option<VmConfig>>,
    hardware: RwSignal<VmHardware>,
    running: Signal<bool>,
) -> impl IntoView {
    let (balloon, set_balloon) = create_signal(None::<dev::BalloonInfo>);
    let (agent, set_agent) = create_signal(None::<dev::AgentInfo>);
    let (host_usb, set_host_usb) = create_signal(Vec::<dev::HostUsbDevice>::new());
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let target = create_rw_signal(String::new());
    let usb_pick = create_rw_signal(String::new());

    let reload = move || {
        let id = vm_id.get_untracked();
        spawn_local(async move {
            if let Ok(b) = dev::get_balloon(&id).await {
                set_balloon.set(Some(b));
            }
            if let Ok(a) = dev::get_agent(&id).await {
                set_agent.set(Some(a));
            }
            if let Ok(u) = dev::list_host_usb().await {
                set_host_usb.set(u);
            }
        });
    };
    create_effect(move |_| {
        vm_id.track();
        running.track();
        reload()
    });

    let apply_balloon = move |_| {
        let id = vm_id.get_untracked();
        let Ok(t) = target.get_untracked().trim().parse::<u64>() else {
            set_error.set(Some("Enter the target in MB".into()));
            return;
        };
        spawn_local(async move {
            match dev::set_balloon(&id, t).await {
                Ok(b) => {
                    set_error.set(None);
                    set_notice.set(Some(format!(
                        "Balloon target {t} MB requested; the guest holds {} now",
                        mb(b.actual_mb)
                    )));
                    set_balloon.set(Some(b));
                }
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };

    let agent_do = move |action: &'static str| {
        let id = vm_id.get_untracked();
        spawn_local(async move {
            match dev::agent_action(&id, action).await {
                Ok(r) => {
                    set_error.set(None);
                    set_notice.set(Some(format!("Agent {}: {}", r.action, r.result)));
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            reload();
        });
    };

    let add_usb = move |_| {
        let id = vm_id.get_untracked();
        let pick = usb_pick.get_untracked();
        let Some((v, p)) = pick.split_once(':') else {
            set_error.set(Some("Pick a host USB device".into()));
            return;
        };
        let (v, p) = (v.to_string(), p.to_string());
        spawn_local(async move {
            match dev::attach_usb(&id, &v, &p).await {
                Ok(cfg) => {
                    set_error.set(None);
                    set_notice.set(Some(format!("USB device {v}:{p} assigned")));
                    // keep the form's copy in step, or Save would undo it
                    hardware.update(|h| h.usb = cfg.hardware.usb.clone());
                    set_vm.set(Some(cfg));
                }
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };
    let remove_usb = move |v: String, p: String| {
        let id = vm_id.get_untracked();
        spawn_local(async move {
            match dev::detach_usb(&id, &v, &p).await {
                Ok(()) => {
                    set_error.set(None);
                    set_notice.set(Some(format!("USB device {v}:{p} removed")));
                    // reload the VM's hardware
                    if let Ok(cfg) = crate::api::get_vm(&id).await {
                        hardware.update(|h| h.usb = cfg.hardware.usb.clone());
                        set_vm.set(Some(cfg));
                    }
                }
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };

    view! {
        <fieldset class="form-section" id="vm-live-devices">
            <legend>"Live devices"</legend>
            {move || notice.get().map(|n| view! { <div class="alert alert-info" id="vm-dev-notice">{n}</div> })}
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="vm-dev-error">{e}</div> })}

            <h4>"Memory balloon"</h4>
            {move || match balloon.get() {
                Some(b) if b.enabled && b.running => view! {
                    <p id="balloon-state">{format!("Configured {} MB, the guest holds {} now", b.memory_mb, mb(b.actual_mb))}
                        {b.target_mb.map(|t| format!(" (target {t} MB)")).unwrap_or_default()}</p>
                    <div class="form-row">
                        <input id="balloon-target" type="number" min="16" placeholder="target MB" prop:value=target
                            on:input=move |ev| target.set(event_target_value(&ev))/>
                        <button id="balloon-apply" type="button" class="btn btn-primary" on:click=apply_balloon>"Set target"</button>
                    </div>
                    <small class="form-help">"The guest's virtio_balloon driver gives memory back to the host (or takes it again) until it holds the target."</small>
                }.into_view(),
                Some(b) if b.enabled => view! { <p class="form-help">"Start the VM to change its balloon."</p> }.into_view(),
                _ => view! { <p class="form-help">"Enable \"Memory ballooning\" above and restart the VM."</p> }.into_view(),
            }}

            <h4>"Guest agent"</h4>
            {move || match agent.get() {
                Some(a) if a.available => view! {
                    <p id="agent-state">
                        {format!("qemu-guest-agent {} - {} {} ({}), host name {}",
                            a.version.clone().unwrap_or_default(),
                            a.os_name.clone().unwrap_or_default(), a.os_version.clone().unwrap_or_default(),
                            a.kernel.clone().unwrap_or_default(), a.hostname.clone().unwrap_or_default())}
                    </p>
                    <ul id="agent-interfaces">
                        {a.interfaces.iter().map(|i| view! { <li>{format!("{} {} {}", i.name, i.mac.clone().unwrap_or_default(), i.addresses.join(", "))}</li> }).collect_view()}
                    </ul>
                    <div class="form-actions">
                        <button type="button" class="btn btn-sm" id="agent-ping" on:click=move |_| agent_do("ping")>"Ping"</button>
                        <button type="button" class="btn btn-sm" id="agent-freeze" on:click=move |_| agent_do("fsfreeze")>"Freeze file systems"</button>
                        <button type="button" class="btn btn-sm" id="agent-thaw" on:click=move |_| agent_do("fsthaw")>"Thaw"</button>
                        <button type="button" class="btn btn-sm" id="agent-trim" on:click=move |_| agent_do("fstrim")>"Trim (fstrim)"</button>
                        <button type="button" class="btn btn-sm btn-warning" id="agent-shutdown" on:click=move |_| agent_do("shutdown")>"Shutdown via agent"</button>
                    </div>
                    <small class="form-help">"A frozen guest is thawed again automatically after 5 minutes."</small>
                }.into_view(),
                Some(a) => view! { <p id="agent-state" class="form-help">{a.error.unwrap_or_else(|| "The agent is not reachable.".into())}</p> }.into_view(),
                None => view! { <p class="form-help">"Loading..."</p> }.into_view(),
            }}

            <h4>"USB passthrough"</h4>
            <ul id="usb-assigned">
                {move || vm.get().map(|v| v.hardware.usb.iter().map(|d| {
                    let (vv, pp) = (d.vendor_id.clone(), d.product_id.clone());
                    let (v2, p2) = (vv.clone(), pp.clone());
                    view! { <li>{format!("{vv}:{pp}")} " " <button type="button" class="btn btn-sm btn-danger usb-remove" on:click=move |_| remove_usb(v2.clone(), p2.clone())>"Remove"</button></li> }
                }).collect_view())}
            </ul>
            <div class="form-row">
                <select id="usb-host-pick" prop:value=usb_pick on:change=move |ev| usb_pick.set(event_target_value(&ev))>
                    <option value="">"host USB device..."</option>
                    {move || host_usb.get().into_iter().filter(|d| !d.is_hub).map(|d| view! {
                        <option value=format!("{}:{}", d.vendor_id, d.product_id)>
                            {format!("{}:{} {} {}", d.vendor_id, d.product_id, d.manufacturer.clone().unwrap_or_default(), d.product.clone().unwrap_or_default())}
                        </option>
                    }).collect_view()}
                </select>
                <button id="usb-add" type="button" class="btn btn-primary" on:click=add_usb>"Add device"</button>
            </div>
            <small class="form-help">"A running VM needs the USB controller (enabled above, applied at the next start) to hot-plug; the device is then taken from the host."</small>
        </fieldset>
    }
}
