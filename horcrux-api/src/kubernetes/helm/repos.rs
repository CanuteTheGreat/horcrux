//! Helm repository management

use crate::kubernetes::error::{K8sError, K8sResult};
use crate::kubernetes::types::HelmRepo;
use tokio::process::Command;

/// List configured repositories
pub async fn list_repos() -> K8sResult<Vec<HelmRepo>> {
    let output = Command::new("helm")
        .arg("repo")
        .arg("list")
        .arg("--output")
        .arg("json")
        .output()
        .await
        .map_err(|e| K8sError::HelmError(format!("Failed to run helm: {}", e)))?;

    if !output.status.success() {
        // Empty repo list returns non-zero
        if output.stdout.is_empty() {
            return Ok(Vec::new());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(K8sError::HelmError(format!(
            "helm repo list failed: {}",
            stderr
        )));
    }

    let repos: Vec<HelmRepoItem> = serde_json::from_slice(&output.stdout)
        .map_err(|e| K8sError::HelmError(format!("Failed to parse helm output: {}", e)))?;

    Ok(repos
        .into_iter()
        .map(|r| HelmRepo {
            name: r.name,
            url: r.url,
        })
        .collect())
}

/// Add a repository
pub async fn add_repo(name: &str, url: &str) -> K8sResult<()> {
    let output = Command::new("helm")
        .arg("repo")
        .arg("add")
        .arg(name)
        .arg(url)
        .output()
        .await
        .map_err(|e| K8sError::HelmError(format!("Failed to run helm: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(K8sError::HelmError(format!(
            "helm repo add failed: {}",
            stderr
        )));
    }

    Ok(())
}

/// Remove a repository
pub async fn remove_repo(name: &str) -> K8sResult<()> {
    let output = Command::new("helm")
        .arg("repo")
        .arg("remove")
        .arg(name)
        .output()
        .await
        .map_err(|e| K8sError::HelmError(format!("Failed to run helm: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(K8sError::HelmError(format!(
            "helm repo remove failed: {}",
            stderr
        )));
    }

    Ok(())
}

/// Update all repositories
pub async fn update_repos() -> K8sResult<()> {
    let output = Command::new("helm")
        .arg("repo")
        .arg("update")
        .output()
        .await
        .map_err(|e| K8sError::HelmError(format!("Failed to run helm: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(K8sError::HelmError(format!(
            "helm repo update failed: {}",
            stderr
        )));
    }

    Ok(())
}

#[derive(Debug, serde::Deserialize)]
struct HelmRepoItem {
    name: String,
    url: String,
}

/// Search for charts in repositories
pub async fn search_charts(keyword: &str, all_versions: bool) -> K8sResult<Vec<HelmChart>> {
    let mut cmd = Command::new("helm");
    cmd.arg("search")
        .arg("repo")
        .arg(keyword)
        .arg("--output")
        .arg("json");

    if all_versions {
        cmd.arg("--versions");
    }

    let output = cmd
        .output()
        .await
        .map_err(|e| K8sError::HelmError(format!("Failed to run helm: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(K8sError::HelmError(format!(
            "helm search failed: {}",
            stderr
        )));
    }

    // Handle empty results
    if output.stdout.is_empty() {
        return Ok(Vec::new());
    }

    let charts: Vec<HelmSearchItem> = serde_json::from_slice(&output.stdout)
        .map_err(|e| K8sError::HelmError(format!("Failed to parse helm output: {}", e)))?;

    Ok(charts.into_iter().map(|c| c.into()).collect())
}

/// Search Helm Hub (Artifact Hub)
pub async fn search_hub(keyword: &str) -> K8sResult<Vec<HelmChart>> {
    let output = Command::new("helm")
        .arg("search")
        .arg("hub")
        .arg(keyword)
        .arg("--output")
        .arg("json")
        .output()
        .await
        .map_err(|e| K8sError::HelmError(format!("Failed to run helm: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(K8sError::HelmError(format!(
            "helm search hub failed: {}",
            stderr
        )));
    }

    // Handle empty results
    if output.stdout.is_empty() {
        return Ok(Vec::new());
    }

    let charts: Vec<HelmHubSearchItem> = serde_json::from_slice(&output.stdout)
        .map_err(|e| K8sError::HelmError(format!("Failed to parse helm output: {}", e)))?;

    Ok(charts.into_iter().map(|c| c.into()).collect())
}

/// Show chart information
pub async fn show_chart(chart_name: &str) -> K8sResult<HelmChartInfo> {
    let output = Command::new("helm")
        .arg("show")
        .arg("chart")
        .arg(chart_name)
        .output()
        .await
        .map_err(|e| K8sError::HelmError(format!("Failed to run helm: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(K8sError::HelmError(format!(
            "helm show chart failed: {}",
            stderr
        )));
    }

    // Parse YAML output
    let yaml_str = String::from_utf8_lossy(&output.stdout);
    let info: HelmChartYaml = serde_yaml::from_str(&yaml_str)
        .map_err(|e| K8sError::HelmError(format!("Failed to parse chart info: {}", e)))?;

    Ok(HelmChartInfo {
        name: info.name,
        version: info.version,
        app_version: info.app_version,
        description: info.description,
        home: info.home,
        sources: info.sources.unwrap_or_default(),
        maintainers: info.maintainers.unwrap_or_default().into_iter().map(|m| m.into()).collect(),
        keywords: info.keywords.unwrap_or_default(),
        icon: info.icon,
        api_version: info.api_version,
        chart_type: info.r#type,
        deprecated: info.deprecated.unwrap_or(false),
    })
}

/// Show chart default values
pub async fn show_chart_values(chart_name: &str) -> K8sResult<String> {
    let output = Command::new("helm")
        .arg("show")
        .arg("values")
        .arg(chart_name)
        .output()
        .await
        .map_err(|e| K8sError::HelmError(format!("Failed to run helm: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(K8sError::HelmError(format!(
            "helm show values failed: {}",
            stderr
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Show chart README
pub async fn show_chart_readme(chart_name: &str) -> K8sResult<String> {
    let output = Command::new("helm")
        .arg("show")
        .arg("readme")
        .arg(chart_name)
        .output()
        .await
        .map_err(|e| K8sError::HelmError(format!("Failed to run helm: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(K8sError::HelmError(format!(
            "helm show readme failed: {}",
            stderr
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Helm chart from search
#[derive(Debug, Clone, serde::Serialize)]
pub struct HelmChart {
    pub name: String,
    pub version: String,
    pub app_version: Option<String>,
    pub description: String,
    pub repository: Option<String>,
}

/// Helm search item
#[derive(Debug, serde::Deserialize)]
struct HelmSearchItem {
    name: String,
    version: String,
    app_version: String,
    description: String,
}

impl From<HelmSearchItem> for HelmChart {
    fn from(item: HelmSearchItem) -> Self {
        // Name is in format "repo/chart"
        let (repo, _name) = if let Some(idx) = item.name.find('/') {
            let (r, n) = item.name.split_at(idx);
            (Some(r.to_string()), n[1..].to_string())
        } else {
            (None, item.name.clone())
        };

        HelmChart {
            name: item.name,
            version: item.version,
            app_version: if item.app_version.is_empty() {
                None
            } else {
                Some(item.app_version)
            },
            description: item.description,
            repository: repo,
        }
    }
}

/// Helm Hub search item
#[derive(Debug, serde::Deserialize)]
struct HelmHubSearchItem {
    url: String,
    version: String,
    app_version: String,
    description: String,
}

impl From<HelmHubSearchItem> for HelmChart {
    fn from(item: HelmHubSearchItem) -> Self {
        // Extract name from URL
        let name = item.url
            .rsplit('/')
            .next()
            .unwrap_or(&item.url)
            .to_string();

        HelmChart {
            name,
            version: item.version,
            app_version: if item.app_version.is_empty() {
                None
            } else {
                Some(item.app_version)
            },
            description: item.description,
            repository: Some(item.url),
        }
    }
}

/// Detailed chart information
#[derive(Debug, Clone, serde::Serialize)]
pub struct HelmChartInfo {
    pub name: String,
    pub version: String,
    pub app_version: Option<String>,
    pub description: Option<String>,
    pub home: Option<String>,
    pub sources: Vec<String>,
    pub maintainers: Vec<HelmMaintainer>,
    pub keywords: Vec<String>,
    pub icon: Option<String>,
    pub api_version: String,
    pub chart_type: Option<String>,
    pub deprecated: bool,
}

/// Chart maintainer
#[derive(Debug, Clone, serde::Serialize)]
pub struct HelmMaintainer {
    pub name: String,
    pub email: Option<String>,
    pub url: Option<String>,
}

/// Chart YAML structure
#[derive(Debug, serde::Deserialize)]
struct HelmChartYaml {
    name: String,
    version: String,
    #[serde(rename = "appVersion")]
    app_version: Option<String>,
    description: Option<String>,
    home: Option<String>,
    sources: Option<Vec<String>>,
    maintainers: Option<Vec<HelmMaintainerYaml>>,
    keywords: Option<Vec<String>>,
    icon: Option<String>,
    #[serde(rename = "apiVersion")]
    api_version: String,
    r#type: Option<String>,
    deprecated: Option<bool>,
}

#[derive(Debug, serde::Deserialize)]
struct HelmMaintainerYaml {
    name: String,
    email: Option<String>,
    url: Option<String>,
}

impl From<HelmMaintainerYaml> for HelmMaintainer {
    fn from(item: HelmMaintainerYaml) -> Self {
        HelmMaintainer {
            name: item.name,
            email: item.email,
            url: item.url,
        }
    }
}
