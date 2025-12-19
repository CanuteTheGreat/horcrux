//! Kubernetes Multi-Cluster Dashboard
//!
//! Comprehensive cluster management interface with multi-cluster overview,
//! health monitoring, and cluster operations.

use leptos::*;
use crate::api;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Kubernetes cluster configuration
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
    pub kubeconfig: Option<String>,
}

/// Cluster health information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterHealth {
    pub cluster_id: String,
    pub status: String,
    pub api_server_healthy: bool,
    pub etcd_healthy: bool,
    pub scheduler_healthy: bool,
    pub controller_manager_healthy: bool,
    pub node_status: NodeHealthSummary,
    pub resource_usage: ResourceUsage,
}

/// Node health summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeHealthSummary {
    pub total_nodes: u32,
    pub ready_nodes: u32,
    pub not_ready_nodes: u32,
    pub unknown_nodes: u32,
}

/// Resource usage summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceUsage {
    pub cpu_total: String,
    pub cpu_used: String,
    pub memory_total: String,
    pub memory_used: String,
    pub storage_total: String,
    pub storage_used: String,
}

/// Kubernetes node information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct K8sNode {
    pub name: String,
    pub status: String,
    pub roles: Vec<String>,
    pub version: String,
    pub cpu_capacity: String,
    pub memory_capacity: String,
    pub pods_capacity: String,
    pub cpu_allocatable: String,
    pub memory_allocatable: String,
    pub pods_allocatable: String,
    pub conditions: Vec<NodeCondition>,
}

/// Node condition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeCondition {
    pub condition_type: String,
    pub status: String,
    pub reason: Option<String>,
    pub message: Option<String>,
}

/// Cluster dashboard component
#[component]
pub fn ClusterDashboard() -> impl IntoView {
    let (clusters, set_clusters) = create_signal(Vec::<K8sCluster>::new());
    let (cluster_health, set_cluster_health) = create_signal(HashMap::<String, ClusterHealth>::new());
    let (selected_cluster, set_selected_cluster) = create_signal(None::<String>);
    let (loading, set_loading) = create_signal(true);
    let (error_message, set_error_message) = create_signal(None::<String>);
    let (show_add_cluster, set_show_add_cluster) = create_signal(false);
    let (auto_refresh, set_auto_refresh) = create_signal(true);

    // Load clusters and health data
    let load_data = move || {
        spawn_local(async move {
            set_loading.set(true);
            set_error_message.set(None);

            // Load clusters
            match api::fetch_json::<Vec<K8sCluster>>("/k8s/clusters").await {
                Ok(clusters_data) => {
                    set_clusters.set(clusters_data.clone());

                    // Load health for each cluster
                    let mut health_map = HashMap::new();
                    for cluster in clusters_data {
                        if let Ok(health) = api::fetch_json::<ClusterHealth>(
                            &format!("/k8s/clusters/{}/health", cluster.id)
                        ).await {
                            health_map.insert(cluster.id, health);
                        }
                    }
                    set_cluster_health.set(health_map);
                }
                Err(e) => {
                    set_error_message.set(Some(format!("Failed to load clusters: {}", e)));
                }
            }

            set_loading.set(false);
        });
    };

    // Initial load
    create_effect(move |_| {
        load_data();
    });

    // Auto-refresh every 30 seconds
    create_effect(move |_| {
        if auto_refresh.get() {
            let timer = set_interval_with_handle(
                move || {
                    if auto_refresh.get() {
                        load_data();
                    }
                },
                std::time::Duration::from_secs(30),
            );

            if let Ok(handle) = timer {
                on_cleanup(move || {
                    handle.clear();
                });
            }
        }
    });

    // Delete cluster
    let delete_cluster = move |cluster_id: String| {
        spawn_local(async move {
            match api::delete_json(&format!("/k8s/clusters/{}", cluster_id)).await {
                Ok(()) => {
                    load_data();
                }
                Err(e) => {
                    set_error_message.set(Some(format!("Failed to delete cluster: {}", e)));
                }
            }
        });
    };

    // Calculate cluster statistics
    let cluster_stats = move || {
        let clusters_list = clusters.get();
        let health_map = cluster_health.get();

        let total_clusters = clusters_list.len();
        let healthy_clusters = health_map.values()
            .filter(|h| h.status == "healthy")
            .count();
        let total_nodes: u32 = clusters_list.iter()
            .map(|c| c.node_count)
            .sum();
        let ready_nodes: u32 = health_map.values()
            .map(|h| h.node_status.ready_nodes)
            .sum();

        (total_clusters, healthy_clusters, total_nodes, ready_nodes)
    };

    view! {
        <div class="k8s-dashboard">
            <div class="dashboard-header">
                <div class="header-content">
                    <h1>"Kubernetes Cluster Management"</h1>
                    <p class="description">
                        "Multi-cluster management dashboard with health monitoring and operations"
                    </p>
                </div>

                <div class="header-actions">
                    <label class="auto-refresh-toggle">
                        <input
                            type="checkbox"
                            checked=move || auto_refresh.get()
                            on:change=move |_| set_auto_refresh.update(|r| *r = !*r)
                        />
                        <span class="toggle-label">"Auto-refresh"</span>
                    </label>

                    <button
                        class="btn btn-secondary"
                        on:click=move |_| load_data()
                        disabled=move || loading.get()
                    >
                        <span class="icon">"🔄"</span>
                        "Refresh"
                    </button>

                    <button
                        class="btn btn-primary"
                        on:click=move |_| set_show_add_cluster.set(!show_add_cluster.get())
                    >
                        <span class="icon">"+"</span>
                        {move || if show_add_cluster.get() { "Cancel" } else { "Add Cluster" }}
                    </button>
                </div>
            </div>

            {move || error_message.get().map(|msg| view! {
                <div class="alert alert-error">
                    <span class="alert-icon">"!"</span>
                    <span class="alert-message">{msg}</span>
                    <button
                        class="alert-close"
                        on:click=move |_| set_error_message.set(None)
                    >"x"</button>
                </div>
            })}

            <div class="cluster-stats">
                {move || {
                    let (total, healthy, total_nodes, ready_nodes) = cluster_stats();
                    view! {
                        <div class="stats-grid">
                            <div class="stat-card">
                                <div class="stat-value">{total}</div>
                                <div class="stat-label">"Clusters"</div>
                            </div>
                            <div class="stat-card" class:success=move || { healthy == total && total > 0 }>
                                <div class="stat-value">{healthy}</div>
                                <div class="stat-label">"Healthy"</div>
                            </div>
                            <div class="stat-card">
                                <div class="stat-value">{total_nodes}</div>
                                <div class="stat-label">"Total Nodes"</div>
                            </div>
                            <div class="stat-card" class:success=move || { ready_nodes == total_nodes && total_nodes > 0 }>
                                <div class="stat-value">{ready_nodes}</div>
                                <div class="stat-label">"Ready Nodes"</div>
                            </div>
                        </div>
                    }
                }}
            </div>

            {move || if show_add_cluster.get() {
                view! {
                    <AddClusterForm
                        on_success=move || {
                            set_show_add_cluster.set(false);
                            load_data();
                        }
                        on_cancel=move || set_show_add_cluster.set(false)
                        on_error=move |msg| set_error_message.set(Some(msg))
                    />
                }.into_view()
            } else {
                view! {}.into_view()
            }}

            <div class="clusters-container">
                {move || if loading.get() && clusters.get().is_empty() {
                    view! {
                        <div class="loading-container">
                            <div class="spinner"></div>
                            <p>"Loading clusters..."</p>
                        </div>
                    }.into_view()
                } else {
                    let clusters_list = clusters.get();
                    if clusters_list.is_empty() {
                        view! {
                            <div class="empty-state">
                                <div class="empty-icon">"⚙️"</div>
                                <h3>"No Kubernetes Clusters"</h3>
                                <p>"Add your first cluster to start managing Kubernetes workloads"</p>
                                <button
                                    class="btn btn-primary"
                                    on:click=move |_| set_show_add_cluster.set(true)
                                >
                                    "Add First Cluster"
                                </button>
                            </div>
                        }.into_view()
                    } else {
                        view! {
                            <div class="clusters-grid">
                                {clusters_list.into_iter().map(|cluster| {
                                    let cluster_id = cluster.id.clone();
                                    let cluster_id_delete = cluster.id.clone();
                                    let cluster_id_select = cluster.id.clone();
                                    let health = cluster_health.get().get(&cluster.id).cloned();

                                    view! {
                                        <ClusterCard
                                            cluster=cluster
                                            health=health
                                            on_select=move || set_selected_cluster.set(Some(cluster_id_select.clone()))
                                            on_delete=move || {
                                                if web_sys::window()
                                                    .unwrap()
                                                    .confirm_with_message(&format!("Delete cluster '{}'? This will remove all associated resources.", &cluster_id_delete))
                                                    .unwrap_or(false)
                                                {
                                                    delete_cluster(cluster_id_delete.clone());
                                                }
                                            }
                                        />
                                    }
                                }).collect_view()}
                            </div>
                        }.into_view()
                    }
                }}
            </div>

            {move || selected_cluster.get().map(|cluster_id| view! {
                <ClusterDetailModal
                    cluster_id=cluster_id
                    on_close=move || set_selected_cluster.set(None)
                />
            })}
        </div>
    }
}

/// Cluster card component
#[component]
pub fn ClusterCard<F, G>(
    cluster: K8sCluster,
    health: Option<ClusterHealth>,
    on_select: F,
    on_delete: G,
) -> impl IntoView
where
    F: Fn() + 'static,
    G: Fn() + 'static,
{
    let status_class = match health.as_ref() {
        Some(h) if h.status == "healthy" => "healthy",
        Some(h) if h.status == "degraded" => "warning",
        Some(_) => "error",
        None => "unknown",
    };

    let format_timestamp = |timestamp: i64| {
        let dt = chrono::DateTime::from_timestamp(timestamp, 0);
        match dt {
            Some(dt) => dt.format("%Y-%m-%d").to_string(),
            None => "Unknown".to_string(),
        }
    };

    view! {
        <div class={format!("cluster-card {}", status_class)}>
            <div class="cluster-header">
                <div class="cluster-title">
                    <h3>{&cluster.name}</h3>
                    <span class={format!("status-badge {}", status_class)}>
                        {health.as_ref().map(|h| h.status.clone()).unwrap_or_else(|| "Unknown".to_string())}
                    </span>
                </div>
                <div class="cluster-actions">
                    <button
                        class="btn-icon"
                        title="View Details"
                        on:click=move |_| on_select()
                    >
                        "👁"
                    </button>
                    <button
                        class="btn-icon delete-btn"
                        title="Delete Cluster"
                        on:click=move |_| on_delete()
                    >
                        "🗑"
                    </button>
                </div>
            </div>

            <div class="cluster-info">
                <div class="info-row">
                    <span class="label">"Context:"</span>
                    <span class="value">{&cluster.context}</span>
                </div>
                <div class="info-row">
                    <span class="label">"Provider:"</span>
                    <span class="value">{&cluster.provider}</span>
                </div>
                <div class="info-row">
                    <span class="label">"Version:"</span>
                    <span class="value">{cluster.version.unwrap_or_else(|| "Unknown".to_string())}</span>
                </div>
                <div class="info-row">
                    <span class="label">"Nodes:"</span>
                    <span class="value">{cluster.node_count.to_string()}</span>
                </div>
                <div class="info-row">
                    <span class="label">"Created:"</span>
                    <span class="value">{format_timestamp(cluster.created_at)}</span>
                </div>
            </div>

            {health.map(|h| view! {
                <div class="cluster-health">
                    <h4>"Component Health"</h4>
                    <div class="health-grid">
                        <div class={format!("health-item {}", if h.api_server_healthy { "healthy" } else { "unhealthy" })}>
                            <span class="health-icon">{if h.api_server_healthy { "[OK]" } else { "[X]" }}</span>
                            <span>"API Server"</span>
                        </div>
                        <div class={format!("health-item {}", if h.etcd_healthy { "healthy" } else { "unhealthy" })}>
                            <span class="health-icon">{if h.etcd_healthy { "[OK]" } else { "[X]" }}</span>
                            <span>"etcd"</span>
                        </div>
                        <div class={format!("health-item {}", if h.scheduler_healthy { "healthy" } else { "unhealthy" })}>
                            <span class="health-icon">{if h.scheduler_healthy { "[OK]" } else { "[X]" }}</span>
                            <span>"Scheduler"</span>
                        </div>
                        <div class={format!("health-item {}", if h.controller_manager_healthy { "healthy" } else { "unhealthy" })}>
                            <span class="health-icon">{if h.controller_manager_healthy { "[OK]" } else { "[X]" }}</span>
                            <span>"Controller"</span>
                        </div>
                    </div>
                </div>
            })}
        </div>
    }
}

/// Add cluster form component
#[component]
pub fn AddClusterForm<F, G, H>(
    on_success: F,
    on_cancel: G,
    on_error: H,
) -> impl IntoView
where
    F: Fn() + 'static,
    G: Fn() + 'static,
    H: Fn(String) + 'static,
{
    let (cluster_name, set_cluster_name) = create_signal(String::new());
    let (kubeconfig_content, set_kubeconfig_content) = create_signal(String::new());
    let (context_name, set_context_name) = create_signal(String::new());
    let (provider, set_provider) = create_signal("generic".to_string());
    let (submitting, set_submitting) = create_signal(false);

    // Wrap callbacks in Rc early so they can be cloned into closures
    let on_success_rc = std::rc::Rc::new(on_success);
    let on_error_rc = std::rc::Rc::new(on_error);

    let is_valid = move || {
        !cluster_name.get().trim().is_empty() &&
        !kubeconfig_content.get().trim().is_empty()
    };

    let on_success_clone = on_success_rc.clone();
    let on_error_clone = on_error_rc.clone();
    let submit_form = move |ev: web_sys::SubmitEvent| {
        ev.prevent_default();

        if !is_valid() {
            return;
        }

        #[derive(Serialize)]
        struct CreateClusterRequest {
            name: String,
            kubeconfig: String,
            context: Option<String>,
            provider: String,
        }

        let request = CreateClusterRequest {
            name: cluster_name.get().trim().to_string(),
            kubeconfig: kubeconfig_content.get().trim().to_string(),
            context: if context_name.get().trim().is_empty() {
                None
            } else {
                Some(context_name.get().trim().to_string())
            },
            provider: provider.get(),
        };

        set_submitting.set(true);

        let on_success_inner = on_success_clone.clone();
        let on_error_inner = on_error_clone.clone();

        spawn_local(async move {
            match api::post_json::<K8sCluster, _>("/k8s/clusters", &request).await {
                Ok(_) => {
                    on_success_inner();
                }
                Err(e) => {
                    on_error_inner(format!("Failed to add cluster: {}", e));
                }
            }
            set_submitting.set(false);
        });
    };

    view! {
        <div class="add-cluster-form-container">
            <form class="add-cluster-form" on:submit=submit_form>
                <h2>"Add Kubernetes Cluster"</h2>

                <div class="form-group">
                    <label for="cluster-name">"Cluster Name *"</label>
                    <input
                        type="text"
                        id="cluster-name"
                        prop:value=move || cluster_name.get()
                        on:input=move |ev| set_cluster_name.set(event_target_value(&ev))
                        placeholder="e.g., production-cluster"
                        required
                    />
                </div>

                <div class="form-group">
                    <label for="kubeconfig">"Kubeconfig Content *"</label>
                    <textarea
                        id="kubeconfig"
                        prop:value=move || kubeconfig_content.get()
                        on:input=move |ev| set_kubeconfig_content.set(event_target_value(&ev))
                        placeholder="Paste your kubeconfig YAML content here..."
                        rows="12"
                        required
                    ></textarea>
                    <small>"Paste the complete kubeconfig file content"</small>
                </div>

                <div class="form-row">
                    <div class="form-group">
                        <label for="context">"Context Name (Optional)"</label>
                        <input
                            type="text"
                            id="context"
                            prop:value=move || context_name.get()
                            on:input=move |ev| set_context_name.set(event_target_value(&ev))
                            placeholder="Leave empty to use default"
                        />
                    </div>

                    <div class="form-group">
                        <label for="provider">"Provider"</label>
                        <select
                            id="provider"
                            prop:value=move || provider.get()
                            on:change=move |ev| set_provider.set(event_target_value(&ev))
                        >
                            <option value="generic">"Generic"</option>
                            <option value="eks">"Amazon EKS"</option>
                            <option value="gke">"Google GKE"</option>
                            <option value="aks">"Azure AKS"</option>
                            <option value="k3s">"K3s"</option>
                            <option value="kind">"Kind"</option>
                            <option value="microk8s">"MicroK8s"</option>
                            <option value="openshift">"OpenShift"</option>
                        </select>
                    </div>
                </div>

                <div class="form-actions">
                    <button
                        type="button"
                        class="btn btn-secondary"
                        on:click=move |_| on_cancel()
                    >
                        "Cancel"
                    </button>
                    <button
                        type="submit"
                        class="btn btn-primary"
                        disabled=move || submitting.get() || !is_valid()
                    >
                        {move || if submitting.get() { "Adding..." } else { "Add Cluster" }}
                    </button>
                </div>
            </form>
        </div>
    }
}

/// Cluster detail modal component
#[component]
pub fn ClusterDetailModal<F>(
    cluster_id: String,
    on_close: F,
) -> impl IntoView
where
    F: Fn() + Clone + 'static,
{
    let (cluster, set_cluster) = create_signal(None::<K8sCluster>);
    let (nodes, set_nodes) = create_signal(Vec::<K8sNode>::new());
    let (loading, set_loading) = create_signal(true);

    // Load cluster details
    create_effect(move |_| {
        let cluster_id = cluster_id.clone();
        spawn_local(async move {
            set_loading.set(true);

            // Load cluster info
            if let Ok(cluster_data) = api::fetch_json::<K8sCluster>(&format!("/k8s/clusters/{}", cluster_id)).await {
                set_cluster.set(Some(cluster_data));
            }

            // Load nodes
            if let Ok(nodes_data) = api::fetch_json::<Vec<K8sNode>>(&format!("/k8s/clusters/{}/nodes", cluster_id)).await {
                set_nodes.set(nodes_data);
            }

            set_loading.set(false);
        });
    });

    let on_close_clone = on_close.clone();

    view! {
        <div class="modal-overlay" on:click=move |_| on_close_clone()>
            <div class="modal-content cluster-detail-modal" on:click=move |ev| ev.stop_propagation()>
                <div class="modal-header">
                    <h2>"Cluster Details"</h2>
                    <button class="modal-close" on:click=move |_| on_close()>"x"</button>
                </div>

                <div class="modal-body">
                    {move || if loading.get() {
                        view! {
                            <div class="loading-container">
                                <div class="spinner"></div>
                                <p>"Loading cluster details..."</p>
                            </div>
                        }.into_view()
                    } else {
                        view! {
                            <div class="cluster-details">
                                {cluster.get().map(|c| view! {
                                    <div class="cluster-overview">
                                        <h3>{&c.name}</h3>
                                        <div class="detail-grid">
                                            <div class="detail-item">
                                                <span class="label">"Context:"</span>
                                                <span class="value">{&c.context}</span>
                                            </div>
                                            <div class="detail-item">
                                                <span class="label">"API Server:"</span>
                                                <span class="value">{&c.api_server}</span>
                                            </div>
                                            <div class="detail-item">
                                                <span class="label">"Version:"</span>
                                                <span class="value">{c.version.clone().unwrap_or_else(|| "Unknown".to_string())}</span>
                                            </div>
                                            <div class="detail-item">
                                                <span class="label">"Provider:"</span>
                                                <span class="value">{&c.provider}</span>
                                            </div>
                                        </div>
                                    </div>
                                })}

                                <div class="nodes-section">
                                    <h4>"Cluster Nodes"</h4>
                                    {move || {
                                        let nodes_list = nodes.get();
                                        if nodes_list.is_empty() {
                                            view! {
                                                <p>"No nodes found"</p>
                                            }.into_view()
                                        } else {
                                            view! {
                                                <div class="nodes-table">
                                                    <table>
                                                        <thead>
                                                            <tr>
                                                                <th>"Name"</th>
                                                                <th>"Status"</th>
                                                                <th>"Roles"</th>
                                                                <th>"Version"</th>
                                                                <th>"CPU"</th>
                                                                <th>"Memory"</th>
                                                            </tr>
                                                        </thead>
                                                        <tbody>
                                                            {nodes_list.into_iter().map(|node| {
                                                                view! {
                                                                    <tr>
                                                                        <td>{&node.name}</td>
                                                                        <td>
                                                                            <span class={format!("status-badge {}",
                                                                                if node.status == "Ready" { "success" } else { "error" }
                                                                            )}>
                                                                                {&node.status}
                                                                            </span>
                                                                        </td>
                                                                        <td>{node.roles.join(", ")}</td>
                                                                        <td>{&node.version}</td>
                                                                        <td>{format!("{} / {}", node.cpu_allocatable, node.cpu_capacity)}</td>
                                                                        <td>{format!("{} / {}", node.memory_allocatable, node.memory_capacity)}</td>
                                                                    </tr>
                                                                }
                                                            }).collect_view()}
                                                        </tbody>
                                                    </table>
                                                </div>
                                            }.into_view()
                                        }
                                    }}
                                </div>
                            </div>
                        }.into_view()
                    }}
                </div>
            </div>
        </div>
    }
}

// Utility function to set interval with cleanup
fn set_interval_with_handle<F>(f: F, duration: std::time::Duration) -> Result<IntervalHandle, wasm_bindgen::JsValue>
where
    F: Fn() + 'static,
{
    use wasm_bindgen::{closure::Closure, JsCast};

    let callback = Closure::wrap(Box::new(f) as Box<dyn Fn()>);
    let handle = web_sys::window()
        .unwrap()
        .set_interval_with_callback_and_timeout_and_arguments_0(
            callback.as_ref().unchecked_ref(),
            duration.as_millis() as i32,
        )?;

    callback.forget(); // Prevent the closure from being dropped
    Ok(IntervalHandle { handle })
}

// Handle for managing intervals
struct IntervalHandle {
    handle: i32,
}

impl IntervalHandle {
    fn clear(self) {
        web_sys::window().unwrap().clear_interval_with_handle(self.handle);
    }
}