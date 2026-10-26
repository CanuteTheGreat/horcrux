//! Monitoring > Alert Center: the alert-rule routes the page needs beyond
//! list/create/delete (main.rs) - edit, enable/disable and "send a test
//! alert" - plus the evaluator that actually checks the rules against the
//! node's metrics (rules used to be stored but never evaluated, so no alert
//! ever fired).

use crate::alerts::{
    Alert, AlertCondition, AlertManager, AlertRule, AlertStatus, MetricType, Silence,
};
use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post, put},
    Extension, Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/alerts/rules/:rule_id", put(update_rule))
        .route("/api/alerts/rules/:rule_id/toggle", put(toggle_rule))
        .route("/api/alerts/rules/:rule_id/test", post(test_rule))
        .route("/api/alerts/silences", get(list_silences))
        .route(
            "/api/alerts/:alert_id/silence",
            post(silence_alert).delete(unsilence_alert),
        )
        .route("/api/alerts/preview", post(preview))
}

/// Running silences (rule + target + end), newest end last.
async fn list_silences(State(state): State<Arc<AppState>>) -> Json<Vec<Silence>> {
    Json(state.alert_manager.list_silences().await)
}

#[derive(Deserialize)]
struct SilenceRequest {
    /// How long to mute, in minutes (1 to 43200 = 30 days).
    minutes: u64,
    #[serde(default)]
    comment: String,
}

/// Mute the rule/target of an active alert: it stays listed but sends no
/// notification or webhook until the silence ends.
async fn silence_alert(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
    Path(alert_id): Path<String>,
    Json(req): Json<SilenceRequest>,
) -> Result<Json<Silence>, ApiError> {
    let by = user
        .map(|Extension(u)| u.username)
        .unwrap_or_else(|| "unknown".into());
    let secs = i64::try_from(req.minutes)
        .ok()
        .and_then(|m| m.checked_mul(60))
        .unwrap_or(i64::MAX);
    Ok(Json(
        state
            .alert_manager
            .silence_alert(&alert_id, secs, &by, &req.comment)
            .await?,
    ))
}

async fn unsilence_alert(
    State(state): State<Arc<AppState>>,
    Path(alert_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.alert_manager.unsilence_alert(&alert_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct PreviewTarget {
    target: String,
    /// Current reading of the metric.
    value: f64,
    /// Would the condition hold right now?
    holds: bool,
}

#[derive(Serialize)]
struct PreviewResult {
    /// Targets the rule's pattern matches, with their current value.
    targets: Vec<PreviewTarget>,
    /// How many of them would start firing (after `duration_seconds`).
    would_fire: usize,
    duration_seconds: u64,
    /// Enabled notification channels a firing alert would reach.
    channels: usize,
    /// Why nothing can fire, or a caveat.
    note: Option<String>,
}

/// "What would this rule do right now?" Evaluates a not-yet-saved condition
/// against the node's live readings; nothing fires and nothing is stored.
async fn preview(
    State(state): State<Arc<AppState>>,
    Json(cond): Json<AlertCondition>,
) -> Result<Json<PreviewResult>, ApiError> {
    if cond.target_pattern.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "target pattern is required (use * for every target)".into(),
        ));
    }
    if !cond.threshold.is_finite() {
        return Err(ApiError::BadRequest("Threshold must be a number".into()));
    }
    let node = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "localhost".into());
    let data_dir = state.config.paths.data_dir.clone();
    let vm_count = state.vm_manager.list_vms().await.len();
    let mut prev = None;
    let _ = node_samples(&node, &data_dir, vm_count, &mut prev);
    if cond.metric_type == MetricType::CpuUsage {
        // a CPU percentage needs two readings
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    let samples = node_samples(&node, &data_dir, vm_count, &mut prev);
    let targets: Vec<PreviewTarget> = samples
        .into_iter()
        .filter(|(m, t, _)| {
            *m == cond.metric_type && AlertManager::matches_target(&cond.target_pattern, t)
        })
        .map(|(_, target, value)| PreviewTarget {
            holds: cond.evaluate(value),
            target,
            value,
        })
        .collect();
    let note = if matches!(cond.metric_type, MetricType::DiskIo | MetricType::NetworkIo) {
        Some("Disk I/O and network I/O rules are not evaluated: this rule would never fire".into())
    } else if targets.is_empty() {
        Some(format!(
            "No target matches {:?} for this metric (node readings are named after the node, {node})",
            cond.target_pattern
        ))
    } else {
        None
    };
    let channels = match state.alert_manager.channel_store() {
        Some(s) => s.list().await.iter().filter(|c| c.enabled).count(),
        None => 0,
    };
    Ok(Json(PreviewResult {
        would_fire: targets.iter().filter(|t| t.holds).count(),
        targets,
        duration_seconds: cond.duration_seconds,
        channels,
        note,
    }))
}

async fn update_rule(
    State(state): State<Arc<AppState>>,
    Path(rule_id): Path<String>,
    Json(rule): Json<AlertRule>,
) -> Result<Json<AlertRule>, ApiError> {
    validate(&rule)?;
    Ok(Json(state.alert_manager.update_rule(&rule_id, rule).await?))
}

#[derive(Deserialize)]
struct ToggleRequest {
    enabled: bool,
}

async fn toggle_rule(
    State(state): State<Arc<AppState>>,
    Path(rule_id): Path<String>,
    Json(req): Json<ToggleRequest>,
) -> Result<Json<AlertRule>, ApiError> {
    Ok(Json(
        state
            .alert_manager
            .set_rule_enabled(&rule_id, req.enabled)
            .await?,
    ))
}

#[derive(Serialize)]
struct TestResult {
    /// Enabled notification channels the test alert was sent to
    channels: usize,
    delivered: usize,
    failed: usize,
}

/// Send a test alert for the rule to every enabled notification channel
/// (Monitoring > Notifications), without touching active alerts/history.
async fn test_rule(
    State(state): State<Arc<AppState>>,
    Path(rule_id): Path<String>,
) -> Result<Json<TestResult>, ApiError> {
    let rule = state
        .alert_manager
        .get_rule(&rule_id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Alert rule {rule_id} not found")))?;
    let alert = Alert {
        id: uuid::Uuid::new_v4().to_string(),
        rule_id: rule.id.clone(),
        rule_name: rule.name.clone(),
        severity: rule.severity.clone(),
        status: AlertStatus::Firing,
        message: format!(
            "Test alert for rule '{}': {} {} {}",
            rule.name,
            rule.condition.metric_type.to_string(),
            rule.condition.operator.to_string(),
            rule.condition.threshold
        ),
        target: rule.condition.target_pattern.clone(),
        metric_value: rule.condition.threshold,
        threshold: rule.condition.threshold,
        fired_at: chrono::Utc::now().timestamp(),
        resolved_at: None,
        acknowledged_at: None,
        acknowledged_by: None,
    };
    let mut msg = crate::notification_channels::Message::from_alert(&alert);
    msg.title = format!("[TEST] {}", msg.title);
    msg.event = "alert_test".into();
    let (delivered, failed) = match state.alert_manager.channel_store() {
        Some(store) => store.dispatch(&msg).await,
        None => (0, 0),
    };
    Ok(Json(TestResult {
        channels: delivered + failed,
        delivered,
        failed,
    }))
}

fn validate(rule: &AlertRule) -> Result<(), ApiError> {
    if rule.name.trim().is_empty() {
        return Err(ApiError::BadRequest("Alert rule name is required".into()));
    }
    if rule.condition.target_pattern.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "Alert rule target pattern is required (use * for every target)".into(),
        ));
    }
    if !rule.condition.threshold.is_finite() {
        return Err(ApiError::BadRequest("Threshold must be a number".into()));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Evaluator
// ---------------------------------------------------------------------------

/// How often the rules are checked.
pub const EVAL_INTERVAL_SECS: u64 = 15;

/// One metric reading for one target (node hostname).
pub type Sample = (MetricType, String, f64);

/// Read the node's current values for every metric type the evaluator
/// supports. CPU usage needs the previous reading (`prev_cpu`) for a delta.
pub fn node_samples(
    node: &str,
    data_dir: &std::path::Path,
    vm_count: usize,
    prev_cpu: &mut Option<crate::metrics::system::CpuStats>,
) -> Vec<Sample> {
    use crate::metrics::system;
    let mut out = Vec::new();
    if let Ok(cpu) = system::read_cpu_stats() {
        if let Some(prev) = prev_cpu.as_ref() {
            out.push((
                MetricType::CpuUsage,
                node.to_string(),
                cpu.usage_percent(prev),
            ));
        }
        *prev_cpu = Some(cpu);
    }
    if let Ok(mem) = system::read_memory_stats() {
        out.push((
            MetricType::MemoryUsage,
            node.to_string(),
            mem.usage_percent(),
        ));
    }
    if let Ok(pct) = system::read_disk_usage_percent(&data_dir.to_string_lossy()) {
        out.push((MetricType::DiskUsage, node.to_string(), pct));
    }
    if let Ok(load) = system::read_load_average() {
        out.push((MetricType::NodeLoad, node.to_string(), load.one_min));
    }
    out.push((MetricType::VmCount, node.to_string(), vm_count as f64));
    out
}

/// Feed one round of samples to the alert manager.
pub async fn evaluate(alerts: &AlertManager, samples: Vec<Sample>) {
    for (metric, target, value) in samples {
        alerts.evaluate_metric(metric, &target, value).await;
    }
}

/// Start the background evaluator.
pub fn start_evaluator(state: Arc<AppState>) {
    tokio::spawn(async move {
        let node = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "localhost".into());
        let data_dir = state.config.paths.data_dir.clone();
        let mut prev_cpu = None;
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(EVAL_INTERVAL_SECS));
        loop {
            tick.tick().await;
            let vm_count = state.vm_manager.list_vms().await.len();
            let samples = node_samples(&node, &data_dir, vm_count, &mut prev_cpu);
            evaluate(&state.alert_manager, samples).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alerts::{AlertCondition, AlertSeverity, ComparisonOperator};

    #[test]
    fn node_samples_cover_every_evaluated_metric() {
        let mut prev = None;
        let first = node_samples("n1", std::path::Path::new("/"), 3, &mut prev);
        assert!(prev.is_some(), "CPU baseline taken");
        let kinds: Vec<&MetricType> = first.iter().map(|(m, _, _)| m).collect();
        for m in [
            MetricType::MemoryUsage,
            MetricType::DiskUsage,
            MetricType::NodeLoad,
            MetricType::VmCount,
        ] {
            assert!(kinds.contains(&&m), "{m:?} sampled");
        }
        assert!(
            !kinds.contains(&&MetricType::CpuUsage),
            "no CPU % without a previous reading"
        );
        assert!(first
            .iter()
            .any(|(m, t, v)| *m == MetricType::VmCount && t == "n1" && *v == 3.0));
        let second = node_samples("n1", std::path::Path::new("/"), 3, &mut prev);
        let cpu = second
            .iter()
            .find(|(m, _, _)| *m == MetricType::CpuUsage)
            .expect("CPU % from the second reading on");
        assert!((0.0..=100.0).contains(&cpu.2), "cpu {}", cpu.2);
    }

    #[tokio::test]
    async fn real_node_samples_fire_a_matching_rule() {
        let alerts = AlertManager::new();
        alerts
            .add_rule(AlertRule {
                id: "load".into(),
                name: "any load".into(),
                description: String::new(),
                severity: AlertSeverity::Info,
                enabled: true,
                condition: AlertCondition {
                    metric_type: MetricType::NodeLoad,
                    operator: ComparisonOperator::GreaterThan,
                    threshold: -1.0,
                    target_pattern: "*".into(),
                    duration_seconds: 0,
                },
            })
            .await
            .unwrap();
        let mut prev = None;
        evaluate(
            &alerts,
            node_samples("n1", std::path::Path::new("/"), 0, &mut prev),
        )
        .await;
        let active = alerts.get_active_alerts().await;
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].target, "n1");
        assert_eq!(active[0].rule_id, "load");
    }
}
