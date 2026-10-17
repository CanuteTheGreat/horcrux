use crate::api;
use leptos::*;

#[component]
pub fn Dashboard() -> impl IntoView {
    let (node_metrics, set_node_metrics) = create_signal(None::<api::NodeMetrics>);
    let (cluster_nodes, set_cluster_nodes) = create_signal(Vec::<api::ClusterNode>::new());
    let (active_alerts, set_active_alerts) = create_signal(Vec::<api::ActiveAlert>::new());
    let (vm_count, set_vm_count) = create_signal(0);
    // Tracks any fetch failure so the dashboard can't silently show a
    // misleading all-zeros "connected" state when it actually isn't talking
    // to the backend at all (2026-10-12: this is exactly how the broken
    // hardcoded-localhost API URL went unnoticed here while surfacing loudly
    // on the /vms page, which does report its fetch errors).
    let (fetch_error, set_fetch_error) = create_signal(None::<String>);

    // Load data on mount
    create_effect(move |_| {
        spawn_local(async move {
            match api::get_node_metrics().await {
                Ok(metrics) => set_node_metrics.set(metrics),
                Err(e) => set_fetch_error.set(Some(e.message)),
            }
            match api::get_cluster_nodes().await {
                Ok(nodes) => set_cluster_nodes.set(nodes),
                Err(e) => set_fetch_error.set(Some(e.message)),
            }
            match api::get_active_alerts().await {
                Ok(alerts) => set_active_alerts.set(alerts),
                Err(e) => set_fetch_error.set(Some(e.message)),
            }
            match api::get_vms().await {
                Ok(vms) => set_vm_count.set(vms.len()),
                Err(e) => set_fetch_error.set(Some(e.message)),
            }
        });
    });

    view! {
        <div class="dashboard">
            <h1>"Dashboard"</h1>

            {move || fetch_error.get().map(|err| view! {
                <p class="error">"Could not reach the backend for one or more panels: " {err}</p>
            })}

            <div class="stats-grid">
                <div class="stat-card">
                    <h3>"Virtual Machines"</h3>
                    <div class="stat-value">{move || vm_count.get()}</div>
                </div>

                <div class="stat-card">
                    <h3>"Cluster Nodes"</h3>
                    <div class="stat-value">{move || cluster_nodes.get().len()}</div>
                </div>

                <div class="stat-card">
                    <h3>"Active Alerts"</h3>
                    <div class="stat-value alert-count">
                        {move || active_alerts.get().len()}
                    </div>
                </div>

                <div class="stat-card">
                    <h3>"CPU Usage"</h3>
                    <div class="stat-value">
                        {move || node_metrics.get().map(|m| format!("{:.1}%", m.cpu_usage)).unwrap_or_else(|| "--".to_string())}
                    </div>
                </div>
            </div>

            <div class="dashboard-section">
                <h2>"System Status"</h2>
                {move || node_metrics.get().map(|metrics| view! {
                    <div class="system-info">
                        <p><strong>"Hostname:"</strong> " " {&metrics.hostname}</p>
                        <p><strong>"Memory:"</strong> " " {format!("{} / {} GB",
                            metrics.memory_used / 1024 / 1024 / 1024,
                            metrics.memory_total / 1024 / 1024 / 1024
                        )}</p>
                        <p><strong>"Uptime:"</strong> " " {format_uptime(metrics.uptime_seconds)}</p>
                    </div>
                })}
            </div>

            <div class="dashboard-section">
                <h2>"Recent Alerts"</h2>
                {move || {
                    let alerts = active_alerts.get();
                    if alerts.is_empty() {
                        view! { <p class="no-data">"No active alerts"</p> }.into_view()
                    } else {
                        view! {
                            <ul class="alert-list">
                                {alerts.into_iter().take(5).map(|alert| view! {
                                    <li class={format!("alert-item severity-{}", alert.severity.to_lowercase())}>
                                        <strong>{&alert.rule_name}</strong>
                                        " - "
                                        {&alert.metric}
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
