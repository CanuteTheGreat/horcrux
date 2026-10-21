//! Network policy enforcement in the kernel (nftables).
//!
//! Policies select CNI attachments (endpoints) by policy namespace and
//! labels, Kubernetes-style. Everything lives in one table,
//! `bridge horcrux_netpol`, rebuilt atomically (`nft -f -`, one
//! transaction) whenever a policy or an attachment changes and at API
//! startup:
//!
//! - the bridge family sees traffic between containers on the same CNI
//!   bridge (the inet family's forward hook never does without
//!   br_netfilter), plus traffic between a container and the host through
//!   the bridge's `input`/`output` hooks, which covers routed traffic too;
//! - base chains `forward`/`input`/`output` (priority -5, policy accept)
//!   let established/related traffic through, then jump to a per-endpoint
//!   egress chain (`ip saddr <endpoint>`) and ingress chain (`ip daddr
//!   <endpoint>`);
//! - an endpoint chain exists only when some enabled policy selects the
//!   endpoint for that direction; its allow rules (one per rule x peer x
//!   port, tagged `comment "np:<policy id>"`) `return`, and anything else
//!   is dropped - an Ingress policy with no rules is deny-all-ingress;
//! - peers: `PodSelector` = endpoints in the policy's namespace with those
//!   labels, `NamespaceSelector` = endpoints of the namespaces whose labels
//!   (`kubernetes.io/metadata.name: <ns>`) match, `IpBlock` = the CIDR
//!   minus its `except` list.
//!
//! A separate table means the host firewall's `inet horcrux` table (and its
//! reload, which flushes that table) never touches policy rules, and
//! deleting every policy removes the table.
use super::cni::{parse_cidr, CniAttachment};
use super::policy::{
    selector_matches, NetworkPolicy, NetworkPolicyPort, PeerSelector, PolicyType, Protocol,
};
use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use tokio::process::Command;

/// The table policies are enforced in (bridge family).
pub const TABLE: &str = "horcrux_netpol";

/// Result of the last attempt to install the policy rules.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EnforcementStatus {
    /// The kernel ruleset matches the stored policies.
    pub applied: bool,
    /// `bridge horcrux_netpol`
    pub table: String,
    /// Replies of allowed connections pass (bridge conntrack available).
    pub stateful: bool,
    /// Endpoints (attachments) with a policy chain.
    pub endpoints: usize,
    /// Allow rules installed.
    pub rules: usize,
    pub error: Option<String>,
    pub applied_at: Option<String>,
}

/// One endpoint chain of a built ruleset.
#[derive(Debug, Clone)]
pub struct EndpointChain {
    pub chain: String,
    /// `container/network ip`
    pub endpoint: String,
    pub lines: Vec<String>,
    /// Policies that select this endpoint for this direction.
    pub policies: Vec<String>,
}

/// A complete `nft -f` script for the policy table.
#[derive(Debug, Clone)]
pub struct Ruleset {
    pub text: String,
    pub chains: Vec<EndpointChain>,
    pub rules: usize,
    pub endpoints: usize,
}

fn fam(ip: &IpAddr) -> &'static str {
    if ip.is_ipv4() {
        "ip"
    } else {
        "ip6"
    }
}

fn set_of(ips: &[IpAddr]) -> String {
    let mut v: Vec<String> = ips.iter().map(|i| i.to_string()).collect();
    v.sort();
    v.dedup();
    format!("{{ {} }}", v.join(", "))
}

fn namespace_labels(ns: &str) -> HashMap<String, String> {
    HashMap::from([
        ("kubernetes.io/metadata.name".to_string(), ns.to_string()),
        ("name".to_string(), ns.to_string()),
    ])
}

/// nft match for one peer, or None when it selects nothing of this family
/// (a selector matching no endpoint allows nothing).
fn peer_expr(
    peer: &PeerSelector,
    family: &str,
    dir: &str,
    policy_ns: &str,
    endpoints: &[CniAttachment],
) -> Option<String> {
    let same_family = |ip: &IpAddr| fam(ip) == family;
    match peer {
        PeerSelector::PodSelector(sel) => {
            let ips: Vec<IpAddr> = endpoints
                .iter()
                .filter(|e| e.namespace == policy_ns && selector_matches(sel, &e.labels))
                .map(|e| e.ip_address)
                .filter(same_family)
                .collect();
            (!ips.is_empty()).then(|| format!("{} {} {}", family, dir, set_of(&ips)))
        }
        PeerSelector::NamespaceSelector(sel) => {
            let ips: Vec<IpAddr> = endpoints
                .iter()
                .filter(|e| selector_matches(sel, &namespace_labels(&e.namespace)))
                .map(|e| e.ip_address)
                .filter(same_family)
                .collect();
            (!ips.is_empty()).then(|| format!("{} {} {}", family, dir, set_of(&ips)))
        }
        PeerSelector::IpBlock { cidr, except } => {
            let (net, _) = parse_cidr(cidr).ok()?;
            if fam(&net) != family {
                return None;
            }
            let mut s = format!("{} {} {}", family, dir, cidr.trim());
            let ex: Vec<&str> = except
                .iter()
                .map(|e| e.trim())
                .filter(|e| {
                    parse_cidr(e)
                        .map(|(a, _)| fam(&a) == family)
                        .unwrap_or(false)
                })
                .collect();
            if !ex.is_empty() {
                s.push_str(&format!(" {} {} != {{ {} }}", family, dir, ex.join(", ")));
            }
            Some(s)
        }
    }
}

fn port_expr(p: &NetworkPolicyPort) -> String {
    let proto = match p.protocol {
        Protocol::TCP => "tcp",
        Protocol::UDP => "udp",
        Protocol::SCTP => "sctp",
    };
    match (p.port, p.end_port) {
        (Some(a), Some(b)) => format!("{} dport {}-{}", proto, a, b),
        (Some(a), None) => format!("{} dport {}", proto, a),
        _ => format!("meta l4proto {}", proto),
    }
}

/// Build the policy table for `policies` over `endpoints` (attachments).
/// `stateful` adds the conntrack accept for established/related traffic.
pub fn build_ruleset(
    policies: &[NetworkPolicy],
    endpoints: &[CniAttachment],
    stateful: bool,
) -> Ruleset {
    let mut eps: Vec<&CniAttachment> = endpoints.iter().collect();
    eps.sort_by(|a, b| {
        (a.network_name.as_str(), a.container_id.as_str())
            .cmp(&(b.network_name.as_str(), b.container_id.as_str()))
    });
    let mut active: Vec<&NetworkPolicy> = policies.iter().filter(|p| p.enabled).collect();
    active.sort_by(|a, b| a.id.cmp(&b.id));

    let mut chains: Vec<EndpointChain> = Vec::new();
    let mut egress_jumps = Vec::new();
    let mut ingress_jumps = Vec::new();
    let mut rules = 0usize;
    let mut selected = 0usize;

    for (i, ep) in eps.iter().enumerate() {
        let family = fam(&ep.ip_address);
        let label = format!("{}/{} {}", ep.container_id, ep.network_name, ep.ip_address);
        let tag = format!("ep:{}/{}", ep.container_id, ep.network_name);
        let mut any = false;
        for dir in [PolicyType::Egress, PolicyType::Ingress] {
            let selecting: Vec<&&NetworkPolicy> = active
                .iter()
                .filter(|p| {
                    p.namespace == ep.namespace
                        && selector_matches(&p.pod_selector, &ep.labels)
                        && p.effective_types().contains(&dir)
                })
                .collect();
            if selecting.is_empty() {
                continue;
            }
            any = true;
            let (suffix, peer_dir) = match dir {
                PolicyType::Ingress => ("in", "saddr"),
                PolicyType::Egress => ("out", "daddr"),
            };
            let chain = format!("ep{}_{}", i, suffix);
            let mut lines = Vec::new();
            if family == "ip6" && dir == PolicyType::Ingress {
                // Neighbour discovery must keep working or the endpoint
                // becomes unreachable even for allowed traffic.
                lines.push(
                    "icmpv6 type { nd-neighbor-solicit, nd-neighbor-advert } return".to_string(),
                );
            }
            for p in &selecting {
                let comment = format!("comment \"np:{}\"", p.id);
                let rule_sets: Vec<(&[PeerSelector], &[NetworkPolicyPort])> = match dir {
                    PolicyType::Ingress => p
                        .ingress
                        .iter()
                        .map(|r| (r.from.as_slice(), r.ports.as_slice()))
                        .collect(),
                    PolicyType::Egress => p
                        .egress
                        .iter()
                        .map(|r| (r.to.as_slice(), r.ports.as_slice()))
                        .collect(),
                };
                for (peers, ports) in rule_sets {
                    let peer_exprs: Vec<Option<String>> = if peers.is_empty() {
                        vec![None]
                    } else {
                        peers
                            .iter()
                            .filter_map(|pe| {
                                peer_expr(pe, family, peer_dir, &p.namespace, endpoints)
                            })
                            .map(Some)
                            .collect()
                    };
                    let port_exprs: Vec<Option<String>> = if ports.is_empty() {
                        vec![None]
                    } else {
                        ports.iter().map(|po| Some(port_expr(po))).collect()
                    };
                    for pe in &peer_exprs {
                        for po in &port_exprs {
                            let mut parts: Vec<&str> = Vec::new();
                            if let Some(x) = pe {
                                parts.push(x);
                            }
                            if let Some(x) = po {
                                parts.push(x);
                            }
                            parts.push("return");
                            lines.push(format!("{} {}", parts.join(" "), comment));
                            rules += 1;
                        }
                    }
                }
            }
            lines.push(format!("counter drop comment \"np-deny:{}\"", tag));
            let jump = format!(
                "{} {} {} jump {} comment \"{}\"",
                family,
                if dir == PolicyType::Ingress {
                    "daddr"
                } else {
                    "saddr"
                },
                ep.ip_address,
                chain,
                tag
            );
            match dir {
                PolicyType::Ingress => ingress_jumps.push(jump),
                PolicyType::Egress => egress_jumps.push(jump),
            }
            chains.push(EndpointChain {
                chain,
                endpoint: label.clone(),
                lines,
                policies: selecting.iter().map(|p| p.id.clone()).collect(),
            });
        }
        if any {
            selected += 1;
        }
    }

    let mut text = format!("table bridge {TABLE}\ndelete table bridge {TABLE}\n");
    if !chains.is_empty() {
        text.push_str(&format!("table bridge {TABLE} {{\n"));
        for c in &chains {
            text.push_str(&format!("\tchain {} {{\n", c.chain));
            for l in &c.lines {
                text.push_str(&format!("\t\t{}\n", l));
            }
            text.push_str("\t}\n");
        }
        for hook in ["forward", "input", "output"] {
            text.push_str(&format!(
                "\tchain {hook} {{\n\t\ttype filter hook {hook} priority -5; policy accept;\n"
            ));
            if stateful {
                text.push_str("\t\tct state established,related accept\n");
            }
            for j in egress_jumps.iter().chain(ingress_jumps.iter()) {
                text.push_str(&format!("\t\t{}\n", j));
            }
            text.push_str("\t}\n");
        }
        text.push_str("}\n");
    }
    Ruleset {
        text,
        chains,
        rules,
        endpoints: selected,
    }
}

/// The chains a policy contributes to (what `/nftables` shows).
pub fn policy_view(ruleset: &Ruleset, policy_id: &str) -> Vec<String> {
    let mut out = Vec::new();
    for c in ruleset
        .chains
        .iter()
        .filter(|c| c.policies.iter().any(|p| p == policy_id))
    {
        out.push(format!(
            "# table bridge {} chain {} ({}; policies: {})",
            TABLE,
            c.chain,
            c.endpoint,
            c.policies.join(", ")
        ));
        out.extend(c.lines.iter().cloned());
    }
    out
}

/// Load an `nft -f` script (one atomic transaction).
pub async fn run_nft_script(script: &str) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut child = Command::new("nft")
        .args(["-f", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| horcrux_common::Error::System(format!("nftables: cannot run nft: {}", e)))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(script.as_bytes()).await?;
    }
    let out = child.wait_with_output().await?;
    if !out.status.success() {
        return Err(horcrux_common::Error::System(format!(
            "nftables: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Install the rules for `policies` over `endpoints`. Stateful first; if
/// the kernel has no bridge conntrack, the rules go in without it (replies
/// then need their own allow rules) and the status says so.
pub async fn apply(policies: &[NetworkPolicy], endpoints: &[CniAttachment]) -> EnforcementStatus {
    let now = chrono::Utc::now().to_rfc3339();
    let table = format!("bridge {}", TABLE);
    let rs = build_ruleset(policies, endpoints, true);
    if rs.chains.is_empty() {
        // Nothing to enforce: remove a leftover table if we can. A host
        // without nft/CAP_NET_ADMIN has nothing installed either.
        if let Err(e) = run_nft_script(&rs.text).await {
            tracing::debug!("network policies: no table to remove ({})", e);
        }
        return EnforcementStatus {
            applied: true,
            table,
            stateful: true,
            endpoints: 0,
            rules: 0,
            error: None,
            applied_at: Some(now),
        };
    }
    let (result, stateful, rs) = match run_nft_script(&rs.text).await {
        Ok(()) => (Ok(()), true, rs),
        Err(first) => {
            let plain = build_ruleset(policies, endpoints, false);
            match run_nft_script(&plain.text).await {
                Ok(()) => {
                    tracing::warn!(
                        "network policies installed without conntrack (bridge conntrack unavailable: {})",
                        first
                    );
                    (Ok(()), false, plain)
                }
                Err(_) => (Err(first), true, rs),
            }
        }
    };
    match result {
        Ok(()) => {
            tracing::info!(
                "Network policies enforced: {} endpoint(s), {} allow rule(s) in {}",
                rs.endpoints,
                rs.rules,
                table
            );
            EnforcementStatus {
                applied: true,
                table,
                stateful,
                endpoints: rs.endpoints,
                rules: rs.rules,
                error: None,
                applied_at: Some(now),
            }
        }
        Err(e) => {
            tracing::warn!("Network policies NOT enforced: {}", e);
            EnforcementStatus {
                applied: false,
                table,
                stateful,
                endpoints: rs.endpoints,
                rules: rs.rules,
                error: Some(e.to_string()),
                applied_at: Some(now),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdn::policy::{EgressRule, IngressRule, LabelSelector};

    fn ep(ct: &str, ip: &str, labels: &[(&str, &str)]) -> CniAttachment {
        CniAttachment {
            container_id: ct.into(),
            network_name: "qa-sdn".into(),
            interface_name: "net0".into(),
            ip_address: ip.parse().unwrap(),
            mac_address: String::new(),
            gateway: None,
            netns_path: String::new(),
            guest: None,
            namespace: "default".into(),
            labels: labels
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    fn policy(id: &str, types: Vec<PolicyType>) -> NetworkPolicy {
        NetworkPolicy {
            id: id.into(),
            name: id.into(),
            namespace: "default".into(),
            pod_selector: LabelSelector::default(),
            policy_types: types,
            ingress: vec![],
            egress: vec![],
            enabled: true,
        }
    }

    /// Daily QA 2026-10-21: "qa-deny-ingress" (Ingress, empty selector)
    /// was stored but nothing reached the kernel. It must give every
    /// endpoint in the namespace an ingress chain that only drops.
    #[test]
    fn deny_all_ingress_drops_to_every_endpoint() {
        let eps = [ep("a", "10.251.0.2", &[]), ep("b", "10.251.0.3", &[])];
        let rs = build_ruleset(&[policy("qa-deny", vec![PolicyType::Ingress])], &eps, true);
        assert_eq!(rs.endpoints, 2);
        assert_eq!(rs.rules, 0);
        let t = &rs.text;
        assert!(t.starts_with("table bridge horcrux_netpol\ndelete table bridge horcrux_netpol\n"));
        assert!(
            t.contains("chain ep0_in {\n\t\tcounter drop comment \"np-deny:ep:a/qa-sdn\""),
            "{t}"
        );
        assert!(
            t.contains("ip daddr 10.251.0.2 jump ep0_in comment \"ep:a/qa-sdn\""),
            "{t}"
        );
        assert!(t.contains("ip daddr 10.251.0.3 jump ep1_in"), "{t}");
        for hook in ["forward", "input", "output"] {
            assert!(t.contains(&format!(
                "type filter hook {hook} priority -5; policy accept;"
            )));
        }
        assert!(t.contains("ct state established,related accept"));
        assert!(
            !t.contains("_out"),
            "no egress chains for an ingress policy: {t}"
        );
        assert!(
            !build_ruleset(&[policy("qa-deny", vec![PolicyType::Ingress])], &eps, false)
                .text
                .contains("ct state")
        );
        let view = policy_view(&rs, "qa-deny");
        assert!(
            view[0].starts_with("# table bridge horcrux_netpol chain ep0_in (a/qa-sdn 10.251.0.2")
        );
    }

    #[test]
    fn selectors_peers_and_ports_become_allow_rules() {
        let eps = [
            ep("web", "10.251.0.2", &[("app", "web")]),
            ep("db", "10.251.0.3", &[("app", "db")]),
            ep("other", "10.251.0.4", &[]),
        ];
        let mut p = policy("db-from-web", vec![PolicyType::Ingress]);
        p.pod_selector
            .match_labels
            .insert("app".into(), "db".into());
        let mut web = LabelSelector::default();
        web.match_labels.insert("app".into(), "web".into());
        p.ingress = vec![
            IngressRule {
                from: vec![PeerSelector::PodSelector(web)],
                ports: vec![NetworkPolicyPort {
                    protocol: Protocol::TCP,
                    port: Some(5432),
                    end_port: None,
                }],
            },
            IngressRule {
                from: vec![PeerSelector::IpBlock {
                    cidr: "192.168.0.0/16".into(),
                    except: vec!["192.168.9.0/24".into()],
                }],
                ports: vec![NetworkPolicyPort {
                    protocol: Protocol::UDP,
                    port: Some(1000),
                    end_port: Some(2000),
                }],
            },
        ];
        let rs = build_ruleset(&[p], &eps, true);
        assert_eq!(rs.endpoints, 1, "only db is selected");
        let t = &rs.text;
        assert!(
            t.contains("ip saddr { 10.251.0.2 } tcp dport 5432 return comment \"np:db-from-web\""),
            "{t}"
        );
        assert!(
            t.contains(
                "ip saddr 192.168.0.0/16 ip saddr != { 192.168.9.0/24 } udp dport 1000-2000 return"
            ),
            "{t}"
        );
        assert!(t.contains("ip daddr 10.251.0.3 jump ep0_in"), "{t}");
        assert!(!t.contains("10.251.0.4 jump"), "{t}");
        assert_eq!(rs.rules, 2);
    }

    #[test]
    fn egress_rules_and_disabled_policies() {
        let eps = [ep("a", "10.251.0.2", &[])];
        let mut p = policy("egress-dns", vec![PolicyType::Egress]);
        p.egress = vec![EgressRule {
            to: vec![],
            ports: vec![NetworkPolicyPort {
                protocol: Protocol::UDP,
                port: Some(53),
                end_port: None,
            }],
        }];
        let rs = build_ruleset(&[p.clone()], &eps, true);
        assert!(rs.text.contains("chain ep0_out"), "{}", rs.text);
        assert!(rs
            .text
            .contains("udp dport 53 return comment \"np:egress-dns\""));
        assert!(rs.text.contains("ip saddr 10.251.0.2 jump ep0_out"));
        // policy_types empty: Ingress (+ Egress since it has egress rules)
        let mut implicit = p.clone();
        implicit.policy_types = vec![];
        let rs = build_ruleset(&[implicit], &eps, true);
        assert!(rs.text.contains("ep0_in") && rs.text.contains("ep0_out"));
        // disabled -> nothing but removing the table
        p.enabled = false;
        let rs = build_ruleset(&[p], &eps, true);
        assert!(rs.chains.is_empty());
        assert_eq!(
            rs.text,
            "table bridge horcrux_netpol\ndelete table bridge horcrux_netpol\n"
        );
        // other namespace -> not selected
        let mut other = policy("x", vec![PolicyType::Ingress]);
        other.namespace = "prod".into();
        assert!(build_ruleset(&[other], &eps, true).chains.is_empty());
    }

    #[test]
    fn ipv6_endpoints_keep_neighbour_discovery() {
        let eps = [ep("v6", "fd00:10::5", &[])];
        let rs = build_ruleset(&[policy("deny", vec![PolicyType::Ingress])], &eps, true);
        assert!(rs
            .text
            .contains("icmpv6 type { nd-neighbor-solicit, nd-neighbor-advert } return"));
        assert!(
            rs.text.contains("ip6 daddr fd00:10::5 jump ep0_in"),
            "{}",
            rs.text
        );
    }
}
