//! Kubernetes Cluster Management Page

use leptos::*;
use serde::{Deserialize, Serialize};
use crate::api::{fetch_json, delete_json, post_empty};

/// Kubernetes cluster
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct K8sCluster {
    pub id: String,
    pub name: String,
    pub context: String,
    pub api_server: String,
    pub version: Option<String>,
    pub status: String,
    pub node_count: u32,
    pub provider: String,
    pub created_at: i64,
}

/// Cluster health response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterHealth {
    pub status: String,
    pub api_server_healthy: bool,
    pub etcd_healthy: bool,
    pub scheduler_healthy: bool,
    pub controller_manager_healthy: bool,
}

/// Kubernetes Management Page Component
#[component]
pub fn KubernetesManagement() -> impl IntoView {
    let (clusters, set_clusters) = create_signal(Vec::<K8sCluster>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);
    let (show_connect_modal, set_show_connect_modal) = create_signal(false);

    // Fetch clusters on mount
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);
            match fetch_json::<Vec<K8sCluster>>("/api/k8s/clusters").await {
                Ok(data) => {
                    set_clusters.set(data);
                    set_loading.set(false);
                }
                Err(e) => {
                    set_error.set(Some(format!("Failed to load clusters: {}", e.message)));
                    set_loading.set(false);
                }
            }
        });
    });

    // Disconnect cluster
    let disconnect_cluster = move |cluster_id: String| {
        spawn_local(async move {
            let url = format!("/api/k8s/clusters/{}", cluster_id);
            match delete_json(&url).await {
                Ok(()) => {
                    // Refresh cluster list
                    if let Ok(data) = fetch_json::<Vec<K8sCluster>>("/api/k8s/clusters").await {
                        set_clusters.set(data);
                    }
                }
                Err(e) => {
                    set_error.set(Some(format!("Disconnect failed: {}", e.message)));
                }
            }
        });
    };

    // Reconnect cluster
    let reconnect_cluster = move |cluster_id: String| {
        spawn_local(async move {
            let url = format!("/api/k8s/clusters/{}/reconnect", cluster_id);
            match post_empty(&url).await {
                Ok(()) => {
                    // Refresh cluster list
                    if let Ok(data) = fetch_json::<Vec<K8sCluster>>("/api/k8s/clusters").await {
                        set_clusters.set(data);
                    }
                }
                Err(e) => {
                    set_error.set(Some(format!("Reconnect failed: {}", e.message)));
                }
            }
        });
    };

    view! {
        <div class="page kubernetes-management">
            <header class="page-header">
                <h2>"Kubernetes Clusters"</h2>
                <button class="btn btn-primary" on:click=move |_| set_show_connect_modal.set(true)>
                    "+ Connect Cluster"
                </button>
            </header>

            // Error display
            {move || error.get().map(|e| view! {
                <div class="alert alert-error">
                    <span>{e}</span>
                    <button class="btn-close" on:click=move |_| set_error.set(None)>"x"</button>
                </div>
            })}

            // Loading state
            {move || loading.get().then(|| view! {
                <div class="loading">
                    <div class="spinner"></div>
                    <span>"Loading clusters..."</span>
                </div>
            })}

            // Cluster List
            {move || if !loading.get() {
                view! {
                    <div class="cluster-grid">
                        {move || clusters.get().into_iter().map(|cluster| {
                            let cluster_id = cluster.id.clone();
                            let cluster_id2 = cluster.id.clone();
                            let cluster_id3 = cluster.id.clone();
                            let is_connected = cluster.status == "connected" || cluster.status == "Connected";

                            view! {
                                <div class={format!("cluster-card status-{}", cluster.status.to_lowercase())}>
                                    <div class="cluster-header">
                                        <div class="cluster-info">
                                            <h3>{&cluster.name}</h3>
                                            <span class={format!("status-badge {}", cluster.status.to_lowercase())}>
                                                {&cluster.status}
                                            </span>
                                        </div>
                                        <span class="provider-badge">{&cluster.provider}</span>
                                    </div>

                                    <div class="cluster-details">
                                        <div class="detail-row">
                                            <span class="label">"API Server:"</span>
                                            <code class="api-server">{&cluster.api_server}</code>
                                        </div>
                                        <div class="detail-row">
                                            <span class="label">"Context:"</span>
                                            <span>{&cluster.context}</span>
                                        </div>
                                        <div class="detail-row">
                                            <span class="label">"Version:"</span>
                                            <span>{cluster.version.clone().unwrap_or_else(|| "Unknown".to_string())}</span>
                                        </div>
                                        <div class="detail-row">
                                            <span class="label">"Nodes:"</span>
                                            <span class="node-count">{cluster.node_count}</span>
                                        </div>
                                    </div>

                                    <div class="cluster-stats">
                                        <div class="stat">
                                            <span class="stat-value">{cluster.node_count}</span>
                                            <span class="stat-label">"Nodes"</span>
                                        </div>
                                    </div>

                                    <div class="cluster-actions">
                                        {if is_connected {
                                            view! {
                                                <div class="btn-group">
                                                    <a href={format!("/kubernetes/{}/dashboard", cluster_id)} class="btn btn-primary">
                                                        "Dashboard"
                                                    </a>
                                                    <a href={format!("/kubernetes/{}/pods", cluster_id)} class="btn btn-secondary">
                                                        "Pods"
                                                    </a>
                                                    <a href={format!("/kubernetes/{}/deployments", cluster_id)} class="btn btn-secondary">
                                                        "Deployments"
                                                    </a>
                                                    <a href={format!("/kubernetes/{}/services", cluster_id)} class="btn btn-secondary">
                                                        "Services"
                                                    </a>
                                                </div>
                                                <button class="btn btn-warning"
                                                        on:click=move |_| disconnect_cluster(cluster_id2.clone())>
                                                    "Disconnect"
                                                </button>
                                            }.into_view()
                                        } else {
                                            view! {
                                                <button class="btn btn-success"
                                                        on:click=move |_| reconnect_cluster(cluster_id3.clone())>
                                                    "Reconnect"
                                                </button>
                                            }.into_view()
                                        }}
                                    </div>
                                </div>
                            }
                        }).collect_view()}
                    </div>
                }.into_view()
            } else {
                view! { <div></div> }.into_view()
            }}

            // Empty state
            {move || if !loading.get() && clusters.get().is_empty() {
                view! {
                    <div class="empty-state">
                        <div class="icon">"☸️"</div>
                        <h3>"No Kubernetes Clusters"</h3>
                        <p>"Connect to an existing cluster or provision a new one."</p>
                        <button class="btn btn-primary" on:click=move |_| set_show_connect_modal.set(true)>
                            "Connect Cluster"
                        </button>
                    </div>
                }.into_view()
            } else {
                view! { <div></div> }.into_view()
            }}

            // Connect Cluster Modal (simplified - would need full form)
            {move || show_connect_modal.get().then(|| view! {
                <div class="modal-overlay" on:click=move |_| set_show_connect_modal.set(false)>
                    <div class="modal" on:click=|e| e.stop_propagation()>
                        <div class="modal-header">
                            <h3>"Connect Kubernetes Cluster"</h3>
                            <button class="btn-close" on:click=move |_| set_show_connect_modal.set(false)>
                                "x"
                            </button>
                        </div>
                        <div class="modal-body">
                            <div class="form-group">
                                <label>"Cluster Name"</label>
                                <input type="text" class="form-control" placeholder="production-cluster"/>
                            </div>
                            <div class="form-group">
                                <label>"Kubeconfig"</label>
                                <textarea class="form-control" rows="10" placeholder="Paste your kubeconfig here..."></textarea>
                            </div>
                            <div class="form-group">
                                <label>"Context (Optional)"</label>
                                <input type="text" class="form-control" placeholder="Leave empty for default"/>
                            </div>
                        </div>
                        <div class="modal-footer">
                            <button class="btn btn-secondary" on:click=move |_| set_show_connect_modal.set(false)>
                                "Cancel"
                            </button>
                            <button class="btn btn-primary">
                                "Connect"
                            </button>
                        </div>
                    </div>
                </div>
            })}

            // Info section
            <div class="info-box">
                <h4>"Kubernetes Integration"</h4>
                <ul>
                    <li>"Connect existing clusters via kubeconfig"</li>
                    <li>"Provision k3s or kubeadm clusters"</li>
                    <li>"Manage workloads, services, and configuration"</li>
                    <li>"Helm chart deployment support"</li>
                    <li>"Real-time metrics and event monitoring"</li>
                </ul>
            </div>
        </div>
    }
}
