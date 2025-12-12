//! Helm chart management
//!
//! Install, upgrade, rollback, and uninstall Helm releases.

pub mod releases;
pub mod repos;

use crate::kubernetes::error::K8sResult;
use crate::kubernetes::types::{HelmInstallRequest, HelmRelease, HelmRepo, HelmUpgradeRequest};

// Re-export types
pub use releases::HelmReleaseRevision;
pub use repos::{HelmChart, HelmChartInfo};

/// Helm manager
pub struct HelmManager;

impl HelmManager {
    pub fn new() -> Self {
        Self
    }

    // =========================================================================
    // Release Management
    // =========================================================================

    /// List all releases in a cluster
    pub async fn list_releases(&self, kubeconfig_path: &str) -> K8sResult<Vec<HelmRelease>> {
        releases::list_releases(kubeconfig_path).await
    }

    /// Install a new release
    pub async fn install_release(
        &self,
        kubeconfig_path: &str,
        request: &HelmInstallRequest,
    ) -> K8sResult<HelmRelease> {
        releases::install_release(kubeconfig_path, request).await
    }

    /// Upgrade an existing release
    pub async fn upgrade_release(
        &self,
        kubeconfig_path: &str,
        release_name: &str,
        request: &HelmUpgradeRequest,
    ) -> K8sResult<HelmRelease> {
        releases::upgrade_release(kubeconfig_path, release_name, request).await
    }

    /// Uninstall a release
    pub async fn uninstall_release(
        &self,
        kubeconfig_path: &str,
        release_name: &str,
        namespace: &str,
    ) -> K8sResult<()> {
        releases::uninstall_release(kubeconfig_path, release_name, namespace).await
    }

    /// Rollback to a previous revision
    pub async fn rollback_release(
        &self,
        kubeconfig_path: &str,
        release_name: &str,
        namespace: &str,
        revision: Option<i32>,
    ) -> K8sResult<()> {
        releases::rollback_release(kubeconfig_path, release_name, namespace, revision).await
    }

    /// Get release history
    pub async fn get_release_history(
        &self,
        kubeconfig_path: &str,
        release_name: &str,
        namespace: &str,
    ) -> K8sResult<Vec<HelmReleaseRevision>> {
        releases::get_release_history(kubeconfig_path, release_name, namespace).await
    }

    /// Get values from an existing release
    pub async fn get_release_values(
        &self,
        kubeconfig_path: &str,
        release_name: &str,
        namespace: &str,
        all_values: bool,
    ) -> K8sResult<serde_json::Value> {
        releases::get_release_values(kubeconfig_path, release_name, namespace, all_values).await
    }

    /// Get release manifest (deployed YAML)
    pub async fn get_release_manifest(
        &self,
        kubeconfig_path: &str,
        release_name: &str,
        namespace: &str,
    ) -> K8sResult<String> {
        releases::get_release_manifest(kubeconfig_path, release_name, namespace).await
    }

    /// Get release notes
    pub async fn get_release_notes(
        &self,
        kubeconfig_path: &str,
        release_name: &str,
        namespace: &str,
    ) -> K8sResult<String> {
        releases::get_release_notes(kubeconfig_path, release_name, namespace).await
    }

    // =========================================================================
    // Repository Management
    // =========================================================================

    /// List configured repositories
    pub async fn list_repos(&self) -> K8sResult<Vec<HelmRepo>> {
        repos::list_repos().await
    }

    /// Add a repository
    pub async fn add_repo(&self, name: &str, url: &str) -> K8sResult<()> {
        repos::add_repo(name, url).await
    }

    /// Remove a repository
    pub async fn remove_repo(&self, name: &str) -> K8sResult<()> {
        repos::remove_repo(name).await
    }

    /// Update repositories
    pub async fn update_repos(&self) -> K8sResult<()> {
        repos::update_repos().await
    }

    // =========================================================================
    // Chart Search & Info
    // =========================================================================

    /// Search for charts in configured repositories
    pub async fn search_charts(&self, keyword: &str, all_versions: bool) -> K8sResult<Vec<HelmChart>> {
        repos::search_charts(keyword, all_versions).await
    }

    /// Search Artifact Hub for charts
    pub async fn search_hub(&self, keyword: &str) -> K8sResult<Vec<HelmChart>> {
        repos::search_hub(keyword).await
    }

    /// Get detailed chart information
    pub async fn show_chart(&self, chart_name: &str) -> K8sResult<HelmChartInfo> {
        repos::show_chart(chart_name).await
    }

    /// Get chart default values (YAML)
    pub async fn show_chart_values(&self, chart_name: &str) -> K8sResult<String> {
        repos::show_chart_values(chart_name).await
    }

    /// Get chart README
    pub async fn show_chart_readme(&self, chart_name: &str) -> K8sResult<String> {
        repos::show_chart_readme(chart_name).await
    }
}

impl Default for HelmManager {
    fn default() -> Self {
        Self::new()
    }
}
