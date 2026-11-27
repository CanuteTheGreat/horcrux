//! Guest microsegmentation: per-guest default-deny profiles, templates,
//! "allow from group X" rules, per-rule hit counters and the flow view
//! (docs/design/security-suite.md, "Guest microsegmentation").
//!
//! A profile expands into ordinary [`FirewallRule`]s that the bridge-family
//! guest chains of [`super::nftables`] install (`vm-<id>-in/out`,
//! `ct-<id>-in/out`), so it is enforced per NIC on every bridge, SDN vnet
//! and veth the guest has. Macvtap NICs are not bridge ports; they get the
//! same rules as netdev-family hooks on the device (stateless, see
//! [`netdev_commands`]).
use super::{Direction, FirewallAction, FirewallRule, Protocol};
use horcrux_common::security::{
    GuestProfile, MicrosegSettings, MsegDefault, MsegFlow, MsegPeer, MsegRuleHit, MsegTemplate,
    MsegTemplateInfo, MSEG_LOG_PREFIX,
};

/// The nft tail of one guest direction: what to accept first, the rules in
/// order, and the default.
#[derive(Debug, Clone)]
pub struct Plan {
    /// Rules generated from the template and the allow lists.
    pub rules: Vec<FirewallRule>,
    pub default_in_deny: bool,
    pub default_out_deny: bool,
    pub log_denied: bool,
    pub log_rate: u32,
    /// ARP, neighbour discovery and DHCP always pass in a default-deny
    /// direction.
    pub infra: bool,
}

/// Turn a profile into its plan. Pure: no kernel access.
pub fn plan(p: &GuestProfile, s: &MicrosegSettings) -> Plan {
    let mut rules: Vec<FirewallRule> = Vec::new();
    let mut n = 0u32;
    let mut push = |id: &str,
                    dir: Direction,
                    proto: Option<Protocol>,
                    src: Option<&str>,
                    dst: Option<&str>,
                    dport: Option<String>,
                    comment: &str| {
        n += 1;
        rules.push(FirewallRule {
            id: format!("mseg-{id}-{n}"),
            enabled: true,
            action: FirewallAction::Accept,
            direction: dir,
            protocol: proto,
            source: src.map(str::to_string),
            dest: dst.map(str::to_string),
            sport: None,
            dport,
            comment: Some(comment.to_string()),
            log: false,
            position: 1000 + n,
        });
    };
    let ports = if p.ports.trim().is_empty() {
        p.template.default_ports().to_string()
    } else {
        p.ports.trim().to_string()
    };
    let nft_ports = |list: &str| -> Option<String> {
        let l = list.trim();
        if l.is_empty() {
            None
        } else if l.contains(',') {
            let items: Vec<&str> = l.split(',').map(str::trim).collect();
            Some(format!("{{ {} }}", items.join(", ")))
        } else {
            Some(l.to_string())
        }
    };
    let proto_of = |s: &str| match s {
        "tcp" => Some(Protocol::Tcp),
        "udp" => Some(Protocol::Udp),
        "icmp" => Some(Protocol::Icmp),
        _ => None,
    };
    let lab = p.lab_network.trim();
    match p.template {
        MsegTemplate::Custom => {}
        MsegTemplate::WebServer => {
            push(
                "web",
                Direction::In,
                Some(Protocol::Tcp),
                None,
                None,
                nft_ports(&ports),
                "web server ports",
            );
            for proto in [Protocol::Udp, Protocol::Tcp] {
                push(
                    "dns",
                    Direction::Out,
                    Some(proto),
                    None,
                    None,
                    Some("53".into()),
                    "DNS",
                );
            }
            push(
                "ntp",
                Direction::Out,
                Some(Protocol::Udp),
                None,
                None,
                Some("123".into()),
                "NTP",
            );
            push(
                "https",
                Direction::Out,
                Some(Protocol::Tcp),
                None,
                None,
                nft_ports("80,443"),
                "updates and API calls",
            );
        }
        MsegTemplate::Database => {
            for peer in &p.allow_from {
                // A peer without its own port gets the database ports.
                if peer.dport.trim().is_empty() && peer.protocol == "any" {
                    push(
                        "db",
                        Direction::In,
                        Some(Protocol::Tcp),
                        Some(peer.group.trim()),
                        None,
                        nft_ports(&ports),
                        "database clients",
                    );
                }
            }
            for proto in [Protocol::Udp, Protocol::Tcp] {
                push(
                    "dns",
                    Direction::Out,
                    Some(proto),
                    None,
                    None,
                    Some("53".into()),
                    "DNS",
                );
            }
            push(
                "ntp",
                Direction::Out,
                Some(Protocol::Udp),
                None,
                None,
                Some("123".into()),
                "NTP",
            );
        }
        MsegTemplate::IsolatedLab => {
            push(
                "lab",
                Direction::In,
                None,
                Some(lab),
                None,
                None,
                "lab network",
            );
            push(
                "lab",
                Direction::Out,
                None,
                None,
                Some(lab),
                None,
                "lab network",
            );
        }
        MsegTemplate::PentestBox => {
            push(
                "lab",
                Direction::Out,
                None,
                None,
                Some(lab),
                None,
                "lab network only",
            );
            if ports.is_empty() {
                push(
                    "callback",
                    Direction::In,
                    None,
                    Some(lab),
                    None,
                    None,
                    "callbacks from the lab",
                );
            } else {
                for proto in [Protocol::Tcp, Protocol::Udp] {
                    push(
                        "callback",
                        Direction::In,
                        Some(proto),
                        Some(lab),
                        None,
                        nft_ports(&ports),
                        "callbacks from the lab",
                    );
                }
            }
        }
    }
    let mut peer_rules = |peers: &[MsegPeer], dir: Direction, tag: &str| {
        for peer in peers {
            // The database template already expanded portless TCP peers.
            if p.template == MsegTemplate::Database
                && dir == Direction::In
                && peer.dport.trim().is_empty()
                && peer.protocol == "any"
            {
                continue;
            }
            let g = peer.group.trim();
            let (src, dst) = match dir {
                Direction::In => (Some(g), None),
                Direction::Out => (None, Some(g)),
            };
            let c = if peer.comment.is_empty() {
                format!("{tag} {g}")
            } else {
                peer.comment.clone()
            };
            n += 1;
            rules.push(FirewallRule {
                id: format!("mseg-{tag}-{n}"),
                enabled: true,
                action: FirewallAction::Accept,
                direction: dir,
                protocol: proto_of(&peer.protocol),
                source: src.map(str::to_string),
                dest: dst.map(str::to_string),
                sport: None,
                dport: nft_ports(&peer.dport),
                comment: Some(c),
                log: false,
                position: 1000 + n,
            });
        }
    };
    peer_rules(&p.allow_from, Direction::In, "from");
    peer_rules(&p.allow_to, Direction::Out, "to");
    Plan {
        rules,
        default_in_deny: p.default_in == MsegDefault::Deny,
        default_out_deny: p.default_out == MsegDefault::Deny,
        log_denied: s.log_denied,
        log_rate: s.log_rate_per_second,
        infra: s.allow_infrastructure,
    }
}

/// What a template allows, for the picker.
pub fn template_infos() -> Vec<MsegTemplateInfo> {
    let allows = |t: MsegTemplate| -> Vec<String> {
        let p = GuestProfile {
            template: t,
            lab_network: "<lab network>".into(),
            ..Default::default()
        };
        let pl = plan(&p, &MicrosegSettings::default());
        let mut out: Vec<String> = pl
            .rules
            .iter()
            .map(|r| {
                format!(
                    "{} {}{}{}{}",
                    match r.direction {
                        Direction::In => "in:",
                        Direction::Out => "out:",
                    },
                    match r.protocol {
                        Some(Protocol::Tcp) => "tcp",
                        Some(Protocol::Udp) => "udp",
                        Some(Protocol::Icmp) => "icmp",
                        _ => "any",
                    },
                    r.dport
                        .as_deref()
                        .map(|d| format!(" port {d}"))
                        .unwrap_or_default(),
                    r.source
                        .as_deref()
                        .map(|d| format!(" from {d}"))
                        .unwrap_or_default(),
                    r.dest
                        .as_deref()
                        .map(|d| format!(" to {d}"))
                        .unwrap_or_default(),
                )
            })
            .collect();
        if t == MsegTemplate::Database {
            out.insert(0, "in: tcp database ports from each allowed group".into());
        }
        out
    };
    MsegTemplate::ALL
        .iter()
        .map(|t| MsegTemplateInfo {
            id: t.id().into(),
            title: t.title().into(),
            description: t.description().into(),
            needs_lab_network: t.needs_lab_network(),
            default_ports: t.default_ports().into(),
            allows: allows(*t),
        })
        .collect()
}

/// A rule id as it is written into an nft rule comment.
pub fn comment_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':') {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect()
}

/// `expr` (ending in its verdict) with a counter before the verdict and the
/// rule id as comment.
pub fn counted(expr: &str, id: &str) -> String {
    let (head, verdict) = expr.rsplit_once(' ').unwrap_or(("", expr));
    let head = if head.is_empty() {
        String::new()
    } else {
        format!("{head} ")
    };
    format!("{head}counter {verdict} comment \"hx:{}\"", comment_id(id))
}

/// The ordered rule expressions of one direction chain of a guest.
/// `build` expands one rule into nft expressions (objects resolved);
/// `stateful` = conntrack is available at the hook (bridge chains), else
/// replies are matched statelessly (netdev hooks run before conntrack).
pub fn chain_body(
    plan: &Plan,
    guest_key: &str,
    dir: Direction,
    legacy: &[FirewallRule],
    stateful: bool,
    build: &dyn Fn(&FirewallRule) -> Vec<String>,
) -> Vec<String> {
    let deny = match dir {
        Direction::In => plan.default_in_deny,
        Direction::Out => plan.default_out_deny,
    };
    let dname = match dir {
        Direction::In => "in",
        Direction::Out => "out",
    };
    let mut out: Vec<String> = Vec::new();
    if deny {
        if stateful {
            out.push(
                "ct state established,related counter accept comment \"hx:established\"".into(),
            );
            out.push("ct state invalid counter drop comment \"hx:invalid\"".into());
        } else {
            // netdev hooks run before conntrack: accept every TCP segment
            // but a new connection's first SYN, and the ICMP answers a
            // connection needs.
            out.push(
                "meta l4proto tcp tcp flags & (syn | ack) != syn counter accept comment \"hx:tcp-replies\""
                    .into(),
            );
            out.push(
                "ip protocol icmp icmp type { echo-reply, destination-unreachable, time-exceeded } counter accept comment \"hx:icmp-replies\""
                    .into(),
            );
        }
        if plan.infra {
            out.push("ether type arp counter accept comment \"hx:arp\"".into());
            out.push(
                "ip6 nexthdr icmpv6 icmpv6 type { nd-router-solicit, nd-router-advert, nd-neighbor-solicit, nd-neighbor-advert } counter accept comment \"hx:nd\""
                    .into(),
            );
            let (s, d) = match dir {
                Direction::In => (67, 68),
                Direction::Out => (68, 67),
            };
            out.push(format!(
                "meta l4proto udp udp sport {s} udp dport {d} counter accept comment \"hx:dhcp\""
            ));
        }
    }
    let mut all: Vec<&FirewallRule> = legacy.iter().filter(|r| r.enabled).collect();
    all.sort_by_key(|r| r.position);
    all.extend(plan.rules.iter());
    for r in all.iter().filter(|r| r.direction == dir) {
        for e in build(r) {
            out.push(counted(&e, &r.id));
        }
    }
    if !stateful && deny {
        // UDP answers: mirror every accepted UDP rule of the other
        // direction with ports and addresses swapped.
        for r in all.iter().filter(|r| {
            r.direction != dir
                && r.action == FirewallAction::Accept
                && r.protocol == Some(Protocol::Udp)
        }) {
            let m = FirewallRule {
                id: format!("{}-reply", r.id),
                direction: dir,
                source: r.dest.clone(),
                dest: r.source.clone(),
                sport: r.dport.clone(),
                dport: r.sport.clone(),
                ..(*r).clone()
            };
            for e in build(&m) {
                out.push(counted(&e, &m.id));
            }
        }
    }
    if deny {
        if plan.log_denied {
            out.push(format!(
                "limit rate {}/second log prefix \"{}{}/{} \"",
                plan.log_rate.max(1),
                MSEG_LOG_PREFIX,
                guest_key,
                dname
            ));
        }
        out.push("counter drop comment \"hx:default\"".into());
    }
    out
}

/// Commands for the macvtap devices of a guest (netdev family). The device
/// hook sees what the guest sends (`ingress`, the rules of direction `out`)
/// and what it receives (`egress`, direction `in`).
pub fn netdev_commands(
    table: &str,
    guest_key: &str,
    devices: &[String],
    plan: &Plan,
    legacy: &[FirewallRule],
    build: &dyn Fn(&FirewallRule) -> Vec<String>,
) -> Vec<Vec<String>> {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let mut cmds = vec![s(&["add", "table", "netdev", table])];
    for dev in devices {
        for (dir, hook, suffix) in [
            (Direction::Out, "ingress", "out"),
            (Direction::In, "egress", "in"),
        ] {
            let chain = netdev_chain(dev, suffix);
            cmds.push(vec![
                "add".into(),
                "chain".into(),
                "netdev".into(),
                table.into(),
                chain.clone(),
                format!(
                    "{{ type filter hook {hook} device \"{dev}\" priority 0 ; policy accept ; }}"
                ),
            ]);
            cmds.push(s(&["flush", "chain", "netdev", table, &chain]));
            for e in chain_body(plan, guest_key, dir, legacy, false, build) {
                cmds.push(vec![
                    "add".into(),
                    "rule".into(),
                    "netdev".into(),
                    table.into(),
                    chain.clone(),
                    e,
                ]);
            }
        }
    }
    cmds
}

pub fn netdev_chain(dev: &str, suffix: &str) -> String {
    format!("{dev}-{suffix}")
}

/// Rule hits out of `nft -j list chain ...`: one entry per rule that
/// carries one of our comments (`hx:<rule id>`).
pub fn parse_hits(json: &str, guest: &str, direction: &str) -> Vec<MsegRuleHit> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(items) = v.get("nftables").and_then(|x| x.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for it in items {
        let Some(rule) = it.get("rule") else { continue };
        let Some(cm) = rule.get("comment").and_then(|c| c.as_str()) else {
            continue;
        };
        let Some(id) = cm.strip_prefix("hx:") else {
            continue;
        };
        let exprs = rule
            .get("expr")
            .and_then(|e| e.as_array())
            .cloned()
            .unwrap_or_default();
        let mut packets = 0;
        let mut bytes = 0;
        let mut action = String::new();
        for e in &exprs {
            if let Some(c) = e.get("counter") {
                packets = c.get("packets").and_then(|x| x.as_u64()).unwrap_or(0);
                bytes = c.get("bytes").and_then(|x| x.as_u64()).unwrap_or(0);
            }
            for verdict in ["accept", "drop", "reject"] {
                if e.get(verdict).is_some() {
                    action = verdict.to_string();
                }
            }
        }
        out.push(MsegRuleHit {
            guest: guest.to_string(),
            direction: direction.to_string(),
            rule_id: id.to_string(),
            action,
            rule: String::new(),
            packets,
            bytes,
        });
    }
    out
}

/// Tracked flows from `/proc/net/nf_conntrack` or `conntrack -L` text. `ip` keeps only flows
/// with that address on either side.
pub fn parse_conntrack(text: &str, ip: Option<&str>, limit: usize) -> Vec<MsegFlow> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut it = line.split_whitespace().peekable();
        // /proc/net/nf_conntrack: `ipv4 2 tcp 6 117 ESTABLISHED src=..`;
        // `conntrack -L`: `tcp 6 117 ESTABLISHED src=..`.
        if matches!(it.peek(), Some(&"ipv4") | Some(&"ipv6")) {
            it.next();
            it.next();
        }
        let Some(proto) = it.next() else { continue };
        let rest: Vec<&str> = it.collect();
        let kv = |k: &str, nth: usize| -> String {
            rest.iter()
                .filter_map(|f| f.strip_prefix(k))
                .nth(nth)
                .unwrap_or("")
                .to_string()
        };
        let src = kv("src=", 0);
        let dst = kv("dst=", 0);
        if src.is_empty() {
            continue;
        }
        if let Some(ip) = ip {
            if src != ip && dst != ip && kv("src=", 1) != ip && kv("dst=", 1) != ip {
                continue;
            }
        }
        let state = rest
            .iter()
            .find(|f| {
                !f.contains('=')
                    && f.chars().all(|c| c.is_ascii_uppercase() || c == '_')
                    && !f.is_empty()
            })
            .map(|s| s.to_string())
            .unwrap_or_default();
        let packets =
            kv("packets=", 0).parse().unwrap_or(0) + kv("packets=", 1).parse().unwrap_or(0);
        let bytes = kv("bytes=", 0).parse().unwrap_or(0) + kv("bytes=", 1).parse().unwrap_or(0);
        out.push(MsegFlow {
            proto: proto.to_string(),
            src,
            dst,
            sport: kv("sport=", 0),
            dport: kv("dport=", 0),
            state,
            packets,
            bytes,
        });
        if out.len() >= limit {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(r: &FirewallRule) -> Vec<String> {
        let mut parts = Vec::new();
        if let Some(s) = &r.source {
            parts.push(format!("ip saddr {s}"));
        }
        if let Some(d) = &r.dest {
            parts.push(format!("ip daddr {d}"));
        }
        match (&r.protocol, &r.dport) {
            (Some(Protocol::Tcp), Some(d)) => parts.push(format!("tcp dport {d}")),
            (Some(Protocol::Udp), Some(d)) => parts.push(format!("udp dport {d}")),
            _ => {}
        }
        parts.push("accept".into());
        vec![parts.join(" ")]
    }

    fn prof(t: MsegTemplate) -> GuestProfile {
        GuestProfile {
            template: t,
            lab_network: "10.99.0.0/24".into(),
            ..Default::default()
        }
    }

    #[test]
    fn web_server_opens_web_ports_in_and_dns_out() {
        let p = plan(&prof(MsegTemplate::WebServer), &MicrosegSettings::default());
        assert!(p.default_in_deny && p.default_out_deny);
        assert!(p.rules.iter().any(|r| r.direction == Direction::In
            && r.dport.as_deref() == Some("{ 80, 443 }")
            && r.source.is_none()));
        assert!(p
            .rules
            .iter()
            .any(|r| r.direction == Direction::Out && r.dport.as_deref() == Some("53")));
    }

    #[test]
    fn database_only_lets_listed_groups_in() {
        let mut g = prof(MsegTemplate::Database);
        g.allow_from.push(MsegPeer {
            group: "+app".into(),
            ..Default::default()
        });
        let p = plan(&g, &MicrosegSettings::default());
        let ins: Vec<_> = p
            .rules
            .iter()
            .filter(|r| r.direction == Direction::In)
            .collect();
        assert_eq!(ins.len(), 1);
        assert_eq!(ins[0].source.as_deref(), Some("+app"));
        assert_eq!(ins[0].dport.as_deref(), Some("{ 3306, 5432 }"));
        // no group = nothing in
        let p = plan(&prof(MsegTemplate::Database), &MicrosegSettings::default());
        assert!(!p.rules.iter().any(|r| r.direction == Direction::In));
    }

    #[test]
    fn pentest_box_goes_only_to_the_lab() {
        let p = plan(
            &prof(MsegTemplate::PentestBox),
            &MicrosegSettings::default(),
        );
        let outs: Vec<_> = p
            .rules
            .iter()
            .filter(|r| r.direction == Direction::Out)
            .collect();
        assert_eq!(outs.len(), 1);
        assert_eq!(outs[0].dest.as_deref(), Some("10.99.0.0/24"));
        assert!(p.default_out_deny);
    }

    #[test]
    fn allow_from_group_becomes_an_inbound_rule() {
        let mut g = GuestProfile::default();
        g.allow_from.push(MsegPeer {
            group: "+ops".into(),
            protocol: "tcp".into(),
            dport: "22".into(),
            comment: String::new(),
        });
        let p = plan(&g, &MicrosegSettings::default());
        assert_eq!(p.rules.len(), 1);
        assert_eq!(p.rules[0].source.as_deref(), Some("+ops"));
        assert_eq!(p.rules[0].protocol, Some(Protocol::Tcp));
    }

    #[test]
    fn counted_puts_counter_before_the_verdict() {
        assert_eq!(
            counted("ip saddr 1.2.3.4 tcp dport 22 accept", "r 1"),
            "ip saddr 1.2.3.4 tcp dport 22 counter accept comment \"hx:r_1\""
        );
        assert_eq!(counted("drop", "d"), "counter drop comment \"hx:d\"");
        assert_eq!(
            counted("tcp dport 22 log prefix \"hx-fw: \" drop", "x"),
            "tcp dport 22 log prefix \"hx-fw: \" counter drop comment \"hx:x\""
        );
    }

    #[test]
    fn default_deny_chain_has_established_infra_rules_and_a_logged_drop() {
        let p = plan(&prof(MsegTemplate::WebServer), &MicrosegSettings::default());
        let body = chain_body(&p, "vm:100", Direction::In, &[], true, &build);
        assert_eq!(
            body[0],
            "ct state established,related counter accept comment \"hx:established\""
        );
        assert!(body.iter().any(|l| l.contains("ether type arp")));
        assert!(body
            .iter()
            .any(|l| l.contains("tcp dport { 80, 443 } counter accept")));
        let n = body.len();
        assert!(body[n - 2].starts_with("limit rate 10/second log prefix \"hx-mseg: vm:100/in \""));
        assert_eq!(body[n - 1], "counter drop comment \"hx:default\"");
    }

    #[test]
    fn default_allow_direction_gets_no_drop() {
        let g = GuestProfile {
            default_out: MsegDefault::Allow,
            ..Default::default()
        };
        let p = plan(&g, &MicrosegSettings::default());
        let body = chain_body(&p, "vm:1", Direction::Out, &[], true, &build);
        assert!(body.is_empty());
        let body = chain_body(&p, "vm:1", Direction::In, &[], true, &build);
        assert!(body.last().unwrap().contains("hx:default"));
    }

    #[test]
    fn log_off_has_no_log_rule() {
        let s = MicrosegSettings {
            log_denied: false,
            ..Default::default()
        };
        let p = plan(&GuestProfile::default(), &s);
        let body = chain_body(&p, "vm:1", Direction::In, &[], true, &build);
        assert!(!body.iter().any(|l| l.contains("log prefix")));
    }

    #[test]
    fn stateless_chain_mirrors_udp_replies() {
        let g = prof(MsegTemplate::WebServer);
        let p = plan(&g, &MicrosegSettings::default());
        // outbound DNS (udp 53) -> inbound reply udp sport 53
        let body = chain_body(&p, "vm:1", Direction::In, &[], false, &build);
        assert!(body.iter().any(|l| l.contains("hx:tcp-replies")));
        assert!(body
            .iter()
            .any(|l| l.contains("-reply\"") && l.contains("accept")));
    }

    #[test]
    fn netdev_commands_hook_the_device_both_ways() {
        let p = plan(
            &prof(MsegTemplate::IsolatedLab),
            &MicrosegSettings::default(),
        );
        let cmds = netdev_commands("hxmseg", "vm:7", &["tapabcdi0".into()], &p, &[], &build);
        let flat: Vec<String> = cmds.iter().map(|c| c.join(" ")).collect();
        assert!(flat
            .iter()
            .any(|c| c.contains("hook ingress device \"tapabcdi0\"")));
        assert!(flat
            .iter()
            .any(|c| c.contains("hook egress device \"tapabcdi0\"")));
        assert!(flat
            .iter()
            .any(|c| c.starts_with("add rule netdev hxmseg tapabcdi0-out ip daddr 10.99.0.0/24")));
    }

    #[test]
    fn parses_hits_from_nft_json() {
        let j = r#"{"nftables":[{"metainfo":{"version":"1.0.9"}},{"rule":{"family":"bridge","table":"horcrux","chain":"vm-1-in","handle":5,"comment":"hx:mseg-web-1","expr":[{"match":{}},{"counter":{"packets":7,"bytes":420}},{"accept":null}]}},{"rule":{"family":"bridge","table":"horcrux","chain":"vm-1-in","handle":6,"expr":[{"drop":null}]}},{"rule":{"chain":"vm-1-in","handle":7,"comment":"hx:default","expr":[{"counter":{"packets":2,"bytes":120}},{"drop":null}]}}]}"#;
        let h = parse_hits(j, "vm:1", "in");
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].rule_id, "mseg-web-1");
        assert_eq!(
            (h[0].packets, h[0].bytes, h[0].action.as_str()),
            (7, 420, "accept")
        );
        assert_eq!(h[1].action, "drop");
    }

    #[test]
    fn parses_conntrack_lines() {
        let t = "ipv4     2 tcp      6 117 ESTABLISHED src=10.0.0.5 dst=10.0.0.9 sport=40000 dport=443 packets=5 bytes=300 src=10.0.0.9 dst=10.0.0.5 sport=443 dport=40000 packets=4 bytes=900 [ASSURED] mark=0 zone=0 use=2\nipv4     2 udp      17 20 src=10.0.0.7 dst=8.8.8.8 sport=5000 dport=53 [UNREPLIED] src=8.8.8.8 dst=10.0.0.7 sport=53 dport=5000 mark=0 use=2\n";
        let f = parse_conntrack(t, None, 10);
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].state, "ESTABLISHED");
        assert_eq!((f[0].packets, f[0].bytes), (9, 1200));
        let f = parse_conntrack(t, Some("8.8.8.8"), 10);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].dport, "53");
        assert_eq!(parse_conntrack(t, None, 1).len(), 1);
        // `conntrack -L` has no family columns
        let c = "tcp      6 431999 ESTABLISHED src=10.77.0.30 dst=10.77.0.10 sport=51000 dport=80 packets=6 bytes=400 src=10.77.0.10 dst=10.77.0.30 sport=80 dport=51000 packets=4 bytes=500 [ASSURED] mark=0 use=1\n";
        let f = parse_conntrack(c, Some("10.77.0.10"), 10);
        assert_eq!(f.len(), 1);
        assert_eq!(
            (
                f[0].proto.as_str(),
                f[0].dport.as_str(),
                f[0].state.as_str()
            ),
            ("tcp", "80", "ESTABLISHED")
        );
    }
}
