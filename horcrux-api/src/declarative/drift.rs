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

// ------------------------------------------------------ text diff of spec/live

fn strip_nulls(v: &mut serde_json::Value) {
    use serde_json::Value;
    match v {
        Value::Object(m) => {
            m.retain(|_, x| !x.is_null());
            m.values_mut().for_each(strip_nulls);
        }
        Value::Array(a) => a.iter_mut().for_each(strip_nulls),
        _ => {}
    }
}

fn identity(v: &serde_json::Value) -> String {
    for k in ["name", "id", "cidr"] {
        if let Some(s) = v.get(k).and_then(|x| x.as_str()) {
            return s.to_string();
        }
    }
    v.to_string()
}

/// The part of `live` that `want` talks about: same keys, and in lists of
/// resources only the ones the spec lists (all of them with `keep_extra`,
/// i.e. prune). What the spec leaves out is never drift, so it must not show
/// up in the text diff either.
pub fn restrict(
    live: &serde_json::Value,
    want: &serde_json::Value,
    keep_extra: bool,
) -> serde_json::Value {
    use serde_json::Value;
    match (live, want) {
        (Value::Object(l), Value::Object(w)) => {
            let mut out = serde_json::Map::new();
            for (k, wv) in w {
                if let Some(lv) = l.get(k) {
                    out.insert(k.clone(), restrict(lv, wv, keep_extra));
                }
            }
            Value::Object(out)
        }
        (Value::Array(l), Value::Array(w)) if w.iter().all(|x| x.is_object()) => {
            let mut out: Vec<Value> = l
                .iter()
                .filter_map(|li| {
                    let id = identity(li);
                    match w.iter().find(|wi| identity(wi) == id) {
                        Some(wi) => Some(restrict(li, wi, keep_extra)),
                        None => keep_extra.then(|| li.clone()),
                    }
                })
                .collect();
            out.sort_by_key(identity);
            Value::Array(out)
        }
        _ => live.clone(),
    }
}

/// Normalised YAML of the desired spec and of the matching slice of the live
/// state, ready for a line diff.
pub async fn diff_texts(state: &Arc<AppState>) -> Result<serde_json::Value, String> {
    let Some(d) = store::desired(state).await else {
        return Ok(serde_json::json!({"configured": false}));
    };
    let spec = parse(&d.spec)?;
    let live = exec::gather(state).await;
    let mut want = serde_json::to_value(&spec).map_err(|e| e.to_string())?;
    strip_nulls(&mut want);
    if let Some(o) = want.as_object_mut() {
        o.remove("prune");
    }
    let mut have = serde_json::to_value(super::export(&live)).map_err(|e| e.to_string())?;
    strip_nulls(&mut have);
    let mut have = restrict(&have, &want, spec.prune);
    // arrays in the desired spec are sorted the same way as the live ones
    sort_lists(&mut want);
    sort_lists(&mut have);
    let desired = serde_yaml::to_string(&want).map_err(|e| e.to_string())?;
    let live_txt = serde_yaml::to_string(&have).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "configured": true,
        "same": desired == live_txt,
        "desired": desired,
        "live": live_txt,
    }))
}

fn sort_lists(v: &mut serde_json::Value) {
    use serde_json::Value;
    match v {
        Value::Object(m) => m.values_mut().for_each(sort_lists),
        Value::Array(a) => {
            a.iter_mut().for_each(sort_lists);
            if a.iter().all(|x| x.is_object()) {
                a.sort_by_key(identity);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn restrict_keeps_only_what_the_spec_states() {
        let live = json!({"vms": [
            {"name": "b", "memory": 64, "cpus": 1, "id": "b"},
            {"name": "a", "memory": 300, "cpus": 2, "id": "a", "architecture": "x86_64"}]});
        let want = json!({"vms": [{"name": "a", "memory": 128}]});
        let out = restrict(&live, &want, false);
        assert_eq!(out, json!({"vms": [{"name": "a", "memory": 300}]}));
        // with prune the VM the spec doesn't list is shown too
        let out = restrict(&live, &want, true);
        assert_eq!(out["vms"].as_array().unwrap().len(), 2);
        // null in the spec means "not stated"
        let mut w = json!({"a": null, "b": {"c": null, "d": 1}});
        strip_nulls(&mut w);
        assert_eq!(w, json!({"b": {"d": 1}}));
    }
}
