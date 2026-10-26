//! VM hardware: network cards, CD-ROM (ISO) and boot order - the fields
//! shared by Create VM and the VM's Hardware page - plus the Hardware page
//! itself (`/vms/:id/hardware`: name, CPUs, memory, hardware, and the power
//! actions Shutdown / Reboot / Reset).
use crate::api::{self, VmConfigUpdate, VmHostOptions};
use horcrux_common::{
    BootDevice, Firmware, MachineType, NicModel, VmConfig, VmDisk, VmHardware, VmHypervisor, VmNic,
    VmStatus,
};
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

/// CPU models offered in the form (value, label); any other model can be
/// typed into the "custom" field.
pub const CPU_TYPES: [(&str, &str); 7] = [
    ("", "Default (QEMU's own)"),
    (
        "host",
        "host (all host CPU features; KVM only, same CPU on every migration target)",
    ),
    ("max", "max (everything QEMU can offer)"),
    (
        "x86-64-v2-AES",
        "x86-64-v2-AES (portable, Proxmox's default)",
    ),
    ("x86-64-v3", "x86-64-v3 (AVX2; Haswell / Zen and newer)"),
    ("kvm64", "kvm64"),
    ("qemu64", "qemu64"),
];

/// Disk buses offered in the forms (value, label).
pub const DISK_BUSES: [(&str, &str); 5] = [
    (
        "virtio",
        "VirtIO Block (fastest; Linux, Windows with virtio drivers)",
    ),
    ("scsi", "SCSI (VirtIO SCSI; discard + SSD flag)"),
    ("sata", "SATA (works without drivers)"),
    ("ide", "IDE (very old guests)"),
    (
        "default",
        "Legacy default (IDE; how disks were attached before buses were selectable)",
    ),
];

/// Host cache modes (value, label).
pub const DISK_CACHES: [(&str, &str); 6] = [
    ("", "Default (writeback)"),
    ("none", "No cache (direct I/O)"),
    ("writethrough", "Write through"),
    ("writeback", "Write back"),
    ("directsync", "Direct sync"),
    ("unsafe", "Unsafe (ignores flushes)"),
];

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

        <SystemFields hardware=hardware options=options running=running/>
    }
}

/// CPU model and topology, chipset, firmware and TPM of a QEMU VM.
#[component]
pub fn SystemFields(
    hardware: RwSignal<VmHardware>,
    #[prop(into)] options: Signal<Option<VmHostOptions>>,
    #[prop(into, default = Signal::derive(|| false))] running: Signal<bool>,
) -> impl IntoView {
    let ovmf = move || options.get().map(|o| o.ovmf).unwrap_or(true);
    let swtpm = move || options.get().map(|o| o.swtpm).unwrap_or(true);
    let cpu_value = move || hardware.get().cpu_type.unwrap_or_default();
    let is_listed = move || CPU_TYPES.iter().any(|(v, _)| *v == cpu_value());
    view! {
        <fieldset class="vm-hardware" id="vm-system">
            <legend>"System"</legend>
            <div class="form-row">
                <div class="form-group">
                    <label for="vm-cpu-type">"CPU type"</label>
                    <select id="vm-cpu-type" disabled={move || running.get()} on:change=move |ev| {
                        let v = event_target_value(&ev);
                        hardware.update(|h| {
                            h.cpu_type = match v.as_str() {
                                "" => None,
                                "custom" => Some(
                                    h.cpu_type
                                        .clone()
                                        .filter(|c| !CPU_TYPES.iter().any(|(x, _)| x == c))
                                        .unwrap_or_else(|| "EPYC".to_string()),
                                ),
                                other => Some(other.to_string()),
                            }
                        });
                    }>
                        {CPU_TYPES.iter().map(|(v, label)| {
                            let v = *v;
                            view! { <option value=v selected={move || cpu_value() == v}>{*label}</option> }
                        }).collect_view()}
                        <option value="custom" selected={move || !is_listed()}>"Other model..."</option>
                    </select>
                    <Show when=move || !is_listed()>
                        <input id="vm-cpu-custom" type="text" placeholder="QEMU CPU model, e.g. EPYC-v4, Skylake-Server"
                            disabled={move || running.get()}
                            prop:value=cpu_value
                            on:change=move |ev| {
                                let v = event_target_value(&ev).trim().to_string();
                                hardware.update(|h| h.cpu_type = (!v.is_empty()).then_some(v));
                            }/>
                    </Show>
                </div>
                <div class="form-group">
                    <label for="vm-sockets">"Sockets"</label>
                    <input id="vm-sockets" type="number" min="1" max="16" disabled={move || running.get()}
                        prop:value={move || hardware.get().sockets.unwrap_or(1)}
                        on:change=move |ev| {
                            let n = event_target_value(&ev).trim().parse::<u32>().unwrap_or(1).max(1);
                            hardware.update(|h| h.sockets = (n > 1).then_some(n));
                        }/>
                    <small class="form-help">"The CPUs are split evenly over the sockets (CPUs must be a multiple)."</small>
                </div>
            </div>
            <div class="form-row">
                <div class="form-group">
                    <label for="vm-machine">"Chipset"</label>
                    <select id="vm-machine" disabled={move || running.get()} on:change=move |ev| {
                        let m = if event_target_value(&ev) == "q35" { MachineType::Q35 } else { MachineType::I440fx };
                        hardware.update(|h| h.machine = m);
                    }>
                        <option value="i440fx" selected={move || hardware.get().machine == MachineType::I440fx}>"i440FX (default)"</option>
                        <option value="q35" selected={move || hardware.get().machine == MachineType::Q35}>"Q35 (PCIe; for UEFI / Windows 11)"</option>
                    </select>
                </div>
                <div class="form-group">
                    <label for="vm-firmware">"Firmware"</label>
                    <select id="vm-firmware" disabled={move || running.get()} on:change=move |ev| {
                        let f = if event_target_value(&ev) == "ovmf" { Firmware::Ovmf } else { Firmware::Seabios };
                        hardware.update(|h| h.firmware = f);
                    }>
                        <option value="seabios" selected={move || hardware.get().firmware == Firmware::Seabios}>"SeaBIOS (legacy BIOS)"</option>
                        <option value="ovmf" disabled={move || !ovmf()} selected={move || hardware.get().firmware == Firmware::Ovmf}>"OVMF (UEFI)"</option>
                    </select>
                    {move || (!ovmf()).then(|| view! {
                        <small class="form-help" id="vm-no-ovmf">"UEFI needs OVMF on this node (sys-firmware/edk2-bin)."</small>
                    })}
                </div>
                <div class="form-group">
                    <label>
                        <input id="vm-tpm" type="checkbox" disabled={move || running.get() || (!swtpm() && !hardware.get().tpm)}
                            prop:checked={move || hardware.get().tpm}
                            on:change=move |ev| {
                                let on = event_target_checked(&ev);
                                hardware.update(|h| h.tpm = on);
                            }/>
                        " TPM 2.0 (emulated, swtpm)"
                    </label>
                    {move || (!swtpm()).then(|| view! {
                        <small class="form-help" id="vm-no-swtpm">"A TPM needs swtpm on this node (app-crypt/swtpm)."</small>
                    })}
                </div>
            </div>
        </fieldset>
    }
}

/// Disk bus / cache / option controls of one disk (Create VM and Add disk).
#[component]
pub fn DiskOptionFields(disk: RwSignal<VmDisk>, #[prop(into)] prefix: String) -> impl IntoView {
    let p = prefix.clone();
    view! {
        <select class="vm-disk-bus" id=format!("{p}-bus") on:change=move |ev| {
            let b = event_target_value(&ev);
            disk.update(|d| {
                d.disk_type = b;
                if d.disk_type == "virtio" { d.ssd = false; }
                if !matches!(d.disk_type.as_str(), "virtio" | "scsi") { d.iothread = false; }
                if d.disk_type == "default" { d.discard = false; d.ssd = false; }
            });
        }>
            {DISK_BUSES.iter().map(|(v, label)| {
                let v = *v;
                view! { <option value=v selected={move || disk.get().disk_type == v}>{*label}</option> }
            }).collect_view()}
        </select>
        <select class="vm-disk-cache" id=format!("{prefix}-cache") on:change=move |ev| {
            let c = event_target_value(&ev);
            disk.update(|d| d.cache = c);
        }>
            {DISK_CACHES.iter().map(|(v, label)| {
                let v = *v;
                view! { <option value=v selected={move || disk.get().cache == v}>{*label}</option> }
            }).collect_view()}
        </select>
        <label><input type="checkbox" class="vm-disk-discard" prop:checked={move || disk.get().discard}
            disabled={move || disk.get().disk_type == "default"}
            on:change=move |ev| { let v = event_target_checked(&ev); disk.update(|d| d.discard = v) }/>" Discard"</label>
        <label><input type="checkbox" class="vm-disk-ssd" prop:checked={move || disk.get().ssd}
            disabled={move || matches!(disk.get().disk_type.as_str(), "virtio" | "default")}
            on:change=move |ev| { let v = event_target_checked(&ev); disk.update(|d| d.ssd = v) }/>" SSD"</label>
        <label><input type="checkbox" class="vm-disk-iothread" prop:checked={move || disk.get().iothread}
            disabled={move || !matches!(disk.get().disk_type.as_str(), "virtio" | "scsi")}
            on:change=move |ev| { let v = event_target_checked(&ev); disk.update(|d| d.iothread = v) }/>" I/O thread"</label>
    }
}

/// Storage pool a new disk is created in (Create VM and Add disk). Empty =
/// the node's VM storage directory (the default, where every disk went
/// before pools could hold VM disks).
#[component]
pub fn DiskStorageSelect(
    disk: RwSignal<VmDisk>,
    #[prop(into)] options: Signal<Option<VmHostOptions>>,
    #[prop(into)] id: String,
) -> impl IntoView {
    view! {
        <select class="vm-disk-storage" id=id title="Storage pool"
            on:change=move |ev| {
                let v = event_target_value(&ev);
                disk.update(|d| d.storage = (!v.is_empty()).then_some(v));
            }>
            <option value="" selected={move || disk.get().storage.is_none()}>"Default (VM storage)"</option>
            {move || options.get().map(|o| o.storage_pools).unwrap_or_default().into_iter().map(|p| {
                let pid = p.id.clone();
                let label = format!("{} ({}, {} GB free)", p.name, p.storage_type, p.available);
                view! {
                    <option value=p.id.clone() selected={move || disk.get().storage.as_deref() == Some(pid.as_str())}>{label}</option>
                }
            }).collect_view()}
        </select>
    }
}

/// The VM's disks on its Hardware page: options/size per disk, add, detach.
#[component]
fn VmDisks(
    #[prop(into)] vm_id: Signal<String>,
    vm: ReadSignal<Option<VmConfig>>,
    set_vm: WriteSignal<Option<VmConfig>>,
    #[prop(into)] running: Signal<bool>,
    #[prop(into)] options: Signal<Option<VmHostOptions>>,
) -> impl IntoView {
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let new_disk = create_rw_signal(VmDisk {
        size_gb: 8,
        disk_type: "virtio".into(),
        ..Default::default()
    });
    let done = move |r: Result<VmConfig, api::ApiError>, msg: String| match r {
        Ok(v) => {
            set_error.set(None);
            set_notice.set(Some(msg));
            set_vm.set(Some(v));
        }
        Err(e) => {
            set_notice.set(None);
            set_error.set(Some(e.message));
        }
    };
    let add = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let d = new_disk.get();
        let id = vm_id.get_untracked();
        spawn_local(async move {
            let spec = api::VmDiskSpec {
                size_gb: d.size_gb,
                bus: d.disk_type.clone(),
                cache: d.cache.clone(),
                discard: d.discard,
                ssd: d.ssd,
                iothread: d.iothread,
                storage: d.storage.clone(),
            };
            done(
                api::add_vm_disk(&id, &spec).await,
                format!("Added a {} GB disk", d.size_gb),
            );
        });
    };
    view! {
        <fieldset class="vm-hardware" id="vm-disks">
            <legend>"Disks"</legend>
            {move || notice.get().map(|n| view! { <div class="alert alert-info" id="vm-disk-notice">{n}</div> })}
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="vm-disk-error">{e}</div> })}
            {move || running.get().then(|| view! {
                <p class="form-help">"The VM is running: disks can be grown now; bus, cache, options, adding and detaching need it stopped."</p>
            })}
            {move || vm.get().map(|v| v.disks).unwrap_or_default().into_iter().enumerate().map(|(i, d)| {
                let row = create_rw_signal(d.clone());
                let size = create_rw_signal(d.size_gb);
                let delete_file = create_rw_signal(false);
                let apply = move |_| {
                    let cur = vm.get_untracked().and_then(|v| v.disks.get(i).cloned()).unwrap_or_default();
                    let next = row.get_untracked();
                    let upd = api::VmDiskUpdate {
                        size_gb: (size.get_untracked() != cur.size_gb).then(|| size.get_untracked()),
                        bus: (next.disk_type != cur.disk_type).then(|| next.disk_type.clone()),
                        cache: (next.cache != cur.cache).then(|| next.cache.clone()),
                        discard: (next.discard != cur.discard).then_some(next.discard),
                        ssd: (next.ssd != cur.ssd).then_some(next.ssd),
                        iothread: (next.iothread != cur.iothread).then_some(next.iothread),
                    };
                    let id = vm_id.get_untracked();
                    spawn_local(async move {
                        done(api::update_vm_disk(&id, i, &upd).await, format!("disk{i} saved"));
                    });
                };
                let detach = move |_| {
                    let id = vm_id.get_untracked();
                    let del = delete_file.get_untracked();
                    spawn_local(async move {
                        let r = match api::remove_vm_disk(&id, i, del).await {
                            Ok(()) => api::get_vm(&id).await,
                            Err(e) => Err(e),
                        };
                        done(r, format!("disk{i} detached{}", if del { " and deleted" } else { "" }));
                    });
                };
                view! {
                    <div class="vm-disk-row form-row" data-disk=i>
                        <strong>{format!("disk{i}")}</strong>
                        <code class="vm-disk-path">{d.path.clone()}</code>
                        {d.storage.clone().map(|p| {
                            let label = move || options.get()
                                .and_then(|o| o.storage_pools.into_iter().find(|x| x.id == p))
                                .map(|x| x.name)
                                .unwrap_or_else(|| p.clone());
                            view! { <span class="type-badge vm-disk-pool">{label}</span> }
                        })}
                        <input class="vm-disk-size" type="number" min=d.size_gb.to_string() prop:value=size
                            on:input=move |ev| size.set(event_target_value(&ev).parse().unwrap_or(0))/>
                        " GB "
                        <DiskOptionFields disk=row prefix=format!("vm-disk{i}")/>
                        <button type="button" class="btn btn-sm btn-primary vm-disk-apply" on:click=apply>"Apply"</button>
                        {(i > 0).then(|| view! {
                            <label><input type="checkbox" class="vm-disk-delete-file" prop:checked=delete_file
                                on:change=move |ev| delete_file.set(event_target_checked(&ev))/>" delete image"</label>
                            <button type="button" class="btn btn-sm btn-danger vm-disk-detach" disabled={move || running.get()}
                                on:click=detach>"Detach"</button>
                        })}
                    </div>
                }
            }).collect_view()}
            <form class="form-row" id="vm-disk-add-form" on:submit=add>
                <strong>"New disk"</strong>
                <input id="vm-disk-add-size" type="number" min="1" prop:value={move || new_disk.get().size_gb}
                    on:input=move |ev| { let n = event_target_value(&ev).parse().unwrap_or(1); new_disk.update(|d| d.size_gb = n) }/>
                " GB "
                <DiskOptionFields disk=new_disk prefix="vm-disk-add"/>
                <DiskStorageSelect disk=new_disk options=options id="vm-disk-add-storage"/>
                <button type="submit" id="vm-disk-add" class="btn btn-secondary" disabled={move || running.get()}>"Add disk"</button>
            </form>
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

            // The form appears once the VM is loaded: rendered earlier, the
            // load would overwrite whatever the user had already changed.
            <Show when=move || vm.get().is_some() fallback=move || view! {
                <p class="form-help" id="vm-hw-loading">{move || if error.get().is_some() { "" } else { "Loading the VM..." }}</p>
            }>
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
                    <crate::pages::vm_devices::DeviceSwitches hardware=hardware running=running/>
                    <crate::pages::vm_advanced::AdvancedHardware hardware=hardware running=running/>
                    <crate::pages::vm_passthrough::HostPciPanel hardware=hardware running=running/>
                </Show>
                <div class="form-actions">
                    <button type="submit" id="vm-hw-save" class="btn btn-primary" disabled=busy>"Save"</button>
                </div>
            </form>
            </Show>

            <Show when=is_qemu>
                <VmDisks vm_id=Signal::derive(vm_id) vm=vm set_vm=set_vm running=running options=options/>
                <crate::pages::vm_devices::VmLiveDevices vm_id=Signal::derive(vm_id) vm=vm set_vm=set_vm hardware=hardware running=running/>
                <crate::pages::vm_advanced::VmHotplug vm_id=Signal::derive(vm_id) set_vm=set_vm running=running/>
                <GuestFirewall guest_id=Signal::derive(vm_id) container=false/>
            </Show>
        </div>
    }
}

/// A guest's own firewall (a VM's network cards, or a container's
/// interfaces): rules applied at once, to that guest's traffic only.
#[component]
pub fn GuestFirewall(
    #[prop(into)] guest_id: Signal<String>,
    /// A container (its veths) instead of a VM (its taps).
    container: bool,
) -> impl IntoView {
    let vm_id = guest_id;
    let (rules, set_rules) = create_signal(Vec::<api::GuestFirewallRule>::new());
    let (interfaces, set_interfaces) = create_signal(None::<(String, Vec<String>)>);
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
            if container {
                match api::get_container_firewall(&id).await {
                    Ok(fw) => {
                        set_rules.set(fw.rules);
                        set_interfaces.set(Some((fw.interface_prefix, fw.interfaces)));
                    }
                    Err(e) => set_error.set(Some(e.message)),
                }
            } else {
                match api::list_vm_firewall(&id).await {
                    Ok(r) => set_rules.set(r),
                    Err(e) => set_error.set(Some(e.message)),
                }
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
            let r = if container {
                api::add_container_firewall(&id, &rule).await
            } else {
                api::add_vm_firewall(&id, &rule).await
            };
            match r {
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
            let r = if container {
                api::delete_container_firewall(&id, &rule_id).await
            } else {
                api::delete_vm_firewall(&id, &rule_id).await
            };
            if let Err(e) = r {
                set_error.set(Some(e.message));
            }
            reload();
        });
    };

    view! {
        <fieldset class="vm-hardware" id="vm-firewall">
            <legend>"Firewall"</legend>
            <p class="form-help">
                {if container {
                    "Rules for this container's network interfaces only, applied immediately. In = traffic to the container, Out = traffic from it. Anything not matched is allowed."
                } else {
                    "Rules for this VM's network cards only, applied immediately. In = traffic to the VM, Out = traffic from it. Anything not matched is allowed."
                }}
            </p>
            {move || interfaces.get().map(|(prefix, ifs)| view! {
                <p class="form-help" id="ct-fw-interfaces">
                    {if ifs.is_empty() {
                        format!("Matches host interfaces {prefix}*: none right now (the container is stopped or has no bridged network); rules apply when it starts.")
                    } else {
                        format!("Applies to host interface(s): {}", ifs.join(", "))
                    }}
                </p>
            })}
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
