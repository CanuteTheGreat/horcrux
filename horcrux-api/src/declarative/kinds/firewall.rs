//! Firewall: options (host default policies), aliases, IP sets, security
//! groups and rules (datacenter, node, VM and container scopes).
//!
//! ```yaml
//! firewall:
//!   options: {policy_in: accept, policy_out: accept}
//!   aliases:  [{name: office, cidr: 192.0.2.0/24}]
//!   ipsets:   [{name: admins, entries: [192.0.2.10, 192.0.2.11]}]
//!   security_groups:
//!     - name: web
//!       rules: [{id: https, action: ACCEPT, protocol: tcp, dport: "443"}]
//!   rules:                       # one list, in evaluation order per scope
//!     - {scope: datacenter, id: ssh-office, action: ACCEPT, protocol: tcp, source: office, dport: "22"}
//!     - {scope: vm:web1, id: allow-https, action: ACCEPT, protocol: tcp, dport: "443"}
//!   allow_lockout: false         # see below
//! ```
//!
//! Semantics:
//! - A scope that appears in `rules` is managed *as an ordered list*: its
//!   rules end up in spec order (unlisted live rules of that scope stay,
//!   after the listed ones, unless `prune`). The manager has no rule edit or
//!   reorder, so a changed or displaced rule and everything after it is
//!   removed and re-added.
//! - Host scopes (`datacenter`, `node:<name>`) are loaded into nftables with
//!   an explicit `fw_reload` step at the end of the plan.
//! - Security groups can be created but not edited or deleted (the manager
//!   has no such call yet): a difference is `blocked`, never ignored.
//! - Anti-lockout: a plan that adds a rule dropping/rejecting the API port
//!   (from the caller's address, or from anywhere) ahead of any accept, while
//!   the inbound host policy is `accept`, is refused unless `allow_lockout:
//!   true`. (With `policy_in: drop` the manager itself always keeps SSH and
//!   the API port open.)
use super::ctx::Ctx;
use super::{ch, to_json, Kind, KindPlan};
use crate::declarative::{Action, Change, Live, Spec};
use crate::AppState;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;
use std::sync::Arc;

fn yes() -> bool {
    true
}
fn is_true(b: &bool) -> bool {
    *b
}
fn is_false(b: &bool) -> bool {
    !*b
}
fn dir_in() -> String {
    "in".into()
}
fn is_in(s: &String) -> bool {
    s == "in"
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct RuleSpec {
    pub id: String,
    /// Required in `rules`, forbidden inside a security group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    /// ACCEPT | DROP | REJECT
    pub action: String,
    #[serde(default = "dir_in", skip_serializing_if = "is_in")]
    pub direction: String,
    /// tcp | udp | icmp | icmpv6 | any (absent = any)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sport: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dport: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub log: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OptionsSpec {
    pub policy_in: String,
    pub policy_out: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AliasSpec {
    pub name: String,
    pub cidr: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub comment: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct IpSetSpec {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub comment: String,
    #[serde(default)]
    pub entries: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SecurityGroupSpec {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default)]
    pub rules: Vec<RuleSpec>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FirewallSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<OptionsSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aliases: Option<Vec<AliasSpec>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipsets: Option<Vec<IpSetSpec>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security_groups: Option<Vec<SecurityGroupSpec>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<RuleSpec>>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub allow_lockout: bool,
}

/// What the node has: every field is the spec's own type, normalised.
#[derive(Debug, Clone, Default)]
pub struct LiveFirewall {
    pub options: Option<OptionsSpec>,
    pub aliases: Vec<AliasSpec>,
    pub ipsets: Vec<IpSetSpec>,
    pub security_groups: Vec<SecurityGroupSpec>,
    /// Gathered scopes (datacenter, every scope named by the spec, guests).
    pub rules: BTreeMap<String, Vec<RuleSpec>>,
}

impl LiveFirewall {
    /// The exported live section (spec shape) back as a live view: the rules
    /// grouped by scope, in order.
    pub fn from_spec(s: &FirewallSpec) -> LiveFirewall {
        let mut rules: BTreeMap<String, Vec<RuleSpec>> = BTreeMap::new();
        for r in s.rules.iter().flatten() {
            let mut x = r.clone();
            let scope = x.scope.take().unwrap_or_default();
            rules.entry(scope).or_default().push(x);
        }
        LiveFirewall {
            options: s.options.clone(),
            aliases: s.aliases.clone().unwrap_or_default(),
            ipsets: s.ipsets.clone().unwrap_or_default(),
            security_groups: s.security_groups.clone().unwrap_or_default(),
            rules,
        }
    }
}

// ---------------------------------------------------------------- helpers

pub fn canon_action(a: &str) -> String {
    a.trim().to_ascii_uppercase()
}

pub fn canon_proto(p: &Option<String>) -> Option<String> {
    match p.as_deref().map(|s| s.trim().to_ascii_lowercase()) {
        None => None,
        Some(s) if s == "any" || s.is_empty() => None,
        Some(s) => Some(s),
    }
}

/// Everything that matters for equality, canonicalised.
fn canon(r: &RuleSpec) -> RuleSpec {
    RuleSpec {
        id: r.id.clone(),
        scope: None,
        enabled: r.enabled,
        action: canon_action(&r.action),
        direction: r.direction.to_ascii_lowercase(),
        protocol: canon_proto(&r.protocol),
        source: r.source.clone().filter(|s| !s.is_empty()),
        dest: r.dest.clone().filter(|s| !s.is_empty()),
        sport: r.sport.clone().filter(|s| !s.is_empty()),
        dport: r.dport.clone().filter(|s| !s.is_empty()),
        comment: r.comment.clone().filter(|s| !s.is_empty()),
        log: r.log,
    }
}

pub fn valid_scope(s: &str) -> bool {
    if s == "datacenter" {
        return true;
    }
    match s.split_once(':') {
        Some(("node" | "vm" | "container", id)) => {
            !id.is_empty()
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        }
        _ => false,
    }
}

fn host_scope(s: &str) -> bool {
    s == "datacenter" || s.starts_with("node:")
}

fn check_rule(r: &RuleSpec, what: &str) -> Result<(), String> {
    if r.id.trim().is_empty() || r.id.contains('/') {
        return Err(format!("{what}: rule id {:?} is empty or has a /", r.id));
    }
    if !matches!(
        canon_action(&r.action).as_str(),
        "ACCEPT" | "DROP" | "REJECT"
    ) {
        return Err(format!(
            "{what}: rule {}: action must be ACCEPT, DROP or REJECT",
            r.id
        ));
    }
    if !matches!(r.direction.to_ascii_lowercase().as_str(), "in" | "out") {
        return Err(format!(
            "{what}: rule {}: direction must be in or out",
            r.id
        ));
    }
    if let Some(p) = canon_proto(&r.protocol) {
        if !matches!(p.as_str(), "tcp" | "udp" | "icmp" | "icmpv6") {
            return Err(format!(
                "{what}: rule {}: protocol must be tcp, udp, icmp, icmpv6 or any",
                r.id
            ));
        }
    }
    Ok(())
}

pub fn validate(f: &FirewallSpec) -> Result<(), String> {
    if let Some(o) = &f.options {
        for (k, v) in [("policy_in", &o.policy_in), ("policy_out", &o.policy_out)] {
            if v != "accept" && v != "drop" {
                return Err(format!("firewall.options.{k} must be accept or drop"));
            }
        }
    }
    let uniq = |kind: &str, names: Vec<&String>| -> Result<(), String> {
        let mut seen = BTreeSet::new();
        for n in names {
            if !seen.insert(n) {
                return Err(format!("firewall {kind} {n:?} is listed twice"));
            }
        }
        Ok(())
    };
    if let Some(a) = &f.aliases {
        uniq("alias", a.iter().map(|x| &x.name).collect())?;
        for x in a {
            if x.name.trim().is_empty() || x.cidr.trim().is_empty() {
                return Err("firewall alias: name and cidr are required".into());
            }
        }
    }
    if let Some(s) = &f.ipsets {
        uniq("ipset", s.iter().map(|x| &x.name).collect())?;
        for x in s {
            if x.name.trim().is_empty() {
                return Err("firewall ipset: name is required".into());
            }
        }
    }
    if let Some(g) = &f.security_groups {
        uniq("security group", g.iter().map(|x| &x.name).collect())?;
        for x in g {
            if x.name.trim().is_empty() {
                return Err("firewall security group: name is required".into());
            }
            uniq("rule", x.rules.iter().map(|r| &r.id).collect())
                .map_err(|e| format!("security group {}: {e}", x.name))?;
            for r in &x.rules {
                if r.scope.is_some() {
                    return Err(format!(
                        "security group {}: rule {}: `scope` is not allowed inside a group",
                        x.name, r.id
                    ));
                }
                check_rule(r, &format!("security group {}", x.name))?;
            }
        }
    }
    if let Some(rules) = &f.rules {
        let mut seen = BTreeSet::new();
        for r in rules {
            let Some(scope) = r.scope.as_deref() else {
                return Err(format!("firewall rule {}: `scope` is required", r.id));
            };
            if !valid_scope(scope) {
                return Err(format!(
                    "firewall rule {}: scope {scope:?} must be datacenter, node:<name>, vm:<id> or container:<id>",
                    r.id
                ));
            }
            if !seen.insert((scope.to_string(), r.id.clone())) {
                return Err(format!(
                    "firewall rule {} is listed twice in scope {scope}",
                    r.id
                ));
            }
            check_rule(r, "firewall")?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- lockout

fn parse_cidr(s: &str) -> Option<(IpAddr, u8)> {
    let (ip, len) = match s.split_once('/') {
        Some((a, l)) => (a.parse::<IpAddr>().ok()?, Some(l.parse::<u8>().ok()?)),
        None => (s.parse::<IpAddr>().ok()?, None),
    };
    let max = if ip.is_ipv4() { 32 } else { 128 };
    let len = len.unwrap_or(max);
    (len <= max).then_some((ip, len))
}

fn cidr_contains(net: IpAddr, len: u8, ip: IpAddr) -> bool {
    match (net, ip) {
        (IpAddr::V4(n), IpAddr::V4(i)) => {
            let mask = if len == 0 {
                0
            } else {
                u32::MAX << (32 - len as u32)
            };
            u32::from(n) & mask == u32::from(i) & mask
        }
        (IpAddr::V6(n), IpAddr::V6(i)) => {
            let mask = if len == 0 {
                0
            } else {
                u128::MAX << (128 - len as u32)
            };
            u128::from(n) & mask == u128::from(i) & mask
        }
        _ => false,
    }
}

/// Does a rule's `source` cover `ip`? `None` source = anywhere. Aliases and
/// `+ipsets` are expanded; a name that can't be resolved counts as a match
/// (the safe direction: more refusals, never fewer).
fn source_matches(
    src: &Option<String>,
    ip: Option<IpAddr>,
    aliases: &BTreeMap<String, String>,
    sets: &BTreeMap<String, Vec<String>>,
) -> bool {
    let Some(src) = src.as_deref().filter(|s| !s.is_empty()) else {
        return true;
    };
    let one = |c: &str| match (parse_cidr(c), ip) {
        (Some((n, l)), Some(i)) => cidr_contains(n, l, i),
        // can't tell where the caller is: a specific source is not "anywhere"
        (Some(_), None) => false,
        (None, _) => true,
    };
    if let Some(set) = src.strip_prefix('+') {
        return match sets.get(set) {
            Some(entries) => entries.iter().any(|e| one(e)),
            None => true,
        };
    }
    if let Some(c) = aliases.get(src) {
        return one(c);
    }
    one(src)
}

fn port_matches(dport: &Option<String>, port: u16) -> bool {
    let Some(d) = dport.as_deref().filter(|d| !d.is_empty()) else {
        return true;
    };
    if let Some(m) = d.strip_prefix("macro:") {
        return crate::firewall::objects::MACROS
            .iter()
            .find(|x| x.name.eq_ignore_ascii_case(m))
            .map(|x| {
                x.rules
                    .iter()
                    .any(|(proto, p)| *proto == "tcp" && p.parse::<u16>().ok() == Some(port))
            })
            .unwrap_or(true);
    }
    d.split(',').any(|part| {
        let part = part.trim();
        match part.split_once([':', '-']) {
            Some((a, b)) => match (a.parse::<u16>(), b.parse::<u16>()) {
                (Ok(a), Ok(b)) => (a..=b).contains(&port),
                _ => true,
            },
            None => part.parse::<u16>().map(|p| p == port).unwrap_or(true),
        }
    })
}

/// The first inbound rule (in chain order) that decides the caller's
/// connection to the API port, if the policy is `accept`. Returns the
/// offending rule id when that decision is DROP/REJECT.
pub fn api_cut_by(
    chain: &[RuleSpec],
    policy_in: &str,
    api_port: u16,
    caller: Option<IpAddr>,
    aliases: &BTreeMap<String, String>,
    sets: &BTreeMap<String, Vec<String>>,
) -> Option<String> {
    if policy_in == "drop" || api_port == 0 {
        // The manager installs `tcp dport {22, 8006, 8026} accept` first.
        return None;
    }
    for r in chain {
        let r = canon(r);
        if !r.enabled || r.direction != "in" {
            continue;
        }
        if !matches!(r.protocol.as_deref(), None | Some("tcp")) {
            continue;
        }
        if !port_matches(&r.dport, api_port) || !source_matches(&r.source, caller, aliases, sets) {
            continue;
        }
        return match r.action.as_str() {
            "ACCEPT" => None,
            _ => Some(r.id),
        };
    }
    None
}

// ---------------------------------------------------------------- plan

type Target = Vec<RuleSpec>;

/// The rule list a managed scope must end up with.
fn target_for(scope: &str, spec: &[RuleSpec], live: &[RuleSpec], prune: bool) -> Target {
    let want: Vec<RuleSpec> = spec
        .iter()
        .filter(|r| r.scope.as_deref() == Some(scope))
        .map(canon)
        .collect();
    let mut t = want;
    if !prune {
        for l in live {
            if !t.iter().any(|w| w.id == l.id) {
                t.push(canon(l));
            }
        }
    }
    t
}

fn rule_diff(a: &RuleSpec, b: &RuleSpec) -> Vec<String> {
    let mut d = Vec::new();
    macro_rules! f {
        ($($n:ident),*) => {$(
            if a.$n != b.$n { d.push(format!("{}: {:?} -> {:?}", stringify!($n), a.$n, b.$n)); }
        )*};
    }
    f!(enabled, action, direction, protocol, source, dest, sport, dport, comment, log);
    d
}

fn scope_changes(scope: &str, live: &[RuleSpec], target: &Target) -> (Vec<Change>, Vec<Change>) {
    let live: Vec<RuleSpec> = live.iter().map(canon).collect();
    let k = live
        .iter()
        .zip(target.iter())
        .take_while(|(a, b)| a == b)
        .count();
    if k == live.len() && k == target.len() {
        return (vec![], vec![]);
    }
    let name = |id: &str| format!("{scope}/{id}");
    let (tail_live, tail_target) = (&live[k..], &target[k..]);
    let mut creates = Vec::new();
    for t in tail_target {
        match tail_live.iter().find(|l| l.id == t.id) {
            None => creates.push(ch(
                "fw_rule",
                &name(&t.id),
                Action::Create,
                vec![format!(
                    "{} {} {}",
                    t.action,
                    t.protocol.as_deref().unwrap_or("any"),
                    t.dport.as_deref().unwrap_or("")
                )
                .trim()
                .to_string()],
            )),
            Some(l) => {
                let mut d = rule_diff(l, t);
                if d.is_empty() {
                    d.push("re-added to keep the rule order".into());
                }
                creates.push(ch("fw_rule", &name(&t.id), Action::Update, d));
            }
        }
    }
    let deletes = tail_live
        .iter()
        .filter(|l| !tail_target.iter().any(|t| t.id == l.id))
        .map(|l| ch("fw_rule", &name(&l.id), Action::Delete, vec![]))
        .collect();
    (creates, deletes)
}

pub fn plan(want: &FirewallSpec, live: &LiveFirewall, prune: bool, ctx: &Ctx) -> KindPlan {
    let mut p = KindPlan::default();

    // ---- options
    if let (Some(w), Some(l)) = (&want.options, &live.options) {
        if w != l {
            p.changes.push(ch(
                "fw_options",
                "host",
                Action::Update,
                vec![format!(
                    "policy_in {} -> {}, policy_out {} -> {}",
                    l.policy_in, w.policy_in, l.policy_out, w.policy_out
                )],
            ));
        }
    }

    // ---- aliases
    if let Some(aliases) = &want.aliases {
        for w in aliases {
            match live.aliases.iter().find(|l| l.name == w.name) {
                None => p.changes.push(ch(
                    "fw_alias",
                    &w.name,
                    Action::Create,
                    vec![w.cidr.clone()],
                )),
                Some(l) if l != w => p.changes.push(ch(
                    "fw_alias",
                    &w.name,
                    Action::Update,
                    vec![format!("{} -> {}", l.cidr, w.cidr)],
                )),
                Some(_) => {}
            }
        }
        if prune {
            for l in live
                .aliases
                .iter()
                .filter(|l| !aliases.iter().any(|w| w.name == l.name))
            {
                p.deletes
                    .push(ch("fw_alias", &l.name, Action::Delete, vec![]));
            }
        }
    }

    // ---- ip sets (entry order is irrelevant)
    if let Some(sets) = &want.ipsets {
        let norm = |s: &IpSetSpec| {
            (
                s.comment.clone(),
                s.entries.iter().cloned().collect::<BTreeSet<_>>(),
            )
        };
        for w in sets {
            match live.ipsets.iter().find(|l| l.name == w.name) {
                None => p.changes.push(ch(
                    "fw_ipset",
                    &w.name,
                    Action::Create,
                    vec![format!("{} entries", w.entries.len())],
                )),
                Some(l) if norm(l) != norm(w) => p.changes.push(ch(
                    "fw_ipset",
                    &w.name,
                    Action::Update,
                    vec![format!(
                        "{} -> {} entries",
                        l.entries.len(),
                        w.entries.len()
                    )],
                )),
                Some(_) => {}
            }
        }
        if prune {
            for l in live
                .ipsets
                .iter()
                .filter(|l| !sets.iter().any(|w| w.name == l.name))
            {
                p.deletes
                    .push(ch("fw_ipset", &l.name, Action::Delete, vec![]));
            }
        }
    }

    // ---- security groups: create only
    if let Some(groups) = &want.security_groups {
        let norm = |g: &SecurityGroupSpec| -> (String, Vec<RuleSpec>) {
            (g.description.clone(), g.rules.iter().map(canon).collect())
        };
        for w in groups {
            match live.security_groups.iter().find(|l| l.name == w.name) {
                None => p.changes.push(ch(
                    "fw_security_group",
                    &w.name,
                    Action::Create,
                    vec![format!("{} rules", w.rules.len())],
                )),
                Some(l) if norm(l) != norm(w) => p.changes.push(ch(
                    "fw_security_group",
                    &w.name,
                    Action::Blocked,
                    vec![
                        "security groups can't be edited yet (the firewall manager has no update call)"
                            .into(),
                    ],
                )),
                Some(_) => {}
            }
        }
        if prune {
            for l in live
                .security_groups
                .iter()
                .filter(|l| !groups.iter().any(|w| w.name == l.name))
            {
                p.deletes.push(ch(
                    "fw_security_group",
                    &l.name,
                    Action::Blocked,
                    vec![
                        "prune: security groups can't be deleted yet (the firewall manager has no delete call)"
                            .into(),
                    ],
                ));
            }
        }
    }

    // ---- rules per managed scope
    let mut final_rules: BTreeMap<String, Target> = BTreeMap::new();
    // every live scope stays as it is, except the ones the spec manages
    for (s, v) in &live.rules {
        final_rules.insert(s.clone(), v.iter().map(canon).collect());
    }
    let mut reload: BTreeSet<String> = BTreeSet::new();
    if let Some(spec_rules) = &want.rules {
        let scopes: BTreeSet<&str> = spec_rules
            .iter()
            .filter_map(|r| r.scope.as_deref())
            .collect();
        for scope in scopes {
            let l = live.rules.get(scope).cloned().unwrap_or_default();
            let target = target_for(scope, spec_rules, &l, prune);
            let (c, d) = scope_changes(scope, &l, &target);
            if (!c.is_empty() || !d.is_empty()) && host_scope(scope) {
                reload.insert(scope.to_string());
            }
            p.changes.extend(c);
            p.deletes.extend(d);
            final_rules.insert(scope.to_string(), target);
        }
        // prune also empties a scope that is no longer listed at all
        if prune {
            for (scope, l) in &live.rules {
                if !final_rules.contains_key(scope) || l.is_empty() {
                    continue;
                }
                let listed = spec_rules
                    .iter()
                    .any(|r| r.scope.as_deref() == Some(scope.as_str()));
                if !listed {
                    let (c, d) = scope_changes(scope, l, &Vec::new());
                    p.changes.extend(c);
                    p.deletes.extend(d);
                    if host_scope(scope) {
                        reload.insert(scope.clone());
                    }
                    final_rules.insert(scope.clone(), Vec::new());
                }
            }
        }
    }
    for s in &reload {
        // after every other change and delete (lowest priority in the sort below)
        p.deletes.push(ch(
            "fw_reload",
            s,
            Action::Update,
            vec!["load the scope's rules into nftables".into()],
        ));
    }

    // ---- references in rules
    let final_aliases: BTreeMap<String, String> = match &want.aliases {
        Some(a) if prune => a.iter().map(|x| (x.name.clone(), x.cidr.clone())).collect(),
        Some(a) => live
            .aliases
            .iter()
            .chain(a.iter())
            .map(|x| (x.name.clone(), x.cidr.clone()))
            .collect(),
        None => live
            .aliases
            .iter()
            .map(|x| (x.name.clone(), x.cidr.clone()))
            .collect(),
    };
    let final_sets: BTreeMap<String, Vec<String>> = match &want.ipsets {
        Some(s) if prune => s
            .iter()
            .map(|x| (x.name.clone(), x.entries.clone()))
            .collect(),
        Some(s) => live
            .ipsets
            .iter()
            .chain(s.iter())
            .map(|x| (x.name.clone(), x.entries.clone()))
            .collect(),
        None => live
            .ipsets
            .iter()
            .map(|x| (x.name.clone(), x.entries.clone()))
            .collect(),
    };
    let groups_known: BTreeSet<&str> = live
        .security_groups
        .iter()
        .map(|g| g.name.as_str())
        .chain(
            want.security_groups
                .iter()
                .flatten()
                .map(|g| g.name.as_str()),
        )
        .collect();
    let check_ref = |what: &str, v: &Option<String>, errs: &mut Vec<String>| {
        let Some(v) = v.as_deref().filter(|v| !v.is_empty()) else {
            return;
        };
        if let Some(set) = v.strip_prefix('+') {
            if !final_sets.contains_key(set) {
                errs.push(format!("{what}: ip set {set} does not exist"));
            }
        } else if parse_cidr(v).is_none()
            && !final_aliases.contains_key(v)
            && !groups_known.contains(v)
        {
            errs.push(format!(
                "{what}: {v:?} is not an address, alias or security group"
            ));
        }
    };
    for r in want.rules.iter().flatten() {
        let what = format!(
            "firewall rule {}/{}",
            r.scope.as_deref().unwrap_or("?"),
            r.id
        );
        check_ref(&what, &r.source, &mut p.errors);
        check_ref(&what, &r.dest, &mut p.errors);
    }

    // ---- anti-lockout (only what this plan newly introduces)
    if !want.allow_lockout && ctx.api_port != 0 {
        let policy_of = |o: &Option<OptionsSpec>| {
            o.as_ref()
                .map(|o| o.policy_in.clone())
                .unwrap_or_else(|| "accept".into())
        };
        let live_policy = policy_of(&live.options);
        let final_policy = want
            .options
            .as_ref()
            .map(|o| o.policy_in.clone())
            .unwrap_or_else(|| live_policy.clone());
        let live_alias: BTreeMap<String, String> = live
            .aliases
            .iter()
            .map(|x| (x.name.clone(), x.cidr.clone()))
            .collect();
        let live_sets: BTreeMap<String, Vec<String>> = live
            .ipsets
            .iter()
            .map(|x| (x.name.clone(), x.entries.clone()))
            .collect();
        for scope in final_rules.keys().filter(|s| host_scope(s)) {
            let new = api_cut_by(
                &final_rules[scope],
                &final_policy,
                ctx.api_port,
                ctx.caller_ip,
                &final_aliases,
                &final_sets,
            );
            let old = api_cut_by(
                &live.rules.get(scope).cloned().unwrap_or_default(),
                &live_policy,
                ctx.api_port,
                ctx.caller_ip,
                &live_alias,
                &live_sets,
            );
            if let (Some(id), None) = (new, old) {
                p.errors.push(format!(
                    "firewall: rule {scope}/{id} would drop tcp/{} (the API) for {} and cut this \
                     session off; refusing. Reorder it below an accept rule, or set \
                     `allow_lockout: true` in the firewall section to do it anyway",
                    ctx.api_port,
                    ctx.caller_ip
                        .map(|i| i.to_string())
                        .unwrap_or_else(|| "every client".into())
                ));
            }
        }
    }
    // Within this kind the engine keeps our order: objects before the rules
    // that use them; on delete, rules before the objects they use; the nft
    // reload last of all.
    let up = |k: &str| match k {
        "fw_alias" => 0,
        "fw_ipset" => 1,
        "fw_security_group" => 2,
        "fw_rule" => 3,
        _ => 4,
    };
    let down = |k: &str| match k {
        "fw_rule" => 0,
        "fw_security_group" => 1,
        "fw_ipset" => 2,
        "fw_alias" => 3,
        "fw_options" => 4,
        _ => 9,
    };
    p.changes.sort_by_key(|c| up(&c.kind));
    p.deletes.sort_by_key(|c| down(&c.kind));
    p
}

// ---------------------------------------------------------------- export

pub fn export(live: &LiveFirewall) -> FirewallSpec {
    let mut aliases = live.aliases.clone();
    aliases.sort_by(|a, b| a.name.cmp(&b.name));
    let mut ipsets: Vec<IpSetSpec> = live
        .ipsets
        .iter()
        .map(|s| {
            let mut s = s.clone();
            s.entries.sort();
            s
        })
        .collect();
    ipsets.sort_by(|a, b| a.name.cmp(&b.name));
    let mut groups: Vec<SecurityGroupSpec> = live
        .security_groups
        .iter()
        .map(|g| SecurityGroupSpec {
            name: g.name.clone(),
            description: g.description.clone(),
            rules: g.rules.iter().map(canon).collect(),
        })
        .collect();
    groups.sort_by(|a, b| a.name.cmp(&b.name));
    let mut rules = Vec::new();
    for (scope, v) in &live.rules {
        for r in v {
            let mut r = canon(r);
            r.scope = Some(scope.clone());
            rules.push(r);
        }
    }
    FirewallSpec {
        options: live.options.clone(),
        aliases: Some(aliases),
        ipsets: Some(ipsets),
        security_groups: Some(groups),
        rules: Some(rules),
        allow_lockout: false,
    }
}

// ------------------------------------------------------------------ kind

pub struct Firewall;

pub fn rule_to_spec(r: &crate::firewall::FirewallRule) -> RuleSpec {
    use crate::firewall::{Direction, FirewallAction, Protocol};
    RuleSpec {
        id: r.id.clone(),
        scope: None,
        enabled: r.enabled,
        action: match r.action {
            FirewallAction::Accept => "ACCEPT",
            FirewallAction::Reject => "REJECT",
            FirewallAction::Drop => "DROP",
        }
        .into(),
        direction: match r.direction {
            Direction::In => "in",
            Direction::Out => "out",
        }
        .into(),
        protocol: r.protocol.as_ref().and_then(|p| match p {
            Protocol::Tcp => Some("tcp".to_string()),
            Protocol::Udp => Some("udp".to_string()),
            Protocol::Icmp => Some("icmp".to_string()),
            Protocol::Icmpv6 => Some("icmpv6".to_string()),
            Protocol::Any => None,
        }),
        source: r.source.clone(),
        dest: r.dest.clone(),
        sport: r.sport.clone(),
        dport: r.dport.clone(),
        comment: r.comment.clone(),
        log: r.log,
    }
}

pub fn spec_to_rule(r: &RuleSpec, position: u32) -> crate::firewall::FirewallRule {
    use crate::firewall::{Direction, FirewallAction, Protocol};
    crate::firewall::FirewallRule {
        id: r.id.clone(),
        enabled: r.enabled,
        action: match canon_action(&r.action).as_str() {
            "REJECT" => FirewallAction::Reject,
            "DROP" => FirewallAction::Drop,
            _ => FirewallAction::Accept,
        },
        direction: if r.direction.eq_ignore_ascii_case("out") {
            Direction::Out
        } else {
            Direction::In
        },
        protocol: canon_proto(&r.protocol).map(|p| match p.as_str() {
            "udp" => Protocol::Udp,
            "icmp" => Protocol::Icmp,
            "icmpv6" => Protocol::Icmpv6,
            _ => Protocol::Tcp,
        }),
        source: r.source.clone().filter(|s| !s.is_empty()),
        dest: r.dest.clone().filter(|s| !s.is_empty()),
        sport: r.sport.clone().filter(|s| !s.is_empty()),
        dport: r.dport.clone().filter(|s| !s.is_empty()),
        comment: r.comment.clone().filter(|s| !s.is_empty()),
        log: r.log,
        position,
    }
}

async fn gather_live(state: &Arc<AppState>) -> LiveFirewall {
    use crate::firewall::FirewallScope;
    let fm = &state.firewall_manager;
    let o = fm.options();
    let mut rules = BTreeMap::new();
    let mut scopes = vec![FirewallScope::Datacenter];
    scopes.extend(
        fm.nodes_with_rules()
            .await
            .into_iter()
            .map(FirewallScope::Node),
    );
    scopes.extend(
        state
            .vm_manager
            .list_vms()
            .await
            .into_iter()
            .map(|v| FirewallScope::Vm(v.id)),
    );
    scopes.extend(
        fm.containers_with_rules()
            .await
            .into_iter()
            .map(FirewallScope::Container),
    );
    for s in scopes {
        let v = fm.list_rules(s.clone()).await;
        if !v.is_empty() {
            rules.insert(s.key(), v.iter().map(rule_to_spec).collect());
        }
    }
    LiveFirewall {
        options: Some(OptionsSpec {
            policy_in: o.policy_in,
            policy_out: o.policy_out,
        }),
        aliases: fm
            .list_aliases()
            .into_iter()
            .map(|a| AliasSpec {
                name: a.name,
                cidr: a.cidr,
                comment: a.comment,
            })
            .collect(),
        ipsets: fm
            .list_ipsets()
            .into_iter()
            .map(|s| IpSetSpec {
                name: s.name,
                comment: s.comment,
                entries: s.entries,
            })
            .collect(),
        security_groups: fm
            .list_security_groups()
            .await
            .into_iter()
            .map(|g| SecurityGroupSpec {
                name: g.name,
                description: g.description,
                rules: g.rules.iter().map(rule_to_spec).collect(),
            })
            .collect(),
        rules,
    }
}

#[async_trait]
impl Kind for Firewall {
    fn key(&self) -> &'static str {
        "firewall"
    }
    fn change_kinds(&self) -> &'static [&'static str] {
        &[
            "fw_alias",
            "fw_ipset",
            "fw_security_group",
            "fw_rule",
            "fw_options",
            "fw_reload",
        ]
    }
    fn rank(&self) -> u32 {
        73
    }
    fn validate(&self, section: &Value) -> Result<(), String> {
        let f: FirewallSpec = serde_json::from_value(section.clone()).map_err(|e| e.to_string())?;
        validate(&f)
    }
    async fn gather(&self, state: &Arc<AppState>) -> Result<Value, String> {
        to_json(&export(&gather_live(state).await))
    }
    fn plan(
        &self,
        section: &Value,
        live: &Value,
        prune: bool,
        whole: (&Spec, &Live),
        out: &mut KindPlan,
    ) {
        let parse = |v: &Value| serde_json::from_value::<FirewallSpec>(v.clone());
        match (
            parse(section),
            if live.is_null() {
                Ok(FirewallSpec::default())
            } else {
                parse(live)
            },
        ) {
            (Ok(w), Ok(l)) => out.merge(plan(
                &w,
                &LiveFirewall::from_spec(&l),
                prune,
                &whole.1.kinds,
            )),
            (Err(e), _) | (_, Err(e)) => out.errors.push(format!("firewall: {e}")),
        }
    }
    async fn apply(
        &self,
        state: &Arc<AppState>,
        section: &Value,
        _live: &Value,
        c: &Change,
    ) -> Result<Option<String>, String> {
        apply(state, section, c).await
    }
}

async fn apply(
    state: &Arc<AppState>,
    section: &Value,
    c: &Change,
) -> Result<Option<String>, String> {
    use crate::firewall::{objects, FirewallScope, SecurityGroup};
    let fm = &state.firewall_manager;
    let e = |x: horcrux_common::Error| format!("{x}");
    let f: FirewallSpec = serde_json::from_value(section.clone()).map_err(|e| e.to_string())?;
    match (c.kind.as_str(), c.action) {
        ("fw_options", Action::Update) => {
            let o = f.options.as_ref().ok_or("not in spec")?;
            fm.set_options(objects::FwOptions {
                policy_in: o.policy_in.clone(),
                policy_out: o.policy_out.clone(),
            })
            .await
            .map_err(e)?;
            Ok(None)
        }
        ("fw_alias", Action::Create | Action::Update) => {
            let a = f
                .aliases
                .iter()
                .flatten()
                .find(|a| a.name == c.name)
                .ok_or("not in spec")?;
            fm.put_alias(objects::Alias {
                name: a.name.clone(),
                cidr: a.cidr.clone(),
                comment: a.comment.clone(),
            })
            .await
            .map_err(e)?;
            Ok(None)
        }
        ("fw_alias", Action::Delete) => {
            fm.delete_alias(&c.name).await.map_err(e)?;
            Ok(None)
        }
        ("fw_ipset", Action::Create | Action::Update) => {
            let s = f
                .ipsets
                .iter()
                .flatten()
                .find(|s| s.name == c.name)
                .ok_or("not in spec")?;
            fm.put_ipset(objects::IpSet {
                name: s.name.clone(),
                comment: s.comment.clone(),
                entries: s.entries.clone(),
            })
            .await
            .map_err(e)?;
            Ok(None)
        }
        ("fw_ipset", Action::Delete) => {
            fm.delete_ipset(&c.name).await.map_err(e)?;
            Ok(None)
        }
        ("fw_security_group", Action::Create) => {
            let g = f
                .security_groups
                .iter()
                .flatten()
                .find(|g| g.name == c.name)
                .ok_or("not in spec")?;
            fm.create_security_group(SecurityGroup {
                name: g.name.clone(),
                description: g.description.clone(),
                rules: g
                    .rules
                    .iter()
                    .enumerate()
                    .map(|(i, r)| spec_to_rule(r, i as u32 + 1))
                    .collect(),
            })
            .await
            .map_err(e)?;
            Ok(None)
        }
        ("fw_rule", act) => {
            let (scope_key, id) = c
                .name
                .rsplit_once('/')
                .ok_or_else(|| format!("bad rule change name {:?}", c.name))?;
            let scope = FirewallScope::from_key(scope_key)
                .ok_or_else(|| format!("bad firewall scope {scope_key}"))?;
            let find = || {
                f.rules
                    .iter()
                    .flatten()
                    .enumerate()
                    .find(|(_, r)| r.scope.as_deref() == Some(scope_key) && r.id == id)
                    .map(|(i, r)| spec_to_rule(r, i as u32 + 1))
            };
            match act {
                Action::Create => {
                    fm.add_rule(scope, find().ok_or("not in spec")?)
                        .await
                        .map_err(e)?;
                }
                Action::Update => {
                    // no edit call: remove and add (the plan re-adds the rules after it)
                    let rule = find();
                    fm.remove_rule(scope.clone(), id).await.map_err(e)?;
                    fm.add_rule(scope, rule.ok_or("not in spec")?)
                        .await
                        .map_err(e)?;
                }
                Action::Delete => fm.remove_rule(scope, id).await.map_err(e)?,
                _ => return Err(format!("no executor for fw_rule {act:?}")),
            }
            Ok(None)
        }
        ("fw_reload", Action::Update) => {
            let scope = FirewallScope::from_key(&c.name)
                .ok_or_else(|| format!("bad firewall scope {}", c.name))?;
            fm.apply_rules(scope).await.map_err(e)?;
            Ok(None)
        }
        (k, a) => Err(format!("no executor for {k} {a:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(scope: &str, id: &str, action: &str) -> RuleSpec {
        RuleSpec {
            id: id.into(),
            scope: Some(scope.into()),
            enabled: true,
            action: action.into(),
            direction: "in".into(),
            protocol: Some("tcp".into()),
            dport: Some("22".into()),
            ..Default::default()
        }
    }
    fn live_rule(id: &str, action: &str) -> RuleSpec {
        RuleSpec {
            scope: None,
            ..rule("x", id, action)
        }
    }
    fn ctx() -> Ctx {
        Ctx {
            api_port: 8006,
            caller_ip: Some("192.0.2.50".parse().unwrap()),
            ..Default::default()
        }
    }
    fn fw(rules: Vec<RuleSpec>) -> FirewallSpec {
        FirewallSpec {
            rules: Some(rules),
            ..Default::default()
        }
    }
    fn lf(scope: &str, rules: Vec<RuleSpec>) -> LiveFirewall {
        LiveFirewall {
            options: Some(OptionsSpec {
                policy_in: "accept".into(),
                policy_out: "accept".into(),
            }),
            rules: BTreeMap::from([(scope.to_string(), rules)]),
            ..Default::default()
        }
    }
    fn acts(p: &KindPlan) -> Vec<String> {
        p.changes
            .iter()
            .chain(&p.deletes)
            .map(|c| format!("{:?} {} {}", c.action, c.kind, c.name))
            .collect()
    }

    #[test]
    fn appended_rules_are_plain_creates_followed_by_a_host_reload() {
        let live = lf("datacenter", vec![live_rule("a", "ACCEPT")]);
        let want = fw(vec![
            rule("datacenter", "a", "ACCEPT"),
            rule("datacenter", "b", "ACCEPT"),
        ]);
        let p = plan(&want, &live, false, &ctx());
        assert_eq!(
            acts(&p),
            ["Create fw_rule datacenter/b", "Update fw_reload datacenter"]
        );
    }

    #[test]
    fn guest_scopes_need_no_reload() {
        let want = fw(vec![rule("vm:web", "a", "ACCEPT")]);
        let p = plan(&want, &LiveFirewall::default(), false, &ctx());
        assert_eq!(acts(&p), ["Create fw_rule vm:web/a"]);
    }

    #[test]
    fn a_changed_rule_and_everything_after_it_is_re_added_in_order() {
        let live = lf(
            "datacenter",
            vec![
                live_rule("a", "ACCEPT"),
                live_rule("b", "ACCEPT"),
                live_rule("c", "ACCEPT"),
            ],
        );
        let mut b = rule("datacenter", "b", "ACCEPT");
        b.dport = Some("2222".into());
        let want = fw(vec![
            rule("datacenter", "a", "ACCEPT"),
            b,
            rule("datacenter", "c", "ACCEPT"),
        ]);
        let p = plan(&want, &live, false, &ctx());
        assert_eq!(
            acts(&p),
            [
                "Update fw_rule datacenter/b",
                "Update fw_rule datacenter/c",
                "Update fw_reload datacenter"
            ]
        );
        assert!(p.changes[0].details[0].starts_with("dport"));
        assert!(p.changes[1].details[0].contains("order"));
    }

    #[test]
    fn reordering_rewrites_from_the_first_difference() {
        let live = lf(
            "datacenter",
            vec![live_rule("a", "ACCEPT"), live_rule("b", "ACCEPT")],
        );
        let want = fw(vec![
            rule("datacenter", "b", "ACCEPT"),
            rule("datacenter", "a", "ACCEPT"),
        ]);
        let p = plan(&want, &live, false, &ctx());
        assert_eq!(
            acts(&p),
            [
                "Update fw_rule datacenter/b",
                "Update fw_rule datacenter/a",
                "Update fw_reload datacenter"
            ]
        );
    }

    #[test]
    fn unlisted_live_rules_stay_at_the_end_without_prune_and_go_with_it() {
        let live = lf(
            "datacenter",
            vec![live_rule("keep", "ACCEPT"), live_rule("extra", "ACCEPT")],
        );
        let want = fw(vec![rule("datacenter", "keep", "ACCEPT")]);
        assert!(acts(&plan(&want, &live, false, &ctx())).is_empty());
        let p = plan(&want, &live, true, &ctx());
        assert_eq!(
            acts(&p),
            [
                "Delete fw_rule datacenter/extra",
                "Update fw_reload datacenter"
            ]
        );
    }

    #[test]
    fn prune_empties_a_scope_that_is_no_longer_listed() {
        let mut live = lf("datacenter", vec![live_rule("a", "ACCEPT")]);
        live.rules
            .insert("vm:old".into(), vec![live_rule("x", "DROP")]);
        let want = fw(vec![rule("datacenter", "a", "ACCEPT")]);
        let p = plan(&want, &live, true, &ctx());
        assert_eq!(acts(&p), ["Delete fw_rule vm:old/x"]);
        assert!(acts(&plan(&want, &live, false, &ctx())).is_empty());
    }

    #[test]
    fn options_aliases_ipsets_are_diffed_and_pruned() {
        let mut live = lf("datacenter", vec![]);
        live.aliases = vec![
            AliasSpec {
                name: "office".into(),
                cidr: "192.0.2.0/24".into(),
                comment: "".into(),
            },
            AliasSpec {
                name: "old".into(),
                cidr: "10.0.0.1".into(),
                comment: "".into(),
            },
        ];
        live.ipsets = vec![IpSetSpec {
            name: "adm".into(),
            comment: "".into(),
            entries: vec!["192.0.2.1".into(), "192.0.2.2".into()],
        }];
        let want = FirewallSpec {
            options: Some(OptionsSpec {
                policy_in: "drop".into(),
                policy_out: "accept".into(),
            }),
            aliases: Some(vec![AliasSpec {
                name: "office".into(),
                cidr: "192.0.2.0/25".into(),
                comment: "".into(),
            }]),
            ipsets: Some(vec![IpSetSpec {
                name: "adm".into(),
                comment: "".into(),
                entries: vec!["192.0.2.2".into(), "192.0.2.1".into()], // same set, other order
            }]),
            ..Default::default()
        };
        let p = plan(&want, &live, true, &ctx());
        assert_eq!(
            acts(&p),
            [
                "Update fw_alias office",
                "Update fw_options host",
                "Delete fw_alias old"
            ]
        );
    }

    #[test]
    fn security_groups_are_create_only_and_say_so() {
        let g = |rules: Vec<RuleSpec>| SecurityGroupSpec {
            name: "web".into(),
            description: String::new(),
            rules,
        };
        let mut live = LiveFirewall::default();
        live.security_groups = vec![g(vec![live_rule("https", "ACCEPT")])];
        let want = FirewallSpec {
            security_groups: Some(vec![
                g(vec![live_rule("https", "DROP")]),
                SecurityGroupSpec {
                    name: "new".into(),
                    description: String::new(),
                    rules: vec![],
                },
            ]),
            ..Default::default()
        };
        let p = plan(&want, &live, false, &ctx());
        assert_eq!(
            acts(&p),
            [
                "Blocked fw_security_group web",
                "Create fw_security_group new"
            ]
        );
        let p = plan(
            &FirewallSpec {
                security_groups: Some(vec![]),
                ..Default::default()
            },
            &live,
            true,
            &ctx(),
        );
        assert_eq!(acts(&p), ["Blocked fw_security_group web"]);
    }

    // ---- anti-lockout

    #[test]
    fn dropping_the_api_port_for_everyone_is_refused() {
        let mut r = rule("datacenter", "no-api", "DROP");
        r.dport = Some("8006".into());
        let p = plan(&fw(vec![r]), &lf("datacenter", vec![]), false, &ctx());
        assert!(
            p.errors.iter().any(|e| e.contains("cut this session off")),
            "{:?}",
            p.errors
        );
    }

    #[test]
    fn dropping_a_range_a_macro_or_any_port_that_covers_the_api_is_refused() {
        for dport in [Some("8000:9000"), Some("22,8006"), None] {
            let mut r = rule("datacenter", "x", "REJECT");
            r.dport = dport.map(Into::into);
            let p = plan(&fw(vec![r]), &lf("datacenter", vec![]), false, &ctx());
            assert!(!p.errors.is_empty(), "{dport:?}");
        }
        let mut r = rule("datacenter", "x", "DROP");
        r.dport = Some("macro:HTTPS".into()); // 443 only
        assert!(plan(&fw(vec![r]), &lf("datacenter", vec![]), false, &ctx())
            .errors
            .is_empty());
    }

    #[test]
    fn dropping_the_callers_network_is_refused_but_another_network_is_fine() {
        let mut r = rule("datacenter", "x", "DROP");
        r.dport = Some("8006".into());
        r.source = Some("192.0.2.0/24".into());
        assert!(!plan(
            &fw(vec![r.clone()]),
            &lf("datacenter", vec![]),
            false,
            &ctx()
        )
        .errors
        .is_empty());
        r.source = Some("198.51.100.0/24".into());
        assert!(plan(&fw(vec![r]), &lf("datacenter", vec![]), false, &ctx())
            .errors
            .is_empty());
    }

    #[test]
    fn an_earlier_accept_for_the_caller_makes_a_later_drop_safe() {
        let mut ok = rule("datacenter", "allow", "ACCEPT");
        ok.dport = Some("8006".into());
        ok.source = Some("office".into());
        let mut bad = rule("datacenter", "deny", "DROP");
        bad.dport = Some("8006".into());
        let mut live = lf("datacenter", vec![]);
        live.aliases = vec![AliasSpec {
            name: "office".into(),
            cidr: "192.0.2.0/24".into(),
            comment: "".into(),
        }];
        let p = plan(&fw(vec![ok.clone(), bad.clone()]), &live, false, &ctx());
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        // other order: the drop wins
        let p = plan(&fw(vec![bad, ok]), &live, false, &ctx());
        assert!(!p.errors.is_empty());
    }

    #[test]
    fn ipset_sources_are_expanded_and_unknown_sets_are_errors() {
        let mut bad = rule("datacenter", "deny", "DROP");
        bad.dport = Some("8006".into());
        bad.source = Some("+admins".into());
        let mut live = lf("datacenter", vec![]);
        live.ipsets = vec![IpSetSpec {
            name: "admins".into(),
            comment: "".into(),
            entries: vec!["192.0.2.50".into()],
        }];
        assert!(!plan(&fw(vec![bad.clone()]), &live, false, &ctx())
            .errors
            .is_empty());
        bad.source = Some("+ghost".into());
        let p = plan(&fw(vec![bad]), &live, false, &ctx());
        assert!(p.errors.iter().any(|e| e.contains("ghost")));
    }

    #[test]
    fn policy_drop_never_cuts_the_api_port_because_the_manager_keeps_it_open() {
        let mut r = rule("datacenter", "x", "DROP");
        r.dport = Some("8006".into());
        let mut live = lf("datacenter", vec![]);
        live.options.as_mut().unwrap().policy_in = "drop".into();
        assert!(plan(&fw(vec![r]), &live, false, &ctx()).errors.is_empty());
    }

    #[test]
    fn switching_the_policy_back_to_accept_exposes_a_dropping_rule() {
        let mut r = live_rule("x", "DROP");
        r.dport = Some("8006".into());
        let mut live = lf("datacenter", vec![r.clone()]);
        live.options.as_mut().unwrap().policy_in = "drop".into();
        let want = FirewallSpec {
            options: Some(OptionsSpec {
                policy_in: "accept".into(),
                policy_out: "accept".into(),
            }),
            ..Default::default()
        };
        let p = plan(&want, &live, false, &ctx());
        assert!(!p.errors.is_empty());
    }

    #[test]
    fn an_existing_dropping_rule_is_not_this_plans_fault_and_allow_lockout_overrides() {
        let mut r = live_rule("x", "DROP");
        r.dport = Some("8006".into());
        r.source = Some("198.51.100.1".into());
        let live = lf("datacenter", vec![r]);
        assert!(plan(&fw(vec![]), &live, false, &ctx()).errors.is_empty());
        let mut bad = rule("datacenter", "deny", "DROP");
        bad.dport = Some("8006".into());
        let mut want = fw(vec![bad]);
        want.allow_lockout = true;
        assert!(plan(&want, &lf("datacenter", vec![]), false, &ctx())
            .errors
            .is_empty());
    }

    #[test]
    fn outbound_and_other_protocol_rules_never_count() {
        let mut out = rule("datacenter", "o", "DROP");
        out.direction = "out".into();
        out.dport = Some("8006".into());
        let mut udp = rule("datacenter", "u", "DROP");
        udp.protocol = Some("udp".into());
        udp.dport = Some("8006".into());
        let mut off = rule("datacenter", "d", "DROP");
        off.enabled = false;
        off.dport = Some("8006".into());
        assert!(plan(
            &fw(vec![out, udp, off]),
            &lf("datacenter", vec![]),
            false,
            &ctx()
        )
        .errors
        .is_empty());
    }

    #[test]
    fn unknown_caller_address_only_trips_on_rules_for_everyone() {
        let mut c = ctx();
        c.caller_ip = None;
        let mut r = rule("datacenter", "x", "DROP");
        r.dport = Some("8006".into());
        r.source = Some("192.0.2.0/24".into());
        assert!(
            plan(&fw(vec![r.clone()]), &lf("datacenter", vec![]), false, &c)
                .errors
                .is_empty()
        );
        r.source = None;
        assert!(!plan(&fw(vec![r]), &lf("datacenter", vec![]), false, &c)
            .errors
            .is_empty());
    }

    #[test]
    fn unresolvable_references_are_errors() {
        let mut r = rule("datacenter", "x", "ACCEPT");
        r.source = Some("nowhere".into());
        let p = plan(&fw(vec![r]), &lf("datacenter", vec![]), false, &ctx());
        assert!(p.errors.iter().any(|e| e.contains("nowhere")));
    }

    #[test]
    fn export_is_a_fixed_point_even_with_prune() {
        let mut live = lf(
            "datacenter",
            vec![live_rule("a", "ACCEPT"), live_rule("b", "DROP")],
        );
        live.rules
            .insert("vm:web".into(), vec![live_rule("c", "ACCEPT")]);
        live.aliases = vec![AliasSpec {
            name: "o".into(),
            cidr: "192.0.2.0/24".into(),
            comment: "".into(),
        }];
        live.ipsets = vec![IpSetSpec {
            name: "s".into(),
            comment: "".into(),
            entries: vec!["192.0.2.1".into()],
        }];
        live.security_groups = vec![SecurityGroupSpec {
            name: "g".into(),
            description: "d".into(),
            rules: vec![live_rule("h", "ACCEPT")],
        }];
        let spec = export(&live);
        validate(&spec).unwrap();
        let yaml = serde_yaml::to_string(&spec).unwrap();
        let back: FirewallSpec = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back, spec);
        let mut c = ctx();
        c.caller_ip = None;
        let p = plan(&spec, &live, true, &c);
        // the exported datacenter chain has `b: DROP tcp/22`, never the API
        assert!(
            acts(&p).is_empty() && p.errors.is_empty(),
            "{:?} {:?}",
            acts(&p),
            p.errors
        );
    }

    #[test]
    fn validation_catches_bad_scopes_actions_and_duplicates() {
        let mut r = rule("galaxy", "a", "ACCEPT");
        assert!(validate(&fw(vec![r.clone()]))
            .unwrap_err()
            .contains("scope"));
        r.scope = None;
        assert!(validate(&fw(vec![r.clone()]))
            .unwrap_err()
            .contains("required"));
        let r = rule("datacenter", "a", "ALLOW");
        assert!(validate(&fw(vec![r])).unwrap_err().contains("action"));
        let a = rule("datacenter", "a", "ACCEPT");
        assert!(validate(&fw(vec![a.clone(), a.clone()]))
            .unwrap_err()
            .contains("twice"));
        let mut g = FirewallSpec::default();
        g.security_groups = Some(vec![SecurityGroupSpec {
            name: "g".into(),
            description: "".into(),
            rules: vec![a],
        }]);
        assert!(validate(&g).unwrap_err().contains("not allowed"));
        let o = FirewallSpec {
            options: Some(OptionsSpec {
                policy_in: "maybe".into(),
                policy_out: "accept".into(),
            }),
            ..Default::default()
        };
        assert!(validate(&o).is_err());
    }

    #[test]
    fn cidr_matching_covers_v4_v6_and_edges() {
        let ip: IpAddr = "192.0.2.50".parse().unwrap();
        assert!(cidr_contains("192.0.2.0".parse().unwrap(), 24, ip));
        assert!(!cidr_contains("192.0.3.0".parse().unwrap(), 24, ip));
        assert!(cidr_contains("0.0.0.0".parse().unwrap(), 0, ip));
        let v6: IpAddr = "2001:db8::5".parse().unwrap();
        assert!(cidr_contains("2001:db8::".parse().unwrap(), 32, v6));
        assert!(!cidr_contains("2001:db9::".parse().unwrap(), 32, v6));
        assert!(!cidr_contains("2001:db8::".parse().unwrap(), 32, ip));
    }
}
