//! Host devices / PCI passthrough page: IOMMU status, every PCI device with
//! its IOMMU group (bind to / release from vfio-pci) and mediated devices
//! (vGPU profiles: create, delete). VMs get a device on their Hardware page.
use crate::api::passthrough as pt;
use leptos::*;

fn is_blocked(d: &pt::PciDevice) -> bool {
    d.class_name == "bridge"
}

#[component]
pub fn GpuManagement() -> impl IntoView {
    let (status, set_status) = create_signal(None::<pt::PciStatus>);
    let (devices, set_devices) = create_signal(Vec::<pt::PciDevice>::new());
    let (types, set_types) = create_signal(Vec::<pt::MdevType>::new());
    let (mdevs, set_mdevs) = create_signal(Vec::<pt::MdevDevice>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let class = create_rw_signal("all".to_string());
    let mdev_pick = create_rw_signal(String::new());

    let reload = move || {
        spawn_local(async move {
            match pt::status().await {
                Ok(s) => set_status.set(Some(s)),
                Err(e) => set_error.set(Some(format!("IOMMU status: {}", e.message))),
            }
            match pt::list_pci().await {
                Ok(d) => set_devices.set(d),
                Err(e) => set_error.set(Some(format!("PCI devices: {}", e.message))),
            }
            if let Ok(t) = pt::mdev_types().await {
                set_types.set(t);
            }
            if let Ok(m) = pt::mdev_devices().await {
                set_mdevs.set(m);
            }
            set_loading.set(false);
        });
    };
    reload();

    let bind = move |addr: String, on: bool| {
        spawn_local(async move {
            set_error.set(None);
            set_notice.set(None);
            let r = if on {
                pt::bind_vfio(&addr).await
            } else {
                pt::unbind_vfio(&addr).await
            };
            match r {
                Ok(_) => set_notice.set(Some(if on {
                    format!("{addr} is bound to vfio-pci")
                } else {
                    format!("{addr} was given back to its host driver")
                })),
                Err(e) => set_error.set(Some(e.message)),
            }
            reload();
        });
    };

    let create_mdev = move |_| {
        let pick = mdev_pick.get_untracked();
        let Some((parent, ty)) = pick.split_once('|') else {
            set_error.set(Some("Pick a mediated device type first".into()));
            return;
        };
        let (parent, ty) = (parent.to_string(), ty.to_string());
        spawn_local(async move {
            set_error.set(None);
            match pt::create_mdev(&parent, &ty).await {
                Ok(v) => set_notice.set(Some(format!(
                    "Created mediated device {}",
                    v["uuid"].as_str().unwrap_or("")
                ))),
                Err(e) => set_error.set(Some(e.message)),
            }
            reload();
        });
    };
    let delete_mdev = move |uuid: String| {
        spawn_local(async move {
            set_error.set(None);
            match pt::delete_mdev(&uuid).await {
                Ok(()) => set_notice.set(Some(format!("Removed mediated device {uuid}"))),
                Err(e) => set_error.set(Some(e.message)),
            }
            reload();
        });
    };

    let shown = move || {
        let c = class.get();
        devices
            .get()
            .into_iter()
            // bridges and virtual functions' parents are noise unless asked for
            .filter(|d| c == "all" && !is_blocked(d) || c == d.class_name)
            .collect::<Vec<_>>()
    };

    view! {
        <div class="page gpu-management">
            <header class="page-header">
                <h2>"Host Devices (PCI passthrough)"</h2>
                <button class="btn btn-primary" id="pci-refresh" on:click=move |_| reload()>"Refresh"</button>
            </header>

            <div class="status-card" id="pci-status">
                {move || match status.get() {
                    Some(s) => view! {
                        <div class={format!("iommu-status {}", if s.iommu_enabled { "enabled" } else { "disabled" })}>
                            <span class="icon">{if s.iommu_enabled { "[OK]" } else { "[X]" }}</span>
                            <span class="message">{s.message}</span>
                        </div>
                    }.into_view(),
                    None => view! { <span>"Checking IOMMU..."</span> }.into_view(),
                }}
            </div>

            {move || error.get().map(|e| view! {
                <div class="alert alert-error" id="pci-error">
                    <span>{e}</span>
                    <button class="btn-close" on:click=move |_| set_error.set(None)>"x"</button>
                </div>
            })}
            {move || notice.get().map(|n| view! { <div class="alert alert-success" id="pci-notice">{n}</div> })}

            <h3>"PCI devices"</h3>
            <div class="form-row">
                <label for="pci-class">"Show "</label>
                <select id="pci-class" prop:value=class on:change=move |ev| class.set(event_target_value(&ev))>
                    <option value="all">"all (except bridges)"</option>
                    <option value="gpu">"GPUs"</option>
                    <option value="network">"Network"</option>
                    <option value="storage">"Storage / NVMe"</option>
                    <option value="usb">"USB controllers"</option>
                    <option value="accelerator">"Accelerators"</option>
                    <option value="bridge">"Bridges"</option>
                </select>
            </div>
            {move || loading.get().then(|| view! { <p>"Loading PCI devices..."</p> })}
            <table class="data-table" id="pci-table">
                <thead><tr>
                    <th>"Address"</th><th>"Class"</th><th>"Device"</th><th>"Driver"</th>
                    <th>"IOMMU group"</th><th>"Used by"</th><th>"Actions"</th>
                </tr></thead>
                <tbody>
                    {move || shown().into_iter().map(|d| {
                        let a1 = d.address.clone();
                        let a2 = d.address.clone();
                        let used = d.vms.join(", ");
                        let busy = !d.vms.is_empty();
                        let blockers = d.group_blockers.clone();
                        let sriov = (d.sriov_max_vfs > 0).then(|| format!(" SR-IOV {}/{} VFs", d.sriov_vfs, d.sriov_max_vfs));
                        view! {
                            <tr class="pci-row" data-addr=d.address.clone()>
                                <td><code>{d.address.clone()}</code></td>
                                <td>{d.class_name.clone()}</td>
                                <td>{format!("{} {}:{}", d.vendor_name, d.vendor_id, d.device_id)}{sriov}</td>
                                <td class="pci-driver">{d.driver.clone().unwrap_or_else(|| "none".into())}</td>
                                <td>
                                    {d.iommu_group.clone().unwrap_or_else(|| "none".into())}
                                    {(!blockers.is_empty() && d.iommu_group.is_some()).then(|| view! {
                                        <small class="form-help pci-blockers">{format!(" group also holds {} still on a host driver", blockers.join(", "))}</small>
                                    })}
                                </td>
                                <td class="pci-used">{used}</td>
                                <td>
                                    {if is_blocked(&d) {
                                        view! { <span class="text-muted">"bridge"</span> }.into_view()
                                    } else if busy {
                                        view! { <span class="text-muted">"assigned to a VM"</span> }.into_view()
                                    } else if d.vfio {
                                        view! { <button class="btn btn-sm btn-warning pci-unbind" on:click=move |_| bind(a1.clone(), false)>"Release from vfio-pci"</button> }.into_view()
                                    } else if d.iommu_group.is_none() {
                                        view! { <span class="text-muted">"no IOMMU group"</span> }.into_view()
                                    } else {
                                        view! { <button class="btn btn-sm btn-success pci-bind" on:click=move |_| bind(a2.clone(), true)>"Bind to vfio-pci"</button> }.into_view()
                                    }}
                                </td>
                            </tr>
                        }
                    }).collect_view()}
                </tbody>
            </table>
            {move || (!loading.get() && shown().is_empty()).then(|| view! {
                <p id="pci-empty" class="form-help">"No PCI devices of this class on this node."</p>
            })}

            <h3>"Mediated devices (vGPU)"</h3>
            <div id="mdev-section">
                {move || types.get().is_empty().then(|| view! {
                    <p id="mdev-none" class="form-help">"This node's hardware offers no mediated device types (NVIDIA vGPU host driver or Intel GVT-g needed)."</p>
                })}
                <div class="form-row" style:display=move || if types.get().is_empty() { "none" } else { "flex" }>
                    <select id="mdev-type" prop:value=mdev_pick on:change=move |ev| mdev_pick.set(event_target_value(&ev))>
                        <option value="">"type to create..."</option>
                        {move || types.get().into_iter().map(|t| view! {
                            <option value=format!("{}|{}", t.parent, t.id)>
                                {format!("{} {} ({}) - {} left", t.parent, t.id, t.name, t.available_instances)}
                            </option>
                        }).collect_view()}
                    </select>
                    <button class="btn btn-primary" id="mdev-create" on:click=create_mdev>"Create"</button>
                </div>
                <table class="data-table" id="mdev-table">
                    <thead><tr><th>"UUID"</th><th>"Parent"</th><th>"Type"</th><th>"Used by"</th><th>"Actions"</th></tr></thead>
                    <tbody>
                        {move || mdevs.get().into_iter().map(|m| {
                            let u = m.uuid.clone();
                            view! {
                                <tr class="mdev-row">
                                    <td><code>{m.uuid.clone()}</code></td>
                                    <td>{m.parent.clone()}</td>
                                    <td>{m.mdev_type.clone()}</td>
                                    <td>{m.vm.clone().unwrap_or_default()}</td>
                                    <td>
                                        {if m.vm.is_some() {
                                            view! { <span class="text-muted">"assigned to a VM"</span> }.into_view()
                                        } else {
                                            view! { <button class="btn btn-sm btn-danger mdev-delete" on:click=move |_| delete_mdev(u.clone())>"Delete"</button> }.into_view()
                                        }}
                                    </td>
                                </tr>
                            }
                        }).collect_view()}
                    </tbody>
                </table>
                {move || mdevs.get().is_empty().then(|| view! { <p id="mdev-empty" class="form-help">"No mediated devices created."</p> })}
            </div>

            <div class="info-box">
                <h4>"Passthrough in short"</h4>
                <ol>
                    <li>"Enable the IOMMU in the BIOS (Intel VT-d / AMD-Vi) and on the kernel command line (" <code>"intel_iommu=on"</code> " / " <code>"amd_iommu=on"</code> "), reboot."</li>
                    <li>"Bind the device (and every other non-bridge device of its IOMMU group) to vfio-pci above."</li>
                    <li>"Add it on the VM's Hardware page under \"Host PCI devices\"."</li>
                    <li>"A running VM with such a device cannot be live-migrated or have its state saved; HA only restarts it on a node that has the same device bound to vfio-pci."</li>
                </ol>
            </div>
        </div>
    }
}
