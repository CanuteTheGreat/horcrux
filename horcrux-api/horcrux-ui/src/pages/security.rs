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
            {tab("/security/events", "events", "Events")}
            {tab("/security/settings", "settings", "Settings")}
        </nav>
    }
}

// ------------------------------------------------------------ overview

#[component]
pub fn SecurityOverviewPage() -> impl IntoView {
    let (ov, set_ov) = create_signal(None::<SecurityOverview>);
    let (err, set_err) = create_signal(None::<String>);
    let (ids, set_ids) = create_signal(None::<IdsStatus>);
    let load = create_action(move |_: &()| async move {
        if let Ok(i) = get_ids_status().await {
            set_ids.set(Some(i));
        }
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
                            {move || ids.get().map(|i| view! {
                                <A href="/security/events?since=24h" class={if i.events_24h > 0 { "stat-card stat-link warning" } else { "stat-card stat-link" }}>
                                    <span class="stat-label">"Security events (24 h)"</span>
                                    <div class="stat-value">{i.events_24h}</div>
                                    <div class="dash-meta">{format!("audit {}, integrity {}",
                                        if i.audit_enabled { "on" } else { "off" },
                                        if i.integrity_enabled { "on" } else { "off" })}</div>
                                </A>
                            })}
                        </div>
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
            <p class="help-text">"Container images (Podman, Docker) and container root file systems (LXC, Incus) are checked against a vulnerability database. A row shows the newest scan; open it for every finding and the fixed versions."</p>
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
                let a = cur.audit.clone();
                let i = cur.integrity.clone();
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
                        <h2>"Intrusion detection (Linux audit)"</h2>
                        <p class="help-text">"Loads Horcrux's audit rules (logins, sudo/su, changes to accounts, sudoers, sshd_config, /etc/horcrux and the Horcrux programs, kernel modules, clock changes), enables auditd on OpenRC or systemd and shows the records on Security > Events."</p>
                        <div class="form-group">
                            <label for="sec-a-enabled"><input id="sec-a-enabled" type="checkbox" prop:checked=a.enabled
                                on:change=move |e| { let c = event_target_checked(&e); upd(Box::new(move |x| x.audit.enabled = c)) }/>" Audit on for this node"</label>
                        </div>
                        <div class="form-row">
                            <div class="form-group">
                                <label for="sec-a-mod"><input id="sec-a-mod" type="checkbox" prop:checked=a.watch_modules
                                    on:change=move |e| { let c = event_target_checked(&e); upd(Box::new(move |x| x.audit.watch_modules = c)) }/>" Record kernel module loads"</label>
                            </div>
                            <div class="form-group">
                                <label for="sec-a-time"><input id="sec-a-time" type="checkbox" prop:checked=a.watch_time
                                    on:change=move |e| { let c = event_target_checked(&e); upd(Box::new(move |x| x.audit.watch_time = c)) }/>" Record clock changes"</label>
                            </div>
                            <div class="form-group">
                                <label for="sec-a-svc"><input id="sec-a-svc" type="checkbox" prop:checked=a.manage_service
                                    on:change=move |e| { let c = event_target_checked(&e); upd(Box::new(move |x| x.audit.manage_service = c)) }/>" Enable and start auditd"</label>
                            </div>
                        </div>
                        <div class="form-group">
                            <label for="sec-a-extra">"Also watch"</label>
                            <input id="sec-a-extra" type="text" placeholder="absolute paths, comma separated" prop:value=a.extra_watch.join(", ")
                                on:change=move |e| { let c: Vec<String> = event_target_value(&e).split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect(); upd(Box::new(move |x| x.audit.extra_watch = c.clone())) }/>
                            <small class="form-help">"Writes and permission changes to these files or directories are recorded (kind 'watch')."</small>
                        </div>
                        <div class="form-group">
                            <span class="form-label">"Notify for"</span>
                            <div class="checkbox-row">
                                {AUDIT_KINDS.iter().map(|k| {
                                    let k = *k;
                                    let on = a.notify_kinds.iter().any(|x| x == k);
                                    let id = format!("sec-a-n-{k}");
                                    view! {
                                        <label for=id.clone()><input id=id type="checkbox" prop:checked=on
                                            on:change=move |e| { let c = event_target_checked(&e); upd(Box::new(move |x| {
                                                x.audit.notify_kinds.retain(|y| y != k);
                                                if c { x.audit.notify_kinds.push(k.to_string()) }
                                            })) }/>{format!(" {k} ")}</label>
                                    }
                                }).collect_view()}
                            </div>
                            <small class="form-help">"Matching events send a 'security' notification (one per collection run)."</small>
                        </div>
                        <div class="form-row">
                            <div class="form-group">
                                <label for="sec-a-every">"Collect every (s)"</label>
                                <input id="sec-a-every" type="number" min="10" prop:value=a.collect_secs.to_string()
                                    on:change=move |e| { let c = num(event_target_value(&e)); upd(Box::new(move |x| x.audit.collect_secs = c)) }/>
                            </div>
                            <div class="form-group">
                                <label for="sec-a-backlog">"Kernel backlog"</label>
                                <input id="sec-a-backlog" type="number" min="64" prop:value=a.backlog.to_string()
                                    on:change=move |e| { let c = num(event_target_value(&e)); upd(Box::new(move |x| x.audit.backlog = c)) }/>
                            </div>
                            <div class="form-group">
                                <label for="sec-a-file">"Rule file"</label>
                                <input id="sec-a-file" type="text" prop:value=a.rules_file.clone()
                                    on:change=move |e| { let c = event_target_value(&e); upd(Box::new(move |x| x.audit.rules_file = c.clone())) }/>
                            </div>
                        </div>
                        <div class="form-group">
                            <label for="sec-a-imm"><input id="sec-a-imm" type="checkbox" prop:checked=a.immutable
                                on:change=move |e| { let c = event_target_checked(&e); upd(Box::new(move |x| x.audit.immutable = c)) }/>" Lock the rules until the next boot"</label>
                            <small class="form-help">"An intruder with root cannot turn the rules off, but neither can you without a reboot."</small>
                        </div>
                        <h2>"File integrity (AIDE)"</h2>
                        <p class="help-text">"Records a hash and the owner/mode of every file below the listed directories, then reports what was added, removed or changed. Press Initialize on Security > Events once after turning it on."</p>
                        <div class="form-group">
                            <label for="sec-i-enabled"><input id="sec-i-enabled" type="checkbox" prop:checked=i.enabled
                                on:change=move |e| { let c = event_target_checked(&e); upd(Box::new(move |x| x.integrity.enabled = c)) }/>" Integrity checks on for this node"</label>
                        </div>
                        <div class="form-group">
                            <label for="sec-i-paths">"Directories"</label>
                            <input id="sec-i-paths" type="text" prop:value=i.paths.join(", ")
                                on:change=move |e| { let c: Vec<String> = event_target_value(&e).split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect(); upd(Box::new(move |x| x.integrity.paths = c.clone())) }/>
                        </div>
                        <div class="form-group">
                            <label for="sec-i-excl">"Left out"</label>
                            <textarea id="sec-i-excl" rows="3" prop:value=i.exclude.join(", ")
                                on:change=move |e| { let c: Vec<String> = event_target_value(&e).split([',', '\n']).map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect(); upd(Box::new(move |x| x.integrity.exclude = c.clone())) }></textarea>
                            <small class="form-help">"VM disks, container and data directories change all the time and are left out by default."</small>
                        </div>
                        <div class="form-row">
                            <div class="form-group">
                                <label for="sec-i-sched">"Check every (hours)"</label>
                                <input id="sec-i-sched" type="number" min="0" prop:value=i.schedule_hours.to_string()
                                    on:change=move |e| { let c = num(event_target_value(&e)); upd(Box::new(move |x| x.integrity.schedule_hours = c)) }/>
                                <small class="form-help">"0 = only when you press Check now."</small>
                            </div>
                            <div class="form-group">
                                <label for="sec-i-nice">"CPU niceness (0-19)"</label>
                                <input id="sec-i-nice" type="number" min="0" max="19" prop:value=i.nice.to_string()
                                    on:change=move |e| { let c = num(event_target_value(&e)).min(255) as u8; upd(Box::new(move |x| x.integrity.nice = c)) }/>
                            </div>
                            <div class="form-group">
                                <label for="sec-i-timeout">"Time limit (s)"</label>
                                <input id="sec-i-timeout" type="number" min="60" prop:value=i.timeout_secs.to_string()
                                    on:change=move |e| { let c = num(event_target_value(&e)); upd(Box::new(move |x| x.integrity.timeout_secs = c)) }/>
                            </div>
                            <div class="form-group">
                                <label for="sec-i-max">"File events per check"</label>
                                <input id="sec-i-max" type="number" min="0" prop:value=i.max_file_events.to_string()
                                    on:change=move |e| { let c = num(event_target_value(&e)); upd(Box::new(move |x| x.integrity.max_file_events = c)) }/>
                            </div>
                        </div>
                        <div class="form-row">
                            <div class="form-group">
                                <label for="sec-i-aide">"AIDE program"</label>
                                <input id="sec-i-aide" type="text" placeholder="looked up in PATH" prop:value=i.aide_path.clone()
                                    on:change=move |e| { let c = event_target_value(&e); upd(Box::new(move |x| x.integrity.aide_path = c.clone())) }/>
                            </div>
                            <div class="form-group">
                                <label for="sec-i-db">"Database directory"</label>
                                <input id="sec-i-db" type="text" placeholder="<data dir>/security/aide" prop:value=i.db_dir.clone()
                                    on:change=move |e| { let c = event_target_value(&e); upd(Box::new(move |x| x.integrity.db_dir = c.clone())) }/>
                            </div>
                            <div class="form-group">
                                <label for="sec-i-notify"><input id="sec-i-notify" type="checkbox" prop:checked=i.notify
                                    on:change=move |e| { let c = event_target_checked(&e); upd(Box::new(move |x| x.integrity.notify = c)) }/>" Notify when a check finds differences"</label>
                            </div>
                        </div>
                        <div class="form-group">
                            <label for="sec-ev-keep">"Security events kept"</label>
                            <input id="sec-ev-keep" type="number" min="100" prop:value=cur.events.keep.to_string()
                                on:change=move |e| { let c = num(event_target_value(&e)); upd(Box::new(move |x| x.events.keep = c)) }/>
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

// ------------------------------------------------------------ events (audit + integrity)

/// `24h` / `7d` / `30d` -> unix seconds `since`.
fn since_secs(v: &str) -> Option<i64> {
    let now = (js_sys::Date::now() / 1000.0) as i64;
    match v {
        "24h" => Some(now - 86_400),
        "7d" => Some(now - 7 * 86_400),
        "30d" => Some(now - 30 * 86_400),
        _ => v.parse().ok(),
    }
}

fn enc(s: &str) -> String {
    js_sys::encode_uri_component(s)
        .as_string()
        .unwrap_or_default()
}

#[component]
pub fn SecurityEventsPage() -> impl IntoView {
    let query = use_query_map();
    let qp = move |k: &'static str| query.with(|q| q.get(k).cloned().unwrap_or_default());
    let (page, set_page) = create_signal(None::<SecurityEventPage>);
    let (ids, set_ids) = create_signal(None::<IdsStatus>);
    let (err, set_err) = create_signal(None::<String>);
    let (ok, set_ok) = create_signal(None::<String>);
    let (unavailable, set_unavailable) = create_signal(None::<Unavailable>);
    let (search, set_search) = create_signal(qp("q"));
    let offset = create_rw_signal(0usize);
    const PAGE: usize = 50;
    let seq = store_value(0u32);
    let api_query = move || {
        let mut parts = Vec::new();
        for k in ["source", "kind", "severity", "user"] {
            let v = qp(k);
            if !v.is_empty() {
                parts.push(format!("{k}={}", enc(&v)));
            }
        }
        if let Some(s) = since_secs(&qp("since")) {
            parts.push(format!("since={s}"));
        }
        let q = search.get_untracked();
        if !q.trim().is_empty() {
            parts.push(format!("q={}", enc(q.trim())));
        }
        parts.push(format!("limit={PAGE}&offset={}", offset.get_untracked()));
        parts.join("&")
    };
    let load = create_action(move |_: &()| async move {
        seq.update_value(|n| *n += 1);
        let mine = seq.get_value();
        let q = api_query();
        let st = get_ids_status().await;
        let ev = list_security_events(&q).await;
        if mine != seq.get_value() {
            return; // a newer load answered first
        }
        if let Ok(s) = st {
            set_ids.set(Some(s));
        }
        match ev {
            Ok(p) => {
                set_err.set(None);
                set_page.set(Some(p))
            }
            Err(e) => set_err.set(Some(format!("Could not load events: {e}"))),
        }
    });
    // Reload when the URL filters change.
    create_effect(move |_| {
        query.track();
        offset.set(0);
        load.dispatch(());
    });
    let running =
        create_memo(move |_| ids.with(|i| i.as_ref().and_then(|i| i.integrity_running.clone())));
    let poll = set_interval_with_handle(
        move || {
            if running.get_untracked().is_some() {
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
    let act = create_action(move |what: &&'static str| {
        let what = *what;
        async move {
            set_err.set(None);
            set_ok.set(None);
            let r = if what == "collect" {
                audit_collect_now()
                    .await
                    .map(|v| format!("{} new audit event(s).", v["added"].as_u64().unwrap_or(0)))
            } else {
                integrity_action(what).await.map(|_| match what {
                    "init" => "Recording the integrity baseline. This reads every file once and runs at low priority.".to_string(),
                    "accept" => "Accepting the current files as the new baseline.".to_string(),
                    _ => "Integrity check started.".to_string(),
                })
            };
            match r {
                Ok(m) => {
                    set_unavailable.set(None);
                    set_ok.set(Some(m))
                }
                Err(e) => match e.unavailable() {
                    Some(u) => set_unavailable.set(Some(u)),
                    None => set_err.set(Some(format!("{e}"))),
                },
            }
            load.dispatch(());
        }
    });
    // Filter links keep the other filters.
    let link = move |key: &'static str, val: &'static str| {
        query.with(|q| {
            let mut parts: Vec<String> = Vec::new();
            for k in ["source", "kind", "severity", "user", "since", "q"] {
                if k == key {
                    continue;
                }
                if let Some(v) = q.get(k).filter(|v| !v.is_empty()) {
                    parts.push(format!("{k}={}", enc(v)));
                }
            }
            if !val.is_empty() {
                parts.push(format!("{key}={val}"));
            }
            if parts.is_empty() {
                "/security/events".to_string()
            } else {
                format!("/security/events?{}", parts.join("&"))
            }
        })
    };
    let filter = move |key: &'static str, val: &'static str, label: &'static str| {
        let href = link(key, val);
        let on = Signal::derive(move || qp(key) == val);
        view! { <NavLink href=href class=Signal::derive(move || if on.get() { "filter-link active".to_string() } else { "filter-link".to_string() }) active=on>{label}</NavLink> }
    };
    view! {
        <div class="security-page">
            <div class="page-header">
                <h1>"Security events"</h1>
                <div class="header-actions">
                    <button class="btn btn-secondary" on:click=move |_| load.dispatch(())>"Refresh"</button>
                </div>
            </div>
            <SecurityTabs active="events"/>
            <p class="help-text">"Intrusion detection (Linux audit: logins, sudo, changes to accounts and system files, kernel modules, clock) and file integrity (AIDE) findings on this node. Who did what through the Horcrux API is in the "<A href="/audit">"audit log"</A>"."</p>
            {move || err.get().map(|m| view! { <div class="alert alert-error">{m}</div> })}
            {move || ok.get().map(|m| view! { <div class="alert alert-success">{m}</div> })}
            {move || unavailable.get().map(|u| view! {
                <crate::components::UnavailableState info=u on_retry=Callback::new(move |_| { set_unavailable.set(None); load.dispatch(()) })/>
            })}
            {move || ids.get().map(|i| {
                let audit_line = if !i.audit_installed {
                    "The audit tools are not installed (sys-process/audit).".to_string()
                } else if !i.audit_enabled {
                    "Off. Turn it on in Settings.".to_string()
                } else {
                    format!("{} Horcrux rules loaded, auditd {}, last collected {}.",
                        i.audit_rules_loaded,
                        if i.auditd_running { "running" } else { "NOT running" },
                        when(i.audit_last_collect))
                };
                let integ_line = if !i.aide_installed {
                    "AIDE is not installed (app-forensics/aide).".to_string()
                } else if !i.integrity_enabled {
                    "Off. Turn it on in Settings.".to_string()
                } else if let Some(r) = &i.integrity_running {
                    format!("{r} running...")
                } else if !i.integrity_initialized {
                    "No baseline yet: press Initialize.".to_string()
                } else {
                    format!("Baseline from {}, last check {}{}.",
                        when(i.integrity_initialized_at),
                        when(i.integrity_last_check),
                        i.integrity_last_differences.map(|n| format!(" ({n} differences)")).unwrap_or_default())
                };
                let busy = i.integrity_running.is_some();
                let can_i = i.aide_installed && i.integrity_enabled && !busy;
                let inited = i.integrity_initialized;
                let can_a = i.audit_installed && i.audit_enabled;
                view! {
                    <div class="stats-grid">
                        <div class="card ids-card">
                            <h2>"Intrusion detection"</h2>
                            <p>{audit_line}</p>
                            {i.audit_error.clone().map(|e| view! { <p class="text-danger">{e}</p> })}
                            <div class="actions">
                                <button class="btn btn-secondary btn-sm" disabled=!can_a on:click=move |_| act.dispatch("collect")>"Collect now"</button>
                                <A href="/security/settings" class="btn btn-secondary btn-sm">"Settings"</A>
                            </div>
                        </div>
                        <div class="card ids-card">
                            <h2>"File integrity"</h2>
                            <p>{integ_line}</p>
                            {i.integrity_error.clone().map(|e| view! { <p class="text-danger">{e}</p> })}
                            <div class="actions">
                                <button class="btn btn-primary btn-sm" disabled=!can_i
                                    on:click=move |_| if !inited || ask("Record a new baseline? Differences found until now are no longer reported.") { act.dispatch("init") }>"Initialize"</button>
                                <button class="btn btn-secondary btn-sm" disabled=!(can_i && inited) on:click=move |_| act.dispatch("check")>"Check now"</button>
                                <button class="btn btn-secondary btn-sm" disabled=!(can_i && inited)
                                    on:click=move |_| if ask("Accept every current file as expected (for example after updates)?") { act.dispatch("accept") }>"Accept current files"</button>
                            </div>
                        </div>
                    </div>
                }
            })}
            <div class="toolbar">
                <div class="filter-links">
                    {filter("source", "", "All sources")}
                    {filter("source", "audit", "Audit")}
                    {filter("source", "integrity", "Integrity")}
                </div>
                <div class="filter-links">
                    {filter("severity", "", "Any severity")}
                    {filter("severity", "high", "High or worse")}
                    {filter("severity", "medium", "Medium or worse")}
                </div>
                <div class="filter-links">
                    {filter("since", "", "Any time")}
                    {filter("since", "24h", "24 h")}
                    {filter("since", "7d", "7 days")}
                </div>
                <label class="sr-only" for="sec-ev-search">"Search events"</label>
                <input id="sec-ev-search" type="search" placeholder="Search events" aria-label="Search events"
                    prop:value=search on:change=move |e| { set_search.set(event_target_value(&e)); offset.set(0); load.dispatch(()) }/>
            </div>
            {move || (!qp("kind").is_empty() || !qp("user").is_empty()).then(|| view! {
                <p class="help-text">{format!("Showing kind '{}' user '{}'. ", qp("kind"), qp("user"))}<A href=link("kind", "")>"Any kind"</A>" · "<A href=link("user", "")>"Any user"</A></p>
            })}
            {move || match page.get() {
                None => view! { <div class="loading">"Loading..."</div> }.into_view(),
                Some(p) if p.total == 0 => {
                    let filtered = ["source", "kind", "severity", "user", "since", "q"].iter().any(|k| !qp(k).is_empty()) || !search.get_untracked().is_empty();
                    if filtered {
                        view! { <div class="empty-state">"No event matches these filters. "<A href="/security/events">"Show all"</A></div> }.into_view()
                    } else {
                        view! { <div class="empty-state">"No security events yet. Turn on audit and file integrity in "<A href="/security/settings">"Settings"</A>", then press Initialize above."</div> }.into_view()
                    }
                }
                Some(p) => {
                    let total = p.total;
                    let off = offset.get_untracked();
                    view! {
                        <div class="table-container">
                        <table class="data-table events-table">
                            <thead><tr><th>"Time"</th><th>"Severity"</th><th>"Source"</th><th>"Event"</th><th>"User"</th></tr></thead>
                            <tbody>
                                {p.events.into_iter().map(|e| {
                                    let sev = e.severity.unwrap_or(Severity::Unknown);
                                    let src = e.source.map(|s| s.as_str()).unwrap_or("");
                                    let kind_href = format!("/security/events?source={src}&kind={}", enc(&e.kind));
                                    let user = e.user.clone();
                                    view! {
                                        <tr>
                                            <td class="nowrap" data-label="Time">{when(Some(e.at))}</td>
                                            <td data-label="Severity"><SevBadge sev=sev/></td>
                                            <td data-label="Source"><A href=kind_href>{format!("{src} · {}", e.kind)}</A></td>
                                            <td data-label="Event">
                                                <div>{e.summary.clone()}</div>
                                                {(!e.path.is_empty() && !e.summary.contains(&e.path)).then(|| view! { <div class="mono">{e.path.clone()}</div> })}
                                                {(!e.detail.is_empty()).then(|| view! { <div class="dash-meta mono">{e.detail.clone()}</div> })}
                                            </td>
                                            <td data-label="User">
                                                {if user.is_empty() || user == "unset" {
                                                    view! { <span class="dash-meta">"-"</span> }.into_view()
                                                } else {
                                                    let ev_href = format!("/security/events?user={}", enc(&user));
                                                    let audit_href = format!("/audit?user={}", enc(&user));
                                                    view! {
                                                        <A href=ev_href>{user}</A>
                                                        <div><A href=audit_href class="dash-meta">"API audit log"</A></div>
                                                    }.into_view()
                                                }}
                                            </td>
                                        </tr>
                                    }
                                }).collect_view()}
                            </tbody>
                        </table>
                        </div>
                        <div class="pagination">
                            <span>{format!("{}-{} of {total}", off + 1, (off + PAGE).min(total))}</span>
                            <button class="btn btn-secondary btn-sm" disabled=off == 0 on:click=move |_| { offset.set(off.saturating_sub(PAGE)); load.dispatch(()) }>"Newer"</button>
                            <button class="btn btn-secondary btn-sm" disabled=off + PAGE >= total on:click=move |_| { offset.set(off + PAGE); load.dispatch(()) }>"Older"</button>
                        </div>
                    }.into_view()
                }
            }}
        </div>
    }
}
