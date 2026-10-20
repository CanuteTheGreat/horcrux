//! Live per-node CPU/memory usage for the cluster node list.
//!
//! Every horcrux-api instance samples its own host from procfs
//! ([`LocalUsageSampler`]) and serves the latest sample at
//! `GET /api/cluster/local-usage`. The [`ClusterManager`](super::ClusterManager)
//! usage monitor ([`super::ClusterManager::start_usage_monitor`]) refreshes
//! every registered node on a fixed interval: the local node is sampled
//! directly, every other node is asked for its `local-usage` over HTTP with a
//! short-lived service JWT (cluster nodes share `JWT_SECRET`, the same thing
//! that lets one login work against every node). The outcome is folded into
//! the node record by [`apply_poll`], which also drives the node's
//! online/offline status -- the poll *is* the cluster heartbeat.
//!
//! Anything that can't be measured stays `None` ("unknown") all the way to
//! the UI/CLI. It is never reported as 0, because 0% CPU / 0 bytes used is
//! exactly what makes a dead node look like the best migration target.

use super::node::{Architecture, Node, NodeStatus};
use crate::metrics::system::{self, CpuStats};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::sync::Mutex;

/// How often the usage monitor polls every node.
pub const POLL_INTERVAL: Duration = Duration::from_secs(10);

/// Per-node HTTP timeout for a remote usage fetch (connect + response).
/// Kept well under [`POLL_INTERVAL`] so one dead node can't stall a round.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(4);

/// How long a node may go without a successful report before its last
/// usage figures are discarded and it is marked offline. Three missed
/// polls, so a single dropped request doesn't flap the node's status.
pub const STALE_AFTER_SECS: i64 = 30;

/// Gap between the two /proc/stat reads when there is no previous sample
/// to diff against (first sample after startup).
const FIRST_SAMPLE_WINDOW: Duration = Duration::from_millis(250);

/// A node's own report of its live usage (`GET /api/cluster/local-usage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeUsage {
    /// CPU utilisation in percent (0-100), averaged over the sampling
    /// window (the interval since the previous sample).
    pub cpu_usage: f64,
    /// RAM in use, bytes (MemTotal - MemAvailable).
    pub memory_used: u64,
    /// Total RAM, bytes.
    pub memory_total: u64,
    /// Online CPU count.
    pub cpu_cores: u32,
    /// Host CPU architecture.
    pub architecture: Architecture,
    /// Unix timestamp (seconds) the sample was taken.
    pub timestamp: i64,
    /// Persistent identity of the reporting Horcrux instance (absent from
    /// older nodes). See [`check_identity`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
}

/// Result of one usage poll of one node.
#[derive(Debug, Clone, PartialEq)]
pub enum PollOutcome {
    /// The node reported its usage.
    Usage(NodeUsage),
    /// Something answered at the node's address, but not with usage data
    /// (auth rejected because the node uses a different JWT secret, an
    /// older Horcrux without the endpoint, an unparseable body, ...); the
    /// string says what came back. For a node without a pinned identity
    /// this means "up, usage unknown"; for a pinned node it can't prove the
    /// answer came from that node (see [`check_identity`]).
    ReachableNoUsage(String),
    /// Whatever answered at a pinned node's address couldn't prove it is
    /// that node (a different identity, no identity, or no usage reply at
    /// all); the string says why. Counts like [`PollOutcome::Unreachable`]
    /// for the node's status, with the reason recorded on the node.
    Unverified(String),
    /// No answer at all (connection refused, timeout, DNS failure).
    Unreachable,
}

/// Fold one poll outcome into a node record. Pure so it can be unit-tested.
pub fn apply_poll(node: &mut Node, outcome: PollOutcome, now: i64) {
    match outcome {
        PollOutcome::Usage(usage) => {
            if node.node_id.is_none() {
                node.node_id = usage.node_id.clone();
            }
            node.status = NodeStatus::Online;
            node.status_reason = None;
            node.cpu_usage = Some(usage.cpu_usage.clamp(0.0, 100.0));
            node.memory_used = Some(usage.memory_used);
            node.last_seen = Some(now);
            // Fill in hardware facts the operator didn't know when adding
            // the node (the UI/CLI add form only asks for an address).
            if usage.memory_total > 0 {
                node.memory_total = usage.memory_total;
            }
            if usage.cpu_cores > 0 {
                node.cpu_cores = usage.cpu_cores;
            }
            if node.architecture == Architecture::Unknown
                && usage.architecture != Architecture::Unknown
            {
                node.architecture = usage.architecture;
            }
        }
        PollOutcome::ReachableNoUsage(why) => {
            node.status = NodeStatus::Online;
            node.status_reason = Some(format!("up, usage unknown: {why}"));
            node.cpu_usage = None;
            node.memory_used = None;
        }
        PollOutcome::Unverified(why) => mark_missed(node, why, now),
        PollOutcome::Unreachable => mark_missed(node, "not answering".to_string(), now),
    }
}

/// A poll that didn't hear from the node itself: it goes offline (usage
/// unknown) once it hasn't reported for [`STALE_AFTER_SECS`].
fn mark_missed(node: &mut Node, why: String, now: i64) {
    let recently_seen = node
        .last_seen
        .map(|seen| now - seen <= STALE_AFTER_SECS)
        .unwrap_or(false);
    node.status_reason = Some(why);
    if !recently_seen {
        node.status = NodeStatus::Offline;
        node.cpu_usage = None;
        node.memory_used = None;
    }
}

/// Memory in use from a /proc/meminfo snapshot: MemTotal - MemAvailable
/// (what the kernel can't hand to a new workload without reclaim/swap).
/// Falls back to total - free - buffers - cached on kernels without
/// MemAvailable.
pub fn memory_in_use(mem: &system::MemoryStats) -> u64 {
    if mem.available > 0 {
        mem.total.saturating_sub(mem.available)
    } else {
        mem.used()
    }
}

/// Once a node's identity is pinned, only a usage report carrying that
/// identity proves the node is up. Anything else that answers at its
/// address becomes [`PollOutcome::Unverified`]: a different Horcrux
/// instance (the address now belongs to another node, e.g. a restarted
/// container got a dead node's IP - possibly this very node, polling
/// itself), a report without an identity, or a reply that isn't a usage
/// report at all (401 from a Horcrux with another JWT secret, any other
/// web server's error page or body). Without that check a dead node is
/// shown online and stays a placement target. An unpinned node (older
/// Horcrux, or not reported yet) takes the identity of whoever answers
/// first (see `apply_poll`), which is also how this node's own entry,
/// registered by address, pins this node; until then a non-usage reply
/// still means "up, usage unknown".
pub fn check_identity(node: &Node, outcome: PollOutcome) -> PollOutcome {
    let Some(pinned) = node.node_id.as_deref() else {
        return outcome;
    };
    let why = match &outcome {
        PollOutcome::Usage(u) => match u.node_id.as_deref() {
            Some(got) if got == pinned => return outcome,
            Some(got) => format!(
                "unverified: answered by a different Horcrux instance ({got} instead of {pinned}); \
                 remove and re-add the node if it was reinstalled"
            ),
            None => format!("unverified: the reply carries no node identity (expected {pinned})"),
        },
        PollOutcome::ReachableNoUsage(reply) => {
            format!("unverified: {reply}, which doesn't prove it is node {pinned}")
        }
        PollOutcome::Unverified(_) | PollOutcome::Unreachable => return outcome,
    };
    tracing::warn!("Cluster node {} ({}): {}", node.name, node.ip, why);
    PollOutcome::Unverified(why)
}

/// "No limit" as cgroup v1 reports it (page-rounded i64::MAX) and anything
/// above it.
const CGROUP_V1_UNLIMITED: u64 = 0x7FFF_FFFF_FFFF_F000;

fn read_u64(p: &std::path::Path) -> Option<u64> {
    std::fs::read_to_string(p).ok()?.trim().parse().ok()
}

fn stat_field(p: &std::path::Path, key: &str) -> u64 {
    std::fs::read_to_string(p)
        .ok()
        .and_then(|s| {
            s.lines().find_map(|l| {
                let (k, v) = l.split_once(' ')?;
                (k == key).then(|| v.trim().parse().ok()).flatten()
            })
        })
        .unwrap_or(0)
}

/// This process's cgroup memory limit and usage (bytes), when it runs
/// under a limit: cgroup v2 `memory.max` (the smallest one up the
/// hierarchy) / `memory.current`, or v1 `memory.limit_in_bytes` /
/// `memory.usage_in_bytes`; usage excludes inactive page cache like
/// MemAvailable does. `root` is the cgroup mount (/sys/fs/cgroup),
/// `self_cgroup` the text of /proc/self/cgroup. `None` = no limit.
pub fn cgroup_memory(root: &std::path::Path, self_cgroup: &str) -> Option<(u64, u64)> {
    // cgroup v2: "0::/path"
    if let Some(path) = self_cgroup.lines().find_map(|l| l.strip_prefix("0::")) {
        let mut dir = root.join(path.trim().trim_start_matches('/'));
        if !dir.join("memory.max").exists() {
            dir = root.to_path_buf(); // cgroup namespace: we are at the root
        }
        let mut limit: Option<u64> = None;
        let mut d = dir.clone();
        loop {
            if let Some(v) = read_u64(&d.join("memory.max")) {
                limit = Some(limit.map_or(v, |l: u64| l.min(v)));
            }
            if d == root || !d.pop() || !d.starts_with(root) {
                break;
            }
        }
        let limit = limit?;
        let current = read_u64(&dir.join("memory.current"))?;
        let inactive = stat_field(&dir.join("memory.stat"), "inactive_file");
        return Some((limit, current.saturating_sub(inactive).min(limit)));
    }
    // cgroup v1: "N:memory:/path" (or "N:...,memory,...:/path")
    let path = self_cgroup.lines().find_map(|l| {
        let mut parts = l.splitn(3, ':');
        let (_, ctrls, path) = (parts.next()?, parts.next()?, parts.next()?);
        ctrls.split(',').any(|c| c == "memory").then_some(path)
    })?;
    let base = root.join("memory");
    let mut dir = base.join(path.trim().trim_start_matches('/'));
    if !dir.join("memory.limit_in_bytes").exists() {
        dir = base;
    }
    let limit = read_u64(&dir.join("memory.limit_in_bytes"))?;
    if limit >= CGROUP_V1_UNLIMITED {
        return None;
    }
    let usage = read_u64(&dir.join("memory.usage_in_bytes"))?;
    let inactive = stat_field(&dir.join("memory.stat"), "total_inactive_file");
    Some((limit, usage.saturating_sub(inactive).min(limit)))
}

/// Host memory (total, in use), narrowed to this process's cgroup limit
/// when that is smaller than the host's RAM (a containerised node can only
/// place workloads within its limit).
pub fn effective_memory(host_total: u64, host_used: u64, cgroup: Option<(u64, u64)>) -> (u64, u64) {
    match cgroup {
        Some((limit, used)) if limit > 0 && limit < host_total => (limit, used.min(limit)),
        _ => (host_total, host_used),
    }
}

/// Samples this host's CPU and memory from procfs. CPU utilisation is a
/// real delta between consecutive /proc/stat reads (not the since-boot
/// average a single read gives you).
pub struct LocalUsageSampler {
    prev_cpu: Mutex<Option<CpuStats>>,
}

impl Default for LocalUsageSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalUsageSampler {
    pub fn new() -> Self {
        Self {
            prev_cpu: Mutex::new(None),
        }
    }

    /// Take a sample. Returns `None` if procfs can't be read (non-Linux
    /// host, restricted container), i.e. usage is genuinely unknown.
    pub async fn sample(&self) -> Option<NodeUsage> {
        let mut prev_lock = self.prev_cpu.lock().await;

        let prev = match prev_lock.take() {
            Some(prev) => prev,
            None => {
                let first = system::read_cpu_stats().ok()?;
                tokio::time::sleep(FIRST_SAMPLE_WINDOW).await;
                first
            }
        };
        let current = system::read_cpu_stats().ok()?;
        let cpu_usage = current.usage_percent(&prev).clamp(0.0, 100.0);
        *prev_lock = Some(current);
        drop(prev_lock);

        let mem = system::read_memory_stats().ok()?;
        if mem.total == 0 {
            return None;
        }
        let cgroup = std::fs::read_to_string("/proc/self/cgroup")
            .ok()
            .and_then(|c| cgroup_memory(std::path::Path::new("/sys/fs/cgroup"), &c));
        let (memory_total, memory_used) = effective_memory(mem.total, memory_in_use(&mem), cgroup);

        Some(NodeUsage {
            cpu_usage,
            memory_used,
            memory_total,
            cpu_cores: system::get_cpu_count() as u32,
            architecture: Architecture::detect(),
            timestamp: chrono::Utc::now().timestamp(),
            node_id: None,
        })
    }
}

/// Ask a remote node for its usage.
pub async fn fetch_remote_usage(
    client: &reqwest::Client,
    base_url: &str,
    bearer_token: Option<&str>,
) -> PollOutcome {
    let mut request = client
        .get(format!("{}/api/cluster/local-usage", base_url))
        .timeout(FETCH_TIMEOUT);
    if let Some(token) = bearer_token {
        request = request.bearer_auth(token);
    }

    match request.send().await {
        Ok(resp) if resp.status().is_success() => match resp.json::<NodeUsage>().await {
            Ok(usage) => PollOutcome::Usage(usage),
            Err(e) => {
                tracing::debug!("{}: unparseable local-usage response: {}", base_url, e);
                PollOutcome::ReachableNoUsage("the reply is not a usage report".to_string())
            }
        },
        Ok(resp) => {
            tracing::debug!("{}: local-usage answered HTTP {}", base_url, resp.status());
            PollOutcome::ReachableNoUsage(format!("local-usage answered HTTP {}", resp.status()))
        }
        Err(e) => {
            tracing::debug!("{}: usage poll failed: {}", base_url, e);
            PollOutcome::Unreachable
        }
    }
}

/// Whether a registered node is this host, so it can be sampled directly
/// instead of over HTTP.
pub fn is_this_host(node: &Node, local_hostname: &str) -> bool {
    if node.is_local {
        return true;
    }
    if !local_hostname.is_empty()
        && (node.name.eq_ignore_ascii_case(local_hostname)
            || node.ip.eq_ignore_ascii_case(local_hostname))
    {
        return true;
    }
    let ip = node.ip.trim_start_matches('[').trim_end_matches(']');
    ip.eq_ignore_ascii_case("localhost")
        || ip
            .parse::<std::net::IpAddr>()
            .map(|addr| addr.is_loopback())
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, ip: &str) -> Node {
        let req: super::super::node::AddNodeRequest =
            serde_json::from_value(serde_json::json!({ "address": ip })).unwrap();
        req.into_node(name.to_string(), 1)
    }

    fn usage(cpu: f64, used: u64) -> NodeUsage {
        NodeUsage {
            cpu_usage: cpu,
            memory_used: used,
            memory_total: 16 * 1024 * 1024 * 1024,
            cpu_cores: 8,
            architecture: Architecture::Aarch64,
            timestamp: 1_000,
            node_id: None,
        }
    }

    #[test]
    fn report_marks_node_online_and_fills_usage_and_hardware() {
        let mut n = node("n1", "10.0.0.1");
        assert_eq!(n.cpu_usage, None);
        assert_eq!(n.memory_used, None);
        assert_eq!(n.cpu_cores, 0);
        apply_poll(&mut n, PollOutcome::Usage(usage(37.5, 4 << 30)), 1_000);
        assert_eq!(n.status, NodeStatus::Online);
        assert_eq!(n.cpu_usage, Some(37.5));
        assert_eq!(n.memory_used, Some(4 << 30));
        assert_eq!(n.last_seen, Some(1_000));
        assert_eq!(n.cpu_cores, 8);
        assert_eq!(n.memory_total, 16 << 30);
        assert_eq!(n.architecture, Architecture::Aarch64);
    }

    #[test]
    fn operator_supplied_architecture_is_not_overwritten() {
        let mut n = node("n1", "10.0.0.1");
        n.architecture = Architecture::X86_64;
        apply_poll(&mut n, PollOutcome::Usage(usage(1.0, 1)), 1_000);
        assert_eq!(n.architecture, Architecture::X86_64);
    }

    #[test]
    fn never_seen_unreachable_node_is_offline_with_unknown_usage() {
        let mut n = node("n1", "10.0.0.1");
        n.status = NodeStatus::Online; // operator claimed it was up
        apply_poll(&mut n, PollOutcome::Unreachable, 1_000);
        assert_eq!(n.status, NodeStatus::Offline);
        assert_eq!(n.cpu_usage, None);
        assert_eq!(n.memory_used, None);
    }

    #[test]
    fn single_missed_poll_keeps_last_figures_until_stale() {
        let mut n = node("n1", "10.0.0.1");
        apply_poll(&mut n, PollOutcome::Usage(usage(20.0, 1 << 30)), 1_000);

        apply_poll(&mut n, PollOutcome::Unreachable, 1_000 + STALE_AFTER_SECS);
        assert_eq!(n.status, NodeStatus::Online);
        assert_eq!(n.cpu_usage, Some(20.0));

        apply_poll(&mut n, PollOutcome::Unreachable, 1_001 + STALE_AFTER_SECS);
        assert_eq!(n.status, NodeStatus::Offline);
        assert_eq!(n.cpu_usage, None, "stale usage must become unknown, not 0");
        assert_eq!(n.memory_used, None);
        assert_eq!(n.last_seen, Some(1_000));
    }

    #[test]
    fn reachable_without_usage_is_online_but_unknown() {
        let mut n = node("n1", "10.0.0.1");
        apply_poll(&mut n, PollOutcome::Usage(usage(20.0, 1 << 30)), 1_000);
        apply_poll(
            &mut n,
            PollOutcome::ReachableNoUsage("HTTP 401".into()),
            1_010,
        );
        assert_eq!(n.status, NodeStatus::Online);
        assert_eq!(n.cpu_usage, None);
        assert_eq!(n.memory_used, None);
        assert_eq!(
            n.status_reason.as_deref(),
            Some("up, usage unknown: HTTP 401")
        );
    }

    #[test]
    fn cpu_usage_is_clamped_to_percent_range() {
        let mut n = node("n1", "10.0.0.1");
        apply_poll(&mut n, PollOutcome::Usage(usage(140.0, 1)), 1_000);
        assert_eq!(n.cpu_usage, Some(100.0));
    }

    fn from(id: Option<&str>) -> PollOutcome {
        let mut u = usage(5.0, 1 << 30);
        u.node_id = id.map(str::to_string);
        PollOutcome::Usage(u)
    }

    #[test]
    fn a_node_pins_the_identity_of_its_first_report() {
        let mut n = node("node3", "172.18.0.4");
        let o = check_identity(&n, from(Some("id-node3")));
        apply_poll(&mut n, o, 1_000);
        assert_eq!(n.status, NodeStatus::Online);
        assert_eq!(n.node_id.as_deref(), Some("id-node3"));
        // the same instance keeps it online
        let o = check_identity(&n, from(Some("id-node3")));
        assert!(matches!(o, PollOutcome::Usage(_)));
    }

    #[test]
    fn a_different_instance_at_a_dead_nodes_address_does_not_make_it_online() {
        // node3 died; node2 restarted onto node3's old address
        let mut n = node("node3", "172.18.0.4");
        n.node_id = Some("id-node3".into());
        let o = check_identity(&n, from(Some("id-node2")));
        assert!(
            matches!(&o, PollOutcome::Unverified(w) if w.contains("id-node2 instead of id-node3")),
            "{o:?}"
        );
        apply_poll(&mut n, o, 1_000);
        assert_eq!(n.status, NodeStatus::Offline);
        assert!(n
            .status_reason
            .as_deref()
            .unwrap()
            .starts_with("unverified"));
        assert_eq!(n.cpu_usage, None);
        assert_eq!(n.node_id.as_deref(), Some("id-node3"), "pin kept");
    }

    #[test]
    fn a_node_polling_itself_through_a_dead_nodes_address_is_not_that_node() {
        // node1 restarted onto node3's old address: polling "node3" reaches
        // node1 itself, whose identity isn't node3's pinned one
        let mut n = node("node3", "172.18.0.4");
        n.node_id = Some("id-node3".into());
        assert!(matches!(
            check_identity(&n, from(Some("id-node1"))),
            PollOutcome::Unverified(_)
        ));
        // ... while this node's own entry (registered by its address) pins
        // and keeps this node
        let mut own = node("node1", "172.18.0.2");
        let o = check_identity(&own, from(Some("id-node1")));
        apply_poll(&mut own, o, 1_000);
        assert_eq!(own.status, NodeStatus::Online);
        assert!(matches!(
            check_identity(&own, from(Some("id-node1"))),
            PollOutcome::Usage(_)
        ));
    }

    #[test]
    fn unpinned_older_nodes_keep_todays_behaviour() {
        // an older Horcrux never sends an identity, so it is never pinned
        let mut n = node("old", "10.0.0.9");
        let o = check_identity(&n, from(None));
        assert!(matches!(o, PollOutcome::Usage(_)));
        apply_poll(&mut n, o, 1_000);
        assert_eq!(n.node_id, None);
        assert_eq!(n.status, NodeStatus::Online);
        assert_eq!(n.status_reason, None);
        let o = check_identity(&n, PollOutcome::ReachableNoUsage("HTTP 401".into()));
        assert_eq!(o, PollOutcome::ReachableNoUsage("HTTP 401".into()));
        apply_poll(&mut n, o, 1_010);
        assert_eq!(n.status, NodeStatus::Online, "up, usage unknown");
    }

    #[test]
    fn a_pinned_node_whose_reply_cant_prove_its_identity_is_not_online() {
        for reply in [
            PollOutcome::ReachableNoUsage("local-usage answered HTTP 401 Unauthorized".into()),
            PollOutcome::ReachableNoUsage("local-usage answered HTTP 404 Not Found".into()),
            PollOutcome::ReachableNoUsage("the reply is not a usage report".into()),
            from(None),
        ] {
            let mut n = node("node3", "172.31.250.13");
            n.node_id = Some("id-node3".into());
            let o = check_identity(&n, reply.clone());
            assert!(
                matches!(o, PollOutcome::Unverified(_)),
                "{reply:?} -> {o:?}"
            );
            // never seen: offline straight away, with the reason
            apply_poll(&mut n, o.clone(), 1_000);
            assert_eq!(n.status, NodeStatus::Offline, "{reply:?}");
            assert_eq!(n.cpu_usage, None);
            let why = n.status_reason.clone().unwrap();
            assert!(
                why.starts_with("unverified") && why.contains("id-node3"),
                "{why}"
            );

            // seen a moment ago: one unverified poll doesn't flap it, but it
            // goes offline once the last real report is stale
            let mut n2 = node("node3", "172.31.250.13");
            apply_poll(&mut n2, from(Some("id-node3")), 1_000);
            assert_eq!(n2.status, NodeStatus::Online);
            apply_poll(&mut n2, o.clone(), 1_010);
            assert_eq!(n2.status, NodeStatus::Online);
            assert!(n2
                .status_reason
                .as_deref()
                .unwrap()
                .starts_with("unverified"));
            apply_poll(&mut n2, o, 1_001 + STALE_AFTER_SECS);
            assert_eq!(n2.status, NodeStatus::Offline);
            assert_eq!(
                n2.last_seen,
                Some(1_000),
                "an unverified reply is not a sighting"
            );
            // the real node back: online, reason cleared
            let back = check_identity(&n2, from(Some("id-node3")));
            apply_poll(&mut n2, back, 1_100);
            assert_eq!(n2.status, NodeStatus::Online);
            assert_eq!(n2.status_reason, None);
        }
    }

    fn cg(dir: &std::path::Path, files: &[(&str, &str)]) {
        for (p, v) in files {
            let f = dir.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, v).unwrap();
        }
    }

    #[test]
    fn node_memory_honours_a_cgroup_v2_limit() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        cg(
            r,
            &[
                ("system.slice/memory.max", "max\n"),
                ("system.slice/hx.service/memory.max", "1610612736\n"),
                ("system.slice/hx.service/memory.current", "524288000\n"),
                (
                    "system.slice/hx.service/memory.stat",
                    "anon 1\ninactive_file 104857600\n",
                ),
            ],
        );
        let got = cgroup_memory(r, "0::/system.slice/hx.service\n");
        assert_eq!(got, Some((1_610_612_736, 524_288_000 - 104_857_600)));
        // a 1.5 GB-capped node on a 30.7 GB host reports 1.5 GB
        let host = 30_700_000_000u64;
        assert_eq!(
            effective_memory(host, 9_000_000_000, got),
            (1_610_612_736, 419_430_400)
        );
        // a parent's lower limit applies too
        cg(r, &[("system.slice/memory.max", "1073741824\n")]);
        assert_eq!(
            cgroup_memory(r, "0::/system.slice/hx.service\n").unwrap().0,
            1_073_741_824
        );
        // no limit anywhere -> host figures
        let free = tempfile::tempdir().unwrap();
        cg(
            free.path(),
            &[("memory.max", "max\n"), ("memory.current", "1\n")],
        );
        assert_eq!(cgroup_memory(free.path(), "0::/\n"), None);
        assert_eq!(effective_memory(host, 5, None), (host, 5));
    }

    #[test]
    fn node_memory_honours_a_cgroup_v1_limit_and_namespace_root() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        // container view: the host path from /proc/self/cgroup isn't mounted,
        // the container's own cgroup is the memory controller root
        cg(
            r,
            &[
                ("memory/memory.limit_in_bytes", "2147483648\n"),
                ("memory/memory.usage_in_bytes", "1073741824\n"),
                (
                    "memory/memory.stat",
                    "cache 5\ntotal_inactive_file 73741824\n",
                ),
            ],
        );
        let self_cg = "12:cpuset:/docker/abc\n4:memory:/docker/abc\n";
        assert_eq!(
            cgroup_memory(r, self_cg),
            Some((2_147_483_648, 1_000_000_000))
        );
        cg(
            r,
            &[("memory/memory.limit_in_bytes", "9223372036854771712\n")],
        );
        assert_eq!(cgroup_memory(r, self_cg), None, "v1 'unlimited'");
        // a limit above the host's RAM changes nothing
        assert_eq!(effective_memory(1000, 400, Some((5000, 600))), (1000, 400));
    }

    #[test]
    fn node_usage_wire_shape() {
        let json = serde_json::to_value(usage(12.5, 2048)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "cpu_usage": 12.5,
                "memory_used": 2048,
                "memory_total": 17179869184u64,
                "cpu_cores": 8,
                "architecture": "aarch64",
                "timestamp": 1000
            })
        );
        let back: NodeUsage = serde_json::from_value(json).unwrap();
        assert_eq!(back, usage(12.5, 2048));
    }

    #[test]
    fn memory_in_use_prefers_mem_available() {
        let mem = system::MemoryStats {
            total: 1000,
            free: 100,
            available: 600,
            buffers: 50,
            cached: 250,
        };
        assert_eq!(memory_in_use(&mem), 400);
        let old_kernel = system::MemoryStats {
            available: 0,
            ..mem
        };
        assert_eq!(memory_in_use(&old_kernel), 600);
    }

    #[test]
    fn this_host_detection() {
        assert!(is_this_host(&node("a", "127.0.0.1"), "box"));
        assert!(is_this_host(&node("a", "::1"), "box"));
        assert!(is_this_host(&node("a", "[::1]"), "box"));
        assert!(is_this_host(&node("a", "localhost"), "box"));
        assert!(is_this_host(&node("box", "10.0.0.5"), "box"));
        assert!(is_this_host(&node("a", "BOX"), "box"));
        assert!(!is_this_host(&node("a", "10.0.0.5"), "box"));
        let mut local = node("a", "10.0.0.5");
        local.is_local = true;
        assert!(is_this_host(&local, "box"));
    }

    #[tokio::test]
    async fn local_sampler_reports_real_values_on_linux() {
        if !std::path::Path::new("/proc/stat").exists() {
            return;
        }
        let sampler = LocalUsageSampler::new();
        let first = sampler.sample().await.expect("procfs sample");
        assert!((0.0..=100.0).contains(&first.cpu_usage));
        assert!(first.memory_total > 0);
        assert!(first.memory_used > 0 && first.memory_used <= first.memory_total);
        assert!(first.cpu_cores > 0);
        // Second sample diffs against the first one's counters.
        let second = sampler.sample().await.expect("procfs sample");
        assert!((0.0..=100.0).contains(&second.cpu_usage));
    }

    /// An HTTP server on loopback answering every request with `status` and
    /// `body`; returns its base URL.
    async fn canned_server(status: &'static str, body: &'static str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                let mut buf = vec![0u8; 4096];
                let mut got = Vec::new();
                while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => got.extend_from_slice(&buf[..n]),
                    }
                }
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(resp.as_bytes()).await;
                let _ = s.shutdown().await;
            }
        });
        base
    }

    /// Poll a pinned node served at `base` the way the usage monitor does.
    async fn poll_pinned(base: &str) -> (PollOutcome, Node) {
        let client = reqwest::Client::new();
        let raw = fetch_remote_usage(&client, base, Some("tok")).await;
        let mut n = node("node3", "172.31.250.13");
        n.node_id = Some("id-node3".into());
        let o = check_identity(&n, raw.clone());
        apply_poll(&mut n, o, 1_000);
        (raw, n)
    }

    fn reason(n: &Node) -> &str {
        n.status_reason.as_deref().unwrap_or_default()
    }

    #[tokio::test]
    async fn heartbeat_replies_that_cant_prove_a_pinned_identity_leave_it_offline() {
        // a Horcrux with another JWT_SECRET
        let base = canned_server("401 Unauthorized", r#"{"error":"invalid token"}"#).await;
        let (raw, n) = poll_pinned(&base).await;
        assert!(
            matches!(&raw, PollOutcome::ReachableNoUsage(w) if w.contains("401")),
            "{raw:?}"
        );
        assert_eq!(n.status, NodeStatus::Offline);
        assert!(reason(&n).contains("HTTP 401"), "{n:?}");

        // any other web server's error
        let base = canned_server("404 Not Found", "<html>nope</html>").await;
        let (raw, n) = poll_pinned(&base).await;
        assert!(matches!(&raw, PollOutcome::ReachableNoUsage(w) if w.contains("404")));
        assert_eq!(n.status, NodeStatus::Offline);
        assert!(reason(&n).contains("HTTP 404"), "{n:?}");
        let base = canned_server("503 Service Unavailable", "").await;
        let (_, n) = poll_pinned(&base).await;
        assert_eq!(n.status, NodeStatus::Offline);
        assert!(reason(&n).contains("HTTP 503"), "{n:?}");

        // 200 with a body that isn't a usage report
        let base = canned_server("200 OK", "<html>welcome to nginx</html>").await;
        let (raw, n) = poll_pinned(&base).await;
        assert!(
            matches!(&raw, PollOutcome::ReachableNoUsage(w) if w.contains("not a usage report")),
            "{raw:?}"
        );
        assert_eq!(n.status, NodeStatus::Offline);
        assert_eq!(n.cpu_usage, None);

        // a usage report without an identity
        let base = canned_server(
            "200 OK",
            r#"{"cpu_usage":5.0,"memory_used":1,"memory_total":2,"cpu_cores":1,"architecture":"x86_64","timestamp":1}"#,
        )
        .await;
        let (raw, n) = poll_pinned(&base).await;
        assert!(matches!(raw, PollOutcome::Usage(_)));
        assert_eq!(n.status, NodeStatus::Offline);
        assert!(reason(&n).contains("no node identity"), "{n:?}");

        // the normal case: the pinned node itself
        let base = canned_server(
            "200 OK",
            r#"{"cpu_usage":12.5,"memory_used":1024,"memory_total":4096,"cpu_cores":4,"architecture":"x86_64","timestamp":1,"node_id":"id-node3"}"#,
        )
        .await;
        let (raw, n) = poll_pinned(&base).await;
        assert!(matches!(raw, PollOutcome::Usage(_)));
        assert_eq!(n.status, NodeStatus::Online);
        assert_eq!(n.status_reason, None);
        assert_eq!(n.cpu_usage, Some(12.5));
        assert_eq!(n.last_seen, Some(1_000));
    }

    #[tokio::test]
    async fn an_unpinned_node_answering_401_is_up_with_usage_unknown() {
        let base = canned_server("401 Unauthorized", "{}").await;
        let client = reqwest::Client::new();
        let mut n = node("old", "10.0.0.9");
        let o = check_identity(&n, fetch_remote_usage(&client, &base, None).await);
        apply_poll(&mut n, o, 1_000);
        assert_eq!(n.status, NodeStatus::Online);
        assert_eq!(n.cpu_usage, None);
        assert!(reason(&n).contains("HTTP 401"), "{n:?}");
    }

    #[tokio::test]
    async fn unreachable_remote_is_reported_unreachable() {
        let client = reqwest::Client::new();
        // Port 9 (discard) on loopback: nothing listens there in CI.
        let outcome = fetch_remote_usage(&client, "http://127.0.0.1:9", None).await;
        assert_eq!(outcome, PollOutcome::Unreachable);
    }
}
