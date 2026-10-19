//! Pure rendering of an LXC container's config file and of the files
//! Horcrux writes inside the container (network, hostname, DNS). Kept free
//! of I/O so every option's exact output is unit-tested.
use horcrux_common::{LxcMount, LxcNet, LxcOptions};
use std::fmt::Write;
use std::path::{Path, PathBuf};

/// cgroup2 `cpu.max` period in microseconds (the kernel default).
pub const CPU_PERIOD_US: u64 = 100_000;

/// `cpu.max` value: "<quota> <period>". `cpu_limit` (cores, fractional)
/// wins over `cpus`; `None` when neither caps anything.
pub fn cpu_max(cpus: u32, cpu_limit: Option<f64>) -> Option<String> {
    let cores = match cpu_limit {
        Some(c) if c > 0.0 => c,
        _ if cpus > 0 => cpus as f64,
        _ => return None,
    };
    let quota = ((cores * CPU_PERIOD_US as f64).round() as u64).max(1000);
    Some(format!("{quota} {CPU_PERIOD_US}"))
}

/// Where the root filesystem lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootfsKind {
    /// Plain directory `<ct>/rootfs`.
    Dir,
    /// ext4 image `<ct>/rootfs.raw` of a fixed size (loop mounted).
    Loop,
}

/// Host facts the config depends on.
#[derive(Debug, Clone, Default)]
pub struct HostEnv {
    /// /usr/share/lxc/config (where <distro>.common.conf live).
    pub lxc_share_config: PathBuf,
    /// AppArmor is enabled and LXC can generate profiles.
    pub apparmor: bool,
}

/// Everything needed to render one container's config.
pub struct ConfigInput<'a> {
    /// Horcrux container id: names the host-side veths (`vc<hash>i<n>`)
    /// the container firewall matches on.
    pub id: &'a str,
    pub name: &'a str,
    pub dir: &'a Path,
    pub distro: &'a str,
    pub arch: &'a str,
    pub memory_mb: u64,
    pub cpus: u32,
    pub rootfs: RootfsKind,
    pub opts: &'a LxcOptions,
    /// The guest's own network config was written (ifupdown/netplan/
    /// networkd); otherwise static addresses are set by LXC itself.
    pub guest_net_managed: bool,
    pub host: &'a HostEnv,
}

/// LXC's name for a Debian-style arch.
pub fn lxc_arch(arch: &str) -> &str {
    match arch {
        "amd64" => "x86_64",
        "arm64" => "aarch64",
        "armhf" => "armv7l",
        "i386" => "i686",
        "ppc64el" => "ppc64le",
        other => other,
    }
}

/// Debian-style arch of this host (the images.linuxcontainers.org naming).
pub fn host_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "arm" => "armhf",
        "x86" => "i386",
        "powerpc64" => "ppc64el",
        "s390x" => "s390x",
        "riscv64" => "riscv64",
        _ => "amd64",
    }
}

/// Mount point source on the host: the bind source, or the managed volume
/// directory `<ct>/volumes/<volume>` (`mp<N>` until a name is assigned).
pub fn mount_source(dir: &Path, index: usize, m: &LxcMount) -> PathBuf {
    match &m.source {
        Some(s) => PathBuf::from(s),
        None => dir
            .join("volumes")
            .join(m.volume.clone().unwrap_or_else(|| format!("mp{index}"))),
    }
}

/// Give every managed volume a stable name: keep the ones it has, name new
/// ones `mp<K>` with the lowest K not in use.
pub fn assign_volume_names(mounts: &mut [LxcMount]) {
    let mut used: std::collections::HashSet<String> =
        mounts.iter().filter_map(|m| m.volume.clone()).collect();
    for m in mounts.iter_mut() {
        if m.source.is_none() && m.volume.is_none() {
            let name = (0..)
                .map(|k| format!("mp{k}"))
                .find(|n| !used.contains(n))
                .unwrap_or_default();
            used.insert(name.clone());
            m.volume = Some(name);
        }
    }
}

/// Seccomp policy: LXC's default deny list, plus keyctl -> ENOSYS for
/// unprivileged containers unless the keyctl feature is on (what Proxmox
/// does: systemd/docker inside need keyctl, everything else is better off
/// without it).
pub fn render_seccomp(opts: &LxcOptions) -> String {
    let mut s = String::from(
        "2\ndenylist\nreject_force_umount  # comment this to allow umount -f;  not recommended\n[all]\n\
         kexec_load errno 1\nopen_by_handle_at errno 1\ninit_module errno 1\nfinit_module errno 1\n\
         delete_module errno 1\n",
    );
    if opts.unprivileged && !opts.features.keyctl {
        s.push_str("keyctl errno 38\n");
    }
    s
}

pub fn render_config(i: &ConfigInput) -> String {
    let o = i.opts;
    let dir = i.dir.display();
    let mut c = String::new();
    let _ = writeln!(
        c,
        "# Managed by Horcrux. Change it with PUT /api/containers/<id>/lxc/config \
         (or the container's Configure page); manual edits are overwritten."
    );
    let share = &i.host.lxc_share_config;
    let common = share.join(format!("{}.common.conf", i.distro));
    let common = if common.exists() {
        common
    } else {
        share.join("common.conf")
    };
    let _ = writeln!(c, "lxc.include = {}", common.display());
    if o.unprivileged {
        let userns = share.join(format!("{}.userns.conf", i.distro));
        let userns = if userns.exists() {
            userns
        } else {
            share.join("userns.conf")
        };
        let _ = writeln!(c, "lxc.include = {}", userns.display());
    }
    let _ = writeln!(c, "lxc.arch = {}", lxc_arch(i.arch));
    let _ = writeln!(
        c,
        "lxc.uts.name = {}",
        o.hostname.as_deref().unwrap_or(i.name)
    );
    match i.rootfs {
        RootfsKind::Dir => {
            let _ = writeln!(c, "lxc.rootfs.path = dir:{dir}/rootfs");
        }
        RootfsKind::Loop => {
            let _ = writeln!(c, "lxc.rootfs.path = loop:{dir}/rootfs.raw");
        }
    }
    if o.unprivileged {
        let (b, n) = (o.idmap_base(), o.idmap_size());
        let _ = writeln!(c, "lxc.idmap = u 0 {b} {n}");
        let _ = writeln!(c, "lxc.idmap = g 0 {b} {n}");
    }
    if i.host.apparmor {
        let _ = writeln!(c, "lxc.apparmor.profile = generated");
        if o.features.nesting {
            let _ = writeln!(c, "lxc.apparmor.allow_nesting = 1");
        }
    } else {
        let _ = writeln!(c, "lxc.apparmor.profile = unconfined");
    }
    let _ = writeln!(c, "lxc.seccomp.profile = {dir}/seccomp.conf");
    let _ = writeln!(c, "lxc.console.logfile = {dir}/console.log");
    let _ = writeln!(c, "lxc.console.size = 1MB");

    let _ = writeln!(c, "\n# Resources");
    if i.memory_mb > 0 {
        let _ = writeln!(c, "lxc.cgroup2.memory.max = {}", i.memory_mb * 1024 * 1024);
        let _ = writeln!(
            c,
            "lxc.cgroup2.memory.swap.max = {}",
            o.swap_mb * 1024 * 1024
        );
    }
    if let Some(m) = cpu_max(i.cpus, o.cpu_limit) {
        let _ = writeln!(c, "lxc.cgroup2.cpu.max = {m}");
    }
    if let Some(w) = o.cpu_units {
        let _ = writeln!(c, "lxc.cgroup2.cpu.weight = {w}");
    }

    if o.onboot || o.startup_order.is_some() || o.startup_delay.is_some() {
        let _ = writeln!(c, "\n# Start at boot");
        let _ = writeln!(c, "lxc.start.auto = {}", u8::from(o.onboot));
        if let Some(n) = o.startup_order {
            let _ = writeln!(c, "lxc.start.order = {n}");
        }
        if let Some(d) = o.startup_delay {
            let _ = writeln!(c, "lxc.start.delay = {d}");
        }
    }

    let _ = writeln!(c, "\n# Network");
    if o.nets.is_empty() {
        let _ = writeln!(c, "lxc.net.0.type = empty");
    }
    for (n, net) in o.nets.iter().enumerate() {
        render_net(&mut c, i.id, n, net, i.guest_net_managed);
    }

    if !o.mounts.is_empty() {
        let _ = writeln!(c, "\n# Mount points");
    }
    for (n, m) in o.mounts.iter().enumerate() {
        let src = mount_source(i.dir, n, m);
        let target = m.path.trim_start_matches('/');
        let ro = if m.read_only { ",ro" } else { "" };
        let _ = writeln!(
            c,
            "lxc.mount.entry = {} {target} none rbind,create=dir{ro} 0 0",
            src.display()
        );
    }

    let f = &o.features;
    if f.nesting || f.fuse {
        let _ = writeln!(c, "\n# Features");
    }
    if f.nesting {
        // What /usr/share/lxc/config/nesting.conf does, minus its fixed
        // AppArmor profile name: uncovered proc/sys for the nested runtime,
        // and a writable cgroup tree it can delegate from.
        let _ = writeln!(
            c,
            "lxc.mount.entry = proc dev/.lxc/proc proc create=dir,optional 0 0"
        );
        let _ = writeln!(
            c,
            "lxc.mount.entry = sys dev/.lxc/sys sysfs create=dir,optional 0 0"
        );
        let _ = writeln!(c, "lxc.mount.auto = cgroup:rw:force");
    }
    if f.fuse {
        // Only a privileged container has a device cgroup policy to extend:
        // for an unprivileged one LXC installs none (userns.conf clears it),
        // and adding a single allow rule would turn on an allowlist that
        // denies everything else (/dev/null included).
        if !o.unprivileged {
            let _ = writeln!(c, "lxc.cgroup2.devices.allow = c 10:229 rwm");
        }
        let _ = writeln!(
            c,
            "lxc.mount.entry = /dev/fuse dev/fuse none bind,create=file,optional 0 0"
        );
    }
    c
}

fn render_net(c: &mut String, id: &str, n: usize, net: &LxcNet, guest_net_managed: bool) {
    let p = format!("lxc.net.{n}");
    let _ = writeln!(c, "{p}.type = veth");
    let _ = writeln!(c, "{p}.link = {}", net.bridge);
    // Stable host-side name (LXC would pick a random vethXXXXXX), so the
    // container's firewall rules match this container's traffic only.
    let _ = writeln!(c, "{p}.veth.pair = {}", horcrux_common::ct_veth_name(id, n));
    let _ = writeln!(c, "{p}.name = {}", net.if_name(n));
    let _ = writeln!(c, "{p}.flags = up");
    if let Some(m) = &net.hwaddr {
        let _ = writeln!(c, "{p}.hwaddr = {}", m.to_ascii_uppercase());
    }
    if let Some(m) = net.mtu {
        let _ = writeln!(c, "{p}.mtu = {m}");
    }
    if let Some(v) = net.vlan {
        let _ = writeln!(c, "{p}.veth.vlan.id = {v}");
    }
    if !guest_net_managed {
        if let Some(ip) = net.ip.as_deref().filter(|ip| ip.contains('/')) {
            let _ = writeln!(c, "{p}.ipv4.address = {ip}");
            if let Some(g) = &net.gateway {
                let _ = writeln!(c, "{p}.ipv4.gateway = {g}");
            }
        }
        if let Some(ip) = net.ip6.as_deref().filter(|ip| ip.contains('/')) {
            let _ = writeln!(c, "{p}.ipv6.address = {ip}");
            if let Some(g) = &net.gateway6 {
                let _ = writeln!(c, "{p}.ipv6.gateway = {g}");
            }
        }
    }
}

/// Which network configuration system the guest uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestNet {
    /// /etc/network/interfaces (Alpine, Debian, Devuan).
    Ifupdown,
    /// /etc/netplan (Ubuntu).
    Netplan,
    /// /etc/systemd/network (Fedora, Arch, openSUSE, ...).
    Networkd,
    /// Unknown: LXC sets static addresses itself.
    Unmanaged,
}

/// /etc/network/interfaces for ifupdown(-ng).
pub fn render_ifupdown(nets: &[LxcNet]) -> String {
    let mut s = String::from("# Generated by Horcrux\nauto lo\niface lo inet loopback\n");
    for (n, net) in nets.iter().enumerate() {
        let ifn = net.if_name(n);
        let _ = writeln!(s, "\nauto {ifn}");
        match net.ip.as_deref() {
            Some("dhcp") => {
                let _ = writeln!(s, "iface {ifn} inet dhcp");
            }
            Some(ip) if ip.contains('/') => {
                let _ = writeln!(s, "iface {ifn} inet static\n\taddress {ip}");
                if let Some(g) = &net.gateway {
                    let _ = writeln!(s, "\tgateway {g}");
                }
            }
            _ => {
                let _ = writeln!(s, "iface {ifn} inet manual");
            }
        }
        if let Some(m) = net.mtu {
            let _ = writeln!(s, "\tmtu {m}");
        }
        match net.ip6.as_deref() {
            Some("dhcp") => {
                let _ = writeln!(s, "iface {ifn} inet6 dhcp");
            }
            Some("auto") => {
                let _ = writeln!(s, "iface {ifn} inet6 auto");
            }
            Some(ip) if ip.contains('/') => {
                let _ = writeln!(s, "iface {ifn} inet6 static\n\taddress {ip}");
                if let Some(g) = &net.gateway6 {
                    let _ = writeln!(s, "\tgateway {g}");
                }
            }
            _ => {}
        }
    }
    s
}

/// Netplan YAML (replaces the image's /etc/netplan/10-lxc.yaml).
pub fn render_netplan(nets: &[LxcNet], dns: Option<&Dns>) -> String {
    let mut s = String::from("# Generated by Horcrux\nnetwork:\n  version: 2\n");
    if nets.is_empty() {
        return s;
    }
    s.push_str("  ethernets:\n");
    for (n, net) in nets.iter().enumerate() {
        let _ = writeln!(s, "    {}:", net.if_name(n));
        let _ = writeln!(s, "      dhcp4: {}", net.ip.as_deref() == Some("dhcp"));
        let _ = writeln!(s, "      dhcp6: {}", net.ip6.as_deref() == Some("dhcp"));
        let _ = writeln!(s, "      accept-ra: {}", net.ip6.as_deref() == Some("auto"));
        let addrs: Vec<&str> = [net.ip.as_deref(), net.ip6.as_deref()]
            .into_iter()
            .flatten()
            .filter(|a| a.contains('/'))
            .collect();
        if !addrs.is_empty() {
            let _ = writeln!(s, "      addresses: [{}]", addrs.join(", "));
        }
        let mut routes = Vec::new();
        if let Some(g) = net.gateway.as_deref().filter(|_| has_static(&net.ip)) {
            routes.push(format!("{{to: default, via: {g}}}"));
        }
        if let Some(g) = net.gateway6.as_deref().filter(|_| has_static(&net.ip6)) {
            routes.push(format!("{{to: default, via: \"{g}\"}}"));
        }
        if !routes.is_empty() {
            let _ = writeln!(s, "      routes: [{}]", routes.join(", "));
        }
        if let Some(m) = net.mtu {
            let _ = writeln!(s, "      mtu: {m}");
        }
        if let Some(d) = dns.filter(|_| n == 0) {
            let _ = writeln!(s, "      nameservers:");
            let _ = writeln!(s, "        addresses: [{}]", d.servers.join(", "));
            if !d.search.is_empty() {
                let _ = writeln!(s, "        search: [{}]", d.search.join(", "));
            }
        }
    }
    s
}

fn has_static(ip: &Option<String>) -> bool {
    ip.as_deref().map(|i| i.contains('/')).unwrap_or(false)
}

/// systemd-networkd unit for one interface.
pub fn render_networkd(n: usize, net: &LxcNet) -> String {
    let mut s = format!(
        "# Generated by Horcrux\n[Match]\nName = {}\n\n[Network]\n",
        net.if_name(n)
    );
    let dhcp = match (net.ip.as_deref(), net.ip6.as_deref()) {
        (Some("dhcp"), Some("dhcp")) => "yes",
        (Some("dhcp"), _) => "ipv4",
        (_, Some("dhcp")) => "ipv6",
        _ => "no",
    };
    let _ = writeln!(s, "DHCP = {dhcp}");
    let _ = writeln!(
        s,
        "IPv6AcceptRA = {}",
        if net.ip6.as_deref() == Some("auto") {
            "true"
        } else {
            "false"
        }
    );
    for a in [net.ip.as_deref(), net.ip6.as_deref()]
        .into_iter()
        .flatten()
    {
        if a.contains('/') {
            let _ = writeln!(s, "Address = {a}");
        }
    }
    if let Some(g) = net.gateway.as_deref().filter(|_| has_static(&net.ip)) {
        let _ = writeln!(s, "Gateway = {g}");
    }
    if let Some(g) = net.gateway6.as_deref().filter(|_| has_static(&net.ip6)) {
        let _ = writeln!(s, "Gateway = {g}");
    }
    if let Some(m) = net.mtu {
        let _ = writeln!(s, "\n[Link]\nMTUBytes = {m}");
    }
    s
}

/// DNS settings for /etc/resolv.conf.
pub struct Dns {
    pub servers: Vec<String>,
    pub search: Vec<String>,
}

pub fn dns_of(o: &LxcOptions) -> Option<Dns> {
    let split = |s: &Option<String>| -> Vec<String> {
        s.as_deref()
            .unwrap_or("")
            .split([' ', ','])
            .filter(|x| !x.is_empty())
            .map(str::to_string)
            .collect()
    };
    let servers = split(&o.nameserver);
    let search = split(&o.searchdomain);
    if servers.is_empty() && search.is_empty() {
        None
    } else {
        Some(Dns { servers, search })
    }
}

pub fn render_resolv_conf(d: &Dns) -> String {
    let mut s = String::from("# Generated by Horcrux\n");
    if !d.search.is_empty() {
        let _ = writeln!(s, "search {}", d.search.join(" "));
    }
    for ns in &d.servers {
        let _ = writeln!(s, "nameserver {ns}");
    }
    s
}

/// /etc/hosts with the hostname on 127.0.1.1 (keeps every other line).
pub fn render_hosts(existing: &str, hostname: &str) -> String {
    let short = hostname.split('.').next().unwrap_or(hostname);
    let names = if short == hostname {
        hostname.to_string()
    } else {
        format!("{hostname} {short}")
    };
    let mut out: Vec<String> = existing
        .lines()
        .filter(|l| !l.trim_start().starts_with("127.0.1.1"))
        .map(str::to_string)
        .collect();
    if !out.iter().any(|l| {
        let mut w = l.split_whitespace();
        w.next() == Some("127.0.0.1")
    }) {
        out.insert(0, "127.0.0.1 localhost".to_string());
    }
    let pos = out
        .iter()
        .position(|l| l.trim_start().starts_with("127.0.0.1"))
        .map(|p| p + 1)
        .unwrap_or(0);
    out.insert(pos, format!("127.0.1.1 {names}"));
    let mut s = out.join("\n");
    s.push('\n');
    s
}

/// Random MAC from the locally administered unicast range (02:xx:...), so
/// it can never collide with a vendor-assigned address.
pub fn random_mac() -> String {
    let b: [u8; 5] = rand::random();
    format!(
        "02:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        b[0], b[1], b[2], b[3], b[4]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use horcrux_common::{LxcFeatures, LxcMount};

    fn host() -> HostEnv {
        HostEnv {
            lxc_share_config: PathBuf::from("/nonexistent/lxc/config"),
            apparmor: false,
        }
    }

    #[test]
    fn cpu_max_is_quota_and_period() {
        // The old code wrote `cpu.max = 2`, which cgroup2 rejects.
        assert_eq!(cpu_max(2, None).as_deref(), Some("200000 100000"));
        assert_eq!(cpu_max(1, Some(0.5)).as_deref(), Some("50000 100000"));
        assert_eq!(cpu_max(4, Some(1.25)).as_deref(), Some("125000 100000"));
        assert_eq!(cpu_max(0, None), None);
        // Never below the kernel's 1ms minimum quota.
        assert_eq!(cpu_max(1, Some(0.001)).as_deref(), Some("1000 100000"));
    }

    #[test]
    fn renders_a_full_config() {
        let opts = LxcOptions {
            hostname: Some("web".into()),
            swap_mb: 64,
            cpu_limit: Some(1.5),
            cpu_units: Some(512),
            nets: vec![
                LxcNet {
                    bridge: "vmbr0".into(),
                    hwaddr: Some("02:aa:bb:cc:dd:ee".into()),
                    vlan: Some(20),
                    ip: Some("10.20.0.50/24".into()),
                    gateway: Some("10.20.0.1".into()),
                    mtu: Some(1400),
                    ..Default::default()
                },
                LxcNet {
                    bridge: "vmbr1".into(),
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
                    path: "/var/lib/app/".into(),
                    ..Default::default()
                },
            ],
            features: LxcFeatures {
                nesting: true,
                keyctl: true,
                fuse: true,
            },
            onboot: true,
            startup_order: Some(2),
            ..Default::default()
        };
        let h = host();
        let cfg = render_config(&ConfigInput {
            id: "ct-test",
            name: "ct1",
            dir: Path::new("/var/lib/lxc/ct1"),
            distro: "alpine",
            arch: "amd64",
            memory_mb: 256,
            cpus: 2,
            rootfs: RootfsKind::Dir,
            opts: &opts,
            guest_net_managed: false,
            host: &h,
        });
        for line in [
            "lxc.include = /nonexistent/lxc/config/common.conf",
            "lxc.include = /nonexistent/lxc/config/userns.conf",
            "lxc.arch = x86_64",
            "lxc.uts.name = web",
            "lxc.rootfs.path = dir:/var/lib/lxc/ct1/rootfs",
            "lxc.idmap = u 0 100000 65536",
            "lxc.idmap = g 0 100000 65536",
            "lxc.apparmor.profile = unconfined",
            "lxc.seccomp.profile = /var/lib/lxc/ct1/seccomp.conf",
            "lxc.console.logfile = /var/lib/lxc/ct1/console.log",
            "lxc.cgroup2.memory.max = 268435456",
            "lxc.cgroup2.memory.swap.max = 67108864",
            "lxc.cgroup2.cpu.max = 150000 100000",
            "lxc.cgroup2.cpu.weight = 512",
            "lxc.start.auto = 1",
            "lxc.start.order = 2",
            "lxc.net.0.type = veth",
            "lxc.net.0.link = vmbr0",
            "lxc.net.0.name = eth0",
            "lxc.net.0.hwaddr = 02:AA:BB:CC:DD:EE",
            "lxc.net.0.mtu = 1400",
            "lxc.net.0.veth.vlan.id = 20",
            "lxc.net.0.ipv4.address = 10.20.0.50/24",
            "lxc.net.0.ipv4.gateway = 10.20.0.1",
            "lxc.net.1.link = vmbr1",
            "lxc.net.1.name = eth1",
            "lxc.mount.entry = /tank/data srv/data none rbind,create=dir,ro 0 0",
            "lxc.mount.entry = /var/lib/lxc/ct1/volumes/mp1 var/lib/app/ none rbind,create=dir 0 0",
            "lxc.mount.auto = cgroup:rw:force",
            "lxc.mount.entry = /dev/fuse dev/fuse none bind,create=file,optional 0 0",
        ] {
            assert!(cfg.lines().any(|l| l == line), "missing `{line}` in\n{cfg}");
        }
        // Stable host-side veth names from the container id (the
        // container firewall matches on them).
        for n in 0..2 {
            let line = format!(
                "lxc.net.{n}.veth.pair = {}",
                horcrux_common::ct_veth_name("ct-test", n)
            );
            assert!(cfg.lines().any(|l| l == line), "missing `{line}` in\n{cfg}");
        }
        // DHCP interfaces get no LXC-side address.
        assert!(!cfg.contains("lxc.net.1.ipv4"));
        // No device-cgroup allowlist for an unprivileged container (it
        // would deny /dev/null & co.); a privileged one gets the fuse rule.
        assert!(!cfg.contains("devices.allow"));
        let privileged = LxcOptions {
            unprivileged: false,
            ..opts.clone()
        };
        let pcfg = render_config(&ConfigInput {
            id: "ct-test",
            name: "ct1",
            dir: Path::new("/var/lib/lxc/ct1"),
            distro: "alpine",
            arch: "amd64",
            memory_mb: 256,
            cpus: 2,
            rootfs: RootfsKind::Dir,
            opts: &privileged,
            guest_net_managed: true,
            host: &h,
        });
        assert!(pcfg.contains("lxc.cgroup2.devices.allow = c 10:229 rwm\n"));
        // When the guest config is managed, LXC sets no addresses at all.
        let managed = render_config(&ConfigInput {
            id: "ct-test",
            name: "ct1",
            dir: Path::new("/var/lib/lxc/ct1"),
            distro: "alpine",
            arch: "arm64",
            memory_mb: 256,
            cpus: 2,
            rootfs: RootfsKind::Loop,
            opts: &opts,
            guest_net_managed: true,
            host: &HostEnv {
                apparmor: true,
                ..host()
            },
        });
        assert!(!managed.contains("ipv4.address"));
        assert!(managed.contains("lxc.rootfs.path = loop:/var/lib/lxc/ct1/rootfs.raw\n"));
        assert!(managed.contains("lxc.arch = aarch64\n"));
        assert!(
            managed.contains("lxc.apparmor.profile = generated\nlxc.apparmor.allow_nesting = 1\n")
        );
    }

    #[test]
    fn privileged_and_no_network() {
        let opts = LxcOptions {
            unprivileged: false,
            ..Default::default()
        };
        let cfg = render_config(&ConfigInput {
            id: "ct-test",
            name: "p",
            dir: Path::new("/x/p"),
            distro: "debian",
            arch: "amd64",
            memory_mb: 0,
            cpus: 0,
            rootfs: RootfsKind::Dir,
            opts: &opts,
            guest_net_managed: true,
            host: &host(),
        });
        assert!(!cfg.contains("lxc.idmap"));
        assert!(!cfg.contains("userns.conf"));
        assert!(!cfg.contains("memory.max"));
        assert!(!cfg.contains("cpu.max"));
        assert!(cfg.contains("lxc.net.0.type = empty\n"));
        assert!(!cfg.contains("lxc.start.auto"));
    }

    #[test]
    fn seccomp_blocks_keyctl_only_for_unprivileged_without_the_feature() {
        let mut o = LxcOptions::default();
        assert!(render_seccomp(&o).contains("keyctl errno 38"));
        o.features.keyctl = true;
        assert!(!render_seccomp(&o).contains("keyctl"));
        o.features.keyctl = false;
        o.unprivileged = false;
        assert!(!render_seccomp(&o).contains("keyctl"));
        assert!(render_seccomp(&o).starts_with("2\ndenylist\n"));
    }

    #[test]
    fn guest_network_files() {
        let nets = vec![
            LxcNet {
                bridge: "b".into(),
                ip: Some("10.0.0.5/24".into()),
                gateway: Some("10.0.0.1".into()),
                ip6: Some("auto".into()),
                mtu: Some(1450),
                ..Default::default()
            },
            LxcNet {
                bridge: "b".into(),
                ip: Some("dhcp".into()),
                ip6: Some("fd00::5/64".into()),
                gateway6: Some("fd00::1".into()),
                ..Default::default()
            },
            LxcNet {
                bridge: "b".into(),
                name: Some("mgmt".into()),
                ..Default::default()
            },
        ];
        let ifu = render_ifupdown(&nets);
        assert!(ifu.contains(
            "auto eth0\niface eth0 inet static\n\taddress 10.0.0.5/24\n\tgateway 10.0.0.1\n\tmtu 1450\niface eth0 inet6 auto\n"
        ));
        assert!(ifu.contains(
            "auto eth1\niface eth1 inet dhcp\niface eth1 inet6 static\n\taddress fd00::5/64\n\tgateway fd00::1\n"
        ));
        assert!(ifu.contains("auto mgmt\niface mgmt inet manual\n"));

        let dns = Dns {
            servers: vec!["1.1.1.1".into()],
            search: vec!["lan".into()],
        };
        let np = render_netplan(&nets, Some(&dns));
        assert!(np.contains("    eth0:\n      dhcp4: false\n      dhcp6: false\n      accept-ra: true\n      addresses: [10.0.0.5/24]\n      routes: [{to: default, via: 10.0.0.1}]\n      mtu: 1450\n      nameservers:\n        addresses: [1.1.1.1]\n        search: [lan]\n"));
        assert!(np.contains("    eth1:\n      dhcp4: true\n"));
        assert!(
            np.contains("addresses: [fd00::5/64]\n      routes: [{to: default, via: \"fd00::1\"}]")
        );

        let nd = render_networkd(1, &nets[1]);
        assert!(nd.contains("Name = eth1\n"));
        assert!(nd.contains("DHCP = ipv4\n"));
        assert!(nd.contains("Address = fd00::5/64\nGateway = fd00::1\n"));
        let nd0 = render_networkd(0, &nets[0]);
        assert!(nd0.contains(
            "DHCP = no\nIPv6AcceptRA = true\nAddress = 10.0.0.5/24\nGateway = 10.0.0.1\n"
        ));
        assert!(nd0.contains("[Link]\nMTUBytes = 1450\n"));
    }

    #[test]
    fn hosts_and_resolv() {
        let h = render_hosts(
            "127.0.0.1\tlocalhost\n127.0.1.1 old\n::1 localhost\n",
            "web.lan",
        );
        assert_eq!(
            h,
            "127.0.0.1\tlocalhost\n127.0.1.1 web.lan web\n::1 localhost\n"
        );
        assert_eq!(render_hosts("", "a"), "127.0.0.1 localhost\n127.0.1.1 a\n");
        let o = LxcOptions {
            nameserver: Some("1.1.1.1, 9.9.9.9".into()),
            searchdomain: Some("lan".into()),
            ..Default::default()
        };
        assert_eq!(
            render_resolv_conf(&dns_of(&o).unwrap()),
            "# Generated by Horcrux\nsearch lan\nnameserver 1.1.1.1\nnameserver 9.9.9.9\n"
        );
        assert!(dns_of(&LxcOptions::default()).is_none());
    }

    #[test]
    fn volume_names_are_stable() {
        let mut m = vec![
            LxcMount {
                path: "/a".into(),
                volume: Some("mp0".into()),
                ..Default::default()
            },
            LxcMount {
                path: "/b".into(),
                source: Some("/host".into()),
                ..Default::default()
            },
            LxcMount {
                path: "/c".into(),
                ..Default::default()
            },
        ];
        assign_volume_names(&mut m);
        assert_eq!(m[0].volume.as_deref(), Some("mp0"));
        assert_eq!(m[1].volume, None);
        assert_eq!(m[2].volume.as_deref(), Some("mp1"));
        // Removing /a later must not re-point /c's data.
        let mut later = vec![m[2].clone()];
        assign_volume_names(&mut later);
        assert_eq!(later[0].volume.as_deref(), Some("mp1"));
        assert_eq!(
            mount_source(Path::new("/ct"), 0, &later[0]),
            PathBuf::from("/ct/volumes/mp1")
        );
    }

    #[test]
    fn random_macs_are_local_unicast() {
        for _ in 0..50 {
            let m = random_mac();
            assert!(m.starts_with("02:"));
            assert_eq!(m.len(), 17);
        }
        assert_ne!(random_mac(), random_mac());
    }
}
