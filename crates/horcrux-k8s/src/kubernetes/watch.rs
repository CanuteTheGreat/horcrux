//! Kubernetes resource watching
//!
//! Watch Kubernetes resources for real-time updates via WebSocket.

use crate::kubernetes::client::K8sClient;
#[cfg(not(feature = "kubernetes"))]
use crate::kubernetes::error::K8sError;
use crate::kubernetes::error::K8sResult;
use serde::{Deserialize, Serialize};

/// Resource type to watch
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WatchResourceType {
    Pods,
    Deployments,
    StatefulSets,
    DaemonSets,
    Services,
    Ingresses,
    ConfigMaps,
    Secrets,
    Nodes,
    Events,
    Jobs,
    CronJobs,
    Namespaces,
    PersistentVolumeClaims,
    PersistentVolumes,
}

/// Watch event type
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum WatchEventType {
    Added,
    Modified,
    Deleted,
    Bookmark,
    Error,
}

/// Generic watch event
#[derive(Debug, Clone, Serialize)]
pub struct WatchEvent {
    pub event_type: WatchEventType,
    pub resource_type: String,
    pub namespace: Option<String>,
    pub name: String,
    pub resource: serde_json::Value,
}

/// Watch configuration
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WatchConfig {
    /// Resource types to watch
    pub resource_types: Vec<WatchResourceType>,
    /// Namespace to watch (None for cluster-wide)
    pub namespace: Option<String>,
    /// Label selector
    pub label_selector: Option<String>,
    /// Field selector
    pub field_selector: Option<String>,
}

/// Watch Pods and return a stream of events
#[cfg(feature = "kubernetes")]
pub async fn watch_pods(
    client: &K8sClient,
    namespace: Option<&str>,
    label_selector: Option<&str>,
) -> K8sResult<
    impl futures::Stream<
        Item = Result<
            kube::runtime::watcher::Event<k8s_openapi::api::core::v1::Pod>,
            kube::runtime::watcher::Error,
        >,
    >,
> {
    use k8s_openapi::api::core::v1::Pod;
    use kube::api::Api;
    use kube::runtime::watcher;
    use kube::runtime::watcher::Config;

    let pods: Api<Pod> = if let Some(ns) = namespace {
        Api::namespaced(client.inner().clone(), ns)
    } else {
        Api::all(client.inner().clone())
    };

    let mut config = Config::default();
    if let Some(selector) = label_selector {
        config = config.labels(selector);
    }

    Ok(watcher(pods, config))
}

/// Watch Deployments and return a stream of events
#[cfg(feature = "kubernetes")]
pub async fn watch_deployments(
    client: &K8sClient,
    namespace: Option<&str>,
    label_selector: Option<&str>,
) -> K8sResult<
    impl futures::Stream<
        Item = Result<
            kube::runtime::watcher::Event<k8s_openapi::api::apps::v1::Deployment>,
            kube::runtime::watcher::Error,
        >,
    >,
> {
    use k8s_openapi::api::apps::v1::Deployment;
    use kube::api::Api;
    use kube::runtime::watcher;
    use kube::runtime::watcher::Config;

    let deployments: Api<Deployment> = if let Some(ns) = namespace {
        Api::namespaced(client.inner().clone(), ns)
    } else {
        Api::all(client.inner().clone())
    };

    let mut config = Config::default();
    if let Some(selector) = label_selector {
        config = config.labels(selector);
    }

    Ok(watcher(deployments, config))
}

/// Watch Services and return a stream of events
#[cfg(feature = "kubernetes")]
pub async fn watch_services(
    client: &K8sClient,
    namespace: Option<&str>,
    label_selector: Option<&str>,
) -> K8sResult<
    impl futures::Stream<
        Item = Result<
            kube::runtime::watcher::Event<k8s_openapi::api::core::v1::Service>,
            kube::runtime::watcher::Error,
        >,
    >,
> {
    use k8s_openapi::api::core::v1::Service;
    use kube::api::Api;
    use kube::runtime::watcher;
    use kube::runtime::watcher::Config;

    let services: Api<Service> = if let Some(ns) = namespace {
        Api::namespaced(client.inner().clone(), ns)
    } else {
        Api::all(client.inner().clone())
    };

    let mut config = Config::default();
    if let Some(selector) = label_selector {
        config = config.labels(selector);
    }

    Ok(watcher(services, config))
}

/// Watch Nodes and return a stream of events
#[cfg(feature = "kubernetes")]
pub async fn watch_nodes(
    client: &K8sClient,
    label_selector: Option<&str>,
) -> K8sResult<
    impl futures::Stream<
        Item = Result<
            kube::runtime::watcher::Event<k8s_openapi::api::core::v1::Node>,
            kube::runtime::watcher::Error,
        >,
    >,
> {
    use k8s_openapi::api::core::v1::Node;
    use kube::api::Api;
    use kube::runtime::watcher;
    use kube::runtime::watcher::Config;

    let nodes: Api<Node> = Api::all(client.inner().clone());

    let mut config = Config::default();
    if let Some(selector) = label_selector {
        config = config.labels(selector);
    }

    Ok(watcher(nodes, config))
}

/// Watch Namespaces and return a stream of events
#[cfg(feature = "kubernetes")]
pub async fn watch_namespaces(
    client: &K8sClient,
    label_selector: Option<&str>,
) -> K8sResult<
    impl futures::Stream<
        Item = Result<
            kube::runtime::watcher::Event<k8s_openapi::api::core::v1::Namespace>,
            kube::runtime::watcher::Error,
        >,
    >,
> {
    use k8s_openapi::api::core::v1::Namespace;
    use kube::api::Api;
    use kube::runtime::watcher;
    use kube::runtime::watcher::Config;

    let namespaces: Api<Namespace> = Api::all(client.inner().clone());

    let mut config = Config::default();
    if let Some(selector) = label_selector {
        config = config.labels(selector);
    }

    Ok(watcher(namespaces, config))
}

/// Watch StatefulSets and return a stream of events
#[cfg(feature = "kubernetes")]
pub async fn watch_statefulsets(
    client: &K8sClient,
    namespace: Option<&str>,
    label_selector: Option<&str>,
) -> K8sResult<
    impl futures::Stream<
        Item = Result<
            kube::runtime::watcher::Event<k8s_openapi::api::apps::v1::StatefulSet>,
            kube::runtime::watcher::Error,
        >,
    >,
> {
    use k8s_openapi::api::apps::v1::StatefulSet;
    use kube::api::Api;
    use kube::runtime::watcher;
    use kube::runtime::watcher::Config;

    let statefulsets: Api<StatefulSet> = if let Some(ns) = namespace {
        Api::namespaced(client.inner().clone(), ns)
    } else {
        Api::all(client.inner().clone())
    };

    let mut config = Config::default();
    if let Some(selector) = label_selector {
        config = config.labels(selector);
    }

    Ok(watcher(statefulsets, config))
}

/// Watch DaemonSets and return a stream of events
#[cfg(feature = "kubernetes")]
pub async fn watch_daemonsets(
    client: &K8sClient,
    namespace: Option<&str>,
    label_selector: Option<&str>,
) -> K8sResult<
    impl futures::Stream<
        Item = Result<
            kube::runtime::watcher::Event<k8s_openapi::api::apps::v1::DaemonSet>,
            kube::runtime::watcher::Error,
        >,
    >,
> {
    use k8s_openapi::api::apps::v1::DaemonSet;
    use kube::api::Api;
    use kube::runtime::watcher;
    use kube::runtime::watcher::Config;

    let daemonsets: Api<DaemonSet> = if let Some(ns) = namespace {
        Api::namespaced(client.inner().clone(), ns)
    } else {
        Api::all(client.inner().clone())
    };

    let mut config = Config::default();
    if let Some(selector) = label_selector {
        config = config.labels(selector);
    }

    Ok(watcher(daemonsets, config))
}

/// Watch Jobs and return a stream of events
#[cfg(feature = "kubernetes")]
pub async fn watch_jobs(
    client: &K8sClient,
    namespace: Option<&str>,
    label_selector: Option<&str>,
) -> K8sResult<
    impl futures::Stream<
        Item = Result<
            kube::runtime::watcher::Event<k8s_openapi::api::batch::v1::Job>,
            kube::runtime::watcher::Error,
        >,
    >,
> {
    use k8s_openapi::api::batch::v1::Job;
    use kube::api::Api;
    use kube::runtime::watcher;
    use kube::runtime::watcher::Config;

    let jobs: Api<Job> = if let Some(ns) = namespace {
        Api::namespaced(client.inner().clone(), ns)
    } else {
        Api::all(client.inner().clone())
    };

    let mut config = Config::default();
    if let Some(selector) = label_selector {
        config = config.labels(selector);
    }

    Ok(watcher(jobs, config))
}

/// Watch ConfigMaps and return a stream of events
#[cfg(feature = "kubernetes")]
pub async fn watch_configmaps(
    client: &K8sClient,
    namespace: Option<&str>,
    label_selector: Option<&str>,
) -> K8sResult<
    impl futures::Stream<
        Item = Result<
            kube::runtime::watcher::Event<k8s_openapi::api::core::v1::ConfigMap>,
            kube::runtime::watcher::Error,
        >,
    >,
> {
    use k8s_openapi::api::core::v1::ConfigMap;
    use kube::api::Api;
    use kube::runtime::watcher;
    use kube::runtime::watcher::Config;

    let configmaps: Api<ConfigMap> = if let Some(ns) = namespace {
        Api::namespaced(client.inner().clone(), ns)
    } else {
        Api::all(client.inner().clone())
    };

    let mut config = Config::default();
    if let Some(selector) = label_selector {
        config = config.labels(selector);
    }

    Ok(watcher(configmaps, config))
}

/// Watch Secrets and return a stream of events
#[cfg(feature = "kubernetes")]
pub async fn watch_secrets(
    client: &K8sClient,
    namespace: Option<&str>,
    label_selector: Option<&str>,
) -> K8sResult<
    impl futures::Stream<
        Item = Result<
            kube::runtime::watcher::Event<k8s_openapi::api::core::v1::Secret>,
            kube::runtime::watcher::Error,
        >,
    >,
> {
    use k8s_openapi::api::core::v1::Secret;
    use kube::api::Api;
    use kube::runtime::watcher;
    use kube::runtime::watcher::Config;

    let secrets: Api<Secret> = if let Some(ns) = namespace {
        Api::namespaced(client.inner().clone(), ns)
    } else {
        Api::all(client.inner().clone())
    };

    let mut config = Config::default();
    if let Some(selector) = label_selector {
        config = config.labels(selector);
    }

    Ok(watcher(secrets, config))
}

/// Watch PVCs and return a stream of events
#[cfg(feature = "kubernetes")]
pub async fn watch_pvcs(
    client: &K8sClient,
    namespace: Option<&str>,
    label_selector: Option<&str>,
) -> K8sResult<
    impl futures::Stream<
        Item = Result<
            kube::runtime::watcher::Event<k8s_openapi::api::core::v1::PersistentVolumeClaim>,
            kube::runtime::watcher::Error,
        >,
    >,
> {
    use k8s_openapi::api::core::v1::PersistentVolumeClaim;
    use kube::api::Api;
    use kube::runtime::watcher;
    use kube::runtime::watcher::Config;

    let pvcs: Api<PersistentVolumeClaim> = if let Some(ns) = namespace {
        Api::namespaced(client.inner().clone(), ns)
    } else {
        Api::all(client.inner().clone())
    };

    let mut config = Config::default();
    if let Some(selector) = label_selector {
        config = config.labels(selector);
    }

    Ok(watcher(pvcs, config))
}

/// Watch PVs and return a stream of events
#[cfg(feature = "kubernetes")]
pub async fn watch_pvs(
    client: &K8sClient,
    label_selector: Option<&str>,
) -> K8sResult<
    impl futures::Stream<
        Item = Result<
            kube::runtime::watcher::Event<k8s_openapi::api::core::v1::PersistentVolume>,
            kube::runtime::watcher::Error,
        >,
    >,
> {
    use k8s_openapi::api::core::v1::PersistentVolume;
    use kube::api::Api;
    use kube::runtime::watcher;
    use kube::runtime::watcher::Config;

    let pvs: Api<PersistentVolume> = Api::all(client.inner().clone());

    let mut config = Config::default();
    if let Some(selector) = label_selector {
        config = config.labels(selector);
    }

    Ok(watcher(pvs, config))
}

// ============================================================================
// Stubs for when kubernetes feature is disabled
// ============================================================================

#[cfg(not(feature = "kubernetes"))]
pub async fn watch_pods(
    _client: &K8sClient,
    _namespace: Option<&str>,
    _label_selector: Option<&str>,
) -> K8sResult<futures::stream::Empty<Result<(), ()>>> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn watch_deployments(
    _client: &K8sClient,
    _namespace: Option<&str>,
    _label_selector: Option<&str>,
) -> K8sResult<futures::stream::Empty<Result<(), ()>>> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn watch_services(
    _client: &K8sClient,
    _namespace: Option<&str>,
    _label_selector: Option<&str>,
) -> K8sResult<futures::stream::Empty<Result<(), ()>>> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn watch_nodes(
    _client: &K8sClient,
    _label_selector: Option<&str>,
) -> K8sResult<futures::stream::Empty<Result<(), ()>>> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn watch_namespaces(
    _client: &K8sClient,
    _label_selector: Option<&str>,
) -> K8sResult<futures::stream::Empty<Result<(), ()>>> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn watch_statefulsets(
    _client: &K8sClient,
    _namespace: Option<&str>,
    _label_selector: Option<&str>,
) -> K8sResult<futures::stream::Empty<Result<(), ()>>> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn watch_daemonsets(
    _client: &K8sClient,
    _namespace: Option<&str>,
    _label_selector: Option<&str>,
) -> K8sResult<futures::stream::Empty<Result<(), ()>>> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn watch_jobs(
    _client: &K8sClient,
    _namespace: Option<&str>,
    _label_selector: Option<&str>,
) -> K8sResult<futures::stream::Empty<Result<(), ()>>> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn watch_configmaps(
    _client: &K8sClient,
    _namespace: Option<&str>,
    _label_selector: Option<&str>,
) -> K8sResult<futures::stream::Empty<Result<(), ()>>> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn watch_secrets(
    _client: &K8sClient,
    _namespace: Option<&str>,
    _label_selector: Option<&str>,
) -> K8sResult<futures::stream::Empty<Result<(), ()>>> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn watch_pvcs(
    _client: &K8sClient,
    _namespace: Option<&str>,
    _label_selector: Option<&str>,
) -> K8sResult<futures::stream::Empty<Result<(), ()>>> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn watch_pvs(
    _client: &K8sClient,
    _label_selector: Option<&str>,
) -> K8sResult<futures::stream::Empty<Result<(), ()>>> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}
