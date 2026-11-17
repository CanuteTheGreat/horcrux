//! Applying a spec with an audit trail: every real apply (API, CLI, GitOps,
//! rollback) is recorded in the history with its actor, source, spec, plan
//! and per-change results, and refreshes the drift view.
use super::exec::{self, ApplyReport};
use super::store::{self, HistoryEntry};
use super::{parse, Action, Spec};
use crate::AppState;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::Arc;

#[derive(Debug)]
pub enum SvcError {
    /// The request is wrong (bad spec, nothing to roll back to).
    Invalid(String),
    NotFound(String),
    /// Another apply is running.
    Busy(String),
    Internal(String),
}

impl From<String> for SvcError {
    fn from(s: String) -> Self {
        SvcError::Internal(s)
    }
}

#[derive(Debug, Clone)]
pub struct Origin {
    pub actor: String,
    /// api | cli | ui | gitops | rollback
    pub source: String,
    pub commit: Option<String>,
    pub rollback_of: Option<String>,
}

impl Origin {
    pub fn new(actor: &str, source: &str) -> Self {
        Origin {
            actor: actor.to_string(),
            source: source.to_string(),
            commit: None,
            rollback_of: None,
        }
    }
}

/// Sources a client may claim; GitOps and rollback are set by the server.
pub fn client_source(s: Option<&str>) -> &'static str {
    match s {
        Some("cli") => "cli",
        Some("ui") => "ui",
        _ => "api",
    }
}

fn summary(r: &ApplyReport) -> String {
    if !r.plan.errors.is_empty() {
        return format!("refused: {}", r.plan.errors.join("; "));
    }
    let n = |st: &str| r.results.iter().filter(|x| x.status == st).count();
    let (a, f, s) = (n("applied"), n("failed"), n("skipped"));
    let mut out = format!("{a} applied");
    if f > 0 {
        out += &format!(", {f} failed");
    }
    if s > 0 {
        out += &format!(", {s} blocked");
    }
    out
}

/// Plan and apply `text`, recording the apply (unless `dry_run`, or there was
/// nothing to do: an in-sync GitOps poll must not fill the history).
pub async fn apply_text(
    state: &Arc<AppState>,
    text: &str,
    prune: Option<bool>,
    dry_run: bool,
    origin: Origin,
) -> Result<ApplyReport, SvcError> {
    let spec = parse(text).map_err(SvcError::Invalid)?;
    apply_spec(state, text, &spec, prune, dry_run, origin, None).await
}

async fn apply_spec(
    state: &Arc<AppState>,
    text: &str,
    spec: &Spec,
    prune: Option<bool>,
    dry_run: bool,
    origin: Origin,
    only_deletes: Option<&BTreeSet<(String, String)>>,
) -> Result<ApplyReport, SvcError> {
    let prune = prune.unwrap_or(spec.prune);
    let mut rep = exec::apply_with(state, spec, prune, dry_run, only_deletes)
        .await
        .map_err(SvcError::Busy)?;
    if dry_run || (rep.plan.changes.is_empty() && rep.plan.errors.is_empty()) {
        return Ok(rep);
    }
    let (id, ts_ms) = store::new_history_id();
    let entry = HistoryEntry {
        id: id.clone(),
        time: store::now(),
        ts_ms,
        actor: origin.actor,
        source: origin.source,
        commit: origin.commit,
        spec_hash: store::sha256_hex(text),
        spec: text.to_string(),
        prune,
        ok: rep.ok,
        plan: rep.plan.clone(),
        results: rep.results.clone(),
        rollback_of: origin.rollback_of,
        summary: summary(&rep),
    };
    match store::record_history(state, &entry).await {
        Ok(()) => rep.history_id = Some(id),
        Err(e) => tracing::warn!("declarative: recording the apply failed: {e}"),
    }
    super::drift::refresh_soon(state.clone());
    Ok(rep)
}

/// Rollback of history entry `id`: apply the spec that was applied before it
/// (or the spec of entry `to`), and delete only what `id` created that the
/// older spec doesn't contain. `dry_run` previews. The result is a new
/// history entry (`source: rollback`).
pub async fn rollback(
    state: &Arc<AppState>,
    id: &str,
    to: Option<&str>,
    dry_run: bool,
    set_desired: bool,
    actor: &str,
) -> Result<Value, SvcError> {
    let all = store::history(state).await; // newest first
    let target = all
        .iter()
        .find(|e| e.id == id)
        .ok_or_else(|| SvcError::NotFound(format!("history entry {id} not found")))?;
    let from = match to {
        Some(t) => all
            .iter()
            .find(|e| e.id == t)
            .ok_or_else(|| SvcError::NotFound(format!("history entry {t} not found")))?,
        None => all.iter().find(|e| e.id.as_str() < id).ok_or_else(|| {
            SvcError::Invalid(
                "there is no earlier apply to go back to (this is the oldest record kept)".into(),
            )
        })?,
    };
    let mut spec = parse(&from.spec)
        .map_err(|e| SvcError::Invalid(format!("the spec of {} no longer parses: {e}", from.id)))?;

    // What the rolled-back apply created: only that may be deleted.
    let created: BTreeSet<(String, String)> = target
        .results
        .iter()
        .filter(|r| r.status == "applied" && r.change.action == Action::Create)
        .map(|r| (r.change.kind.clone(), r.change.name.clone()))
        .collect();
    let made = |k: &str| created.iter().any(|(ck, _)| ck == k);
    // A section the older spec didn't manage becomes managed-and-empty, so
    // prune can see what the rolled-back apply added to it.
    if made("storage_pool") && spec.storage_pools.is_none() {
        spec.storage_pools = Some(vec![]);
    }
    if made("vm") && spec.vms.is_none() {
        spec.vms = Some(vec![]);
    }
    if made("zone") || made("vnet") || made("subnet") {
        let n = spec.networks.get_or_insert_with(Default::default);
        if made("zone") && n.zones.is_none() {
            n.zones = Some(vec![]);
        }
        if made("vnet") && n.vnets.is_none() {
            n.vnets = Some(vec![]);
        }
        if made("subnet") && n.subnets.is_none() {
            n.subnets = Some(vec![]);
        }
    }
    if let Ok(newer) = parse(&target.spec) {
        for (key, sec) in &newer.extra {
            let ours = super::kinds::find(key)
                .map(|k| k.change_kinds().iter().any(|ck| made(ck)))
                .unwrap_or(false);
            if ours && !spec.extra.contains_key(key) && sec.is_array() {
                spec.extra.insert(key.clone(), json!([]));
            }
        }
    }
    let origin = Origin {
        actor: actor.to_string(),
        source: "rollback".into(),
        commit: None,
        rollback_of: Some(target.id.clone()),
    };
    let rep = apply_spec(
        state,
        &from.spec,
        &spec,
        Some(true),
        dry_run,
        origin,
        Some(&created),
    )
    .await?;
    if !dry_run && set_desired && rep.ok {
        store::put_desired(state, &from.spec, actor, "rollback", None)
            .await
            .map_err(SvcError::Internal)?;
    }
    Ok(json!({
        "rollback_of": target.id,
        "restored_from": from.id,
        "restored_spec_hash": from.spec_hash,
        "dry_run": dry_run,
        "report": rep,
    }))
}
