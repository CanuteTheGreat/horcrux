//! Declarative configuration (`/declarative`): the desired spec the node
//! should follow, drift against it, GitOps, and the apply history with
//! rollback. Tabs are real links (`?tab=desired|drift|gitops|history`).
use crate::api::declarative::{self as d, ApplyReport, DriftItem, GitConfig, Plan};
use crate::api::ApiError;
use crate::components::UnavailableState;
use leptos::*;

const TABS: [(&str, &str); 4] = [
    ("desired", "Desired spec"),
    ("drift", "Plan & drift"),
    ("gitops", "GitOps"),
    ("history", "History"),
];

fn confirm(msg: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.confirm_with_message(msg).ok())
        .unwrap_or(false)
}

fn short(s: &str) -> String {
    s.chars().take(12).collect()
}

/// The page that shows a resource of this kind (None for kinds without one).
fn resource_href(kind: &str, name: &str) -> Option<String> {
    let n = urlencoding::encode(name);
    match kind {
        "vm" => Some(format!("/vms/{n}")),
        "storage_pool" => Some("/storage".to_string()),
        "zone" | "vnet" | "subnet" => Some("/network/sdn".to_string()),
        _ => None,
    }
}

/// A plain line diff (LCS) of two texts: `(' ' | '+' | '-', line)`.
pub fn line_diff(a: &str, b: &str) -> Vec<(char, String)> {
    let a: Vec<&str> = a.lines().collect();
    let b: Vec<&str> = b.lines().collect();
    let (n, m) = (a.len(), b.len());
    if n * m > 4_000_000 {
        return vec![(' ', "(the texts are too large to diff here)".into())];
    }
    let mut t = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            t[i][j] = if a[i] == b[j] {
                t[i + 1][j + 1] + 1
            } else {
                t[i + 1][j].max(t[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < n && j < m {
        if a[i] == b[j] {
            out.push((' ', a[i].to_string()));
            i += 1;
            j += 1;
        } else if t[i + 1][j] >= t[i][j + 1] {
            out.push(('-', a[i].to_string()));
            i += 1;
        } else {
            out.push(('+', b[j].to_string()));
            j += 1;
        }
    }
    out.extend(a[i..].iter().map(|l| ('-', l.to_string())));
    out.extend(b[j..].iter().map(|l| ('+', l.to_string())));
    out
}

#[component]
pub fn DeclarativePage() -> impl IntoView {
    let query = leptos_router::use_query_map();
    let tab = move || {
        query
            .get()
            .get("tab")
            .cloned()
            .filter(|t| TABS.iter().any(|(k, _)| k == t))
            .unwrap_or_else(|| "desired".to_string())
    };
    view! {
        <div class="declarative-page">
            <div class="page-header"><h1>"Declarative Configuration"</h1></div>
            <p class="form-help">
                "Describe this node in a spec file (storage pools, networks, VMs); Horcrux shows how the node differs from it and can apply only the difference. "
                "Keep the spec here, or follow a git repository (GitOps). Every apply is recorded and can be rolled back."
            </p>
            <nav class="tabs" aria-label="Declarative sections">
                {TABS.iter().map(|(key, label)| {
                    let key = *key;
                    view! {
                        <a class="tab" id=format!("decl-tab-{key}") class:active=move || tab() == key
                            href=format!("/declarative?tab={key}")>{*label}</a>
                    }
                }).collect_view()}
            </nav>
            {move || match tab().as_str() {
                "drift" => view! { <DriftTab/> }.into_view(),
                "gitops" => view! { <GitopsTab/> }.into_view(),
                "history" => view! { <HistoryTab/> }.into_view(),
                _ => view! { <DesiredTab/> }.into_view(),
            }}
        </div>
    }
}

// ------------------------------------------------------------ shared bits

#[component]
fn Banner(notice: ReadSignal<Option<String>>, error: ReadSignal<Option<String>>) -> impl IntoView {
    view! {
        {move || notice.get().map(|n| view! { <div class="alert alert-info decl-notice" role="status">{n}</div> })}
        {move || error.get().map(|e| view! { <div class="alert alert-error decl-error" role="alert">{e}</div> })}
    }
}

/// Differences as a table; every kind/name is a link to the resource page
/// (or to the filtered drift view for kinds without a page of their own).
#[component]
fn ItemsTable(items: Vec<DriftItem>) -> impl IntoView {
    view! {
        <div class="table-container">
            <table class="data-table drift-table">
                <thead><tr>
                    <th>"Kind"</th><th>"Name"</th><th>"Field"</th><th>"Expected"</th><th>"Actual"</th><th>"Apply would"</th>
                </tr></thead>
                <tbody>
                    {items.into_iter().map(|i| {
                        let href = resource_href(&i.kind, &i.name).unwrap_or_else(|| {
                            format!("/declarative?tab=drift&kind={}", urlencoding::encode(&i.kind))
                        });
                        let kind_href = format!("/declarative?tab=drift&kind={}", urlencoding::encode(&i.kind));
                        let what = if i.blocked {
                            format!("blocked: {}", if i.note.is_empty() { "needs a manual change" } else { &i.note })
                        } else {
                            i.action.clone()
                        };
                        let blocked = i.blocked;
                        view! {
                            <tr class="drift-row" class:drift-blocked=blocked>
                                <td><a href=kind_href>{i.kind.clone()}</a></td>
                                <td><a href=href>{i.name.clone()}</a></td>
                                <td><code>{i.field.clone()}</code></td>
                                <td>{i.expected.clone()}</td>
                                <td>{i.actual.clone()}</td>
                                <td>{what}</td>
                            </tr>
                        }
                    }).collect_view()}
                </tbody>
            </table>
        </div>
    }
}

/// A plan or apply result: one line per change with its detail.
#[component]
fn PlanView(plan: Plan, #[prop(optional)] results: Vec<d::ChangeResult>) -> impl IntoView {
    let errors = plan.errors.clone();
    let rows: Vec<(String, String, String, String, String)> = if results.is_empty() {
        plan.changes
            .iter()
            .map(|c| {
                (
                    c.kind.clone(),
                    c.name.clone(),
                    c.action.clone(),
                    c.details.join("; "),
                    String::new(),
                )
            })
            .collect()
    } else {
        results
            .iter()
            .map(|r| {
                (
                    r.kind.clone(),
                    r.name.clone(),
                    r.action.clone(),
                    r.details.join("; "),
                    if r.message.is_empty() {
                        r.status.clone()
                    } else {
                        format!("{}: {}", r.status, r.message)
                    },
                )
            })
            .collect()
    };
    view! {
        {errors.into_iter().map(|e| view! { <div class="alert alert-error">{e}</div> }).collect_view()}
        {if rows.is_empty() {
            view! { <p class="no-data">"Nothing to change."</p> }.into_view()
        } else {
            view! {
                <div class="table-container">
                    <table class="data-table plan-table">
                        <thead><tr><th>"Action"</th><th>"Kind"</th><th>"Name"</th><th>"Detail"</th><th>"Result"</th></tr></thead>
                        <tbody>
                            {rows.into_iter().map(|(kind, name, action, detail, result)| {
                                let href = resource_href(&kind, &name);
                                let name_view = match href {
                                    Some(h) => view! { <a href=h>{name.clone()}</a> }.into_view(),
                                    None => name.clone().into_view(),
                                };
                                let failed = result.starts_with("failed") || result.starts_with("skipped");
                                view! {
                                    <tr class:drift-blocked=failed>
                                        <td>{action}</td><td>{kind}</td><td>{name_view}</td>
                                        <td>{detail}</td><td>{result}</td>
                                    </tr>
                                }
                            }).collect_view()}
                        </tbody>
                    </table>
                </div>
            }.into_view()
        }}
    }
}

#[component]
fn ReportView(report: ApplyReport) -> impl IntoView {
    let history = report.history_id.clone();
    view! {
        <div class="apply-report">
            <p class:alert-count-text=!report.ok>
                {if report.ok { "Applied." } else { "Not everything could be applied." }}
                {history.map(|h| view! {
                    " " <a href=format!("/declarative?tab=history&id={}", urlencoding::encode(&h))>"See it in the history"</a>
                })}
            </p>
            <PlanView plan=report.plan results=report.results/>
        </div>
    }
}

// ---------------------------------------------------------- desired spec

#[component]
fn DesiredTab() -> impl IntoView {
    let (meta, set_meta) = create_signal(d::Desired::default());
    let text = create_rw_signal(String::new());
    let (loaded, set_loaded) = create_signal(false);
    let (check, set_check) = create_signal(None::<d::ValidateResult>);
    let (notice, set_notice) = create_signal(None::<String>);
    let (error, set_error) = create_signal(None::<String>);
    let (busy, set_busy) = create_signal(false);

    let load = move || {
        spawn_local(async move {
            match d::get_desired().await {
                Ok(doc) => {
                    if doc.configured {
                        text.set(doc.spec.clone());
                    }
                    set_meta.set(doc);
                    set_error.set(None);
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            set_loaded.set(true);
        });
    };
    create_effect(move |_| load());

    let validate = move |_| {
        set_busy.set(true);
        set_notice.set(None);
        let spec = text.get();
        spawn_local(async move {
            match d::validate(&spec).await {
                Ok(r) => {
                    set_check.set(Some(r));
                    set_error.set(None);
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            set_busy.set(false);
        });
    };
    let save = move |_| {
        set_busy.set(true);
        let spec = text.get();
        spawn_local(async move {
            match d::put_desired(&spec).await {
                Ok(r) => {
                    set_error.set(None);
                    set_check.set(None);
                    set_notice.set(Some(if r.in_sync {
                        "Saved. The node already matches this spec.".to_string()
                    } else {
                        format!(
                            "Saved. The node differs from it in {} place(s).",
                            r.drift_count
                        )
                    }));
                    load();
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            set_busy.set(false);
        });
    };
    let from_live = move |_| {
        spawn_local(async move {
            match d::export_text().await {
                Ok(t) => {
                    text.set(t);
                    set_error.set(None);
                    set_notice.set(Some(
                        "Filled in from this node's current state: edit it, then Save.".into(),
                    ));
                }
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };
    let clear = move |_| {
        if !confirm(
            "Remove the stored desired spec? Drift checking stops; nothing on the node changes.",
        ) {
            return;
        }
        spawn_local(async move {
            match d::clear_desired().await {
                Ok(()) => {
                    text.set(String::new());
                    set_check.set(None);
                    set_notice.set(Some("Desired spec removed.".into()));
                    load();
                }
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };

    view! {
        <section class="desired-tab" id="decl-desired">
            <Banner notice error/>
            {move || {
                let m = meta.get();
                if !loaded.get() {
                    view! { <p class="loading">"Loading..."</p> }.into_view()
                } else if m.configured {
                    view! {
                        <p class="form-help" id="decl-desired-meta">
                            "Stored " {m.updated_at.clone()} " by " {m.updated_by.clone()} " via " {m.source.clone()}
                            {m.commit.clone().map(|c| format!(" (commit {})", short(&c)))}
                            ", hash " <code>{short(&m.hash)}</code> ". "
                            <a href="/declarative?tab=drift">"See how the node differs"</a>
                        </p>
                    }.into_view()
                } else {
                    view! {
                        <div class="empty-state" id="decl-desired-empty">
                            <h2 class="empty-title">"No desired spec yet"</h2>
                            <p class="empty-todo">"Write a spec below, or start from what this node has now. Once saved, Horcrux checks the node against it and shows the differences."</p>
                            <div class="empty-actions">
                                <button type="button" class="btn btn-primary" id="decl-from-live" on:click=from_live>"Start from this node's current state"</button>
                                <a class="btn btn-secondary" href="/declarative?tab=gitops">"Follow a git repository instead"</a>
                            </div>
                        </div>
                    }.into_view()
                }
            }}
            <label for="decl-spec" class="form-label">"Spec (YAML or JSON)"</label>
            <textarea id="decl-spec" class="spec-editor" rows="22" spellcheck="false"
                placeholder="version: 1\nvms:\n  - {name: web1, memory: 2048, cpus: 2, power: running}"
                prop:value=move || text.get()
                on:input=move |ev| text.set(event_target_value(&ev))></textarea>
            <div class="form-actions">
                <button type="button" class="btn btn-secondary" id="decl-validate" disabled=move || busy.get() on:click=validate>"Validate"</button>
                <button type="button" class="btn btn-primary" id="decl-save" disabled=move || busy.get() on:click=save>"Save as desired spec"</button>
                {move || meta.get().configured.then(|| view! {
                    <button type="button" class="btn btn-danger" id="decl-clear" on:click=clear>"Remove"</button>
                })}
            </div>
            {move || check.get().map(|r| {
                if let Some(e) = r.error.clone() {
                    view! { <div class="alert alert-error" id="decl-validate-result">{e}</div> }.into_view()
                } else if !r.ok {
                    view! {
                        <div id="decl-validate-result">
                            {r.errors.iter().map(|e| view! { <div class="alert alert-error">{e.clone()}</div> }).collect_view()}
                        </div>
                    }.into_view()
                } else if r.items.is_empty() {
                    view! { <div class="alert alert-info" id="decl-validate-result">"The spec is valid and the node already matches it."</div> }.into_view()
                } else {
                    view! {
                        <div id="decl-validate-result">
                            <div class="alert alert-info">"The spec is valid. Applying it would change:"</div>
                            <ItemsTable items=r.items.clone()/>
                        </div>
                    }.into_view()
                }
            })}
        </section>
    }
}

// ----------------------------------------------------------- plan & drift

#[component]
fn DriftTab() -> impl IntoView {
    let query = leptos_router::use_query_map();
    let kind_filter = move || query.get().get("kind").cloned().unwrap_or_default();
    let (report, set_report) = create_signal(None::<d::DriftReport>);
    let (diff, set_diff) = create_signal(None::<d::DiffTexts>);
    let (settings, set_settings) = create_signal(None::<d::Settings>);
    let (result, set_result) = create_signal(None::<(bool, ApplyReport)>);
    let (notice, set_notice) = create_signal(None::<String>);
    let (error, set_error) = create_signal(None::<String>);
    let (busy, set_busy) = create_signal(false);
    let interval = create_rw_signal(String::new());
    let limit = create_rw_signal(String::new());
    let alert = create_rw_signal(true);

    let load = move |refresh: bool| {
        spawn_local(async move {
            match d::get_drift(refresh).await {
                Ok(r) => {
                    let configured = r.configured;
                    set_report.set(Some(r));
                    set_error.set(None);
                    if configured {
                        if let Ok(t) = d::drift_diff().await {
                            set_diff.set(Some(t));
                        }
                    } else {
                        set_diff.set(None);
                    }
                }
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };
    create_effect(move |_| {
        load(false);
        spawn_local(async move {
            if let Ok(s) = d::get_settings().await {
                interval.set(s.drift_interval_secs.to_string());
                limit.set(s.history_limit.to_string());
                alert.set(s.drift_alert);
                set_settings.set(Some(s));
            }
        });
    });

    let check_now = move |_| {
        set_busy.set(true);
        spawn_local(async move {
            match d::check_drift().await {
                Ok(_) => load(false),
                Err(e) => set_error.set(Some(e.message)),
            }
            set_busy.set(false);
        });
    };
    let run = move |dry: bool| {
        if !dry
            && !confirm("Apply the desired spec now? The node is changed to match it (only the differences listed).")
        {
            return;
        }
        set_busy.set(true);
        set_notice.set(None);
        spawn_local(async move {
            match d::apply_desired(dry).await {
                Ok(r) => {
                    set_result.set(Some((dry, r)));
                    set_error.set(None);
                    if !dry {
                        load(true);
                    }
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            set_busy.set(false);
        });
    };
    let save_settings = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let (Ok(i), Ok(l)) = (
            interval.get().trim().parse::<u64>(),
            limit.get().trim().parse::<u64>(),
        ) else {
            set_error.set(Some(
                "The interval and the history limit are whole numbers.".into(),
            ));
            return;
        };
        let s = d::Settings {
            drift_interval_secs: i,
            history_limit: l,
            drift_alert: alert.get(),
        };
        spawn_local(async move {
            match d::put_settings(&s).await {
                Ok(s) => {
                    set_settings.set(Some(s));
                    set_error.set(None);
                    set_notice.set(Some("Settings saved.".into()));
                }
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };

    view! {
        <section class="drift-tab" id="decl-drift">
            <Banner notice error/>
            {move || match report.get() {
                None => view! { <p class="loading">"Loading..."</p> }.into_view(),
                Some(r) if !r.configured => view! {
                    <div class="empty-state" id="decl-drift-empty">
                        <h2 class="empty-title">"No desired spec to compare against"</h2>
                        <p class="empty-todo">"Drift is the difference between a stored spec and this node. Save a spec first (or connect a git repository)."</p>
                        <div class="empty-actions">
                            <a class="btn btn-primary" href="/declarative?tab=desired">"Write a desired spec"</a>
                            <a class="btn btn-secondary" href="/declarative?tab=gitops">"Set up GitOps"</a>
                        </div>
                    </div>
                }.into_view(),
                Some(r) => {
                    let kf = kind_filter();
                    let items: Vec<DriftItem> = r.items.iter().filter(|i| kf.is_empty() || i.kind == kf).cloned().collect();
                    let summary = if let Some(e) = r.error.clone() {
                        view! { <div class="alert alert-error" id="decl-drift-summary">{e}</div> }.into_view()
                    } else if r.in_sync {
                        view! { <div class="alert alert-info" id="decl-drift-summary">"In sync: the node matches the desired spec."</div> }.into_view()
                    } else {
                        let n = r.count;
                        let b = r.blocked;
                        view! {
                            <div class="alert alert-error" id="decl-drift-summary">
                                {format!("{n} difference(s) from the desired spec")}
                                {(b > 0).then(|| format!(", {b} that Horcrux can't change in place"))}
                            </div>
                        }.into_view()
                    };
                    view! {
                        {summary}
                        <p class="form-help">
                            "Last checked " {r.checked_at.clone()} ". The check runs on its own every "
                            {move || settings.get().map(|s| s.drift_interval_secs.to_string()).unwrap_or_else(|| "...".into())}
                            " seconds and feeds the \"Configuration drift\" alert rule (" <a href="/monitoring/alerts">"Alert Center"</a> ")."
                            {(!kf.is_empty()).then(|| view! {
                                " Showing " <code>{kf.clone()}</code> " only: " <a href="/declarative?tab=drift">"show all"</a> "."
                            })}
                        </p>
                        <div class="form-actions">
                            <button type="button" class="btn btn-secondary" id="decl-check" disabled=move || busy.get() on:click=check_now>"Check now"</button>
                            <button type="button" class="btn btn-secondary" id="decl-preview" disabled=move || busy.get() on:click=move |_| run(true)>"Preview apply"</button>
                            <button type="button" class="btn btn-primary" id="decl-apply" disabled=move || busy.get() || r.in_sync on:click=move |_| run(false)>"Apply desired spec"</button>
                        </div>
                        {r.errors.iter().map(|e| view! { <div class="alert alert-error">{e.clone()}</div> }).collect_view()}
                        {if items.is_empty() {
                            ().into_view()
                        } else {
                            view! { <ItemsTable items=items/> }.into_view()
                        }}
                    }.into_view()
                }
            }}
            {move || result.get().map(|(dry, r)| view! {
                <div id="decl-apply-result">
                    <h3>{if dry { "Preview (nothing changed)" } else { "Result" }}</h3>
                    {if dry {
                        view! { <PlanView plan=r.plan.clone()/> }.into_view()
                    } else {
                        view! { <ReportView report=r.clone()/> }.into_view()
                    }}
                </div>
            })}
            {move || diff.get().filter(|t| t.configured).map(|t| {
                let lines = line_diff(&t.desired, &t.live);
                view! {
                    <details class="spec-diff-box" id="decl-diff" open=!t.same>
                        <summary>"Spec compared with the live state (text diff)"</summary>
                        <p class="form-help">
                            "Lines starting with "<code>"-"</code>" are in the spec but not on the node, "<code>"+"</code>" are on the node but differ from the spec. Only what the spec states is compared."
                        </p>
                        {if t.same {
                            view! { <p class="no-data">"No differences."</p> }.into_view()
                        } else {
                            view! {
                                <pre class="spec-diff">{lines.into_iter().map(|(c, l)| {
                                    let cls = match c { '+' => "diff-add", '-' => "diff-del", _ => "diff-same" };
                                    view! { <span class=cls>{format!("{c} {l}\n")}</span> }
                                }).collect_view()}</pre>
                            }.into_view()
                        }}
                    </details>
                }
            })}
            <form class="form-row settings-form" id="decl-settings" on:submit=save_settings>
                <label>"Check every (seconds) "
                    <input type="number" id="decl-interval" min="5" max="86400" prop:value=move || interval.get()
                        on:input=move |ev| interval.set(event_target_value(&ev))/></label>
                <label>"Keep the last "
                    <input type="number" id="decl-limit" min="10" max="5000" prop:value=move || limit.get()
                        on:input=move |ev| limit.set(event_target_value(&ev))/>" applies"</label>
                <label class="checkbox-label"><input type="checkbox" id="decl-alert" prop:checked=move || alert.get()
                    on:change=move |ev| alert.set(event_target_checked(&ev))/>" Raise an alert while the node drifts"</label>
                <button type="submit" class="btn btn-secondary" id="decl-settings-save">"Save settings"</button>
            </form>
        </section>
    }
}

// ----------------------------------------------------------------- gitops

fn opt(s: String) -> Option<String> {
    let t = s.trim().to_string();
    (!t.is_empty()).then_some(t)
}

#[component]
fn GitopsTab() -> impl IntoView {
    let (status, set_status) = create_signal(None::<d::GitStatus>);
    let (notice, set_notice) = create_signal(None::<String>);
    let (error, set_error) = create_signal(None::<String>);
    let (unavailable, set_unavailable) = create_signal(None::<crate::api::Unavailable>);
    let (busy, set_busy) = create_signal(false);
    let (report, set_report) = create_signal(None::<ApplyReport>);

    let url = create_rw_signal(String::new());
    let branch = create_rw_signal("main".to_string());
    let path = create_rw_signal("horcrux.yaml".to_string());
    let interval = create_rw_signal("300".to_string());
    let mode = create_rw_signal("approve".to_string());
    let verify = create_rw_signal(false);
    let sig_format = create_rw_signal("ssh".to_string());
    let signers = create_rw_signal(String::new());
    let cred_user = create_rw_signal(String::new());
    let cred_ref = create_rw_signal(String::new());
    let ssh_ref = create_rw_signal(String::new());
    let hook_ref = create_rw_signal(String::new());
    let node_name = create_rw_signal(String::new());

    let fail = move |e: ApiError| {
        if let Some(u) = e.unavailable() {
            set_unavailable.set(Some(u));
        } else {
            set_error.set(Some(e.message));
        }
    };
    let load = move || {
        spawn_local(async move {
            match d::git_status().await {
                Ok(s) => set_status.set(Some(s)),
                Err(e) => fail(e),
            }
        });
    };
    create_effect(move |_| {
        load();
        spawn_local(async move {
            if let Ok(doc) = d::git_config().await {
                if let Some(c) = doc.config {
                    url.set(c.url);
                    branch.set(c.branch);
                    path.set(c.path);
                    interval.set(c.interval_secs.to_string());
                    mode.set(c.mode);
                    verify.set(c.verify_signatures);
                    if let Some(s) = c.signing {
                        sig_format.set(s.format);
                        signers.set(
                            s.allowed_signers
                                .or(s.allowed_signers_file)
                                .unwrap_or_default(),
                        );
                    }
                    if let Some(c2) = c.credentials {
                        cred_user.set(c2.username.unwrap_or_default());
                        cred_ref.set(c2.secret);
                    }
                    ssh_ref.set(c.ssh_key.unwrap_or_default());
                    hook_ref.set(c.webhook_secret.unwrap_or_default());
                    node_name.set(c.node_name.unwrap_or_default());
                }
            }
        });
    });

    let save = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let Ok(iv) = interval.get().trim().parse::<u64>() else {
            set_error.set(Some("The interval is a whole number of seconds.".into()));
            return;
        };
        let signing = verify.get().then(|| {
            let t = signers.get();
            let t = t.trim().to_string();
            // an absolute path is a file on the node, anything else is the keys themselves
            let file = t.starts_with('/') && !t.contains('\n');
            d::Signing {
                format: sig_format.get(),
                allowed_signers: (!file).then(|| t.clone()),
                allowed_signers_file: file.then(|| t.clone()),
            }
        });
        let cfg = GitConfig {
            enabled: true,
            url: url.get().trim().to_string(),
            branch: branch.get().trim().to_string(),
            path: path.get().trim().to_string(),
            interval_secs: iv,
            mode: mode.get(),
            verify_signatures: verify.get(),
            signing,
            credentials: opt(cred_ref.get()).map(|secret| d::Credentials {
                username: opt(cred_user.get()),
                secret,
            }),
            ssh_key: opt(ssh_ref.get()),
            webhook_secret: opt(hook_ref.get()),
            node_name: opt(node_name.get()),
        };
        spawn_local(async move {
            match d::put_git_config(&cfg).await {
                Ok(_) => {
                    set_error.set(None);
                    set_notice.set(Some(
                        "GitOps settings saved. Use \"Sync now\" to fetch.".into(),
                    ));
                    load();
                }
                Err(e) => fail(e),
            }
        });
    };
    let sync = move |_| {
        set_busy.set(true);
        set_notice.set(None);
        set_error.set(None);
        spawn_local(async move {
            match d::git_sync().await {
                Ok(o) => set_notice.set(Some(o.result)),
                Err(e) => fail(e),
            }
            load();
            set_busy.set(false);
        });
    };
    let remove = move |_| {
        if !confirm("Stop following the repository? Pending approvals are dropped; the node is not changed.") {
            return;
        }
        spawn_local(async move {
            match d::delete_git_config().await {
                Ok(()) => {
                    set_notice.set(Some("GitOps removed.".into()));
                    url.set(String::new());
                    load();
                }
                Err(e) => fail(e),
            }
        });
    };
    let approve = move |hash: String| {
        if !confirm("Apply this plan to the node?") {
            return;
        }
        set_busy.set(true);
        spawn_local(async move {
            match d::git_approve(&hash).await {
                Ok(r) => {
                    set_report.set(Some(r));
                    set_error.set(None);
                }
                Err(e) => fail(e),
            }
            load();
            set_busy.set(false);
        });
    };
    let reject = move |hash: String| {
        let reason = web_sys::window().and_then(|w| {
            w.prompt_with_message("Reject this commit? Optional reason:")
                .ok()
                .flatten()
        });
        let Some(reason) = reason else { return };
        spawn_local(async move {
            match d::git_reject(&hash, &reason).await {
                Ok(_) => {
                    set_notice.set(Some("Rejected. This commit won't be queued again.".into()))
                }
                Err(e) => fail(e),
            }
            load();
        });
    };

    view! {
        <section class="gitops-tab" id="decl-gitops">
            <Banner notice error/>
            {move || unavailable.get().map(|u| view! {
                <UnavailableState info=u on_retry=move |_| { set_unavailable.set(None); load(); }/>
            })}
            {move || status.get().map(|s| {
                if !s.configured {
                    view! {
                        <div class="empty-state" id="decl-git-empty">
                            <h2 class="empty-title">"GitOps is not set up"</h2>
                            <p class="empty-todo">"Point this node at a git repository holding its spec. Changes there are planned (and applied, or held for your approval) automatically."</p>
                            <div class="empty-actions"><a class="btn btn-primary" href="#decl-git-form">"Fill in the repository below"</a></div>
                        </div>
                    }.into_view()
                } else {
                    let sig = s.signature.clone().unwrap_or_else(|| "unchecked".into());
                    view! {
                        <div class="status-card" id="decl-git-status">
                            <div class="info-item">"Repository: " <code>{s.url.clone().unwrap_or_default()}</code> " (" {s.branch.clone()} ", file " <code>{s.path.clone()}</code> ")"</div>
                            <div class="info-item">"Mode: " <strong>{s.mode.clone()}</strong> ", every " {s.interval_secs} " s, signatures "
                                {if s.verify_signatures { "verified" } else { "not checked" }}</div>
                            <div class="info-item" id="decl-git-commit">"Last commit: "
                                {s.last_commit.clone().map(|c| view! { <code>{short(&c)}</code> })}
                                " " {s.last_commit_subject.clone()}
                                {s.last_commit_author.clone().map(|a| format!(" by {a}"))}
                                " · signature " <span class=format!("sig sig-{sig}")>{sig.clone()}</span>
                                {s.signer.clone().map(|x| format!(" ({x})"))}</div>
                            <div class="info-item">"Last sync: " {s.last_sync.clone().unwrap_or_else(|| "never".into())}
                                " · " {s.last_result.clone()}</div>
                            {s.last_error.clone().map(|e| view! { <div class="alert alert-error" id="decl-git-error">{e}</div> })}
                            <div class="form-actions">
                                <button type="button" class="btn btn-primary" id="decl-sync" disabled=move || busy.get() on:click=sync>"Sync now"</button>
                                <a class="btn btn-secondary" href="/declarative?tab=history">"Apply history"</a>
                            </div>
                        </div>
                        <h3>"Pending approval"</h3>
                        {if s.pending.is_empty() {
                            view! { <p class="no-data" id="decl-pending-empty">
                                {if s.mode.as_deref() == Some("auto") { "Auto mode applies changes straight away: nothing waits for approval." } else { "Nothing waits for approval. A new commit (or drift from the repository) shows up here." }}
                            </p> }.into_view()
                        } else {
                            s.pending.iter().map(|p| {
                                let (h1, h2) = (p.hash.clone(), p.hash.clone());
                                view! {
                                    <div class="pending-card" id=format!("decl-pending-{}", p.hash)>
                                        <p><strong>{p.subject.clone()}</strong> " · commit " <code>{short(&p.commit)}</code> " · queued " {p.created_at.clone()}</p>
                                        <PlanView plan=p.plan.clone()/>
                                        <div class="form-actions">
                                            <button type="button" class="btn btn-primary decl-approve" disabled=move || busy.get() on:click=move |_| approve(h1.clone())>"Approve and apply"</button>
                                            <button type="button" class="btn btn-secondary decl-reject" on:click=move |_| reject(h2.clone())>"Reject"</button>
                                        </div>
                                    </div>
                                }
                            }).collect_view().into_view()
                        }}
                    }.into_view()
                }
            })}
            {move || report.get().map(|r| view! { <div id="decl-approve-result"><h3>"Result"</h3><ReportView report=r/></div> })}

            <h3 id="decl-git-form">"Repository"</h3>
            <form class="git-form" on:submit=save>
                <div class="form-row">
                    <label>"Repository URL"<input id="git-url" type="text" required placeholder="https://git.example.com/ops/nodes.git" prop:value=move || url.get() on:input=move |ev| url.set(event_target_value(&ev))/></label>
                    <label>"Branch"<input id="git-branch" type="text" prop:value=move || branch.get() on:input=move |ev| branch.set(event_target_value(&ev))/></label>
                </div>
                <div class="form-row">
                    <label>"Spec file in the repo"<input id="git-path" type="text" prop:value=move || path.get() on:input=move |ev| path.set(event_target_value(&ev))/></label>
                    <label>"Sync every (seconds)"<input id="git-interval" type="number" min="10" prop:value=move || interval.get() on:input=move |ev| interval.set(event_target_value(&ev))/></label>
                    <label>"When the repository changes"
                        <select id="git-mode" prop:value=move || mode.get() on:change=move |ev| mode.set(event_target_value(&ev))>
                            <option value="approve">"Ask me first (approve)"</option>
                            <option value="auto">"Apply automatically"</option>
                        </select></label>
                </div>
                <p class="form-help">"Several nodes can share one repository: put " <code>"{node}"</code> " in the file name, e.g. " <code>"nodes/{node}.yaml"</code> ", and each node reads its own file."</p>
                <label class="checkbox-label"><input id="git-verify" type="checkbox" prop:checked=move || verify.get() on:change=move |ev| verify.set(event_target_checked(&ev))/>" Only accept commits signed by trusted keys"</label>
                {move || verify.get().then(|| view! {
                    <div class="form-row">
                        <label>"Signature type"
                            <select id="git-sigfmt" prop:value=move || sig_format.get() on:change=move |ev| sig_format.set(event_target_value(&ev))>
                                <option value="ssh">"SSH (allowed_signers lines)"</option>
                                <option value="gpg">"OpenPGP (armored public keys)"</option>
                            </select></label>
                        <label style="flex: 3">"Trusted signers (the keys, or a file path on this node)"
                            <textarea id="git-signers" rows="3" spellcheck="false" prop:value=move || signers.get() on:input=move |ev| signers.set(event_target_value(&ev))></textarea></label>
                    </div>
                })}
                <details>
                    <summary>"Private repository and webhook (secrets are given as references, never typed in)"</summary>
                    <p class="form-help">"A reference is " <code>"file:/etc/horcrux/git-token"</code> ", " <code>"env:GIT_TOKEN"</code> " or " <code>"vault:secret/path#key"</code> "."</p>
                    <div class="form-row">
                        <label>"HTTPS user"<input id="git-user" type="text" prop:value=move || cred_user.get() on:input=move |ev| cred_user.set(event_target_value(&ev))/></label>
                        <label>"HTTPS token reference"<input id="git-cred" type="text" prop:value=move || cred_ref.get() on:input=move |ev| cred_ref.set(event_target_value(&ev))/></label>
                        <label>"SSH key reference"<input id="git-ssh" type="text" prop:value=move || ssh_ref.get() on:input=move |ev| ssh_ref.set(event_target_value(&ev))/></label>
                    </div>
                    <div class="form-row">
                        <label>"Webhook secret reference"<input id="git-hook" type="text" prop:value=move || hook_ref.get() on:input=move |ev| hook_ref.set(event_target_value(&ev))/></label>
                        <label>"Node name for {node}"<input id="git-node" type="text" placeholder="(this host's name)" prop:value=move || node_name.get() on:input=move |ev| node_name.set(event_target_value(&ev))/></label>
                    </div>
                    <p class="form-help">"Push webhook: send a JSON push event signed with HMAC-SHA256 to " <code>"/api/declarative/gitops/webhook"</code> " (GitHub, Forgejo and Gitea send it that way). It only wakes the sync; the usual checks still apply."</p>
                </details>
                <div class="form-actions">
                    <button type="submit" class="btn btn-primary" id="git-save">"Save"</button>
                    {move || status.get().filter(|s| s.configured).map(|_| view! {
                        <button type="button" class="btn btn-danger" id="git-remove" on:click=remove>"Stop following the repository"</button>
                    })}
                </div>
            </form>
        </section>
    }
}

// ---------------------------------------------------------------- history

#[component]
fn HistoryTab() -> impl IntoView {
    let query = leptos_router::use_query_map();
    let selected = move || query.get().get("id").cloned();
    let (list, set_list) = create_signal(None::<d::HistoryList>);
    let (entry, set_entry) = create_signal(None::<d::HistoryEntry>);
    let (preview, set_preview) = create_signal(None::<d::RollbackResult>);
    let (done, set_done) = create_signal(None::<d::RollbackResult>);
    let (notice, set_notice) = create_signal(None::<String>);
    let (error, set_error) = create_signal(None::<String>);
    let (busy, set_busy) = create_signal(false);
    let also_desired = create_rw_signal(false);

    let load = move || {
        spawn_local(async move {
            match d::history(200).await {
                Ok(l) => {
                    set_list.set(Some(l));
                    set_error.set(None);
                }
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };
    create_effect(move |_| load());
    create_effect(move |_| {
        set_preview.set(None);
        set_done.set(None);
        match selected() {
            Some(id) => {
                spawn_local(async move {
                    match d::history_entry(&id).await {
                        Ok(e) => {
                            set_entry.set(Some(e));
                            set_error.set(None);
                        }
                        Err(e) => {
                            set_entry.set(None);
                            set_error.set(Some(e.message));
                        }
                    }
                });
            }
            None => set_entry.set(None),
        }
    });

    let rollback = move |id: String, dry: bool| {
        if !dry
            && !confirm("Roll back this apply? Horcrux restores the previous spec and removes only what this apply created.")
        {
            return;
        }
        set_busy.set(true);
        set_notice.set(None);
        let keep = also_desired.get();
        spawn_local(async move {
            match d::rollback(&id, dry, keep).await {
                Ok(r) => {
                    set_error.set(None);
                    if dry {
                        set_preview.set(Some(r));
                    } else {
                        set_preview.set(None);
                        set_done.set(Some(r));
                        load();
                    }
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            set_busy.set(false);
        });
    };

    view! {
        <section class="history-tab" id="decl-history">
            <Banner notice error/>
            {move || match list.get() {
                None => view! { <p class="loading">"Loading..."</p> }.into_view(),
                Some(l) if l.items.is_empty() => view! {
                    <div class="empty-state" id="decl-history-empty">
                        <h2 class="empty-title">"No applies recorded yet"</h2>
                        <p class="empty-todo">"Every apply (from the API, the CLI, GitOps or a rollback) is kept here with its spec, plan and results. Apply the desired spec to create the first record."</p>
                        <div class="empty-actions">
                            <a class="btn btn-primary" href="/declarative?tab=drift">"Go to Plan & drift"</a>
                        </div>
                    </div>
                }.into_view(),
                Some(l) => view! {
                    <p class="form-help">{format!("{} apply record(s) kept, newest first.", l.total)}</p>
                    <div class="table-container">
                        <table class="data-table history-table">
                            <thead><tr><th>"When"</th><th>"Who"</th><th>"Source"</th><th>"Result"</th><th>"Summary"</th></tr></thead>
                            <tbody>
                                {l.items.into_iter().map(|i| {
                                    let href = format!("/declarative?tab=history&id={}", urlencoding::encode(&i.id));
                                    let sel = selected().as_deref() == Some(i.id.as_str());
                                    let ok = i.ok;
                                    view! {
                                        <tr class="history-row" class:selected=sel>
                                            <td><a href=href>{i.time.clone()}</a></td>
                                            <td>{i.actor.clone()}</td>
                                            <td>{i.source.clone()}{i.commit.clone().map(|c| format!(" {}", short(&c)))}</td>
                                            <td><span class:drift-blocked=!ok>{if ok { "ok" } else { "failed" }}</span></td>
                                            <td>{i.summary.clone()}</td>
                                        </tr>
                                    }
                                }).collect_view()}
                            </tbody>
                        </table>
                    </div>
                }.into_view(),
            }}
            {move || entry.get().map(|e| {
                let (id1, id2) = (e.id.clone(), e.id.clone());
                let plan = e.plan.clone();
                let results = e.results.clone();
                view! {
                    <div class="history-detail" id="decl-history-detail">
                        <h3>{format!("Apply {}", e.id)}</h3>
                        <p>{e.time.clone()} " by " {e.actor.clone()} " via " {e.source.clone()}
                            {e.commit.clone().map(|c| format!(", commit {}", short(&c)))}
                            {e.rollback_of.clone().map(|r| view! { ", rollback of " <a href=format!("/declarative?tab=history&id={}", urlencoding::encode(&r))>{r.clone()}</a> })}
                            ". " {e.summary.clone()}</p>
                        <h4>"Plan and results"</h4>
                        <PlanView plan=plan results=results/>
                        <details>
                            <summary>"The spec that was applied"</summary>
                            <pre class="spec-view">{e.spec.clone()}</pre>
                        </details>
                        <h4>"Roll back"</h4>
                        <p class="form-help">"Restores the spec applied before this one and removes only what this apply created. Preview first."</p>
                        <label class="checkbox-label"><input type="checkbox" id="decl-rb-desired" prop:checked=move || also_desired.get()
                            on:change=move |ev| also_desired.set(event_target_checked(&ev))/>" Also make the restored spec the desired spec"</label>
                        <div class="form-actions">
                            <button type="button" class="btn btn-secondary" id="decl-rb-preview" disabled=move || busy.get() on:click=move |_| rollback(id1.clone(), true)>"Preview roll back"</button>
                            <button type="button" class="btn btn-primary" id="decl-rb-apply" disabled=move || busy.get() on:click=move |_| rollback(id2.clone(), false)>"Roll back"</button>
                        </div>
                    </div>
                }
            })}
            {move || preview.get().map(|r| view! {
                <div id="decl-rb-preview-result">
                    <h4>{format!("Preview: restore the spec of {}", r.restored_from)}</h4>
                    <PlanView plan=r.report.plan.clone()/>
                </div>
            })}
            {move || done.get().map(|r| view! {
                <div id="decl-rb-result">
                    <h4>{format!("Rolled back using the spec of {}", r.restored_from)}</h4>
                    <ReportView report=r.report.clone()/>
                </div>
            })}
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_diff_marks_changes() {
        let d = line_diff("a\nb\nc", "a\nx\nc");
        let s: String = d.iter().map(|(c, l)| format!("{c}{l};")).collect();
        assert_eq!(s, " a;-b;+x; c;");
        assert!(line_diff("a", "a").iter().all(|(c, _)| *c == ' '));
        assert_eq!(line_diff("", "a\nb").len(), 2);
    }

    #[test]
    fn resource_links_exist_for_builtin_kinds() {
        assert_eq!(
            resource_href("vm", "web 1").as_deref(),
            Some("/vms/web%201")
        );
        assert_eq!(
            resource_href("subnet", "x").as_deref(),
            Some("/network/sdn")
        );
        assert_eq!(resource_href("container", "x"), None);
    }
}
