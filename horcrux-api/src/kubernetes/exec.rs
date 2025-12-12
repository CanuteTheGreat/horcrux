//! Kubernetes exec operations
//!
//! Execute commands in containers via WebSocket-based exec.

use crate::kubernetes::client::K8sClient;
#[cfg(not(feature = "kubernetes"))]
use crate::kubernetes::error::K8sError;
use crate::kubernetes::error::K8sResult;
use crate::kubernetes::types::PodExecRequest;

/// Execute a command in a container and return the output
#[cfg(feature = "kubernetes")]
pub async fn exec_command(
    client: &K8sClient,
    namespace: &str,
    pod_name: &str,
    request: &PodExecRequest,
) -> K8sResult<ExecOutput> {
    use k8s_openapi::api::core::v1::Pod;
    use kube::api::{Api, AttachParams};
    use tokio::io::AsyncReadExt;

    let pods: Api<Pod> = Api::namespaced(client.inner().clone(), namespace);

    let attach_params = AttachParams {
        container: request.container.clone(),
        tty: request.tty,
        stdin: request.stdin,
        stdout: true,
        stderr: true,
        max_stdin_buf_size: Some(1024),
        max_stdout_buf_size: Some(1024 * 1024),
        max_stderr_buf_size: Some(1024 * 1024),
    };

    let mut attached = pods
        .exec(pod_name, &request.command, &attach_params)
        .await?;

    // Collect stdout
    let mut stdout = String::new();
    if let Some(mut stdout_reader) = attached.stdout() {
        let mut buf = Vec::new();
        if stdout_reader.read_to_end(&mut buf).await.is_ok() {
            stdout = String::from_utf8_lossy(&buf).to_string();
        }
    }

    // Collect stderr
    let mut stderr = String::new();
    if let Some(mut stderr_reader) = attached.stderr() {
        let mut buf = Vec::new();
        if stderr_reader.read_to_end(&mut buf).await.is_ok() {
            stderr = String::from_utf8_lossy(&buf).to_string();
        }
    }

    // Wait for the process to complete and get exit code
    let status = attached
        .take_status()
        .ok_or_else(|| {
            crate::kubernetes::error::K8sError::ExecError("No status channel".to_string())
        })?
        .await
        .ok_or_else(|| {
            crate::kubernetes::error::K8sError::ExecError("Status channel closed".to_string())
        })?;

    let exit_code = status
        .status
        .as_ref()
        .and_then(|s| {
            if s == "Success" {
                Some(0)
            } else {
                // Try to parse exit code from reason
                status.reason.as_ref().and_then(|r| {
                    if r.starts_with("ExitCode:") {
                        r.trim_start_matches("ExitCode:").trim().parse().ok()
                    } else {
                        Some(1)
                    }
                })
            }
        })
        .unwrap_or(1);

    Ok(ExecOutput {
        stdout,
        stderr,
        exit_code,
    })
}

/// Get an interactive exec session (returns the attached process for WebSocket integration)
#[cfg(feature = "kubernetes")]
pub async fn exec_interactive(
    client: &K8sClient,
    namespace: &str,
    pod_name: &str,
    request: &PodExecRequest,
) -> K8sResult<kube::api::AttachedProcess> {
    use k8s_openapi::api::core::v1::Pod;
    use kube::api::{Api, AttachParams};

    let pods: Api<Pod> = Api::namespaced(client.inner().clone(), namespace);

    let attach_params = AttachParams {
        container: request.container.clone(),
        tty: request.tty,
        stdin: request.stdin,
        stdout: true,
        stderr: !request.tty, // stderr is merged with stdout when tty is enabled
        max_stdin_buf_size: Some(4096),
        max_stdout_buf_size: Some(1024 * 1024),
        max_stderr_buf_size: Some(1024 * 1024),
    };

    let attached = pods
        .exec(pod_name, &request.command, &attach_params)
        .await?;

    Ok(attached)
}

/// Attach to a running container (similar to docker attach)
#[cfg(feature = "kubernetes")]
pub async fn attach_container(
    client: &K8sClient,
    namespace: &str,
    pod_name: &str,
    container: Option<&str>,
    tty: bool,
) -> K8sResult<kube::api::AttachedProcess> {
    use k8s_openapi::api::core::v1::Pod;
    use kube::api::{Api, AttachParams};

    let pods: Api<Pod> = Api::namespaced(client.inner().clone(), namespace);

    let attach_params = AttachParams {
        container: container.map(String::from),
        tty,
        stdin: true,
        stdout: true,
        stderr: !tty,
        max_stdin_buf_size: Some(4096),
        max_stdout_buf_size: Some(1024 * 1024),
        max_stderr_buf_size: Some(1024 * 1024),
    };

    let attached = pods.attach(pod_name, &attach_params).await?;

    Ok(attached)
}

/// Output from an exec command
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExecOutput {
    /// Standard output
    pub stdout: String,
    /// Standard error
    pub stderr: String,
    /// Exit code (0 for success)
    pub exit_code: i32,
}

// ============================================================================
// Stubs for when kubernetes feature is disabled
// ============================================================================

#[cfg(not(feature = "kubernetes"))]
pub async fn exec_command(
    _client: &K8sClient,
    _namespace: &str,
    _pod_name: &str,
    _request: &PodExecRequest,
) -> K8sResult<ExecOutput> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}

#[cfg(not(feature = "kubernetes"))]
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExecOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}
