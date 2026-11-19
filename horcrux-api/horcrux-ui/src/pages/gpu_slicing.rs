//! GPU capability view and declarative SR-IOV provisioning (phase A of the
//! vGPU design): per GPU which modes exist (passthrough, SR-IOV VFs, vendor
//! vGPU), why the others do not, the VFs with their VRAM and VFIO migration
//! (host save) capability, and a plan / apply / roll back form per
//! SR-IOV-capable GPU.
use crate::api::passthrough as pt;
use leptos::*;

/// The PF's entry in the config; a PF that has none starts from what the
/// hardware has now (`init` = VF count and an equal VRAM quota, if any).
fn pf_entry(cfg: &mut pt::SriovConfig, addr: &str, init: (u32, Option<u64>)) -> usize {
    if let Some(i) = cfg.pfs.iter().position(|p| p.pf == addr) {
        return i;
    }
    cfg.pfs.push(pt::PfSpec {
        pf: addr.to_string(),
        num_vfs: init.0,
        vf_driver: "auto".into(),
        autoprobe: false,
        all: pt::VfSpec {
            vram_quota_mib: init.1,
            ..Default::default()
        },
        vfs: Default::default(),
    });
    cfg.pfs.len() - 1
}

#[component]
pub fn GpuSlicing() -> impl IntoView {
    let (caps, set_caps) = create_signal(Vec::<pt::GpuCapability>::new());
    let (loaded, set_loaded) = create_signal(false);
    let cfg = create_rw_signal(None::<pt::SriovConfig>);
    let (plan, set_plan) = create_signal(None::<pt::SriovPlan>);
    let (report, set_report) = create_signal(None::<pt::SriovReport>);
    let (gens, set_gens) = create_signal(Vec::<pt::SriovGeneration>::new());
    let (error, set_error) = create_signal(None::<String>);

    let reload = move || {
        spawn_local(async move {
            match pt::gpu_capabilities().await {
                Ok(c) => set_caps.set(c),
                Err(e) => set_error.set(Some(format!("GPU capabilities: {}", e.message))),
            }
            if let Ok(c) = pt::sriov_config().await {
                cfg.set(Some(c));
            }
            if let Ok(g) = pt::sriov_generations().await {
                set_gens.set(g);
            }
            set_loaded.set(true);
        });
    };
    reload();

    let spec_of = move |a: &str| {
        cfg.get()
            .and_then(|c| c.pfs.into_iter().find(|p| p.pf == a))
    };

    let edit = move |addr: String, init: (u32, Option<u64>), f: Box<dyn Fn(&mut pt::PfSpec)>| {
        cfg.update(|c| {
            if let Some(c) = c.as_mut() {
                let i = pf_entry(c, &addr, init);
                f(&mut c.pfs[i]);
            }
        });
        set_plan.set(None);
        set_report.set(None);
    };

    let run_plan = move |_| {
        let Some(c) = cfg.get_untracked() else { return };
        spawn_local(async move {
            set_error.set(None);
            set_report.set(None);
            match pt::sriov_plan(&c).await {
                Ok(p) => set_plan.set(Some(p)),
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };
    let run_apply = move |_| {
        let Some(c) = cfg.get_untracked() else { return };
        spawn_local(async move {
            set_error.set(None);
            match pt::sriov_apply(&c).await {
                Ok(r) => {
                    set_plan.set(None);
                    set_report.set(Some(r));
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            reload();
        });
    };
    let run_rollback = move |_| {
        spawn_local(async move {
            set_error.set(None);
            match pt::sriov_rollback(None).await {
                Ok(r) => {
                    set_plan.set(None);
                    set_report.set(Some(r));
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            reload();
        });
    };

    view! {
        <section id="gpu-slicing">
            <h3>"GPU capabilities"</h3>
            <p class="form-help">
                "Per GPU: how it can be given to VMs. SR-IOV virtual functions and vendor vGPU devices are \
                 assigned like any PCI device on the VM's Hardware page; \"host save\" says whether a VM using \
                 the device can be saved on host shutdown (read from the device's VFIO migration support)."
            </p>
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="gpu-slicing-error">{e}</div> })}
            {move || (!loaded.get()).then(|| view! { <p>"Reading GPU capabilities..."</p> })}
            {move || (loaded.get() && caps.get().is_empty()).then(|| view! {
                <p id="gpu-caps-none" class="form-help">
                    "No GPU found on this node. "
                    <button class="btn btn-sm" id="gpu-caps-rescan" on:click=move |_| reload()>"Scan again"</button>
                </p>
            })}
            <div id="gpu-caps">
                {move || caps.get().into_iter().map(|g| {
                    let addr = g.address.clone();
                    let sriov = g.sriov.clone();
                    let vgpu = g.vendor_vgpu.clone();
                    let (q1, q2, q3) = (addr.clone(), addr.clone(), addr.clone());
                    let pf_addr2 = addr.clone();
                    let pf_addr3 = addr.clone();
                    let pf_addr4 = addr.clone();
                    let max = sriov.as_ref().map(|s| s.total_vfs).unwrap_or(0);
                    let cur_vfs = sriov.as_ref().map(|s| s.current_vfs).unwrap_or(0);
                    // an equal VRAM share on every VF is the form's starting value
                    let init_vram = sriov.as_ref().and_then(|s| {
                        let first = s.vfs.first()?.vram_mib?;
                        s.vfs.iter().all(|v| v.vram_mib == Some(first)).then_some(first)
                    });
                    let init = (cur_vfs, init_vram);
                    let drivers = sriov.as_ref().map(|s| s.vf_drivers.clone()).unwrap_or_default();
                    let per_vf = sriov.as_ref().map(|s| s.per_vf_resources).unwrap_or(false);
                    view! {
                        <div class="gpu-cap card" data-addr=addr.clone()>
                            <h4>
                                <code>{addr.clone()}</code>
                                {format!(" {} {}:{} ({})", g.vendor_name, g.vendor_id, g.device_id, g.driver.clone().unwrap_or_else(|| "no driver".into()))}
                            </h4>
                            <ul class="gpu-modes">
                                <li class="gpu-mode" data-mode="passthrough" data-available=g.passthrough.available.to_string()>
                                    {if g.passthrough.available { "Passthrough: available".to_string() } else { format!("Passthrough: not available, {}", g.passthrough.reason) }}
                                </li>
                                <li class="gpu-mode" data-mode="sriov" data-available=sriov.is_some().to_string()>
                                    {match &sriov {
                                        Some(s) => format!("SR-IOV: {} of {} virtual functions enabled ({} driver{})", s.current_vfs, s.total_vfs, s.driver,
                                            if s.per_vf_resources { ", VRAM and scheduling per VF" } else { "" }),
                                        None => "SR-IOV: not available".to_string(),
                                    }}
                                </li>
                                <li class="gpu-mode" data-mode="vgpu" data-available=vgpu.is_some().to_string()>
                                    {match &vgpu {
                                        Some(v) => format!("Vendor vGPU: {} type(s), {} device(s)", v.types.len(), v.devices.len()),
                                        None => "Vendor vGPU: not available".to_string(),
                                    }}
                                </li>
                            </ul>
                            {(!g.why_not.is_empty()).then(|| view! {
                                <ul class="gpu-why-not">
                                    {g.why_not.iter().map(|w| view! { <li class="gpu-why">{w.clone()}</li> }).collect_view()}
                                </ul>
                            })}
                            {sriov.as_ref().filter(|s| !s.vfs.is_empty()).map(|s| view! {
                                <table class="data-table vf-table">
                                    <thead><tr><th>"VF"</th><th>"Address"</th><th>"Driver"</th><th>"VRAM"</th><th>"Used by"</th><th>"Save method"</th></tr></thead>
                                    <tbody>
                                        {s.vfs.iter().map(|v| view! {
                                            <tr class="vf-row" data-addr=v.address.clone()>
                                                <td>{v.vf}</td>
                                                <td><code>{v.address.clone()}</code></td>
                                                <td>{v.driver.clone().unwrap_or_else(|| "none".into())}</td>
                                                <td>{v.vram_mib.map(|m| format!("{m} MiB")).unwrap_or_else(|| "-".into())}</td>
                                                <td>{v.vm.clone().unwrap_or_default()}</td>
                                                <td class="vf-save" data-state=v.migration.state.clone()>{v.migration.label()}</td>
                                            </tr>
                                        }).collect_view()}
                                    </tbody>
                                </table>
                            })}
                            {vgpu.as_ref().filter(|v| !v.devices.is_empty()).map(|v| view! {
                                <table class="data-table mdev-save-table">
                                    <thead><tr><th>"vGPU device"</th><th>"Type"</th><th>"Used by"</th><th>"Save method"</th></tr></thead>
                                    <tbody>
                                        {v.devices.iter().map(|m| view! {
                                            <tr class="mdev-save-row">
                                                <td><code>{m.uuid.clone()}</code></td>
                                                <td>{m.mdev_type.clone()}</td>
                                                <td>{m.vm.clone().unwrap_or_default()}</td>
                                                <td class="vf-save" data-state=m.migration.state.clone()>{m.migration.label()}</td>
                                            </tr>
                                        }).collect_view()}
                                    </tbody>
                                </table>
                            })}
                            {(max > 0).then(|| view! {
                                <fieldset class="sriov-form" data-pf=addr.clone()>
                                    <legend>"Virtual functions (applied now and at every boot)"</legend>
                                    <div class="form-row">
                                        <label>"VFs (0-" {max} ")"
                                            <input type="number" class="sriov-num-vfs" min="0" max=max.to_string()
                                                prop:value=move || spec_of(&q1).map(|p| p.num_vfs).unwrap_or(cur_vfs).to_string()
                                                on:input=move |ev| {
                                                    let n = event_target_value(&ev).parse::<u32>().unwrap_or(0);
                                                    edit(pf_addr2.clone(), init, Box::new(move |p| p.num_vfs = n));
                                                }/>
                                        </label>
                                        {per_vf.then(|| view! {
                                            <label>"VRAM per VF (MiB, empty = equal share)"
                                                <input type="number" class="sriov-vram" min="1"
                                                    prop:value=move || spec_of(&q2).map(|p| p.all.vram_quota_mib).unwrap_or(init_vram).map(|m| m.to_string()).unwrap_or_default()
                                                    on:input=move |ev| {
                                                        let v = event_target_value(&ev).parse::<u64>().ok().filter(|m| *m > 0);
                                                        edit(pf_addr3.clone(), init, Box::new(move |p| p.all.vram_quota_mib = v));
                                                    }/>
                                            </label>
                                        })}
                                        <label>"Bind VFs to"
                                            <select class="sriov-driver"
                                                prop:value=move || spec_of(&q3).map(|p| p.vf_driver).unwrap_or_else(|| "auto".into())
                                                on:change=move |ev| {
                                                    let v = event_target_value(&ev);
                                                    edit(pf_addr4.clone(), init, Box::new(move |p| p.vf_driver = v.clone()));
                                                }>
                                                <option value="auto">"automatic (vendor VFIO driver when loaded)"</option>
                                                {drivers.iter().map(|d| view! { <option value=d.clone()>{d.clone()}</option> }).collect_view()}
                                            </select>
                                        </label>
                                    </div>
                                </fieldset>
                            })}
                        </div>
                    }
                }).collect_view()}
            </div>

            {move || caps.get().iter().any(|g| g.sriov.is_some()).then(|| view! {
                <div class="form-row" id="sriov-actions">
                    <button class="btn" id="sriov-plan-btn" on:click=run_plan>"Plan"</button>
                    <button class="btn btn-primary" id="sriov-apply-btn" on:click=run_apply>"Apply"</button>
                    <button class="btn btn-warning" id="sriov-rollback-btn" on:click=run_rollback
                        disabled=move || gens.get().len() < 2>"Roll back to the previous generation"</button>
                    <small class="form-help">{move || format!("{} generation(s) recorded", gens.get().len())}</small>
                </div>
            })}
            {move || plan.get().map(|p| view! {
                <div id="sriov-plan" class="info-box">
                    <h4>"Plan"</h4>
                    {p.errors.iter().map(|e| view! { <p class="alert alert-error sriov-plan-error">{e.clone()}</p> }).collect_view()}
                    {(p.steps.is_empty() && p.errors.is_empty()).then(|| view! { <p class="sriov-plan-empty">"Nothing to do: the host already matches."</p> })}
                    <ol>{p.steps.iter().map(|s| view! { <li class="sriov-plan-step">{format!("{}: {}", s.pf, s.detail)}</li> }).collect_view()}</ol>
                </div>
            })}
            {move || report.get().map(|r| view! {
                <div id="sriov-result" class={if r.ok { "alert alert-success" } else { "alert alert-error" }}>
                    {if r.ok { format!("Applied {} step(s){}.", r.results.len(), r.generation.map(|g| format!(", generation {g}")).unwrap_or_default()) } else { "Not applied.".to_string() }}
                    {r.plan.errors.iter().map(|e| view! { <div>{e.clone()}</div> }).collect_view()}
                    {r.results.iter().filter(|s| s.status != "applied").map(|s| view! {
                        <div>{format!("{} {} {}: {}", s.step.pf, s.step.action, s.status, s.message)}</div>
                    }).collect_view()}
                </div>
            })}
        </section>
    }
}
