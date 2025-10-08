use leptos::*;
use leptos_router::*;
use horcrux_common::{VmConfig, VmHypervisor, VmStatus, VmArchitecture};
use crate::api;

#[component]
pub fn VmCreate() -> impl IntoView {
    let (name, set_name) = create_signal(String::new());
    let (cpus, set_cpus) = create_signal(2u32);
    let (memory, set_memory) = create_signal(2048u64);
    let (disk_size, set_disk_size) = create_signal(20u64);
    let (hypervisor, set_hypervisor) = create_signal(VmHypervisor::Qemu);
    let (architecture, set_architecture) = create_signal(VmArchitecture::X86_64);
    let (creating, set_creating) = create_signal(false);
    let (error, set_error) = create_signal(None::<String>);

    let navigate = use_navigate();

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
        };

        let navigate = navigate.clone();
        spawn_local(async move {
            match api::create_vm(config).await {
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
                    <select on:change=move |ev| {
                        let value = event_target_value(&ev);
                        let hv = match value.as_str() {
                            "lxd" => VmHypervisor::Lxd,
                            "incus" => VmHypervisor::Incus,
                            _ => VmHypervisor::Qemu,
                        };
                        set_hypervisor.set(hv);
                    }>
                        <option value="qemu">"QEMU/KVM"</option>
                        <option value="lxd">"LXD"</option>
                        <option value="incus">"Incus"</option>
                    </select>
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
