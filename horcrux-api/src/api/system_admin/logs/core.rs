//! Pure parts of the system log viewer: entry types, filter resolution,
//! journalctl argument building, wildcard matching, syslog / journal
//! parsing and reboot (boot) detection. No I/O here, so the same code
//! behaves identically on journald (systemd) and syslog-file (OpenRC) hosts.
use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

pub const PRIORITY_NAMES: [&str; 8] = [
    "emerg", "alert", "crit", "err", "warning", "notice", "info", "debug",
];

pub fn priority_name(p: u8) -> &'static str {
    PRIORITY_NAMES[usize::from(p.min(7))]
}

/// One log line, from journald or a syslog file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct LogEntry {
    pub timestamp: String,
    /// Four-step summary (`error`, `warn`, `info`, `debug`), kept for older clients.
    pub level: String,
    /// The syslog identifier, else the unit (older clients' "service").
    pub service: Option<String>,
    pub message: String,
    pub details: Option<String>,
    /// Microseconds since the Unix epoch (for ordering and live mode).
    #[serde(default)]
    pub ts_us: Option<i64>,
    /// 0 (emerg) .. 7 (debug).
    #[serde(default)]
    pub priority: Option<u8>,
    #[serde(default)]
    pub priority_name: Option<String>,
    /// True when the source file carries no priority and it was guessed
    /// from the message text.
    #[serde(default)]
    pub priority_inferred: bool,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub identifier: Option<String>,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub boot_id: Option<String>,
    #[serde(default)]
    pub hostname: Option<String>,
    #[serde(default)]
    pub kernel: bool,
    /// Which cluster node the entry came from.
    #[serde(default)]
    pub node: Option<String>,
}

/// Query parameters of the log routes (also the body sent to peer nodes).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Filters {
    /// Legacy minimum level: `error`, `warn`, `info`, `debug` (or `all`).
    pub level: Option<String>,
    /// Legacy: substring of the identifier/unit.
    pub service: Option<String>,
    pub search: Option<String>,
    pub limit: Option<usize>,
    pub format: Option<String>,
    /// Start of the time span: `2026-11-25 10:00`, RFC 3339, epoch seconds,
    /// or relative (`1h`, `-30m`, `2d`, `now`).
    pub since: Option<String>,
    pub until: Option<String>,
    /// Minimum priority 0-7 (3 = errors and worse).
    pub priority: Option<u8>,
    /// systemd unit / OpenRC service (`ssh`, `ssh.service`, `docker*`).
    pub unit: Option<String>,
    /// SYSLOG_IDENTIFIER; `*` matches any run of characters.
    pub identifier: Option<String>,
    /// Kernel messages only.
    pub kernel: Option<bool>,
    /// A boot: `0` (current), `-1` (previous) or a boot id.
    pub boot: Option<String>,
    /// Another cluster node's logs.
    pub node: Option<String>,
}

/// Filters after validation: times as microseconds, empty strings dropped.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Resolved {
    pub since_us: Option<i64>,
    pub until_us: Option<i64>,
    /// Entries must have `priority <= max_priority`.
    pub max_priority: Option<u8>,
    pub unit: Option<String>,
    pub identifier: Option<String>,
    pub kernel: bool,
    pub boot: Option<String>,
    pub search: Option<String>,
    pub service: Option<String>,
    pub limit: usize,
}

fn nonempty(s: &Option<String>) -> Option<String> {
    s.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "all")
        .map(str::to_string)
}

/// A relative or absolute time as microseconds since the epoch.
/// `date_end` makes a bare date mean the end of that day (for `until`).
pub fn parse_time(s: &str, now_us: i64, local: FixedOffset) -> Result<i64, String> {
    let t = s.trim();
    let bad = || {
        format!(
            "cannot read the time {t:?}: use 2026-11-25 10:00, an RFC 3339 time, \
             epoch seconds or a relative span such as 1h, 30m, 2d"
        )
    };
    if t.is_empty() {
        return Err(bad());
    }
    let lower = t.to_ascii_lowercase();
    if lower == "now" {
        return Ok(now_us);
    }
    // relative: 15m, -2h, 3 days ago
    let rel = lower.trim_end_matches("ago").trim().trim_start_matches('-');
    let digits_end = rel.find(|c: char| !c.is_ascii_digit()).unwrap_or(rel.len());
    if digits_end > 0
        && digits_end < rel.len()
        && rel[digits_end..]
            .chars()
            .all(|c| c.is_ascii_alphabetic() || c == ' ')
    {
        let n: i64 = rel[..digits_end].parse().map_err(|_| bad())?;
        let unit = rel[digits_end..].trim();
        let secs = match unit {
            "s" | "sec" | "secs" | "second" | "seconds" => 1,
            "m" | "min" | "mins" | "minute" | "minutes" => 60,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3600,
            "d" | "day" | "days" => 86_400,
            "w" | "week" | "weeks" => 7 * 86_400,
            _ => return Err(bad()),
        };
        return n
            .checked_mul(secs)
            .and_then(|s| s.checked_mul(1_000_000))
            .and_then(|us| now_us.checked_sub(us))
            .ok_or_else(bad);
    }
    // epoch seconds (optionally fractional)
    if let Ok(secs) = t.parse::<f64>() {
        if secs >= 1_000_000_000.0 {
            return Ok((secs * 1_000_000.0) as i64);
        }
        return Err(bad());
    }
    if let Ok(d) = DateTime::parse_from_rfc3339(t) {
        return Ok(d.timestamp_micros());
    }
    for f in [
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%dT%H:%M",
    ] {
        if let Ok(n) = NaiveDateTime::parse_from_str(t, f) {
            return local
                .from_local_datetime(&n)
                .single()
                .map(|d| d.timestamp_micros())
                .ok_or_else(bad);
        }
    }
    if let Ok(d) = NaiveDate::parse_from_str(t, "%Y-%m-%d") {
        let n = d.and_hms_opt(0, 0, 0).ok_or_else(bad)?;
        return local
            .from_local_datetime(&n)
            .single()
            .map(|d| d.timestamp_micros())
            .ok_or_else(bad);
    }
    Err(bad())
}

fn safe_word(name: &str, v: &str) -> Result<(), String> {
    if v.starts_with('-') || v.chars().any(|c| c.is_control()) || v.len() > 256 {
        return Err(format!("{name} {v:?} is not a valid value"));
    }
    Ok(())
}

pub fn level_to_priority(l: &str) -> Option<u8> {
    match l {
        "error" => Some(3),
        "warn" | "warning" => Some(4),
        "info" => Some(6),
        "debug" | "trace" => Some(7),
        _ => None,
    }
}

/// Validate and normalise the filters.
pub fn resolve(
    f: &Filters,
    default_limit: usize,
    max_limit: usize,
    now_us: i64,
    local: FixedOffset,
) -> Result<Resolved, String> {
    let since_us = nonempty(&f.since)
        .map(|s| parse_time(&s, now_us, local))
        .transpose()?;
    let until_us = nonempty(&f.until)
        .map(|s| parse_time(&s, now_us, local))
        .transpose()?;
    if let (Some(a), Some(b)) = (since_us, until_us) {
        if a > b {
            return Err("since is later than until".into());
        }
    }
    if let Some(p) = f.priority {
        if p > 7 {
            return Err("priority must be 0 (emerg) to 7 (debug)".into());
        }
    }
    let max_priority = f
        .priority
        .or_else(|| nonempty(&f.level).and_then(|l| level_to_priority(&l)));
    let unit = nonempty(&f.unit);
    if let Some(u) = &unit {
        safe_word("unit", u)?;
    }
    let identifier = nonempty(&f.identifier);
    if let Some(i) = &identifier {
        safe_word("identifier", i)?;
    }
    let boot = nonempty(&f.boot)
        .map(|b| match b.as_str() {
            "current" => Ok("0".to_string()),
            "previous" => Ok("-1".to_string()),
            _ => {
                let id = b.len() == 32 && b.chars().all(|c| c.is_ascii_hexdigit());
                let idx = b.parse::<i64>().is_ok();
                let syn = b.starts_with("boot-")
                    && b[5..]
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-');
                if id || idx || syn {
                    Ok(b)
                } else {
                    Err(format!(
                        "boot {b:?} must be a number (0 = current, -1 = previous) or a boot id"
                    ))
                }
            }
        })
        .transpose()?;
    Ok(Resolved {
        since_us,
        until_us,
        max_priority,
        unit,
        identifier,
        kernel: f.kernel.unwrap_or(false),
        boot,
        search: nonempty(&f.search),
        service: nonempty(&f.service),
        limit: f.limit.unwrap_or(default_limit).clamp(1, max_limit),
    })
}

/// Case-insensitive match where `*` stands for any run of characters.
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);
    while ti < t.len() {
        if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

fn utc_text(us: i64, round_up: bool) -> String {
    let mut secs = us.div_euclid(1_000_000);
    if round_up && us.rem_euclid(1_000_000) != 0 {
        secs += 1;
    }
    Utc.timestamp_opt(secs, 0)
        .single()
        .unwrap_or_default()
        .format("%Y-%m-%d %H:%M:%S UTC")
        .to_string()
}

/// True when a post-filter (done here, not by journalctl) is needed so
/// journalctl must be allowed to scan more than `limit` lines.
pub fn needs_scan(r: &Resolved) -> bool {
    r.search.is_some()
        || r.service.is_some()
        || r.identifier.as_deref().is_some_and(|i| i.contains('*'))
}

/// Arguments for `journalctl`: every filter that journald can apply itself.
pub fn journal_args(r: &Resolved, max_scan: usize) -> Vec<String> {
    let n = if needs_scan(r) { max_scan } else { r.limit };
    let mut a: Vec<String> = ["-o", "json", "--no-pager", "-r", "-n"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    a.push(n.to_string());
    if let Some(us) = r.since_us {
        a.push(format!("--since={}", utc_text(us, false)));
    }
    if let Some(us) = r.until_us {
        a.push(format!("--until={}", utc_text(us, true)));
    }
    if let Some(p) = r.max_priority {
        a.push(format!("--priority={p}"));
    }
    if let Some(u) = &r.unit {
        a.push(format!("--unit={u}"));
    }
    if let Some(i) = r.identifier.as_deref().filter(|i| !i.contains('*')) {
        a.push(format!("--identifier={i}"));
    }
    if let Some(b) = r.boot.as_deref().filter(|b| !b.starts_with("boot-")) {
        a.push(format!("--boot={b}"));
    }
    if r.kernel {
        // not `-k`: that one also pins the current boot
        a.push("_TRANSPORT=kernel".into());
    }
    a
}

/// The post-filters shared by every source (what journalctl cannot do).
pub fn post_match(e: &LogEntry, r: &Resolved) -> bool {
    if let Some(i) = r.identifier.as_deref().filter(|i| i.contains('*')) {
        if !e
            .identifier
            .as_deref()
            .is_some_and(|x| wildcard_match(i, x))
        {
            return false;
        }
    }
    if let Some(s) = &r.service {
        if !e.service.as_deref().is_some_and(|x| x.contains(s.as_str())) {
            return false;
        }
    }
    if let Some(q) = &r.search {
        let q = q.to_ascii_lowercase();
        if !(e.message.to_ascii_lowercase().contains(&q)
            || e.service
                .as_deref()
                .is_some_and(|s| s.to_ascii_lowercase().contains(&q)))
        {
            return false;
        }
    }
    true
}

fn strip_service(u: &str) -> &str {
    u.strip_suffix(".service").unwrap_or(u)
}

/// Filters journalctl does itself, applied to entries read from a file.
pub fn file_match(e: &LogEntry, r: &Resolved, boots: &[BootInfo]) -> bool {
    if let (Some(a), Some(t)) = (r.since_us, e.ts_us) {
        if t < a {
            return false;
        }
    }
    if let (Some(b), Some(t)) = (r.until_us, e.ts_us) {
        if t > b {
            return false;
        }
    }
    if let Some(p) = r.max_priority {
        if e.priority.unwrap_or(6) > p {
            return false;
        }
    }
    if r.kernel && !e.kernel {
        return false;
    }
    if let Some(u) = &r.unit {
        let who = e.unit.as_deref().or(e.identifier.as_deref());
        if !who.is_some_and(|w| wildcard_match(strip_service(u), strip_service(w))) {
            return false;
        }
    }
    if let Some(i) = r.identifier.as_deref().filter(|i| !i.contains('*')) {
        if !e
            .identifier
            .as_deref()
            .is_some_and(|x| x.eq_ignore_ascii_case(i))
        {
            return false;
        }
    }
    if let Some(b) = &r.boot {
        match resolve_boot(b, boots) {
            Some(id) => {
                if e.boot_id.as_deref() != Some(id.as_str()) {
                    return false;
                }
            }
            None => return false,
        }
    }
    post_match(e, r)
}

/// A `--boot` value (index or id) as the boot id it names.
pub fn resolve_boot(b: &str, boots: &[BootInfo]) -> Option<String> {
    if let Ok(i) = b.parse::<i64>() {
        return boots
            .iter()
            .find(|x| x.index == i)
            .map(|x| x.boot_id.clone());
    }
    boots
        .iter()
        .find(|x| x.boot_id == b)
        .map(|x| x.boot_id.clone())
}

// ------------------------------------------------------------- parsing

pub fn infer_level(msg: &str) -> &'static str {
    let m = msg.to_ascii_lowercase();
    if ["error", "fail", "fatal", "crit", "panic", "denied"]
        .iter()
        .any(|k| m.contains(k))
    {
        "error"
    } else if m.contains("warn") {
        "warn"
    } else if m.contains("debug") {
        "debug"
    } else {
        "info"
    }
}

fn level_of_priority(p: u8) -> &'static str {
    match p {
        0..=3 => "error",
        4 => "warn",
        5 | 6 => "info",
        _ => "debug",
    }
}

fn priority_of_level(l: &str) -> u8 {
    match l {
        "error" => 3,
        "warn" => 4,
        "debug" => 7,
        _ => 6,
    }
}

/// `sshd[411]` -> (`sshd`, Some(411))
pub fn split_tag(tag: &str) -> (String, Option<u32>) {
    match tag.split_once('[') {
        Some((id, rest)) => (
            id.to_string(),
            rest.trim_end_matches(']').parse::<u32>().ok(),
        ),
        None => (tag.to_string(), None),
    }
}

/// The offset the host's local time has now (classic syslog stamps carry no zone).
pub fn local_offset() -> FixedOffset {
    *chrono::Local::now().offset()
}

/// One syslog line: classic (`Oct 22 12:00:01 host prog[12]: msg`), ISO
/// (`2026-10-22T12:00:01.1+00:00 host prog[12]: msg`), either optionally
/// starting with a `<PRI>` field.
pub fn parse_syslog_line(line: &str, year: i32, tz: FixedOffset) -> Option<LogEntry> {
    let mut line = line.trim_end();
    let mut logged_pri = None::<u8>;
    let mut facility = None::<u8>;
    if let Some(rest) = line.strip_prefix('<') {
        if let Some((n, r)) = rest.split_once('>') {
            if let Ok(n) = n.parse::<u16>() {
                if n < 192 {
                    logged_pri = Some((n & 7) as u8);
                    facility = Some((n >> 3) as u8);
                    line = r;
                }
            }
        }
    }
    let (ts, ts_us, rest) = if line.len() > 20 && line.as_bytes().get(4) == Some(&b'-') {
        let (t, r) = line.split_once(' ')?;
        let us = DateTime::parse_from_rfc3339(t).ok()?.timestamp_micros();
        (t.to_string(), us, r)
    } else if line.len() > 16 {
        let (head, r) = (line.get(..15)?, line.get(15..)?);
        let mut it = head.split_whitespace();
        let (mon, day, time) = (it.next()?, it.next()?, it.next()?);
        let m = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ]
        .iter()
        .position(|x| *x == mon)?
            + 1;
        let day: u32 = day.parse().ok()?;
        let stamp = format!("{year}-{m:02}-{day:02}T{time}");
        let naive = NaiveDateTime::parse_from_str(&stamp, "%Y-%m-%dT%H:%M:%S").ok()?;
        let us = tz.from_local_datetime(&naive).single()?.timestamp_micros();
        (stamp, us, r.trim_start())
    } else {
        return None;
    };
    let rest = rest.trim_start();
    let (host, rest) = rest.split_once(' ')?;
    let (tag, msg) = match rest.split_once(": ") {
        Some((t, m)) if !t.contains(' ') => (Some(t), m),
        _ => (None, rest),
    };
    let (identifier, pid) = tag.map(split_tag).unzip();
    let identifier = identifier.filter(|i| !i.is_empty());
    let (priority, inferred) = match logged_pri {
        Some(p) => (p, false),
        None => (priority_of_level(infer_level(msg)), true),
    };
    let kernel = identifier.as_deref() == Some("kernel") || facility == Some(0);
    Some(LogEntry {
        timestamp: ts,
        level: level_of_priority(priority).into(),
        service: identifier.clone(),
        message: msg.to_string(),
        details: None,
        ts_us: Some(ts_us),
        priority: Some(priority),
        priority_name: Some(priority_name(priority).into()),
        priority_inferred: inferred,
        unit: None,
        identifier,
        pid: pid.flatten(),
        boot_id: None,
        hostname: Some(host.to_string()),
        kernel,
        node: None,
    })
}

/// A syslog line stamped in December that is read in January belongs to
/// last year: the year the classic format lacks is whichever puts the
/// stamp closest to (and not after) `now`.
pub fn parse_syslog_line_now(line: &str, now_us: i64, tz: FixedOffset) -> Option<LogEntry> {
    let year = DateTime::from_timestamp(now_us.div_euclid(1_000_000), 0)?
        .with_timezone(&tz)
        .year();
    let mut e = parse_syslog_line(line, year, tz)?;
    if e.ts_us.is_some_and(|t| t > now_us + 86_400_000_000) {
        e = parse_syslog_line(line, year - 1, tz)?;
    }
    Some(e)
}

/// One `journalctl -o json` object.
pub fn parse_journal_json(line: &str) -> Option<LogEntry> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let s = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .map(|x| x.to_string())
            .filter(|x| !x.is_empty())
    };
    let micros: i64 = s("__REALTIME_TIMESTAMP")?.parse().ok()?;
    let ts =
        chrono::DateTime::from_timestamp(micros / 1_000_000, ((micros % 1_000_000) * 1000) as u32)?
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let prio = s("PRIORITY")
        .and_then(|p| p.parse::<u8>().ok())
        .unwrap_or(6)
        .min(7);
    // MESSAGE is an array of bytes for binary messages.
    let message = match v.get("MESSAGE") {
        Some(serde_json::Value::String(m)) => m.clone(),
        Some(serde_json::Value::Array(b)) => String::from_utf8_lossy(
            &b.iter()
                .filter_map(|x| x.as_u64().map(|n| n as u8))
                .collect::<Vec<_>>(),
        )
        .to_string(),
        _ => String::new(),
    };
    let ident = s("SYSLOG_IDENTIFIER");
    let unit = s("_SYSTEMD_UNIT");
    let pid = s("_PID")
        .or_else(|| s("SYSLOG_PID"))
        .and_then(|p| p.parse::<u32>().ok());
    let kernel = s("_TRANSPORT").as_deref() == Some("kernel")
        || s("SYSLOG_FACILITY").as_deref() == Some("0");
    Some(LogEntry {
        timestamp: ts,
        level: level_of_priority(prio).into(),
        service: ident.clone().or_else(|| unit.clone()),
        details: unit.clone().filter(|u| Some(u) != ident.as_ref()),
        message,
        ts_us: Some(micros),
        priority: Some(prio),
        priority_name: Some(priority_name(prio).into()),
        priority_inferred: false,
        unit,
        identifier: ident,
        pid,
        boot_id: s("_BOOT_ID"),
        hostname: s("_HOSTNAME"),
        kernel,
        node: None,
    })
}

/// One `/dev/kmsg` record: `pri,seq,usec-since-boot,flags;message`.
/// Continuation lines (they start with a space) belong to the record before.
pub fn parse_kmsg(rec: &str, boot_us: i64) -> Option<LogEntry> {
    if rec.starts_with(' ') {
        return None;
    }
    let (head, msg) = rec.split_once(';')?;
    let mut f = head.split(',');
    let pri: u16 = f.next()?.parse().ok()?;
    let _seq = f.next()?;
    let usec: i64 = f.next()?.parse().ok()?;
    let sev = (pri & 7) as u8;
    let ts_us = boot_us.checked_add(usec)?;
    let ts = DateTime::from_timestamp(ts_us.div_euclid(1_000_000), (ts_us.rem_euclid(1_000_000) * 1000) as u32)?
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    Some(LogEntry {
        timestamp: ts,
        level: level_of_priority(sev).into(),
        service: Some("kernel".into()),
        message: msg.trim_end().to_string(),
        ts_us: Some(ts_us),
        priority: Some(sev),
        priority_name: Some(priority_name(sev).into()),
        identifier: Some("kernel".into()),
        kernel: true,
        ..Default::default()
    })
}

// --------------------------------------------------------------- boots

/// One boot of the node.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct BootInfo {
    /// 0 = the current boot, -1 the one before, ...
    pub index: i64,
    pub boot_id: String,
    pub first_us: Option<i64>,
    pub last_us: Option<i64>,
    /// RFC 3339 forms of the two times above.
    pub first_entry: Option<String>,
    pub last_entry: Option<String>,
    /// `journald`, `wtmp`, `kernel-log` or `uptime`: where the boot was found.
    #[serde(default)]
    pub source: String,
}

fn rfc(us: i64) -> Option<String> {
    DateTime::from_timestamp(us.div_euclid(1_000_000), 0)
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

/// `journalctl --list-boots -o json`
pub fn parse_list_boots_json(text: &str) -> Option<Vec<BootInfo>> {
    let v: serde_json::Value = serde_json::from_str(text.trim()).ok()?;
    let arr = v.as_array()?;
    let mut out: Vec<BootInfo> = arr
        .iter()
        .filter_map(|b| {
            let id = b.get("boot_id")?.as_str()?.to_string();
            let idx = b.get("index")?.as_i64()?;
            let f = b.get("first_entry").and_then(|x| x.as_i64());
            let l = b.get("last_entry").and_then(|x| x.as_i64());
            Some(BootInfo {
                index: idx,
                boot_id: id,
                first_us: f,
                last_us: l,
                first_entry: f.and_then(rfc),
                last_entry: l.and_then(rfc),
                source: "journald".into(),
            })
        })
        .collect();
    normalise_indexes(&mut out);
    Some(out)
}

/// Older journalctl: ` -1 <32 hex> Mon 2026-.. TZ—Tue 2026-.. TZ`. Positive
/// indexes count from the oldest boot, so they are converted to 0/-n.
pub fn parse_list_boots_text(text: &str) -> Vec<BootInfo> {
    let mut out = Vec::new();
    for l in text.lines() {
        let mut it = l.split_whitespace();
        let (Some(i), Some(id)) = (it.next(), it.next()) else {
            continue;
        };
        let (Ok(idx), true) = (
            i.parse::<i64>(),
            id.len() == 32 && id.chars().all(|c| c.is_ascii_hexdigit()),
        ) else {
            continue;
        };
        let rest = l.splitn(3, char::is_whitespace).nth(2).unwrap_or("").trim();
        let rest = rest.trim_start_matches(id).trim();
        let (a, b) = rest
            .split_once('—')
            .map(|(a, b)| (a.trim().to_string(), b.trim().to_string()))
            .unwrap_or((rest.to_string(), String::new()));
        out.push(BootInfo {
            index: idx,
            boot_id: id.to_string(),
            first_us: None,
            last_us: None,
            first_entry: Some(a).filter(|x| !x.is_empty()),
            last_entry: Some(b).filter(|x| !x.is_empty()),
            source: "journald".into(),
        });
    }
    normalise_indexes(&mut out);
    out
}

/// Whatever numbering journalctl used, make the newest boot 0 and number
/// older ones -1, -2 ... in list order (oldest first).
fn normalise_indexes(v: &mut [BootInfo]) {
    let n = v.len() as i64;
    let positive_first = v.first().is_some_and(|b| b.index >= 0) && n > 1;
    let oldest_first = positive_first
        || v.windows(2).all(|w| w[0].index <= w[1].index) && v.last().is_some_and(|b| b.index == 0);
    if oldest_first {
        for (i, b) in v.iter_mut().enumerate() {
            b.index = i as i64 - (n - 1);
        }
    }
}

/// BOOT_TIME records (type 2) of a `wtmp` file, in seconds.
pub fn wtmp_boot_times(bytes: &[u8]) -> Vec<i64> {
    const SZ: usize = 384;
    let mut out = Vec::new();
    for rec in bytes.chunks_exact(SZ) {
        let ty = i16::from_le_bytes([rec[0], rec[1]]);
        if ty == 2 {
            let sec = i32::from_le_bytes([rec[340], rec[341], rec[342], rec[343]]);
            if sec > 0 {
                out.push(i64::from(sec));
            }
        }
    }
    out
}

/// A kernel message that starts every boot.
pub fn is_boot_line(e: &LogEntry) -> bool {
    e.kernel && e.message.starts_with("Linux version ")
}

/// Boot windows for a host without journald: `starts_us` are the moments
/// reboots happened (from wtmp, kernel boot lines, uptime). Starts closer
/// together than 120 s are one boot. Entries before the first start belong
/// to an earlier boot of unknown start.
pub fn boots_from_starts(
    mut starts_us: Vec<i64>,
    first_entry_us: Option<i64>,
    now_us: i64,
) -> Vec<BootInfo> {
    starts_us.sort_unstable();
    let mut merged: Vec<i64> = Vec::new();
    for s in starts_us {
        if merged.last().is_none_or(|l| s - l > 120_000_000) {
            merged.push(s);
        }
    }
    let mut out: Vec<BootInfo> = Vec::new();
    if let (Some(fe), Some(first)) = (first_entry_us, merged.first()) {
        if fe < *first - 1_000_000 {
            out.push(BootInfo {
                boot_id: "boot-unknown".into(),
                first_us: Some(fe),
                last_us: Some(*first),
                first_entry: rfc(fe),
                last_entry: rfc(*first),
                source: "log".into(),
                ..Default::default()
            });
        }
    }
    for (i, s) in merged.iter().enumerate() {
        let end = merged.get(i + 1).copied().unwrap_or(now_us);
        out.push(BootInfo {
            boot_id: format!("boot-{}", s.div_euclid(1_000_000)),
            first_us: Some(*s),
            last_us: Some(end),
            first_entry: rfc(*s),
            last_entry: rfc(end),
            source: "syslog".into(),
            ..Default::default()
        });
    }
    let n = out.len() as i64;
    for (i, b) in out.iter_mut().enumerate() {
        b.index = i as i64 - (n - 1);
    }
    out
}

/// The boot a timestamp belongs to.
pub fn boot_of(ts_us: i64, boots: &[BootInfo]) -> Option<&BootInfo> {
    boots
        .iter()
        .rev()
        .find(|b| b.first_us.is_some_and(|f| ts_us >= f))
        .or_else(|| boots.first().filter(|b| b.first_us.is_none()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }
    const NOW: i64 = 1_792_720_241_000_000; // 2026-10-23T01:50:41Z

    #[test]
    fn times_absolute_relative_and_bad() {
        let p = |s: &str| parse_time(s, NOW, utc());
        assert_eq!(p("now").unwrap(), NOW);
        assert_eq!(p("1h").unwrap(), NOW - 3_600_000_000);
        assert_eq!(p("-30m").unwrap(), NOW - 1_800_000_000);
        assert_eq!(p("2 days ago").unwrap(), NOW - 2 * 86_400_000_000);
        assert_eq!(p("1792720241").unwrap(), NOW);
        assert_eq!(p("2026-10-23 01:50:41").unwrap(), NOW);
        assert_eq!(p("2026-10-23T01:50:41Z").unwrap(), NOW);
        assert_eq!(p("2026-10-23T01:50:41+00:00").unwrap(), NOW);
        assert_eq!(p("2026-10-23").unwrap(), NOW - (NOW % 86_400_000_000));
        assert!(p("yesterday-ish").is_err());
        assert!(p("").is_err());
        assert!(p("5x").is_err());
        // local zone applies to zone-less text
        let west = FixedOffset::west_opt(8 * 3600).unwrap();
        assert_eq!(
            parse_time("2026-10-23 00:00", NOW, west).unwrap(),
            NOW - (NOW % 86_400_000_000) + 8 * 3_600_000_000
        );
    }

    #[test]
    fn wildcards_match_like_a_glob() {
        assert!(wildcard_match("ssh*", "sshd"));
        assert!(wildcard_match("*d", "sshd"));
        assert!(wildcard_match("s*h*", "sshd-session"));
        assert!(wildcard_match("SSHD", "sshd"));
        assert!(wildcard_match("*", ""));
        assert!(!wildcard_match("ssh", "sshd"));
        assert!(!wildcard_match("a*b", "ac"));
        assert!(wildcard_match("a**b", "axxb"));
        assert!(!wildcard_match("", "x"));
    }

    fn rs(f: &Filters) -> Resolved {
        resolve(f, 200, 5000, NOW, utc()).unwrap()
    }

    #[test]
    fn journalctl_arguments_carry_every_server_side_filter() {
        let r = rs(&Filters {
            since: Some("1h".into()),
            until: Some("2026-10-23 01:50:41".into()),
            priority: Some(4),
            unit: Some("ssh.service".into()),
            identifier: Some("sshd".into()),
            kernel: Some(true),
            boot: Some("-1".into()),
            limit: Some(50),
            ..Default::default()
        });
        let a = journal_args(&r, 1000);
        assert_eq!(&a[..5], ["-o", "json", "--no-pager", "-r", "-n"]);
        assert_eq!(a[5], "50");
        for want in [
            "--since=2026-10-23 00:50:41 UTC",
            "--until=2026-10-23 01:50:41 UTC",
            "--priority=4",
            "--unit=ssh.service",
            "--identifier=sshd",
            "--boot=-1",
            "_TRANSPORT=kernel",
        ] {
            assert!(a.iter().any(|x| x == want), "{want} missing in {a:?}");
        }
        assert!(
            !a.iter().any(|x| x == "-k"),
            "-k would pin the current boot"
        );
    }

    #[test]
    fn wildcard_identifier_and_search_scan_more_and_post_filter() {
        let r = rs(&Filters {
            identifier: Some("ssh*".into()),
            limit: Some(10),
            ..Default::default()
        });
        let a = journal_args(&r, 777);
        assert_eq!(a[5], "777");
        assert!(!a.iter().any(|x| x.starts_with("--identifier")));
        let e = |i: &str| LogEntry {
            identifier: Some(i.into()),
            message: "m".into(),
            ..Default::default()
        };
        assert!(post_match(&e("sshd"), &r));
        assert!(!post_match(&e("cron"), &r));
    }

    #[test]
    fn bad_filters_are_refused() {
        let b = |f: Filters| resolve(&f, 200, 5000, NOW, utc()).unwrap_err();
        assert!(b(Filters {
            priority: Some(9),
            ..Default::default()
        })
        .contains("priority"));
        assert!(b(Filters {
            unit: Some("--output=x".into()),
            ..Default::default()
        })
        .contains("unit"));
        assert!(b(Filters {
            boot: Some("yesterday".into()),
            ..Default::default()
        })
        .contains("boot"));
        assert!(b(Filters {
            since: Some("1h".into()),
            until: Some("2h".into()),
            ..Default::default()
        })
        .contains("later"));
        // legacy level maps to a minimum priority; explicit priority wins
        assert_eq!(
            rs(&Filters {
                level: Some("warn".into()),
                ..Default::default()
            })
            .max_priority,
            Some(4)
        );
        assert_eq!(
            rs(&Filters {
                level: Some("warn".into()),
                priority: Some(2),
                ..Default::default()
            })
            .max_priority,
            Some(2)
        );
        assert_eq!(
            rs(&Filters {
                limit: Some(999_999),
                ..Default::default()
            })
            .limit,
            5000
        );
    }

    #[test]
    fn classic_iso_and_pri_syslog_lines_parse() {
        let e = parse_syslog_line(
            "Oct  2 07:01:09 gw sshd[411]: Failed password for root",
            2026,
            utc(),
        )
        .unwrap();
        assert_eq!(e.timestamp, "2026-10-02T07:01:09");
        assert_eq!(
            (
                e.identifier.as_deref(),
                e.pid,
                e.hostname.as_deref(),
                e.level.as_str()
            ),
            (Some("sshd"), Some(411), Some("gw"), "error")
        );
        assert!(e.priority_inferred);
        assert_eq!(e.priority_name.as_deref(), Some("err"));
        let e = parse_syslog_line(
            "2026-10-22T12:00:01.123456-07:00 gw horcrux-api: started on :8006",
            2026,
            utc(),
        )
        .unwrap();
        assert_eq!(
            (e.service.as_deref(), e.level.as_str(), e.pid),
            (Some("horcrux-api"), "info", None)
        );
        assert_eq!(e.ts_us, Some(1_792_695_601_123_456));
        // a logged <PRI> wins over guessing: facility 0 = kernel, severity 4 = warning
        let e =
            parse_syslog_line("<4>Oct  2 07:01:09 gw kernel: [ 1.0] hello", 2026, utc()).unwrap();
        assert_eq!(
            (e.priority, e.priority_inferred, e.kernel),
            (Some(4), false, true)
        );
        assert!(parse_syslog_line("garbage", 2026, utc()).is_none());
    }

    #[test]
    fn a_december_stamp_read_in_january_is_last_year() {
        // 2027-01-02T00:00:00Z
        let now = 1_798_848_000_000_000;
        let e = parse_syslog_line_now("Dec 31 23:59:00 h x: y", now, utc()).unwrap();
        assert!(e.timestamp.starts_with("2026-12-31"), "{}", e.timestamp);
    }

    #[test]
    fn journal_json_keeps_priority_unit_identifier_pid_boot_host() {
        let e = parse_journal_json(
            r#"{"__REALTIME_TIMESTAMP":"1792720241123456","PRIORITY":"3","SYSLOG_IDENTIFIER":"kernel","_TRANSPORT":"kernel","_BOOT_ID":"abc","_HOSTNAME":"n1","_PID":"0","MESSAGE":"oops"}"#,
        )
        .unwrap();
        assert_eq!(
            (e.level.as_str(), e.priority, e.priority_name.as_deref()),
            ("error", Some(3), Some("err"))
        );
        assert_eq!(
            (e.boot_id.as_deref(), e.hostname.as_deref(), e.kernel),
            (Some("abc"), Some("n1"), true)
        );
        assert_eq!(e.timestamp, "2026-10-23T01:50:41.123Z");
        let b = parse_journal_json(
            r#"{"__REALTIME_TIMESTAMP":"1000000","MESSAGE":[104,105],"_SYSTEMD_UNIT":"x.service","SYSLOG_IDENTIFIER":"xd","_PID":"77","PRIORITY":"7"}"#,
        )
        .unwrap();
        assert_eq!(
            (
                b.message.as_str(),
                b.service.as_deref(),
                b.unit.as_deref(),
                b.pid,
                b.level.as_str()
            ),
            ("hi", Some("xd"), Some("x.service"), Some(77), "debug")
        );
        assert_eq!(b.details.as_deref(), Some("x.service"));
    }

    #[test]
    fn list_boots_json_and_text_number_the_newest_boot_zero() {
        let j = r#"[{"index":-1,"boot_id":"aa11","first_entry":1792000000000000,"last_entry":1792100000000000},
                    {"index":0,"boot_id":"bb22","first_entry":1792200000000000,"last_entry":1792300000000000}]"#;
        let b = parse_list_boots_json(j).unwrap();
        assert_eq!(
            (b[0].index, b[1].index, b[1].boot_id.as_str()),
            (-1, 0, "bb22")
        );
        assert!(b[0].first_entry.as_deref().unwrap().starts_with("2026-"));
        // oldest-first positive numbering (older journalctl JSON/text)
        let t = " 1 0123456789abcdef0123456789abcdef Mon 2026-10-19 10:00:00 UTC—Tue 2026-10-20 10:00:00 UTC\n 2 fedcba9876543210fedcba9876543210 Tue 2026-10-20 10:01:00 UTC—Wed 2026-10-21 10:00:00 UTC\n";
        let b = parse_list_boots_text(t);
        assert_eq!((b.len(), b[0].index, b[1].index), (2, -1, 0));
        assert!(b[1].boot_id.starts_with("fedcba"));
        assert!(b[0]
            .last_entry
            .as_deref()
            .unwrap()
            .contains("Tue 2026-10-20"));
        assert!(parse_list_boots_json("not json").is_none());
    }

    #[test]
    fn kernel_ring_records_become_kernel_entries() {
        let e = parse_kmsg("4,339,5140900,-;nf_conntrack: table full", 1_792_700_000_000_000).unwrap();
        assert_eq!((e.priority, e.kernel, e.identifier.as_deref()), (Some(4), true, Some("kernel")));
        assert_eq!(e.ts_us, Some(1_792_700_005_140_900));
        assert_eq!(e.message, "nf_conntrack: table full");
        // facility bits are dropped from the severity: 30 = daemon.info
        assert_eq!(parse_kmsg("30,1,1,-;x", 0).unwrap().priority, Some(6));
        assert!(parse_kmsg(" SUBSYSTEM=net", 0).is_none());
        assert!(parse_kmsg("garbage", 0).is_none());
    }

    #[test]
    fn wtmp_boot_records_are_found() {
        let mut f = vec![0u8; 384 * 3];
        f[0] = 2; // BOOT_TIME
        f[340..344].copy_from_slice(&1_792_000_000i32.to_le_bytes());
        f[384] = 7; // USER_PROCESS: ignored
        f[384 + 340..384 + 344].copy_from_slice(&1_792_000_100i32.to_le_bytes());
        f[768] = 2;
        f[768 + 340..768 + 344].copy_from_slice(&1_792_500_000i32.to_le_bytes());
        assert_eq!(wtmp_boot_times(&f), vec![1_792_000_000, 1_792_500_000]);
        assert!(wtmp_boot_times(&[1, 2, 3]).is_empty());
    }

    #[test]
    fn boots_are_derived_from_starts_and_entries_are_assigned() {
        let s = |x: i64| x * 1_000_000;
        let boots = boots_from_starts(
            vec![s(2000), s(1000), s(1050), s(3000)],
            Some(s(500)),
            s(4000),
        );
        // 1000 and 1050 are one boot; an earlier unknown one precedes
        assert_eq!(boots.len(), 4);
        assert_eq!(boots[0].boot_id, "boot-unknown");
        assert_eq!(
            (boots[3].index, boots[2].index, boots[1].index),
            (0, -1, -2)
        );
        assert_eq!(boot_of(s(1500), &boots).unwrap().boot_id, "boot-1000");
        assert_eq!(boot_of(s(3500), &boots).unwrap().index, 0);
        assert_eq!(boot_of(s(700), &boots).unwrap().boot_id, "boot-unknown");
        assert_eq!(resolve_boot("-1", &boots).as_deref(), Some("boot-2000"));
        assert_eq!(
            resolve_boot("boot-1000", &boots).as_deref(),
            Some("boot-1000")
        );
        assert!(resolve_boot("-9", &boots).is_none());
    }

    #[test]
    fn file_entries_obey_every_filter() {
        let mk = |ts: i64, id: &str, pr: u8, k: bool, boot: &str| LogEntry {
            ts_us: Some(ts * 1_000_000),
            identifier: Some(id.into()),
            service: Some(id.into()),
            priority: Some(pr),
            kernel: k,
            boot_id: Some(boot.into()),
            message: "m".into(),
            ..Default::default()
        };
        let boots = boots_from_starts(vec![1_000_000_000, 2_000_000_000], None, 3_000_000_000);
        let f = |fl: Filters, e: &LogEntry| file_match(e, &rs(&fl), &boots);
        let e = mk(1500, "sshd", 3, false, "boot-1000");
        assert!(f(Filters::default(), &e));
        assert!(!f(
            Filters {
                priority: Some(2),
                ..Default::default()
            },
            &e
        ));
        assert!(f(
            Filters {
                unit: Some("sshd.service".into()),
                ..Default::default()
            },
            &e
        ));
        assert!(!f(
            Filters {
                unit: Some("cron".into()),
                ..Default::default()
            },
            &e
        ));
        assert!(!f(
            Filters {
                kernel: Some(true),
                ..Default::default()
            },
            &e
        ));
        assert!(f(
            Filters {
                kernel: Some(true),
                ..Default::default()
            },
            &mk(1, "kernel", 6, true, "x")
        ));
        assert!(!f(
            Filters {
                since: Some("1792720300".into()),
                ..Default::default()
            },
            &e
        ));
        let b = mk(1100, "x", 6, false, "boot-1000");
        assert!(f(
            Filters {
                boot: Some("-1".into()),
                ..Default::default()
            },
            &b
        ));
        assert!(!f(
            Filters {
                boot: Some("0".into()),
                ..Default::default()
            },
            &b
        ));
    }
}
