//! Kubernetes types for Horcrux API
//!
//! Simplified representations of Kubernetes resources for API responses.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Kubernetes cluster information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct K8sCluster {
    /// Unique cluster identifier
    pub id: String,
    /// User-friendly cluster name
    pub name: String,
    /// Kubeconfig context name
    pub context: String,
    /// Kubernetes API server URL
    pub api_server: String,
    /// Kubernetes version (e.g., "v1.30.0")
    pub version: Option<String>,
    /// Cluster connection status
    pub status: ClusterStatus,
    /// Number of nodes in the cluster
    pub node_count: u32,
    /// How the cluster was provisioned
    pub provider: ClusterProvider,
    /// Unix timestamp of cluster registration
    pub created_at: i64,
    /// Unix timestamp of last update
    pub updated_at: i64,
}

/// Cluster connection status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClusterStatus {
    /// Successfully connected to cluster
    Connected,
    /// Not connected or connection lost
    Disconnected,
    /// Cluster is being provisioned
    Provisioning,
    /// Connection error or cluster unhealthy
    Error,
    /// Status unknown
    Unknown,
}

impl Default for ClusterStatus {
    fn default() -> Self {
        Self::Unknown
    }
}

impl std::fmt::Display for ClusterStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connected => write!(f, "connected"),
            Self::Disconnected => write!(f, "disconnected"),
            Self::Provisioning => write!(f, "provisioning"),
            Self::Error => write!(f, "error"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// Cluster provisioning method
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClusterProvider {
    /// External cluster (connected via kubeconfig)
    External,
    /// Provisioned with k3s
    K3s,
    /// Provisioned with kubeadm
    Kubeadm,
    /// Managed Kubernetes (EKS, GKE, AKS, etc.)
    Managed,
}

impl Default for ClusterProvider {
    fn default() -> Self {
        Self::External
    }
}

impl std::fmt::Display for ClusterProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::External => write!(f, "external"),
            Self::K3s => write!(f, "k3s"),
            Self::Kubeadm => write!(f, "kubeadm"),
            Self::Managed => write!(f, "managed"),
        }
    }
}

/// Request to connect a new cluster
#[derive(Debug, Clone, Deserialize)]
pub struct ClusterConnectRequest {
    /// User-friendly name for the cluster
    pub name: String,
    /// Kubeconfig content (YAML)
    pub kubeconfig: String,
    /// Context to use from kubeconfig (optional, uses current-context if not specified)
    pub context: Option<String>,
}

/// Request to provision a new cluster
#[derive(Debug, Clone, Deserialize)]
pub struct ClusterProvisionRequest {
    /// Cluster name
    pub name: String,
    /// Provider to use
    pub provider: ClusterProvider,
    /// Target nodes (SSH accessible)
    pub nodes: Vec<ProvisionNode>,
    /// Kubernetes version to install
    pub version: Option<String>,
    /// Additional configuration
    #[serde(default)]
    pub config: ProvisionConfig,
}

/// Node for cluster provisioning
#[derive(Debug, Clone, Deserialize)]
pub struct ProvisionNode {
    /// Node hostname or IP
    pub address: String,
    /// SSH user
    pub user: String,
    /// SSH private key (optional, uses agent if not provided)
    pub ssh_key: Option<String>,
    /// SSH port (default: 22)
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    /// Node role
    pub role: NodeRole,
}

fn default_ssh_port() -> u16 {
    22
}

/// Node role in the cluster
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeRole {
    /// Control plane node
    ControlPlane,
    /// Worker node
    Worker,
}

/// Additional provisioning configuration
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProvisionConfig {
    /// Pod network CIDR
    pub pod_cidr: Option<String>,
    /// Service network CIDR
    pub service_cidr: Option<String>,
    /// CNI plugin to install
    pub cni: Option<String>,
    /// Enable high availability control plane
    #[serde(default)]
    pub ha: bool,
}

/// Cluster health information
#[derive(Debug, Clone, Serialize)]
pub struct ClusterHealth {
    /// Overall health status
    pub status: HealthStatus,
    /// API server reachable
    pub api_server_healthy: bool,
    /// Controller manager healthy
    pub controller_manager_healthy: bool,
    /// Scheduler healthy
    pub scheduler_healthy: bool,
    /// etcd healthy
    pub etcd_healthy: bool,
    /// Node health summary
    pub nodes: NodeHealthSummary,
    /// Component statuses
    pub components: Vec<ComponentHealth>,
}

/// Health status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
    Unknown,
}

/// Node health summary
#[derive(Debug, Clone, Serialize)]
pub struct NodeHealthSummary {
    pub total: u32,
    pub ready: u32,
    pub not_ready: u32,
}

/// Component health information
#[derive(Debug, Clone, Serialize)]
pub struct ComponentHealth {
    pub name: String,
    pub status: HealthStatus,
    pub message: Option<String>,
}

/// Kubernetes version information
#[derive(Debug, Clone, Serialize)]
pub struct K8sVersion {
    /// Server version
    pub server: String,
    /// Git version
    pub git_version: String,
    /// Git commit
    pub git_commit: String,
    /// Build date
    pub build_date: String,
    /// Platform
    pub platform: String,
}

/// Simplified pod information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PodInfo {
    pub name: String,
    pub namespace: String,
    pub status: PodStatus,
    pub node_name: Option<String>,
    pub pod_ip: Option<String>,
    pub host_ip: Option<String>,
    pub containers: Vec<ContainerInfo>,
    pub labels: BTreeMap<String, String>,
    pub annotations: BTreeMap<String, String>,
    pub created_at: Option<String>,
    pub restart_count: i32,
}

/// Pod status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PodStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Unknown,
}

impl Default for PodStatus {
    fn default() -> Self {
        Self::Unknown
    }
}

/// Container information within a pod
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerInfo {
    pub name: String,
    pub image: String,
    pub ready: bool,
    pub restart_count: i32,
    pub state: ContainerState,
}

/// Container state
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "type")]
pub enum ContainerState {
    Waiting { reason: Option<String> },
    Running { started_at: Option<String> },
    Terminated { exit_code: i32, reason: Option<String> },
    Unknown,
}

impl Default for ContainerState {
    fn default() -> Self {
        Self::Unknown
    }
}

/// Simplified deployment information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentInfo {
    pub name: String,
    pub namespace: String,
    pub replicas: i32,
    pub ready_replicas: i32,
    pub available_replicas: i32,
    pub updated_replicas: i32,
    pub labels: BTreeMap<String, String>,
    pub selector: BTreeMap<String, String>,
    pub strategy: String,
    pub created_at: Option<String>,
}

/// Scale request
#[derive(Debug, Clone, Deserialize)]
pub struct ScaleRequest {
    pub replicas: i32,
}

/// Simplified service information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceInfo {
    pub name: String,
    pub namespace: String,
    pub service_type: ServiceType,
    pub cluster_ip: Option<String>,
    pub external_ip: Option<String>,
    pub ports: Vec<ServicePort>,
    pub selector: BTreeMap<String, String>,
    pub labels: BTreeMap<String, String>,
    pub created_at: Option<String>,
}

/// Service type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServiceType {
    ClusterIP,
    NodePort,
    LoadBalancer,
    ExternalName,
}

impl Default for ServiceType {
    fn default() -> Self {
        Self::ClusterIP
    }
}

/// Service port definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServicePort {
    pub name: Option<String>,
    pub protocol: String,
    pub port: i32,
    pub target_port: String,
    pub node_port: Option<i32>,
}

/// Simplified node information
#[derive(Debug, Clone, Serialize)]
pub struct NodeInfo {
    pub name: String,
    pub status: NodeStatus,
    pub roles: Vec<String>,
    pub internal_ip: Option<String>,
    pub external_ip: Option<String>,
    pub os_image: String,
    pub kernel_version: String,
    pub container_runtime: String,
    pub kubelet_version: String,
    pub allocatable_cpu: String,
    pub allocatable_memory: String,
    pub conditions: Vec<NodeCondition>,
    pub created_at: Option<String>,
}

/// Node status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeStatus {
    Ready,
    NotReady,
    Unknown,
}

/// Node condition
#[derive(Debug, Clone, Serialize)]
pub struct NodeCondition {
    pub condition_type: String,
    pub status: String,
    pub reason: Option<String>,
    pub message: Option<String>,
}

/// Simplified namespace information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamespaceInfo {
    pub name: String,
    pub status: String,
    pub labels: BTreeMap<String, String>,
    pub annotations: BTreeMap<String, String>,
    pub created_at: Option<String>,
}

/// Create namespace request
#[derive(Debug, Clone, Deserialize)]
pub struct CreateNamespaceRequest {
    pub name: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub annotations: BTreeMap<String, String>,
}

/// Pod log request parameters
#[derive(Debug, Clone, Deserialize)]
pub struct PodLogParams {
    /// Container name (required if pod has multiple containers)
    pub container: Option<String>,
    /// Follow log stream
    #[serde(default)]
    pub follow: bool,
    /// Number of lines from the end to show
    pub tail_lines: Option<i64>,
    /// Show timestamps
    #[serde(default)]
    pub timestamps: bool,
    /// Return logs since this time (RFC3339)
    pub since_time: Option<String>,
    /// Return logs newer than this duration (e.g., "1h", "5m")
    pub since_seconds: Option<i64>,
    /// Limit bytes returned
    pub limit_bytes: Option<i64>,
}

/// Pod exec request
#[derive(Debug, Clone, Deserialize)]
pub struct PodExecRequest {
    /// Container to exec into
    pub container: Option<String>,
    /// Command to execute
    pub command: Vec<String>,
    /// Allocate TTY
    #[serde(default)]
    pub tty: bool,
    /// Attach stdin
    #[serde(default)]
    pub stdin: bool,
}

/// Helm release information
#[derive(Debug, Clone, Serialize)]
pub struct HelmRelease {
    pub name: String,
    pub namespace: String,
    pub chart: String,
    pub chart_version: String,
    pub app_version: Option<String>,
    pub status: String,
    pub revision: i32,
    pub updated: String,
}

/// Helm install request
#[derive(Debug, Clone, Deserialize)]
pub struct HelmInstallRequest {
    /// Release name
    pub name: String,
    /// Target namespace
    pub namespace: String,
    /// Chart reference (repo/chart or URL)
    pub chart: String,
    /// Chart version
    pub version: Option<String>,
    /// Values to override (YAML string or JSON object)
    pub values: Option<serde_json::Value>,
    /// Create namespace if it doesn't exist
    #[serde(default)]
    pub create_namespace: bool,
    /// Wait for resources to be ready
    #[serde(default)]
    pub wait: bool,
    /// Timeout for wait
    pub timeout: Option<String>,
}

/// Helm upgrade request
#[derive(Debug, Clone, Deserialize)]
pub struct HelmUpgradeRequest {
    /// Chart reference
    pub chart: String,
    /// Chart version
    pub version: Option<String>,
    /// Values to override
    pub values: Option<serde_json::Value>,
    /// Wait for resources to be ready
    #[serde(default)]
    pub wait: bool,
    /// Timeout for wait
    pub timeout: Option<String>,
    /// Reset values to chart defaults
    #[serde(default)]
    pub reset_values: bool,
    /// Reuse existing values
    #[serde(default)]
    pub reuse_values: bool,
}

/// Helm repository information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelmRepo {
    pub name: String,
    pub url: String,
}

/// Kubernetes event
#[derive(Debug, Clone, Serialize)]
pub struct K8sEvent {
    pub namespace: String,
    pub name: String,
    pub event_type: String,
    pub reason: String,
    pub message: String,
    pub involved_object: InvolvedObject,
    pub first_timestamp: Option<String>,
    pub last_timestamp: Option<String>,
    pub count: i32,
}

/// Object involved in an event
#[derive(Debug, Clone, Serialize)]
pub struct InvolvedObject {
    pub kind: String,
    pub name: String,
    pub namespace: Option<String>,
}

/// Event filter for listing events
#[derive(Debug, Clone, Default, Deserialize)]
pub struct EventFilter {
    /// Filter by involved object kind (e.g., "Pod", "Deployment")
    pub involved_kind: Option<String>,
    /// Filter by involved object name
    pub involved_name: Option<String>,
    /// Filter by event type (e.g., "Normal", "Warning")
    pub event_type: Option<String>,
    /// Filter by reason
    pub reason: Option<String>,
    /// Limit number of results
    pub limit: Option<i32>,
}

/// Resource metrics from metrics-server
#[derive(Debug, Clone, Serialize)]
pub struct PodMetrics {
    pub name: String,
    pub namespace: String,
    pub containers: Vec<ContainerMetrics>,
    pub timestamp: String,
}

/// Container-level metrics
#[derive(Debug, Clone, Serialize)]
pub struct ContainerMetrics {
    pub name: String,
    pub cpu_usage: String,
    pub memory_usage: String,
}

/// Node metrics from metrics-server
#[derive(Debug, Clone, Serialize)]
pub struct NodeMetrics {
    pub name: String,
    pub cpu_usage: String,
    pub memory_usage: String,
    pub timestamp: String,
}

/// Simplified StatefulSet information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatefulSetInfo {
    pub name: String,
    pub namespace: String,
    pub replicas: i32,
    pub ready_replicas: i32,
    pub current_replicas: i32,
    pub updated_replicas: i32,
    pub labels: BTreeMap<String, String>,
    pub selector: BTreeMap<String, String>,
    pub service_name: String,
    pub pod_management_policy: String,
    pub update_strategy: String,
    pub created_at: Option<String>,
}

/// Simplified DaemonSet information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonSetInfo {
    pub name: String,
    pub namespace: String,
    pub desired_number_scheduled: i32,
    pub current_number_scheduled: i32,
    pub number_ready: i32,
    pub number_available: i32,
    pub number_misscheduled: i32,
    pub labels: BTreeMap<String, String>,
    pub selector: BTreeMap<String, String>,
    pub update_strategy: String,
    pub created_at: Option<String>,
}

/// Simplified Job information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobInfo {
    pub name: String,
    pub namespace: String,
    pub status: JobStatus,
    pub completions: Option<i32>,
    pub succeeded: i32,
    pub failed: i32,
    pub active: i32,
    pub parallelism: Option<i32>,
    pub backoff_limit: Option<i32>,
    pub labels: BTreeMap<String, String>,
    pub start_time: Option<String>,
    pub completion_time: Option<String>,
    pub created_at: Option<String>,
}

/// Job status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Pending,
    Running,
    Complete,
    Failed,
    Suspended,
    Unknown,
}

impl Default for JobStatus {
    fn default() -> Self {
        Self::Unknown
    }
}

/// Simplified CronJob information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJobInfo {
    pub name: String,
    pub namespace: String,
    pub schedule: String,
    pub suspend: bool,
    pub concurrency_policy: String,
    pub successful_jobs_history_limit: Option<i32>,
    pub failed_jobs_history_limit: Option<i32>,
    pub active_jobs: i32,
    pub last_schedule_time: Option<String>,
    pub last_successful_time: Option<String>,
    pub labels: BTreeMap<String, String>,
    pub created_at: Option<String>,
}

/// Create Job request
#[derive(Debug, Clone, Deserialize)]
pub struct CreateJobRequest {
    pub name: String,
    pub namespace: String,
    pub image: String,
    pub command: Option<Vec<String>>,
    pub args: Option<Vec<String>>,
    pub completions: Option<i32>,
    pub parallelism: Option<i32>,
    pub backoff_limit: Option<i32>,
    pub active_deadline_seconds: Option<i64>,
    pub ttl_seconds_after_finished: Option<i32>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub restart_policy: Option<String>,
}

/// Create CronJob request
#[derive(Debug, Clone, Deserialize)]
pub struct CreateCronJobRequest {
    pub name: String,
    pub namespace: String,
    pub schedule: String,
    pub image: String,
    pub command: Option<Vec<String>>,
    pub args: Option<Vec<String>>,
    #[serde(default)]
    pub suspend: bool,
    pub concurrency_policy: Option<String>,
    pub successful_jobs_history_limit: Option<i32>,
    pub failed_jobs_history_limit: Option<i32>,
    pub starting_deadline_seconds: Option<i64>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub restart_policy: Option<String>,
}

// ============================================================================
// Networking Types
// ============================================================================

/// Create Service request
#[derive(Debug, Clone, Deserialize)]
pub struct CreateServiceRequest {
    pub name: String,
    pub namespace: String,
    pub service_type: Option<String>,
    pub ports: Vec<ServicePortSpec>,
    #[serde(default)]
    pub selector: BTreeMap<String, String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    pub cluster_ip: Option<String>,
    pub external_ips: Option<Vec<String>>,
    pub load_balancer_ip: Option<String>,
    pub session_affinity: Option<String>,
}

/// Service port specification for creation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServicePortSpec {
    pub name: Option<String>,
    pub protocol: Option<String>,
    pub port: i32,
    pub target_port: Option<String>,
    pub node_port: Option<i32>,
}

/// Ingress information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngressInfo {
    pub name: String,
    pub namespace: String,
    pub ingress_class: Option<String>,
    pub rules: Vec<IngressRule>,
    pub tls: Vec<IngressTls>,
    pub labels: BTreeMap<String, String>,
    pub annotations: BTreeMap<String, String>,
    pub load_balancer_ips: Vec<String>,
    pub created_at: Option<String>,
}

/// Ingress rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngressRule {
    pub host: Option<String>,
    pub paths: Vec<IngressPath>,
}

/// Ingress path
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngressPath {
    pub path: String,
    pub path_type: String,
    pub backend_service: String,
    pub backend_port: String,
}

/// Ingress TLS configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngressTls {
    pub hosts: Vec<String>,
    pub secret_name: Option<String>,
}

/// Create Ingress request
#[derive(Debug, Clone, Deserialize)]
pub struct CreateIngressRequest {
    pub name: String,
    pub namespace: String,
    pub ingress_class: Option<String>,
    pub rules: Vec<CreateIngressRule>,
    pub tls: Option<Vec<CreateIngressTls>>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub annotations: BTreeMap<String, String>,
}

/// Create Ingress rule
#[derive(Debug, Clone, Deserialize)]
pub struct CreateIngressRule {
    pub host: Option<String>,
    pub paths: Vec<CreateIngressPath>,
}

/// Create Ingress path
#[derive(Debug, Clone, Deserialize)]
pub struct CreateIngressPath {
    pub path: String,
    pub path_type: Option<String>,
    pub service_name: String,
    pub service_port: i32,
}

/// Create Ingress TLS
#[derive(Debug, Clone, Deserialize)]
pub struct CreateIngressTls {
    pub hosts: Vec<String>,
    pub secret_name: Option<String>,
}

// ============================================================================
// ConfigMap and Secret Types
// ============================================================================

/// ConfigMap information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigMapInfo {
    pub name: String,
    pub namespace: String,
    pub data: BTreeMap<String, String>,
    pub binary_data_keys: Vec<String>,
    pub labels: BTreeMap<String, String>,
    pub annotations: BTreeMap<String, String>,
    pub created_at: Option<String>,
}

/// Create ConfigMap request
#[derive(Debug, Clone, Deserialize)]
pub struct CreateConfigMapRequest {
    pub name: String,
    pub namespace: String,
    #[serde(default)]
    pub data: BTreeMap<String, String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub annotations: BTreeMap<String, String>,
}

/// Update ConfigMap request
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateConfigMapRequest {
    #[serde(default)]
    pub data: BTreeMap<String, String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub annotations: BTreeMap<String, String>,
}

/// Secret information (data values are not exposed)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretInfo {
    pub name: String,
    pub namespace: String,
    pub secret_type: String,
    pub data_keys: Vec<String>,
    pub labels: BTreeMap<String, String>,
    pub annotations: BTreeMap<String, String>,
    pub created_at: Option<String>,
}

/// Create Secret request
#[derive(Debug, Clone, Deserialize)]
pub struct CreateSecretRequest {
    pub name: String,
    pub namespace: String,
    pub secret_type: Option<String>,
    #[serde(default)]
    pub data: BTreeMap<String, String>,
    #[serde(default)]
    pub string_data: BTreeMap<String, String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub annotations: BTreeMap<String, String>,
}

// ============================================================================
// NetworkPolicy Types
// ============================================================================

/// NetworkPolicy information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkPolicyInfo {
    pub name: String,
    pub namespace: String,
    pub pod_selector: BTreeMap<String, String>,
    pub policy_types: Vec<String>,
    pub ingress_rules_count: usize,
    pub egress_rules_count: usize,
    pub labels: BTreeMap<String, String>,
    pub created_at: Option<String>,
}

/// Create NetworkPolicy request
#[derive(Debug, Clone, Deserialize)]
pub struct CreateNetworkPolicyRequest {
    pub name: String,
    pub namespace: String,
    #[serde(default)]
    pub pod_selector: BTreeMap<String, String>,
    pub policy_types: Option<Vec<String>>,
    pub ingress: Option<Vec<NetworkPolicyIngressRule>>,
    pub egress: Option<Vec<NetworkPolicyEgressRule>>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

/// NetworkPolicy ingress rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkPolicyIngressRule {
    pub from: Option<Vec<NetworkPolicyPeer>>,
    pub ports: Option<Vec<NetworkPolicyPort>>,
}

/// NetworkPolicy egress rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkPolicyEgressRule {
    pub to: Option<Vec<NetworkPolicyPeer>>,
    pub ports: Option<Vec<NetworkPolicyPort>>,
}

/// NetworkPolicy peer (source/destination)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkPolicyPeer {
    pub pod_selector: Option<BTreeMap<String, String>>,
    pub namespace_selector: Option<BTreeMap<String, String>>,
    pub ip_block: Option<NetworkPolicyIpBlock>,
}

/// NetworkPolicy IP block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkPolicyIpBlock {
    pub cidr: String,
    pub except: Option<Vec<String>>,
}

/// NetworkPolicy port
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkPolicyPort {
    pub protocol: Option<String>,
    pub port: Option<i32>,
    pub end_port: Option<i32>,
}

// ============================================================================
// Storage Types (PVC, PV, StorageClass)
// ============================================================================

/// PersistentVolumeClaim information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PvcInfo {
    pub name: String,
    pub namespace: String,
    pub status: String,
    pub volume_name: Option<String>,
    pub storage_class: Option<String>,
    pub access_modes: Vec<String>,
    pub capacity: Option<String>,
    pub requested_capacity: Option<String>,
    pub labels: BTreeMap<String, String>,
    pub created_at: Option<String>,
}

/// Create PVC request
#[derive(Debug, Clone, Deserialize)]
pub struct CreatePvcRequest {
    pub name: String,
    pub namespace: String,
    pub storage_class: Option<String>,
    pub access_modes: Vec<String>,
    pub storage: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    pub volume_mode: Option<String>,
    pub selector: Option<BTreeMap<String, String>>,
}

/// PersistentVolume information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PvInfo {
    pub name: String,
    pub status: String,
    pub capacity: String,
    pub access_modes: Vec<String>,
    pub reclaim_policy: String,
    pub storage_class: Option<String>,
    pub volume_mode: Option<String>,
    pub claim_ref: Option<PvClaimRef>,
    pub labels: BTreeMap<String, String>,
    pub created_at: Option<String>,
}

/// PV claim reference
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PvClaimRef {
    pub name: String,
    pub namespace: String,
}

/// Create PV request
#[derive(Debug, Clone, Deserialize)]
pub struct CreatePvRequest {
    pub name: String,
    pub capacity: String,
    pub access_modes: Vec<String>,
    pub reclaim_policy: Option<String>,
    pub storage_class: Option<String>,
    pub volume_mode: Option<String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    pub host_path: Option<String>,
    pub nfs: Option<NfsVolumeSource>,
}

/// NFS volume source
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NfsVolumeSource {
    pub server: String,
    pub path: String,
    #[serde(default)]
    pub read_only: bool,
}

/// StorageClass information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageClassInfo {
    pub name: String,
    pub provisioner: String,
    pub reclaim_policy: Option<String>,
    pub volume_binding_mode: Option<String>,
    pub allow_volume_expansion: bool,
    pub parameters: BTreeMap<String, String>,
    pub labels: BTreeMap<String, String>,
    pub is_default: bool,
    pub created_at: Option<String>,
}

/// Create StorageClass request
#[derive(Debug, Clone, Deserialize)]
pub struct CreateStorageClassRequest {
    pub name: String,
    pub provisioner: String,
    pub reclaim_policy: Option<String>,
    pub volume_binding_mode: Option<String>,
    #[serde(default)]
    pub allow_volume_expansion: bool,
    #[serde(default)]
    pub parameters: BTreeMap<String, String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub is_default: bool,
}

// ============================================================================
// ResourceQuota and LimitRange Types
// ============================================================================

/// ResourceQuota information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceQuotaInfo {
    pub name: String,
    pub namespace: String,
    pub hard: BTreeMap<String, String>,
    pub used: BTreeMap<String, String>,
    pub labels: BTreeMap<String, String>,
    pub created_at: Option<String>,
}

/// Create ResourceQuota request
#[derive(Debug, Clone, Deserialize)]
pub struct CreateResourceQuotaRequest {
    pub name: String,
    pub namespace: String,
    pub hard: BTreeMap<String, String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

/// LimitRange information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitRangeInfo {
    pub name: String,
    pub namespace: String,
    pub limits: Vec<LimitRangeItem>,
    pub labels: BTreeMap<String, String>,
    pub created_at: Option<String>,
}

/// LimitRange item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitRangeItem {
    pub limit_type: String,
    pub default: Option<BTreeMap<String, String>>,
    pub default_request: Option<BTreeMap<String, String>>,
    pub max: Option<BTreeMap<String, String>>,
    pub min: Option<BTreeMap<String, String>>,
}

/// Create LimitRange request
#[derive(Debug, Clone, Deserialize)]
pub struct CreateLimitRangeRequest {
    pub name: String,
    pub namespace: String,
    pub limits: Vec<CreateLimitRangeItem>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

/// Create LimitRange item
#[derive(Debug, Clone, Deserialize)]
pub struct CreateLimitRangeItem {
    pub limit_type: String,
    pub default: Option<BTreeMap<String, String>>,
    pub default_request: Option<BTreeMap<String, String>>,
    pub max: Option<BTreeMap<String, String>>,
    pub min: Option<BTreeMap<String, String>>,
}

// ============================================================================
// RBAC Types
// ============================================================================

/// ServiceAccount information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceAccountInfo {
    pub name: String,
    pub namespace: String,
    pub secrets: Vec<String>,
    pub image_pull_secrets: Vec<String>,
    pub labels: BTreeMap<String, String>,
    pub created_at: Option<String>,
}

/// Create ServiceAccount request
#[derive(Debug, Clone, Deserialize)]
pub struct CreateServiceAccountRequest {
    pub name: String,
    pub namespace: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub annotations: BTreeMap<String, String>,
}

/// Role information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleInfo {
    pub name: String,
    pub namespace: String,
    pub rules: Vec<PolicyRule>,
    pub labels: BTreeMap<String, String>,
    pub created_at: Option<String>,
}

/// ClusterRole information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterRoleInfo {
    pub name: String,
    pub rules: Vec<PolicyRule>,
    pub labels: BTreeMap<String, String>,
    pub created_at: Option<String>,
}

/// Policy rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRule {
    pub api_groups: Vec<String>,
    pub resources: Vec<String>,
    pub verbs: Vec<String>,
    pub resource_names: Option<Vec<String>>,
}

/// Create Role request
#[derive(Debug, Clone, Deserialize)]
pub struct CreateRoleRequest {
    pub name: String,
    pub namespace: String,
    pub rules: Vec<PolicyRule>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

/// Create ClusterRole request
#[derive(Debug, Clone, Deserialize)]
pub struct CreateClusterRoleRequest {
    pub name: String,
    pub rules: Vec<PolicyRule>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

/// RoleBinding information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleBindingInfo {
    pub name: String,
    pub namespace: String,
    pub role_ref: RoleRef,
    pub subjects: Vec<Subject>,
    pub labels: BTreeMap<String, String>,
    pub created_at: Option<String>,
}

/// ClusterRoleBinding information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterRoleBindingInfo {
    pub name: String,
    pub role_ref: RoleRef,
    pub subjects: Vec<Subject>,
    pub labels: BTreeMap<String, String>,
    pub created_at: Option<String>,
}

/// Role reference
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleRef {
    pub api_group: String,
    pub kind: String,
    pub name: String,
}

/// Subject (user, group, or service account)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subject {
    pub kind: String,
    pub name: String,
    pub namespace: Option<String>,
    pub api_group: Option<String>,
}

/// Create RoleBinding request
#[derive(Debug, Clone, Deserialize)]
pub struct CreateRoleBindingRequest {
    pub name: String,
    pub namespace: String,
    pub role_ref: RoleRef,
    pub subjects: Vec<Subject>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

/// Create ClusterRoleBinding request
#[derive(Debug, Clone, Deserialize)]
pub struct CreateClusterRoleBindingRequest {
    pub name: String,
    pub role_ref: RoleRef,
    pub subjects: Vec<Subject>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}
