//! Lane M: Helm (Kubernetes), Monitoring > Notifications channels and
//! Monitoring > Metrics Explorer backends for pages the web UI already
//! routes.

use crate::AppState;
use axum::Router;
use std::sync::Arc;

/// Build the lane's routes and start its background work (metrics sampler).
pub async fn routes(state: &Arc<AppState>) -> anyhow::Result<Router<Arc<AppState>>> {
    let n = &state.config.notifications;
    let policy = crate::notification_channels::DeliveryPolicy {
        attempts: n.attempts.max(1),
        backoff_ms: n.backoff_ms,
        backoff_factor: n.backoff_factor.max(1),
        timeout_secs: n.timeout_secs.max(1),
    };
    let channels = Arc::new(
        crate::notification_channels::NotificationChannelStore::load_with_policy(
            state.database.clone(),
            policy,
        )
        .await
        .map_err(|e| anyhow::anyhow!("loading notification channels: {:?}", e))?,
    );
    // A notification that fails every attempt is an audit-log entry.
    {
        let logger = state.audit_logger.clone();
        channels.attach_audit(move |f| {
            let logger = logger.clone();
            tokio::spawn(async move {
                logger
                    .log(crate::audit::AuditEvent {
                        timestamp: chrono::Utc::now(),
                        event_type: crate::audit::AuditEventType::NotificationDeliveryFailed,
                        severity: crate::audit::AuditSeverity::Warning,
                        user: Some("system".into()),
                        source_ip: None,
                        resource: Some(format!("notification-channel:{}", f.channel_id)),
                        action: format!(
                            "notification '{}' via {} channel '{}' not delivered after {} attempts",
                            f.title, f.channel_type, f.channel_name, f.attempts
                        ),
                        result: crate::audit::AuditResult::Failure,
                        details: Some(
                            serde_json::json!({
                                "channel_id": f.channel_id, "channel_name": f.channel_name,
                                "channel_type": f.channel_type, "event": f.event,
                                "severity": f.severity, "attempts": f.attempts,
                                "error": f.error,
                            })
                            .to_string(),
                        ),
                        session_id: None,
                    })
                    .await;
            });
        });
    }
    // Event webhooks: same backoff base, and a delivery that fails every
    // attempt is an audit-log entry too.
    state.webhook_manager.set_backoff_ms(n.backoff_ms);
    {
        let logger = state.audit_logger.clone();
        state.webhook_manager.attach_audit(move |f| {
            let logger = logger.clone();
            tokio::spawn(async move {
                logger
                    .log(crate::audit::AuditEvent {
                        timestamp: chrono::Utc::now(),
                        event_type: crate::audit::AuditEventType::NotificationDeliveryFailed,
                        severity: crate::audit::AuditSeverity::Warning,
                        user: Some("system".into()),
                        source_ip: None,
                        resource: Some(format!("webhook:{}", f.webhook_id)),
                        action: format!(
                            "webhook '{}' event {} not delivered after {} attempts",
                            f.webhook_name, f.event_type, f.attempts
                        ),
                        result: crate::audit::AuditResult::Failure,
                        details: Some(
                            serde_json::json!({
                                "webhook_id": f.webhook_id, "webhook_name": f.webhook_name,
                                "channel_type": "event-webhook", "event": f.event_type,
                                "attempts": f.attempts, "error": f.error,
                            })
                            .to_string(),
                        ),
                        session_id: None,
                    })
                    .await;
            });
        });
    }
    state
        .alert_manager
        .attach_webhooks(state.webhook_manager.clone());

    let metrics = Arc::new(crate::metrics_explorer::MetricsStore::new());
    crate::metrics_explorer::start_sampler(
        metrics.clone(),
        state.monitoring_manager.clone(),
        state.config.paths.data_dir.clone(),
    );

    let router = Router::new()
        .merge(super::notifications::routes(channels))
        .merge(super::metrics::routes(metrics))
        // Monitoring > Alert Center: rule edit/toggle/test + rule evaluation.
        .merge(super::alert_rules::routes());
    super::alert_rules::start_evaluator(state.clone());
    #[cfg(feature = "kubernetes")]
    let router = router
        .merge(super::helm::routes(state))
        .merge(super::k8s_config::routes());
    Ok(router)
}
