//! `/vms/:id/firewall`: one VM's own firewall, as its own tab like Proxmox's
//! guest Firewall view (it used to sit at the bottom of Hardware, and the VM
//! page's "Firewall" tab opened the node-wide firewall objects instead).
use crate::api;
use horcrux_common::{VmConfig, VmHypervisor};
use leptos::*;
use leptos_router::*;

#[component]
pub fn VmFirewallPage() -> impl IntoView {
    let params = use_params_map();
    let vm_id = move || params.with(|p| p.get("id").cloned().unwrap_or_default());
    let (vm, set_vm) = create_signal(None::<VmConfig>);
    let (missing, set_missing) = create_signal(false);
    let (error, set_error) = create_signal(None::<String>);
    create_effect(move |_| {
        let id = vm_id();
        spawn_local(async move {
            match api::get_vm(&id).await {
                Ok(v) => {
                    set_missing.set(false);
                    set_vm.set(Some(v))
                }
                Err(e) if api::status_of(&e.message) == Some(404) => set_missing.set(true),
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    });
    let is_qemu = move || vm.get().map(|v| v.hypervisor == VmHypervisor::Qemu);
    view! {
        <div class="vm-hardware-page" id="vm-firewall-page">
            <div class="page-header">
                <h1>"VM firewall: " {move || vm.get().map(|v| format!("{} ({})", v.name, v.id)).unwrap_or_else(vm_id)}</h1>
                <A href="/vms" class="btn btn-secondary">"Back to VMs"</A>
            </div>
            <crate::components::vm_tabs::VmTabs vm_id=Signal::derive(vm_id) active="Firewall"/>
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="vm-fw-error">{e}</div> })}
            {move || {
                if missing.get() {
                    view! {
                        <div class="empty-state" id="vm-fw-missing">
                            <p>{format!("There is no VM with the ID {} on this node. It may have been deleted or moved to another node.", vm_id())}</p>
                            <A href="/vms" class="btn btn-primary">"Back to the VM list"</A>
                        </div>
                    }
                    .into_view()
                } else {
                    match is_qemu() {
                        Some(true) => view! {
                            <p class="help-text">"Node-wide rules, IP sets and aliases are under "
                                <A href="/network/firewall-objects">"Network > Firewall Objects"</A>"."</p>
                            <crate::pages::vm_hardware::GuestFirewall guest_id=Signal::derive(vm_id) container=false/>
                        }
                        .into_view(),
                        Some(false) => view! {
                            <p class="form-help" id="vm-fw-lxd">"LXD/Incus VMs get their network filtering from their LXD/Incus profile; per-VM rules here are for QEMU VMs. Node-wide rules are under "
                                <A href="/network/firewall-objects">"Network > Firewall Objects"</A>"."</p>
                        }
                        .into_view(),
                        None if error.get().is_some() => ().into_view(),
                        None => view! { <p class="help-text">"Loading the VM..."</p> }.into_view(),
                    }
                }
            }}
        </div>
    }
}
