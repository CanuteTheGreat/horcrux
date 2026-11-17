//! "Create Container" form (/containers/create), reached from the
//! Containers page's "Create Container" button.
use crate::api;
use leptos::*;
use leptos_router::*;

/// Split a textarea into non-empty trimmed lines.
pub fn lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Build the API request from the raw form values. Kept free of any
/// browser APIs so it can be unit-tested natively.
#[allow(clippy::too_many_arguments)]
pub fn build_request(
    name: &str,
    runtime: &str,
    image: &str,
    memory: &str,
    cpus: &str,
    env: &str,
    ports: &str,
    volumes: &str,
    command: &str,
) -> Result<api::CreateContainerRequest, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Name is required".to_string());
    }
    let mut image = image.trim();
    // Gentoo is the default LXC template (must match
    // horcrux-container's DEFAULT_LXC_TEMPLATE); Alpine etc. stay selectable.
    if image.is_empty() && runtime == "lxc" {
        image = "gentoo/current/openrc";
    }
    if image.is_empty() {
        return Err("Image is required".to_string());
    }
    let memory: u64 = memory
        .trim()
        .parse()
        .map_err(|_| "Memory must be a whole number of MB".to_string())?;
    let cpus: u32 = cpus
        .trim()
        .parse()
        .map_err(|_| "CPUs must be a whole number".to_string())?;
    if memory == 0 || cpus == 0 {
        return Err("Memory and CPUs must be greater than zero".to_string());
    }
    let oci = matches!(runtime, "docker" | "podman");
    let (env, ports, volumes, command) = (
        lines(env),
        lines(ports),
        lines(volumes),
        command
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>(),
    );
    if !oci && (!env.is_empty() || !ports.is_empty() || !volumes.is_empty() || !command.is_empty())
    {
        return Err(
            "Environment, ports, volumes and command are only supported for Docker and Podman"
                .to_string(),
        );
    }
    Ok(api::CreateContainerRequest {
        id: name.to_string(),
        name: name.to_string(),
        runtime: runtime.to_string(),
        memory,
        cpus,
        rootfs: image.to_string(),
        status: "stopped".to_string(),
        env,
        ports,
        volumes,
        command,
        lxc: None,
    })
}

/// Raw values of the LXC section of the form.
#[derive(Clone, Debug, Default)]
pub struct LxcForm {
    pub hostname: String,
    pub unprivileged: bool,
    pub disk_gb: String,
    pub swap_mb: String,
    pub cpu_limit: String,
    pub bridge: String,
    pub vlan: String,
    /// "dhcp", "static" or "none".
    pub ip_mode: String,
    pub ip: String,
    pub gateway: String,
    /// "none", "auto", "dhcp" or "static".
    pub ip6_mode: String,
    pub ip6: String,
    pub gateway6: String,
    pub mounts: String,
    pub nesting: bool,
    pub keyctl: bool,
    pub fuse: bool,
    pub onboot: bool,
    pub nameserver: String,
    pub ssh_keys: String,
}

impl LxcForm {
    pub fn to_options(&self) -> Result<horcrux_common::LxcOptions, String> {
        use crate::pages::lxc::spec::{opt_num, opt_text, parse_mounts};
        let mut nets = Vec::new();
        let bridge = self.bridge.trim();
        if !bridge.is_empty() {
            let ip = match self.ip_mode.as_str() {
                "dhcp" => Some("dhcp".to_string()),
                "static" => Some(
                    opt_text(&self.ip).ok_or("Static IPv4 needs an address like 10.0.0.5/24")?,
                ),
                _ => None,
            };
            let ip6 = match self.ip6_mode.as_str() {
                "auto" => Some("auto".to_string()),
                "dhcp" => Some("dhcp".to_string()),
                "static" => Some(
                    opt_text(&self.ip6).ok_or("Static IPv6 needs an address like fd00::5/64")?,
                ),
                _ => None,
            };
            nets.push(horcrux_common::LxcNet {
                name: Some("eth0".into()),
                bridge: bridge.to_string(),
                vlan: opt_num("VLAN tag", &self.vlan)?,
                gateway: if self.ip_mode == "static" {
                    opt_text(&self.gateway)
                } else {
                    None
                },
                gateway6: if self.ip6_mode == "static" {
                    opt_text(&self.gateway6)
                } else {
                    None
                },
                ip,
                ip6,
                ..Default::default()
            });
        }
        let o = horcrux_common::LxcOptions {
            unprivileged: self.unprivileged,
            hostname: opt_text(&self.hostname),
            disk_gb: opt_num("Disk size", &self.disk_gb)?,
            swap_mb: opt_num("Swap", &self.swap_mb)?.unwrap_or(0),
            cpu_limit: opt_num("CPU limit", &self.cpu_limit)?,
            nets,
            mounts: parse_mounts(&self.mounts)?,
            features: horcrux_common::LxcFeatures {
                nesting: self.nesting,
                keyctl: self.keyctl,
                fuse: self.fuse,
                ..Default::default()
            },
            onboot: self.onboot,
            nameserver: opt_text(&self.nameserver),
            ssh_public_keys: lines(&self.ssh_keys),
            ..Default::default()
        };
        o.validate()?;
        Ok(o)
    }
}

#[component]
pub fn ContainerCreate() -> impl IntoView {
    let (name, set_name) = create_signal(String::new());
    let (runtime, set_runtime) = create_signal(String::new());
    let (image, set_image) = create_signal(String::new());
    // CPU/RAM/disk sliders (same component as the Create VM form)
    let memory = create_rw_signal(512u64);
    let cpus = create_rw_signal(1u64);
    let disk_gb = create_rw_signal(8u64);
    let (disk_limit, set_disk_limit) = create_signal(false);
    let (env, set_env) = create_signal(String::new());
    let (ports, set_ports) = create_signal(String::new());
    let (volumes, set_volumes) = create_signal(String::new());
    let (command, set_command) = create_signal(String::new());
    let (start_after, set_start_after) = create_signal(true);
    let (creating, set_creating) = create_signal(false);
    let (error, set_error) = create_signal(None::<String>);

    // Which runtimes really work on this node (live probe), plus the
    // host's CPU/RAM/disk bounds. The form offers only working runtimes and
    // says what to do when there is none (journey QA finding 13).
    let rt_info = create_rw_signal(None::<api::ContainerRuntimes>);
    let (rt_error, set_rt_error) = create_signal(None::<String>);
    let (checking, set_checking) = create_signal(true);
    // `?runtime=` asked for (LXC templates page); honoured when it works.
    let wanted = create_rw_signal(None::<String>);
    let (wanted_note, set_wanted_note) = create_signal(None::<String>);
    let load_runtimes = move |refresh: bool| {
        set_checking.set(true);
        spawn_local(async move {
            match api::get_container_runtimes(refresh).await {
                Ok(r) => {
                    let host = r.host.clone();
                    cpus.update(|c| *c = (*c).clamp(1, host.cpus.max(1) as u64));
                    let want = wanted.get_untracked();
                    let pick = choose_runtime(&r.runtimes, want.as_deref(), r.default.as_deref());
                    set_wanted_note.set(want.as_deref().and_then(|w| {
                        r.runtimes
                            .iter()
                            .find(|p| p.runtime == w && !p.available)
                            .map(|p| {
                                format!(
                                    "{} is not available on this node: {}",
                                    p.label,
                                    p.message.clone().unwrap_or_default()
                                )
                            })
                    }));
                    set_runtime.set(pick.unwrap_or_default());
                    set_rt_error.set(None);
                    rt_info.set(Some(r));
                }
                Err(e) => set_rt_error.set(Some(e.message)),
            }
            set_checking.set(false);
        });
    };
    load_runtimes(false);
    let available = move || {
        rt_info
            .get()
            .map(|r| {
                r.runtimes
                    .into_iter()
                    .filter(|p| p.available)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    let unavailable = move || {
        rt_info
            .get()
            .map(|r| {
                r.runtimes
                    .into_iter()
                    .filter(|p| p.compiled && !p.available)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    let none_available = move || rt_info.with(|r| r.is_some()) && available().is_empty();

    // LXC section.
    let lxc_form = create_rw_signal(LxcForm {
        unprivileged: true,
        ip_mode: "dhcp".into(),
        ip6_mode: "none".into(),
        swap_mb: "0".into(),
        ..Default::default()
    });
    let (lxc_host, set_lxc_host) = create_signal(None::<api::LxcHostInfo>);
    let (lxc_templates, set_lxc_templates) = create_signal(Vec::<api::LxcTemplate>::new());
    let (lxc_catalog, set_lxc_catalog) = create_signal(Vec::<api::LxcCatalogEntry>::new());

    // `?runtime=lxc&template=alpine/3.22/amd64/default` (from the LXC
    // templates page) preselects the runtime (once the probe says it works)
    // and the template.
    let query = use_query_map();
    create_effect(move |_| {
        let q = query.get();
        if let Some(rt) = q.get("runtime") {
            wanted.set(Some(rt.clone()));
            if rt_info.with_untracked(|r| {
                r.as_ref()
                    .is_some_and(|r| r.runtimes.iter().any(|p| &p.runtime == rt && p.available))
            }) {
                set_runtime.set(rt.clone());
            }
        }
        if let Some(t) = q.get("template") {
            set_image.set(t.clone());
        }
    });
    let lxc_loaded = create_rw_signal(false);
    create_effect(move |_| {
        if runtime.get() == "lxc" && !lxc_loaded.get_untracked() {
            lxc_loaded.set(true);
            spawn_local(async move {
                if let Ok(h) = api::get_lxc_host().await {
                    if let Some(b) = h.bridges.first() {
                        lxc_form.update(|f| {
                            if f.bridge.is_empty() {
                                f.bridge = b.name.clone();
                            }
                        });
                    }
                    set_lxc_host.set(Some(h));
                }
                if let Ok(t) = api::get_lxc_templates().await {
                    set_lxc_templates.set(t);
                }
                if let Ok(c) = api::get_lxc_catalog(None, false).await {
                    set_lxc_catalog.set(c.into_iter().filter(|e| e.variant == "default").collect());
                }
            });
        }
    });

    let navigate = use_navigate();

    let submit = Callback::new(move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        set_error.set(None);
        let mut req = match build_request(
            &name.get(),
            &runtime.get(),
            &image.get(),
            &memory.get().to_string(),
            &cpus.get().to_string(),
            &env.get(),
            &ports.get(),
            &volumes.get(),
            &command.get(),
        ) {
            Ok(r) => r,
            Err(e) => {
                set_error.set(Some(e));
                return;
            }
        };
        if runtime.get() == "lxc" {
            let mut f = lxc_form.get();
            f.disk_gb = if disk_limit.get() {
                disk_gb.get().to_string()
            } else {
                String::new()
            };
            match f.to_options() {
                Ok(o) => req.lxc = Some(o),
                Err(e) => {
                    set_error.set(Some(e));
                    return;
                }
            }
        }
        set_creating.set(true);
        let start = start_after.get();
        let navigate = navigate.clone();
        spawn_local(async move {
            match api::create_container(&req).await {
                Ok(created) => {
                    if start {
                        if let Err(e) = api::start_container(&created.id).await {
                            set_creating.set(false);
                            set_error.set(Some(format!(
                                "Container created, but starting it failed: {}",
                                e.message
                            )));
                            return;
                        }
                    }
                    navigate("/containers", Default::default());
                }
                Err(e) => {
                    set_creating.set(false);
                    set_error.set(Some(e.message));
                }
            }
        });
    });

    let oci = move || matches!(runtime.get().as_str(), "docker" | "podman");
    let host = move || rt_info.get().map(|r| r.host);

    view! {
        <div class="container-create">
            <div class="page-header">
                <h1>"Create Container"</h1>
                <div class="header-actions">
                    <a href="/containers" class="btn">"Cancel"</a>
                </div>
            </div>

            {move || error.get().map(|e| view! { <div class="alert alert-error" id="create-container-error">{e}</div> })}
            {move || rt_error.get().map(|e| view! { <div class="alert alert-error" id="ct-runtimes-error">{format!("Could not check the container runtimes: {e}")}</div> })}
            {move || wanted_note.get().map(|n| view! { <div class="alert alert-warning" id="ct-runtime-wanted-note">{n}</div> })}

            <Show when=move || checking.get() && rt_info.with(|r| r.is_none())>
                <p class="loading" id="ct-runtimes-loading">"Checking which container runtimes work on this node..."</p>
            </Show>

            <Show when=none_available>
                <div class="empty-state" id="ct-no-runtime">
                    <h2>"No container runtime is working on this node"</h2>
                    <p>"Containers need one of Podman, Docker, LXC, Incus or LXD installed and running here. Enable one, then check again:"</p>
                    <ul class="ct-runtime-help">
                        {move || {
                            let mut list = unavailable();
                            if list.is_empty() {
                                // nothing compiled in: show the USE flag hints
                                list = rt_info.get().map(|r| r.runtimes).unwrap_or_default();
                            }
                            list.into_iter().map(|p| view! {
                                <li data-runtime=p.runtime.clone()>
                                    <strong>{p.label.clone()}</strong>": "{p.message.clone().unwrap_or_default()}
                                    <br/><small class="ct-runtime-hint">{p.hint.clone().unwrap_or_default()}</small>
                                </li>
                            }).collect_view()
                        }}
                    </ul>
                    <div class="form-actions">
                        <button id="ct-runtime-recheck" type="button" class="btn btn-primary"
                            disabled=checking
                            on:click=move |_| load_runtimes(true)>
                            {move || if checking.get() { "Checking..." } else { "Check again" }}
                        </button>
                        <a href="/containers" class="btn">"Back to Containers"</a>
                    </div>
                </div>
            </Show>

            <Show when=move || !available().is_empty()>
            <form id="create-container-form" on:submit=move |ev| submit.call(ev) class="form">
                <div class="form-group">
                    <label for="ct-name">"Name"</label>
                    <input id="ct-name" name="name" type="text" required placeholder="web-1"
                        prop:value=name
                        on:input=move |ev| set_name.set(event_target_value(&ev))/>
                </div>
                <div class="form-group">
                    <label for="ct-runtime">"Runtime"</label>
                    // Only the runtimes that work on this node right now (live probe).
                    <select id="ct-runtime" name="runtime"
                        prop:value=runtime
                        on:change=move |ev| set_runtime.set(event_target_value(&ev))>
                        {move || {
                            available()
                                .into_iter()
                                .map(|p| {
                                    let v = p.runtime.clone();
                                    let label = match &p.version {
                                        Some(ver) => format!("{} ({ver})", p.label),
                                        None => p.label.clone(),
                                    };
                                    view! { <option value=p.runtime.clone() selected=move || runtime.get() == v>{label}</option> }
                                })
                                .collect_view()
                        }}
                    </select>
                    {move || {
                        let un = unavailable();
                        (!un.is_empty()).then(|| view! {
                            <details class="ct-runtime-unavailable" id="ct-runtime-unavailable">
                                <summary>{format!("Not available on this node: {}",
                                    un.iter().map(|p| p.label.as_str()).collect::<Vec<_>>().join(", "))}</summary>
                                <ul>
                                    {un.iter().map(|p| view! {
                                        <li data-runtime=p.runtime.clone()>
                                            <strong>{p.label.clone()}</strong>": "{p.message.clone().unwrap_or_default()}
                                            <br/><small class="ct-runtime-hint">{p.hint.clone().unwrap_or_default()}</small>
                                        </li>
                                    }).collect_view()}
                                </ul>
                            </details>
                        })
                    }}
                </div>
                <div class="form-group">
                    <label for="ct-image">{move || if oci() { "Image" } else { "Template / rootfs" }}</label>
                    <input id="ct-image" name="image" type="text" required
                        placeholder=move || if runtime.get() == "lxc" { "gentoo/current/openrc (default; distro/release from the catalog, e.g. alpine/3.22)" } else { "nginx:alpine" }
                        list="lxc-template-list"
                        prop:value=image
                        on:input=move |ev| set_image.set(event_target_value(&ev))/>
                    <datalist id="lxc-template-list">
                        {move || if runtime.get() == "lxc" {
                            let mut ids: Vec<String> = lxc_templates.get().into_iter()
                                .map(|t| format!("{}/{}/{}/{}", t.distro, t.release, t.arch, t.variant)).collect();
                            ids.extend(lxc_catalog.get().into_iter().map(|e| format!("{}/{}", e.distro, e.release)));
                            ids.dedup();
                            ids.into_iter().map(|i| view! { <option value=i.clone()></option> }).collect_view()
                        } else {
                            view! {}.into_view()
                        }}
                    </datalist>
                    <Show when=move || runtime.get() == "lxc">
                        <small>"Downloaded templates are used directly; any other catalog entry is downloaded (SHA-256 verified) on create. "
                            <a href="/containers/lxc/templates">"Manage templates"</a></small>
                    </Show>
                </div>
                <crate::components::ResourceSlider label="CPUs" id="ct-cpus" value=cpus
                    steps=Signal::derive(move || crate::components::cpu_steps(host().map(|h| h.cpus).unwrap_or(4)))
                    limit=Signal::derive(move || host().map(|h| h.cpus as u64).unwrap_or(u64::MAX))
                    hard_max=Signal::derive(move || host().map(|h| h.cpus as u64 * 4).unwrap_or(1024).max(4))
                    fmt=crate::components::human_cpus unit="CPUs" limit_what="logical CPUs" guest="container"/>
                <crate::components::ResourceSlider label="Memory" id="ct-memory" value=memory
                    steps=Signal::derive(move || crate::components::memory_steps(host().map(|h| h.max_vm_memory_mb).unwrap_or(8192)))
                    limit=Signal::derive(move || host().map(|h| h.max_vm_memory_mb).unwrap_or(u64::MAX))
                    hard_max=Signal::derive(move || host().map(|h| h.memory_mb * 2).unwrap_or(1 << 20).max(512))
                    fmt=crate::components::human_mb unit="MB" limit_what="available RAM (after the host's reserve)" guest="container"/>
                <Show when=move || runtime.get() == "lxc"
                    fallback=move || view! {
                        <p class="form-help" id="ct-disk-note">
                            {move || if oci() {
                                "Disk: the container's files live in the runtime's image storage (no per-container size limit)."
                            } else {
                                "Disk: the root filesystem lives in the runtime's default storage pool."
                            }}
                        </p>
                    }>
                    <div class="form-group">
                        <label class="checkbox-label">
                            <input type="checkbox" id="ct-disk-limit" prop:checked=disk_limit
                                on:change=move |ev| set_disk_limit.set(event_target_checked(&ev))/>
                            " Limit the root disk size (otherwise a plain directory, no limit)"
                        </label>
                    </div>
                    <Show when=move || disk_limit.get()>
                        <crate::components::ResourceSlider label="Disk" id="ct-disk" value=disk_gb
                            steps=Signal::derive(move || crate::components::disk_steps(rt_info.get().map(|r| r.lxc_disk_available_gb).unwrap_or(64)))
                            limit=Signal::derive(move || rt_info.get().map(|r| r.lxc_disk_available_gb).filter(|g| *g > 0).unwrap_or(u64::MAX))
                            hard_max=Signal::derive(move || 1u64 << 20)
                            fmt=crate::components::human_gb unit="GB" limit_what="free where LXC keeps containers" guest="container"/>
                    </Show>
                </Show>
                <Show when=oci>
                    <div class="form-group">
                        <label for="ct-ports">"Published ports (one per line, host:container)"</label>
                        <textarea id="ct-ports" name="ports" rows="2" placeholder="8080:80"
                            prop:value=ports
                            on:input=move |ev| set_ports.set(event_target_value(&ev))></textarea>
                    </div>
                    <div class="form-group">
                        <label for="ct-env">"Environment (one KEY=value per line)"</label>
                        <textarea id="ct-env" name="env" rows="3"
                            prop:value=env
                            on:input=move |ev| set_env.set(event_target_value(&ev))></textarea>
                    </div>
                    <div class="form-group">
                        <label for="ct-volumes">"Volumes (one source:/target per line)"</label>
                        <textarea id="ct-volumes" name="volumes" rows="2"
                            prop:value=volumes
                            on:input=move |ev| set_volumes.set(event_target_value(&ev))></textarea>
                    </div>
                    <div class="form-group">
                        <label for="ct-command">"Command (optional, overrides the image CMD)"</label>
                        <input id="ct-command" name="command" type="text" placeholder="sleep 3600"
                            prop:value=command
                            on:input=move |ev| set_command.set(event_target_value(&ev))/>
                    </div>
                </Show>
                <Show when=move || runtime.get() == "lxc">
                    <fieldset class="lxc-create" id="lxc-create-section">
                        <legend>"LXC"</legend>
                        {move || lxc_host.get().filter(|h| !h.available).map(|_| view! {
                            <div class="alert alert-warning">"LXC is not installed on this host (emerge app-containers/lxc)."</div>
                        })}
                        <div class="form-row">
                            <div class="form-group"><label for="lxc-hostname">"Hostname"</label>
                                <input id="lxc-hostname" type="text" placeholder="defaults to the name"
                                    prop:value=move || lxc_form.get().hostname
                                    on:input=move |ev| lxc_form.update(|f| f.hostname = event_target_value(&ev))/></div>
                            <div class="form-group"><label for="lxc-swap">"Swap (MB)"</label>
                                <input id="lxc-swap" type="number" min="0"
                                    prop:value=move || lxc_form.get().swap_mb
                                    on:input=move |ev| lxc_form.update(|f| f.swap_mb = event_target_value(&ev))/></div>
                            <div class="form-group"><label for="lxc-cpu-limit">"CPU limit (cores)"</label>
                                <input id="lxc-cpu-limit" type="text" placeholder="= CPUs"
                                    prop:value=move || lxc_form.get().cpu_limit
                                    on:input=move |ev| lxc_form.update(|f| f.cpu_limit = event_target_value(&ev))/></div>
                        </div>
                        <div class="form-row">
                            <div class="form-group"><label for="lxc-bridge">"Bridge"</label>
                                <select id="lxc-bridge" on:change=move |ev| lxc_form.update(|f| f.bridge = event_target_value(&ev))>
                                    <option value="" selected=move || lxc_form.get().bridge.is_empty()>"(no network)"</option>
                                    {move || lxc_host.get().map(|h| h.bridges).unwrap_or_default().into_iter().map(|b| {
                                        let n = b.name.clone();
                                        let label = if b.vlan_aware { format!("{} (VLAN aware)", b.name) } else { b.name.clone() };
                                        view! { <option value=b.name.clone() selected=move || lxc_form.get().bridge == n>{label}</option> }
                                    }).collect_view()}
                                </select></div>
                            <div class="form-group"><label for="lxc-vlan">"VLAN tag"</label>
                                <input id="lxc-vlan" type="number" min="1" max="4094"
                                    prop:value=move || lxc_form.get().vlan
                                    on:input=move |ev| lxc_form.update(|f| f.vlan = event_target_value(&ev))/></div>
                            <div class="form-group"><label for="lxc-ip-mode">"IPv4"</label>
                                <select id="lxc-ip-mode" on:change=move |ev| lxc_form.update(|f| f.ip_mode = event_target_value(&ev))>
                                    <option value="dhcp" selected=true>"DHCP"</option>
                                    <option value="static">"Static"</option>
                                    <option value="none">"None"</option>
                                </select></div>
                            <Show when=move || lxc_form.get().ip_mode == "static">
                                <div class="form-group"><label for="lxc-ip">"Address/prefix"</label>
                                    <input id="lxc-ip" type="text" placeholder="10.0.0.5/24"
                                        prop:value=move || lxc_form.get().ip
                                        on:input=move |ev| lxc_form.update(|f| f.ip = event_target_value(&ev))/></div>
                                <div class="form-group"><label for="lxc-gw">"Gateway"</label>
                                    <input id="lxc-gw" type="text" placeholder="10.0.0.1"
                                        prop:value=move || lxc_form.get().gateway
                                        on:input=move |ev| lxc_form.update(|f| f.gateway = event_target_value(&ev))/></div>
                            </Show>
                            <div class="form-group"><label for="lxc-ip6-mode">"IPv6"</label>
                                <select id="lxc-ip6-mode" on:change=move |ev| lxc_form.update(|f| f.ip6_mode = event_target_value(&ev))>
                                    <option value="none" selected=true>"None"</option>
                                    <option value="auto">"SLAAC"</option>
                                    <option value="dhcp">"DHCPv6"</option>
                                    <option value="static">"Static"</option>
                                </select></div>
                            <Show when=move || lxc_form.get().ip6_mode == "static">
                                <div class="form-group"><label for="lxc-ip6">"IPv6 address/prefix"</label>
                                    <input id="lxc-ip6" type="text" placeholder="fd00::5/64"
                                        prop:value=move || lxc_form.get().ip6
                                        on:input=move |ev| lxc_form.update(|f| f.ip6 = event_target_value(&ev))/></div>
                                <div class="form-group"><label for="lxc-gw6">"IPv6 gateway"</label>
                                    <input id="lxc-gw6" type="text"
                                        prop:value=move || lxc_form.get().gateway6
                                        on:input=move |ev| lxc_form.update(|f| f.gateway6 = event_target_value(&ev))/></div>
                            </Show>
                        </div>
                        <div class="form-group"><label for="lxc-mounts">"Mount points (one per line: mp=/srv/data,source=/host/dir[,ro=1] or mp=/var/lib/app for a managed volume)"</label>
                            <textarea id="lxc-mounts" rows="2"
                                prop:value=move || lxc_form.get().mounts
                                on:input=move |ev| lxc_form.update(|f| f.mounts = event_target_value(&ev))></textarea></div>
                        <div class="form-row">
                            <label><input id="lxc-unprivileged" type="checkbox" prop:checked=move || lxc_form.get().unprivileged
                                on:change=move |ev| lxc_form.update(|f| f.unprivileged = event_target_checked(&ev))/>" Unprivileged"</label>
                            <label><input id="lxc-nesting" type="checkbox" prop:checked=move || lxc_form.get().nesting
                                on:change=move |ev| lxc_form.update(|f| f.nesting = event_target_checked(&ev))/>" Nesting"</label>
                            <label><input id="lxc-keyctl" type="checkbox" prop:checked=move || lxc_form.get().keyctl
                                on:change=move |ev| lxc_form.update(|f| f.keyctl = event_target_checked(&ev))/>" keyctl"</label>
                            <label><input id="lxc-fuse" type="checkbox" prop:checked=move || lxc_form.get().fuse
                                on:change=move |ev| lxc_form.update(|f| f.fuse = event_target_checked(&ev))/>" FUSE"</label>
                            <label><input id="lxc-onboot" type="checkbox" prop:checked=move || lxc_form.get().onboot
                                on:change=move |ev| lxc_form.update(|f| f.onboot = event_target_checked(&ev))/>" Start at boot"</label>
                        </div>
                        <div class="form-row">
                            <div class="form-group"><label for="lxc-nameserver">"DNS servers"</label>
                                <input id="lxc-nameserver" type="text" placeholder="use host settings"
                                    prop:value=move || lxc_form.get().nameserver
                                    on:input=move |ev| lxc_form.update(|f| f.nameserver = event_target_value(&ev))/></div>
                            <div class="form-group"><label for="lxc-ssh">"SSH public keys (root)"</label>
                                <textarea id="lxc-ssh" rows="2"
                                    prop:value=move || lxc_form.get().ssh_keys
                                    on:input=move |ev| lxc_form.update(|f| f.ssh_keys = event_target_value(&ev))></textarea></div>
                        </div>
                    </fieldset>
                </Show>
                <div class="form-group">
                    <label>
                        <input id="ct-start" type="checkbox" prop:checked=start_after
                            on:change=move |ev| set_start_after.set(event_target_checked(&ev))/>
                        " Start after creating"
                    </label>
                </div>
                <div class="form-actions">
                    <button id="ct-submit" type="submit" class="btn btn-primary" disabled=creating>
                        {move || if creating.get() { "Creating..." } else { "Create Container" }}
                    </button>
                </div>
            </form>
            </Show>
        </div>
    }
}

/// The runtime the form starts on: the one asked for in `?runtime=` when it
/// works here, else the server's default (first working one in menu order).
pub fn choose_runtime(
    runtimes: &[api::ContainerRuntimeProbe],
    wanted: Option<&str>,
    default: Option<&str>,
) -> Option<String> {
    let works = |r: &str| runtimes.iter().any(|p| p.runtime == r && p.available);
    wanted
        .filter(|w| works(w))
        .or(default.filter(|d| works(d)))
        .map(str::to_string)
        .or_else(|| {
            runtimes
                .iter()
                .find(|p| p.available)
                .map(|p| p.runtime.clone())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(rt: &str, ok: bool) -> api::ContainerRuntimeProbe {
        api::ContainerRuntimeProbe {
            runtime: rt.into(),
            label: rt.into(),
            compiled: true,
            available: ok,
            ..Default::default()
        }
    }

    #[test]
    fn the_form_starts_on_a_runtime_that_works() {
        let v = [
            probe("podman", false),
            probe("docker", true),
            probe("lxc", true),
        ];
        // ?runtime=lxc is honoured when LXC works
        assert_eq!(
            choose_runtime(&v, Some("lxc"), Some("docker")).as_deref(),
            Some("lxc")
        );
        // ?runtime=podman is not: Podman does not work here
        assert_eq!(
            choose_runtime(&v, Some("podman"), Some("docker")).as_deref(),
            Some("docker")
        );
        assert_eq!(choose_runtime(&v, None, None).as_deref(), Some("docker"));
        // nothing works: no runtime (the form shows the empty state instead)
        assert_eq!(choose_runtime(&v[..1], Some("podman"), None), None);
    }

    #[test]
    fn builds_a_docker_request_from_form_values() {
        let r = build_request(
            " web-1 ",
            "docker",
            "nginx:alpine",
            "256",
            "2",
            "A=1\n\n B=2 ",
            "8080:80",
            "data:/data",
            "sleep 60",
        )
        .unwrap();
        assert_eq!(r.id, "web-1");
        assert_eq!(r.name, "web-1");
        assert_eq!(r.rootfs, "nginx:alpine");
        assert_eq!((r.memory, r.cpus), (256, 2));
        assert_eq!(r.env, vec!["A=1", "B=2"]);
        assert_eq!(r.ports, vec!["8080:80"]);
        assert_eq!(r.volumes, vec!["data:/data"]);
        assert_eq!(r.command, vec!["sleep", "60"]);
        // Serialized shape is exactly what POST /api/containers accepts.
        let json = serde_json::to_value(&r).unwrap();
        let cfg: horcrux_common::ContainerConfig = serde_json::from_value(json).unwrap();
        assert_eq!(cfg.runtime, horcrux_common::ContainerRuntime::Docker);
        assert_eq!(cfg.options.ports, vec!["8080:80"]);
        assert_eq!(cfg.options.command, vec!["sleep", "60"]);
    }

    #[test]
    fn builds_lxc_options_from_the_form() {
        let f = LxcForm {
            unprivileged: true,
            hostname: "web".into(),
            disk_gb: "4".into(),
            swap_mb: "0".into(),
            bridge: "vmbr0".into(),
            vlan: "20".into(),
            ip_mode: "static".into(),
            ip: "10.20.0.50/24".into(),
            gateway: "10.20.0.1".into(),
            ip6_mode: "auto".into(),
            mounts: "mp=/srv,source=/tank,ro=1\nmp=/data".into(),
            nesting: true,
            ssh_keys: "ssh-ed25519 AAAA a@b".into(),
            ..Default::default()
        };
        let o = f.to_options().unwrap();
        assert_eq!(o.disk_gb, Some(4));
        assert_eq!(o.nets.len(), 1);
        assert_eq!(o.nets[0].vlan, Some(20));
        assert_eq!(o.nets[0].ip.as_deref(), Some("10.20.0.50/24"));
        assert_eq!(o.nets[0].gateway.as_deref(), Some("10.20.0.1"));
        assert_eq!(o.nets[0].ip6.as_deref(), Some("auto"));
        assert_eq!(o.mounts.len(), 2);
        assert!(o.features.nesting && !o.features.keyctl);
        // The request body is exactly what POST /api/containers accepts.
        // Empty template on lxc = the Gentoo default; alpine stays selectable.
        let d = build_request("web", "lxc", "", "256", "1", "", "", "", "").unwrap();
        assert_eq!(d.rootfs, "gentoo/current/openrc");
        assert!(build_request("web", "docker", "", "256", "1", "", "", "", "").is_err());
        let mut req =
            build_request("web", "lxc", "alpine/3.22", "256", "1", "", "", "", "").unwrap();
        req.lxc = Some(o.clone());
        let cfg: horcrux_common::ContainerConfig =
            serde_json::from_value(serde_json::to_value(&req).unwrap()).unwrap();
        assert_eq!(cfg.lxc, Some(o));
        // No bridge = no network; dhcp ignores a stale gateway field.
        let none = LxcForm {
            unprivileged: true,
            ip_mode: "dhcp".into(),
            gateway: "1.2.3.4".into(),
            bridge: "br0".into(),
            ..Default::default()
        }
        .to_options()
        .unwrap();
        assert_eq!(none.nets[0].gateway, None);
        assert!(LxcForm::default().to_options().unwrap().nets.is_empty());
        // Bad values are rejected before submit.
        for bad in [
            LxcForm {
                bridge: "b".into(),
                ip_mode: "static".into(),
                ..Default::default()
            },
            LxcForm {
                bridge: "b".into(),
                vlan: "x".into(),
                ..Default::default()
            },
            LxcForm {
                disk_gb: "0".into(),
                ..Default::default()
            },
            LxcForm {
                mounts: "nope".into(),
                ..Default::default()
            },
        ] {
            assert!(bad.to_options().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn rejects_missing_or_invalid_fields() {
        let ok = |n: &str, img: &str, mem: &str, cpu: &str| {
            build_request(n, "docker", img, mem, cpu, "", "", "", "")
        };
        assert!(ok("", "nginx", "1", "1").is_err());
        assert!(ok("a", " ", "1", "1").is_err());
        assert!(ok("a", "nginx", "lots", "1").is_err());
        assert!(ok("a", "nginx", "0", "1").is_err());
        assert!(build_request("a", "lxc", "ubuntu", "512", "1", "A=1", "", "", "").is_err());
        assert!(build_request("a", "lxc", "ubuntu", "512", "1", "", "", "", "").is_ok());
    }
}
