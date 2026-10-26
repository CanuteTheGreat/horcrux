//! VMs > Import from ESXi: list the VMs of a standalone ESXi host, preview
//! the mapping, copy + convert one (it must be powered off and have no
//! snapshots). Credentials are sent with each request only.
use crate::api::{self, ovf};
use leptos::*;
use leptos_router::A;

#[component]
pub fn VmImportEsxiPage() -> impl IntoView {
    let host = create_rw_signal(String::new());
    let user = create_rw_signal("root".to_string());
    let pass = create_rw_signal(String::new());
    let verify = create_rw_signal(false);
    let id = create_rw_signal(String::new());
    let name = create_rw_signal(String::new());
    let bridge = create_rw_signal(String::new());
    let storage = create_rw_signal(String::new());
    let (stores, set_stores) = create_signal(Vec::<ovf::EsxiDatastore>::new());
    let (picked, set_picked) = create_signal(None::<(String, String)>);
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
    let conn = move || {
        (
            host.get_untracked(),
            user.get_untracked(),
            pass.get_untracked(),
            verify.get_untracked(),
        )
    };

    let do_list = move |_| {
        let (h, u, p, v) = conn();
        set_busy.set(true);
        set_error.set(None);
        set_plan.set(None);
        set_done.set(None);
        spawn_local(async move {
            match ovf::esxi_list(&h, &u, &p, v).await {
                Ok(l) => set_stores.set(l),
                Err(e) => {
                    set_stores.set(Vec::new());
                    set_error.set(Some(e.message));
                }
            }
            set_busy.set(false);
        });
    };
    let do_preview = move |ds: String, folder: String| {
        let (h, u, p, v) = conn();
        set_busy.set(true);
        set_error.set(None);
        set_done.set(None);
        spawn_local(async move {
            match ovf::esxi_preview((&h, &u, &p, v), &ds, &folder).await {
                Ok(pr) => {
                    name.set(pr.plan.name.clone());
                    set_picked.set(Some((ds, folder)));
                    set_plan.set(Some(pr));
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
        let Some((ds, folder)) = picked.get_untracked() else {
            return;
        };
        let (h, u, p, v) = conn();
        let (i, n, b, s) = (
            id.get_untracked(),
            name.get_untracked(),
            bridge.get_untracked(),
            storage.get_untracked(),
        );
        set_busy.set(true);
        set_error.set(None);
        spawn_local(async move {
            match ovf::esxi_import((&h, &u, &p, v), &ds, &folder, &i, &n, &b, &s).await {
                Ok(r) => set_done.set(Some(r)),
                Err(e) => set_error.set(Some(e.message)),
            }
            set_busy.set(false);
        });
    };

    view! {
        <div class="page vm-import-esxi">
            <header class="page-header"><h2>"Import from ESXi"</h2></header>
            <p class="form-help">
                "Copies a powered-off VM without snapshots from a standalone ESXi host over HTTPS (datastore browser) and converts its disks to qcow2. For vCenter VMs export an OVA and use "
                <A href="/vms/import-ovf">"Import OVF / OVA"</A>
                ". The credentials are used for these requests only and are not stored."
            </p>
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="esxi-error">{e}</div> })}
            <div class="form-row">
                <label for="esxi-host">"ESXi host"</label>
                <input id="esxi-host" type="text" placeholder="esxi1.lab" prop:value=host on:input=move |ev| host.set(event_target_value(&ev))/>
                <label for="esxi-user">"User"</label>
                <input id="esxi-user" type="text" prop:value=user on:input=move |ev| user.set(event_target_value(&ev))/>
                <label for="esxi-pass">"Password"</label>
                <input id="esxi-pass" type="password" autocomplete="off" prop:value=pass on:input=move |ev| pass.set(event_target_value(&ev))/>
                <label><input id="esxi-verify" type="checkbox" prop:checked=verify on:change=move |ev| verify.set(event_target_checked(&ev))/>" Verify certificate"</label>
                <button class="btn btn-primary" id="esxi-list" disabled=busy on:click=do_list>"List VMs"</button>
            </div>
            <table class="data-table" id="esxi-vms">
                <thead><tr><th>"Datastore"</th><th>"VM folder"</th><th>"Descriptor"</th><th></th></tr></thead>
                <tbody>
                    {move || stores.get().into_iter().flat_map(|d| {
                        let ds = d.name.clone();
                        d.vms.into_iter().map(move |v| {
                            let (a, b) = (ds.clone(), v.folder.clone());
                            view! {
                                <tr class="esxi-vm" data-folder=v.folder.clone()>
                                    <td>{ds.clone()}</td><td>{v.folder.clone()}</td><td>{v.vmx.clone()}</td>
                                    <td><button class="btn btn-sm esxi-preview" disabled=busy on:click=move |_| do_preview(a.clone(), b.clone())>"Preview"</button></td>
                                </tr>
                            }
                        }).collect::<Vec<_>>()
                    }).collect_view()}
                </tbody>
            </table>
            {move || plan.get().map(|p| {
                let pl = p.plan.clone();
                view! {
                    <div id="esxi-plan" class="info-box">
                        <h3>{format!("{} -> VM {}", pl.name, p.id)}</h3>
                        <p>{format!("{} CPU(s), {} MB memory, firmware {}{}", pl.cpus, pl.memory_mb, pl.firmware,
                            pl.os.clone().map(|o| format!(", guest: {o}")).unwrap_or_default())}</p>
                        <ul>
                            {pl.disks.iter().map(|d| view! { <li class="esxi-disk">{format!("{} on {} ({})", d.label, d.bus,
                                d.capacity_bytes.map(|b| format!("{:.1} GiB", b as f64 / (1u64 << 30) as f64)).unwrap_or_else(|| "size unknown".into()))}</li> }).collect_view()}
                            {pl.nics.iter().map(|n| view! { <li>{format!("network card {} ({})", n.model, n.network)}</li> }).collect_view()}
                        </ul>
                        {(!pl.warnings.is_empty()).then(|| view! {
                            <div id="esxi-warnings"><strong>"Not mapped / notes"</strong>
                                <ul>{pl.warnings.iter().map(|w| view! { <li>{w.clone()}</li> }).collect_view()}</ul></div>
                        })}
                        <div class="form-row">
                            <label for="esxi-id">"VM id"</label>
                            <input id="esxi-id" type="text" placeholder=p.id.clone() prop:value=id on:input=move |ev| id.set(event_target_value(&ev))/>
                            <label for="esxi-name">"Name"</label>
                            <input id="esxi-name" type="text" prop:value=name on:input=move |ev| name.set(event_target_value(&ev))/>
                            <label for="esxi-bridge">"Bridge"</label>
                            <select id="esxi-bridge" prop:value=bridge on:change=move |ev| bridge.set(event_target_value(&ev))>
                                <option value="">"first bridge"</option>
                                {move || options.get().map(|o| o.bridges.into_iter().map(|b| view! { <option value=b.name.clone()>{b.name.clone()}</option> }).collect_view())}
                            </select>
                            <label for="esxi-storage">"Storage"</label>
                            <select id="esxi-storage" prop:value=storage on:change=move |ev| storage.set(event_target_value(&ev))>
                                <option value="">"VM storage directory"</option>
                                {move || options.get().map(|o| o.storage_pools.into_iter().map(|p| view! { <option value=p.id.clone()>{format!("{} ({})", p.name, p.storage_type)}</option> }).collect_view())}
                            </select>
                        </div>
                        <button class="btn btn-primary" id="esxi-import" disabled=busy on:click=do_import>"Copy and import"</button>
                        {move || busy.get().then(|| view! { <span id="esxi-busy">" Working (copying disks over the network can take a while)..."</span> })}
                    </div>
                }
            })}
            {move || done.get().map(|d| view! {
                <div class="alert alert-success" id="esxi-done">
                    {format!("Imported as VM {} ({} disk(s)). ", d.vm.id, d.vm.disks.len())}
                    <A href=format!("/vms/{}/hardware", d.vm.id)>"Open its hardware"</A>
                </div>
            })}
        </div>
    }
}
