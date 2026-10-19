//! `/containers/:id/firewall`: one container's own firewall (any runtime).
//! Rules match the container's host-side interfaces only (stable names
//! `vc<hash>i<n>` that Horcrux gives every container NIC).
use leptos::*;
use leptos_router::*;

#[component]
pub fn ContainerFirewallPage() -> impl IntoView {
    let params = use_params_map();
    let ct_id = move || params.with(|p| p.get("id").cloned().unwrap_or_default());
    let (name, set_name) = create_signal(None::<String>);
    create_effect(move |_| {
        let id = ct_id();
        spawn_local(async move {
            if let Ok(c) = crate::api::get_container_config(&id).await {
                set_name.set(Some(format!("{} ({}, {:?})", c.name, c.id, c.runtime)));
            }
        });
    });
    view! {
        <div class="vm-hardware-page" id="container-firewall-page">
            <div class="page-header">
                <h1>"Container firewall: " {move || name.get().unwrap_or_else(ct_id)}</h1>
                <A href="/containers" class="btn btn-secondary">"Back to containers"</A>
            </div>
            <super::vm_hardware::GuestFirewall guest_id=Signal::derive(ct_id) container=true/>
        </div>
    }
}
