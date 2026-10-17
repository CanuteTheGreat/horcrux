//! Kubernetes HTTP handlers (USE=kubernetes), moved out of app/mod.rs.

use super::*;
use crate::kubernetes::types::*;

// Cluster management handlers

pub async fn list_clusters(State(state): State<Arc<AppState>>) -> Json<Vec<K8sCluster>> {
    let clusters = state.kubernetes_manager.list_clusters().await;
    Json(clusters)
}

pub async fn connect_cluster(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ClusterConnectRequest>,
) -> Result<(StatusCode, Json<K8sCluster>), ApiError> {
    let cluster = state.kubernetes_manager.connect_cluster(payload).await?;
    Ok((StatusCode::CREATED, Json(cluster)))
}

pub async fn get_cluster(
    State(state): State<Arc<AppState>>,
    Path(cluster_id): Path<String>,
) -> Result<Json<K8sCluster>, ApiError> {
    let cluster = state.kubernetes_manager.get_cluster(&cluster_id).await?;
    Ok(Json(cluster))
}

pub async fn delete_cluster(
    State(state): State<Arc<AppState>>,
    Path(cluster_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.kubernetes_manager.delete_cluster(&cluster_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn reconnect_cluster(
    State(state): State<Arc<AppState>>,
    Path(cluster_id): Path<String>,
) -> Result<Json<K8sCluster>, ApiError> {
    let cluster = state
        .kubernetes_manager
        .reconnect_cluster(&cluster_id)
        .await?;
    Ok(Json(cluster))
}

pub async fn get_cluster_health(
    State(state): State<Arc<AppState>>,
    Path(cluster_id): Path<String>,
) -> Result<Json<ClusterHealth>, ApiError> {
    let health = state
        .kubernetes_manager
        .check_cluster_health(&cluster_id)
        .await?;
    Ok(Json(health))
}

pub async fn get_cluster_version(
    State(state): State<Arc<AppState>>,
    Path(cluster_id): Path<String>,
) -> Result<Json<K8sVersion>, ApiError> {
    let version = state
        .kubernetes_manager
        .get_cluster_version(&cluster_id)
        .await?;
    Ok(Json(version))
}

// Namespace handlers

pub async fn list_namespaces(
    State(state): State<Arc<AppState>>,
    Path(cluster_id): Path<String>,
) -> Result<Json<Vec<NamespaceInfo>>, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let namespaces =
        crate::kubernetes::cluster_resources::namespaces::list_namespaces(&client).await?;
    Ok(Json(namespaces))
}

pub async fn create_namespace(
    State(state): State<Arc<AppState>>,
    Path(cluster_id): Path<String>,
    Json(payload): Json<CreateNamespaceRequest>,
) -> Result<(StatusCode, Json<NamespaceInfo>), ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let ns = crate::kubernetes::cluster_resources::namespaces::create_namespace(&client, &payload)
        .await?;
    Ok((StatusCode::CREATED, Json(ns)))
}

pub async fn get_namespace(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace)): Path<(String, String)>,
) -> Result<Json<NamespaceInfo>, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let ns = crate::kubernetes::cluster_resources::namespaces::get_namespace(&client, &namespace)
        .await?;
    Ok(Json(ns))
}

pub async fn delete_namespace(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    crate::kubernetes::cluster_resources::namespaces::delete_namespace(&client, &namespace).await?;
    Ok(StatusCode::NO_CONTENT)
}

// Node handlers

pub async fn list_nodes(
    State(state): State<Arc<AppState>>,
    Path(cluster_id): Path<String>,
) -> Result<Json<Vec<NodeInfo>>, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let nodes = crate::kubernetes::cluster_resources::nodes::list_nodes(&client).await?;
    Ok(Json(nodes))
}

pub async fn get_node(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, node)): Path<(String, String)>,
) -> Result<Json<NodeInfo>, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let node_info = crate::kubernetes::cluster_resources::nodes::get_node(&client, &node).await?;
    Ok(Json(node_info))
}

pub async fn cordon_node(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, node)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    crate::kubernetes::cluster_resources::nodes::cordon_node(&client, &node).await?;
    Ok(StatusCode::OK)
}

pub async fn uncordon_node(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, node)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    crate::kubernetes::cluster_resources::nodes::uncordon_node(&client, &node).await?;
    Ok(StatusCode::OK)
}

#[derive(Deserialize)]
pub struct DrainNodeRequest {
    #[serde(default)]
    pub ignore_daemonsets: bool,
    #[serde(default)]
    pub delete_emptydir_data: bool,
    pub grace_period_seconds: Option<i64>,
}

pub async fn drain_node(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, node)): Path<(String, String)>,
    Json(payload): Json<DrainNodeRequest>,
) -> Result<StatusCode, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    crate::kubernetes::cluster_resources::nodes::drain_node(
        &client,
        &node,
        payload.ignore_daemonsets,
        payload.delete_emptydir_data,
        payload.grace_period_seconds,
    )
    .await?;
    Ok(StatusCode::OK)
}

// Pod handlers

#[derive(Deserialize)]
pub struct PodListQuery {
    pub label_selector: Option<String>,
}

pub async fn list_pods(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace)): Path<(String, String)>,
    Query(query): Query<PodListQuery>,
) -> Result<Json<Vec<PodInfo>>, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let pods = crate::kubernetes::workloads::pods::list_pods(
        &client,
        &namespace,
        query.label_selector.as_deref(),
    )
    .await?;
    Ok(Json(pods))
}

pub async fn get_pod(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace, pod)): Path<(String, String, String)>,
) -> Result<Json<PodInfo>, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let pod_info = crate::kubernetes::workloads::pods::get_pod(&client, &namespace, &pod).await?;
    Ok(Json(pod_info))
}

pub async fn delete_pod(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace, pod)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    crate::kubernetes::workloads::pods::delete_pod(&client, &namespace, &pod).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct PodLogQuery {
    pub container: Option<String>,
    pub tail_lines: Option<i64>,
    #[serde(default)]
    pub timestamps: bool,
}

pub async fn get_pod_logs(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace, pod)): Path<(String, String, String)>,
    Query(query): Query<PodLogQuery>,
) -> Result<String, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let logs = crate::kubernetes::workloads::pods::get_pod_logs(
        &client,
        &namespace,
        &pod,
        query.container.as_deref(),
        query.tail_lines,
        query.timestamps,
    )
    .await?;
    Ok(logs)
}

// Deployment handlers

pub async fn list_deployments(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace)): Path<(String, String)>,
) -> Result<Json<Vec<DeploymentInfo>>, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let deployments =
        crate::kubernetes::workloads::deployments::list_deployments(&client, &namespace).await?;
    Ok(Json(deployments))
}

pub async fn get_deployment(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace, deployment)): Path<(String, String, String)>,
) -> Result<Json<DeploymentInfo>, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let deploy =
        crate::kubernetes::workloads::deployments::get_deployment(&client, &namespace, &deployment)
            .await?;
    Ok(Json(deploy))
}

pub async fn scale_deployment(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace, deployment)): Path<(String, String, String)>,
    Json(payload): Json<ScaleRequest>,
) -> Result<Json<DeploymentInfo>, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let deploy = crate::kubernetes::workloads::deployments::scale_deployment(
        &client,
        &namespace,
        &deployment,
        payload.replicas,
    )
    .await?;
    Ok(Json(deploy))
}

pub async fn restart_deployment(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace, deployment)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    crate::kubernetes::workloads::deployments::restart_deployment(&client, &namespace, &deployment)
        .await?;
    Ok(StatusCode::OK)
}

// Event handlers

pub async fn list_events(
    State(state): State<Arc<AppState>>,
    Path(cluster_id): Path<String>,
) -> Result<Json<Vec<K8sEvent>>, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let events = crate::kubernetes::observability::events::list_events(&client, None).await?;
    Ok(Json(events))
}

pub async fn list_namespace_events(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace)): Path<(String, String)>,
) -> Result<Json<Vec<K8sEvent>>, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let events =
        crate::kubernetes::observability::events::list_events(&client, Some(&namespace)).await?;
    Ok(Json(events))
}

// =========================================================================
// Service handlers
// =========================================================================

pub async fn list_services(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace)): Path<(String, String)>,
) -> Result<Json<Vec<ServiceInfo>>, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let services =
        crate::kubernetes::networking::services::list_services(&client, &namespace).await?;
    Ok(Json(services))
}

pub async fn get_service(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace, service)): Path<(String, String, String)>,
) -> Result<Json<ServiceInfo>, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let svc =
        crate::kubernetes::networking::services::get_service(&client, &namespace, &service).await?;
    Ok(Json(svc))
}

pub async fn create_service(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace)): Path<(String, String)>,
    Json(mut payload): Json<CreateServiceRequest>,
) -> Result<(StatusCode, Json<ServiceInfo>), ApiError> {
    // The namespace is the one in the URL (the web UI does not repeat it).
    payload.namespace = namespace;
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let svc = crate::kubernetes::networking::services::create_service(&client, &payload).await?;
    Ok((StatusCode::CREATED, Json(svc)))
}

pub async fn delete_service(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace, service)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    crate::kubernetes::networking::services::delete_service(&client, &namespace, &service).await?;
    Ok(StatusCode::NO_CONTENT)
}

// =========================================================================
// Ingress handlers
// =========================================================================

pub async fn list_ingresses(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace)): Path<(String, String)>,
) -> Result<Json<Vec<IngressInfo>>, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let ingresses =
        crate::kubernetes::networking::ingress::list_ingresses(&client, &namespace).await?;
    Ok(Json(ingresses))
}

pub async fn get_ingress(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace, ingress)): Path<(String, String, String)>,
) -> Result<Json<IngressInfo>, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let ing =
        crate::kubernetes::networking::ingress::get_ingress(&client, &namespace, &ingress).await?;
    Ok(Json(ing))
}

pub async fn create_ingress(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace)): Path<(String, String)>,
    Json(mut payload): Json<CreateIngressRequest>,
) -> Result<(StatusCode, Json<IngressInfo>), ApiError> {
    // The namespace is the one in the URL (the web UI does not repeat it).
    payload.namespace = namespace;
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let ing = crate::kubernetes::networking::ingress::create_ingress(&client, &payload).await?;
    Ok((StatusCode::CREATED, Json(ing)))
}

pub async fn delete_ingress(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace, ingress)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    crate::kubernetes::networking::ingress::delete_ingress(&client, &namespace, &ingress).await?;
    Ok(StatusCode::NO_CONTENT)
}

// =========================================================================
// ConfigMap handlers
// =========================================================================

pub async fn list_configmaps(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace)): Path<(String, String)>,
) -> Result<Json<Vec<ConfigMapInfo>>, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let cms =
        crate::kubernetes::config_storage::configmaps::list_configmaps(&client, &namespace).await?;
    Ok(Json(cms))
}

pub async fn get_configmap(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace, name)): Path<(String, String, String)>,
) -> Result<Json<ConfigMapInfo>, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let cm =
        crate::kubernetes::config_storage::configmaps::get_configmap(&client, &namespace, &name)
            .await?;
    Ok(Json(cm))
}

pub async fn create_configmap(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace)): Path<(String, String)>,
    Json(mut payload): Json<CreateConfigMapRequest>,
) -> Result<(StatusCode, Json<ConfigMapInfo>), ApiError> {
    // The namespace is the one in the URL (the web UI does not repeat it).
    payload.namespace = namespace;
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let cm =
        crate::kubernetes::config_storage::configmaps::create_configmap(&client, &payload).await?;
    Ok((StatusCode::CREATED, Json(cm)))
}

pub async fn delete_configmap(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace, name)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    crate::kubernetes::config_storage::configmaps::delete_configmap(&client, &namespace, &name)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

// =========================================================================
// Secret handlers
// =========================================================================

pub async fn list_secrets(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace)): Path<(String, String)>,
) -> Result<Json<Vec<SecretInfo>>, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let secrets =
        crate::kubernetes::config_storage::secrets::list_secrets(&client, &namespace).await?;
    Ok(Json(secrets))
}

pub async fn get_secret(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace, name)): Path<(String, String, String)>,
) -> Result<Json<SecretInfo>, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let secret =
        crate::kubernetes::config_storage::secrets::get_secret(&client, &namespace, &name).await?;
    Ok(Json(secret))
}

pub async fn create_secret(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace)): Path<(String, String)>,
    Json(mut payload): Json<CreateSecretRequest>,
) -> Result<(StatusCode, Json<SecretInfo>), ApiError> {
    // The namespace is the one in the URL (the web UI does not repeat it).
    payload.namespace = namespace;
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let secret =
        crate::kubernetes::config_storage::secrets::create_secret(&client, &payload).await?;
    Ok((StatusCode::CREATED, Json(secret)))
}

pub async fn delete_secret(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace, name)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    crate::kubernetes::config_storage::secrets::delete_secret(&client, &namespace, &name).await?;
    Ok(StatusCode::NO_CONTENT)
}

// =========================================================================
// PVC handlers
// =========================================================================

pub async fn list_pvcs(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace)): Path<(String, String)>,
) -> Result<Json<Vec<PvcInfo>>, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let pvcs = crate::kubernetes::config_storage::pvcs::list_pvcs(&client, &namespace).await?;
    Ok(Json(pvcs))
}

pub async fn get_pvc(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace, name)): Path<(String, String, String)>,
) -> Result<Json<PvcInfo>, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let pvc = crate::kubernetes::config_storage::pvcs::get_pvc(&client, &namespace, &name).await?;
    Ok(Json(pvc))
}

pub async fn create_pvc(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace)): Path<(String, String)>,
    Json(mut payload): Json<CreatePvcRequest>,
) -> Result<(StatusCode, Json<PvcInfo>), ApiError> {
    // The namespace is the one in the URL (the web UI does not repeat it).
    payload.namespace = namespace;
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let pvc = crate::kubernetes::config_storage::pvcs::create_pvc(&client, &payload).await?;
    Ok((StatusCode::CREATED, Json(pvc)))
}

pub async fn delete_pvc(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace, name)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    crate::kubernetes::config_storage::pvcs::delete_pvc(&client, &namespace, &name).await?;
    Ok(StatusCode::NO_CONTENT)
}

// =========================================================================
// StatefulSet handlers
// =========================================================================

pub async fn list_statefulsets(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace)): Path<(String, String)>,
) -> Result<Json<Vec<StatefulSetInfo>>, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let sts =
        crate::kubernetes::workloads::statefulsets::list_statefulsets(&client, &namespace).await?;
    Ok(Json(sts))
}

pub async fn get_statefulset(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace, name)): Path<(String, String, String)>,
) -> Result<Json<StatefulSetInfo>, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let sts =
        crate::kubernetes::workloads::statefulsets::get_statefulset(&client, &namespace, &name)
            .await?;
    Ok(Json(sts))
}

pub async fn scale_statefulset(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace, name)): Path<(String, String, String)>,
    Json(payload): Json<ScaleRequest>,
) -> Result<Json<StatefulSetInfo>, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let sts = crate::kubernetes::workloads::statefulsets::scale_statefulset(
        &client,
        &namespace,
        &name,
        payload.replicas,
    )
    .await?;
    Ok(Json(sts))
}

pub async fn delete_statefulset(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace, name)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    crate::kubernetes::workloads::statefulsets::delete_statefulset(&client, &namespace, &name)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

// =========================================================================
// DaemonSet handlers
// =========================================================================

pub async fn list_daemonsets(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace)): Path<(String, String)>,
) -> Result<Json<Vec<DaemonSetInfo>>, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let ds = crate::kubernetes::workloads::daemonsets::list_daemonsets(&client, &namespace).await?;
    Ok(Json(ds))
}

pub async fn get_daemonset(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace, name)): Path<(String, String, String)>,
) -> Result<Json<DaemonSetInfo>, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let ds =
        crate::kubernetes::workloads::daemonsets::get_daemonset(&client, &namespace, &name).await?;
    Ok(Json(ds))
}

pub async fn delete_daemonset(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace, name)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    crate::kubernetes::workloads::daemonsets::delete_daemonset(&client, &namespace, &name).await?;
    Ok(StatusCode::NO_CONTENT)
}

// =========================================================================
// Job handlers
// =========================================================================

pub async fn list_jobs(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace)): Path<(String, String)>,
) -> Result<Json<Vec<JobInfo>>, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let jobs = crate::kubernetes::workloads::jobs::list_jobs(&client, &namespace).await?;
    Ok(Json(jobs))
}

pub async fn get_job(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace, name)): Path<(String, String, String)>,
) -> Result<Json<JobInfo>, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let job = crate::kubernetes::workloads::jobs::get_job(&client, &namespace, &name).await?;
    Ok(Json(job))
}

pub async fn delete_job(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace, name)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    crate::kubernetes::workloads::jobs::delete_job(&client, &namespace, &name, None).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_cronjobs(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace)): Path<(String, String)>,
) -> Result<Json<Vec<CronJobInfo>>, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let cronjobs = crate::kubernetes::workloads::jobs::list_cronjobs(&client, &namespace).await?;
    Ok(Json(cronjobs))
}

pub async fn get_cronjob(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace, name)): Path<(String, String, String)>,
) -> Result<Json<CronJobInfo>, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let cronjob =
        crate::kubernetes::workloads::jobs::get_cronjob(&client, &namespace, &name).await?;
    Ok(Json(cronjob))
}

pub async fn delete_cronjob(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace, name)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    crate::kubernetes::workloads::jobs::delete_cronjob(&client, &namespace, &name).await?;
    Ok(StatusCode::NO_CONTENT)
}

// =========================================================================
// Metrics handlers
// =========================================================================

pub async fn get_node_metrics(
    State(state): State<Arc<AppState>>,
    Path(cluster_id): Path<String>,
) -> Result<Json<Vec<crate::kubernetes::types::NodeMetrics>>, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let metrics = crate::kubernetes::observability::metrics::get_node_metrics(&client).await?;
    Ok(Json(metrics))
}

pub async fn get_pod_metrics(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace)): Path<(String, String)>,
) -> Result<Json<Vec<crate::kubernetes::types::PodMetrics>>, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    let metrics =
        crate::kubernetes::observability::metrics::get_pod_metrics(&client, &namespace).await?;
    Ok(Json(metrics))
}
