//! Notification routing: evaluate matcher trees against events, validate
//! them and persist them. See `horcrux_common::notify` for the model.

use crate::notification_channels::{NotificationChannelStore, StoreError};
use chrono::{DateTime, Datelike, Timelike, Utc};
use horcrux_common::notify::{
    Event, MatchMode, MatchNode, Matcher, MatcherVerdict, NodeTrace, TargetRef, TestResult, Weekday,
};
use sqlx::Row;

/// Deepest nesting of groups a matcher may have (typed limit, documented
/// in docs/NOTIFICATIONS.md).
pub const MAX_DEPTH: usize = 16;
/// Most nodes (groups and leaves) in one matcher.
pub const MAX_NODES: usize = 500;

fn parse_hhmm(s: &str, allow_24: bool) -> Result<u32, String> {
    let (h, m) = s
        .split_once(':')
        .ok_or_else(|| format!("'{s}' is not a time of day (use HH:MM)"))?;
    let h: u32 = h
        .parse()
        .map_err(|_| format!("'{s}' is not a time of day (use HH:MM)"))?;
    let m: u32 = m
        .parse()
        .map_err(|_| format!("'{s}' is not a time of day (use HH:MM)"))?;
    if m > 59 || h > 24 || (h == 24 && (m != 0 || !allow_24)) {
        return Err(format!("'{s}' is not a valid time of day"));
    }
    Ok(h * 60 + m)
}

fn weekday_of(w: chrono::Weekday) -> Weekday {
    match w {
        chrono::Weekday::Mon => Weekday::Mon,
        chrono::Weekday::Tue => Weekday::Tue,
        chrono::Weekday::Wed => Weekday::Wed,
        chrono::Weekday::Thu => Weekday::Thu,
        chrono::Weekday::Fri => Weekday::Fri,
        chrono::Weekday::Sat => Weekday::Sat,
        chrono::Weekday::Sun => Weekday::Sun,
    }
}

fn previous(w: Weekday) -> Weekday {
    let i = Weekday::ALL.iter().position(|x| *x == w).unwrap_or(0);
    Weekday::ALL[(i + 6) % 7]
}

/// Whether `at` falls into a calendar window. Returns the verdict and the
/// local time it judged, for the test panel.
pub fn calendar_matches(
    weekdays: &[Weekday],
    start: &str,
    end: &str,
    timezone: &str,
    at: DateTime<Utc>,
) -> Result<(bool, String), String> {
    let tz: chrono_tz::Tz = timezone
        .parse()
        .map_err(|_| format!("unknown time zone '{timezone}'"))?;
    let s = parse_hhmm(start, false)?;
    let e = parse_hhmm(end, true)?;
    let local = at.with_timezone(&tz);
    let t = local.hour() * 60 + local.minute();
    let day = weekday_of(local.weekday());
    let on = |d: Weekday| weekdays.is_empty() || weekdays.contains(&d);
    let ok = if s == e {
        on(day)
    } else if s < e {
        on(day) && t >= s && t < e
    } else {
        // crosses midnight: the window belongs to the day it starts on
        (t >= s && on(day)) || (t < e && on(previous(day)))
    };
    Ok((ok, format!("{} {}", day.label(), local.format("%H:%M %Z"))))
}

fn field_value(ev: &Event, field: &str) -> Option<String> {
    Some(match field {
        "title" => ev.title.clone(),
        "body" => ev.body.clone(),
        "node" => ev.node.clone().unwrap_or_default(),
        "guest_id" => ev.guest_id.clone().unwrap_or_default(),
        "rule_id" => ev.rule_id.clone().unwrap_or_default(),
        "subtype" => ev.subtype.clone(),
        "kind" => ev.kind.as_str().to_string(),
        "severity" => ev.severity.as_str().to_string(),
        f => {
            let key = f.strip_prefix("fields.")?;
            ev.fields.get(key).cloned().unwrap_or_default()
        }
    })
}

fn is_known_field(field: &str) -> bool {
    matches!(
        field,
        "title" | "body" | "node" | "guest_id" | "rule_id" | "subtype" | "kind" | "severity"
    ) || field.strip_prefix("fields.").is_some_and(|k| !k.is_empty())
}

fn list(items: &[String]) -> String {
    items.join(", ")
}

fn leaf(label: String, matched: bool, invert: bool, reason: String) -> (bool, NodeTrace) {
    let result = matched != invert;
    let reason = if invert {
        format!("{reason} (inverted)")
    } else {
        reason
    };
    (
        result,
        NodeTrace {
            label: if invert {
                format!("NOT {label}")
            } else {
                label
            },
            matched: result,
            reason,
            children: Vec::new(),
        },
    )
}

fn eval_children(
    mode: MatchMode,
    children: &[MatchNode],
    ev: &Event,
    at: DateTime<Utc>,
) -> (bool, Vec<NodeTrace>) {
    let mut traces = Vec::new();
    let mut results = Vec::new();
    for c in children {
        let (r, t) = eval_node(c, ev, at);
        results.push(r);
        traces.push(t);
    }
    let r = if children.is_empty() {
        true
    } else {
        match mode {
            MatchMode::All => results.iter().all(|r| *r),
            MatchMode::Any => results.iter().any(|r| *r),
        }
    };
    (r, traces)
}

fn group_label(mode: MatchMode, invert: bool) -> String {
    let m = match mode {
        MatchMode::All => "all of",
        MatchMode::Any => "any of",
    };
    if invert {
        format!("NOT {m}")
    } else {
        m.to_string()
    }
}

/// Evaluate one node against `ev`; `at` is the instant calendar leaves
/// judge (the event's own timestamp in normal use).
pub fn eval_node(node: &MatchNode, ev: &Event, at: DateTime<Utc>) -> (bool, NodeTrace) {
    match node {
        MatchNode::Group {
            mode,
            invert,
            children,
        } => {
            let (r, traces) = eval_children(*mode, children, ev, at);
            let result = r != *invert;
            let reason = if children.is_empty() {
                "empty group matches everything".to_string()
            } else {
                let hit = traces.iter().filter(|t| t.matched).count();
                format!("{hit} of {} children matched", traces.len())
            };
            (
                result,
                NodeTrace {
                    label: group_label(*mode, *invert),
                    matched: result,
                    reason,
                    children: traces,
                },
            )
        }
        MatchNode::Severity {
            invert,
            levels,
            min,
        } => {
            let in_levels = levels.is_empty() || levels.contains(&ev.severity);
            let above = min.is_none_or(|m| ev.severity >= m);
            let mut label = Vec::new();
            if !levels.is_empty() {
                label.push(format!(
                    "severity in {}",
                    list(
                        &levels
                            .iter()
                            .map(|l| l.as_str().to_string())
                            .collect::<Vec<_>>()
                    )
                ));
            }
            if let Some(m) = min {
                label.push(format!("severity >= {}", m.as_str()));
            }
            leaf(
                label.join(" and "),
                in_levels && above,
                *invert,
                format!("event severity is {}", ev.severity.as_str()),
            )
        }
        MatchNode::EventType {
            invert,
            kinds,
            subtypes,
            rule_ids,
        } => {
            let k = kinds.is_empty() || kinds.contains(&ev.kind);
            let s = subtypes.is_empty() || subtypes.contains(&ev.subtype);
            let r =
                rule_ids.is_empty() || ev.rule_id.as_ref().is_some_and(|id| rule_ids.contains(id));
            let mut label = Vec::new();
            if !kinds.is_empty() {
                label.push(format!(
                    "type in {}",
                    list(
                        &kinds
                            .iter()
                            .map(|k| k.as_str().to_string())
                            .collect::<Vec<_>>()
                    )
                ));
            }
            if !subtypes.is_empty() {
                label.push(format!("subtype in {}", list(subtypes)));
            }
            if !rule_ids.is_empty() {
                label.push(format!("alert rule in {}", list(rule_ids)));
            }
            leaf(
                label.join(" and "),
                k && s && r,
                *invert,
                format!(
                    "event is {}{}{}",
                    ev.kind.as_str(),
                    if ev.subtype.is_empty() {
                        String::new()
                    } else {
                        format!("/{}", ev.subtype)
                    },
                    ev.rule_id
                        .as_ref()
                        .map(|r| format!(" (rule {r})"))
                        .unwrap_or_default()
                ),
            )
        }
        MatchNode::Node { invert, names } => {
            let m = ev.node.as_ref().is_some_and(|n| names.contains(n));
            leaf(
                format!("node in {}", list(names)),
                m,
                *invert,
                format!("event node is {}", ev.node.as_deref().unwrap_or("(none)")),
            )
        }
        MatchNode::Guest { invert, ids, tags } => {
            let by_id = ev.guest_id.as_ref().is_some_and(|g| ids.contains(g));
            let by_tag = ev.guest_tags.iter().any(|t| tags.contains(t));
            let m = if ids.is_empty() && tags.is_empty() {
                false
            } else {
                by_id || by_tag
            };
            let mut label = Vec::new();
            if !ids.is_empty() {
                label.push(format!("guest id in {}", list(ids)));
            }
            if !tags.is_empty() {
                label.push(format!("guest tag in {}", list(tags)));
            }
            leaf(
                label.join(" or "),
                m,
                *invert,
                format!(
                    "event guest is {}, tags [{}]",
                    ev.guest_id.as_deref().unwrap_or("(none)"),
                    list(&ev.guest_tags)
                ),
            )
        }
        MatchNode::Calendar {
            invert,
            weekdays,
            start,
            end,
            timezone,
        } => {
            let days = if weekdays.is_empty() {
                "every day".to_string()
            } else {
                weekdays
                    .iter()
                    .map(|d| d.label())
                    .collect::<Vec<_>>()
                    .join(",")
            };
            let label = format!("{days} {start}-{end} {timezone}");
            match calendar_matches(weekdays, start, end, timezone, at) {
                Ok((m, local)) => leaf(label, m, *invert, format!("event time is {local}")),
                Err(e) => leaf(label, false, false, format!("invalid calendar: {e}")),
            }
        }
        MatchNode::Field {
            invert,
            field,
            regex,
        } => {
            let label = format!("{field} matches /{regex}/");
            match (field_value(ev, field), regex::Regex::new(regex)) {
                (Some(v), Ok(re)) => {
                    let m = re.is_match(&v);
                    let shown: String = v.chars().take(80).collect();
                    leaf(label, m, *invert, format!("{field} is \"{shown}\""))
                }
                (None, _) => leaf(label, false, false, format!("unknown field '{field}'")),
                (_, Err(e)) => leaf(label, false, false, format!("invalid regex: {e}")),
            }
        }
    }
}

/// Evaluate a whole matcher (a disabled one never matches).
pub fn eval_matcher(m: &Matcher, ev: &Event, at: DateTime<Utc>) -> (bool, NodeTrace) {
    let (r, traces) = eval_children(m.mode, &m.children, ev, at);
    let result = m.enabled && (r != m.invert);
    let reason = if !m.enabled {
        "matcher is disabled".to_string()
    } else if m.children.is_empty() {
        "no conditions: matches every event".to_string()
    } else {
        format!(
            "{} of {} conditions matched",
            traces.iter().filter(|t| t.matched).count(),
            traces.len()
        )
    };
    (
        result,
        NodeTrace {
            label: format!("matcher '{}': {}", m.name, group_label(m.mode, m.invert)),
            matched: result,
            reason,
            children: traces,
        },
    )
}

fn count_nodes(nodes: &[MatchNode], depth: usize, max_depth: &mut usize) -> usize {
    *max_depth = (*max_depth).max(depth);
    nodes
        .iter()
        .map(|n| match n {
            MatchNode::Group { children, .. } => 1 + count_nodes(children, depth + 1, max_depth),
            _ => 1,
        })
        .sum()
}

fn validate_node(n: &MatchNode, path: &str, errs: &mut Vec<String>) {
    let mut bad = |m: String| errs.push(format!("{path}: {m}"));
    match n {
        MatchNode::Group { children, .. } => {
            for (i, c) in children.iter().enumerate() {
                validate_node(c, &format!("{path}.{}", i + 1), errs);
            }
        }
        MatchNode::Severity { levels, min, .. } => {
            if levels.is_empty() && min.is_none() {
                bad("a severity condition needs levels or a minimum".into());
            }
        }
        MatchNode::EventType {
            kinds,
            subtypes,
            rule_ids,
            ..
        } => {
            if kinds.is_empty() && subtypes.is_empty() && rule_ids.is_empty() {
                bad("an event type condition needs at least one type, subtype or rule".into());
            }
        }
        MatchNode::Node { names, .. } => {
            if names.is_empty() {
                bad("a node condition needs at least one node name".into());
            }
        }
        MatchNode::Guest { ids, tags, .. } => {
            if ids.is_empty() && tags.is_empty() {
                bad("a guest condition needs at least one id or tag".into());
            }
        }
        MatchNode::Calendar {
            start,
            end,
            timezone,
            ..
        } => {
            if let Err(e) = parse_hhmm(start, false) {
                bad(format!("start: {e}"));
            }
            if let Err(e) = parse_hhmm(end, true) {
                bad(format!("end: {e}"));
            }
            if timezone.parse::<chrono_tz::Tz>().is_err() {
                bad(format!("unknown time zone '{timezone}'"));
            }
        }
        MatchNode::Field { field, regex, .. } => {
            if !is_known_field(field) {
                bad(format!(
                    "unknown field '{field}' (title, body, node, guest_id, rule_id, subtype, kind, severity or fields.<key>)"
                ));
            }
            if let Err(e) = regex::Regex::new(regex) {
                bad(format!("invalid regex: {e}"));
            }
        }
    }
}

/// Check a matcher; `known_targets` are the existing channel ids.
pub fn validate(m: &Matcher, known_targets: &[String]) -> Result<(), String> {
    let mut errs = Vec::new();
    if m.name.trim().is_empty() {
        errs.push("matcher name is required".to_string());
    }
    let mut depth = 0;
    let total = count_nodes(&m.children, 1, &mut depth);
    if depth > MAX_DEPTH {
        errs.push(format!(
            "groups are nested {depth} deep (limit {MAX_DEPTH})"
        ));
    }
    if total > MAX_NODES {
        errs.push(format!("{total} conditions (limit {MAX_NODES})"));
    }
    for (i, c) in m.children.iter().enumerate() {
        validate_node(c, &format!("condition {}", i + 1), &mut errs);
    }
    for t in &m.targets {
        if !known_targets.contains(t) {
            errs.push(format!("target '{t}' is not a notification channel"));
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("; "))
    }
}

/// Channel ids that should receive `ev`, with the matchers that selected
/// them. `enabled_channels` are the ids of enabled channels.
pub fn route(
    matchers: &[Matcher],
    ev: &Event,
    enabled_channels: &[String],
) -> (Vec<String>, Vec<String>) {
    let mut targets: Vec<String> = Vec::new();
    let mut hit = Vec::new();
    for m in matchers {
        if eval_matcher(m, ev, ev.timestamp).0 {
            hit.push(m.name.clone());
            let add: Vec<&String> = if m.all_channels {
                enabled_channels.iter().collect()
            } else {
                m.targets.iter().collect()
            };
            for t in add {
                if enabled_channels.contains(t) && !targets.contains(t) {
                    targets.push(t.clone());
                }
            }
        }
    }
    (targets, hit)
}

fn db_err(e: sqlx::Error) -> StoreError {
    StoreError::Db(format!("notification matchers: {e}"))
}

impl NotificationChannelStore {
    pub(crate) async fn load_matchers(
        db: &crate::db::Database,
    ) -> Result<Vec<Matcher>, StoreError> {
        let rows = sqlx::query(
            "SELECT id, definition FROM notification_matchers ORDER BY position, created_at, id",
        )
        .fetch_all(db.pool())
        .await
        .map_err(db_err)?;
        let mut out = Vec::new();
        for r in rows {
            let id: String = r.get("id");
            let def: String = r.get("definition");
            match serde_json::from_str::<Matcher>(&def) {
                Ok(mut m) => {
                    m.id = id;
                    out.push(m);
                }
                Err(e) => tracing::error!("notification matcher {id} is unreadable: {e}"),
            }
        }
        Ok(out)
    }

    pub async fn list_matchers(&self) -> Vec<Matcher> {
        self.matchers.read().await.clone()
    }

    pub async fn get_matcher(&self, id: &str) -> Result<Matcher, StoreError> {
        self.matchers
            .read()
            .await
            .iter()
            .find(|m| m.id == id)
            .cloned()
            .ok_or_else(|| StoreError::NotFound(format!("notification matcher '{id}' not found")))
    }

    async fn channel_ids(&self) -> Vec<String> {
        self.channels
            .read()
            .await
            .iter()
            .map(|c| c.id.clone())
            .collect()
    }

    async fn save_matcher(&self, m: &Matcher, position: i64) -> Result<(), StoreError> {
        let def = serde_json::to_string(m).map_err(|e| StoreError::Db(e.to_string()))?;
        sqlx::query(
            "INSERT INTO notification_matchers (id, name, definition, position, created_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, definition = excluded.definition",
        )
        .bind(&m.id)
        .bind(&m.name)
        .bind(def)
        .bind(position)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    pub async fn create_matcher(&self, mut m: Matcher) -> Result<Matcher, StoreError> {
        validate(&m, &self.channel_ids().await).map_err(StoreError::Invalid)?;
        let mut all = self.matchers.write().await;
        if all.iter().any(|x| x.name == m.name) {
            return Err(StoreError::Invalid(format!(
                "a matcher named '{}' already exists",
                m.name
            )));
        }
        m.id = uuid::Uuid::new_v4().to_string();
        self.save_matcher(&m, all.len() as i64).await?;
        all.push(m.clone());
        Ok(m)
    }

    pub async fn update_matcher(&self, id: &str, mut m: Matcher) -> Result<Matcher, StoreError> {
        validate(&m, &self.channel_ids().await).map_err(StoreError::Invalid)?;
        let mut all = self.matchers.write().await;
        if all.iter().any(|x| x.name == m.name && x.id != id) {
            return Err(StoreError::Invalid(format!(
                "a matcher named '{}' already exists",
                m.name
            )));
        }
        let pos = all.iter().position(|x| x.id == id).ok_or_else(|| {
            StoreError::NotFound(format!("notification matcher '{id}' not found"))
        })?;
        m.id = id.to_string();
        self.save_matcher(&m, pos as i64).await?;
        all[pos] = m.clone();
        Ok(m)
    }

    pub async fn delete_matcher(&self, id: &str) -> Result<(), StoreError> {
        let mut all = self.matchers.write().await;
        let before = all.len();
        all.retain(|m| m.id != id);
        if all.len() == before {
            return Err(StoreError::NotFound(format!(
                "notification matcher '{id}' not found"
            )));
        }
        sqlx::query("DELETE FROM notification_matchers WHERE id = ?")
            .bind(id)
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }

    /// Reorder: `ids` is the new order (unknown ids are an error, missing
    /// ones keep their relative order at the end).
    pub async fn reorder_matchers(&self, ids: &[String]) -> Result<(), StoreError> {
        let mut all = self.matchers.write().await;
        for id in ids {
            if !all.iter().any(|m| &m.id == id) {
                return Err(StoreError::NotFound(format!(
                    "notification matcher '{id}' not found"
                )));
            }
        }
        let mut ordered: Vec<Matcher> = ids
            .iter()
            .filter_map(|id| all.iter().find(|m| &m.id == id).cloned())
            .collect();
        for m in all.iter() {
            if !ids.contains(&m.id) {
                ordered.push(m.clone());
            }
        }
        for (i, m) in ordered.iter().enumerate() {
            self.save_matcher(m, i as i64).await?;
            sqlx::query("UPDATE notification_matchers SET position = ? WHERE id = ?")
                .bind(i as i64)
                .bind(&m.id)
                .execute(self.pool())
                .await
                .map_err(db_err)?;
        }
        *all = ordered;
        Ok(())
    }

    /// A deleted channel disappears from every matcher's targets.
    pub(crate) async fn forget_target(&self, channel_id: &str) {
        let mut all = self.matchers.write().await;
        for (i, m) in all.iter_mut().enumerate() {
            if m.targets.iter().any(|t| t == channel_id) {
                m.targets.retain(|t| t != channel_id);
                if let Err(e) = self.save_matcher(m, i as i64).await {
                    tracing::warn!("could not drop channel {channel_id} from matcher: {e:?}");
                }
            }
        }
    }

    /// Evaluate `ev` against every matcher (or only `only`) without
    /// sending anything. `at` overrides the instant calendar leaves judge.
    pub async fn test_event(
        &self,
        ev: &Event,
        at: Option<chrono::DateTime<chrono::Utc>>,
        only: &[String],
        draft: Option<Matcher>,
    ) -> TestResult {
        let at = at.unwrap_or(ev.timestamp);
        let channels = self.channels.read().await.clone();
        let enabled: Vec<&crate::notification_channels::NotificationChannel> =
            channels.iter().filter(|c| c.enabled).collect();
        let refs = |m: &Matcher| -> Vec<TargetRef> {
            if m.all_channels {
                enabled
                    .iter()
                    .map(|c| TargetRef {
                        id: c.id.clone(),
                        name: c.name.clone(),
                    })
                    .collect()
            } else {
                m.targets
                    .iter()
                    .filter_map(|t| enabled.iter().find(|c| &c.id == t))
                    .map(|c| TargetRef {
                        id: c.id.clone(),
                        name: c.name.clone(),
                    })
                    .collect()
            }
        };
        let mut verdicts = Vec::new();
        let mut union: Vec<TargetRef> = Vec::new();
        let all: Vec<Matcher> = match draft {
            Some(d) => vec![d],
            None => self.matchers.read().await.clone(),
        };
        for m in all.iter() {
            if !only.is_empty() && !only.contains(&m.id) {
                continue;
            }
            let (matched, trace) = eval_matcher(m, ev, at);
            let targets = refs(m);
            if matched {
                for t in &targets {
                    if !union.contains(t) {
                        union.push(t.clone());
                    }
                }
            }
            verdicts.push(MatcherVerdict {
                id: m.id.clone(),
                name: m.name.clone(),
                enabled: m.enabled,
                matched,
                trace,
                targets,
            });
        }
        TestResult {
            matchers: verdicts,
            targets: union,
        }
    }

    /// Replace the whole matcher set (declarative apply helper).
    pub async fn channel_names(&self) -> Vec<(String, String)> {
        self.channels
            .read()
            .await
            .iter()
            .map(|c| (c.id.clone(), c.name.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use horcrux_common::notify::{EventKind, Severity};

    fn ev(kind: EventKind, sev: Severity) -> Event {
        Event::new(kind, sev, "t")
    }
    fn sev_min(s: Severity) -> MatchNode {
        MatchNode::Severity {
            invert: false,
            levels: vec![],
            min: Some(s),
        }
    }
    fn kind(k: EventKind) -> MatchNode {
        MatchNode::EventType {
            invert: false,
            kinds: vec![k],
            subtypes: vec![],
            rule_ids: vec![],
        }
    }
    fn cal(days: &[Weekday], s: &str, e: &str, tz: &str) -> MatchNode {
        MatchNode::Calendar {
            invert: false,
            weekdays: days.to_vec(),
            start: s.into(),
            end: e.into(),
            timezone: tz.into(),
        }
    }
    fn group(mode: MatchMode, invert: bool, children: Vec<MatchNode>) -> MatchNode {
        MatchNode::Group {
            mode,
            invert,
            children,
        }
    }
    fn m(children: Vec<MatchNode>) -> Matcher {
        Matcher {
            children,
            ..Matcher::default_matcher()
        }
    }
    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }
    fn hit(m: &Matcher, e: &Event) -> bool {
        eval_matcher(m, e, e.timestamp).0
    }

    #[test]
    fn empty_matcher_matches_everything() {
        assert!(hit(&m(vec![]), &ev(EventKind::Test, Severity::Info)));
    }

    #[test]
    fn nested_any_all_not() {
        // (package_update AND business hours) OR severity >= warning
        let tree = group(
            MatchMode::Any,
            false,
            vec![
                group(
                    MatchMode::All,
                    false,
                    vec![
                        kind(EventKind::PackageUpdate),
                        cal(
                            &[
                                Weekday::Mon,
                                Weekday::Tue,
                                Weekday::Wed,
                                Weekday::Thu,
                                Weekday::Fri,
                            ],
                            "09:00",
                            "17:00",
                            "UTC",
                        ),
                    ],
                ),
                sev_min(Severity::Warning),
            ],
        );
        let mm = m(vec![tree]);
        // Wed 2026-11-25 10:00 UTC
        let mut e = ev(EventKind::PackageUpdate, Severity::Info);
        e.timestamp = utc(2026, 11, 25, 10, 0);
        assert!(hit(&mm, &e));
        e.timestamp = utc(2026, 11, 25, 20, 0);
        assert!(!hit(&mm, &e), "update after hours is not routed");
        e.timestamp = utc(2026, 11, 28, 10, 0); // Saturday
        assert!(!hit(&mm, &e));
        let mut w = ev(EventKind::Backup, Severity::Warning);
        w.timestamp = utc(2026, 11, 28, 3, 0);
        assert!(hit(&mm, &w), "warning always routed");
        let i = ev(EventKind::Backup, Severity::Info);
        assert!(!hit(&mm, &i));
    }

    #[test]
    fn not_inverts_groups_leaves_and_the_matcher() {
        let e = ev(EventKind::Alert, Severity::Critical);
        assert!(!hit(
            &m(vec![group(
                MatchMode::All,
                true,
                vec![kind(EventKind::Alert)]
            )]),
            &e
        ));
        let mut leafnot = kind(EventKind::Alert);
        if let MatchNode::EventType { invert, .. } = &mut leafnot {
            *invert = true;
        }
        assert!(!hit(&m(vec![leafnot]), &e));
        let mut mm = m(vec![kind(EventKind::Backup)]);
        mm.invert = true;
        assert!(hit(&mm, &e));
        // NOT(any of []) : empty group matches, so its negation does not
        assert!(!hit(&m(vec![group(MatchMode::Any, true, vec![])]), &e));
    }

    #[test]
    fn disabled_matcher_never_matches() {
        let mut mm = m(vec![]);
        mm.enabled = false;
        assert!(!hit(&mm, &ev(EventKind::Test, Severity::Info)));
    }

    #[test]
    fn severity_levels_and_minimum() {
        let lv = MatchNode::Severity {
            invert: false,
            levels: vec![Severity::Warning, Severity::Error],
            min: None,
        };
        let mm = m(vec![lv]);
        assert!(hit(&mm, &ev(EventKind::Alert, Severity::Error)));
        assert!(!hit(&mm, &ev(EventKind::Alert, Severity::Critical)));
        let mm = m(vec![sev_min(Severity::Error)]);
        assert!(hit(&mm, &ev(EventKind::Alert, Severity::Critical)));
        assert!(!hit(&mm, &ev(EventKind::Alert, Severity::Notice)));
    }

    #[test]
    fn event_type_kind_subtype_and_rule() {
        let n = MatchNode::EventType {
            invert: false,
            kinds: vec![EventKind::Backup],
            subtypes: vec!["backup_failed".into()],
            rule_ids: vec![],
        };
        let mm = m(vec![n]);
        assert!(hit(
            &mm,
            &ev(EventKind::Backup, Severity::Error).subtype("backup_failed")
        ));
        assert!(!hit(
            &mm,
            &ev(EventKind::Backup, Severity::Info).subtype("backup_ok")
        ));
        let r = MatchNode::EventType {
            invert: false,
            kinds: vec![],
            subtypes: vec![],
            rule_ids: vec!["cpu-high".into()],
        };
        let mut e = ev(EventKind::Alert, Severity::Warning);
        assert!(!hit(&m(vec![r.clone()]), &e));
        e.rule_id = Some("cpu-high".into());
        assert!(hit(&m(vec![r]), &e));
    }

    #[test]
    fn node_guest_and_field_leaves() {
        let mut e = ev(EventKind::Alert, Severity::Warning)
            .node("pve1")
            .guest("101");
        e.guest_tags = vec!["prod".into()];
        e.title = "Disk almost full on db".into();
        let node = MatchNode::Node {
            invert: false,
            names: vec!["pve1".into()],
        };
        assert!(hit(&m(vec![node]), &e));
        let g = MatchNode::Guest {
            invert: false,
            ids: vec![],
            tags: vec!["prod".into()],
        };
        assert!(hit(&m(vec![g]), &e));
        let g2 = MatchNode::Guest {
            invert: false,
            ids: vec!["102".into()],
            tags: vec!["dev".into()],
        };
        assert!(!hit(&m(vec![g2]), &e));
        let f = MatchNode::Field {
            invert: false,
            field: "title".into(),
            regex: "(?i)disk .* full".into(),
        };
        assert!(hit(&m(vec![f]), &e));
        let f2 = MatchNode::Field {
            invert: false,
            field: "fields.volume".into(),
            regex: "^tank$".into(),
        };
        let e2 = e.clone().field("volume", "tank");
        assert!(hit(&m(vec![f2.clone()]), &e2));
        assert!(!hit(&m(vec![f2]), &e));
    }

    #[test]
    fn calendar_plain_window_is_end_exclusive() {
        let c = |h, mi| {
            calendar_matches(&[], "09:00", "17:00", "UTC", utc(2026, 11, 25, h, mi))
                .unwrap()
                .0
        };
        assert!(!c(8, 59));
        assert!(c(9, 0));
        assert!(c(16, 59));
        assert!(!c(17, 0));
    }

    #[test]
    fn calendar_crossing_midnight_belongs_to_the_start_day() {
        // Fri 22:00 - 06:00 (Friday night shift)
        let f = |d: u32, h, mi| {
            calendar_matches(
                &[Weekday::Fri],
                "22:00",
                "06:00",
                "UTC",
                utc(2026, 11, d, h, mi),
            )
            .unwrap()
            .0
        };
        // 2026-11-27 is a Friday
        assert!(f(27, 22, 0));
        assert!(f(27, 23, 59));
        assert!(f(28, 0, 0), "Saturday 00:00 is still Friday's window");
        assert!(f(28, 5, 59));
        assert!(!f(28, 6, 0));
        assert!(!f(28, 22, 0), "Saturday evening starts no window");
        assert!(
            !f(27, 5, 0),
            "Friday early morning belongs to Thursday's window"
        );
        assert!(!f(27, 21, 59));
    }

    #[test]
    fn calendar_whole_day_and_24h_end() {
        assert!(
            calendar_matches(
                &[Weekday::Wed],
                "00:00",
                "00:00",
                "UTC",
                utc(2026, 11, 25, 23, 59)
            )
            .unwrap()
            .0
        );
        assert!(
            !calendar_matches(
                &[Weekday::Wed],
                "00:00",
                "00:00",
                "UTC",
                utc(2026, 11, 26, 0, 0)
            )
            .unwrap()
            .0
        );
        assert!(
            calendar_matches(&[], "12:00", "24:00", "UTC", utc(2026, 11, 25, 23, 59))
                .unwrap()
                .0
        );
    }

    #[test]
    fn calendar_uses_the_zone_and_follows_dst() {
        // America/Los_Angeles: PST (UTC-8) until 2027-03-14 02:00, then PDT (UTC-7).
        let la = |at| {
            calendar_matches(&[], "09:00", "17:00", "America/Los_Angeles", at)
                .unwrap()
                .0
        };
        // 2027-03-13 (PST): 17:00Z is 09:00 local -> in; 16:59Z -> out
        assert!(la(utc(2027, 3, 13, 17, 0)));
        assert!(!la(utc(2027, 3, 13, 16, 59)));
        // 2027-03-15 (PDT): the same wall-clock 09:00 is 16:00Z
        assert!(la(utc(2027, 3, 15, 16, 0)));
        assert!(!la(utc(2027, 3, 15, 15, 59)));
        // a window inside the skipped hour never fires on the change day
        // and the repeated autumn hour fires twice: 2026-11-01 01:30 local
        let night = |at| {
            calendar_matches(&[], "01:00", "02:00", "America/Los_Angeles", at)
                .unwrap()
                .0
        };
        assert!(night(utc(2026, 11, 1, 8, 30))); // 01:30 PDT
        assert!(night(utc(2026, 11, 1, 9, 30))); // 01:30 PST (repeated hour)
        assert!(!night(utc(2026, 11, 1, 10, 30))); // 02:30 PST
        let spring = |at| {
            calendar_matches(&[], "02:00", "03:00", "America/Los_Angeles", at)
                .unwrap()
                .0
        };
        for h in 8..12 {
            assert!(!spring(utc(2027, 3, 14, h, 30)), "hour {h}");
        }
    }

    #[test]
    fn calendar_weekday_follows_local_date_not_utc() {
        // Sun 2026-11-29 23:30 UTC is Monday 08:30 in Tokyo
        let r = calendar_matches(
            &[Weekday::Mon],
            "08:00",
            "09:00",
            "Asia/Tokyo",
            utc(2026, 11, 29, 23, 30),
        )
        .unwrap();
        assert!(r.0, "{}", r.1);
    }

    #[test]
    fn bad_calendar_values_are_rejected() {
        assert!(calendar_matches(&[], "9", "17:00", "UTC", Utc::now()).is_err());
        assert!(calendar_matches(&[], "25:00", "17:00", "UTC", Utc::now()).is_err());
        assert!(calendar_matches(&[], "09:00", "17:00", "Mars/Base", Utc::now()).is_err());
    }

    #[test]
    fn validate_catches_empty_leaves_depth_and_unknown_targets() {
        let known = vec!["c1".to_string()];
        let mut mm = m(vec![MatchNode::Severity {
            invert: false,
            levels: vec![],
            min: None,
        }]);
        assert!(validate(&mm, &known).unwrap_err().contains("severity"));
        mm.children = vec![cal(&[], "09:00", "17:00", "Nowhere/Land")];
        assert!(validate(&mm, &known).unwrap_err().contains("time zone"));
        mm.children = vec![MatchNode::Field {
            invert: false,
            field: "bogus".into(),
            regex: "(".into(),
        }];
        let e = validate(&mm, &known).unwrap_err();
        assert!(
            e.contains("unknown field") && e.contains("invalid regex"),
            "{e}"
        );
        let mut deep = kind(EventKind::Alert);
        for _ in 0..MAX_DEPTH + 1 {
            deep = group(MatchMode::All, false, vec![deep]);
        }
        mm.children = vec![deep];
        assert!(validate(&mm, &known).unwrap_err().contains("nested"));
        mm.children = vec![];
        mm.targets = vec!["gone".into()];
        assert!(validate(&mm, &known)
            .unwrap_err()
            .contains("not a notification channel"));
        mm.targets = vec!["c1".into()];
        assert!(validate(&mm, &known).is_ok());
    }

    #[test]
    fn routing_unions_targets_of_every_hit_and_skips_disabled_channels() {
        let a = Matcher {
            id: "a".into(),
            name: "a".into(),
            children: vec![sev_min(Severity::Warning)],
            targets: vec!["hook".into(), "mail".into()],
            all_channels: false,
            ..Matcher::default_matcher()
        };
        let b = Matcher {
            id: "b".into(),
            name: "b".into(),
            children: vec![kind(EventKind::Backup)],
            targets: vec!["mail".into()],
            all_channels: false,
            ..Matcher::default_matcher()
        };
        let enabled = vec!["hook".to_string()];
        let (t, hits) = route(
            &[a.clone(), b.clone()],
            &ev(EventKind::Backup, Severity::Error),
            &enabled,
        );
        assert_eq!(t, vec!["hook"]);
        assert_eq!(hits, vec!["a", "b"]);
        let (t, hits) = route(&[a, b], &ev(EventKind::Test, Severity::Info), &enabled);
        assert!(t.is_empty() && hits.is_empty());
        let (t, _) = route(
            &[Matcher::default_matcher()],
            &ev(EventKind::Test, Severity::Info),
            &["x".to_string(), "y".to_string()],
        );
        assert_eq!(t, vec!["x", "y"]);
    }
}
