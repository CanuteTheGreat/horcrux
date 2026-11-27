//! Security events dashboard, brute-force detection and alert rules
//! (docs/design/security-suite.md row 3, user guide docs/SECURITY-SUITE.md).
//!
//! Types are shared by the API and the web UI. Every operational value is a
//! setting with a documented default (`security.detection` in the node
//! settings and the declarative `security:` section).
use serde::{Deserialize, Serialize};

/// Event kinds the dashboard shows and the alert rules can count.
pub const EVENT_KINDS: [&str; 14] = [
    "login_failed",
    "lockout",
    "access_denied",
    "rate_limited",
    "not_found",
    "server_error",
    "permission_denied",
    "auth_change",
    "policy_change",
    "brute_force",
    "suspicious",
    "rule_fired",
    "blocked",
    "unblocked",
];

/// Brute-force detection: `failures` failed logins from one client within
/// `window_minutes` raise a `security` notification and, when `block` is
/// on, drop that client's traffic to this host for `block_minutes`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct BruteForceSettings {
    pub enabled: bool,
    /// N: failed logins that trigger the detector.
    pub failures: u32,
    /// M: the sliding window in minutes.
    pub window_minutes: u32,
    pub notify: bool,
    /// Temporarily block the client in the host firewall (off by default).
    pub block: bool,
    pub block_minutes: u32,
    /// Addresses or CIDR ranges that are never blocked and never counted.
    pub allowlist: Vec<String>,
}

impl Default for BruteForceSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            failures: 5,
            window_minutes: 10,
            notify: true,
            block: false,
            block_minutes: 30,
            allowlist: Vec::new(),
        }
    }
}

/// One alert rule over the security events: `threshold` events of the given
/// kinds (optionally one API path group) within `window_minutes`, counted
/// per client, per user or in total.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct EventRule {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub kinds: Vec<String>,
    /// Only API requests of this path group (`vms`, `storage`, ...); empty = any.
    pub path_group: String,
    /// `client`, `user` or `none` (one total).
    pub group_by: String,
    pub threshold: u32,
    pub window_minutes: u32,
    /// info / warning / critical.
    pub severity: String,
    /// Minutes before the same rule fires again for the same client/user.
    pub cooldown_minutes: u32,
}

impl Default for EventRule {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            enabled: true,
            kinds: Vec::new(),
            path_group: String::new(),
            group_by: "client".into(),
            threshold: 10,
            window_minutes: 5,
            severity: "warning".into(),
            cooldown_minutes: 30,
        }
    }
}

fn plain_word(s: &str, max: usize) -> bool {
    s.len() <= max
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

impl EventRule {
    pub fn validate(&self) -> Result<(), String> {
        let id = &self.id;
        if id.is_empty() || !plain_word(id, 48) {
            return Err("detection.rules[].id: 1-48 letters, digits, '-' or '_'".into());
        }
        if self.name.trim().is_empty() || self.name.len() > 100 {
            return Err(format!("detection.rules.{id}.name: 1-100 characters"));
        }
        if self.kinds.is_empty() {
            return Err(format!("detection.rules.{id}.kinds: at least one kind"));
        }
        for k in &self.kinds {
            if !EVENT_KINDS.contains(&k.as_str()) {
                return Err(format!(
                    "detection.rules.{id}.kinds: unknown kind '{k}' (one of {})",
                    EVENT_KINDS.join(", ")
                ));
            }
        }
        if !plain_word(&self.path_group, 48) {
            return Err(format!(
                "detection.rules.{id}.path_group: a route group name"
            ));
        }
        if !matches!(self.group_by.as_str(), "client" | "user" | "none") {
            return Err(format!(
                "detection.rules.{id}.group_by: client, user or none"
            ));
        }
        if !(1..=100_000).contains(&self.threshold) {
            return Err(format!("detection.rules.{id}.threshold: 1 to 100000"));
        }
        if !(1..=1440).contains(&self.window_minutes) {
            return Err(format!("detection.rules.{id}.window_minutes: 1 to 1440"));
        }
        if self.cooldown_minutes > 10_080 {
            return Err(format!("detection.rules.{id}.cooldown_minutes: 0 to 10080"));
        }
        if !matches!(self.severity.as_str(), "info" | "warning" | "critical") {
            return Err(format!(
                "detection.rules.{id}.severity: info, warning or critical"
            ));
        }
        Ok(())
    }
}

fn default_event_rules() -> Vec<EventRule> {
    vec![
        EventRule {
            id: "access-denied-burst".into(),
            name: "Many denied API requests from one client".into(),
            kinds: vec!["access_denied".into()],
            threshold: 30,
            window_minutes: 5,
            ..EventRule::default()
        },
        EventRule {
            id: "path-scan".into(),
            name: "Many unknown API paths from one client".into(),
            kinds: vec!["not_found".into()],
            threshold: 100,
            window_minutes: 5,
            cooldown_minutes: 60,
            ..EventRule::default()
        },
        EventRule {
            id: "permission-denials".into(),
            name: "One user keeps hitting permission denials".into(),
            kinds: vec!["permission_denied".into()],
            group_by: "user".into(),
            threshold: 10,
            window_minutes: 10,
            ..EventRule::default()
        },
    ]
}

/// Security events settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct DetectionSettings {
    /// Run the detector and the alert rules.
    pub enabled: bool,
    /// Seconds between detector passes.
    pub check_secs: u32,
    pub brute_force: BruteForceSettings,
    pub rules: Vec<EventRule>,
    /// Detector, scan and baseline events kept on disk.
    pub journal_max_entries: u32,
    /// nftables table that holds the temporary blocks.
    pub block_table: String,
}

impl Default for DetectionSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            check_secs: 30,
            brute_force: BruteForceSettings::default(),
            rules: default_event_rules(),
            journal_max_entries: 5000,
            block_table: "horcrux_secblock".into(),
        }
    }
}

/// `1.2.3.4`, `2001:db8::1` or a CIDR range of either.
pub fn parse_ip_or_cidr(s: &str) -> Option<(std::net::IpAddr, u8)> {
    let s = s.trim();
    let (addr, bits) = match s.split_once('/') {
        Some((a, b)) => (a, Some(b.parse::<u8>().ok()?)),
        None => (s, None),
    };
    let ip = normalize_ip(addr.parse().ok()?);
    let max = if ip.is_ipv4() { 32 } else { 128 };
    let bits = bits.unwrap_or(max);
    (bits <= max).then_some((ip, bits))
}

/// An IPv4-mapped IPv6 address as plain IPv4.
pub fn normalize_ip(ip: std::net::IpAddr) -> std::net::IpAddr {
    match ip {
        std::net::IpAddr::V6(v6) => v6.to_ipv4_mapped().map(std::net::IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    }
}

/// Is `ip` inside the address or range `net`?
pub fn ip_in(net: (std::net::IpAddr, u8), ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr::{V4, V6};
    let bits = net.1 as u32;
    match (normalize_ip(net.0), normalize_ip(ip)) {
        (V4(a), V4(b)) => {
            let mask = if bits == 0 {
                0
            } else {
                u32::MAX << (32 - bits.min(32))
            };
            u32::from(a) & mask == u32::from(b) & mask
        }
        (V6(a), V6(b)) => {
            let mask = if bits == 0 {
                0
            } else {
                u128::MAX << (128 - bits.min(128))
            };
            u128::from(a) & mask == u128::from(b) & mask
        }
        _ => false,
    }
}

impl DetectionSettings {
    pub fn validate(&self) -> Result<(), String> {
        if !(10..=3600).contains(&self.check_secs) {
            return Err("detection.check_secs: 10 to 3600".into());
        }
        if !(100..=100_000).contains(&self.journal_max_entries) {
            return Err("detection.journal_max_entries: 100 to 100000".into());
        }
        let t = &self.block_table;
        if t.is_empty()
            || !plain_word(t, 32)
            || t.contains('-')
            || t.starts_with(|c: char| c.is_ascii_digit())
        {
            return Err("detection.block_table: letters, digits and '_' (up to 32)".into());
        }
        let b = &self.brute_force;
        if !(1..=1000).contains(&b.failures) {
            return Err("detection.brute_force.failures: 1 to 1000".into());
        }
        if !(1..=1440).contains(&b.window_minutes) {
            return Err("detection.brute_force.window_minutes: 1 to 1440".into());
        }
        if !(1..=10_080).contains(&b.block_minutes) {
            return Err("detection.brute_force.block_minutes: 1 to 10080".into());
        }
        if b.allowlist.len() > 200 {
            return Err("detection.brute_force.allowlist: at most 200 entries".into());
        }
        for a in &b.allowlist {
            if parse_ip_or_cidr(a).is_none() {
                return Err(format!(
                    "detection.brute_force.allowlist: '{a}' is not an address or CIDR range"
                ));
            }
        }
        if self.rules.len() > 50 {
            return Err("detection.rules: at most 50 rules".into());
        }
        let mut seen = std::collections::BTreeSet::new();
        for r in &self.rules {
            r.validate()?;
            if !seen.insert(r.id.clone()) {
                return Err(format!("detection.rules: duplicate id '{}'", r.id));
            }
        }
        Ok(())
    }
}

/// One line of the security timeline.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TimelineEvent {
    /// Unix seconds.
    pub ts: i64,
    /// `audit`, `access`, `detector`, `scan` or `hardening`.
    pub source: String,
    pub kind: String,
    /// info / warning / critical.
    pub severity: String,
    #[serde(default)]
    pub client: Option<String>,
    #[serde(default)]
    pub user: Option<String>,
    pub summary: String,
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ActivityTotals {
    pub failed_logins: u32,
    pub lockouts: u32,
    pub access_denied: u32,
    pub permission_denied: u32,
    pub brute_force: u32,
    pub blocks_active: u32,
    pub scan_findings: u32,
    pub baseline_drift: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventBucket {
    pub start: i64,
    pub failed_logins: u32,
    pub access_denied: u32,
    pub other: u32,
}

/// A client or user that keeps failing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TopFailing {
    pub key: String,
    pub failed_logins: u32,
    pub access_denied: u32,
    pub total: u32,
    pub last_seen: i64,
    pub blocked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActivityDashboard {
    pub since: i64,
    pub until: i64,
    pub bucket_secs: i64,
    pub totals: ActivityTotals,
    pub timeline: Vec<EventBucket>,
    pub top_clients: Vec<TopFailing>,
    pub top_users: Vec<TopFailing>,
    pub recent: Vec<TimelineEvent>,
    pub detector_enabled: bool,
    pub block_enabled: bool,
    pub firewall_available: bool,
    pub access_log_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TimelinePage {
    pub total: usize,
    pub events: Vec<TimelineEvent>,
}

/// A client currently dropped by the host firewall.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlockedClient {
    pub ip: String,
    /// Unix seconds.
    pub since: i64,
    pub until: i64,
    pub reason: String,
    pub by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlockRequest {
    pub ip: String,
    /// Minutes; 0 = the configured `block_minutes`.
    #[serde(default)]
    pub minutes: u32,
    #[serde(default)]
    pub reason: String,
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate_and_block_is_off() {
        let s = DetectionSettings::default();
        s.validate().unwrap();
        assert!(!s.brute_force.block, "blocking is opt-in");
        assert!(s.brute_force.enabled);
        assert_eq!(
            (s.brute_force.failures, s.brute_force.window_minutes),
            (5, 10)
        );
        assert_eq!(s.rules.len(), 3);
    }

    #[test]
    fn bad_values_are_refused() {
        let mut s = DetectionSettings::default();
        s.brute_force.failures = 0;
        assert!(s.validate().is_err());
        let mut s = DetectionSettings::default();
        s.brute_force.allowlist = vec!["not-an-ip".into()];
        assert!(s.validate().is_err());
        let mut s = DetectionSettings::default();
        s.rules[0].kinds = vec!["bogus".into()];
        assert!(s.validate().unwrap_err().contains("unknown kind"));
        let mut s = DetectionSettings::default();
        s.rules[1].id = s.rules[0].id.clone();
        assert!(s.validate().unwrap_err().contains("duplicate"));
        let mut s = DetectionSettings::default();
        s.block_table = "bad name; flush ruleset".into();
        assert!(s.validate().is_err());
        let mut s = DetectionSettings::default();
        s.rules[0].group_by = "everyone".into();
        assert!(s.validate().is_err());
    }

    #[test]
    fn partial_json_fills_defaults() {
        let s: DetectionSettings =
            serde_json::from_str(r#"{"brute_force":{"failures":3,"block":true}}"#).unwrap();
        assert_eq!(s.brute_force.failures, 3);
        assert!(s.brute_force.block);
        assert_eq!(s.brute_force.window_minutes, 10);
        assert_eq!(s.check_secs, 30);
        s.validate().unwrap();
    }

    #[test]
    fn address_ranges() {
        let net = parse_ip_or_cidr("10.1.0.0/16").unwrap();
        assert!(ip_in(net, "10.1.200.3".parse().unwrap()));
        assert!(!ip_in(net, "10.2.0.1".parse().unwrap()));
        assert!(ip_in(net, "::ffff:10.1.2.3".parse().unwrap()));
        let v6 = parse_ip_or_cidr("2001:db8::/32").unwrap();
        assert!(ip_in(v6, "2001:db8:1::5".parse().unwrap()));
        assert!(!ip_in(v6, "2001:db9::1".parse().unwrap()));
        assert!(!ip_in(v6, "10.1.2.3".parse().unwrap()));
        assert!(parse_ip_or_cidr("10.0.0.1/33").is_none());
        let one = parse_ip_or_cidr("192.0.2.7").unwrap();
        assert!(ip_in(one, "192.0.2.7".parse().unwrap()));
        assert!(!ip_in(one, "192.0.2.8".parse().unwrap()));
        let all = parse_ip_or_cidr("0.0.0.0/0").unwrap();
        assert!(ip_in(all, "8.8.8.8".parse().unwrap()));
    }
}
