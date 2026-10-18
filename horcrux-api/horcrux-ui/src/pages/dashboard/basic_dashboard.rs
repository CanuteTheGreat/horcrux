use crate::api;
use crate::features::IfFeature;
use leptos::*;

/// Whether this user may read a panel's data.
#[derive(Clone, Copy, PartialEq)]
enum Access {
    Allowed,
    /// The server answered 403: the user's role can't read this panel.
    Denied,
}

#[component]
pub fn Dashboard() -> impl IntoView {
    let (node_metrics, set_node_metrics) = create_signal(None::<api::NodeMetrics>);
    let (cluster_nodes, set_cluster_nodes) = create_signal(Vec::<api::ClusterNode>::new());
    let (active_alerts, set_active_alerts) = create_signal(Vec::<api::ActiveAlert>::new());
    let (vm_count, set_vm_count) = create_signal(0);
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
            match api::get_node_metrics().await {
                Ok(metrics) => set_node_metrics.set(metrics),
                Err(e) => failed(e, set_metrics_access),
            }
            // USE=-cluster: no cluster routes on the server, no panel.
            if crate::features::load().await.has("cluster") {
                match api::get_cluster_nodes().await {
                    Ok(nodes) => set_cluster_nodes.set(nodes),
                    Err(e) => failed(e, set_nodes_access),
                }
            }
            match api::get_active_alerts().await {
                Ok(alerts) => set_active_alerts.set(alerts),
                Err(e) => failed(e, set_alerts_access),
            }
            match api::get_vms().await {
                Ok(vms) => set_vm_count.set(vms.len()),
                Err(e) => failed(e, set_vms_access),
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

    view! {
        <div class="dashboard">
            <h1>"Dashboard"</h1>

            {move || fetch_error.get().map(|err| view! {
                <p class="error">"Could not reach the backend for one or more panels: " {err}</p>
            })}

            <div class="stats-grid">
                <div class="stat-card" id="stat-vms">
                    <h3>"Virtual Machines"</h3>
                    <div class="stat-value">
                        {stat(vms_access, Signal::derive(move || vm_count.get().to_string()))}
                    </div>
                </div>

                <IfFeature feature="cluster">
                    <div class="stat-card" id="stat-cluster-nodes">
                        <h3>"Cluster Nodes"</h3>
                        <div class="stat-value">
                            {stat(nodes_access, Signal::derive(move || cluster_nodes.get().len().to_string()))}
                        </div>
                    </div>
                </IfFeature>

                <div class="stat-card" id="stat-active-alerts">
                    <h3>"Active Alerts"</h3>
                    <div class="stat-value alert-count">
                        {stat(alerts_access, Signal::derive(move || active_alerts.get().len().to_string()))}
                    </div>
                </div>

                <div class="stat-card" id="stat-cpu">
                    <h3>"CPU Usage"</h3>
                    <div class="stat-value">
                        {stat(metrics_access, Signal::derive(move || {
                            node_metrics
                                .get()
                                .map(|m| format!("{:.1}%", m.cpu_usage))
                                .unwrap_or_else(|| "--".to_string())
                        }))}
                    </div>
                </div>
            </div>

            <div class="dashboard-section">
                <h2>"System Status"</h2>
                {move || if metrics_access.get() == Access::Denied {
                    view! { <p class="no-data">"No permission to view node status"</p> }.into_view()
                } else {
                    node_metrics.get().map(|metrics| view! {
                        <div class="system-info">
                            <p><strong>"Hostname:"</strong> " " {&metrics.hostname}</p>
                            <p><strong>"Memory:"</strong> " " {format!("{} / {} GB",
                                metrics.memory_used / 1024 / 1024 / 1024,
                                metrics.memory_total / 1024 / 1024 / 1024
                            )}</p>
                            <p><strong>"Uptime:"</strong> " " {format_uptime(metrics.uptime_seconds)}</p>
                        </div>
                    }).into_view()
                }}
            </div>

            <div class="dashboard-section">
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
                                        <strong>{&alert.rule_name}</strong>
                                        " - "
                                        {&alert.target}
                                        " - "
                                        {&alert.message}
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
