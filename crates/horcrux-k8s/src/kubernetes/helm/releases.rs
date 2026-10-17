//! Helm releases: list/status/install/upgrade/rollback/uninstall/history/values.
//!
//! All operations are namespace-scoped (a release name is only unique within
//! its namespace) and run against one cluster via [`KubeTarget`].

use super::cli::{
    helm, run, split_chart_version, validate_chart_ref, validate_namespace, validate_plain,
    validate_release_name, KubeTarget,
};
use crate::kubernetes::error::{K8sError, K8sResult};
use serde::{Deserialize, Serialize};

/// A Helm release (one revision of it).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HelmRelease {
    pub name: String,
    pub namespace: String,
    pub revision: u32,
    /// deployed, superseded, failed, pending-install, pending-upgrade, ...
    pub status: String,
    /// Chart name (without version)
    pub chart: String,
    pub chart_version: String,
    pub app_version: Option<String>,
    pub updated: String,
    pub description: String,
}

/// Install / upgrade request (the web UI's `HelmInstallRequest`).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct HelmInstallRequest {
    /// Release name (install only; upgrade takes it from the path)
    #[serde(default)]
    pub name: String,
    /// `repo/chart`, `oci://...`, an URL or a path. For upgrades a bare chart
    /// name is resolved against the configured repositories.
    pub chart: String,
    pub version: Option<String>,
    /// Values (JSON object; helm reads it as YAML). `None` on upgrade keeps
    /// the release's current values.
    pub values: Option<serde_json::Value>,
    pub create_namespace: Option<bool>,
    pub wait: Option<bool>,
    pub timeout: Option<String>,
}

/// User-supplied and computed values of a release.
#[derive(Debug, Clone, Serialize)]
pub struct HelmValues {
    /// Same as `computed_values` (what the release actually runs with)
    pub values: serde_json::Value,
    /// Chart defaults merged with user values (`helm get values --all`)
    pub computed_values: serde_json::Value,
    /// Only what the user supplied (`helm get values`)
    pub user_supplied_values: serde_json::Value,
}

#[derive(Deserialize)]
struct ListItem {
    name: String,
    namespace: String,
    revision: String,
    #[serde(default)]
    updated: String,
    status: String,
    chart: String,
    #[serde(default)]
    app_version: String,
}

#[derive(Deserialize)]
struct StatusOut {
    name: String,
    namespace: String,
    version: u32,
    info: StatusInfo,
    #[serde(default)]
    chart: Option<StatusChart>,
}

#[derive(Deserialize)]
struct StatusInfo {
    #[serde(default)]
    last_deployed: String,
    #[serde(default)]
    description: String,
    status: String,
}

#[derive(Deserialize)]
struct StatusChart {
    metadata: StatusChartMeta,
}

#[derive(Deserialize)]
struct StatusChartMeta {
    name: String,
    #[serde(default)]
    version: String,
    #[serde(rename = "appVersion", default)]
    app_version: Option<String>,
}

#[derive(Deserialize)]
struct HistoryItem {
    revision: u32,
    #[serde(default)]
    updated: String,
    status: String,
    chart: String,
    #[serde(default)]
    app_version: String,
    #[serde(default)]
    description: String,
}

fn non_empty(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn parse_json<T: for<'de> Deserialize<'de>>(what: &str, out: &[u8]) -> K8sResult<T> {
    serde_json::from_slice(out).map_err(|e| {
        K8sError::HelmError(format!(
            "unexpected `helm {}` output ({}): {}",
            what,
            e,
            String::from_utf8_lossy(&out[..out.len().min(300)])
        ))
    })
}

fn from_status(s: StatusOut) -> HelmRelease {
    let (chart, chart_version, app_version) = match s.chart {
        Some(c) => (c.metadata.name, c.metadata.version, c.metadata.app_version),
        None => (String::new(), String::new(), None),
    };
    HelmRelease {
        name: s.name,
        namespace: s.namespace,
        revision: s.version,
        status: s.info.status,
        chart,
        chart_version,
        app_version: app_version.filter(|v| !v.is_empty()),
        updated: s.info.last_deployed,
        description: s.info.description,
    }
}

/// List releases in one namespace, or in all namespaces when `None`.
/// Pending, failed and uninstalling releases are included, not only
/// deployed ones (explicit state flags: helm 4 dropped v3's `--all`).
pub async fn list(target: &KubeTarget, namespace: Option<&str>) -> K8sResult<Vec<HelmRelease>> {
    let mut cmd = helm();
    cmd.args([
        "list",
        "--deployed",
        "--failed",
        "--pending",
        "--uninstalling",
        "--max",
        "10000",
        "--output",
        "json",
    ]);
    match namespace {
        Some(ns) => {
            validate_namespace(ns)?;
            cmd.arg("--namespace").arg(ns);
        }
        None => {
            cmd.arg("--all-namespaces");
        }
    }
    target.apply(&mut cmd);
    let out = run(cmd, "list").await?;
    if out.iter().all(u8::is_ascii_whitespace) {
        return Ok(Vec::new());
    }
    let items: Vec<ListItem> = parse_json("list", &out)?;
    Ok(items
        .into_iter()
        .map(|i| {
            let (chart, chart_version) = split_chart_version(&i.chart);
            HelmRelease {
                name: i.name,
                namespace: i.namespace,
                revision: i.revision.parse().unwrap_or(0),
                status: i.status,
                chart,
                chart_version,
                app_version: non_empty(i.app_version),
                updated: i.updated,
                description: String::new(),
            }
        })
        .collect())
}

/// Current state of one release (`helm status`).
pub async fn status(target: &KubeTarget, namespace: &str, name: &str) -> K8sResult<HelmRelease> {
    validate_namespace(namespace)?;
    validate_release_name(name)?;
    let mut cmd = helm();
    cmd.args(["status", name, "--namespace", namespace, "--output", "json"]);
    target.apply(&mut cmd);
    let out = run(cmd, "status").await?;
    Ok(from_status(parse_json("status", &out)?))
}

/// Writes values to a private temp file passed with `--values`.
fn values_file(values: &serde_json::Value) -> K8sResult<tempfile::NamedTempFile> {
    if !values.is_object() && !values.is_null() {
        return Err(K8sError::InvalidRequest(
            "values must be a JSON object".to_string(),
        ));
    }
    let mut f = tempfile::Builder::new()
        .prefix("helm-values-")
        .suffix(".json")
        .tempfile()
        .map_err(|e| K8sError::Internal(format!("values temp file: {}", e)))?;
    use std::io::Write;
    serde_json::to_writer(&mut f, values)
        .map_err(|e| K8sError::Internal(format!("values temp file: {}", e)))?;
    f.flush()
        .map_err(|e| K8sError::Internal(format!("values temp file: {}", e)))?;
    Ok(f)
}

fn common_flags(
    cmd: &mut tokio::process::Command,
    req: &HelmInstallRequest,
) -> K8sResult<Option<tempfile::NamedTempFile>> {
    if let Some(v) = req.version.as_deref().filter(|v| !v.is_empty()) {
        validate_plain("version", v)?;
        cmd.arg("--version").arg(v);
    }
    if req.wait.unwrap_or(false) {
        cmd.arg("--wait");
    }
    if let Some(t) = req.timeout.as_deref().filter(|t| !t.is_empty()) {
        validate_plain("timeout", t)?;
        cmd.arg("--timeout").arg(t);
    }
    match &req.values {
        Some(values) if !values.is_null() => {
            let f = values_file(values)?;
            cmd.arg("--values").arg(f.path());
            Ok(Some(f))
        }
        _ => Ok(None),
    }
}

/// `helm install` into `namespace`.
pub async fn install(
    target: &KubeTarget,
    namespace: &str,
    req: &HelmInstallRequest,
) -> K8sResult<HelmRelease> {
    validate_namespace(namespace)?;
    validate_release_name(&req.name)?;
    validate_chart_ref(&req.chart)?;
    let mut cmd = helm();
    cmd.arg("install").arg(&req.name).arg(&req.chart).args([
        "--namespace",
        namespace,
        "--output",
        "json",
    ]);
    if req.create_namespace.unwrap_or(false) {
        cmd.arg("--create-namespace");
    }
    let _values = common_flags(&mut cmd, req)?;
    target.apply(&mut cmd);
    let out = run(cmd, "install").await?;
    Ok(from_status(parse_json("install", &out)?))
}

/// Resolve a bare chart name (what `helm list` reports) to `repo/chart`.
async fn resolve_chart_ref(chart: &str) -> K8sResult<String> {
    if chart.contains('/') || chart.starts_with('.') || chart.contains("://") {
        return Ok(chart.to_string());
    }
    let candidates: Vec<String> = super::repos::search(chart, None)
        .await?
        .into_iter()
        .filter(|c| c.name == chart)
        .map(|c| format!("{}/{}", c.repository, c.name))
        .collect();
    match candidates.len() {
        1 => Ok(candidates.into_iter().next().unwrap()),
        0 => Err(K8sError::HelmNotFound(format!(
            "chart '{}' is not in any configured repository; add its repository or pass repo/chart",
            chart
        ))),
        _ => Err(K8sError::InvalidRequest(format!(
            "chart '{}' exists in several repositories ({}); pass repo/chart",
            chart,
            candidates.join(", ")
        ))),
    }
}

/// `helm upgrade` an existing release. Without `values` the current values
/// are kept (`--reuse-values`); with them they replace the user values.
pub async fn upgrade(
    target: &KubeTarget,
    namespace: &str,
    name: &str,
    req: &HelmInstallRequest,
) -> K8sResult<HelmRelease> {
    validate_namespace(namespace)?;
    validate_release_name(name)?;
    validate_chart_ref(&req.chart)?;
    let chart = resolve_chart_ref(&req.chart).await?;
    let mut cmd = helm();
    cmd.arg("upgrade")
        .arg(name)
        .arg(&chart)
        .args(["--namespace", namespace, "--output", "json"]);
    let values = common_flags(&mut cmd, req)?;
    if values.is_none() {
        cmd.arg("--reuse-values");
    }
    target.apply(&mut cmd);
    let out = run(cmd, "upgrade").await?;
    Ok(from_status(parse_json("upgrade", &out)?))
}

/// `helm uninstall`.
pub async fn uninstall(target: &KubeTarget, namespace: &str, name: &str) -> K8sResult<()> {
    validate_namespace(namespace)?;
    validate_release_name(name)?;
    let mut cmd = helm();
    cmd.args(["uninstall", name, "--namespace", namespace]);
    target.apply(&mut cmd);
    run(cmd, "uninstall").await?;
    Ok(())
}

/// `helm rollback` to `revision` (0 = previous). Returns the new revision.
pub async fn rollback(
    target: &KubeTarget,
    namespace: &str,
    name: &str,
    revision: u32,
) -> K8sResult<HelmRelease> {
    validate_namespace(namespace)?;
    validate_release_name(name)?;
    let mut cmd = helm();
    cmd.args(["rollback", name])
        .arg(revision.to_string())
        .args(["--namespace", namespace]);
    target.apply(&mut cmd);
    run(cmd, "rollback").await?;
    status(target, namespace, name).await
}

/// Every revision of a release, oldest first.
pub async fn history(
    target: &KubeTarget,
    namespace: &str,
    name: &str,
) -> K8sResult<Vec<HelmRelease>> {
    validate_namespace(namespace)?;
    validate_release_name(name)?;
    let mut cmd = helm();
    cmd.args([
        "history",
        name,
        "--namespace",
        namespace,
        "--max",
        "256",
        "--output",
        "json",
    ]);
    target.apply(&mut cmd);
    let out = run(cmd, "history").await?;
    let items: Vec<HistoryItem> = parse_json("history", &out)?;
    Ok(items
        .into_iter()
        .map(|h| {
            let (chart, chart_version) = split_chart_version(&h.chart);
            HelmRelease {
                name: name.to_string(),
                namespace: namespace.to_string(),
                revision: h.revision,
                status: h.status,
                chart,
                chart_version,
                app_version: non_empty(h.app_version),
                updated: h.updated,
                description: h.description,
            }
        })
        .collect())
}

async fn get_values(
    target: &KubeTarget,
    namespace: &str,
    name: &str,
    all: bool,
) -> K8sResult<serde_json::Value> {
    let mut cmd = helm();
    cmd.args([
        "get",
        "values",
        name,
        "--namespace",
        namespace,
        "--output",
        "json",
    ]);
    if all {
        cmd.arg("--all");
    }
    target.apply(&mut cmd);
    let out = run(cmd, "get values").await?;
    let text = String::from_utf8_lossy(&out);
    if text.trim().is_empty() || text.trim() == "null" {
        return Ok(serde_json::json!({}));
    }
    parse_json("get values", &out)
}

/// User-supplied and computed values of a release.
pub async fn values(target: &KubeTarget, namespace: &str, name: &str) -> K8sResult<HelmValues> {
    validate_namespace(namespace)?;
    validate_release_name(name)?;
    let user = get_values(target, namespace, name, false).await?;
    let computed = get_values(target, namespace, name, true).await?;
    Ok(HelmValues {
        values: computed.clone(),
        computed_values: computed,
        user_supplied_values: user,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_json_is_parsed() {
        // Trimmed real `helm status -o json` output (helm v3.16).
        let raw = br#"{"name":"web","info":{"first_deployed":"2026-10-16T10:00:00Z","last_deployed":"2026-10-16T10:05:00Z","deleted":"","description":"Upgrade complete","status":"deployed"},"chart":{"metadata":{"name":"hello","version":"0.2.0","appVersion":"1.1","apiVersion":"v2"}},"config":{"x":1},"version":2,"namespace":"apps"}"#;
        let r = from_status(parse_json("status", raw).unwrap());
        assert_eq!(r.name, "web");
        assert_eq!(r.namespace, "apps");
        assert_eq!(r.revision, 2);
        assert_eq!(r.status, "deployed");
        assert_eq!(r.chart, "hello");
        assert_eq!(r.chart_version, "0.2.0");
        assert_eq!(r.app_version.as_deref(), Some("1.1"));
        assert_eq!(r.description, "Upgrade complete");
    }

    #[test]
    fn upgrade_rejects_flag_like_chart() {
        let req = HelmInstallRequest {
            chart: "--post-renderer=/bin/sh".into(),
            ..Default::default()
        };
        assert!(validate_chart_ref(&req.chart).is_err());
    }
}
