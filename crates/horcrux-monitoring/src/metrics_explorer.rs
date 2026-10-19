//! Metrics Explorer backend: an in-memory time-series store fed from the
//! host's real counters, plus a small PromQL-subset query engine.
//!
//! Series (24 h retention):
//! * node CPU (delta of /proc/stat between samples), memory, load, uptime and
//!   data-directory disk usage, sampled every [`SAMPLE_INTERVAL_SECS`];
//! * per-VM, per-container and per-storage-pool values as collected by the
//!   [`MonitoringManager`](crate::monitoring::MonitoringManager).
//!
//! Query language (what the web UI's query builder emits):
//! `metric`, `metric{label="v", other!="x", name=~"web-.*"}`,
//! `avg(metric{...})`, `sum(metric) by (label, ...)` with
//! sum/avg/min/max/count. Points are aligned to `step` buckets (last sample
//! in each bucket).

use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};
use tokio::sync::RwLock;

pub const SAMPLE_INTERVAL_SECS: u64 = 15;
pub const RETENTION_SECS: i64 = 24 * 3600;
const MAX_POINTS_PER_QUERY: i64 = 11_000;

pub type Labels = BTreeMap<String, String>;
/// (metric name, labels) -> samples (unix seconds, value), oldest first.
type SeriesMap = HashMap<(String, Labels), VecDeque<(i64, f64)>>;

/// Static description of one metric family.
#[derive(Debug, Clone)]
pub struct Family {
    pub name: &'static str,
    pub help: &'static str,
    pub unit: &'static str,
    pub category: &'static str,
    pub labels: &'static [&'static str],
    pub interval: &'static str,
}

pub const FAMILIES: &[Family] = &[
    Family {
        name: "horcrux_node_cpu_usage_percent",
        help: "Host CPU busy time since the previous sample",
        unit: "percent",
        category: "system",
        labels: &["node"],
        interval: "15s",
    },
    Family {
        name: "horcrux_node_memory_used_bytes",
        help: "Host memory in use (total - available)",
        unit: "bytes",
        category: "system",
        labels: &["node"],
        interval: "15s",
    },
    Family {
        name: "horcrux_node_memory_total_bytes",
        help: "Host memory installed",
        unit: "bytes",
        category: "system",
        labels: &["node"],
        interval: "15s",
    },
    Family {
        name: "horcrux_node_memory_usage_percent",
        help: "Host memory in use as a share of total",
        unit: "percent",
        category: "system",
        labels: &["node"],
        interval: "15s",
    },
    Family {
        name: "horcrux_node_load1",
        help: "1-minute load average",
        unit: "",
        category: "system",
        labels: &["node"],
        interval: "15s",
    },
    Family {
        name: "horcrux_node_load5",
        help: "5-minute load average",
        unit: "",
        category: "system",
        labels: &["node"],
        interval: "15s",
    },
    Family {
        name: "horcrux_node_load15",
        help: "15-minute load average",
        unit: "",
        category: "system",
        labels: &["node"],
        interval: "15s",
    },
    Family {
        name: "horcrux_node_uptime_seconds",
        help: "Host uptime",
        unit: "seconds",
        category: "system",
        labels: &["node"],
        interval: "15s",
    },
    Family {
        name: "horcrux_node_disk_usage_percent",
        help: "Usage of the filesystem holding the Horcrux data directory",
        unit: "percent",
        category: "storage",
        labels: &["node", "path"],
        interval: "15s",
    },
    Family {
        name: "horcrux_vm_cpu_usage_percent",
        help: "VM (QEMU process) CPU usage",
        unit: "percent",
        category: "vm",
        labels: &["vm_id", "name"],
        interval: "60s",
    },
    Family {
        name: "horcrux_vm_memory_used_bytes",
        help: "VM (QEMU process) resident memory",
        unit: "bytes",
        category: "vm",
        labels: &["vm_id", "name"],
        interval: "60s",
    },
    Family {
        name: "horcrux_container_cpu_usage_percent",
        help: "Container CPU usage",
        unit: "percent",
        category: "container",
        labels: &["container_id", "name"],
        interval: "60s",
    },
    Family {
        name: "horcrux_container_memory_used_bytes",
        help: "Container memory in use",
        unit: "bytes",
        category: "container",
        labels: &["container_id", "name"],
        interval: "60s",
    },
    Family {
        name: "horcrux_storage_used_bytes",
        help: "Storage pool space in use",
        unit: "bytes",
        category: "storage",
        labels: &["pool", "type"],
        interval: "60s",
    },
    Family {
        name: "horcrux_storage_total_bytes",
        help: "Storage pool capacity",
        unit: "bytes",
        category: "storage",
        labels: &["pool", "type"],
        interval: "60s",
    },
    Family {
        name: "horcrux_storage_usage_percent",
        help: "Storage pool space in use as a share of capacity",
        unit: "percent",
        category: "storage",
        labels: &["pool", "type"],
        interval: "60s",
    },
];

/// Check a query string the way `MetricsStore::query` would, without running
/// it (dashboards validate their panels with this before saving).
pub fn validate_query(query: &str) -> Result<(), String> {
    let parsed = parse(query)?;
    family(&parsed.metric).map(|_| ()).ok_or_else(|| {
        format!(
            "unknown metric '{}' (GET /api/metrics/definitions lists the available metrics)",
            parsed.metric
        )
    })
}

pub fn family(name: &str) -> Option<&'static Family> {
    FAMILIES.iter().find(|f| f.name == name)
}

/// The web UI's `MetricDefinition`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MetricDefinition {
    pub name: String,
    pub description: String,
    pub metric_type: String,
    pub labels: Vec<String>,
    pub unit: Option<String>,
    pub help: String,
    pub scrape_interval: String,
    pub retention: String,
    pub cardinality: u64,
    pub last_scraped: String,
    pub source: String,
    pub category: String,
}

/// The web UI's `MetricQuery`.
#[derive(Debug, Clone, Deserialize)]
pub struct MetricQuery {
    pub metric: String,
    #[serde(default)]
    pub labels: HashMap<String, String>,
    pub start_time: Option<DateTime<Utc>>,
    pub end_time: Option<DateTime<Utc>>,
    #[serde(default = "default_step")]
    pub step: u64,
}

fn default_step() -> u64 {
    60
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MetricDataPoint {
    pub timestamp: DateTime<Utc>,
    pub value: f64,
    pub labels: Labels,
}

/// The web UI's `MetricResult`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MetricResult {
    pub metric: String,
    pub labels: Labels,
    pub values: Vec<MetricDataPoint>,
}

#[derive(Default)]
pub struct MetricsStore {
    series: RwLock<SeriesMap>,
}

impl MetricsStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a sample (ignored if not newer than the series' last one).
    pub async fn record(&self, name: &str, labels: Labels, ts: i64, value: f64) {
        if !value.is_finite() {
            return;
        }
        let mut series = self.series.write().await;
        let points = series.entry((name.to_string(), labels)).or_default();
        if points.back().map(|(t, _)| *t >= ts).unwrap_or(false) {
            return;
        }
        points.push_back((ts, value));
        while points
            .front()
            .map(|(t, _)| *t < ts - RETENTION_SECS)
            .unwrap_or(false)
        {
            points.pop_front();
        }
    }

    pub async fn definitions(&self) -> Vec<MetricDefinition> {
        let series = self.series.read().await;
        FAMILIES
            .iter()
            .map(|f| {
                let mine: Vec<_> = series.iter().filter(|((n, _), _)| n == f.name).collect();
                let last = mine
                    .iter()
                    .filter_map(|(_, p)| p.back().map(|(t, _)| *t))
                    .max();
                MetricDefinition {
                    name: f.name.to_string(),
                    description: f.help.to_string(),
                    metric_type: "gauge".into(),
                    labels: f.labels.iter().map(|l| l.to_string()).collect(),
                    unit: Some(f.unit.to_string()).filter(|u| !u.is_empty()),
                    help: f.help.to_string(),
                    scrape_interval: f.interval.into(),
                    retention: "24h".into(),
                    cardinality: mine.len() as u64,
                    last_scraped: last
                        .and_then(|t| Utc.timestamp_opt(t, 0).single())
                        .map(|t| t.to_rfc3339())
                        .unwrap_or_default(),
                    source: "horcrux".into(),
                    category: f.category.into(),
                }
            })
            .collect()
    }

    pub async fn query(&self, q: &MetricQuery) -> Result<MetricResult, String> {
        let mut parsed = parse(&q.metric)?;
        for (k, v) in &q.labels {
            parsed.matchers.push(Matcher::new(k, MatchOp::Eq, v)?);
        }
        let fam = family(&parsed.metric).ok_or_else(|| {
            format!(
                "unknown metric '{}' (GET /api/metrics/definitions lists the available metrics)",
                parsed.metric
            )
        })?;
        let end = q.end_time.unwrap_or_else(Utc::now).timestamp();
        let start = q.start_time.map(|t| t.timestamp()).unwrap_or(end - 3600);
        if start > end {
            return Err("start_time is after end_time".into());
        }
        let step = q.step.max(1) as i64;
        if (end - start) / step > MAX_POINTS_PER_QUERY {
            return Err(format!(
                "range/step gives more than {} points; use a larger step",
                MAX_POINTS_PER_QUERY
            ));
        }
        let bucket = |t: i64| start + ((t - start) / step) * step;

        let series = self.series.read().await;
        // Bucketed points per matching series.
        let mut selected: Vec<(&Labels, BTreeMap<i64, f64>)> = Vec::new();
        for ((name, labels), points) in series.iter() {
            if name != fam.name || !parsed.matchers.iter().all(|m| m.matches(labels)) {
                continue;
            }
            let mut b = BTreeMap::new();
            for (t, v) in points.iter().filter(|(t, _)| *t >= start && *t <= end) {
                b.insert(bucket(*t), *v);
            }
            if !b.is_empty() {
                selected.push((labels, b));
            }
        }

        let to_ts = |t: i64| Utc.timestamp_opt(t, 0).single().unwrap_or_else(Utc::now);
        let mut values = Vec::new();
        let mut result_labels = Labels::new();
        match &parsed.aggregation {
            None => {
                if selected.len() == 1 {
                    result_labels = selected[0].0.clone();
                }
                for (labels, points) in &selected {
                    for (t, v) in points {
                        values.push(MetricDataPoint {
                            timestamp: to_ts(*t),
                            value: *v,
                            labels: (*labels).clone(),
                        });
                    }
                }
            }
            Some((func, by)) => {
                let mut groups: BTreeMap<(Labels, i64), Vec<f64>> = BTreeMap::new();
                for (labels, points) in &selected {
                    let key: Labels = labels
                        .iter()
                        .filter(|(k, _)| by.contains(k))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    for (t, v) in points {
                        groups.entry((key.clone(), *t)).or_default().push(*v);
                    }
                }
                let distinct: std::collections::BTreeSet<&Labels> =
                    groups.keys().map(|(l, _)| l).collect();
                if distinct.len() == 1 {
                    result_labels = (*distinct.iter().next().unwrap()).clone();
                }
                for ((labels, t), vals) in &groups {
                    values.push(MetricDataPoint {
                        timestamp: to_ts(*t),
                        value: func.apply(vals),
                        labels: labels.clone(),
                    });
                }
            }
        }
        values.sort_by(|a, b| {
            a.timestamp
                .cmp(&b.timestamp)
                .then_with(|| a.labels.cmp(&b.labels))
        });
        Ok(MetricResult {
            metric: q.metric.trim().to_string(),
            labels: result_labels,
            values,
        })
    }
}

// ---------------------------------------------------------------------------
// Query parsing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AggFn {
    Sum,
    Avg,
    Min,
    Max,
    Count,
}

impl AggFn {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "sum" => AggFn::Sum,
            "avg" => AggFn::Avg,
            "min" => AggFn::Min,
            "max" => AggFn::Max,
            "count" => AggFn::Count,
            _ => return None,
        })
    }

    fn apply(&self, v: &[f64]) -> f64 {
        match self {
            AggFn::Sum => v.iter().sum(),
            AggFn::Avg => v.iter().sum::<f64>() / v.len().max(1) as f64,
            AggFn::Min => v.iter().cloned().fold(f64::INFINITY, f64::min),
            AggFn::Max => v.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            AggFn::Count => v.len() as f64,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum MatchOp {
    Eq,
    Ne,
    Re,
    NotRe,
}

#[derive(Debug, Clone)]
struct Matcher {
    label: String,
    op: MatchOp,
    value: String,
    re: Option<regex::Regex>,
}

impl Matcher {
    fn new(label: &str, op: MatchOp, value: &str) -> Result<Self, String> {
        let re = match op {
            MatchOp::Re | MatchOp::NotRe => Some(
                regex::Regex::new(&format!("^(?:{})$", value))
                    .map_err(|e| format!("bad regex for {}: {}", label, e))?,
            ),
            _ => None,
        };
        Ok(Self {
            label: label.to_string(),
            op,
            value: value.to_string(),
            re,
        })
    }

    fn matches(&self, labels: &Labels) -> bool {
        let v = labels.get(&self.label).map(String::as_str).unwrap_or("");
        match self.op {
            MatchOp::Eq => v == self.value,
            MatchOp::Ne => v != self.value,
            MatchOp::Re => self.re.as_ref().map(|r| r.is_match(v)).unwrap_or(false),
            MatchOp::NotRe => !self.re.as_ref().map(|r| r.is_match(v)).unwrap_or(false),
        }
    }
}

#[derive(Debug)]
struct Parsed {
    metric: String,
    matchers: Vec<Matcher>,
    aggregation: Option<(AggFn, Vec<String>)>,
}

fn is_ident(s: &str) -> bool {
    let mut c = s.chars();
    matches!(c.next(), Some(ch) if ch.is_ascii_alphabetic() || ch == '_' || ch == ':')
        && c.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == ':')
}

fn parse_selector(s: &str) -> Result<(String, Vec<Matcher>), String> {
    let s = s.trim();
    let (name, rest) = match s.find('{') {
        Some(i) => (s[..i].trim(), Some(&s[i..])),
        None => (s, None),
    };
    if !is_ident(name) {
        return Err(format!("expected a metric name, got '{}'", name));
    }
    let mut matchers = Vec::new();
    if let Some(rest) = rest {
        let inner = rest
            .strip_prefix('{')
            .and_then(|r| r.strip_suffix('}'))
            .ok_or_else(|| format!("unbalanced braces in '{}'", s))?;
        let mut chars = inner.chars().peekable();
        loop {
            while chars
                .peek()
                .map(|c| c.is_whitespace() || *c == ',')
                .unwrap_or(false)
            {
                chars.next();
            }
            if chars.peek().is_none() {
                break;
            }
            let mut label = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_ascii_alphanumeric() || c == '_' {
                    label.push(c);
                    chars.next();
                } else {
                    break;
                }
            }
            while chars.peek().map(|c| c.is_whitespace()).unwrap_or(false) {
                chars.next();
            }
            let mut op = String::new();
            while let Some(&c) = chars.peek() {
                if c == '=' || c == '!' || c == '~' {
                    op.push(c);
                    chars.next();
                } else {
                    break;
                }
            }
            let op = match op.as_str() {
                "=" => MatchOp::Eq,
                "!=" => MatchOp::Ne,
                "=~" => MatchOp::Re,
                "!~" => MatchOp::NotRe,
                o => return Err(format!("unsupported label operator '{}' in '{}'", o, s)),
            };
            while chars.peek().map(|c| c.is_whitespace()).unwrap_or(false) {
                chars.next();
            }
            if chars.next() != Some('"') {
                return Err(format!("label values must be double-quoted in '{}'", s));
            }
            let mut value = String::new();
            loop {
                match chars.next() {
                    Some('\\') => {
                        if let Some(c) = chars.next() {
                            value.push(c);
                        }
                    }
                    Some('"') => break,
                    Some(c) => value.push(c),
                    None => return Err(format!("unterminated label value in '{}'", s)),
                }
            }
            if label.is_empty() {
                return Err(format!("missing label name in '{}'", s));
            }
            matchers.push(Matcher::new(&label, op, &value)?);
        }
    }
    Ok((name.to_string(), matchers))
}

fn parse(query: &str) -> Result<Parsed, String> {
    let q = query.trim();
    if q.is_empty() {
        return Err("empty query".into());
    }
    // agg(selector) [by (a, b)]
    if let Some(open) = q.find('(') {
        let func = q[..open].trim();
        if let Some(f) = AggFn::parse(func) {
            // `by (...)` has its own parentheses: find the ')' closing agg(
            let mut depth = 0;
            let mut end = None;
            for (i, c) in q[open..].char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(open + i);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let end = end.ok_or_else(|| format!("unbalanced parentheses in '{}'", q))?;
            let (metric, matchers) = parse_selector(&q[open + 1..end])?;
            let tail = q[end + 1..].trim();
            let mut by = Vec::new();
            if !tail.is_empty() {
                let list = tail
                    .strip_prefix("by")
                    .map(str::trim)
                    .and_then(|t| t.strip_prefix('('))
                    .and_then(|t| t.strip_suffix(')'))
                    .ok_or_else(|| format!("expected 'by (labels)' after {}(...)", func))?;
                for l in list.split(',').map(str::trim).filter(|l| !l.is_empty()) {
                    if !is_ident(l) {
                        return Err(format!("bad label '{}' in by()", l));
                    }
                    by.push(l.to_string());
                }
            }
            return Ok(Parsed {
                metric,
                matchers,
                aggregation: Some((f, by)),
            });
        }
        return Err(format!(
            "unsupported function '{}' (supported: sum, avg, min, max, count)",
            func
        ));
    }
    let (metric, matchers) = parse_selector(q)?;
    Ok(Parsed {
        metric,
        matchers,
        aggregation: None,
    })
}

// ---------------------------------------------------------------------------
// Sampler
// ---------------------------------------------------------------------------

fn labels(pairs: &[(&str, &str)]) -> Labels {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Start the background sampler feeding `store`.
pub fn start_sampler(
    store: std::sync::Arc<MetricsStore>,
    monitoring: std::sync::Arc<crate::monitoring::MonitoringManager>,
    data_dir: std::path::PathBuf,
) {
    tokio::spawn(async move {
        let node = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "localhost".into());
        let mut prev_cpu = crate::metrics::system::read_cpu_stats().ok();
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(SAMPLE_INTERVAL_SECS));
        tick.tick().await;
        // First CPU delta needs a short baseline, not a full interval.
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        loop {
            sample_once(&store, &monitoring, &node, &data_dir, &mut prev_cpu).await;
            tick.tick().await;
        }
    });
}

async fn sample_once(
    store: &MetricsStore,
    monitoring: &crate::monitoring::MonitoringManager,
    node: &str,
    data_dir: &std::path::Path,
    prev_cpu: &mut Option<crate::metrics::system::CpuStats>,
) {
    use crate::metrics::system;
    let now = Utc::now().timestamp();
    let n = labels(&[("node", node)]);
    if let Ok(cpu) = system::read_cpu_stats() {
        if let Some(prev) = prev_cpu.as_ref() {
            store
                .record(
                    "horcrux_node_cpu_usage_percent",
                    n.clone(),
                    now,
                    cpu.usage_percent(prev),
                )
                .await;
        }
        *prev_cpu = Some(cpu);
    }
    if let Ok(mem) = system::read_memory_stats() {
        store
            .record(
                "horcrux_node_memory_used_bytes",
                n.clone(),
                now,
                mem.used() as f64,
            )
            .await;
        store
            .record(
                "horcrux_node_memory_total_bytes",
                n.clone(),
                now,
                mem.total as f64,
            )
            .await;
        store
            .record(
                "horcrux_node_memory_usage_percent",
                n.clone(),
                now,
                mem.usage_percent(),
            )
            .await;
    }
    if let Ok(load) = system::read_load_average() {
        store
            .record("horcrux_node_load1", n.clone(), now, load.one_min)
            .await;
        store
            .record("horcrux_node_load5", n.clone(), now, load.five_min)
            .await;
        store
            .record("horcrux_node_load15", n.clone(), now, load.fifteen_min)
            .await;
    }
    if let Ok(up) = system::read_uptime() {
        store
            .record("horcrux_node_uptime_seconds", n.clone(), now, up as f64)
            .await;
    }
    let dd = data_dir.to_string_lossy().to_string();
    if let Ok(pct) = system::read_disk_usage_percent(&dd) {
        store
            .record(
                "horcrux_node_disk_usage_percent",
                labels(&[("node", node), ("path", &dd)]),
                now,
                pct,
            )
            .await;
    }
    for m in monitoring.list_vm_metrics().await {
        let l = labels(&[("vm_id", &m.id), ("name", &m.name)]);
        store
            .record(
                "horcrux_vm_cpu_usage_percent",
                l.clone(),
                m.timestamp,
                m.cpu.usage_percent,
            )
            .await;
        store
            .record(
                "horcrux_vm_memory_used_bytes",
                l,
                m.timestamp,
                m.memory.used_bytes as f64,
            )
            .await;
    }
    for m in monitoring.list_container_metrics().await {
        let l = labels(&[("container_id", &m.id), ("name", &m.name)]);
        store
            .record(
                "horcrux_container_cpu_usage_percent",
                l.clone(),
                m.timestamp,
                m.cpu.usage_percent,
            )
            .await;
        store
            .record(
                "horcrux_container_memory_used_bytes",
                l,
                m.timestamp,
                m.memory.used_bytes as f64,
            )
            .await;
    }
    for s in monitoring.list_storage_metrics().await {
        let l = labels(&[("pool", &s.name), ("type", &s.storage_type)]);
        store
            .record(
                "horcrux_storage_used_bytes",
                l.clone(),
                now,
                s.used_bytes as f64,
            )
            .await;
        store
            .record(
                "horcrux_storage_total_bytes",
                l.clone(),
                now,
                s.total_bytes as f64,
            )
            .await;
        store
            .record("horcrux_storage_usage_percent", l, now, s.usage_percent)
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store_with_data() -> (MetricsStore, i64) {
        let s = MetricsStore::new();
        let t0 = 1_800_000_000;
        for i in 0..10 {
            s.record(
                "horcrux_vm_cpu_usage_percent",
                labels(&[("vm_id", "a"), ("name", "web")]),
                t0 + i * 60,
                10.0 + i as f64,
            )
            .await;
            s.record(
                "horcrux_vm_cpu_usage_percent",
                labels(&[("vm_id", "b"), ("name", "db")]),
                t0 + i * 60,
                30.0,
            )
            .await;
        }
        (s, t0)
    }

    fn q(metric: &str, t0: i64) -> MetricQuery {
        MetricQuery {
            metric: metric.into(),
            labels: HashMap::new(),
            start_time: Utc.timestamp_opt(t0, 0).single(),
            end_time: Utc.timestamp_opt(t0 + 600, 0).single(),
            step: 60,
        }
    }

    #[tokio::test]
    async fn raw_selector_returns_every_series() {
        let (s, t0) = store_with_data().await;
        let r = s
            .query(&q("horcrux_vm_cpu_usage_percent", t0))
            .await
            .unwrap();
        assert_eq!(r.values.len(), 20);
        let r = s
            .query(&q("horcrux_vm_cpu_usage_percent{ name=\"web\" }", t0))
            .await
            .unwrap();
        assert_eq!(r.values.len(), 10);
        assert_eq!(r.labels.get("vm_id").map(String::as_str), Some("a"));
        let r = s
            .query(&q(
                "horcrux_vm_cpu_usage_percent{name=~\"w.*\", vm_id!=\"b\"}",
                t0,
            ))
            .await
            .unwrap();
        assert_eq!(r.values.len(), 10);
    }

    #[tokio::test]
    async fn aggregations() {
        let (s, t0) = store_with_data().await;
        let r = s
            .query(&q("avg(horcrux_vm_cpu_usage_percent)", t0))
            .await
            .unwrap();
        assert_eq!(r.values.len(), 10);
        assert_eq!(r.values[0].value, 20.0); // (10 + 30) / 2
        let r = s
            .query(&q("max(horcrux_vm_cpu_usage_percent)", t0))
            .await
            .unwrap();
        assert_eq!(r.values[9].value, 30.0);
        let r = s
            .query(&q("sum(horcrux_vm_cpu_usage_percent) by (name)", t0))
            .await
            .unwrap();
        assert_eq!(r.values.len(), 20);
        assert!(r.values.iter().all(|p| p.labels.contains_key("name")));
        let r = s
            .query(&q("count(horcrux_vm_cpu_usage_percent)", t0))
            .await
            .unwrap();
        assert_eq!(r.values[0].value, 2.0);
    }

    #[tokio::test]
    async fn step_buckets_and_errors() {
        let (s, t0) = store_with_data().await;
        let mut query = q("horcrux_vm_cpu_usage_percent{vm_id=\"a\"}", t0);
        query.step = 300;
        let r = s.query(&query).await.unwrap();
        assert_eq!(r.values.len(), 2);
        assert_eq!(r.values[0].value, 14.0); // last sample in the first 5 minutes
        assert!(s.query(&q("nope_metric", t0)).await.is_err());
        assert!(s
            .query(&q("rate(horcrux_vm_cpu_usage_percent)", t0))
            .await
            .is_err());
        assert!(s
            .query(&q("horcrux_vm_cpu_usage_percent{a=b}", t0))
            .await
            .is_err());
        let defs = s.definitions().await;
        let vm = defs
            .iter()
            .find(|d| d.name == "horcrux_vm_cpu_usage_percent")
            .unwrap();
        assert_eq!(vm.cardinality, 2);
        assert!(!vm.last_scraped.is_empty());
    }

    #[tokio::test]
    async fn retention_drops_old_points() {
        let s = MetricsStore::new();
        s.record("horcrux_node_load1", Labels::new(), 0, 1.0).await;
        s.record(
            "horcrux_node_load1",
            Labels::new(),
            RETENTION_SECS + 10,
            2.0,
        )
        .await;
        let series = s.series.read().await;
        assert_eq!(series.values().next().unwrap().len(), 1);
    }
}
