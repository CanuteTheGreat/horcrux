//! LXC pages (Proxmox `pct`/`pveam` parity):
//! - `/containers/lxc/templates`: template catalog + local cache (pveam).
//! - `/containers/:id/lxc`: one container's lifecycle, settings (pct set),
//!   disk resize, snapshots, clone, and the generated LXC config.
pub mod spec;

use crate::api;
use leptos::*;
use leptos_router::*;

fn err_box(e: ReadSignal<Option<String>>) -> impl IntoView {
    move || {
        e.get()
            .map(|m| view! { <div class="alert alert-error lxc-error">{m}</div> })
    }
}

fn ok_box(m: ReadSignal<Option<String>>) -> impl IntoView {
    move || {
        m.get()
            .map(|m| view! { <div class="alert alert-success lxc-ok">{m}</div> })
    }
}

fn human_size(b: u64) -> String {
    let mb = b as f64 / (1024.0 * 1024.0);
    if mb >= 1024.0 {
        format!("{:.1} GiB", mb / 1024.0)
    } else {
        format!("{mb:.1} MiB")
    }
}

/// `/containers/lxc/templates`
#[component]
pub fn LxcTemplatesPage() -> impl IntoView {
    let (templates, set_templates) = create_signal(Vec::<api::LxcTemplate>::new());
    let (catalog, set_catalog) = create_signal(Vec::<api::LxcCatalogEntry>::new());
    let (filter, set_filter) = create_signal(String::new());
    let (variant_default_only, set_variant_default_only) = create_signal(true);
    let (busy, set_busy) = create_signal(None::<String>);
    let (error, set_error) = create_signal(None::<String>);
    let (msg, set_msg) = create_signal(None::<String>);

    let load_templates = move || {
        spawn_local(async move {
            match api::get_lxc_templates().await {
                Ok(t) => set_templates.set(t),
                Err(e) => set_error.set(Some(e.message)),
            }
        })
    };
    let load_catalog = move |refresh: bool| {
        spawn_local(async move {
            match api::get_lxc_catalog(None, refresh).await {
                Ok(c) => set_catalog.set(c),
                Err(e) => set_error.set(Some(format!("Template catalog: {}", e.message))),
            }
        })
    };
    create_effect(move |_| {
        load_templates();
        load_catalog(false);
    });

    let download = move |e: api::LxcCatalogEntry| {
        set_busy.set(Some(e.id.clone()));
        set_error.set(None);
        set_msg.set(None);
        spawn_local(async move {
            match api::download_lxc_template(&e).await {
                Ok(t) => set_msg.set(Some(format!(
                    "Downloaded {} (build {}, {}, sha256 verified)",
                    t.id,
                    t.build,
                    human_size(t.size_bytes)
                ))),
                Err(err) => set_error.set(Some(format!("Download {}: {}", e.id, err.message))),
            }
            set_busy.set(None);
            load_templates();
        });
    };
    let remove = move |id: String| {
        spawn_local(async move {
            match api::delete_lxc_template(&id).await {
                Ok(()) => set_msg.set(Some(format!("Deleted {id}"))),
                Err(e) => set_error.set(Some(format!("Delete {id}: {}", e.message))),
            }
            load_templates();
        });
    };

    view! {
        <div class="container-list lxc-templates">
            <div class="page-header">
                <h1>"LXC Templates"</h1>
                <div class="header-actions">
                    <a href="/containers/create" class="btn btn-primary">"Create Container"</a>
                    <a href="/containers" class="btn">"Containers"</a>
                </div>
            </div>
            {err_box(error)}
            {ok_box(msg)}

            <h2>"Downloaded"</h2>
            {move || {
                let list = templates.get();
                if list.is_empty() {
                    view! { <p class="no-data" id="lxc-no-templates">"No templates downloaded yet. Pick one from the catalog below (or just create a container: its template is downloaded on first use)."</p> }.into_view()
                } else {
                    view! {
                        <table class="container-table" id="lxc-template-table">
                            <thead><tr><th>"Template"</th><th>"Build"</th><th>"Size"</th><th>"SHA-256"</th><th>"Downloaded"</th><th></th></tr></thead>
                            <tbody>
                                {list.into_iter().map(|t| {
                                    let id = t.id.clone();
                                    let create_href = format!("/containers/create?runtime=lxc&template={}", urlencoding::encode(&format!("{}/{}/{}/{}", t.distro, t.release, t.arch, t.variant)));
                                    view! {
                                        <tr data-template-id=t.id.clone()>
                                            <td><code>{t.id.clone()}</code></td>
                                            <td>{t.build.clone()}</td>
                                            <td>{human_size(t.size_bytes)}</td>
                                            <td><code title=t.sha256.clone()>{t.sha256.chars().take(12).collect::<String>()}"…"</code></td>
                                            <td>{crate::utils::format_rfc3339(&t.downloaded_at)}</td>
                                            <td class="actions">
                                                <a class="btn btn-sm btn-primary" href=create_href>"Create container"</a>
                                                <button class="btn btn-sm btn-danger" on:click=move |_| remove(id.clone())>"Delete"</button>
                                            </td>
                                        </tr>
                                    }
                                }).collect_view()}
                            </tbody>
                        </table>
                    }.into_view()
                }
            }}

            <h2>"Catalog (images.linuxcontainers.org)"</h2>
            <div class="form-row">
                <input id="lxc-catalog-filter" type="text" placeholder="filter: debian, alpine/3.22, ubuntu noble..."
                    prop:value=filter on:input=move |ev| set_filter.set(event_target_value(&ev))/>
                <label>
                    <input type="checkbox" prop:checked=variant_default_only
                        on:change=move |ev| set_variant_default_only.set(event_target_checked(&ev))/>
                    " default variant only"
                </label>
                <button class="btn btn-sm" on:click=move |_| load_catalog(true)>"Refresh"</button>
            </div>
            <table class="container-table" id="lxc-catalog-table">
                <thead><tr><th>"Distribution"</th><th>"Release"</th><th>"Arch"</th><th>"Variant"</th><th>"Build"</th><th></th></tr></thead>
                <tbody>
                    {move || {
                        let f = filter.get().to_lowercase();
                        let words: Vec<String> = f.split_whitespace().map(str::to_string).collect();
                        let only_default = variant_default_only.get();
                        catalog.get().into_iter()
                            .filter(|e| !only_default || e.variant == "default")
                            .filter(|e| words.iter().all(|w| e.id.contains(w.as_str())))
                            .take(200)
                            .map(|e| {
                                let e2 = e.clone();
                                let id = e.id.clone();
                                let downloading = move || busy.get().as_deref() == Some(id.as_str());
                                view! {
                                    <tr data-catalog-id=e.id.clone()>
                                        <td>{e.distro.clone()}</td>
                                        <td>{e.release.clone()}</td>
                                        <td>{e.arch.clone()}</td>
                                        <td>{e.variant.clone()}</td>
                                        <td>{e.build.clone()}</td>
                                        <td><button class="btn btn-sm btn-primary lxc-download"
                                            disabled=move || busy.get().is_some()
                                            on:click=move |_| download(e2.clone())>
                                            {move || if downloading() { "Downloading..." } else { "Download" }}
                                        </button></td>
                                    </tr>
                                }
                            }).collect_view()
                    }}
                </tbody>
            </table>
        </div>
    }
}

/// `/containers/:id/lxc`
#[component]
pub fn LxcContainerPage() -> impl IntoView {
    let params = use_params_map();
    let id = move || params.get().get("id").cloned().unwrap_or_default();

    let (status, set_status) = create_signal(String::from("unknown"));
    let (view_cfg, set_view_cfg) = create_signal(None::<api::LxcConfigView>);
    let (snapshots, set_snapshots) = create_signal(Vec::<api::LxcSnapshot>::new());
    let (error, set_error) = create_signal(None::<String>);
    let (msg, set_msg) = create_signal(None::<String>);
    let (working, set_working) = create_signal(false);

    // Editable settings.
    let (memory, set_memory) = create_signal(String::new());
    let (cpus, set_cpus) = create_signal(String::new());
    let (swap, set_swap) = create_signal(String::new());
    let (cpu_limit, set_cpu_limit) = create_signal(String::new());
    let (cpu_units, set_cpu_units) = create_signal(String::new());
    let (hostname, set_hostname) = create_signal(String::new());
    let (nets, set_nets) = create_signal(String::new());
    let (mounts, set_mounts) = create_signal(String::new());
    let (nesting, set_nesting) = create_signal(false);
    let (keyctl, set_keyctl) = create_signal(false);
    let (fuse, set_fuse) = create_signal(false);
    let (onboot, set_onboot) = create_signal(false);
    let (order, set_order) = create_signal(String::new());
    let (nameserver, set_nameserver) = create_signal(String::new());
    let (searchdomain, set_searchdomain) = create_signal(String::new());
    let (ssh_keys, set_ssh_keys) = create_signal(String::new());
    let (resize_gb, set_resize_gb) = create_signal(String::new());
    let (snap_name, set_snap_name) = create_signal(String::new());
    let (snap_comment, set_snap_comment) = create_signal(String::new());
    let (clone_name, set_clone_name) = create_signal(String::new());

    let fill = move |v: &api::LxcConfigView| {
        let o = &v.lxc;
        set_memory.set(v.memory.to_string());
        set_cpus.set(v.cpus.to_string());
        set_swap.set(o.swap_mb.to_string());
        set_cpu_limit.set(o.cpu_limit.map(|c| c.to_string()).unwrap_or_default());
        set_cpu_units.set(o.cpu_units.map(|c| c.to_string()).unwrap_or_default());
        set_hostname.set(o.hostname.clone().unwrap_or_default());
        set_nets.set(spec::format_nets(&o.nets));
        set_mounts.set(spec::format_mounts(&o.mounts));
        set_nesting.set(o.features.nesting);
        set_keyctl.set(o.features.keyctl);
        set_fuse.set(o.features.fuse);
        set_onboot.set(o.onboot);
        set_order.set(o.startup_order.map(|n| n.to_string()).unwrap_or_default());
        set_nameserver.set(o.nameserver.clone().unwrap_or_default());
        set_searchdomain.set(o.searchdomain.clone().unwrap_or_default());
        set_ssh_keys.set(o.ssh_public_keys.join("\n"));
    };

    // Fetch status, settings and snapshots and refill the form from the server.
    let load = move || {
        let cid = id();
        async move {
            match api::get_container_config(&cid).await {
                Ok(c) => set_status.set(c.status.to_string()),
                Err(e) => set_error.set(Some(e.message)),
            }
            match api::get_lxc_config(&cid).await {
                Ok(v) => {
                    fill(&v);
                    set_view_cfg.set(Some(v));
                }
                Err(e) => set_error.set(Some(format!("Settings: {}", e.message))),
            }
            if let Ok(s) = api::get_container_snapshots(&cid).await {
                set_snapshots.set(s);
            }
        }
    };
    create_effect(move |_| spawn_local(load()));

    // Run an action, show its outcome, reload.
    let act = move |label: &'static str,
                    fut: std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, api::ApiError>>>,
    >| {
        set_working.set(true);
        set_error.set(None);
        set_msg.set(None);
        spawn_local(async move {
            let outcome = fut.await;
            // Refill the form BEFORE reporting the outcome and re-enabling the buttons: the
            // reload overwrites every field, so anything typed after "Settings saved" appeared
            // but before the reload landed was silently lost (seen in CI: a hostname typed right
            // after a save was wiped and the next save sent no change).
            load().await;
            match outcome {
                Ok(m) => set_msg.set(Some(m)),
                Err(e) => set_error.set(Some(format!("{label}: {}", e.message))),
            }
            set_working.set(false);
        });
    };

    let build_options = move || -> Result<(Option<u64>, Option<u32>, api::LxcOptions), String> {
        let base = view_cfg.get().map(|v| v.lxc).unwrap_or_default();
        let o = api::LxcOptions {
            swap_mb: spec::opt_num::<u64>("Swap", &swap.get())?.unwrap_or(0),
            cpu_limit: spec::opt_num("CPU limit", &cpu_limit.get())?,
            cpu_units: spec::opt_num("CPU units", &cpu_units.get())?,
            hostname: spec::opt_text(&hostname.get()),
            nets: spec::parse_nets(&nets.get())?,
            mounts: spec::parse_mounts(&mounts.get())?,
            features: horcrux_common::LxcFeatures {
                nesting: nesting.get(),
                keyctl: keyctl.get(),
                fuse: fuse.get(),
            },
            onboot: onboot.get(),
            startup_order: spec::opt_num("Boot order", &order.get())?,
            nameserver: spec::opt_text(&nameserver.get()),
            searchdomain: spec::opt_text(&searchdomain.get()),
            ssh_public_keys: crate::pages::container_create::lines(&ssh_keys.get()),
            ..base
        };
        o.validate()?;
        Ok((
            spec::opt_num("Memory", &memory.get())?,
            spec::opt_num("CPUs", &cpus.get())?,
            o,
        ))
    };

    let save = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        match build_options() {
            Ok((m, c, o)) => {
                let cid = id();
                act(
                    "Save",
                    Box::pin(async move {
                        let v = api::update_lxc_config(&cid, m, c, &o).await?;
                        Ok(if v.pending.is_empty() {
                            "Settings saved and applied".to_string()
                        } else {
                            format!(
                                "Settings saved; resource limits applied now, {} apply on the next reboot",
                                v.pending.join(", ")
                            )
                        })
                    }),
                );
            }
            Err(e) => set_error.set(Some(e)),
        }
    };

    let lifecycle = move |what: &'static str| {
        let cid = id();
        act(
            what,
            Box::pin(async move {
                match what {
                    "start" => api::start_container(&cid).await?,
                    "shutdown" => {
                        api::shutdown_container(&cid, None, false).await?;
                    }
                    "stop" => {
                        api::kill_container(&cid).await?;
                    }
                    "reboot" => {
                        api::reboot_container(&cid).await?;
                    }
                    "pause" => {
                        api::pause_container(&cid).await?;
                    }
                    _ => {
                        api::resume_container(&cid).await?;
                    }
                }
                Ok(format!("{what}: done"))
            }),
        );
    };

    view! {
        <div class="container-list lxc-container">
            <div class="page-header">
                <h1>"LXC container " {id}</h1>
                <div class="header-actions">
                    <span id="lxc-status" class=move || format!("status-{}", status.get())>{move || status.get()}</span>
                    <button id="lxc-start" class="btn btn-sm btn-success" disabled=working on:click=move |_| lifecycle("start")>"Start"</button>
                    <button id="lxc-shutdown" class="btn btn-sm btn-warning" disabled=working on:click=move |_| lifecycle("shutdown")>"Shutdown"</button>
                    <button id="lxc-stop" class="btn btn-sm btn-danger" disabled=working on:click=move |_| lifecycle("stop")>"Stop"</button>
                    <button id="lxc-reboot" class="btn btn-sm" disabled=working on:click=move |_| lifecycle("reboot")>"Reboot"</button>
                    <button id="lxc-pause" class="btn btn-sm" disabled=working on:click=move |_| lifecycle("pause")>"Freeze"</button>
                    <button id="lxc-resume" class="btn btn-sm" disabled=working on:click=move |_| lifecycle("resume")>"Thaw"</button>
                    <a class="btn btn-sm" href=move || format!("/containers/{}/exec?runtime=lxc", id())>"Console"</a>
                    <a class="btn btn-sm" href=move || format!("/containers/{}/logs?runtime=lxc", id())>"Boot log"</a>
                    <a class="btn btn-sm" href="/containers">"Back"</a>
                </div>
            </div>
            {err_box(error)}
            {ok_box(msg)}
            {move || view_cfg.get().map(|v| view! {
                <p class="lxc-summary">
                    "Template " <code>{v.template.clone()}</code>
                    " · " {v.distro.clone()} " " {v.arch.clone()}
                    " · rootfs: " {if v.rootfs_kind == "loop" { format!("{} GB ext4 image", v.lxc.disk_gb.unwrap_or(0)) } else { "directory (no size limit)".to_string() }}
                    " · " {if v.lxc.unprivileged { format!("unprivileged (uid 0 → {})", v.lxc.idmap_base.unwrap_or(100000)) } else { "privileged".to_string() }}
                </p>
                {(!v.pending.is_empty()).then(|| view! {
                    <div class="alert alert-warning" id="lxc-pending">"Pending until next reboot: " {v.pending.join(", ")}</div>
                })}
            })}

            <h2>"Settings"</h2>
            <form id="lxc-config-form" class="form" on:submit=save>
                <div class="form-row">
                    <div class="form-group"><label>"Memory (MB)"</label>
                        <input id="lxc-memory" type="number" min="16" prop:value=memory on:input=move |ev| set_memory.set(event_target_value(&ev))/></div>
                    <div class="form-group"><label>"Swap (MB)"</label>
                        <input id="lxc-swap" type="number" min="0" prop:value=swap on:input=move |ev| set_swap.set(event_target_value(&ev))/></div>
                    <div class="form-group"><label>"CPU cores"</label>
                        <input id="lxc-cpus" type="number" min="1" prop:value=cpus on:input=move |ev| set_cpus.set(event_target_value(&ev))/></div>
                    <div class="form-group"><label>"CPU limit (cores, optional)"</label>
                        <input id="lxc-cpu-limit" type="text" placeholder="e.g. 0.5" prop:value=cpu_limit on:input=move |ev| set_cpu_limit.set(event_target_value(&ev))/></div>
                    <div class="form-group"><label>"CPU units (weight)"</label>
                        <input id="lxc-cpu-units" type="number" min="1" max="10000" placeholder="100" prop:value=cpu_units on:input=move |ev| set_cpu_units.set(event_target_value(&ev))/></div>
                </div>
                <div class="form-group"><label>"Hostname"</label>
                    <input id="lxc-hostname" type="text" prop:value=hostname on:input=move |ev| set_hostname.set(event_target_value(&ev))/></div>
                <div class="form-group"><label>"Network devices (one per line: name=eth0,bridge=vmbr0,tag=20,ip=dhcp|10.0.0.5/24,gw=10.0.0.1,ip6=auto,mtu=1500)"</label>
                    <textarea id="lxc-nets" rows="3" prop:value=nets on:input=move |ev| set_nets.set(event_target_value(&ev))></textarea></div>
                <div class="form-group"><label>"Mount points (one per line: mp=/srv/data,source=/host/dir[,ro=1] for a bind mount, or mp=/var/lib/app for a managed volume)"</label>
                    <textarea id="lxc-mounts" rows="3" prop:value=mounts on:input=move |ev| set_mounts.set(event_target_value(&ev))></textarea></div>
                <div class="form-row">
                    <label><input id="lxc-nesting" type="checkbox" prop:checked=nesting on:change=move |ev| set_nesting.set(event_target_checked(&ev))/>" nesting"</label>
                    <label><input id="lxc-keyctl" type="checkbox" prop:checked=keyctl on:change=move |ev| set_keyctl.set(event_target_checked(&ev))/>" keyctl"</label>
                    <label><input id="lxc-fuse" type="checkbox" prop:checked=fuse on:change=move |ev| set_fuse.set(event_target_checked(&ev))/>" fuse"</label>
                    <label><input id="lxc-onboot" type="checkbox" prop:checked=onboot on:change=move |ev| set_onboot.set(event_target_checked(&ev))/>" start at boot"</label>
                    <div class="form-group"><label>"Boot order"</label>
                        <input id="lxc-order" type="number" min="0" prop:value=order on:input=move |ev| set_order.set(event_target_value(&ev))/></div>
                </div>
                <div class="form-row">
                    <div class="form-group"><label>"DNS servers"</label>
                        <input id="lxc-nameserver" type="text" placeholder="use host settings" prop:value=nameserver on:input=move |ev| set_nameserver.set(event_target_value(&ev))/></div>
                    <div class="form-group"><label>"DNS search domain"</label>
                        <input id="lxc-searchdomain" type="text" prop:value=searchdomain on:input=move |ev| set_searchdomain.set(event_target_value(&ev))/></div>
                </div>
                <div class="form-group"><label>"SSH public keys (root, one per line)"</label>
                    <textarea id="lxc-ssh-keys" rows="2" prop:value=ssh_keys on:input=move |ev| set_ssh_keys.set(event_target_value(&ev))></textarea></div>
                <div class="form-actions"><button id="lxc-save" type="submit" class="btn btn-primary" disabled=working>"Save settings"</button></div>
            </form>

            <h2>"Root disk"</h2>
            {move || view_cfg.get().map(|v| if v.rootfs_kind == "loop" {
                let cid = id();
                view! {
                    <div class="form-row">
                        <span>"Current size: " {v.lxc.disk_gb.unwrap_or(0)} " GB"</span>
                        <input id="lxc-resize-gb" type="number" min="1" placeholder="new size (GB)" prop:value=resize_gb on:input=move |ev| set_resize_gb.set(event_target_value(&ev))/>
                        <button id="lxc-resize" class="btn btn-sm" disabled=working on:click=move |_| {
                            let cid = cid.clone();
                            match spec::opt_num::<u64>("Disk size", &resize_gb.get()) {
                                Ok(Some(gb)) => act("Resize", Box::pin(async move {
                                    api::resize_container_disk(&cid, gb).await?;
                                    Ok(format!("Root disk grown to {gb} GB"))
                                })),
                                Ok(None) => set_error.set(Some("Enter the new size in GB".into())),
                                Err(e) => set_error.set(Some(e)),
                            }
                        }>"Grow disk"</button>
                    </div>
                }.into_view()
            } else {
                view! { <p>"Directory rootfs (no size limit). Create the container with a disk size to get a resizable, size-limited disk."</p> }.into_view()
            })}

            <h2>"Snapshots"</h2>
            <div class="form-row">
                <input id="lxc-snap-name" type="text" placeholder="name (e.g. before-upgrade)" prop:value=snap_name on:input=move |ev| set_snap_name.set(event_target_value(&ev))/>
                <input id="lxc-snap-comment" type="text" placeholder="comment (optional)" prop:value=snap_comment on:input=move |ev| set_snap_comment.set(event_target_value(&ev))/>
                <button id="lxc-snap-create" class="btn btn-sm btn-primary" disabled=working on:click=move |_| {
                    let (cid, n, c) = (id(), snap_name.get(), spec::opt_text(&snap_comment.get()));
                    act("Snapshot", Box::pin(async move {
                        api::create_container_snapshot(&cid, n.trim(), c).await?;
                        Ok(format!("Snapshot {} created", n.trim()))
                    }));
                    set_snap_name.set(String::new());
                }>"Take snapshot"</button>
            </div>
            <table class="container-table" id="lxc-snapshots">
                <thead><tr><th>"Name"</th><th>"Comment"</th><th>"Taken"</th><th></th></tr></thead>
                <tbody>
                    {move || snapshots.get().into_iter().map(|s| {
                        let (n1, n2) = (s.name.clone(), s.name.clone());
                        view! {
                            <tr data-snapshot=s.name.clone()>
                                <td>{s.name.clone()}</td>
                                <td>{s.comment.clone().unwrap_or_default()}</td>
                                <td>{crate::utils::format_rfc3339(&s.created_at)}</td>
                                <td class="actions">
                                    <button class="btn btn-sm btn-warning lxc-rollback" disabled=working on:click=move |_| {
                                        let (cid, n) = (id(), n1.clone());
                                        let ok = web_sys::window().and_then(|w| w.confirm_with_message(&format!("Roll back to {n}? Changes since then are lost.")).ok()).unwrap_or(false);
                                        if ok {
                                            act("Rollback", Box::pin(async move {
                                                api::rollback_container_snapshot(&cid, &n).await?;
                                                Ok(format!("Rolled back to {n}"))
                                            }));
                                        }
                                    }>"Rollback"</button>
                                    <button class="btn btn-sm btn-danger lxc-snap-delete" disabled=working on:click=move |_| {
                                        let (cid, n) = (id(), n2.clone());
                                        act("Delete snapshot", Box::pin(async move {
                                            api::delete_container_snapshot(&cid, &n).await?;
                                            Ok(format!("Snapshot {n} deleted"))
                                        }));
                                    }>"Delete"</button>
                                </td>
                            </tr>
                        }
                    }).collect_view()}
                </tbody>
            </table>

            <h2>"Clone"</h2>
            <div class="form-row">
                <input id="lxc-clone-name" type="text" placeholder="new container name" prop:value=clone_name on:input=move |ev| set_clone_name.set(event_target_value(&ev))/>
                <button id="lxc-clone" class="btn btn-sm" disabled=working on:click=move |_| {
                    let (cid, n) = (id(), clone_name.get());
                    act("Clone", Box::pin(async move {
                        let c = api::clone_container(&cid, n.trim()).await?;
                        Ok(format!("Cloned to {} (full copy, new MAC addresses)", c.id))
                    }));
                }>"Full clone"</button>
            </div>

            <h2>"Generated LXC config"</h2>
            <pre id="lxc-raw-config" class="lxc-raw">{move || view_cfg.get().map(|v| v.raw_config).unwrap_or_default()}</pre>
        </div>
    }
}
