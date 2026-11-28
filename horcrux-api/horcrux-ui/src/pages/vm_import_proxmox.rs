//! VMs > Import from Proxmox VE: read the guests of a Proxmox source (a
//! directory with the guest configs next to the received volumes, a node
//! over SSH, or a container vzdump archive), show per guest what Horcrux
//! makes of every Proxmox setting and how the QEMU command line compares,
//! then import the chosen guests with progress and links to the results.
use crate::api::{self, proxmox};
use leptos::*;
use leptos_router::A;
use std::collections::{BTreeMap, BTreeSet};

fn carry_label(c: &str) -> &'static str {
    match c {
        "carried" => "carried",
        "changed" => "changed",
        "not-carried" => "not carried",
        _ => "ignored",
    }
}

fn count(g: &proxmox::PveGuest, c: &str) -> usize {
    g.rows.iter().filter(|r| r.carry == c).count()
}

#[component]
pub fn VmImportProxmoxPage() -> impl IntoView {
    let kind = create_rw_signal("dir".to_string());
    let path = create_rw_signal(String::new());
    let host = create_rw_signal(String::new());
    let port = create_rw_signal(String::new());
    let identity = create_rw_signal(String::new());
    let storage_map = create_rw_signal(String::new());
    let bridge_map = create_rw_signal(String::new());
    let io_devices = create_rw_signal(String::new());
    let storage = create_rw_signal(String::new());
    let (options, set_options) = create_signal(None::<api::VmHostOptions>);
    let (plan, set_plan) = create_signal(None::<proxmox::PvePlan>);
    let picked = create_rw_signal(BTreeSet::<u32>::new());
    let ids = create_rw_signal(BTreeMap::<u32, String>::new());
    let open = create_rw_signal(None::<u32>);
    let (job, set_job) = create_signal(None::<proxmox::PveJob>);
    let (error, set_error) = create_signal(None::<String>);
    let (busy, set_busy) = create_signal(false);
    spawn_local(async move {
        if let Ok(o) = api::get_vm_host_options().await {
            set_options.set(Some(o));
        }
    });
    let form = move || proxmox::PveSourceForm {
        kind: kind.get_untracked(),
        path: path.get_untracked(),
        host: host.get_untracked(),
        port: port.get_untracked(),
        identity: identity.get_untracked(),
        storage_map: storage_map.get_untracked(),
        bridge_map: bridge_map.get_untracked(),
        io_devices: io_devices.get_untracked(),
    };

    let do_plan = move |_| {
        let f = form();
        set_busy.set(true);
        set_error.set(None);
        set_job.set(None);
        spawn_local(async move {
            match proxmox::plan(&f).await {
                Ok(p) => {
                    // pre-select every guest that can be imported
                    picked.set(
                        p.guests
                            .iter()
                            .filter(|g| {
                                g.blockers.is_empty() && !g.exists && g.imported_as.is_none()
                            })
                            .map(|g| g.vmid)
                            .collect(),
                    );
                    ids.set(
                        p.guests
                            .iter()
                            .map(|g| (g.vmid, g.target_id.clone()))
                            .collect(),
                    );
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
        let f = form();
        let guests: Vec<u32> = picked.get_untracked().into_iter().collect();
        if guests.is_empty() {
            set_error.set(Some("Pick at least one guest to import.".into()));
            return;
        }
        let all_ids = ids.get_untracked();
        let chosen: BTreeMap<u32, String> = all_ids
            .into_iter()
            .filter(|(k, v)| guests.contains(k) && !v.trim().is_empty())
            .collect();
        let st = storage.get_untracked();
        set_busy.set(true);
        set_error.set(None);
        spawn_local(async move {
            match proxmox::start_import(&f, &guests, &chosen, &st).await {
                Ok(jid) => loop {
                    gloo_timers::future::TimeoutFuture::new(2000).await;
                    match proxmox::status(&jid).await {
                        Ok(j) => {
                            let running = j.state == "running";
                            set_job.set(Some(j));
                            if !running {
                                break;
                            }
                        }
                        Err(e) => {
                            set_error.set(Some(e.message));
                            break;
                        }
                    }
                },
                Err(e) => set_error.set(Some(e.message)),
            }
            set_busy.set(false);
        });
    };

    view! {
        <div class="page vm-import-proxmox">
            <header class="page-header"><h2>"Import from Proxmox VE"</h2></header>
            <p class="form-help">
                "Brings Proxmox VMs and containers over with their hardware, MAC addresses, UEFI variables, TPM state, cloud-init seed, disk limits and start order. Disks and container files are copied into Horcrux storage and checked; the source is never changed (a local source is offered for removal afterwards). Shut the guests down first."
            </p>
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="pve-error">{e}</div> })}
            <fieldset class="form-section" id="pve-source">
                <legend>"Source"</legend>
                <div class="form-row">
                    <label for="pve-kind">"Type"</label>
                    <select id="pve-kind" prop:value=kind on:change=move |ev| kind.set(event_target_value(&ev))>
                        <option value="dir">"Directory on this host (configs + received volumes)"</option>
                        <option value="ssh">"Proxmox node over SSH (read only)"</option>
                        <option value="vzdump">"Container backup (vzdump .tar.zst)"</option>
                    </select>
                </div>
                {move || match kind.get().as_str() {
                    "ssh" => view! {
                        <div class="form-row">
                            <label for="pve-host">"Node"</label>
                            <input id="pve-host" type="text" placeholder="root@pve1" prop:value=host on:input=move |ev| host.set(event_target_value(&ev))/>
                            <label for="pve-port">"SSH port"</label>
                            <input id="pve-port" type="text" placeholder="22" prop:value=port on:input=move |ev| port.set(event_target_value(&ev))/>
                            <label for="pve-identity">"Key file on this host"</label>
                            <input id="pve-identity" type="text" placeholder="/root/.ssh/id_ed25519" prop:value=identity on:input=move |ev| identity.set(event_target_value(&ev))/>
                        </div>
                    }.into_view(),
                    "vzdump" => view! {
                        <div class="form-row">
                            <label for="pve-path">"Archive"</label>
                            <input id="pve-path" type="text" placeholder="/tank/dump/vzdump-lxc-110-2026_11_27-02_30_00.tar.zst" prop:value=path on:input=move |ev| path.set(event_target_value(&ev))/>
                        </div>
                    }.into_view(),
                    _ => view! {
                        <div class="form-row">
                            <label for="pve-path">"Directory"</label>
                            <input id="pve-path" type="text" placeholder="/tank/migration/pve (holds qemu-server/ and lxc/)" prop:value=path on:input=move |ev| path.set(event_target_value(&ev))/>
                        </div>
                        <div class="form-row">
                            <label for="pve-storage-map">"Where each Proxmox storage is now"</label>
                            <textarea id="pve-storage-map" rows="2" placeholder="local-zfs=zfs:tank/migration/data\nlocal=dir:/tank/migration/vz" prop:value=storage_map on:input=move |ev| storage_map.set(event_target_value(&ev))></textarea>
                        </div>
                    }.into_view(),
                }}
                <div class="form-row">
                    <label for="pve-bridge-map">"Bridge mapping (Proxmox=Horcrux)"</label>
                    <textarea id="pve-bridge-map" rows="2" placeholder="vmbr0=br0" prop:value=bridge_map on:input=move |ev| bridge_map.set(event_target_value(&ev))></textarea>
                    <label for="pve-io-devices">"Container I/O limit devices (source major:minor=device)"</label>
                    <textarea id="pve-io-devices" rows="2" placeholder="259:0=/dev/nvme0n1" prop:value=io_devices on:input=move |ev| io_devices.set(event_target_value(&ev))></textarea>
                </div>
                <button class="btn btn-primary" id="pve-plan" disabled=busy on:click=do_plan>"Read guests"</button>
                {move || options.get().map(|o| view! {
                    <p class="form-help">{format!("Bridges on this host: {}", o.bridges.iter().map(|b| b.name.clone()).collect::<Vec<_>>().join(", "))}</p>
                })}
            </fieldset>

            {move || match plan.get() {
                None => view! {
                    <div class="empty-state" id="pve-empty">
                        <p>"No source read yet: enter where the Proxmox guests are and press Read guests. Nothing is changed until you import."</p>
                    </div>
                }.into_view(),
                Some(p) if p.guests.is_empty() => view! {
                    <div class="empty-state" id="pve-empty"><p>"The source has no guests."</p></div>
                }.into_view(),
                Some(p) => {
                    let guests = p.guests.clone();
                    view! {
                        <table class="data-table" id="pve-guests">
                            <thead><tr><th>"Import"</th><th>"Proxmox id"</th><th>"Kind"</th><th>"Name"</th><th>"Horcrux id"</th><th>"Carried / changed / not carried"</th><th>"Status"</th><th></th></tr></thead>
                            <tbody>
                                {guests.into_iter().map(|g| {
                                    let vmid = g.vmid;
                                    let status = if let Some(a) = &g.imported_as {
                                        view! { <span>"already imported as "<a href=if g.kind == "vm" { format!("/vms/{a}") } else { format!("/containers/{a}/lxc") }>{a.clone()}</a></span> }.into_view()
                                    } else if g.exists {
                                        view! { <span class="text-warning">"a guest with this id exists: change the id"</span> }.into_view()
                                    } else if !g.blockers.is_empty() {
                                        view! { <span class="text-danger">{format!("{} blocker(s)", g.blockers.len())}</span> }.into_view()
                                    } else {
                                        view! { <span>"ready"</span> }.into_view()
                                    };
                                    view! {
                                        <tr class="pve-guest" data-vmid=vmid.to_string()>
                                            <td><input type="checkbox" class="pve-pick" aria-label=format!("import guest {vmid}")
                                                prop:checked=move || picked.with(|s| s.contains(&vmid))
                                                disabled=!g.blockers.is_empty() || g.imported_as.is_some()
                                                on:change=move |ev| { let on = event_target_checked(&ev); picked.update(|s| { if on { s.insert(vmid); } else { s.remove(&vmid); } }); }/></td>
                                            <td>{vmid}</td>
                                            <td>{if g.kind == "vm" { "VM" } else { "container" }}</td>
                                            <td>{g.name.clone()}</td>
                                            <td><input type="text" class="pve-id" aria-label=format!("Horcrux id for guest {vmid}")
                                                prop:value=move || ids.with(|m| m.get(&vmid).cloned().unwrap_or_default())
                                                on:input=move |ev| { let v = event_target_value(&ev); ids.update(|m| { m.insert(vmid, v); }); }/></td>
                                            <td>{format!("{} / {} / {}", count(&g, "carried"), count(&g, "changed"), count(&g, "not-carried"))}</td>
                                            <td>{status}</td>
                                            <td><button class="btn btn-sm pve-details" on:click=move |_| open.update(|o| *o = if *o == Some(vmid) { None } else { Some(vmid) })>
                                                {move || if open.get() == Some(vmid) { "Hide details" } else { "Details" }}</button></td>
                                        </tr>
                                    }
                                }).collect_view()}
                            </tbody>
                        </table>
                        {move || open.get().and_then(|v| plan.get().and_then(|p| p.guests.into_iter().find(|g| g.vmid == v))).map(|g| view! {
                            <div class="info-box" id="pve-detail">
                                <h3>{format!("{} {} -> {}", if g.kind == "vm" { "VM" } else { "Container" }, g.vmid, g.target_id)}</h3>
                                {(!g.blockers.is_empty()).then(|| view! {
                                    <div class="alert alert-error" id="pve-blockers"><strong>"Not imported until fixed"</strong>
                                        <ul>{g.blockers.iter().map(|b| view! { <li>{b.clone()}</li> }).collect_view()}</ul></div>
                                })}
                                <h4>"Data that moves into Horcrux storage"</h4>
                                <ul id="pve-volumes">{g.volumes.iter().map(|v| view! { <li>{format!("{} ({}): {}{}{}", v.key, v.role, v.source,
                                    v.size_bytes.map(|b| format!(", {:.1} GiB", b as f64 / (1u64 << 30) as f64)).unwrap_or_default(),
                                    if v.resolved.is_some() { "" } else { " - not found" })}</li> }).collect_view()}</ul>
                                <h4>"Every Proxmox setting"</h4>
                                <table class="data-table" id="pve-rows">
                                    <thead><tr><th>"Key"</th><th>"Value"</th><th>"Result"</th><th>"In Horcrux"</th><th>"Note"</th></tr></thead>
                                    <tbody>{g.rows.iter().map(|r| view! {
                                        <tr class=format!("pve-row pve-{}", r.carry)><td>{r.key.clone()}</td><td class="mono">{r.value.clone()}</td>
                                            <td>{carry_label(&r.carry)}</td><td>{r.horcrux.clone()}</td><td>{r.note.clone()}</td></tr>
                                    }).collect_view()}</tbody>
                                </table>
                                {g.qemu_diff.clone().map(|d| view! {
                                    <h4>"QEMU command line: Proxmox vs Horcrux"</h4>
                                    <table class="data-table" id="pve-qemu-diff">
                                        <thead><tr><th>"What the guest sees"</th><th>"Proxmox"</th><th>"Horcrux"</th><th></th></tr></thead>
                                        <tbody>{d.into_iter().map(|r| view! {
                                            <tr class=if r.same { "pve-same" } else { "pve-differs" }><td>{r.fact.clone()}</td><td class="mono">{r.proxmox.clone()}</td>
                                                <td class="mono">{r.horcrux.clone()}</td><td>{if r.same { "same".to_string() } else if r.note.is_empty() { "differs".to_string() } else { r.note.clone() }}</td></tr>
                                        }).collect_view()}</tbody>
                                    </table>
                                })}
                                {g.horcrux_cmdline.clone().map(|c| view! {
                                    <details><summary>"Horcrux QEMU command line"</summary><pre class="mono" id="pve-cmdline">{c}</pre></details>
                                })}
                            </div>
                        })}
                        <div class="form-row">
                            <label for="pve-storage">"VM disk storage"</label>
                            <select id="pve-storage" prop:value=storage on:change=move |ev| storage.set(event_target_value(&ev))>
                                <option value="">"VM storage directory"</option>
                                {move || options.get().map(|o| o.storage_pools.into_iter().map(|p| view! { <option value=p.id.clone()>{format!("{} ({})", p.name, p.storage_type)}</option> }).collect_view())}
                            </select>
                            <button class="btn btn-primary" id="pve-import" disabled=busy on:click=do_import>
                                {move || format!("Import {} guest(s)", picked.with(|s| s.len()))}
                            </button>
                        </div>
                    }.into_view()
                }
            }}

            {move || job.get().map(|j| view! {
                <div class="info-box" id="pve-job">
                    <h3>{format!("Import {}: {} ({:.0}%)", j.id, j.state, j.percent)}</h3>
                    <progress max="100" value=j.percent.to_string()></progress>
                    <p>{j.message.clone()}</p>
                    <table class="data-table" id="pve-results">
                        <thead><tr><th>"Proxmox id"</th><th>"Name"</th><th>"State"</th><th>"Result"</th></tr></thead>
                        <tbody>{j.guests.into_iter().map(|r| view! {
                            <tr class=format!("pve-result pve-result-{}", r.state)>
                                <td>{r.vmid}</td><td>{r.name.clone()}</td><td>{r.state.clone()}</td>
                                <td>
                                    {match (r.link.clone(), r.id.clone()) {
                                        (Some(l), Some(i)) => view! { <A href=l>{format!("Open {i}")}</A> }.into_view(),
                                        _ => view! { <span>{r.error.clone().unwrap_or_else(|| r.message.clone())}</span> }.into_view(),
                                    }}
                                    {(!r.warnings.is_empty()).then(|| view! {
                                        <details><summary>{format!("{} note(s)", r.warnings.len())}</summary>
                                            <ul>{r.warnings.iter().map(|w| view! { <li>{w.clone()}</li> }).collect_view()}</ul></details>
                                    })}
                                </td>
                            </tr>
                        }).collect_view()}</tbody>
                    </table>
                    {(j.state != "running").then(|| view! {
                        <p><A href="/vms/import-libvirt#imported-sources">"Review the old source for removal"</A>" (opt-in, with sizes)"</p>
                    })}
                </div>
            })}
        </div>
    }
}
