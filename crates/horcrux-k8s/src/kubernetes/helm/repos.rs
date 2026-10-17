//! Helm repositories and charts.
//!
//! Repository CRUD and search go through `helm repo` / `helm search repo`.
//! Chart metadata that `helm search` does not print (icon, keywords,
//! maintainers, creation time, every version) is read from the repository
//! index helm downloaded (`<cache>/repository/<repo>-index.yaml`).

use super::cli::{
    helm, helm_state_file, repository_cache_dir, run, run_with_stdin, validate_chart_name,
    validate_plain, validate_repo_name,
};
use crate::kubernetes::error::{K8sError, K8sResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

/// A configured chart repository (the web UI's `HelmRepository`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HelmRepository {
    pub name: String,
    pub url: String,
    pub description: Option<String>,
    /// When the repository was added through Horcrux (helm itself does not
    /// record it; repositories added with the helm CLI show none).
    pub added_at: Option<String>,
    /// "ready" once helm has downloaded the index, otherwise "not-indexed"
    pub status: String,
    /// When the index was last downloaded (RFC 3339)
    pub last_update: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AddRepoRequest {
    pub name: String,
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub force_update: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HelmMaintainer {
    pub name: String,
    pub email: Option<String>,
}

/// A chart in a repository (the web UI's `HelmChart`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HelmChart {
    /// Chart name without the repository prefix
    pub name: String,
    pub version: String,
    pub app_version: Option<String>,
    pub description: String,
    /// Repository name (install as `<repository>/<name>`)
    pub repository: String,
    pub icon: Option<String>,
    pub keywords: Vec<String>,
    pub maintainers: Vec<HelmMaintainer>,
    pub created: String,
}

#[derive(Deserialize)]
struct RepoListItem {
    name: String,
    url: String,
}

/// name -> RFC 3339 time the repository was added through Horcrux.
fn load_added() -> HashMap<String, String> {
    helm_state_file("repos-added.json")
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn store_added(map: &HashMap<String, String>) {
    if let Some(p) = helm_state_file("repos-added.json") {
        if let Ok(b) = serde_json::to_vec_pretty(map) {
            let _ = std::fs::write(p, b);
        }
    }
}

async fn cache_dir() -> K8sResult<PathBuf> {
    if let Some(dir) = repository_cache_dir() {
        return Ok(dir);
    }
    let mut cmd = helm();
    cmd.args(["env", "HELM_REPOSITORY_CACHE"]);
    let out = run(cmd, "env").await?;
    Ok(PathBuf::from(String::from_utf8_lossy(&out).trim()))
}

fn index_path(cache: &std::path::Path, repo: &str) -> PathBuf {
    cache.join(format!("{}-index.yaml", repo))
}

/// Configured repositories with their index state.
pub async fn list() -> K8sResult<Vec<HelmRepository>> {
    let mut cmd = helm();
    cmd.args(["repo", "list", "--output", "json"]);
    let out = match run(cmd, "repo list").await {
        Ok(out) => out,
        // helm exits 1 with "no repositories to show" when none are configured
        Err(K8sError::HelmError(msg)) | Err(K8sError::HelmNotFound(msg))
            if msg.contains("no repositories") =>
        {
            return Ok(Vec::new())
        }
        Err(e) => return Err(e),
    };
    if out.iter().all(u8::is_ascii_whitespace) {
        return Ok(Vec::new());
    }
    let items: Vec<RepoListItem> = serde_json::from_slice(&out)
        .map_err(|e| K8sError::HelmError(format!("unexpected `helm repo list` output: {}", e)))?;
    let cache = cache_dir().await?;
    let added = load_added();
    Ok(items
        .into_iter()
        .map(|r| {
            let modified = std::fs::metadata(index_path(&cache, &r.name))
                .and_then(|m| m.modified())
                .ok();
            HelmRepository {
                status: if modified.is_some() {
                    "ready".into()
                } else {
                    "not-indexed".into()
                },
                last_update: modified
                    .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339()),
                added_at: added.get(&r.name).cloned(),
                name: r.name,
                url: r.url,
                description: None,
            }
        })
        .collect())
}

/// One repository by name.
pub async fn get(name: &str) -> K8sResult<HelmRepository> {
    list()
        .await?
        .into_iter()
        .find(|r| r.name == name)
        .ok_or_else(|| K8sError::HelmNotFound(format!("helm repository '{}' not found", name)))
}

/// `helm repo add` (downloads the index, so a bad URL fails here).
pub async fn add(req: &AddRepoRequest) -> K8sResult<HelmRepository> {
    validate_repo_name(&req.name)?;
    let url = req.url.trim();
    if !(url.starts_with("https://") || url.starts_with("http://"))
        || url.contains(char::is_whitespace)
    {
        return Err(K8sError::InvalidRequest(format!(
            "repository URL must be http(s)://..., got '{}'",
            req.url
        )));
    }
    let mut cmd = helm();
    cmd.args(["repo", "add", &req.name, url]);
    if req.force_update.unwrap_or(false) {
        cmd.arg("--force-update");
    }
    let password = match (
        req.username.as_deref().filter(|u| !u.is_empty()),
        req.password.as_deref().filter(|p| !p.is_empty()),
    ) {
        (Some(user), pass) => {
            if user.starts_with('-') {
                return Err(K8sError::InvalidRequest("invalid username".into()));
            }
            cmd.arg("--username").arg(user);
            if pass.is_some() {
                cmd.arg("--password-stdin");
            }
            pass
        }
        (None, Some(_)) => {
            return Err(K8sError::InvalidRequest(
                "a password needs a username".into(),
            ))
        }
        (None, None) => None,
    };
    run_with_stdin(&mut cmd, "repo add", password.map(str::as_bytes)).await?;
    let mut added = load_added();
    added.insert(req.name.clone(), chrono::Utc::now().to_rfc3339());
    store_added(&added);
    get(&req.name).await
}

/// `helm repo update <name>`.
pub async fn update(name: &str) -> K8sResult<HelmRepository> {
    validate_repo_name(name)?;
    get(name).await?;
    let mut cmd = helm();
    cmd.args(["repo", "update", name]);
    run(cmd, "repo update").await?;
    get(name).await
}

/// `helm repo remove <name>`.
pub async fn remove(name: &str) -> K8sResult<()> {
    validate_repo_name(name)?;
    get(name).await?;
    let mut cmd = helm();
    cmd.args(["repo", "remove", name]);
    run(cmd, "repo remove").await?;
    let mut added = load_added();
    if added.remove(name).is_some() {
        store_added(&added);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Repository index
// ---------------------------------------------------------------------------

/// YAML scalars that may be written unquoted as numbers (`version: 1.0`).
fn de_stringish<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let v: Option<serde_yaml::Value> = Option::deserialize(d)?;
    Ok(match v {
        Some(serde_yaml::Value::String(s)) => Some(s),
        Some(serde_yaml::Value::Number(n)) => Some(n.to_string()),
        Some(serde_yaml::Value::Bool(b)) => Some(b.to_string()),
        _ => None,
    })
}

#[derive(Debug, Clone, Deserialize)]
struct IndexMaintainer {
    #[serde(default)]
    name: String,
    #[serde(default)]
    email: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct IndexEntry {
    #[serde(default, deserialize_with = "de_stringish")]
    version: Option<String>,
    #[serde(rename = "appVersion", default, deserialize_with = "de_stringish")]
    app_version: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    keywords: Option<Vec<String>>,
    #[serde(default)]
    maintainers: Option<Vec<IndexMaintainer>>,
    #[serde(default, deserialize_with = "de_stringish")]
    created: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IndexFile {
    #[serde(default)]
    entries: HashMap<String, Vec<IndexEntry>>,
}

type Index = Arc<HashMap<String, Vec<IndexEntry>>>;

fn index_cache() -> &'static Mutex<HashMap<PathBuf, (SystemTime, Index)>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, (SystemTime, Index)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Parsed index of `repo` (cached until the file changes).
async fn load_index(repo: &str) -> K8sResult<Index> {
    let path = index_path(&cache_dir().await?, repo);
    let modified = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .map_err(|_| {
            K8sError::HelmNotFound(format!(
                "helm repository '{}' has no downloaded index (add or update it first)",
                repo
            ))
        })?;
    if let Some((t, idx)) = index_cache().lock().unwrap().get(&path) {
        if *t == modified {
            return Ok(idx.clone());
        }
    }
    let p = path.clone();
    let parsed = tokio::task::spawn_blocking(move || -> K8sResult<IndexFile> {
        let f = std::fs::File::open(&p)
            .map_err(|e| K8sError::HelmError(format!("{}: {}", p.display(), e)))?;
        serde_yaml::from_reader(std::io::BufReader::new(f))
            .map_err(|e| K8sError::HelmError(format!("{}: {}", p.display(), e)))
    })
    .await
    .map_err(|e| K8sError::Internal(e.to_string()))??;
    let idx: Index = Arc::new(parsed.entries);
    index_cache()
        .lock()
        .unwrap()
        .insert(path, (modified, idx.clone()));
    Ok(idx)
}

fn chart_from_entry(repo: &str, name: &str, e: &IndexEntry) -> HelmChart {
    HelmChart {
        name: name.to_string(),
        version: e.version.clone().unwrap_or_default(),
        app_version: e.app_version.clone().filter(|v| !v.is_empty()),
        description: e.description.clone().unwrap_or_default(),
        repository: repo.to_string(),
        icon: e.icon.clone().filter(|i| !i.is_empty()),
        keywords: e.keywords.clone().unwrap_or_default(),
        maintainers: e
            .maintainers
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|m| HelmMaintainer {
                name: m.name,
                email: m.email.filter(|e| !e.is_empty()),
            })
            .collect(),
        created: e.created.clone().unwrap_or_default(),
    }
}

#[derive(Deserialize)]
struct SearchItem {
    name: String,
    version: String,
    #[serde(default)]
    app_version: String,
    #[serde(default)]
    description: String,
}

/// `helm search repo [query]` (latest version of each chart), optionally
/// limited to one repository, enriched with index metadata.
pub async fn search(query: &str, repo: Option<&str>) -> K8sResult<Vec<HelmChart>> {
    let query = query.trim();
    if let Some(r) = repo {
        validate_repo_name(r)?;
    }
    let mut cmd = helm();
    cmd.args(["search", "repo"]);
    if !query.is_empty() {
        if query.starts_with('-') || query.chars().any(char::is_control) {
            return Err(K8sError::InvalidRequest(format!(
                "invalid search query '{}'",
                query
            )));
        }
        cmd.arg(query);
    }
    cmd.args(["--output", "json"]);
    let out = match run(cmd, "search repo").await {
        Ok(out) => out,
        Err(K8sError::HelmError(msg)) if msg.contains("no repositories configured") => {
            return Ok(Vec::new())
        }
        Err(e) => return Err(e),
    };
    if out.iter().all(u8::is_ascii_whitespace) {
        return Ok(Vec::new());
    }
    let items: Vec<SearchItem> = serde_json::from_slice(&out)
        .map_err(|e| K8sError::HelmError(format!("unexpected `helm search` output: {}", e)))?;
    let mut charts = Vec::new();
    let mut indexes: HashMap<String, Option<Index>> = HashMap::new();
    for item in items {
        let Some((repo_name, chart_name)) = item.name.split_once('/') else {
            continue;
        };
        if repo.map(|r| r != repo_name).unwrap_or(false) {
            continue;
        }
        if !indexes.contains_key(repo_name) {
            indexes.insert(repo_name.to_string(), load_index(repo_name).await.ok());
        }
        let entry = indexes
            .get(repo_name)
            .and_then(|i| i.as_ref())
            .and_then(|i| i.get(chart_name))
            .and_then(|versions| {
                versions
                    .iter()
                    .find(|e| e.version.as_deref() == Some(item.version.as_str()))
            });
        let mut chart = match entry {
            Some(e) => chart_from_entry(repo_name, chart_name, e),
            None => HelmChart {
                name: chart_name.to_string(),
                version: String::new(),
                app_version: None,
                description: String::new(),
                repository: repo_name.to_string(),
                icon: None,
                keywords: Vec::new(),
                maintainers: Vec::new(),
                created: String::new(),
            },
        };
        chart.version = item.version;
        chart.app_version = Some(item.app_version).filter(|v| !v.is_empty());
        chart.description = item.description;
        charts.push(chart);
    }
    Ok(charts)
}

/// Latest version of one chart.
pub async fn chart(repo: &str, name: &str) -> K8sResult<HelmChart> {
    validate_repo_name(repo)?;
    validate_chart_name(name)?;
    let idx = load_index(repo).await?;
    let entry = idx
        .get(name)
        .and_then(|v| v.first())
        .ok_or_else(|| K8sError::HelmNotFound(format!("chart {}/{} not found", repo, name)))?;
    Ok(chart_from_entry(repo, name, entry))
}

/// Every published version of a chart, newest first (index order).
pub async fn versions(repo: &str, name: &str) -> K8sResult<Vec<String>> {
    validate_repo_name(repo)?;
    validate_chart_name(name)?;
    let idx = load_index(repo).await?;
    let entries = idx
        .get(name)
        .ok_or_else(|| K8sError::HelmNotFound(format!("chart {}/{} not found", repo, name)))?;
    Ok(entries.iter().filter_map(|e| e.version.clone()).collect())
}

/// The chart's default values (`helm show values`) as JSON.
pub async fn chart_values(
    repo: &str,
    name: &str,
    version: Option<&str>,
) -> K8sResult<serde_json::Value> {
    validate_repo_name(repo)?;
    validate_chart_name(name)?;
    let mut cmd = helm();
    cmd.args(["show", "values"])
        .arg(format!("{}/{}", repo, name));
    if let Some(v) = version.filter(|v| !v.is_empty()) {
        validate_plain("version", v)?;
        cmd.arg("--version").arg(v);
    }
    let out = run(cmd, "show values").await?;
    yaml_to_json(&String::from_utf8_lossy(&out))
}

fn yaml_to_json(text: &str) -> K8sResult<serde_json::Value> {
    if text.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    let v: serde_json::Value = serde_yaml::from_str(text)
        .map_err(|e| K8sError::HelmError(format!("chart values are not valid YAML: {}", e)))?;
    Ok(if v.is_null() {
        serde_json::json!({})
    } else {
        v
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_entries_with_numeric_versions_parse() {
        let raw = r#"
apiVersion: v1
entries:
  hello:
  - apiVersion: v2
    appVersion: 1.16
    created: "2026-10-16T10:00:00Z"
    description: Hello chart
    icon: https://example.org/i.png
    keywords: [demo, web]
    maintainers:
    - name: Horcrux
      email: hermes@errant.solutions
    name: hello
    version: 0.2.0
  - name: hello
    version: 0.1.0
generated: "2026-10-16T10:00:00Z"
"#;
        let idx: IndexFile = serde_yaml::from_str(raw).unwrap();
        let e = &idx.entries["hello"][0];
        let c = chart_from_entry("local", "hello", e);
        assert_eq!(c.version, "0.2.0");
        assert_eq!(c.app_version.as_deref(), Some("1.16"));
        assert_eq!(c.keywords, vec!["demo", "web"]);
        assert_eq!(c.maintainers[0].name, "Horcrux");
        assert_eq!(c.repository, "local");
        assert_eq!(idx.entries["hello"].len(), 2);
    }

    #[test]
    fn values_yaml_becomes_json() {
        let v = yaml_to_json("# comment\nreplicaCount: 1\nimage:\n  tag: \"1.2\"\n").unwrap();
        assert_eq!(v["replicaCount"], 1);
        assert_eq!(v["image"]["tag"], "1.2");
        assert_eq!(yaml_to_json("").unwrap(), serde_json::json!({}));
    }
}
