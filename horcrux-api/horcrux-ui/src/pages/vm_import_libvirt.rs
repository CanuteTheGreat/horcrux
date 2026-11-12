//! VMs > Import from libvirt: VMs defined directly in libvirt/QEMU on this
//! host are not Horcrux VMs and would stay invisible. This page lists every
//! libvirt domain, previews what it becomes and imports it (disks stay in
//! place by default; the domain is undefined in libvirt afterwards).
use crate::api::{self, libvirt};
use leptos::*;
use leptos_router::A;

/// "Unmanaged, import?" banner for the VM list: nothing when libvirt is
/// absent or every domain is imported.
#[component]
pub fn UnmanagedLibvirtBanner() -> impl IntoView {
    let (list, set_list) = create_signal(Vec::<(String, bool, libvirt::LvListed)>::new());
    let (pending, set_pending) = create_signal(Vec::<libvirt::CleanupOffer>::new());
    spawn_local(async move {
        if let Ok(p) = libvirt::cleanup_pending().await {
            set_pending.set(p);
        }
        if let Ok(c) = libvirt::list_cluster().await {
            set_list.set(
                c.nodes
                    .into_iter()
                    .flat_map(|n| {
                        let (name, local) = (n.node, n.local);
                        n.domains
                            .into_iter()
                            .filter(|d| d.imported_as.is_none())
                            .map(move |d| (name.clone(), local, d))
                    })
                    .collect(),
            );
        }
    });
    view! {
        {move || {
            let p = pending.get();
            (!p.is_empty()).then(|| view! {
                <div class="alert alert-info" id="import-cleanup-pending">
                    {format!("Old sources of imported VMs can be cleaned up ({}): ", p.iter().map(|o| format!("{} {}", o.vm_id, libvirt::human_bytes(o.total_bytes))).collect::<Vec<_>>().join(", "))}
                    <A href="/vms/import-libvirt" class="btn">"Review"</A>
                </div>
            })
        }}
        {move || {
            let l = list.get();
            (!l.is_empty()).then(|| view! {
                <div class="alert alert-warning" id="unmanaged-libvirt">
                    <strong>{format!("{} unmanaged libvirt VM(s) in this cluster, not shown above: ", l.len())}</strong>
                    {l.iter().map(|(node, local, d)| view! { <span class="unmanaged-vm">{if *local { format!("{} ({}) ", d.domain, d.state) } else { format!("{} ({}) on {} ", d.domain, d.state, node) }}</span> }).collect_view()}
                    <A href="/vms/import-libvirt" class="btn btn-primary" attr:id="unmanaged-import">"Import into Horcrux"</A>
                </div>
            })
        }}
    }
}

/// The opt-in cleanup of an import's old source: lists exactly what goes, with
/// sizes; nothing is removed before the button is clicked.
#[component]
pub fn ImportCleanup(vm_id: String) -> impl IntoView {
    let (offer, set_offer) = create_signal(None::<libvirt::CleanupOffer>);
    let (done, set_done) = create_signal(None::<libvirt::CleanupDone>);
    let (error, set_error) = create_signal(None::<String>);
    let (busy, set_busy) = create_signal(false);
    let id = vm_id.clone();
    let show = move |_| {
        let id = id.clone();
        spawn_local(async move {
            match libvirt::cleanup_preview(&id).await {
                Ok(o) => set_offer.set(Some(o)),
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };
    let id = vm_id.clone();
    let run = move |_| {
        let id = id.clone();
        set_busy.set(true);
        spawn_local(async move {
            match libvirt::cleanup(&id).await {
                Ok(d) => {
                    set_done.set(Some(d));
                    set_offer.set(None);
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            set_busy.set(false);
        });
    };
    view! {
        <div class="import-cleanup" id="import-cleanup">
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="cleanup-error">{e}</div> })}
            {move || (offer.get().is_none() && done.get().is_none()).then(|| view! {
                <button class="btn" id="cleanup-show" on:click=show.clone()>"Clean up the old source..."</button>
            })}
            {move || offer.get().map(|o| view! {
                <div id="cleanup-offer" class="info-box">
                    <strong>{format!("Removing the old source of VM {} (the VM and its disks are not touched):", o.vm_id)}</strong>
                    <ul>
                        {o.items.iter().map(|i| view! { <li class=if i.remove { "cleanup-remove" } else { "cleanup-keep" }>
                            {format!("{} {} ({}) - {}", i.kind, i.path, libvirt::human_bytes(i.bytes),
                                if i.remove { "will be removed".to_string() } else { i.reason.clone().unwrap_or_default() })}
                        </li> }).collect_view()}
                    </ul>
                    <p>{format!("Total {} . The libvirt XML is backed up first. Default is to keep everything.", libvirt::human_bytes(o.total_bytes))}</p>
                    <button class="btn btn-danger" id="cleanup-run" disabled=move || busy.get() on:click=run.clone()>"Remove these"</button>
                </div>
            })}
            {move || done.get().map(|d| view! {
                <div class="alert alert-success" id="cleanup-done">
                    {format!("Removed {} item(s), kept {}. {}", d.removed.len(), d.kept.len(),
                        d.xml_backup.clone().map(|x| format!("XML backup: {x}")).unwrap_or_default())}
                </div>
            })}
        </div>
    }
}

#[component]
pub fn VmImportLibvirtPage() -> impl IntoView {
    let id = create_rw_signal(String::new());
    let name = create_rw_signal(String::new());
    let bridge = create_rw_signal(String::new());
    let storage = create_rw_signal(String::new());
    let (progress, set_progress) = create_signal(None::<(f32, String)>);
    let (options, set_options) = create_signal(None::<api::VmHostOptions>);
    let (list, set_list) = create_signal(None::<libvirt::LvList>);
    let (plan, set_plan) = create_signal(None::<libvirt::LvPreview>);
    let (done, set_done) = create_signal(None::<libvirt::LvImported>);
    let (error, set_error) = create_signal(None::<String>);
    let (busy, set_busy) = create_signal(false);
    let reload = move || {
        spawn_local(async move {
            match libvirt::list().await {
                Ok(l) => set_list.set(Some(l)),
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };
    reload();
    spawn_local(async move {
        if let Ok(o) = api::get_vm_host_options().await {
            set_options.set(Some(o));
        }
    });

    let do_preview = move |domain: String| {
        set_busy.set(true);
        set_error.set(None);
        set_done.set(None);
        spawn_local(async move {
            match libvirt::preview(&domain).await {
                Ok(p) => {
                    name.set(p.plan.name.clone());
                    id.set(p.id.clone());
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
        let Some(p) = plan.get_untracked() else {
            return;
        };
        let (i, n, b, s) = (
            id.get_untracked(),
            name.get_untracked(),
            bridge.get_untracked(),
            storage.get_untracked(),
        );
        set_busy.set(true);
        set_error.set(None);
        set_progress.set(Some((0.0, "starting".into())));
        spawn_local(async move {
            let job = match libvirt::import(&p.plan.domain, &i, &n, &b, &s).await {
                Ok(j) => j,
                Err(e) => {
                    set_error.set(Some(e.message));
                    set_progress.set(None);
                    set_busy.set(false);
                    return;
                }
            };
            loop {
                gloo_timers::future::TimeoutFuture::new(2000).await;
                match libvirt::import_status(&job).await {
                    Ok(j) if j.state == "running" => set_progress.set(Some((j.percent, j.message))),
                    Ok(j) if j.state == "done" => {
                        set_done.set(j.result);
                        set_plan.set(None);
                        reload();
                        break;
                    }
                    Ok(j) => {
                        set_error.set(Some(j.error.unwrap_or_else(|| "import failed".into())));
                        break;
                    }
                    Err(e) => {
                        set_error.set(Some(e.message));
                        break;
                    }
                }
            }
            set_progress.set(None);
            set_busy.set(false);
        });
    };

    view! {
        <div class="page vm-import-libvirt">
            <header class="page-header"><h2>"Import from libvirt"</h2></header>
            <p class="form-help">
                "VMs defined directly in libvirt/QEMU on this host (virt-manager, virsh) are not Horcrux VMs. Importing maps CPU, memory, network cards with their MAC, UEFI/Secure Boot, the TPM and the machine type, and copies the disks, the UEFI variable store and the TPM state into Horcrux storage (verified). Nothing of the old source is deleted: afterwards you are offered its removal. The domain must be shut off."
            </p>
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="lv-error">{e}</div> })}
            {move || match list.get() {
                None => view! { <p class="loading">"Looking for libvirt domains..."</p> }.into_view(),
                Some(l) if !l.available => view! { <p id="lv-none">"libvirt (virsh) is not installed on this host: nothing to import."</p> }.into_view(),
                Some(l) if l.domains.is_empty() => view! { <p id="lv-none">"libvirt defines no domains on this host."</p> }.into_view(),
                Some(l) => view! {
                    <table class="data-table" id="lv-domains">
                        <thead><tr><th>"Domain"</th><th>"State"</th><th>"CPUs"</th><th>"Memory"</th><th>"Horcrux VM"</th><th></th></tr></thead>
                        <tbody>
                            {l.domains.into_iter().map(|d| {
                                let dom = d.domain.clone();
                                let managed = d.imported_as.clone();
                                view! {
                                    <tr class="lv-domain">
                                        <td>{d.domain.clone()}{d.problem.clone().map(|p| format!(" - {p}"))}</td>
                                        <td>{d.state.clone()}</td>
                                        <td>{d.cpus}</td>
                                        <td>{format!("{} MB", d.memory_mb)}</td>
                                        <td>{match &managed { Some(v) => v.clone(), None => "Unmanaged".to_string() }}</td>
                                        <td>{match managed.clone() {
                                            None => view! {
                                                <button class="btn lv-preview" disabled=move || busy.get() on:click=move |_| do_preview(dom.clone())>"Import..."</button>
                                            }.into_view(),
                                            Some(v) => view! { <ImportCleanup vm_id=v/> }.into_view(),
                                        }}</td>
                                    </tr>
                                }
                            }).collect_view()}
                        </tbody>
                    </table>
                }.into_view(),
            }}
            {move || plan.get().map(|p| {
                let pl = p.plan.clone();
                view! {
                    <div id="lv-plan" class="info-box">
                        <h3>{format!("{} -> VM {}", pl.domain, p.id)}</h3>
                        <p>{format!("{} CPU(s), {} MB, machine {}, firmware {}{}{}", pl.cpus, pl.memory_mb, pl.machine, pl.firmware,
                            if pl.secure_boot { " + Secure Boot" } else { "" }, if pl.tpm { ", TPM 2.0" } else { "" })}</p>
                        <ul id="lv-disks">
                            {pl.disks.iter().map(|d| view! { <li>{format!("{} ({}, {}) - copied into Horcrux storage", d.path, d.format, d.bus)}</li> }).collect_view()}
                        </ul>
                        <ul id="lv-nics">
                            {pl.nics.iter().map(|n| view! { <li>{format!("{} MAC {} ({})", n.model, n.mac.clone().unwrap_or_else(|| "new".into()), n.bridge.clone().unwrap_or(n.network.clone()))}</li> }).collect_view()}
                        </ul>
                        <p id="lv-state">{format!("UEFI variables: {}; TPM state: {}",
                            pl.nvram.clone().unwrap_or_else(|| "fresh".into()), pl.tpm_state.clone().unwrap_or_else(|| "none yet".into()))}</p>
                        {(!pl.warnings.is_empty()).then(|| view! {
                            <div id="lv-warnings"><strong>"Not mapped / notes"</strong>
                                <ul>{pl.warnings.iter().map(|w| view! { <li>{w.clone()}</li> }).collect_view()}</ul>
                            </div>
                        })}
                        <div class="form-row">
                            <label for="lv-id">"VM id"</label>
                            <input id="lv-id" type="text" prop:value=id on:input=move |ev| id.set(event_target_value(&ev))/>
                            <label for="lv-name">"Name"</label>
                            <input id="lv-name" type="text" prop:value=name on:input=move |ev| name.set(event_target_value(&ev))/>
                            <label for="lv-bridge">"Bridge"</label>
                            <select id="lv-bridge" prop:value=bridge on:change=move |ev| bridge.set(event_target_value(&ev))>
                                <option value="">"as in libvirt"</option>
                                {move || options.get().map(|o| o.bridges.into_iter().map(|b| view! { <option value=b.name.clone()>{b.name.clone()}</option> }).collect_view())}
                            </select>
                        </div>
                        <div class="form-row">
                            <label for="lv-storage">"Storage"</label>
                            <select id="lv-storage" prop:value=storage on:change=move |ev| storage.set(event_target_value(&ev))>
                                <option value="">"VM storage directory"</option>
                                {move || options.get().map(|o| o.storage_pools.into_iter().map(|p| view! { <option value=p.id.clone()>{format!("{} ({})", p.name, p.storage_type)}</option> }).collect_view())}
                            </select>
                        </div>
                        <p class="form-help">"The disks are copied into Horcrux storage at low priority and verified; the old files stay until you clean them up."</p>
                        {move || progress.get().map(|(p, m)| view! {
                            <div id="lv-progress"><progress max="100" value=p></progress>{format!(" {p:.0}% - {m}")}</div>
                        })}
                        <div><button class="btn btn-primary" id="lv-import" disabled=move || busy.get() on:click=do_import>"Import"</button></div>
                    </div>
                }
            })}
            {move || done.get().map(|d| view! {
                <div class="alert alert-success" id="lv-done">
                    {format!("Imported as VM {} ({} disk(s)). XML backup: {}. libvirt still defines the domain (shut off); nothing was deleted. ", d.vm.id, d.vm.disks.len(), d.xml_backup)}
                    <A href=format!("/vms/{}/hardware", d.vm.id)>"Open its hardware"</A>
                    {d.cleanup_offered.then(|| view! { <ImportCleanup vm_id=d.vm.id.clone()/> })}
                </div>
            })}
        </div>
    }
}
