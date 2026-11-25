use crate::api;
use crate::components::ui::{
    fmt_bytes, state_tone, task_tone, usage_tone, Gauge, StateBadge, UsageBar,
};
use crate::features::IfFeature;
use horcrux_common::VmStatus;
use leptos::*;
use leptos_router::A;

/// Whether this user may read a panel's data.
#[derive(Clone, Copy, PartialEq)]
enum Access {
    Allowed,
    /// The server answered 403: the user's role can't read this panel.
    Denied,
}

/// Which dashboard panels the logged-in user may read, from the same
/// privileges the server's rbac middleware requires for each endpoint
/// (`GET /monitoring/node` and `/alerts/active`: Sys.Audit; `/cluster/nodes`:
/// Sys.Audit on /cluster). A role without them is not sent the request at
/// all, so a VM user's dashboard no longer fires three 403s.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PanelAccess {
    pub metrics: bool,
    pub nodes: bool,
    pub alerts: bool,
}

impl PanelAccess {
    pub fn for_me(me: &api::Me) -> Self {
        Self {
            metrics: me.can_at("Sys.Audit", "/nodes"),
            nodes: me.can_at("Sys.Audit", "/cluster"),
            alerts: me.can_at("Sys.Audit", "/"),
        }
    }

    /// Everything is attempted (the session's privileges are unknown); a 403
    /// is then still handled per panel.
    pub fn assume_all() -> Self {
        Self {
            metrics: true,
            nodes: true,
            alerts: true,
        }
    }
}

#[component]
pub fn Dashboard() -> impl IntoView {
    let (node_metrics, set_node_metrics) = create_signal(None::<api::NodeMetrics>);
    let (cluster_nodes, set_cluster_nodes) = create_signal(Vec::<api::ClusterNode>::new());
    let (active_alerts, set_active_alerts) = create_signal(Vec::<api::ActiveAlert>::new());
    let (vms, set_vms) = create_signal(Vec::<horcrux_common::VmConfig>::new());
    let (containers, set_containers) = create_signal(None::<Vec<api::Container>>);
    let (pools, set_pools) = create_signal(None::<Vec<api::StoragePoolInfo>>);
    let (tasks, set_tasks) = create_signal(None::<Vec<api::TaskRecord>>);
    // Per-panel permission: a non-admin (e.g. a VM user) gets 403 for node
    // metrics, cluster nodes and alerts. Those panels say "No permission"
    // instead of a fake 0 under a "could not reach the backend" banner.
    let (metrics_access, set_metrics_access) = create_signal(Access::Allowed);
    let (nodes_access, set_nodes_access) = create_signal(Access::Allowed);
    let (alerts_access, set_alerts_access) = create_signal(Access::Allowed);
    let (vms_access, set_vms_access) = create_signal(Access::Allowed);
    // Tracks any real fetch failure (not a permission refusal) so the
    // dashboard can't silently show a misleading all-zeros "connected" state
    // when it actually isn't talking to the backend at all (2026-10-12: this
    // is exactly how the broken hardcoded-localhost API URL went unnoticed
    // here while surfacing loudly on the /vms page).
    let (fetch_error, set_fetch_error) = create_signal(None::<String>);

    // A failed panel load: 403 marks the panel "no permission", anything
    // else is a backend problem worth the banner.
    let failed = move |e: api::ApiError, deny: WriteSignal<Access>| {
        if e.is_forbidden() {
            deny.set(Access::Denied);
        } else {
            set_fetch_error.set(Some(e.message));
        }
    };

    // Load on mount, then every 10 s: the summary is a live view (counts, gauges, tasks).
    let load = move || {
        spawn_local(async move {
            let access = match api::get_me().await {
                Ok(me) => PanelAccess::for_me(&me),
                Err(_) => PanelAccess::assume_all(),
            };
            if access.metrics {
                match api::get_node_metrics().await {
                    Ok(metrics) => set_node_metrics.set(metrics),
                    Err(e) => failed(e, set_metrics_access),
                }
            } else {
                set_metrics_access.set(Access::Denied);
            }
            // USE=-cluster: no cluster routes on the server, no panel.
            if crate::features::load().await.has("cluster") {
                if access.nodes {
                    match api::get_cluster_nodes().await {
                        Ok(nodes) => set_cluster_nodes.set(nodes),
                        Err(e) => failed(e, set_nodes_access),
                    }
                } else {
                    set_nodes_access.set(Access::Denied);
                }
            }
            if access.alerts {
                match api::get_active_alerts().await {
                    Ok(alerts) => set_active_alerts.set(alerts),
                    Err(e) => failed(e, set_alerts_access),
                }
            } else {
                set_alerts_access.set(Access::Denied);
            }
            match api::get_vms().await {
                Ok(list) => set_vms.set(list),
                Err(e) => failed(e, set_vms_access),
            }
            // Containers, storage pools and tasks are optional panels: a
            // runtime that isn't built in or a role without access shows "--"
            // rather than the backend-error banner.
            if let Ok(c) = api::get_containers().await {
                set_containers.set(Some(c));
            }
            if let Ok(p) = api::get_storage_pools().await {
                set_pools.set(Some(p));
            }
            if let Ok(t) = api::list_tasks("").await {
                set_tasks.set(Some(t));
            }
        });
    };
    load();
    if let Ok(h) = set_interval_with_handle(load, std::time::Duration::from_secs(10)) {
        on_cleanup(move || h.clear());
    }

    // A stat card's value, or "No permission" for a refused panel.
    let stat = move |access: ReadSignal<Access>, value: Signal<String>| {
        move || {
            if access.get() == Access::Denied {
                view! { <span class="no-permission" title="Your role can't read this">"No permission"</span> }
                    .into_view()
            } else {
                value.get().into_view()
            }
        }
    };

    let count_of =
        move |f: fn(&VmStatus) -> bool| vms.get().iter().filter(|v| f(&v.status)).count();
    let opt_len = |n: Option<usize>| n.map(|n| n.to_string()).unwrap_or_else(|| "--".to_string());

    let running_vms = move || count_of(|s| matches!(s, VmStatus::Running));
    let stopped_vms = move || count_of(|s| matches!(s, VmStatus::Stopped));
    let ct_running = move || {
        containers
            .get()
            .map(|c| c.iter().filter(|c| state_tone(&c.status) == "ok").count())
    };
    let mem_pct = move || {
        node_metrics
            .get()
            .filter(|m| m.memory_total > 0)
            .map(|m| m.memory_used as f64 * 100.0 / m.memory_total as f64)
    };
    let storage_totals = move || {
        pools.get().map(|ps| {
            let (u, t) = ps.iter().fold((0u64, 0u64), |(u, t), p| {
                (u + p.used_bytes, t + p.total_bytes)
            });
            (u, t)
        })
    };
    let storage_pct = move || {
        storage_totals()
            .filter(|(_, t)| *t > 0)
            .map(|(u, t)| u as f64 * 100.0 / t as f64)
    };
    let alert_n = move || active_alerts.get().len();
    let running_tasks = move || {
        tasks
            .get()
            .map(|t| t.iter().filter(|t| t.status == "running").count())
    };

    view! {
        <div class="dashboard">
            <div class="page-header">
                <div>
                    <h1>"Datacenter"</h1>
                    <p class="page-subtitle">
                        {move || node_metrics.get().map(|m| format!("Node {} \u{b7} up {} \u{b7} refreshes every 10 s", m.hostname, format_uptime(m.uptime_seconds)))
                            .unwrap_or_else(|| "Summary \u{b7} refreshes every 10 s".to_string())}
                    </p>
                </div>

            </div>

            {move || fetch_error.get().map(|err| view! {
                <p class="error">"Could not reach the backend for one or more panels: " {err}</p>
            })}

            <crate::pages::UnmanagedLibvirtBanner/>

            // Every number is a real link (middle-click, keyboard, new tab) to
            // the page behind it, pre-filtered where the page can.
            <div class="panel-grid">
                <section class="panel" id="dash-health">
                    <div class="panel-head"><h2>"Health"</h2></div>
                    <div class="panel-body">
                        <div class="health-list">
                            <A href="/system/health" class=move || format!("health-tile stat-link {}", if metrics_access.get() == Access::Denied { "" } else if node_metrics.get().is_some() { "ok" } else { "" }) attr:id="dash-system-status">
                                <span class="ht-icon" aria-hidden="true">{move || if node_metrics.get().is_some() { "\u{2713}" } else { "\u{2026}" }}</span>
                                <span>
                                    <span class="ht-title">"Node"</span>
                                    <span class="ht-sub">
                                        {move || if metrics_access.get() == Access::Denied {
                                            "No permission".to_string()
                                        } else {
                                            node_metrics.get().map(|m| format!("{} online", m.hostname)).unwrap_or_else(|| "Loading".into())
                                        }}
                                    </span>
                                </span>
                            </A>
                            <A href="/alerts" class=move || format!("health-tile stat-link {}", if alerts_access.get() == Access::Denied { "" } else if alert_n() > 0 { "crit" } else { "ok" }) attr:id="stat-active-alerts">
                                <span class="ht-icon" aria-hidden="true">{move || if alert_n() > 0 { "!" } else { "\u{2713}" }}</span>
                                <span>
                                    <span class="ht-title">"Alerts"</span>
                                    <span class="ht-sub">
                                        <span class="stat-value" class:alert-count=move || { alert_n() > 0 }>
                                            {stat(alerts_access, Signal::derive(move || alert_n().to_string()))}
                                        </span>
                                        " active"
                                    </span>
                                </span>
                            </A>
                            <A href="/tasks?status=running" class=move || format!("health-tile stat-link {}", if running_tasks().unwrap_or(0) > 0 { "" } else { "ok" }) attr:id="stat-tasks-running">
                                <span class="ht-icon" aria-hidden="true">{move || match running_tasks() { Some(0) => "\u{2713}".to_string(), Some(n) => n.to_string(), None => "-".into() }}</span>
                                <span>
                                    <span class="ht-title">"Tasks"</span>
                                    <span class="ht-sub">
                                        <span class="stat-value">{move || opt_len(running_tasks())}</span>
                                        " running"
                                    </span>
                                </span>
                            </A>
                        </div>
                    </div>
                </section>

                <section class="panel" id="dash-guests">
                    <div class="panel-head"><h2>"Guests"</h2><A href="/resources">"All resources"</A></div>
                    <div class="panel-body">
                        <div class="count-rows">
                            <A href="/vms" class="count-row stat-link" attr:id="stat-vms">
                                <span class="dot info" aria-hidden="true"></span>"Virtual machines"
                                <span class="n stat-value">{stat(vms_access, Signal::derive(move || vms.get().len().to_string()))}</span>
                            </A>
                            <A href="/vms?state=running" class="count-row stat-link" attr:id="stat-vms-running">
                                <span class="dot ok" aria-hidden="true"></span>"Running"
                                <span class="n stat-value">{stat(vms_access, Signal::derive(move || running_vms().to_string()))}</span>
                            </A>
                            <A href="/vms?state=stopped" class="count-row stat-link" attr:id="stat-vms-stopped">
                                <span class="dot off" aria-hidden="true"></span>"Stopped"
                                <span class="n stat-value">{stat(vms_access, Signal::derive(move || stopped_vms().to_string()))}</span>
                            </A>
                            <A href="/containers" class="count-row stat-link" attr:id="stat-containers">
                                <span class="dot info" aria-hidden="true"></span>"Containers"
                                <span class="n stat-value">{move || match (containers.get(), ct_running()) {
                                    (Some(c), Some(r)) => format!("{}", c.len()) + &if c.is_empty() { String::new() } else { format!(" ({r} running)") },
                                    _ => "--".to_string(),
                                }}</span>
                            </A>
                            <IfFeature feature="cluster">
                                <A href="/ha/cluster" class="count-row stat-link" attr:id="stat-cluster-nodes">
                                    <span class="dot ok" aria-hidden="true"></span>"Cluster nodes"
                                    <span class="n stat-value">{stat(nodes_access, Signal::derive(move || cluster_nodes.get().len().to_string()))}</span>
                                </A>
                            </IfFeature>
                            <A href="/storage" class="count-row stat-link" attr:id="stat-storage-pools">
                                <span class="side-storage-glyph" aria-hidden="true"></span>"Storage pools"
                                <span class="n stat-value">{move || opt_len(pools.get().map(|p| p.len()))}</span>
                            </A>
                        </div>
                    </div>
                </section>

                <section class="panel" id="dash-resources">
                    <div class="panel-head"><h2>"Resources"</h2><A href="/monitoring">"Graphs"</A></div>
                    <div class="panel-body">
                        <div class="gauges">
                            <Gauge id="stat-cpu" href="/monitoring" label="CPU"
                                pct=Signal::derive(move || node_metrics.get().map(|m| m.cpu_usage))
                                value=Signal::derive(move || stat(metrics_access, Signal::derive(move || node_metrics.get().map(|m| format!("{:.0}%", m.cpu_usage)).unwrap_or_else(|| "--".to_string())))().into_view())
                                sub=Signal::derive(move || cluster_nodes.get().iter().find(|n| n.is_local).map(|n| format!("of {} CPU(s)", n.cpu_cores)).unwrap_or_else(|| "node load".to_string()))/>
                            <Gauge id="stat-memory" href="/system/health" label="Memory"
                                pct=Signal::derive(mem_pct)
                                value=Signal::derive(move || mem_pct().map(|p| format!("{:.0}%", p)).unwrap_or_else(|| "--".into()).into_view())
                                sub=Signal::derive(move || node_metrics.get().map(|m| format!("{} of {}", fmt_bytes(m.memory_used), fmt_bytes(m.memory_total))).unwrap_or_default())/>
                            <Gauge id="stat-storage" href="/storage" label="Storage"
                                pct=Signal::derive(storage_pct)
                                value=Signal::derive(move || storage_pct().map(|p| format!("{:.0}%", p)).unwrap_or_else(|| "--".into()).into_view())
                                sub=Signal::derive(move || storage_totals().map(|(u, t)| format!("{} of {}", fmt_bytes(u), fmt_bytes(t))).unwrap_or_default())/>
                        </div>
                    </div>
                </section>
            </div>

            <div class="panel-grid">
                <section class="panel dashboard-section" id="dash-vms">
                    <div class="panel-head"><h2>"Virtual machines"</h2><A href="/vms">"All"</A></div>
                    {move || {
                        let list = vms.get();
                        if list.is_empty() {
                            view! {
                                <div class="empty-state">
                                    <div class="empty-title">"No virtual machines yet"</div>
                                    <p class="empty-todo">"Create one from an ISO, or import from libvirt, OVF or ESXi."</p>
                                    <div class="empty-actions">
                                        <A href="/vms/create" class="btn btn-primary">"Create VM"</A>
                                        <A href="/vms/import-libvirt" class="btn">"Import"</A>
                                    </div>
                                </div>
                            }.into_view()
                        } else {
                            view! {
                                <ul class="dash-list">
                                    {list.into_iter().take(8).map(|vm| view! {
                                        <li>
                                            <A href=format!("/vms/{}", vm.id) class="dash-link dash-item">
                                                <span class="dash-name">
                                                    <span class=format!("dot {}", state_tone(&vm.status.to_string())) aria-hidden="true"></span>
                                                    <strong>{vm.name.clone()}</strong>
                                                </span>
                                                <span class=format!("dash-meta status-{}", vm.status)>
                                                    {format!("{} vCPU \u{b7} {} \u{b7} ", vm.cpus, fmt_bytes(vm.memory * 1024 * 1024))}
                                                    {vm.status.to_string()}
                                                </span>
                                            </A>
                                        </li>
                                    }).collect_view()}
                                </ul>
                            }.into_view()
                        }
                    }}
                </section>

                <section class="panel dashboard-section" id="dash-pools">
                    <div class="panel-head"><h2>"Storage"</h2><A href="/storage">"All"</A></div>
                    {move || match pools.get() {
                        Some(list) if !list.is_empty() => view! {
                            <ul class="dash-list">
                                {list.into_iter().map(|p| {
                                    let pct = if p.total_bytes > 0 { p.used_bytes as f64 * 100.0 / p.total_bytes as f64 } else { 0.0 };
                                    view! {
                                        <li>
                                            <A href=format!("/storage?pool={}", p.id) class="dash-link dash-item">
                                                <span class="dash-name">
                                                    <span class=format!("dot {}", if usage_tone(pct).is_empty() { state_tone(&p.status) } else { usage_tone(pct) }) aria-hidden="true"></span>
                                                    <strong>{p.name.clone()}</strong>
                                                    <span class="dash-meta">{p.pool_type.clone()}</span>
                                                </span>
                                                <span class="dash-right">
                                                    <span class="dash-meta">{format!("{} of {}", fmt_bytes(p.used_bytes), fmt_bytes(p.total_bytes))}</span>
                                                    <span class="dash-usage"><UsageBar pct=pct/></span>
                                                </span>
                                            </A>
                                        </li>
                                    }
                                }).collect_view()}
                            </ul>
                        }.into_view(),
                        _ => view! {
                            <div class="empty-state">
                                <div class="empty-title">"No storage pools"</div>
                                <div class="empty-actions"><A href="/storage" class="btn">"Add storage"</A></div>
                            </div>
                        }.into_view(),
                    }}
                </section>
            </div>

            <IfFeature feature="cluster">
                <section class="panel dashboard-section" id="dash-nodes">
                    <div class="panel-head"><h2>"Nodes"</h2><A href="/ha/cluster">"Cluster"</A></div>
                    {move || {
                        let list = cluster_nodes.get();
                        if nodes_access.get() == Access::Denied {
                            view! { <p class="no-data">"No permission to view nodes"</p> }.into_view()
                        } else if list.is_empty() {
                            view! { <p class="no-data">"Single node: no cluster configured. " <A href="/ha/cluster">"Create or join a cluster"</A></p> }.into_view()
                        } else {
                            view! {
                                <ul class="dash-list">
                                    {list.into_iter().map(|n| {
                                        let mem = n.memory_used.filter(|_| n.memory_total > 0).map(|u| u as f64 * 100.0 / n.memory_total as f64);
                                        view! {
                                            <li>
                                                <A href="/ha/cluster" class="dash-link node-row">
                                                    <span class="dash-name">
                                                        <span class=format!("dot {}", state_tone(&n.status)) aria-hidden="true"></span>
                                                        <strong>{n.name.clone()}</strong>
                                                        <span class="dash-meta mono">{n.ip.clone()}</span>
                                                    </span>
                                                    <span class="metric cpu"><span class="dash-meta">"CPU"</span>{n.cpu_usage.map(|c| view! { <UsageBar pct=c/> }.into_view()).unwrap_or_else(|| view! { <span class="dash-meta">"unknown"</span> }.into_view())}</span>
                                                    <span class="metric ram"><span class="dash-meta">"RAM"</span>{mem.map(|c| view! { <UsageBar pct=c/> }.into_view()).unwrap_or_else(|| view! { <span class="dash-meta">"unknown"</span> }.into_view())}</span>
                                                    <span class=format!("dash-meta node-state node-{}", n.status)>{n.status.clone()}</span>
                                                </A>
                                            </li>
                                        }
                                    }).collect_view()}
                                </ul>
                            }.into_view()
                        }
                    }}
                </section>
            </IfFeature>

            <div class="panel-grid">
                <section class="panel dashboard-section" id="dash-tasks">
                    <div class="panel-head"><h2>"Recent tasks"</h2><A href="/tasks">"All tasks"</A></div>
                    {move || match tasks.get() {
                        Some(list) if !list.is_empty() => view! {
                            <ul class="dash-list">
                                {list.into_iter().take(6).map(|t| view! {
                                    <li>
                                        <A href=format!("/tasks?upid={}", t.upid) class="dash-link dash-item">
                                            <span class="dash-name">
                                                <span class=format!("dot {}", task_tone(&t.status)) aria-hidden="true"></span>
                                                <strong>{t.task_type.clone()}</strong>
                                                <span class="dash-meta">{t.target.clone().unwrap_or_default()}</span>
                                            </span>
                                            <span class="dash-right">
                                                <span class="dash-meta">{fmt_time(t.start_time)}</span>
                                                <span class=format!("dash-meta task-{}", t.status)>{t.status.clone()}</span>
                                            </span>
                                        </A>
                                    </li>
                                }).collect_view()}
                            </ul>
                        }.into_view(),
                        _ => view! { <p class="no-data">"No recent tasks. VM starts, backups and migrations show up here."</p> }.into_view(),
                    }}
                </section>

                <section class="panel dashboard-section" id="dash-alerts">
                    <div class="panel-head"><h2>"Active alerts"</h2><A href="/monitoring/alerts">"Alert rules"</A></div>
                    {move || {
                        let alerts = active_alerts.get();
                        if alerts_access.get() == Access::Denied {
                            view! { <p class="no-data">"No permission to view alerts"</p> }.into_view()
                        } else if alerts.is_empty() {
                            view! { <p class="no-data"><span class="state-badge ok"><span class="dot ok"></span>"All clear"</span>" No active alerts."</p> }.into_view()
                        } else {
                            view! {
                                <ul class="dash-list">
                                    {alerts.into_iter().take(6).map(|alert| view! {
                                        <li>
                                            <A href="/alerts" class="dash-link dash-item">
                                                <span class="dash-name">
                                                    <StateBadge state=alert.severity.to_lowercase()/>
                                                    <strong>{alert.rule_name.clone()}</strong>
                                                    <span class="dash-meta">{alert.target.clone()}</span>
                                                </span>
                                                <span class="dash-meta dash-msg">{alert.message.clone()}</span>
                                            </A>
                                        </li>
                                    }).collect_view()}
                                </ul>
                            }.into_view()
                        }
                    }}
                </section>
            </div>
        </div>
    }
}

/// Local "Nov 25 17:51" for a unix timestamp.
fn fmt_time(ts: i64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ts as f64 * 1000.0));
    const M: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!(
        "{} {} {:02}:{:02}",
        M[(d.get_month() as usize).min(11)],
        d.get_date(),
        d.get_hours(),
        d.get_minutes()
    )
}

fn format_uptime(seconds: u64) -> String {
    let days = seconds / 86400;
    let hours = (seconds % 86400) / 3600;
    let mins = (seconds % 3600) / 60;

    if days > 0 {
        format!("{}d {}h {}m", days, hours, mins)
    } else if hours > 0 {
        format!("{}h {}m", hours, mins)
    } else {
        format!("{}m", mins)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn me(is_admin: bool, perms: serde_json::Value) -> api::Me {
        serde_json::from_value(serde_json::json!({
            "user": {"id": "u1", "username": "u1", "email": "", "role": "VMUser", "realm": "local",
                     "enabled": true},
            "is_admin": is_admin,
            "permissions": perms,
        }))
        .expect("Me json")
    }

    #[test]
    fn vm_user_dashboard_issues_no_admin_only_requests() {
        let vm_user = me(
            false,
            serde_json::json!({"/": [], "/vms": ["VM.Audit", "VM.Console", "VM.PowerMgmt"]}),
        );
        assert_eq!(
            PanelAccess::for_me(&vm_user),
            PanelAccess {
                metrics: false,
                nodes: false,
                alerts: false
            }
        );
    }

    #[test]
    fn admin_and_auditor_dashboards_read_every_panel() {
        let admin = me(true, serde_json::json!({}));
        assert_eq!(PanelAccess::for_me(&admin), PanelAccess::assume_all());
        let auditor = me(false, serde_json::json!({"/": ["Sys.Audit"]}));
        assert_eq!(PanelAccess::for_me(&auditor), PanelAccess::assume_all());
    }
}
