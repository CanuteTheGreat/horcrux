//! Security events: one dashboard over the API access log, the audit log,
//! failed logins and lockouts, baseline drift and scan results, with
//! click-through filters, the temporary blocks, and the alert rules'
//! results. API: horcrux-api/src/api/security/activity.rs; settings live on
//! Security > Settings.
use super::security::{ask, when, NavLink, SecurityTabs};
use crate::api::*;
use leptos::*;
use leptos_router::*;

fn enc(s: &str) -> String {
    js_sys::encode_uri_component(s)
        .as_string()
        .unwrap_or_default()
}

fn now_secs() -> i64 {
    (js_sys::Date::now() / 1000.0) as i64
}

/// A link to the same page with these filters (hours is kept by the caller).
fn events_href(hours: u32, kind: &str, client: &str, user: &str, q: &str) -> String {
    let mut p = vec![format!("hours={hours}")];
    for (k, v) in [("kind", kind), ("client", client), ("user", user), ("q", q)] {
        if !v.is_empty() {
            p.push(format!("{k}={}", enc(v)));
        }
    }
    format!("/security/activity?{}", p.join("&"))
}

fn kind_label(k: &str) -> &'static str {
    match k {
        "login_failed" => "failed login",
        "lockout" => "lockout",
        "access_denied" => "access denied",
        "rate_limited" => "rate limited",
        "not_found" => "unknown path",
        "server_error" => "server error",
        "permission_denied" => "permission denied",
        "auth_change" => "account change",
        "policy_change" => "policy change",
        "brute_force" => "brute force",
        "suspicious" => "suspicious",
        "rule_fired" => "alert rule",
        "blocked" => "blocked",
        "unblocked" => "unblocked",
        "hardening_regression" => "baseline drift",
        "scan_finding" => "scan finding",
        _ => "event",
    }
}

fn sev_class(s: &str) -> &'static str {
    match s {
        "critical" => "sev-badge sev-critical",
        "warning" => "sev-badge sev-medium",
        _ => "sev-badge sev-low",
    }
}

/// Stacked bars: failed logins, denied requests, everything else.
#[component]
fn Timeline(buckets: Vec<EventBucket>, bucket_secs: i64) -> impl IntoView {
    let first = buckets.first().map(|b| b.start);
    let last = buckets.last().map(|b| b.start + bucket_secs);
    let peak = buckets
        .iter()
        .map(|b| b.failed_logins + b.access_denied + b.other)
        .max()
        .unwrap_or(0);
    let max = buckets
        .iter()
        .map(|b| b.failed_logins + b.access_denied + b.other)
        .max()
        .unwrap_or(0)
        .max(1) as f64;
    let n = buckets.len().max(1);
    let total: u32 = buckets
        .iter()
        .map(|b| b.failed_logins + b.access_denied + b.other)
        .sum();
    let bars = buckets
        .into_iter()
        .enumerate()
        .map(|(i, b)| {
            let x = i as f64 * 10.0;
            let h = |v: u32| v as f64 / max * 90.0;
            let (hl, hd, ho) = (h(b.failed_logins), h(b.access_denied), h(b.other));
            let tip = format!(
                "{}: {} failed logins, {} denied requests, {} other",
                when(Some(b.start)),
                b.failed_logins,
                b.access_denied,
                b.other
            );
            view! {
                <g>
                    <title>{tip}</title>
                    <rect class="tl-other" x=x + 1.0 y=95.0 - hl - hd - ho width=8.0 height=ho/>
                    <rect class="tl-denied" x=x + 1.0 y=95.0 - hl - hd width=8.0 height=hd/>
                    <rect class="tl-login" x=x + 1.0 y=95.0 - hl width=8.0 height=hl/>
                </g>
            }
        })
        .collect_view();
    let label = format!("Security events over time, {total} in total");
    view! {
        <div class="card events-timeline">
            <h2>"Over time"</h2>
            <svg viewBox=format!("0 0 {} 100", n * 10) preserveAspectRatio="none" role="img" aria-label=label class="events-chart">
                <line x1="0" y1="95" x2=n * 10 y2="95" class="tl-axis"/>
                {bars}
            </svg>
            <div class="events-axis"><span>{format!("from {}", when(first))}</span><span>{format!("peak {peak} per {} min", bucket_secs / 60)}</span><span>{format!("to {}", when(last))}</span></div>
            <div class="events-legend">
                <span><i class="tl-key tl-login"></i>"failed logins"</span>
                <span><i class="tl-key tl-denied"></i>"denied requests"</span>
                <span><i class="tl-key tl-other"></i>"other security events"</span>
            </div>
            {(total == 0).then(|| view! { <p class="dash-meta">"Nothing in this time range."</p> })}
        </div>
    }
}

#[component]
pub fn SecurityActivityPage() -> impl IntoView {
    let query = use_query_map();
    let get = move |k: &'static str| query.with(|q| q.get(k).cloned().unwrap_or_default());
    let hours = move || {
        query.with(|q| {
            q.get("hours")
                .and_then(|h| h.parse::<u32>().ok())
                .filter(|h| (1..=720).contains(h))
                .unwrap_or(24)
        })
    };
    let (tick, set_tick) = create_signal(0u32);
    let (limit, set_limit) = create_signal(50usize);
    let (err, set_err) = create_signal(None::<String>);
    let (ok, set_ok) = create_signal(None::<String>);
    let dash = create_resource(
        move || (hours(), tick.get()),
        |(h, _)| async move { get_activity_dashboard(h).await },
    );
    let blocks = create_resource(
        move || tick.get(),
        |_| async move { list_blocked_clients().await },
    );
    let events = create_resource(
        move || {
            (
                hours(),
                get("kind"),
                get("client"),
                get("user"),
                get("q"),
                limit.get(),
                tick.get(),
            )
        },
        |(h, kind, client, user, q, limit, _)| async move {
            let mut qs = vec![
                format!("since={}", now_secs() - h as i64 * 3600),
                format!("limit={limit}"),
            ];
            for (k, v) in [
                ("kind", &kind),
                ("client", &client),
                ("user", &user),
                ("q", &q),
            ] {
                if !v.is_empty() {
                    qs.push(format!("{k}={}", enc(v)));
                }
            }
            list_activity_events(&qs.join("&")).await
        },
    );
    let poll = set_interval_with_handle(
        move || set_tick.update(|t| *t += 1),
        std::time::Duration::from_secs(30),
    )
    .ok();
    on_cleanup(move || {
        if let Some(p) = poll {
            p.clear();
        }
    });
    let navigate = use_navigate();
    let nav2 = navigate.clone();
    let act = create_action(move |(ip, block): &(String, bool)| {
        let (ip, block) = (ip.clone(), *block);
        async move {
            set_err.set(None);
            set_ok.set(None);
            let r = if block {
                block_client(&ip, 0, "blocked from the security events page")
                    .await
                    .map(|b| format!("{} is blocked until {}.", b.ip, when(Some(b.until))))
            } else {
                unblock_client(&ip)
                    .await
                    .map(|_| format!("{ip} is unblocked."))
            };
            match r {
                Ok(m) => set_ok.set(Some(m)),
                Err(e) => match e.unavailable() {
                    Some(u) => set_err.set(Some(format!("{} {}", u.what, u.todo))),
                    None => set_err.set(Some(format!("Not changed: {e}"))),
                },
            }
            set_tick.update(|t| *t += 1);
        }
    });
    let range_link = move |h: u32, label: &'static str| {
        let href = events_href(h, &get("kind"), &get("client"), &get("user"), &get("q"));
        let on = Signal::derive(move || hours() == h);
        view! { <NavLink href=href class=Signal::derive(move || if on.get() { "filter-link active".to_string() } else { "filter-link".to_string() }) active=on>{label}</NavLink> }
    };
    view! {
        <div class="security-page security-events">
            <div class="page-header">
                <h1>"Security Activity"</h1>
                <div class="header-actions">
                    <button class="btn btn-secondary" on:click=move |_| set_tick.update(|t| *t += 1)>"Refresh"</button>
                </div>
            </div>
            <SecurityTabs active="activity"/>
            <p class="help-text">
                "Failed logins and lockouts, denied and failing API calls, permission denials, account and policy changes, "
                "brute-force detections, baseline drift and new scan findings on this node, in one place. "
                "Alert rules and brute-force detection are set in "<A href="/security/settings">"Security > Settings"</A>
                "; their alerts go out as security notifications."
            </p>
            <div class="filter-links" role="group" aria-label="Time range">
                {range_link(1, "Last hour")}{range_link(24, "24 hours")}{range_link(168, "7 days")}{range_link(720, "30 days")}
            </div>
            {move || err.get().map(|m| view! { <div class="alert alert-error">{m}</div> })}
            {move || ok.get().map(|m| view! { <div class="alert alert-success">{m}</div> })}
            {move || dash.get().map(|r| match r {
                Err(e) => view! { <div class="alert alert-error">{format!("Could not load the security events: {e}")}</div> }.into_view(),
                Ok(d) => {
                    let h = hours();
                    let t = d.totals.clone();
                    let top_clients = d.top_clients.clone();
                    let top_users = d.top_users.clone();
                    let card = |href: String, label: &'static str, n: u32, hot: bool, note: String| {
                        let cls = if hot && n > 0 { "stat-card stat-link warning attention" } else { "stat-card stat-link" };
                        view! {
                            <A href=href class=cls>
                                <span class="stat-label">{label}</span>
                                <div class="stat-value">{n}</div>
                                <div class="dash-meta">{note}</div>
                            </A>
                        }
                    };
                    view! {
                        {(!d.detector_enabled).then(|| view! { <div class="alert alert-warning">"The detector is turned off, so alert rules and brute-force detection are not running. "<A href="/security/settings">"Security settings"</A></div> })}
                        {(!d.access_log_enabled).then(|| view! { <div class="alert alert-warning">"The API access log is off, so denied and failing API calls are not counted."</div> })}
                        <div class="stats-grid">
                            {card(events_href(h, "login_failed,lockout", "", "", ""), "Failed logins", t.failed_logins + t.lockouts, true, format!("{} locked out", t.lockouts))}
                            {card(events_href(h, "access_denied,rate_limited", "", "", ""), "Denied API requests", t.access_denied, true, "401, 403 and 429 answers".into())}
                            {card(events_href(h, "permission_denied", "", "", ""), "Permission denials", t.permission_denied, false, "users refused by access control".into())}
                            {card(events_href(h, "brute_force,rule_fired", "", "", ""), "Detections", t.brute_force, true, "brute force and alert rules".into())}
                            {card(events_href(h, "blocked,unblocked", "", "", ""), "Blocked clients", t.blocks_active, false, if d.block_enabled { "automatic blocking is on".into() } else { "automatic blocking is off".into() })}
                            {card("/security/hardening".to_string(), "Baseline drift", t.baseline_drift, true, "rules that started failing".into())}
                            {card(events_href(h, "scan_finding", "", "", ""), "Scan findings", t.scan_findings, true, "scans with critical or high".into())}
                        </div>
                        <Timeline buckets=d.timeline.clone() bucket_secs=d.bucket_secs/>
                        <div class="events-tops">
                            <div class="card">
                                <h2>"Top failing clients"</h2>
                                {if top_clients.is_empty() {
                                    view! { <p class="empty-state">"No failing clients in this range."</p> }.into_view()
                                } else {
                                    let fw = d.firewall_available;
                                    view! {
                                        <table class="data-table">
                                            <thead><tr><th>"Client"</th><th>"Logins"</th><th>"Denied"</th><th>"Last"</th><th>"Actions"</th></tr></thead>
                                            <tbody>
                                            {top_clients.into_iter().map(|c| {
                                                let ip = c.key.clone();
                                                let ip2 = c.key.clone();
                                                let blocked = c.blocked;
                                                let label = if blocked { format!("Unblock {}", c.key) } else { format!("Block {}", c.key) };
                                                view! {
                                                    <tr>
                                                        <td class="nowrap"><A href=events_href(h, "", &c.key, "", "") class="dash-link mono">{c.key.clone()}</A>{blocked.then(|| view! { <span class="sev-badge sev-high">"blocked"</span> })}</td>
                                                        <td>{c.failed_logins}</td><td>{c.access_denied}</td>
                                                        <td class="nowrap">{when(Some(c.last_seen))}</td>
                                                        <td class="actions">
                                                            {(!fw && !blocked).then(|| view! { <span class="dash-meta">"needs nftables"</span> })}
                                                            {(fw || blocked).then(|| view! {
                                                                <button class="btn btn-xs" aria-label=label.clone() title=label.clone()
                                                                    on:click=move |_| {
                                                                        if blocked { act.dispatch((ip.clone(), false)) }
                                                                        else if ask(&format!("Block {} in this node's firewall? Its traffic to this node is dropped for the configured time (default 30 minutes); you can unblock it here.", ip2)) { act.dispatch((ip2.clone(), true)) }
                                                                    }>{if blocked { "Unblock" } else { "Block" }}</button>
                                                            })}
                                                        </td>
                                                    </tr>
                                                }
                                            }).collect_view()}
                                            </tbody>
                                        </table>
                                    }.into_view()
                                }}
                            </div>
                            <div class="card">
                                <h2>"Top failing users"</h2>
                                {if top_users.is_empty() {
                                    view! { <p class="empty-state">"No failing users in this range."</p> }.into_view()
                                } else {
                                    view! {
                                        <table class="data-table">
                                            <thead><tr><th>"User"</th><th>"Logins"</th><th>"Denied"</th><th>"Last"</th></tr></thead>
                                            <tbody>
                                            {top_users.into_iter().map(|u| view! {
                                                <tr>
                                                    <td><A href=events_href(h, "", "", &u.key, "") class="dash-link">{u.key.clone()}</A></td>
                                                    <td>{u.failed_logins}</td><td>{u.access_denied}</td><td class="nowrap">{when(Some(u.last_seen))}</td>
                                                </tr>
                                            }).collect_view()}
                                            </tbody>
                                        </table>
                                    }.into_view()
                                }}
                            </div>
                        </div>
                    }.into_view()
                }
            })}
            <div class="card" id="blocks">
                <h2>"Blocked clients"</h2>
                {move || blocks.get().map(|r| match r {
                    Err(e) => view! { <div class="alert alert-error">{format!("Could not list blocks: {e}")}</div> }.into_view(),
                    Ok(v) if v.is_empty() => view! { <p class="empty-state">"No client is blocked. Automatic blocking after repeated failed logins is off by default: turn it on in Security > Settings."</p> }.into_view(),
                    Ok(v) => view! {
                        <table class="data-table">
                            <thead><tr><th>"Client"</th><th>"Blocked until"</th><th>"Why"</th><th>"By"</th><th>"Actions"</th></tr></thead>
                            <tbody>
                            {v.into_iter().map(|b| {
                                let ip = b.ip.clone();
                                let label = format!("Unblock {}", b.ip);
                                view! {
                                    <tr>
                                        <td class="mono">{b.ip.clone()}</td><td>{when(Some(b.until))}</td>
                                        <td>{b.reason.clone()}</td><td>{b.by.clone()}</td>
                                        <td class="actions"><button class="btn btn-xs" aria-label=label.clone() title=label on:click=move |_| act.dispatch((ip.clone(), false))>"Unblock"</button></td>
                                    </tr>
                                }
                            }).collect_view()}
                            </tbody>
                        </table>
                    }.into_view(),
                })}
            </div>
            <div class="card">
                <h2>"Events"</h2>
                <form class="events-filter" on:submit=move |e| {
                    e.prevent_default();
                    let val = |id: &str| -> String {
                        web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id(id))
                            .and_then(|el| js_sys::Reflect::get(&el, &"value".into()).ok()).and_then(|v| v.as_string()).unwrap_or_default()
                    };
                    nav2(&events_href(hours(), &val("ev-kind"), val("ev-client").trim(), val("ev-user").trim(), val("ev-q").trim()), Default::default());
                }>
                    <div class="form-row">
                        <div class="form-group">
                            <label for="ev-kind">"Kind"</label>
                            <select id="ev-kind">
                                <option value="" selected=move || get("kind").is_empty()>"All"</option>
                                {EVENT_KINDS.iter().chain(["hardening_regression", "scan_finding"].iter()).map(|k| {
                                    let k = *k;
                                    view! { <option value=k selected=move || get("kind") == k>{kind_label(k)}</option> }
                                }).collect_view()}
                            </select>
                        </div>
                        <div class="form-group"><label for="ev-client">"Client"</label><input id="ev-client" type="text" placeholder="IP address" prop:value=move || get("client")/></div>
                        <div class="form-group"><label for="ev-user">"User"</label><input id="ev-user" type="text" prop:value=move || get("user")/></div>
                        <div class="form-group"><label for="ev-q">"Contains"</label><input id="ev-q" type="text" prop:value=move || get("q")/></div>
                        <div class="form-group form-actions"><button type="submit" class="btn btn-primary">"Filter"</button>
                            <A href=events_href(hours(), "", "", "", "") class="btn btn-secondary">"Clear"</A></div>
                    </div>
                </form>
                {move || events.get().map(|r| match r {
                    Err(e) => view! { <div class="alert alert-error">{format!("Could not load events: {e}")}</div> }.into_view(),
                    Ok(p) if p.events.is_empty() => view! { <p class="empty-state">"No events match in this time range."</p> }.into_view(),
                    Ok(p) => {
                        let h = hours();
                        let more = p.total > p.events.len();
                        let shown = p.events.len();
                        view! {
                            <div class="table-container">
                            <table class="data-table events-table">
                                <thead><tr><th>"When"</th><th>"Kind"</th><th>"Client"</th><th>"User"</th><th>"What"</th></tr></thead>
                                <tbody>
                                {p.events.into_iter().map(|e| view! {
                                    <tr>
                                        <td class="nowrap">{when(Some(e.ts))}</td>
                                        <td><A href=events_href(h, &e.kind, "", "", "") class=sev_class(&e.severity)>{kind_label(&e.kind)}</A></td>
                                        <td class="mono nowrap">{e.client.clone().map(|c| view! { <A href=events_href(h, "", &c, "", "") class="dash-link">{c.clone()}</A> })}</td>
                                        <td>{e.user.clone().map(|u| view! { <A href=events_href(h, "", "", &u, "") class="dash-link">{u.clone()}</A> })}</td>
                                        <td>{e.summary.clone()}{e.detail.clone().map(|d| view! { <div class="dash-meta">{d}</div> })}</td>
                                    </tr>
                                }).collect_view()}
                                </tbody>
                            </table>
                            </div>
                            <p class="dash-meta">{format!("Showing {shown} of {}.", p.total)}
                                {more.then(|| view! { " " <button class="btn btn-xs" on:click=move |_| set_limit.update(|l| *l += 100)>"Show more"</button> })}</p>
                        }.into_view()
                    }
                })}
            </div>
        </div>
    }
}
