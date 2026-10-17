use crate::api;
use horcrux_common::{VmArchitecture, VmConfig, VmHypervisor, VmStatus};
use leptos::*;
use leptos_router::*;

/// (value, label) of every VM hypervisor, in menu order.
const HYPERVISORS: [(&str, &str); 3] = [("qemu", "QEMU/KVM"), ("lxd", "LXD"), ("incus", "Incus")];

fn hypervisor_from(value: &str) -> VmHypervisor {
    match value {
        "lxd" => VmHypervisor::Lxd,
        "incus" => VmHypervisor::Incus,
        _ => VmHypervisor::Qemu,
    }
}

#[component]
pub fn VmCreate() -> impl IntoView {
    let (name, set_name) = create_signal(String::new());
    let (cpus, set_cpus) = create_signal(2u32);
    let (memory, set_memory) = create_signal(2048u64);
    let (disk_size, set_disk_size) = create_signal(20u64);
    let (hypervisor, set_hypervisor) = create_signal(VmHypervisor::Qemu);
    let (architecture, set_architecture) = create_signal(VmArchitecture::X86_64);
    let display = create_rw_signal(horcrux_common::VmDisplay::default());
    let (image, set_image) = create_signal(String::new());
    let (creating, set_creating) = create_signal(false);
    let (error, set_error) = create_signal(None::<String>);

    let navigate = use_navigate();

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

        set_creating.set(true);
        set_error.set(None);

        let config = VmConfig {
            id: format!("vm-{}", chrono::Utc::now().timestamp()),
            name: name.get(),
            hypervisor: hypervisor.get(),
            memory: memory.get(),
            cpus: cpus.get(),
            disk_size: disk_size.get(),
            status: VmStatus::Stopped,
            architecture: architecture.get(),
            disks: Vec::new(),
            display: display.get(),
        };

        let navigate = navigate.clone();
        let image = image.get().trim().to_string();
        spawn_local(async move {
            let created =
                match api::create_vm(config).await {
                    Ok(vm) if !image.is_empty() => api::import_vm_disk(&vm.id, &image)
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
                Ok(_) => {
                    navigate("/vms", Default::default());
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
                    <label>"VM Name"</label>
                    <input
                        type="text"
                        required
                        placeholder="my-vm"
                        on:input=move |ev| set_name.set(event_target_value(&ev))
                        prop:value=name
                    />
                </div>

                <div class="form-group">
                    <label>"Hypervisor"</label>
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
                    <label>"Architecture"</label>
                    <select on:change=move |ev| {
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

                <div class="form-group">
                    <label>"CPUs"</label>
                    <input
                        type="number"
                        min="1"
                        max="64"
                        required
                        on:input=move |ev| set_cpus.set(event_target_value(&ev).parse().unwrap_or(2))
                        prop:value=cpus
                    />
                </div>

                <div class="form-group">
                    <label>"Memory (MB)"</label>
                    <input
                        type="number"
                        min="512"
                        step="512"
                        required
                        on:input=move |ev| set_memory.set(event_target_value(&ev).parse().unwrap_or(2048))
                        prop:value=memory
                    />
                </div>

                <div class="form-group">
                    <label>"Disk Size (GB)"</label>
                    <input
                        type="number"
                        min="10"
                        required
                        on:input=move |ev| set_disk_size.set(event_target_value(&ev).parse().unwrap_or(20))
                        prop:value=disk_size
                    />
                </div>

                <div class="form-group">
                    <label>"Boot disk from image (optional)"</label>
                    <input
                        type="text"
                        placeholder="https://.../cloud-image.qcow2 or a file in the images directory"
                        on:input=move |ev| set_image.set(event_target_value(&ev))
                        prop:value=image
                    />
                    <small>"A cloud image or disk image (qcow2, raw, vmdk, vdi, vhdx) becomes the VM's disk, grown to the disk size above."</small>
                </div>

                <super::vm_display::DisplaySettingsFields display=display/>

                {move || error.get().map(|err| view! {
                    <p class="error">"Error: " {err}</p>
                })}

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
