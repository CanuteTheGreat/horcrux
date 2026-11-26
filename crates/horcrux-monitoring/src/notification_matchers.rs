//! Notification matchers (Monitoring > Notifications > Matchers).
//!
//! A matcher is a typed tree of `all` / `any` / `not` groups whose leaves test
//! severity, event type, a message field (regex) or a calendar window
//! (weekdays plus start/end time at a configured UTC offset). A matcher that
//! evaluates true routes the message to its `targets` (channel ids). With no
//! enabled matcher the store keeps the old behaviour: every enabled channel
//! gets every message.

use crate::notification_channels::Message;
use chrono::{DateTime, Datelike, Duration, NaiveTime, Timelike, Utc};
use serde::{Deserialize, Serialize};

const MAX_DEPTH: usize = 16;
const MAX_NODES: usize = 256;

/// One node of a matcher tree.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum MatchNode {
    /// True when every child is true (an empty group is true).
    All {
        #[serde(default)]
        children: Vec<MatchNode>,
    },
    /// True when at least one child is true (an empty group is false).
    Any {
        #[serde(default)]
        children: Vec<MatchNode>,
    },
    /// Negates its child.
    Not { child: Box<MatchNode> },
    /// `min`: info / warning / critical, matches that severity and above.
    /// `any_of`: exact severities. Both may be given (both must hold).
    Severity {
        #[serde(default)]
        min: Option<String>,
        #[serde(default)]
        any_of: Vec<String>,
    },
    /// Event type: alert, alert_test, backup, update, ha, replication, ...
    Event {
        #[serde(default)]
        any_of: Vec<String>,
    },
    /// Regex (unanchored) on one of: title, body, severity, event, rule, target.
    Field { field: String, regex: String },
    /// Weekdays (0 = Monday .. 6 = Sunday; empty = every day) and an optional
    /// `start`..`end` time of day (`HH:MM`, a window with start > end wraps
    /// midnight) evaluated at `utc_offset_minutes` from UTC.
    Calendar {
        #[serde(default)]
        weekdays: Vec<u8>,
        #[serde(default)]
        start: Option<String>,
        #[serde(default)]
        end: Option<String>,
        #[serde(default)]
        utc_offset_minutes: i32,
    },
}

/// A named routing rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Matcher {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub rule: MatchNode,
    /// Channel ids that receive a matching message.
    pub targets: Vec<String>,
}

fn yes() -> bool {
    true
}

/// A message as the matchers see it (also the body of the "test" endpoint).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SampleEvent {
    #[serde(default)]
    pub severity: String,
    #[serde(default)]
    pub event: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub rule: String,
    #[serde(default)]
    pub target: String,
}

impl SampleEvent {
    pub fn from_message(m: &Message) -> Self {
        Self {
            severity: m.severity.clone(),
            event: m.event.clone(),
            title: m.title.clone(),
            body: m.body.clone(),
            rule: m
                .alert
                .as_ref()
                .map(|a| a.rule_name.clone())
                .unwrap_or_default(),
            target: m
                .alert
                .as_ref()
                .map(|a| a.target.clone())
                .unwrap_or_default(),
        }
    }

    fn field(&self, name: &str) -> Option<&str> {
        Some(match name {
            "title" => &self.title,
            "body" => &self.body,
            "severity" => &self.severity,
            "event" => &self.event,
            "rule" => &self.rule,
            "target" => &self.target,
            _ => return None,
        })
    }
}

fn severity_rank(s: &str) -> Option<u8> {
    match s.to_ascii_lowercase().as_str() {
        "info" => Some(0),
        "warning" => Some(1),
        "critical" => Some(2),
        _ => None,
    }
}

fn parse_time(s: &str) -> Result<NaiveTime, String> {
    NaiveTime::parse_from_str(s, "%H:%M")
        .map_err(|_| format!("'{}' is not a time of day (use HH:MM)", s))
}

/// Check a tree: known severities and fields, compilable regexes, valid times,
/// bounded depth and size.
pub fn validate_node(node: &MatchNode) -> Result<(), String> {
    fn walk(n: &MatchNode, depth: usize, count: &mut usize) -> Result<(), String> {
        *count += 1;
        if depth > MAX_DEPTH {
            return Err(format!("matcher nesting is deeper than {}", MAX_DEPTH));
        }
        if *count > MAX_NODES {
            return Err(format!("matcher has more than {} nodes", MAX_NODES));
        }
        match n {
            MatchNode::All { children } | MatchNode::Any { children } => {
                for c in children {
                    walk(c, depth + 1, count)?;
                }
            }
            MatchNode::Not { child } => walk(child, depth + 1, count)?,
            MatchNode::Severity { min, any_of } => {
                if min.is_none() && any_of.is_empty() {
                    return Err("severity needs 'min' or 'any_of'".into());
                }
                for s in min.iter().chain(any_of.iter()) {
                    if severity_rank(s).is_none() {
                        return Err(format!(
                            "unknown severity '{}' (info, warning, critical)",
                            s
                        ));
                    }
                }
            }
            MatchNode::Event { any_of } => {
                if any_of.is_empty() || any_of.iter().any(|e| e.trim().is_empty()) {
                    return Err("event needs at least one non-empty event type".into());
                }
            }
            MatchNode::Field { field, regex } => {
                if SampleEvent::default().field(field).is_none() {
                    return Err(format!(
                        "unknown field '{}' (title, body, severity, event, rule, target)",
                        field
                    ));
                }
                regex::Regex::new(regex)
                    .map_err(|e| format!("invalid regex '{}': {}", regex, e))?;
            }
            MatchNode::Calendar {
                weekdays,
                start,
                end,
                utc_offset_minutes,
            } => {
                if weekdays.iter().any(|d| *d > 6) {
                    return Err("weekdays are 0 (Monday) to 6 (Sunday)".into());
                }
                if start.is_some() != end.is_some() {
                    return Err("calendar needs both start and end, or neither".into());
                }
                if let (Some(s), Some(e)) = (start, end) {
                    parse_time(s)?;
                    parse_time(e)?;
                }
                if utc_offset_minutes.abs() > 14 * 60 {
                    return Err("utc_offset_minutes must be within +-840".into());
                }
            }
        }
        Ok(())
    }
    walk(node, 1, &mut 0)
}

/// Evaluate a tree against an event at time `now`. Invalid leaves (which
/// `validate_node` rejects on save) evaluate false.
pub fn evaluate(node: &MatchNode, ev: &SampleEvent, now: DateTime<Utc>) -> bool {
    match node {
        MatchNode::All { children } => children.iter().all(|c| evaluate(c, ev, now)),
        MatchNode::Any { children } => children.iter().any(|c| evaluate(c, ev, now)),
        MatchNode::Not { child } => !evaluate(child, ev, now),
        MatchNode::Severity { min, any_of } => {
            let got = severity_rank(&ev.severity);
            let min_ok = match min {
                Some(m) => matches!((got, severity_rank(m)), (Some(g), Some(m)) if g >= m),
                None => true,
            };
            let set_ok =
                any_of.is_empty() || any_of.iter().any(|s| s.eq_ignore_ascii_case(&ev.severity));
            min_ok && set_ok
        }
        MatchNode::Event { any_of } => any_of.iter().any(|e| e.eq_ignore_ascii_case(&ev.event)),
        MatchNode::Field { field, regex } => match (ev.field(field), regex::Regex::new(regex)) {
            (Some(v), Ok(re)) => re.is_match(v),
            _ => false,
        },
        MatchNode::Calendar {
            weekdays,
            start,
            end,
            utc_offset_minutes,
        } => {
            let local = now + Duration::minutes(*utc_offset_minutes as i64);
            if !weekdays.is_empty()
                && !weekdays.contains(&(local.weekday().num_days_from_monday() as u8))
            {
                return false;
            }
            match (start, end) {
                (Some(s), Some(e)) => match (parse_time(s), parse_time(e)) {
                    (Ok(s), Ok(e)) => {
                        let t = NaiveTime::from_hms_opt(local.hour(), local.minute(), 0)
                            .unwrap_or_default();
                        if s <= e {
                            t >= s && t < e
                        } else {
                            t >= s || t < e
                        }
                    }
                    _ => false,
                },
                _ => true,
            }
        }
    }
}

/// Channel ids that should receive `ev`: the union of the targets of every
/// enabled matcher whose tree is true. `None` when no matcher is enabled
/// (caller keeps the send-to-all behaviour).
pub fn route(matchers: &[Matcher], ev: &SampleEvent, now: DateTime<Utc>) -> Option<Vec<String>> {
    let enabled: Vec<&Matcher> = matchers.iter().filter(|m| m.enabled).collect();
    if enabled.is_empty() {
        return None;
    }
    let mut out: Vec<String> = Vec::new();
    for m in enabled {
        if evaluate(&m.rule, ev, now) {
            for t in &m.targets {
                if !out.contains(t) {
                    out.push(t.clone());
                }
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ev(sev: &str, event: &str, target: &str) -> SampleEvent {
        SampleEvent {
            severity: sev.into(),
            event: event.into(),
            title: format!("[{}] cpu - {}", sev, target),
            body: "high".into(),
            rule: "cpu".into(),
            target: target.into(),
        }
    }

    // 2026-11-23 is a Monday.
    fn at(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 23, h, m, 0).unwrap()
    }

    fn sev_min(m: &str) -> MatchNode {
        MatchNode::Severity {
            min: Some(m.into()),
            any_of: vec![],
        }
    }

    #[test]
    fn severity_min_and_set() {
        assert!(evaluate(
            &sev_min("warning"),
            &ev("critical", "alert", "a"),
            at(1, 0)
        ));
        assert!(evaluate(
            &sev_min("warning"),
            &ev("warning", "alert", "a"),
            at(1, 0)
        ));
        assert!(!evaluate(
            &sev_min("warning"),
            &ev("info", "alert", "a"),
            at(1, 0)
        ));
        let only = MatchNode::Severity {
            min: None,
            any_of: vec!["info".into()],
        };
        assert!(evaluate(&only, &ev("info", "x", "a"), at(1, 0)));
        assert!(!evaluate(&only, &ev("critical", "x", "a"), at(1, 0)));
    }

    #[test]
    fn nested_and_or_not() {
        // critical AND (event backup OR target ~ ^db) AND NOT target ~ test
        let tree = MatchNode::All {
            children: vec![
                sev_min("critical"),
                MatchNode::Any {
                    children: vec![
                        MatchNode::Event {
                            any_of: vec!["backup".into()],
                        },
                        MatchNode::Field {
                            field: "target".into(),
                            regex: "^db".into(),
                        },
                    ],
                },
                MatchNode::Not {
                    child: Box::new(MatchNode::Field {
                        field: "target".into(),
                        regex: "test".into(),
                    }),
                },
            ],
        };
        validate_node(&tree).unwrap();
        let n = at(12, 0);
        assert!(evaluate(&tree, &ev("critical", "alert", "db1"), n));
        assert!(evaluate(&tree, &ev("critical", "backup", "web"), n));
        assert!(!evaluate(&tree, &ev("critical", "alert", "web"), n));
        assert!(!evaluate(&tree, &ev("critical", "alert", "db-test"), n));
        assert!(!evaluate(&tree, &ev("warning", "alert", "db1"), n));
    }

    #[test]
    fn empty_groups() {
        let n = at(0, 0);
        let e = ev("info", "x", "a");
        assert!(evaluate(&MatchNode::All { children: vec![] }, &e, n));
        assert!(!evaluate(&MatchNode::Any { children: vec![] }, &e, n));
    }

    #[test]
    fn calendar_weekday_window_and_wrap() {
        let work = MatchNode::Calendar {
            weekdays: vec![0, 1, 2, 3, 4],
            start: Some("09:00".into()),
            end: Some("17:00".into()),
            utc_offset_minutes: 0,
        };
        let e = ev("info", "x", "a");
        assert!(evaluate(&work, &e, at(9, 0)));
        assert!(evaluate(&work, &e, at(16, 59)));
        assert!(!evaluate(&work, &e, at(17, 0)));
        assert!(!evaluate(&work, &e, at(8, 59)));
        // Sunday 2026-11-29 is outside the weekdays.
        let sun = Utc.with_ymd_and_hms(2026, 11, 29, 12, 0, 0).unwrap();
        assert!(!evaluate(&work, &e, sun));
        // Night window 22:00..06:00 wraps midnight.
        let night = MatchNode::Calendar {
            weekdays: vec![],
            start: Some("22:00".into()),
            end: Some("06:00".into()),
            utc_offset_minutes: 0,
        };
        assert!(evaluate(&night, &e, at(23, 30)));
        assert!(evaluate(&night, &e, at(5, 59)));
        assert!(!evaluate(&night, &e, at(6, 0)));
        assert!(!evaluate(&night, &e, at(12, 0)));
        // UTC-8: 17:00 UTC is 09:00 local, inside the 09:00..17:00 window.
        let local = MatchNode::Calendar {
            weekdays: vec![0],
            start: Some("09:00".into()),
            end: Some("17:00".into()),
            utc_offset_minutes: -480,
        };
        assert!(evaluate(&local, &e, at(17, 0)));
        assert!(!evaluate(&local, &e, at(9, 0)));
    }

    #[test]
    fn validation_rejects_bad_trees() {
        assert!(validate_node(&sev_min("bogus")).is_err());
        assert!(
            validate_node(&MatchNode::Field {
                field: "nope".into(),
                regex: "x".into()
            })
            .is_err()
        );
        assert!(
            validate_node(&MatchNode::Field {
                field: "body".into(),
                regex: "(".into()
            })
            .is_err()
        );
        assert!(
            validate_node(&MatchNode::Calendar {
                weekdays: vec![7],
                start: None,
                end: None,
                utc_offset_minutes: 0
            })
            .is_err()
        );
        assert!(
            validate_node(&MatchNode::Calendar {
                weekdays: vec![],
                start: Some("25:00".into()),
                end: Some("06:00".into()),
                utc_offset_minutes: 0
            })
            .is_err()
        );
        assert!(validate_node(&MatchNode::Event { any_of: vec![] }).is_err());
        let mut deep = MatchNode::All { children: vec![] };
        for _ in 0..20 {
            deep = MatchNode::Not {
                child: Box::new(deep),
            };
        }
        assert!(validate_node(&deep).is_err());
    }

    #[test]
    fn routing_unions_targets_and_defaults_to_all() {
        let m = |name: &str, enabled: bool, rule: MatchNode, t: &[&str]| Matcher {
            id: name.into(),
            name: name.into(),
            enabled,
            rule,
            targets: t.iter().map(|s| s.to_string()).collect(),
        };
        let e = ev("critical", "alert", "db1");
        assert_eq!(route(&[], &e, at(1, 0)), None);
        let ms = vec![
            m("page", true, sev_min("critical"), &["pd", "mail"]),
            m(
                "all",
                true,
                MatchNode::All { children: vec![] },
                &["mail", "slack"],
            ),
            m(
                "off",
                false,
                MatchNode::All { children: vec![] },
                &["never"],
            ),
            m(
                "info",
                true,
                MatchNode::Event {
                    any_of: vec!["backup".into()],
                },
                &["backup"],
            ),
        ];
        assert_eq!(
            route(&ms, &e, at(1, 0)),
            Some(vec!["pd".to_string(), "mail".into(), "slack".into()])
        );
        // Only disabled matchers = old behaviour.
        assert_eq!(route(&ms[2..3], &e, at(1, 0)), None);
        // Enabled matchers, none matching: nothing is sent.
        assert_eq!(route(&ms[3..], &e, at(1, 0)), Some(vec![]));
    }

    #[test]
    fn json_round_trip() {
        let j = r#"{"name":"n","rule":{"op":"all","children":[{"op":"severity","min":"warning"},{"op":"not","child":{"op":"event","any_of":["test"]}}]},"targets":["c1"]}"#;
        let m: Matcher = serde_json::from_str(j).unwrap();
        assert!(m.enabled);
        validate_node(&m.rule).unwrap();
        let back = serde_json::to_string(&m).unwrap();
        let again: Matcher = serde_json::from_str(&back).unwrap();
        assert_eq!(again.rule, m.rule);
    }
}
