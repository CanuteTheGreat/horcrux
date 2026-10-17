//! Backup job schedules.
//!
//! A schedule is a standard 5-field cron expression
//! (`minute hour day-of-month month day-of-week`, local time) supporting
//! `*`, lists (`1,15`), ranges (`1-5`), steps (`*/15`, `0-30/10`), month and
//! weekday names (`jan`, `mon-fri`), weekday 7 = Sunday, and the aliases
//! `@hourly`/`hourly`, `@daily`/`daily`/`@midnight`, `@weekly`/`weekly`,
//! `@monthly`/`monthly`, `@yearly`/`yearly`/`@annually`. Like cron, when
//! both day-of-month and day-of-week are restricted a day matching either
//! one fires.
//!
//! The scheduler loop that fires due jobs lives in `BackupManager`
//! (`start_scheduler`); this module is the pure schedule logic.
use chrono::{DateTime, Datelike, Duration, Local, TimeZone, Timelike};

/// A parsed cron schedule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    minutes: Vec<bool>, // 0..60
    hours: Vec<bool>,   // 0..24
    mdays: Vec<bool>,   // 1..32 (index 0 unused)
    months: Vec<bool>,  // 1..13 (index 0 unused)
    wdays: Vec<bool>,   // 0..7 (0 = Sunday)
    mday_star: bool,
    wday_star: bool,
}

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];
const WDAYS: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

impl Schedule {
    /// Parse a cron expression or alias.
    pub fn parse(expr: &str) -> Result<Self, String> {
        let expr = expr.trim();
        let expanded = match expr.to_ascii_lowercase().as_str() {
            "@hourly" | "hourly" => "0 * * * *".to_string(),
            "@daily" | "daily" | "@midnight" => "0 0 * * *".to_string(),
            "@weekly" | "weekly" => "0 0 * * 0".to_string(),
            "@monthly" | "monthly" => "0 0 1 * *".to_string(),
            "@yearly" | "yearly" | "@annually" => "0 0 1 1 *".to_string(),
            _ => expr.to_string(),
        };
        let fields: Vec<&str> = expanded.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(format!(
                "schedule '{expr}' must have 5 fields (minute hour day-of-month month day-of-week) or be an alias like 'daily'"
            ));
        }
        let minutes = parse_field(fields[0], 0, 59, &[], "minute")?;
        let hours = parse_field(fields[1], 0, 23, &[], "hour")?;
        let mdays = parse_field(fields[2], 1, 31, &[], "day-of-month")?;
        let months = parse_field(fields[3], 1, 12, &MONTHS, "month")?;
        let mut wdays = parse_field(fields[4], 0, 7, &WDAYS, "day-of-week")?;
        if wdays[7] {
            wdays[0] = true;
        }
        wdays.truncate(7);
        Ok(Self {
            minutes,
            hours,
            mdays,
            months,
            wdays,
            mday_star: fields[2] == "*" || fields[2] == "?",
            wday_star: fields[4] == "*" || fields[4] == "?",
        })
    }

    fn day_matches(&self, d: &DateTime<Local>) -> bool {
        if !self.months[d.month() as usize] {
            return false;
        }
        let md = self.mdays[d.day() as usize];
        let wd = self.wdays[d.weekday().num_days_from_sunday() as usize];
        match (self.mday_star, self.wday_star) {
            (true, true) => true,
            (true, false) => wd,
            (false, true) => md,
            (false, false) => md || wd,
        }
    }

    /// First time strictly after `after` (to the minute) that matches.
    /// Returns None if nothing matches within ~5 years (e.g. Feb 31).
    pub fn next_after(&self, after: DateTime<Local>) -> Option<DateTime<Local>> {
        // Start at the next whole minute.
        let mut t = after
            .with_second(0)
            .and_then(|t| t.with_nanosecond(0))
            .unwrap_or(after)
            + Duration::minutes(1);
        let limit = after + Duration::days(366 * 5);
        while t <= limit {
            if !self.day_matches(&t) {
                // Jump to the start of the next day.
                let next_day = t.date_naive().succ_opt()?.and_hms_opt(0, 0, 0)?;
                t = match Local.from_local_datetime(&next_day).earliest() {
                    Some(v) => v,
                    // Midnight skipped by a DST change: try 01:00.
                    None => Local
                        .from_local_datetime(&next_day.with_hour(1)?)
                        .earliest()?,
                };
                continue;
            }
            if !self.hours[t.hour() as usize] {
                t = (t + Duration::hours(1)).with_minute(0)?;
                continue;
            }
            if self.minutes[t.minute() as usize] {
                return Some(t);
            }
            t += Duration::minutes(1);
        }
        None
    }
}

fn parse_value(s: &str, names: &[&str], min: u32) -> Result<u32, String> {
    if let Ok(v) = s.parse::<u32>() {
        return Ok(v);
    }
    let lower = s.to_ascii_lowercase();
    names
        .iter()
        .position(|n| *n == lower)
        .map(|i| i as u32 + min)
        .ok_or_else(|| format!("'{s}' is not a number"))
}

fn parse_field(
    field: &str,
    min: u32,
    max: u32,
    names: &[&str],
    what: &str,
) -> Result<Vec<bool>, String> {
    let mut set = vec![false; max as usize + 1];
    // Month names map jan=1, weekday names sun=0.
    let name_base = if names.len() == 12 { 1 } else { 0 };
    for part in field.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((r, s)) => {
                let step: u32 = s
                    .parse()
                    .map_err(|_| format!("bad step '{s}' in {what} field"))?;
                if step == 0 {
                    return Err(format!("step 0 in {what} field"));
                }
                (r, step)
            }
            None => (part, 1),
        };
        let (lo, hi) = if range == "*" || range == "?" {
            (min, max)
        } else if let Some((a, b)) = range.split_once('-') {
            (
                parse_value(a, names, name_base).map_err(|e| format!("{e} in {what} field"))?,
                parse_value(b, names, name_base).map_err(|e| format!("{e} in {what} field"))?,
            )
        } else {
            let v =
                parse_value(range, names, name_base).map_err(|e| format!("{e} in {what} field"))?;
            // "5/15" means 5,20,35,50 (from 5 to max).
            if part.contains('/') {
                (v, max)
            } else {
                (v, v)
            }
        };
        if lo < min || hi > max || lo > hi {
            return Err(format!("{what} value {lo}-{hi} out of range {min}-{max}"));
        }
        let mut v = lo;
        while v <= hi {
            set[v as usize] = true;
            v += step;
        }
    }
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    #[test]
    fn every_minute() {
        let s = Schedule::parse("* * * * *").unwrap();
        assert_eq!(
            s.next_after(at(2026, 1, 5, 10, 7)).unwrap(),
            at(2026, 1, 5, 10, 8)
        );
    }

    #[test]
    fn daily_at_two() {
        let s = Schedule::parse("0 2 * * *").unwrap();
        assert_eq!(
            s.next_after(at(2026, 1, 5, 10, 7)).unwrap(),
            at(2026, 1, 6, 2, 0)
        );
        assert_eq!(
            s.next_after(at(2026, 1, 5, 1, 59)).unwrap(),
            at(2026, 1, 5, 2, 0)
        );
    }

    #[test]
    fn steps_lists_ranges_names() {
        let s = Schedule::parse("*/15 9-17 * * mon-fri").unwrap();
        // 2026-01-09 is a Friday; next slot after 17:50 Fri is Mon 09:00.
        assert_eq!(
            s.next_after(at(2026, 1, 9, 17, 50)).unwrap(),
            at(2026, 1, 12, 9, 0)
        );
        assert_eq!(
            s.next_after(at(2026, 1, 12, 9, 0)).unwrap(),
            at(2026, 1, 12, 9, 15)
        );
        let s = Schedule::parse("30 4 1,15 jan,jul *").unwrap();
        assert_eq!(
            s.next_after(at(2026, 1, 15, 5, 0)).unwrap(),
            at(2026, 7, 1, 4, 30)
        );
    }

    #[test]
    fn sunday_is_0_or_7_and_aliases() {
        let a = Schedule::parse("0 0 * * 7").unwrap();
        let b = Schedule::parse("weekly").unwrap();
        // 2026-01-05 is a Monday; next Sunday is the 11th.
        assert_eq!(
            a.next_after(at(2026, 1, 5, 0, 0)).unwrap(),
            at(2026, 1, 11, 0, 0)
        );
        assert_eq!(
            b.next_after(at(2026, 1, 5, 0, 0)).unwrap(),
            at(2026, 1, 11, 0, 0)
        );
        assert!(Schedule::parse("@daily").is_ok());
        assert!(Schedule::parse("hourly").is_ok());
    }

    #[test]
    fn dom_or_dow_when_both_restricted() {
        // The 13th OR any Friday.
        let s = Schedule::parse("0 12 13 * fri").unwrap();
        // From Mon 2026-01-05: Fri 9th comes before the 13th.
        assert_eq!(
            s.next_after(at(2026, 1, 5, 0, 0)).unwrap(),
            at(2026, 1, 9, 12, 0)
        );
        assert_eq!(
            s.next_after(at(2026, 1, 9, 12, 0)).unwrap(),
            at(2026, 1, 13, 12, 0)
        );
    }

    #[test]
    fn rejects_garbage() {
        for bad in [
            "",
            "* * * *",
            "61 * * * *",
            "* 25 * * *",
            "*/0 * * * *",
            "x * * * *",
        ] {
            assert!(Schedule::parse(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn impossible_date_has_no_next_run() {
        let s = Schedule::parse("0 0 31 2 *").unwrap();
        assert!(s.next_after(at(2026, 1, 1, 0, 0)).is_none());
    }
}
