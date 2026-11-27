//! Security: overview, vulnerability scans of container images and
//! containers, the host hardening baseline, and the settings.
//! API: horcrux-api/src/api/security; design docs/design/security-suite.md.
use crate::api::*;
use leptos::*;
use leptos_router::*;
use wasm_bindgen::JsCast;

fn ask(msg: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.confirm_with_message(msg).ok())
        .unwrap_or(false)
}

fn when(ts: Option<i64>) -> String {
    match ts {
        None => "never".into(),
        Some(t) => js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(t as f64 * 1000.0))
            .to_locale_string("default", &wasm_bindgen::JsValue::UNDEFINED)
            .as_string()
            .unwrap_or_default(),
    }
}

fn download(name: &str, mime: &str, text: &str) {
    use wasm_bindgen::JsValue;
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let parts = js_sys::Array::new();
    parts.push(&JsValue::from_str(text));
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type(mime);
    let Ok(blob) = web_sys::Blob::new_with_str_sequence_and_options(&parts, &opts) else {
        return;
    };
    let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob) else {
        return;
    };
    if let Ok(a) = doc
        .create_element("a")
        .map(|e| e.unchecked_into::<web_sys::HtmlAnchorElement>())
    {
        a.set_href(&url);
        a.set_download(name);
        a.click();
    }
    let _ = web_sys::Url::revoke_object_url(&url);
}

/// In-app link with exact active state (query strings count), no full reload.
#[component]
fn NavLink(
    #[prop(into)] href: String,
    #[prop(into)] class: Signal<String>,
    active: Signal<bool>,
    children: Children,
) -> impl IntoView {
    let navigate = use_navigate();
    let to = href.clone();
    view! {
        <a href=href class=class aria-current=move || active.get().then_some("page")
            on:click=move |ev: web_sys::MouseEvent| {
                if ev.button() == 0 && !ev.ctrl_key() && !ev.meta_key() && !ev.shift_key() {
                    ev.prevent_default();
                    navigate(&to, Default::default());
                }
            }>
            {children()}
        </a>
    }
}

/// Severity badge; zero/negligible never red.
#[component]
fn SevBadge(sev: Severity) -> impl IntoView {
    view! { <span class=format!("sev-badge sev-{}", sev.as_str())>{sev.as_str()}</span> }
}

/// Counts as compact badges, only non-zero severities; "none" when clean.
#[component]
fn CountBadges(counts: SeverityCounts) -> impl IntoView {
    let parts: Vec<_> = [Severity::Critical, Severity::High, Severity::Medium, Severity::Low]
        .into_iter()
        .filter(|s| counts.get(*s) > 0)
        .map(|s| {
            view! { <span class=format!("sev-badge sev-{}", s.as_str())>{format!("{} {}", counts.get(s), s.as_str())}</span> }
        })
        .collect();
    if parts.is_empty() {
        view! { <span class="sev-badge sev-none">"no known vulnerabilities"</span> }.into_view()
    } else {
        parts.into_view()
    }
}

#[component]
fn SecurityTabs(active: &'static str) -> impl IntoView {
    let tab = |href: &'static str, id: &'static str, label: &'static str| {
        let on = id == active;
        let cls = if on { "tab active" } else { "tab" };
        view! { <NavLink href=href class=Signal::derive(move || cls.to_string()) active=Signal::derive(move || on)>{label}</NavLink> }
    };
    view! {
        <nav class="tabs security-tabs" aria-label="Security sections">
            {tab("/security", "overview", "Overview")}
            {tab("/security/vulnerabilities", "vulns", "Vulnerabilities")}
            {tab("/security/hardening", "hardening", "Hardening")}
            {tab("/security/runtime", "runtime", "Runtime")}
            {tab("/security/settings", "settings", "Settings")}
        </nav>
    }
}

// ------------------------------------------------------------ overview

#[component]
pub fn SecurityOverviewPage() -> impl IntoView {
    let (ov, set_ov) = create_signal(None::<SecurityOverview>);
    let (err, set_err) = create_signal(None::<String>);
    let load = create_action(move |_: &()| async move {
        match get_security_overview().await {
            Ok(o) => {
                set_err.set(None);
                set_ov.set(Some(o))
            }
            Err(e) => set_err.set(Some(format!("Could not load the security overview: {e}"))),
        }
    });
    load.dispatch(());
    view! {
        <div class="security-page">
            <div class="page-header">
                <h1>"Security"</h1>
                <div class="header-actions">
                    <button class="btn btn-secondary" on:click=move |_| load.dispatch(())>"Refresh"</button>
                </div>
            </div>
            <SecurityTabs active="overview"/>
            <p class="help-text">
                "Built-in security tooling for this node: vulnerability scans of container images and containers, "
                "and the host hardening baseline with a compliance report. Everything here is free and open-source tooling and can be turned off in Settings."
            </p>
            {move || err.get().map(|m| view! { <div class="alert alert-error">{m}</div> })}
            {move || match ov.get() {
                None => view! { <div class="loading">"Loading..."</div> }.into_view(),
                Some(o) => {
                    let crit = o.counts.critical;
                    let high = o.counts.high;
                    let unscanned = o.targets.saturating_sub(o.targets_scanned);
                    view! {
                        {(!o.scanner_available).then(|| view! {
                            <div class="alert alert-warning">
                                "The vulnerability scanner (" {o.scanner.clone()} ") is not installed on this node, so images and containers cannot be scanned yet. "
                                <A href="/security/settings">"Scanner settings"</A>
                            </div>
                        })}
                        <div class="stats-grid">
                            <A href="/security/vulnerabilities?severity=critical" class={if crit > 0 { "stat-card stat-link critical attention" } else { "stat-card stat-link" }}>
                                <span class="stat-label">"Critical vulnerabilities"</span>
                                <div class="stat-value">{crit}</div>
                                <div class="dash-meta">"newest scan of each target"</div>
                            </A>
                            <A href="/security/vulnerabilities?severity=high" class={if high > 0 { "stat-card stat-link warning attention" } else { "stat-card stat-link" }}>
                                <span class="stat-label">"High vulnerabilities"</span>
                                <div class="stat-value">{high}</div>
                                <div class="dash-meta">{format!("{} of {} findings have a fix", o.counts.fixable, o.counts.total())}</div>
                            </A>
                            <A href="/security/vulnerabilities" class="stat-card stat-link">
                                <span class="stat-label">"Targets scanned"</span>
                                <div class="stat-value">{format!("{}/{}", o.targets_scanned, o.targets)}</div>
                                <div class="dash-meta">{if unscanned > 0 { format!("{unscanned} never scanned") } else { format!("last scan {}", when(o.last_scan_at)) }}</div>
                            </A>
                            <A href="/security/hardening" class={if o.hardening_failed > 0 { "stat-card stat-link warning attention" } else { "stat-card stat-link healthy" }}>
                                <span class="stat-label">"Hardening score"</span>
                                <div class="stat-value">{format!("{:.0}%", o.hardening_score)}</div>
                                <div class="dash-meta">{format!("{} failing, {} need a manual fix", o.hardening_failed, o.hardening_manual)}</div>
                            </A>
                        </div>
                        <HostCveCard/>
                        <A href="/security/runtime" class={if o.runtime_events_24h > 0 { "stat-card stat-link warning attention" } else { "stat-card stat-link" }}>
                            <span class="stat-label">"Runtime detections (24 h)"</span>
                            <div class="stat-value">{o.runtime_events_24h}</div>
                            <div class="dash-meta">{if o.runtime_running { "container runtime detection is running" } else if o.runtime_enabled { "enabled but not running" } else { "runtime detection is off" }}</div>
                        </A>
                        <div class="card">
                            <h2>"Scanner"</h2>
                            <p>{if o.scanner_available { format!("{} {} is installed.", o.scanner, o.scanner_version) } else { format!("{} is not installed.", o.scanner) }}
                               " Scanning is " {if o.scanning_enabled { "enabled" } else { "turned off" }} " on this node. "
                               {(o.running_scans > 0).then(|| format!("{} scan(s) running.", o.running_scans))}</p>
                        </div>
                    }.into_view()
                }
            }}
        </div>
    }
}

/// Host vulnerabilities: the host's own installed packages against Gentoo
/// security advisories. Loads on its own so the overview never waits for it.
#[component]
fn HostCveCard() -> impl IntoView {
    let (rep, set_rep) = create_signal(None::<HostCveReport>);
    let (err, set_err) = create_signal(None::<String>);
    let (busy, set_busy) = create_signal(false);
    let load = create_action(move |_: &()| async move {
        match get_host_cve().await {
            Ok(r) => {
                set_err.set(None);
                set_rep.set(Some(r))
            }
            Err(e) => set_err.set(Some(format!("Could not load the host check: {e}"))),
        }
    });
    load.dispatch(());
    let recheck = create_action(move |_: &()| async move {
        set_busy.set(true);
        match check_host_cve().await {
            Ok(r) => {
                set_err.set(None);
                set_rep.set(Some(r))
            }
            Err(e) => set_err.set(Some(format!("Check failed: {e}"))),
        }
        set_busy.set(false);
    });
    view! {
        <div class="card host-cve-card" id="host-vulnerabilities">
            <div class="page-header">
                <h2>"Host vulnerabilities"</h2>
                <div class="header-actions">
                    <button class="btn btn-secondary" disabled=move || busy.get() on:click=move |_| recheck.dispatch(())>
                        {move || if busy.get() { "Checking..." } else { "Check now" }}
                    </button>
                </div>
            </div>
            {move || err.get().map(|m| view! { <div class="alert alert-error">{m}</div> })}
            {move || match rep.get() {
                None => view! { <div class="loading">"Loading..."</div> }.into_view(),
                Some(r) if !r.supported => view! {
                    <p class="help-text">{r.reason.clone()} " "
                        <A href="/security/settings">"Host check settings"</A></p>
                }.into_view(),
                Some(r) => {
                    let n = r.advisories.len();
                    let cmd = r.update_command.clone();
                    view! {
                        <p class="dash-meta">
                            {format!("{} installed packages checked against {} Gentoo security advisories ({}), {}. ",
                                r.installed_packages, r.advisories_checked, r.arch, when(Some(r.checked_at)))}
                            {(r.ignored > 0).then(|| format!("{} ignored in Settings.", r.ignored))}
                        </p>
                        {if n == 0 {
                            view! { <p class="alert alert-success">"No installed package is affected by a published advisory."</p> }.into_view()
                        } else {
                            view! {
                                <p class="alert alert-warning">
                                    {format!("{} advisor{} affect{} {} installed package{}.", n,
                                        if n == 1 { "y" } else { "ies" }, if n == 1 { "s" } else { "" },
                                        r.affected_packages, if r.affected_packages == 1 { "" } else { "s" })}
                                </p>
                                <ul class="host-advisories">
                                {r.advisories.iter().map(|a| {
                                    let url = a.url.clone();
                                    let id = a.id.clone();
                                    let sev = a.severity.clone();
                                    let title = a.title.clone();
                                    let cves = if a.cves.is_empty() { "no CVE ids listed".to_string() } else { a.cves.join(", ") };
                                    let pk: Vec<String> = a.packages.iter().map(|p| format!(
                                        "{} {} (fixed in {})", p.package, p.installed,
                                        if p.fixed.is_empty() { "no fixed version published".to_string() } else { p.fixed.join(", ") })).collect();
                                    view! {
                                        <li class="host-advisory">
                                            <div>
                                                <a href=url target="_blank" rel="noopener noreferrer"><strong>{id}</strong></a>
                                                " "
                                                <span class=format!("sev-badge sev-{}", sev)>{sev.clone()}</span>
                                                " " {title}
                                            </div>
                                            <div class="host-advisory-pkgs">{pk.into_iter().map(|n| view! { <div>{n}</div> }).collect_view()}</div>
                                            <div class="dash-meta">{cves}</div>
                                        </li>
                                    }
                                }).collect_view()}
                                </ul>
                                <p>"Update with "<code>{cmd}</code>" (Horcrux never runs it for you), or open "
                                    <A href="/system/packages">"System > Packages"</A>"."</p>
                            }.into_view()
                        }}
                    }.into_view()
                }
            }}
        </div>
    }
}

// ------------------------------------------------------------ vulnerabilities

#[component]
pub fn SecurityVulnsPage() -> impl IntoView {
    let query = use_query_map();
    let sev_filter = move || query.with(|q| q.get("severity").cloned().unwrap_or_default());
    let (targets, set_targets) = create_signal(None::<Vec<ScanTarget>>);
    let (scans, set_scans) = create_signal(Vec::<ScanSummary>::new());
    let (err, set_err) = create_signal(None::<String>);
    let (ok, set_ok) = create_signal(None::<String>);
    let (unavailable, set_unavailable) = create_signal(None::<Unavailable>);
    let (search, set_search) = create_signal(String::new());
    let seq = store_value(0u32);
    let load = create_action(move |_: &()| async move {
        seq.update_value(|n| *n += 1);
        let mine = seq.get_value();
        let t = get_scan_targets().await;
        let s = list_security_scans().await;
        if mine != seq.get_value() {
            return; // a newer load answered first
        }
        match t {
            Ok(t) => set_targets.set(Some(t)),
            Err(e) => set_err.set(Some(format!("Could not list targets: {e}"))),
        }
        if let Ok(s) = s {
            set_scans.set(s);
        }
    });
    load.dispatch(());
    let running = create_memo(move |_| {
        scans.with(|s| {
            s.iter()
                .any(|x| matches!(x.status, ScanStatus::Queued | ScanStatus::Running))
        })
    });
    // Poll while scans run (reads the memo only, so form state survives).
    let poll = set_interval_with_handle(
        move || {
            if running.get_untracked() {
                load.dispatch(());
            }
        },
        std::time::Duration::from_secs(4),
    )
    .ok();
    on_cleanup(move || {
        if let Some(p) = poll {
            p.clear();
        }
    });
    let start = create_action(move |(kind, target): &(Option<ScanTargetKind>, String)| {
        let (kind, target) = (*kind, target.clone());
        async move {
            set_err.set(None);
            set_ok.set(None);
            let r = match kind {
                Some(k) => start_security_scan(k, &target)
                    .await
                    .map(|_| format!("Scan of {target} started.")),
                None => scan_all_targets()
                    .await
                    .map(|v| format!("{} scans queued.", v.len())),
            };
            match r {
                Ok(m) => {
                    set_unavailable.set(None);
                    set_ok.set(Some(m))
                }
                Err(e) => match e.unavailable() {
                    Some(u) => set_unavailable.set(Some(u)),
                    None => set_err.set(Some(format!("Scan failed to start: {e}"))),
                },
            }
            load.dispatch(());
        }
    });
    let remove = create_action(move |id: &String| {
        let id = id.clone();
        async move {
            match delete_security_scan(&id).await {
                Ok(()) => set_scans.update(|v| v.retain(|s| s.id != id)),
                Err(e) => set_err.set(Some(format!("Delete failed: {e}"))),
            }
        }
    });
    let shown = move || {
        let f = sev_filter();
        let q = search.get().to_lowercase();
        targets
            .get()
            .unwrap_or_default()
            .into_iter()
            .filter(move |t| {
                let sev_ok = match f.as_str() {
                    "critical" => t.last_scan.as_ref().is_some_and(|l| l.counts.critical > 0),
                    "high" => t
                        .last_scan
                        .as_ref()
                        .is_some_and(|l| l.counts.at_least(Severity::High) > 0),
                    "unscanned" => t.last_scan.is_none(),
                    _ => true,
                };
                sev_ok && (q.is_empty() || t.name.to_lowercase().contains(&q))
            })
            .collect::<Vec<_>>()
    };
    let filter_link = move |val: &'static str, label: &'static str| {
        let href = if val.is_empty() {
            "/security/vulnerabilities".to_string()
        } else {
            format!("/security/vulnerabilities?severity={val}")
        };
        let on = Signal::derive(move || sev_filter() == val);
        view! { <NavLink href=href class=Signal::derive(move || if on.get() { "filter-link active".to_string() } else { "filter-link".to_string() }) active=on>{label}</NavLink> }
    };
    view! {
        <div class="security-page">
            <div class="page-header">
                <h1>"Vulnerabilities"</h1>
                <div class="header-actions">
                    <button class="btn btn-secondary" on:click=move |_| load.dispatch(())>"Refresh"</button>
                    <button class="btn btn-primary" disabled=move || start.pending().get()
                        on:click=move |_| if ask("Scan every image and container on this node? Scans run one after another at low priority.") { start.dispatch((None, String::new())) }>
                        "Scan all"
                    </button>
                </div>
            </div>
            <SecurityTabs active="vulns"/>
            <p class="help-text">"Container images (Podman, Docker), container root file systems (LXC, Incus) and virtual machines are checked against a vulnerability database. A stopped VM is read from a read-only disk mount, a running VM from its guest agent's package list. A row shows the newest scan; open it for every finding and the fixed versions."</p>
            {move || err.get().map(|m| view! { <div class="alert alert-error">{m}</div> })}
            {move || ok.get().map(|m| view! { <div class="alert alert-success">{m}</div> })}
            {move || unavailable.get().map(|u| view! {
                <crate::components::UnavailableState info=u on_retry=Callback::new(move |_| { set_unavailable.set(None); load.dispatch(()) })/>
            })}
            <div class="toolbar">
                <div class="filter-links">
                    {filter_link("", "All")}
                    {filter_link("critical", "With critical")}
                    {filter_link("high", "High or worse")}
                    {filter_link("unscanned", "Never scanned")}
                </div>
                <label class="sr-only" for="sec-target-search">"Search targets"</label>
                <input id="sec-target-search" type="search" placeholder="Search targets" aria-label="Search targets"
                    prop:value=search on:input=move |e| set_search.set(event_target_value(&e))/>
            </div>
            {move || match targets.get() {
                None => view! { <div class="loading">"Loading..."</div> }.into_view(),
                Some(all) if all.is_empty() => view! {
                    <div class="empty-state">
                        "No container images or containers on this node yet. "
                        <A href="/containers/images">"Pull an image"</A>" or "<A href="/containers/create">"create a container"</A>", then scan it here."
                    </div>
                }.into_view(),
                Some(_) => {
                    let rows = shown();
                    if rows.is_empty() {
                        return view! { <div class="empty-state">"No target matches this filter. "<A href="/security/vulnerabilities">"Show all"</A></div> }.into_view();
                    }
                    view! {
                        <div class="table-container">
                        <table class="data-table">
                            <thead><tr><th>"Target"</th><th>"Type"</th><th>"Newest scan"</th><th>"Findings"</th><th>"Actions"</th></tr></thead>
                            <tbody>
                            {rows.into_iter().map(|t| {
                                let kind = t.kind;
                                let name = t.name.clone();
                                let label = format!("Scan {}", t.name);
                                let last = t.last_scan.clone();
                                view! {
                                    <tr>
                                        <td><span class="mono">{t.name.clone()}</span>{(!t.detail.is_empty()).then(|| view! { <div class="dash-meta">{t.detail.clone()}</div> })}</td>
                                        <td>{t.kind.label()}</td>
                                        <td>{match last.clone() {
                                            None => view! { <span class="dash-meta">"never"</span> }.into_view(),
                                            Some(l) => view! { <A href=format!("/security/vulnerabilities/{}", l.id)>{when(l.finished.or(Some(l.created)))}</A> }.into_view(),
                                        }}</td>
                                        <td>{match last {
                                            None => view! { <span class="dash-meta">"-"</span> }.into_view(),
                                            Some(l) if l.status == ScanStatus::Failed => view! { <span class="sev-badge sev-high" title=l.error.clone().unwrap_or_default()>"scan failed"</span> }.into_view(),
                                            Some(l) => view! { <CountBadges counts=l.counts/> }.into_view(),
                                        }}</td>
                                        <td class="actions">
                                            <button class="btn btn-xs btn-primary" aria-label=label.clone() title=label
                                                on:click=move |_| start.dispatch((Some(kind), name.clone()))>"Scan"</button>
                                        </td>
                                    </tr>
                                }
                            }).collect_view()}
                            </tbody>
                        </table>
                        </div>
                    }.into_view()
                }
            }}
            <h2>"Recent scans"</h2>
            {move || {
                let list = scans.get();
                if list.is_empty() {
                    return view! { <div class="empty-state">"No scans yet. Use "<strong>"Scan"</strong>" on a target or "<strong>"Scan all"</strong>"."</div> }.into_view();
                }
                view! {
                    <div class="table-container">
                    <table class="data-table">
                        <thead><tr><th>"Started"</th><th>"Target"</th><th>"Status"</th><th>"Findings"</th><th>"By"</th><th>"Actions"</th></tr></thead>
                        <tbody>
                        {list.into_iter().take(50).map(|s| {
                            let id = s.id.clone();
                            let busy = matches!(s.status, ScanStatus::Queued | ScanStatus::Running);
                            let del = format!("Delete scan of {}", s.target);
                            view! {
                                <tr>
                                    <td><A href=format!("/security/vulnerabilities/{}", s.id)>{when(Some(s.created))}</A></td>
                                    <td><span class="mono">{s.target.clone()}</span></td>
                                    <td>{format!("{:?}", s.status).to_lowercase()}{s.error.clone().map(|e| view! { <div class="dash-meta">{e}</div> })}</td>
                                    <td>{if s.status == ScanStatus::Done { view! { <CountBadges counts=s.counts.clone()/> }.into_view() } else { view! { <span class="dash-meta">"-"</span> }.into_view() }}</td>
                                    <td>{s.requested_by.clone()}</td>
                                    <td class="actions">
                                        <button class="btn btn-xs btn-danger" disabled=busy aria-label=del.clone() title=del
                                            on:click=move |_| if ask("Delete this scan result?") { remove.dispatch(id.clone()) }>"Delete"</button>
                                    </td>
                                </tr>
                            }
                        }).collect_view()}
                        </tbody>
                    </table>
                    </div>
                }.into_view()
            }}
        </div>
    }
}

#[component]
pub fn SecurityScanPage() -> impl IntoView {
    let params = use_params_map();
    let id = move || params.with(|p| p.get("id").cloned().unwrap_or_default());
    let (scan, set_scan) = create_signal(None::<ScanResult>);
    let (err, set_err) = create_signal(None::<String>);
    let (min_sev, set_min_sev) = create_signal("all".to_string());
    let (only_fix, set_only_fix) = create_signal(false);
    let (search, set_search) = create_signal(String::new());
    let load = create_action(move |id: &String| {
        let id = id.clone();
        async move {
            match get_security_scan(&id).await {
                Ok(s) => set_scan.set(Some(s)),
                Err(e) => set_err.set(Some(format!("Could not load the scan: {e}"))),
            }
        }
    });
    create_effect(move |_| load.dispatch(id()));
    view! {
        <div class="security-page">
            <div class="page-header">
                <h1>{move || scan.get().map(|s| format!("Scan: {}", s.summary.target)).unwrap_or_else(|| "Scan".into())}</h1>
                <div class="header-actions">
                    <A href="/security/vulnerabilities" class="btn btn-secondary">"All targets"</A>
                    <button class="btn btn-secondary" on:click=move |_| load.dispatch(id())>"Refresh"</button>
                </div>
            </div>
            <SecurityTabs active="vulns"/>
            {move || err.get().map(|m| view! { <div class="alert alert-error">{m}</div> })}
            {move || match scan.get() {
                None => view! { <div class="loading">"Loading..."</div> }.into_view(),
                Some(s) => {
                    let sum = s.summary.clone();
                    view! {
                        <div class="card">
                            <p>
                                {sum.kind.map(|k| k.label()).unwrap_or("Target")} " " <span class="mono">{sum.target.clone()}</span>
                                ", scanned with " {sum.scanner.clone()} " " {sum.scanner_version.clone()}
                                " (" {format!("{:?}", sum.status).to_lowercase()} ", " {when(sum.finished.or(Some(sum.created)))} ")."
                            </p>
                            {sum.error.clone().map(|e| view! { <div class="alert alert-error">{e}</div> })}
                            {(sum.status == ScanStatus::Done).then(|| view! { <p><CountBadges counts=sum.counts.clone()/>" " {format!("{} of {} {} a fixed version.", sum.counts.fixable, sum.counts.total(), if sum.counts.fixable == 1 { "has" } else { "have" })}</p> })}
                        </div>
                        <div class="toolbar">
                            <div class="form-group inline">
                                <label for="sec-min-sev">"Severity"</label>
                                <select id="sec-min-sev" on:change=move |e| set_min_sev.set(event_target_value(&e))>
                                    <option value="all">"All"</option>
                                    <option value="critical">"Critical"</option>
                                    <option value="high">"High or worse"</option>
                                    <option value="medium">"Medium or worse"</option>
                                </select>
                            </div>
                            <div class="form-group inline">
                                <label for="sec-only-fix"><input id="sec-only-fix" type="checkbox" on:change=move |e| set_only_fix.set(event_target_checked(&e))/>" Only with a fix"</label>
                            </div>
                            <label class="sr-only" for="sec-find-search">"Search findings"</label>
                            <input id="sec-find-search" type="search" placeholder="CVE or package" aria-label="Search findings"
                                prop:value=search on:input=move |e| set_search.set(event_target_value(&e))/>
                        </div>
                        {move || {
                            let min = match min_sev.get().as_str() {
                                "critical" => Severity::Critical,
                                "high" => Severity::High,
                                "medium" => Severity::Medium,
                                _ => Severity::Unknown,
                            };
                            let q = search.get().to_lowercase();
                            let fix = only_fix.get();
                            let rows: Vec<_> = scan.get().map(|s| s.findings).unwrap_or_default().into_iter()
                                .filter(|f| f.severity >= min)
                                .filter(|f| !fix || !f.fixed_in.is_empty())
                                .filter(|f| q.is_empty() || f.id.to_lowercase().contains(&q) || f.package.to_lowercase().contains(&q))
                                .collect();
                            if rows.is_empty() {
                                return view! { <div class="empty-state">"No findings match."</div> }.into_view();
                            }
                            let n = rows.len();
                            view! {
                                <div class="table-container">
                                <table class="data-table">
                                    <thead><tr><th>"Severity"</th><th>"Vulnerability"</th><th>"Package"</th><th>"Installed"</th><th>"Fixed in"</th></tr></thead>
                                    <tbody>
                                    {rows.into_iter().take(1000).map(|f| view! {
                                        <tr>
                                            <td><SevBadge sev=f.severity/></td>
                                            <td>{if f.url.starts_with("https://") || f.url.starts_with("http://") {
                                                    view! { <a href=f.url.clone() target="_blank" rel="noopener noreferrer">{f.id.clone()}</a> }.into_view()
                                                } else { f.id.clone().into_view() }}
                                                {(!f.title.is_empty()).then(|| view! { <div class="dash-meta">{f.title.clone()}</div> })}</td>
                                            <td class="nowrap"><span class="mono">{f.package.clone()}</span><div class="dash-meta">{f.pkg_type.clone()}</div></td>
                                            <td class="mono nowrap">{f.installed.clone()}</td>
                                            <td class="nowrap">{if f.fixed_in.is_empty() {
                                                view! { <span class="dash-meta">"no fix yet"</span> }.into_view()
                                            } else {
                                                view! { <span class="mono">{f.fixed_in.join(", ")}</span> }.into_view()
                                            }}</td>
                                        </tr>
                                    }).collect_view()}
                                    </tbody>
                                </table>
                                </div>
                                <p class="dash-meta">{format!("{n} findings shown")}</p>
                            }.into_view()
                        }}
                    }.into_view()
                }
            }}
        </div>
    }
}

// ------------------------------------------------------------ hardening

#[component]
pub fn SecurityHardeningPage() -> impl IntoView {
    let query = use_query_map();
    let status_filter = move || query.with(|q| q.get("status").cloned().unwrap_or_default());
    let (rep, set_rep) = create_signal(None::<HardeningReport>);
    let (err, set_err) = create_signal(None::<String>);
    let (ok, set_ok) = create_signal(None::<String>);
    let (busy, set_busy) = create_signal(false);
    let load = create_action(move |_: &()| async move {
        match get_hardening().await {
            Ok(r) => set_rep.set(Some(r)),
            Err(e) => set_err.set(Some(format!("Could not check the baseline: {e}"))),
        }
    });
    load.dispatch(());
    let act = create_action(move |(revert, rules): &(bool, Vec<String>)| {
        let (revert, rules) = (*revert, rules.clone());
        async move {
            set_busy.set(true);
            set_err.set(None);
            set_ok.set(None);
            // Preview first, then ask.
            let preview = if revert {
                hardening_revert(rules.clone(), true).await
            } else {
                hardening_apply(rules.clone(), true).await
            };
            match preview {
                Err(e) => set_err.set(Some(format!("Preview failed: {e}"))),
                Ok(p) if p.changes.is_empty() => {
                    let why: Vec<String> = p
                        .skipped
                        .iter()
                        .map(|s| format!("{}: {}", s.rule, s.error.clone().unwrap_or_default()))
                        .collect();
                    set_ok.set(Some(if why.is_empty() {
                        "Nothing to change.".into()
                    } else {
                        format!("Nothing to change ({}).", why.join("; "))
                    }));
                }
                Ok(p) => {
                    let list: Vec<String> = p.changes.iter().map(|c| c.change.clone()).collect();
                    let msg = format!(
                        "{} these {} change(s) on this node?\n\n{}\n\nKernel settings take effect now and are kept across reboots; every change can be reverted.",
                        if revert { "Revert" } else { "Apply" }, list.len(), list.join("\n")
                    );
                    if ask(&msg) {
                        let r = if revert {
                            hardening_revert(rules, false).await
                        } else {
                            hardening_apply(rules, false).await
                        };
                        match r {
                            Ok(r) => {
                                let bad: Vec<String> = r
                                    .changes
                                    .iter()
                                    .filter(|c| !c.ok)
                                    .map(|c| {
                                        format!(
                                            "{}: {}",
                                            c.change,
                                            c.error.clone().unwrap_or_default()
                                        )
                                    })
                                    .collect();
                                if bad.is_empty() {
                                    set_ok
                                        .set(Some(format!("{} change(s) done.", r.changes.len())));
                                } else {
                                    set_err.set(Some(format!(
                                        "Some changes failed: {}",
                                        bad.join("; ")
                                    )));
                                }
                            }
                            Err(e) => set_err.set(Some(format!("Failed: {e}"))),
                        }
                    }
                }
            }
            set_busy.set(false);
            load.dispatch(());
        }
    });
    let report = create_action(move |fmt: &&'static str| {
        let fmt = *fmt;
        async move {
            match hardening_report_text(fmt).await {
                Ok(t) => {
                    let node = rep
                        .get_untracked()
                        .map(|r| r.node)
                        .unwrap_or_else(|| "node".into());
                    let (ext, mime) = match fmt {
                        "html" => ("html", "text/html"),
                        "json" => ("json", "application/json"),
                        _ => ("txt", "text/plain"),
                    };
                    download(&format!("hardening-{node}.{ext}"), mime, &t);
                }
                Err(e) => set_err.set(Some(format!("Report failed: {e}"))),
            }
        }
    });
    let filter_link = move |val: &'static str, label: &'static str| {
        let href = if val.is_empty() {
            "/security/hardening".to_string()
        } else {
            format!("/security/hardening?status={val}")
        };
        let on = Signal::derive(move || status_filter() == val);
        view! { <NavLink href=href class=Signal::derive(move || if on.get() { "filter-link active".to_string() } else { "filter-link".to_string() }) active=on>{label}</NavLink> }
    };
    view! {
        <div class="security-page">
            <div class="page-header">
                <h1>"Hardening"</h1>
                <div class="header-actions">
                    <button class="btn btn-secondary" disabled=busy on:click=move |_| load.dispatch(())>"Check again"</button>
                    <button class="btn btn-secondary" on:click=move |_| report.dispatch("html")>"Report (HTML)"</button>
                    <button class="btn btn-secondary" on:click=move |_| report.dispatch("text")>"Report (text)"</button>
                    <button class="btn btn-primary" disabled=move || busy.get() || rep.get().map(|r| !r.rules.iter().any(|x| x.status == RuleStatus::Fail && x.fix == horcrux_common::security::FixKind::Auto)).unwrap_or(true)
                        on:click=move |_| act.dispatch((false, vec![]))>"Apply baseline"</button>
                </div>
            </div>
            <SecurityTabs active="hardening"/>
            <p class="help-text">
                "The Horcrux baseline adapts the CIS Linux benchmark to a hypervisor: it never turns off IP forwarding, strict reverse-path filtering or user namespaces, which guests and containers need. "
                "Apply changes only kernel settings and file modes, shows every change first, and Revert undoes them. SSH, account and mount findings tell you what to do; Horcrux never changes those itself."
            </p>
            {move || err.get().map(|m| view! { <div class="alert alert-error">{m}</div> })}
            {move || ok.get().map(|m| view! { <div class="alert alert-success">{m}</div> })}
            {move || match rep.get() {
                None => view! { <div class="loading">"Checking..."</div> }.into_view(),
                Some(r) => {
                    let any_applied = r.rules.iter().any(|x| x.applied);
                    let f = status_filter();
                    let rows: Vec<HardeningRuleResult> = r.rules.iter().filter(|x| match f.as_str() {
                        "fail" => x.status == RuleStatus::Fail,
                        "manual" => x.status == RuleStatus::Manual,
                        "pass" => x.status == RuleStatus::Pass,
                        "applied" => x.applied,
                        _ => true,
                    }).cloned().collect();
                    view! {
                        <div class="stats-grid">
                            <A href="/security/hardening" class="stat-card stat-link"><span class="stat-label">"Score"</span><div class="stat-value">{format!("{:.0}%", r.score)}</div><div class="dash-meta">{format!("baseline {}", r.baseline)}</div></A>
                            <A href="/security/hardening?status=fail" class={if r.failed > 0 { "stat-card stat-link critical attention" } else { "stat-card stat-link" }}><span class="stat-label">"Failing"</span><div class="stat-value">{r.failed}</div></A>
                            <A href="/security/hardening?status=manual" class={if r.manual > 0 { "stat-card stat-link warning attention" } else { "stat-card stat-link" }}><span class="stat-label">"Manual fix"</span><div class="stat-value">{r.manual}</div></A>
                            <A href="/security/hardening?status=pass" class="stat-card stat-link"><span class="stat-label">"Passing"</span><div class="stat-value">{r.passed}</div><div class="dash-meta">{format!("{} not applicable, {} excluded", r.not_applicable, r.excluded)}</div></A>
                        </div>
                        <div class="toolbar">
                            <div class="filter-links">
                                {filter_link("", "All")}
                                {filter_link("fail", "Failing")}
                                {filter_link("manual", "Manual")}
                                {filter_link("pass", "Passing")}
                                {filter_link("applied", "Applied by Horcrux")}
                            </div>
                            {any_applied.then(|| view! {
                                <button class="btn btn-warning" disabled=busy on:click=move |_| act.dispatch((true, vec![]))>"Revert all"</button>
                            })}
                        </div>
                        <div class="table-container">
                        <table class="data-table hardening-table">
                            <thead><tr><th>"Status"</th><th>"Rule"</th><th>"Actions"</th></tr></thead>
                            <tbody>
                            {rows.into_iter().map(|x| {
                                let st = format!("{:?}", x.status).to_lowercase();
                                let id = x.id.clone();
                                let id2 = x.id.clone();
                                let can_apply = x.status == RuleStatus::Fail && x.fix == horcrux_common::security::FixKind::Auto;
                                let applied = x.applied;
                                let la = format!("Apply {}", x.id);
                                let lr = format!("Revert {}", x.id);
                                view! {
                                    <tr class=format!("rule-{st}")>
                                        <td><span class=format!("rule-status rule-{st}")>{st.clone()}</span><div><SevBadge sev=x.severity/></div></td>
                                        <td><strong>{x.title.clone()}</strong>
                                            <div class="dash-meta"><span class="mono">{x.id.clone()}</span>" · "{x.category.clone()}" · "{x.reference.clone()}</div>
                                            <div class="dash-meta">{x.rationale.clone()}</div>
                                            <div class="rule-now">"Now: "<span class="mono">{x.current.clone()}</span>
                                                {(!x.expected.is_empty() && x.status != RuleStatus::Pass).then(|| view! { <span>" · wanted: "<span class="mono">{x.expected.clone()}</span></span> })}</div>
                                            {matches!(x.status, RuleStatus::Fail | RuleStatus::Manual).then(|| view! { <div class="rule-fix"><strong>"Fix: "</strong>{x.remediation.clone()}</div> })}</td>
                                        <td class="actions">
                                            {can_apply.then(|| view! { <button class="btn btn-xs btn-primary" disabled=busy aria-label=la.clone() title=la on:click=move |_| act.dispatch((false, vec![id.clone()]))>"Apply"</button> })}
                                            {applied.then(|| view! { <button class="btn btn-xs" disabled=busy aria-label=lr.clone() title=lr on:click=move |_| act.dispatch((true, vec![id2.clone()]))>"Revert"</button> })}
                                        </td>
                                    </tr>
                                }
                            }).collect_view()}
                            </tbody>
                        </table>
                        </div>
                        <p class="dash-meta">{format!("Checked {}.", when(Some(r.checked_at)))}</p>
                    }.into_view()
                }
            }}
        </div>
    }
}

// ------------------------------------------------------------ settings

#[component]
pub fn SecurityRuntimePage() -> impl IntoView {
    let query = use_query_map();
    let prio = move || query.with(|q| q.get("priority").cloned().unwrap_or_default());
    let rule_f = move || query.with(|q| q.get("rule").cloned().unwrap_or_default());
    let cont_f = move || query.with(|q| q.get("container").cloned().unwrap_or_default());
    let (st, set_st) = create_signal(None::<RuntimeStatus>);
    let (events, set_events) = create_signal(None::<Vec<RuntimeEvent>>);
    let (err, set_err) = create_signal(None::<String>);
    let (settings, set_settings) = create_signal(None::<SecuritySettings>);
    let (ver, bump) = create_signal(0u32);
    let load = create_action(move |_: &()| async move {
        match get_runtime_status().await {
            Ok(s) => {
                set_err.set(None);
                set_st.set(Some(s))
            }
            Err(e) => set_err.set(Some(format!("Could not load the detector status: {e}"))),
        }
        match list_runtime_events(&prio(), &rule_f(), &cont_f()).await {
            Ok(l) => set_events.set(Some(l)),
            Err(e) => set_err.set(Some(format!("Could not load detections: {e}"))),
        }
        if let Ok(s) = get_security_settings().await {
            set_settings.set(Some(s));
        }
    });
    load.dispatch(());
    create_effect(move |_| {
        let _ = (ver.get(), prio(), rule_f(), cont_f());
        load.dispatch(());
    });
    let toggle = create_action(move |on: &bool| {
        let on = *on;
        async move {
            if let Some(mut s) = settings.get_untracked() {
                s.runtime.enabled = on;
                match put_security_settings(&s).await {
                    Ok(v) => set_settings.set(Some(v)),
                    Err(e) => set_err.set(Some(format!("Not saved: {e}"))),
                }
            }
            bump.update(|v| *v += 1);
        }
    });
    let clear = create_action(move |_: &()| async move {
        if let Err(e) = clear_runtime_events().await {
            set_err.set(Some(format!("Could not clear detections: {e}")));
        }
        bump.update(|v| *v += 1);
    });
    let filter_link = move |val: &'static str, label: &'static str| {
        let href = if val.is_empty() {
            "/security/runtime".to_string()
        } else {
            format!("/security/runtime?priority={val}")
        };
        let on = Signal::derive(move || prio() == val);
        view! { <NavLink href=href class=Signal::derive(move || if on.get() { "filter-link active".to_string() } else { "filter-link".to_string() }) active=on>{label}</NavLink> }
    };
    view! {
        <div class="security-page">
            <div class="page-header">
                <h1>"Runtime detection"</h1>
                <div class="header-actions">
                    <button class="btn btn-secondary" on:click=move |_| bump.update(|v| *v += 1)>"Refresh"</button>
                </div>
            </div>
            <SecurityTabs active="runtime"/>
            <p class="help-text">
                "Watches the containers on this node for suspicious behaviour with Falco (Apache-2.0, eBPF, no kernel module): an interactive shell in a container, writes below /etc, "
                "a privileged container starting, crypto-miner patterns. Off by default; it runs resource-limited and keeps its detections here and in Notifications."
            </p>
            {move || err.get().map(|m| view! { <div class="alert alert-error">{m}</div> })}
            {move || st.get().map(|s| {
                let on = s.enabled;
                view! {
                    {(!s.available).then(|| view! {
                        <div class="alert alert-warning">
                            "Falco is not installed on this node. Install it with the " <span class="mono">"runtime-detection"</span> " USE flag of Horcrux (app-containers/falco-bin), or set its path in "
                            <A href="/security/settings">"Security settings"</A> "."
                        </div>
                    })}
                    {s.last_error.clone().filter(|_| s.available).map(|m| view! { <div class="alert alert-warning">{m}</div> })}
                    <div class="stats-grid">
                        <A href="/security/settings" class={if s.running { "stat-card stat-link healthy" } else { "stat-card stat-link" }}>
                            <span class="stat-label">"Detector"</span>
                            <div class="stat-value">{if s.running { "running" } else if on { "stopped" } else { "off" }}</div>
                            <div class="dash-meta">{if s.version.is_empty() { String::new() } else { format!("Falco {}", s.version) }} {s.since.map(|t| format!(" since {}", when(Some(t))))}</div>
                        </A>
                        <A href="/security/settings" class="stat-card stat-link">
                            <span class="stat-label">"Detector CPU"</span>
                            <div class="stat-value">{format!("{:.0}%", s.cpu_percent)}</div>
                            <div class="dash-meta">{format!("{:.0} MB resident, {}", s.memory_mb, if s.limits_applied { "limits applied" } else { "no cgroup limits" })}</div>
                        </A>
                        <A href="/security/runtime" class="stat-card stat-link">
                            <span class="stat-label">"Kernel events"</span>
                            <div class="stat-value">{format!("{:.0}/s", s.events_per_sec)}</div>
                            <div class={if s.kernel_drops > 0 { "dash-meta critical" } else { "dash-meta" }}>{format!("{} dropped", s.kernel_drops)}</div>
                        </A>
                        <A href="/security/runtime?priority=warning" class="stat-card stat-link">
                            <span class="stat-label">"Detections kept"</span>
                            <div class="stat-value">{s.events_kept}</div>
                            <div class="dash-meta">{format!("{} since the log began", s.events_total)}</div>
                        </A>
                    </div>
                    <div class="card">
                        <label for="rt-enabled"><input id="rt-enabled" type="checkbox" prop:checked=on disabled=move || toggle.pending().get()
                            on:change=move |e| toggle.dispatch(event_target_checked(&e))/>" Detect threats in containers on this node"</label>
                        <div class="dash-meta">{format!("Rules: {}", s.rules.join(", "))}</div>
                    </div>
                }
            })}
            <h2>"Detections"</h2>
            <div class="filter-links">
                {filter_link("", "All")}
                {filter_link("warning", "Warning or worse")}
                {filter_link("error", "Error or worse")}
                {filter_link("critical", "Critical")}
                {move || (!rule_f().is_empty() || !cont_f().is_empty()).then(|| view! { <A href="/security/runtime" class="filter-link">"Clear rule/container filter"</A> })}
                <button class="btn btn-secondary btn-sm" on:click=move |_| { if ask("Delete all kept detections on this node?") { clear.dispatch(()) } }>"Clear all"</button>
            </div>
            {move || match events.get() {
                None => view! { <div class="loading">"Loading..."</div> }.into_view(),
                Some(list) if list.is_empty() => view! {
                    <div class="empty-state">
                        <h3>"No detections"</h3>
                        <p>{if st.get().map(|s| s.running).unwrap_or(false) { "The detector is running and has seen nothing suspicious." } else { "Turn the detector on above to start watching containers." }}</p>
                    </div>
                }.into_view(),
                Some(list) => view! {
                    <div class="table-container">
                    <table class="data-table">
                        <thead><tr><th>"Time"</th><th>"Priority"</th><th>"Rule"</th><th>"Container"</th><th>"Detail"</th></tr></thead>
                        <tbody>
                        {list.into_iter().map(|e| {
                            let cls = match e.priority.as_str() { "critical" => "sev-badge sev-critical", "error" => "sev-badge sev-high", "warning" => "sev-badge sev-medium", _ => "sev-badge sev-low" };
                            let rule_href = format!("/security/runtime?rule={}", js_sys::encode_uri_component(&e.rule));
                            let cont_href = format!("/security/runtime?container={}", js_sys::encode_uri_component(&e.container));
                            view! {
                                <tr>
                                    <td>{when(Some(e.time))}</td>
                                    <td><span class=cls>{e.priority.clone()}</span></td>
                                    <td><A href=rule_href>{e.rule.clone()}</A></td>
                                    <td>{if e.container.is_empty() || e.container == "host" { view! { <span class="dash-meta">"host"</span> }.into_view() } else { view! { <A href=cont_href>{e.container.clone()}</A> }.into_view() }}
                                        <div class="dash-meta">{e.image.clone()}</div></td>
                                    <td><span class="mono">{e.cmdline.clone()}</span><div class="dash-meta">{e.message.clone()}</div></td>
                                </tr>
                            }
                        }).collect_view()}
                        </tbody>
                    </table>
                    </div>
                }.into_view(),
            }}
        </div>
    }
}

#[component]
pub fn SecuritySettingsPage() -> impl IntoView {
    let (s, set_s) = create_signal(None::<SecuritySettings>);
    let (err, set_err) = create_signal(None::<String>);
    let (ok, set_ok) = create_signal(None::<String>);
    let load = create_action(move |_: &()| async move {
        match get_security_settings().await {
            Ok(v) => set_s.set(Some(v)),
            Err(e) => set_err.set(Some(format!("Could not load settings: {e}"))),
        }
    });
    load.dispatch(());
    let save = create_action(move |v: &SecuritySettings| {
        let v = v.clone();
        async move {
            set_err.set(None);
            set_ok.set(None);
            match put_security_settings(&v).await {
                Ok(v) => {
                    set_s.set(Some(v));
                    set_ok.set(Some("Saved.".into()))
                }
                Err(e) => set_err.set(Some(format!("Not saved: {e}"))),
            }
        }
    });
    let upd = move |f: Box<dyn Fn(&mut SecuritySettings)>| {
        set_s.update(|o| {
            if let Some(x) = o.as_mut() {
                f(x)
            }
        })
    };
    let num = |v: String| v.trim().parse::<u32>().unwrap_or(0);
    view! {
        <div class="security-page">
            <div class="page-header"><h1>"Security Settings"</h1></div>
            <SecurityTabs active="settings"/>
            <p class="help-text">"These settings are also part of the declarative configuration (section "<span class="mono">"security:"</span>")."</p>
            {move || err.get().map(|m| view! { <div class="alert alert-error">{m}</div> })}
            {move || ok.get().map(|m| view! { <div class="alert alert-success">{m}</div> })}
            {move || s.get().map(|cur| {
                let v = cur.vuln_scan.clone();
                let h = cur.hardening.clone();
                let hc = cur.host_cve.clone();
                let r = cur.runtime.clone();
                view! {
                    <form class="card settings-form" on:submit=move |e| { e.prevent_default(); if let Some(x) = s.get_untracked() { save.dispatch(x) } }>
                        <h2>"Vulnerability scanning"</h2>
                        <div class="form-group">
                            <label for="sec-enabled"><input id="sec-enabled" type="checkbox" prop:checked=v.enabled
                                on:change=move |e| { let c = event_target_checked(&e); upd(Box::new(move |x| x.vuln_scan.enabled = c)) }/>" Scanning on for this node"</label>
                        </div>
                        <div class="form-row">
                            <div class="form-group">
                                <label for="sec-scanner">"Scanner"</label>
                                <select id="sec-scanner" on:change=move |e| { let c = event_target_value(&e); upd(Box::new(move |x| x.vuln_scan.scanner = c.clone())) }>
                                    <option value="grype" selected=v.scanner == "grype">"Grype (Anchore, Apache-2.0)"</option>
                                    <option value="trivy" selected=v.scanner == "trivy">"Trivy (Aqua, Apache-2.0)"</option>
                                </select>
                            </div>
                            <div class="form-group">
                                <label for="sec-path">"Scanner program"</label>
                                <input id="sec-path" type="text" placeholder="looked up in PATH" prop:value=v.scanner_path.clone()
                                    on:change=move |e| { let c = event_target_value(&e); upd(Box::new(move |x| x.vuln_scan.scanner_path = c.clone())) }/>
                                <small class="form-help">"Empty: the scanner's name in PATH."</small>
                            </div>
                        </div>
                        <div class="form-row">
                            <div class="form-group">
                                <label for="sec-db-url">"Database mirror"</label>
                                <input id="sec-db-url" type="text" placeholder="scanner default" prop:value=v.db_url.clone()
                                    on:change=move |e| { let c = event_target_value(&e); upd(Box::new(move |x| x.vuln_scan.db_url = c.clone())) }/>
                                <small class="form-help">"For nodes without internet: your own copy of the vulnerability database."</small>
                            </div>
                            <div class="form-group">
                                <label for="sec-db-update"><input id="sec-db-update" type="checkbox" prop:checked=v.update_db
                                    on:change=move |e| { let c = event_target_checked(&e); upd(Box::new(move |x| x.vuln_scan.update_db = c)) }/>" Update the database before scanning"</label>
                            </div>
                        </div>
                        <div class="form-row">
                            <div class="form-group">
                                <label for="sec-sched">"Scan every (hours)"</label>
                                <input id="sec-sched" type="number" min="0" prop:value=v.schedule_hours.to_string()
                                    on:change=move |e| { let c = num(event_target_value(&e)); upd(Box::new(move |x| x.vuln_scan.schedule_hours = c)) }/>
                                <small class="form-help">"0 = only when you start a scan."</small>
                            </div>
                            <div class="form-group">
                                <label for="sec-conc">"Scans at once"</label>
                                <input id="sec-conc" type="number" min="1" max="16" prop:value=v.max_concurrent.to_string()
                                    on:change=move |e| { let c = num(event_target_value(&e)); upd(Box::new(move |x| x.vuln_scan.max_concurrent = c)) }/>
                            </div>
                            <div class="form-group">
                                <label for="sec-nice">"CPU niceness (0-19)"</label>
                                <input id="sec-nice" type="number" min="0" max="19" prop:value=v.nice.to_string()
                                    on:change=move |e| { let c = num(event_target_value(&e)).min(255) as u8; upd(Box::new(move |x| x.vuln_scan.nice = c)) }/>
                            </div>
                            <div class="form-group">
                                <label for="sec-timeout">"Time limit per scan (s)"</label>
                                <input id="sec-timeout" type="number" min="30" prop:value=v.timeout_secs.to_string()
                                    on:change=move |e| { let c = num(event_target_value(&e)); upd(Box::new(move |x| x.vuln_scan.timeout_secs = c)) }/>
                            </div>
                        </div>
                        <div class="form-row">
                            <div class="form-group">
                                <label for="sec-notify">"Notify at or above"</label>
                                <select id="sec-notify" on:change=move |e| { let c = event_target_value(&e); upd(Box::new(move |x| x.vuln_scan.notify_min_severity = c.clone())) }>
                                    {["none", "critical", "high", "medium", "low"].into_iter().map(|o| view! {
                                        <option value=o selected=v.notify_min_severity == o>{o}</option>
                                    }).collect_view()}
                                </select>
                                <small class="form-help">"Sends a 'security' notification through Notifications."</small>
                            </div>
                            <div class="form-group">
                                <label for="sec-keep">"Scans kept"</label>
                                <input id="sec-keep" type="number" min="1" prop:value=v.keep_scans.to_string()
                                    on:change=move |e| { let c = num(event_target_value(&e)); upd(Box::new(move |x| x.vuln_scan.keep_scans = c)) }/>
                            </div>
                            <div class="form-group">
                                <label for="sec-onlyfix"><input id="sec-onlyfix" type="checkbox" prop:checked=v.only_fixed
                                    on:change=move |e| { let c = event_target_checked(&e); upd(Box::new(move |x| x.vuln_scan.only_fixed = c)) }/>" Only findings with a fix"</label>
                            </div>
                        </div>
                        <div class="form-row">
                            <div class="form-group">
                                <label for="sec-lxc">"LXC container directory"</label>
                                <input id="sec-lxc" type="text" prop:value=v.lxc_path.clone()
                                    on:change=move |e| { let c = event_target_value(&e); upd(Box::new(move |x| x.vuln_scan.lxc_path = c.clone())) }/>
                            </div>
                            <div class="form-group">
                                <label for="sec-incus">"Incus data directory"</label>
                                <input id="sec-incus" type="text" prop:value=v.incus_path.clone()
                                    on:change=move |e| { let c = event_target_value(&e); upd(Box::new(move |x| x.vuln_scan.incus_path = c.clone())) }/>
                            </div>
                        </div>
                        <h2>"Hardening baseline"</h2>
                        <div class="form-row">
                            <div class="form-group">
                                <label for="sec-h-sched">"Check every (hours)"</label>
                                <input id="sec-h-sched" type="number" min="0" prop:value=h.schedule_hours.to_string()
                                    on:change=move |e| { let c = num(event_target_value(&e)); upd(Box::new(move |x| x.hardening.schedule_hours = c)) }/>
                            </div>
                            <div class="form-group">
                                <label for="sec-h-notify"><input id="sec-h-notify" type="checkbox" prop:checked=h.notify_on_regression
                                    on:change=move |e| { let c = event_target_checked(&e); upd(Box::new(move |x| x.hardening.notify_on_regression = c)) }/>" Notify when a rule starts failing"</label>
                            </div>
                        </div>
                        <div class="form-group">
                            <label for="sec-h-exclude">"Excluded rules"</label>
                            <input id="sec-h-exclude" type="text" placeholder="rule ids, comma separated" prop:value=h.exclude.join(", ")
                                on:change=move |e| { let c: Vec<String> = event_target_value(&e).split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect(); upd(Box::new(move |x| x.hardening.exclude = c.clone())) }/>
                            <small class="form-help">"Excluded rules are listed but never checked or applied (for example kernel-ptrace-scope on a debugging host)."</small>
                        </div>
                        <div class="form-group">
                            <label for="sec-h-file">"Kernel settings file"</label>
                            <input id="sec-h-file" type="text" prop:value=h.sysctl_file.clone()
                                on:change=move |e| { let c = event_target_value(&e); upd(Box::new(move |x| x.hardening.sysctl_file = c.clone())) }/>
                            <small class="form-help">"Read at boot by OpenRC's sysctl service and systemd-sysctl."</small>
                        </div>
                        <h2>"Host vulnerabilities (Gentoo advisories)"</h2>
                        <div class="form-row">
                            <div class="form-group">
                                <label for="sec-c-en"><input id="sec-c-en" type="checkbox" prop:checked=hc.enabled
                                    on:change=move |e| { let c = event_target_checked(&e); upd(Box::new(move |x| x.host_cve.enabled = c)) }/>" Check this host against security advisories"</label>
                            </div>
                            <div class="form-group">
                                <label for="sec-c-sched">"Check every (hours)"</label>
                                <input id="sec-c-sched" type="number" min="0" prop:value=hc.schedule_hours.to_string()
                                    on:change=move |e| { let c = num(event_target_value(&e)); upd(Box::new(move |x| x.host_cve.schedule_hours = c)) }/>
                            </div>
                            <div class="form-group">
                                <label for="sec-c-notify"><input id="sec-c-notify" type="checkbox" prop:checked=hc.notify_on_new
                                    on:change=move |e| { let c = event_target_checked(&e); upd(Box::new(move |x| x.host_cve.notify_on_new = c)) }/>" Notify about a new advisory"</label>
                            </div>
                        </div>
                        <div class="form-row">
                            <div class="form-group">
                                <label for="sec-c-glsa">"Advisory directory"</label>
                                <input id="sec-c-glsa" type="text" prop:value=hc.glsa_dir.clone()
                                    on:change=move |e| { let c = event_target_value(&e); upd(Box::new(move |x| x.host_cve.glsa_dir = c.clone())) }/>
                            </div>
                            <div class="form-group">
                                <label for="sec-c-vdb">"Installed-package database"</label>
                                <input id="sec-c-vdb" type="text" prop:value=hc.vdb_dir.clone()
                                    on:change=move |e| { let c = event_target_value(&e); upd(Box::new(move |x| x.host_cve.vdb_dir = c.clone())) }/>
                            </div>
                            <div class="form-group">
                                <label for="sec-c-arch">"Architecture"</label>
                                <input id="sec-c-arch" type="text" placeholder="detected" prop:value=hc.arch.clone()
                                    on:change=move |e| { let c = event_target_value(&e); upd(Box::new(move |x| x.host_cve.arch = c.trim().to_string())) }/>
                            </div>
                        </div>
                        <div class="form-group">
                            <label for="sec-c-ign">"Ignored advisories"</label>
                            <input id="sec-c-ign" type="text" placeholder="advisory ids, comma separated" prop:value=hc.ignore.join(", ")
                                on:change=move |e| { let c: Vec<String> = event_target_value(&e).split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect(); upd(Box::new(move |x| x.host_cve.ignore = c.clone())) }/>
                            <small class="form-help">"Advisories you accept or that do not apply (for example 202401-01)."</small>
                        </div>
                        <div class="form-group">
                            <label for="sec-c-cmd">"Update command shown"</label>
                            <input id="sec-c-cmd" type="text" prop:value=hc.update_command.clone()
                                on:change=move |e| { let c = event_target_value(&e); upd(Box::new(move |x| x.host_cve.update_command = c.clone())) }/>
                            <small class="form-help">"Only displayed; Horcrux never runs it."</small>
                        </div>
                        <h2>"Runtime detection"</h2>
                        <p class="help-text">"Falco watches containers for suspicious behaviour. Off by default; turn it on here or on the Runtime tab."</p>
                        <div class="form-row">
                            <div class="form-group">
                                <label for="sec-r-enabled"><input id="sec-r-enabled" type="checkbox" prop:checked=r.enabled
                                    on:change=move |e| { let c = event_target_checked(&e); upd(Box::new(move |x| x.runtime.enabled = c)) }/>" Detect threats in containers"</label>
                            </div>
                            <div class="form-group">
                                <label for="sec-r-path">"Falco program"</label>
                                <input id="sec-r-path" type="text" prop:value=r.falco_path.clone()
                                    on:change=move |e| { let c = event_target_value(&e); upd(Box::new(move |x| x.runtime.falco_path = c.clone())) }/>
                            </div>
                            <div class="form-group">
                                <label for="sec-r-plugins">"Falco plugin directory"</label>
                                <input id="sec-r-plugins" type="text" prop:value=r.plugin_dir.clone()
                                    on:change=move |e| { let c = event_target_value(&e); upd(Box::new(move |x| x.runtime.plugin_dir = c.clone())) }/>
                            </div>
                        </div>
                        <div class="form-row">
                            <div class="form-group">
                                <label for="sec-r-cpu">"CPU limit (% of one core)"</label>
                                <input id="sec-r-cpu" type="number" min="5" max="800" prop:value=r.cpu_limit_percent.to_string()
                                    on:change=move |e| { let c = num(event_target_value(&e)); upd(Box::new(move |x| x.runtime.cpu_limit_percent = c)) }/>
                            </div>
                            <div class="form-group">
                                <label for="sec-r-mem">"Memory limit (MiB)"</label>
                                <input id="sec-r-mem" type="number" min="768" prop:value=r.memory_limit_mb.to_string()
                                    on:change=move |e| { let c = num(event_target_value(&e)); upd(Box::new(move |x| x.runtime.memory_limit_mb = c)) }/>
                                <small class="form-help">"The eBPF buffers need headroom: 512 MiB was killed at start in testing."</small>
                            </div>
                            <div class="form-group">
                                <label for="sec-r-nice">"CPU niceness (0-19)"</label>
                                <input id="sec-r-nice" type="number" min="0" max="19" prop:value=r.nice.to_string()
                                    on:change=move |e| { let c = num(event_target_value(&e)).min(255) as u8; upd(Box::new(move |x| x.runtime.nice = c)) }/>
                            </div>
                        </div>
                        <div class="form-row">
                            <div class="form-group">
                                <label for="sec-r-min">"Keep detections at or above"</label>
                                <select id="sec-r-min" on:change=move |e| { let c = event_target_value(&e); upd(Box::new(move |x| x.runtime.min_priority = c.clone())) }>
                                    {["notice", "warning", "error", "critical"].into_iter().map(|o| view! { <option value=o selected=r.min_priority == o>{o}</option> }).collect_view()}
                                </select>
                            </div>
                            <div class="form-group">
                                <label for="sec-r-notify">"Notify at or above"</label>
                                <select id="sec-r-notify" on:change=move |e| { let c = event_target_value(&e); upd(Box::new(move |x| x.runtime.notify_min_priority = c.clone())) }>
                                    {["notice", "warning", "error", "critical"].into_iter().map(|o| view! { <option value=o selected=r.notify_min_priority == o>{o}</option> }).collect_view()}
                                </select>
                            </div>
                            <div class="form-group">
                                <label for="sec-r-dedupe">"Same rule and container notifies every (s)"</label>
                                <input id="sec-r-dedupe" type="number" min="0" prop:value=r.notify_dedupe_secs.to_string()
                                    on:change=move |e| { let c = num(event_target_value(&e)); upd(Box::new(move |x| x.runtime.notify_dedupe_secs = c)) }/>
                            </div>
                            <div class="form-group">
                                <label for="sec-r-keep">"Detections kept"</label>
                                <input id="sec-r-keep" type="number" min="100" prop:value=r.keep_events.to_string()
                                    on:change=move |e| { let c = num(event_target_value(&e)); upd(Box::new(move |x| x.runtime.keep_events = c)) }/>
                            </div>
                        </div>
                        <div class="form-group">
                            <label for="sec-r-ignore">"Ignored containers"</label>
                            <input id="sec-r-ignore" type="text" placeholder="comma separated" prop:value=r.ignore_containers.join(", ")
                                on:change=move |e| { let c: Vec<String> = event_target_value(&e).split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect(); upd(Box::new(move |x| x.runtime.ignore_containers = c.clone())) }/>
                            <small class="form-help">"Detections from these containers are dropped (for example build containers that write below /etc by design)."</small>
                        </div>
                        <div class="form-group">
                            <label for="sec-r-off">"Disabled rules"</label>
                            <input id="sec-r-off" type="text" placeholder="comma separated" prop:value=r.disabled_rules.join(", ")
                                on:change=move |e| { let c: Vec<String> = event_target_value(&e).split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect(); upd(Box::new(move |x| x.runtime.disabled_rules = c.clone())) }/>
                        </div>
                        <div class="form-group">
                            <label for="sec-r-extra">"Extra rule files"</label>
                            <input id="sec-r-extra" type="text" placeholder="comma separated" prop:value=r.extra_rules_files.join(", ")
                                on:change=move |e| { let c: Vec<String> = event_target_value(&e).split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect(); upd(Box::new(move |x| x.runtime.extra_rules_files = c.clone())) }/>
                            <small class="form-help">"Your own Falco rule files, read after the curated rules."</small>
                        </div>
                        <div class="form-actions">
                            <button type="submit" class="btn btn-primary" disabled=move || save.pending().get()>"Save"</button>
                        </div>
                    </form>
                }
            })}
        </div>
    }
}
