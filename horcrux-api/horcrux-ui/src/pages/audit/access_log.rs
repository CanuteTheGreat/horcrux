//! API access log (`/access-log`, Datacenter > Access Log): who called the
//! API, from where, which requests failed, over an exact time range, with
//! instant statistics. The whole filter lives in the URL, so every value in
//! the statistics panel (and every user, client, token, status and route in
//! the table) is a real link that applies that filter, and a filtered view
//! can be bookmarked or shared.
use crate::api::*;
use crate::components::UnavailableState;
use leptos::*;
use leptos_router::{use_navigate, use_query_map};
use std::collections::BTreeMap;

/// Filter keys that go to the API as they are.
pub const FILTER_KEYS: &[&str] = &[
    "start",
    "end",
    "user",
    "token",
    "client",
    "auth_method",
    "method",
    "path",
    "group",
    "status",
    "node",
];
/// Keys the page keeps in its URL besides the filter.
const PAGE_KEYS: &[&str] = &["last", "offset", "detail", "dnode"];
const PAGE_SIZE: u32 = 50;
/// Range shown when the URL names none, seconds.
const DEFAULT_LAST_SECS: i64 = 3600;

pub type Params = BTreeMap<String, String>;

/// `/access-log?a=b&c=d` for `params` with `set` applied and `drop` removed.
/// Moving to another page of results or to an entry is never carried along
/// when the filter changes.
pub fn href(params: &Params, set: &[(&str, &str)], drop: &[&str]) -> String {
    let mut p = params.clone();
    let filter_changes = set
        .iter()
        .any(|(k, _)| FILTER_KEYS.contains(k) || *k == "last");
    if filter_changes {
        p.remove("offset");
        p.remove("detail");
        p.remove("dnode");
    }
    for k in drop {
        p.remove(*k);
    }
    for (k, v) in set {
        if v.is_empty() {
            p.remove(*k);
        } else {
            p.insert((*k).to_string(), (*v).to_string());
        }
    }
    let q: Vec<String> = p
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| format!("{k}={}", urlencoding::encode(v)))
        .collect();
    if q.is_empty() {
        "/access-log".into()
    } else {
        format!("/access-log?{}", q.join("&"))
    }
}

/// The query string for the API: the filter keys, `last` turned into a start
/// time, plus paging.
pub fn api_query(params: &Params, now_ms: i64, limit: u32, offset: u32) -> String {
    let mut p: Params = params
        .iter()
        .filter(|(k, v)| FILTER_KEYS.contains(&k.as_str()) && !v.is_empty())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if !p.contains_key("start") && !p.contains_key("end") {
        let secs = params
            .get("last")
            .and_then(|l| l.parse::<i64>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(DEFAULT_LAST_SECS);
        p.insert("start".into(), (now_ms / 1000 - secs).to_string());
    }
    let mut q: Vec<String> = p
        .iter()
        .map(|(k, v)| format!("{k}={}", urlencoding::encode(v)))
        .collect();
    q.push(format!("limit={limit}"));
    if offset > 0 {
        q.push(format!("offset={offset}"));
    }
    q.join("&")
}

/// `2026-11-25T14:03:07.123Z` -> `2026-11-25 14:03:07.123`.
pub fn fmt_time(t: &str) -> String {
    t.trim_end_matches('Z').replace('T', " ")
}

fn status_class(s: u16) -> &'static str {
    match s {
        200..=299 => "al-s2",
        300..=399 => "al-s3",
        400..=499 => "al-s4",
        500..=599 => "al-s5",
        _ => "al-s1",
    }
}

fn fmt_bytes(n: i64) -> String {
    match n {
        n if n < 1024 => format!("{n} B"),
        n if n < 1024 * 1024 => format!("{:.1} KiB", n as f64 / 1024.0),
        n => format!("{:.1} MiB", n as f64 / 1048576.0),
    }
}

fn read_params(q: &leptos_router::ParamsMap) -> Params {
    FILTER_KEYS
        .iter()
        .chain(PAGE_KEYS.iter())
        .filter_map(|k| {
            q.get(k)
                .filter(|v| !v.is_empty())
                .map(|v| ((*k).to_string(), v.clone()))
        })
        .collect()
}

/// A statistics list whose every value is a link applying that filter.
fn top_list(
    title: &'static str,
    key: &'static str,
    items: Vec<AccessTop>,
    params: Params,
) -> impl IntoView {
    view! {
        <div class="al-top" data-top=key>
            <h3>{title}</h3>
            {if items.is_empty() {
                view! { <p class="al-muted">"none"</p> }.into_view()
            } else {
                view! {
                    <ul>
                        {items.into_iter().map(|t| {
                            let h = href(&params, &[(key, t.value.as_str())], &[]);
                            let name = match &t.label {
                                Some(l) if !l.is_empty() => format!("{l} ({})", t.value),
                                _ => t.value.clone(),
                            };
                            view! { <li><a href=h>{name}</a><span class="al-count">{t.count}</span></li> }
                        }).collect_view()}
                    </ul>
                }.into_view()
            }}
        </div>
    }
}

#[component]
pub fn AccessLogPage() -> impl IntoView {
    let query = use_query_map();
    let navigate = use_navigate();
    let params = Signal::derive(move || query.with(read_params));

    let (page, set_page) = create_signal(None::<AccessPage>);
    let (stats, set_stats) = create_signal(None::<AccessStats>);
    let (error, set_error) = create_signal(None::<String>);
    let (unavailable, set_unavailable) = create_signal(None::<Unavailable>);
    let (entry, set_entry) = create_signal(None::<AccessEntry>);
    let (loading, set_loading) = create_signal(true);
    let load_id = store_value(0u64);

    // Form fields, refreshed whenever the URL changes (a statistics link).
    let f_start = create_rw_signal(String::new());
    let f_end = create_rw_signal(String::new());
    let f_user = create_rw_signal(String::new());
    let f_token = create_rw_signal(String::new());
    let f_client = create_rw_signal(String::new());
    let f_auth = create_rw_signal(String::new());
    let f_method = create_rw_signal(String::new());
    let f_path = create_rw_signal(String::new());
    let f_group = create_rw_signal(String::new());
    let f_status = create_rw_signal(String::new());
    let f_node = create_rw_signal(String::new());
    let fields: Vec<(&'static str, RwSignal<String>)> = vec![
        ("start", f_start),
        ("end", f_end),
        ("user", f_user),
        ("token", f_token),
        ("client", f_client),
        ("auth_method", f_auth),
        ("method", f_method),
        ("path", f_path),
        ("group", f_group),
        ("status", f_status),
        ("node", f_node),
    ];

    let load_fields = fields.clone();
    create_effect(move |_| {
        let p = params.get();
        for (k, s) in &load_fields {
            s.set(p.get(*k).cloned().unwrap_or_default());
        }
    });

    let reload = move || {
        let p = params.get_untracked();
        let id = {
            load_id.update_value(|v| *v += 1);
            load_id.get_value()
        };
        set_loading.set(true);
        spawn_local(async move {
            let now = web_sys::js_sys::Date::now() as i64;
            let offset = p.get("offset").and_then(|o| o.parse().ok()).unwrap_or(0);
            let list = get_access_log(&api_query(&p, now, PAGE_SIZE, offset)).await;
            let st = get_access_stats(&api_query(&p, now, 1, 0)).await;
            // an older answer must not replace a newer one
            if load_id.get_value() != id {
                return;
            }
            set_loading.set(false);
            match (list, st) {
                (Ok(l), Ok(s)) => {
                    set_error.set(None);
                    set_unavailable.set(None);
                    set_page.set(Some(l));
                    set_stats.set(Some(s));
                }
                (Err(e), _) | (_, Err(e)) => match e.unavailable() {
                    Some(u) => set_unavailable.set(Some(u)),
                    None => {
                        set_unavailable.set(None);
                        set_error.set(Some(format!("Failed to load the access log: {e}")));
                    }
                },
            }
        });
    };
    create_effect(move |_| {
        let p = params.get();
        reload();
        match p.get("detail").and_then(|d| d.parse::<i64>().ok()) {
            Some(id) => {
                let node = p.get("dnode").cloned().unwrap_or_default();
                spawn_local(async move {
                    match get_access_entry(id, &node).await {
                        Ok(e) => set_entry.set(Some(e)),
                        Err(_) => set_entry.set(None),
                    }
                });
            }
            None => set_entry.set(None),
        }
    });

    // Apply: the form becomes the URL.
    let nav = navigate.clone();
    let apply_fields = fields.clone();
    let apply = move || {
        let mut p = params.get_untracked();
        for k in FILTER_KEYS {
            p.remove(*k);
        }
        p.remove("offset");
        p.remove("detail");
        p.remove("dnode");
        let mut set: Vec<(&str, String)> = Vec::new();
        for (k, s) in &apply_fields {
            let v = s.get_untracked().trim().to_string();
            if !v.is_empty() {
                set.push((*k, v));
            }
        }
        // an explicit range replaces the quick range
        if set.iter().any(|(k, _)| *k == "start" || *k == "end") {
            p.remove("last");
        }
        let set_ref: Vec<(&str, &str)> = set.iter().map(|(k, v)| (*k, v.as_str())).collect();
        nav(&href(&p, &set_ref, &[]), Default::default());
    };

    // Settings (collapsible).
    let (settings, set_settings) = create_signal(None::<AccessLogSettings>);
    let (settings_msg, set_settings_msg) = create_signal(None::<(bool, String)>);
    let s_enabled = create_rw_signal(true);
    let s_days = create_rw_signal(String::new());
    let s_mb = create_rw_signal(String::new());
    let s_proxies = create_rw_signal(String::new());
    let s_static = create_rw_signal(false);
    let s_exclude = create_rw_signal(String::new());
    create_effect(move |_| {
        spawn_local(async move {
            if let Ok(s) = get_access_settings().await {
                s_enabled.set(s.enabled);
                s_days.set(s.retention_days.to_string());
                s_mb.set(s.max_mb.to_string());
                s_proxies.set(s.trusted_proxies.join("\n"));
                s_static.set(s.log_static);
                s_exclude.set(s.exclude_path_prefixes.join("\n"));
                set_settings.set(Some(s));
            }
        });
    });
    let save_settings = move || {
        let Some(mut s) = settings.get_untracked() else {
            return;
        };
        let (Ok(days), Ok(mb)) = (
            s_days.get_untracked().trim().parse::<u32>(),
            s_mb.get_untracked().trim().parse::<u32>(),
        ) else {
            set_settings_msg.set(Some((
                false,
                "Retention days and size must be whole numbers.".into(),
            )));
            return;
        };
        let lines = |v: String| -> Vec<String> {
            v.lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect()
        };
        s.enabled = s_enabled.get_untracked();
        s.retention_days = days;
        s.max_mb = mb;
        s.log_static = s_static.get_untracked();
        s.trusted_proxies = lines(s_proxies.get_untracked());
        s.exclude_path_prefixes = lines(s_exclude.get_untracked());
        spawn_local(async move {
            match put_access_settings(&s).await {
                Ok(saved) => {
                    set_settings.set(Some(saved));
                    set_settings_msg.set(Some((true, "Settings saved.".into())));
                }
                Err(e) => set_settings_msg.set(Some((false, format!("Not saved: {e}")))),
            }
        });
    };

    let text_input =
        move |id: &'static str, label: &'static str, sig: RwSignal<String>, ph: &'static str| {
            view! {
                <div class="form-group">
                    <label for=id>{label}</label>
                    <input id=id type="text" placeholder=ph prop:value=sig
                        on:input=move |ev| sig.set(event_target_value(&ev))/>
                </div>
            }
        };

    let apply_a = apply.clone();
    let range_links = move || {
        let p = params.get();
        [
            ("15 minutes", "900"),
            ("1 hour", "3600"),
            ("24 hours", "86400"),
            ("7 days", "604800"),
        ]
        .into_iter()
        .map(|(label, secs)| {
            let mut q = p.clone();
            q.remove("start");
            q.remove("end");
            let h = href(&q, &[("last", secs)], &[]);
            view! { <a class="btn btn-sm btn-secondary" href=h>{label}</a> }
        })
        .collect_view()
    };

    view! {
        <div class="access-log">
            <div class="page-header">
                <h1>"Access Log"</h1>
                <div class="header-actions">
                    <button class="btn btn-secondary" on:click=move |_| reload()>"Refresh"</button>
                </div>
            </div>
            <p class="al-muted">
                "Every request to the API: who made it, from where, and how it ended. Times are UTC. "
                "Click any value to filter by it."
            </p>
            {move || unavailable.get().map(|u| view! {
                <UnavailableState info=u on_retry=Callback::new(move |_| reload())/>
            })}
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="al-error">{e}</div> })}
            <div style:display=move || if unavailable.get().is_some() { "none" } else { "" }>
                <form class="al-filter" id="al-filter" on:submit=move |ev| { ev.prevent_default(); apply_a(); }>
                    <div class="form-row">
                        {text_input("al-start", "Start (UTC)", f_start, "2026-11-25T14:00:00")}
                        {text_input("al-end", "End (UTC)", f_end, "2026-11-25T14:59:59")}
                        {text_input("al-user", "User", f_user, "any user")}
                        {text_input("al-token", "API token (id or name)", f_token, "any token")}
                        {text_input("al-client", "Client address", f_client, "any address")}
                        <div class="form-group">
                            <label for="al-auth">"Credential"</label>
                            <select id="al-auth" prop:value=f_auth on:change=move |ev| f_auth.set(event_target_value(&ev))>
                                <option value="">"any"</option>
                                <option value="jwt">"login token"</option>
                                <option value="session">"session cookie"</option>
                                <option value="api_key">"API token"</option>
                                <option value="cluster">"cluster node"</option>
                            </select>
                        </div>
                        <div class="form-group">
                            <label for="al-method">"Method"</label>
                            <select id="al-method" prop:value=f_method on:change=move |ev| f_method.set(event_target_value(&ev))>
                                <option value="">"any"</option>
                                <option value="GET">"GET"</option>
                                <option value="POST">"POST"</option>
                                <option value="PUT">"PUT"</option>
                                <option value="DELETE">"DELETE"</option>
                                <option value="PATCH">"PATCH"</option>
                            </select>
                        </div>
                        {text_input("al-path", "Path contains", f_path, "/api/vms")}
                        {text_input("al-group", "Route", f_group, "/api/vms/:id/start")}
                        {text_input("al-status", "Status (404 or 4xx)", f_status, "any status")}
                        {text_input("al-node", "Node (name or all)", f_node, "this node")}
                    </div>
                    <div class="form-row al-actions">
                        <button id="al-apply" type="submit" class="btn btn-primary">"Apply filter"</button>
                        <a id="al-clear" class="btn btn-secondary" href="/access-log">"Clear filter"</a>
                        <span class="al-ranges">"Quick range: "{range_links}</span>
                    </div>
                </form>

                {move || stats.get().map(|s| {
                    let p = params.get();
                    let max = s.buckets.iter().map(|b| b.total).max().unwrap_or(0).max(1);
                    let bars = s.buckets.iter().map(|b| {
                        let start = (b.start_ms / 1000).to_string();
                        let end = ((b.start_ms + s.bucket_ms - 1) / 1000).to_string();
                        let mut q = p.clone();
                        q.remove("last");
                        let h = href(&q, &[("start", start.as_str()), ("end", end.as_str())], &[]);
                        let title = format!("{} requests from {}", b.total, fmt_time(&iso_of(b.start_ms)));
                        let seg = |n: i64, class: &'static str| {
                            let pct = n as f64 * 100.0 / max as f64;
                            view! { <span class=class style=format!("height:{pct:.1}%")></span> }
                        };
                        view! {
                            <a class="al-bar" href=h title=title.clone() aria-label=title>
                                {seg(b.s5xx, "al-s5")}{seg(b.s4xx, "al-s4")}{seg(b.s3xx, "al-s3")}{seg(b.s2xx, "al-s2")}
                            </a>
                        }
                    }).collect_view();
                    let errors_href = href(&p, &[("status", "4xx")], &[]);
                    let server_href = href(&p, &[("status", "5xx")], &[]);
                    view! {
                        <section id="al-stats" class="al-stats">
                            <h2>"Statistics for this filter"</h2>
                            <p class="al-summary">
                                <strong id="al-total">{s.total}</strong>" requests, "
                                <a id="al-errors-link" href=errors_href>"client errors (4xx)"</a>" and "
                                <a id="al-5xx-link" href=server_href>"server errors (5xx)"</a>": "
                                <strong id="al-errors">{s.errors}</strong>" together; average "
                                {format!("{:.1} ms", s.avg_duration_ms)}", "{fmt_bytes(s.bytes_out)}" sent"
                            </p>
                            {s.nodes.iter().filter(|n| n.error.is_some()).map(|n| view! {
                                <div class="alert alert-error">{format!("Node {} did not answer: {}", n.node, n.error.clone().unwrap_or_default())}</div>
                            }).collect_view()}
                            {(s.collector.dropped > 0).then(|| view! {
                                <div class="alert alert-warning" id="al-dropped">
                                    {format!("{} requests were not recorded because the queue was full.", s.collector.dropped)}
                                </div>
                            })}
                            <div class="al-chart" role="img" aria-label="Requests over time">{bars}</div>
                            <div class="al-tops">
                                {top_list("Clients", "client", s.top_clients.clone(), p.clone())}
                                {top_list("Users", "user", s.top_users.clone(), p.clone())}
                                {top_list("API tokens", "token", s.top_tokens.clone(), p.clone())}
                                {top_list("Routes", "group", s.top_groups.clone(), p.clone())}
                                {top_list("Status codes", "status", s.top_statuses.clone(), p.clone())}
                                {top_list("Methods", "method", s.top_methods.clone(), p.clone())}
                            </div>
                        </section>
                    }
                })}

                {move || entry.get().map(|e| {
                    let close = href(&params.get(), &[], &["detail", "dnode"]);
                    let rows: Vec<(&str, String)> = vec![
                        ("Time (UTC)", fmt_time(&e.time)),
                        ("Node", e.node.clone()),
                        ("User", format!("{}{}", e.user.clone().unwrap_or_else(|| "(not authenticated)".into()), e.realm.as_ref().map(|r| format!(" @ {r}")).unwrap_or_default())),
                        ("Credential", e.auth_method.clone().unwrap_or_else(|| "none".into())),
                        ("API token", match (&e.token_name, &e.token_id) { (Some(n), Some(i)) => format!("{n} ({i})"), _ => "-".into() }),
                        ("Client", e.client.clone()),
                        ("Request", format!("{} {}", e.method, e.path)),
                        ("Route", e.path_group.clone()),
                        ("Status", e.status.to_string()),
                        ("Duration", format!("{:.2} ms", e.duration_ms)),
                        ("Bytes sent", e.bytes_out.map(fmt_bytes).unwrap_or_else(|| "streamed".into())),
                        ("User agent", e.user_agent.clone().unwrap_or_else(|| "-".into())),
                    ];
                    view! {
                        <section id="al-detail" class="al-detail">
                            <h2>{format!("Request {}", e.id)}</h2>
                            <dl>
                                {rows.into_iter().map(|(k, v)| view! { <dt>{k}</dt><dd>{v}</dd> }).collect_view()}
                            </dl>
                            <a class="btn btn-secondary" href=close>"Close"</a>
                        </section>
                    }
                })}

                <p id="al-count">{move || page.get().map(|p| format!("{} matching request{}", p.total, if p.total == 1 { "" } else { "s" })).unwrap_or_default()}</p>
                <table class="vm-table" id="al-table">
                    <thead>
                        <tr>
                            <th>"Time (UTC)"</th><th>"Node"</th><th>"User"</th><th>"Credential"</th><th>"Client"</th>
                            <th>"Method"</th><th>"Path"</th><th>"Status"</th><th>"ms"</th><th>"Details"</th>
                        </tr>
                    </thead>
                    <tbody>
                        {move || {
                            let p = params.get();
                            let Some(pg) = page.get() else {
                                return view! { <tr><td colspan="10" class="no-data">{move || if loading.get() { "Loading..." } else { "" }}</td></tr> }.into_view();
                            };
                            if pg.entries.is_empty() {
                                return view! { <tr><td colspan="10" class="no-data">"No requests match this filter."</td></tr> }.into_view();
                            }
                            pg.entries.into_iter().map(|e| {
                                let detail_href = href(&p, &[("detail", &e.id.to_string()), ("dnode", e.node.as_str())], &[]);
                                let dbl = detail_href.clone();
                                let nav = navigate.clone();
                                let link = |key: &'static str, val: Option<String>| match val {
                                    Some(v) if !v.is_empty() => {
                                        let h = href(&p, &[(key, v.as_str())], &[]);
                                        view! { <a href=h>{v}</a> }.into_view()
                                    }
                                    _ => view! { <span class="al-muted">"-"</span> }.into_view(),
                                };
                                let cred = match (&e.token_name, &e.auth_method) {
                                    (Some(n), _) => {
                                        let h = href(&p, &[("token", e.token_id.as_deref().unwrap_or(n.as_str()))], &[]);
                                        view! { <a href=h>{format!("token {n}")}</a> }.into_view()
                                    }
                                    (None, Some(m)) => {
                                        let h = href(&p, &[("auth_method", m.as_str())], &[]);
                                        view! { <a href=h>{m.clone()}</a> }.into_view()
                                    }
                                    _ => view! { <span class="al-muted">"none"</span> }.into_view(),
                                };
                                let status_href = href(&p, &[("status", &e.status.to_string())], &[]);
                                let group_href = href(&p, &[("group", e.path_group.as_str())], &[]);
                                view! {
                                    <tr class="al-row" on:dblclick=move |_| nav(&dbl, Default::default())>
                                        <td>{fmt_time(&e.time)}</td>
                                        <td>{e.node.clone()}</td>
                                        <td>{link("user", e.user.clone())}</td>
                                        <td>{cred}</td>
                                        <td>{link("client", Some(e.client.clone()))}</td>
                                        <td>{e.method.clone()}</td>
                                        <td><a href=group_href title="Filter by this route">{e.path.clone()}</a></td>
                                        <td><a href=status_href class=status_class(e.status)>{e.status}</a></td>
                                        <td>{format!("{:.1}", e.duration_ms)}</td>
                                        <td><a href=detail_href>"Details"</a></td>
                                    </tr>
                                }
                            }).collect_view()
                        }}
                    </tbody>
                </table>
                <div class="form-row al-paging">
                    {move || {
                        let p = params.get();
                        let total = page.get().map(|x| x.total).unwrap_or(0);
                        let off: u32 = p.get("offset").and_then(|o| o.parse().ok()).unwrap_or(0);
                        let newer = (off > 0).then(|| {
                            let o = off.saturating_sub(PAGE_SIZE).to_string();
                            view! { <a id="al-newer" class="btn btn-sm" href=href(&p, &[("offset", o.as_str())], &[])>"Newer"</a> }
                        });
                        let older = ((off + PAGE_SIZE) as i64 <= total.saturating_sub(1)).then(|| {
                            let o = (off + PAGE_SIZE).to_string();
                            view! { <a id="al-older" class="btn btn-sm" href=href(&p, &[("offset", o.as_str())], &[])>"Older"</a> }
                        });
                        view! { {newer}{older} }
                    }}
                </div>

                <details class="al-settings" id="al-settings">
                    <summary>"Recording settings"</summary>
                    <p class="al-muted">
                        "Also available as the declarative key access_log and GET/PUT /api/access-log/settings. "
                        "Each node keeps its own log."
                    </p>
                    <div class="form-row">
                        <div class="form-group">
                            <label for="als-enabled">"Record requests"</label>
                            <input id="als-enabled" type="checkbox" prop:checked=s_enabled on:change=move |ev| s_enabled.set(event_target_checked(&ev))/>
                        </div>
                        <div class="form-group">
                            <label for="als-days">"Keep for (days)"</label>
                            <input id="als-days" type="number" min="1" prop:value=s_days on:input=move |ev| s_days.set(event_target_value(&ev))/>
                        </div>
                        <div class="form-group">
                            <label for="als-mb">"Keep at most (MiB)"</label>
                            <input id="als-mb" type="number" min="1" prop:value=s_mb on:input=move |ev| s_mb.set(event_target_value(&ev))/>
                        </div>
                        <div class="form-group">
                            <label for="als-static">"Also record web UI file requests"</label>
                            <input id="als-static" type="checkbox" prop:checked=s_static on:change=move |ev| s_static.set(event_target_checked(&ev))/>
                        </div>
                    </div>
                    <div class="form-row">
                        <div class="form-group">
                            <label for="als-proxies">"Trusted proxies (one address or network per line; their X-Forwarded-For is believed)"</label>
                            <textarea id="als-proxies" rows="3" prop:value=s_proxies on:input=move |ev| s_proxies.set(event_target_value(&ev))></textarea>
                        </div>
                        <div class="form-group">
                            <label for="als-exclude">"Do not record paths starting with (one per line)"</label>
                            <textarea id="als-exclude" rows="3" prop:value=s_exclude on:input=move |ev| s_exclude.set(event_target_value(&ev))></textarea>
                        </div>
                    </div>
                    <button id="als-save" class="btn btn-primary" on:click=move |_| save_settings()>"Save settings"</button>
                    {move || settings_msg.get().map(|(ok, m)| view! {
                        <div id="als-msg" class=if ok { "alert alert-success" } else { "alert alert-error" }>{m}</div>
                    })}
                    {move || stats.get().map(|s| view! {
                        <p class="al-muted" id="al-collector">
                            {format!("{} entries stored ({}), {} recorded since start, {} dropped, {} write errors.",
                                s.collector.entries, fmt_bytes(s.collector.db_bytes as i64),
                                s.collector.written, s.collector.dropped, s.collector.write_errors)}
                        </p>
                    })}
                </details>
            </div>
        </div>
    }
}

/// RFC 3339 UTC form of a millisecond timestamp (for tooltips).
pub fn iso_of(ms: i64) -> String {
    let d = web_sys::js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms as f64));
    d.to_iso_string().as_string().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(v: &[(&str, &str)]) -> Params {
        v.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn href_applies_a_filter_and_resets_paging() {
        let cur = p(&[("user", "alice"), ("offset", "50"), ("detail", "9")]);
        assert_eq!(
            href(&cur, &[("status", "4xx")], &[]),
            "/access-log?status=4xx&user=alice"
        );
        // moving to another page keeps the filter
        assert_eq!(
            href(&cur, &[("offset", "100")], &[]),
            "/access-log?detail=9&offset=100&user=alice"
        );
        assert_eq!(href(&cur, &[("user", "")], &[]), "/access-log");
        assert_eq!(
            href(&p(&[("path", "/api/vms")]), &[], &[]),
            "/access-log?path=%2Fapi%2Fvms"
        );
    }

    #[test]
    fn api_query_defaults_to_the_last_hour() {
        let now = 1_700_000_000_000;
        assert_eq!(
            api_query(&p(&[("user", "a b")]), now, 50, 0),
            format!("start={}&user=a%20b&limit=50", 1_700_000_000 - 3600)
        );
        assert_eq!(
            api_query(&p(&[("last", "86400")]), now, 50, 100),
            format!("start={}&limit=50&offset=100", 1_700_000_000 - 86400)
        );
        // an explicit range wins over `last`; page keys never reach the API
        assert_eq!(
            api_query(
                &p(&[
                    ("start", "5"),
                    ("end", "9"),
                    ("last", "60"),
                    ("detail", "3")
                ]),
                now,
                1,
                0
            ),
            "end=9&start=5&limit=1"
        );
    }

    #[test]
    fn time_is_shown_without_t_and_z() {
        assert_eq!(
            fmt_time("2026-11-25T14:03:07.123Z"),
            "2026-11-25 14:03:07.123"
        );
    }
}
