//! Drift detection: the stored desired spec planned against the live node.
//! A periodic check (default every 5 minutes, `Settings::drift_interval_secs`)
//! keeps `GET /api/declarative/drift` current, feeds the alert system
//! (metric `config_drift`, rule "Configuration drift") and drives the
//! dashboard badge. The same background task runs the GitOps sync.
use super::{exec, parse, store, Action, Plan};
use crate::AppState;
use horcrux_monitoring::alerts::{
    AlertCondition, AlertRule, AlertSeverity, ComparisonOperator, MetricType,
};
use serde::Serialize;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, RwLock};

pub const RULE_NAME: &str = "Configuration drift";

pub struct Runtime {
    pub drift: RwLock<Option<DriftReport>>,
    pub kick_drift: Notify,
    pub kick_git: Notify,
}

pub fn rt() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| Runtime {
        drift: RwLock::new(None),
        kick_drift: Notify::new(),
        kick_git: Notify::new(),
    })
}

/// One difference between the desired spec and the live node.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DriftItem {
    pub kind: String,
    pub name: String,
    /// The differing field, or `(resource)` / `power` for whole-resource items.
    pub field: String,
    pub expected: String,
    pub actual: String,
    /// create | update | delete | start | stop | blocked: what apply would do.
    pub action: String,
    /// The engine can't fix it in place (apply reports it, a human must).
    pub blocked: bool,
    /// Why it is blocked / extra detail.
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DriftReport {
    /// A desired spec is stored.
    pub configured: bool,
    pub checked_at: String,
    pub desired_hash: Option<String>,
    pub desired_source: Option<String>,
    pub in_sync: bool,
    /// Differences plus plan errors.
    pub count: usize,
    pub blocked: usize,
    pub items: Vec<DriftItem>,
    /// The spec cannot be applied at all (dangling reference, ...).
    pub errors: Vec<String>,
    pub plan: Option<Plan>,
    /// The check itself failed (stored spec no longer parses, ...).
    pub error: Option<String>,
}

impl DriftReport {
    fn empty(configured: bool) -> Self {
        DriftReport {
            configured,
            checked_at: store::now(),
            desired_hash: None,
            desired_source: None,
            in_sync: true,
            count: 0,
            blocked: 0,
            items: vec![],
            errors: vec![],
            plan: None,
            error: None,
        }
    }
}

/// Flatten a plan into per-field drift items.
pub fn items_of(plan: &Plan) -> Vec<DriftItem> {
    let mut out = Vec::new();
    for c in &plan.changes {
        let action = format!("{:?}", c.action).to_lowercase();
        let blocked = c.action == Action::Blocked;
        let note = c
            .details
            .iter()
            .filter(|d| {
                !c.fields
                    .iter()
                    .any(|f| d.starts_with(&format!("{}: ", f.field)))
            })
            .cloned()
            .collect::<Vec<_>>()
            .join("; ");
        let mk = |field: &str, expected: String, actual: String, note: String| DriftItem {
            kind: c.kind.clone(),
            name: c.name.clone(),
            field: field.into(),
            expected,
            actual,
            action: action.clone(),
            blocked,
            note,
        };
        if !c.fields.is_empty() {
            for f in &c.fields {
                out.push(mk(
                    &f.field,
                    f.expected.clone(),
                    f.actual.clone(),
                    note.clone(),
                ));
            }
            continue;
        }
        out.push(match c.action {
            Action::Create => mk("(resource)", "present".into(), "missing".into(), note),
            Action::Delete => mk("(resource)", "absent".into(), "present".into(), note),
            Action::Start => mk("power", "running".into(), "stopped".into(), note),
            Action::Stop => mk("power", "stopped".into(), "running".into(), note),
            _ => mk(
                "(resource)",
                "as specified".into(),
                "different".into(),
                note,
            ),
        });
    }
    out
}

/// Plan the stored desired spec against the live node (pure read).
pub async fn compute(state: &Arc<AppState>) -> DriftReport {
    let Some(d) = store::desired(state).await else {
        return DriftReport::empty(false);
    };
    let mut r = DriftReport::empty(true);
    r.desired_hash = Some(d.hash.clone());
    r.desired_source = Some(d.source.clone());
    let spec = match parse(&d.spec) {
        Ok(s) => s,
        Err(e) => {
            r.in_sync = false;
            r.count = 1;
            r.error = Some(format!("the stored desired spec doesn't parse: {e}"));
            return r;
        }
    };
    let live = exec::gather(state).await;
    let plan = super::plan(&spec, &live, spec.prune);
    r.items = items_of(&plan);
    r.errors = plan.errors.clone();
    r.blocked = r.items.iter().filter(|i| i.blocked).count();
    r.count = r.items.len() + r.errors.len();
    r.in_sync = plan.in_sync();
    r.plan = Some(plan);
    r
}

/// Compute, cache and feed the alert system.
pub async fn check(state: &Arc<AppState>) -> DriftReport {
    let r = compute(state).await;
    *rt().drift.write().await = Some(r.clone());
    if store::settings(state).await.drift_alert {
        let node = node_name();
        state
            .alert_manager
            .evaluate_metric(MetricType::ConfigDrift, &node, r.count as f64)
            .await;
    }
    r
}

pub async fn cached_or_check(state: &Arc<AppState>) -> DriftReport {
    if let Some(r) = rt().drift.read().await.clone() {
        return r;
    }
    check(state).await
}

/// Re-check shortly (after an apply or a desired-spec change).
pub fn refresh_soon(_state: Arc<AppState>) {
    rt().kick_drift.notify_one();
}

pub fn node_name() -> String {
    hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "localhost".into())
}

async fn ensure_rule(state: &Arc<AppState>) {
    if state
        .alert_manager
        .list_rules()
        .await
        .iter()
        .any(|r| r.name == RULE_NAME)
    {
        return;
    }
    let rule = AlertRule::new(
        RULE_NAME.into(),
        "The node differs from its desired declarative spec (see Declarative > Drift)".into(),
        AlertSeverity::Warning,
        AlertCondition {
            metric_type: MetricType::ConfigDrift,
            operator: ComparisonOperator::GreaterThan,
            threshold: 0.0,
            target_pattern: "*".into(),
            duration_seconds: 0,
        },
    );
    if let Err(e) = state.alert_manager.add_rule(rule).await {
        tracing::warn!("declarative: creating the drift alert rule: {e}");
    }
}

/// Background task: drift checks and the GitOps sync.
pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(10)).await;
        ensure_rule(&state).await;
        let mut last_drift: Option<Instant> = None;
        loop {
            let mut force_drift = false;
            let mut force_git = false;
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(3)) => {}
                _ = rt().kick_drift.notified() => force_drift = true,
                _ = rt().kick_git.notified() => force_git = true,
            }
            let settings = store::settings(&state).await;
            let due = |last: Option<Instant>, every: u64| {
                last.map(|t| t.elapsed() >= Duration::from_secs(every))
                    .unwrap_or(true)
            };
            if force_drift || due(last_drift, settings.drift_interval_secs) {
                last_drift = Some(Instant::now());
                check(&state).await;
            }
            if let Some(cfg) = store::gitops_config(&state).await {
                // "due" comes from the persisted last sync, so a restart
                // doesn't resync early and a manual sync resets the clock
                let git_due = || async {
                    let last = store::gitops_state(&state).await.last_sync;
                    match last.and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok()) {
                        Some(t) => {
                            (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_seconds()
                                >= cfg.interval_secs as i64
                        }
                        None => true,
                    }
                };
                if cfg.enabled && (force_git || git_due().await) {
                    if let Err(e) =
                        super::gitops::sync(&state, super::gitops::Trigger::Schedule).await
                    {
                        tracing::debug!("declarative gitops: {}", e.message());
                    }
                }
            }
        }
    });
}
