//! VM hardware page: advanced settings (`hardware.adv`): extra serial ports,
//! virtiofs shares, shared memory, SMBIOS, VM Generation ID, NUMA,
//! CPU/memory hotplug limits, protection, lock, hookscript - and the
//! hotplug actions for a running VM.
use crate::api::vm_devices as dev;
use horcrux_common::{VmConfig, VmHardware, VmIvshmem, VmSerialPort, VmSmbios, VmVirtiofs};
use leptos::*;

fn opt(s: String) -> Option<String> {
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// One serial port per line: empty / `socket` = Unix socket on the host,
/// `/dev/ttyUSB0` = host tty.
pub fn parse_serial(text: &str) -> Vec<VmSerialPort> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| VmSerialPort {
            device: (l != "socket").then(|| l.to_string()),
        })
        .collect()
}

pub fn serial_text(v: &[VmSerialPort]) -> String {
    v.iter()
        .map(|p| p.device.clone().unwrap_or_else(|| "socket".into()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// One share per line: `tag=/path`.
pub fn parse_virtiofs(text: &str) -> Vec<VmVirtiofs> {
    text.lines()
        .filter_map(|l| l.trim().split_once('='))
        .map(|(t, p)| VmVirtiofs {
            tag: t.trim().into(),
            path: p.trim().into(),
        })
        .collect()
}

pub fn virtiofs_text(v: &[VmVirtiofs]) -> String {
    v.iter()
        .map(|s| format!("{}={}", s.tag, s.path))
        .collect::<Vec<_>>()
        .join("\n")
}

#[component]
pub fn AdvancedHardware(hardware: RwSignal<VmHardware>, running: Signal<bool>) -> impl IntoView {
    let serial = create_rw_signal(serial_text(&hardware.get_untracked().adv.serial));
    let virtiofs = create_rw_signal(virtiofs_text(&hardware.get_untracked().adv.virtiofs));
    // keep the text boxes in step when the page loads the VM's hardware
    create_effect(move |_| {
        let h = hardware.get();
        serial.set(serial_text(&h.adv.serial));
        virtiofs.set(virtiofs_text(&h.adv.virtiofs));
    });
    let sm = move |f: fn(&VmSmbios) -> Option<String>| {
        hardware
            .get()
            .adv
            .smbios
            .as_ref()
            .and_then(f)
            .unwrap_or_default()
    };
    let set_sm = move |f: fn(&mut VmSmbios, Option<String>), v: String| {
        hardware.update(|h| {
            let mut s = h.adv.smbios.take().unwrap_or_default();
            f(&mut s, opt(v));
            h.adv.smbios = (s != VmSmbios::default()).then_some(s);
        })
    };
    view! {
        <fieldset class="form-section" id="vm-advanced-fieldset">
            <legend>"Advanced: serial, virtiofs, shared memory, SMBIOS, NUMA, hotplug, protection"</legend>
            <div class="form-row">
                <div class="form-group">
                    <label for="vm-adv-serial">"Extra serial ports (one per line: socket or /dev/ttyUSB0)"</label>
                    <textarea id="vm-adv-serial" rows="2" disabled={move || running.get()} prop:value=serial
                        on:input=move |ev| { let t = event_target_value(&ev); hardware.update(|h| h.adv.serial = parse_serial(&t)); }></textarea>
                </div>
                <div class="form-group">
                    <label for="vm-adv-virtiofs">"virtiofs shares (tag=/pool/dir, one per line)"</label>
                    <textarea id="vm-adv-virtiofs" rows="2" disabled={move || running.get()} prop:value=virtiofs
                        on:input=move |ev| { let t = event_target_value(&ev); hardware.update(|h| h.adv.virtiofs = parse_virtiofs(&t)); }></textarea>
                </div>
            </div>
            <div class="form-row">
                <div class="form-group">
                    <label for="vm-adv-shm-name">"Shared memory (ivshmem) name"</label>
                    <input id="vm-adv-shm-name" type="text" disabled={move || running.get()}
                        prop:value={move || hardware.get().adv.ivshmem.map(|i| i.name).unwrap_or_default()}
                        on:input=move |ev| { let n = event_target_value(&ev); hardware.update(|h| {
                            let size = h.adv.ivshmem.as_ref().map(|i| i.size_mb).unwrap_or(4);
                            h.adv.ivshmem = opt(n).map(|name| VmIvshmem { name, size_mb: size });
                        }); }/>
                </div>
                <div class="form-group">
                    <label for="vm-adv-shm-size">"Size (MB, power of two)"</label>
                    <input id="vm-adv-shm-size" type="number" min="1" max="1024" disabled={move || running.get()}
                        prop:value={move || hardware.get().adv.ivshmem.map(|i| i.size_mb).unwrap_or(4)}
                        on:input=move |ev| { let n = event_target_value(&ev).parse().unwrap_or(4); hardware.update(|h| {
                            if let Some(i) = h.adv.ivshmem.as_mut() { i.size_mb = n; }
                        }); }/>
                </div>
            </div>
            <div class="form-row">
                <div class="form-group"><label for="vm-adv-smbios-mf">"SMBIOS manufacturer"</label>
                    <input id="vm-adv-smbios-mf" type="text" disabled={move || running.get()} prop:value={move || sm(|s| s.manufacturer.clone())}
                        on:input=move |ev| set_sm(|s, v| s.manufacturer = v, event_target_value(&ev))/></div>
                <div class="form-group"><label for="vm-adv-smbios-product">"Product"</label>
                    <input id="vm-adv-smbios-product" type="text" disabled={move || running.get()} prop:value={move || sm(|s| s.product.clone())}
                        on:input=move |ev| set_sm(|s, v| s.product = v, event_target_value(&ev))/></div>
                <div class="form-group"><label for="vm-adv-smbios-serial">"Serial"</label>
                    <input id="vm-adv-smbios-serial" type="text" disabled={move || running.get()} prop:value={move || sm(|s| s.serial.clone())}
                        on:input=move |ev| set_sm(|s, v| s.serial = v, event_target_value(&ev))/></div>
            </div>
            <div class="form-row">
                <label><input id="vm-adv-genid" type="checkbox" disabled={move || running.get()}
                    prop:checked={move || hardware.get().adv.vmgenid.is_some()}
                    on:change=move |ev| { let on = event_target_checked(&ev); hardware.update(|h| h.adv.vmgenid = on.then(|| h.adv.vmgenid.clone().unwrap_or_else(|| "auto".into()))); }/>
                    " VM Generation ID"</label>
                <label><input id="vm-adv-numa" type="checkbox" disabled={move || running.get()}
                    prop:checked={move || hardware.get().adv.numa}
                    on:change=move |ev| { let on = event_target_checked(&ev); hardware.update(|h| h.adv.numa = on); }/>
                    " NUMA (one node per socket)"</label>
                <div class="form-group"><label for="vm-adv-maxcpus">"Max CPUs (hotplug)"</label>
                    <input id="vm-adv-maxcpus" type="number" min="1" disabled={move || running.get()}
                        prop:value={move || hardware.get().adv.max_cpus.map(|v| v.to_string()).unwrap_or_default()}
                        on:input=move |ev| { let v = event_target_value(&ev).parse().ok(); hardware.update(|h| h.adv.max_cpus = v); }/></div>
                <div class="form-group"><label for="vm-adv-maxmem">"Max memory MB (hotplug)"</label>
                    <input id="vm-adv-maxmem" type="number" min="1" disabled={move || running.get()}
                        prop:value={move || hardware.get().adv.max_memory_mb.map(|v| v.to_string()).unwrap_or_default()}
                        on:input=move |ev| { let v = event_target_value(&ev).parse().ok(); hardware.update(|h| h.adv.max_memory_mb = v); }/></div>
            </div>
            <div class="form-row">
                <label><input id="vm-adv-protection" type="checkbox"
                    prop:checked={move || hardware.get().adv.protection}
                    on:change=move |ev| { let on = event_target_checked(&ev); hardware.update(|h| h.adv.protection = on); }/>
                    " Protection (refuse delete / disk removal)"</label>
                <label><input id="vm-adv-lock" type="checkbox"
                    prop:checked={move || hardware.get().adv.lock.is_some()}
                    on:change=move |ev| { let on = event_target_checked(&ev); hardware.update(|h| h.adv.lock = on.then(|| "manual".to_string())); }/>
                    " Locked (refuse start/stop/delete/config until unlocked)"</label>
                <div class="form-group"><label for="vm-adv-hook">"Hookscript (an executable snippet)"</label>
                    <input id="vm-adv-hook" type="text" placeholder="/pool/snippets/hook.sh"
                        prop:value={move || hardware.get().adv.hookscript.unwrap_or_default()}
                        on:input=move |ev| { let v = opt(event_target_value(&ev)); hardware.update(|h| h.adv.hookscript = v); }/></div>
            </div>
        </fieldset>
    }
}

/// Hotplug actions of a running VM.
#[component]
pub fn VmHotplug(
    #[prop(into)] vm_id: Signal<String>,
    set_vm: WriteSignal<Option<VmConfig>>,
    running: Signal<bool>,
) -> impl IntoView {
    let cpus = create_rw_signal(String::new());
    let mem = create_rw_signal(String::new());
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let add_cpus = move |_| {
        let id = vm_id.get_untracked();
        let Ok(n) = cpus.get_untracked().trim().parse::<u32>() else {
            set_error.set(Some("Enter the total number of CPUs".into()));
            return;
        };
        spawn_local(async move {
            match dev::hotplug_cpu(&id, n).await {
                Ok(v) => {
                    set_error.set(None);
                    set_notice.set(Some(format!("The VM has {} CPUs now", v.cpus)));
                    set_vm.set(Some(v));
                }
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };
    let add_mem = move |_| {
        let id = vm_id.get_untracked();
        let Ok(n) = mem.get_untracked().trim().parse::<u64>() else {
            set_error.set(Some("Enter the memory to add in MB".into()));
            return;
        };
        spawn_local(async move {
            match dev::hotplug_memory(&id, n).await {
                Ok(v) => {
                    set_error.set(None);
                    set_notice.set(Some(format!("The VM has {} MB now", v.memory)));
                    set_vm.set(Some(v));
                }
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };
    view! {
        <Show when=move || running.get()>
            <div class="vm-hotplug" id="vm-hotplug">
                <h4>"Hotplug (running VM)"</h4>
                {move || notice.get().map(|n| view! { <div class="alert alert-info" id="hotplug-notice">{n}</div> })}
                {move || error.get().map(|e| view! { <div class="alert alert-error" id="hotplug-error">{e}</div> })}
                <div class="form-row">
                    <input id="hotplug-cpus" type="number" min="1" placeholder="total CPUs" prop:value=cpus
                        on:input=move |ev| cpus.set(event_target_value(&ev))/>
                    <button id="hotplug-cpu-add" type="button" class="btn btn-primary" on:click=add_cpus>"Add CPUs"</button>
                    <input id="hotplug-mem" type="number" min="128" step="128" placeholder="MB to add" prop:value=mem
                        on:input=move |ev| mem.set(event_target_value(&ev))/>
                    <button id="hotplug-mem-add" type="button" class="btn btn-primary" on:click=add_mem>"Add memory"</button>
                </div>
                <small class="form-help">"Needs max CPUs / max memory set in the hardware before the VM started."</small>
            </div>
        </Show>
    }
}
