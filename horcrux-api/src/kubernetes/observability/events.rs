//! Kubernetes events
//!
//! List and watch Kubernetes events for cluster observability.

use crate::kubernetes::client::K8sClient;
#[cfg(not(feature = "kubernetes"))]
use crate::kubernetes::error::K8sError;
use crate::kubernetes::error::K8sResult;
use crate::kubernetes::types::{EventFilter, InvolvedObject, K8sEvent};

/// List events in a namespace
#[cfg(feature = "kubernetes")]
pub async fn list_events(
    client: &K8sClient,
    namespace: Option<&str>,
) -> K8sResult<Vec<K8sEvent>> {
    use k8s_openapi::api::core::v1::Event;
    use kube::api::{Api, ListParams};

    let events: Api<Event> = if let Some(ns) = namespace {
        Api::namespaced(client.inner().clone(), ns)
    } else {
        Api::all(client.inner().clone())
    };

    let list = events.list(&ListParams::default()).await?;

    Ok(list.items.into_iter().map(event_to_info).collect())
}

/// List events with filtering
#[cfg(feature = "kubernetes")]
pub async fn list_events_filtered(
    client: &K8sClient,
    namespace: Option<&str>,
    filter: &EventFilter,
) -> K8sResult<Vec<K8sEvent>> {
    use k8s_openapi::api::core::v1::Event;
    use kube::api::{Api, ListParams};

    let events: Api<Event> = if let Some(ns) = namespace {
        Api::namespaced(client.inner().clone(), ns)
    } else {
        Api::all(client.inner().clone())
    };

    // Build field selector for involved object
    let mut field_selectors = Vec::new();

    if let Some(kind) = &filter.involved_kind {
        field_selectors.push(format!("involvedObject.kind={}", kind));
    }

    if let Some(name) = &filter.involved_name {
        field_selectors.push(format!("involvedObject.name={}", name));
    }

    if let Some(event_type) = &filter.event_type {
        field_selectors.push(format!("type={}", event_type));
    }

    if let Some(reason) = &filter.reason {
        field_selectors.push(format!("reason={}", reason));
    }

    let list_params = if field_selectors.is_empty() {
        ListParams::default()
    } else {
        ListParams::default().fields(&field_selectors.join(","))
    };

    let list = events.list(&list_params).await?;

    let mut results: Vec<K8sEvent> = list.items.into_iter().map(event_to_info).collect();

    // Sort by last timestamp (most recent first)
    results.sort_by(|a, b| {
        let a_time = a.last_timestamp.as_deref().unwrap_or("");
        let b_time = b.last_timestamp.as_deref().unwrap_or("");
        b_time.cmp(a_time)
    });

    // Apply limit
    if let Some(limit) = filter.limit {
        results.truncate(limit as usize);
    }

    Ok(results)
}

/// List events for a specific resource
#[cfg(feature = "kubernetes")]
pub async fn list_events_for_resource(
    client: &K8sClient,
    namespace: &str,
    kind: &str,
    name: &str,
) -> K8sResult<Vec<K8sEvent>> {
    let filter = EventFilter {
        involved_kind: Some(kind.to_string()),
        involved_name: Some(name.to_string()),
        event_type: None,
        reason: None,
        limit: None,
    };

    list_events_filtered(client, Some(namespace), &filter).await
}

/// Watch events and return a stream
#[cfg(feature = "kubernetes")]
pub async fn watch_events(
    client: &K8sClient,
    namespace: Option<&str>,
) -> K8sResult<impl futures::Stream<Item = Result<kube::runtime::watcher::Event<k8s_openapi::api::core::v1::Event>, kube::runtime::watcher::Error>>>
{
    use k8s_openapi::api::core::v1::Event;
    use kube::api::Api;
    use kube::runtime::watcher;
    use kube::runtime::watcher::Config;

    let events: Api<Event> = if let Some(ns) = namespace {
        Api::namespaced(client.inner().clone(), ns)
    } else {
        Api::all(client.inner().clone())
    };

    let watcher_config = Config::default();
    let stream = watcher(events, watcher_config);

    Ok(stream)
}

/// Get warning events only
#[cfg(feature = "kubernetes")]
pub async fn list_warning_events(
    client: &K8sClient,
    namespace: Option<&str>,
    limit: Option<i32>,
) -> K8sResult<Vec<K8sEvent>> {
    let filter = EventFilter {
        involved_kind: None,
        involved_name: None,
        event_type: Some("Warning".to_string()),
        reason: None,
        limit,
    };

    list_events_filtered(client, namespace, &filter).await
}

/// Get recent events (within last N minutes)
#[cfg(feature = "kubernetes")]
pub async fn list_recent_events(
    client: &K8sClient,
    namespace: Option<&str>,
    minutes: i64,
) -> K8sResult<Vec<K8sEvent>> {
    use chrono::{Duration, Utc};

    let events = list_events(client, namespace).await?;

    let cutoff = Utc::now() - Duration::minutes(minutes);
    let cutoff_str = cutoff.to_rfc3339();

    let filtered: Vec<K8sEvent> = events
        .into_iter()
        .filter(|e| {
            e.last_timestamp
                .as_ref()
                .map(|t| t.as_str() >= cutoff_str.as_str())
                .unwrap_or(false)
        })
        .collect();

    Ok(filtered)
}

#[cfg(feature = "kubernetes")]
fn event_to_info(event: k8s_openapi::api::core::v1::Event) -> K8sEvent {
    let metadata = event.metadata;
    let involved = event.involved_object;

    K8sEvent {
        namespace: metadata.namespace.unwrap_or_default(),
        name: metadata.name.unwrap_or_default(),
        event_type: event.type_.unwrap_or_default(),
        reason: event.reason.unwrap_or_default(),
        message: event.message.unwrap_or_default(),
        involved_object: InvolvedObject {
            kind: involved.kind.unwrap_or_default(),
            name: involved.name.unwrap_or_default(),
            namespace: involved.namespace,
        },
        first_timestamp: event.first_timestamp.map(|t| t.0.to_rfc3339()),
        last_timestamp: event.last_timestamp.map(|t| t.0.to_rfc3339()),
        count: event.count.unwrap_or(1),
    }
}

// ============================================================================
// Stubs for when kubernetes feature is disabled
// ============================================================================

#[cfg(not(feature = "kubernetes"))]
pub async fn list_events(
    _client: &K8sClient,
    _namespace: Option<&str>,
) -> K8sResult<Vec<K8sEvent>> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn list_events_filtered(
    _client: &K8sClient,
    _namespace: Option<&str>,
    _filter: &EventFilter,
) -> K8sResult<Vec<K8sEvent>> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn list_events_for_resource(
    _client: &K8sClient,
    _namespace: &str,
    _kind: &str,
    _name: &str,
) -> K8sResult<Vec<K8sEvent>> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn list_warning_events(
    _client: &K8sClient,
    _namespace: Option<&str>,
    _limit: Option<i32>,
) -> K8sResult<Vec<K8sEvent>> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn list_recent_events(
    _client: &K8sClient,
    _namespace: Option<&str>,
    _minutes: i64,
) -> K8sResult<Vec<K8sEvent>> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}
