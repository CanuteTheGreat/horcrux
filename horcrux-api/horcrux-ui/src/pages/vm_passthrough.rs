//! VM hardware page: host PCI / mediated devices (`hardware.hostpci`).
//! Edits the form's hardware signal; the page's Save applies it (the host
//! refuses a device that is missing, not on vfio-pci or without an IOMMU
//! group, and the message is shown there).
use crate::api::passthrough as pt;
use horcrux_common::{VmHardware, VmHostPci};
use leptos::*;

#[component]
pub fn HostPciPanel(hardware: RwSignal<VmHardware>, running: Signal<bool>) -> impl IntoView {
    let (pci, set_pci) = create_signal(Vec::<pt::PciDevice>::new());
    let (mdevs, set_mdevs) = create_signal(Vec::<pt::MdevDevice>::new());
    let pick = create_rw_signal(String::new());
    let manual = create_rw_signal(String::new());
    spawn_local(async move {
        if let Ok(d) = pt::list_pci().await {
            set_pci.set(d);
        }
        if let Ok(m) = pt::mdev_devices().await {
            set_mdevs.set(m);
        }
    });
    let add = move |dev: String| {
        let dev = dev.trim().to_string();
        if dev.is_empty() {
            return;
        }
        // a device still on its host driver is handed over at each start
        let managed = pci
            .get_untracked()
            .iter()
            .any(|d| d.address.eq_ignore_ascii_case(&dev) && !d.vfio);
        hardware.update(|h| {
            if !h
                .hostpci
                .iter()
                .any(|d| d.device.eq_ignore_ascii_case(&dev))
            {
                h.hostpci.push(VmHostPci {
                    device: dev,
                    primary_gpu: false,
                    managed,
                    ..Default::default()
                });
            }
        });
    };
    view! {
        <fieldset class="form-section" id="vm-hostpci-fieldset">
            <legend>"Host PCI devices (passthrough)"</legend>
            <ul id="vm-hostpci-list">
                {move || hardware.get().hostpci.into_iter().enumerate().map(|(i, d)| {
                    let name = d.device.clone();
                    view! {
                        <li class="vm-hostpci-row" data-device=name.clone()>
                            <code>{name}</code>
                            <label>
                                <input type="checkbox" class="vm-hostpci-primary" disabled={move || running.get()}
                                    prop:checked=d.primary_gpu
                                    on:change=move |ev| {
                                        let on = event_target_checked(&ev);
                                        hardware.update(|h| {
                                            for (j, x) in h.hostpci.iter_mut().enumerate() {
                                                x.primary_gpu = on && i == j;
                                            }
                                        });
                                    }/>
                                " primary GPU"
                            </label>
                            <label title="Move the device from its host driver to vfio-pci when the VM starts, and give it back when the VM stops">
                                <input type="checkbox" class="vm-hostpci-managed" disabled={move || running.get()}
                                    prop:checked=d.managed
                                    on:change=move |ev| {
                                        let on = event_target_checked(&ev);
                                        hardware.update(|h| {
                                            if let Some(x) = h.hostpci.get_mut(i) {
                                                x.managed = on;
                                                if !on {
                                                    x.allow_display_loss = false;
                                                }
                                            }
                                        });
                                    }/>
                                " hand over at start"
                            </label>
                            <button type="button" class="btn btn-sm btn-danger vm-hostpci-remove" disabled={move || running.get()}
                                on:click=move |_| hardware.update(|h| { h.hostpci.remove(i); })>"Remove"</button>
                        </li>
                    }
                }).collect_view()}
            </ul>
            {move || hardware.get().hostpci.is_empty().then(|| view! {
                <p class="form-help" id="vm-hostpci-none">"No host devices assigned."</p>
            })}
            <div class="form-row">
                <select id="vm-hostpci-pick" aria-label="PCI device on this node" prop:value=pick on:change=move |ev| pick.set(event_target_value(&ev))>
                    <option value="">"device on this node..."</option>
                    {move || pci.get().into_iter()
                        .filter(|d| d.vms.is_empty() && d.class_name != "bridge" && d.iommu_group.is_some() && (d.vfio || d.class_name == "gpu"))
                        .map(|d| view! {
                        <option value=d.address.clone()>
                            {format!("{} {} {} {}:{}{}", d.address, d.class_name, d.vendor_name, d.vendor_id, d.device_id,
                                if d.vfio { "" } else { " (handed over at start)" })}
                        </option>
                    }).collect_view()}
                    {move || mdevs.get().into_iter().filter(|m| m.vm.is_none()).map(|m| view! {
                        <option value=m.uuid.clone()>{format!("vGPU {} ({})", m.uuid, m.mdev_type)}</option>
                    }).collect_view()}
                </select>
                <button type="button" class="btn btn-primary" id="vm-hostpci-add" disabled={move || running.get()}
                    on:click=move |_| { add(pick.get_untracked()); pick.set(String::new()); }>"Add"</button>
            </div>
            <div class="form-row">
                <input id="vm-hostpci-manual" type="text" class="w-wide" aria-label="PCI address or mediated device UUID" placeholder="0000:01:00.0 or mediated device UUID"
                    prop:value=manual on:input=move |ev| manual.set(event_target_value(&ev))/>
                <button type="button" class="btn" id="vm-hostpci-add-manual" disabled={move || running.get()}
                    on:click=move |_| { add(manual.get_untracked()); manual.set(String::new()); }>"Add by address"</button>
            </div>
            <small class="form-help">
                "Devices bound to vfio-pci and GPUs (handed over at each start) appear in the list (bind others under "
                <a href="/gpu">"Infrastructure > GPU Passthrough"</a>
                "). Takes effect at the next start. A VM with passthrough hardware cannot be live-migrated or have its state saved."
            </small>
        </fieldset>
    }
}
