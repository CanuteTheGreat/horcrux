//! Helm routes used by the web UI's Kubernetes > Helm pages
//! (`pages/kubernetes/helm/{repositories,charts,releases}.rs`).
//!
//! Repositories and charts are host-wide (one Helm home under the data
//! directory); releases are per cluster and namespace and run with the
//! cluster's stored kubeconfig.

use crate::error::ApiError;
use crate::kubernetes::helm::{
    releases, repos, AddRepoRequest, HelmChart, HelmInstallRequest, HelmRelease, HelmRepository,
    HelmValues,
};
use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;

pub fn routes(state: &Arc<AppState>) -> Router<Arc<AppState>> {
    crate::kubernetes::helm::set_helm_home(&state.config.paths.data_dir);
    Router::new()
        .route("/api/k8s/helm/repos", get(list_repos).post(add_repo))
        .route(
            "/api/k8s/helm/repos/:repo",
            axum::routing::delete(remove_repo),
        )
        .route("/api/k8s/helm/repos/:repo/update", post(update_repo))
        .route("/api/k8s/helm/charts", get(search_charts))
        .route("/api/k8s/helm/charts/:repo/:chart", get(get_chart))
        .route(
            "/api/k8s/helm/charts/:repo/:chart/versions",
            get(chart_versions),
        )
        .route(
            "/api/k8s/helm/charts/:repo/:chart/values",
            get(chart_values),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/helm/releases",
            get(list_releases),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/helm/releases/:namespace/:release",
            get(get_release),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/helm/install",
            post(install),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/helm/releases/:release",
            get(get_release_ns).delete(uninstall),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/helm/releases/:release/upgrade",
            post(upgrade),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/helm/releases/:release/rollback",
            post(rollback),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/helm/releases/:release/history",
            get(history),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/helm/releases/:release/values",
            get(values),
        )
}

// ---- repositories ----------------------------------------------------------

async fn list_repos() -> Result<Json<Vec<HelmRepository>>, ApiError> {
    Ok(Json(repos::list().await?))
}

async fn add_repo(
    Json(req): Json<AddRepoRequest>,
) -> Result<(StatusCode, Json<HelmRepository>), ApiError> {
    Ok((StatusCode::CREATED, Json(repos::add(&req).await?)))
}

async fn update_repo(Path(repo): Path<String>) -> Result<Json<HelmRepository>, ApiError> {
    Ok(Json(repos::update(&repo).await?))
}

async fn remove_repo(Path(repo): Path<String>) -> Result<StatusCode, ApiError> {
    repos::remove(&repo).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- charts ----------------------------------------------------------------

#[derive(Deserialize)]
struct ChartSearch {
    #[serde(default)]
    query: String,
    repo: Option<String>,
}

async fn search_charts(Query(q): Query<ChartSearch>) -> Result<Json<Vec<HelmChart>>, ApiError> {
    let repo = q.repo.as_deref().filter(|r| !r.is_empty());
    Ok(Json(repos::search(&q.query, repo).await?))
}

async fn get_chart(
    Path((repo, chart)): Path<(String, String)>,
) -> Result<Json<HelmChart>, ApiError> {
    Ok(Json(repos::chart(&repo, &chart).await?))
}

async fn chart_versions(
    Path((repo, chart)): Path<(String, String)>,
) -> Result<Json<Vec<String>>, ApiError> {
    Ok(Json(repos::versions(&repo, &chart).await?))
}

#[derive(Deserialize)]
struct VersionQuery {
    version: Option<String>,
}

async fn chart_values(
    Path((repo, chart)): Path<(String, String)>,
    Query(q): Query<VersionQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(
        repos::chart_values(&repo, &chart, q.version.as_deref()).await?,
    ))
}

// ---- releases --------------------------------------------------------------

#[derive(Deserialize)]
struct NamespaceQuery {
    namespace: Option<String>,
}

async fn list_releases(
    State(state): State<Arc<AppState>>,
    Path(cluster_id): Path<String>,
    Query(q): Query<NamespaceQuery>,
) -> Result<Json<Vec<HelmRelease>>, ApiError> {
    let target = state.kubernetes_manager.helm_target(&cluster_id).await?;
    let ns = q.namespace.as_deref().filter(|n| !n.is_empty());
    Ok(Json(releases::list(&target, ns).await?))
}

async fn get_release(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace, release)): Path<(String, String, String)>,
) -> Result<Json<HelmRelease>, ApiError> {
    let target = state.kubernetes_manager.helm_target(&cluster_id).await?;
    Ok(Json(releases::status(&target, &namespace, &release).await?))
}

async fn get_release_ns(
    state: State<Arc<AppState>>,
    path: Path<(String, String, String)>,
) -> Result<Json<HelmRelease>, ApiError> {
    get_release(state, path).await
}

async fn install(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace)): Path<(String, String)>,
    Json(req): Json<HelmInstallRequest>,
) -> Result<(StatusCode, Json<HelmRelease>), ApiError> {
    let target = state.kubernetes_manager.helm_target(&cluster_id).await?;
    let release = releases::install(&target, &namespace, &req).await?;
    Ok((StatusCode::CREATED, Json(release)))
}

async fn upgrade(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace, release)): Path<(String, String, String)>,
    Json(req): Json<HelmInstallRequest>,
) -> Result<Json<HelmRelease>, ApiError> {
    let target = state.kubernetes_manager.helm_target(&cluster_id).await?;
    Ok(Json(
        releases::upgrade(&target, &namespace, &release, &req).await?,
    ))
}

async fn uninstall(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace, release)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    let target = state.kubernetes_manager.helm_target(&cluster_id).await?;
    releases::uninstall(&target, &namespace, &release).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct RollbackRequest {
    /// Target revision; 0 or absent = the previous one
    #[serde(default)]
    revision: u32,
}

async fn rollback(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace, release)): Path<(String, String, String)>,
    body: Option<Json<RollbackRequest>>,
) -> Result<Json<HelmRelease>, ApiError> {
    let revision = body.map(|Json(b)| b.revision).unwrap_or(0);
    let target = state.kubernetes_manager.helm_target(&cluster_id).await?;
    Ok(Json(
        releases::rollback(&target, &namespace, &release, revision).await?,
    ))
}

async fn history(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace, release)): Path<(String, String, String)>,
) -> Result<Json<Vec<HelmRelease>>, ApiError> {
    let target = state.kubernetes_manager.helm_target(&cluster_id).await?;
    Ok(Json(
        releases::history(&target, &namespace, &release).await?,
    ))
}

async fn values(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace, release)): Path<(String, String, String)>,
) -> Result<Json<HelmValues>, ApiError> {
    let target = state.kubernetes_manager.helm_target(&cluster_id).await?;
    Ok(Json(releases::values(&target, &namespace, &release).await?))
}
