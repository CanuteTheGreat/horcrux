//! API access log: node settings and the pure helpers (client address with
//! trusted proxies, path grouping, status classes). Shared by the API, the
//! declarative config and the web UI.
//!
//! Every request to the node is recorded (who, from where, what, how it
//! ended); the settings below control what is kept and for how long. All
//! values are typed keys with documented defaults (`access_log:` in the
//! declarative spec, `GET/PUT /api/access-log/settings`).
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

/// Node settings of the access log (`access_log` in the declarative spec).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct AccessLogSettings {
    /// Record requests at all (default on).
    pub enabled: bool,
    /// Keep entries this many days (1-3650; default 30).
    pub retention_days: u32,
    /// Keep at most this many MiB of entries; the oldest go first
    /// (1-1048576; default 256).
    pub max_mb: u32,
    /// Peers (IP or CIDR) whose `X-Forwarded-For` is believed: a reverse
    /// proxy or load balancer in front of the node. Empty (default) = the
    /// TCP peer address is the client address.
    pub trusted_proxies: Vec<String>,
    /// Also record requests for the web UI's static files (default off:
    /// only `/api` and `/metrics`-style endpoints are recorded).
    pub log_static: bool,
    /// Request paths starting with one of these are not recorded (for
    /// example a load balancer's health probe, `/api/health/live`). Empty
    /// by default: every request is recorded.
    pub exclude_path_prefixes: Vec<String>,
    /// Entries waiting for the writer; a full queue drops entries and
    /// counts them instead of slowing requests (64-1000000; default 8192).
    pub queue_capacity: u32,
    /// Entries written per database transaction (1-10000; default 256).
    pub batch_size: u32,
    /// Longest wait before queued entries are written, ms (50-60000;
    /// default 500).
    pub flush_interval_ms: u32,
    /// How often retention is enforced, seconds (10-86400; default 600).
    pub prune_interval_secs: u32,
    /// Longest `User-Agent` kept, characters (16-1024; default 256).
    pub max_user_agent_len: u32,
    /// Rows per page when the caller gives no `limit` (1-1000; default 100).
    pub default_page_size: u32,
    /// Largest page a caller may ask for (1-10000; default 1000).
    pub max_page_size: u32,
    /// Time buckets in the statistics when the caller gives none
    /// (1-500; default 48).
    pub stats_buckets: u32,
    /// Entries in each "top" list of the statistics (1-100; default 10).
    pub stats_top_n: u32,
    /// How long a cluster query waits for another node, seconds
    /// (1-120; default 10).
    pub peer_timeout_secs: u32,
    /// Database file name inside the node's data directory
    /// (default `access-log.db`).
    pub db_file: String,
}

impl Default for AccessLogSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            retention_days: 30,
            max_mb: 256,
            trusted_proxies: Vec::new(),
            log_static: false,
            exclude_path_prefixes: Vec::new(),
            queue_capacity: 8192,
            batch_size: 256,
            flush_interval_ms: 500,
            prune_interval_secs: 600,
            max_user_agent_len: 256,
            default_page_size: 100,
            max_page_size: 1000,
            stats_buckets: 48,
            stats_top_n: 10,
            peer_timeout_secs: 10,
            db_file: "access-log.db".into(),
        }
    }
}

fn range(name: &str, v: u32, lo: u32, hi: u32) -> Result<(), String> {
    if (lo..=hi).contains(&v) {
        Ok(())
    } else {
        Err(format!("access_log.{name}: {lo}-{hi}"))
    }
}

impl AccessLogSettings {
    pub fn validate(&self) -> Result<(), String> {
        range("retention_days", self.retention_days, 1, 3650)?;
        range("max_mb", self.max_mb, 1, 1_048_576)?;
        range("queue_capacity", self.queue_capacity, 64, 1_000_000)?;
        range("batch_size", self.batch_size, 1, 10_000)?;
        range("flush_interval_ms", self.flush_interval_ms, 50, 60_000)?;
        range("prune_interval_secs", self.prune_interval_secs, 10, 86_400)?;
        range("max_user_agent_len", self.max_user_agent_len, 16, 1024)?;
        range("default_page_size", self.default_page_size, 1, 1000)?;
        range("max_page_size", self.max_page_size, 1, 10_000)?;
        range("stats_buckets", self.stats_buckets, 1, 500)?;
        range("stats_top_n", self.stats_top_n, 1, 100)?;
        range("peer_timeout_secs", self.peer_timeout_secs, 1, 120)?;
        if self.default_page_size > self.max_page_size {
            return Err("access_log.default_page_size: must not exceed max_page_size".into());
        }
        if self.trusted_proxies.len() > 64 {
            return Err("access_log.trusted_proxies: at most 64 entries".into());
        }
        if self.exclude_path_prefixes.len() > 64
            || self
                .exclude_path_prefixes
                .iter()
                .any(|p| !p.starts_with('/') || p.len() > 256)
        {
            return Err(
                "access_log.exclude_path_prefixes: at most 64 paths, each starting with /".into(),
            );
        }
        for p in &self.trusted_proxies {
            Cidr::parse(p).map_err(|e| format!("access_log.trusted_proxies: {e}"))?;
        }
        let f = self.db_file.trim();
        if f.is_empty()
            || f.len() > 64
            || f.contains('/')
            || f.contains('\\')
            || f.starts_with('.')
            || !f
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        {
            return Err(
                "access_log.db_file: a plain file name (letters, digits, . - _), no path".into(),
            );
        }
        Ok(())
    }

    /// The parsed trusted proxy list (invalid entries are skipped; they are
    /// refused by [`validate`] before they can be stored).
    pub fn proxies(&self) -> Vec<Cidr> {
        self.trusted_proxies
            .iter()
            .filter_map(|p| Cidr::parse(p).ok())
            .collect()
    }
}

/// An IP network (a single address is a /32 or /128).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr {
    addr: IpAddr,
    prefix: u8,
}

impl Cidr {
    pub fn parse(s: &str) -> Result<Cidr, String> {
        let s = s.trim();
        let (a, p) = match s.split_once('/') {
            Some((a, p)) => (a, Some(p)),
            None => (s, None),
        };
        let addr: IpAddr = a
            .trim()
            .parse()
            .map_err(|_| format!("{s:?} is not an IP address or CIDR network"))?;
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = match p {
            None => max,
            Some(p) => p
                .trim()
                .parse::<u8>()
                .ok()
                .filter(|p| *p <= max)
                .ok_or_else(|| format!("{s:?}: prefix length must be 0-{max}"))?,
        };
        Ok(Cidr { addr, prefix })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        // An IPv4-mapped IPv6 peer (dual-stack listener) is the IPv4 address.
        let ip = match ip {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
            v4 => v4,
        };
        match (self.addr, ip) {
            (IpAddr::V4(n), IpAddr::V4(i)) => {
                let m = if self.prefix == 0 {
                    0
                } else {
                    u32::MAX << (32 - u32::from(self.prefix))
                };
                u32::from(n) & m == u32::from(i) & m
            }
            (IpAddr::V6(n), IpAddr::V6(i)) => {
                let m = if self.prefix == 0 {
                    0
                } else {
                    u128::MAX << (128 - u32::from(self.prefix))
                };
                u128::from(n) & m == u128::from(i) & m
            }
            _ => false,
        }
    }
}

fn parse_hop(s: &str) -> Option<IpAddr> {
    let s = s.trim().trim_matches('"');
    if let Ok(ip) = s.parse::<IpAddr>() {
        return Some(ip);
    }
    // "1.2.3.4:5678" and "[::1]:80"
    s.parse::<std::net::SocketAddr>().ok().map(|a| a.ip())
}

fn normalize(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    }
}

/// The client address of a request. `X-Forwarded-For` is believed only when
/// the TCP `peer` is a trusted proxy; the list is then walked from the right
/// (nearest hop) and the first address that is not itself a trusted proxy is
/// the client. An unparseable entry ends the walk at the last good hop, so a
/// client cannot make up the address by sending its own header.
pub fn client_address(peer: IpAddr, xff: Option<&str>, trusted: &[Cidr]) -> IpAddr {
    let peer = normalize(peer);
    let is_trusted = |ip: IpAddr| trusted.iter().any(|c| c.contains(ip));
    let Some(xff) = xff.filter(|_| is_trusted(peer)) else {
        return peer;
    };
    let mut current = peer;
    for part in xff.split(',').rev() {
        let Some(ip) = parse_hop(part) else {
            return current;
        };
        let ip = normalize(ip);
        current = ip;
        if !is_trusted(ip) {
            return ip;
        }
    }
    current
}

/// The route template a request was grouped under: the matched route
/// (`/api/vms/:id/start`), else `unmatched` (a path nobody serves, so a
/// scanner cannot make the group list grow without bound).
pub fn path_group(matched: Option<&str>) -> String {
    match matched {
        Some(m) if !m.is_empty() => m.to_string(),
        _ => "unmatched".into(),
    }
}

/// A request path with identifiers masked (`/api/vms/1234/start` ->
/// `/api/vms/:id/start`), used only to describe requests that matched no
/// route. Numbers, UUIDs and long hex strings become `:id`.
pub fn mask_ids(path: &str) -> String {
    let mut out = String::new();
    for (i, seg) in path.split('/').enumerate() {
        if i > 0 {
            out.push('/');
        }
        let hexish = seg.len() >= 16 && seg.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
        let numeric = !seg.is_empty() && seg.chars().all(|c| c.is_ascii_digit());
        out.push_str(if hexish || numeric { ":id" } else { seg });
    }
    out
}

/// `2xx`/`4xx`/`5xx` class filter, or an exact status. `None` for anything
/// else.
pub fn parse_status_filter(s: &str) -> Option<(u16, u16)> {
    let s = s.trim().to_ascii_lowercase();
    if s.len() == 3 && s.ends_with("xx") {
        let d = s[..1].parse::<u16>().ok().filter(|d| (1..=5).contains(d))?;
        return Some((d * 100, d * 100 + 99));
    }
    let n = s.parse::<u16>().ok().filter(|n| (100..=599).contains(n))?;
    Some((n, n))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }
    fn nets(v: &[&str]) -> Vec<Cidr> {
        v.iter().map(|s| Cidr::parse(s).unwrap()).collect()
    }

    #[test]
    fn defaults_validate() {
        AccessLogSettings::default().validate().unwrap();
    }

    #[test]
    fn invalid_settings_are_refused() {
        let bad = |f: fn(&mut AccessLogSettings)| {
            let mut s = AccessLogSettings::default();
            f(&mut s);
            s.validate().is_err()
        };
        assert!(bad(|s| s.retention_days = 0));
        assert!(bad(|s| s.max_mb = 0));
        assert!(bad(|s| s.queue_capacity = 1));
        assert!(bad(|s| s.default_page_size = 2000));
        assert!(bad(|s| s.trusted_proxies = vec!["not-an-ip".into()]));
        assert!(bad(|s| s.trusted_proxies = vec!["10.0.0.0/33".into()]));
        assert!(bad(|s| s.db_file = "../x.db".into()));
        assert!(bad(|s| s.db_file = String::new()));
    }

    #[test]
    fn cidr_membership() {
        let c = Cidr::parse("10.1.0.0/16").unwrap();
        assert!(c.contains(ip("10.1.200.3")));
        assert!(!c.contains(ip("10.2.0.1")));
        assert!(Cidr::parse("0.0.0.0/0").unwrap().contains(ip("8.8.8.8")));
        assert!(Cidr::parse("192.0.2.7").unwrap().contains(ip("192.0.2.7")));
        assert!(!Cidr::parse("192.0.2.7").unwrap().contains(ip("192.0.2.8")));
        let v6 = Cidr::parse("2001:db8::/32").unwrap();
        assert!(v6.contains(ip("2001:db8::1")));
        assert!(!v6.contains(ip("2001:db9::1")));
        // a dual-stack listener reports IPv4 peers as mapped IPv6
        assert!(c.contains(ip("::ffff:10.1.2.3")));
    }

    #[test]
    fn xff_ignored_without_trusted_peer() {
        let t = nets(&["10.0.0.0/8"]);
        assert_eq!(
            client_address(ip("203.0.113.9"), Some("1.2.3.4"), &t),
            ip("203.0.113.9")
        );
        assert_eq!(
            client_address(ip("203.0.113.9"), Some("1.2.3.4"), &[]),
            ip("203.0.113.9")
        );
    }

    #[test]
    fn xff_believed_from_trusted_peer() {
        let t = nets(&["10.0.0.0/8"]);
        assert_eq!(
            client_address(ip("10.0.0.5"), Some("198.51.100.20"), &t),
            ip("198.51.100.20")
        );
        // spoofed leftmost value is ignored: the nearest untrusted hop wins
        assert_eq!(
            client_address(ip("10.0.0.5"), Some("1.1.1.1, 198.51.100.20, 10.0.0.9"), &t),
            ip("198.51.100.20")
        );
        // all hops trusted: the leftmost
        assert_eq!(
            client_address(ip("10.0.0.5"), Some("10.9.9.9, 10.0.0.9"), &t),
            ip("10.9.9.9")
        );
        // garbage stops the walk at the last good hop
        assert_eq!(
            client_address(ip("10.0.0.5"), Some("1.1.1.1, nonsense, 10.0.0.9"), &t),
            ip("10.0.0.9")
        );
        assert_eq!(
            client_address(ip("10.0.0.5"), Some("nonsense"), &t),
            ip("10.0.0.5")
        );
        // ports and mapped addresses
        assert_eq!(
            client_address(ip("10.0.0.5"), Some("198.51.100.20:4711"), &t),
            ip("198.51.100.20")
        );
    }

    #[test]
    fn path_groups() {
        assert_eq!(path_group(Some("/api/vms/:id/start")), "/api/vms/:id/start");
        assert_eq!(path_group(None), "unmatched");
        assert_eq!(path_group(Some("")), "unmatched");
        assert_eq!(mask_ids("/api/vms/1234/start"), "/api/vms/:id/start");
        assert_eq!(
            mask_ids("/x/0a1b2c3d-0000-4000-8000-123456789abc/y"),
            "/x/:id/y"
        );
        assert_eq!(mask_ids("/api/vms/web1"), "/api/vms/web1");
    }

    #[test]
    fn status_filters() {
        assert_eq!(parse_status_filter("4xx"), Some((400, 499)));
        assert_eq!(parse_status_filter("2XX"), Some((200, 299)));
        assert_eq!(parse_status_filter("404"), Some((404, 404)));
        assert_eq!(parse_status_filter("9xx"), None);
        assert_eq!(parse_status_filter("abc"), None);
        assert_eq!(parse_status_filter("42"), None);
    }
}
