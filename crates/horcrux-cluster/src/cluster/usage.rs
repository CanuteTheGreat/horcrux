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
}

/// Result of one usage poll of one node.
#[derive(Debug, Clone, PartialEq)]
pub enum PollOutcome {
    /// The node reported its usage.
    Usage(NodeUsage),
    /// The node's API answered, but not with usage data (auth rejected
    /// because the node uses a different JWT secret, an older Horcrux
    /// without the endpoint, ...). The node is up; its usage is unknown.
    ReachableNoUsage,
    /// No answer at all (connection refused, timeout, DNS failure).
    Unreachable,
}

/// Fold one poll outcome into a node record. Pure so it can be unit-tested.
pub fn apply_poll(node: &mut Node, outcome: PollOutcome, now: i64) {
    match outcome {
        PollOutcome::Usage(usage) => {
            node.status = NodeStatus::Online;
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
        PollOutcome::ReachableNoUsage => {
            node.status = NodeStatus::Online;
            node.cpu_usage = None;
            node.memory_used = None;
        }
        PollOutcome::Unreachable => {
            let recently_seen = node
                .last_seen
                .map(|seen| now - seen <= STALE_AFTER_SECS)
                .unwrap_or(false);
            if !recently_seen {
                node.status = NodeStatus::Offline;
                node.cpu_usage = None;
                node.memory_used = None;
            }
        }
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

        Some(NodeUsage {
            cpu_usage,
            memory_used: memory_in_use(&mem),
            memory_total: mem.total,
            cpu_cores: system::get_cpu_count() as u32,
            architecture: Architecture::detect(),
            timestamp: chrono::Utc::now().timestamp(),
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
                PollOutcome::ReachableNoUsage
            }
        },
        Ok(resp) => {
            tracing::debug!(
                "{}: local-usage answered HTTP {} (node up, usage unknown)",
                base_url,
                resp.status()
            );
            PollOutcome::ReachableNoUsage
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
        apply_poll(&mut n, PollOutcome::ReachableNoUsage, 1_010);
        assert_eq!(n.status, NodeStatus::Online);
        assert_eq!(n.cpu_usage, None);
        assert_eq!(n.memory_used, None);
    }

    #[test]
    fn cpu_usage_is_clamped_to_percent_range() {
        let mut n = node("n1", "10.0.0.1");
        apply_poll(&mut n, PollOutcome::Usage(usage(140.0, 1)), 1_000);
        assert_eq!(n.cpu_usage, Some(100.0));
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

    #[tokio::test]
    async fn unreachable_remote_is_reported_unreachable() {
        let client = reqwest::Client::new();
        // Port 9 (discard) on loopback: nothing listens there in CI.
        let outcome = fetch_remote_usage(&client, "http://127.0.0.1:9", None).await;
        assert_eq!(outcome, PollOutcome::Unreachable);
    }
}
