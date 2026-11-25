//! Notification events and routing matchers (types only).
//!
//! Every notification source (alerts, backup jobs, package updates, HA,
//! replication, certificate expiry, login security events) describes what
//! happened as an [`Event`]. A [`Matcher`] is a tree of conditions that
//! decides which notification targets (channels) get it: groups combine
//! their children with `all` (AND) or `any` (OR) and can be inverted (NOT),
//! leaves test one property of the event. The evaluator lives in
//! `horcrux-monitoring` (it needs the time zone database and regexes); the
//! web UI shares these types with the API.
//!
//! Emitting code anywhere in the workspace calls [`emit`]; the API process
//! installs the sink once with [`set_sink`], so library crates never depend
//! on the monitoring crate.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

fn yes() -> bool {
    true
}

/// Ordered from least to most severe, so `min: Warning` matches warning,
/// error and critical.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Notice,
    Warning,
    Error,
    Critical,
}

impl Severity {
    pub const ALL: [Severity; 5] = [
        Severity::Info,
        Severity::Notice,
        Severity::Warning,
        Severity::Error,
        Severity::Critical,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Notice => "notice",
            Severity::Warning => "warning",
            Severity::Error => "error",
            Severity::Critical => "critical",
        }
    }
}

/// The category of an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// A monitoring alert rule fired or resolved (`rule_id` names the rule).
    Alert,
    /// A backup job finished (subtype `backup_ok` / `backup_failed`).
    Backup,
    /// Package updates are available (subtype `updates_available`).
    PackageUpdate,
    /// A high-availability event (subtypes `failover`, `fence`, `quorum_lost`).
    Ha,
    /// A replication job result (subtypes `replication_ok`, `replication_failed`).
    Replication,
    /// A certificate is about to expire or a renewal failed (subtypes
    /// `expiring`, `expired`, `renewal_failed`).
    CertificateExpiry,
    /// A security event such as a failed or successful login (subtypes
    /// `login_failed`, `login_success`, `lockout`).
    Security,
    /// A test message.
    Test,
}

impl EventKind {
    pub const ALL: [EventKind; 8] = [
        EventKind::Alert,
        EventKind::Backup,
        EventKind::PackageUpdate,
        EventKind::Ha,
        EventKind::Replication,
        EventKind::CertificateExpiry,
        EventKind::Security,
        EventKind::Test,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::Alert => "alert",
            EventKind::Backup => "backup",
            EventKind::PackageUpdate => "package_update",
            EventKind::Ha => "ha",
            EventKind::Replication => "replication",
            EventKind::CertificateExpiry => "certificate_expiry",
            EventKind::Security => "security",
            EventKind::Test => "test",
        }
    }
}

/// One thing that happened, as the router sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub kind: EventKind,
    /// Finer category inside the kind, e.g. `backup_failed`.
    #[serde(default)]
    pub subtype: String,
    pub severity: Severity,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    /// Cluster node the event happened on.
    #[serde(default)]
    pub node: Option<String>,
    #[serde(default)]
    pub guest_id: Option<String>,
    #[serde(default)]
    pub guest_tags: Vec<String>,
    /// Alert rule id for alert events.
    #[serde(default)]
    pub rule_id: Option<String>,
    /// Free-form extra fields (`field` leaves can match `fields.<key>`).
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
    #[serde(default = "Utc::now")]
    pub timestamp: DateTime<Utc>,
}

impl Event {
    pub fn new(kind: EventKind, severity: Severity, title: impl Into<String>) -> Self {
        Self {
            kind,
            subtype: String::new(),
            severity,
            title: title.into(),
            body: String::new(),
            node: None,
            guest_id: None,
            guest_tags: Vec::new(),
            rule_id: None,
            fields: BTreeMap::new(),
            timestamp: Utc::now(),
        }
    }

    pub fn subtype(mut self, s: impl Into<String>) -> Self {
        self.subtype = s.into();
        self
    }
    pub fn body(mut self, s: impl Into<String>) -> Self {
        self.body = s.into();
        self
    }
    pub fn node(mut self, s: impl Into<String>) -> Self {
        self.node = Some(s.into());
        self
    }
    pub fn guest(mut self, s: impl Into<String>) -> Self {
        self.guest_id = Some(s.into());
        self
    }
    pub fn field(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.fields.insert(k.into(), v.into());
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MatchMode {
    /// Every child must match (AND). An empty list matches.
    #[default]
    All,
    /// At least one child must match (OR). An empty list matches.
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Weekday {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

impl Weekday {
    pub const ALL: [Weekday; 7] = [
        Weekday::Mon,
        Weekday::Tue,
        Weekday::Wed,
        Weekday::Thu,
        Weekday::Fri,
        Weekday::Sat,
        Weekday::Sun,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Weekday::Mon => "mon",
            Weekday::Tue => "tue",
            Weekday::Wed => "wed",
            Weekday::Thu => "thu",
            Weekday::Fri => "fri",
            Weekday::Sat => "sat",
            Weekday::Sun => "sun",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Weekday::Mon => "Mon",
            Weekday::Tue => "Tue",
            Weekday::Wed => "Wed",
            Weekday::Thu => "Thu",
            Weekday::Fri => "Fri",
            Weekday::Sat => "Sat",
            Weekday::Sun => "Sun",
        }
    }
}

fn default_start() -> String {
    "00:00".into()
}
fn default_end() -> String {
    "00:00".into()
}
/// Documented default of a calendar leaf's `timezone`.
pub const DEFAULT_TIMEZONE: &str = "UTC";
fn default_tz() -> String {
    DEFAULT_TIMEZONE.into()
}

/// A node of a matcher tree: a group or one condition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MatchNode {
    /// Combines children with AND (`all`) or OR (`any`); `invert` is NOT.
    Group {
        #[serde(default)]
        mode: MatchMode,
        #[serde(default)]
        invert: bool,
        #[serde(default)]
        children: Vec<MatchNode>,
    },
    /// Severity is in `levels` (when not empty) and at least `min` (when set).
    Severity {
        #[serde(default)]
        invert: bool,
        #[serde(default)]
        levels: Vec<Severity>,
        #[serde(default)]
        min: Option<Severity>,
    },
    /// Event kind in `kinds`, subtype in `subtypes`, alert rule in
    /// `rule_ids` (every non-empty list must match).
    EventType {
        #[serde(default)]
        invert: bool,
        #[serde(default)]
        kinds: Vec<EventKind>,
        #[serde(default)]
        subtypes: Vec<String>,
        #[serde(default)]
        rule_ids: Vec<String>,
    },
    /// The event's node is one of `names`.
    Node {
        #[serde(default)]
        invert: bool,
        #[serde(default)]
        names: Vec<String>,
    },
    /// The guest id is one of `ids`, or the guest carries one of `tags`.
    Guest {
        #[serde(default)]
        invert: bool,
        #[serde(default)]
        ids: Vec<String>,
        #[serde(default)]
        tags: Vec<String>,
    },
    /// Local time of the event in `timezone` falls in `start`..`end`
    /// (HH:MM, `24:00` allowed as the end, end exclusive) on one of `weekdays` (empty = every day). A
    /// window with `end` earlier than `start` crosses midnight and belongs to
    /// the weekday it starts on; `start` equal to `end` means the whole day.
    Calendar {
        #[serde(default)]
        invert: bool,
        #[serde(default)]
        weekdays: Vec<Weekday>,
        #[serde(default = "default_start")]
        start: String,
        #[serde(default = "default_end")]
        end: String,
        #[serde(default = "default_tz")]
        timezone: String,
    },
    /// A regular expression matches the named field: `title`, `body`,
    /// `node`, `guest_id`, `rule_id`, `subtype`, `kind`, `severity` or
    /// `fields.<key>`.
    Field {
        #[serde(default)]
        invert: bool,
        field: String,
        regex: String,
    },
}

impl MatchNode {
    pub fn empty_group() -> Self {
        MatchNode::Group {
            mode: MatchMode::All,
            invert: false,
            children: Vec::new(),
        }
    }
}

/// A routing rule: when the tree matches an event, it is sent to `targets`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Matcher {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub mode: MatchMode,
    #[serde(default)]
    pub invert: bool,
    #[serde(default)]
    pub children: Vec<MatchNode>,
    /// Notification channel ids (names in the declarative spec).
    #[serde(default)]
    pub targets: Vec<String>,
    /// Also send to every enabled channel, including ones created later.
    #[serde(default)]
    pub all_channels: bool,
}

/// Name and id of the matcher created on upgrade (alerts, backup results and
/// test messages to every channel, exactly what was sent before routing
/// existed): behaviour is unchanged until the user edits routing. Newer event
/// kinds (packages, HA, replication, certificates, security) reach no channel
/// until a matcher routes them.
pub const DEFAULT_MATCHER_ID: &str = "default";
pub const DEFAULT_MATCHER_NAME: &str = "default";

impl Matcher {
    pub fn default_matcher() -> Self {
        Matcher {
            id: DEFAULT_MATCHER_ID.into(),
            name: DEFAULT_MATCHER_NAME.into(),
            description: Some(
                "Alerts, backup results and test messages go to every enabled channel (what \
                 notifications did before routing existed)"
                    .into(),
            ),
            enabled: true,
            mode: MatchMode::All,
            invert: false,
            children: vec![MatchNode::EventType {
                invert: false,
                kinds: vec![EventKind::Alert, EventKind::Backup, EventKind::Test],
                subtypes: Vec::new(),
                rule_ids: Vec::new(),
            }],
            targets: Vec::new(),
            all_channels: true,
        }
    }
}

/// Per-node verdict of an evaluation (for the test panel).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeTrace {
    /// What the node tests, e.g. `severity >= warning`.
    pub label: String,
    pub matched: bool,
    /// Why it did or did not match, e.g. `event severity is info`.
    pub reason: String,
    #[serde(default)]
    pub children: Vec<NodeTrace>,
}

/// What the test endpoint answers for one matcher.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MatcherVerdict {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub matched: bool,
    /// Per-node reasons.
    pub trace: NodeTrace,
    /// Channels this matcher would send to (enabled ones only).
    pub targets: Vec<TargetRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TargetRef {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TestResult {
    pub matchers: Vec<MatcherVerdict>,
    /// Union of the targets of every matcher that matched.
    pub targets: Vec<TargetRef>,
}

// ------------------------------------------------------------ tree editing

/// Position of a node in a matcher's tree: indexes from the matcher's own
/// child list down. The empty path is the matcher itself (the root group).
pub type NodePath = Vec<usize>;

/// The child list of the group at `group` (empty path = the root list).
pub fn children_mut<'a>(
    root: &'a mut Vec<MatchNode>,
    group: &[usize],
) -> Option<&'a mut Vec<MatchNode>> {
    let mut cur = root;
    for i in group {
        match cur.get_mut(*i)? {
            MatchNode::Group { children, .. } => cur = children,
            _ => return None,
        }
    }
    Some(cur)
}

pub fn node_at<'a>(root: &'a [MatchNode], path: &[usize]) -> Option<&'a MatchNode> {
    let (last, parent) = path.split_last()?;
    let mut cur = root;
    for i in parent {
        match cur.get(*i)? {
            MatchNode::Group { children, .. } => cur = children,
            _ => return None,
        }
    }
    cur.get(*last)
}

pub fn node_at_mut<'a>(root: &'a mut Vec<MatchNode>, path: &[usize]) -> Option<&'a mut MatchNode> {
    let (last, parent) = path.split_last()?;
    children_mut(root, parent)?.get_mut(*last)
}

/// Append `node` to the group at `group`.
pub fn add_node(root: &mut Vec<MatchNode>, group: &[usize], node: MatchNode) -> Result<(), String> {
    children_mut(root, group)
        .ok_or_else(|| "that is not a group".to_string())?
        .push(node);
    Ok(())
}

pub fn remove_node(root: &mut Vec<MatchNode>, path: &[usize]) -> Option<MatchNode> {
    let (last, parent) = path.split_last()?;
    let list = children_mut(root, parent)?;
    (*last < list.len()).then(|| list.remove(*last))
}

/// Move the node at `from` into the group `to_group`, before position
/// `index` (clamped to the end). A group can not move into itself or one of
/// its own descendants. `to_group` and `index` are given as they are on
/// screen before the move; this adjusts them for the removal.
pub fn move_node(
    root: &mut Vec<MatchNode>,
    from: &[usize],
    to_group: &[usize],
    index: usize,
) -> Result<(), String> {
    let (i, parent) = from.split_last().ok_or("nothing to move")?;
    if to_group.len() >= from.len() && to_group[..from.len()] == *from {
        return Err("a group can not be moved into itself".into());
    }
    if node_at(root, from).is_none() {
        return Err("the dragged condition no longer exists".into());
    }
    let mut to = to_group.to_vec();
    let mut index = index;
    if to.len() > parent.len() && to[..parent.len()] == *parent && to[parent.len()] > *i {
        to[parent.len()] -= 1;
    }
    if to == parent && index > *i {
        index -= 1;
    }
    // work on a copy: a refused move must leave the tree untouched
    let mut work = root.clone();
    let node = remove_node(&mut work, from).ok_or("the dragged condition no longer exists")?;
    let list = children_mut(&mut work, &to).ok_or("the target is not a group")?;
    let at = index.min(list.len());
    list.insert(at, node);
    *root = work;
    Ok(())
}

// --------------------------------------------------------------------- bus

type Sink = Arc<dyn Fn(Event) + Send + Sync>;
static SINK: OnceLock<Sink> = OnceLock::new();

/// Install the router (once, at startup). Later calls are ignored.
pub fn set_sink(f: impl Fn(Event) + Send + Sync + 'static) {
    let _ = SINK.set(Arc::new(f));
}

/// Hand an event to the notification router. Never blocks and never fails:
/// before the router is installed (early start, unit tests) the event is
/// dropped.
pub fn emit(ev: Event) {
    if let Some(s) = SINK.get() {
        s(ev);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severities_order_from_info_to_critical() {
        assert!(Severity::Info < Severity::Warning);
        assert!(Severity::Error < Severity::Critical);
    }

    #[test]
    fn tree_round_trips_through_json() {
        let m = Matcher {
            children: vec![MatchNode::Group {
                mode: MatchMode::Any,
                invert: true,
                children: vec![MatchNode::Severity {
                    invert: false,
                    levels: vec![],
                    min: Some(Severity::Warning),
                }],
            }],
            ..Matcher::default_matcher()
        };
        let j = serde_json::to_string(&m).unwrap();
        assert!(j.contains("\"type\":\"group\""), "{j}");
        let back: Matcher = serde_json::from_str(&j).unwrap();
        assert_eq!(back, m);
    }

    fn leafn(n: &str) -> MatchNode {
        MatchNode::Node {
            invert: false,
            names: vec![n.into()],
        }
    }
    fn grp(children: Vec<MatchNode>) -> MatchNode {
        MatchNode::Group {
            mode: MatchMode::Any,
            invert: false,
            children,
        }
    }
    fn names(l: &[MatchNode]) -> String {
        l.iter()
            .map(|n| match n {
                MatchNode::Node { names, .. } => names[0].clone(),
                MatchNode::Group { children, .. } => format!("[{}]", names(children)),
                _ => "?".into(),
            })
            .collect::<Vec<_>>()
            .join(",")
    }

    #[test]
    fn moving_nodes_between_groups_adjusts_for_the_removal() {
        // a, [b, c], d
        let mut t = vec![leafn("a"), grp(vec![leafn("b"), leafn("c")]), leafn("d")];
        // a -> end of the group (group index shifts from 1 to 0 after removal)
        move_node(&mut t, &[0], &[1], 2).unwrap();
        assert_eq!(names(&t), "[b,c,a],d");
        // d -> first place inside the group
        move_node(&mut t, &[1], &[0], 0).unwrap();
        assert_eq!(names(&t), "[d,b,c,a]");
        // c out of the group to the top level, before everything
        move_node(&mut t, &[0, 2], &[], 0).unwrap();
        assert_eq!(names(&t), "c,[d,b,a]");
        // reorder inside one list: move c after the group (index counts the on-screen list)
        move_node(&mut t, &[0], &[], 2).unwrap();
        assert_eq!(names(&t), "[d,b,a],c");
        // a group moves with its children
        move_node(&mut t, &[0], &[], 2).unwrap();
        assert_eq!(names(&t), "c,[d,b,a]");
    }

    #[test]
    fn a_group_never_moves_into_itself() {
        let mut t = vec![grp(vec![grp(vec![leafn("x")])])];
        assert!(move_node(&mut t, &[0], &[0, 0], 0).is_err());
        assert!(move_node(&mut t, &[0], &[0], 0).is_err());
        assert_eq!(names(&t), "[[x]]");
        assert!(move_node(&mut t, &[5], &[], 0).is_err());
        // leaves are not groups
        let mut t = vec![leafn("a"), leafn("b")];
        assert!(move_node(&mut t, &[0], &[1], 0).is_err());
        assert_eq!(names(&t), "a,b");
    }

    #[test]
    fn add_and_remove_by_path() {
        let mut t = vec![grp(vec![])];
        add_node(&mut t, &[0], leafn("a")).unwrap();
        add_node(&mut t, &[], leafn("b")).unwrap();
        assert_eq!(names(&t), "[a],b");
        assert!(add_node(&mut t, &[1], leafn("c")).is_err());
        assert!(remove_node(&mut t, &[0, 0]).is_some());
        assert_eq!(names(&t), "[],b");
        assert!(remove_node(&mut t, &[3]).is_none());
    }

    #[test]
    fn leaves_default_missing_fields() {
        let n: MatchNode =
            serde_json::from_str(r#"{"type":"calendar","weekdays":["mon"]}"#).unwrap();
        match n {
            MatchNode::Calendar {
                start,
                end,
                timezone,
                ..
            } => {
                assert_eq!((start.as_str(), end.as_str()), ("00:00", "00:00"));
                assert_eq!(timezone, DEFAULT_TIMEZONE);
            }
            other => panic!("{other:?}"),
        }
    }
}
