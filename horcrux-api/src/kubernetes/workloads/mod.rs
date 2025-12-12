//! Kubernetes workload management
//!
//! Handles Pods, Deployments, StatefulSets, DaemonSets, Jobs, and CronJobs.

pub mod pods;
pub mod deployments;
pub mod statefulsets;
pub mod daemonsets;
pub mod jobs;

use crate::kubernetes::client::K8sClient;
use crate::kubernetes::error::K8sResult;
use crate::kubernetes::types::*;

/// Workload manager aggregating all workload operations
pub struct WorkloadManager;

impl WorkloadManager {
    pub fn new() -> Self {
        Self
    }

    // Pod operations
    pub async fn list_pods(
        &self,
        client: &K8sClient,
        namespace: &str,
        label_selector: Option<&str>,
    ) -> K8sResult<Vec<PodInfo>> {
        pods::list_pods(client, namespace, label_selector).await
    }

    pub async fn get_pod(
        &self,
        client: &K8sClient,
        namespace: &str,
        name: &str,
    ) -> K8sResult<PodInfo> {
        pods::get_pod(client, namespace, name).await
    }

    pub async fn delete_pod(
        &self,
        client: &K8sClient,
        namespace: &str,
        name: &str,
    ) -> K8sResult<()> {
        pods::delete_pod(client, namespace, name).await
    }

    // Deployment operations
    pub async fn list_deployments(
        &self,
        client: &K8sClient,
        namespace: &str,
    ) -> K8sResult<Vec<DeploymentInfo>> {
        deployments::list_deployments(client, namespace).await
    }

    pub async fn get_deployment(
        &self,
        client: &K8sClient,
        namespace: &str,
        name: &str,
    ) -> K8sResult<DeploymentInfo> {
        deployments::get_deployment(client, namespace, name).await
    }

    pub async fn scale_deployment(
        &self,
        client: &K8sClient,
        namespace: &str,
        name: &str,
        replicas: i32,
    ) -> K8sResult<DeploymentInfo> {
        deployments::scale_deployment(client, namespace, name, replicas).await
    }

    pub async fn restart_deployment(
        &self,
        client: &K8sClient,
        namespace: &str,
        name: &str,
    ) -> K8sResult<()> {
        deployments::restart_deployment(client, namespace, name).await
    }
}

impl Default for WorkloadManager {
    fn default() -> Self {
        Self::new()
    }
}
