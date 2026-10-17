//! LXC container options (the `pct create` / `pct set` surface).
//!
//! Sent as the `lxc` object of a `ContainerConfig` when `runtime` is `lxc`.
//! Every field has a default, so `{"lxc": {}}` (or no `lxc` object at all)
//! gives an unprivileged container with no network and no limits beyond
//! memory/cpus, like `pct create` with only an ostemplate.

use serde::{Deserialize, Serialize};

/// Default first host uid/gid of an unprivileged container's id map
/// (Proxmox and the LXC docs use the same range).
pub const LXC_DEFAULT_IDMAP_BASE: u32 = 100_000;
/// Default size of the id map (a full 16-bit uid space).
pub const LXC_DEFAULT_IDMAP_SIZE: u32 = 65_536;

fn default_true() -> bool {
    true
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// LXC-specific container settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LxcOptions {
    /// Run with a user namespace (root in the container is an
    /// unprivileged host uid). Default true, as in Proxmox.
    #[serde(default = "default_true")]
    pub unprivileged: bool,
    /// First host uid/gid the container's uid/gid 0 maps to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idmap_base: Option<u32>,
    /// Number of mapped ids (default 65536).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idmap_size: Option<u32>,
    /// Template architecture (amd64, arm64, ...). Default: the host's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
    /// Hostname inside the container (default: the container name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    /// Swap in MB on top of memory (cgroup memory.swap.max). 0 = no swap.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub swap_mb: u64,
    /// CPU bandwidth cap in cores (fractions allowed, e.g. 0.5). When unset
    /// the cap is `cpus` cores. Written as cgroup2 `cpu.max`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_limit: Option<f64>,
    /// Relative CPU weight (cgroup2 cpu.weight, 1-10000, default 100).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_units: Option<u32>,
    /// Root disk size in GB. When set, the rootfs is an ext4 image of that
    /// size (hard limit, can be grown later); otherwise a plain directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_gb: Option<u64>,
    /// Network interfaces (net0, net1, ...).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nets: Vec<LxcNet>,
    /// Mount points (mp0, mp1, ...): host bind mounts or managed volumes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mounts: Vec<LxcMount>,
    #[serde(default)]
    pub features: LxcFeatures,
    /// Start when the host boots (lxc.start.auto).
    #[serde(default, skip_serializing_if = "is_false")]
    pub onboot: bool,
    /// Boot order (lower starts first).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_order: Option<u32>,
    /// Seconds to wait after starting this container before the next one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_delay: Option<u32>,
    /// DNS server(s) written to the container's /etc/resolv.conf.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nameserver: Option<String>,
    /// DNS search domain for /etc/resolv.conf.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub searchdomain: Option<String>,
    /// Public keys installed in /root/.ssh/authorized_keys.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ssh_public_keys: Vec<String>,
}

impl Default for LxcOptions {
    fn default() -> Self {
        Self {
            unprivileged: true,
            idmap_base: None,
            idmap_size: None,
            arch: None,
            hostname: None,
            swap_mb: 0,
            cpu_limit: None,
            cpu_units: None,
            disk_gb: None,
            nets: Vec::new(),
            mounts: Vec::new(),
            features: LxcFeatures::default(),
            onboot: false,
            startup_order: None,
            startup_delay: None,
            nameserver: None,
            searchdomain: None,
            ssh_public_keys: Vec::new(),
        }
    }
}

/// `features:` of a Proxmox container.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LxcFeatures {
    /// Allow nested containers (writable cgroup/proc/sys, nesting apparmor).
    #[serde(default, skip_serializing_if = "is_false")]
    pub nesting: bool,
    /// Allow the keyctl() syscall (systemd/docker in unprivileged CTs).
    #[serde(default, skip_serializing_if = "is_false")]
    pub keyctl: bool,
    /// Expose /dev/fuse.
    #[serde(default, skip_serializing_if = "is_false")]
    pub fuse: bool,
}

/// One network interface (`netN` in Proxmox).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LxcNet {
    /// Interface name inside the container (default eth<N>).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Host bridge the veth is attached to.
    pub bridge: String,
    /// MAC address (generated and stored on create when unset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hwaddr: Option<String>,
    /// VLAN tag (bridge port PVID; the bridge must be VLAN-aware).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vlan: Option<u16>,
    /// IPv4: "dhcp", or an address with prefix ("10.0.0.5/24"). Unset = no
    /// IPv4 configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    /// IPv4 default gateway (static addressing only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway: Option<String>,
    /// IPv6: "dhcp", "auto" (SLAAC) or an address with prefix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip6: Option<String>,
    /// IPv6 default gateway (static addressing only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway6: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtu: Option<u32>,
}

impl LxcNet {
    pub fn if_name(&self, index: usize) -> String {
        self.name.clone().unwrap_or_else(|| format!("eth{index}"))
    }
}

/// One mount point (`mpN` in Proxmox).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LxcMount {
    /// Absolute path inside the container.
    pub path: String,
    /// Host directory to bind-mount. Unset = a volume directory managed by
    /// Horcrux (created with the container, owned by the container's root,
    /// removed with it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Name of the managed volume (only when `source` is unset). Assigned
    /// on create (`mp0`, `mp1`, ...) and kept, so reordering or removing
    /// other mount points never re-points a volume at another path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub read_only: bool,
}

// ---- Proxmox-style one-line specs (CLI flags, UI text fields) ----

fn spec_kv(spec: &str) -> Result<Vec<(String, String)>, String> {
    spec.split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| {
            p.split_once('=')
                .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
                .ok_or_else(|| format!("'{p}' is not key=value"))
        })
        .collect()
}

fn spec_opt(v: String) -> Option<String> {
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

impl LxcNet {
    /// Parse a Proxmox-style NIC spec:
    /// `name=eth0,bridge=vmbr0,tag=20,ip=10.0.0.5/24,gw=10.0.0.1,ip6=auto,hwaddr=..,mtu=1500`.
    pub fn parse_spec(spec: &str) -> Result<LxcNet, String> {
        let mut n = LxcNet::default();
        for (k, v) in spec_kv(spec)? {
            match k.as_str() {
                "name" => n.name = spec_opt(v),
                "bridge" => n.bridge = v,
                "tag" | "vlan" => {
                    n.vlan = if v.is_empty() {
                        None
                    } else {
                        Some(
                            v.parse()
                                .map_err(|_| format!("VLAN tag '{v}' is not a number"))?,
                        )
                    }
                }
                "ip" => n.ip = spec_opt(v),
                "gw" | "gateway" => n.gateway = spec_opt(v),
                "ip6" => n.ip6 = spec_opt(v),
                "gw6" | "gateway6" => n.gateway6 = spec_opt(v),
                "hwaddr" | "mac" => n.hwaddr = spec_opt(v),
                "mtu" => {
                    n.mtu = if v.is_empty() {
                        None
                    } else {
                        Some(
                            v.parse()
                                .map_err(|_| format!("MTU '{v}' is not a number"))?,
                        )
                    }
                }
                other => return Err(format!("unknown NIC option '{other}'")),
            }
        }
        if n.bridge.is_empty() {
            return Err(format!("'{spec}': bridge=<name> is required"));
        }
        Ok(n)
    }

    /// The spec `parse_spec` reads (name defaults to eth<index>).
    pub fn to_spec(&self, index: usize) -> String {
        let n = self;
        let mut parts = vec![
            format!("name={}", n.if_name(index)),
            format!("bridge={}", n.bridge),
        ];
        if let Some(t) = n.vlan {
            parts.push(format!("tag={t}"));
        }
        if let Some(v) = &n.ip {
            parts.push(format!("ip={v}"));
        }
        if let Some(v) = &n.gateway {
            parts.push(format!("gw={v}"));
        }
        if let Some(v) = &n.ip6 {
            parts.push(format!("ip6={v}"));
        }
        if let Some(v) = &n.gateway6 {
            parts.push(format!("gw6={v}"));
        }
        if let Some(v) = &n.hwaddr {
            parts.push(format!("hwaddr={v}"));
        }
        if let Some(v) = n.mtu {
            parts.push(format!("mtu={v}"));
        }
        parts.join(",")
    }
}

impl LxcMount {
    /// Parse `mp=/srv/data,source=/host/dir,ro=1` (bind mount) or
    /// `mp=/var/lib/app[,volume=mp0]` (managed volume).
    pub fn parse_spec(spec: &str) -> Result<LxcMount, String> {
        let mut m = LxcMount::default();
        for (k, v) in spec_kv(spec)? {
            match k.as_str() {
                "mp" | "path" => m.path = v,
                "source" | "src" => m.source = spec_opt(v),
                "volume" => m.volume = spec_opt(v),
                "ro" => m.read_only = matches!(v.as_str(), "1" | "true" | "yes"),
                other => return Err(format!("unknown mount option '{other}'")),
            }
        }
        if m.path.is_empty() {
            return Err(format!("'{spec}': mp=<path in container> is required"));
        }
        Ok(m)
    }

    pub fn to_spec(&self) -> String {
        let m = self;
        let mut parts = vec![format!("mp={}", m.path)];
        if let Some(s) = &m.source {
            parts.push(format!("source={s}"));
        }
        if let Some(v) = &m.volume {
            parts.push(format!("volume={v}"));
        }
        if m.read_only {
            parts.push("ro=1".into());
        }
        parts.join(",")
    }
}

impl LxcFeatures {
    /// Parse `nesting=1,keyctl=1,fuse=1` (Proxmox `features:`).
    pub fn parse_spec(spec: &str) -> Result<LxcFeatures, String> {
        let mut f = LxcFeatures::default();
        for (k, v) in spec_kv(spec)? {
            let on = matches!(v.as_str(), "1" | "true" | "yes" | "on");
            match k.as_str() {
                "nesting" => f.nesting = on,
                "keyctl" => f.keyctl = on,
                "fuse" => f.fuse = on,
                other => return Err(format!("unknown feature '{other}'")),
            }
        }
        Ok(f)
    }
}

fn valid_ipv4_cidr(s: &str) -> bool {
    match s.split_once('/') {
        Some((a, p)) => {
            a.parse::<std::net::Ipv4Addr>().is_ok()
                && p.parse::<u8>().map(|p| p <= 32).unwrap_or(false)
        }
        None => false,
    }
}

fn valid_ipv6_cidr(s: &str) -> bool {
    match s.split_once('/') {
        Some((a, p)) => {
            a.parse::<std::net::Ipv6Addr>().is_ok()
                && p.parse::<u8>().map(|p| p <= 128).unwrap_or(false)
        }
        None => false,
    }
}

fn valid_mac(s: &str) -> bool {
    let parts: Vec<&str> = s.split(':').collect();
    parts.len() == 6
        && parts
            .iter()
            .all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Linux interface / bridge name rules (IFNAMSIZ 16, no '/', no spaces).
pub fn valid_ifname(s: &str) -> bool {
    !s.is_empty()
        && s.len() < 16
        && !s.starts_with('-')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Characters LXC config values and mount entries can carry safely:
/// no whitespace (mount entries are space separated), no newlines (one key
/// per line), no '#' comments.
fn safe_config_path(s: &str) -> bool {
    s.starts_with('/')
        && !s.contains("..")
        && !s
            .chars()
            .any(|c| c.is_whitespace() || c == '#' || c.is_control())
}

/// Hostname per RFC 1123 (labels of letters, digits and '-').
pub fn valid_hostname(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 253
        && s.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

impl LxcOptions {
    pub fn idmap_base(&self) -> u32 {
        self.idmap_base.unwrap_or(LXC_DEFAULT_IDMAP_BASE)
    }

    pub fn idmap_size(&self) -> u32 {
        self.idmap_size.unwrap_or(LXC_DEFAULT_IDMAP_SIZE)
    }

    /// Reject anything that would produce a broken or injected LXC config,
    /// so a bad form value is a 4xx with a clear reason.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(a) = &self.arch {
            if !matches!(
                a.as_str(),
                "amd64" | "arm64" | "armhf" | "i386" | "ppc64el" | "s390x" | "riscv64"
            ) {
                return Err(format!("unsupported architecture '{a}'"));
            }
        }
        if let Some(h) = &self.hostname {
            if !valid_hostname(h) {
                return Err(format!("invalid hostname '{h}'"));
            }
        }
        if self.unprivileged {
            if self.idmap_size() == 0 {
                return Err("idmap_size must be greater than zero".into());
            }
            if self.idmap_base() == 0 {
                return Err("idmap_base 0 would map container root to host root".into());
            }
            if (self.idmap_base() as u64) + (self.idmap_size() as u64) > u32::MAX as u64 {
                return Err("idmap range exceeds the 32-bit uid space".into());
            }
        }
        if let Some(c) = self.cpu_limit {
            if !(c > 0.0 && c <= 8192.0) || !c.is_finite() {
                return Err(format!(
                    "cpu_limit must be between 0 and 8192 cores, got {c}"
                ));
            }
        }
        if let Some(u) = self.cpu_units {
            if !(1..=10000).contains(&u) {
                return Err(format!("cpu_units must be 1-10000, got {u}"));
            }
        }
        if let Some(d) = self.disk_gb {
            if d == 0 || d > 1 << 20 {
                return Err(format!(
                    "disk_gb must be between 1 and {}, got {d}",
                    1 << 20
                ));
            }
        }
        let mut names = std::collections::HashSet::new();
        for (i, n) in self.nets.iter().enumerate() {
            if !valid_ifname(&n.bridge) {
                return Err(format!("net{i}: invalid bridge name '{}'", n.bridge));
            }
            let ifname = n.if_name(i);
            if !valid_ifname(&ifname) {
                return Err(format!("net{i}: invalid interface name '{ifname}'"));
            }
            if !names.insert(ifname.clone()) {
                return Err(format!("net{i}: duplicate interface name '{ifname}'"));
            }
            if let Some(m) = &n.hwaddr {
                if !valid_mac(m) {
                    return Err(format!("net{i}: invalid MAC address '{m}'"));
                }
                let first = u8::from_str_radix(&m[..2], 16).unwrap_or(1);
                if first & 1 == 1 {
                    return Err(format!("net{i}: '{m}' is a multicast MAC address"));
                }
            }
            if let Some(v) = n.vlan {
                if !(1..=4094).contains(&v) {
                    return Err(format!("net{i}: VLAN tag must be 1-4094, got {v}"));
                }
            }
            match n.ip.as_deref() {
                None | Some("dhcp") | Some("manual") => {
                    if n.gateway.is_some() {
                        return Err(format!("net{i}: an IPv4 gateway needs a static address"));
                    }
                }
                Some(ip) if valid_ipv4_cidr(ip) => {}
                Some(ip) => {
                    return Err(format!(
                        "net{i}: invalid IPv4 '{ip}' (use dhcp or address/prefix)"
                    ))
                }
            }
            if let Some(g) = &n.gateway {
                if g.parse::<std::net::Ipv4Addr>().is_err() {
                    return Err(format!("net{i}: invalid IPv4 gateway '{g}'"));
                }
            }
            match n.ip6.as_deref() {
                None | Some("dhcp") | Some("auto") | Some("manual") => {
                    if n.gateway6.is_some() {
                        return Err(format!("net{i}: an IPv6 gateway needs a static address"));
                    }
                }
                Some(ip) if valid_ipv6_cidr(ip) => {}
                Some(ip) => {
                    return Err(format!(
                        "net{i}: invalid IPv6 '{ip}' (use dhcp, auto or address/prefix)"
                    ))
                }
            }
            if let Some(g) = &n.gateway6 {
                if g.parse::<std::net::Ipv6Addr>().is_err() {
                    return Err(format!("net{i}: invalid IPv6 gateway '{g}'"));
                }
            }
            if let Some(m) = n.mtu {
                if !(576..=65535).contains(&m) {
                    return Err(format!("net{i}: MTU must be 576-65535, got {m}"));
                }
            }
        }
        let mut paths = std::collections::HashSet::new();
        for (i, m) in self.mounts.iter().enumerate() {
            if !safe_config_path(&m.path) || m.path == "/" {
                return Err(format!(
                    "mp{i}: mount path '{}' must be an absolute path without spaces or '..' (and not /)",
                    m.path
                ));
            }
            if !paths.insert(m.path.trim_end_matches('/').to_string()) {
                return Err(format!("mp{i}: duplicate mount path '{}'", m.path));
            }
            if let Some(v) = &m.volume {
                let ok = !v.is_empty()
                    && v.len() <= 64
                    && v.chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'));
                if !ok || m.source.is_some() {
                    return Err(format!(
                        "mp{i}: volume name '{v}' must be letters, digits, '_' or '-' (and no host path)"
                    ));
                }
            }
            if let Some(s) = &m.source {
                if !safe_config_path(s) {
                    return Err(format!(
                        "mp{i}: host path '{s}' must be an absolute path without spaces or '..'"
                    ));
                }
            }
        }
        if let Some(ns) = &self.nameserver {
            for a in ns.split([' ', ',']).filter(|s| !s.is_empty()) {
                if a.parse::<std::net::IpAddr>().is_err() {
                    return Err(format!("invalid nameserver '{a}'"));
                }
            }
        }
        if let Some(sd) = &self.searchdomain {
            if sd
                .split([' ', ','])
                .filter(|s| !s.is_empty())
                .any(|d| !valid_hostname(d))
            {
                return Err(format!("invalid search domain '{sd}'"));
            }
        }
        for k in &self.ssh_public_keys {
            let k = k.trim();
            if k.is_empty() || k.contains('\n') || k.contains('\r') {
                return Err("ssh public keys must be single non-empty lines".into());
            }
            if !(k.starts_with("ssh-") || k.starts_with("ecdsa-") || k.starts_with("sk-")) {
                return Err(format!(
                    "'{}' does not look like an OpenSSH public key",
                    &k[..k.len().min(20)]
                ));
            }
        }
        Ok(())
    }
}

/// One image in the images.linuxcontainers.org catalog (the `pveam
/// available` equivalent).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LxcCatalogEntry {
    pub distro: String,
    pub release: String,
    pub arch: String,
    pub variant: String,
    /// Build serial, e.g. 20261016_13:00.
    pub build: String,
    /// `distro/release/arch/variant`, the id used to download/create.
    pub id: String,
}

/// A downloaded template in the local cache (`pveam list`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LxcTemplate {
    /// `distro/release/arch/variant`.
    pub id: String,
    pub distro: String,
    pub release: String,
    pub arch: String,
    pub variant: String,
    pub build: String,
    pub size_bytes: u64,
    /// SHA-256 of rootfs.tar.xz, verified against the server's SHA256SUMS.
    pub sha256: String,
    pub downloaded_at: String,
}

/// A container snapshot (`pct listsnapshot`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LxcSnapshot {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    pub created_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_unprivileged_and_empty() {
        let o: LxcOptions = serde_json::from_str("{}").unwrap();
        assert!(o.unprivileged);
        assert_eq!(o, LxcOptions::default());
        assert_eq!(o.idmap_base(), 100_000);
        assert_eq!(o.idmap_size(), 65_536);
        assert!(o.validate().is_ok());
        // Defaults are not serialized (only unprivileged + features).
        let v = serde_json::to_value(&o).unwrap();
        assert_eq!(v, serde_json::json!({"unprivileged": true, "features": {}}));
    }

    #[test]
    fn validation_accepts_a_full_config() {
        let o = LxcOptions {
            hostname: Some("web.example.com".into()),
            swap_mb: 256,
            cpu_limit: Some(0.5),
            cpu_units: Some(200),
            disk_gb: Some(4),
            nets: vec![
                LxcNet {
                    bridge: "vmbr0".into(),
                    vlan: Some(20),
                    ip: Some("10.0.0.5/24".into()),
                    gateway: Some("10.0.0.1".into()),
                    ip6: Some("auto".into()),
                    hwaddr: Some("BC:24:11:00:00:01".into()),
                    mtu: Some(1500),
                    ..Default::default()
                },
                LxcNet {
                    bridge: "lxcbr0".into(),
                    ip: Some("dhcp".into()),
                    ..Default::default()
                },
            ],
            mounts: vec![
                LxcMount {
                    path: "/srv/data".into(),
                    source: Some("/tank/data".into()),
                    read_only: true,
                    ..Default::default()
                },
                LxcMount {
                    path: "/var/lib/app".into(),
                    ..Default::default()
                },
            ],
            features: LxcFeatures {
                nesting: true,
                keyctl: true,
                fuse: true,
            },
            nameserver: Some("1.1.1.1 2606:4700:4700::1111".into()),
            searchdomain: Some("example.com".into()),
            ssh_public_keys: vec!["ssh-ed25519 AAAAC3Nz test@host".into()],
            ..Default::default()
        };
        assert_eq!(o.validate(), Ok(()));
        let back: LxcOptions = serde_json::from_value(serde_json::to_value(&o).unwrap()).unwrap();
        assert_eq!(back, o);
    }

    #[test]
    fn validation_rejects_injection_and_bad_values() {
        let net = |f: fn(&mut LxcNet)| {
            let mut n = LxcNet {
                bridge: "vmbr0".into(),
                ..Default::default()
            };
            f(&mut n);
            LxcOptions {
                nets: vec![n],
                ..Default::default()
            }
        };
        let mount = |p: &str, s: Option<&str>| LxcOptions {
            mounts: vec![LxcMount {
                path: p.into(),
                source: s.map(Into::into),
                ..Default::default()
            }],
            ..Default::default()
        };
        let bad = vec![
            net(|n| n.bridge = "br0\nlxc.apparmor.profile = unconfined".into()),
            net(|n| n.bridge = "a-very-long-bridge-name".into()),
            net(|n| n.vlan = Some(0)),
            net(|n| n.vlan = Some(4095)),
            net(|n| n.ip = Some("10.0.0.5".into())),
            net(|n| n.ip = Some("10.0.0.500/24".into())),
            net(|n| n.gateway = Some("10.0.0.1".into())),
            net(|n| n.hwaddr = Some("01:00:5e:00:00:01".into())),
            net(|n| n.ip6 = Some("fe80::1".into())),
            net(|n| n.mtu = Some(100)),
            mount("relative", None),
            mount("/", None),
            mount("/a b", None),
            mount("/ok", Some("/host/../etc")),
            mount("/ok", Some("/x\nlxc.mount.entry")),
            LxcOptions {
                mounts: vec![LxcMount {
                    path: "/v".into(),
                    volume: Some("../../etc".into()),
                    ..Default::default()
                }],
                ..Default::default()
            },
            LxcOptions {
                hostname: Some("-bad".into()),
                ..Default::default()
            },
            LxcOptions {
                cpu_limit: Some(0.0),
                ..Default::default()
            },
            LxcOptions {
                cpu_units: Some(0),
                ..Default::default()
            },
            LxcOptions {
                disk_gb: Some(0),
                ..Default::default()
            },
            LxcOptions {
                idmap_base: Some(0),
                ..Default::default()
            },
            LxcOptions {
                arch: Some("x86".into()),
                ..Default::default()
            },
            LxcOptions {
                nameserver: Some("not-an-ip".into()),
                ..Default::default()
            },
            LxcOptions {
                ssh_public_keys: vec!["ssh-rsa AAA\nssh-rsa BBB".into()],
                ..Default::default()
            },
        ];
        for o in bad {
            assert!(o.validate().is_err(), "{o:?} should be rejected");
        }
        // Duplicate interface names / mount paths.
        let dup = LxcOptions {
            nets: vec![
                LxcNet {
                    bridge: "a".into(),
                    name: Some("eth1".into()),
                    ..Default::default()
                },
                LxcNet {
                    bridge: "b".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        assert!(dup.validate().is_err());
    }

    #[test]
    fn specs_parse() {
        let n = LxcNet::parse_spec("bridge=vmbr0,tag=20,ip=dhcp").unwrap();
        assert_eq!((n.bridge.as_str(), n.vlan), ("vmbr0", Some(20)));
        assert_eq!(n.to_spec(0), "name=eth0,bridge=vmbr0,tag=20,ip=dhcp");
        let m = LxcMount::parse_spec("mp=/a,source=/b,ro=1").unwrap();
        assert!(m.read_only && m.source.as_deref() == Some("/b"));
        let f = LxcFeatures::parse_spec("nesting=1,keyctl=0,fuse=yes").unwrap();
        assert_eq!(
            f,
            LxcFeatures {
                nesting: true,
                keyctl: false,
                fuse: true
            }
        );
        assert!(LxcFeatures::parse_spec("mknod=1").is_err());
    }
}
