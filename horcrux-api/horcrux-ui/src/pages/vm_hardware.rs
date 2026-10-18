//! VM hardware: network cards, CD-ROM (ISO) and boot order - the fields
//! shared by Create VM and the VM's Hardware page - plus the Hardware page
//! itself (`/vms/:id/hardware`: name, CPUs, memory, hardware, and the power
//! actions Shutdown / Reboot / Reset).
use crate::api::{self, VmConfigUpdate, VmHostOptions};
use horcrux_common::{BootDevice, NicModel, VmConfig, VmHardware, VmHypervisor, VmNic, VmStatus};
use leptos::*;
use leptos_router::*;

const NIC_MODELS: [(NicModel, &str, &str); 3] = [
    (NicModel::Virtio, "virtio", "VirtIO (paravirtual)"),
    (NicModel::E1000, "e1000", "Intel E1000"),
    (NicModel::Rtl8139, "rtl8139", "Realtek RTL8139"),
];

fn model_from(v: &str) -> NicModel {
    NIC_MODELS
        .iter()
        .find(|(_, val, _)| *val == v)
        .map(|(m, _, _)| *m)
        .unwrap_or_default()
}

fn model_value(m: NicModel) -> &'static str {
    NIC_MODELS
        .iter()
        .find(|(x, _, _)| *x == m)
        .map(|(_, v, _)| *v)
        .unwrap_or("virtio")
}

/// Boot-order presets offered by the form.
pub fn boot_preset(hw: &VmHardware) -> &'static str {
    match hw.boot.first() {
        Some(BootDevice::Cdrom) => "cdrom",
        Some(BootDevice::Net) => "net",
        _ => "disk",
    }
}

pub fn boot_from_preset(p: &str) -> Vec<BootDevice> {
    match p {
        "cdrom" => vec![BootDevice::Cdrom, BootDevice::Disk],
        "net" => vec![BootDevice::Net, BootDevice::Disk, BootDevice::Cdrom],
        _ => Vec::new(),
    }
}

/// Network cards, CD-ROM and boot order of a QEMU VM.
#[component]
pub fn HardwareFields(
    hardware: RwSignal<VmHardware>,
    #[prop(into)] options: Signal<Option<VmHostOptions>>,
    /// Network cards and boot order are fixed while the VM runs.
    #[prop(into, default = Signal::derive(|| false))]
    running: Signal<bool>,
) -> impl IntoView {
    let bridges = move || options.get().map(|o| o.bridges).unwrap_or_default();
    let add_nic = move |_| {
        let bridge = bridges()
            .first()
            .map(|b| b.name.clone())
            .unwrap_or_else(|| "vmbr0".to_string());
        hardware.update(|h| {
            h.nets.push(VmNic {
                model: NicModel::Virtio,
                bridge,
                mac: None,
                vlan: None,
            })
        });
    };

    view! {
        <fieldset class="vm-hardware">
            <legend>"Network"</legend>
            {move || {
                let nets = hardware.get().nets;
                if nets.is_empty() {
                    view! { <p class="form-help" id="vm-no-nic">"No network card: the VM has no network."</p> }.into_view()
                } else {
                    nets.into_iter().enumerate().map(|(i, nic)| {
                        let mut names: Vec<(String, bool)> =
                            bridges().into_iter().map(|b| (b.name, b.vlan_aware)).collect();
                        if !names.iter().any(|(n, _)| *n == nic.bridge) {
                            names.push((nic.bridge.clone(), false));
                        }
                        let current_bridge = nic.bridge.clone();
                        let vlan_aware = names.iter().any(|(n, v)| *n == nic.bridge && *v);
                        let model = model_value(nic.model);
                        let vlan = nic.vlan.map(|v| v.to_string()).unwrap_or_default();
                        let mac = nic.mac.clone().unwrap_or_else(|| "generated on save".to_string());
                        view! {
                            <div class="vm-nic-row form-row" data-nic=i>
                                <strong>{format!("net{i}")}</strong>
                                <select class="vm-nic-model" disabled={move || running.get()}
                                    on:change=move |ev| {
                                        let m = model_from(&event_target_value(&ev));
                                        hardware.update(|h| if let Some(n) = h.nets.get_mut(i) { n.model = m });
                                    }>
                                    {NIC_MODELS.iter().map(|(_, v, label)| view! {
                                        <option value=*v selected={*v == model}>{*label}</option>
                                    }).collect_view()}
                                </select>
                                <select class="vm-nic-bridge" disabled={move || running.get()}
                                    on:change=move |ev| {
                                        let b = event_target_value(&ev);
                                        hardware.update(|h| if let Some(n) = h.nets.get_mut(i) { n.bridge = b });
                                    }>
                                    {names.into_iter().map(|(n, aware)| {
                                        let sel = n == current_bridge;
                                        let label = if aware { format!("{n} (VLAN-aware)") } else { n.clone() };
                                        view! { <option value=n.clone() selected=sel>{label}</option> }
                                    }).collect_view()}
                                </select>
                                <input class="vm-nic-vlan" type="number" min="1" max="4094"
                                    placeholder=if vlan_aware { "VLAN tag (none)" } else { "no VLANs on this bridge" }
                                    disabled={move || running.get() || !vlan_aware}
                                    prop:value=vlan
                                    on:change=move |ev| {
                                        let v = event_target_value(&ev).trim().parse::<u16>().ok();
                                        hardware.update(|h| if let Some(n) = h.nets.get_mut(i) { n.vlan = v });
                                    }/>
                                <code class="vm-nic-mac">{mac}</code>
                                <button type="button" class="btn btn-sm btn-danger vm-nic-remove" disabled={move || running.get()}
                                    on:click=move |_| hardware.update(|h| { if i < h.nets.len() { h.nets.remove(i); } })>
                                    "Remove"
                                </button>
                            </div>
                        }
                    }).collect_view()
                }
            }}
            <button type="button" id="vm-nic-add" class="btn btn-sm btn-secondary"
                disabled={move || running.get() || hardware.get().nets.len() >= horcrux_common::MAX_VM_NICS}
                on:click=add_nic>
                "Add network card"
            </button>
            {move || (options.get().is_some() && bridges().is_empty()).then(|| view! {
                <p class="error" id="vm-no-bridges">
                    "This host has no Linux bridge to attach a network card to. "
                    <A href="/network/bridges">"Create one in Host Bridges"</A>
                    ", then come back."
                </p>
            })}
        </fieldset>

        <fieldset class="vm-hardware">
            <legend>"CD-ROM and boot"</legend>
            <div class="form-group">
                <label for="vm-cdrom">"CD-ROM (ISO image)"</label>
                <select id="vm-cdrom" on:change=move |ev| {
                    let v = event_target_value(&ev);
                    hardware.update(|h| h.cdrom = (!v.is_empty()).then_some(v));
                }>
                    <option value="" selected={move || hardware.get().cdrom.is_none()}>"(empty drive)"</option>
                    {move || {
                        let current = hardware.get().cdrom.clone();
                        let mut isos: Vec<api::IsoFile> = options
                            .get()
                            .map(|o| o.isos)
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|i| !i.downloading && i.error.is_none())
                            .collect();
                        if let Some(c) = &current {
                            if !isos.iter().any(|i| &i.path == c) {
                                isos.push(api::IsoFile { name: c.clone(), path: c.clone(), size: 0, downloading: false, error: None });
                            }
                        }
                        isos.into_iter().map(|iso| {
                            let sel = current.as_deref() == Some(iso.path.as_str());
                            let label = if iso.size > 0 { format!("{} ({})", iso.name, api::human_bytes(iso.size)) } else { iso.name.clone() };
                            view! { <option value=iso.path.clone() selected=sel>{label}</option> }
                        }).collect_view()
                    }}
                </select>
                <small class="form-help">
                    "ISOs come from the ISO library (Infrastructure > ISO Library: download one from a URL). The disc can be changed while the VM runs."
                </small>
            </div>
            <div class="form-group">
                <label for="vm-boot">"Boot order"</label>
                <select id="vm-boot" disabled={move || running.get()} on:change=move |ev| {
                    let b = boot_from_preset(&event_target_value(&ev));
                    hardware.update(|h| h.boot = b);
                }>
                    <option value="disk" selected={move || boot_preset(&hardware.get()) == "disk"}>"Disk, then CD-ROM"</option>
                    <option value="cdrom" selected={move || boot_preset(&hardware.get()) == "cdrom"}>"CD-ROM first (install from the ISO)"</option>
                    <option value="net" selected={move || boot_preset(&hardware.get()) == "net"}>"Network (PXE) first"</option>
                </select>
            </div>
        </fieldset>
    }
}

/// `/vms/:id/hardware`
#[component]
pub fn VmHardwarePage() -> impl IntoView {
    let params = use_params_map();
    let vm_id = move || params.with(|p| p.get("id").cloned().unwrap_or_default());
    let (vm, set_vm) = create_signal(None::<VmConfig>);
    let (options, set_options) = create_signal(None::<VmHostOptions>);
    let (notice, set_notice) = create_signal(None::<String>);
    let (error, set_error) = create_signal(None::<String>);
    let (busy, set_busy) = create_signal(false);
    let name = create_rw_signal(String::new());
    let cpus = create_rw_signal(1u32);
    let memory = create_rw_signal(512u64);
    let hardware = create_rw_signal(VmHardware::default());

    let load = move || {
        let id = vm_id();
        spawn_local(async move {
            match api::get_vm(&id).await {
                Ok(v) => {
                    name.set(v.name.clone());
                    cpus.set(v.cpus);
                    memory.set(v.memory);
                    hardware.set(v.hardware.clone());
                    set_vm.set(Some(v));
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            match api::get_vm_host_options().await {
                Ok(o) => set_options.set(Some(o)),
                Err(e) => {
                    set_error.set(Some(format!("Could not load bridges/ISOs: {}", e.message)))
                }
            }
        });
    };
    create_effect(move |_| load());

    let running = Signal::derive(move || {
        vm.get()
            .map(|v| v.status == VmStatus::Running)
            .unwrap_or(false)
    });
    let is_qemu = move || {
        vm.get()
            .map(|v| v.hypervisor == VmHypervisor::Qemu)
            .unwrap_or(true)
    };

    let save = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let id = vm_id();
        let Some(cur) = vm.get() else { return };
        let mut upd = VmConfigUpdate {
            name: Some(name.get()),
            ..Default::default()
        };
        if cpus.get() != cur.cpus {
            upd.cpus = Some(cpus.get());
        }
        if memory.get() != cur.memory {
            upd.memory = Some(memory.get());
        }
        if is_qemu() && hardware.get() != cur.hardware {
            upd.hardware = Some(hardware.get());
        }
        set_busy.set(true);
        set_error.set(None);
        spawn_local(async move {
            match api::update_vm_config(&id, &upd).await {
                Ok(v) => {
                    hardware.set(v.hardware.clone());
                    set_notice.set(Some(if v.status == VmStatus::Running {
                        "Saved (CD-ROM changed in the running VM)".to_string()
                    } else {
                        "Saved: applies at the next start".to_string()
                    }));
                    set_vm.set(Some(v));
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            set_busy.set(false);
        });
    };

    let power = move |action: &'static str| {
        let id = vm_id();
        set_busy.set(true);
        set_error.set(None);
        set_notice.set(Some(format!("{action}...")));
        spawn_local(async move {
            let r = match action {
                "Shutdown" => api::shutdown_vm(&id, 60, false).await,
                "Shutdown (force after 60 s)" => api::shutdown_vm(&id, 60, true).await,
                "Reboot" => api::reboot_vm(&id).await,
                _ => api::reset_vm(&id).await,
            };
            match r {
                Ok(v) => {
                    set_notice.set(Some(format!("{action}: VM is {}", v.status)));
                    set_vm.set(Some(v));
                }
                Err(e) => {
                    set_notice.set(None);
                    set_error.set(Some(e.message));
                }
            }
            set_busy.set(false);
        });
    };

    view! {
        <div class="vm-hardware-page">
            <div class="page-header">
                <h1>"VM " {move || vm.get().map(|v| format!("{} ({})", v.name, v.id)).unwrap_or_else(vm_id)}</h1>
                <A href="/vms" class="btn btn-secondary">"Back to VMs"</A>
            </div>
            {move || vm.get().map(|v| view! {
                <p>"Status: " <span id="vm-hw-status" class=format!("status-{}", v.status)>{v.status.to_string()}</span></p>
            })}
            {move || notice.get().map(|n| view! { <div class="alert alert-info" id="vm-hw-notice">{n}</div> })}
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="vm-hw-error">{e}</div> })}

            <div class="form-actions" id="vm-power">
                <button class="btn btn-warning" id="vm-shutdown" disabled={move || busy.get() || !running.get()}
                    on:click=move |_| power("Shutdown")>"Shutdown"</button>
                <button class="btn btn-warning" id="vm-shutdown-force" disabled={move || busy.get() || !running.get()}
                    on:click=move |_| power("Shutdown (force after 60 s)")>"Shutdown, then stop"</button>
                <button class="btn btn-secondary" id="vm-reboot" disabled={move || busy.get() || !running.get()}
                    on:click=move |_| power("Reboot")>"Reboot"</button>
                <button class="btn btn-danger" id="vm-reset" disabled={move || busy.get() || !running.get()}
                    on:click=move |_| power("Reset")>"Reset"</button>
            </div>

            <form id="vm-hardware-form" on:submit=save>
                <div class="form-group">
                    <label for="vm-hw-name">"Name"</label>
                    <input id="vm-hw-name" type="text" required prop:value=name
                        on:input=move |ev| name.set(event_target_value(&ev))/>
                </div>
                <div class="form-row">
                    <div class="form-group">
                        <label for="vm-hw-cpus">"CPUs"</label>
                        <input id="vm-hw-cpus" type="number" min="1" max="512" prop:value=cpus disabled={move || running.get()}
                            on:input=move |ev| cpus.set(event_target_value(&ev).parse().unwrap_or(1))/>
                    </div>
                    <div class="form-group">
                        <label for="vm-hw-memory">"Memory (MB)"</label>
                        <input id="vm-hw-memory" type="number" min="16" prop:value=memory disabled={move || running.get()}
                            on:input=move |ev| memory.set(event_target_value(&ev).parse().unwrap_or(512))/>
                    </div>
                </div>
                {move || running.get().then(|| view! {
                    <p class="form-help" id="vm-hw-running">"The VM is running: CPUs, memory, network cards and boot order can be changed after it is stopped. The name and the CD-ROM can change now."</p>
                })}
                <Show when=is_qemu fallback=|| view! {
                    <p class="form-help">"LXD/Incus VMs get their network and devices from their LXD/Incus profile."</p>
                }>
                    <HardwareFields hardware=hardware options=options running=running/>
                </Show>
                <div class="form-actions">
                    <button type="submit" id="vm-hw-save" class="btn btn-primary" disabled=busy>"Save"</button>
                </div>
            </form>

            <Show when=is_qemu>
                <VmFirewall vm_id=Signal::derive(vm_id)/>
            </Show>
        </div>
    }
}

/// The VM's own firewall: rules on its network cards, applied at once.
#[component]
fn VmFirewall(#[prop(into)] vm_id: Signal<String>) -> impl IntoView {
    let (rules, set_rules) = create_signal(Vec::<api::GuestFirewallRule>::new());
    let (error, set_error) = create_signal(None::<String>);
    let direction = create_rw_signal("in".to_string());
    let action = create_rw_signal("DROP".to_string());
    let protocol = create_rw_signal("tcp".to_string());
    let dport = create_rw_signal(String::new());
    let source = create_rw_signal(String::new());
    let comment = create_rw_signal(String::new());

    let reload = move || {
        let id = vm_id.get_untracked();
        spawn_local(async move {
            match api::list_vm_firewall(&id).await {
                Ok(r) => set_rules.set(r),
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };
    create_effect(move |_| {
        vm_id.track();
        reload()
    });

    let add = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let opt = |s: String| Some(s.trim().to_string()).filter(|s| !s.is_empty());
        let proto = protocol.get();
        let rule = api::GuestFirewallRule {
            id: String::new(),
            enabled: true,
            action: action.get(),
            direction: direction.get(),
            protocol: (proto != "any").then_some(proto.clone()),
            source: opt(source.get()),
            dest: None,
            sport: None,
            dport: if proto == "tcp" || proto == "udp" {
                opt(dport.get())
            } else {
                None
            },
            comment: opt(comment.get()),
            log: false,
            position: rules.get_untracked().len() as u32,
        };
        let id = vm_id.get_untracked();
        spawn_local(async move {
            match api::add_vm_firewall(&id, &rule).await {
                Ok(_) => {
                    set_error.set(None);
                    dport.set(String::new());
                    comment.set(String::new());
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            reload();
        });
    };
    let remove = move |rule_id: String| {
        let id = vm_id.get_untracked();
        spawn_local(async move {
            if let Err(e) = api::delete_vm_firewall(&id, &rule_id).await {
                set_error.set(Some(e.message));
            }
            reload();
        });
    };

    view! {
        <fieldset class="vm-hardware" id="vm-firewall">
            <legend>"Firewall"</legend>
            <p class="form-help">
                "Rules for this VM's network cards only, applied immediately. In = traffic to the VM, Out = traffic from it. Anything not matched is allowed."
            </p>
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="vm-fw-error">{e}</div> })}
            <table class="vm-table" id="vm-fw-table">
                <thead><tr><th>"Direction"</th><th>"Action"</th><th>"Protocol"</th><th>"Port"</th><th>"Source"</th><th>"Comment"</th><th></th></tr></thead>
                <tbody>
                    {move || rules.get().into_iter().map(|r| {
                        let rid = r.id.clone();
                        view! {
                            <tr class="vm-fw-row" data-rule=r.id.clone()>
                                <td>{r.direction.clone()}</td>
                                <td>{r.action.clone()}</td>
                                <td>{r.protocol.clone().unwrap_or_else(|| "any".into())}</td>
                                <td>{r.dport.clone().unwrap_or_default()}</td>
                                <td>{r.source.clone().unwrap_or_default()}</td>
                                <td>{r.comment.clone().unwrap_or_default()}</td>
                                <td><button type="button" class="btn btn-sm btn-danger vm-fw-delete"
                                    on:click=move |_| remove(rid.clone())>"Delete"</button></td>
                            </tr>
                        }
                    }).collect_view()}
                </tbody>
            </table>
            <form class="form-row" id="vm-fw-form" on:submit=add>
                <select id="vm-fw-direction" on:change=move |ev| direction.set(event_target_value(&ev))>
                    <option value="in">"In (to VM)"</option>
                    <option value="out">"Out (from VM)"</option>
                </select>
                <select id="vm-fw-action" on:change=move |ev| action.set(event_target_value(&ev))>
                    <option value="DROP">"Drop"</option>
                    <option value="REJECT">"Reject"</option>
                    <option value="ACCEPT">"Accept"</option>
                </select>
                <select id="vm-fw-protocol" on:change=move |ev| protocol.set(event_target_value(&ev))>
                    <option value="tcp">"TCP"</option>
                    <option value="udp">"UDP"</option>
                    <option value="icmp">"ICMP"</option>
                    <option value="any">"Any"</option>
                </select>
                <input id="vm-fw-dport" type="text" placeholder="port (22, 80-90)" prop:value=dport
                    on:input=move |ev| dport.set(event_target_value(&ev))/>
                <input id="vm-fw-source" type="text" placeholder="source IP/CIDR (any)" prop:value=source
                    on:input=move |ev| source.set(event_target_value(&ev))/>
                <input id="vm-fw-comment" type="text" placeholder="comment" prop:value=comment
                    on:input=move |ev| comment.set(event_target_value(&ev))/>
                <button type="submit" id="vm-fw-add" class="btn btn-primary">"Add rule"</button>
            </form>
        </fieldset>
    }
}
