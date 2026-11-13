use crate::api;
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

    // Load data on mount
    create_effect(move |_| {
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
    });

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

    view! {
        <div class="dashboard">
            <h1>"Dashboard"</h1>

            {move || fetch_error.get().map(|err| view! {
                <p class="error">"Could not reach the backend for one or more panels: " {err}</p>
            })}

            <crate::pages::UnmanagedLibvirtBanner/>

            // Every card is a real link (middle-click, keyboard, new tab) to
            // the page behind the number, pre-filtered where the page can.
            <div class="stats-grid">
                <A href="/vms" class="stat-card stat-link" attr:id="stat-vms">
                    <h3>"Virtual Machines"</h3>
                    <div class="stat-value">
                        {stat(vms_access, Signal::derive(move || vms.get().len().to_string()))}
                    </div>
                </A>

                <A href="/vms?state=running" class="stat-card stat-link" attr:id="stat-vms-running">
                    <h3>"Running VMs"</h3>
                    <div class="stat-value">
                        {stat(vms_access, Signal::derive(move || count_of(|s| matches!(s, VmStatus::Running)).to_string()))}
                    </div>
                </A>

                <A href="/vms?state=stopped" class="stat-card stat-link" attr:id="stat-vms-stopped">
                    <h3>"Stopped VMs"</h3>
                    <div class="stat-value">
                        {stat(vms_access, Signal::derive(move || count_of(|s| matches!(s, VmStatus::Stopped)).to_string()))}
                    </div>
                </A>

                <A href="/containers" class="stat-card stat-link" attr:id="stat-containers">
                    <h3>"Containers"</h3>
                    <div class="stat-value">
                        {move || opt_len(containers.get().map(|c| c.len()))}
                    </div>
                </A>

                <IfFeature feature="cluster">
                    <A href="/ha/cluster" class="stat-card stat-link" attr:id="stat-cluster-nodes">
                        <h3>"Cluster Nodes"</h3>
                        <div class="stat-value">
                            {stat(nodes_access, Signal::derive(move || cluster_nodes.get().len().to_string()))}
                        </div>
                    </A>
                </IfFeature>

                <A href="/storage" class="stat-card stat-link" attr:id="stat-storage-pools">
                    <h3>"Storage Pools"</h3>
                    <div class="stat-value">
                        {move || opt_len(pools.get().map(|p| p.len()))}
                    </div>
                </A>

                <A href="/alerts" class="stat-card stat-link" attr:id="stat-active-alerts">
                    <h3>"Active Alerts"</h3>
                    <div class="stat-value alert-count">
                        {stat(alerts_access, Signal::derive(move || active_alerts.get().len().to_string()))}
                    </div>
                </A>

                <A href="/tasks?status=running" class="stat-card stat-link" attr:id="stat-tasks-running">
                    <h3>"Running Tasks"</h3>
                    <div class="stat-value">
                        {move || opt_len(tasks.get().map(|t| t.iter().filter(|t| t.status == "running").count()))}
                    </div>
                </A>

                <A href="/monitoring" class="stat-card stat-link" attr:id="stat-cpu">
                    <h3>"CPU Usage"</h3>
                    <div class="stat-value">
                        {stat(metrics_access, Signal::derive(move || {
                            node_metrics
                                .get()
                                .map(|m| format!("{:.1}%", m.cpu_usage))
                                .unwrap_or_else(|| "--".to_string())
                        }))}
                    </div>
                </A>
            </div>

            <div class="dashboard-section">
                <h2>"System Status"</h2>
                {move || if metrics_access.get() == Access::Denied {
                    view! { <p class="no-data">"No permission to view node status"</p> }.into_view()
                } else {
                    node_metrics.get().map(|metrics| view! {
                        <A href="/system/health" class="system-info dash-link" attr:id="dash-system-status">
                            <p><strong>"Hostname:"</strong> " " {metrics.hostname.clone()}</p>
                            <p><strong>"Memory:"</strong> " " {format!("{} / {} GB",
                                metrics.memory_used / 1024 / 1024 / 1024,
                                metrics.memory_total / 1024 / 1024 / 1024
                            )}</p>
                            <p><strong>"Uptime:"</strong> " " {format_uptime(metrics.uptime_seconds)}</p>
                        </A>
                    }).into_view()
                }}
            </div>

            <div class="dashboard-section" id="dash-vms">
                <h2>"Virtual Machines"</h2>
                {move || {
                    let list = vms.get();
                    if list.is_empty() {
                        view! { <p class="no-data">"No virtual machines yet"</p> }.into_view()
                    } else {
                        view! {
                            <ul class="dash-list">
                                {list.into_iter().take(8).map(|vm| view! {
                                    <li>
                                        <A href=format!("/vms/{}", vm.id) class="dash-link dash-item">
                                            <strong>{vm.name.clone()}</strong>
                                            <span class=format!("dash-meta status-{}", vm.status)>{vm.status.to_string()}</span>
                                        </A>
                                    </li>
                                }).collect_view()}
                            </ul>
                            <A href="/vms" class="dash-more">"All virtual machines \u{2192}"</A>
                        }.into_view()
                    }
                }}
            </div>

            <IfFeature feature="cluster">
                <div class="dashboard-section" id="dash-nodes">
                    <h2>"Nodes"</h2>
                    {move || {
                        let list = cluster_nodes.get();
                        if nodes_access.get() == Access::Denied {
                            view! { <p class="no-data">"No permission to view nodes"</p> }.into_view()
                        } else if list.is_empty() {
                            view! { <p class="no-data">"No cluster nodes"</p> }.into_view()
                        } else {
                            view! {
                                <ul class="dash-list">
                                    {list.into_iter().map(|n| view! {
                                        <li>
                                            <A href="/ha/cluster" class="dash-link dash-item">
                                                <strong>{n.name.clone()}</strong>
                                                <span class=format!("dash-meta node-{}", n.status)>{n.status.clone()}</span>
                                            </A>
                                        </li>
                                    }).collect_view()}
                                </ul>
                            }.into_view()
                        }
                    }}
                </div>
            </IfFeature>

            <div class="dashboard-section" id="dash-pools">
                <h2>"Storage"</h2>
                {move || match pools.get() {
                    Some(list) if !list.is_empty() => view! {
                        <ul class="dash-list">
                            {list.into_iter().map(|p| {
                                let pct = (p.used_bytes * 100).checked_div(p.total_bytes).unwrap_or(0);
                                view! {
                                    <li>
                                        <A href=format!("/storage?pool={}", p.id) class="dash-link dash-item">
                                            <strong>{p.name.clone()}</strong>
                                            <span class="dash-meta">{format!("{} \u{b7} {}% used", p.pool_type, pct)}</span>
                                        </A>
                                    </li>
                                }
                            }).collect_view()}
                        </ul>
                    }.into_view(),
                    _ => view! { <p class="no-data">"No storage pools"</p> }.into_view(),
                }}
            </div>

            <div class="dashboard-section" id="dash-tasks">
                <h2>"Recent Tasks"</h2>
                {move || match tasks.get() {
                    Some(list) if !list.is_empty() => view! {
                        <ul class="dash-list">
                            {list.into_iter().take(5).map(|t| view! {
                                <li>
                                    <A href=format!("/tasks?upid={}", t.upid) class="dash-link dash-item">
                                        <strong>{t.task_type.clone()}</strong>
                                        {t.target.clone().map(|x| format!(" {x}"))}
                                        <span class=format!("dash-meta task-{}", t.status)>{t.status.clone()}</span>
                                    </A>
                                </li>
                            }).collect_view()}
                        </ul>
                        <A href="/tasks" class="dash-more">"All tasks \u{2192}"</A>
                    }.into_view(),
                    _ => view! { <p class="no-data">"No recent tasks"</p> }.into_view(),
                }}
            </div>

            <div class="dashboard-section" id="dash-alerts">
                <h2>"Recent Alerts"</h2>
                {move || {
                    let alerts = active_alerts.get();
                    if alerts_access.get() == Access::Denied {
                        view! { <p class="no-data">"No permission to view alerts"</p> }.into_view()
                    } else if alerts.is_empty() {
                        view! { <p class="no-data">"No active alerts"</p> }.into_view()
                    } else {
                        view! {
                            <ul class="alert-list">
                                {alerts.into_iter().take(5).map(|alert| view! {
                                    <li class={format!("alert-item severity-{}", alert.severity.to_lowercase())}>
                                        <A href="/alerts" class="dash-link">
                                            <strong>{alert.rule_name.clone()}</strong>
                                            " - "
                                            {alert.target.clone()}
                                            " - "
                                            {alert.message.clone()}
                                        </A>
                                    </li>
                                }).collect_view()}
                            </ul>
                        }.into_view()
                    }
                }}
            </div>
        </div>
    }
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
