use crate::api;
use horcrux_common::{VmArchitecture, VmConfig, VmHypervisor, VmStatus};
use leptos::*;
use leptos_router::*;

/// (value, label) of every VM hypervisor, in menu order.
const HYPERVISORS: [(&str, &str); 3] = [("qemu", "QEMU/KVM"), ("incus", "Incus"), ("lxd", "LXD")];

fn hypervisor_from(value: &str) -> VmHypervisor {
    match value {
        "lxd" => VmHypervisor::Lxd,
        "incus" => VmHypervisor::Incus,
        _ => VmHypervisor::Qemu,
    }
}

/// The first free `vm-<N>` ID from 100 up (Proxmox offers the next free VMID the same way).
pub fn next_free_vm_id(existing: &[String]) -> String {
    (100u32..)
        .map(|n| format!("vm-{n}"))
        .find(|id| !existing.iter().any(|e| e == id))
        .unwrap_or_else(|| "vm-100".into())
}

/// Why `id` can't be a new VM's ID (same rules as the server), or `None`.
pub fn vm_id_problem(id: &str, existing: &[String]) -> Option<String> {
    let id = id.trim();
    if id.is_empty() {
        return Some("Enter a VM ID".into());
    }
    if id.len() > 64 {
        return Some("The VM ID can have at most 64 characters".into());
    }
    if id.starts_with('-') || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Some("The VM ID can only contain letters, digits and hyphens (not first)".into());
    }
    if existing.iter().any(|e| e == id) {
        return Some(format!("A VM with the ID {id} already exists"));
    }
    None
}

/// Why `name` is not a valid VM name (same rules as the server), or `None`.
pub fn vm_name_problem(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return Some("Enter a name".into());
    }
    if name.len() > 255 {
        return Some("The name can have at most 255 characters".into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Some("The name can only contain letters, digits, '-' and '_'".into());
    }
    None
}

#[component]
pub fn VmCreate() -> impl IntoView {
    let (name, set_name) = create_signal(String::new());
    // VM ID: the next free one, editable (like Proxmox's VM ID field).
    let (vm_id, set_vm_id) = create_signal(String::new());
    let (id_touched, set_id_touched) = create_signal(false);
    let (existing_ids, set_existing_ids) = create_signal(Vec::<String>::new());
    let (start_after, set_start_after) = create_signal(false);
    let (name_touched, set_name_touched) = create_signal(false);
    spawn_local(async move {
        if let Ok(vms) = api::get_vms().await {
            let ids: Vec<String> = vms.into_iter().map(|v| v.id).collect();
            if !id_touched.get_untracked() {
                set_vm_id.set(next_free_vm_id(&ids));
            }
            set_existing_ids.set(ids);
        }
    });
    let id_problem = move || vm_id_problem(&vm_id.get(), &existing_ids.get());
    let name_problem = move || vm_name_problem(&name.get());
    let cpus = create_rw_signal(2u64);
    let memory = create_rw_signal(2048u64);
    let disk_size = create_rw_signal(32u64);
    let (hypervisor, set_hypervisor) = create_signal(VmHypervisor::Qemu);
    let (architecture, set_architecture) = create_signal(VmArchitecture::X86_64);
    let display = create_rw_signal(horcrux_common::VmDisplay::default());
    let (image, set_image) = create_signal(String::new());
    // Network cards, CD-ROM and boot order (QEMU).
    let hardware = create_rw_signal(horcrux_common::VmHardware::default());
    // The first disk's bus/cache/options (its size is "Disk Size") and any
    // additional disks (QEMU).
    let disk0 = create_rw_signal(horcrux_common::VmDisk {
        disk_type: "virtio".into(),
        ..Default::default()
    });
    // Each extra disk is its own signal, so editing a row only touches that
    // row; the list signal changes only on Add/Remove. (Syncing row edits
    // back into one Vec<VmDisk> from a per-row effect re-rendered the rows,
    // which re-created the effects, which wrote again - an endless loop that
    // crashed the tab on the first "Add disk".)
    let extra_disks = create_rw_signal(Vec::<RwSignal<horcrux_common::VmDisk>>::new());
    // Row signals are created in the click handler; own them by this
    // component so they are disposed with the page.
    let owner = Owner::current();
    let (host_options, set_host_options) = create_signal(None::<api::VmHostOptions>);
    let load_options = move |first: bool| {
        spawn_local(async move {
            if let Ok(o) = api::get_vm_host_options().await {
                if first {
                    // Every new VM gets a NIC on the global default network if
                    // one is set, else the hypervisor's own (QEMU: vmbr0, else
                    // the bridge the host reaches the network through); the
                    // user can still change or remove it.
                    let dn = &o.default_networks;
                    let bridge = dn.override_network.clone().or_else(|| dn.qemu.clone());
                    if let Some(bridge) = bridge {
                        hardware.update(|h| {
                            if h.nets.is_empty() {
                                h.nets.push(horcrux_common::VmNic {
                                    model: horcrux_common::NicModel::Virtio,
                                    bridge,
                                    attach: Default::default(),
                                    mac: None,
                                    vlan: None,
                                    rate_mbps: None,
                                })
                            }
                        });
                    }
                    // Default ISO is not preselected, but the sliders start
                    // inside what the host can give.
                    let cpu_max = o.host.cpus.max(1) as u64;
                    if cpus.get_untracked() > cpu_max {
                        cpus.set(cpu_max);
                    }
                    let mem_max = o.host.max_vm_memory_mb;
                    if mem_max >= 512 && memory.get_untracked() > mem_max {
                        memory.set(
                            *crate::components::memory_steps(mem_max)
                                .last()
                                .unwrap_or(&512),
                        );
                    }
                }
                set_host_options.set(Some(o));
            }
        });
    };
    load_options(true);
    let (creating, set_creating) = create_signal(false);
    let (error, set_error) = create_signal(None::<String>);
    // A VM that was created but whose next step (disk import, start) failed: Create
    // again only retries that step instead of making a second VM.
    let (created_id, set_created_id) = create_signal(None::<String>);

    // GB free where disk0 goes: the chosen pool, else the VM storage dir.
    let disk_free = move || {
        host_options
            .get()
            .map(|o| match disk0.get().storage {
                Some(id) => o
                    .storage_pools
                    .iter()
                    .find(|p| p.id == id)
                    .map(|p| p.available)
                    .unwrap_or(0),
                None => o.host.default_disk_available_gb,
            })
            .unwrap_or(256)
    };

    let navigate = use_navigate();
    let is_instance = move || matches!(hypervisor.get(), VmHypervisor::Lxd | VmHypervisor::Incus);

    let features = crate::features::use_features();
    create_effect(move |_| {
        let current = match hypervisor.get_untracked() {
            VmHypervisor::Qemu => "qemu",
            VmHypervisor::Lxd => "lxd",
            VmHypervisor::Incus => "incus",
        };
        if features.loaded() && !features.has(current) {
            if let Some((v, _)) = HYPERVISORS.iter().find(|(v, _)| features.has(v)) {
                set_hypervisor.set(hypervisor_from(v));
            }
        }
    });

    let submit = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        set_name_touched.set(true);
        set_id_touched.set(true);
        if let Some(p) = name_problem().or_else(id_problem) {
            set_error.set(Some(p));
            return;
        }

        set_creating.set(true);
        set_error.set(None);

        let config = VmConfig {
            id: vm_id.get().trim().to_string(),
            name: name.get().trim().to_string(),
            hypervisor: hypervisor.get(),
            memory: memory.get(),
            cpus: cpus.get() as u32,
            disk_size: disk_size.get(),
            status: VmStatus::Stopped,
            architecture: architecture.get(),
            disks: if matches!(hypervisor.get(), VmHypervisor::Qemu) {
                let mut d0 = disk0.get();
                d0.size_gb = disk_size.get();
                std::iter::once(d0)
                    .chain(extra_disks.get().iter().map(|d| d.get()))
                    .collect()
            } else {
                Vec::new()
            },
            display: display.get(),
            // Hardware settings are for QEMU VMs (LXD/Incus use profiles).
            hardware: if matches!(hypervisor.get(), VmHypervisor::Qemu) {
                hardware.get()
            } else {
                Default::default()
            },
        };

        let navigate = navigate.clone();
        let image = image.get().trim().to_string();
        // LXD/Incus VMs are created from an instance image; QEMU VMs get a
        // disk image imported after creation.
        let instance = matches!(config.hypervisor, VmHypervisor::Lxd | VmHypervisor::Incus);
        let already = created_id.get_untracked();
        spawn_local(async move {
            let result = match &already {
                Some(id) => api::get_vm(id).await,
                None if instance && !image.is_empty() => {
                    api::create_vm_from_image(config, &image).await
                }
                None => api::create_vm(config).await,
            };
            if let Ok(vm) = &result {
                set_created_id.set(Some(vm.id.clone()));
            }
            let created = match result {
                Ok(vm) if !image.is_empty() && !instance => api::import_vm_disk(&vm.id, &image)
                    .await
                    .map_err(|e| api::ApiError {
                        message: format!(
                            "VM {} created, but importing its disk failed: {}",
                            vm.id, e.message
                        ),
                    }),
                other => other,
            };
            match created {
                // Straight to the new VM's page (power controls, console), started if asked.
                Ok(vm) => {
                    if start_after.get_untracked() {
                        if let Err(e) = api::start_vm(&vm.id).await {
                            set_error.set(Some(format!(
                                "VM {} was created, but starting it failed: {}",
                                vm.id, e.message
                            )));
                            set_creating.set(false);
                            return;
                        }
                    }
                    navigate(&format!("/vms/{}", vm.id), Default::default());
                }
                Err(e) => {
                    set_error.set(Some(e.message));
                    set_creating.set(false);
                }
            }
        });
    };

    view! {
        <div class="vm-create">
            <h1>"Create Virtual Machine"</h1>

            <form on:submit=submit>
                <div class="form-group">
                    <label for="vm-name">"VM Name"</label>
                    <input
                        id="vm-name"
                        type="text"
                        required
                        placeholder="my-vm"
                        on:input=move |ev| set_name.set(event_target_value(&ev))
                        on:blur=move |_| set_name_touched.set(true)
                        prop:value=name
                    />
                    {move || (name_touched.get()).then(name_problem).flatten().map(|p| view! { <small class="error-text" id="vm-name-problem">{p}</small> })}
                </div>

                <div class="form-group">
                    <label for="vm-id">"VM ID"</label>
                    <input
                        id="vm-id"
                        type="text"
                        required
                        on:input=move |ev| { set_id_touched.set(true); set_vm_id.set(event_target_value(&ev)); }
                        prop:value=vm_id
                    />
                    {move || match id_problem() {
                        Some(p) if id_touched.get() => view! { <small class="error-text" id="vm-id-problem">{p}</small> }.into_view(),
                        _ => view! { <small>"Names the VM's disk files; the next free ID is filled in."</small> }.into_view(),
                    }}
                </div>

                <div class="form-group">
                    <label for="vm-hypervisor">"Hypervisor"</label>
                    // Only the hypervisors compiled into the server (USE flags).
                    <select id="vm-hypervisor" on:change=move |ev| {
                        set_hypervisor.set(hypervisor_from(&event_target_value(&ev)));
                    }>
                        {move || {
                            HYPERVISORS
                                .iter()
                                .filter(|(v, _)| features.has(v))
                                .map(|(v, label)| {
                                    let hv = hypervisor_from(v);
                                    view! { <option value=*v selected=move || hypervisor.get() == hv>{*label}</option> }
                                })
                                .collect_view()
                        }}
                    </select>
                    <Show when=move || features.loaded() && !HYPERVISORS.iter().any(|(v, _)| features.has(v))>
                        <p class="error" id="vm-no-hypervisor">
                            "No VM hypervisor is compiled into this server (USE=qemu, lxd or incus)."
                        </p>
                    </Show>
                </div>

                <div class="form-group">
                    <label for="vm-arch">"Architecture"</label>
                    <select id="vm-arch" on:change=move |ev| {
                        let value = event_target_value(&ev);
                        let arch = match value.as_str() {
                            "aarch64" => VmArchitecture::Aarch64,
                            "riscv64" => VmArchitecture::Riscv64,
                            "ppc64le" => VmArchitecture::Ppc64le,
                            _ => VmArchitecture::X86_64,
                        };
                        set_architecture.set(arch);

                    }>
                        <option value="x86_64">"x86_64 (amd64)"</option>
                        <option value="aarch64">"aarch64 (ARM64)"</option>
                        <option value="riscv64">"riscv64 (RISC-V)"</option>
                        <option value="ppc64le">"ppc64le (PowerPC)"</option>
                    </select>
                </div>

                <crate::components::ResourceSlider label="CPUs" id="vm-cpus" value=cpus
                    steps=Signal::derive(move || crate::components::cpu_steps(host_options.get().map(|o| o.host.cpus).unwrap_or(4)))
                    limit=Signal::derive(move || host_options.get().map(|o| o.host.cpus as u64).unwrap_or(u64::MAX))
                    hard_max=Signal::derive(move || host_options.get().map(|o| o.host.cpus as u64 * 4).unwrap_or(1024).max(4))
                    fmt=crate::components::human_cpus unit="vCPUs" limit_what="logical CPUs"/>

                <crate::components::ResourceSlider label="Memory" id="vm-memory" value=memory
                    steps=Signal::derive(move || crate::components::memory_steps(host_options.get().map(|o| o.host.max_vm_memory_mb).unwrap_or(8192)))
                    limit=Signal::derive(move || host_options.get().map(|o| o.host.max_vm_memory_mb).unwrap_or(u64::MAX))
                    hard_max=Signal::derive(move || host_options.get().map(|o| o.host.memory_mb * 2).unwrap_or(1 << 20).max(512))
                    fmt=crate::components::human_mb unit="MB" limit_what="available RAM (after the host's reserve)"/>

                <crate::components::ResourceSlider label="Disk" id="vm-disk" value=disk_size
                    steps=Signal::derive(move || crate::components::disk_steps(disk_free()))
                    limit=Signal::derive(move || if host_options.get().is_some() { disk_free() } else { u64::MAX })
                    hard_max=Signal::derive(move || 1u64 << 20)
                    fmt=crate::components::human_gb unit="GB" limit_what="free in the selected storage"/>

                <div class="form-group">
                    {move || if is_instance() {
                        view! { <label for="vm-image">"Image (optional)"</label> }
                    } else {
                        view! { <label for="vm-image">"Boot disk from image (optional)"</label> }
                    }}
                    <input
                        id="vm-image"
                        type="text"
                        placeholder=move || if is_instance() {
                            "debian/12"
                        } else {
                            "https://example.org/disk.qcow2"
                        }
                        on:input=move |ev| set_image.set(event_target_value(&ev))
                        prop:value=image
                    />
                    {move || if is_instance() {
                        view! { <small>"The LXD/Incus image the VM is created from: debian/12, images:alpine/3.20 or a local image alias. Empty means Ubuntu 22.04."</small> }
                    } else {
                        view! { <small>"A URL, or a file in the images directory: a cloud image or disk image (qcow2, raw, vmdk, vdi, vhdx) becomes the VM's disk, grown to the disk size above."</small> }
                    }}
                </div>

                <Show when=move || !is_instance()>
                    <fieldset class="vm-hardware" id="vm-create-disks">
                        <legend>"Disks"</legend>
                        <div class="form-row">
                            <strong>"disk0"</strong>
                            <span id="vm-create-disk0-size">{move || format!("{} GB", disk_size.get())}</span>
                            <super::vm_hardware::DiskOptionFields disk=disk0 prefix="vm-create-disk0"/>
                            <super::vm_hardware::DiskStorageSelect disk=disk0 options=host_options id="vm-create-disk0-storage"/>
                        </div>
                        {move || extra_disks.get().into_iter().enumerate().map(|(i, row)| {
                            let n = i + 1;
                            view! {
                                <div class="form-row vm-create-disk" data-disk=n>
                                    <strong>{format!("disk{n}")}</strong>
                                    <input type="number" min="1" class="vm-create-disk-size" aria-label="Disk size in GB" prop:value={move || row.get().size_gb}
                                        on:change=move |ev| { let s = event_target_value(&ev).parse().unwrap_or(1); row.update(|d| d.size_gb = s) }/>
                                    " GB "
                                    <super::vm_hardware::DiskOptionFields disk=row prefix=format!("vm-create-disk{n}")/>
                                    <super::vm_hardware::DiskStorageSelect disk=row options=host_options id=format!("vm-create-disk{n}-storage")/>
                                    <button type="button" class="btn btn-sm btn-danger"
                                        on:click=move |_| extra_disks.update(|ds| { if i < ds.len() { ds.remove(i); } })>"Remove"</button>
                                </div>
                            }
                        }).collect_view()}
                        <button type="button" id="vm-create-add-disk" class="btn btn-sm btn-secondary"
                            disabled={move || extra_disks.get().len() + 1 >= horcrux_common::MAX_VM_DISKS}
                            on:click=move |_| {
                                let disk = horcrux_common::VmDisk {
                                    size_gb: 8,
                                    disk_type: "virtio".into(),
                                    ..Default::default()
                                };
                                let row = match owner {
                                    Some(o) => with_owner(o, || create_rw_signal(disk)),
                                    None => create_rw_signal(disk),
                                };
                                extra_disks.update(|ds| ds.push(row));
                            }>"Add disk"</button>
                    </fieldset>
                </Show>

                <super::vm_display::DisplaySettingsFields display=display/>

                <Show when=move || !is_instance()>
                    <super::vm_hardware::HardwareFields hardware=hardware options=host_options
                        reload_options=Callback::new(move |_: ()| load_options(false))/>
                </Show>

                <div class="form-group">
                    <label class="checkbox" for="vm-start-after">
                        <input type="checkbox" id="vm-start-after" prop:checked=start_after
                            on:change=move |ev| set_start_after.set(event_target_checked(&ev))/>
                        " Start after created"
                    </label>
                </div>

                {move || error.get().map(|err| view! {
                    <p class="error" role="alert">{err}</p>
                })}
                {move || created_id.get().map(|id| view! {
                    <p class="form-help" id="vm-create-created">
                        {format!("VM {id} exists; Create retries only the step that failed. ")}
                        <A href=format!("/vms/{id}")>"Open the VM"</A>
                    </p>
                })}

                // Proxmox ends its wizard on a Confirm tab: the same summary, right above Create.
                <fieldset class="vm-hardware" id="vm-create-summary">
                    <legend>"Summary"</legend>
                    <table class="vm-table">
                        <tr><th>"VM"</th><td id="vm-sum-id">{move || format!("{} ({})", if name.get().trim().is_empty() { "unnamed".to_string() } else { name.get().trim().to_string() }, vm_id.get().trim())}</td></tr>
                        <tr><th>"Hypervisor"</th><td>{move || format!("{:?}, {:?}", hypervisor.get(), architecture.get()).to_lowercase()}</td></tr>
                        <tr><th>"CPU / memory"</th><td>{move || format!("{} vCPUs, {} MB", cpus.get(), memory.get())}</td></tr>
                        <tr><th>"Disks"</th><td>{move || if is_instance() { "from the instance image".to_string() } else { format!("{} GB{}", disk_size.get(), match extra_disks.get().len() { 0 => String::new(), n => format!(" + {n} more") }) }}</td></tr>
                        <tr><th>"Network"</th><td id="vm-sum-net">{move || { let n = hardware.get().nets; if n.is_empty() { "no network card".to_string() } else { n.iter().map(|c| c.bridge.clone()).collect::<Vec<_>>().join(", ") } }}</td></tr>
                        <tr><th>"Install from"</th><td>{move || hardware.get().cdrom.filter(|c| !c.is_empty()).or_else(|| Some(image.get()).filter(|i| !i.trim().is_empty())).unwrap_or_else(|| "nothing (empty disk)".into())}</td></tr>
                        <tr><th>"Display"</th><td>{move || format!("{:?}", display.get().protocol).to_lowercase()}</td></tr>
                        <tr><th>"After create"</th><td id="vm-sum-after">{move || if start_after.get() { "start it, then open its page" } else { "open its page (stopped)" }}</td></tr>
                    </table>
                </fieldset>

                <div class="form-actions">
                    <button
                        type="submit"
                        class="btn btn-primary"
                        disabled=move || creating.get()
                    >
                        {move || if creating.get() { "Creating..." } else { "Create VM" }}
                    </button>
                    <A href="/vms" class="btn btn-secondary">"Cancel"</A>
                </div>
            </form>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_next_free_id_skips_taken_ones() {
        assert_eq!(next_free_vm_id(&[]), "vm-100");
        let taken: Vec<String> = ["vm-100", "vm-101", "vm-103", "kali"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(next_free_vm_id(&taken), "vm-102");
    }

    #[test]
    fn ids_and_names_follow_the_server_rules() {
        let taken = vec!["vm-100".to_string()];
        assert!(vm_id_problem("vm-101", &taken).is_none());
        assert!(vm_id_problem("vm-100", &taken)
            .unwrap()
            .contains("already exists"));
        for bad in ["", "-x", "a b", "a/b", "a.b"] {
            assert!(vm_id_problem(bad, &taken).is_some(), "{bad}");
        }
        assert!(vm_id_problem(&"a".repeat(65), &taken).is_some());
        assert!(vm_name_problem("web_1-a").is_none());
        for bad in ["", "a b", "a/b", "../x"] {
            assert!(vm_name_problem(bad).is_some(), "{bad}");
        }
    }
}
