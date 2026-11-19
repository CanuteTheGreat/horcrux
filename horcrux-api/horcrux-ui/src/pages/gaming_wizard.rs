//! Gaming VM wizard (docs/design/gaming-vm.md, phase 2): pick a GPU from
//! the host check, size the VM, pick (or download) the Windows 11 and
//! virtio-win ISOs, set the Windows account, list the games to check
//! against the anti-cheat table, preview, then create and start the
//! unattended install. The result is an ordinary VM (Secure Boot, TPM 2.0,
//! VBS-ready, pinned vCPUs, huge pages, guest agent) editable on its pages.
use crate::api;
use crate::api::passthrough as pt;
use leptos::*;
use leptos_router::*;

fn opt_u64(s: &str) -> Option<u64> {
    s.trim().parse().ok()
}

#[component]
pub fn GamingVmWizard() -> impl IntoView {
    let (report, set_report) = create_signal(None::<pt::GamingReport>);
    let (opts, set_opts) = create_signal(None::<api::VmHostOptions>);
    let (settings, set_settings) = create_signal(None::<pt::GamingSettings>);
    let (compat, set_compat) = create_signal(Vec::<pt::CompatEntry>::new());
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let (plan, set_plan) = create_signal(None::<pt::GamingPlan>);
    let (busy, set_busy) = create_signal(false);

    let name = create_rw_signal("windows-gaming".to_string());
    let gpu = create_rw_signal(String::new());
    let allow_display_loss = create_rw_signal(false);
    let vcpus = create_rw_signal(String::new());
    let memory = create_rw_signal(String::new());
    let disk = create_rw_signal(String::new());
    let storage = create_rw_signal(String::new());
    let windows_iso = create_rw_signal(String::new());
    let virtio_iso = create_rw_signal(String::new());
    let windows_url = create_rw_signal(String::new());
    let username = create_rw_signal(String::new());
    let password = create_rw_signal(String::new());
    let product_key = create_rw_signal(String::new());
    let edition = create_rw_signal(String::new());
    let timezone = create_rw_signal(String::new());
    let games = create_rw_signal(String::new());
    let vbs = create_rw_signal(true);
    let hugepages = create_rw_signal(true);
    let start = create_rw_signal(true);
    let show_settings = create_rw_signal(false);
    let edit_settings = create_rw_signal(pt::GamingSettings::default());

    let reload_isos = move || {
        spawn_local(async move {
            if let Ok(o) = api::get_vm_host_options().await {
                // pre-pick ISOs whose names say what they are
                let find = |needle: &str| {
                    o.isos
                        .iter()
                        .find(|i| !i.downloading && i.name.to_lowercase().contains(needle))
                        .map(|i| i.path.clone())
                };
                if virtio_iso.get_untracked().is_empty() {
                    if let Some(p) = find("virtio") {
                        virtio_iso.set(p);
                    }
                }
                if windows_iso.get_untracked().is_empty() {
                    if let Some(p) = find("win").filter(|p| !p.to_lowercase().contains("virtio")) {
                        windows_iso.set(p);
                    }
                }
                set_opts.set(Some(o));
            }
        });
    };

    spawn_local(async move {
        match pt::gaming_check().await {
            Ok(g) => {
                if let Some(best) = g
                    .gpus
                    .iter()
                    .find(|c| c.verdict == "ready")
                    .or_else(|| g.gpus.iter().find(|c| c.verdict == "ready-with-warnings"))
                {
                    gpu.set(best.address.clone());
                }
                vcpus.set(g.suggested_vcpus.to_string());
                vbs.set(g.nested_virt);
                set_report.set(Some(g));
            }
            Err(e) => set_error.set(Some(format!("Gaming VM check: {}", e.message))),
        }
        match pt::gaming_settings().await {
            Ok(s) => {
                disk.set(s.default_disk_gb.to_string());
                hugepages.set(s.hugepages_kb != 0);
                timezone.set(s.timezone.clone());
                // the wizard always sends its choice, so it applies the
                // node's VBS defaults itself
                if let Some(g) = report.get_untracked() {
                    vbs.set(g.nested_virt && s.vbs && (!g.host_is_vm || s.vbs_on_virtual_host));
                }
                edit_settings.set(s.clone());
                set_settings.set(Some(s));
            }
            Err(e) => set_error.set(Some(format!("Wizard settings: {}", e.message))),
        }
        if let Ok(c) = pt::gaming_compat().await {
            set_compat.set(c.games);
        }
    });
    reload_isos();

    let request = move |dry_run: bool| pt::GamingVmRequest {
        name: name.get_untracked().trim().to_string(),
        gpu: Some(gpu.get_untracked()).filter(|g| !g.is_empty()),
        allow_display_loss: allow_display_loss.get_untracked(),
        vcpus: vcpus.get_untracked().trim().parse().ok(),
        memory_mb: opt_u64(&memory.get_untracked()).map(|gb| gb * 1024),
        disk_gb: opt_u64(&disk.get_untracked()),
        storage: Some(storage.get_untracked()).filter(|s| !s.is_empty()),
        bridge: None,
        windows_iso: windows_iso.get_untracked(),
        virtio_iso: virtio_iso.get_untracked(),
        username: username.get_untracked().trim().to_string(),
        password: password.get_untracked(),
        product_key: Some(product_key.get_untracked().trim().to_string()).filter(|k| !k.is_empty()),
        image_name: Some(edition.get_untracked().trim().to_string()).filter(|k| !k.is_empty()),
        ui_language: None,
        input_locale: None,
        timezone: Some(timezone.get_untracked().trim().to_string()).filter(|k| !k.is_empty()),
        evdev: vec![],
        hugepages: Some(hugepages.get_untracked()),
        vbs: Some(vbs.get_untracked()),
        games: games
            .get_untracked()
            .split([',', '\n'])
            .map(|g| g.trim().to_string())
            .filter(|g| !g.is_empty())
            .collect(),
        start: start.get_untracked(),
        dry_run,
    };

    let navigate = use_navigate();
    let submit = move |dry_run: bool| {
        let req = request(dry_run);
        let navigate = navigate.clone();
        set_error.set(None);
        set_notice.set(None);
        set_busy.set(true);
        spawn_local(async move {
            match pt::create_gaming_vm(&req).await {
                Ok(p) if p.created => {
                    let id = p.config.id.clone();
                    navigate(
                        &format!("/vms/{}", urlencoding::encode(&id)),
                        Default::default(),
                    );
                }
                Ok(p) => {
                    set_plan.set(Some(p));
                    set_busy.set(false);
                }
                Err(e) => {
                    set_error.set(Some(e.message));
                    set_busy.set(false);
                }
            }
        });
    };
    let preview = {
        let submit = submit.clone();
        move |_| submit(true)
    };
    let create = move |_| submit(false);

    let download = move |url: String| {
        set_error.set(None);
        spawn_local(async move {
            match api::download_iso(&url, None, "", None).await {
                Ok(f) => {
                    set_notice.set(Some(format!(
                        "Downloading {} into the ISO library; pick it below when it is complete.",
                        f.name
                    )));
                    reload_isos();
                }
                Err(e) => set_error.set(Some(format!("Download: {}", e.message))),
            }
        });
    };
    let download_virtio = move |_| {
        if let Some(s) = settings.get_untracked() {
            download(s.virtio_win_url)
        }
    };
    let download_eval = move |_| {
        if let Some(s) = settings.get_untracked() {
            download(s.windows_eval_iso_url)
        }
    };
    let download_windows = move |_| {
        let u = windows_url.get_untracked().trim().to_string();
        if u.is_empty() {
            set_error.set(Some(
                "Paste the download link Microsoft's page gave you first.".into(),
            ));
        } else {
            download(u)
        }
    };
    let save_settings = move |_| {
        let s = edit_settings.get_untracked();
        spawn_local(async move {
            match pt::save_gaming_settings(&s).await {
                Ok(s) => {
                    set_settings.set(Some(s));
                    set_notice.set(Some("Wizard settings saved.".into()));
                }
                Err(e) => set_error.set(Some(format!("Settings: {}", e.message))),
            }
        });
    };

    let iso_options = move |selected: RwSignal<String>| {
        opts.get()
            .map(|o| {
                o.isos
                    .into_iter()
                    .map(|i| {
                        let p = i.path.clone();
                        let p2 = p.clone();
                        let label = if i.downloading {
                            format!("{} (downloading...)", i.name)
                        } else {
                            i.name.clone()
                        };
                        view! { <option value=p selected=move || selected.get() == p2 disabled=i.downloading>{label}</option> }
                    })
                    .collect_view()
            })
            .unwrap_or_default()
    };

    // settings field helpers
    let text_setting = move |id: &'static str,
                             label: &'static str,
                             get: fn(&pt::GamingSettings) -> String,
                             set: fn(&mut pt::GamingSettings, String)| {
        view! {
            <div class="form-group">
                <label for=id>{label}</label>
                <input id=id type="text" prop:value=move || get(&edit_settings.get())
                    on:input=move |ev| edit_settings.update(|s| set(s, event_target_value(&ev)))/>
            </div>
        }
    };
    let num_setting = move |id: &'static str,
                            label: &'static str,
                            get: fn(&pt::GamingSettings) -> u64,
                            set: fn(&mut pt::GamingSettings, u64)| {
        view! {
            <div class="form-group">
                <label for=id>{label}</label>
                <input id=id type="number" prop:value=move || get(&edit_settings.get()).to_string()
                    on:input=move |ev| if let Ok(v) = event_target_value(&ev).parse() { edit_settings.update(|s| set(s, v)) }/>
            </div>
        }
    };

    view! {
        <div class="vm-create gaming-wizard" id="gaming-wizard">
            <h1>"New Gaming VM (Windows 11)"</h1>
            <p class="form-help">
                "A Windows 11 VM set up for games: the chosen GPU is handed to it while it runs, its vCPUs are pinned, Secure Boot, TPM 2.0 and VBS are on, and Windows installs by itself. The hypervisor is never hidden: games whose anti-cheat refuses VMs keep refusing (listed below)."
            </p>
            {move || error.get().map(|e| view! { <div class="error" id="gaming-error">{e}</div> })}
            {move || notice.get().map(|n| view! { <div class="notice" id="gaming-notice">{n}</div> })}

            <h3>"1. GPU"</h3>
            {move || report.get().map(|g| {
                let missing: Vec<&str> = [
                    (!g.kvm).then_some("KVM (enable VT-x / AMD-V in the firmware)"),
                    (!g.secure_boot_firmware).then_some("Secure Boot UEFI firmware (OVMF)"),
                    (!g.swtpm).then_some("swtpm (TPM 2.0)"),
                    g.iso_tool.is_empty().then_some("an ISO builder (xorriso)"),
                ].into_iter().flatten().collect();
                view! {
                    {(!missing.is_empty()).then(|| view! {
                        <p class="error" id="gaming-missing">{format!("This host is missing: {}.", missing.join(", "))}</p>
                    })}
                    {(!g.nested_virt).then(|| view! {
                        <p class="warning" id="gaming-nested">{format!("VBS / Memory Integrity unavailable: {}", g.nested_virt_problem)}</p>
                    })}
                    {(g.nested_virt && g.host_is_vm).then(|| view! {
                        <p class="warning" id="gaming-host-vm">"This Horcrux host is itself a virtual machine: with VBS / Memory Integrity, Windows' hypervisor would run a third level deep, and on many clouds and CPUs Windows then restarts in a loop at boot. VBS is off unless you turn it on."</p>
                    })}
                    <div class="form-group" id="gaming-gpu-pick">
                        {g.gpus.iter().map(|c| {
                            let a = c.address.clone();
                            let a2 = a.clone();
                            let usable = c.verdict != "blocked";
                            let label = format!(
                                "{} {} {}:{} - {}{}",
                                c.address, c.vendor_name, c.vendor_id, c.device_id,
                                match c.verdict.as_str() {
                                    "ready" => "ready",
                                    "ready-with-warnings" => "usable, see notes",
                                    "single-gpu" => "the host's only screen (single-GPU passthrough)",
                                    _ => "not usable",
                                },
                                if c.has_outputs == Some(false) { ", no display outputs" } else { "" },
                            );
                            view! {
                                <label class="radio gaming-gpu-option" data-addr=a.clone()>
                                    <input type="radio" name="gaming-gpu" value=a.clone() disabled=!usable
                                        prop:checked=move || gpu.get() == a
                                        on:change=move |_| gpu.set(a2.clone())/>
                                    {label}
                                </label>
                                <ul class="gaming-reasons">{c.reasons.iter().map(|r| view! { <li>{r.clone()}</li> }).collect_view()}</ul>
                            }
                        }).collect_view()}
                        <label class="radio">
                            <input type="radio" name="gaming-gpu" value="" id="gaming-gpu-none"
                                prop:checked=move || gpu.get().is_empty() on:change=move |_| gpu.set(String::new())/>
                            "No GPU yet (install Windows first, add the GPU on the Hardware page later)"
                        </label>
                    </div>
                    {
                        let g2 = g.clone();
                        move || {
                            let single = g2.gpus.iter().any(|c| c.address == gpu.get() && c.verdict == "single-gpu");
                            single.then(|| view! {
                                <label class="checkbox" id="gaming-single-gpu">
                                    <input type="checkbox" prop:checked=allow_display_loss
                                        on:change=move |ev| allow_display_loss.set(event_target_checked(&ev))/>
                                    "Single-GPU passthrough: this machine's screen and desktop stop while the VM runs and come back when Windows shuts down."
                                </label>
                            })
                        }
                    }
                }
            })}

            <h3>"2. Resources"</h3>
            <div class="form-row">
                <div class="form-group">
                    <label for="gaming-name">"Name"</label>
                    <input id="gaming-name" type="text" prop:value=name on:input=move |ev| name.set(event_target_value(&ev))/>
                </div>
                <div class="form-group">
                    <label for="gaming-vcpus">"vCPUs (pinned)"</label>
                    <input id="gaming-vcpus" type="number" min="2" prop:value=vcpus on:input=move |ev| vcpus.set(event_target_value(&ev))/>
                    <p class="form-help">{move || report.get().map(|g| format!("Suggested: {} on host CPUs {} (performance cores / one CCD).", g.suggested_vcpus, g.suggested_pinning))}</p>
                </div>
                <div class="form-group">
                    <label for="gaming-memory">"Memory (GB)"</label>
                    <input id="gaming-memory" type="number" min="4" placeholder="automatic (half the host, capped)" prop:value=memory on:input=move |ev| memory.set(event_target_value(&ev))/>
                </div>
                <div class="form-group">
                    <label for="gaming-disk">"System disk (GB)"</label>
                    <input id="gaming-disk" type="number" min="64" prop:value=disk on:input=move |ev| disk.set(event_target_value(&ev))/>
                </div>
                <div class="form-group">
                    <label for="gaming-storage">"Disk storage"</label>
                    <select id="gaming-storage" on:change=move |ev| storage.set(event_target_value(&ev))>
                        <option value="">"VM storage directory"</option>
                        {move || opts.get().map(|o| o.storage_pools.into_iter().map(|p| view! {
                            <option value=p.id.clone()>{format!("{} ({}, {} GB free)", p.name, p.storage_type, p.available)}</option>
                        }).collect_view())}
                    </select>
                </div>
            </div>
            <label class="checkbox"><input type="checkbox" id="gaming-hugepages" prop:checked=hugepages on:change=move |ev| hugepages.set(event_target_checked(&ev))/>"Huge pages for guest memory"</label>
            <label class="checkbox"><input type="checkbox" id="gaming-vbs" prop:checked=vbs on:change=move |ev| vbs.set(event_target_checked(&ev))/>"VBS / Memory Integrity (needs nested virtualisation; some anti-cheats ask for it)"</label>

            <h3>"3. Installation media"</h3>
            <div class="form-group">
                <label for="gaming-windows-iso">"Windows 11 ISO"</label>
                <select id="gaming-windows-iso" on:change=move |ev| windows_iso.set(event_target_value(&ev))>
                    <option value="">"pick from the ISO library..."</option>
                    {move || iso_options(windows_iso)}
                </select>
                <p class="form-help">
                    "Get it from Microsoft: open "
                    {move || settings.get().map(|s| view! { <a id="gaming-ms-page" href=s.windows_download_page.clone() target="_blank" rel="noopener">"Microsoft's Windows 11 download page"</a> })}
                    " (or the "
                    {move || settings.get().map(|s| view! { <a id="gaming-eval-page" href=s.windows_eval_page.clone() target="_blank" rel="noopener">"90-day evaluation"</a> })}
                    "), choose the 64-bit ISO and paste its download link here; it goes straight into the ISO library. Or upload your own ISO on the ISO Library page."
                </p>
                <div class="form-row">
                    <input id="gaming-windows-url" type="url" placeholder="https://software.download.prss.microsoft.com/..." prop:value=windows_url on:input=move |ev| windows_url.set(event_target_value(&ev))/>
                    <button type="button" class="btn" id="gaming-windows-download" on:click=download_windows>"Download to the library"</button>
                    <button type="button" class="btn" id="gaming-eval-download" on:click=download_eval>"Download the 90-day evaluation ISO"</button>
                    <A href="/storage/isos" class="btn">"ISO Library"</A>
                </div>
            </div>
            <div class="form-group">
                <label for="gaming-virtio-iso">"virtio-win drivers ISO"</label>
                <select id="gaming-virtio-iso" on:change=move |ev| virtio_iso.set(event_target_value(&ev))>
                    <option value="">"pick from the ISO library..."</option>
                    {move || iso_options(virtio_iso)}
                </select>
                <button type="button" class="btn" id="gaming-virtio-download" on:click=download_virtio>"Download the stable virtio-win ISO"</button>
                <button type="button" class="btn" id="gaming-iso-refresh" on:click=move |_| reload_isos()>"Refresh list"</button>
            </div>

            <h3>"4. Windows"</h3>
            <div class="form-row">
                <div class="form-group">
                    <label for="gaming-user">"Account name"</label>
                    <input id="gaming-user" type="text" autocomplete="off" prop:value=username on:input=move |ev| username.set(event_target_value(&ev))/>
                </div>
                <div class="form-group">
                    <label for="gaming-password">"Password"</label>
                    <input id="gaming-password" type="password" autocomplete="new-password" prop:value=password on:input=move |ev| password.set(event_target_value(&ev))/>
                    <p class="form-help">"Only written to the one-time answer disc, which is deleted once Windows is installed."</p>
                </div>
                <div class="form-group">
                    <label for="gaming-key">"Product key (optional)"</label>
                    <input id="gaming-key" type="text" placeholder="XXXXX-XXXXX-XXXXX-XXXXX-XXXXX" prop:value=product_key on:input=move |ev| product_key.set(event_target_value(&ev))/>
                </div>
                <div class="form-group">
                    <label for="gaming-edition">"Edition (optional)"</label>
                    <input id="gaming-edition" type="text" placeholder="Windows 11 Pro" prop:value=edition on:input=move |ev| edition.set(event_target_value(&ev))/>
                </div>
                <div class="form-group">
                    <label for="gaming-tz">"Time zone"</label>
                    <input id="gaming-tz" type="text" placeholder="Pacific Standard Time" prop:value=timezone on:input=move |ev| timezone.set(event_target_value(&ev))/>
                </div>
            </div>

            <h3>"5. Games you plan to play"</h3>
            <div class="form-group">
                <textarea id="gaming-games" rows="2" placeholder="e.g. Counter-Strike 2, Valorant" prop:value=games on:input=move |ev| games.set(event_target_value(&ev))></textarea>
                <p class="form-help">"Checked against the anti-cheat table below before anything is created."</p>
            </div>

            <label class="checkbox"><input type="checkbox" id="gaming-start" prop:checked=start on:change=move |ev| start.set(event_target_checked(&ev))/>"Start the VM and install Windows right away"</label>
            <div class="form-actions">
                <button type="button" class="btn" id="gaming-preview" disabled=busy on:click=preview>"Check plan"</button>
                <button type="button" class="btn btn-primary" id="gaming-create" disabled=busy on:click=create>"Create gaming VM"</button>
            </div>

            {move || plan.get().map(|p| view! {
                <div class="info-box" id="gaming-plan">
                    <h4>"Plan"</h4>
                    <ul id="gaming-plan-summary">
                        <li>{format!("{} vCPUs, {} GB memory, {} GB disk", p.config.cpus, p.config.memory / 1024, p.config.disk_size)}</li>
                        <li>{format!("Windows computer name {}, account {}", p.unattend.computer_name, p.unattend.username)}</li>
                        <li>{format!("GPU: {}", p.config.hardware.hostpci.first().map(|h| h.device.clone()).unwrap_or_else(|| "none".into()))}</li>
                        <li>{format!("Secure Boot {}, TPM 2.0 {}, VBS {}", p.config.hardware.secure_boot, p.config.hardware.tpm, p.config.hardware.adv.nested_virt)}</li>
                    </ul>
                    {(!p.warnings.is_empty()).then(|| view! {
                        <ul class="warning" id="gaming-plan-warnings">{p.warnings.iter().map(|w| view! { <li>{w.clone()}</li> }).collect_view()}</ul>
                    })}
                    <ul id="gaming-plan-notes">{p.notes.iter().map(|n| view! { <li>{n.clone()}</li> }).collect_view()}</ul>
                </div>
            })}

            <h3>"Online games and anti-cheat in a VM"</h3>
            <table class="data-table" id="gaming-compat">
                <thead><tr><th>"Game"</th><th>"Anti-cheat"</th><th>"In this VM"</th><th>"On the host (Steam)"</th><th>"Note"</th><th>"Source, checked"</th></tr></thead>
                <tbody>
                    {move || compat.get().into_iter().map(|g| view! {
                        <tr class={format!("gaming-compat-row vm-{}", g.vm_status)}>
                            <td>{g.title.clone()}</td>
                            <td>{g.anti_cheat.clone()}</td>
                            <td>{match g.vm_status.as_str() {
                                "works" => "works".to_string(),
                                "works-if" => format!("works with {}", g.requires.join(", ")),
                                "blocked-by-publisher" => "blocked by the publisher".to_string(),
                                _ => "unknown".to_string(),
                            }}</td>
                            <td>{g.host_status.clone()}</td>
                            <td>{g.note.clone()}</td>
                            <td><a href=g.source_url.clone() target="_blank" rel="noopener">{g.source.clone()}</a>{format!(" ({})", g.checked_on)}</td>
                        </tr>
                    }).collect_view()}
                </tbody>
            </table>

            <h3>
                <button type="button" class="btn btn-sm" id="gaming-settings-toggle" on:click=move |_| show_settings.update(|v| *v = !*v)>
                    {move || if show_settings.get() { "Hide wizard settings" } else { "Wizard settings (node defaults)" }}
                </button>
            </h3>
            <Show when=move || show_settings.get()>
                <div class="form-row" id="gaming-settings">
                    {text_setting("gs-ms-page", "Windows download page", |s| s.windows_download_page.clone(), |s, v| s.windows_download_page = v)}
                    {text_setting("gs-eval-page", "Windows evaluation page", |s| s.windows_eval_page.clone(), |s, v| s.windows_eval_page = v)}
                    {text_setting("gs-eval-iso", "Evaluation ISO link", |s| s.windows_eval_iso_url.clone(), |s, v| s.windows_eval_iso_url = v)}
                    {text_setting("gs-virtio-url", "virtio-win ISO URL", |s| s.virtio_win_url.clone(), |s, v| s.virtio_win_url = v)}
                    {num_setting("gs-mem-cap", "Default memory cap (MB)", |s| s.default_memory_cap_mb, |s, v| s.default_memory_cap_mb = v)}
                    {num_setting("gs-mem-min", "Minimum memory (MB)", |s| s.min_memory_mb, |s, v| s.min_memory_mb = v)}
                    {num_setting("gs-disk", "Default disk (GB)", |s| s.default_disk_gb, |s, v| s.default_disk_gb = v)}
                    {num_setting("gs-disk-min", "Minimum disk (GB)", |s| s.min_disk_gb, |s, v| s.min_disk_gb = v)}
                    {num_setting("gs-huge", "Huge page size (KiB, 0 = off)", |s| s.hugepages_kb, |s, v| s.hugepages_kb = v)}
                    {text_setting("gs-smbios-mfr", "SMBIOS manufacturer", |s| s.smbios_manufacturer.clone(), |s, v| s.smbios_manufacturer = v)}
                    {text_setting("gs-smbios-product", "SMBIOS product", |s| s.smbios_product.clone(), |s, v| s.smbios_product = v)}
                    {text_setting("gs-smbios-family", "SMBIOS family", |s| s.smbios_family.clone(), |s, v| s.smbios_family = v)}
                    {text_setting("gs-smbios-version", "SMBIOS version", |s| s.smbios_version.clone(), |s, v| s.smbios_version = v)}
                    {text_setting("gs-lang", "Windows language", |s| s.ui_language.clone(), |s, v| s.ui_language = v)}
                    {text_setting("gs-input", "Keyboard layout", |s| s.input_locale.clone(), |s, v| s.input_locale = v)}
                    {text_setting("gs-tz", "Time zone", |s| s.timezone.clone(), |s, v| s.timezone = v)}
                    {num_setting("gs-index", "install.wim image index", |s| s.image_index as u64, |s, v| s.image_index = v as u32)}
                    {text_setting("gs-letters", "Driver disc letters", |s| s.driver_search_letters.clone(), |s, v| s.driver_search_letters = v)}
                    {num_setting("gs-bootkey", "Boot key seconds", |s| s.boot_key_secs as u64, |s, v| s.boot_key_secs = v as u32)}
                    {num_setting("gs-poll", "Answer disc check interval (s)", |s| s.unattend_poll_secs, |s, v| s.unattend_poll_secs = v)}
                    <label class="checkbox"><input type="checkbox" id="gs-vbs" prop:checked=move || edit_settings.get().vbs
                        on:change=move |ev| { let c = event_target_checked(&ev); edit_settings.update(|s| s.vbs = c) }/>"VBS on by default"</label>
                    <label class="checkbox"><input type="checkbox" id="gs-vbs-vhost" prop:checked=move || edit_settings.get().vbs_on_virtual_host
                        on:change=move |ev| { let c = event_target_checked(&ev); edit_settings.update(|s| s.vbs_on_virtual_host = c) }/>"VBS on by default even when this host is itself a VM"</label>
                    <button type="button" class="btn btn-primary" id="gs-save" on:click=save_settings>"Save settings"</button>
                </div>
            </Show>
        </div>
    }
}
