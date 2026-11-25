//! System log viewer node settings (`GET/PUT /api/system/logs/settings`,
//! declarative section `logs`). Shared by the API and the web UI.
//!
//! Every operational value of the viewer is a key here with a documented
//! default: nothing about line limits, scan bounds, timeouts, file
//! locations or the live stream is hard-coded in the handlers.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct LogSettings {
    /// Lines returned when a request names no `limit`.
    pub default_lines: usize,
    /// Upper bound for one request's `limit` (and for an export).
    pub max_lines: usize,
    /// How many journal/log lines one request may scan while looking for
    /// `max_lines` matches (bounds work for rare filters and wildcards).
    pub max_scan_lines: usize,
    /// Seconds a journalctl run or a peer node may take to answer.
    pub query_timeout_secs: u64,
    /// How much of the end of a syslog file is read (bytes).
    pub syslog_tail_bytes: u64,
    /// Syslog files tried in order when journald is not running.
    pub syslog_files: Vec<String>,
    /// Login records used to find reboots on hosts without journald boots.
    pub wtmp_path: String,
    /// Live mode: milliseconds between two looks for new entries.
    pub follow_poll_ms: u64,
    /// Live mode: the stream closes after this many seconds without a new
    /// matching entry.
    pub follow_idle_secs: u64,
    /// Live mode: longest a single stream stays open (seconds).
    pub follow_max_secs: u64,
    /// Live mode: entries one stream may send before it closes.
    pub follow_max_entries: usize,
    /// Live mode: streams open at once on this node.
    pub follow_max_streams: usize,
}

impl Default for LogSettings {
    fn default() -> Self {
        Self {
            default_lines: 200,
            max_lines: 5000,
            max_scan_lines: 200_000,
            query_timeout_secs: 30,
            syslog_tail_bytes: 8 * 1024 * 1024,
            syslog_files: vec![
                "/var/log/messages".into(),
                "/var/log/syslog".into(),
                "/var/log/everything/current".into(),
            ],
            wtmp_path: "/var/log/wtmp".into(),
            follow_poll_ms: 1000,
            follow_idle_secs: 600,
            follow_max_secs: 3600,
            follow_max_entries: 20_000,
            follow_max_streams: 8,
        }
    }
}

impl LogSettings {
    pub fn validate(&self) -> Result<(), String> {
        fn range<T: PartialOrd + std::fmt::Display>(
            name: &str,
            v: T,
            lo: T,
            hi: T,
        ) -> Result<(), String> {
            if v < lo || v > hi {
                Err(format!("{name} must be between {lo} and {hi} (got {v})"))
            } else {
                Ok(())
            }
        }
        range("max_lines", self.max_lines, 100, 100_000)?;
        range("default_lines", self.default_lines, 1, self.max_lines)?;
        range("max_scan_lines", self.max_scan_lines, 1000, 5_000_000)?;
        range("query_timeout_secs", self.query_timeout_secs, 1, 600)?;
        range(
            "syslog_tail_bytes",
            self.syslog_tail_bytes,
            64 * 1024,
            512 * 1024 * 1024,
        )?;
        range("follow_poll_ms", self.follow_poll_ms, 200, 60_000)?;
        range("follow_idle_secs", self.follow_idle_secs, 10, 86_400)?;
        range("follow_max_secs", self.follow_max_secs, 30, 86_400)?;
        range(
            "follow_max_entries",
            self.follow_max_entries,
            100,
            1_000_000,
        )?;
        range("follow_max_streams", self.follow_max_streams, 1, 64)?;
        if self.syslog_files.is_empty() {
            return Err("syslog_files must name at least one file".into());
        }
        for p in self.syslog_files.iter().chain([&self.wtmp_path]) {
            if !p.starts_with('/') || p.contains("..") {
                return Err(format!("{p:?} must be an absolute path without `..`"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate_and_bad_values_are_named() {
        assert!(LogSettings::default().validate().is_ok());
        let s = LogSettings {
            max_lines: 10,
            ..Default::default()
        };
        assert!(s.validate().unwrap_err().contains("max_lines"));
        let s = LogSettings {
            syslog_files: vec!["relative".into()],
            ..Default::default()
        };
        assert!(s.validate().unwrap_err().contains("absolute"));
        let s = LogSettings {
            default_lines: 9000,
            ..Default::default()
        };
        assert!(s.validate().unwrap_err().contains("default_lines"));
    }

    #[test]
    fn a_partial_document_takes_the_defaults() {
        let s: LogSettings = serde_json::from_str(r#"{"max_lines": 1000}"#).unwrap();
        assert_eq!(s.max_lines, 1000);
        assert_eq!(s.follow_idle_secs, 600);
    }
}
