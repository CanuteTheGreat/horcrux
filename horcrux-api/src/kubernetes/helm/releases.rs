//! Helm release operations
//!
//! Uses helm CLI for release management.

use crate::kubernetes::error::{K8sError, K8sResult};
use crate::kubernetes::types::{HelmInstallRequest, HelmRelease, HelmUpgradeRequest};
use tokio::process::Command;

/// List all releases
pub async fn list_releases(kubeconfig_path: &str) -> K8sResult<Vec<HelmRelease>> {
    let output = Command::new("helm")
        .arg("list")
        .arg("--all-namespaces")
        .arg("--output")
        .arg("json")
        .env("KUBECONFIG", kubeconfig_path)
        .output()
        .await
        .map_err(|e| K8sError::HelmError(format!("Failed to run helm: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(K8sError::HelmError(format!("helm list failed: {}", stderr)));
    }

    let releases: Vec<HelmListItem> = serde_json::from_slice(&output.stdout)
        .map_err(|e| K8sError::HelmError(format!("Failed to parse helm output: {}", e)))?;

    Ok(releases.into_iter().map(|r| r.into()).collect())
}

/// Install a new release
pub async fn install_release(
    kubeconfig_path: &str,
    request: &HelmInstallRequest,
) -> K8sResult<HelmRelease> {
    let mut cmd = Command::new("helm");
    cmd.arg("install")
        .arg(&request.name)
        .arg(&request.chart)
        .arg("--namespace")
        .arg(&request.namespace)
        .arg("--output")
        .arg("json")
        .env("KUBECONFIG", kubeconfig_path);

    if request.create_namespace {
        cmd.arg("--create-namespace");
    }

    if let Some(version) = &request.version {
        cmd.arg("--version").arg(version);
    }

    if request.wait {
        cmd.arg("--wait");
    }

    if let Some(timeout) = &request.timeout {
        cmd.arg("--timeout").arg(timeout);
    }

    // Handle values
    if let Some(values) = &request.values {
        let values_str = serde_json::to_string(values)
            .map_err(|e| K8sError::HelmError(format!("Failed to serialize values: {}", e)))?;

        // Write to temp file
        let temp_file = tempfile::NamedTempFile::new()
            .map_err(|e| K8sError::HelmError(format!("Failed to create temp file: {}", e)))?;

        tokio::fs::write(temp_file.path(), values_str)
            .await
            .map_err(|e| K8sError::HelmError(format!("Failed to write values: {}", e)))?;

        cmd.arg("--values").arg(temp_file.path());
    }

    let output = cmd
        .output()
        .await
        .map_err(|e| K8sError::HelmError(format!("Failed to run helm: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(K8sError::HelmError(format!(
            "helm install failed: {}",
            stderr
        )));
    }

    // Get the release info
    get_release(kubeconfig_path, &request.name, &request.namespace).await
}

/// Upgrade an existing release
pub async fn upgrade_release(
    kubeconfig_path: &str,
    release_name: &str,
    request: &HelmUpgradeRequest,
) -> K8sResult<HelmRelease> {
    let mut cmd = Command::new("helm");
    cmd.arg("upgrade")
        .arg(release_name)
        .arg(&request.chart)
        .arg("--output")
        .arg("json")
        .env("KUBECONFIG", kubeconfig_path);

    if let Some(version) = &request.version {
        cmd.arg("--version").arg(version);
    }

    if request.wait {
        cmd.arg("--wait");
    }

    if let Some(timeout) = &request.timeout {
        cmd.arg("--timeout").arg(timeout);
    }

    if request.reset_values {
        cmd.arg("--reset-values");
    }

    if request.reuse_values {
        cmd.arg("--reuse-values");
    }

    // Handle values
    if let Some(values) = &request.values {
        let values_str = serde_json::to_string(values)
            .map_err(|e| K8sError::HelmError(format!("Failed to serialize values: {}", e)))?;

        let temp_file = tempfile::NamedTempFile::new()
            .map_err(|e| K8sError::HelmError(format!("Failed to create temp file: {}", e)))?;

        tokio::fs::write(temp_file.path(), values_str)
            .await
            .map_err(|e| K8sError::HelmError(format!("Failed to write values: {}", e)))?;

        cmd.arg("--values").arg(temp_file.path());
    }

    let output = cmd
        .output()
        .await
        .map_err(|e| K8sError::HelmError(format!("Failed to run helm: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(K8sError::HelmError(format!(
            "helm upgrade failed: {}",
            stderr
        )));
    }

    // Get release info - we need to determine the namespace
    // For simplicity, list all and find by name
    let releases = list_releases(kubeconfig_path).await?;
    releases
        .into_iter()
        .find(|r| r.name == release_name)
        .ok_or_else(|| K8sError::HelmError("Release not found after upgrade".to_string()))
}

/// Uninstall a release
pub async fn uninstall_release(
    kubeconfig_path: &str,
    release_name: &str,
    namespace: &str,
) -> K8sResult<()> {
    let output = Command::new("helm")
        .arg("uninstall")
        .arg(release_name)
        .arg("--namespace")
        .arg(namespace)
        .env("KUBECONFIG", kubeconfig_path)
        .output()
        .await
        .map_err(|e| K8sError::HelmError(format!("Failed to run helm: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(K8sError::HelmError(format!(
            "helm uninstall failed: {}",
            stderr
        )));
    }

    Ok(())
}

/// Rollback to a previous revision
pub async fn rollback_release(
    kubeconfig_path: &str,
    release_name: &str,
    namespace: &str,
    revision: Option<i32>,
) -> K8sResult<()> {
    let mut cmd = Command::new("helm");
    cmd.arg("rollback")
        .arg(release_name)
        .arg("--namespace")
        .arg(namespace)
        .env("KUBECONFIG", kubeconfig_path);

    if let Some(rev) = revision {
        cmd.arg(rev.to_string());
    }

    let output = cmd
        .output()
        .await
        .map_err(|e| K8sError::HelmError(format!("Failed to run helm: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(K8sError::HelmError(format!(
            "helm rollback failed: {}",
            stderr
        )));
    }

    Ok(())
}

/// Get release info
async fn get_release(
    kubeconfig_path: &str,
    release_name: &str,
    namespace: &str,
) -> K8sResult<HelmRelease> {
    let output = Command::new("helm")
        .arg("status")
        .arg(release_name)
        .arg("--namespace")
        .arg(namespace)
        .arg("--output")
        .arg("json")
        .env("KUBECONFIG", kubeconfig_path)
        .output()
        .await
        .map_err(|e| K8sError::HelmError(format!("Failed to run helm: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(K8sError::HelmError(format!(
            "helm status failed: {}",
            stderr
        )));
    }

    let status: HelmStatusOutput = serde_json::from_slice(&output.stdout)
        .map_err(|e| K8sError::HelmError(format!("Failed to parse helm output: {}", e)))?;

    Ok(HelmRelease {
        name: status.name,
        namespace: status.namespace,
        chart: status.chart,
        chart_version: status.version.to_string(),
        app_version: status.app_version,
        status: status.info.status,
        revision: status.version,
        updated: status.info.last_deployed,
    })
}

/// Helm list output item
#[derive(Debug, serde::Deserialize)]
struct HelmListItem {
    name: String,
    namespace: String,
    revision: String,
    updated: String,
    status: String,
    chart: String,
    app_version: String,
}

impl From<HelmListItem> for HelmRelease {
    fn from(item: HelmListItem) -> Self {
        // Parse chart name and version (format: "name-version")
        let (chart_name, chart_version) = if let Some(idx) = item.chart.rfind('-') {
            let (name, ver) = item.chart.split_at(idx);
            (name.to_string(), ver[1..].to_string())
        } else {
            (item.chart.clone(), String::new())
        };

        HelmRelease {
            name: item.name,
            namespace: item.namespace,
            chart: chart_name,
            chart_version,
            app_version: if item.app_version.is_empty() {
                None
            } else {
                Some(item.app_version)
            },
            status: item.status,
            revision: item.revision.parse().unwrap_or(0),
            updated: item.updated,
        }
    }
}

/// Helm status output
#[derive(Debug, serde::Deserialize)]
struct HelmStatusOutput {
    name: String,
    namespace: String,
    chart: String,
    version: i32,
    app_version: Option<String>,
    info: HelmStatusInfo,
}

#[derive(Debug, serde::Deserialize)]
struct HelmStatusInfo {
    status: String,
    last_deployed: String,
}

/// Get release history
pub async fn get_release_history(
    kubeconfig_path: &str,
    release_name: &str,
    namespace: &str,
) -> K8sResult<Vec<HelmReleaseRevision>> {
    let output = Command::new("helm")
        .arg("history")
        .arg(release_name)
        .arg("--namespace")
        .arg(namespace)
        .arg("--output")
        .arg("json")
        .env("KUBECONFIG", kubeconfig_path)
        .output()
        .await
        .map_err(|e| K8sError::HelmError(format!("Failed to run helm: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(K8sError::HelmError(format!(
            "helm history failed: {}",
            stderr
        )));
    }

    let history: Vec<HelmHistoryItem> = serde_json::from_slice(&output.stdout)
        .map_err(|e| K8sError::HelmError(format!("Failed to parse helm output: {}", e)))?;

    Ok(history.into_iter().map(|h| h.into()).collect())
}

/// Get values from an existing release
pub async fn get_release_values(
    kubeconfig_path: &str,
    release_name: &str,
    namespace: &str,
    all_values: bool,
) -> K8sResult<serde_json::Value> {
    let mut cmd = Command::new("helm");
    cmd.arg("get")
        .arg("values")
        .arg(release_name)
        .arg("--namespace")
        .arg(namespace)
        .arg("--output")
        .arg("json")
        .env("KUBECONFIG", kubeconfig_path);

    if all_values {
        cmd.arg("--all");
    }

    let output = cmd
        .output()
        .await
        .map_err(|e| K8sError::HelmError(format!("Failed to run helm: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(K8sError::HelmError(format!(
            "helm get values failed: {}",
            stderr
        )));
    }

    // Handle empty values (helm returns "null" as string)
    let stdout_str = String::from_utf8_lossy(&output.stdout);
    if stdout_str.trim() == "null" || stdout_str.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }

    let values: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| K8sError::HelmError(format!("Failed to parse helm output: {}", e)))?;

    Ok(values)
}

/// Get release manifest
pub async fn get_release_manifest(
    kubeconfig_path: &str,
    release_name: &str,
    namespace: &str,
) -> K8sResult<String> {
    let output = Command::new("helm")
        .arg("get")
        .arg("manifest")
        .arg(release_name)
        .arg("--namespace")
        .arg(namespace)
        .env("KUBECONFIG", kubeconfig_path)
        .output()
        .await
        .map_err(|e| K8sError::HelmError(format!("Failed to run helm: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(K8sError::HelmError(format!(
            "helm get manifest failed: {}",
            stderr
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Get release notes
pub async fn get_release_notes(
    kubeconfig_path: &str,
    release_name: &str,
    namespace: &str,
) -> K8sResult<String> {
    let output = Command::new("helm")
        .arg("get")
        .arg("notes")
        .arg(release_name)
        .arg("--namespace")
        .arg(namespace)
        .env("KUBECONFIG", kubeconfig_path)
        .output()
        .await
        .map_err(|e| K8sError::HelmError(format!("Failed to run helm: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(K8sError::HelmError(format!(
            "helm get notes failed: {}",
            stderr
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Release revision from history
#[derive(Debug, Clone, serde::Serialize)]
pub struct HelmReleaseRevision {
    pub revision: i32,
    pub updated: String,
    pub status: String,
    pub chart: String,
    pub app_version: Option<String>,
    pub description: String,
}

/// Helm history output item
#[derive(Debug, serde::Deserialize)]
struct HelmHistoryItem {
    revision: i32,
    updated: String,
    status: String,
    chart: String,
    app_version: String,
    description: String,
}

impl From<HelmHistoryItem> for HelmReleaseRevision {
    fn from(item: HelmHistoryItem) -> Self {
        HelmReleaseRevision {
            revision: item.revision,
            updated: item.updated,
            status: item.status,
            chart: item.chart,
            app_version: if item.app_version.is_empty() {
                None
            } else {
                Some(item.app_version)
            },
            description: item.description,
        }
    }
}
