//! Dashboard page for mobile UI

use yew::prelude::*;
use wasm_bindgen_futures::spawn_local;
use gloo_timers::callback::Interval;

use crate::api::{ApiClient, NodeStats, ClusterStatus};
use crate::components::{Header, Card, Loading};

#[function_component(Dashboard)]
pub fn dashboard() -> Html {
    let stats = use_state(|| None::<NodeStats>);
    let cluster = use_state(|| None::<ClusterStatus>);
    let loading = use_state(|| true);

    // Fetch data on mount
    {
        let stats = stats.clone();
        let cluster = cluster.clone();
        let loading = loading.clone();

        use_effect_with((), move |_| {
            let stats_initial = stats.clone();
            let cluster_initial = cluster.clone();
            let loading_initial = loading.clone();

            spawn_local(async move {
                if let Ok(node_stats) = ApiClient::get_node_stats().await {
                    stats_initial.set(Some(node_stats));
                }

                if let Ok(cluster_status) = ApiClient::get_cluster_status().await {
                    cluster_initial.set(Some(cluster_status));
                }

                loading_initial.set(false);
            });

            // Refresh every 10 seconds
            let stats_refresh = stats.clone();
            let cluster_refresh = cluster.clone();
            let interval = Interval::new(10_000, move || {
                let stats = stats_refresh.clone();
                let cluster = cluster_refresh.clone();

                spawn_local(async move {
                    if let Ok(node_stats) = ApiClient::get_node_stats().await {
                        stats.set(Some(node_stats));
                    }

                    if let Ok(cluster_status) = ApiClient::get_cluster_status().await {
                        cluster.set(Some(cluster_status));
                    }
                });
            });

            move || drop(interval)
        });
    }

    if *loading {
        return html! { <Loading /> };
    }

    html! {
        <div class="dashboard-page">
            <Header title="Dashboard" />

            <div class="page-content">
                // Cluster status card
                {if let Some(ref cluster_status) = *cluster {
                    html! {
                        <Card title="Cluster Status">
                            <div class="status-grid">
                                <div class="status-item">
                                    <span class="label">{"Name:"}</span>
                                    <span class="value">{&cluster_status.name}</span>
                                </div>
                                <div class="status-item">
                                    <span class="label">{"Nodes:"}</span>
                                    <span class="value">{cluster_status.nodes.len()}</span>
                                </div>
                                <div class="status-item">
                                    <span class="label">{"Quorum:"}</span>
                                    <span class={if cluster_status.quorum { "value success" } else { "value error" }}>
                                        {if cluster_status.quorum { "Yes" } else { "No" }}
                                    </span>
                                </div>
                            </div>
                        </Card>
                    }
                } else {
                    html! {}
                }}

                // Node stats card
                {if let Some(ref node_stats) = *stats {
                    html! {
                        <Card title="Node Statistics">
                            <div class="stats-grid">
                                <div class="stat-item">
                                    <div class="stat-label">{"CPU Usage"}</div>
                                    <div class="stat-value">{format!("{:.1}%", node_stats.cpu_usage)}</div>
                                    <div class="progress-bar">
                                        <div class="progress-fill" style={format!("width: {}%", node_stats.cpu_usage)}></div>
                                    </div>
                                </div>

                                <div class="stat-item">
                                    <div class="stat-label">{"Memory Usage"}</div>
                                    <div class="stat-value">
                                        {format!("{:.1}%", (node_stats.memory_used as f64 / node_stats.memory_total as f64) * 100.0)}
                                    </div>
                                    <div class="progress-bar">
                                        <div class="progress-fill" style={format!("width: {}%", (node_stats.memory_used as f64 / node_stats.memory_total as f64) * 100.0)}></div>
                                    </div>
                                </div>

                                <div class="stat-item">
                                    <div class="stat-label">{"Load Average"}</div>
                                    <div class="stat-value">
                                        {format!("{:.2} / {:.2} / {:.2}", node_stats.load_average.0, node_stats.load_average.1, node_stats.load_average.2)}
                                    </div>
                                </div>

                                <div class="stat-item">
                                    <div class="stat-label">{"Uptime"}</div>
                                    <div class="stat-value">
                                        {format!("{} days", node_stats.uptime / 86400)}
                                    </div>
                                </div>
                            </div>
                        </Card>
                    }
                } else {
                    html! {}
                }}

                // Quick actions
                <Card title="Quick Actions">
                    <div class="action-grid">
                        <a href="/vms" class="action-button">
                            <span class="icon">{"💻"}</span>
                            <span class="label">{"Virtual Machines"}</span>
                        </a>
                        <a href="/cluster" class="action-button">
                            <span class="icon">{"🔗"}</span>
                            <span class="label">{"Cluster Nodes"}</span>
                        </a>
                        <a href="/storage" class="action-button">
                            <span class="icon">{"💾"}</span>
                            <span class="label">{"Storage"}</span>
                        </a>
                        <a href="/network" class="action-button">
                            <span class="icon">{"🌐"}</span>
                            <span class="label">{"Network"}</span>
                        </a>
                    </div>
                </Card>
            </div>
        </div>
    }
}
