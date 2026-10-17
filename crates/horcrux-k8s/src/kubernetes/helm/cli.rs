//! Running the `helm` binary: environment, kubeconfig hand-off, errors and
//! argument validation.

use crate::kubernetes::error::{K8sError, K8sResult};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

static HELM_HOME: OnceLock<PathBuf> = OnceLock::new();

/// Keep Helm's config/cache/data under `<data_dir>/helm`. Called once at
/// startup; without it helm falls back to its usual `$HOME` locations.
pub fn set_helm_home(data_dir: &Path) {
    let home = data_dir.join("helm");
    for sub in ["config", "cache", "data", "tmp"] {
        let _ = std::fs::create_dir_all(home.join(sub));
    }
    let _ = HELM_HOME.set(home);
}

fn helm_home() -> Option<&'static PathBuf> {
    HELM_HOME.get()
}

/// Directory helm keeps downloaded repository indexes in.
pub fn repository_cache_dir() -> Option<PathBuf> {
    helm_home().map(|h| h.join("cache").join("repository"))
}

/// A Horcrux bookkeeping file next to helm's own state (None without a
/// Helm home).
pub fn helm_state_file(name: &str) -> Option<PathBuf> {
    helm_home().map(|h| h.join(name))
}

/// The helm binary (override with HORCRUX_HELM_BIN, e.g. for a vendored copy).
fn helm_bin() -> String {
    std::env::var("HORCRUX_HELM_BIN").unwrap_or_else(|_| "helm".to_string())
}

/// A `helm` command with Horcrux's Helm home applied.
pub fn helm() -> Command {
    let mut cmd = Command::new(helm_bin());
    if let Some(home) = helm_home() {
        cmd.env("HELM_CONFIG_HOME", home.join("config"))
            .env("HELM_CACHE_HOME", home.join("cache"))
            .env("HELM_DATA_HOME", home.join("data"));
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    cmd
}

/// Run a helm command and return stdout. A non-zero exit becomes a
/// [`K8sError`] carrying helm's own error text (helm's messages are what the
/// user needs to see: "chart not found", "cannot re-use a name", ...).
pub async fn run(mut cmd: Command, what: &str) -> K8sResult<Vec<u8>> {
    run_with_stdin(&mut cmd, what, None).await
}

pub async fn run_with_stdin(
    cmd: &mut Command,
    what: &str,
    stdin: Option<&[u8]>,
) -> K8sResult<Vec<u8>> {
    if stdin.is_some() {
        cmd.stdin(Stdio::piped());
    }
    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            K8sError::HelmError(format!(
                "the helm binary was not found ({}); install app-admin/helm (USE=kubernetes pulls it in)",
                helm_bin()
            ))
        } else {
            K8sError::HelmError(format!("failed to run helm {}: {}", what, e))
        }
    })?;
    if let (Some(data), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(data)
            .await
            .map_err(|e| K8sError::HelmError(format!("helm {}: stdin: {}", what, e)))?;
        drop(pipe);
    }
    let out = child
        .wait_with_output()
        .await
        .map_err(|e| K8sError::HelmError(format!("helm {}: {}", what, e)))?;
    if out.status.success() {
        return Ok(out.stdout);
    }
    Err(classify(what, &String::from_utf8_lossy(&out.stderr)))
}

/// Map helm's stderr onto an API-meaningful error class.
pub fn classify(what: &str, stderr: &str) -> K8sError {
    let msg = stderr
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("; ");
    let msg = msg.strip_prefix("Error: ").unwrap_or(&msg).to_string();
    let lower = msg.to_lowercase();
    if lower.contains("release: not found")
        || lower.contains("not found")
            && (lower.contains("release") || lower.contains("repo") || lower.contains("chart"))
    {
        K8sError::HelmNotFound(format!("helm {}: {}", what, msg))
    } else if lower.contains("is not a valid chart repository") || lower.contains("invalid chart") {
        K8sError::InvalidRequest(format!("helm {}: {}", what, msg))
    } else if lower.contains("cannot re-use a name")
        || lower.contains("already exists")
        || lower.contains("another operation (install/upgrade/rollback) is in progress")
    {
        K8sError::Conflict(format!("helm {}: {}", what, msg))
    } else {
        K8sError::HelmError(format!("helm {} failed: {}", what, msg))
    }
}

/// Kubeconfig for one cluster, written to a private temp file that lives as
/// long as this value (helm is handed `--kubeconfig <file> --kube-context`).
pub struct KubeTarget {
    file: tempfile::NamedTempFile,
    context: Option<String>,
}

impl KubeTarget {
    pub fn new(kubeconfig_yaml: &str, context: Option<&str>) -> K8sResult<Self> {
        let dir = helm_home()
            .map(|h| h.join("tmp"))
            .unwrap_or_else(std::env::temp_dir);
        let _ = std::fs::create_dir_all(&dir);
        let mut file = tempfile::Builder::new()
            .prefix("kubeconfig-")
            .tempfile_in(&dir)
            .map_err(|e| K8sError::Internal(format!("kubeconfig temp file: {}", e)))?;
        use std::io::Write;
        file.write_all(kubeconfig_yaml.as_bytes())
            .and_then(|_| file.flush())
            .map_err(|e| K8sError::Internal(format!("kubeconfig temp file: {}", e)))?;
        Ok(Self {
            file,
            context: context.map(str::to_string),
        })
    }

    pub fn path(&self) -> &Path {
        self.file.path()
    }

    /// Add `--kubeconfig`/`--kube-context` to a helm command.
    pub fn apply(&self, cmd: &mut Command) {
        cmd.arg("--kubeconfig").arg(self.path());
        if let Some(ctx) = &self.context {
            cmd.arg("--kube-context").arg(ctx);
        }
    }
}

// ---------------------------------------------------------------------------
// Validation: every user-supplied value reaches helm as its own argv element,
// but a value starting with '-' would still be parsed as a flag.
// ---------------------------------------------------------------------------

fn bad(what: &str, value: &str, rule: &str) -> K8sError {
    K8sError::InvalidRequest(format!("invalid {} '{}': {}", what, value, rule))
}

/// Release names: DNS-1123 label, at most 53 characters (helm's own limit).
pub fn validate_release_name(name: &str) -> K8sResult<()> {
    if is_dns_label(name, 53) {
        Ok(())
    } else {
        Err(bad(
            "release name",
            name,
            "lowercase letters, digits and '-', starting and ending alphanumeric, max 53 chars",
        ))
    }
}

/// Kubernetes namespace: DNS-1123 label, at most 63 characters.
pub fn validate_namespace(ns: &str) -> K8sResult<()> {
    if is_dns_label(ns, 63) {
        Ok(())
    } else {
        Err(bad(
            "namespace",
            ns,
            "lowercase letters, digits and '-', starting and ending alphanumeric, max 63 chars",
        ))
    }
}

/// Repository names as helm accepts them (no '/', no leading '-').
pub fn validate_repo_name(name: &str) -> K8sResult<()> {
    let ok = !name.is_empty()
        && name.len() <= 100
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if ok {
        Ok(())
    } else {
        Err(bad(
            "repository name",
            name,
            "letters, digits, '-', '_' and '.', not starting with '-'",
        ))
    }
}

/// Chart names inside a repository.
pub fn validate_chart_name(name: &str) -> K8sResult<()> {
    let ok = !name.is_empty()
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if ok {
        Ok(())
    } else {
        Err(bad("chart name", name, "letters, digits, '-', '_' and '.'"))
    }
}

/// A chart reference: `repo/chart`, an OCI/HTTP(S) URL or a local path.
pub fn validate_chart_ref(chart: &str) -> K8sResult<()> {
    if chart.trim().is_empty() || chart.starts_with('-') || chart.chars().any(char::is_whitespace) {
        return Err(bad(
            "chart reference",
            chart,
            "expected repo/chart, oci://..., https://... or a path",
        ));
    }
    Ok(())
}

/// Versions and timeouts are free-form for helm but must not look like flags.
pub fn validate_plain(what: &str, value: &str) -> K8sResult<()> {
    if value.is_empty()
        || value.starts_with('-')
        || !value.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(
                    c,
                    '.' | '-' | '+' | '_' | '^' | '~' | '<' | '>' | '=' | '*' | ' ' | ','
                )
        })
    {
        return Err(bad(what, value, "unexpected characters"));
    }
    Ok(())
}

fn is_dns_label(s: &str, max: usize) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= max
        && b[0].is_ascii_alphanumeric()
        && b[b.len() - 1].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// Split helm's `chart` column ("name-1.2.3", "my-app-0.1.0-rc.1") into
/// name and version: the version starts at the first '-' that is followed
/// by a digit (optionally after a 'v').
pub fn split_chart_version(chart: &str) -> (String, String) {
    let bytes = chart.as_bytes();
    for (i, c) in bytes.iter().enumerate() {
        if *c != b'-' || i == 0 {
            continue;
        }
        let rest = &chart[i + 1..];
        let rest_digits = rest.strip_prefix('v').unwrap_or(rest);
        if rest_digits
            .chars()
            .next()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false)
        {
            return (chart[..i].to_string(), rest.to_string());
        }
    }
    (chart.to_string(), String::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chart_version_split() {
        assert_eq!(
            split_chart_version("nginx-15.4.2"),
            ("nginx".into(), "15.4.2".into())
        );
        assert_eq!(
            split_chart_version("my-app-0.1.0-rc.1"),
            ("my-app".into(), "0.1.0-rc.1".into())
        );
        assert_eq!(
            split_chart_version("kube-state-metrics-v2.0.0"),
            ("kube-state-metrics".into(), "v2.0.0".into())
        );
        assert_eq!(
            split_chart_version("noversion"),
            ("noversion".into(), "".into())
        );
    }

    #[test]
    fn names_that_look_like_flags_are_rejected() {
        assert!(validate_release_name("web-1").is_ok());
        assert!(validate_release_name("--set").is_err());
        assert!(validate_release_name("Web").is_err());
        assert!(validate_release_name(&"a".repeat(54)).is_err());
        assert!(validate_namespace("kube-system").is_ok());
        assert!(validate_namespace("-n").is_err());
        assert!(validate_repo_name("bitnami").is_ok());
        assert!(validate_repo_name("a/b").is_err());
        assert!(validate_chart_ref("bitnami/nginx").is_ok());
        assert!(validate_chart_ref("--post-renderer=x").is_err());
        assert!(validate_chart_ref("a b").is_err());
        assert!(validate_plain("version", "1.2.3").is_ok());
        assert!(validate_plain("version", "--x").is_err());
        assert!(validate_plain("timeout", "300s").is_ok());
    }

    #[test]
    fn helm_errors_are_classified() {
        assert!(matches!(
            classify(
                "install",
                "Error: INSTALLATION FAILED: cannot re-use a name that is still in use\n"
            ),
            K8sError::Conflict(_)
        ));
        assert!(matches!(
            classify("status", "Error: release: not found\n"),
            K8sError::HelmNotFound(_)
        ));
        assert!(matches!(
            classify("install", "Error: boom\n"),
            K8sError::HelmError(_)
        ));
    }
}
