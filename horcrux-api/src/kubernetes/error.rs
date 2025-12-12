//! Kubernetes error types and ApiError mapping
//!
//! Maps kube-rs errors to Horcrux API errors for consistent error handling.

use crate::error::ApiError;
use thiserror::Error;

/// Kubernetes-specific errors
#[derive(Debug, Error)]
pub enum K8sError {
    /// Cluster is not connected
    #[error("Cluster not connected: {0}")]
    ClusterNotConnected(String),

    /// Cluster not found in registry
    #[error("Cluster not found: {0}")]
    ClusterNotFound(String),

    /// Kubernetes resource not found
    #[error("Resource not found: {kind}/{name} in namespace {namespace}")]
    ResourceNotFound {
        kind: String,
        name: String,
        namespace: String,
    },

    /// Error from kube-rs client
    #[cfg(feature = "kubernetes")]
    #[error("Kubernetes API error: {0}")]
    KubeError(#[from] kube::Error),

    /// Invalid kubeconfig
    #[error("Invalid kubeconfig: {0}")]
    InvalidKubeconfig(String),

    /// Namespace not found
    #[error("Namespace not found: {0}")]
    NamespaceNotFound(String),

    /// Permission denied
    #[error("Permission denied: {0}")]
    Forbidden(String),

    /// Resource conflict
    #[error("Conflict: {0}")]
    Conflict(String),

    /// Cluster provisioning error
    #[error("Provisioning error: {0}")]
    ProvisioningError(String),

    /// Helm operation error
    #[error("Helm error: {0}")]
    HelmError(String),

    /// Watch stream error
    #[error("Watch error: {0}")]
    WatchError(String),

    /// Exec session error
    #[error("Exec error: {0}")]
    ExecError(String),

    /// Port forward error
    #[error("Port forward error: {0}")]
    PortForwardError(String),

    /// Internal system error
    #[error("Internal error: {0}")]
    Internal(String),
}

impl From<K8sError> for ApiError {
    fn from(err: K8sError) -> Self {
        match err {
            K8sError::ClusterNotConnected(id) => {
                ApiError::ServiceUnavailable(format!("Kubernetes cluster '{}' not connected", id))
            }
            K8sError::ClusterNotFound(id) => {
                ApiError::NotFound(format!("Kubernetes cluster '{}' not found", id))
            }
            K8sError::ResourceNotFound {
                kind,
                name,
                namespace,
            } => ApiError::NotFound(format!(
                "{}/{} not found in namespace {}",
                kind, name, namespace
            )),
            #[cfg(feature = "kubernetes")]
            K8sError::KubeError(e) => {
                let err_str = e.to_string();
                if err_str.contains("401") || err_str.contains("Unauthorized") {
                    ApiError::AuthenticationFailed
                } else if err_str.contains("403") || err_str.contains("Forbidden") {
                    ApiError::Forbidden(err_str)
                } else if err_str.contains("404") || err_str.contains("NotFound") {
                    ApiError::NotFound(err_str)
                } else if err_str.contains("409") || err_str.contains("AlreadyExists") {
                    ApiError::Conflict(err_str)
                } else if err_str.contains("422") || err_str.contains("Invalid") {
                    ApiError::ValidationError(err_str)
                } else {
                    ApiError::Internal(format!("Kubernetes error: {}", e))
                }
            }
            K8sError::InvalidKubeconfig(msg) => ApiError::ValidationError(msg),
            K8sError::NamespaceNotFound(ns) => {
                ApiError::NotFound(format!("Namespace '{}' not found", ns))
            }
            K8sError::Forbidden(msg) => ApiError::Forbidden(msg),
            K8sError::Conflict(msg) => ApiError::Conflict(msg),
            K8sError::ProvisioningError(msg) => {
                ApiError::Internal(format!("Provisioning error: {}", msg))
            }
            K8sError::HelmError(msg) => ApiError::Internal(format!("Helm error: {}", msg)),
            K8sError::WatchError(msg) => ApiError::Internal(format!("Watch error: {}", msg)),
            K8sError::ExecError(msg) => ApiError::Internal(format!("Exec error: {}", msg)),
            K8sError::PortForwardError(msg) => {
                ApiError::Internal(format!("Port forward error: {}", msg))
            }
            K8sError::Internal(msg) => ApiError::Internal(msg),
        }
    }
}

impl From<K8sError> for horcrux_common::Error {
    fn from(err: K8sError) -> Self {
        horcrux_common::Error::System(err.to_string())
    }
}

/// Result type alias for Kubernetes operations
pub type K8sResult<T> = std::result::Result<T, K8sError>;
