//! Backup retention ("prune") policy.
//!
//! Implements the same marking algorithm as Proxmox VE's `prune-backups`
//! (PVE::Storage::prune_mark): the options are applied in the order
//! keep-last, keep-hourly, keep-daily, keep-weekly, keep-monthly,
//! keep-yearly. Each option walks the backups newest-first and keeps the
//! newest backup of every period (hour/day/ISO week/month/year) that no
//! earlier option already covered, until it has kept N new periods.
//! Protected backups are always kept and never count against any option.
//! Everything not kept is removed. A policy with no keep-* option keeps
//! everything.
use super::{Backup, RetentionPolicy};
use chrono::{Datelike, Local, TimeZone};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Why a backup was kept or removed by a prune run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PruneMark {
    Keep,
    Remove,
    Protected,
}

/// One backup's prune decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PruneDecision {
    pub backup_id: String,
    pub timestamp: i64,
    pub mark: PruneMark,
}

impl RetentionPolicy {
    /// True when no keep-* option is set (prune would keep everything).
    pub fn keeps_all(&self) -> bool {
        [
            self.keep_last,
            self.keep_hourly,
            self.keep_daily,
            self.keep_weekly,
            self.keep_monthly,
            self.keep_yearly,
        ]
        .iter()
        .all(|o| o.unwrap_or(0) == 0)
    }
}

type IdFn = fn(i64, usize) -> String;

fn id_last(_: i64, idx: usize) -> String {
    format!("idx-{idx}")
}
fn id_hour(ts: i64, _: usize) -> String {
    period(ts, |d| d.format("%Y/%m/%d/%H").to_string())
}
fn id_day(ts: i64, _: usize) -> String {
    period(ts, |d| d.format("%Y/%m/%d").to_string())
}
fn id_week(ts: i64, _: usize) -> String {
    period(ts, |d| {
        let w = d.iso_week();
        format!("{}/{}", w.year(), w.week())
    })
}
fn id_month(ts: i64, _: usize) -> String {
    period(ts, |d| format!("{}/{}", d.year(), d.month()))
}
fn id_year(ts: i64, _: usize) -> String {
    period(ts, |d| d.year().to_string())
}

/// Mark every backup in `backups` (one guest, one storage) as keep, remove
/// or protected. Returned newest-first.
pub fn mark(backups: &[&Backup], policy: &RetentionPolicy) -> Vec<PruneDecision> {
    let mut sorted: Vec<&Backup> = backups.to_vec();
    sorted.sort_by_key(|b| std::cmp::Reverse(b.timestamp));

    let mut marks: Vec<Option<PruneMark>> = sorted
        .iter()
        .map(|b| b.protected.then_some(PruneMark::Protected))
        .collect();

    if policy.keeps_all() {
        for m in marks.iter_mut() {
            if m.is_none() {
                *m = Some(PruneMark::Keep);
            }
        }
    } else {
        let options: [(Option<u32>, IdFn); 6] = [
            (policy.keep_last, id_last),
            (policy.keep_hourly, id_hour),
            (policy.keep_daily, id_day),
            (policy.keep_weekly, id_week),
            (policy.keep_monthly, id_month),
            (policy.keep_yearly, id_year),
        ];

        for (count, id_fn) in options {
            let Some(count) = count.filter(|c| *c > 0) else {
                continue;
            };
            // Periods already covered by a backup an earlier option kept.
            let already: HashSet<String> = sorted
                .iter()
                .enumerate()
                .filter(|(i, _)| marks[*i] == Some(PruneMark::Keep))
                .map(|(i, b)| id_fn(b.timestamp, i))
                .collect();
            let mut newly: HashSet<String> = HashSet::new();
            for (i, b) in sorted.iter().enumerate() {
                if marks[i].is_some() {
                    continue;
                }
                let id = id_fn(b.timestamp, i);
                if already.contains(&id) {
                    continue;
                }
                if !newly.contains(&id) {
                    if newly.len() >= count as usize {
                        break;
                    }
                    newly.insert(id);
                    marks[i] = Some(PruneMark::Keep);
                } else {
                    marks[i] = Some(PruneMark::Remove);
                }
            }
        }
    }

    sorted
        .iter()
        .zip(marks)
        .map(|(b, m)| PruneDecision {
            backup_id: b.id.clone(),
            timestamp: b.timestamp,
            mark: m.unwrap_or(PruneMark::Remove),
        })
        .collect()
}

/// Period key of a unix timestamp in the host's local time zone (the same
/// zone the scheduler uses), or a unique key if it is out of range.
fn period(ts: i64, f: impl Fn(&chrono::DateTime<Local>) -> String) -> String {
    match Local.timestamp_opt(ts, 0).single() {
        Some(d) => f(&d),
        None => format!("invalid-{ts}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::{BackupMode, Compression, TargetType};
    use std::path::PathBuf;

    fn backup(id: &str, ts: i64, protected: bool) -> Backup {
        Backup {
            id: id.to_string(),
            target_type: TargetType::Vm,
            target_id: "100".to_string(),
            target_name: "test".to_string(),
            timestamp: ts,
            size: 1,
            mode: BackupMode::Stop,
            compression: Compression::Zstd,
            path: PathBuf::from("/tmp/x"),
            storage: "/tmp".to_string(),
            config_included: true,
            notes: None,
            protected,
            sha256: String::new(),
            job_id: None,
            verify_state: None,
            verified_at: None,
            verify_error: None,
        }
    }

    fn removed(d: &[PruneDecision]) -> Vec<String> {
        let mut v: Vec<String> = d
            .iter()
            .filter(|d| d.mark == PruneMark::Remove)
            .map(|d| d.backup_id.clone())
            .collect();
        v.sort();
        v
    }

    // Noon local time on consecutive days, so DST never moves a backup
    // across a day boundary.
    fn day(n: i64) -> i64 {
        Local
            .with_ymd_and_hms(2024, 1, 1, 12, 0, 0)
            .unwrap()
            .timestamp()
            + n * 86400
    }

    #[test]
    fn keep_daily_keeps_newest_n_days() {
        let bs: Vec<Backup> = (0..10)
            .map(|i| backup(&format!("b{i}"), day(i), false))
            .collect();
        let refs: Vec<&Backup> = bs.iter().collect();
        let d = mark(
            &refs,
            &RetentionPolicy {
                keep_daily: Some(7),
                ..Default::default()
            },
        );
        assert_eq!(removed(&d), vec!["b0", "b1", "b2"]);
    }

    #[test]
    fn keep_last_counts_individual_backups() {
        // Three backups on the same day: keep-daily=1 would keep one,
        // keep-last=2 keeps the two newest.
        let bs = [
            backup("a", day(0), false),
            backup("b", day(0) + 60, false),
            backup("c", day(0) + 120, false),
        ];
        let refs: Vec<&Backup> = bs.iter().collect();
        let d = mark(
            &refs,
            &RetentionPolicy {
                keep_last: Some(2),
                ..Default::default()
            },
        );
        assert_eq!(removed(&d), vec!["a"]);
    }

    #[test]
    fn later_options_skip_periods_covered_by_earlier_ones() {
        // keep-last=1 keeps day 3; keep-daily=2 must then keep days 2 and 1
        // (day 3 is already covered), so only day 0 is removed.
        let bs: Vec<Backup> = (0..4)
            .map(|i| backup(&format!("d{i}"), day(i), false))
            .collect();
        let refs: Vec<&Backup> = bs.iter().collect();
        let d = mark(
            &refs,
            &RetentionPolicy {
                keep_last: Some(1),
                keep_daily: Some(2),
                ..Default::default()
            },
        );
        assert_eq!(removed(&d), vec!["d0"]);
    }

    #[test]
    fn protected_backups_survive_and_do_not_count() {
        let bs = [
            backup("old-protected", day(0), true),
            backup("mid", day(1), false),
            backup("new", day(2), false),
        ];
        let refs: Vec<&Backup> = bs.iter().collect();
        let d = mark(
            &refs,
            &RetentionPolicy {
                keep_last: Some(1),
                ..Default::default()
            },
        );
        assert_eq!(removed(&d), vec!["mid"]);
        assert!(d
            .iter()
            .any(|x| x.backup_id == "old-protected" && x.mark == PruneMark::Protected));
    }

    #[test]
    fn empty_policy_keeps_everything() {
        let bs: Vec<Backup> = (0..5)
            .map(|i| backup(&format!("e{i}"), day(i), false))
            .collect();
        let refs: Vec<&Backup> = bs.iter().collect();
        assert!(removed(&mark(&refs, &RetentionPolicy::default())).is_empty());
    }

    #[test]
    fn weekly_and_monthly() {
        // 70 daily backups from Mon 2024-01-01 to Sun 2024-03-10.
        // keep-weekly=2 keeps the newest of ISO week 10 (Mar 10, day 69)
        // and week 9 (Mar 3, day 62) and, like Proxmox, marks the other
        // backups of those two weeks (down to Mon Feb 26, day 56) as
        // removed. keep-monthly=2 then finds March already covered and
        // keeps the newest still-unmarked backup of February (Feb 25,
        // day 55) and of January (Jan 31, day 30).
        let bs: Vec<Backup> = (0..70)
            .map(|i| backup(&format!("w{i:02}"), day(i), false))
            .collect();
        let refs: Vec<&Backup> = bs.iter().collect();
        let d = mark(
            &refs,
            &RetentionPolicy {
                keep_weekly: Some(2),
                keep_monthly: Some(2),
                ..Default::default()
            },
        );
        let kept: Vec<&str> = d
            .iter()
            .filter(|x| x.mark == PruneMark::Keep)
            .map(|x| x.backup_id.as_str())
            .collect();
        assert_eq!(kept, vec!["w69", "w62", "w55", "w30"]);
    }
}
