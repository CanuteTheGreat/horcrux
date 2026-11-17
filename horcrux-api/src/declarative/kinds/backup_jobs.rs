//! Backup jobs (schedules). Needs a node built with USE=backup.
//!
//! ```yaml
//! backup_jobs:
//!   - id: nightly
//!     schedule: "0 2 * * *"        # cron expression or alias (daily, weekly...)
//!     targets: [web, db]           # VM / container ids
//!     storage: local               # "" / local = default directory, a path, or a pool
//!     mode: snapshot               # snapshot | suspend | stop
//!     compression: zstd            # none | gzip | lzo | zstd | dedup
//!     retention: {keep_last: 3, keep_daily: 7}
//!     notify: failure              # always | failure | never
//!     enabled: true
//! ```
//! Run history (`last_run`, `next_run`...) is server state, never part of the
//! spec. Deleting a job keeps the backups it made.
use super::{ch, items, to_json, Kind, KindPlan};
use crate::declarative::{Action, Change, Live, Spec};
use crate::AppState;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::Arc;

fn yes() -> bool {
    true
}
fn is_true(b: &bool) -> bool {
    *b
}
fn snapshot() -> String {
    "snapshot".into()
}
fn zstd() -> String {
    "zstd".into()
}
fn is_snapshot(s: &String) -> bool {
    s == "snapshot"
}
fn is_zstd(s: &String) -> bool {
    s == "zstd"
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RetentionSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_last: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_hourly: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_daily: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_weekly: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_monthly: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_yearly: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BackupJobSpec {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    pub schedule: String,
    pub targets: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub storage: String,
    #[serde(default = "snapshot", skip_serializing_if = "is_snapshot")]
    pub mode: String,
    #[serde(default = "zstd", skip_serializing_if = "is_zstd")]
    pub compression: String,
    #[serde(default, skip_serializing_if = "is_default_retention")]
    pub retention: RetentionSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify: Option<String>,
}

fn is_default_retention(r: &RetentionSpec) -> bool {
    r == &RetentionSpec::default()
}

/// The live job in the spec's own shape.
pub type LiveBackupJob = BackupJobSpec;

fn canon(j: &BackupJobSpec) -> BackupJobSpec {
    let mut c = j.clone();
    c.targets = j
        .targets
        .iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    c.targets.dedup();
    c.mode = j.mode.trim().to_ascii_lowercase();
    c.compression = j.compression.trim().to_ascii_lowercase();
    c.schedule = j.schedule.trim().to_string();
    c.storage = if j.storage.trim() == "local" {
        String::new()
    } else {
        j.storage.trim().to_string()
    };
    c.notify = j
        .notify
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string);
    c.comment = j.comment.clone().filter(|c| !c.is_empty());
    c
}

pub fn validate(v: &[BackupJobSpec]) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for j in v {
        let id = &j.id;
        if id.is_empty()
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        {
            return Err(format!(
                "backup job {id:?}: id may only hold letters, digits, _ - ."
            ));
        }
        if !seen.insert(id) {
            return Err(format!("backup job {id:?} is listed twice"));
        }
        let c = canon(j);
        if c.targets.is_empty() {
            return Err(format!("backup job {id}: at least one target is required"));
        }
        if c.schedule.is_empty() {
            return Err(format!("backup job {id}: schedule is required"));
        }
        if !matches!(c.mode.as_str(), "snapshot" | "suspend" | "stop") {
            return Err(format!(
                "backup job {id}: mode must be snapshot, suspend or stop"
            ));
        }
        if !matches!(
            c.compression.as_str(),
            "none" | "gzip" | "lzo" | "zstd" | "dedup"
        ) {
            return Err(format!(
                "backup job {id}: compression must be none, gzip, lzo, zstd or dedup"
            ));
        }
        if let Some(n) = &c.notify {
            if !matches!(n.as_str(), "always" | "failure" | "never") {
                return Err(format!(
                    "backup job {id}: notify must be always, failure or never"
                ));
            }
        }
    }
    Ok(())
}

pub fn plan(want: &[BackupJobSpec], live: &[LiveBackupJob], prune: bool) -> KindPlan {
    let mut p = KindPlan::default();
    for w in want {
        let w = canon(w);
        match live.iter().map(canon).find(|l| l.id == w.id) {
            None => p.changes.push(ch(
                "backup_job",
                &w.id,
                Action::Create,
                vec![format!("{} -> {} target(s)", w.schedule, w.targets.len())],
            )),
            Some(l) if l != w => {
                let mut d = Vec::new();
                macro_rules! f {
                    ($($n:ident),*) => {$(
                        if l.$n != w.$n { d.push(format!("{}: {:?} -> {:?}", stringify!($n), l.$n, w.$n)); }
                    )*};
                }
                f!(
                    comment,
                    enabled,
                    schedule,
                    targets,
                    storage,
                    mode,
                    compression,
                    retention,
                    notify
                );
                p.changes.push(ch("backup_job", &w.id, Action::Update, d));
            }
            Some(_) => {}
        }
    }
    if prune {
        for l in live.iter().filter(|l| !want.iter().any(|w| w.id == l.id)) {
            p.deletes.push(ch(
                "backup_job",
                &l.id,
                Action::Delete,
                vec!["the backups it made are kept".into()],
            ));
        }
    }
    p
}

pub fn export(live: &[LiveBackupJob]) -> Vec<BackupJobSpec> {
    let mut v: Vec<BackupJobSpec> = live.iter().map(canon).collect();
    v.sort_by(|a, b| a.id.cmp(&b.id));
    v
}

// ------------------------------------------------------------------ kind

pub struct BackupJobs;

#[async_trait]
impl Kind for BackupJobs {
    fn key(&self) -> &'static str {
        "backup_jobs"
    }
    fn change_kinds(&self) -> &'static [&'static str] {
        &["backup_job"]
    }
    fn rank(&self) -> u32 {
        80
    }
    fn validate(&self, section: &Value) -> Result<(), String> {
        validate(&items::<BackupJobSpec>(section)?)
    }
    async fn gather(&self, state: &Arc<AppState>) -> Result<Value, String> {
        to_json(&export(&gather_live(state).await?))
    }
    fn plan(
        &self,
        section: &Value,
        live: &Value,
        prune: bool,
        _whole: (&Spec, &Live),
        out: &mut KindPlan,
    ) {
        match (
            items::<BackupJobSpec>(section),
            items::<BackupJobSpec>(live),
        ) {
            (Ok(w), Ok(l)) => out.merge(plan(&w, &l, prune)),
            (Err(e), _) | (_, Err(e)) => out.errors.push(format!("backup_jobs: {e}")),
        }
    }
    async fn apply(
        &self,
        state: &Arc<AppState>,
        section: &Value,
        _live: &Value,
        c: &Change,
    ) -> Result<Option<String>, String> {
        apply(state, section, c).await
    }
}

#[cfg(feature = "backup")]
fn job_to_spec(j: &crate::backup::BackupJob) -> BackupJobSpec {
    let name = |v: serde_json::Value| v.as_str().unwrap_or_default().to_string();
    BackupJobSpec {
        id: j.id.clone(),
        comment: j.comment.clone(),
        enabled: j.enabled,
        schedule: j.schedule.clone(),
        targets: j.targets.clone(),
        storage: j.storage.clone(),
        mode: name(serde_json::to_value(&j.mode).unwrap_or_default()),
        compression: name(serde_json::to_value(&j.compression).unwrap_or_default()),
        retention: RetentionSpec {
            keep_last: j.retention.keep_last,
            keep_hourly: j.retention.keep_hourly,
            keep_daily: j.retention.keep_daily,
            keep_weekly: j.retention.keep_weekly,
            keep_monthly: j.retention.keep_monthly,
            keep_yearly: j.retention.keep_yearly,
        },
        notify: j.notify.clone(),
    }
}

#[cfg(feature = "backup")]
fn spec_to_job(j: &BackupJobSpec) -> Result<crate::backup::BackupJob, String> {
    let mode = serde_json::from_value(serde_json::json!(j.mode.trim().to_ascii_lowercase()))
        .map_err(|e| e.to_string())?;
    let compression =
        serde_json::from_value(serde_json::json!(j.compression.trim().to_ascii_lowercase()))
            .map_err(|e| e.to_string())?;
    Ok(crate::backup::BackupJob {
        id: j.id.clone(),
        comment: j.comment.clone(),
        enabled: j.enabled,
        schedule: j.schedule.clone(),
        targets: j.targets.clone(),
        storage: j.storage.clone(),
        mode,
        compression,
        retention: crate::backup::RetentionPolicy {
            keep_last: j.retention.keep_last,
            keep_hourly: j.retention.keep_hourly,
            keep_daily: j.retention.keep_daily,
            keep_weekly: j.retention.keep_weekly,
            keep_monthly: j.retention.keep_monthly,
            keep_yearly: j.retention.keep_yearly,
        },
        notify: j.notify.clone(),
        last_run: None,
        last_status: None,
        last_error: None,
        next_run: None,
    })
}

#[cfg(feature = "backup")]
async fn gather_live(state: &Arc<AppState>) -> Result<Vec<LiveBackupJob>, String> {
    let jobs = state
        .backup_manager
        .list_jobs()
        .await
        .map_err(|e| format!("{e}"))?;
    Ok(jobs.iter().map(job_to_spec).collect())
}

#[cfg(not(feature = "backup"))]
async fn gather_live(_: &Arc<AppState>) -> Result<Vec<LiveBackupJob>, String> {
    Err("this node was built without backups (USE=backup)".into())
}

#[cfg(feature = "backup")]
async fn apply(
    state: &Arc<AppState>,
    section: &Value,
    c: &Change,
) -> Result<Option<String>, String> {
    let e = |x: horcrux_common::Error| format!("{x}");
    let bm = &state.backup_manager;
    match c.action {
        Action::Create | Action::Update => {
            let want: Vec<BackupJobSpec> = items(section)?;
            let w = want.iter().find(|j| j.id == c.name).ok_or("not in spec")?;
            let job = spec_to_job(w)?;
            if c.action == Action::Create {
                bm.create_job(job).await.map_err(e)?;
            } else {
                bm.update_job(&c.name, job).await.map_err(e)?;
            }
            Ok(None)
        }
        Action::Delete => {
            bm.delete_job(&c.name).await.map_err(e)?;
            Ok(None)
        }
        a => Err(format!("no executor for backup_job {a:?}")),
    }
}

#[cfg(not(feature = "backup"))]
async fn apply(_: &Arc<AppState>, _: &Value, _: &Change) -> Result<Option<String>, String> {
    Err("this server was built without backups (USE=backup)".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: &str) -> BackupJobSpec {
        BackupJobSpec {
            id: id.into(),
            comment: None,
            enabled: true,
            schedule: "0 2 * * *".into(),
            targets: vec!["web".into()],
            storage: String::new(),
            mode: "snapshot".into(),
            compression: "zstd".into(),
            retention: RetentionSpec::default(),
            notify: None,
        }
    }
    fn acts(p: &KindPlan) -> Vec<String> {
        p.changes
            .iter()
            .chain(&p.deletes)
            .map(|c| format!("{:?} {}", c.action, c.name))
            .collect()
    }

    #[test]
    fn create_update_prune_in_order() {
        let mut changed = job("b");
        changed.schedule = "daily".into();
        changed.retention.keep_last = Some(3);
        let live = vec![job("b"), job("old")];
        let p = plan(&[job("a"), changed], &live, true);
        assert_eq!(acts(&p), ["Create a", "Update b", "Delete old"]);
        assert!(p.changes[1]
            .details
            .iter()
            .any(|d| d.starts_with("schedule")));
        assert!(p.changes[1]
            .details
            .iter()
            .any(|d| d.starts_with("retention")));
        let p = plan(&[job("a"), job("b")], &live, false);
        assert_eq!(acts(&p), ["Create a"]);
    }

    #[test]
    fn spellings_the_manager_normalises_are_not_drift() {
        let mut w = job("a");
        w.storage = "local".into();
        w.notify = Some(String::new());
        w.mode = "Snapshot".into();
        assert!(acts(&plan(&[w], &[job("a")], true)).is_empty());
    }

    #[test]
    fn export_is_a_fixed_point_and_omits_defaults() {
        let mut j = job("n");
        j.retention.keep_daily = Some(7);
        j.notify = Some("failure".into());
        let live = vec![j, job("a")];
        let spec = export(&live);
        assert_eq!(spec[0].id, "a");
        let yaml = serde_yaml::to_string(&spec).unwrap();
        assert!(
            !yaml.contains("mode:") && !yaml.contains("compression:"),
            "{yaml}"
        );
        let back: Vec<BackupJobSpec> = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back, spec);
        validate(&spec).unwrap();
        assert!(acts(&plan(&spec, &live, true)).is_empty());
    }

    #[test]
    fn validation() {
        validate(&[job("a")]).unwrap();
        assert!(validate(&[job("a"), job("a")])
            .unwrap_err()
            .contains("twice"));
        let mut j = job("a");
        j.targets.clear();
        assert!(validate(&[j]).unwrap_err().contains("target"));
        let mut j = job("a");
        j.mode = "yolo".into();
        assert!(validate(&[j]).is_err());
        let mut j = job("a");
        j.compression = "rar".into();
        assert!(validate(&[j]).is_err());
        let mut j = job("a");
        j.notify = Some("sometimes".into());
        assert!(validate(&[j]).is_err());
        assert!(validate(&[job("bad id")]).is_err());
        assert!(serde_yaml::from_str::<BackupJobSpec>(
            "id: a\nschedule: x\ntargets: [a]\nbogus: 1"
        )
        .is_err());
    }
}
