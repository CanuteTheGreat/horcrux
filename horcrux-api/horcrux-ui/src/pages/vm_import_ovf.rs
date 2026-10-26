//! VMs > Import OVF / OVA: preview what an OVF/OVA (VMware, VirtualBox,
//! Proxmox export, ...) becomes, then import it (disks are converted to
//! qcow2 on the node). Sources: a file in the image directory or a storage
//! pool directory (Storage Content > Disk images accepts .ova/.ovf) or an
//! http(s) URL.
use crate::api::{self, ovf};
use leptos::*;
use leptos_router::A;

fn gib(b: Option<u64>) -> String {
    b.map(|b| format!("{:.1} GiB", b as f64 / (1u64 << 30) as f64))
        .unwrap_or_else(|| "size from the image".into())
}

#[component]
pub fn VmImportOvfPage() -> impl IntoView {
    let source = create_rw_signal(String::new());
    let id = create_rw_signal(String::new());
    let name = create_rw_signal(String::new());
    let bridge = create_rw_signal(String::new());
    let storage = create_rw_signal(String::new());
    let (options, set_options) = create_signal(None::<api::VmHostOptions>);
    let (plan, set_plan) = create_signal(None::<ovf::OvfPreview>);
    let (done, set_done) = create_signal(None::<ovf::OvfImported>);
    let (error, set_error) = create_signal(None::<String>);
    let (busy, set_busy) = create_signal(false);
    spawn_local(async move {
        if let Ok(o) = api::get_vm_host_options().await {
            set_options.set(Some(o));
        }
    });

    let do_preview = move |_| {
        let src = source.get_untracked();
        set_busy.set(true);
        set_error.set(None);
        set_done.set(None);
        spawn_local(async move {
            match ovf::preview(src.trim()).await {
                Ok(p) => {
                    if name.get_untracked().is_empty() {
                        name.set(p.plan.name.clone());
                    }
                    set_plan.set(Some(p));
                }
                Err(e) => {
                    set_plan.set(None);
                    set_error.set(Some(e.message));
                }
            }
            set_busy.set(false);
        });
    };
    let do_import = move |_| {
        let (src, i, n, b, s) = (
            source.get_untracked(),
            id.get_untracked(),
            name.get_untracked(),
            bridge.get_untracked(),
            storage.get_untracked(),
        );
        set_busy.set(true);
        set_error.set(None);
        spawn_local(async move {
            match ovf::import(src.trim(), &i, &n, &b, &s).await {
                Ok(r) => set_done.set(Some(r)),
                Err(e) => set_error.set(Some(e.message)),
            }
            set_busy.set(false);
        });
    };

    view! {
        <div class="page vm-import-ovf">
            <header class="page-header"><h2>"Import OVF / OVA"</h2></header>
            <p class="form-help">
                "Imports a virtual machine exported as OVF/OVA (VMware, VirtualBox, Proxmox, XenServer ...): CPU, memory, disks (converted to qcow2), network cards and firmware are mapped; anything that cannot be mapped is listed."
            </p>
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="ovf-error">{e}</div> })}
            <div class="form-row">
                <label for="ovf-source">"Source"</label>
                <input id="ovf-source" type="text" style="min-width:32em"
                    placeholder="/var/lib/horcrux/images/vm.ova or https://host/vm.ova"
                    prop:value=source on:input=move |ev| source.set(event_target_value(&ev))/>
                <button class="btn" id="ovf-preview" disabled=busy on:click=do_preview>"Preview"</button>
            </div>
            <small class="form-help">
                {move || format!("A file under {} or a storage pool directory (upload with Storage Content > Disk images), or an http(s) URL.",
                    options.get().map(|o| o.iso_dir.replace("/iso", "/images")).unwrap_or_else(|| "the images directory".into()))}
            </small>
            {move || plan.get().map(|p| {
                let pl = p.plan.clone();
                view! {
                    <div id="ovf-plan" class="info-box">
                        <h3>{format!("{} -> VM {}", pl.name, p.id)}</h3>
                        <p>{format!("{} CPU(s), {} MB memory, firmware {}{}", pl.cpus, pl.memory_mb, pl.firmware,
                            pl.os.clone().map(|o| format!(", guest: {o}")).unwrap_or_default())}</p>
                        <table class="data-table" id="ovf-disks">
                            <thead><tr><th>"Disk file"</th><th>"Size"</th><th>"Bus"</th></tr></thead>
                            <tbody>
                                {pl.disks.iter().map(|d| view! { <tr class="ovf-disk"><td>{d.file.clone()}</td><td>{gib(d.capacity_bytes)}</td><td>{d.bus.clone()}</td></tr> }).collect_view()}
                            </tbody>
                        </table>
                        <ul id="ovf-nics">
                            {pl.nics.iter().map(|n| view! { <li>{format!("network card {} (OVF network \"{}\")", n.model, n.network)}</li> }).collect_view()}
                        </ul>
                        {(!pl.warnings.is_empty()).then(|| view! {
                            <div id="ovf-warnings"><strong>"Not mapped / notes"</strong>
                                <ul>{pl.warnings.iter().map(|w| view! { <li>{w.clone()}</li> }).collect_view()}</ul>
                            </div>
                        })}
                        <div class="form-row">
                            <label for="ovf-id">"VM id"</label>
                            <input id="ovf-id" type="text" placeholder=p.id.clone() prop:value=id on:input=move |ev| id.set(event_target_value(&ev))/>
                            <label for="ovf-name">"Name"</label>
                            <input id="ovf-name" type="text" prop:value=name on:input=move |ev| name.set(event_target_value(&ev))/>
                        </div>
                        <div class="form-row">
                            <label for="ovf-bridge">"Bridge"</label>
                            <select id="ovf-bridge" prop:value=bridge on:change=move |ev| bridge.set(event_target_value(&ev))>
                                <option value="">"first bridge"</option>
                                {move || options.get().map(|o| o.bridges.into_iter().map(|b| view! { <option value=b.name.clone()>{b.name.clone()}</option> }).collect_view())}
                            </select>
                            <label for="ovf-storage">"Storage"</label>
                            <select id="ovf-storage" prop:value=storage on:change=move |ev| storage.set(event_target_value(&ev))>
                                <option value="">"VM storage directory"</option>
                                {move || options.get().map(|o| o.storage_pools.into_iter().map(|p| view! { <option value=p.id.clone()>{format!("{} ({})", p.name, p.storage_type)}</option> }).collect_view())}
                            </select>
                        </div>
                        <button class="btn btn-primary" id="ovf-import" disabled=busy on:click=do_import>"Import"</button>
                        {move || busy.get().then(|| view! { <span id="ovf-busy">" Converting disks, this can take a while..."</span> })}
                    </div>
                }
            })}
            {move || done.get().map(|d| view! {
                <div class="alert alert-success" id="ovf-done">
                    {format!("Imported as VM {} ({} disk(s)). ", d.vm.id, d.vm.disks.len())}
                    <A href=format!("/vms/{}/hardware", d.vm.id)>"Open its hardware"</A>
                    " - start it from the VM list."
                </div>
            })}
        </div>
    }
}
